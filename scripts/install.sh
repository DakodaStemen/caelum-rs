#!/usr/bin/env bash
set -euo pipefail
project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source_binary="$project_dir/target/release/caelum-rs"
if [[ ! -x "$source_binary" ]]; then
  echo 'Build first: cargo build --release --locked' >&2
  exit 1
fi
install -Dm755 "$source_binary" "$HOME/.local/bin/caelum-rs"
install -Dm644 "$project_dir/assets/brand/mark-app.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/caelum-rs.svg"
mkdir -p "$HOME/.local/share/applications"
# Desktop Entry values use their own quoting rules, not shell interpolation.
binary_path="$HOME/.local/bin/caelum-rs"
escaped_binary="${binary_path//\\/\\\\}"
escaped_binary="${escaped_binary//\"/\\\"}"
escaped_binary="${escaped_binary//\`/\\\`}"
escaped_binary="${escaped_binary//\$/\\\$}"
escaped_binary="${escaped_binary//%/%%}"
cat > "$HOME/.local/share/applications/io.github.DakodaStemen.CaelumRust.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Caelum Rust
Comment=Native Rust workspace terminal
Exec="$escaped_binary"
Icon=caelum-rs
Terminal=false
Categories=System;TerminalEmulator;
StartupWMClass=io.github.DakodaStemen.CaelumRust
DESKTOP
if command -v update-desktop-database >/dev/null; then update-desktop-database "$HOME/.local/share/applications"; fi
printf 'Installed %s\n' "$binary_path"
