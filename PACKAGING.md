# Packaging

`release.yml` builds **unsigned** installers on `v*` tags (or manually):

| OS | Artifacts |
|----|-----------|
| Linux (Debian/Ubuntu) | `caelum-rs_<ver>_amd64.deb` (cargo-packager; install-tested in clean ubuntu:24.04), AppImage |
| Linux (Fedora) | `.rpm` (cargo-generate-rpm) |
| Linux (Arch) | `caelum-rs-<ver>-1-x86_64.pkg.tar.zst` (built with `makepkg` in an archlinux container) |
| Windows | `.msi` (WiX), `.zip` |
| macOS | `Caelum_<ver>_universal.dmg` (arm64 + x86_64, verified with `lipo -info`) |

On Linux, the package/binary name is `caelum-rs` across distributions (deb, rpm, pacman). On macOS, the product is `Caelum.app` (inside `Caelum_<ver>_universal.dmg`), and on Windows the executable is `caelum-rs.exe` (distributed in `.msi` and `.zip`).

The workflow has not been exercised on a tag yet; run it via *workflow_dispatch* and check the artifacts before relying on it.

## Needs the maintainer
- **Windows**: an Authenticode certificate, otherwise SmartScreen warns.
- **macOS**: an Apple Developer ID certificate plus notarization credentials, otherwise Gatekeeper blocks the dmg.
- **Icons**: only a PNG is shipped; add `icon.ico` / `icon.icns` to `package.metadata.packager.icons` for best results.
