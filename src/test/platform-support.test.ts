import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import viteConfig from "../../vite.config";
import platformSupport from "../../config/platform-support.json";

function readRepositoryFile(path: string): string {
  return readFileSync(resolve(path), "utf8");
}

describe("platform support policy", () => {
  it("drives the production JavaScript targets", () => {
    expect(viteConfig.build?.target).toEqual([
      `chrome${String(platformSupport.browser.chromiumMinimumMajor)}`,
      `safari${String(platformSupport.browser.safariMinimumMajor)}`,
    ]);
  });

  it("matches Tauri packaging and native runtime diagnostics", () => {
    const tauri = JSON.parse(readRepositoryFile("src-tauri/tauri.conf.json")) as {
      app: {
        withGlobalTauri: boolean;
        security: { capabilities: string[]; csp: string };
      };
      bundle: {
        macOS: { minimumSystemVersion: string };
        windows: {
          minimumWebview2Version: string;
          webviewInstallMode: { type: string; silent: boolean };
        };
      };
    };
    expect(tauri.bundle.macOS.minimumSystemVersion).toBe(platformSupport.macos.minimumSystemVersion);
    expect(tauri.bundle.windows.minimumWebview2Version).toBe(platformSupport.windows.webview2Minimum);
    expect(tauri.bundle.windows.webviewInstallMode).toEqual({ type: "downloadBootstrapper", silent: true });
    expect(tauri.app.withGlobalTauri).toBe(false);
    expect(tauri.app.security.capabilities).toEqual(["main-window"]);
    expect(tauri.app.security.csp).toContain("worker-src 'self'");

    const ipc = readRepositoryFile("src-tauri/src/ipc.rs");
    expect(ipc).toContain("/../config/platform-support.json");
    expect(ipc).not.toContain(`Some("${platformSupport.windows.webview2Minimum}".to_owned())`);

    const buildScript = readRepositoryFile("src-tauri/build.rs");
    const containmentPlist = readRepositoryFile("src-tauri/native/macos/ContainmentHost-Info.plist");
    const xpcPlist = readRepositoryFile("src-tauri/native/macos/SetwrightCompilerXPC/Info.plist");
    for (const nativePolicy of [buildScript, containmentPlist, xpcPlist]) {
      expect(nativePolicy).toContain(platformSupport.macos.minimumSystemVersion);
    }

    const e2eConfig = JSON.parse(readRepositoryFile("src-tauri/tauri.e2e.conf.json")) as {
      app: { withGlobalTauri: boolean; security: { capabilities: unknown[] } };
    };
    expect(e2eConfig.app.withGlobalTauri).toBe(true);
    expect(JSON.stringify(e2eConfig.app.security.capabilities)).toContain("pdf-preview-e2e");
  });

  it("matches release documentation and runner coverage", () => {
    const support = readRepositoryFile("docs/platform-support.md");
    const releasing = readRepositoryFile("docs/releasing.md");
    const releaseWorkflow = readRepositoryFile(".github/workflows/release.yml");
    const ci = readRepositoryFile(".github/workflows/ci.yml");

    for (const value of [
      String(platformSupport.browser.chromiumMinimumMajor),
      String(platformSupport.browser.safariMinimumMajor),
      platformSupport.windows.webview2Minimum,
      platformSupport.macos.minimumSystemVersion,
      platformSupport.linux.minimumRelease,
    ]) {
      expect(support).toContain(value);
    }
    expect(releasing).toContain(platformSupport.windows.webview2Minimum);
    expect(releasing).toContain("macOS 15+");
    expect(releasing).toContain("Ubuntu 22.04");
    expect(releasing).toContain("August\n2027");
    expect(releaseWorkflow).toContain("macos-15-intel");
    expect(releaseWorkflow).toContain("macos-15");
    expect(releaseWorkflow).toContain("MACOSX_DEPLOYMENT_TARGET: '15.0'");
    expect(ci).toContain("PDF preview floor (Chromium 125)");
    expect(ci).toContain("PDF preview (${{ matrix.os }})");
    for (const runner of ["windows-2022", "ubuntu-22.04", "macos-15", "macos-15-intel"]) {
      expect(ci).toContain(runner);
    }
  });
});
