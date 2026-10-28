# Native preview verification

Checked on Linux on 2026-10-28:

- Rust formatting and Clippy with warnings denied.
- 20 Rust unit/regression tests (`cargo test --locked`; two further desktop tests are `#[ignore]`d and run under Xvfb): layout persistence, duplicate-ID rejection,
  legacy browser/SSH import, themes, safe file saves, encoding preservation,
  unsaved draft recovery, PTY output and resize, ANSI true color and wide cells,
  alternate-screen restoration, history search, burst-input retention and owned process-group shutdown.
- Nine in-app desktop checks in an isolated profile: real shell execution,
  splits, zoom, tabs, persistence, editor save/conflict behavior, system/process
  data and light/dark themes. The PNG in this directory is from this native app.
- An XTest integration test on an isolated Xvfb display: real keyboard input,
  Ctrl+C interrupting a running process, Tab retaining terminal focus, Vim editing,
  typing into a new split, tab creation,
  native editor typing and saving, graceful shutdown, and restarting the saved layout.
- The initial in-app smoke checks also ran on the local Wayland desktop.

The screenshot uses generated sample files and a plain shell with history disabled.
There is no comparative performance benchmark. Broader TUI, IME, accessibility,
GPU/driver and cross-platform coverage remains unverified. See `migration.md`.
