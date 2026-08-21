# Authentication, model catalogs, setup, and foreground launch

Date: 2026-08-21. This note records the evidence behind the guided onboarding
flow. It distinguishes read-only observations from deterministic fake-account
tests. No login or provider install was performed. One bounded Claude
background-contract probe was created and then stopped by its exact returned
ID; no other model-backed task was submitted.

## Product contract

The dashboard follows one sequence for every launch-capable provider:

1. If the CLI is missing, `open-agent-view setup HARNESS` names the official
   source, asks for confirmation, and retains native download/install progress.
2. Shift+Tab asks the installed provider for its account-visible model catalog
   on a worker thread. OAV does not ship a guessed cross-account list.
3. If the catalog says authentication is required, Enter or `l` suspends the
   alternate screen and runs the provider's native login UI. `/login` exposes
   the same handoff explicitly.
4. On return, OAV restores the dashboard and reloads the same catalog. It never
   reads, copies, or logs the resulting credential.
5. The exact selected ID is revalidated by the provider adapter and sent on the
   launch protocol. Background launches animate in the dashboard; foreground
   launches own the terminal and reserve Left for returning to OAV.

## Provider surfaces

| Provider | Native setup/authentication | Account/model surface | Selected-model launch |
| --- | --- | --- | --- |
| Claude Code | `claude auth login` | aliases advertised next to `--model` in installed `claude --help` | `--model`; host launch lets `--background` allocate the ID, resolves the exact full row, then `attach` |
| OpenAI Codex | `codex login` | App Server `model/list` pages | App Server `thread/start` model |
| Pi | `pi --no-session`, then `/login` in Pi | `pi --offline --list-models` | exact `--model` on the owned RPC child |
| OpenCode | `opencode auth login` | `opencode models` | exact `providerID`/`modelID` in `prompt_async` |
| Cursor | `cursor-agent login` | `cursor-agent models` | exact `--model` on the managed stream-JSON turn |
| GitHub Copilot | `copilot login` | short-lived `copilot --headless --stdio` SDK connection, `models.list` | ACP `session/set_config_option` before the first prompt |
| Antigravity | first-run `agy` browser flow | `agy models` | `agy --sandbox --model ID --prompt-interactive PROMPT` |

Primary provider references:

- [Cursor CLI authentication](https://docs.cursor.com/en/cli/reference/authentication),
  [installation](https://docs.cursor.com/en/cli/installation), and
  [parameters](https://docs.cursor.com/en/cli/reference/parameters)
- [GitHub Copilot ACP server](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server),
  [CLI command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference),
  and [installation](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli)
- [Antigravity CLI source and installation](https://github.com/google-antigravity/antigravity-cli)
- [Claude agent view](https://code.claude.com/docs/en/agent-view)

## Reporting-account read-only observations

- Claude Code 2.1.238 returned 19 `claude agents --json --all` rows in roughly
  0.42 seconds. A bounded disposable background probe then returned
  `backgrounded · SHORT_ID`; it was stopped by that exact ID and verified
  stopped. The CLI's own strings and help confirmed that `--bg` manages the
  session ID and ignores a simultaneous `--session-id`.
- Cursor Agent `2026.03.20-44cb435` returned `No models available for this
  account`. This was the exact failure that formerly produced only a manual
  `cursor-agent login` instruction.
- GitHub Copilot CLI 1.0.80 returned its documented ACP `Authentication
  required` error. No login or session creation was attempted.
- Antigravity 1.1.17 printed its model-fetching state and did not return a
  catalog inside the bounded read-only probe. Its redacted native log showed
  the reported launch failure was `neither PlanModel nor RequestedModel
  specified`, which OAV now prevents by requiring an exact model. No token or
  account identifier was read or recorded.
- Pi's installed help documents `--model`, `--models`, `--list-models`, and its
  interactive OAuth `/login`; `pi auth` itself exposes credential
  print/readiness commands rather than a generic provider-selection login.
- OpenCode's installed `auth --help` documents `auth login [url]`; its read-only
  `auth list` showed configured provider names but no tokens were read or
  emitted.

These observations justify provider-native login handoff and account-scoped
catalogs. They are not evidence that the reporting account can invoke every
listed model.

## Deterministic real-terminal validation

The actual release binary runs under `libc::openpty` with isolated temporary
homes and provider executables:

- Cursor begins signed out, renders the catalog error inside the model picker,
  hands Enter to an interactive login, reloads two exact model IDs, retains the
  task draft, selects `claude-sonnet-4.6`, and passes it to a deliberately slow
  managed launch. The launch spinner remains visible and the new exact row is
  selected.
- Copilot begins signed out, returns the real LSP-framed headless authentication
  error, hands `l` to native login, reloads exact SDK model IDs without creating
  a session, and retains/selects the draft/model.
- Claude accepts a background launch, returns its provider-owned ID, waits for
  the exact UUID in the agent JSON inventory, opens full-screen attach, and returns on
  Left to the newly selected row.
- Antigravity begins signed out, completes its first-run handoff, reloads an
  exact model, launches full-screen with `--sandbox`, selected `--model`, and
  `--prompt-interactive`, then returns on Left to the exact OAV-owned cached
  conversation. The captured argv contains no dangerous permission bypass.

An isolated installer integration test puts fake `curl` and `bash` in a private
`PATH`. It proves setup without a TTY and without `--yes` invokes nothing;
confirmed setup shows both download and provider progress, uses the exact
official URL, executes a staged file, and removes it afterward.

## Boundaries

- A successful catalog is not a credential oracle and does not prove a paid
  account, region, or model entitlement will accept a prompt.
- Copilot managed authority remains tied to the retained ACP connection even
  though catalog discovery uses a separate short-lived headless process.
- Cursor still has no documented machine-readable global session list; OAV
  discovers only its own managed records.
- Antigravity's documented cache exposes only the last conversation per
  workspace. OAV cannot truthfully recover an arbitrary older owned
  conversation after that cache entry changes.
- Pi's setup handoff opens its no-session native TUI because its `pi auth`
  subcommands do not expose a provider-neutral interactive login command.
- Real-account observations remain on the host. Credential/config directories
  were deliberately not copied into Docker; fresh-container probes use empty
  homes and prove unauthenticated protocol/error handling, not account access.
