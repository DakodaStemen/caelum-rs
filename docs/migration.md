# Feature status

Caelum has its own executable, app ID and profile.

| Capability | Status |
| --- | --- |
| PTYs, shell input, Ctrl+C, ANSI/true color, alternate screen | Implemented; real PTY and native input tests |
| Workspaces, tabs, nested splits, resizing, zoom | Implemented |
| Theme presets and custom color import/export | Implemented; all ten source palettes retained, plus the new Graphite default |
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

## Workspace import

`--import-legacy PATH` reads an exported JSON workspace and saves
it to a new profile. It rejects an existing destination or unsupported
schema rather than replacing user data. IDs and layout structure are preserved.
`web` panes map to browser links. Unknown pane kinds fail import.
SSH and assistant pane fields are retained as explicit terminal commands
when supported by the importer; review the resulting layout before use.

The import does not restore browser state, running processes, terminal history,
unsaved editor buffers, or interface font choices. It never writes back to the source file.

## Next gates

1. Expand terminal compatibility tests: representative editors, pagers and
   multiplexers; selection, wide characters, mouse reporting and input methods.
2. Complete Markdown/image previews and interface typography.
3. Prototype native embedded browsing and assess its isolation and packaging.
4. Audit accessibility and benchmark common workflows.
5. Ship a packaged release once the remaining gates pass.
