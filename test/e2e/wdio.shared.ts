import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { browser } from "@wdio/globals";
import type { Options } from "@wdio/types";

export const resultRoot = resolve(process.env.SETWRIGHT_PDF_RESULTS ?? "test-results/pdf-preview");

function safeFileName(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 100);
}

export const sharedConfig = {
  runner: "local",
  specs: [resolve("test/e2e/pdf-preview.e2e.ts")],
  maxInstances: 1,
  logLevel: "warn",
  bail: 0,
  waitforTimeout: 30_000,
  connectionRetryTimeout: 120_000,
  connectionRetryCount: 1,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 120_000,
  },
  afterTest: async (test, _context, result) => {
    if (result.passed) return;
    const screenshots = resolve(resultRoot, "screenshots");
    await mkdir(screenshots, { recursive: true });
    await browser.saveScreenshot(resolve(screenshots, `${safeFileName(test.fullTitle)}.png`));
  },
} satisfies Partial<Options.Testrunner>;
