import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "phone", width: 390, height: 844 },
] as const;

for (const viewport of viewports) {
  test(`${viewport.name} layout is complete, local, and horizontally bounded`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    await page.goto("/");
    await expect(page.getByRole("heading", { name: /See every agent/ })).toBeVisible();
    await expect(page.locator(".product-frame img")).toHaveJSProperty("complete", true);
    await expect(page.locator("video")).toBeVisible();
    await expect(page.getByText("GitHub Copilot", { exact: true }).first()).toBeVisible();

    const overflow = await page.evaluate(() => ({
      viewport: document.documentElement.clientWidth,
      content: document.documentElement.scrollWidth,
    }));
    expect(overflow.content).toBeLessThanOrEqual(overflow.viewport);

    await page.screenshot({ path: testInfo.outputPath(`${viewport.name}.png`), fullPage: true });
  });
}

test("copy, keyboard focus, FAQ, reduced motion, and accessibility work", async ({ browser }) => {
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"], reducedMotion: "reduce" });
  const page = await context.newPage();
  await page.goto("/");

  const copy = page.getByRole("button", { name: /curl -fsSL/ }).first();
  await expect(copy).toHaveAttribute("data-copy-ready", "true");
  await copy.focus();
  await expect(copy).toBeFocused();
  await copy.press("Enter");
  await expect(copy).toContainText("Copied");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toContain("open-agent-view.github.io/install.sh");

  const question = page.getByText("Does Open Agent View replace the native CLIs?");
  await question.click();
  await expect(page.getByText(/shared queue and verified controls/)).toBeVisible();

  const animationDuration = await page.locator(".brand-mark i").first().evaluate((node) => getComputedStyle(node).animationDuration);
  expect(["0s", "0.00001s", "1e-05s"]).toContain(animationDuration);

  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
  await context.close();
});
