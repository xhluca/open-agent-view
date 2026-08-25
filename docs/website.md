# Website and demo

The public project site is
[open-agent-view.github.io](https://open-agent-view.github.io/). Its canonical
source is [`website/`](../website/); the GitHub Pages repository contains only
the generated static artifact.

## Local preview and validation

```console
cd website
npm ci
npm audit --omit=dev --audit-level=high
npm run lint
npm test
npx playwright install chromium
npm run test:visual
npm run export
```

`npm run export` writes the deployable site to `website/dist/static`. Browser
tests render both 1440×900 and 390×844 viewports, reject horizontal overflow,
exercise every timeline control, provider deep links, keyboard-accessible tabs,
the separate eight-second tab handoff, clipboard copy, and reduced motion, then
run Axe. Rendered-HTML tests also require canonical Open Graph metadata, all
eight local provider marks, the finite/non-looping story engine, and a
byte-identical public installer.

## Interactive product stories

The page has three finite, deterministic terminal walkthroughs implemented by
[`website/public/site.js`](../website/public/site.js):

1. the hero opens `opav`, creates sessions for all eight launch targets in
   `/work/acme-dashboard`, renames one, and stops on the complete dashboard;
2. the start story installs from the public URL, opens the app, types
   `/harness`, and stops with every option visible;
3. tabbed harness and common-control stories demonstrate a bounded
   provider-specific conversation, rename, native-session switching, model
   selection, and login/setup.

Every player has independent −5s, pause/resume, +5s, restart, and scrub
controls. Harness tabs can be opened directly from the provider marks in the
hero. Control tabs advance only after the current story has ended and a separate
eight-second countdown has elapsed. Reduced-motion mode disables autoplay and
automatic tab changes.

These walkthroughs contain scripted, credential-free copy so they are fast,
repeatable, and safe to publish. The separate Docker recording below remains
the executable proof that the shipped TUI renders and handles a real PTY.

## Real Docker demo

[`scripts/capture-site-demo.sh`](../scripts/capture-site-demo.sh) builds the
actual release binary and records it through a real PTY in a disposable Docker
container. The container is pinned by immutable image ID, unprivileged,
read-only, network-disabled, capability-dropped, PID-limited, and given only a
tmpfs home plus the committed all-provider fixture. Fixture mode prevents every
provider action.

The recording navigates the real queue, opens contextual help, switches between
status and directory grouping, then exits through the application's terminal
restoration path. It produces a local Asciinema cast, GIF, MP4, and poster; tests
reject credential-shaped text and oversized media. It is deliberately not used
as a hidden looping replacement for the interactive stories.

## Manual publication

Deployment is intentionally manual. After the complete repository and website
gates pass:

```console
scripts/publish-site.sh
```

The script installs with lockfile fidelity, requires a clean production
dependency audit, exports the site, validates that the destination is exactly the
`open-agent-view/open-agent-view.github.io` repository, replaces only that
checkout's generated files, commits the artifact when it changed, and pushes
`main`. It never deploys from a source-branch push or GitHub Action.

The production smoke check is:

```console
curl -fsS https://open-agent-view.github.io/ >/dev/null
curl -fsS https://open-agent-view.github.io/install.sh |
  cmp - install.sh
```
