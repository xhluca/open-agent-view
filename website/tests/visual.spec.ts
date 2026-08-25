import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "phone", width: 390, height: 844 },
] as const;

for (const viewport of viewports) {
  test(`${viewport.name} layout is complete, interactive, and horizontally bounded`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    await page.goto("/");
    await expect(page.getByRole("heading", { name: /See every agent/ })).toBeVisible();
    await expect(page.locator("html")).toHaveAttribute("data-stories-ready", "true");
    await expect(page.locator(".provider-row img")).toHaveCount(8);
    await expect(page.locator(".story-player")).toHaveCount(4);
    await expect(page.locator("video")).toHaveCount(0);
    await expect(page.locator('[data-story="story-overview"] [data-demo-screen]')).toContainText("opav");

    const overflow = await page.evaluate(() => ({
      viewport: document.documentElement.clientWidth,
      content: document.documentElement.scrollWidth,
    }));
    expect(overflow.content).toBeLessThanOrEqual(overflow.viewport);

    await page.screenshot({ path: testInfo.outputPath(`${viewport.name}.png`), fullPage: true });
  });
}

test("playback controls, provider deep links, keyboard tabs, and eight-second tab handoff work", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-stories-ready", "true");

  const overview = page.locator('[data-story="story-overview"]');
  const progress = overview.locator("[data-demo-progress]");
  await overview.scrollIntoViewIfNeeded();
  await expect.poll(async () => Number(await progress.inputValue())).toBeGreaterThan(0);
  await overview.getByRole("button", { name: "Pause demo" }).click();
  const pausedAt = Number(await progress.inputValue());
  await page.waitForTimeout(350);
  expect(Number(await progress.inputValue())).toBeLessThanOrEqual(pausedAt + 2);
  await overview.getByRole("button", { name: "Go forward five seconds" }).click();
  expect(Number(await progress.inputValue())).toBeGreaterThan(pausedAt + 100);
  await overview.getByRole("button", { name: "Restart demo" }).click();
  await expect.poll(async () => Number(await progress.inputValue())).toBeLessThan(40);

  await progress.fill("1000");
  await progress.dispatchEvent("input");
  await expect(overview.locator("[data-demo-status]")).toHaveText("COMPLETE");
  for (const harness of ["Claude Code", "OpenAI Codex", "Pi", "OpenCode", "Cursor", "GitHub Copilot", "Antigravity", "Terminal"]) {
    await expect(overview.locator("[data-demo-screen]")).toContainText(harness);
  }
  await page.waitForTimeout(350);
  expect(await progress.inputValue()).toBe("1000");

  await page.getByRole("link", { name: "Watch the Antigravity demo" }).click();
  const antigravity = page.getByRole("tab", { name: "Antigravity" });
  await expect(antigravity).toHaveAttribute("aria-selected", "true");
  await expect(page.locator("#harness-demo [data-demo-screen]")).toContainText("Antigravity");
  await antigravity.press("ArrowRight");
  await expect(page.getByRole("tab", { name: "Terminal" })).toHaveAttribute("aria-selected", "true");

  const controls = page.locator("#controls");
  await controls.scrollIntoViewIfNeeded();
  const rename = controls.getByRole("tab", { name: "Rename" });
  await expect(rename).toHaveAttribute("aria-selected", "true");
  await controls.locator("[data-demo-player]").dispatchEvent("demo-ended");
  await expect.poll(async () => {
    const width = await controls.locator("[data-tab-hold-progress]").evaluate((node) => parseFloat(getComputedStyle(node).width));
    return width;
  }).toBeGreaterThan(0);
  await expect(controls.getByRole("tab", { name: "Switch sessions" })).toHaveAttribute("aria-selected", "true", { timeout: 9_500 });
});

test("copy, keyboard focus, reduced motion, and accessibility work", async ({ browser }) => {
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"], reducedMotion: "reduce" });
  const page = await context.newPage();
  await page.goto("/");
  await expect(page.locator("html")).toHaveAttribute("data-stories-ready", "true");

  const copy = page.getByRole("button", { name: /curl -fsSL/ }).first();
  await expect(copy).toHaveAttribute("data-copy-ready", "true");
  await copy.focus();
  await expect(copy).toBeFocused();
  await copy.press("Enter");
  await expect(copy).toContainText("Copied");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("open-agent-view.github.io/install.sh");

  const progress = page.locator('[data-story="story-overview"] [data-demo-progress]');
  await page.waitForTimeout(400);
  expect(Number(await progress.inputValue())).toBe(0);
  await expect(page.locator('[data-story="story-overview"] [data-demo-status]')).toHaveText("PAUSED");

  const animationDuration = await page.locator(".brand-mark i").first().evaluate((node) => getComputedStyle(node).animationDuration);
  expect(["0s", "0.00001s", "1e-05s"]).toContain(animationDuration);

  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
  await context.close();
});
