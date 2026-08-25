#!/usr/bin/env bash

set -euo pipefail

provider="${1:?provider is required}"
export HOME=/tmp/oav-home
export XDG_CONFIG_HOME="${HOME}/.config"
export XDG_CACHE_HOME="${HOME}/.cache"
export XDG_STATE_HOME="${HOME}/.local/state"
export PATH="${HOME}/.local/bin:${HOME}/.opencode/bin:/usr/local/bin:/usr/bin:/bin"
export MUSE_INSTALL_DIR="${HOME}/.local/bin"
export MUSE_NO_MODIFY_PATH=1
export KIMI_INSTALL_DIR="${HOME}/.local"
export KIMI_NO_MODIFY_PATH=1
export KIMI_CODE_HOME="${HOME}/.kimi-code"
export QWEN_NO_MODIFY_PATH=1
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"

log="/tmp/${provider}-setup.log"
set +e
timeout 120 script -qefc "/usr/local/bin/open-agent-view setup '${provider}' --yes" "$log" \
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
  mistral-vibe) executable="$(command -v vibe)" ;;
  muse) executable="$(command -v muse)" ;;
  qwen) executable="$(command -v qwen)" ;;
  kimi) executable="$(command -v kimi)" ;;
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

timeout 20 "$executable" --help >/dev/null 2>&1 || {
  printf 'installed %s executable did not expose help\n' "$provider" >&2
  exit 1
}

case "$provider" in
  mistral-vibe)
    app_server="$(command -v vibe-app-server)"
    [[ -x "$app_server" ]] || {
      printf 'Mistral Vibe installer omitted vibe-app-server\n' >&2
      exit 1
    }
    # Exercise the same documented JSON-RPC surfaces as the adapter, using a
    # fresh process per request and no credential files. Node is part of the
    # pinned setup image; the provider process itself remains the system under
    # test.
    timeout 30 node - "$app_server" <<'NODE'
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const readline = require('node:readline');

async function request(binary, method, params, validate) {
  const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const found = new Promise((resolve, reject) => {
    const lines = readline.createInterface({ input: child.stdout });
    lines.on('line', (line) => {
      let message;
      try { message = JSON.parse(line); } catch (error) { reject(error); return; }
      if (message.id === 2) {
        if (message.error) reject(new Error(JSON.stringify(message.error)));
        else {
          try { validate(message.result); resolve(); } catch (error) { reject(error); }
        }
      }
    });
  });
  for (const message of [
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { clientInfo: { name: 'open-agent-view-fresh-test', version: '0', entrypoint: 'programmatic' }, capabilities: {} } },
    { jsonrpc: '2.0', method: 'initialized', params: {} },
    { jsonrpc: '2.0', id: 2, method, params },
  ]) child.stdin.write(`${JSON.stringify(message)}\n`);
  child.stdin.end();
  await Promise.race([
    found,
    new Promise((_, reject) => {
      const timer = setTimeout(() => reject(new Error(`timed out: ${stderr}`)), 15000);
      timer.unref();
    }),
  ]);
  if (child.exitCode === null) {
    await Promise.race([
      once(child, 'exit'),
      new Promise((_, reject) => {
        const timer = setTimeout(() => reject(new Error('app-server did not exit after stdin EOF')), 5000);
        timer.unref();
      }),
    ]);
  }
  if (child.exitCode !== 0) throw new Error(`app-server exited ${child.exitCode}: ${stderr}`);
}

(async () => {
  const binary = process.argv[2];
  await request(binary, 'session/list', { limit: 1 }, (result) => {
    if (!result || !Array.isArray(result.items)) throw new Error('session/list omitted items');
  });
  await request(binary, 'config/read', { cwd: process.env.HOME }, (result) => {
    if (!result || !result.config || !Array.isArray(result.config.models)) {
      throw new Error('config/read omitted config.models');
    }
  });
})().catch((error) => { console.error(error); process.exit(1); });
NODE
    ;;
  muse)
    timeout 30 "$executable" exec --provider echo 'OAV isolated probe' \
      >"/tmp/${provider}-auth-free.log" 2>&1 || {
        printf 'Muse auth-free echo provider probe failed\n' >&2
        tail -n 40 "/tmp/${provider}-auth-free.log" >&2
        exit 1
      }
    grep -Fq 'OAV isolated probe' "/tmp/${provider}-auth-free.log" || {
      printf 'Muse auth-free echo provider omitted its probe text\n' >&2
      exit 1
    }
    ;;
  kimi)
    timeout 30 "$executable" provider list --json \
      >"/tmp/${provider}-auth-free.log" 2>&1 || {
        printf 'Kimi auth-free provider catalog probe failed\n' >&2
        tail -n 40 "/tmp/${provider}-auth-free.log" >&2
        exit 1
    }
    ;;
  qwen)
    timeout 30 "$executable" sessions list --json --limit 1 \
      >"/tmp/${provider}-history.log" 2>&1 || {
        printf 'Qwen auth-free history JSONL probe failed\n' >&2
        tail -n 40 "/tmp/${provider}-history.log" >&2
        exit 1
      }
    timeout 30 "$executable" sessions ps --json \
      >"/tmp/${provider}-live.log" 2>&1 || {
        printf 'Qwen auth-free live-session JSONL probe failed\n' >&2
        tail -n 40 "/tmp/${provider}-live.log" >&2
        exit 1
      }
    ;;
esac

printf '%-12s installed=%s login_handoff=pty version=%s\n' "$provider" "$executable" "$version"
