# Security policy

## Supported versions

No version is publicly stable or supported yet. Private preview releases and
the `main` branch receive best-effort security fixes; a `0.x` package version
does not constitute a supported public release.

## Reporting a vulnerability

Do not open a public issue or paste a proof containing credentials, task
transcripts, provider state, filesystem paths, process details, Docker labels,
or owner records. Authorized collaborators should report privately through a
GitHub private vulnerability report/security advisory if that feature is
enabled for the repository. Otherwise contact the repository owner through an
already established private channel and ask for a secure transfer method.

Include only the minimum initially necessary:

- affected commit and `coding-agents --version` output;
- operating system, terminal context, and provider/runtime versions;
- whether the issue crosses visibility/authority or exact-target boundaries;
- a redacted reproduction using disposable state where possible;
- impact and whether exploitation requires local access, credentials, Docker
  access, or an already-owned provider session.

Do not test against another person's sessions, state directory, provider
account, or containers. Do not publish details until the maintainer confirms a
coordinated disclosure plan.

## Security boundaries

The primary invariants are documented in
[docs/control-model.md](docs/control-model.md). Security-sensitive areas
include:

- exact provider session/thread/turn/request identity and ownership;
- persisted PID start-token and command-line validation;
- single-process Codex response authority and replayed server requests;
- immutable Docker IDs, random instance labels, and the external owner record;
- private state directory/file ownership, modes, locks, and symlink refusal;
- terminal suspension/restoration, dynamic-text control-character sanitization,
  display-width bounds, and provider-native handoff;
- argument-array process execution, timeouts, and bounded output;
- accidental storage or display of credentials, secret input, or unbounded
  transcripts.

Docker daemon access and authenticated Claude/Codex CLIs are already powerful
local capabilities. Open Agent View does not sandbox those external tools. Its
responsibility is to avoid silently broadening their authority, adopting
unowned resources, leaking their data, or targeting an identity that was not
revalidated.

The project does not currently claim hardened multi-user isolation, hostile
local-root resistance, secure deletion, or protection from a compromised
provider CLI/Docker daemon. Managed Docker creation adds defense-in-depth but
does not make an untrusted image safe.

## Response expectations

Because this is a private pre-alpha, no response-time SLA is promised. A
maintainer should acknowledge a complete private report, assess whether users
must stop using a feature, prepare a tested fix and documentation, and decide
whether any future release/advisory is required. Security fixes must preserve
evidence without committing secrets or exploit artifacts.
