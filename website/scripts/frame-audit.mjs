#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, normalize } from "node:path";
import { chromium } from "@playwright/test";

const stories = [
  ["setup", "#start"],
  ["claude", "#harness-demo"],
  ["codex", "#harness-demo"],
  ["pi", "#harness-demo"],
  ["opencode", "#harness-demo"],
  ["cursor", "#harness-demo"],
  ["copilot", "#harness-demo"],
  ["antigravity", "#harness-demo"],
  ["mistral-vibe", "#harness-demo"],
  ["muse", "#harness-demo"],
  ["qwen", "#harness-demo"],
  ["kimi", "#harness-demo"],
  ["terminal", "#harness-demo"],
  ["rename", "#controls"],
  ["switch", "#controls"],
  ["model", "#controls"],
  ["login", "#controls"],
];
const viewports = [
  ["desktop", { width: 1440, height: 900 }],
  ["mac-laptop", { width: 1280, height: 800 }],
  ["mobile", { width: 390, height: 844 }],
];
const positions = [0, 0.25, 0.5, 0.75, 1];
const staticRoot = new URL("../dist/static/", import.meta.url);
const mime = {
  ".cast": "application/x-asciicast",
  ".css": "text/css",
  ".html": "text/html",
  ".js": "text/javascript",
  ".json": "application/json",
  ".png": "image/png",
  ".svg": "image/svg+xml",
};

function serveStatic() {
  const rootPath = normalize(staticRoot.pathname);
  const server = createServer(async (request, response) => {
    try {
      const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
      const relative = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
      const path = normalize(join(rootPath, relative));
      assert.ok(path.startsWith(rootPath), "request escaped static root");
      response.setHeader("content-type", mime[extname(path)] ?? "application/octet-stream");
      response.end(await readFile(path));
    } catch {
      response.statusCode = 404;
      response.end("Not found");
    }
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      resolve({ server, url: `http://127.0.0.1:${address.port}` });
    });
  });
}

async function chooseStory(page, id, section) {
  if (id !== "setup") {
    await page.locator(`${section} [data-story-tab="${id}"]`).click();
  }
  const player = page.locator(`${section} [data-demo-player]`);
  await player.waitFor();
  await page.waitForFunction(
    ({ selector, story }) => {
      const root = document.querySelector(selector);
      return root?.dataset.story === `story-${story}`
        && root._realCastPlayer?.player
        && root.querySelectorAll(".ap-wrapper").length === 1;
    },
    { selector: `${section} [data-demo-player]`, story: id },
  );
  await player.scrollIntoViewIfNeeded();
  return player;
}

async function seekAndAudit(player, position) {
  return player.evaluate(async (root, ratio) => {
    const controller = root._realCastPlayer;
    controller.pause();
    const target = controller.manifest.duration * ratio;
    controller.seekTo(target);
    controller.update(target);
    await new Promise((resolve) => setTimeout(resolve, 180));
    const screen = root.querySelector("[data-demo-screen]").getBoundingClientRect();
    const terminal = root.querySelector(".ap-player").getBoundingClientRect();
    const termGrid = root.querySelector(".ap-term").getBoundingClientRect();
    const badge = root.querySelector("[data-demo-last-action]");
    const badgeStyle = getComputedStyle(badge);
    return {
      action: badge.textContent.trim(),
      badgeBorder: badgeStyle.borderTopStyle,
      badgeBackground: badgeStyle.backgroundColor,
      covers: root.querySelectorAll(".story-frame-cover").length,
      wrappers: root.querySelectorAll(".ap-wrapper").length,
      screen: { top: screen.top, bottom: screen.bottom, left: screen.left, right: screen.right },
      terminal: { top: terminal.top, bottom: terminal.bottom, left: terminal.left, right: terminal.right },
      termGrid: { top: termGrid.top, bottom: termGrid.bottom, left: termGrid.left, right: termGrid.right },
    };
  }, position);
}

const output = await mkdtemp(join(tmpdir(), "open-agent-view-frame-audit-"));
const { server, url } = await serveStatic();
const browser = await chromium.launch({ headless: true });
const report = { generatedAt: new Date().toISOString(), output, stories: [] };

try {
  for (const [viewportName, viewport] of viewports) {
    const page = await browser.newPage({ viewport });
    await page.goto(url);
    await page.locator("html[data-stories-ready=true]").waitFor();
    for (const [id, section] of stories) {
      const player = await chooseStory(page, id, section);
      const frames = [];
      for (const position of positions) {
        const audit = await seekAndAudit(player, position);
        assert.equal(audit.wrappers, 1, `${id}/${viewportName}: player wrapper flashed or duplicated`);
        assert.equal(audit.covers, 0, `${id}/${viewportName}: retained frame did not clear`);
        assert.ok(audit.action.length > 0, `${id}/${viewportName}: action badge is empty`);
        assert.equal(audit.badgeBorder, "solid", `${id}/${viewportName}: action keycap has no border`);
        assert.notEqual(audit.badgeBackground, "rgba(0, 0, 0, 0)", `${id}/${viewportName}: action keycap is transparent`);
        assert.ok(audit.terminal.top >= audit.screen.top - 1, `${id}/${viewportName}: terminal is cropped at top`);
        assert.ok(audit.terminal.bottom <= audit.screen.bottom + 1, `${id}/${viewportName}: terminal footer is cropped`);
        assert.ok(audit.termGrid.bottom <= audit.screen.bottom + 1, `${id}/${viewportName}: final terminal row is cropped`);
        const filename = `${viewportName}-${id}-${String(Math.round(position * 100)).padStart(3, "0")}.png`;
        await player.screenshot({ path: join(output, filename) });
        frames.push({ position, filename, ...audit });
      }
      report.stories.push({ id, viewport: viewportName, frames });
      const filenames = frames.map(({ filename }) => join(output, filename));
      execFileSync("montage", [
        ...filenames,
        "-tile", "5x1",
        "-geometry", "+8+8",
        "-background", "#080b0e",
        join(output, `${viewportName}-${id}-contact-sheet.png`),
      ]);
    }
    await page.close();
  }
  await writeFile(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  await writeFile(
    join(output, "README.md"),
    `# Open Agent View frame audit\n\nGenerated ${report.generatedAt}.\n\n` +
      stories.map(([id]) => (
        `## ${id}\n\n` + viewports.map(([name]) => `![${id} ${name}](${name}-${id}-contact-sheet.png)`).join("\n\n")
      )).join("\n\n"),
  );
  process.stdout.write(`${output}\n`);
} finally {
  await browser.close();
  server.close();
}
