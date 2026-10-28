<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/mark-dark.svg">
    <img src="assets/brand/mark.svg" width="180" alt="Caelum icon">
  </picture>
</p>

<h1 align="center">Caelum · Rust</h1>
<p align="center"><strong>Your space to build.</strong><br>A native workspace terminal, built in Rust.</p>

<p align="center"><img src="docs/demo.gif" width="100%" alt="Caelum Rust running shell commands, splitting panes right and below, zooming one, and switching between the Orbit, Aurora, Dawn and Starlight themes"></p>

**An early, working native rewrite of [Caelum](https://github.com/DakodaStemen/caelum).**
This repository is independent of the Electron version. It uses egui/eframe for the
interface, Alacritty's terminal core, and portable-pty for real shell sessions.
There is no Electron, JavaScript frontend, or Python bridge in this application.

The existing Caelum release remains the reference application. This preview is
not yet a feature-complete replacement, and no performance superiority is claimed.

## Working today

- Real terminal sessions with ANSI, 256-color and true-color output, alternate
  screens, keyboard input, scrollback, text selection, copy and bracketed paste.
- Workspaces and tabs, nested resizable splits, and pane zoom.
- Syntax-highlighted editing for common languages, colored by each theme's own palette.
- File browsing and UTF-8 text editing; external-change checks, atomic saves,
  permission and symlink preservation, and read-only previews for unknown encodings.
- Recovery snapshots for unsaved edits and confirmation before discarding them.
- Ten coordinated interface, terminal and code themes, custom color editing with live preview,
  JSON import/export, terminal font, size, line height and cursor settings.
- Live Linux memory, load, uptime and process views.
- SSH through your existing client and configuration; Codex and Claude commands
  can run in terminal panes when installed.
- Command palette, separate settings profile, and optional legacy workspace import.

## Build and run

Linux is the current target. Install a stable Rust toolchain, a C linker and the
usual X11/Wayland and OpenGL runtime libraries. Fontconfig is used to locate a
system monospace font; bundled font fallbacks remain available.

On Ubuntu, the runtime packages include `libxkbcommon-x11-0`, `libx11-6`,
`libwayland-client0`, `libegl1`, `libgl1` and `fontconfig`. On Arch, install
`libxkbcommon-x11`, `libx11`, `wayland`, `mesa` and `fontconfig`.

```sh
git clone https://github.com/DakodaStemen/caelum-rs.git
cd caelum-rs
cargo run --release --locked
```

To install alongside the existing Caelum, build once and run:

```sh
cargo build --release --locked
./scripts/install.sh
caelum-rs
```

The installer adds `~/.local/bin/caelum-rs` and a **Caelum Rust** app-menu entry.
It copies the binary, so rebuilding the checkout does not change the installed app.
It leaves the original `caelum` launcher and profile alone.

## Settings and migration

The Rust workspace lives under `$XDG_DATA_HOME/caelum-rs/`, or
`~/.local/share/caelum-rs/` by default. Override it with `--data-dir PATH` or
`CAELUM_RS_DATA`. Each profile permits one running instance to avoid competing saves.

To import a **copy** of your existing workspace into a fresh Rust profile:

```sh
caelum-rs --import-legacy ~/.local/share/caelum/workspace.json
```

The source is read without modification. An existing destination workspace is
never overwritten. Browser URLs are retained as links opening in your system
browser. Shell working directories are restored, not the previous running processes
or terminal scrollback. Review [migration coverage](docs/migration.md) before importing.

Unsaved editor text is snapshotted in `drafts.json` every three seconds.
If a file changed on disk, conflicting recovery is archived in a
`drafts-conflict-*.json` file rather than written over that file.

## Keyboard

| Shortcut | Action |
| --- | --- |
| Ctrl+Shift+P | Command palette |
| Ctrl+Shift+T | New tab |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+Shift+D / E | Split right / below |
| Ctrl+Shift+Enter | Zoom / restore pane |
| Ctrl+Shift+W | Close pane |
| Ctrl+Shift+C / V | Copy selection / paste |
| Ctrl+Shift+F | Search terminal output and scrollback |
| Ctrl+O / Ctrl+S | Open / save file |

Double-click a tab to rename it, drag a divider to resize, and right-click a
terminal for copy/paste. Ctrl-click an OSC 8 HTTP(S) hyperlink to open it.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked

# Native in-app integration checks; uses an isolated profile and a plain shell.
mkdir -p .runtime
cargo run --locked -- --smoke-test "$PWD/.runtime/desktop.json"

# Actual X11 keyboard/mouse input and restart checks (requires X11 or Xvfb).
cargo test --locked --test desktop -- --ignored --nocapture
```

The smoke test writes a JSON report and PNG screenshot. It drives real PTYs and
native application actions; the separate XTest test drives real window input.
`scripts/record-demo.sh` regenerates `docs/demo.gif` by playing a scripted session
through the same real X11 input path under Xvfb.
Neither test uses your normal Caelum profile or shell initialization files.

See [architecture](docs/architecture.md), [migration coverage](docs/migration.md),
and [contributing](CONTRIBUTING.md). The native terminal canvas still needs a full
screen-reader and IME compatibility audit; accessibility of ordinary UI controls
does not establish accessibility of terminal content.

## License

[MIT](LICENSE). Third-party crates retain their own licenses; see
[third-party notices](THIRD_PARTY_NOTICES.md). The icon is the supplied Caelum SVG
with its lettering removed; a heavier rendering is used at small app-icon sizes.
