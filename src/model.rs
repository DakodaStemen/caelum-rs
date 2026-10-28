use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub fn id() -> String {
    Uuid::new_v4().to_string()
}
pub fn home() -> PathBuf {
    resolve_home(std::env::var_os("HOME"), passwd_home)
}
/// Pure helper: use `env` unless unset/empty, then `passwd`, then `/`.
fn resolve_home(
    env: Option<std::ffi::OsString>,
    passwd: impl FnOnce() -> Option<PathBuf>,
) -> PathBuf {
    env.filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| passwd().filter(|p| !p.as_os_str().is_empty()))
        .unwrap_or_else(|| PathBuf::from("/"))
}
#[cfg(unix)]
fn passwd_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: getpwuid_r writes only into the buffers we provide; pw_dir is read
    // while buf is alive and copied before returning.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0u8; 4096];
        loop {
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            let rc = libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            );
            if rc == libc::ERANGE && buf.len() < 1 << 20 {
                let n = buf.len() * 2;
                buf.resize(n, 0);
                continue;
            }
            if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
                return None;
            }
            let dir = std::ffi::CStr::from_ptr(pwd.pw_dir).to_bytes().to_vec();
            return Some(PathBuf::from(std::ffi::OsString::from_vec(dir)));
        }
    }
}
#[cfg(not(unix))]
fn passwd_home() -> Option<PathBuf> {
    None
}
/// Creates `path` (and parents); on unix new directories get mode 0700. When
/// `fix_existing` is set, an already-existing `path` is tightened to 0700 (best effort).
pub fn create_private_dir(path: &Path, fix_existing: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        if fix_existing {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = fix_existing;
        fs::create_dir_all(path)?;
    }
    Ok(())
}
pub fn profile() -> PathBuf {
    std::env::var_os("CAELUM_RS_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".local/share"))
                .join("caelum-rs")
        })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Pane {
    Terminal {
        id: String,
        #[serde(default)]
        cwd: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Vec<String>>,
    },
    Files {
        id: String,
        path: PathBuf,
    },
    Editor {
        id: String,
        path: PathBuf,
    },
    System {
        id: String,
    },
    Processes {
        id: String,
    },
    #[serde(alias = "web")]
    Browser {
        id: String,
        #[serde(default = "default_url")]
        url: String,
    },
    Split {
        id: String,
        axis: Axis,
        ratio: f32,
        first: Box<Pane>,
        second: Box<Pane>,
    },
}
fn default_url() -> String {
    "https://www.rust-lang.org".into()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Horizontal,
    Vertical,
}
impl Pane {
    pub fn terminal(cwd: PathBuf) -> Self {
        Self::Terminal {
            id: id(),
            cwd,
            command: None,
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Terminal { id, .. }
            | Self::Files { id, .. }
            | Self::Editor { id, .. }
            | Self::System { id }
            | Self::Processes { id }
            | Self::Browser { id, .. }
            | Self::Split { id, .. } => id,
        }
    }
    pub fn leaves<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            Self::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
            _ => out.push(self),
        }
    }
    pub fn split(&mut self, target: &str, new: Pane, axis: Axis) -> bool {
        if self.id() == target {
            *self = Self::Split {
                id: id(),
                axis,
                ratio: 0.5,
                first: Box::new(self.clone()),
                second: Box::new(new),
            };
            true
        } else if let Self::Split { first, second, .. } = self {
            first.split(target, new.clone(), axis) || second.split(target, new, axis)
        } else {
            false
        }
    }
    pub fn remove(&mut self, target: &str) -> bool {
        if let Self::Split { first, second, .. } = self {
            if first.id() == target {
                *self = *second.clone();
                return true;
            }
            if second.id() == target {
                *self = *first.clone();
                return true;
            }
            return first.remove(target) || second.remove(target);
        }
        false
    }
    pub fn find(&self, target: &str) -> Option<&Pane> {
        if self.id() == target {
            return Some(self);
        }
        if let Self::Split { first, second, .. } = self {
            first.find(target).or_else(|| second.find(target))
        } else {
            None
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: String,
    pub name: String,
    pub layout: Pane,
}
impl Tab {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            id: id(),
            name: "Terminal".into(),
            layout: Pane::terminal(cwd),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: String,
}
impl Workspace {
    pub fn new(name: impl Into<String>, cwd: PathBuf) -> Self {
        let t = Tab::new(cwd);
        Self {
            id: id(),
            name: name.into(),
            active_tab: t.id.clone(),
            tabs: vec![t],
        }
    }
    pub fn active_index(&self) -> usize {
        self.tabs
            .iter()
            .position(|t| t.id == self.active_tab)
            .unwrap_or(0)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub theme: String,
    pub font_size: f32,
    pub terminal_font: String,
    pub line_height: f32,
    pub cursor_style: String,
    pub cursor_blink: bool,
    /// Whether the workspace sidebar is shown; hidden by default for a minimal window.
    pub sidebar: bool,
    pub custom_themes: Vec<crate::theme::Theme>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: "orbit".into(),
            font_size: 15.,
            terminal_font: String::new(),
            line_height: 1.2,
            cursor_style: "block".into(),
            cursor_blink: false,
            sidebar: false,
            custom_themes: vec![],
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: String,
    #[serde(default)]
    pub preferences: Preferences,
}
impl Default for State {
    fn default() -> Self {
        let mut w = Workspace::new("Mission Control", home());
        let target = w.tabs[0].layout.id().to_string();
        w.tabs[0].layout.split(
            &target,
            Pane::Files {
                id: id(),
                path: home(),
            },
            Axis::Horizontal,
        );
        Self {
            version: 1,
            active_workspace: w.id.clone(),
            workspaces: vec![w],
            preferences: Preferences::default(),
        }
    }
}
impl State {
    pub fn active_index(&self) -> usize {
        self.workspaces
            .iter()
            .position(|w| w.id == self.active_workspace)
            .unwrap_or(0)
    }
    pub fn validate(&mut self) -> Result<()> {
        if self.version != 1 {
            bail!("Unsupported workspace version {}", self.version);
        }
        if self.workspaces.is_empty() {
            bail!("Workspace file contains no workspaces");
        }
        let mut ids = HashSet::new();
        fn node(p: &mut Pane, ids: &mut HashSet<String>, depth: usize) -> Result<()> {
            if depth > 32 {
                bail!("Workspace layout is too deeply nested");
            }
            if !ids.insert(p.id().to_string()) {
                bail!("Duplicate workspace identifier");
            }
            if let Pane::Split {
                ratio,
                first,
                second,
                ..
            } = p
            {
                *ratio = if ratio.is_finite() {
                    ratio.clamp(0.1, 0.9)
                } else {
                    0.5
                };
                node(first, ids, depth + 1)?;
                node(second, ids, depth + 1)?;
            }
            Ok(())
        }
        for w in &mut self.workspaces {
            if !ids.insert(w.id.clone()) {
                bail!("Duplicate workspace identifier");
            }
            if w.tabs.is_empty() {
                bail!("Workspace contains no tabs");
            }
            for t in &mut w.tabs {
                if !ids.insert(t.id.clone()) {
                    bail!("Duplicate tab identifier");
                }
                node(&mut t.layout, &mut ids, 0)?;
            }
            if !w.tabs.iter().any(|t| t.id == w.active_tab) {
                w.active_tab = w.tabs[0].id.clone();
            }
        }
        if !self
            .workspaces
            .iter()
            .any(|w| w.id == self.active_workspace)
        {
            self.active_workspace = self.workspaces[0].id.clone();
        }
        self.preferences.font_size = self.preferences.font_size.clamp(10., 28.);
        self.preferences.line_height = self.preferences.line_height.clamp(1., 2.);
        for theme in &self.preferences.custom_themes {
            theme.validate()?;
        }
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self> {
        let mut s: Self = serde_json::from_slice(
            &fs::read(path).with_context(|| format!("Reading {}", path.display()))?,
        )?;
        s.validate()?;
        Ok(s)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("Workspace path has no parent")?;
        create_private_dir(parent, false)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temp.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        temp.write_all(&serde_json::to_vec_pretty(self)?)?;
        temp.as_file().sync_all()?;
        temp.persist(path)?;
        Ok(())
    }
    /// Imports a copy. Unsupported legacy pane kinds are rejected rather than silently discarded.
    pub fn import_legacy(source: &Path, destination: &Path) -> Result<()> {
        if destination.exists() {
            bail!("A Rust workspace already exists; it will not be overwritten");
        }
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(source)?)?;
        fn convert(value: &mut serde_json::Value) -> Result<()> {
            if let Some(object) = value.as_object_mut() {
                if object.get("kind").and_then(|v| v.as_str()) == Some("terminal")
                    && !object.contains_key("command")
                {
                    if let Some(host) = object.get("ssh").and_then(|v| v.as_str()) {
                        if host.is_empty()
                            || host.starts_with('-')
                            || !host
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "_.@:-[]".contains(c))
                        {
                            bail!("Unsupported SSH host in legacy workspace");
                        }
                        object.insert("command".into(), serde_json::json!(["ssh", "--", host]));
                    } else if let Some(assistant) = object.get("assistant").and_then(|v| v.as_str())
                    {
                        if !["codex", "claude"].contains(&assistant) {
                            bail!("Unsupported assistant in legacy workspace");
                        }
                        object.insert("command".into(), serde_json::json!([assistant]));
                    }
                }
                for child in object.values_mut() {
                    convert(child)?;
                }
            } else if let Some(array) = value.as_array_mut() {
                for child in array {
                    convert(child)?;
                }
            }
            Ok(())
        }
        convert(&mut value)?;
        let mut state: Self = serde_json::from_value(value)?;
        state.validate()?;
        state.save(destination)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn private_dir_and_workspace_file_modes() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let d = tempfile::tempdir().unwrap();
        let profile = d.path().join("a/profile");
        create_private_dir(&profile, true).unwrap();
        assert_eq!(fs::metadata(&profile).unwrap().mode() & 0o777, 0o700);
        let file = profile.join("workspace.json");
        State::default().save(&file).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        // Re-saving over an existing file keeps 0600.
        State::default().save(&file).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        // Existing loose dir is tightened when requested.
        let loose = d.path().join("loose");
        fs::create_dir(&loose).unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir(&loose, false).unwrap();
        assert_eq!(fs::metadata(&loose).unwrap().mode() & 0o777, 0o755);
        create_private_dir(&loose, true).unwrap();
        assert_eq!(fs::metadata(&loose).unwrap().mode() & 0o777, 0o700);
    }
    #[test]
    fn home_fallback_order() {
        let pw = || Some(PathBuf::from("/home/pw"));
        assert_eq!(resolve_home(Some("/h".into()), pw), PathBuf::from("/h"));
        assert_eq!(resolve_home(Some("".into()), pw), PathBuf::from("/home/pw"));
        assert_eq!(resolve_home(None, pw), PathBuf::from("/home/pw"));
        assert_eq!(resolve_home(None, || None), PathBuf::from("/"));
        assert_eq!(
            resolve_home(Some("".into()), || Some(PathBuf::new())),
            PathBuf::from("/")
        );
    }
    #[test]
    fn nested_layout_collapses_without_losing_sibling() {
        let mut p = Pane::terminal(home());
        let original = p.id().to_string();
        let b = Pane::terminal(home());
        let bid = b.id().to_string();
        p.split(&original, b, Axis::Horizontal);
        p.split(&bid, Pane::System { id: id() }, Axis::Vertical);
        assert!(p.remove(&bid));
        assert!(p.find(&original).is_some());
        let mut leaves = vec![];
        p.leaves(&mut leaves);
        assert_eq!(leaves.len(), 2);
    }
    #[test]
    fn saved_workspace_roundtrip_and_import_never_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join("old.json");
        let new = d.path().join("new.json");
        State::default().save(&old).unwrap();
        State::import_legacy(&old, &new).unwrap();
        let bytes = fs::read(&old).unwrap();
        assert!(State::import_legacy(&old, &new).is_err());
        assert_eq!(bytes, fs::read(&old).unwrap());
        assert_eq!(State::read(&new).unwrap().workspaces.len(), 1);
    }
    #[test]
    fn duplicate_panes_are_rejected() {
        let mut s = State::default();
        let t = s.workspaces[0].tabs[0].clone();
        s.workspaces[0].tabs.push(t);
        assert!(s.validate().is_err());
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn imports_legacy_browser_and_ssh_without_changing_source() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source.json");
        let destination = d.path().join("dest.json");
        let original = serde_json::json!({"version":1,"activeWorkspace":"w","workspaces":[{"id":"w","name":"Legacy","activeTab":"t","tabs":[{"id":"t","name":"SSH","layout":{"id":"s","kind":"split","axis":"horizontal","ratio":0.6,"first":{"id":"terminal","kind":"terminal","cwd":"/tmp","ssh":"user@example.org"},"second":{"id":"browser","kind":"web","url":"https://example.org"}}}]}],"preferences":{"theme":"orbit","fontSize":13}});
        let bytes = serde_json::to_vec(&original).unwrap();
        fs::write(&source, &bytes).unwrap();
        State::import_legacy(&source, &destination).unwrap();
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let state = State::read(&destination).unwrap();
        let mut leaves = vec![];
        state.workspaces[0].tabs[0].layout.leaves(&mut leaves);
        assert!(
            matches!(&leaves[0],Pane::Terminal{command:Some(command),..} if command==&vec!["ssh","--","user@example.org"])
        );
        assert!(matches!(&leaves[1],Pane::Browser{url,..} if url=="https://example.org"));
    }
}
