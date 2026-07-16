import type { Capabilities, Options } from "@wdio/types";
import { browser } from "@wdio/globals";
import chromeForTesting from "../../config/chrome-for-testing.json";
import { sharedConfig } from "./wdio.shared";

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0) throw new Error(`${name} must point to the pinned Chrome for Testing artifact.`);
  return value;
}

const chromeBinary = requiredEnvironment("SETWRIGHT_CHROME_BINARY");
const chromedriverBinary = requiredEnvironment("CHROMEDRIVER_PATH");
const devServerUrl = process.env.SETWRIGHT_PDF_URL ?? "http://127.0.0.1:4173";

export const config = {
  ...sharedConfig,
  baseUrl: devServerUrl,
  before: async () => {
    await browser.url(devServerUrl);
  },
  capabilities: [
    {
      browserName: "chrome",
      browserVersion: chromeForTesting.version,
      pageLoadStrategy: "eager",
      "wdio:enforceWebDriverClassic": true,
      "goog:chromeOptions": {
        binary: chromeBinary,
        args: ["--headless=new", "--disable-gpu", "--no-sandbox", "--disable-dev-shm-usage", "--window-size=1440,920"],
      },
      "wdio:chromedriverOptions": {
        binary: chromedriverBinary,
      },
    },
  ],
} satisfies Options.Testrunner & Capabilities.WithRequestedTestrunnerCapabilities;
