import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: ".", testMatch: "*.spec.mjs", workers: 1, retries: 0,
  timeout: 45_000, expect: { timeout: 15_000 },
  outputDir: "../../test-results/pdf-browser",
  use: {
    browserName: process.env.PDF_BROWSER ?? "chromium",
    launchOptions: process.env.PDF_BROWSER_PATH ? { executablePath: process.env.PDF_BROWSER_PATH } : {},
    viewport: { width: 1100, height: 800 },
    baseURL: "http://127.0.0.1:1425",
    trace: "retain-on-failure",
  },
  webServer: { command: "node scripts/pdf-preview-server.mjs", cwd: "../..", url: "http://127.0.0.1:1425/test/browser/index.html", timeout: 120_000, reuseExistingServer: false },
});
