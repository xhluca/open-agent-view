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
  assert.match(html, /See every agent/);
  assert.match(html, /Step in when it matters/);
  assert.match(html, /Discover/);
  assert.match(html, /Follow/);
  assert.match(html, /Intervene/);
  assert.match(html, /Return/);
  assert.match(html, /https:\/\/open-agent-view\.github\.io\/install\.sh/);
  assert.match(html, /rel="canonical" href="https:\/\/open-agent-view\.github\.io"/);
  assert.match(html, /property="og:image" content="https:\/\/open-agent-view\.github\.io\/og\.png"/);
  assert.match(html, /name="twitter:card" content="summary_large_image"/);
  assert.doesNotMatch(html, /codex-preview|Your site is taking shape|react-loading-skeleton/);
  assert.doesNotMatch(html, /raw\.githubusercontent\.com/);

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

test("publishes real local demo media with no credential-shaped text", async () => {
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
  assert.match(visibleCast, /Open Agent View v0\.1\.32/);
  assert.match(visibleCast, /GitHub Copilot/);
  assert.match(visibleCast, /Antigravity/);
  assert.doesNotMatch(cast, /(?:api[_-]?key|oauth[_-]?token|authorization: bearer|ghp_)/i);

  assert.equal(video.subarray(4, 8).toString("ascii"), "ftyp");
  assert.ok(video.length > 20_000);
  assert.deepEqual([poster.width, poster.height], [1190, 784]);
  assert.deepEqual([product.width, product.height], [1190, 784]);
  assert.deepEqual([og.width, og.height], [1200, 630]);
});

test("keeps the public installer byte-identical to the application installer", async () => {
  const [source, published] = await Promise.all([
    readFile(new URL("../../install.sh", import.meta.url)),
    readFile(new URL("public/install.sh", root)),
  ]);
  assert.deepEqual(published, source);
});

test("keeps accessibility and motion safeguards in source", async () => {
  const [page, styles, copy, layout, video] = await Promise.all([
    readFile(new URL("app/page.tsx", root), "utf8"),
    readFile(new URL("app/globals.css", root), "utf8"),
    readFile(new URL("app/CopyCommand.tsx", root), "utf8"),
    readFile(new URL("app/layout.tsx", root), "utf8"),
    stat(new URL("public/oav-demo.mp4", root)),
  ]);
  assert.match(page, /aria-label="Supported harness capabilities"/);
  assert.match(page, /video controls/);
  assert.match(styles, /prefers-reduced-motion:\s*reduce/);
  assert.match(styles, /:focus-visible/);
  assert.match(copy, /aria-live="polite"/);
  assert.match(layout, /summary_large_image/);
  assert.ok(video.size < 8 * 1024 * 1024, "demo video should stay lightweight");
});
