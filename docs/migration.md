# Migration coverage

The Electron project is preserved at https://github.com/DakodaStemen/caelum.
The Rust project has its own Git history, executable, app ID and profile. It does
not replace the old installation or migrate settings automatically.

| Capability | Rust preview |
| --- | --- |
| PTYs, shell input, Ctrl+C, ANSI/true color, alternate screen | Implemented; real PTY and native input tests |
| Workspaces, tabs, nested splits, resizing, zoom | Implemented |
| Theme presets and custom color import/export | Implemented; all ten source palettes retained |
| Terminal font, size, cursor, line height | Implemented; font discovery via system fontconfig |
| Per-interface font settings | Pending |
| Files, text editing, save conflict checks | Implemented |
| Unsaved-edit recovery | Implemented; conflicting disk changes retain a separate recovery artifact |
| Markdown rendering and image previews | Pending; text Markdown editing works |
| Embedded sandboxed browser | Pending; imported URLs open in the system browser |
| SSH and assistant terminals | Launch implemented; external tools and credentials remain user-managed |
| Live system and process views | Implemented on Linux |
| Literal terminal search | Literal forward/backward search through scrollback and visible highlighting |
| Clickable links | OSC 8 HTTP(S) links; detection of bare URLs pending |
| Layout restoration | Implemented; sessions restart rather than survive application exit |
| Full-screen TUI compatibility, mouse protocols, Unicode/IME | Terminal-core support exists; broad application and international-input validation pending |
| Screen-reader access to terminal content | Pending |
| Packaged distribution and signed updates | Pending; local installer and source build available |
| Measured speed, memory or latency improvements | Not established |

## Legacy workspace import

`--import-legacy PATH` reads the old JSON workspace into the Rust model and saves
it to a new Rust profile. It rejects an existing destination or unsupported
schema rather than replacing user data. IDs and layout structure are preserved.
Legacy `web` panes map to browser links. Unknown pane kinds fail import.
Legacy SSH and assistant pane fields are retained as explicit terminal commands
when supported by the importer; review the resulting layout before use.

The import does not restore old Electron browser state, running processes,
terminal history, unsaved Electron editor buffers, or interface font choices.
Import does not modify or write anything back to the legacy profile.

## Next gates

1. Expand terminal compatibility tests: representative editors, pagers and
   multiplexers; selection, wide characters, mouse reporting and input methods.
2. Complete Markdown/image previews and interface typography.
3. Prototype native embedded browsing and assess its isolation and packaging.
4. Audit accessibility and benchmark equivalent workflows against the preserved app.
5. Ship a replacement release only after the remaining compatibility gates pass.
