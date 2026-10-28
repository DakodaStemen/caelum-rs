# Third-party notices

Caelum Rust is MIT licensed. Its dependencies retain their own licenses.
`Cargo.lock` records the exact dependency graph.

Core components include:

- Alacritty terminal core: Apache-2.0 — https://github.com/alacritty/alacritty
- egui/eframe: MIT OR Apache-2.0 — https://github.com/emilk/egui
- portable-pty: MIT — https://github.com/wezterm/wezterm
- serde/serde_json: MIT OR Apache-2.0 — https://github.com/serde-rs
- image: MIT OR Apache-2.0 — https://github.com/image-rs/image

The supplied Caelum SVG is retained as an icon without its lettering. Source SVGs
are in `assets/brand`; `mark-app.svg` adds weight for small-size readability, and
`icon.png` is its application rendering. Rust does not remove licensing obligations
for bundled assets, fonts or third-party code.
