import { resolve } from "node:path";
import type { Capabilities, Options } from "@wdio/types";
import type { TauriCapabilities, TauriServiceOptions } from "@wdio/tauri-service";
import { resultRoot, sharedConfig } from "./wdio.shared";

const executableName = process.platform === "win32" ? "setwright-desktop.exe" : "setwright-desktop";
const appBinaryPath = resolve(process.env.SETWRIGHT_E2E_BINARY ?? `src-tauri/target/release/${executableName}`);
const embeddedPort = Number(process.env.TAURI_WEBDRIVER_PORT ?? "4445");
const serviceOptions: TauriServiceOptions = {
  appBinaryPath,
  driverProvider: "embedded",
  embeddedPort,
  captureBackendLogs: true,
  captureFrontendLogs: true,
  backendLogLevel: "info",
  frontendLogLevel: "info",
  logDir: resolve(resultRoot, "logs"),
  startTimeout: 120_000,
  statusPollTimeout: 5_000,
};
const capability: TauriCapabilities = {
  browserName: "tauri",
  "tauri:options": {
    application: appBinaryPath,
  },
  "wdio:tauriServiceOptions": serviceOptions,
};

export const config = {
  ...sharedConfig,
  services: [["@wdio/tauri-service", serviceOptions]],
  capabilities: [capability],
} satisfies Options.Testrunner & Capabilities.WithRequestedTestrunnerCapabilities;
