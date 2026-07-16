import type { WebviewRuntimeInfo } from "./contracts";
import { desktopBridge } from "./bridge";
import type { PdfEngine } from "./pdf-engine";
import { isPdfOperationCancelled, pdfEngine } from "./pdf-engine";
import { createPdfProbeFixture } from "./pdf-probe-fixture";

export type PdfPreviewFailureReason = "runtime-floor" | "worker-start" | "render-probe" | "cleanup";

export type PdfPreviewCapability =
  | {
      supported: true;
      pdfJsVersion: string;
      runtime: WebviewRuntimeInfo;
    }
  | {
      supported: false;
      pdfJsVersion: string;
      runtime: WebviewRuntimeInfo;
      reason: PdfPreviewFailureReason;
      message: string;
    };

export interface PdfCapabilityDependencies {
  engine: PdfEngine;
  getRuntimeInfo(): Promise<WebviewRuntimeInfo>;
  createCanvas(): HTMLCanvasElement;
}

const unknownRuntime: WebviewRuntimeInfo = {
  engine: "unknown",
  detectedVersion: null,
  minimumVersion: null,
  comparison: "not-comparable",
};

function updateGuidance(runtime: WebviewRuntimeInfo): string {
  switch (runtime.engine) {
    case "webview2":
      return "Update the Microsoft Edge WebView2 Runtime, then restart Setwright.";
    case "webkit":
      return "Install the latest macOS and Safari updates, then restart Setwright.";
    case "webkitgtk":
      return "Install current Ubuntu security updates for WebKitGTK, then restart Setwright.";
    default:
      return "Update the operating system webview, then restart Setwright.";
  }
}

function describeFailure(runtime: WebviewRuntimeInfo, detail: string): string {
  const detected = runtime.detectedVersion === null ? "unknown" : runtime.detectedVersion;
  return `PDF preview is unavailable in ${runtime.engine} ${detected}. ${updateGuidance(runtime)} ${detail}`;
}

function pixelMatches(
  pixel: Uint8ClampedArray,
  expected: "red" | "blue",
): boolean {
  const [red = 0, green = 0, blue = 0, alpha = 0] = pixel;
  if (alpha < 200) return false;
  return expected === "red"
    ? red > 180 && green < 80 && blue < 80
    : blue > 180 && red < 80 && green < 80;
}

export async function probePdfPreviewCapability(
  dependencies: PdfCapabilityDependencies,
): Promise<PdfPreviewCapability> {
  let runtime = unknownRuntime;
  try {
    runtime = await dependencies.getRuntimeInfo();
  } catch {
    // Runtime diagnostics improve the recovery message, but the real render
    // probe remains authoritative if the diagnostic command is unavailable.
  }

  if (runtime.comparison === "below-floor") {
    return {
      supported: false,
      pdfJsVersion: dependencies.engine.pdfJsVersion,
      runtime,
      reason: "runtime-floor",
      message: describeFailure(runtime, `Required runtime: ${runtime.minimumVersion ?? "a supported system webview"}.`),
    };
  }

  let session: ReturnType<PdfEngine["createSession"]> | null = null;
  let phase: PdfPreviewFailureReason = "worker-start";
  try {
    session = dependencies.engine.createSession(createPdfProbeFixture());
    const { pageCount } = await session.ready();
    if (pageCount !== 2) throw new Error(`The renderer reported ${String(pageCount)} probe pages instead of 2.`);
    phase = "render-probe";
    const canvas = dependencies.createCanvas();
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (context === null) throw new Error("The webview did not provide a readable 2D canvas.");

    for (const [pageNumber, expected] of [[1, "red"], [2, "blue"]] as const) {
      const controller = new AbortController();
      const rendered = await session.renderPage({ pageNumber, scale: 1, canvas, signal: controller.signal });
      if (rendered.width !== 200 || rendered.height !== 200) {
        throw new Error(`Probe page ${String(pageNumber)} rendered at ${String(rendered.width)}x${String(rendered.height)} instead of 200x200.`);
      }
      const pixel = context.getImageData(100, 100, 1, 1).data;
      if (!pixelMatches(pixel, expected)) {
        throw new Error(`Probe page ${String(pageNumber)} did not paint its expected ${expected} region.`);
      }
    }

    phase = "cleanup";
    const completedSession = session;
    session = null;
    await completedSession.dispose();
    return {
      supported: true,
      pdfJsVersion: dependencies.engine.pdfJsVersion,
      runtime,
    };
  } catch (cause: unknown) {
    let failure = cause;
    if (session !== null) {
      try {
        await session.dispose();
      } catch (cleanupCause: unknown) {
        phase = "cleanup";
        failure = new AggregateError(
          [cause, cleanupCause],
          "The PDF preview self-test could not release its worker cleanly.",
        );
      }
    }
    const detail = failure instanceof Error && !isPdfOperationCancelled(failure)
      ? failure.message
      : "The PDF worker or canvas renderer could not complete its self-test.";
    return {
      supported: false,
      pdfJsVersion: dependencies.engine.pdfJsVersion,
      runtime,
      reason: phase,
      message: describeFailure(runtime, detail),
    };
  }
}

const productionDependencies: PdfCapabilityDependencies = {
  engine: pdfEngine,
  getRuntimeInfo: () => desktopBridge.getWebviewRuntimeInfo(),
  createCanvas: () => document.createElement("canvas"),
};

let cachedCapability: Promise<PdfPreviewCapability> | null = null;

export function getPdfPreviewCapability(): Promise<PdfPreviewCapability> {
  cachedCapability ??= probePdfPreviewCapability(productionDependencies);
  return cachedCapability;
}

export function resetPdfPreviewCapabilityForTests(): void {
  cachedCapability = null;
}
