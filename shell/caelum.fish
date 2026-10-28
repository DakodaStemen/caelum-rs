# Caelum shell integration. Source from ~/.config/fish/config.fish:
#   test -n "$CAELUM_RS"; and source /path/to/caelum.fish
test -n "$CAELUM_RS"; or exit 0
function __caelum_prompt --on-event fish_prompt
    printf '\e]133;D;%s\a' $status
    printf '\e]7;file://%s%s\a' (hostname) (string escape --style=url -- $PWD)
    printf '\e]133;A\a'
end
function __caelum_preexec --on-event fish_preexec
    printf '\e]133;C\a'
end
