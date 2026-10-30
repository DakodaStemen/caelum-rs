//! PTY ownership, terminal emulation and input translation. The UI never blocks on PTY I/O.
use crate::{
    model::Preferences,
    osc::{OscEvent, OscScanner},
    theme::Theme,
};
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{Config, Term, TermMode},
    vte::ansi::{self, Color, NamedColor},
};
#[cfg_attr(not(unix), allow(unused_imports))]
use anyhow::{Context, Result, bail};
use eframe::egui::{self, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, Vec2};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
/// Terminal events forwarded from the parser thread to the UI thread.
///
/// The queue is bounded so a session whose pane is not being drawn (and therefore
/// not polled) cannot grow memory without limit. Title changes are coalesced into
/// a single latest-value slot, so a program spamming OSC 0/2 costs O(1) memory and
/// the UI only ever sees the newest title. Only events the UI acts on are queued;
/// if the queue is full, further replies are dropped rather than blocking the
/// parser (alacritty's `EventListener::send_event` must not block).
#[derive(Clone)]
pub struct Events {
    tx: SyncSender<Event>,
    title: Arc<Mutex<Option<String>>>,
}
/// UI-side end of [`Events`].
struct EventQueue {
    rx: Receiver<Event>,
    title: Arc<Mutex<Option<String>>>,
}
const EVENT_QUEUE: usize = 1024;
impl Events {
    fn new() -> (Self, EventQueue) {
        let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE);
        let title = Arc::new(Mutex::new(None));
        (
            Self {
                tx,
                title: title.clone(),
            },
            EventQueue { rx, title },
        )
    }
}
impl EventListener for Events {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => *lock(&self.title) = Some(sanitize_title(&title)),
            Event::ResetTitle => *lock(&self.title) = Some(DEFAULT_TITLE.into()),
            Event::PtyWrite(_) | Event::ColorRequest(..) | Event::TextAreaSizeRequest(_) => {
                let _ = self.tx.try_send(event);
            }
            // Everything else (wakeups, bells, clipboard requests, ...) is ignored by the UI.
            _ => {}
        }
    }
}
const DEFAULT_TITLE: &str = "Terminal";
const MAX_TITLE_CHARS: usize = 100;
/// Bidirectional formatting characters that can visually reorder surrounding text.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}
/// Makes a program-supplied window title safe to show in tabs and window chrome:
/// drops C0/C1 controls and DEL plus bidi overrides/isolates, trims, and caps length.
fn sanitize_title(raw: &str) -> String {
    let clean: String = raw
        .chars()
        .filter(|&c| !c.is_control() && !is_bidi_control(c))
        .collect();
    let clean: String = clean.trim().chars().take(MAX_TITLE_CHARS).collect();
    let clean = clean.trim_end();
    if clean.is_empty() {
        DEFAULT_TITLE.into()
    } else {
        clean.into()
    }
}
/// Removes bytes a paste must never deliver to the PTY: every C0 control except
/// tab, newline and carriage return (this includes ESC, so a payload cannot end
/// bracketed paste early or inject sequences), DEL, and C1 controls.
fn sanitize_paste(text: &str) -> String {
    text.chars()
        .filter(|&c| matches!(c, '\t' | '\n' | '\r') || !c.is_control())
        .collect()
}
#[derive(Clone, Copy)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}
/// Shell-reported state from OSC 7 / OSC 133, shared with the PTY reader thread.
#[derive(Default, Debug)]
pub struct ShellState {
    /// Last OSC 7 working directory for this host.
    pub cwd: Option<PathBuf>,
    /// Prompt starts as absolute line indexes (0 = oldest scrollback line).
    marks: Vec<i64>,
    /// Exit status of the most recent finished command, if the shell reported one.
    pub last_exit: Option<i32>,
    /// True between OSC 133;C and OSC 133;D.
    pub running: bool,
    last_history: usize,
}
impl ShellState {
    fn clear_marks(&mut self) {
        self.marks.clear();
    }
}
fn apply_osc(term: &Term<Events>, shell: &mut ShellState, event: OscEvent) {
    match event {
        OscEvent::Cwd(path) => shell.cwd = Some(path),
        OscEvent::PromptStart => {
            // Marks are only meaningful on the primary screen.
            if term.mode().contains(TermMode::ALT_SCREEN) {
                return;
            }
            let abs =
                term.grid().history_size() as i64 + i64::from(term.grid().cursor.point.line.0);
            if shell.marks.last() != Some(&abs) {
                shell.marks.push(abs);
            }
        }
        OscEvent::OutputStart => shell.running = true,
        OscEvent::CommandEnd(code) => {
            shell.running = false;
            shell.last_exit = code;
        }
    }
}
/// Absolute indexes drift once Alacritty starts discarding old scrollback, so marks
/// are dropped when history is truncated or full instead of pointing at wrong lines.
fn reconcile_marks(term: &Term<Events>, shell: &mut ShellState) {
    let history = term.grid().history_size();
    if history < shell.last_history || history >= Config::default().scrolling_history {
        shell.clear_marks();
    }
    shell.last_history = history;
}
pub struct Session {
    pub term: Arc<Mutex<Term<Events>>>,
    pub title: String,
    pub cwd: PathBuf,
    pub exited: bool,
    pub error: Option<String>,
    pub shell: Arc<Mutex<ShellState>>,
    // `Option` only so `Drop` can hand them to the cleanup thread; always `Some` while alive.
    master: Option<Box<dyn MasterPty + Send>>,
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    input: Option<SyncSender<Vec<u8>>>,
    events: EventQueue,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    size: Size,
    search_origin: Option<(String, Point)>,
    pending_input: Vec<u8>,
    /// Multi-line paste held for user confirmation (already sanitised).
    pending_paste: Option<String>,
    wake: egui::Context,
}
impl Session {
    pub fn spawn(cwd: &Path, argv: Option<&[String]>, ctx: egui::Context) -> Result<Self> {
        let cwd = if cwd.is_dir() {
            cwd.to_path_buf()
        } else {
            crate::model::home()
        };
        let size = Size { cols: 80, rows: 24 };
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let shell = crate::platform::resolve_shell(std::env::var("SHELL").ok().as_deref());
        let mut cmd = if let Some(argv) = argv {
            let first = argv.first().context("Empty terminal command")?;
            let mut c = CommandBuilder::new(first);
            c.args(&argv[1..]);
            c
        } else {
            let mut c = CommandBuilder::new(&shell);
            c.args(crate::platform::shell_args(&shell));
            c
        };
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Caelum Rust");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        // Lets shell rc files enable the integration only inside Caelum.
        cmd.env("CAELUM_RS", env!("CARGO_PKG_VERSION"));
        // Nonblocking descriptors let both workers honor shutdown even if a descendant holds the PTY open.
        #[cfg(unix)]
        if let Some(fd) = pair.master.as_raw_fd() {
            // SAFETY: fd is owned by the live PTY; fcntl changes flags without taking ownership.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                    bail!("Cannot configure nonblocking PTY");
                }
            }
        }
        // The reader thread waits for readiness with poll(2) on its own duplicate of the
        // master descriptor, so it never outlives the fd it polls.
        #[cfg(unix)]
        let poll_fd = {
            use std::os::fd::{FromRawFd, OwnedFd};
            let fd = pair.master.as_raw_fd().context("PTY has no descriptor")?;
            // SAFETY: fd is a live descriptor; dup returns a new one we exclusively own.
            let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if dup < 0 {
                bail!("Cannot duplicate PTY descriptor");
            }
            // SAFETY: dup is a freshly created, valid descriptor owned by nobody else.
            unsafe { OwnedFd::from_raw_fd(dup) }
        };
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let (listener, events) = Events::new();
        let term = Arc::new(Mutex::new(Term::new(Config::default(), &size, listener)));
        let stop = Arc::new(AtomicBool::new(false));
        let read_stop = stop.clone();
        let read_term = term.clone();
        let read_ctx = ctx.clone();
        let shell = Arc::new(Mutex::new(ShellState::default()));
        let read_shell = shell.clone();
        let read_thread = thread::spawn(move || {
            read_loop(
                reader,
                read_term,
                read_shell,
                read_stop,
                read_ctx,
                #[cfg(unix)]
                poll_fd,
            )
        });
        let (input, rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let write_stop = stop.clone();
        let write_thread = thread::spawn(move || write_loop(writer, rx, write_stop));
        Ok(Self {
            term,
            title: "Terminal".into(),
            cwd,
            exited: false,
            error: None,
            shell,
            master: Some(pair.master),
            child: Some(child),
            input: Some(input),
            events,
            stop,
            threads: vec![read_thread, write_thread],
            size,
            search_origin: None,
            pending_input: Vec::new(),
            pending_paste: None,
            wake: ctx,
        })
    }
    pub fn write(&mut self, bytes: Vec<u8>) {
        const MAX_PENDING: usize = 8 * 1024 * 1024;
        if self.pending_input.len().saturating_add(bytes.len()) > MAX_PENDING {
            self.error = Some(
                "Terminal input buffer is full; wait for the process to read before pasting again"
                    .into(),
            );
            return;
        }
        self.pending_input.extend(bytes);
        self.flush_input();
    }
    fn flush_input(&mut self) {
        if self.pending_input.is_empty() {
            return;
        }
        if let Some(tx) = &self.input {
            let bytes = std::mem::take(&mut self.pending_input);
            match tx.try_send(bytes) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(bytes)) => {
                    self.pending_input = bytes;
                    self.wake.request_repaint_after(Duration::from_millis(8));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.error = Some("Terminal input has closed".into())
                }
            }
        }
    }
    /// Writes a paste to the PTY now, bracketed if the program enabled it.
    pub fn paste(&mut self, text: &str) {
        let bracketed = self.bracketed_paste();
        self.write(paste_bytes(text, bracketed));
    }
    /// Pastes like [`Session::paste`], unless [`needs_confirm`] says the user should
    /// see it first; then it is held until [`Session::take_pending_paste`].
    /// Returns true when the paste was held.
    pub fn paste_or_hold(&mut self, text: &str, confirm_multiline: bool) -> bool {
        if needs_confirm(text, self.bracketed_paste(), confirm_multiline) {
            self.pending_paste = Some(sanitize_paste(text));
            true
        } else {
            self.paste(text);
            false
        }
    }
    /// The held multi-line paste, if any, for the app to confirm.
    pub fn take_pending_paste(&mut self) -> Option<String> {
        self.pending_paste.take()
    }
    fn bracketed_paste(&self) -> bool {
        lock(&self.term).mode().contains(TermMode::BRACKETED_PASTE)
    }
    pub fn poll(&mut self, theme: &Theme) {
        self.flush_input();
        if let Some(title) = lock(&self.events.title).take() {
            self.title = title;
        }
        while let Ok(event) = self.events.rx.try_recv() {
            match event {
                Event::PtyWrite(text) => self.write(text.into_bytes()),
                Event::ColorRequest(i, format) => {
                    let c = index_color(i, theme);
                    self.write(
                        format(ansi::Rgb {
                            r: c.r(),
                            g: c.g(),
                            b: c.b(),
                        })
                        .into_bytes(),
                    );
                }
                Event::TextAreaSizeRequest(format) => self.write(
                    format(alacritty_terminal::event::WindowSize {
                        num_lines: self.size.rows as u16,
                        num_cols: self.size.cols as u16,
                        cell_width: 0,
                        cell_height: 0,
                    })
                    .into_bytes(),
                ),
                // Clipboard escape sequences are deliberately not granted automatic clipboard access.
                _ => {}
            }
        }
        if !self.exited
            && let Some(child) = &mut self.child
        {
            match child.try_wait() {
                Ok(Some(_)) => self.exited = true,
                Ok(None) => {}
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        // A shell-reported OSC 7 directory wins; the platform lookup is the fallback
        // for shells without integration (it only sees the process we spawned, not subshells).
        let reported = lock(&self.shell).cwd.clone().filter(|p| p.is_dir());
        if let Some(cwd) = reported {
            self.cwd = cwd;
        } else if let Some(pid) = self.pid()
            && !self.exited
            && let Some(cwd) = crate::platform::process_cwd(pid)
        {
            self.cwd = cwd;
        }
    }
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let size = Size {
            cols: cols.clamp(2, 1000),
            rows: rows.clamp(1, 1000),
        };
        if self.size.cols == size.cols && self.size.rows == size.rows {
            return;
        }
        let Some(master) = &self.master else { return };
        if let Err(e) = master.resize(PtySize {
            rows: size.rows as u16,
            cols: size.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            self.error = Some(e.to_string());
            return;
        }
        if self.size.cols != size.cols {
            // Reflow changes line counts, so stored line positions are no longer valid.
            lock(&self.shell).clear_marks();
        }
        lock(&self.term).resize(size);
        self.size = size;
    }
    pub fn text(&self) -> String {
        let term = lock(&self.term);
        let mut text = String::new();
        for row in 0..term.screen_lines() {
            for col in 0..term.columns() {
                let c = &term.grid()[Point::new(Line(row as i32), Column(col))];
                if !c
                    .flags
                    .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
                {
                    text.push(c.c);
                }
            }
            text.push('\n');
        }
        text
    }
    pub fn search(&mut self, query: &str, forward: bool) -> bool {
        use alacritty_terminal::{index::Direction, term::search::RegexSearch};
        if query.is_empty() {
            return false;
        }
        let escaped: String = query
            .chars()
            .flat_map(|c| {
                if ".+*?()[]{}|^$\\".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        let Ok(mut regex) = RegexSearch::new(&escaped) else {
            return false;
        };
        let mut term = lock(&self.term);
        let mut origin = self
            .search_origin
            .as_ref()
            .filter(|(q, _)| q == query)
            .map(|(_, p)| *p)
            .unwrap_or_else(|| Point::new(Line(-(term.grid().display_offset() as i32)), Column(0)));
        if self.search_origin.as_ref().is_some_and(|(q, _)| q == query) {
            if forward {
                if origin.column.0 + 1 < term.columns() {
                    origin.column.0 += 1;
                } else {
                    origin.column = Column(0);
                    origin.line.0 += 1;
                    if origin.line > term.bottommost_line() {
                        origin.line = term.topmost_line();
                    }
                }
            } else if origin.column.0 > 0 {
                origin.column.0 -= 1;
            } else {
                origin.column = term.last_column();
                origin.line.0 -= 1;
                if origin.line < term.topmost_line() {
                    origin.line = term.bottommost_line();
                }
            }
        }
        let found = term.search_next(
            &mut regex,
            origin,
            if forward {
                Direction::Right
            } else {
                Direction::Left
            },
            Side::Left,
            None,
        );
        if let Some(found) = found {
            let start = *found.start();
            let end = *found.end();
            let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
            selection.update(end, Side::Right);
            term.selection = Some(selection);
            let target = (-start.line.0 + term.screen_lines() as i32 / 2).max(0);
            term.scroll_display(Scroll::Bottom);
            term.scroll_display(Scroll::Delta(target));
            self.search_origin = Some((query.to_string(), start));
            true
        } else {
            false
        }
    }
    /// Scroll to the previous/next shell prompt (needs OSC 133 integration).
    /// Returns false when there is no such prompt.
    pub fn jump_prompt(&mut self, forward: bool) -> bool {
        let mut term = lock(&self.term);
        let shell = lock(&self.shell);
        let history = term.grid().history_size() as i64;
        let top = -(term.grid().display_offset() as i64);
        let lines = shell.marks.iter().map(|abs| abs - history);
        let target = if forward {
            lines.filter(|l| *l > top).min()
        } else {
            lines.filter(|l| *l < top).max()
        };
        match target {
            Some(line) => {
                let offset = (-line).clamp(0, history) as i32;
                term.scroll_display(Scroll::Bottom);
                term.scroll_display(Scroll::Delta(offset));
                true
            }
            None if forward => {
                term.scroll_display(Scroll::Bottom);
                false
            }
            None => false,
        }
    }
    pub fn selection(&self) -> Option<String> {
        lock(&self.term).selection_to_string()
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref()?.process_id()
    }
}
/// PTY reader thread: feeds output to the parser (applying OSC 7/133 events at
/// their exact byte positions) until EOF, an error, or the stop flag.
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    term: Arc<Mutex<Term<Events>>>,
    shell: Arc<Mutex<ShellState>>,
    stop: Arc<AtomicBool>,
    ctx: egui::Context,
    #[cfg(unix)] poll_fd: std::os::fd::OwnedFd,
) {
    let mut parser: ansi::Processor = ansi::Processor::new();
    let mut scanner = OscScanner::default();
    let mut bytes = [0; 16384];
    while !stop.load(Ordering::Relaxed) {
        match reader.read(&mut bytes) {
            Ok(0) => break,
            Ok(n) => {
                {
                    let events = scanner.scan(&bytes[..n]);
                    let mut t = lock(&term);
                    let mut shell = lock(&shell);
                    let mut done = 0;
                    for (end, event) in events {
                        parser.advance(&mut *t, &bytes[done..end]);
                        done = end;
                        apply_osc(&t, &mut shell, event);
                    }
                    parser.advance(&mut *t, &bytes[done..n]);
                    reconcile_marks(&t, &mut shell);
                }
                ctx.request_repaint();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                #[cfg(unix)]
                wait_readable(&poll_fd, READ_IDLE_WAIT);
                #[cfg(not(unix))]
                thread::sleep(READ_IDLE_WAIT);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    ctx.request_repaint();
}
/// PTY writer thread: drains queued input chunks, retrying partial and
/// would-block writes, until the channel closes or the stop flag is set.
fn write_loop(mut writer: Box<dyn Write + Send>, rx: Receiver<Vec<u8>>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let bytes = match rx.recv_timeout(Duration::from_millis(30)) {
            Ok(b) => b,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        let mut offset = 0;
        while offset < bytes.len() && !stop.load(Ordering::Relaxed) {
            match writer.write(&bytes[offset..]) {
                Ok(0) => return,
                Ok(n) => offset += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(4))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return,
            }
        }
    }
}
/// Longest the reader sleeps between checks of the stop flag while the PTY is idle.
const READ_IDLE_WAIT: Duration = if cfg!(unix) {
    Duration::from_millis(100)
} else {
    Duration::from_millis(25)
};
/// Blocks until `fd` is readable, hung up, or `timeout` elapses, so an idle
/// session costs no CPU while staying responsive to its stop flag.
#[cfg(unix)]
fn wait_readable(fd: &std::os::fd::OwnedFd, timeout: Duration) {
    use std::os::fd::AsRawFd;
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pfd is a valid pollfd for a descriptor we own for the whole call.
    // EINTR and errors simply return; the caller retries the read.
    unsafe {
        libc::poll(&mut pfd, 1, timeout.as_millis() as libc::c_int);
    }
}
/// Whether a paste must be confirmed before it reaches the PTY: without bracketed
/// paste every line break acts as Enter, so a multi-line paste can run commands the
/// user never saw. Bracketed-paste programs (and users who opted out) paste at once.
pub fn needs_confirm(text: &str, bracketed: bool, confirm_multiline: bool) -> bool {
    confirm_multiline && !bracketed && sanitize_paste(text).contains(['\n', '\r'])
}
/// Lines shown in the paste confirmation preview.
const PREVIEW_LINES: usize = 5;
const PREVIEW_LINE_CHARS: usize = 120;
/// Line count and a safe preview of a held paste: the first [`PREVIEW_LINES`]
/// lines with controls and bidi characters escaped, long lines shortened.
pub fn paste_preview(text: &str) -> (usize, String) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.strip_suffix('\n').unwrap_or(&normalized);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut preview: Vec<String> = lines
        .iter()
        .take(PREVIEW_LINES)
        .map(|line| {
            let escaped: String = line
                .chars()
                .flat_map(|c| {
                    if c.is_control() || is_bidi_control(c) {
                        c.escape_default().collect::<Vec<_>>()
                    } else {
                        vec![c]
                    }
                })
                .collect();
            if escaped.chars().count() > PREVIEW_LINE_CHARS {
                let head: String = escaped.chars().take(PREVIEW_LINE_CHARS).collect();
                format!("{head}…")
            } else {
                escaped
            }
        })
        .collect();
    if lines.len() > PREVIEW_LINES {
        preview.push(format!("… {} more line(s)", lines.len() - PREVIEW_LINES));
    }
    (lines.len(), preview.join("\n"))
}
/// Bytes sent to the PTY for a paste, after sanitising and optional bracketing.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = sanitize_paste(text);
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\n").replace('\n', "\r").into_bytes()
    }
}
impl Drop for Session {
    /// Closing a pane must never stall the UI: sweeping the session's process tree
    /// (a `/proc` scan on Linux), reaping the child and joining the I/O threads
    /// (each can sit in a poll/recv timeout) all happen on a detached cleanup thread.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.input.take();
        let (Some(mut child), Some(master)) = (self.child.take(), self.master.take()) else {
            return;
        };
        let threads = std::mem::take(&mut self.threads);
        // Decide liveness before any signal so the cleanup thread never reaps the
        // child before it has walked the child's session.
        let running = matches!(child.try_wait(), Ok(None));
        // Signal the foreground job and the shell's process group right away (a few
        // non-blocking syscalls) so they die even if the app exits before the cleanup
        // thread runs. The thread then also sweeps background jobs in the session.
        #[cfg(unix)]
        if running && let Some(pid) = child.process_id() {
            let pid = pid as libc::pid_t;
            // SAFETY: getsid/kill have no memory-safety preconditions. The child is
            // unreaped, so its pid (a session and group leader created by portable-pty)
            // is still ours, and the foreground group is only signalled if it belongs
            // to that session.
            unsafe {
                if let Some(fg) = master.process_group_leader()
                    && fg > 0
                    && libc::getsid(fg) == pid
                {
                    libc::kill(-fg, libc::SIGKILL);
                }
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let cleanup = move || {
            if running && let Some(pid) = child.process_id() {
                crate::platform::kill_process_tree(pid, &*master);
            }
            let _ = child.kill();
            // Unix: close every master descriptor before reaping. The I/O threads
            // hold clones of it, and the writer's drop queues a newline + EOF whose
            // echo nobody reads once the reader has stopped. On macOS an exiting
            // session leader waits in the kernel (exit -> ttywait) for its tty's
            // output to drain, so reaping while any master fd is open blocks forever
            // and the child never finishes exiting. Both threads honour the stop flag
            // within one poll interval and close their clones on return.
            #[cfg(unix)]
            {
                drop(master);
                for t in threads {
                    let _ = t.join();
                }
                let _ = child.wait();
            }
            // Windows: the child is already terminated; reap it first, then let the
            // threads finish before closing the pseudoconsole (unchanged order).
            #[cfg(not(unix))]
            {
                let _ = child.wait();
                for t in threads {
                    let _ = t.join();
                }
                drop(master);
            }
        };
        if let Err(e) = thread::Builder::new()
            .name("caelum-pty-cleanup".into())
            .spawn(cleanup)
        {
            // Out of threads: the closure (and the child) was dropped unrun. Nothing
            // more can be done without blocking the UI; record it for diagnosis.
            eprintln!("caelum: could not spawn PTY cleanup thread: {e}");
        }
    }
}
const ANSI: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];
fn index_color(i: usize, theme: &Theme) -> Color32 {
    if i < 16 {
        return theme.term(ANSI[i]);
    }
    if i < 232 {
        let n = i - 16;
        let level = |v| if v == 0 { 0 } else { 55 + 40 * v as u8 };
        return Color32::from_rgb(level(n / 36), level((n / 6) % 6), level(n % 6));
    }
    if i < 256 {
        let c = 8 + 10 * (i - 232) as u8;
        return Color32::from_gray(c);
    }
    match i {
        256 => theme.term("foreground"),
        257 => theme.term("background"),
        258 => theme.term("cursor"),
        _ => theme.term("foreground"),
    }
}
fn resolve_color(c: Color, theme: &Theme) -> Color32 {
    match c {
        Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => index_color(i as usize, theme),
        Color::Named(n) => match n {
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                theme.term("foreground")
            }
            NamedColor::Background => theme.term("background"),
            NamedColor::Cursor => theme.term("cursor"),
            _ => index_color((n as usize) % 16, theme),
        },
    }
}
/// Xterm-style key sequences including application cursor mode and modifiers.
pub fn key_bytes(key: Key, m: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    let modifier = 1 + usize::from(m.shift) + 2 * usize::from(m.alt) + 4 * usize::from(m.ctrl);
    let seq: String = match key {
        Key::Enter => "\r".into(),
        Key::Tab => if m.shift { "\x1b[Z" } else { "\t" }.into(),
        Key::Backspace => if m.alt { "\x1b\x7f" } else { "\x7f" }.into(),
        Key::Escape => "\x1b".into(),
        Key::ArrowUp | Key::ArrowDown | Key::ArrowRight | Key::ArrowLeft | Key::Home | Key::End => {
            let c = match key {
                Key::ArrowUp => 'A',
                Key::ArrowDown => 'B',
                Key::ArrowRight => 'C',
                Key::ArrowLeft => 'D',
                Key::Home => 'H',
                _ => 'F',
            };
            if modifier > 1 {
                format!("\x1b[1;{modifier}{c}")
            } else if mode.contains(TermMode::APP_CURSOR) {
                format!("\x1bO{c}")
            } else {
                format!("\x1b[{c}")
            }
        }
        Key::Insert
        | Key::Delete
        | Key::PageUp
        | Key::PageDown
        | Key::F5
        | Key::F6
        | Key::F7
        | Key::F8
        | Key::F9
        | Key::F10
        | Key::F11
        | Key::F12 => {
            let n = match key {
                Key::Insert => 2,
                Key::Delete => 3,
                Key::PageUp => 5,
                Key::PageDown => 6,
                Key::F5 => 15,
                Key::F6 => 17,
                Key::F7 => 18,
                Key::F8 => 19,
                Key::F9 => 20,
                Key::F10 => 21,
                Key::F11 => 23,
                _ => 24,
            };
            if modifier > 1 {
                format!("\x1b[{n};{modifier}~")
            } else {
                format!("\x1b[{n}~")
            }
        }
        Key::F1 | Key::F2 | Key::F3 | Key::F4 => {
            let c = match key {
                Key::F1 => 'P',
                Key::F2 => 'Q',
                Key::F3 => 'R',
                _ => 'S',
            };
            if modifier > 1 {
                format!("\x1b[1;{modifier}{c}")
            } else {
                format!("\x1bO{c}")
            }
        }
        _ if m.ctrl => {
            let name = key.name();
            if name.len() == 1 {
                let c = name.as_bytes()[0].to_ascii_uppercase();
                if (b'@'..=b'_').contains(&c) {
                    return Some(vec![c & 0x1f]);
                }
            }
            if key == Key::Space {
                return Some(vec![0]);
            }
            return None;
        }
        _ => return None,
    };
    Some(seq.into_bytes())
}
/// Only web links from OSC 8 hyperlinks are ever opened.
fn openable_link(uri: &str) -> bool {
    uri.starts_with("https://") || uri.starts_with("http://")
}

/// Renders a hyperlink target for a tooltip: control characters are escaped so a
/// URI cannot hide its real destination behind newlines or bidi tricks, and very
/// long targets are shortened in the middle.
fn hover_label(uri: &str) -> String {
    let clean: String = uri
        .chars()
        .flat_map(|c| {
            if c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect();
    let chars: Vec<char> = clean.chars().collect();
    if chars.len() > 300 {
        let head: String = chars[..200].iter().collect();
        let tail: String = chars[chars.len() - 80..].iter().collect();
        format!("{head}…{tail}")
    } else {
        clean
    }
}

/// Pixel geometry of the terminal grid for one frame.
#[derive(Clone, Copy)]
struct Metrics {
    /// Whole pane, including padding.
    rect: Rect,
    /// Padded area the cells are drawn in.
    area: Rect,
    /// Cell width and height in points.
    cw: f32,
    ch: f32,
    size: Size,
}
impl Metrics {
    /// Grid point under `pos`, clamped to the visible grid, for the given display offset.
    fn point_at(&self, pos: Pos2, offset: usize) -> Point {
        Point::new(
            Line(
                ((pos.y - self.area.top()) / self.ch)
                    .floor()
                    .clamp(0., self.size.rows as f32 - 1.) as i32
                    - offset as i32,
            ),
            Column(
                ((pos.x - self.area.left()) / self.cw)
                    .floor()
                    .clamp(0., self.size.cols as f32 - 1.) as usize,
            ),
        )
    }
    /// Top-left corner of the cell at grid `col`/screen `row`.
    fn cell_pos(&self, col: usize, row: i32) -> Pos2 {
        self.area.min + Vec2::new(col as f32 * self.cw, row as f32 * self.ch)
    }
}

/// How pointer input is interpreted this frame.
#[derive(Clone, Copy)]
struct PointerMode {
    /// Display offset (scrollback position) at the start of the frame.
    offset: usize,
    /// The program asked for mouse reports.
    reporting: bool,
    /// Pointer input selects text (no reporting, or Shift is held).
    selecting: bool,
}
impl PointerMode {
    fn forwards_to_program(self) -> bool {
        self.reporting && !self.selecting
    }
}

pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    theme: &Theme,
    prefs: &Preferences,
    focused: bool,
    search: &str,
) -> bool {
    let font = FontId::monospace(prefs.font_size);
    let (response, m) = layout(ui, s, theme, prefs, &font);
    let pointer = pointer_mode(ui, s);
    handle_selection(s, &response, &m, pointer);
    manage_focus(ui, &response, focused);
    let mut clicked = response.clicked() || response.drag_started();
    handle_links(ui, s, &response, &m, pointer);
    handle_scroll(ui, s, &response, &m, pointer);
    if focused && (response.has_focus() || !ui.ctx().wants_keyboard_input()) {
        handle_events(ui, s, prefs, &response, &m, pointer);
    }
    let painter = ui.painter_at(m.rect);
    paint_grid(ui, s, theme, prefs, &painter, &m, &font, focused, search);
    paint_exit_badge(s, theme, prefs, &painter, m.rect);
    context_menu(s, &response, &mut clicked);
    clicked
}

/// Allocates the pane, paints its background, sizes the PTY to fit and drains
/// pending session events.
fn layout(
    ui: &mut egui::Ui,
    s: &mut Session,
    theme: &Theme,
    prefs: &Preferences,
    font: &FontId,
) -> (egui::Response, Metrics) {
    let cw = ui.fonts_mut(|f| f.glyph_width(font, 'M'));
    let ch = prefs.font_size * prefs.line_height;
    let (rect, response) = ui.allocate_exact_size(
        ui.available_size().max(Vec2::splat(1.)),
        Sense::click_and_drag(),
    );
    let area = rect.shrink(8.);
    ui.painter_at(rect)
        .rect_filled(rect, 0., theme.term("background"));
    s.resize((area.width() / cw) as usize, (area.height() / ch) as usize);
    s.poll(theme);
    let m = Metrics {
        rect,
        area,
        cw,
        ch,
        size: s.size,
    };
    (response, m)
}

fn pointer_mode(ui: &egui::Ui, s: &Session) -> PointerMode {
    let offset = lock(&s.term).grid().display_offset();
    let reporting = lock(&s.term)
        .mode()
        .intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION);
    let selecting = !reporting || ui.input(|i| i.modifiers.shift);
    PointerMode {
        offset,
        reporting,
        selecting,
    }
}

/// Drag selects characters, double-click selects a word.
fn handle_selection(s: &Session, response: &egui::Response, m: &Metrics, p: PointerMode) {
    if !p.selecting {
        return;
    }
    let start = |kind| {
        response
            .interact_pointer_pos()
            .map(|pos| Selection::new(kind, m.point_at(pos, p.offset), Side::Left))
    };
    if response.drag_started()
        && let Some(selection) = start(SelectionType::Simple)
    {
        lock(&s.term).selection = Some(selection);
    }
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(selection) = &mut lock(&s.term).selection
    {
        selection.update(m.point_at(pos, p.offset), Side::Right);
    }
    if response.double_clicked()
        && let Some(selection) = start(SelectionType::Semantic)
    {
        lock(&s.term).selection = Some(selection);
    }
}

/// Takes keyboard focus when the pane is the active one, and keeps Tab, arrows and
/// Escape for the terminal while it has focus.
fn manage_focus(ui: &egui::Ui, response: &egui::Response, focused: bool) {
    if focused && !ui.ctx().wants_keyboard_input() {
        response.request_focus();
    }
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    }
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
}

/// OSC 8 link text can differ from its target, so the real URI is always revealed
/// on hover before a Ctrl+click opens it. A plain click clears the selection.
fn handle_links(
    ui: &egui::Ui,
    s: &Session,
    response: &egui::Response,
    m: &Metrics,
    p: PointerMode,
) {
    if let Some(pos) = response.hover_pos() {
        let uri = lock(&s.term).grid()[m.point_at(pos, p.offset)]
            .hyperlink()
            .map(|link| link.uri().to_string());
        if let Some(uri) = uri {
            let tip = if openable_link(&uri) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                format!("{}\nCtrl+click to open", hover_label(&uri))
            } else {
                format!(
                    "{}\n(not opened: only http and https links)",
                    hover_label(&uri)
                )
            };
            response.clone().on_hover_text_at_pointer(tip);
        }
    }
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    if response.clicked()
        && ctrl
        && let Some(pos) = response.interact_pointer_pos()
    {
        let term = lock(&s.term);
        if let Some(link) = term.grid()[m.point_at(pos, p.offset)].hyperlink() {
            let url = link.uri();
            if openable_link(url) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
        }
    }
    if response.clicked() && !ctrl && p.selecting {
        lock(&s.term).selection = None;
    }
}

/// Wheel scrolls the scrollback, or is reported to a program that asked for mouse events.
fn handle_scroll(
    ui: &egui::Ui,
    s: &mut Session,
    response: &egui::Response,
    m: &Metrics,
    p: PointerMode,
) {
    if !response.hovered() {
        return;
    }
    let delta = ui.input(|i| i.raw_scroll_delta.y);
    if delta == 0. {
        return;
    }
    if p.forwards_to_program() {
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            let point = m.point_at(pos, 0);
            s.write(
                format!(
                    "\x1b[<{};{};{}M",
                    if delta > 0. { 64 } else { 65 },
                    point.column.0 + 1,
                    point.line.0 + 1
                )
                .into_bytes(),
            );
        }
    } else {
        lock(&s.term).scroll_display(Scroll::Delta((delta / 12.).round() as i32));
    }
}

/// Translates this frame's keyboard, clipboard, IME and mouse-button events into PTY input.
fn handle_events(
    ui: &egui::Ui,
    s: &mut Session,
    prefs: &Preferences,
    response: &egui::Response,
    m: &Metrics,
    p: PointerMode,
) {
    let mode = *lock(&s.term).mode();
    let events = ui.input(|i| i.events.clone());
    for event in events {
        match event {
            egui::Event::Text(text) => {
                if !ui.input(|i| i.modifiers.ctrl || i.modifiers.command) {
                    let text = if ui.input(|i| i.modifiers.alt) {
                        format!("\x1b{text}")
                    } else {
                        text
                    };
                    s.write(text.into_bytes());
                    lock(&s.term).scroll_display(Scroll::Bottom);
                }
            }
            egui::Event::Copy => {
                if ui.input(|i| i.modifiers.shift) {
                    if let Some(text) = s.selection() {
                        ui.ctx().copy_text(text);
                    }
                } else {
                    s.write(vec![3]);
                }
            }
            egui::Event::Cut => s.write(vec![24]),
            egui::Event::Paste(text) => {
                s.paste_or_hold(&text, prefs.confirm_multiline_paste);
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => handle_key(s, key, modifiers, mode),
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => s.write(text.into_bytes()),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } if response.hovered() && p.forwards_to_program() => {
                report_button(s, m.point_at(pos, 0), button, pressed)
            }
            _ => {}
        }
    }
}

/// Shift+PageUp/PageDown/Home/End scroll the scrollback; other keys go to the PTY.
fn handle_key(s: &mut Session, key: Key, modifiers: Modifiers, mode: TermMode) {
    if modifiers.shift
        && !modifiers.ctrl
        && matches!(key, Key::PageUp | Key::PageDown | Key::Home | Key::End)
    {
        lock(&s.term).scroll_display(match key {
            Key::PageUp => Scroll::PageUp,
            Key::PageDown => Scroll::PageDown,
            Key::Home => Scroll::Top,
            _ => Scroll::Bottom,
        });
        return;
    }
    if !(modifiers.ctrl && modifiers.shift)
        && let Some(bytes) = key_bytes(key, modifiers, mode)
    {
        s.write(bytes);
        lock(&s.term).scroll_display(Scroll::Bottom);
    }
}

/// SGR (1006) mouse button report.
fn report_button(s: &mut Session, p: Point, button: egui::PointerButton, pressed: bool) {
    let b = match button {
        egui::PointerButton::Primary => 0,
        egui::PointerButton::Middle => 1,
        _ => 2,
    };
    s.write(
        format!(
            "\x1b[<{b};{};{}{}",
            p.column.0 + 1,
            p.line.0 + 1,
            if pressed { 'M' } else { 'm' }
        )
        .into_bytes(),
    );
}

/// Cells (line, column) covered by literal matches of `search` on the visible screen.
fn search_cells(
    term: &Term<Events>,
    size: Size,
    offset: i32,
    search: &str,
) -> std::collections::HashSet<(i32, usize)> {
    use alacritty_terminal::term::cell::Flags;
    let mut cells = std::collections::HashSet::new();
    if search.is_empty() {
        return cells;
    }
    for row in 0..size.rows {
        let line = Line(row as i32 - offset);
        let mut text = String::new();
        let mut positions = Vec::new();
        for col in 0..size.cols {
            let point = Point::new(line, Column(col));
            let cell = &term.grid()[point];
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                positions.push((text.len(), point));
                text.push(cell.c);
            }
        }
        for (start, found) in text.match_indices(search) {
            for (byte, point) in &positions {
                if *byte >= start && *byte < start + found.len() {
                    cells.insert((point.line.0, point.column.0));
                }
            }
        }
    }
    cells
}

/// Paints every visible cell, search highlights and the cursor.
#[allow(clippy::too_many_arguments)]
fn paint_grid(
    ui: &egui::Ui,
    s: &Session,
    theme: &Theme,
    prefs: &Preferences,
    painter: &egui::Painter,
    m: &Metrics,
    font: &FontId,
    focused: bool,
    search: &str,
) {
    let term = lock(&s.term);
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let cursor = content.cursor;
    let colors = content.colors;
    let highlighted = search_cells(&term, m.size, offset, search);
    for cell in content.display_iter {
        let point = cell.point;
        let row = point.line.0 + offset;
        if row < 0 || row >= m.size.rows as i32 {
            continue;
        }
        let selected = selection.is_some_and(|s| s.contains(point));
        let found = highlighted.contains(&(point.line.0, point.column.0));
        paint_cell(
            theme,
            painter,
            m,
            font,
            colors,
            &cell,
            m.cell_pos(point.column.0, row),
            selected,
            found,
        );
    }
    let show_cursor = focused
        && term.mode().contains(TermMode::SHOW_CURSOR)
        && offset == 0
        && (!prefs.cursor_blink || ui.input(|i| ((i.time * 2.) as u64).is_multiple_of(2)));
    drop(term);
    if show_cursor {
        let r = Rect::from_min_size(
            m.cell_pos(cursor.point.column.0, cursor.point.line.0),
            Vec2::new(m.cw, m.ch),
        );
        paint_cursor(theme, prefs, painter, r);
    }
}

/// Paints one cell's background, glyph (with combining marks) and underline.
#[allow(clippy::too_many_arguments)]
fn paint_cell(
    theme: &Theme,
    painter: &egui::Painter,
    m: &Metrics,
    font: &FontId,
    colors: &alacritty_terminal::term::color::Colors,
    cell: &alacritty_terminal::grid::Indexed<&alacritty_terminal::term::cell::Cell>,
    pos: Pos2,
    selected: bool,
    found: bool,
) {
    use alacritty_terminal::term::cell::Flags;
    let (cw, ch) = (m.cw, m.ch);
    let cell_rect = Rect::from_min_size(pos, Vec2::new(cw, ch));
    let dynamic = |c: Color| {
        let index = match c {
            Color::Named(n) => Some(n as usize),
            Color::Indexed(n) => Some(n as usize),
            Color::Spec(_) => None,
        };
        index
            .and_then(|i| colors[i])
            .map(|rgb| Color32::from_rgb(rgb.r, rgb.g, rgb.b))
    };
    let mut fg = dynamic(cell.fg).unwrap_or_else(|| resolve_color(cell.fg, theme));
    let mut bg = dynamic(cell.bg).unwrap_or_else(|| resolve_color(cell.bg, theme));
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if selected {
        bg = theme.term("selectionBackground");
    }
    if found {
        bg = theme.ui("button");
    }
    if bg != theme.term("background") {
        painter.rect_filled(cell_rect, 0., bg);
    }
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::HIDDEN)
    {
        return;
    }
    if cell.flags.contains(Flags::DIM) {
        fg = fg.gamma_multiply(0.66);
    }
    if cell.c != ' ' {
        let mut text = cell.c.to_string();
        if let Some(extra) = cell.zerowidth() {
            text.extend(extra);
        }
        painter.text(pos, egui::Align2::LEFT_TOP, text, font.clone(), fg);
    }
    if cell.flags.intersects(Flags::ALL_UNDERLINES) {
        painter.line_segment(
            [
                Pos2::new(pos.x, pos.y + ch - 2.),
                Pos2::new(pos.x + cw, pos.y + ch - 2.),
            ],
            Stroke::new(1_f32, fg),
        );
    }
}

fn paint_cursor(theme: &Theme, prefs: &Preferences, painter: &egui::Painter, r: Rect) {
    let color = theme.term("cursor");
    match prefs.cursor_style.as_str() {
        "bar" => {
            painter.line_segment([r.left_top(), r.left_bottom()], Stroke::new(2_f32, color));
        }
        "underline" => {
            painter.line_segment(
                [r.left_bottom(), r.right_bottom()],
                Stroke::new(2_f32, color),
            );
        }
        _ => {
            painter.rect_stroke(r, 0., Stroke::new(1.5_f32, color), egui::StrokeKind::Inside);
        }
    }
}

/// Small "exit N" badge in the corner after a command reported a non-zero status.
fn paint_exit_badge(
    s: &Session,
    theme: &Theme,
    prefs: &Preferences,
    painter: &egui::Painter,
    rect: Rect,
) {
    let shell = lock(&s.shell);
    let Some(code) = shell.last_exit.filter(|c| *c != 0 && !shell.running) else {
        return;
    };
    let galley = painter.layout_no_wrap(
        format!("exit {code}"),
        FontId::monospace(prefs.font_size * 0.8),
        theme.ui("danger"),
    );
    let badge = Rect::from_min_size(
        Pos2::new(rect.right() - galley.size().x - 14., rect.top() + 4.),
        galley.size() + Vec2::new(10., 4.),
    );
    painter.rect_filled(badge, 4., theme.term("background").gamma_multiply(0.85));
    painter.rect_stroke(
        badge,
        4.,
        Stroke::new(1_f32, theme.ui("danger")),
        egui::StrokeKind::Inside,
    );
    painter.galley(badge.min + Vec2::new(5., 2.), galley, theme.ui("danger"));
}

fn context_menu(s: &mut Session, response: &egui::Response, clicked: &mut bool) {
    response.context_menu(|ui| {
        if ui.button("Copy selection").clicked() {
            if let Some(text) = s.selection() {
                ui.ctx().copy_text(text);
            }
            ui.close();
        }
        if ui.button("Paste").clicked() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            ui.close();
        }
        if ui.button("Scroll to bottom").clicked() {
            lock(&s.term).scroll_display(Scroll::Bottom);
            ui.close();
        }
        *clicked = true;
    });
}

/// Lock a mutex, recovering the guard if a previous holder panicked.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg_attr(not(unix), allow(unused_imports))]
    use std::time::Instant;
    #[test]
    fn multiline_paste_needs_confirmation_only_without_bracketed_paste() {
        // Single line, any mode: never asks.
        assert!(!needs_confirm("ls -la", false, true));
        assert!(!needs_confirm("ls -la", true, true));
        // Line breaks of every flavour ask when bracketed paste is off.
        for text in ["a\nb", "a\r\nb", "a\rb", "rm -rf ~\n"] {
            assert!(needs_confirm(text, false, true), "{text:?}");
            // Bracketed-paste programs receive it as text, so no prompt.
            assert!(!needs_confirm(text, true, true), "{text:?}");
            // "Don't ask again" turns the prompt off.
            assert!(!needs_confirm(text, false, false), "{text:?}");
        }
        // Decided on the sanitised text: stripped controls are not line breaks.
        assert!(!needs_confirm("a\x0bb\x0cc\u{85}d", false, true));
        assert!(!needs_confirm("", false, true));
    }
    #[test]
    fn paste_preview_counts_lines_and_escapes_controls() {
        assert_eq!(paste_preview("one\ntwo\n"), (2, "one\ntwo".into()));
        assert_eq!(paste_preview("a\r\nb\rc"), (3, "a\nb\nc".into()));
        let (n, preview) = paste_preview("x\ty\u{202e}z\n2\n3\n4\n5\n6\n7");
        assert_eq!(n, 7);
        let lines: Vec<&str> = preview.lines().collect();
        assert_eq!(lines.len(), 6, "{preview}");
        assert_eq!(lines[0], "x\\ty\\u{202e}z");
        assert_eq!(lines[5], "… 2 more line(s)");
        let (_, long) = paste_preview(&"a".repeat(500));
        assert_eq!(long.chars().count(), PREVIEW_LINE_CHARS + 1);
    }
    #[test]
    fn hyperlink_targets_are_shown_safely() {
        assert!(openable_link("https://example.org"));
        assert!(!openable_link("file:///etc/passwd"));
        assert!(!openable_link("javascript:alert(1)"));
        assert_eq!(hover_label("https://a.example/x"), "https://a.example/x");
        let tricky = hover_label("https://good.example\nhttps://evil.example\u{202e}");
        assert!(
            !tricky.contains('\n') && !tricky.contains('\u{202e}'),
            "{tricky}"
        );
        assert!(tricky.contains("evil.example"));
        let long = format!("https://x.example/{}", "a".repeat(1000));
        assert!(hover_label(&long).chars().count() < 300);
    }
    #[test]
    fn key_modes_and_control() {
        assert_eq!(
            key_bytes(Key::C, Modifiers::CTRL, TermMode::empty()).unwrap(),
            vec![3]
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, Modifiers::NONE, TermMode::APP_CURSOR).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            key_bytes(
                Key::ArrowLeft,
                Modifiers {
                    ctrl: true,
                    ..Modifiers::NONE
                },
                TermMode::empty()
            )
            .unwrap(),
            b"\x1b[1;5D"
        );
    }
    #[cfg(unix)]
    #[test]
    fn real_pty_decodes_colors_resizes_and_accepts_input() {
        let d = tempfile::tempdir().unwrap();
        let cmd = vec![
            "/usr/bin/env".into(),
            "HISTFILE=/dev/null".into(),
            "/bin/bash".into(),
            "--noprofile".into(),
            "--norc".into(),
        ];
        let mut s = Session::spawn(d.path(), Some(&cmd), egui::Context::default()).unwrap();
        s.resize(100, 30);
        s.write(b"printf '\\033[31mRUST_PTY_OK\\033[0m\\n'; stty size\r".to_vec());
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if s.text().contains("30 100") && s.text().contains("RUST_PTY_OK") {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("{}", s.text());
    }
    #[cfg(unix)]
    #[test]
    fn missing_cwd_falls_back() {
        let s = Session::spawn(
            Path::new("/caelum-missing-directory"),
            Some(&["/bin/sh".into()]),
            egui::Context::default(),
        )
        .unwrap();
        assert_eq!(s.cwd, crate::model::home());
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    use alacritty_terminal::term::cell::Flags;
    #[cfg_attr(not(unix), allow(unused_imports))]
    use std::time::Instant;
    #[test]
    fn parser_preserves_truecolor_wide_cells_and_alternate_screen() {
        let (events, _queue) = Events::new();
        let mut term = Term::new(Config::default(), &Size { cols: 20, rows: 5 }, events);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, "\x1b[38;2;12;34;56m界\x1b[0m".as_bytes());
        let cell = &term.grid()[Point::new(Line(0), Column(0))];
        assert_eq!(cell.c, '界');
        assert_eq!(
            cell.fg,
            Color::Spec(ansi::Rgb {
                r: 12,
                g: 34,
                b: 56
            })
        );
        assert!(cell.flags.contains(Flags::WIDE_CHAR));
        assert!(
            term.grid()[Point::new(Line(0), Column(1))]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
        );
        parser.advance(&mut term, b"\x1b[?1049h");
        assert!(term.mode().contains(TermMode::ALT_SCREEN));
        parser.advance(&mut term, b"\x1b[?1049l");
        assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, '界');
    }
    #[cfg(unix)]
    #[test]
    fn literal_search_reaches_history() {
        let argv = [
            "/usr/bin/env".into(),
            "HISTFILE=/dev/null".into(),
            "/bin/bash".into(),
            "--noprofile".into(),
            "--norc".into(),
        ];
        let mut s =
            Session::spawn(&crate::model::home(), Some(&argv), egui::Context::default()).unwrap();
        s.write(b"printf 'line-%s\\n' {1..100}\r".to_vec());
        let end = Instant::now() + Duration::from_secs(5);
        while !s.text().contains("line-100") && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(s.search("line-5", true));
        assert!(s.term.lock().unwrap().grid().display_offset() > 0);
        assert_eq!(s.selection().as_deref(), Some("line-5"));
    }
    #[cfg(target_os = "linux")] // inspects /proc
    #[test]
    fn closing_session_stops_foreground_even_when_hangup_is_ignored() {
        let argv = [
            "/bin/bash".into(),
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            "trap '' HUP TERM; sleep 60 & wait".into(),
        ];
        let s =
            Session::spawn(&crate::model::home(), Some(&argv), egui::Context::default()).unwrap();
        let pid = s.pid().unwrap();
        let mut children = Vec::new();
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            children = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|s| s.parse::<u32>().ok())
                .collect();
            if !children.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!children.is_empty());
        drop(s);
        // Cleanup runs on a background thread, so give it a moment to finish.
        let gone = |child: &u32| {
            let status =
                std::fs::read_to_string(format!("/proc/{child}/status")).unwrap_or_default();
            status.is_empty()
                || status
                    .lines()
                    .any(|l| l.starts_with("State:") && l.contains('Z'))
        };
        let end = Instant::now() + Duration::from_secs(5);
        while !children.iter().all(gone) && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        for child in children {
            assert!(gone(&child), "child {child} is still running");
        }
    }
    #[cfg(unix)]
    #[test]
    fn dropping_a_session_does_not_block_the_caller() {
        // Previously kill, reap, the /proc sweep and both thread joins ran here, on
        // the caller's (UI) thread.
        let argv = [
            "/bin/sh".into(),
            "-c".into(),
            "trap '' HUP TERM; while :; do sleep 1; done".into(),
        ];
        let s =
            Session::spawn(&crate::model::home(), Some(&argv), egui::Context::default()).unwrap();
        let pid = s.pid().unwrap();
        thread::sleep(Duration::from_millis(100));
        let start = Instant::now();
        drop(s);
        let took = start.elapsed();
        assert!(
            took < Duration::from_millis(50),
            "drop blocked for {took:?}"
        );
        // The process is still reaped in the background.
        let end = Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 only checks for existence.
        while unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { libc::kill(pid as libc::pid_t, 0) },
            0,
            "{pid} survived"
        );
    }
    #[cfg(unix)]
    #[test]
    fn idle_wait_blocks_until_data_or_timeout() {
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut fds = [0; 2];
        // SAFETY: pipe fills two fresh descriptors that we take ownership of below.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both descriptors were just created and are owned by nobody else.
        let (r, mut w) = unsafe {
            (
                OwnedFd::from_raw_fd(fds[0]),
                std::fs::File::from(OwnedFd::from_raw_fd(fds[1])),
            )
        };
        // Idle: the reader sleeps in the kernel for the whole timeout, no spinning.
        let start = Instant::now();
        wait_readable(&r, Duration::from_millis(100));
        assert!(
            start.elapsed() >= Duration::from_millis(80),
            "{:?}",
            start.elapsed()
        );
        // Data available: it wakes immediately.
        w.write_all(b"x").unwrap();
        let start = Instant::now();
        wait_readable(&r, Duration::from_secs(5));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "{:?}",
            start.elapsed()
        );
    }
    #[test]
    fn titles_are_sanitised() {
        assert_eq!(sanitize_title("vim ~/notes"), "vim ~/notes");
        assert_eq!(
            sanitize_title("a\x07b\x1b[31mc\u{9b}d\x7fe\nf\tg"),
            "ab[31mcdefg"
        );
        assert_eq!(
            sanitize_title("safe\u{202e}txt.exe\u{2066}\u{2069}\u{200e}\u{200f}\u{61c}\u{202a}"),
            "safetxt.exe"
        );
        let long = sanitize_title(&"é".repeat(500));
        assert_eq!(long.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(sanitize_title(" \x1b\u{202e} "), DEFAULT_TITLE);
    }
    #[test]
    fn title_events_are_coalesced_and_sanitised() {
        let (events, queue) = Events::new();
        for i in 0..10_000 {
            events.send_event(Event::Title(format!("t{i}\u{202e}\x07")));
        }
        events.send_event(Event::Wakeup);
        assert_eq!(lock(&queue.title).take().as_deref(), Some("t9999"));
        assert!(
            queue.rx.try_recv().is_err(),
            "titles and wakeups are not queued"
        );
        // Replies are bounded rather than unbounded when the UI is not polling.
        for _ in 0..EVENT_QUEUE * 3 {
            events.send_event(Event::PtyWrite("x".into()));
        }
        assert_eq!(queue.rx.try_iter().count(), EVENT_QUEUE);
    }
    #[test]
    fn paste_strips_controls_but_keeps_text_layout() {
        let payload = "echo hi\x1b[201~\x03\x04\x7f\u{9b}31m\x00\tok\r\nnext\n";
        assert_eq!(sanitize_paste(payload), "echo hi[201~31m\tok\r\nnext\n");
        assert_eq!(
            paste_bytes(payload, false),
            b"echo hi[201~31m\tok\rnext\r".to_vec()
        );
        assert_eq!(
            paste_bytes(payload, true),
            b"\x1b[200~echo hi[201~31m\tok\r\nnext\n\x1b[201~".to_vec()
        );
        assert_eq!(sanitize_paste("naïve 界 😀"), "naïve 界 😀");
    }
}

#[cfg(all(test, unix))]
mod input_queue_tests {
    use super::*;
    use std::time::Instant;
    #[cfg(unix)]
    #[test]
    fn burst_of_small_key_events_is_retained_under_backpressure() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("typed");
        let argv = [
            "/usr/bin/env".into(),
            "HISTFILE=/dev/null".into(),
            "/bin/bash".into(),
            "--noprofile".into(),
            "--norc".into(),
        ];
        let mut s = Session::spawn(dir.path(), Some(&argv), egui::Context::default()).unwrap();
        let text = "x".repeat(if cfg!(target_os = "linux") { 1000 } else { 300 });
        let command = format!("printf '%s' '{text}' > {}\r", file.display());
        for byte in command.bytes() {
            s.write(vec![byte]);
        }
        let theme = crate::theme::presets().remove("starlight").unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            s.poll(&theme);
            if std::fs::read_to_string(&file).ok().as_deref() == Some(&text) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Input was lost: {:?}", s.error);
    }
}

#[cfg(test)]
mod lock_shell_tests {
    use super::*;

    #[test]
    fn poisoned_mutex_still_usable() {
        let m = Arc::new(Mutex::new(5));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.is_poisoned());
        *lock(&m) += 1;
        assert_eq!(*lock(&m), 6);
    }

    #[cfg(unix)]
    #[test]
    fn bad_shell_falls_back() {
        for bad in [
            None,
            Some(""),
            Some("bash"),
            Some("./sh"),
            Some("/nonexistent/shell"),
            Some("/tmp"),
        ] {
            assert_eq!(
                crate::platform::resolve_shell(bad),
                crate::platform::FALLBACK_SHELL,
                "{bad:?}"
            );
        }
        assert_eq!(crate::platform::resolve_shell(Some("/bin/sh")), "/bin/sh");
    }
}
