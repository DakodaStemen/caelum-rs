# Contributing

Preserve the reference app and its settings. Implement changes in this repository
and test them with a separate profile.

Run formatting, Clippy and the Rust tests before submitting changes. Changes to
terminal input, pane focus or lifecycle also need native desktop validation.
Report executed checks and their limitations; do not claim feature parity or
performance improvements from language choice alone.

Keep terminal semantics in Alacritty, PTY ownership in `terminal.rs`, document
saving and recovery in `files.rs`, and workspace serialization in `model.rs`.

Keep `docs/demo.gif` around 1 MB. Re-record it sparingly, only when the UI changes
visibly, to avoid bloating repository history.
