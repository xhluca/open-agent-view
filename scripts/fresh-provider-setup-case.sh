#!/usr/bin/env bash

set -euo pipefail

provider="${1:?provider is required}"
export HOME=/tmp/oav-home
export XDG_CONFIG_HOME="${HOME}/.config"
export XDG_CACHE_HOME="${HOME}/.cache"
export XDG_STATE_HOME="${HOME}/.local/state"
export PATH="${HOME}/.local/bin:${HOME}/.opencode/bin:/usr/local/bin:/usr/bin:/bin"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"

log="/tmp/${provider}-setup.log"
set +e
timeout 45 script -qefc "/usr/local/bin/open-agent-view setup '${provider}' --yes" "$log" \
  >"/tmp/${provider}-setup.console" 2>&1
status=$?
set -e

# Interactive OAuth may wait for a browser or device confirmation. Installation
# must still have completed, and the native login process must have reached a
# real PTY before that bounded timeout.
case "$status" in
  0|124|137|143) ;;
  *)
    printf 'setup exited unexpectedly for %s (status %s)\n' "$provider" "$status" >&2
    tail -n 80 "$log" >&2
    exit 1
    ;;
esac

case "$provider" in
  claude) executable="$(command -v claude)" ;;
  codex) executable="$(command -v codex)" ;;
  pi) executable="$(command -v pi)" ;;
  opencode) executable="$(command -v opencode)" ;;
  cursor) executable="$(command -v cursor-agent)" ;;
  copilot) executable="$(command -v copilot)" ;;
  antigravity) executable="$(command -v agy)" ;;
  *) printf 'unknown provider: %s\n' "$provider" >&2; exit 2 ;;
esac

[[ -x "$executable" ]] || {
  printf 'setup did not install %s\n' "$provider" >&2
  exit 1
}

grep -Eiq 'install|download|auth|login|browser|device|setup|welcome|provider' "$log" || {
  printf 'setup never reached an observable install/login state for %s\n' "$provider" >&2
  tail -n 80 "$log" >&2
  exit 1
}

version="$($executable --version 2>&1 | grep -m1 -E '[0-9]+\.[0-9]+' || true)"
[[ -n "$version" ]] || {
  printf 'installed %s executable did not report a version\n' "$provider" >&2
  exit 1
}

printf '%-12s installed=%s login_handoff=pty version=%s\n' "$provider" "$executable" "$version"
