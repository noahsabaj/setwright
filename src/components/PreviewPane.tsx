import { useEffect, useRef, useState } from "react";
import type { PDFDocumentLoadingTask, PDFDocumentProxy, RenderTask } from "pdfjs-dist";
import { Check, ChevronLeft, ChevronRight, Download, FileWarning, Maximize2, Minus, Plus, RefreshCw } from "lucide-react";
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { useWorkspaceStore } from "../store/workspace-store";
import { IconButton } from "./IconButton";

export interface PreviewPaneProps {
  pdfBytes?: Uint8Array | undefined;
  stale?: boolean | undefined;
  compileStatus?: "unavailable" | "compiling" | "success" | "failed" | undefined;
  onCompile?: (() => void) | undefined;
  onExport?: (() => void) | undefined;
}

export function PreviewPane({
  pdfBytes,
  stale = false,
  compileStatus,
  onCompile,
  onExport,
}: PreviewPaneProps) {
  const page = useWorkspaceStore((state) => state.previewPage);
  const zoom = useWorkspaceStore((state) => state.previewZoom);
  const scrollTop = useWorkspaceStore((state) => state.previewScrollTop);
  const setPage = useWorkspaceStore((state) => state.setPreviewPage);
  const setZoom = useWorkspaceStore((state) => state.setPreviewZoom);
  const setScrollTop = useWorkspaceStore((state) => state.setPreviewScrollTop);
  const [loadedPdf, setLoadedPdf] = useState<{ source: Uint8Array; document: PDFDocumentProxy } | null>(null);
  const [renderError, setRenderError] = useState<string | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const workspaceCompileState = useWorkspaceStore((state) => state.compileState);
  const resolvedCompileStatus = compileStatus ?? (workspaceCompileState === "idle" ? "unavailable" : workspaceCompileState);
  const hasPdf = pdfBytes !== undefined;
  const pdfDocument = loadedPdf !== null && loadedPdf.source === pdfBytes ? loadedPdf.document : null;

  useEffect(() => {
    if (pdfBytes === undefined) {
      return undefined;
    }
    let disposed = false;
    let loadedDocument: PDFDocumentProxy | null = null;
    let loadingTask: PDFDocumentLoadingTask | null = null;
    const loadPdf = async () => {
      const pdfjs = await import("pdfjs-dist");
      pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;
      loadingTask = pdfjs.getDocument({ data: pdfBytes.slice() });
      const document = await loadingTask.promise;
      loadedDocument = document;
      if (disposed) {
        await document.destroy();
        loadedDocument = null;
        return;
      }
      const storedPage = useWorkspaceStore.getState().previewPage;
      setPage(Math.min(Math.max(1, storedPage), document.numPages));
      setLoadedPdf({ source: pdfBytes, document });
    };
    void loadPdf().catch((cause: unknown) => {
      if (!disposed) setRenderError(cause instanceof Error ? cause.message : "The PDF could not be loaded.");
    });
    return () => {
      disposed = true;
      if (loadedDocument !== null) void loadedDocument.destroy();
      else void loadingTask?.destroy();
    };
  }, [pdfBytes, setPage]);

  useEffect(() => {
    if (pdfDocument === null || canvasRef.current === null) return undefined;
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    const renderPage = async () => {
      const pdfPage = await pdfDocument.getPage(Math.min(page, pdfDocument.numPages));
      if (cancelled || canvasRef.current === null) return;
      const viewport = pdfPage.getViewport({ scale: 1.35 * (zoom / 100) });
      const canvas = canvasRef.current;
      const context = canvas.getContext("2d");
      if (context === null) throw new Error("Canvas rendering is unavailable.");
      canvas.width = Math.ceil(viewport.width);
      canvas.height = Math.ceil(viewport.height);
      renderTask = pdfPage.render({ canvas, canvasContext: context, viewport });
      await renderTask.promise;
      pdfPage.cleanup();
      if (!cancelled) setRenderError(null);
    };
    void renderPage().catch((cause: unknown) => {
      if (!cancelled && cause instanceof Error && cause.name !== "RenderingCancelledException") setRenderError(cause.message);
    });
    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
  }, [page, pdfDocument, zoom]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller !== null) scroller.scrollTop = scrollTop;
  }, [pdfDocument, scrollTop]);

  const emptyStatus = resolvedCompileStatus === "compiling"
    ? "Compiling project snapshot…"
    : resolvedCompileStatus === "failed"
      ? "Compile failed · no new PDF published"
      : resolvedCompileStatus === "success"
        ? "Compiled PDF bytes unavailable"
        : "No PDF compiled";
  const pageCount = pdfDocument?.numPages ?? 0;

  return (
    <section className="preview-pane" aria-label="Compiled PDF preview">
      <div className="preview-toolbar">
        <div className="preview-toolbar__group">
          <span className={`preview-status${hasPdf ? "" : " preview-status--unavailable"}`}>
            {hasPdf ? <Check size={13} aria-hidden="true" /> : <FileWarning size={13} aria-hidden="true" />}
            {hasPdf ? (stale ? "Last successful PDF · stale" : "PDF ready") : emptyStatus}
          </span>
          <button
            className="preview-compile"
            type="button"
            disabled={onCompile === undefined || resolvedCompileStatus === "compiling"}
            title={onCompile === undefined ? "Compilation is blocked until the managed runtime and OS sandbox are attested." : undefined}
            onClick={onCompile}
          >
            <RefreshCw size={13} aria-hidden="true" />
            {resolvedCompileStatus === "compiling" ? "Compiling…" : onCompile === undefined ? "Compile unavailable" : hasPdf ? "Recompile" : "Compile"}
          </button>
        </div>
        <div className="preview-toolbar__group preview-toolbar__pages">
          {pageCount > 0 ? (
            <>
              <IconButton label="Previous page" disabled={page === 1} onPress={() => setPage(Math.max(1, page - 1))}><ChevronLeft size={14} /></IconButton>
              <label>
                <span className="sr-only">Page</span>
                <input
                  value={page}
                  min={1}
                  max={pageCount}
                  type="number"
                  onChange={(event) => setPage(Math.min(pageCount, Math.max(1, Number(event.target.value) || 1)))}
                />
                <span>of {pageCount}</span>
              </label>
              <IconButton label="Next page" disabled={page === pageCount} onPress={() => setPage(Math.min(pageCount, page + 1))}><ChevronRight size={14} /></IconButton>
            </>
          ) : <span className="preview-page-unavailable">{hasPdf ? "Loading pages…" : "No pages"}</span>}
        </div>
        <div className="preview-toolbar__group">
          <IconButton label="Zoom out" disabled={!hasPdf} onPress={() => setZoom(Math.max(50, zoom - 10))}><Minus size={14} /></IconButton>
          <span className="preview-zoom">{hasPdf ? `${String(zoom)}%` : "—"}</span>
          <IconButton label="Zoom in" disabled={!hasPdf} onPress={() => setZoom(Math.min(160, zoom + 10))}><Plus size={14} /></IconButton>
          <IconButton label="Fit page" disabled={!hasPdf} onPress={() => setZoom(82)}><Maximize2 size={14} /></IconButton>
          <IconButton label={onExport === undefined ? "Export PDF unavailable" : "Export PDF"} disabled={!hasPdf || onExport === undefined} onPress={() => onExport?.()}><Download size={14} /></IconButton>
        </div>
      </div>

      <div
        className="preview-pane__scroller"
        ref={scrollerRef}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        {hasPdf ? <canvas className="pdf-canvas" ref={canvasRef} aria-label={`Rendered PDF page ${String(page)}`} /> : (
          <div className="preview-empty" role="status">
            <FileWarning size={28} aria-hidden="true" />
            <strong>{emptyStatus}</strong>
            <p>No PDF artifact has been published for this project revision.</p>
          </div>
        )}
        {renderError === null ? null : <p className="pdf-render-error" role="alert">{renderError}</p>}
      </div>
    </section>
  );
}
