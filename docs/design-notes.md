# Design notes (not implemented)

## Persistent session daemon
Goal: shells survive closing the window. Sketch: a small `caelum-rs-sessiond` owns PTYs and an `alacritty_terminal` grid per session, exposing a Unix socket (mode 0600 in `$XDG_RUNTIME_DIR`) with a versioned framed protocol (attach, detach, resize, input, screen snapshot + damage stream). The GUI becomes a client; on attach it receives a snapshot then deltas. Open questions: scrollback ownership/limits, multiple clients and resize arbitration, daemon lifecycle (socket activation vs. on demand), auth beyond file permissions, and how this interacts with the existing workspace JSON persistence.

## Image protocols
Candidates: Kitty graphics, Sixel, iTerm2 inline images. `alacritty_terminal` does not parse them, so the existing byte scanner in `src/osc.rs` (which watches the PTY stream beside the parser) is the natural hook for APC (Kitty) and DCS (Sixel) extraction. Needs: a placement model tied to grid cells that scrolls/clears with text, texture upload in egui, hard caps on decoded size and count, and chunked-transfer handling. Security: treat all payloads as hostile; no file/shared-memory transmission modes initially.
