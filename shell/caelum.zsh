# Caelum shell integration. Source from ~/.zshrc:  [[ -n $CAELUM_RS ]] && source /path/to/caelum.zsh
[[ -n $CAELUM_RS ]] || return 0
# Percent-encode every byte outside [A-Za-z0-9/._~-] so control bytes (BEL, ESC),
# '%', spaces and non-ASCII bytes in a directory name cannot end or inject sequences.
__caelum_urlencode() {
  emulate -L zsh
  setopt no_multibyte
  local s=$1 c h safe='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789/._~-'
  local -i i
  REPLY=
  for (( i = 1; i <= ${#s}; i++ )); do
    c=$s[i]
    if [[ $safe == *"$c"* ]]; then
      REPLY+=$c
    else
      printf -v h '%%%02X' $(( #c & 255 ))
      REPLY+=$h
    fi
  done
}
__caelum_osc7() {
  local REPLY
  __caelum_urlencode "$PWD"
  print -rn -- $'\e]7;file://'"${HOST}${REPLY}"$'\a'
}
__caelum_precmd() {
  local s=$?
  print -n "\e]133;D;$s\a"
  __caelum_osc7
  print -n "\e]133;A\a"
}
__caelum_preexec() { print -n "\e]133;C\a"; }
autoload -Uz add-zsh-hook
add-zsh-hook precmd __caelum_precmd
add-zsh-hook preexec __caelum_preexec
