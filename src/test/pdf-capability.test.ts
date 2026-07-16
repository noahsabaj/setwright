import { createHash } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import type { WebviewRuntimeInfo } from "../lib/contracts";
import { probePdfPreviewCapability } from "../lib/pdf-capability";
import type { PdfDocumentSession, PdfEngine } from "../lib/pdf-engine";
import type { PdfRenderRequest } from "../lib/pdf-engine";
import { createPdfProbeFixture, PDF_PROBE_FIXTURE_SHA256 } from "../lib/pdf-probe-fixture";

const supportedRuntime: WebviewRuntimeInfo = {
  engine: "webview2",
  detectedVersion: "125.0.2535.41",
  minimumVersion: "125.0.2535.41",
  comparison: "meets-floor",
};

function probeHarness(options: {
  runtime?: WebviewRuntimeInfo;
  readyError?: Error;
  corruptBluePixel?: boolean;
  createError?: Error;
  disposeError?: Error;
} = {}) {
  let renderedPage = 0;
  const disposeError = options.disposeError;
  const dispose = disposeError === undefined
    ? vi.fn(() => Promise.resolve())
    : vi.fn(() => Promise.reject(disposeError));
  const readyError = options.readyError;
  const ready = readyError === undefined
    ? vi.fn(() => Promise.resolve({ pageCount: 2 }))
    : vi.fn(() => Promise.reject(readyError));
  const renderPage = vi.fn((request: PdfRenderRequest) => {
    renderedPage = request.pageNumber;
    return Promise.resolve({ width: 200, height: 200 });
  });
  const session: PdfDocumentSession = {
    ready,
    renderPage,
    dispose,
  };
  const createError = options.createError;
  const createSession = createError === undefined
    ? vi.fn(() => session)
    : vi.fn(() => { throw createError; });
  const engine: PdfEngine = {
    pdfJsVersion: "6.1.200",
    createSession,
  };
  const context = {
    getImageData: vi.fn(() => {
      if (renderedPage === 1) return { data: new Uint8ClampedArray([255, 0, 0, 255]) };
      return { data: new Uint8ClampedArray(options.corruptBluePixel === true
        ? [255, 255, 255, 255]
        : [0, 0, 255, 255]) };
    }),
  } as unknown as CanvasRenderingContext2D;
  const canvas = document.createElement("canvas");
  vi.spyOn(canvas, "getContext").mockReturnValue(context);
  return {
    dependencies: {
      engine,
      getRuntimeInfo: vi.fn(() => Promise.resolve(options.runtime ?? supportedRuntime)),
      createCanvas: vi.fn(() => canvas),
    },
    dispose,
    engine,
    createSession,
    renderPage,
    session,
  };
}

describe("PDF preview capability probe", () => {
  it("keeps the programmatic probe fixture byte-stable", () => {
    const fixture = createPdfProbeFixture();

    expect(fixture).toHaveLength(748);
    expect(createHash("sha256").update(fixture).digest("hex")).toBe(PDF_PROBE_FIXTURE_SHA256);
  });

  it("proves both expected vector pages with the real session contract", async () => {
    const test = probeHarness();

    await expect(probePdfPreviewCapability(test.dependencies)).resolves.toEqual({
      supported: true,
      pdfJsVersion: "6.1.200",
      runtime: supportedRuntime,
    });
    expect(test.createSession).toHaveBeenCalledOnce();
    expect(test.renderPage).toHaveBeenCalledTimes(2);
    expect(test.dispose).toHaveBeenCalledOnce();
  });

  it("rejects a known-below-floor runtime before starting PDF.js", async () => {
    const runtime: WebviewRuntimeInfo = {
      ...supportedRuntime,
      detectedVersion: "124.0.0.0",
      comparison: "below-floor",
    };
    const test = probeHarness({ runtime });
    const result = await probePdfPreviewCapability(test.dependencies);

    expect(result).toMatchObject({ supported: false, reason: "runtime-floor", runtime });
    expect(test.createSession).not.toHaveBeenCalled();
  });

  it("classifies loading failures separately from render failures", async () => {
    const loadFailure = probeHarness({ readyError: new Error("worker handshake failed") });
    await expect(probePdfPreviewCapability(loadFailure.dependencies)).resolves.toMatchObject({
      supported: false,
      reason: "worker-start",
    });
    expect(loadFailure.dispose).toHaveBeenCalledOnce();

    const renderFailure = probeHarness({ corruptBluePixel: true });
    await expect(probePdfPreviewCapability(renderFailure.dependencies)).resolves.toMatchObject({
      supported: false,
      reason: "render-probe",
    });
    expect(renderFailure.dispose).toHaveBeenCalledOnce();
  });

  it("turns synchronous worker startup and successful-probe cleanup failures into unsupported results", async () => {
    const startupFailure = probeHarness({ createError: new Error("Worker constructor failed") });
    await expect(probePdfPreviewCapability(startupFailure.dependencies)).resolves.toMatchObject({
      supported: false,
      reason: "worker-start",
    });
    expect(startupFailure.dispose).not.toHaveBeenCalled();

    const cleanupFailure = probeHarness({ disposeError: new Error("worker shutdown failed") });
    await expect(probePdfPreviewCapability(cleanupFailure.dependencies)).resolves.toMatchObject({
      supported: false,
      reason: "cleanup",
    });
    expect(cleanupFailure.dispose).toHaveBeenCalledOnce();
  });

  it("continues with the authoritative render probe when diagnostics fail", async () => {
    const test = probeHarness();
    test.dependencies.getRuntimeInfo.mockRejectedValue(new Error("IPC unavailable"));

    await expect(probePdfPreviewCapability(test.dependencies)).resolves.toMatchObject({
      supported: true,
      runtime: { engine: "unknown", detectedVersion: null },
    });
  });
});
