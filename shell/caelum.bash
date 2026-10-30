# Caelum shell integration (OSC 7 cwd + OSC 133 prompt marks). Source from ~/.bashrc:
#   [ -n "$CAELUM_RS" ] && source /path/to/caelum.bash
[ -n "$CAELUM_RS" ] || return 0
# Percent-encode every byte outside [A-Za-z0-9/._~-] so control bytes (BEL, ESC),
# '%', spaces and non-ASCII bytes in a directory name cannot end or inject sequences.
__caelum_urlencode() {
  local LC_ALL=C s=$1 c h i
  REPLY=
  for (( i = 0; i < ${#s}; i++ )); do
    c=${s:i:1}
    case $c in
      [A-Za-z0-9/._~-]) REPLY+=$c ;;
      *) printf -v h '%%%02X' "'$c"; REPLY+=$h ;;
    esac
  done
}
__caelum_osc7() {
  local REPLY
  __caelum_urlencode "$PWD"
  printf '\e]7;file://%s%s\a' "${HOSTNAME}" "$REPLY"
}
__caelum_precmd() { local s=$?; printf '\e]133;D;%s\a' "$s"; __caelum_osc7; printf '\e]133;A\a'; return $s; }
PROMPT_COMMAND="__caelum_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS0='\e]133;C\a'"${PS0}"
