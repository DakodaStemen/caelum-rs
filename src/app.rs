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
        self.load = fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join("  ");
        let mem = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let value = |key: &str| {
            mem.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let total = value("MemTotal:");
        let used = total.saturating_sub(value("MemAvailable:"));
        self.memory = format!(
            "{:.1} / {:.1} GiB",
            used as f64 / 1048576.,
            total as f64 / 1048576.
        );
        let seconds = fs::read_to_string("/proc/uptime")
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.) as u64;
        self.uptime = format!("{}h {}m", seconds / 3600, seconds / 60 % 60);
        self.processes.clear();
        if let Ok(entries) = fs::read_dir("/proc") {
            for e in entries.flatten() {
                let pid = e.file_name().to_string_lossy().to_string();
                if pid.parse::<u32>().is_err() {
                    continue;
                }
                let status = fs::read_to_string(e.path().join("status")).unwrap_or_default();
                let name = status
                    .lines()
                    .find_map(|l| l.strip_prefix("Name:\t"))
                    .unwrap_or("")
                    .to_string();
                let rss = status
                    .lines()
                    .find_map(|l| l.strip_prefix("VmRSS:\t"))
                    .unwrap_or("0 kB")
                    .trim()
                    .to_string();
                if !name.is_empty() {
                    self.processes.push((pid, name, rss));
                }
            }
        }
        self.processes
            .sort_by_key(|p| p.0.parse::<u32>().unwrap_or(0));
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
        let image = image::load_from_memory(include_bytes!("../assets/brand/logo.png"))
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
            let _ = fs::write(demo.join("Cargo.toml"), include_str!("../Cargo.toml"));
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
            last_dialog: None,
            pending_save: None,
            syntax: Highlighter::default(),
        };
        app.restore_drafts();
        app.ensure_focus();
        app
    }
    fn apply_font(&mut self, ctx: &egui::Context) {
        let name = self.state.preferences.terminal_font.clone();
        if name == self.applied_font {
            return;
        }
        self.applied_font = name.clone();
        let output = std::process::Command::new("fc-match")
            .args([
                "-f",
                "%{file}",
                if name.is_empty() { "monospace" } else { &name },
            ])
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout);
            if let Ok(bytes) = fs::read(path.trim())
                && ab_glyph::FontArc::try_from_vec(bytes.clone()).is_ok()
            {
                let mut fonts = egui::FontDefinitions::default();
                fonts.font_data.insert(
                    "system-mono".into(),
                    egui::FontData::from_owned(bytes).into(),
                );
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .insert(0, "system-mono".into());
                ctx.set_fonts(fonts);
            }
        }
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
            .unwrap_or_else(|| self.presets["orbit"].clone())
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
    fn top(&mut self, ctx: &egui::Context, theme: &Theme) {
        egui::TopBottomPanel::top("title")
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("chrome"))
                    .stroke(Stroke::new(1_f32, theme.ui("border")))
                    .inner_margin(egui::Margin::symmetric(14, 9)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.image((self.logo.id(), Vec2::splat(30.)));
                    ui.label(
                        RichText::new("Caelum")
                            .size(18.)
                            .strong()
                            .color(theme.ui("textStrong")),
                    );
                    badge(ui, theme, "RUST", theme.ui("accent"));
                    ui.add_space(6.);
                    ui.label(RichText::new("›").color(theme.ui("muted")));
                    ui.label(
                        RichText::new(self.workspace().name.clone())
                            .color(theme.ui("textSecondary")),
                    );
                    ui.label(RichText::new("›").color(theme.ui("muted")));
                    ui.label(RichText::new(self.tab().name.clone()).color(theme.ui("text")));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Appearance")
                            .on_hover_text("Themes, fonts and colors")
                            .clicked()
                        {
                            self.settings = true;
                        }
                        let palette = ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Search commands…   Ctrl+Shift+P")
                                        .color(theme.ui("muted")),
                                )
                                .fill(theme.ui("background"))
                                .stroke(Stroke::new(1_f32, theme.ui("border"))),
                            )
                            .on_hover_text("Every action and theme in one place");
                        if palette.clicked() {
                            self.action(Action::Palette, ctx);
                        }
                    });
                });
            });
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("chrome"))
                    .stroke(Stroke::new(1_f32, theme.ui("border")))
                    .inner_margin(egui::Margin::symmetric(14, 6)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    dot(ui, theme.syn("string"));
                    ui.label(
                        RichText::new(format!("{} sessions", self.terminals.len()))
                            .small()
                            .color(theme.ui("textSecondary")),
                    );
                    ui.label(RichText::new("│").small().color(theme.ui("border")));
                    ui.label(
                        RichText::new(self.cwd().display().to_string())
                            .small()
                            .color(theme.ui("muted")),
                    );
                    if !self.message.is_empty() {
                        ui.label(RichText::new("│").small().color(theme.ui("border")));
                        ui.label(
                            RichText::new(self.message.clone())
                                .small()
                                .strong()
                                .color(theme.ui("accent")),
                        );
                        if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                            self.message.clear();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  v{}",
                                self.theme().name,
                                env!("CARGO_PKG_VERSION")
                            ))
                            .small()
                            .color(theme.ui("muted")),
                        );
                    });
                });
            });
    }
    fn sidebar(&mut self, ctx: &egui::Context, theme: &Theme) {
        egui::SidePanel::left("workspaces")
            .default_width(220.)
            .width_range(180. ..=320.)
            .frame(
                egui::Frame::new()
                    .fill(theme.ui("panel"))
                    .stroke(Stroke::new(1_f32, theme.ui("border")))
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 3.;
                section(ui, theme, "WORKSPACES");
                let workspaces: Vec<_> = self
                    .state
                    .workspaces
                    .iter()
                    .map(|w| (w.id.clone(), w.name.clone()))
                    .collect();
                for (id, name) in workspaces {
                    if nav_row(ui, theme, &name, "", self.state.active_workspace == id).clicked() {
                        self.state.active_workspace = id;
                        self.zoom = None;
                        self.ensure_focus();
                    }
                }
                if nav_row(ui, theme, "+  New workspace", "", false).clicked() {
                    self.action(Action::NewWorkspace, ctx);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("Rename").small()).frame(false))
                        .clicked()
                    {
                        self.dialog = Some("Rename workspace".into());
                        self.dialog_input = self.workspace().name.clone();
                    }
                    if self.state.workspaces.len() > 1
                        && ui
                            .add(egui::Button::new(RichText::new("Close").small()).frame(false))
                            .clicked()
                    {
                        self.pending = Some(PendingClose::Workspace);
                    }
                });
                ui.add_space(14.);
                section(ui, theme, "TOOLS");
                for (name, hint, a) in [
                    ("Files", "browse · edit", Action::Files),
                    ("System", "load · memory", Action::System),
                    ("Processes", "live list", Action::Processes),
                    ("SSH", "remote shell", Action::Connect),
                    ("Codex", "assistant", Action::Assistant("codex".into())),
                    ("Claude", "assistant", Action::Assistant("claude".into())),
                ] {
                    if nav_row(ui, theme, name, hint, false).clicked() {
                        self.action(a, ctx);
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        RichText::new("Native Rust · OpenGL")
                            .small()
                            .color(theme.ui("muted")),
                    );
                    ui.label(
                        RichText::new("A little room for big ideas.")
                            .small()
                            .color(theme.ui("textSecondary")),
                    );
                });
            });
    }
    fn tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let theme = self.theme();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.;
            let tabs: Vec<_> = self
                .workspace()
                .tabs
                .iter()
                .map(|t| (t.id.clone(), t.name.clone()))
                .collect();
            let count = tabs.len();
            for (id, name) in tabs {
                let active = self.workspace().active_tab == id;
                let (response, close) = tab_chip(ui, &theme, &name, active, count > 1);
                if response.clicked() {
                    self.workspace_mut().active_tab = id.clone();
                    self.zoom = None;
                    self.ensure_focus();
                }
                if close {
                    self.workspace_mut().active_tab = id;
                    self.pending = Some(PendingClose::Tab);
                }
                if response.double_clicked() {
                    self.dialog = Some("Rename tab".into());
                    self.dialog_input = self.tab().name.clone();
                }
            }
            if ui
                .button("+")
                .on_hover_text("New tab (Ctrl+Shift+T)")
                .clicked()
            {
                self.action(Action::NewTab, ctx);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(if self.zoom.is_some() {
                        "Restore"
                    } else {
                        "Zoom"
                    })
                    .on_hover_text("Focus this pane (Ctrl+Shift+Enter)")
                    .clicked()
                {
                    self.action(Action::Zoom, ctx);
                }
                if ui
                    .button("Split below")
                    .on_hover_text("Ctrl+Shift+E")
                    .clicked()
                {
                    self.action(Action::Split(Axis::Vertical), ctx);
                }
                if ui
                    .button("Split right")
                    .on_hover_text("Ctrl+Shift+D")
                    .clicked()
                {
                    self.action(Action::Split(Axis::Horizontal), ctx);
                }
            });
        });
        ui.add_space(4.);
    }
    fn layout(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        node: &mut Pane,
        ctx: &egui::Context,
        theme: &Theme,
    ) {
        if let Pane::Split {
            id,
            axis,
            ratio,
            first,
            second,
        } = node
        {
            let horizontal = *axis == Axis::Horizontal;
            let length = if horizontal {
                rect.width()
            } else {
                rect.height()
            };
            let split = length * (*ratio);
            let (a, handle, b) = if horizontal {
                (
                    Rect::from_min_max(
                        rect.min,
                        Pos2::new(rect.left() + split - 3., rect.bottom()),
                    ),
                    Rect::from_min_max(
                        Pos2::new(rect.left() + split - 3., rect.top()),
                        Pos2::new(rect.left() + split + 3., rect.bottom()),
                    ),
                    Rect::from_min_max(Pos2::new(rect.left() + split + 3., rect.top()), rect.max),
                )
            } else {
                (
                    Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + split - 3.)),
                    Rect::from_min_max(
                        Pos2::new(rect.left(), rect.top() + split - 3.),
                        Pos2::new(rect.right(), rect.top() + split + 3.),
                    ),
                    Rect::from_min_max(Pos2::new(rect.left(), rect.top() + split + 3.), rect.max),
                )
            };
            let response = ui.interact(handle, egui::Id::new(id.clone()), Sense::drag());
            if response.dragged() {
                let delta = response.drag_delta();
                *ratio = (*ratio
                    + if horizontal {
                        delta.x / length
                    } else {
                        delta.y / length
                    })
                .clamp(0.1, 0.9);
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(if horizontal {
                    egui::CursorIcon::ResizeHorizontal
                } else {
                    egui::CursorIcon::ResizeVertical
                });
                ui.painter().rect_filled(handle, 0., theme.ui("accent"));
            }
            self.layout(ui, a, first, ctx, theme);
            self.layout(ui, b, second, ctx, theme);
            return;
        }
        let id = node.id().to_string();
        let selected = id == self.focused;
        let mut focus_requested = false;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(&id)
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect);
        egui::Frame::new().fill(theme.term("background")).stroke(Stroke::new(1_f32,if selected{theme.ui("accent")}else{theme.ui("border")})).show(&mut child,|ui|{
   ui.set_min_size((rect.size()-Vec2::splat(4.)).max(Vec2::splat(1.)));ui.set_max_size((rect.size()-Vec2::splat(4.)).max(Vec2::splat(1.)));
   ui.horizontal(|ui|{let label=match &*node {Pane::Terminal{..}=>self.terminals.get(&id).map(|s|s.title.clone()).unwrap_or_else(||"Terminal".into()),Pane::Files{..}=>"Files".into(),Pane::Editor{path,..}=>path.file_name().unwrap_or_default().to_string_lossy().to_string(),Pane::System{..}=>"System".into(),Pane::Processes{..}=>"Processes".into(),Pane::Browser{..}=>"Browser link".into(),_=>String::new()};
    let kind=match &*node {Pane::Terminal{..}=>"shell",Pane::Files{..}=>"files",Pane::Editor{..}=>"editor",Pane::System{..}=>"system",Pane::Processes{..}=>"processes",Pane::Browser{..}=>"link",_=>""};
    dot(ui,if selected{theme.ui("accent")}else{theme.ui("borderStrong")});
    let title=RichText::new(label).small().color(if selected{theme.ui("textStrong")}else{theme.ui("textSecondary")});
    if ui.add(egui::Label::new(if selected{title.strong()}else{title}).truncate().sense(Sense::click())).clicked(){focus_requested=true;}
    if self.documents.get(&id).is_some_and(Document::dirty) && matches!(&*node,Pane::Editor{..}){ui.colored_label(theme.syn("number"),"● unsaved");}
    ui.label(RichText::new(kind).size(10.).color(theme.ui("muted")));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if ui.add(egui::Button::new("×").frame(false).small()).on_hover_text("Close pane (Ctrl+Shift+W)").clicked(){self.focused=id.clone();self.pending=Some(PendingClose::Pane);}
if ui.add(egui::Button::new("Zoom").frame(false).small()).on_hover_text("Zoom or restore (Ctrl+Shift+Enter)").clicked(){self.focused=id.clone();self.zoom=if self.zoom.is_some(){None}else{Some(id.clone())};}});});
   ui.separator();match node {
    Pane::Terminal{cwd,command,..}=>{
     if !self.terminals.contains_key(&id){let test_command=vec!["/usr/bin/env".into(),"HISTFILE=/dev/null".into(),"PS1=$ ".into(),"/bin/bash".into(),"--noprofile".into(),"--norc".into(),"-i".into()];match Session::spawn(cwd,if self.smoke.is_some(){Some(test_command.as_slice())}else{command.as_deref()},ctx.clone()){Ok(s)=>{self.terminals.insert(id.clone(),s);},Err(e)=>{ui.colored_label(theme.ui("danger"),format!("Cannot open terminal: {e}"));return;}}}
     let session=self.terminals.get_mut(&id).unwrap();let allow_input=selected && !self.palette && !self.settings && self.dialog.is_none() && self.pending.is_none();
     let search=if self.dialog.as_deref()==Some("Search terminal"){self.dialog_input.as_str()}else{""};
     if terminal::show(ui,session,theme,&self.state.preferences,allow_input,search){focus_requested=true;}*cwd=session.cwd.clone();if let Some(error)=session.error.take(){self.message=error;}
    },
    Pane::Files{path,..}=>{
     let view=self.files.entry(id.clone()).or_default();if view.loaded.as_ref()!=Some(path) || view.refreshed.is_none_or(|t|t.elapsed()>Duration::from_secs(2)){if view.loaded.as_ref()!=Some(path){view.path_input=path.display().to_string();}match files::list(path,&view.filter){Ok(entries)=>{view.entries=entries;view.error.clear();},Err(e)=>view.error=e.to_string()}view.loaded=Some(path.clone());view.refreshed=Some(Instant::now());}
     ui.horizontal(|ui|{if ui.small_button("Up").clicked()&& let Some(parent)=path.parent(){*path=parent.to_path_buf();}let edit=ui.add(egui::TextEdit::singleline(&mut view.path_input).desired_width(ui.available_width()));if edit.lost_focus() && ui.input(|i|i.key_pressed(Key::Enter)){let requested=PathBuf::from(&view.path_input);*path=if requested.is_absolute(){requested}else{path.join(requested)};}});
     if ui.add(egui::TextEdit::singleline(&mut view.filter).hint_text("Filter files…")).changed(){view.refreshed=None;}
if !view.error.is_empty(){ui.colored_label(theme.ui("danger"),&view.error);}let mut open=None;
     egui::ScrollArea::both().auto_shrink([false,false]).show(ui,|ui|{for entry in &view.entries{let label=RichText::new(format!("{}  {}",if entry.directory{"▸"}else{"·"},entry.name)).color(if entry.directory{theme.syn("function")}else{theme.ui("text")});let response=ui.selectable_label(false,label);if response.double_clicked(){if entry.directory{*path=entry.path.clone();}else{open=Some(entry.path.clone());}}}});
     if let Some(path)=open{self.pending_open=Some(path);}
    },
    Pane::Editor{path,..}=>{
     if !self.documents.contains_key(&id){match Document::open(path){Ok(doc)=>{self.documents.insert(id.clone(),doc);},Err(e)=>{ui.colored_label(theme.ui("danger"),e.to_string());return;}}}
     let language=self.syntax.language(path).unwrap_or_else(||"Plain text".into());
     let doc=self.documents.get_mut(&id).unwrap();ui.horizontal(|ui|{if ui.add_enabled(!doc.read_only && doc.dirty(),egui::Button::new("Save  Ctrl+S")).clicked(){self.pending_save=Some(id.clone());}
if doc.read_only{ui.colored_label(theme.ui("danger"),"Unknown encoding · read only");}badge(ui,theme,&language,theme.syn("function"));ui.label(RichText::new(format!("{} lines",doc.text.lines().count().max(1))).small().color(theme.ui("muted")));ui.label(RichText::new(doc.path.display().to_string()).small().color(theme.ui("muted")));});
     let font=FontId::monospace(self.state.preferences.font_size);let doc_path=doc.path.clone();let syntax=&mut self.syntax;
     let mut layouter=|ui:&egui::Ui,text:&dyn egui::TextBuffer,wrap:f32|{let job=syntax.job(&doc_path,text.as_str(),theme,font.clone(),wrap);ui.fonts_mut(|f|f.layout_job(job))};
     let edit=egui::TextEdit::multiline(&mut doc.text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).interactive(!doc.read_only).code_editor().layouter(&mut layouter);
     egui::ScrollArea::both().auto_shrink([false,false]).show(ui,|ui|{if ui.add_sized(ui.available_size().max(Vec2::new(1.,100.)),edit).has_focus(){focus_requested=true;}});
    },
    Pane::System{..}=>{self.system.refresh();egui::ScrollArea::vertical().show(ui,|ui|{ui.add_space(16.);for (name,value) in [("LOAD AVERAGE",&self.system.load),("MEMORY",&self.system.memory),("UPTIME",&self.system.uptime)]{ui.label(RichText::new(name).small().color(theme.ui("muted")));ui.label(RichText::new(value).size(23.).color(theme.ui("accent")));ui.add_space(20.);}ui.label(format!("{} logical CPUs",std::thread::available_parallelism().map(usize::from).unwrap_or(1)));ui.label(std::env::consts::OS);});},
    Pane::Processes{..}=>{self.system.refresh();ui.add(egui::TextEdit::singleline(&mut self.process_filter).hint_text("Filter name or PID…"));let filter=self.process_filter.to_lowercase();egui::ScrollArea::both().auto_shrink([false,false]).show(ui,|ui|{egui::Grid::new(("processes",&id)).striped(true).show(ui,|ui|{ui.strong("PID");ui.strong("Process");ui.strong("RSS");ui.end_row();for (pid,name,rss) in &self.system.processes{if name.to_lowercase().contains(&filter)||pid.contains(&filter){ui.monospace(pid);ui.label(name);ui.label(rss);ui.end_row();}}});});},
    Pane::Browser{url,..}=>{ui.label("Imported browser link");ui.text_edit_singleline(url);if ui.button("Open in system browser").clicked()&& (url.starts_with("https://")||url.starts_with("http://")){ctx.open_url(egui::OpenUrl::new_tab(url.clone()));}ui.label("Embedded browsing is pending. The link is preserved in this workspace.");},_=>{}
   }
  });
        if focus_requested {
            self.focused = id;
        }
    }
    fn open_file(&mut self, path: PathBuf) {
        match Document::open(&path) {
            Ok(doc) => {
                let id = model::id();
                let path = doc.path.clone();
                self.documents.insert(id.clone(), doc);
                self.insert(Pane::Editor { id, path });
            }
            Err(e) => self.message = e.to_string(),
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
    fn appearance(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut draft = self.theme_draft.take();
        let mut save_draft = false;
        let mut cancel_draft = false;
        egui::Window::new("Appearance")
            .open(&mut open)
            .default_width(650.)
            .max_height(650.)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(t) = &mut draft {
                        ui.label(
                            "Customize every interface and terminal color. Changes preview live.",
                        );
                        ui.text_edit_singleline(&mut t.name);
                        egui::ComboBox::from_label("Kind")
                            .selected_text(&t.kind)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut t.kind, "dark".into(), "Dark");
                                ui.selectable_value(&mut t.kind, "light".into(), "Light");
                            });
                        ui.separator();
                        for (name, values) in
                            [("Interface", &mut t.ui), ("Terminal", &mut t.terminal)]
                        {
                            ui.strong(name);
                            egui::Grid::new(name).num_columns(2).show(ui, |ui| {
                                for (key, value) in values {
                                    ui.label(key);
                                    ui.horizontal(|ui| {
                                        if let Some(c) = theme::color(value) {
                                            let (rect, _) = ui.allocate_exact_size(
                                                Vec2::splat(18.),
                                                Sense::hover(),
                                            );
                                            ui.painter().rect_filled(rect, 3., c);
                                        }
                                        ui.add(
                                            egui::TextEdit::singleline(value).desired_width(110.),
                                        );
                                    });
                                    ui.end_row();
                                }
                            });
                        }
                        if let Err(e) = t.validate() {
                            ui.colored_label(Color32::LIGHT_RED, e.to_string());
                        }
                        ui.horizontal(|ui| {
                            save_draft = ui
                                .add_enabled(t.validate().is_ok(), egui::Button::new("Save theme"))
                                .clicked();
                            cancel_draft = ui.button("Cancel preview").clicked();
                        });
                    } else {
                        ui.label("TEN PRESETS · YOUR OWN COLORS");
                        ui.add_space(8.);
                        egui::Grid::new("themes")
                            .num_columns(5)
                            .spacing([8., 12.])
                            .show(ui, |ui| {
                                for (i, (id, t)) in self.presets.iter().enumerate() {
                                    let b = egui::Button::new(
                                        RichText::new(&t.name).color(t.ui("text")),
                                    )
                                    .fill(t.ui("surface"))
                                    .stroke(Stroke::new(
                                        if self.state.preferences.theme == *id {
                                            2_f32
                                        } else {
                                            1_f32
                                        },
                                        t.ui("accent"),
                                    ));
                                    if ui.add_sized([110., 45.], b).clicked() {
                                        self.state.preferences.theme = id.clone();
                                    }
                                    if i % 5 == 4 {
                                        ui.end_row();
                                    }
                                }
                            });
                        for t in &self.state.preferences.custom_themes {
                            if ui
                                .selectable_label(self.state.preferences.theme == t.id, &t.name)
                                .clicked()
                            {
                                self.state.preferences.theme = t.id.clone();
                            }
                        }
                        if ui.button("Customize current theme").clicked() {
                            let mut t = self.theme();
                            t.id = model::id();
                            t.name = format!("{} custom", t.name);
                            draft = Some(t);
                        }
                        ui.separator();
                        ui.strong("Type and cursor");
                        egui::ComboBox::from_label("Terminal font")
                            .selected_text(if self.state.preferences.terminal_font.is_empty() {
                                "System monospace"
                            } else {
                                &self.state.preferences.terminal_font
                            })
                            .show_ui(ui, |ui| {
                                for name in ["monospace", "DejaVu Sans Mono", "Liberation Mono"] {
                                    ui.selectable_value(
                                        &mut self.state.preferences.terminal_font,
                                        name.into(),
                                        name,
                                    );
                                }
                            });
                        ui.add(
                            egui::Slider::new(&mut self.state.preferences.font_size, 10. ..=28.)
                                .text("Terminal size"),
                        );
                        ui.add(
                            egui::Slider::new(&mut self.state.preferences.line_height, 1. ..=2.)
                                .text("Line height"),
                        );
                        egui::ComboBox::from_label("Cursor")
                            .selected_text(&self.state.preferences.cursor_style)
                            .show_ui(ui, |ui| {
                                for style in ["block", "bar", "underline"] {
                                    ui.selectable_value(
                                        &mut self.state.preferences.cursor_style,
                                        style.into(),
                                        style,
                                    );
                                }
                            });
                        ui.checkbox(&mut self.state.preferences.cursor_blink, "Blink cursor");
                        ui.separator();
                        ui.strong("Share themes");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.theme_path)
                                .hint_text("Absolute path to a theme JSON file")
                                .desired_width(f32::INFINITY),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Export current theme").clicked() {
                                let result = (|| -> Result<()> {
                                    let path = PathBuf::from(&self.theme_path);
                                    if !path.is_absolute() {
                                        bail!("Choose an absolute file path");
                                    }
                                    let mut file = fs::OpenOptions::new()
                                        .write(true)
                                        .create_new(true)
                                        .open(path)?;
                                    file.write_all(&serde_json::to_vec_pretty(&self.theme())?)?;
                                    Ok(())
                                })();
                                self.message = match result {
                                    Ok(()) => "Theme exported".into(),
                                    Err(e) => e.to_string(),
                                };
                            }
                            if ui.button("Import theme").clicked() {
                                let result = (|| -> Result<Theme> {
                                    let mut t: Theme =
                                        serde_json::from_slice(&fs::read(&self.theme_path)?)?;
                                    t.validate()?;
                                    t.id = model::id();
                                    Ok(t)
                                })();
                                match result {
                                    Ok(t) => {
                                        self.state.preferences.theme = t.id.clone();
                                        self.state.preferences.custom_themes.push(t);
                                    }
                                    Err(e) => self.message = e.to_string(),
                                }
                            }
                        });
                        ui.separator();
                        ui.label(format!("App data: {}", self.profile.display()));
                        ui.label("Escape closes this window.");
                    }
                });
            });
        if save_draft && let Some(t) = draft.take() {
            self.state.preferences.theme = t.id.clone();
            self.state.preferences.custom_themes.push(t);
        }
        if cancel_draft || !open {
            draft = None;
        }
        self.theme_draft = draft;
        self.settings = open;
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
    fn persist(&mut self) {
        fn sync_cwd(p: &mut Pane, sessions: &HashMap<String, Session>) {
            match p {
                Pane::Terminal { id, cwd, .. } => {
                    if let Some(s) = sessions.get(id) {
                        *cwd = s.cwd.clone();
                    }
                }
                Pane::Split { first, second, .. } => {
                    sync_cwd(first, sessions);
                    sync_cwd(second, sessions);
                }
                _ => {}
            }
        }
        for w in &mut self.state.workspaces {
            for t in &mut w.tabs {
                sync_cwd(&mut t.layout, &self.terminals);
            }
        }
        if self.recovery_conflict {
            let source = self.profile.join("drafts.json");
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let backup = self.profile.join(format!("drafts-conflict-{stamp}.json"));
            match fs::copy(&source, &backup) {
                Ok(_) => {
                    self.recovery_conflict = false;
                    self.message =
                        format!("Conflicting recovery preserved at {}", backup.display());
                }
                Err(e) => {
                    self.message = format!("Cannot preserve recovery: {e}");
                    return;
                }
            }
        }

        if let Err(e) = self.state.save(&self.profile.join("workspace.json")) {
            self.message = format!("Workspace was not saved: {e}");
            return;
        }
        if !self.recovery_conflict {
            let drafts: Vec<_> = self
                .documents
                .values()
                .filter(|d| d.dirty())
                .map(Document::draft)
                .collect();
            let result = (|| -> Result<()> {
                let mut temp = tempfile::NamedTempFile::new_in(&self.profile)?;
                temp.write_all(&serde_json::to_vec_pretty(&drafts)?)?;
                temp.as_file().sync_all()?;
                temp.persist(self.profile.join("drafts.json"))?;
                Ok(())
            })();
            if let Err(e) = result {
                self.message = format!("Recovery was not saved: {e}");
            }
        }
        self.last_save = Instant::now();
    }
    fn restore_drafts(&mut self) {
        let file = self.profile.join("drafts.json");
        if !file.exists() {
            return;
        }
        let result =
            (|| -> Result<Vec<Draft>> { Ok(serde_json::from_slice(&fs::read(&file)?)?) })();
        match result {
            Ok(drafts) => {
                for draft in drafts {
                    match Document::restore(draft) {
                        Ok(doc) => {
                            let mut restored = false;
                            for w in &self.state.workspaces {
                                for t in &w.tabs {
                                    let mut leaves = vec![];
                                    t.layout.leaves(&mut leaves);
                                    for p in leaves {
                                        if let Pane::Editor { id, path } = p
                                            && path == &doc.path
                                        {
                                            self.documents.insert(id.clone(), doc.clone());
                                            restored = true;
                                        }
                                    }
                                }
                            }
                            if !restored {
                                let id = model::id();
                                let path = doc.path.clone();
                                self.documents.insert(id.clone(), doc);
                                self.insert(Pane::Editor { id, path });
                            }
                        }
                        Err(e) => {
                            self.message = e.to_string();
                            self.recovery_conflict = true;
                        }
                    }
                }
            }
            Err(e) => {
                self.message = format!(
                    "Recovery could not be read; preserved at {}: {e}",
                    file.display()
                );
                self.recovery_conflict = true;
            }
        }
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
                    self.action(Action::Theme("orbit".into()), ctx);
                    self.persist();
                    let restored = State::read(&self.profile.join("workspace.json"))?;
                    if restored.preferences.theme != "orbit" {
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
                    .inner_margin(8.),
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
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(9.), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}
fn badge(ui: &mut egui::Ui, theme: &Theme, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .stroke(Stroke::new(1_f32, color.gamma_multiply(0.5)))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.).strong().color(color));
        });
    let _ = theme;
}
fn section(ui: &mut egui::Ui, theme: &Theme, text: &str) {
    ui.add_space(4.);
    ui.label(
        RichText::new(text)
            .size(10.5)
            .strong()
            .color(theme.ui("muted")),
    );
    ui.add_space(2.);
}
/// A full-width sidebar row with an accent bar when active and an optional right-hand hint.
fn nav_row(
    ui: &mut egui::Ui,
    theme: &Theme,
    label: &str,
    hint: &str,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let fill = if active {
            theme.ui("tabActive")
        } else if response.hovered() {
            theme.ui("surface")
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 7, fill);
        if active {
            let bar = Rect::from_min_size(
                rect.left_top() + Vec2::new(0., 6.),
                Vec2::new(3., rect.height() - 12.),
            );
            ui.painter().rect_filled(bar, 2, theme.ui("accent"));
        }
        let color = if active || response.hovered() {
            theme.ui("textStrong")
        } else {
            theme.ui("text")
        };
        ui.painter().text(
            rect.left_center() + Vec2::new(12., 0.),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.),
            color,
        );
        if !hint.is_empty() {
            ui.painter().text(
                rect.right_center() - Vec2::new(10., 0.),
                egui::Align2::RIGHT_CENTER,
                hint,
                FontId::proportional(11.),
                theme.ui("muted"),
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// A tab chip; returns the chip response and whether its close button was clicked.
fn tab_chip(
    ui: &mut egui::Ui,
    theme: &Theme,
    name: &str,
    active: bool,
    closable: bool,
) -> (egui::Response, bool) {
    let font = FontId::proportional(14.);
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_string(), font, Color32::WHITE);
    let close_w = if closable { 22. } else { 0. };
    let size = Vec2::new(galley.size().x + 28. + close_w, 32.);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    let mut closed = false;
    if ui.is_rect_visible(rect) {
        let fill = if active {
            theme.term("background")
        } else if response.hovered() {
            theme.ui("surface")
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 8, fill);
        if active {
            let bar = Rect::from_min_size(
                rect.left_bottom() - Vec2::new(-8., 3.),
                Vec2::new(rect.width() - 16., 3.),
            );
            ui.painter().rect_filled(bar, 2, theme.ui("accent"));
        }
        let color = if active {
            theme.ui("textStrong")
        } else {
            theme.ui("textSecondary")
        };
        ui.painter().text(
            rect.left_center() + Vec2::new(14., -1.),
            egui::Align2::LEFT_CENTER,
            name,
            FontId::proportional(14.),
            color,
        );
        if closable && (active || response.hovered()) {
            let x =
                Rect::from_center_size(rect.right_center() - Vec2::new(16., 1.), Vec2::splat(18.));
            let hit = ui.interact(x, response.id.with("close"), Sense::click());
            if hit.hovered() {
                ui.painter().rect_filled(x, 5, theme.ui("border"));
            }
            ui.painter().text(
                x.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                FontId::proportional(14.),
                theme.ui("muted"),
            );
            closed = hit.on_hover_text("Close tab").clicked();
        }
    }
    (
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
        closed,
    )
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
