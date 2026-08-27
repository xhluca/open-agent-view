import { defineConfig } from "@playwright/test";

const port = process.env.OAV_SITE_TEST_PORT ?? "48173";
const baseURL = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./tests",
  testMatch: "visual.spec.ts",
  timeout: 30_000,
  fullyParallel: false,
  use: {
    baseURL,
    browserName: "chromium",
    trace: "retain-on-failure",
  },
  webServer: {
    command: `npm run export && node scripts/serve-static.mjs ${port}`,
    url: baseURL,
    // Exercise the exact static export that is published, not vinext's dev server.
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
