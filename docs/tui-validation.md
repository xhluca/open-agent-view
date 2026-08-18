# Real-TTY and visual validation

This guide separates four different claims that must not be conflated:

1. Ratatui test-backend tests validate deterministic layout and key routing.
2. A real PTY validates terminal modes, actual key encoding, and restoration.
3. A fresh credential-free Docker environment validates isolation and
   empty/synthetic rendering, but cannot validate an authenticated task.
4. A credentialed provider lifecycle validates real provider behavior and is
   opt-in because it consumes external service access and exposes task state.

The committed [validation record](testing.md) says which layers have actually
passed. This document is the reproducible procedure and release checklist; a
checklist item is not evidence until its result is recorded with a commit.

## Build and record the subject

From a clean checkout:

```console
cargo +1.75.0 test --locked
cargo +1.75.0 build --release --locked
git rev-parse HEAD
sha256sum target/release/coding-agents
target/release/coding-agents --version
docker image inspect \
  sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392 \
  --format '{{.Id}} {{json .RepoTags}}'
```

The image ID above is the locally investigated `basic-claude-uv:latest`
environment, containing Claude Code 2.1.209 and Codex 0.144.4. The immutable ID
is the test identity; the mutable tag is only a convenience. If that image is
not present on another machine, the commands are reproducible only after an
authorized equivalent image is built or supplied. Do not silently substitute a
new tag and cite the old result.

## Local real-PTY matrix

First validate the synthetic populated screen without contacting providers:

```console
target/release/coding-agents \
  --fixture fixtures/populated-sessions.json \
  --no-host-claude \
  --no-host-codex
```

The canonical fixture contains nine sessions spanning all normalized states
and current actionable capabilities. Fixture mode itself fences every provider operation, including
launch and native open; all attempted operations end in an explicit local
refusal. It is a UI exercise, not proof of provider authority or lifecycle
success.

Run the automated real-PTY harness first:

```console
scripts/real-tui-tests.sh
```

This runs `cargo test --locked --test real_tty -- --test-threads=1`. It uses
`libc::openpty`, feeds real key bytes, parses output with VT100 semantics, and
isolates `HOME` and `XDG_STATE_HOME`. Five tests create PTYs at 120×34, 105×30,
100×28, 90×24, 55×18, and 31×7 (the narrow/fallback test creates two PTYs).
They cover startup/sections, help, status/directory grouping, filter
apply/cancel/clear, multiline launch/cancellation, inspect peek, rename
cancellation/submission, native-open suspend/restore, reply, interrupt,
approval `y`/`n`, structured input, single/bulk delete, archive, safe
fixture-fenced refusals without echoing submitted text into notices, real arrow
navigation/group collapse, bounded narrow rendering, the tiny-terminal
fallback, and alternate-screen/cursor restoration.

Repeat at these terminal sizes and record a capture for each materially
different layout: reference-wide (at least 160×40), ordinary 120×30, 80×24,
and narrow 60×20. At minimum verify:

- the logo/header, runtime context, counts, headings, rows, right-aligned
  metadata, composer, separators, and footer retain a readable hierarchy;
- long names/summaries truncate cleanly without overwriting metadata or
  spilling into the next line, including CJK and multi-code-point graphemes;
- provider text containing newlines, tabs, Escape, or C0/C1 controls cannot
  move the cursor, alter style, or inject a new terminal control sequence;
- compact `C@H`/`C@D`/`X@H`/`X@D` markers align across rows while Peek expands
  the full provider and runtime label;
- every state is present in reference order and the selected row is apparent;
- 60×20 retains a discoverable `?` help affordance and usable composer;
- no stale glyphs remain after changing view, filter, overlay, or selection;
- colors remain distinguishable on the reference dark theme and still convey
  state through text/symbols when color is unavailable.

### Exhaustive safe key route

Exercise every entry independently so one failed mutation does not change the
fixture expected by the next entry:

| Route | Keys / expected result |
| --- | --- |
| Cyclic selection | Repeated `↓`, then repeated `↑`; headings and rows wrap at both ends. |
| Collapse | Move to each heading, `enter`, then `enter`; its rows disappear and return without losing a valid selection. |
| Directory view | `ctrl+s`; rows regroup by working directory, then `ctrl+s` restores status order. |
| Filter | `/`, type `codex`, edit with Backspace, `enter`; only matches remain. `/`, erase all text, `enter` restores all rows. |
| Help | `?`; inspect the contextual rows, then close with `?`. Repeat and close with `enter`, then with `esc`. |
| New task | `tab`, type text, `ctrl+j`, type another line, then `esc` to discard. Repeat and `enter`; the fixture-mode refusal is expected and must not repeat the prompt. Also verify a printable key from the list seeds the composer. |
| Rename | Select a row, `ctrl+r`, edit the prefilled name, `esc`. Repeat and `enter`; the explicit unsupported notice must not repeat the proposed name. |
| Inspect | Select `release-reviewer`, `space`; peek opens without raw escape sequences. `space` closes it and `esc` also closes it. |
| Native open | Select any row and `enter`; fixture mode refuses before invoking a provider, and the dashboard is restored. Repeat with empty peek then `enter`. |
| Reply / steer | Select `owned-codex-worker`, `space`, type text, `enter`; the fixture-mode refusal appears, the text is cleared, and the notice does not repeat it. |
| Approval | Select `approval-needed`, `space`, press `y`; a fixture-mode refusal is visible. Restart the fixture and repeat with `n`. The footer advertises only capabilities in the row. |
| Structured input | Select `needs-environment`, `space`, type an answer, `enter`; it uses the distinct response route, reports the fixture refusal, clears the input, and does not repeat the answer in the notice. |
| Interrupt confirmation | Select `owned-codex-worker`, `ctrl+x`; the prompt names the exact active ID and `esc` cancels. Repeat, then `enter`; the fixture-mode refusal is visible. |
| Delete confirmation | Select `schema-migration`, `ctrl+x`; `esc` cancels. Repeat, then `enter`; the fixture-mode refusal is visible. |
| Archive confirmation | Select `schema-migration`, `ctrl+a`; `esc` cancels. Repeat, then `enter`; the fixture-mode refusal is visible. |
| Bulk safety | On an active status heading, `ctrl+x` refuses bulk stop. Filter for `migration`, move to Completed, and `ctrl+x`; the prompt names both deletable rows. Verify `esc`, then repeat with `enter` for a fixture-mode refusal. |
| Unsupported controls | On `legacy-session`, `space`, `ctrl+a`, and `ctrl+x` show authority/read-only notices rather than opening dangerous modes. |
| Escape restoration | From the plain session list press `esc`; the prior terminal screen, visible cursor, echo, and ordinary line input return. |

Approval request text and sequential-question contents are supplied by the
mock App Server tests, not this normalized session fixture. The fixture verifies
their capability-aware TUI routes; the mocks verify exact opaque request IDs,
turn ownership, replay, answer normalization, and provider response payloads.

## Fresh Docker: Claude reference TUI

Run the reference in its own fresh, automatically removed, network-disabled
container. This mounts no host credentials or workspace:

```console
OAV_PROBE_IMAGE=sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392
docker run --rm --interactive --tty \
  --name oav-claude-tui-probe \
  --network none \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges \
  --pids-limit 256 \
  --tmpfs /tmp:rw,nosuid,nodev,size=64m,mode=1777 \
  --tmpfs /home/oav:rw,nosuid,nodev,size=64m,uid=65532,gid=65532,mode=0700 \
  --env HOME=/home/oav \
  --env XDG_STATE_HOME=/home/oav/state \
  --user 65532:65532 \
  --workdir /tmp \
  --entrypoint /usr/local/bin/claude \
  "$OAV_PROBE_IMAGE" agents
```

Capture the empty grouping, header spacing, footer, composer, selection marker,
and help overlay at 120×34, 55×18, and 31×7. Exercise `?`, every harmless
navigation/collapse key available in the empty state, and Escape. Record the
exact Claude version separately:

```console
docker run --rm --network none \
  --entrypoint /usr/local/bin/claude \
  "$OAV_PROBE_IMAGE" --version
```

## Fresh Docker: Open Agent View empty TUI

Cargo artifacts can inherit a private umask (and this checkout does), so stage
an executable copy in a temporary traversable directory before mounting it as
an unprivileged container user. Mount only that copy into a separate container:

```console
OAV_PROBE_IMAGE=sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392
OAV_PROBE_DIR="$(mktemp -d)"
trap 'rm -r -- "$OAV_PROBE_DIR"' EXIT
chmod 0755 "$OAV_PROBE_DIR"
install -m 0755 target/release/coding-agents "$OAV_PROBE_DIR/coding-agents"
OAV_BINARY="$OAV_PROBE_DIR/coding-agents"
docker run --rm --interactive --tty \
  --name oav-empty-tui-probe \
  --network none \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges \
  --pids-limit 256 \
  --tmpfs /tmp:rw,nosuid,nodev,size=64m,mode=1777 \
  --tmpfs /home/oav:rw,nosuid,nodev,size=64m,uid=65532,gid=65532,mode=0700 \
  --env HOME=/home/oav \
  --env XDG_STATE_HOME=/home/oav/state \
  --user 65532:65532 \
  --workdir /tmp \
  --volume "$OAV_BINARY:/usr/local/bin/coding-agents:ro" \
  --entrypoint /usr/local/bin/coding-agents \
  "$OAV_PROBE_IMAGE" --no-host-claude --no-host-codex
```

Compare the same empty-state landmarks with the Claude capture. Exercise help,
both grouping modes, filter/new-task composers, cancellation, and Escape. The
container must disappear after exit (`docker ps -a --filter
name=oav-empty-tui-probe` should show no row).

## Fresh Docker: synthetic populated TUI

Mount the documentation fixture read-only in a third fresh container. This is
the isolated way to run the exhaustive safe key route above:

```console
OAV_PROBE_IMAGE=sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392
OAV_PROBE_DIR="$(mktemp -d)"
trap 'rm -r -- "$OAV_PROBE_DIR"' EXIT
chmod 0755 "$OAV_PROBE_DIR"
install -m 0755 target/release/coding-agents "$OAV_PROBE_DIR/coding-agents"
install -m 0644 fixtures/populated-sessions.json "$OAV_PROBE_DIR/populated-sessions.json"
OAV_BINARY="$OAV_PROBE_DIR/coding-agents"
OAV_FIXTURE="$OAV_PROBE_DIR/populated-sessions.json"
docker run --rm --interactive --tty \
  --name oav-populated-tui-probe \
  --network none \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges \
  --pids-limit 256 \
  --tmpfs /tmp:rw,nosuid,nodev,size=64m,mode=1777 \
  --tmpfs /home/oav:rw,nosuid,nodev,size=64m,uid=65532,gid=65532,mode=0700 \
  --env HOME=/home/oav \
  --env XDG_STATE_HOME=/home/oav/state \
  --user 65532:65532 \
  --workdir /tmp \
  --volume "$OAV_BINARY:/usr/local/bin/coding-agents:ro" \
  --volume "$OAV_FIXTURE:/fixtures/populated-sessions.json:ro" \
  --entrypoint /usr/local/bin/coding-agents \
  "$OAV_PROBE_IMAGE" \
  --fixture /fixtures/populated-sessions.json \
  --no-host-claude \
  --no-host-codex
```

This probe has no network, credentials, live provider state, writable
workspace, or Docker socket. Its synthetic capabilities cannot become real
authority.

## Optional tmux capture

`tmux` makes dimensions and text captures repeatable while still allocating a
real PTY. Use a unique session and a command from above:

```console
tmux new-session -d -s oav-tui-check -x 120 -y 30 \
  'target/release/coding-agents --fixture fixtures/populated-sessions.json --no-host-claude --no-host-codex'
tmux capture-pane -e -p -t oav-tui-check
tmux send-keys -t oav-tui-check '?'
tmux capture-pane -e -p -t oav-tui-check
tmux send-keys -t oav-tui-check Escape
tmux send-keys -t oav-tui-check Escape
tmux kill-session -t oav-tui-check
```

For visual evidence, retain terminal screenshots as external test artifacts;
ANSI text capture cannot prove exact colors or every border glyph. Do not add
screenshots containing real task prompts or credentials to the repository.

## Credentialed lifecycle gate

A full provider gate requires explicit authorization to mount a dedicated test
identity and enable network access. Never reuse personal production state by
default. Use disposable tasks/workspaces, record the provider versions, and
exercise launch, discovery, inspect, native open/return, reply/steer, each
supported approval/input type, interrupt, completion, archive/delete, and
dashboard restart/reconnect. Managed Docker lifecycle additionally requires an
approved digest-pinned image and dedicated empty state home.

The current claim must remain narrower until that gate is recorded:
fresh-container tests prove real-TTY rendering, empty interaction, and
credential-free synthetic startup; deterministic mocks prove lifecycle
protocol behavior. They do not prove a real authenticated task in a fresh
container.

## Evidence record template

Record enough context to make a result auditable:

```text
date/timezone:
git commit and worktree status:
binary version and SHA-256:
terminal + TERM + dimensions:
host/SSH/tmux context:
container image immutable ID:
Claude/Codex/Docker versions:
exact command (secrets omitted):
keys/actions exercised:
expected vs observed:
terminal restoration result:
artifact locations and redactions:
known omissions:
```
