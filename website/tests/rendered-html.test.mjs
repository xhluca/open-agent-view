import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const escapeCharacter = String.fromCharCode(27);
const bellCharacter = String.fromCharCode(7);
const oscSequence = new RegExp(`${escapeCharacter}\\][\\s\\S]*?(?:${bellCharacter}|${escapeCharacter}\\\\)`, "g");
const csiSequence = new RegExp(`${escapeCharacter}\\[[0-?]*[ -/]*[@-~]`, "g");

const demos = [
  ["setup", null, null],
  ["claude", "Claude Code", "Claude"],
  ["codex", "OpenAI Codex", "OpenAI Codex"],
  ["pi", "Pi", "Pi"],
  ["opencode", "OpenCode", "OpenCode"],
  ["cursor", "Cursor", "Cursor"],
  ["copilot", "GitHub Copilot", "GitHub Copilot"],
  ["antigravity", "Antigravity", "Antigravity"],
  ["terminal", "Terminal", "Terminal"],
  ["rename", null, null],
  ["switch", null, null],
  ["model", null, null],
  ["login", null, null],
];

const privateMaterial = [
  /(?:api[_-]?key|access[_-]?token|oauth[_-]?token|authorization\s*[:=]\s*bearer)/i,
  /(?:gh[pousr]_|sk-(?:proj-)?|AKIA)[A-Za-z0-9_-]{8,}/,
  /(?:^|[\s"'])\/(?:home|Users|tmp|private\/var)\//m,
  /(?:^|[\s"'])[A-Z]:\\Users\\/im,
  /(?:xlu41|@mcgill\.|@mila\.)/i,
];

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

function parseCast(source, name) {
  const lines = source.trim().split("\n");
  assert.ok(lines.length > 20, `${name}.cast should contain a real terminal timeline`);
  const header = JSON.parse(lines[0]);
  const events = lines.slice(1).map((line, index) => {
    const event = JSON.parse(line);
    assert.ok(Array.isArray(event), `${name}.cast event ${index + 1} should be an array`);
    assert.equal(event.length, 3, `${name}.cast event ${index + 1} should be cast v2-shaped`);
    return event;
  });
  return { header, events };
}

test("server-renders real recording controls, provider tabs, and canonical metadata", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);

  const html = await response.text();
  assert.match(html, /<title>Open Agent View/);
  assert.match(html, /Monitor every agent/);
  assert.match(html, /Step in when it matters/);
  assert.match(html, /data-story="story-setup"/);
  assert.match(html, /aria-label="INSTALL · OPEN · \/HARNESS playback controls"/);
  assert.match(html, /aria-label="Seek through INSTALL · OPEN · \/HARNESS"/);
  assert.match(html, /role="tablist" aria-label="Harness demos"/);
  assert.match(html, /data-demo-action="back"/);
  assert.match(html, /data-demo-action="pause"/);
  assert.match(html, /data-demo-action="forward"/);
  assert.match(html, /data-demo-action="restart"/);
  assert.match(html, /data-copy-command="open-agent-view"/);
  assert.match(html, /https:\/\/open-agent-view\.github\.io\/install\.sh/);
  assert.match(html, /rel="canonical" href="https:\/\/open-agent-view\.github\.io"/);
  assert.match(html, /property="og:image" content="https:\/\/open-agent-view\.github\.io\/og\.png"/);
  assert.match(html, /name="twitter:card" content="summary_large_image"/);

  for (const [id, label] of demos.slice(1, 9)) {
    assert.match(html, new RegExp(`data-story-tab="${id}"`));
    assert.match(html, new RegExp(`data-story="story-${id}"`));
    assert.match(html, new RegExp(`data-select-harness="${id}"`));
    assert.match(html, new RegExp(`Watch the ${label} demo`));
  }

  assert.doesNotMatch(html, /codex-preview|Your site is taking shape|react-loading-skeleton/);
  assert.doesNotMatch(html, /raw\.githubusercontent\.com/);
  assert.doesNotMatch(html, /<video\b|data-demo-status|data-terminal-(?:row|grid|frame)/);
});

test("publishes genuine cast v2 recordings and action timelines for setup and every harness", async () => {
  for (const [name, , manifestName] of demos) {
    const [cast, actionsSource] = await Promise.all([
      readFile(new URL(`public/demos/${name}.cast`, root), "utf8"),
      readFile(new URL(`public/demos/${name}.actions.json`, root), "utf8"),
    ]);
    const { header, events } = parseCast(cast, name);
    const manifest = JSON.parse(actionsSource);
    const output = events.filter((event) => event[1] === "o").map((event) => event[2]).join("");
    const visibleOutput = output
      .replace(oscSequence, "")
      .replace(csiSequence, "")
      .replaceAll("\r", "");
    const finalTime = events.at(-1)[0];

    assert.equal(header.version, 2, `${name}.cast should use asciinema cast v2`);
    assert.ok(Number.isInteger(header.width) && header.width >= 80, `${name}.cast should record a useful width`);
    assert.ok(Number.isInteger(header.height) && header.height >= 24, `${name}.cast should record a useful height`);
    assert.ok(events.every((event) => Number.isFinite(event[0]) && ["o", "r"].includes(event[1]) && typeof event[2] === "string"));
    assert.ok(events.every((event, index) => index === 0 || event[0] >= events[index - 1][0]), `${name}.cast timestamps should be ordered`);
    assert.ok(output.length > 1_000, `${name}.cast should contain substantial real terminal output`);
    assert.ok(output.includes(`${escapeCharacter}[`), `${name}.cast should preserve terminal control sequences`);
    assert.match(output, /Open Agent View v\d+\.\d+\.\d+/, `${name}.cast should show the real application`);

    assert.ok(Number.isFinite(manifest.duration) && manifest.duration > 1);
    assert.ok(Math.abs(manifest.duration - finalTime) < 0.01, `${name} action duration should match its cast`);
    assert.ok(Array.isArray(manifest.actions) && manifest.actions.length > 1);
    assert.ok(manifest.actions.every((action, index) => (
      Number.isFinite(action.at)
      && action.at >= 0
      && action.at <= manifest.duration
      && (index === 0 || action.at >= manifest.actions[index - 1].at)
      && typeof action.action === "string"
      && action.action.length > 0
      && typeof action.window === "string"
      && action.window.length > 0
    )), `${name} actions should be ordered, bounded, and labelled`);

    if (manifestName) {
      const actionTimes = [0, ...manifest.actions.map((action) => action.at), manifest.duration];
      const longestGap = Math.max(...actionTimes.slice(1).map((at, index) => at - actionTimes[index]));
      assert.ok(longestGap <= 3.001, `${name} should shorten provider waits without dropping frames`);
    }

    if (name === "setup") {
      assert.match(visibleOutput, /curl -fsSL https:\/\/open-agent-view\.github\.io\/install\.sh \| bash/);
      assert.match(visibleOutput, /\$ opav\b/);
    } else if (manifestName) {
      assert.ok(
        manifest.actions.some((action) => `${action.action} ${action.window}`.includes(manifestName)),
        `${name} actions should identify ${manifestName}`,
      );
    } else {
      assert.ok(
        manifest.actions.some((action) => action.window === "open-agent-view"),
        `${name} controls should visibly use the real Open Agent View TUI`,
      );
    }

    for (const pattern of privateMaterial) {
      assert.doesNotMatch(`${cast}\n${actionsSource}`, pattern, `${name} must not publish secrets or private machine paths`);
    }
  }
});

test("uses the local asciinema player without a synthetic terminal generator or playback loop", async () => {
  const [page, player, styles, script, playerBundle] = await Promise.all([
    readFile(new URL("app/page.tsx", root), "utf8"),
    readFile(new URL("app/DemoPlayer.tsx", root), "utf8"),
    readFile(new URL("app/globals.css", root), "utf8"),
    readFile(new URL("public/site.js", root), "utf8"),
    stat(new URL("public/asciinema-player.min.js", root)),
  ]);

  assert.ok(playerBundle.size > 50_000, "the local asciinema player bundle should be published");
  assert.match(script, /class RealCastPlayer/);
  assert.match(script, /AsciinemaPlayer\.create\(story\.cast/);
  assert.match(player, /dispatchEvent\(new Event\("oav:react-hydrated"\)\)/);
  assert.match(script, /window\.addEventListener\("oav:react-hydrated", mountStories/);
  assert.match(script, /if \(!window\.__oavReactHydrated\)/);
  assert.match(script, /if \(document\.documentElement\.dataset\.storiesReady === "true"\) return/);
  assert.match(script, /loop:\s*false/);
  assert.match(script, /this\.ended = true/);
  assert.match(script, /this\.pauseButton\.textContent = "Replay"/);
  assert.doesNotMatch(script, /loop:\s*true|class StoryPlayer|syntheticFrames|terminalRows|renderTerminalFrame/);
  assert.doesNotMatch(page, /data-terminal-(?:row|grid|frame)|<video\b/);
  assert.doesNotMatch(player, /data-terminal-(?:row|grid|frame)|<video\b/);

  const tabUnderline = styles.match(/\.story-tabs button i\s*\{([^}]+)\}/s)?.[1] ?? "";
  assert.match(tabUnderline, /background:\s*var\(--cyan\)/);
  assert.match(styles, /\.story-tabs button\[aria-selected="true"\] i/);
  assert.match(page, /thin\s+cyan\s+line/i);
  assert.match(page, /Actual provider TUI output · waits shortened · no HTML terminal simulation/);
  assert.doesNotMatch(`${page}\n${styles}\n${script}`, /data-tab-hold-progress|yellow hold bar|\.tab-hold/);
});

test("keeps the public installer byte-identical to the application installer", async () => {
  const [source, published] = await Promise.all([
    readFile(new URL("../../install.sh", import.meta.url)),
    readFile(new URL("public/install.sh", root)),
  ]);
  assert.deepEqual(published, source);
});
