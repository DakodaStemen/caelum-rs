//! Headless tests (no display server): PTY sessions, documents, workspace state and themes.
#![allow(clippy::field_reassign_with_default)]
#[cfg(unix)]
use alacritty_terminal::{grid::Dimensions, term::TermMode};
#[cfg(unix)]
use caelum_rs::model::Workspace;
use caelum_rs::{
    files::Document,
    model::{Axis, Pane, State},
    terminal::Session,
    theme,
};
use eframe::egui;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn argv(script: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), script.into()]
}

fn spawn(script: &str) -> Session {
    let args = argv(script);
    Session::spawn(Path::new("/tmp"), Some(&args), egui::Context::default()).expect("spawn")
}

fn theme() -> theme::Theme {
    theme::presets().remove("starlight").expect("starlight")
}

/// Polls the session until `cond` holds or 20 s pass.
fn wait(s: &mut Session, what: &str, cond: impl Fn(&Session) -> bool) {
    let t = theme();
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        s.poll(&t);
        if cond(s) {
            return;
        }
        assert!(
            Instant::now() < end,
            "timed out waiting for {what}; screen:\n{}",
            s.text()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn terminal_exit_is_detected_and_output_kept() {
    let mut s = spawn("echo hello-caelum; exit 3");
    wait(&mut s, "exit", |s| s.exited);
    wait(&mut s, "output", |s| s.text().contains("hello-caelum"));
}

#[cfg(unix)]
#[test]
fn terminal_empty_argv_is_an_error() {
    let none: Vec<String> = vec![];
    assert!(Session::spawn(Path::new("/tmp"), Some(&none), egui::Context::default()).is_err());
}

#[test]
fn terminal_ctrl_c_interrupts_foreground_process() {
    let mut s = spawn("trap 'echo got-int; exit 7' INT; echo ready; while :; do sleep 0.05; done");
    wait(&mut s, "ready", |s| s.text().contains("ready"));
    assert!(!s.exited);
    s.write(vec![0x03]);
    wait(&mut s, "interrupt", |s| s.exited);
    wait(&mut s, "trap output", |s| s.text().contains("got-int"));
}

#[cfg(unix)]
#[test]
fn terminal_resize_reaches_pty_and_grid() {
    let mut s = spawn("stty size; read x; stty size; sleep 30");
    wait(&mut s, "initial size", |s| s.text().contains("24 80"));
    s.resize(100, 30);
    {
        let term = s.term.lock().unwrap();
        assert_eq!((term.columns(), term.screen_lines()), (100, 30));
    }
    s.write(b"\n".to_vec());
    wait(&mut s, "resized size", |s| s.text().contains("30 100"));
    // Out-of-range requests are clamped rather than rejected.
    s.resize(1, 0);
    let term = s.term.lock().unwrap();
    assert_eq!((term.columns(), term.screen_lines()), (2, 1));
}

#[cfg(unix)] // needs a POSIX shell; hangs on Windows runners
#[test]
fn terminal_scrollback_retains_history() {
    let mut s = spawn("seq 1 300; sleep 30");
    wait(&mut s, "last line", |s| s.text().contains("300"));
    let term = s.term.lock().unwrap();
    assert!(
        term.history_size() >= 250,
        "history {}",
        term.history_size()
    );
}

#[cfg(unix)] // needs a POSIX shell; hangs on Windows runners
#[test]
fn terminal_alt_screen_enter_and_leave() {
    let mut s = spawn(
        "printf 'main-text'; printf '\\033[?1049h'; printf 'alt-text'; read a; \
         printf '\\033[?1049l'; sleep 30",
    );
    let alt = |s: &Session| s.term.lock().unwrap().mode().contains(TermMode::ALT_SCREEN);
    wait(&mut s, "alt screen", |s| {
        alt(s) && s.text().contains("alt-text")
    });
    assert!(!s.text().contains("main-text"));
    s.write(b"\n".to_vec());
    wait(&mut s, "main screen", |s| !alt(s));
    wait(&mut s, "main restored", |s| s.text().contains("main-text"));
    assert!(!s.text().contains("alt-text"));
}

#[cfg(unix)] // needs a POSIX shell; hangs on Windows runners
#[test]
fn terminal_wide_characters_occupy_two_cells_but_one_char() {
    let mut s = spawn("printf '日本a'; sleep 30");
    wait(&mut s, "wide text", |s| s.text().contains("日本a"));
    let term = s.term.lock().unwrap();
    use alacritty_terminal::{
        index::{Column, Line, Point},
        term::cell::Flags,
    };
    let g = term.grid();
    assert!(
        g[Point::new(Line(0), Column(0))]
            .flags
            .contains(Flags::WIDE_CHAR)
    );
    assert!(
        g[Point::new(Line(0), Column(1))]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
    assert_eq!(g[Point::new(Line(0), Column(2))].c, '本');
    assert_eq!(g[Point::new(Line(0), Column(4))].c, 'a');
}

#[cfg(unix)]
#[test]
fn terminal_burst_input_is_fully_retained_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.txt");
    let mut s = spawn(&format!(
        "stty -echo; cat > '{}'; echo finished; sleep 30",
        out.display()
    ));
    wait(&mut s, "stty", |_| true);
    std::thread::sleep(Duration::from_millis(300));
    let mut expected = String::new();
    for i in 0..30_000 {
        expected.push_str(&format!("line-{i:06}\n"));
    }
    // Written in many chunks, as pastes/keystrokes would be.
    for chunk in expected.as_bytes().chunks(4096) {
        s.write(chunk.to_vec());
    }
    s.write(vec![0x04]);
    wait(&mut s, "cat to finish", |s| s.text().contains("finished"));
    assert_eq!(fs::read_to_string(&out).unwrap(), expected);
    assert!(s.error.is_none(), "{:?}", s.error);
}

/// A multi-line paste into a program without bracketed paste is held until
/// confirmed; a bracketed-paste program receives it immediately.
#[cfg(unix)]
#[test]
fn terminal_multiline_paste_is_held_unless_bracketed() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.txt");
    let mut s = spawn(&format!(
        "stty -echo; echo ready; cat > '{}'; echo finished; sleep 30",
        plain.display()
    ));
    wait(&mut s, "ready", |s| s.text().contains("ready"));
    assert!(s.paste_or_hold("echo one\necho two\n", true));
    // A single line is never held.
    assert!(!s.paste_or_hold("single line", true));
    let held = s.take_pending_paste().expect("paste held");
    assert_eq!(held, "echo one\necho two\n");
    assert!(s.take_pending_paste().is_none());
    // The user confirms: the held text is written with line breaks as Enter.
    s.paste(&held);
    s.write(vec![0x04]);
    wait(&mut s, "cat to finish", |s| s.text().contains("finished"));
    assert_eq!(
        fs::read_to_string(&plain).unwrap(),
        "single lineecho one\necho two\n"
    );

    let bracketed = dir.path().join("bracketed.txt");
    let mut s = spawn(&format!(
        "stty -echo; printf '\\033[?2004h'; cat > '{}'; echo finished; sleep 30",
        bracketed.display()
    ));
    wait(&mut s, "bracketed paste mode", |s| {
        s.term
            .lock()
            .unwrap()
            .mode()
            .contains(TermMode::BRACKETED_PASTE)
    });
    assert!(!s.paste_or_hold("a\nb", true));
    assert!(s.take_pending_paste().is_none());
    s.write(b"\r".to_vec());
    s.write(vec![0x04]);
    wait(&mut s, "cat to finish", |s| s.text().contains("finished"));
    assert_eq!(
        fs::read_to_string(&bracketed).unwrap(),
        "\x1b[200~a\nb\x1b[201~\n"
    );
}

#[test]
fn terminal_oversized_input_sets_error_instead_of_growing() {
    let mut s = spawn("sleep 30");
    s.write(vec![b'x'; 9 * 1024 * 1024]);
    assert!(s.error.is_some());
}

// ---------- files::Document ----------

#[cfg(unix)]
#[test]
fn document_refuses_to_save_hardlinked_file() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a");
    fs::write(&a, "one").unwrap();
    fs::hard_link(&a, d.path().join("b")).unwrap();
    let mut doc = Document::open(&a).unwrap();
    doc.text = "two".into();
    assert!(doc.save().is_err());
    assert_eq!(fs::read_to_string(&a).unwrap(), "one");
    assert_eq!(fs::read_to_string(d.path().join("b")).unwrap(), "one");
}

#[test]
fn document_size_limit_is_five_megabytes() {
    let d = tempfile::tempdir().unwrap();
    let big = d.path().join("big");
    fs::write(&big, vec![b'a'; 5_000_001]).unwrap();
    assert!(Document::open(&big).is_err());
    let ok = d.path().join("ok");
    fs::write(&ok, vec![b'a'; 5_000_000]).unwrap();
    let mut doc = Document::open(&ok).unwrap();
    doc.text.push('b');
    assert!(doc.save().is_err(), "growing past 5 MB must be refused");
    assert_eq!(fs::metadata(&ok).unwrap().len(), 5_000_000);
}

#[test]
fn document_refuses_nul_binary_and_directories() {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("bin");
    fs::write(&bin, b"abc\0def").unwrap();
    assert!(Document::open(&bin).is_err());
    assert!(Document::open(d.path()).is_err());
    assert!(Document::open(&d.path().join("missing")).is_err());
}

#[test]
fn document_same_content_external_touch_still_saves() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f");
    fs::write(&p, "same").unwrap();
    let mut doc = Document::open(&p).unwrap();
    // Rewrite identical bytes (new mtime): the hash is unchanged so saving is allowed.
    fs::write(&p, "same").unwrap();
    assert!(!doc.dirty());
    doc.text = "edited".into();
    assert!(doc.dirty());
    doc.save().unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "edited");
    assert!(!doc.dirty());
    // A second save after our own write must still be accepted.
    doc.text = "edited again".into();
    doc.save().unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "edited again");
}

// ---------- model::State ----------

#[cfg(unix)]
#[test]
fn state_round_trips_through_disk() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("nested/dir/workspace.json");
    let mut s = State::default();
    s.preferences.theme = "starlight".into();
    s.preferences.font_size = 17.;
    let mut w = Workspace::new("Second", "/tmp".into());
    let target = w.tabs[0].layout.id().to_string();
    w.tabs[0].layout.split(
        &target,
        Pane::System {
            id: caelum_rs::model::id(),
        },
        Axis::Vertical,
    );
    s.workspaces.push(w);
    s.save(&path).unwrap();
    let back = State::read(&path).unwrap();
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        serde_json::to_value(&back).unwrap()
    );
    // Saving twice leaves no temp files behind.
    s.save(&path).unwrap();
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn state_validate_repairs_or_rejects() {
    let mut s = State::default();
    s.active_workspace = "nope".into();
    s.preferences.font_size = 500.;
    s.preferences.line_height = 0.;
    s.workspaces[0].active_tab = "nope".into();
    s.validate().unwrap();
    assert_eq!(s.active_workspace, s.workspaces[0].id);
    assert_eq!(s.workspaces[0].active_tab, s.workspaces[0].tabs[0].id);
    assert_eq!(s.preferences.font_size, 28.);
    assert_eq!(s.preferences.line_height, 1.);

    let mut bad = State::default();
    bad.version = 2;
    assert!(bad.validate().is_err());
    let mut empty = State::default();
    empty.workspaces.clear();
    assert!(empty.validate().is_err());
    let mut notabs = State::default();
    notabs.workspaces[0].tabs.clear();
    assert!(notabs.validate().is_err());
}

#[test]
fn state_validate_clamps_split_ratio_and_limits_depth() {
    let mut s = State::default();
    let layout = &mut s.workspaces[0].tabs[0].layout;
    if let Pane::Split { ratio, .. } = layout {
        *ratio = f32::NAN;
    }
    s.validate().unwrap();
    if let Pane::Split { ratio, .. } = &s.workspaces[0].tabs[0].layout {
        assert_eq!(*ratio, 0.5);
    } else {
        panic!("default layout should be a split");
    }

    let mut deep = Pane::System {
        id: caelum_rs::model::id(),
    };
    for _ in 0..40 {
        deep = Pane::Split {
            id: caelum_rs::model::id(),
            axis: Axis::Horizontal,
            ratio: 0.5,
            first: Box::new(deep),
            second: Box::new(Pane::System {
                id: caelum_rs::model::id(),
            }),
        };
    }
    let mut s = State::default();
    s.workspaces[0].tabs[0].layout = deep;
    assert!(s.validate().is_err());
}

#[test]
fn corrupt_workspace_files_are_errors_not_panics() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("workspace.json");
    let cases: Vec<&[u8]> = vec![
        b"",
        b"{",
        b"not json",
        b"[]",
        b"null",
        b"{\"version\":1}",
        b"{\"version\":1,\"workspaces\":[],\"activeWorkspace\":\"x\"}",
        b"{\"version\":99,\"workspaces\":[],\"activeWorkspace\":\"x\"}",
        b"\xff\xfe\x00garbage",
        b"{\"version\":1,\"activeWorkspace\":\"w\",\"workspaces\":[{\"id\":\"w\",\"name\":\"n\",\"activeTab\":\"t\",\"tabs\":[{\"id\":\"t\",\"name\":\"t\",\"layout\":{\"kind\":\"bogus\",\"id\":\"p\"}}]}]}",
    ];
    for (i, c) in cases.iter().enumerate() {
        fs::write(&p, c).unwrap();
        assert!(State::read(&p).is_err(), "case {i} should fail");
        // The corrupt file itself is left untouched for the user to inspect.
        assert_eq!(&fs::read(&p).unwrap(), c);
    }
    assert!(State::read(&d.path().join("absent.json")).is_err());
}

#[cfg(unix)]
#[test]
fn legacy_import_rejects_bad_ssh_host() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src.json");
    let dst = d.path().join("dst.json");
    let legacy = serde_json::json!({"version":1,"activeWorkspace":"w","workspaces":[{"id":"w","name":"L","activeTab":"t","tabs":[{"id":"t","name":"T","layout":{"id":"p","kind":"terminal","cwd":"/tmp","ssh":"-oProxyCommand=evil"}}]}]});
    fs::write(&src, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(State::import_legacy(&src, &dst).is_err());
    assert!(!dst.exists());
}

// ---------- themes ----------

#[test]
fn every_theme_preset_validates_and_is_self_consistent() {
    let presets = theme::presets();
    assert!(!presets.is_empty());
    assert!(presets.contains_key("starlight") && presets.contains_key("orbit"));
    for (id, t) in &presets {
        assert_eq!(&t.id, id);
        t.validate().unwrap_or_else(|e| panic!("{id}: {e}"));
        let json = serde_json::to_string(t).unwrap();
        let back: theme::Theme = serde_json::from_str(&json).unwrap();
        back.validate().unwrap();
    }
}

#[test]
fn broken_themes_fail_validation() {
    let mut t = theme();
    t.name = "  ".into();
    assert!(t.validate().is_err());
    let mut t = theme();
    t.kind = "sepia".into();
    assert!(t.validate().is_err());
    let mut t = theme();
    let key = t.ui.keys().next().unwrap().clone();
    t.ui.insert(key, "nothex".into());
    assert!(t.validate().is_err());
    let mut t = theme();
    let key = t.terminal.keys().next().unwrap().clone();
    t.terminal.remove(&key);
    assert!(t.validate().is_err());
}
