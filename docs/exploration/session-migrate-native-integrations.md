# Oh My Pi, Grok, Kilo Code, and OpenHands integrations

This note records the native contracts used to complete parity with the coding
harnesses supported by Session Migrate. Open Agent View does not import,
rewrite, or copy conversations. It reads each harness's own bounded session
inventory, launches the harness itself, and records only the exact session ID
created by that launch.

## Supported native surfaces

| Harness | Session inventory | Model catalog | New session | Resume | Login |
| --- | --- | --- | --- | --- | --- |
| Oh My Pi | Bounded JSONL journals under `$PI_CODING_AGENT_DIR` or `~/.omp/agent` | `omp models list --no-extensions --json` | `omp --model ID -- PROMPT` | `omp --resume PATH_OR_ID` | `omp --no-session`, then `/login` |
| Grok | `summary.json` plus `updates.jsonl` under `$GROK_HOME/sessions` | `grok models` | `grok --no-auto-update --model ID -- PROMPT` | `grok --no-auto-update --resume UUID` | `grok login` |
| Kilo Code | bounded read-only `kilo db "SELECT … FROM session" --format json` | `kilo models` | `kilo run --interactive --model ID PROMPT` | `kilo --session ID` | `kilo auth login` |
| OpenHands | Bounded event files under `$OPENHANDS_CONVERSATIONS_DIR` or `~/.openhands/conversations` | Saved `agent.llm.model` values and `LLM_MODEL`; exact IDs remain accepted | `LLM_MODEL=ID openhands --override-with-envs --task PROMPT` | `openhands --resume UUID` | `openhands login` |

The defaults and commands above were checked against the same upstream
interfaces used by Session Migrate. Provider credentials stay inside the
native CLI and its own state directory.

Primary upstream references:

- [Oh My Pi repository and CLI reference](https://github.com/can1357/oh-my-pi)
- [Grok Build repository and user guide](https://github.com/xai-org/grok-build)
- [Kilo Code CLI reference](https://kilo.ai/docs/code-with-ai/platforms/cli-reference)
- [OpenHands CLI repository](https://github.com/OpenHands/OpenHands-CLI)

## Ownership and safety boundary

- External history is excluded unless `--include-external` is explicit.
- JSON, JSONL, and event reads are size/count bounded and refuse symlinked
  roots or files.
- IDs, paths, model values, and visible text are validated before projection.
- A foreground launch snapshots the provider inventory before starting. OAV
  claims ownership only when exactly one new ID appears in the requested
  workspace. Zero or multiple candidates fail closed.
- The private OAV registry stores only ID, workspace, display name, creation
  time, and the provider path needed for exact resume. It does not store
  transcript bodies or credentials.
- Ctrl+X can stop only an exact OAV-owned native frontend retained by the
  current dashboard process. Saved external sessions remain inspect/open only.

## Verification

`src/adapters/session_migrate_native.rs` has parser tests for every native
shape, ID validation, bounded traversal, and model output. The isolated
integration test in `tests/session_migrate_native.rs` launches four real fake
executables through the same process and PTY boundaries as production, then
checks discovery, exact ownership correlation, model arguments, native resume,
login commands, and stop refusal after the frontend has exited.

`tests/setup_installer.rs` separately checks that each absent executable
requires consent, selects only the official installer/package, and gives the
exact native login command its own real PTY. The opt-in
`scripts/fresh-provider-setup-tests.sh` runs the current official installers in
credential-empty disposable containers.

This contract is intentionally narrower than Session Migrate's conversion
contract: OAV observes and opens provider-owned sessions; it does not convert
their contents.
