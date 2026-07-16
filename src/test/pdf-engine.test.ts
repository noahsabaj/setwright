import { describe, expect, it, vi } from "vitest";
import {
  PdfOperationCancelledError,
  createPdfEngine,
  isPdfOperationCancelled,
} from "../lib/pdf-engine";
import type {
  PdfDocumentPort,
  PdfJsAdapter,
  PdfLoadingTaskPort,
  PdfPagePort,
  PdfRenderTaskPort,
} from "../lib/pdf-engine";

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(cause: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

function fakeCanvas(): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  const context = {} as CanvasRenderingContext2D;
  vi.spyOn(canvas, "getContext").mockReturnValue(context);
  return canvas;
}

class FakeRenderTask implements PdfRenderTaskPort {
  readonly completion = deferred<void>();
  readonly promise = this.completion.promise;
  cancelCount = 0;

  cancel(): void {
    this.cancelCount += 1;
    const cancellation = new Error("cancelled");
    cancellation.name = "RenderingCancelledException";
    this.completion.reject(cancellation);
  }
}

class FakePage implements PdfPagePort {
  readonly task = new FakeRenderTask();
  cleanupCount = 0;
  renderCount = 0;

  getViewport({ scale }: { scale: number }): { width: number; height: number } {
    return { width: 200 * scale, height: 300 * scale };
  }

  render(): PdfRenderTaskPort {
    this.renderCount += 1;
    return this.task;
  }

  cleanup(): void {
    this.cleanupCount += 1;
  }
}

function harness(options: {
  pageCount?: number;
  destroyError?: Error;
  destroySynchronousError?: Error;
  destroyPromise?: Promise<void>;
  usesDedicatedWorker?: boolean;
} = {}) {
  const documentReady = deferred<PdfDocumentPort>();
  const page = new FakePage();
  let destroyCount = 0;
  let capturedBytes: Uint8Array | null = null;
  let createCount = 0;
  const getPage = vi.fn(() => Promise.resolve(page));
  const document: PdfDocumentPort = {
    numPages: options.pageCount ?? 2,
    getPage,
  };
  const task: PdfLoadingTaskPort = {
    promise: documentReady.promise,
    usesDedicatedWorker: () => options.usesDedicatedWorker ?? true,
    destroy: vi.fn(() => {
      destroyCount += 1;
      if (options.destroySynchronousError !== undefined) {
        throw options.destroySynchronousError;
      }
      return options.destroyError === undefined
        ? options.destroyPromise ?? Promise.resolve()
        : Promise.reject(options.destroyError);
    }),
  };
  const adapter: PdfJsAdapter = {
    version: "6.1.200-test",
    createLoadingTask(bytes) {
      createCount += 1;
      capturedBytes = bytes;
      return task;
    },
  };
  return {
    adapter,
    document,
    documentReady,
    page,
    getPage,
    get createCount() { return createCount; },
    get destroyCount() { return destroyCount; },
    get capturedBytes() { return capturedBytes; },
  };
}

describe("PDF document sessions", () => {
  it("owns the loading task synchronously and copies caller bytes", async () => {
    const test = harness();
    const bytes = new Uint8Array([1, 2, 3]);
    const session = createPdfEngine(test.adapter).createSession(bytes);
    bytes[0] = 99;

    expect(test.createCount).toBe(1);
    expect(test.capturedBytes).toEqual(new Uint8Array([1, 2, 3]));
    test.documentReady.resolve(test.document);
    await expect(session.ready()).resolves.toEqual({ pageCount: 2 });
    await session.dispose();
  });

  it("fails closed when PDF.js silently selects its fake worker", async () => {
    const test = harness({ usesDedicatedWorker: false });
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    test.documentReady.resolve(test.document);

    await expect(session.ready()).rejects.toThrow("dedicated worker");
    await session.dispose();
    expect(test.destroyCount).toBe(1);
  });

  it("renders through one tracked task and always cleans the page", async () => {
    const test = harness();
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    test.documentReady.resolve(test.document);
    const canvas = fakeCanvas();
    const render = session.renderPage({
      pageNumber: 2,
      scale: 1.5,
      canvas,
      signal: new AbortController().signal,
    });
    await vi.waitFor(() => expect(test.page.renderCount).toBe(1));
    test.page.task.completion.resolve();

    await expect(render).resolves.toEqual({ width: 300, height: 450 });
    expect(canvas.width).toBe(300);
    expect(canvas.height).toBe(450);
    expect(test.page.cleanupCount).toBe(1);
    await session.dispose();
  });

  it("rejects invalid page and scale requests before allocating a page", async () => {
    const test = harness();
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    test.documentReady.resolve(test.document);
    const request = { canvas: fakeCanvas(), signal: new AbortController().signal };

    await expect(session.renderPage({ ...request, pageNumber: 3, scale: 1 })).rejects.toBeInstanceOf(RangeError);
    await expect(session.renderPage({ ...request, pageNumber: 1, scale: 0 })).rejects.toBeInstanceOf(RangeError);
    expect(test.getPage).not.toHaveBeenCalled();
    await session.dispose();
  });

  it("can be disposed before the loading promise settles", async () => {
    const test = harness();
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));

    await session.dispose();
    await expect(session.ready()).rejects.toBeInstanceOf(PdfOperationCancelledError);
    expect(test.destroyCount).toBe(1);
  });

  it("awaits loading-task destruction while a worker handshake is still pending", async () => {
    const destruction = deferred<void>();
    const test = harness({ destroyPromise: destruction.promise });
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    let settled = false;
    const dispose = session.dispose().then(() => { settled = true; });

    await Promise.resolve();
    expect(settled).toBe(false);
    expect(test.destroyCount).toBe(1);
    destruction.resolve();
    await dispose;
    expect(settled).toBe(true);
  });

  it("makes disposal idempotent", async () => {
    const test = harness();
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));

    await Promise.all([session.dispose(), session.dispose(), session.dispose()]);
    expect(test.destroyCount).toBe(1);
  });

  it("cancels and drains an active render while destroying the loading task", async () => {
    const test = harness();
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    test.documentReady.resolve(test.document);
    const render = session.renderPage({
      pageNumber: 1,
      scale: 1,
      canvas: fakeCanvas(),
      signal: new AbortController().signal,
    });
    await vi.waitFor(() => expect(test.page.renderCount).toBe(1));

    await session.dispose();
    await expect(render).rejects.toSatisfy(isPdfOperationCancelled);
    expect(test.page.task.cancelCount).toBe(1);
    expect(test.page.cleanupCount).toBe(1);
    expect(test.destroyCount).toBe(1);
  });

  it("tracks and drains concurrent page renders", async () => {
    const test = harness();
    const firstPage = new FakePage();
    const secondPage = new FakePage();
    test.document.getPage = vi.fn((pageNumber) => Promise.resolve(pageNumber === 1 ? firstPage : secondPage));
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    test.documentReady.resolve(test.document);
    const firstRender = session.renderPage({
      pageNumber: 1,
      scale: 1,
      canvas: fakeCanvas(),
      signal: new AbortController().signal,
    });
    const secondRender = session.renderPage({
      pageNumber: 2,
      scale: 1,
      canvas: fakeCanvas(),
      signal: new AbortController().signal,
    });
    await vi.waitFor(() => {
      expect(firstPage.renderCount).toBe(1);
      expect(secondPage.renderCount).toBe(1);
    });

    await session.dispose();
    await expect(firstRender).rejects.toSatisfy(isPdfOperationCancelled);
    await expect(secondRender).rejects.toSatisfy(isPdfOperationCancelled);
    expect(firstPage.task.cancelCount).toBe(1);
    expect(secondPage.task.cancelCount).toBe(1);
    expect(firstPage.cleanupCount).toBe(1);
    expect(secondPage.cleanupCount).toBe(1);
  });

  it("maps AbortSignal cancellation to the stable cancellation error", async () => {
    const test = harness();
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));
    test.documentReady.resolve(test.document);
    const controller = new AbortController();
    const render = session.renderPage({
      pageNumber: 1,
      scale: 1,
      canvas: fakeCanvas(),
      signal: controller.signal,
    });
    await vi.waitFor(() => expect(test.page.renderCount).toBe(1));
    controller.abort();

    await expect(render).rejects.toBeInstanceOf(PdfOperationCancelledError);
    expect(test.page.task.cancelCount).toBe(1);
    await session.dispose();
  });

  it("does not hide a loading-task destruction failure", async () => {
    const failure = new Error("worker shutdown failed");
    const test = harness({ destroyError: failure });
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));

    await expect(session.dispose()).rejects.toBe(failure);
    expect(test.destroyCount).toBe(1);
  });

  it("memoizes disposal when a loading-task adapter throws synchronously", async () => {
    const failure = new Error("synchronous worker shutdown failure");
    const test = harness({ destroySynchronousError: failure });
    const session = createPdfEngine(test.adapter).createSession(new Uint8Array([1]));

    const first = session.dispose();
    const second = session.dispose();

    expect(second).toBe(first);
    await expect(first).rejects.toBe(failure);
    await expect(session.dispose()).rejects.toBe(failure);
    expect(test.destroyCount).toBe(1);
  });
});
