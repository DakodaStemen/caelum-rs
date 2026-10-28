# Caelum shell integration. Source from ~/.zshrc:  [[ -n $CAELUM_RS ]] && source /path/to/caelum.zsh
[[ -n $CAELUM_RS ]] || return 0
__caelum_precmd() {
  local s=$? p=${PWD//\%/%25}
  p=${p// /%20}
  print -n "\e]133;D;$s\a\e]7;file://${HOST}${p}\a\e]133;A\a"
}
__caelum_preexec() { print -n "\e]133;C\a"; }
autoload -Uz add-zsh-hook
add-zsh-hook precmd __caelum_precmd
add-zsh-hook preexec __caelum_preexec
