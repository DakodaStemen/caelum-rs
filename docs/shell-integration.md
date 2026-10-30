# Shell integration

Caelum sets `CAELUM_RS` in every shell it spawns. Snippets in [`shell/`](../shell) emit:

- **OSC 7** (`file://host/path`) after each prompt, so new panes/tabs open in the shell's current directory (including inside subshells; `/proc` is the fallback without integration). URIs naming a different host (e.g. over SSH) are ignored.
- **OSC 133** A/C/D prompt and command marks, enabling **Ctrl+Shift+Up / Ctrl+Shift+Down** to jump between prompts in scrollback (rebindable as `prev_prompt` / `next_prompt` in config.toml). Marks are dropped if scrollback is truncated rather than pointing at wrong lines.

Setup (guarded so other terminals are unaffected):

```sh
# ~/.bashrc
[ -n "$CAELUM_RS" ] && source /path/to/caelum-rs/shell/caelum.bash
# ~/.zshrc
[[ -n $CAELUM_RS ]] && source /path/to/caelum-rs/shell/caelum.zsh
# ~/.config/fish/config.fish
test -n "$CAELUM_RS"; and source /path/to/caelum-rs/shell/caelum.fish
```

Only the bash snippet syntax was checked locally (`bash -n`); zsh/fish snippets are untested.
