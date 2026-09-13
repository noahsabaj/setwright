import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";
import { getDocument } from "pdfjs-dist";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PreviewPane } from "../components/PreviewPane";
import { useWorkspaceStore } from "../store/workspace-store";

vi.mock("pdfjs-dist", () => ({ GlobalWorkerOptions: { workerSrc: "" }, getDocument: vi.fn() }));

function makePdf(numPages: number) {
  const cancel = vi.fn();
  const cleanup = vi.fn();
  const page = {
    getViewport: vi.fn().mockReturnValue({ width: 612, height: 792 }),
    render: vi.fn().mockReturnValue({ promise: Promise.resolve(), cancel }),
    cleanup,
  } as unknown as PDFPageProxy;
  const getPage = vi.fn().mockResolvedValue(page);
  const document = { numPages, getPage } as unknown as PDFDocumentProxy;
  return { document, getPage, cancel, cleanup };
}

function makeTask(promise: Promise<PDFDocumentProxy>) {
  const destroy = vi.fn().mockResolvedValue(undefined);
  return { task: { promise, destroy } as unknown as PDFDocumentLoadingTask, destroy };
}

beforeEach(() => {
  vi.clearAllMocks();
  useWorkspaceStore.setState({ previewPage: 1, previewZoom: 82, previewScrollTop: 0, compileState: "idle" });
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({} as CanvasRenderingContext2D);
});

afterEach(() => vi.restoreAllMocks());

describe("PDF artifact controls and lifecycle", () => {
  it("keeps page, zoom and export controls usable for stale PDFs and failed recompilation", async () => {
    const user = userEvent.setup();
    const pdf = makePdf(3);
    const loading = makeTask(Promise.resolve(pdf.document));
    vi.mocked(getDocument).mockReturnValue(loading.task);
    const onExport = vi.fn();
    const onCompile = vi.fn();
    const { unmount } = render(<PreviewPane pdfBytes={new Uint8Array([1, 2, 3])} stale compileStatus="failed" onCompile={onCompile} onExport={onExport} />);
    expect(await screen.findByText("Compile failed · previous PDF shown")).toBeInTheDocument();
    expect(screen.getByText(/This PDF shows an earlier version/)).toBeInTheDocument();
    const page = await screen.findByRole("spinbutton", { name: "PDF page" });
    expect(page).toHaveValue(1);
    expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(page).toHaveValue(2);
    await user.click(screen.getByRole("button", { name: "Next page" }));
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
    fireEvent.change(page, { target: { value: "2.5" } });
    expect(page).toHaveValue(2);
    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(screen.getByText("92%")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Export PDF" }));
    await user.click(screen.getByRole("button", { name: "Recompile" }));
    expect(onExport).toHaveBeenCalledOnce();
    expect(onCompile).toHaveBeenCalledOnce();
    unmount();
    expect(loading.destroy).toHaveBeenCalledOnce();
  });

  it("destroys superseded loading tasks and ignores a late document from the previous artifact", async () => {
    let finishFirst: (document: PDFDocumentProxy) => void = () => undefined;
    const first = makePdf(10);
    const second = makePdf(2);
    const firstTask = makeTask(new Promise((resolve) => { finishFirst = resolve; }));
    const secondTask = makeTask(Promise.resolve(second.document));
    vi.mocked(getDocument).mockReturnValueOnce(firstTask.task).mockReturnValueOnce(secondTask.task);
    useWorkspaceStore.setState({ previewPage: 9 });
    const { rerender, unmount } = render(<PreviewPane pdfBytes={new Uint8Array([1])} />);
    await waitFor(() => expect(getDocument).toHaveBeenCalledOnce());
    expect(screen.getByText("Loading PDF…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Zoom in" })).toBeDisabled();
    rerender(<PreviewPane pdfBytes={new Uint8Array([2])} />);
    expect(firstTask.destroy).toHaveBeenCalledOnce();
    const page = await screen.findByRole("spinbutton", { name: "PDF page" });
    expect(page).toHaveValue(2);
    act(() => { finishFirst(first.document); });
    await waitFor(() => expect(second.getPage).toHaveBeenCalledWith(2));
    expect(first.getPage).not.toHaveBeenCalled();
    expect(screen.getByText("of 2")).toBeInTheDocument();
    unmount();
    expect(secondTask.destroy).toHaveBeenCalledOnce();
  });

  it("drops the old PDF error when the artifact is removed", async () => {
    const task = makeTask(Promise.reject(new Error("The PDF is malformed.")));
    vi.mocked(getDocument).mockReturnValue(task.task);
    const { rerender } = render(<PreviewPane pdfBytes={new Uint8Array([0])} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("The PDF is malformed.");
    expect(screen.getByText("PDF preview unavailable")).toBeInTheDocument();
    expect(screen.getByText("Pages unavailable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Zoom in" })).toBeDisabled();
    expect(screen.queryByText("PDF ready")).not.toBeInTheDocument();
    rerender(<PreviewPane runtimeReason="Compiler unavailable" />);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByText("Compiler unavailable")).toBeInTheDocument();
    expect(task.destroy).toHaveBeenCalledOnce();
  });

  it("hides the previous canvas immediately while replacement bytes load and after they fail", async () => {
    const pdf = makePdf(1);
    const validTask = makeTask(Promise.resolve(pdf.document));
    let rejectReplacement: (error: Error) => void = () => undefined;
    const invalidTask = makeTask(new Promise((_resolve, reject) => { rejectReplacement = reject; }));
    vi.mocked(getDocument).mockReturnValueOnce(validTask.task).mockReturnValueOnce(invalidTask.task);
    const { rerender } = render(<PreviewPane pdfBytes={new Uint8Array([1])} />);
    const canvas = screen.getByLabelText("Rendered PDF page 1");
    await waitFor(() => expect(canvas).toBeVisible());
    rerender(<PreviewPane pdfBytes={new Uint8Array([0])} />);
    expect(canvas).toBeInTheDocument();
    expect(canvas).not.toBeVisible();
    expect(screen.getByText("Loading PDF…")).toBeInTheDocument();
    await waitFor(() => expect(getDocument).toHaveBeenCalledTimes(2));
    act(() => { rejectReplacement(new Error("The replacement PDF is malformed.")); });
    expect(await screen.findByRole("alert")).toHaveTextContent("The replacement PDF is malformed.");
    expect(canvas).not.toBeVisible();
    expect(screen.getByText("PDF preview unavailable")).toBeInTheDocument();
  });
});
