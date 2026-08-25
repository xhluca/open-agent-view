# Website and demo

The public project site is
[open-agent-view.github.io](https://open-agent-view.github.io/). Its canonical
source is [`website/`](../website/); the GitHub Pages repository contains only
the generated static artifact.

## Local preview and validation

```console
cd website
npm ci
npm run lint
npm test
npx playwright install chromium
npm run test:visual
npm run export
```

`npm run export` writes the deployable site to `website/dist/static`. Browser
tests render both 1440×900 and 390×844 viewports, reject horizontal overflow,
exercise keyboard copy and FAQ controls, run Axe, and check reduced-motion
behavior. Rendered-HTML tests also require canonical Open Graph metadata, local
media, all eight harness targets, and a byte-identical public installer.

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
reject credential-shaped text and oversized media.

## Manual publication

Deployment is intentionally manual. After the complete repository and website
gates pass:

```console
scripts/publish-site.sh
```

The script exports the site, validates that the destination is exactly the
`open-agent-view/open-agent-view.github.io` repository, replaces only that
checkout's generated files, commits the artifact when it changed, and pushes
`main`. It never deploys from a source-branch push or GitHub Action.

The production smoke check is:

```console
curl -fsS https://open-agent-view.github.io/ >/dev/null
curl -fsS https://open-agent-view.github.io/install.sh |
  cmp - install.sh
```
