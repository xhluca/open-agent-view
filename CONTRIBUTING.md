# Contributing

Open Agent View is currently developed in a private repository. Contributions
are accepted from authorized collaborators; public contribution and support
channels have not been announced.

## Before changing code

Read the [product specification](docs/product-spec.md),
[architecture](docs/architecture.md), and especially the
[control/ownership model](docs/control-model.md). Discovery does not imply
authority. A new adapter or action must fail closed when identity, ownership,
provider state, or protocol support is uncertain.

Never use existing personal sessions or long-running containers as mutation
targets. In particular, preserve the protected `webqwen-sbx-*` and
`at-codex-*` environments named in the product specification. Active probes
must use explicit disposable targets, dedicated state, and the minimum network
or credential access required by the test.

## Development setup

The minimum supported Rust version is 1.75.0:

```console
rustup toolchain install 1.75.0 --profile minimal
cargo +1.75.0 test --locked
cargo +1.75.0 build --release --locked
```

Keep `Cargo.lock` committed and use `--locked` in validation and install
commands. Do not update dependencies incidentally. The implementation uses
provider argv arrays and bounded subprocesses; do not introduce shell
interpolation for session, prompt, path, or container values.

## Change expectations

- Keep provider parsing in adapters and the normalized model provider-neutral.
- Advertise a capability only after the exact action is supported and its
  ownership proof is current.
- Preserve unknown provider states/fields for compatibility and keep failures
  in one adapter from hiding healthy sessions in another.
- Require an exact target and explicit confirmation for destructive actions.
- Never persist structured answers, credentials, or raw secrets in fixtures,
  diagnostics, screenshots, or commits.
- Update operator documentation, the roadmap, and changelog when behavior or
  safety boundaries change.
- Make coherent commits with tests and documentation that describe the same
  state. Do not mark a path verified merely because code was added.

## Validation

At minimum run:

```console
cargo +1.75.0 test --locked
cargo +1.75.0 build --release --locked
cargo test --locked
cargo build --release --locked
```

CI repeats the locked test/build on Rust 1.75.0 and stable. For TUI changes,
also follow [the real-TTY validation guide](docs/tui-validation.md): exercise
the synthetic all-state fixture in a real PTY at wide, ordinary, and narrow
sizes, run the relevant fresh-container probe, verify terminal restoration, and
record what was and was not authenticated. Review actual screenshots for color
and border fidelity; Ratatui text assertions are necessary but not sufficient.

For provider-control changes, add deterministic success, refusal, wrong-target,
stale-state, malformed-response, timeout, and reconnect coverage. Prefer an
injected runner or disposable mock server. A real authenticated lifecycle is a
separate opt-in gate and requires dedicated test identity/state.

## Documentation and review

Use relative Markdown links inside the repository and run the link checker
described in [docs/testing.md](docs/testing.md). Keep observations, inferences,
and implementation claims distinct in exploration notes. Reviewers should be
able to answer:

1. What exact provider/runtime identity is acted on?
2. Where did authority come from, and when is it revalidated?
3. What confirmation or user input is required?
4. What happens on timeout, reconnect, malformed data, or partial failure?
5. Which deterministic, real-PTY, disposable-runtime, or credentialed test
   supports each claim?

Report potential vulnerabilities through [SECURITY.md](SECURITY.md), not in a
general change discussion.
