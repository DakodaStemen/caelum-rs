# Caelum shell integration (OSC 7 cwd + OSC 133 prompt marks). Source from ~/.bashrc:
#   [ -n "$CAELUM_RS" ] && source /path/to/caelum.bash
[ -n "$CAELUM_RS" ] || return 0
__caelum_osc7() { printf '\e]7;file://%s%s\a' "${HOSTNAME}" "$(printf %s "$PWD" | sed 's/%/%25/g; s/ /%20/g')"; }
__caelum_precmd() { local s=$?; printf '\e]133;D;%s\a' "$s"; __caelum_osc7; printf '\e]133;A\a'; return $s; }
PROMPT_COMMAND="__caelum_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS0='\e]133;C\a'"${PS0}"
