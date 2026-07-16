import {
  GlobalWorkerOptions,
  getDocument,
  version as pdfJsVersion,
} from "pdfjs-dist/legacy/build/pdf.mjs";
import type {
  PDFDocumentLoadingTask,
  PDFDocumentProxy,
  PDFPageProxy,
} from "pdfjs-dist";
import pdfWorkerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";

// Configure the worker exactly once from the same installed pdfjs-dist package
// as the API entry point. Vite fingerprints and serves this local module; there
// is deliberately no CDN or mismatched-worker fallback.
GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

export interface PdfDocumentMetadata {
  pageCount: number;
}

export interface PdfRenderRequest {
  pageNumber: number;
  scale: number;
  canvas: HTMLCanvasElement;
  signal: AbortSignal;
}

export interface PdfRenderedPage {
  width: number;
  height: number;
}

export interface PdfDocumentSession {
  ready(): Promise<PdfDocumentMetadata>;
  renderPage(request: PdfRenderRequest): Promise<PdfRenderedPage>;
  dispose(): Promise<void>;
}

export interface PdfEngine {
  readonly pdfJsVersion: string;
  createSession(bytes: Uint8Array): PdfDocumentSession;
}

export interface PdfViewportPort {
  width: number;
  height: number;
}

export interface PdfRenderTaskPort {
  promise: Promise<void>;
  cancel(): void;
}

export interface PdfPagePort {
  getViewport(input: { scale: number }): PdfViewportPort;
  render(input: {
    canvas: HTMLCanvasElement;
    canvasContext: CanvasRenderingContext2D;
    viewport: PdfViewportPort;
  }): PdfRenderTaskPort;
  cleanup(): void;
}

export interface PdfDocumentPort {
  numPages: number;
  getPage(pageNumber: number): Promise<PdfPagePort>;
}

export interface PdfLoadingTaskPort {
  promise: Promise<PdfDocumentPort>;
  usesDedicatedWorker(): boolean;
  destroy(): Promise<void>;
}

export interface PdfJsAdapter {
  readonly version: string;
  createLoadingTask(bytes: Uint8Array): PdfLoadingTaskPort;
}

export class PdfOperationCancelledError extends Error {
  constructor(message = "The PDF operation was cancelled.") {
    super(message);
    this.name = "AbortError";
  }
}

export function isPdfOperationCancelled(cause: unknown): boolean {
  return cause instanceof PdfOperationCancelledError
    || (cause instanceof Error
      && (cause.name === "AbortError" || cause.name === "RenderingCancelledException"));
}

function adaptPage(page: PDFPageProxy): PdfPagePort {
  return {
    getViewport: ({ scale }) => page.getViewport({ scale }),
    render: ({ canvas, canvasContext, viewport }) => page.render({
      canvas,
      canvasContext,
      viewport: viewport as ReturnType<PDFPageProxy["getViewport"]>,
    }),
    cleanup: () => {
      page.cleanup();
    },
  };
}

function adaptDocument(document: PDFDocumentProxy): PdfDocumentPort {
  return {
    numPages: document.numPages,
    getPage: async (pageNumber) => adaptPage(await document.getPage(pageNumber)),
  };
}

function adaptLoadingTask(loadingTask: PDFDocumentLoadingTask): PdfLoadingTaskPort {
  // PDF.js deliberately falls back to an in-process LoopbackPort when a real
  // Worker cannot start, but exposes no public discriminator on the loading
  // task that owns that worker. In the pinned 6.1.200 generic build `_worker`
  // is the task-owned PDFWorker and its public `port` is a native Worker only
  // for the dedicated-worker path. Keep this version-sensitive detail inside
  // the adapter; browser-floor and native E2E tests guard future upgrades.
  return {
    promise: loadingTask.promise.then(adaptDocument),
    usesDedicatedWorker: () => {
      if (typeof Worker === "undefined") return false;
      const worker: unknown = Reflect.get(loadingTask, "_worker");
      if (typeof worker !== "object" || worker === null) return false;
      const port: unknown = Reflect.get(worker, "port");
      return port instanceof Worker;
    },
    destroy: () => loadingTask.destroy(),
  };
}

const browserPdfJsAdapter: PdfJsAdapter = {
  version: pdfJsVersion,
  createLoadingTask(bytes) {
    return adaptLoadingTask(getDocument({ data: bytes }));
  },
};

class PdfDocumentSessionImpl implements PdfDocumentSession {
  readonly #loadingTask: PdfLoadingTaskPort;
  readonly #documentPromise: Promise<PdfDocumentPort>;
  readonly #renderTasks = new Set<PdfRenderTaskPort>();
  #disposed = false;
  #disposePromise: Promise<void> | null = null;

  constructor(bytes: Uint8Array, adapter: PdfJsAdapter) {
    this.#loadingTask = adapter.createLoadingTask(bytes.slice());
    this.#documentPromise = this.#loadingTask.promise.then((document) => {
      if (!this.#loadingTask.usesDedicatedWorker()) {
        throw new Error("PDF.js could not start its dedicated worker.");
      }
      return document;
    });
    void this.#documentPromise.catch(() => undefined);
  }

  async ready(): Promise<PdfDocumentMetadata> {
    const document = await this.#awaitDocument();
    return { pageCount: document.numPages };
  }

  async renderPage({ pageNumber, scale, canvas, signal }: PdfRenderRequest): Promise<PdfRenderedPage> {
    this.#throwIfCancelled(signal);
    const document = await this.#awaitDocument();
    if (!Number.isSafeInteger(pageNumber) || pageNumber < 1 || pageNumber > document.numPages) {
      throw new RangeError(`PDF page ${String(pageNumber)} is outside 1-${String(document.numPages)}.`);
    }
    if (!Number.isFinite(scale) || scale <= 0) {
      throw new RangeError("PDF render scale must be a positive finite number.");
    }

    const page = await document.getPage(pageNumber);
    let renderTask: PdfRenderTaskPort | null = null;
    const cancel = () => renderTask?.cancel();
    try {
      this.#throwIfCancelled(signal);
      const context = canvas.getContext("2d");
      if (context === null) throw new Error("Canvas rendering is unavailable.");
      const viewport = page.getViewport({ scale });
      canvas.width = Math.ceil(viewport.width);
      canvas.height = Math.ceil(viewport.height);
      renderTask = page.render({ canvas, canvasContext: context, viewport });
      this.#renderTasks.add(renderTask);
      signal.addEventListener("abort", cancel, { once: true });
      try {
        await renderTask.promise;
      } catch (cause: unknown) {
        if (this.#disposed || signal.aborted || isPdfOperationCancelled(cause)) {
          throw new PdfOperationCancelledError();
        }
        throw cause;
      }
      this.#throwIfCancelled(signal);
      return { width: canvas.width, height: canvas.height };
    } finally {
      signal.removeEventListener("abort", cancel);
      if (renderTask !== null) this.#renderTasks.delete(renderTask);
      page.cleanup();
    }
  }

  dispose(): Promise<void> {
    if (this.#disposePromise !== null) return this.#disposePromise;
    this.#disposed = true;
    const activeTasks = [...this.#renderTasks];
    for (const task of activeTasks) task.cancel();
    // Schedule destruction through a promise so a non-conforming adapter that
    // throws synchronously still shares this one, memoized disposal attempt.
    const destroyPromise = Promise.resolve().then(() => this.#loadingTask.destroy());
    this.#disposePromise = (async () => {
      const [destroyResult] = await Promise.all([
        Promise.allSettled([destroyPromise]),
        Promise.allSettled(activeTasks.map((task) => task.promise)),
      ]);
      const result = destroyResult[0];
      if (result?.status === "rejected") throw result.reason;
    })();
    return this.#disposePromise;
  }

  async #awaitDocument(): Promise<PdfDocumentPort> {
    if (this.#disposed) throw new PdfOperationCancelledError();
    try {
      const document = await this.#documentPromise;
      if (this.#disposed) throw new PdfOperationCancelledError();
      return document;
    } catch (cause: unknown) {
      if (this.#disposed || isPdfOperationCancelled(cause)) {
        throw new PdfOperationCancelledError();
      }
      throw cause;
    }
  }

  #throwIfCancelled(signal: AbortSignal): void {
    if (this.#disposed || signal.aborted) throw new PdfOperationCancelledError();
  }
}

export function createPdfEngine(adapter: PdfJsAdapter = browserPdfJsAdapter): PdfEngine {
  return {
    pdfJsVersion: adapter.version,
    createSession: (bytes) => new PdfDocumentSessionImpl(bytes, adapter),
  };
}

export const pdfEngine = createPdfEngine();
