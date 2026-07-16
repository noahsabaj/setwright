import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { $, browser, expect } from "@wdio/globals";
import { resultRoot } from "./wdio.shared";

interface CanvasMetrics {
  width: number;
  height: number;
  center: number[];
  nonWhiteRatio: number;
}

async function waitForReady(): Promise<void> {
  const status = $(".preview-status");
  await browser.waitUntil(async () => (await status.getText()).includes("PDF ready"), {
    timeout: 30_000,
    timeoutMsg: "PDF preview did not reach the ready state",
  });
  await $("[data-testid='pdf-preview-canvas']").waitForDisplayed({ timeout: 30_000 });
}

async function canvasMetrics(): Promise<CanvasMetrics> {
  return browser.execute(() => {
    const canvas = document.querySelector("[data-testid='pdf-preview-canvas']");
    if (!(canvas instanceof HTMLCanvasElement)) throw new Error("The PDF canvas is missing.");
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (context === null) throw new Error("The PDF canvas has no 2D context.");
    const center = Array.from(context.getImageData(Math.floor(canvas.width / 2), Math.floor(canvas.height / 2), 1, 1).data);
    const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
    const stride = Math.max(1, Math.floor(Math.min(canvas.width, canvas.height) / 80));
    let samples = 0;
    let nonWhite = 0;
    for (let y = 0; y < canvas.height; y += stride) {
      for (let x = 0; x < canvas.width; x += stride) {
        const index = (y * canvas.width + x) * 4;
        const red = pixels[index] ?? 255;
        const green = pixels[index + 1] ?? 255;
        const blue = pixels[index + 2] ?? 255;
        const alpha = pixels[index + 3] ?? 0;
        samples += 1;
        if (alpha > 200 && (red < 245 || green < 245 || blue < 245)) nonWhite += 1;
      }
    }
    return {
      width: canvas.width,
      height: canvas.height,
      center,
      nonWhiteRatio: samples === 0 ? 0 : nonWhite / samples,
    };
  });
}

async function selectProbeFixture(): Promise<void> {
  await clickHarnessButton("Probe fixture");
  await waitForReady();
  const previous = $("[aria-label='Previous page']");
  if (await previous.isEnabled()) {
    await previous.click();
    await browser.waitUntil(async () => (await $(".preview-toolbar__pages input").getValue()) === "1");
  }
  const fitPage = $("[aria-label='Fit page']");
  await fitPage.waitForClickable({ timeout: 30_000 });
  await fitPage.click();
  await browser.waitUntil(async () => (await $(".preview-zoom").getText()) === "82%");
}

async function clickHarnessButton(label: string): Promise<void> {
  const button = $(`button=${label}`);
  await button.waitForDisplayed({ timeout: 30_000 });
  await button.waitForEnabled({ timeout: 30_000 });
  await button.click();
}

describe("durable PDF preview boundary", () => {
  beforeEach(async () => {
    await $("[data-testid='pdf-preview-e2e-harness']").waitForDisplayed({ timeout: 30_000 });
    await selectProbeFixture();
  });

  it("starts the real worker and paints the stable probe", async () => {
    await expect($("[data-testid='pdf-e2e-pdfjs-version']")).toHaveAttribute("data-value", "6.1.200");
    await expect($("[data-testid='pdf-e2e-probe-sha256']")).toHaveAttribute("data-value", "137a07612d530b29102095fdc6685439f430a91cc4df26ff19c416297e858c54");
    await expect($(".preview-toolbar__pages input")).toHaveValue("1");
    await expect($(".preview-toolbar__pages input")).toHaveAttribute("max", "2");
    const metrics = await canvasMetrics();
    expect(metrics.width).toBeGreaterThanOrEqual(220);
    expect(metrics.height).toBeGreaterThanOrEqual(220);
    expect(metrics.center[0]).toBeGreaterThan(180);
    expect(metrics.center[1]).toBeLessThan(80);
    expect(metrics.center[2]).toBeLessThan(80);
  });

  it("navigates to the second page and paints its blue region", async () => {
    await $("[aria-label='Next page']").click();
    await browser.waitUntil(async () => (await $(".preview-toolbar__pages input").getValue()) === "2");
    await browser.waitUntil(async () => (await canvasMetrics()).center[2] > 180);
    const metrics = await canvasMetrics();
    expect(metrics.center[0]).toBeLessThan(80);
    expect(metrics.center[1]).toBeLessThan(80);
    expect(metrics.center[2]).toBeGreaterThan(180);
  });

  it("rerenders at a larger canvas size when zoom changes", async () => {
    const before = await canvasMetrics();
    await $("[aria-label='Zoom in']").click();
    await browser.waitUntil(async () => (await canvasMetrics()).width > before.width);
    const after = await canvasMetrics();
    expect(after.width).toBeGreaterThan(before.width);
    expect(after.height).toBeGreaterThan(before.height);
  });

  it("renders the representative paper fixture", async () => {
    await clickHarnessButton("Representative fixture");
    await browser.waitUntil(async () => (await $("[data-testid='pdf-e2e-fixture']").getText()) === "representative");
    await waitForReady();
    await expect($("[data-testid='pdf-e2e-representative-sha256']")).toHaveAttribute(
      "data-value",
      "01fe33bf01f3e80ed62ce7e4f281277dfaf6b6d91e3e300b9337d29029faddbb",
    );
    await expect($(".preview-toolbar__pages input")).toHaveAttribute("max", "2");
    await browser.waitUntil(async () => (await canvasMetrics()).nonWhiteRatio > 0.005, {
      timeout: 30_000,
      timeoutMsg: "The representative PDF canvas never painted non-white content.",
    });
    const metrics = await canvasMetrics();
    expect(metrics.width).toBeGreaterThan(500);
    expect(metrics.height).toBeGreaterThan(700);
    expect(metrics.nonWhiteRatio).toBeGreaterThan(0.005);
  });

  it("suppresses stale completions during rapid byte replacement", async () => {
    await clickHarnessButton("Rapid replacement");
    await browser.waitUntil(async () => (await $("[data-testid='pdf-e2e-fixture']").getText()) === "rapid-representative");
    await waitForReady();
    await expect($(".preview-status")).toHaveText("PDF ready");
    await expect($(".pdf-render-error")).not.toExist();
  });

  it("reports corrupt input without disabling the rest of the harness", async () => {
    await clickHarnessButton("Corrupt input");
    const alert = $(".preview-empty[role='alert']");
    await alert.waitForDisplayed({ timeout: 30_000 });
    await expect(alert).toHaveText(expect.stringContaining("PDF could not be loaded"));
    await clickHarnessButton("Probe fixture");
    await waitForReady();
  });

  it("disposes work when the React preview unmounts and can restart", async () => {
    await clickHarnessButton("Unmount preview");
    await expect($("[data-testid='pdf-e2e-unmounted']")).toBeDisplayed();
    await expect($("[data-testid='pdf-preview-canvas']")).not.toExist();
    await clickHarnessButton("Restore preview");
    await waitForReady();
  });

  it("runs the worker under the strict production script policy", async () => {
    await browser.execute(() => {
      Reflect.set(window, "__setwrightInlineScriptRan", false);
      const script = document.createElement("script");
      script.textContent = "window.__setwrightInlineScriptRan = true";
      document.head.append(script);
    });
    await browser.pause(100);
    const inlineScriptRan = await browser.execute(() => Reflect.get(window, "__setwrightInlineScriptRan") === true);
    expect(inlineScriptRan).toBe(false);
    await expect($(".preview-status")).toHaveText("PDF ready");
  });

  after(async () => {
    const runtimeOutput = $("[data-testid='pdf-e2e-runtime']");
    await browser.waitUntil(async () => (await runtimeOutput.getAttribute("data-value")) !== "detecting", { timeout: 30_000 });
    const runtime = await runtimeOutput.getAttribute("data-value");
    if (runtime === null || runtime === "detecting") throw new Error("Webview runtime evidence was not populated.");
    const evidencePath = resolve(resultRoot, "evidence.json");
    const evidence = {
      schemaVersion: 1,
      platform: process.platform,
      architecture: process.arch,
      userAgent: await browser.execute(() => navigator.userAgent),
      runtime: JSON.parse(runtime) as unknown,
      pdfJsVersion: await $("[data-testid='pdf-e2e-pdfjs-version']").getAttribute("data-value"),
      workerMode: "dedicated",
      probeSha256: await $("[data-testid='pdf-e2e-probe-sha256']").getAttribute("data-value"),
      representativeSha256: await $("[data-testid='pdf-e2e-representative-sha256']").getAttribute("data-value"),
      canvas: await canvasMetrics(),
    };
    await mkdir(dirname(evidencePath), { recursive: true });
    await writeFile(evidencePath, `${JSON.stringify(evidence, null, 2)}\n`, "utf8");
  });
});
