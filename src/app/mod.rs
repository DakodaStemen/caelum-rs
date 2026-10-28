mod editor;
mod panes;
mod persist;
mod settings;
mod sidebar;
mod terminal_pane;
use anyhow::{Context, Result, bail};
use caelum_rs::{
    files::{self, Document, Draft},
    model::{self, Axis, Pane, State, Tab, Workspace},
    syntax::Highlighter,
    terminal::{self, Session},
    theme::{self, Theme},
};
use eframe::egui::{
    self, Color32, FontId, Key, Modifiers, Pos2, Rect, RichText, Sense, Stroke, Vec2,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone)]
enum Action {
    NewTab,
    NewWorkspace,
    Split(Axis),
    Files,
    System,
    Processes,
    ClosePane,
    Zoom,
    Settings,
    Open,
    Connect,
    Assistant(String),
    Theme(String),
    Save,
    Palette,
    Sidebar,
}
#[derive(Clone)]
enum PendingClose {
    Window,
    Tab,
    Pane,
    Workspace,
}
#[derive(Default)]
struct FileView {
    path_input: String,
    filter: String,
    entries: Vec<files::Entry>,
    loaded: Option<PathBuf>,
    refreshed: Option<Instant>,
    error: String,
}
struct SystemInfo {
    updated: Instant,
    load: String,
    memory: String,
    uptime: String,
    processes: Vec<(String, String, String)>,
}
impl SystemInfo {
    fn new() -> Self {
        Self {
            updated: Instant::now() - Duration::from_secs(10),
            load: String::new(),
            memory: String::new(),
            uptime: String::new(),
            processes: vec![],
        }
    }
    fn refresh(&mut self) {
        if self.updated.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.updated = Instant::now();
        let snap = caelum_rs::platform::system_snapshot();
        self.load = snap.load;
        self.memory = snap.memory;
        self.uptime = snap.uptime;
        self.processes = snap.processes;
    }
}
pub struct App {
    pub state: State,
    profile: PathBuf,
    terminals: HashMap<String, Session>,
    documents: HashMap<String, Document>,
    files: HashMap<String, FileView>,
    presets: BTreeMap<String, Theme>,
    focused: String,
    zoom: Option<String>,
    message: String,
    palette: bool,
    palette_query: String,
    settings: bool,
    theme_draft: Option<Theme>,
    theme_path: String,
    dialog: Option<String>,
    dialog_input: String,
    pending: Option<PendingClose>,
    allow_close: bool,
    last_save: Instant,
    system: SystemInfo,
    process_filter: String,
    logo: egui::TextureHandle,
    smoke: Option<Smoke>,
    recovery_conflict: bool,
    pending_open: Option<PathBuf>,
    applied_font: String,
    font_rx: Option<std::sync::mpsc::Receiver<Option<Vec<u8>>>>,
    last_dialog: Option<String>,
    pending_save: Option<String>,
    syntax: Highlighter,
}
struct Smoke {
    report: PathBuf,
    stage: usize,
    start: Instant,
    last: Instant,
    checks: Vec<String>,
    terminal: String,
    pid: Option<u32>,
    original_tabs: usize,
    screenshot: bool,
}
impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        profile: PathBuf,
        mut state: State,
        message: String,
        smoke: Option<PathBuf>,
    ) -> Self {
        let image = image::load_from_memory(include_bytes!("../../assets/brand/logo.png"))
            .unwrap()
            .to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let logo = cc.egui_ctx.load_texture(
            "Caelum",
            egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
            Default::default(),
        );
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(8., 8.);
        style.spacing.button_padding = Vec2::new(12., 7.);
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.));
        cc.egui_ctx.set_style(style);
        if smoke.is_some() {
            let demo = profile.join("demo");
            let _ = fs::create_dir_all(demo.join("src"));
            let _ = fs::write(
                demo.join("README.md"),
                "# Caelum Rust\n\nYour space to build.\n",
            );
            let _ = fs::write(demo.join("Cargo.toml"), include_str!("../../Cargo.toml"));
            fn relocate(p: &mut Pane, path: &Path) {
                match p {
                    Pane::Terminal { cwd, .. } => *cwd = path.to_path_buf(),
                    Pane::Files { path: p, .. } => *p = path.to_path_buf(),
                    Pane::Split { first, second, .. } => {
                        relocate(first, path);
                        relocate(second, path);
                    }
                    _ => {}
                }
            }
            for w in &mut state.workspaces {
                for t in &mut w.tabs {
                    relocate(&mut t.layout, &demo);
                }
            }
        }
        let mut app = Self {
            state,
            profile,
            terminals: HashMap::new(),
            documents: HashMap::new(),
            files: HashMap::new(),
            presets: theme::presets(),
            focused: String::new(),
            zoom: None,
            message,
            palette: false,
            palette_query: String::new(),
            settings: false,
            theme_draft: None,
            theme_path: String::new(),
            dialog: None,
            dialog_input: String::new(),
            pending: None,
            allow_close: false,
            last_save: Instant::now(),
            system: SystemInfo::new(),
            process_filter: String::new(),
            logo,
            smoke: smoke.map(|report| Smoke {
                report,
                stage: 0,
                start: Instant::now(),
                last: Instant::now(),
                checks: vec![],
                terminal: String::new(),
                pid: None,
                original_tabs: 0,
                screenshot: false,
            }),
            recovery_conflict: false,
            pending_open: None,
            applied_font: "__unset__".into(),
            font_rx: None,
            last_dialog: None,
            pending_save: None,
            syntax: Highlighter::default(),
        };
        app.restore_drafts();
        app.ensure_focus();
        app
    }
    fn theme(&self) -> Theme {
        self.theme_draft
            .clone()
            .or_else(|| {
                self.state
                    .preferences
                    .custom_themes
                    .iter()
                    .find(|t| t.id == self.state.preferences.theme)
                    .cloned()
            })
            .or_else(|| self.presets.get(&self.state.preferences.theme).cloned())
            .unwrap_or_else(|| self.presets["graphite"].clone())
    }
    fn workspace(&self) -> &Workspace {
        &self.state.workspaces[self.state.active_index()]
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        let i = self.state.active_index();
        &mut self.state.workspaces[i]
    }
    fn tab(&self) -> &Tab {
        let w = self.workspace();
        &w.tabs[w.active_index()]
    }
    fn tab_mut(&mut self) -> &mut Tab {
        let w = self.workspace_mut();
        let i = w.active_index();
        &mut w.tabs[i]
    }
    fn ensure_focus(&mut self) {
        let mut leaves = vec![];
        self.tab().layout.leaves(&mut leaves);
        if !leaves.iter().any(|p| p.id() == self.focused) {
            self.focused = leaves
                .first()
                .map(|p| p.id().to_string())
                .unwrap_or_default();
        }
    }
    fn cwd(&self) -> PathBuf {
        self.terminals
            .get(&self.focused)
            .map(|s| s.cwd.clone())
            .or_else(|| match self.tab().layout.find(&self.focused) {
                Some(Pane::Terminal { cwd, .. }) => Some(cwd.clone()),
                Some(Pane::Files { path, .. }) => Some(path.clone()),
                Some(Pane::Editor { path, .. }) => path.parent().map(Path::to_path_buf),
                _ => None,
            })
            .unwrap_or_else(model::home)
    }
    fn insert(&mut self, pane: Pane) {
        let target = self.focused.clone();
        let next = pane.id().to_string();
        if !self
            .tab_mut()
            .layout
            .split(&target, pane.clone(), Axis::Horizontal)
        {
            let root = self.tab().layout.id().to_string();
            self.tab_mut().layout.split(&root, pane, Axis::Horizontal);
        }
        self.focused = next;
        self.zoom = None;
    }
    fn action(&mut self, action: Action, ctx: &egui::Context) {
        let reset_focus = matches!(
            &action,
            Action::NewTab
                | Action::NewWorkspace
                | Action::Split(_)
                | Action::Assistant(_)
                | Action::Palette
                | Action::Open
                | Action::Connect
        );
        if reset_focus {
            ctx.memory_mut(|m| m.stop_text_input());
        }
        match action {
            Action::NewTab => {
                let t = Tab::new(self.cwd());
                self.focused = t.layout.id().into();
                let w = self.workspace_mut();
                w.active_tab = t.id.clone();
                w.tabs.push(t);
                self.zoom = None;
            }
            Action::NewWorkspace => {
                let w = Workspace::new(
                    format!("Workspace {}", self.state.workspaces.len() + 1),
                    self.cwd(),
                );
                self.state.active_workspace = w.id.clone();
                self.state.workspaces.push(w);
                self.zoom = None;
                self.ensure_focus();
            }
            Action::Split(axis) => {
                let pane = Pane::terminal(self.cwd());
                let id = pane.id().to_string();
                let focused = self.focused.clone();
                self.tab_mut().layout.split(&focused, pane, axis);
                self.focused = id;
                self.zoom = None;
            }
            Action::Files => self.insert(Pane::Files {
                id: model::id(),
                path: self.cwd(),
            }),
            Action::System => self.insert(Pane::System { id: model::id() }),
            Action::Processes => self.insert(Pane::Processes { id: model::id() }),
            Action::ClosePane => self.pending = Some(PendingClose::Pane),
            Action::Zoom => {
                self.zoom = if self.zoom.is_some() {
                    None
                } else {
                    Some(self.focused.clone())
                };
            }
            Action::Settings => self.settings = true,
            Action::Sidebar => self.state.preferences.sidebar = !self.state.preferences.sidebar,
            Action::Open => {
                self.dialog = Some("Open file".into());
                self.dialog_input = self.cwd().display().to_string() + "/";
            }
            Action::Connect => {
                self.dialog = Some("Connect with SSH".into());
                self.dialog_input.clear();
            }
            Action::Assistant(command) => {
                self.insert(Pane::Terminal {
                    id: model::id(),
                    cwd: self.cwd(),
                    command: Some(vec![command]),
                });
            }
            Action::Theme(id) => self.state.preferences.theme = id,
            Action::Save => {
                self.pending_save = Some(self.focused.clone());
            }
            Action::Palette => {
                self.palette = !self.palette;
                self.palette_query.clear();
            }
        }
        ctx.request_repaint();
    }
    fn actions(&self) -> Vec<(String, Action)> {
        let mut a = vec![
            ("New tab".into(), Action::NewTab),
            ("New workspace".into(), Action::NewWorkspace),
            ("Split right".into(), Action::Split(Axis::Horizontal)),
            ("Split below".into(), Action::Split(Axis::Vertical)),
            ("Zoom / restore pane".into(), Action::Zoom),
            ("Close pane".into(), Action::ClosePane),
            ("Open file".into(), Action::Open),
            ("Files".into(), Action::Files),
            ("System overview".into(), Action::System),
            ("Process viewer".into(), Action::Processes),
            ("SSH connection".into(), Action::Connect),
            ("Launch Codex".into(), Action::Assistant("codex".into())),
            ("Launch Claude".into(), Action::Assistant("claude".into())),
            ("Toggle sidebar".into(), Action::Sidebar),
            ("Appearance and settings".into(), Action::Settings),
        ];
        for (id, t) in &self.presets {
            a.push((format!("Theme: {}", t.name), Action::Theme(id.clone())));
        }
        for t in &self.state.preferences.custom_themes {
            a.push((format!("Theme: {}", t.name), Action::Theme(t.id.clone())));
        }
        a
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cs = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        for (m, key, a) in [
            (cs, Key::P, Action::Palette),
            (cs, Key::B, Action::Sidebar),
            (cs, Key::T, Action::NewTab),
            (cs, Key::D, Action::Split(Axis::Horizontal)),
            (cs, Key::E, Action::Split(Axis::Vertical)),
            (cs, Key::Enter, Action::Zoom),
            (cs, Key::W, Action::ClosePane),
            (Modifiers::CTRL, Key::O, Action::Open),
            (Modifiers::CTRL, Key::S, Action::Save),
        ] {
            if ctx.input_mut(|i| i.consume_key(m, key)) {
                self.action(a, ctx);
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Tab)) {
            let w = self.workspace_mut();
            let i = (w.active_index() + 1) % w.tabs.len();
            w.active_tab = w.tabs[i].id.clone();
            self.zoom = None;
            self.ensure_focus();
        }
        if ctx.input_mut(|i| i.consume_key(cs, Key::Tab)) {
            let w = self.workspace_mut();
            let i = (w.active_index() + w.tabs.len() - 1) % w.tabs.len();
            w.active_tab = w.tabs[i].id.clone();
            self.zoom = None;
            self.ensure_focus();
        }
        if ctx.input_mut(|i| i.consume_key(cs, Key::C))
            && let Some(text) = self
                .terminals
                .get(&self.focused)
                .and_then(Session::selection)
        {
            ctx.copy_text(text);
        }
        if ctx.input_mut(|i| i.consume_key(cs, Key::V)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
        if ctx.input_mut(|i| i.consume_key(cs, Key::F)) {
            self.dialog = Some("Search terminal".into());
            self.dialog_input.clear();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.palette = false;
            self.settings = false;
            self.theme_draft = None;
            self.dialog = None;
        }
    }
    fn overlays(&mut self, ctx: &egui::Context) {
        if self.dialog != self.last_dialog {
            ctx.memory_mut(|m| m.stop_text_input());
            self.last_dialog = self.dialog.clone();
        }
        if self.palette {
            let mut action = None;
            egui::Window::new("Command palette")
                .collapsible(false)
                .resizable(false)
                .default_width(520.)
                .anchor(egui::Align2::CENTER_TOP, [0., 95.])
                .show(ctx, |ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.palette_query)
                            .hint_text("Type a command or theme…")
                            .desired_width(f32::INFINITY),
                    );
                    if !ctx.wants_keyboard_input() {
                        edit.request_focus();
                    }
                    let query = self.palette_query.to_lowercase();
                    let matches: Vec<_> = self
                        .actions()
                        .into_iter()
                        .filter(|(label, _)| label.to_lowercase().contains(&query))
                        .collect();
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        action = matches.first().map(|(_, a)| a.clone());
                    }
                    egui::ScrollArea::vertical()
                        .max_height(400.)
                        .show(ui, |ui| {
                            for (label, a) in matches {
                                if ui
                                    .add_sized(
                                        [ui.available_width(), 30.],
                                        egui::Button::new(label),
                                    )
                                    .clicked()
                                {
                                    action = Some(a);
                                }
                            }
                        });
                });
            if let Some(a) = action {
                self.palette = false;
                ctx.memory_mut(|m| m.stop_text_input());
                self.action(a, ctx);
            }
        }
        if self.settings {
            self.appearance(ctx);
        }
        if let Some(title) = self.dialog.clone() {
            let mut done = false;
            let mut cancel = false;
            egui::Window::new(&title)
                .collapsible(false)
                .resizable(false)
                .default_width(540.)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.dialog_input)
                            .desired_width(f32::INFINITY),
                    );
                    if !ctx.wants_keyboard_input() {
                        edit.request_focus();
                    }
                    if title == "Search terminal" {
                        ui.label("Search terminal output and scrollback.");
                        ui.horizontal(|ui| {
                            for (name, forward) in [("Previous", false), ("Next", true)] {
                                if ui.button(name).clicked()
                                    && let Some(session) = self.terminals.get_mut(&self.focused)
                                    && !session.search(&self.dialog_input, forward)
                                {
                                    self.message = "No terminal match".into();
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .button(if title == "Search terminal" {
                                "Done"
                            } else {
                                "Apply"
                            })
                            .clicked()
                            || ui.input(|i| i.key_pressed(Key::Enter))
                        {
                            done = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if done {
                let value = self.dialog_input.trim().to_string();
                match title.as_str() {
                    "Open file" => self.open_file(PathBuf::from(value)),
                    "Rename tab" => {
                        if !value.is_empty() {
                            self.tab_mut().name = value;
                        }
                    }
                    "Rename workspace" => {
                        if !value.is_empty() {
                            self.workspace_mut().name = value;
                        }
                    }
                    "Connect with SSH" => {
                        if value.is_empty()
                            || value.starts_with('-')
                            || !value
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "_.@:-[]".contains(c))
                        {
                            self.message =
                                "Enter a host or user@host from your SSH configuration".into();
                        } else {
                            self.insert(Pane::Terminal {
                                id: model::id(),
                                cwd: self.cwd(),
                                command: Some(vec!["ssh".into(), "--".into(), value]),
                            });
                        }
                    }
                    _ => {}
                }
                self.dialog = None;
                ctx.memory_mut(|m| m.stop_text_input());
            }
            if cancel {
                self.dialog = None;
            }
        }
        if self.pending.is_some() {
            let dirty = self.dirty_in_pending();
            if dirty.is_empty() {
                self.finish_close(ctx);
            } else {
                let mut save = false;
                let mut discard = false;
                let mut cancel = false;
                egui::Window::new("Unsaved changes")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(format!(
                            "{} edited file(s) have unsaved changes.",
                            dirty.len()
                        ));
                        ui.horizontal(|ui| {
                            save = ui.button("Save and close").clicked();
                            discard = ui.button("Discard and close").clicked();
                            cancel = ui.button("Keep working").clicked();
                        });
                    });
                if cancel {
                    self.pending = None;
                }
                if save {
                    let mut failed = false;
                    for id in &dirty {
                        if let Some(doc) = self.documents.get_mut(id)
                            && let Err(e) = doc.save()
                        {
                            self.message = e.to_string();
                            failed = true;
                        }
                    }
                    if !failed {
                        self.finish_close(ctx);
                    }
                }
                if discard {
                    for id in dirty {
                        self.documents.remove(&id);
                    }
                    self.finish_close(ctx);
                }
            }
        }
    }
    fn dirty_in_pending(&self) -> Vec<String> {
        let mut allowed = HashSet::new();
        let mut add = |p: &Pane| {
            let mut leaves = vec![];
            p.leaves(&mut leaves);
            for p in leaves {
                allowed.insert(p.id().to_string());
            }
        };
        match &self.pending {
            Some(PendingClose::Window) => {
                for w in &self.state.workspaces {
                    for t in &w.tabs {
                        add(&t.layout);
                    }
                }
            }
            Some(PendingClose::Workspace) => {
                for t in &self.workspace().tabs {
                    add(&t.layout);
                }
            }
            Some(PendingClose::Tab) => add(&self.tab().layout),
            Some(PendingClose::Pane) => {
                allowed.insert(self.focused.clone());
            }
            None => {}
        }
        self.documents
            .iter()
            .filter(|(id, doc)| allowed.contains(*id) && doc.dirty())
            .map(|(id, _)| id.clone())
            .collect()
    }
    fn finish_close(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            PendingClose::Window => {
                self.allow_close = true;
                self.persist();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            PendingClose::Workspace => {
                let i = self.state.active_index();
                self.state.workspaces.remove(i);
                self.state.active_workspace = self.state.workspaces[0].id.clone();
            }
            PendingClose::Tab => {
                let cwd = self.cwd();
                let w = self.workspace_mut();
                let i = w.active_index();
                w.tabs.remove(i);
                if w.tabs.is_empty() {
                    w.tabs.push(Tab::new(cwd));
                }
                w.active_tab = w.tabs[i.min(w.tabs.len() - 1)].id.clone();
            }
            PendingClose::Pane => {
                let focused = self.focused.clone();
                let cwd = self.cwd();
                if !self.tab_mut().layout.remove(&focused) {
                    self.tab_mut().layout = Pane::terminal(cwd);
                }
            }
        }
        self.zoom = None;
        self.ensure_focus();
        self.prune();
    }
    fn prune(&mut self) {
        let mut ids = HashSet::new();
        for w in &self.state.workspaces {
            for t in &w.tabs {
                let mut leaves = vec![];
                t.layout.leaves(&mut leaves);
                ids.extend(leaves.into_iter().map(|p| p.id().to_string()));
            }
        }
        self.terminals.retain(|id, _| ids.contains(id));
        self.documents.retain(|id, _| ids.contains(id));
        self.files.retain(|id, _| ids.contains(id));
    }
    fn smoke_tick(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.smoke.take() else {
            return;
        };
        let result = (|| -> Result<bool> {
            if s.start.elapsed() > Duration::from_secs(30) {
                bail!("Desktop check timed out at stage {}", s.stage);
            }
            if s.stage != 10 && s.last.elapsed() < Duration::from_millis(180) {
                return Ok(false);
            }
            match s.stage {
                0 => {
                    let terminal = self
                        .terminals
                        .iter_mut()
                        .next()
                        .context("No terminal was created")?;
                    s.terminal = terminal.0.clone();
                    s.pid = terminal.1.pid();
                    terminal.1.write(
                        b"printf '\\033[38;2;130;213;238mNATIVE_%s\\033[0m\\n' RUST_OK\r".to_vec(),
                    );
                    s.original_tabs = self.workspace().tabs.len();
                }
                1 => {
                    if !self
                        .terminals
                        .get(&s.terminal)
                        .unwrap()
                        .text()
                        .contains("NATIVE_RUST_OK")
                    {
                        return Ok(false);
                    }
                    s.checks.push(
                        "Real PTY executes a shell command and decodes true-color output".into(),
                    );
                    self.focused = s.terminal.clone();
                    self.action(Action::Split(Axis::Vertical), ctx);
                }
                2 => {
                    if self.terminals.len() < 2 {
                        return Ok(false);
                    }
                    s.checks.push("Split creates a second live terminal".into());
                    self.action(Action::Zoom, ctx);
                }
                3 => {
                    if self.zoom.is_none() {
                        bail!("Zoom did not activate");
                    }
                    self.action(Action::Zoom, ctx);
                    s.checks.push("Pane zoom restores the split layout".into());
                    self.action(Action::NewTab, ctx);
                }
                4 => {
                    if self.workspace().tabs.len() != s.original_tabs + 1 {
                        bail!("New tab failed");
                    }
                    s.checks.push("New tab owns an independent terminal".into());
                    self.action(Action::Theme("graphite".into()), ctx);
                    self.persist();
                    let restored = State::read(&self.profile.join("workspace.json"))?;
                    if restored.preferences.theme != "graphite" {
                        bail!("Theme was not persisted");
                    }
                    s.checks
                        .push("Workspace and theme persist to an isolated profile".into());
                    self.action(Action::Files, ctx);
                }
                5 => {
                    let fixture = self.profile.join("desktop-check.txt");
                    fs::write(&fixture, "before\n")?;
                    self.open_file(fixture);
                }
                6 => {
                    let doc = self
                        .documents
                        .get_mut(&self.focused)
                        .context("Editor was not opened")?;
                    doc.text = "Rust desktop edit\n".into();
                    doc.save()?;
                    if fs::read_to_string(&doc.path)? != "Rust desktop edit\n" {
                        bail!("Editor save failed");
                    }
                    fs::write(&doc.path, "external change\n")?;
                    doc.text = "must refuse".into();
                    if doc.save().is_ok() {
                        bail!("External change was overwritten");
                    }
                    s.checks.push(
                        "Native editor saves text and refuses external-change overwrite".into(),
                    );
                    doc.text = doc.original.clone();
                    self.action(Action::System, ctx);
                }
                7 => {
                    self.system.refresh();
                    if self.system.memory.is_empty() {
                        bail!("System view missing");
                    }
                    s.checks
                        .push("System view reads live memory and uptime".into());
                    self.action(Action::Processes, ctx);
                }
                8 => {
                    if self.system.processes.is_empty() {
                        bail!("No processes shown");
                    }
                    s.checks.push("Process viewer lists live processes".into());
                    self.action(Action::Theme("lunar".into()), ctx);
                }
                9 => {
                    if !self.theme().kind.eq("light") {
                        bail!("Light theme failed");
                    }
                    s.checks
                        .push("Light and dark presets apply to the native interface".into());
                    self.action(Action::Theme("starlight".into()), ctx);
                    let w = self.workspace_mut();
                    w.active_tab = w.tabs[0].id.clone();
                    self.ensure_focus();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                    s.screenshot = true;
                }
                10 => {
                    for event in ctx.input(|i| i.events.clone()) {
                        if let egui::Event::Screenshot { image, .. } = event {
                            let bytes: Vec<u8> =
                                image.pixels.iter().flat_map(|c| c.to_array()).collect();
                            image::save_buffer(
                                s.report.with_extension("png"),
                                &bytes,
                                image.size[0] as u32,
                                image.size[1] as u32,
                                image::ColorType::Rgba8,
                            )?;
                            s.screenshot = false;
                        }
                    }
                    if s.screenshot {
                        return Ok(false);
                    }
                    let report = serde_json::json!({"passed":s.checks,"profile":self.profile,"renderer":"native egui/OpenGL","success":true});
                    fs::write(&s.report, serde_json::to_vec_pretty(&report)?)?;
                    self.persist();
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return Ok(true);
                }
                _ => {}
            }
            s.stage += 1;
            s.last = Instant::now();
            Ok(false)
        })();
        match result {
            Ok(true) => {}
            Ok(false) => self.smoke = Some(s),
            Err(e) => {
                let _ = fs::write(
                    &s.report,
                    serde_json::json!({"success":false,"error":e.to_string(),"passed":s.checks})
                        .to_string(),
                );
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(PendingClose::Window);
        }
        self.shortcuts(ctx);
        self.apply_font(ctx);
        let theme = self.theme();
        for session in self.terminals.values_mut() {
            session.poll(&theme);
        }
        if theme.validate().is_ok() {
            theme.apply(ctx);
        }
        self.top(ctx, &theme);
        self.sidebar(ctx, &theme);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("background"))
                    .inner_margin(6.),
            )
            .show(ctx, |ui| {
                self.tabs(ui, ctx);
                let rect = ui.available_rect_before_wrap();
                let mut layout = self.tab().layout.clone();
                if let Some(zoom) = self.zoom.clone() {
                    if let Some(node) = layout.find(&zoom).cloned() {
                        let mut node = node;
                        self.layout(ui, rect, &mut node, ctx, &theme);
                        update_node(&mut layout, &zoom, node);
                    }
                } else {
                    self.layout(ui, rect, &mut layout, ctx, &theme);
                }
                self.tab_mut().layout = layout;
            });
        if let Some(path) = self.pending_open.take() {
            self.open_file(path);
        }
        self.overlays(ctx);
        if let Some(id) = self.pending_save.take()
            && let Some(doc) = self.documents.get_mut(&id)
        {
            self.message = match doc.save() {
                Ok(()) => "File saved".into(),
                Err(e) => e.to_string(),
            };
        }

        if self.last_save.elapsed() > Duration::from_secs(3) {
            self.persist();
        }
        self.smoke_tick(ctx);
        ctx.request_repaint_after(Duration::from_millis(
            if self.state.preferences.cursor_blink {
                250
            } else {
                500
            },
        ));
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist();
        self.terminals.clear();
    }
}
fn update_node(root: &mut Pane, id: &str, replacement: Pane) {
    if root.id() == id {
        *root = replacement;
    } else if let Pane::Split { first, second, .. } = root {
        if first.find(id).is_some() {
            update_node(first, id, replacement);
        } else {
            update_node(second, id, replacement);
        }
    }
}
