import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PdfDocumentSession, PdfRenderRequest } from "../lib/pdf-engine";
import { useWorkspaceStore } from "../store/workspace-store";

const mocks = vi.hoisted(() => ({
  getCapability: vi.fn(),
  createSession: vi.fn(),
}));

vi.mock("../lib/pdf-capability", () => ({
  getPdfPreviewCapability: mocks.getCapability,
}));

vi.mock("../lib/pdf-engine", () => ({
  pdfEngine: {
    pdfJsVersion: "6.1.200-test",
    createSession: mocks.createSession,
  },
  isPdfOperationCancelled: (cause: unknown) => cause instanceof Error && cause.name === "AbortError",
}));

import { PreviewPane } from "../components/PreviewPane";

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

function supportedCapability() {
  return {
    supported: true as const,
    pdfJsVersion: "6.1.200-test",
    runtime: {
      engine: "webview2" as const,
      detectedVersion: "125.0.2535.41",
      minimumVersion: "125.0.2535.41",
      comparison: "meets-floor" as const,
    },
  };
}

function sessionHarness(pageCount = 2, readyPromise = Promise.resolve({ pageCount })) {
  const renderPage = vi.fn((request: PdfRenderRequest) => {
    void request;
    return Promise.resolve({ width: 200, height: 200 });
  });
  const dispose = vi.fn(() => Promise.resolve());
  const session: PdfDocumentSession = {
    ready: () => readyPromise,
    renderPage,
    dispose,
  };
  return { session, renderPage, dispose };
}

describe("PreviewPane PDF lifecycle", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useWorkspaceStore.setState({ previewPage: 1, previewZoom: 82, previewScrollTop: 0 });
    mocks.getCapability.mockResolvedValue(supportedCapability());
  });

  it("shows a scoped recovery state when the webview probe is unsupported", async () => {
    mocks.getCapability.mockResolvedValue({
      supported: false,
      pdfJsVersion: "6.1.200",
      runtime: {
        engine: "webkit",
        detectedVersion: "old",
        minimumVersion: "macOS 15.0 with current system updates",
        comparison: "not-comparable",
      },
      reason: "render-probe",
      message: "PDF preview is unavailable. Install the latest macOS updates.",
    });

    render(<PreviewPane pdfBytes={new Uint8Array([1])} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("PDF preview unavailable");
    expect(screen.getByRole("alert")).toHaveTextContent("latest macOS updates");
    expect(mocks.createSession).not.toHaveBeenCalled();
    expect(screen.getByText("Compile unavailable")).toBeInTheDocument();
  });

  it("loads and renders through the session without exposing PDF.js to React", async () => {
    const test = sessionHarness(2);
    mocks.createSession.mockReturnValue(test.session);

    render(<PreviewPane pdfBytes={new Uint8Array([1])} />);

    expect(await screen.findByText("of 2")).toBeInTheDocument();
    await waitFor(() => expect(test.renderPage).toHaveBeenCalledOnce());
    expect(screen.getByTestId("pdf-preview-canvas")).toBeInTheDocument();
    expect(screen.getByText("PDF ready")).toBeInTheDocument();
  });

  it("discards a late document after a rapid byte replacement", async () => {
    const firstReady = deferred<{ pageCount: number }>();
    const first = sessionHarness(7, firstReady.promise);
    const second = sessionHarness(2);
    mocks.createSession.mockReturnValueOnce(first.session).mockReturnValueOnce(second.session);
    const firstBytes = new Uint8Array([1]);
    const secondBytes = new Uint8Array([2]);
    const view = render(<PreviewPane pdfBytes={firstBytes} />);
    await waitFor(() => expect(mocks.createSession).toHaveBeenCalledOnce());

    view.rerender(<PreviewPane pdfBytes={secondBytes} />);
    expect(await screen.findByText("of 2")).toBeInTheDocument();
    act(() => firstReady.resolve({ pageCount: 7 }));

    expect(screen.queryByText("of 7")).not.toBeInTheDocument();
    expect(first.dispose).toHaveBeenCalled();
    expect(second.renderPage).toHaveBeenCalled();
  });

  it("disposes a loading session when the component unmounts", async () => {
    const ready = deferred<{ pageCount: number }>();
    const test = sessionHarness(2, ready.promise);
    mocks.createSession.mockReturnValue(test.session);
    const view = render(<PreviewPane pdfBytes={new Uint8Array([1])} />);
    await waitFor(() => expect(mocks.createSession).toHaveBeenCalledOnce());

    view.unmount();

    expect(test.dispose).toHaveBeenCalledOnce();
  });

  it("aborts the previous render when page navigation starts a newer one", async () => {
    const requests: PdfRenderRequest[] = [];
    const test = sessionHarness(2);
    test.renderPage.mockImplementation((request) => {
      requests.push(request);
      return new Promise(() => undefined);
    });
    mocks.createSession.mockReturnValue(test.session);
    render(<PreviewPane pdfBytes={new Uint8Array([1])} />);
    await waitFor(() => expect(requests).toHaveLength(1));

    fireEvent.click(screen.getByLabelText("Next page"));
    await waitFor(() => expect(requests).toHaveLength(2));

    expect(requests[0]?.signal.aborted).toBe(true);
    expect(requests[1]?.pageNumber).toBe(2);
  });

  it("reports a malformed document without replacing it with stale state", async () => {
    const failure = Promise.reject<{ pageCount: number }>(new Error("Invalid PDF structure"));
    const test = sessionHarness(0, failure);
    mocks.createSession.mockReturnValue(test.session);

    render(<PreviewPane pdfBytes={new Uint8Array([0])} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Invalid PDF structure");
    expect(screen.getByText("PDF preview failed")).toBeInTheDocument();
    expect(test.dispose).toHaveBeenCalledOnce();
  });

  it("aborts the previous render when zoom changes and disposes on unmount", async () => {
    const requests: PdfRenderRequest[] = [];
    const test = sessionHarness(2);
    test.renderPage.mockImplementation((request) => {
      requests.push(request);
      return new Promise(() => undefined);
    });
    mocks.createSession.mockReturnValue(test.session);
    const view = render(<PreviewPane pdfBytes={new Uint8Array([1])} />);
    await waitFor(() => expect(requests).toHaveLength(1));

    fireEvent.click(screen.getByLabelText("Zoom in"));
    await waitFor(() => expect(requests).toHaveLength(2));
    expect(requests[0]?.signal.aborted).toBe(true);
    expect(requests[1]?.scale).toBeGreaterThan(requests[0]?.scale ?? Number.POSITIVE_INFINITY);

    view.unmount();
    expect(requests[1]?.signal.aborted).toBe(true);
    expect(test.dispose).toHaveBeenCalledOnce();
  });
});
