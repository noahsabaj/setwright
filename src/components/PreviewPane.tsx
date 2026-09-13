import { useEffect, useRef, useState } from "react";
import type { PDFDocumentLoadingTask, PDFDocumentProxy, RenderTask } from "pdfjs-dist";
import { Check, ChevronLeft, ChevronRight, Download, FileWarning, Maximize2, Minus, Plus, RefreshCw } from "lucide-react";
import pdfWorkerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import { useWorkspaceStore } from "../store/workspace-store";
import { IconButton } from "./IconButton";

export interface PreviewPaneProps {
  pdfBytes?: Uint8Array | undefined;
  stale?: boolean | undefined;
  compileStatus?: "unavailable" | "compiling" | "success" | "failed" | undefined;
  onCompile?: (() => void) | undefined;
  onExport?: (() => void) | undefined;
  runtimeReason?: string | undefined;
  projectTitle?: string | undefined;
}

export function PreviewPane({
  pdfBytes,
  stale = false,
  compileStatus,
  onCompile,
  onExport,
  runtimeReason,
  projectTitle,
}: PreviewPaneProps) {
  const page = useWorkspaceStore((state) => state.previewPage);
  const zoom = useWorkspaceStore((state) => state.previewZoom);
  const scrollTop = useWorkspaceStore((state) => state.previewScrollTop);
  const setPage = useWorkspaceStore((state) => state.setPreviewPage);
  const setZoom = useWorkspaceStore((state) => state.setPreviewZoom);
  const setScrollTop = useWorkspaceStore((state) => state.setPreviewScrollTop);
  const [loadedPdf, setLoadedPdf] = useState<{ source: Uint8Array; document: PDFDocumentProxy } | null>(null);
  const [renderedFrame, setRenderedFrame] = useState<{ source: Uint8Array; page: number; zoom: number } | null>(null);
  const [renderFailure, setRenderFailure] = useState<{ source: Uint8Array; message: string } | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const workspaceCompileState = useWorkspaceStore((state) => state.compileState);
  const resolvedCompileStatus = compileStatus ?? (workspaceCompileState === "idle" ? "unavailable" : workspaceCompileState);
  const hasPdf = pdfBytes !== undefined;
  const pdfDocument = loadedPdf !== null && loadedPdf.source === pdfBytes ? loadedPdf.document : null;
  const renderError = renderFailure !== null && renderFailure.source === pdfBytes ? renderFailure.message : null;
  const canvasVisible = pdfDocument !== null && renderError === null && renderedFrame !== null
    && renderedFrame.source === pdfBytes && renderedFrame.page === page && renderedFrame.zoom === zoom;

  useEffect(() => {
    if (pdfBytes === undefined) {
      return undefined;
    }
    let disposed = false;
    let loadingTask: PDFDocumentLoadingTask | null = null;
    const loadPdf = async () => {
      // Use PDF.js's maintained compatibility build in both realms. Modern
      // builds require Promise.try, which is absent from Chromium 125.
      const pdfjs = await import("pdfjs-dist/legacy/build/pdf.mjs");
      if (disposed) return;
      pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;
      loadingTask = pdfjs.getDocument({ data: pdfBytes.slice() });
      const document = await loadingTask.promise;
      if (disposed) return;
      const storedPage = useWorkspaceStore.getState().previewPage;
      setPage(Math.min(Math.max(1, storedPage), document.numPages));
      setLoadedPdf({ source: pdfBytes, document });
    };
    void loadPdf().catch((cause: unknown) => {
      if (!disposed) setRenderFailure({ source: pdfBytes, message: cause instanceof Error ? cause.message : "The PDF could not be loaded." });
    });
    return () => {
      disposed = true;
      // The loading task owns the worker and document in the installed PDF.js API.
      void loadingTask?.destroy();
    };
  }, [pdfBytes, setPage]);

  useEffect(() => {
    if (pdfDocument === null || pdfBytes === undefined || canvasRef.current === null) return undefined;
    let cancelled = false;
    let renderTask: RenderTask | null = null;
    const renderPage = async () => {
      setRenderedFrame(null);
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
      if (!cancelled) {
        setRenderFailure(null);
        setRenderedFrame({ source: pdfBytes, page, zoom });
      }
    };
    void renderPage().catch((cause: unknown) => {
      if (!cancelled && cause instanceof Error && cause.name !== "RenderingCancelledException") setRenderFailure({ source: pdfBytes, message: cause.message });
    });
    return () => {
      cancelled = true;
      renderTask?.cancel();
    };
  }, [page, pdfDocument, pdfBytes, zoom]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller !== null) scroller.scrollTop = scrollTop;
  }, [pdfDocument, scrollTop]);

  const emptyStatus = resolvedCompileStatus === "compiling"
    ? "Compiling your paper…"
    : resolvedCompileStatus === "failed"
      ? "The paper could not be compiled"
      : resolvedCompileStatus === "success"
        ? "The compiled PDF could not be opened"
        : "No PDF compiled";
  const pageCount = pdfDocument?.numPages ?? 0;
  const unavailableReason = onCompile === undefined
    ? runtimeReason ?? "PDF compilation is not available in this workspace. You can keep writing and editing your source."
    : null;
  const pdfStatus = renderError !== null ? "PDF preview unavailable"
    : !canvasVisible ? "Loading PDF…"
    : resolvedCompileStatus === "compiling" ? "Updating PDF…"
    : resolvedCompileStatus === "failed" ? "Compile failed · previous PDF shown"
      : stale ? "Last successful PDF · stale" : "PDF ready";

  return (
    <section className="preview-pane" aria-label="Compiled PDF preview">
      <div className="preview-toolbar">
        <div className="preview-toolbar__group">
          <span className={`preview-status${canvasVisible && !stale && resolvedCompileStatus !== "failed" ? "" : " preview-status--unavailable"}`} role="status">
            {canvasVisible ? <Check size={13} aria-hidden="true" /> : <FileWarning size={13} aria-hidden="true" />}
            {hasPdf ? pdfStatus : emptyStatus}
          </span>
          <button
            className="preview-compile"
            type="button"
            disabled={onCompile === undefined || resolvedCompileStatus === "compiling"}
            title={unavailableReason ?? undefined}
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
                  aria-label="PDF page"
                  value={page}
                  min={1}
                  max={pageCount}
                  type="number"
                  onChange={(event) => setPage(Math.min(pageCount, Math.max(1, Math.trunc(Number(event.target.value)) || 1)))}
                />
                <span>of {pageCount}</span>
              </label>
              <IconButton label="Next page" disabled={page === pageCount} onPress={() => setPage(Math.min(pageCount, page + 1))}><ChevronRight size={14} /></IconButton>
            </>
          ) : <span className="preview-page-unavailable">{renderError !== null ? "Pages unavailable" : hasPdf ? "Loading pages…" : "No pages"}</span>}
        </div>
        <div className="preview-toolbar__group">
          <IconButton label="Zoom out" disabled={pdfDocument === null || zoom <= 50} onPress={() => setZoom(Math.max(50, zoom - 10))}><Minus size={14} /></IconButton>
          <span className="preview-zoom">{hasPdf ? `${String(zoom)}%` : "—"}</span>
          <IconButton label="Zoom in" disabled={pdfDocument === null || zoom >= 160} onPress={() => setZoom(Math.min(160, zoom + 10))}><Plus size={14} /></IconButton>
          <IconButton label="Fit page" disabled={pdfDocument === null} onPress={() => setZoom(82)}><Maximize2 size={14} /></IconButton>
          <IconButton label={onExport === undefined ? "Export PDF unavailable" : "Export PDF"} disabled={!hasPdf || onExport === undefined} onPress={() => onExport?.()}><Download size={14} /></IconButton>
        </div>
      </div>

      {hasPdf && stale ? <p className="preview-stale-note" role="status">This PDF shows an earlier version of your paper. Recompile to include your latest edits.</p> : null}
      {hasPdf && unavailableReason !== null ? <p className="preview-readiness">{unavailableReason}</p> : null}

      <div
        className="preview-pane__scroller"
        ref={scrollerRef}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        {hasPdf ? <canvas className="pdf-canvas" ref={canvasRef} hidden={!canvasVisible} aria-label={`Rendered PDF page ${String(page)}`} /> : (
          <div className="preview-empty" role="status">
            <FileWarning size={28} aria-hidden="true" />
            <strong>{emptyStatus}</strong>
            <p>{unavailableReason ?? (resolvedCompileStatus === "compiling" ? "Your PDF will appear here when compilation finishes." : resolvedCompileStatus === "failed" ? "Check the compile message, correct your source, and try again." : `Compile ${projectTitle === undefined ? "your paper" : `“${projectTitle}”`} to see its PDF here.`)}</p>
          </div>
        )}
        {renderError === null ? null : <p className="pdf-render-error" role="alert">{renderError}</p>}
      </div>
    </section>
  );
}
