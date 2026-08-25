# Open Agent View website

This directory is the single source for
[open-agent-view.github.io](https://open-agent-view.github.io/). It builds with
Vinext, exports to plain static files for GitHub Pages, and keeps all media
local—there are no runtime CDN or private-repository asset dependencies.

## Local development

```console
npm ci
npm run dev
```

## Verification

```console
npm run lint
npm test
npx playwright install chromium
npm run test:visual
npm run export
```

The tests cover rendered product copy and metadata, installer parity, media
dimensions and credential-shaped text, desktop/phone overflow, keyboard copy,
FAQ behavior, reduced motion, and Axe accessibility checks.

The real TUI demo is regenerated from an isolated Docker environment with:

```console
../scripts/capture-site-demo.sh
```

See [`docs/website.md`](../docs/website.md) for media provenance and the exact
manual publication flow.
