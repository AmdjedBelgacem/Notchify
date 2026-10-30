# NotchForge shell helpers. Source from ~/.zshrc or ~/.bashrc:
#   source /path/to/integrations/shell/notchforge.sh
#
#   nf "Deploy done" "prod is green"      # one-off notification
#   nfwrap cargo build --release          # run anything; notify when it ends
#   nfwrap --source aider aider --yes     # attribute to an agent

nf() {
  notchforge-notify --source "${NF_SOURCE:-agent}" --title "${1:-Done}" --body "${2:-}" --best-effort
}

nfwrap() {
  local source="${NF_SOURCE:-agent}"
  if [[ "$1" == "--source" ]]; then source="$2"; shift 2; fi
  local start=$SECONDS
  "$@"
  local status=$?
  local elapsed=$(( SECONDS - start ))
  local cmd="$*"
  if (( status == 0 )); then
    notchforge-notify --source "$source" --kind task_finished --title "Finished: ${cmd:0:60}" \
      --body "Took ${elapsed}s" --best-effort
  else
    notchforge-notify --source "$source" --kind error --title "Failed ($status): ${cmd:0:60}" \
      --body "After ${elapsed}s" --best-effort
  fi
  return $status
}
