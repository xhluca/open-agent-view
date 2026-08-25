import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = new URL("../", import.meta.url);

async function render() {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}`);
  const { default: worker } = await import(workerUrl.href);

  return worker.fetch(
    new Request("https://open-agent-view.github.io/", {
      headers: { accept: "text/html" },
    }),
    { ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) } },
    { waitUntil() {}, passThroughOnException() {} },
  );
}

test("server-renders the complete product story and canonical metadata", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);

  const html = await response.text();
  assert.match(html, /<title>Open Agent View/);
  assert.match(html, /Monitor every agent/);
  assert.match(html, /Step in when it matters/);
  assert.match(html, /Install once/);
  assert.match(html, /Pick a harness/);
  assert.match(html, /Small commands/);
  assert.match(html, /One index/);
  assert.match(html, /data-story="story-overview"/);
  assert.match(html, /data-story-tab="antigravity"/);
  assert.match(html, /data-story-tab="login"/);
  assert.match(html, /data-demo-action="back"/);
  assert.match(html, /data-demo-action="pause"/);
  assert.match(html, /data-demo-action="forward"/);
  assert.match(html, /data-demo-action="restart"/);
  assert.match(html, /data-demo-window/);
  assert.match(html, /data-demo-last-action/);
  assert.match(html, /https:\/\/open-agent-view\.github\.io\/install\.sh/);
  assert.match(html, /rel="canonical" href="https:\/\/open-agent-view\.github\.io"/);
  assert.match(html, /property="og:image" content="https:\/\/open-agent-view\.github\.io\/og\.png"/);
  assert.match(html, /name="twitter:card" content="summary_large_image"/);
  assert.doesNotMatch(html, /codex-preview|Your site is taking shape|react-loading-skeleton/);
  assert.doesNotMatch(html, /raw\.githubusercontent\.com/);
  assert.doesNotMatch(html, /One loop|Run more agents|Never pretend controls|Built to stay honest/);
  assert.doesNotMatch(html, /data-demo-status/);
  assert.doesNotMatch(html, /<video\b/);

  for (const provider of [
    "Claude Code",
    "OpenAI Codex",
    "Pi",
    "OpenCode",
    "Cursor",
    "GitHub Copilot",
    "Antigravity",
    "Terminal",
  ]) {
    assert.match(html, new RegExp(provider));
  }
});

test("retains the reproducible Docker demo and publishes every local provider mark", async () => {
  const [cast, video, poster, product, og] = await Promise.all([
    readFile(new URL("public/oav-demo.cast", root), "utf8"),
    readFile(new URL("public/oav-demo.mp4", root)),
    sharp(fileURLToPath(new URL("public/oav-demo.png", root))).metadata(),
    sharp(fileURLToPath(new URL("public/open-agent-view.png", root))).metadata(),
    sharp(fileURLToPath(new URL("public/og.png", root))).metadata(),
  ]);

  const header = JSON.parse(cast.split("\n", 1)[0]);
  const escape = String.fromCharCode(27);
  const visibleCast = cast
    .trim()
    .split("\n")
    .slice(1)
    .map((line) => JSON.parse(line)[2])
    .join("")
    .replace(new RegExp(`${escape}\\[[0-?]*[ -/]*[@-~]`, "g"), "");
  assert.equal(header.version, 2);
  assert.equal(header.width, 150);
  assert.equal(header.height, 42);
  assert.match(visibleCast, /Open Agent View v0\.1\.33/);
  assert.match(visibleCast, /GitHub Copilot/);
  assert.match(visibleCast, /Antigravity/);
  assert.doesNotMatch(cast, /(?:api[_-]?key|oauth[_-]?token|authorization: bearer|ghp_)/i);

  assert.equal(video.subarray(4, 8).toString("ascii"), "ftyp");
  assert.ok(video.length > 20_000);
  assert.deepEqual([poster.width, poster.height], [1190, 784]);
  assert.deepEqual([product.width, product.height], [1190, 784]);
  assert.deepEqual([og.width, og.height], [1200, 630]);

  for (const icon of [
    "claude.svg",
    "codex.png",
    "pi.svg",
    "opencode.svg",
    "cursor.svg",
    "copilot.svg",
    "antigravity.svg",
    "terminal.svg",
  ]) {
    const iconFile = await stat(new URL(`public/providers/${icon}`, root));
    assert.ok(iconFile.size > 100, `${icon} should be a non-empty local asset`);
  }
});

test("keeps the public installer byte-identical to the application installer", async () => {
  const [source, published] = await Promise.all([
    readFile(new URL("../../install.sh", import.meta.url)),
    readFile(new URL("public/install.sh", root)),
  ]);
  assert.deepEqual(published, source);
});

test("keeps accessible playback, tab cycling, and motion safeguards in source", async () => {
  const [page, player, styles, copy, layout, script, video] = await Promise.all([
    readFile(new URL("app/page.tsx", root), "utf8"),
    readFile(new URL("app/DemoPlayer.tsx", root), "utf8"),
    readFile(new URL("app/globals.css", root), "utf8"),
    readFile(new URL("app/CopyCommand.tsx", root), "utf8"),
    readFile(new URL("app/layout.tsx", root), "utf8"),
    readFile(new URL("public/site.js", root), "utf8"),
    stat(new URL("public/oav-demo.mp4", root)),
  ]);
  assert.match(page, /aria-label="Choose a harness demo"/);
  assert.match(page, /role="tablist"/);
  assert.match(page, /data-select-harness/);
  assert.match(player, /aria-label={`Seek through \$\{label\}`}/);
  assert.match(player, /data-demo-action="restart"/);
  assert.match(script, /class StoryPlayer/);
  assert.match(script, /IntersectionObserver/);
  assert.match(script, /\/ 8000/);
  assert.match(script, /prefers-reduced-motion: reduce/);
  assert.match(script, /this\.ended = true/);
  assert.doesNotMatch(script, /loop\s*:/);
  assert.match(styles, /prefers-reduced-motion:\s*reduce/);
  assert.match(styles, /:focus-visible/);
  assert.match(copy, /aria-live="polite"/);
  assert.match(layout, /summary_large_image/);
  assert.ok(video.size < 8 * 1024 * 1024, "demo video should stay lightweight");
});
