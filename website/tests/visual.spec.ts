import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

type BrowserPlayer = {
  finish(): void;
  manifest: { duration: number };
  pause(): void;
  seekTo(seconds: number): void;
  update(seconds: number): void;
};

type BrowserTabbedStory = { holdDelay: number };
type PlayerNode = HTMLElement & { _realCastPlayer: BrowserPlayer };
type TabbedStoryNode = HTMLElement & { _tabbedStory: BrowserTabbedStory };

const harnesses = [
  ["claude", "Claude Code"],
  ["codex", "OpenAI Codex"],
  ["pi", "Pi"],
  ["opencode", "OpenCode"],
  ["cursor", "Cursor"],
  ["copilot", "GitHub Copilot"],
  ["antigravity", "Antigravity"],
  ["mistral-vibe", "Mistral Vibe"],
  ["muse", "Muse Code"],
  ["qwen", "Qwen Code"],
  ["kimi", "Kimi Code"],
  ["terminal", "Terminal"],
] as const;

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "phone", width: 390, height: 844 },
] as const;

async function openReady(page: import("@playwright/test").Page) {
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-stories-ready", "true");
  // The player must survive the framework's hydration pass, not merely mount
  // early enough to race this assertion.
  await page.waitForTimeout(750);
}

for (const viewport of viewports) {
  test(`${viewport.name} renders real players without horizontal overflow`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    await openReady(page);
    await expect(page.getByRole("heading", { name: /Monitor every agent/ })).toBeVisible();
    await expect(page.locator(".provider-row > a")).toHaveCount(12);
    await expect(page.locator(".provider-row img")).toHaveCount(10);
    await expect(page.locator("video")).toHaveCount(0);
    await expect(page.locator("#start .ap-wrapper")).toHaveCount(1);
    await expect(page.locator("#harness-demo .ap-wrapper")).toHaveCount(1);

    const actions = page.locator(".hero-actions [data-copy-command]");
    await expect(actions).toHaveCount(2);
    await expect(actions.nth(1)).toHaveAttribute("data-copy-command", "open-agent-view");

    const overflow = await page.evaluate(() => ({
      viewport: document.documentElement.clientWidth,
      content: document.documentElement.scrollWidth,
      offenders: [...document.querySelectorAll("body *")]
        .filter((element) => element.getBoundingClientRect().right > document.documentElement.clientWidth + 1)
        .slice(0, 8)
        .map((element) => `${element.tagName.toLowerCase()}.${element.className}`),
    }));
    expect(overflow.content, `overflowing elements: ${overflow.offenders.join(", ")}`).toBeLessThanOrEqual(overflow.viewport);

    await page.screenshot({ path: testInfo.outputPath(`${viewport.name}.png`), fullPage: true });
  });
}

test("real player controls are accessible and the final frame does not loop", async ({ page }) => {
  await openReady(page);

  const setup = page.locator("#start [data-demo-player]");
  await setup.scrollIntoViewIfNeeded();
  await expect(setup.getByRole("button", { name: "Go back five seconds" })).toBeVisible();
  const pause = setup.getByRole("button", { name: "Pause demo" });
  await expect(pause).toBeVisible();
  await expect(setup.getByRole("button", { name: "Go forward five seconds" })).toBeVisible();
  await expect(setup.getByRole("button", { name: "Restart demo" })).toBeVisible();

  const progress = setup.getByRole("slider", { name: "Seek through INSTALL · OPEN · /HARNESS" });
  await expect(pause).toHaveText("Replay", { timeout: 15_000 });
  const finalProgress = Number(await progress.inputValue());
  const finalFrame = await setup.locator(".ap-wrapper").textContent();
  expect(finalProgress).toBeGreaterThan(900);
  await page.waitForTimeout(800);
  await expect(pause).toHaveText("Replay");
  await expect(progress).toHaveValue(String(finalProgress));
  await expect(setup.locator(".ap-wrapper")).toHaveText(finalFrame ?? "");
  await expect(setup.locator(".ap-wrapper")).toHaveCount(1);
});

test("every provider logo jumps to its real recording and tabs support arrow navigation", async ({ page }) => {
  await openReady(page);
  const section = page.locator("#harness-demo");

  for (const [id, label] of harnesses) {
    await page.getByRole("link", { name: `Watch the ${label} demo` }).click();
    const tab = section.getByRole("tab", { name: label, exact: true });
    await expect(tab).toHaveAttribute("aria-selected", "true");
    await expect(section.locator("[data-demo-player]")).toHaveAttribute("data-story", `story-${id}`);
    await expect(section.locator(".recording-unavailable")).toHaveCount(0);
    await expect(section.locator(".ap-wrapper")).toHaveCount(1);
  }

  const antigravity = section.getByRole("tab", { name: "Antigravity" });
  await antigravity.click();
  await antigravity.press("ArrowRight");
  await expect(section.getByRole("tab", { name: "Mistral Vibe" })).toHaveAttribute("aria-selected", "true");
  await section.getByRole("tab", { name: "Mistral Vibe" }).press("End");
  await expect(section.getByRole("tab", { name: "Terminal" })).toHaveAttribute("aria-selected", "true");
  await section.getByRole("tab", { name: "Terminal" }).press("Home");
  await expect(section.getByRole("tab", { name: "Claude Code" })).toHaveAttribute("aria-selected", "true");
});

test("the second copy control copies exactly the full executable name", async ({ browser }) => {
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
  const page = await context.newPage();
  await openReady(page);

  const command = page.locator(".hero-actions [data-copy-command]").nth(1);
  await expect(command).toHaveAttribute("data-copy-command", "open-agent-view");
  await expect(command).toHaveAttribute("data-copy-ready", "true");
  await command.focus();
  await expect(command).toBeFocused();
  await command.press("Enter");
  await expect(command).toContainText("Copied");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("open-agent-view");
  await context.close();
});

test("selected tabs use a cyan underline as the eight-second countdown", async ({ page }) => {
  await openReady(page);

  const controls = page.locator("#controls");
  await controls.scrollIntoViewIfNeeded();
  const selected = controls.getByRole("tab", { name: "Rename" });
  await expect(selected).toHaveAttribute("aria-selected", "true");
  await expect(controls.locator("[data-tab-hold-progress]")).toHaveCount(0);

  const underline = selected.locator("i");
  const before = await underline.evaluate((node) => {
    const root = getComputedStyle(document.documentElement);
    const style = getComputedStyle(node);
    return {
      width: node.getBoundingClientRect().width,
      color: style.backgroundColor,
      cyan: root.getPropertyValue("--cyan").trim(),
    };
  });
  const expectedCyan = await page.evaluate((cyan) => {
    const probe = document.createElement("span");
    probe.style.color = cyan;
    document.body.append(probe);
    const resolved = getComputedStyle(probe).color;
    probe.remove();
    return resolved;
  }, before.cyan);
  expect(before.color).toBe(expectedCyan);
  expect(before.width).toBeGreaterThan(0);

  await controls.getByRole("button", { name: "Pause demo" }).click();
  await controls.locator("[data-demo-player]").evaluate((node) => {
    node.dispatchEvent(new CustomEvent("demo-ended", { bubbles: true }));
  });
  await page.waitForTimeout(350);
  const during = await underline.evaluate((node) => ({
    width: node.getBoundingClientRect().width,
    color: getComputedStyle(node).backgroundColor,
  }));
  expect(during.color).toBe(expectedCyan);
  expect(during.width).toBeGreaterThan(0);
  expect(during.width).toBeLessThan(before.width);
  await expect(controls.getByRole("tab", { name: "Switch sessions" })).toHaveAttribute("aria-selected", "true", { timeout: 9_000 });
});

test("harness stories advance after the final frame and stop after the last tab", async ({ page }) => {
  await openReady(page);
  const section = page.locator("#harness-demo");
  const claude = section.getByRole("tab", { name: "Claude Code" });
  await claude.click();
  await section.locator("[data-tabbed-story]").evaluate((node) => {
    (node as TabbedStoryNode)._tabbedStory.holdDelay = 220;
    (node.querySelector("[data-demo-player]") as PlayerNode)._realCastPlayer.finish();
  });
  await expect(section.getByRole("tab", { name: "OpenAI Codex" })).toHaveAttribute("aria-selected", "true", { timeout: 1_500 });

  const terminal = section.getByRole("tab", { name: "Terminal" });
  await terminal.click();
  await section.locator("[data-tabbed-story]").evaluate((node) => {
    (node as TabbedStoryNode)._tabbedStory.holdDelay = 100;
    (node.querySelector("[data-demo-player]") as PlayerNode)._realCastPlayer.finish();
  });
  await page.waitForTimeout(250);
  await expect(terminal).toHaveAttribute("aria-selected", "true");
});

test("every story keeps the complete terminal and a clear action keycap", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openReady(page);
  const stories = [
    ["setup", "#start"],
    ...harnesses.map(([id]) => [id, "#harness-demo"]),
    ["rename", "#controls"],
    ["switch", "#controls"],
    ["model", "#controls"],
    ["login", "#controls"],
  ] as const;

  for (const [id, sectionSelector] of stories) {
    const section = page.locator(sectionSelector);
    if (id !== "setup") await section.locator(`[data-story-tab="${id}"]`).click();
    const player = section.locator("[data-demo-player]");
    await expect(player).toHaveAttribute("data-story", `story-${id}`);
    await expect(player.locator(".ap-wrapper")).toHaveCount(1);
    await player.evaluate((node) => {
      const controller = (node as PlayerNode)._realCastPlayer;
      controller.pause();
      const target = controller.manifest.duration * 0.72;
      controller.seekTo(target);
      controller.update(target);
    });
    await page.waitForTimeout(180);
    const geometry = await player.evaluate((node) => {
      const screen = node.querySelector("[data-demo-screen]").getBoundingClientRect();
      const terminal = node.querySelector(".ap-player").getBoundingClientRect();
      const grid = node.querySelector(".ap-term").getBoundingClientRect();
      const badge = node.querySelector("[data-demo-last-action]");
      const style = getComputedStyle(badge);
      return {
        badge: badge.textContent.trim(),
        badgeBackground: style.backgroundColor,
        badgeBorder: style.borderTopStyle,
        covers: node.querySelectorAll(".story-frame-cover").length,
        screen: { top: screen.top, bottom: screen.bottom },
        terminal: { top: terminal.top, bottom: terminal.bottom },
        grid: { top: grid.top, bottom: grid.bottom },
      };
    });
    expect(geometry.badge, `${id} action keycap`).not.toBe("");
    expect(geometry.badgeBorder, `${id} action keycap border`).toBe("solid");
    expect(geometry.badgeBackground, `${id} action keycap background`).not.toBe("rgba(0, 0, 0, 0)");
    expect(geometry.covers, `${id} retained frame cleanup`).toBe(0);
    expect(geometry.terminal.top, `${id} terminal top`).toBeGreaterThanOrEqual(geometry.screen.top - 1);
    expect(geometry.terminal.bottom, `${id} terminal bottom`).toBeLessThanOrEqual(geometry.screen.bottom + 1);
    expect(geometry.grid.bottom, `${id} final terminal row`).toBeLessThanOrEqual(geometry.screen.bottom + 1);
  }
});

test("seeking retains the previous frame until the terminal has repainted", async ({ page }) => {
  await openReady(page);
  const player = page.locator("#harness-demo [data-demo-player]");
  const immediate = await player.evaluate((node) => {
    const controller = (node as PlayerNode)._realCastPlayer;
    controller.pause();
    controller.seekTo(controller.manifest.duration * 0.6);
    return {
      covers: node.querySelectorAll(".story-frame-cover").length,
      wrappers: node.querySelectorAll(".ap-wrapper").length,
    };
  });
  expect(immediate.covers).toBe(1);
  expect(immediate.wrappers).toBe(2);
  await page.waitForTimeout(180);
  await expect(player.locator(".story-frame-cover")).toHaveCount(0);
  await expect(player.locator(".ap-wrapper")).toHaveCount(1);
});

test("keyboard focus, reduced motion, and accessibility remain intact", async ({ browser }) => {
  const context = await browser.newContext({ reducedMotion: "reduce" });
  const page = await context.newPage();
  await openReady(page);

  const progress = page.locator("#start [data-demo-progress]");
  await page.waitForTimeout(400);
  await expect(progress).toHaveValue("0");
  const firstTab = page.locator("#harness-demo").getByRole("tab", { name: "Claude Code" });
  await firstTab.focus();
  await expect(firstTab).toBeFocused();

  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
  await context.close();
});
