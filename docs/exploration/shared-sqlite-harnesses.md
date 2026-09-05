# Hermes Agent, MastraCode, and Devin

OAV supports Session Migrate's 18 coding harnesses, plus Terminal. These three
integrations open the native CLI in the foreground and read its local session
database. OAV does not modify provider history or copy credentials.

## Setup and use

```sh
oav setup hermes
oav setup mastracode
oav setup devin
```

Choose `/harness`, configure `/setup`, and type a task. Enter opens a session;
Shift+Left returns to the dashboard while it runs. Ctrl+R sets a local display
name. Ctrl+M includes all three as migration sources and targets when supported
by your installed `session-migrate`. Stop requires a frontend owned by this OAV
process. Local hide does not delete native history.

| Harness | Launch / exact resume | Setup | Models |
| --- | --- | --- | --- |
| Hermes Agent | `hermes chat --cli`; queued editor input. Resume: `hermes chat --cli --resume ID` | Official installer, then `hermes setup` | Saved session models or exact `--model` ID |
| MastraCode | Native TUI, `/new`, then editor input. Resume: `/threads`, search full UUID, select | `npm install --global mastracode`, then native onboarding | Saved thread models or exact `MASTRACODE_MODEL_ID` |
| Devin | `devin --model ID -- PROMPT`; resume: `devin --resume ID` | Official installer, then `devin auth login` | Native `devin models list --format json` |

Hermes and MastraCode do not expose a verified noninteractive account-wide
catalog here. Saved choices are not a guarantee of current account access. Use
their native setup/model picker to configure another provider or model. OAV
does not invent IDs or infer credentials from conversations.

## Native interfaces and platforms

Research baseline: Hermes Agent 0.20.6, MastraCode 0.37.1, Devin CLI 3000.6.7.
The reader checks required columns and reports incompatible schemas. It is not
the version-pinned database writer used by Session Migrate.

MastraCode's `--prompt` and `--thread` are **headless-only** in 0.37.1. OAV uses
its real `/threads` picker with the full UUID, preserving the stored resource
ID in `MASTRA_RESOURCE_ID`. It does not resume by title or choose the latest
thread. Live frontends reattach directly.

Hermes/MastraCode initial input is bracketed paste and waits for a native
editor marker. Hermes must show both its welcome text and ready editor, not
just the startup banner. MastraCode must acknowledge `/new` before receiving
the task; its empty startup thread is excluded from launch correlation.
This automation requires Unix PTYs (Linux/macOS). Windows can inspect their
saved sessions; automated TUI launch for these two is not advertised there.
Devin uses ordinary argv for launch/resume. None of these three advertises OAV
YOLO support without a dedicated permission-behavior verification gate.

## Storage and safety

| Harness | Database |
| --- | --- |
| Hermes | `$HERMES_HOME/state.db`, otherwise `~/.hermes/state.db` |
| MastraCode | `$MASTRA_DB_PATH`, then `$MASTRA_APP_DATA_DIR/mastra.db`, then platform app-data `mastracode/mastra.db` |
| Devin | Platform app-data `devin/cli/sessions.db` |

Platform app-data: `$XDG_DATA_HOME` or `~/.local/share` on Linux,
`~/Library/Application Support` on macOS, `%APPDATA%` on Windows.
Remote MastraCode LibSQL/PostgreSQL is outside this integration.

- Refresh queries exact owned IDs. External inventory is opt-in and capped at
  10,000 rows. SQLite lock waits are limited to 150 ms, execution to 500 ms;
  discovery runs off the TUI input thread.
- Bundled SQLite includes committed WAL frames and supports JSONB metadata.
  No system `sqlite3` or extra Python package is needed.
- Previews use active messages. Hermes excludes inactive history; Devin walks
  the active branch. Reasoning and tool output are not conversation previews.
- Missing stores are not created. Symlinked database files/sidecars, invalid
  IDs, relative stored workspaces, and incompatible schemas fail closed.
- Hermes may leave CWD null. OAV retains its launch workspace privately;
  ambiguous newly created identities are rejected.

## Tests

```sh
cargo test --locked --lib adapters::session_migrate_native::sqlite
cargo test --locked --test setup_installer
cargo test --locked --test real_tty
cargo test --locked --test readme_metadata
scripts/fresh-provider-setup-tests.sh hermes mastracode devin
```

Default tests cover isolated database shapes, WAL refresh, JSONB, active
branches, previews/timestamps, ownership after restart, absent/unknown stores,
malformed IDs, model parsing, and a 10,000-session lookup. Installer tests use
fixtures for consent, missing/existing binaries, failures, and login handoff.
The Docker script downloads actual clients in disposable homes; completing
browser/device login requires an account and is not claimed by fixture tests.

Read the databases written by the actual clients from a Session Migrate
checkout (the `native-session-corpus` CI job pins its revision):

```sh
OAV_NATIVE_CORPUS_ROOT=../session-migrate/tests/native_corpus/v1/sources \
  cargo test --locked --lib reads_actual_native_client_databases -- --ignored
```

To exercise actual Hermes/MastraCode terminals against a credential-free
local model, install `pyte` in your test Python environment, build OAV, then:

```sh
cargo build --locked --bin open-agent-view
python3 scripts/test-sqlite-native-tui.py hermes /absolute/path/to/hermes
python3 scripts/test-sqlite-native-tui.py mastracode /absolute/path/to/mastracode
```

These Unix probes use private temporary homes and a loopback HTTP model. They
launch the **installed native CLI**, complete a turn, return to OAV, reattach,
continue, restart OAV, resume the exact saved conversation, and complete a
third turn. They do not inherit API credentials. This verifies the CLI and
OAV integration, not a commercial model or OAuth service.

The fresh Hermes installer test permits up to 15 minutes for its Python,
compiler, browser, and Node dependencies. It suppresses npm's unrelated audit
request and answers npx package-install consent through `NPM_CONFIG_YES`.
These settings apply only to the disposable installer test, not user sessions.

References: [Hermes](https://github.com/NousResearch/hermes-agent),
[MastraCode](https://github.com/mastra-ai/mastra/tree/main/mastracode),
[Devin commands](https://docs.devin.ai/cli/reference/commands),
[Session Migrate storage research](https://github.com/xhluca/session-migrate#compatibility).
