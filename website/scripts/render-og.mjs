import { chromium } from "@playwright/test";
import { pathToFileURL } from "node:url";

const source = new URL("../assets/og.svg", import.meta.url);
const output = new URL("../public/og.png", import.meta.url);

const browser = await chromium.launch({ headless: true });

try {
  const page = await browser.newPage({
    viewport: { width: 1200, height: 630 },
    deviceScaleFactor: 1,
  });
  await page.goto(pathToFileURL(source.pathname).href, { waitUntil: "load" });
  await page.screenshot({ path: output.pathname, animations: "disabled" });
} finally {
  await browser.close();
}
