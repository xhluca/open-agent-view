import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

const harnesses = [
  ["claude", "Claude Code"],
  ["codex", "OpenAI Codex"],
  ["pi", "Pi"],
  ["opencode", "OpenCode"],
  ["cursor", "Cursor"],
  ["copilot", "GitHub Copilot"],
  ["antigravity", "Antigravity"],
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
    await expect(page.locator(".provider-row img")).toHaveCount(8);
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
