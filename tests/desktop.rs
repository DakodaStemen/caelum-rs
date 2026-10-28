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
        if let Ok(Some(window)) = find_window(self.0.id())
            && let Ok(keyboard) = Keyboard::new(window)
        {
            let _ = keyboard.capture("native-input-last-window");
        }
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
    fn capture(&self, name: &str) -> Result<()> {
        let geometry = self.connection.get_geometry(self.window)?.reply()?;
        let image = self
            .connection
            .get_image(
                xproto::ImageFormat::Z_PIXMAP,
                self.window,
                0,
                0,
                geometry.width,
                geometry.height,
                u32::MAX,
            )?
            .reply()?;
        let bpp = self
            .connection
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .map(|f| f.bits_per_pixel)
            .unwrap_or(0);
        if bpp == 32 {
            let rgba: Vec<u8> = image
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".runtime");
            fs::create_dir_all(&dir)?;
            image::save_buffer(
                dir.join(format!("{name}.png")),
                &rgba,
                geometry.width as u32,
                geometry.height as u32,
                image::ColorType::Rgba8,
            )?;
        }
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
    let tab_marker = dir.path().join("tab_marker");
    keyboard.type_text(&format!("printf tab_ok > {}", tab_marker.display()))?;
    keyboard.press(0xff09)?;
    keyboard.press(0xff0d)?;
    wait(|| tab_marker.exists()).context("Terminal Tab must not move UI focus")?;
    if Command::new("vim")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        let tui_file = dir.path().join("vim.txt");
        keyboard.type_text(&format!("vim -Nu NONE -i NONE -n {}", tui_file.display()))?;
        keyboard.press(0xff0d)?;
        thread::sleep(Duration::from_millis(600));
        keyboard.type_text("inative_tui_ok")?;
        keyboard.press(0xff1b)?;
        thread::sleep(Duration::from_millis(100));
        keyboard.type_text(":wq")?;
        keyboard.press(0xff0d)?;
        wait(|| fs::read_to_string(&tui_file).ok().as_deref() == Some("native_tui_ok\n"))
            .context("Native terminal Vim editing")?;
    }

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
    eprintln!("new tab persisted");
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
    eprintln!("native editor opened");
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
    eprintln!("native editor saved");
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
/// Plays a paced session for README recording; run under Xvfb with `ffmpeg -f x11grab` capturing.
#[test]
#[ignore = "demo recording: scripts/record-demo.sh"]
fn record_demo() -> Result<()> {
    // Shares the display with the input test, so only play it when the recorder asks.
    if std::env::var_os("CAELUM_RECORD_DEMO").is_none() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let profile = dir.path().join("profile");
    fs::create_dir(&profile)?;
    let shell = dir.path().join("shell");
    fs::write(
        &shell,
        "#!/bin/sh\nexport HISTFILE=/dev/null PS1='\\033[35m❯\\033[0m '\nexec /bin/bash --noprofile --norc -i\n",
    )?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700))?;
    let work = dir.path().join("caelum-rs");
    fs::create_dir(&work)?;
    fs::write(work.join("README.md"), "# caelum-rs\n")?;
    let sample = work.join("main.rs");
    fs::write(
        &sample,
        r##"use std::collections::HashMap;

/// A tiny word counter.
#[derive(Debug, Default)]
struct Counter {
    seen: HashMap<String, usize>,
}

impl Counter {
    fn add(&mut self, word: &str) -> usize {
        let n = self.seen.entry(word.to_lowercase()).or_insert(0);
        *n += 1; // count it
        *n
    }
}

fn main() {
    let mut c = Counter::default();
    for w in "Caelum builds fast, caelum looks great".split_whitespace() {
        println!("{w:>8} -> {}", c.add(w));
    }
    assert_eq!(c.seen.len(), 5, "unique words: {:#?}", 3.14);
}
"##,
    )?;
    let mut state = caelum_rs::model::State::default();
    state.workspaces[0].tabs[0].layout = caelum_rs::model::Pane::terminal(work.clone());
    state.save(&profile.join("workspace.json"))?;
    let app = AppChild(
        Command::new(env!("CARGO_BIN_EXE_caelum-rs"))
            .args(["--data-dir", profile.to_str().unwrap()])
            .env_remove("WAYLAND_DISPLAY")
            .env("SHELL", &shell)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    let mut window = None;
    wait(|| {
        window = find_window(app.0.id()).ok().flatten();
        window.is_some()
    })?;
    let k = Keyboard::new(window.unwrap())?;
    k.focus()?;
    let pause = |ms| thread::sleep(Duration::from_millis(ms));
    let run = |cmd: &str| -> Result<()> {
        for c in cmd.chars() {
            k.type_text(&c.to_string())?;
            pause(45);
        }
        pause(250);
        k.press(0xff0d)?;
        pause(1100);
        Ok(())
    };
    pause(1500);
    k.click(500, 300)?;
    run("printf '\\033[31mred  \\033[32mgreen  \\033[34mblue\\033[0m\\n'")?;
    pause(1200);
    let ctrl = k.key(0xffe3)?.0;
    k.event(ctrl, true)?;
    k.press('o' as u32)?;
    k.event(ctrl, false)?;
    pause(700);
    k.event(ctrl, true)?;
    k.press('a' as u32)?;
    k.event(ctrl, false)?;
    k.type_text(sample.to_str().context("sample path")?)?;
    pause(500);
    k.press(0xff0d)?;
    pause(1200);
    k.shortcut(0xff0d)?;
    pause(1500);
    let theme = |name: &str, wait: u64| -> Result<()> {
        k.shortcut('p' as u32)?;
        pause(500);
        k.type_text(&format!("Theme: {name}"))?;
        pause(600);
        k.press(0xff0d)?;
        pause(wait);
        Ok(())
    };
    // Default dark, a second dark, a light theme, then back to the default.
    for name in ["Aurora", "Dawn", "Orbit"] {
        theme(name, 1600)?;
    }
    // Edit in the pane: add a comment, then save with the pane's own Save button.
    let geometry = k.connection.get_geometry(k.window)?.reply()?;
    let width = geometry.width as i16;
    k.click(width / 2, 420)?;
    k.event(ctrl, true)?;
    k.press(0xff50)?;
    k.event(ctrl, false)?;
    pause(400);
    for c in "// Saved straight from the pane".chars() {
        k.type_text(&c.to_string())?;
        pause(60);
    }
    k.press(0xff0d)?;
    pause(1200);
    k.click(276, 163)?;
    pause(1800);
    // Close the pane with its own button, back to the terminal.
    k.click(width - 18, 117)?;
    pause(1500);
    k.click(width / 2, 300)?;
    run(
        "for i in $(seq 0 3 255); do printf \"\\033[48;2;%d;%d;255m \\033[0m\" $i $((255-i)); done; echo",
    )?;
    pause(1200);
    run("echo 'Saved and shipped with Caelum.'")?;
    pause(2500);
    drop(app);
    Ok(())
}
