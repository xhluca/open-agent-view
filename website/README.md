# Open Agent View website

This directory is the single source for
[open-agent-view.github.io](https://open-agent-view.github.io/). It builds with
Vinext, exports to plain static files for GitHub Pages, and keeps all media
local—there are no runtime CDN or private-repository asset dependencies.

## Local development

```console
npm ci
npm audit --omit=dev --audit-level=high
npm run dev
```

## Verification

```console
npm audit --omit=dev --audit-level=high
npm run lint
npm test
npx playwright install chromium
npm run test:visual
npm run export
```

The tests cover rendered product copy and metadata, installer parity, media
dimensions and credential-shaped text, desktop/phone overflow, all playback
controls, finite final frames, provider deep links, keyboard tabs, the delayed
tab handoff, clipboard copy, reduced motion, and Axe accessibility checks.

The published stories are sanitized, finite recordings of the real binary and
native provider TUIs. They use one `/work/acme-dashboard` workspace and share
one small player runtime in `public/site.js`; no website component fabricates
terminal rows.

The canonical overview GIF is regenerated from its reviewed real cast with:

```console
../scripts/capture-site-demo.sh
```

See [`docs/website.md`](../docs/website.md) for media provenance and the exact
manual publication flow.
