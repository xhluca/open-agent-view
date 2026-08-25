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
    command: `CHOKIDAR_USEPOLLING=true npm run dev -- --host 127.0.0.1 --port ${port}`,
    url: baseURL,
    // Never accept an unrelated development server as the site under test.
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
