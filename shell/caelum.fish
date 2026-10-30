# Caelum shell integration. Source from ~/.config/fish/config.fish:
#   test -n "$CAELUM_RS"; and source /path/to/caelum.fish
test -n "$CAELUM_RS"; or exit 0
# `string escape --style=url` percent-encodes every byte outside [A-Za-z0-9/._~-]
# (control bytes such as BEL and ESC, '%', spaces, non-ASCII and invalid UTF-8 bytes),
# so a directory name cannot end the OSC 7 sequence or inject another one.
function __caelum_osc7
    printf '\e]7;file://%s%s\a' $hostname (string escape --style=url -- $PWD)
end
function __caelum_prompt --on-event fish_prompt
    printf '\e]133;D;%s\a' $status
    __caelum_osc7
    printf '\e]133;A\a'
end
function __caelum_preexec --on-event fish_preexec
    printf '\e]133;C\a'
end
