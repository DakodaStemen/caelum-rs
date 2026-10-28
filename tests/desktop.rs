//! Opt-in X11 tests drive the real native window using XTest, with a throwaway profile and shell.
#![cfg(target_os = "linux")]
use anyhow::{Context, Result, bail};
use std::{
    fs,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    CURRENT_TIME,
    connection::Connection,
    protocol::{
        xproto::{self, AtomEnum, ConnectionExt as _, InputFocus, Window},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};
struct AppChild(Child);
impl Drop for AppChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait(mut condition: impl FnMut() -> bool) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(12);
    while Instant::now() < end {
        if condition() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(40));
    }
    bail!("Timed out waiting for native UI");
}
struct Keyboard {
    connection: RustConnection,
    window: Window,
    keys: Vec<u32>,
    min: u8,
    per: usize,
    root: Window,
}
impl Keyboard {
    fn new(window: Window) -> Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let min = connection.setup().min_keycode;
        let max = connection.setup().max_keycode;
        let mapping = connection
            .get_keyboard_mapping(min, max - min + 1)?
            .reply()?;
        Ok(Self {
            connection,
            window,
            keys: mapping.keysyms,
            min,
            per: mapping.keysyms_per_keycode as usize,
            root,
        })
    }
    fn key(&self, sym: u32) -> Result<(u8, bool)> {
        for (i, keys) in self.keys.chunks(self.per).enumerate() {
            for (level, key) in keys.iter().take(2).enumerate() {
                if *key == sym {
                    return Ok((self.min + i as u8, level == 1));
                }
            }
        }
        bail!("No keycode for {sym}")
    }
    fn event(&self, code: u8, press: bool) -> Result<()> {
        self.connection
            .xtest_fake_input(
                if press {
                    xproto::KEY_PRESS_EVENT
                } else {
                    xproto::KEY_RELEASE_EVENT
                },
                code,
                CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            )?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }
    fn focus(&self) -> Result<()> {
        self.connection
            .set_input_focus(InputFocus::PARENT, self.window, CURRENT_TIME)?
            .check()?;
        self.connection.flush()?;
        thread::sleep(Duration::from_millis(100));
        Ok(())
    }
    fn type_text(&self, text: &str) -> Result<()> {
        let shift = self.key(0xffe1)?.0;
        for c in text.chars() {
            let (code, shifted) = self.key(c as u32)?;
            if shifted {
                self.event(shift, true)?;
            }
            self.event(code, true)?;
            self.event(code, false)?;
            if shifted {
                self.event(shift, false)?;
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn press(&self, sym: u32) -> Result<()> {
        let code = self.key(sym)?.0;
        self.event(code, true)?;
        self.event(code, false)
    }
    fn shortcut(&self, sym: u32) -> Result<()> {
        let ctrl = self.key(0xffe3)?.0;
        let shift = self.key(0xffe1)?.0;
        let key = self.key(sym)?.0;
        self.event(ctrl, true)?;
        self.event(shift, true)?;
        self.event(key, true)?;
        self.event(key, false)?;
        self.event(shift, false)?;
        self.event(ctrl, false)?;
        thread::sleep(Duration::from_millis(200));
        Ok(())
    }
    fn click(&self, x: i16, y: i16) -> Result<()> {
        self.connection
            .warp_pointer(x11rb::NONE, self.window, 0, 0, 0, 0, x, y)?
            .check()?;
        self.connection
            .xtest_fake_input(xproto::BUTTON_PRESS_EVENT, 1, 0, self.root, 0, 0, 0)?
            .check()?;
        self.connection
            .xtest_fake_input(xproto::BUTTON_RELEASE_EVENT, 1, 0, self.root, 0, 0, 0)?
            .check()?;
        self.connection.flush()?;
        thread::sleep(Duration::from_millis(120));
        Ok(())
    }
}
fn find_window(pid: u32) -> Result<Option<Window>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = conn.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let mut todo = vec![root];
    while let Some(window) = todo.pop() {
        if let Ok(reply) = conn
            .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)?
            .reply()
            && reply.value32().is_some_and(|mut p| p.next() == Some(pid))
            && conn.get_window_attributes(window)?.reply()?.map_state == xproto::MapState::VIEWABLE
        {
            return Ok(Some(window));
        }
        if let Ok(tree) = conn.query_tree(window)?.reply() {
            todo.extend(tree.children);
        }
    }
    Ok(None)
}
#[test]
#[ignore = "requires an X11 desktop or Xvfb; cargo test --test desktop -- --ignored"]
fn native_keyboard_splits_files_and_restart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let profile = dir.path().join("profile");
    fs::create_dir(&profile)?;
    let shell = dir.path().join("shell");
    fs::write(
        &shell,
        "#!/bin/sh\nexport HISTFILE=/dev/null PS1='$ '\nexec /bin/bash --noprofile --norc -i\n",
    )?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
    let mut state = caelum_rs::model::State::default();
    state.workspaces[0].tabs[0].layout = caelum_rs::model::Pane::terminal(dir.path().to_path_buf());
    state.save(&profile.join("workspace.json"))?;
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_caelum-rs"))
            .args(["--data-dir", profile.to_str().unwrap()])
            .env_remove("WAYLAND_DISPLAY")
            .env("SHELL", &shell)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
    };
    let mut app = AppChild(spawn()?);
    let mut window = None;
    wait(|| {
        window = find_window(app.0.id()).ok().flatten();
        window.is_some()
    })?;
    let keyboard = Keyboard::new(window.unwrap())?;
    keyboard.focus()?;
    thread::sleep(Duration::from_millis(700));
    keyboard.click(450, 250)?;
    eprintln!("native window ready");
    // Initial focus reaches the real PTY. Test Ctrl+C without relying on shell echo.
    let marker = dir.path().join("typed");
    keyboard.type_text(&format!("printf native_input_ok > {}", marker.display()))?;
    keyboard.press(0xff0d)?;
    wait(|| fs::read_to_string(&marker).ok().as_deref() == Some("native_input_ok"))?;
    eprintln!("typed command passed");
    keyboard.type_text("sleep 30")?;
    keyboard.press(0xff0d)?;
    thread::sleep(Duration::from_millis(100));
    let ctrl = keyboard.key(0xffe3)?.0;
    keyboard.event(ctrl, true)?;
    keyboard.press('c' as u32)?;
    keyboard.event(ctrl, false)?;
    thread::sleep(Duration::from_millis(150));
    let canceled = dir.path().join("canceled");
    keyboard.type_text(&format!("printf canceled > {}", canceled.display()))?;
    keyboard.press(0xff0d)?;
    wait(|| canceled.exists())?;
    // A real keyboard shortcut creates a split and focuses its new terminal.
    eprintln!("ctrl-c passed");
    keyboard.shortcut('d' as u32)?;
    let split = dir.path().join("split");
    keyboard.type_text(&format!("printf split_ok > {}", split.display()))?;
    keyboard.press(0xff0d)?;
    wait(|| split.exists())?;
    eprintln!("split terminal input passed");
    keyboard.shortcut('t' as u32)?;
    wait(|| {
        caelum_rs::model::State::read(&profile.join("workspace.json"))
            .is_ok_and(|s| s.workspaces[0].tabs.len() == 2)
    })?;
    // Native editor edits, saves, and isolates keyboard input from the terminal behind its dialog.
    let fixture = dir.path().join("edit.txt");
    fs::write(&fixture, "original\n")?;
    let ctrl = keyboard.key(0xffe3)?.0;
    keyboard.event(ctrl, true)?;
    keyboard.press('o' as u32)?;
    keyboard.event(ctrl, false)?;
    thread::sleep(Duration::from_millis(200));
    keyboard.event(ctrl, true)?;
    keyboard.press('a' as u32)?;
    keyboard.event(ctrl, false)?;
    keyboard.type_text(fixture.to_str().context("Fixture path")?)?;
    keyboard.press(0xff0d)?;
    wait(|| {
        caelum_rs::model::State::read(&profile.join("workspace.json")).is_ok_and(|s| {
            let mut p = vec![];
            s.workspaces[0].tabs[1].layout.leaves(&mut p);
            p.iter()
                .any(|p| matches!(p, caelum_rs::model::Pane::Editor { .. }))
        })
    })?;
    // Click the editor text area in the right-hand pane; coordinates are logical X11 pixels.
    let geometry = keyboard.connection.get_geometry(keyboard.window)?.reply()?;
    keyboard.click((geometry.width as f32 * 0.80) as i16, 210)?;
    keyboard.event(ctrl, true)?;
    keyboard.press('a' as u32)?;
    keyboard.event(ctrl, false)?;
    keyboard.type_text("native editor save")?;
    keyboard.event(ctrl, true)?;
    keyboard.press('s' as u32)?;
    keyboard.event(ctrl, false)?;
    wait(|| fs::read_to_string(&fixture).ok().as_deref() == Some("native editor save"))?;
    // Graceful WM_DELETE_WINDOW causes native persistence and PTY shutdown.
    let atom = keyboard
        .connection
        .intern_atom(false, b"WM_PROTOCOLS")?
        .reply()?
        .atom;
    let delete = keyboard
        .connection
        .intern_atom(false, b"WM_DELETE_WINDOW")?
        .reply()?
        .atom;
    let event =
        xproto::ClientMessageEvent::new(32, keyboard.window, atom, [delete, CURRENT_TIME, 0, 0, 0]);
    keyboard
        .connection
        .send_event(false, keyboard.window, xproto::EventMask::NO_EVENT, event)?
        .check()?;
    keyboard.connection.flush()?;
    wait(|| app.0.try_wait().ok().flatten().is_some())?;
    let restored = caelum_rs::model::State::read(&profile.join("workspace.json"))?;
    assert_eq!(restored.workspaces[0].tabs.len(), 2);
    let mut leaves = vec![];
    restored.workspaces[0].tabs[0].layout.leaves(&mut leaves);
    assert_eq!(leaves.len(), 2);
    let mut restarted = AppChild(spawn()?);
    wait(|| find_window(restarted.0.id()).ok().flatten().is_some())?;
    restarted.0.kill()?;
    restarted.0.wait()?;
    Ok(())
}
