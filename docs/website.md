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
tests render 1440×900 desktop, 1280×800 Mac-laptop, and 390×844 phone
viewports, reject horizontal overflow,
exercise every timeline control, provider deep links, keyboard-accessible tabs,
finite next-tab handoff, clipboard copy, and reduced motion, then run Axe.
Rendered-HTML tests also parse all 17 genuine cast-v2 recordings and their
action timelines, reject private paths/account identities/credential-like
material, require canonical Open Graph metadata and the locally vendored marks,
and compare the published installer byte-for-byte with the application
installer.

## Interactive product stories

The page has three finite terminal walkthroughs implemented by
[`website/public/site.js`](../website/public/site.js):

1. the start story installs from the public URL, opens the app, types
   `/harness`, and stops with every option visible;
2. twelve tabbed harness stories begin at the same `/harness` picker, choose
   the tab's harness, browse and select its model (or Terminal shell), run two complete native-TUI
   turns, type the return shortcut explanation, return with Shift+Left, wait on
   the growing shared dashboard, rename the exact new row, and end at the next
   `/harness` picker; and
3. four tabbed common-control stories demonstrate rename, native-session
   switching, model selection, and login/setup.

Every player has independent −5s, pause/resume, +5s, restart, and scrub
controls. No recording advances when the page loads. Scrolling or moving
keyboard focus to a demo, choosing one of its tabs, selecting a provider mark,
or pressing Play starts only that focused recording; the previous recording
pauses when focus moves elsewhere or the browser tab is hidden. A manual pause
remains paused until the visitor explicitly resumes it. Harness tabs can be
opened directly from the provider marks in the hero. Harness and control tabs
advance only after the current story has ended and a separate visible countdown
has elapsed. The long harness stories play at 0.5× so model browsing, native
responses, returning, and renaming remain legible; setup and focused control
stories remain literal 1×. Meaningful response and dashboard states are held
long enough to read. Reduced-motion mode also disables automatic tab changes
and requires an explicit tab, terminal, or Play activation.

The keystrokes are scripted, but every pixel comes from the shell, release
binary, or provider TUI recorded through tmux and Asciinema. The recorder
prepares the harnesses before the Section 2 recording begins, copies only the
minimum existing login state into a private disposable home, and deletes that
home after validating the sanitized cast. Each later tab reuses the same
disposable workspace and provider stores, so the dashboard truthfully gains one
renamed row at a time. There are no fabricated conversations or terminal rows.
Before Asciinema starts, the recorder invokes every real CLI's bounded
version/catalog path in that same disposable environment so first-process
startup is not presented as interaction time. The Terminal story uses the real
`/shell` picker and literal `printf` commands; it does not install fake
chat-like executables.

## Recording and README media

[`scripts/capture-real-site-demo.py`](../scripts/capture-real-site-demo.py)
records an individual real setup, harness, or control story. Each run uses an
owned tmux session, disposable home/state/workspace roots, bounded waits, and
exact owned-process cleanup. The script rejects host identities, email
addresses, temporary paths, and credential-like output.

For the install story, the disposable environment exposes genuine installed
provider executables and installs any missing new-provider CLIs from their
official scripts before recording starts. It copies no provider login state;
this lets the released `/harness` picker show every supported choice without
inventing rows or authenticating an account.

[`scripts/capture-site-demo.sh`](../scripts/capture-site-demo.sh) joins the
reviewed real setup, Claude, and rename casts without generating terminal rows,
then renders the finite README GIF, MP4, and poster. Asset provenance and exact
recapture commands are in [`docs/assets/README.md`](assets/README.md).

The repeatable visual audit builds and exports the current site, then captures
start, 25%, 50%, 75%, and end
frames for every story at desktop, Mac-laptop, and mobile sizes, checks that the native TUI
footer and action keycap stay in bounds, and writes contact sheets to a temporary
review directory:

```console
cd website
npm run audit:frames
```

The command prints the exact `/tmp/open-agent-view-frame-audit-*` directory. It
is review evidence, not a public-site asset, so it is intentionally not
committed.

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
