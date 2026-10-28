# Packaging

`release.yml` builds **unsigned** installers on `v*` tags (or manually):

| OS | Artifacts |
|----|-----------|
| Linux (Debian/Ubuntu) | `.deb` (cargo-packager), AppImage |
| Linux (Fedora) | `.rpm` (cargo-generate-rpm) |
| Linux (Arch) | `.pacman` package (cargo-packager) |
| Windows | `.msi` (WiX), `.zip` |
| macOS | universal (arm64 + x86_64) `.dmg` |

The workflow has not been exercised on a tag yet; run it via *workflow_dispatch* and check the artifacts before relying on it.

## Needs the maintainer
- **Windows**: an Authenticode certificate, otherwise SmartScreen warns.
- **macOS**: an Apple Developer ID certificate plus notarization credentials, otherwise Gatekeeper blocks the dmg.
- **Icons**: only a PNG is shipped; add `icon.ico` / `icon.icns` to `package.metadata.packager.icons` for best results.
