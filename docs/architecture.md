# Architecture

The native application owns its UI and application logic in Rust. It uses the OS
graphics stack, fonts, shells and external commands as dependencies.

- `model.rs`: typed workspace tree, layout operations, validation, separate profile
  paths and atomic persistence. Import reads the legacy source without modifying it.
- `terminal.rs`: owned PTY sessions, terminal parser, keyboard encoding and grid
  rendering. Alacritty owns terminal semantics; the application owns UI/input and
  process lifetime. Reader/parser and bounded input workers keep PTY I/O off the UI
  thread. PTYs use nonblocking descriptors on Unix so shutdown can stop both workers.
- `files.rs`: strict UTF-8 editing, read-only unknown-encoding display, content-hash
  conflict detection, atomic replacement and recovery documents. File edits resolve
  symlinks and retain permissions. Hard-linked files are not atomically replaced.
- `theme.rs`: the shared palette model, color validation and native visual styling.
- `app.rs`: native workspace/pane presentation, commands, tools, confirmation flows
  and application integration checks.
- `main.rs`: CLI, exclusive profile lock and native window startup.

PTY reader threads update a synchronized Alacritty grid and request repaints.
Only the focused terminal receives keyboard input. Ordinary editable controls own
keyboard focus while typing; application shortcuts are consumed before terminal
translation. Remote terminal clipboard escape requests do not automatically read
or replace the desktop clipboard.

Closing a terminal kills its owned shell and foreground process groups while the
shell is still owned, reaps the child and stops its I/O workers. Running processes
are not recovered when the app exits. A persistent terminal server would be a
separate lifecycle feature.

Workspace and recovery files use same-directory temporary files and rename.
Profile locks prevent competing application instances from racing those writes.
Unknown workspace schemas stop startup and preserve the source file.

The save conflict check catches changes since a file was loaded; it is not a
filesystem-wide transaction against other applications writing simultaneously.
The app currently targets Linux. X11 and Wayland window backends are included;
platform behavior requires explicit runtime validation.
