//! TOML user configuration (`~/.config/caelum-rs/config.toml`) with hot reload.
//!
//! The JSON profile keeps application *state* (workspaces, tabs, drafts). This file
//! holds hand-edited *settings*: fonts, theme choice, cursor and keybindings. Values
//! present in the file override the profile's preferences while the app runs.
use crate::model::Preferences;
use anyhow::{Context, Result, bail};
use eframe::egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::Duration,
};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    /// Fontconfig family name; empty or absent means the system monospace font.
    pub family: Option<String>,
    pub size: Option<f32>,
    pub line_height: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorConfig {
    /// `block`, `bar` or `underline`.
    pub style: Option<String>,
    pub blink: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Id of a bundled or custom theme.
    pub theme: Option<String>,
    pub font: FontConfig,
    pub cursor: CursorConfig,
    /// Action name -> shortcut such as `"ctrl+shift+t"`. See [`ACTIONS`].
    pub keybindings: BTreeMap<String, String>,
}

/// Bindable actions and their default shortcuts.
pub const ACTIONS: &[(&str, &str)] = &[
    ("command_palette", "ctrl+shift+p"),
    ("toggle_sidebar", "ctrl+shift+b"),
    ("new_tab", "ctrl+shift+t"),
    ("split_right", "ctrl+shift+d"),
    ("split_below", "ctrl+shift+e"),
    ("zoom_pane", "ctrl+shift+enter"),
    ("close_pane", "ctrl+shift+w"),
    ("open_file", "ctrl+o"),
    ("save_file", "ctrl+s"),
    ("search", "ctrl+shift+f"),
    ("next_tab", "ctrl+tab"),
    ("prev_tab", "ctrl+shift+tab"),
    ("copy", "ctrl+shift+c"),
    ("paste", "ctrl+shift+v"),
    ("prev_prompt", "ctrl+shift+up"),
    ("next_prompt", "ctrl+shift+down"),
];

/// Where the config lives: `--config`/`CAELUM_RS_CONFIG`, else XDG, else `~/.config`.
pub fn default_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CAELUM_RS_CONFIG").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::model::home().join(".config"))
        .join("caelum-rs")
        .join("config.toml")
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text).context("Invalid config.toml")?;
        config.validate()?;
        Ok(config)
    }
    /// Reads `path`; a missing file is the default config, anything else must be valid.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).with_context(|| path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(s) = self.font.size
            && !(6.0..=72.0).contains(&s)
        {
            bail!("font.size must be between 6 and 72");
        }
        if let Some(l) = self.font.line_height
            && !(1.0..=3.0).contains(&l)
        {
            bail!("font.line_height must be between 1.0 and 3.0");
        }
        if let Some(s) = &self.cursor.style
            && !["block", "bar", "underline"].contains(&s.as_str())
        {
            bail!("cursor.style must be block, bar or underline");
        }
        Keymap::build(&self.keybindings).map(|_| ())
    }
    /// Overlay the values this file sets onto the profile preferences.
    pub fn apply(&self, prefs: &mut Preferences) {
        if let Some(f) = &self.font.family {
            prefs.terminal_font = f.clone();
        }
        if let Some(s) = self.font.size {
            prefs.font_size = s;
        }
        if let Some(l) = self.font.line_height {
            prefs.line_height = l;
        }
        if let Some(s) = &self.cursor.style {
            prefs.cursor_style = s.clone();
        }
        if let Some(b) = self.cursor.blink {
            prefs.cursor_blink = b;
        }
    }
    /// A commented starter file, printed by `--print-config` when nothing is set.
    pub fn example() -> &'static str {
        include_str!("../packaging/config.example.toml")
    }
}

/// A resolved shortcut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Binding {
    pub modifiers: Modifiers,
    pub key: Key,
}

pub fn parse_binding(text: &str) -> Result<Binding> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            "" => bail!("Empty part in shortcut {text:?}"),
            other => {
                if key.is_some() {
                    bail!("Shortcut {text:?} has more than one key");
                }
                let mut name = other.to_string();
                if let Some(first) = name.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                key = Some(
                    Key::from_name(&name)
                        .or_else(|| Key::from_name(other))
                        .with_context(|| format!("Unknown key {part:?} in shortcut {text:?}"))?,
                );
            }
        }
    }
    let key = key.with_context(|| format!("Shortcut {text:?} has no key"))?;
    if !(modifiers.ctrl || modifiers.alt) {
        bail!("Shortcut {text:?} needs ctrl or alt so it cannot swallow typing");
    }
    Ok(Binding { modifiers, key })
}

/// Action name -> binding, defaults overlaid with the user's choices.
#[derive(Clone, Debug)]
pub struct Keymap(BTreeMap<&'static str, Binding>);

impl Default for Keymap {
    fn default() -> Self {
        Self::build(&BTreeMap::new()).expect("default bindings parse")
    }
}

impl Keymap {
    pub fn build(overrides: &BTreeMap<String, String>) -> Result<Self> {
        for name in overrides.keys() {
            if !ACTIONS.iter().any(|(a, _)| a == name) {
                let known: Vec<&str> = ACTIONS.iter().map(|(a, _)| *a).collect();
                bail!(
                    "Unknown keybinding action {name:?}; known: {}",
                    known.join(", ")
                );
            }
        }
        let mut map = BTreeMap::new();
        for (action, default) in ACTIONS {
            let text = overrides
                .get(*action)
                .map(String::as_str)
                .unwrap_or(default);
            map.insert(*action, parse_binding(text).with_context(|| *action)?);
        }
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (action, b) in &map {
            let id = format!("{:?}{:?}", b.modifiers, b.key);
            if let Some(other) = seen.insert(id, action) {
                bail!("Actions {other} and {action} share a shortcut");
            }
        }
        Ok(Self(map))
    }
    pub fn get(&self, action: &str) -> Option<Binding> {
        self.0.get(action).copied()
    }
}

/// How long the file must be quiet before it is re-read, so an editor's burst of
/// write/rename events produces one reload of the final contents.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// Watches the config file's directory (so editors that replace the file are seen).
/// A background thread debounces change events, then reads and parses the file off
/// the UI thread and sends the result on `changes`.
pub struct Watcher {
    _inner: notify::RecommendedWatcher,
    pub changes: Receiver<Result<Config>>,
}

impl Watcher {
    pub fn new(path: &Path, wake: eframe::egui::Context) -> Result<Self> {
        use notify::{RecursiveMode, Watcher as _};
        let dir = path.parent().context("Config path has no directory")?;
        std::fs::create_dir_all(dir)?;
        let name = path.file_name().map(|n| n.to_os_string());
        let (raw_tx, raw_rx) = mpsc::channel::<()>();
        let mut inner =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event
                    && !event.kind.is_access()
                    && event.paths.iter().any(|p| p.file_name() == name.as_deref())
                {
                    let _ = raw_tx.send(());
                }
            })?;
        inner.watch(dir, RecursiveMode::NonRecursive)?;
        let (tx, changes) = mpsc::channel();
        let path = path.to_path_buf();
        std::thread::Builder::new()
            .name("config-reload".into())
            .spawn(move || reload_loop(&path, &raw_rx, &tx, &wake))
            .context("Cannot start config reload thread")?;
        Ok(Self {
            _inner: inner,
            changes,
        })
    }
}

/// Runs until the watcher (event sender) or the app (result receiver) goes away.
fn reload_loop(
    path: &Path,
    events: &Receiver<()>,
    results: &mpsc::Sender<Result<Config>>,
    wake: &eframe::egui::Context,
) {
    while events.recv().is_ok() {
        loop {
            match events.recv_timeout(DEBOUNCE) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if results.send(Config::load(path)).is_err() {
            return;
        }
        wake.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_file_and_applies() {
        let c = Config::parse(
            r#"
theme = "starlight"
[font]
family = "JetBrains Mono"
size = 13.5
line_height = 1.3
[cursor]
style = "bar"
blink = true
[keybindings]
new_tab = "ctrl+alt+t"
"#,
        )
        .unwrap();
        let mut p = Preferences::default();
        c.apply(&mut p);
        assert_eq!(p.terminal_font, "JetBrains Mono");
        assert_eq!(p.font_size, 13.5);
        assert_eq!(p.cursor_style, "bar");
        assert!(p.cursor_blink);
        let k = Keymap::build(&c.keybindings).unwrap();
        let b = k.get("new_tab").unwrap();
        assert!(b.modifiers.ctrl && b.modifiers.alt && b.key == Key::T);
        assert_eq!(k.get("split_right").unwrap().key, Key::D);
    }

    #[test]
    fn unset_keys_leave_preferences_alone() {
        let mut p = Preferences::default();
        let before = p.font_size;
        Config::parse("").unwrap().apply(&mut p);
        assert_eq!(p.font_size, before);
    }

    #[test]
    fn rejects_bad_values() {
        for bad in [
            "bogus = 1",
            "[font]\nsize = 2",
            "[font]\nline_height = 9",
            "[cursor]\nstyle = \"blob\"",
            "[keybindings]\nnope = \"ctrl+x\"",
            "[keybindings]\nnew_tab = \"t\"",
            "[keybindings]\nnew_tab = \"ctrl+shift+nonsense\"",
            "[keybindings]\nnew_tab = \"ctrl+shift+d\"",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn binding_parser_handles_named_keys() {
        let b = parse_binding("Ctrl+Shift+Enter").unwrap();
        assert_eq!(b.key, Key::Enter);
        assert_eq!(parse_binding("ctrl+shift+up").unwrap().key, Key::ArrowUp);
        assert_eq!(parse_binding("ctrl+tab").unwrap().key, Key::Tab);
    }

    #[test]
    fn missing_file_is_default_and_example_is_valid() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&d.path().join("none.toml")).unwrap(),
            Config::default()
        );
        Config::parse(Config::example()).unwrap();
    }

    #[test]
    fn watcher_sees_create_and_atomic_replace() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let w = Watcher::new(&path, eframe::egui::Context::default()).unwrap();
        std::fs::write(&path, "theme = \"a\"").unwrap();
        let c = w
            .changes
            .recv_timeout(Duration::from_secs(5))
            .expect("create");
        assert_eq!(c.unwrap().theme.as_deref(), Some("a"));
        while w.changes.try_recv().is_ok() {}
        let tmp = d.path().join(".config.toml.tmp");
        std::fs::write(&tmp, "theme = \"b\"").unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        let c = w
            .changes
            .recv_timeout(Duration::from_secs(5))
            .expect("replace");
        assert_eq!(c.unwrap().theme.as_deref(), Some("b"));
    }

    #[test]
    fn watcher_debounces_bursts_and_reports_parse_errors() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("config.toml");
        let w = Watcher::new(&path, eframe::egui::Context::default()).unwrap();
        // Writes closer together than DEBOUNCE coalesce into one read of the final text.
        for t in ["a", "b", "c", "d", "final"] {
            std::fs::write(&path, format!("theme = \"{t}\"")).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
        // A slow runner can stretch a gap past DEBOUNCE and surface an intermediate
        // value first; what matters is that the burst settles on the final text in
        // far fewer reads than writes.
        let mut reads = 0;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let last = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let c = w.changes.recv_timeout(left).expect("burst");
            reads += 1;
            let theme = c.ok().and_then(|c| c.theme);
            if theme.as_deref() == Some("final") {
                break theme;
            }
        };
        assert_eq!(last.as_deref(), Some("final"));
        assert!(reads <= 5, "{reads} reads for 5 writes: not debounced");
        while w.changes.recv_timeout(DEBOUNCE * 3).is_ok() {}
        std::fs::write(&path, "[font]\nsize = 2").unwrap();
        let c = w.changes.recv_timeout(Duration::from_secs(5)).expect("bad");
        assert!(c.is_err());
    }
}
