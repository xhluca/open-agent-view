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
run Axe. Rendered-HTML tests also parse all 13 genuine cast-v2 recordings and
their action timelines, reject private paths/account identities/credential-like
material, require canonical Open Graph metadata and all eight local provider
marks, and compare the published installer byte-for-byte with the application
installer.

## Interactive product stories

The page has three finite terminal walkthroughs implemented by
[`website/public/site.js`](../website/public/site.js):

1. the start story installs from the public URL, opens the app, types
   `/harness`, and stops with every option visible;
2. eight tabbed harness stories launch the real native CLI, send a bounded
   conversation, return to OAV, and reopen the same session; and
3. four tabbed common-control stories demonstrate rename, native-session
   switching, model selection, and login/setup.

Every player has independent −5s, pause/resume, +5s, restart, and scrub
controls. Harness tabs can be opened directly from the provider marks in the
hero. Control tabs advance only after the current story has ended and a separate
eight-second countdown has elapsed. Reduced-motion mode disables autoplay and
automatic tab changes.

The keystrokes are scripted, but every pixel comes from the real shell, release
binary, or provider TUI recorded through tmux and Asciinema. Authenticated clips
use the minimum existing login state copied into disposable private homes; the
capture deletes those homes and validates the sanitized cast before publishing.
No HTML terminal simulation is used.

## Recording and README media

[`scripts/capture-real-site-demo.py`](../scripts/capture-real-site-demo.py)
records an individual real setup, harness, or control story. Each run uses an
owned tmux session, disposable home/state/workspace roots, bounded waits, and
exact owned-process cleanup. The script rejects host identities, email
addresses, temporary paths, and credential-like output.

[`scripts/capture-site-demo.sh`](../scripts/capture-site-demo.sh) joins the
reviewed real setup, Claude, and rename casts without generating terminal rows,
then renders the finite README GIF, MP4, and poster. Asset provenance and exact
recapture commands are in [`docs/assets/README.md`](assets/README.md).

Fresh hardened Docker PTYs remain a separate auth-free verification layer for
the shipped OAV binary: they prove terminal layout, input, restoration, and the
fixture I/O fence. They are not presented as provider-authenticated website
recordings.

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
