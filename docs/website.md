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
2. twelve tabbed harness stories launch the native CLI, send a bounded
   conversation when the disposable account is ready—or show that provider's
   genuine setup/login TUI when it is not—then return to OAV; and
3. four tabbed common-control stories demonstrate rename, native-session
   switching, model selection, and login/setup.

Every player has independent −5s, pause/resume, +5s, restart, and scrub
controls. Harness tabs can be opened directly from the provider marks in the
hero. Harness and control tabs advance only after the current story has ended
and a separate visible countdown has elapsed. Playback is literal 1×. Only
genuine idle/provider waits are shortened before publication; meaningful
response, setup, and reopened-session states are held long enough to read.
Reduced-motion mode disables autoplay and automatic tab changes.

The keystrokes are scripted, but every pixel comes from the shell, release
binary, or provider TUI recorded through tmux and Asciinema. Authenticated clips
use the minimum existing login state copied into disposable private homes; the
capture deletes those homes and validates the sanitized cast before publishing.
Providers without disposable authentication show their actual setup/login UI;
there are no fabricated conversations or terminal rows. Provider spinners can
repaint continuously while a model or CLI starts, so harness stories cap
uninteresting gaps between labelled actions at three seconds.
[`scripts/compact_real_recordings.py`](../scripts/compact_real_recordings.py)
applies one time map to the genuine cast and its action labels; it never changes,
reorders, or invents terminal output.

## Recording and README media

[`scripts/capture-real-site-demo.py`](../scripts/capture-real-site-demo.py)
records an individual real setup, harness, or control story. Each run uses an
owned tmux session, disposable home/state/workspace roots, bounded waits, and
exact owned-process cleanup. The script rejects host identities, email
addresses, temporary paths, and credential-like output.

Release follow-up: immediately after the release containing Mistral Vibe, Muse
Code, Qwen Code, and Kimi Code is public, recapture `setup.cast` through the
public installer and make the rendered-HTML gate require all twelve picker
choices. This is intentionally the only recording deferred past the release;
the public v0.1.36 installer cannot display providers it does not contain.

[`scripts/capture-site-demo.sh`](../scripts/capture-site-demo.sh) joins the
reviewed real setup, Claude, and rename casts without generating terminal rows,
then renders the finite README GIF, MP4, and poster. Asset provenance and exact
recapture commands are in [`docs/assets/README.md`](assets/README.md).

The repeatable visual audit builds and exports the current site, then captures
start, 25%, 50%, 75%, and end
frames for every story at desktop and mobile sizes, checks that the native TUI
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
