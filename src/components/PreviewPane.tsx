import { useEffect, useRef, useState } from "react";
import { Check, ChevronLeft, ChevronRight, Download, FileWarning, Maximize2, Minus, Plus, RefreshCw } from "lucide-react";
import { getPdfPreviewCapability } from "../lib/pdf-capability";
import type { PdfDocumentSession } from "../lib/pdf-engine";
import { isPdfOperationCancelled, pdfEngine } from "../lib/pdf-engine";
import { useWorkspaceStore } from "../store/workspace-store";
import { IconButton } from "./IconButton";

export interface PreviewPaneProps {
  pdfBytes?: Uint8Array | undefined;
  stale?: boolean | undefined;
  compileStatus?: "unavailable" | "compiling" | "success" | "failed" | undefined;
  onCompile?: (() => void) | undefined;
  onExport?: (() => void) | undefined;
}

type PdfLoadState =
  | { status: "idle" }
  | { status: "probing"; source: Uint8Array }
  | { status: "loading"; source: Uint8Array; session: PdfDocumentSession }
  | { status: "ready"; source: Uint8Array; session: PdfDocumentSession; pageCount: number }
  | { status: "unsupported"; source: Uint8Array; message: string }
  | { status: "error"; source: Uint8Array; message: string };

const idleState: PdfLoadState = { status: "idle" };

function messageFor(cause: unknown, fallback: string): string {
  return cause instanceof Error && cause.message !== "" ? cause.message : fallback;
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
  const [loadState, setLoadState] = useState<PdfLoadState>(idleState);
  const [renderError, setRenderError] = useState<string | null>(null);
  const generationRef = useRef(0);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);
  const workspaceCompileState = useWorkspaceStore((state) => state.compileState);
  const resolvedCompileStatus = compileStatus ?? (workspaceCompileState === "idle" ? "unavailable" : workspaceCompileState);
  const hasPdf = pdfBytes !== undefined;
  const activeState = pdfBytes !== undefined && "source" in loadState && loadState.source === pdfBytes
    ? loadState
    : pdfBytes === undefined
      ? idleState
      : { status: "probing", source: pdfBytes } satisfies PdfLoadState;

  useEffect(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    if (pdfBytes === undefined) return undefined;

    let session: PdfDocumentSession | null = null;
    const isCurrent = () => generationRef.current === generation;
    const loadPdf = async () => {
      setLoadState({ status: "probing", source: pdfBytes });
      setRenderError(null);
      const capability = await getPdfPreviewCapability();
      if (!isCurrent()) return;
      if (!capability.supported) {
        setLoadState({ status: "unsupported", source: pdfBytes, message: capability.message });
        return;
      }

      session = pdfEngine.createSession(pdfBytes);
      setLoadState({ status: "loading", source: pdfBytes, session });
      const { pageCount } = await session.ready();
      if (!isCurrent()) {
        await session.dispose();
        session = null;
        return;
      }
      const storedPage = useWorkspaceStore.getState().previewPage;
      setPage(Math.min(Math.max(1, storedPage), pageCount));
      setLoadState({ status: "ready", source: pdfBytes, session, pageCount });
    };

    void loadPdf().catch(async (cause: unknown) => {
      const failedSession = session;
      session = null;
      if (failedSession !== null) {
        try {
          await failedSession.dispose();
        } catch {
          // The load error remains the user-facing cause. Disposal failures are
          // covered by the capability probe and session lifecycle tests.
        }
      }
      if (isCurrent() && !isPdfOperationCancelled(cause)) {
        setLoadState({
          status: "error",
          source: pdfBytes,
          message: messageFor(cause, "The PDF could not be loaded."),
        });
      }
    });

    return () => {
      if (generationRef.current === generation) generationRef.current = generation + 1;
      if (session !== null) void session.dispose().catch(() => undefined);
    };
  }, [pdfBytes, setPage]);

  useEffect(() => {
    if (activeState.status !== "ready" || canvasRef.current === null) return undefined;
    const controller = new AbortController();
    const renderPage = async () => {
      await activeState.session.renderPage({
        pageNumber: Math.min(page, activeState.pageCount),
        scale: 1.35 * (zoom / 100),
        canvas: canvasRef.current as HTMLCanvasElement,
        signal: controller.signal,
      });
      if (!controller.signal.aborted) setRenderError(null);
    };
    void renderPage().catch((cause: unknown) => {
      if (!controller.signal.aborted && !isPdfOperationCancelled(cause)) {
        setRenderError(messageFor(cause, "The PDF page could not be rendered."));
      }
    });
    return () => controller.abort();
  }, [activeState, page, zoom]);

  useEffect(() => {
    const scroller = scrollerRef.current;
    if (scroller !== null && activeState.status === "ready") scroller.scrollTop = scrollTop;
  }, [activeState, scrollTop]);

  const emptyStatus = resolvedCompileStatus === "compiling"
    ? "Compiling project snapshot…"
    : resolvedCompileStatus === "failed"
      ? "Compile failed · no new PDF published"
      : resolvedCompileStatus === "success"
        ? "Compiled PDF bytes unavailable"
        : "No PDF compiled";
  const pageCount = activeState.status === "ready" ? activeState.pageCount : 0;
  const previewReady = activeState.status === "ready";
  const previewStatus = !hasPdf
    ? emptyStatus
    : activeState.status === "ready"
      ? stale ? "Last successful PDF · stale" : "PDF ready"
      : activeState.status === "unsupported" ? "PDF preview unsupported"
        : activeState.status === "error" ? "PDF preview failed"
          : activeState.status === "loading" ? "Loading PDF…" : "Checking PDF support…";

  return (
    <section className="preview-pane" aria-label="Compiled PDF preview">
      <div className="preview-toolbar">
        <div className="preview-toolbar__group">
          <span className={`preview-status${previewReady ? "" : " preview-status--unavailable"}`}>
            {previewReady ? <Check size={13} aria-hidden="true" /> : <FileWarning size={13} aria-hidden="true" />}
            {previewStatus}
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
          ) : <span className="preview-page-unavailable">{hasPdf ? activeState.status === "unsupported" ? "Unsupported" : activeState.status === "error" ? "Unavailable" : "Loading pages…" : "No pages"}</span>}
        </div>
        <div className="preview-toolbar__group">
          <IconButton label="Zoom out" disabled={!previewReady} onPress={() => setZoom(Math.max(50, zoom - 10))}><Minus size={14} /></IconButton>
          <span className="preview-zoom">{previewReady ? `${String(zoom)}%` : "—"}</span>
          <IconButton label="Zoom in" disabled={!previewReady} onPress={() => setZoom(Math.min(160, zoom + 10))}><Plus size={14} /></IconButton>
          <IconButton label="Fit page" disabled={!previewReady} onPress={() => setZoom(82)}><Maximize2 size={14} /></IconButton>
          <IconButton label={onExport === undefined ? "Export PDF unavailable" : "Export PDF"} disabled={!hasPdf || onExport === undefined} onPress={() => onExport?.()}><Download size={14} /></IconButton>
        </div>
      </div>

      <div
        className="preview-pane__scroller"
        ref={scrollerRef}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        {!hasPdf ? (
          <div className="preview-empty" role="status">
            <FileWarning size={28} aria-hidden="true" />
            <strong>{emptyStatus}</strong>
            <p>No PDF artifact has been published for this project revision.</p>
          </div>
        ) : activeState.status === "unsupported" || activeState.status === "error" ? (
          <div className="preview-empty" role="alert">
            <FileWarning size={28} aria-hidden="true" />
            <strong>{activeState.status === "unsupported" ? "PDF preview unavailable" : "PDF could not be loaded"}</strong>
            <p>{activeState.message}</p>
          </div>
        ) : (
          <canvas className="pdf-canvas" ref={canvasRef} aria-label={`Rendered PDF page ${String(page)}`} data-testid="pdf-preview-canvas" />
        )}
        {renderError === null ? null : <p className="pdf-render-error" role="alert">{renderError}</p>}
      </div>
    </section>
  );
}
