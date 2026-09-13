import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";
import type { VisualSourceChange } from "../editor/latex-roundtrip";
import type { SourceNavigation } from "../editor/navigation";
import type { PdfArtifact, ProjectFile, ProjectSnapshot, RuntimeReadiness } from "../lib/contracts";
import { deriveProjectMetrics } from "../lib/project-metrics";
import type { ProjectOutlineItem } from "../lib/project-metrics";
import { useKeyboardShortcuts } from "../hooks/useKeyboardShortcuts";
import { useCloseProtection } from "../hooks/useCloseProtection";
import { desktopBridge } from "../lib/bridge";
import { selectedProjectLocation } from "../lib/project-location";
import { events } from "../lib/bindings";
import { WorkspaceSession } from "../lib/workspace-session";
import { useWorkspaceStore } from "../store/workspace-store";
import { AppHeader } from "./AppHeader";
import { CommandPalette } from "./CommandPalette";
import { EncodingConversionPanel } from "./EncodingConversionPanel";
import { ProjectSidebar } from "./ProjectSidebar";
import { ReviewRail } from "./ReviewRail";
import { SplitWorkspace } from "./SplitWorkspace";
import { StatusBar } from "./StatusBar";
import { VisualEditor } from "./VisualEditor";

const SourceEditor = lazy(async () => ({ default: (await import("./SourceEditor")).SourceEditor }));
const compactQuery = "(max-width: 1099px)";
const subscribeCompact = (listener: () => void) => {
  const media = window.matchMedia(compactQuery);
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
};

interface WorkspaceShellProps {
  project: ProjectSnapshot;
  onProjectChange: (project: ProjectSnapshot) => void;
  onProjectClosed?: () => void;
}

export function WorkspaceShell({ project: initialProject, onProjectChange, onProjectClosed }: WorkspaceShellProps) {
  const [session] = useState(() => new WorkspaceSession(initialProject, desktopBridge));
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot);
  const { project, drafts } = state;
  const [selectedFileId, setSelectedFileId] = useState(project.mainFile);
  const [visitedFiles, setVisitedFiles] = useState([project.mainFile]);
  const [navigation, setNavigation] = useState<SourceNavigation>();
  const nextNavigationId = useRef(0);
  const mode = useWorkspaceStore((store) => store.mode);
  const setMode = useWorkspaceStore((store) => store.setMode);
  const theme = useWorkspaceStore((store) => store.theme);
  const outlineOpen = useWorkspaceStore((store) => store.outlineOpen);
  const setOutlineOpen = useWorkspaceStore((store) => store.setOutlineOpen);
  const reviewPanel = useWorkspaceStore((store) => store.reviewPanel);
  const setReviewPanel = useWorkspaceStore((store) => store.setReviewPanel);
  const setSaveState = useWorkspaceStore((store) => store.setSaveState);
  const compileState = useWorkspaceStore((store) => store.compileState);
  const setCompileState = useWorkspaceStore((store) => store.setCompileState);
  const lastEditingMode = useRef<"write" | "source" | "split">("write");
  const [operationError, setOperationError] = useState<string | null>(null);
  const [runtimeReadiness, setRuntimeReadiness] = useState<RuntimeReadiness | null>(null);
  const [pdfArtifact, setPdfArtifact] = useState<PdfArtifact | null>(null);
  const compact = useSyncExternalStore(subscribeCompact, () => window.matchMedia(compactQuery).matches);
  const activeFile = project.files.find((file) => file.id === selectedFileId)
    ?? project.files.find((file) => file.id === project.mainFile);
  const canWrite = activeFile?.kind === "tex" && activeFile.content !== null && activeFile.encoding === "utf8";
  const effectiveMode = mode === "write" && !canWrite ? "source" : mode;
  useKeyboardShortcuts({ canWrite });

  useEffect(() => { onProjectChange(project); }, [onProjectChange, project]);
  useEffect(() => { setSaveState(session.saveState); }, [session, state, setSaveState]);
  useEffect(() => () => session.stopAutosave(), [session]);
  useEffect(() => {
    if (mode !== "preview") lastEditingMode.current = mode;
    if (mode === "write" && !canWrite) setMode("source");
  }, [canWrite, mode, setMode]);
  useEffect(() => { if (compact) setOutlineOpen(false); }, [compact, setOutlineOpen]);

  const reportOperationError = useCallback((cause: unknown) => {
    setOperationError(cause instanceof Error ? cause.message : "The operation could not be completed.");
  }, []);
  useCloseProtection(session, reportOperationError);

  useEffect(() => {
    let cancelled = false;
    void desktopBridge.getRuntimeReadiness().then((readiness) => {
      if (!cancelled) setRuntimeReadiness(readiness);
    }).catch((cause: unknown) => { if (!cancelled) reportOperationError(cause); });
    void desktopBridge.readCompilePdf(project.sessionId).then((artifact) => {
      if (!cancelled) { setPdfArtifact(artifact); setCompileState("success"); }
    }).catch(() => { if (!cancelled) setPdfArtifact(null); });
    return () => { cancelled = true; };
  }, [project.sessionId, reportOperationError, setCompileState]);

  useEffect(() => {
    if (desktopBridge.runtime !== "tauri") return;
    let disposed = false;
    const stops: (() => void)[] = [];
    const remember = (stop: () => void) => { if (disposed) stop(); else stops.push(stop); };
    void import("@tauri-apps/api/event").then(({ listen }) => listen<string>("setwright-close-blocked", (event) => {
      if (!disposed) setOperationError(event.payload);
    })).then(remember).catch(reportOperationError);
    void events.setwrightCompileEvent.listen(({ payload: envelope }) => {
      if (disposed || envelope.sessionId !== project.sessionId) return;
      const event = envelope.event;
      if (event.kind === "queued" || event.kind === "started") setCompileState("compiling");
      else if (event.kind === "finished") {
        if (!event.success) {
          setCompileState("failed");
          setPdfArtifact((current) => current === null ? null : { ...current, stale: true });
        } else {
          void desktopBridge.readCompilePdf(envelope.sessionId).then((artifact) => {
            if (!disposed) { setPdfArtifact(artifact); setCompileState("success"); }
          }).catch(reportOperationError);
        }
      } else if (event.kind === "cancelled") setCompileState("idle");
    }).then(remember).catch(reportOperationError);
    return () => { disposed = true; stops.forEach((stop) => stop()); };
  }, [project.sessionId, reportOperationError, setCompileState]);

  const displayedProject = useMemo(() => ({
    ...project,
    files: project.files.map((file) => drafts[file.id] === undefined ? file : { ...file, content: drafts[file.id]!.text, dirty: true }),
  }), [project, drafts]);
  const metrics = useMemo(() => deriveProjectMetrics(displayedProject), [displayedProject]);

  const selectFile = useCallback((fileId: string) => {
    const file = session.getSnapshot().project.files.find((candidate) => candidate.id === fileId);
    if (file === undefined) return;
    setSelectedFileId(fileId);
    setVisitedFiles((files) => files.includes(fileId) ? files : [...files, fileId]);
    setNavigation(undefined);
    const editingMode = mode === "preview" ? lastEditingMode.current : mode;
    setMode(editingMode === "write" && (file.kind !== "tex" || file.content === null || file.encoding !== "utf8") ? "source" : editingMode);
    if (compact) setOutlineOpen(false);
  }, [compact, mode, session, setMode, setOutlineOpen]);

  const navigate = useCallback((item: ProjectOutlineItem) => {
    selectFile(item.fileId);
    setNavigation({ fileId: item.fileId, sourceOffset: item.sourceOffset, requestId: ++nextNavigationId.current });
  }, [selectFile]);

  const fallbackToSource = useCallback((request: SourceNavigation) => {
    setMode("source");
    setNavigation(request);
  }, [setMode]);

  const openAnother = useCallback(() => {
    void desktopBridge.pickProjectPath().then(async (path) => {
      if (path !== null) {
        const { rootPath, mainFile } = selectedProjectLocation(path);
        await desktopBridge.openProjectWindow(rootPath, mainFile);
      }
    }).catch(reportOperationError);
  }, [reportOperationError]);

  const searchCitations = useCallback(async (query: string) => {
    const current = session.getSnapshot().project;
    const bibliographies = current.files.filter((file) => file.kind === "bib" && file.content !== null);
    const responses = await Promise.all(bibliographies.map((file) => desktopBridge.searchLocalCitations(current.sessionId, file.id, query)));
    return responses.flatMap((response) => response.results).filter((result, index, all) => all.findIndex((other) => other.key === result.key) === index).map((result) => ({
      key: result.key,
      ...(result.title === null ? {} : { title: result.title }),
      ...(result.authors === null ? {} : { authors: result.authors.split(/\s+and\s+/u) }),
      ...(result.year === null ? {} : { year: result.year }),
    }));
  }, [session]);

  const runtimeReady = runtimeReadiness?.runtimeManifestKeysConfigured === true
    && runtimeReadiness.runtimeInstallAvailable && runtimeReadiness.sandboxAttested;
  const compile = useCallback(() => {
    if (!runtimeReady) return;
    setOperationError(null);
    setCompileState("compiling");
    void session.compile().catch((cause: unknown) => { setCompileState("failed"); reportOperationError(cause); });
  }, [reportOperationError, runtimeReady, session, setCompileState]);
  const pdfBytes = useMemo(() => pdfArtifact === null ? undefined : Uint8Array.from(pdfArtifact.bytes), [pdfArtifact]);
  const pdfStale = pdfArtifact !== null && (pdfArtifact.stale || pdfArtifact.revision !== project.revision || Object.keys(drafts).length > 0);
  const previewProps = {
    ...(pdfBytes === undefined ? {} : { pdfBytes }),
    stale: pdfStale,
    projectTitle: project.title,
    runtimeReason: state.paused ? "Recover the retained drafts before compiling." : runtimeReadiness?.reason ?? "Checking compiler availability…",
    compileStatus: runtimeReady ? (compileState === "idle" ? "unavailable" as const : compileState) : "unavailable" as const,
    ...(runtimeReady && !state.paused ? { onCompile: compile } : {}),
  };

  const editor = (
    <div className="editor-stack" inert={state.restoring || state.closing}>
      {[...new Set([...visitedFiles, activeFile?.id])].map((fileId) => {
        const file = displayedProject.files.find((candidate) => candidate.id === fileId);
        if (file === undefined) return null;
        const active = file.id === activeFile?.id;
        const isVisual = file.kind === "tex" && effectiveMode !== "source";
        const textAvailable = file.kind !== "asset" && file.encoding === "utf8" && file.content !== null;
        const change = (source: string, changes?: readonly VisualSourceChange[], basis?: string) => session.edit(file.id, source, changes, basis);
        return (
          <div className="file-workspace" key={`${file.id}:${String(state.epoch)}`} hidden={!active}>
            {textAvailable ? <>
              {file.kind === "tex" ? <div className="file-editor-surface" hidden={!isVisual}>
                <VisualEditor source={file.content ?? ""} fileId={file.id} fileName={file.relativePath} onSourceChange={change}
                  onSearchCitations={searchCitations} active={active && isVisual && effectiveMode !== "preview"}
                  navigation={navigation} onNavigationFallback={fallbackToSource} />
              </div> : null}
              <div className="file-editor-surface" hidden={isVisual}>
                <Suspense fallback={<div className="editor-loading">Opening source…</div>}>
                  <SourceEditor value={file.content ?? ""} fileId={file.id} fileName={file.relativePath}
                    language={file.kind === "tex" || file.kind === "style" ? "latex" : "text"}
                    authorityState={drafts[file.id] !== undefined || file.dirty ? "working" : "canonical"}
                    onChange={change} navigation={navigation} active={active && !isVisual && effectiveMode !== "preview"} />
                </Suspense>
              </div>
            </> : file.kind !== "asset" && file.encoding === "nonUtf8" ? (
              <EncodingConversionPanel project={project} file={file} onConvert={(id, text, hash) => session.convert(id, text, hash)} />
            ) : <FileDetails file={file} />}
          </div>
        );
      })}
    </div>
  );

  const sidebar = <ProjectSidebar project={displayedProject} metrics={metrics} activeFileId={activeFile?.id ?? project.mainFile}
    onSelectFile={selectFile} onNavigate={navigate} onClose={() => setOutlineOpen(false)} />;
  const review = <ReviewRail project={project} canRestore={!session.hasChanges && !state.paused && !state.restoring}
    disabled={state.paused || state.restoring || state.closing}
    disabledReason={state.paused ? "Resolve the draft error before saving a version." : undefined}
    onCreateSnapshot={(name) => session.createSnapshot(name)} onRestoreSnapshot={(id) => session.restore(id)} />;

  return (
    <div className="workspace" data-theme={theme}>
      <AppHeader project={project} activeFileName={activeFile?.relativePath ?? ""} canWrite={canWrite} onOpenAnother={openAnother}
        onClosePaper={onProjectClosed ? () => { void session.close().then(onProjectClosed).catch(reportOperationError); } : undefined} />
      <div className="workspace__body">
        {!compact && outlineOpen ? sidebar : null}
        <main className="workspace__main">
          {state.paused ? <section className="draft-recovery" aria-label="Draft recovery" role="alert">
            <strong>Saving is paused. Your drafts are retained.</strong><p>{state.error}</p>
            <ul>{Object.entries(drafts).map(([fileId, draft]) => {
              const name = project.files.find((file) => file.id === fileId)?.relativePath ?? fileId;
              return <li key={fileId}><button type="button" onClick={() => selectFile(fileId)}>{name}</button>
                <button type="button" onClick={() => { void navigator.clipboard.writeText(draft.text).catch(reportOperationError); }}>Copy draft</button>
                <button type="button" onClick={() => { if (window.confirm(`Discard the retained draft for ${name} and return to its accepted source?`)) session.discardDraft(fileId); }}>Discard draft</button></li>;
            })}</ul>
            <button type="button" disabled={Object.keys(drafts).length > 0} onClick={() => { void session.retrySave().catch(reportOperationError); }}>Retry saving accepted source</button>
          </section> : null}
          {operationError === null ? null : <div className="workspace-error" role="alert"><span>{operationError}</span><button type="button" onClick={() => setOperationError(null)}>Dismiss</button></div>}
          {state.restoring ? <p role="status">Restoring version…</p> : null}
          {state.closing ? <p role="status">Saving and closing paper…</p> : null}
          <SplitWorkspace mode={effectiveMode} editor={editor} preview={previewProps} navigationKey={`${activeFile?.id ?? ""}:${navigation?.requestId ?? ""}`} />
        </main>
        {!compact && reviewPanel !== null ? review : null}
      </div>
      {compact ? <ModalOverlay data-theme={theme} isOpen={outlineOpen || reviewPanel !== null} isDismissable className="workspace-drawer-overlay"
        onOpenChange={(open) => { if (!open) { setOutlineOpen(false); setReviewPanel(null); } }}>
        <Modal className={`workspace-drawer ${outlineOpen ? "workspace-drawer--left" : "workspace-drawer--right"}`}>
          <Dialog aria-label={outlineOpen ? "Project navigation" : "Paper tools"}>{outlineOpen ? sidebar : review}</Dialog>
        </Modal>
      </ModalOverlay> : null}
      <StatusBar project={project} metrics={metrics} runtimeReadiness={runtimeReadiness} stale={pdfStale} />
      <CommandPalette canWrite={canWrite} />
    </div>
  );
}

function FileDetails({ file }: { file: ProjectFile }) {
  return <section className="file-details" aria-label="File details"><span className="eyebrow">{file.kind === "asset" ? "Project asset" : "Source unavailable"}</span>
    <h1>{file.relativePath.split("/").at(-1)}</h1><p>{file.kind === "asset" ? "This asset is part of your paper. Its original file is preserved." : "This file cannot be opened as text. Its original bytes are preserved."}</p>
    <dl><dt>Path</dt><dd>{file.relativePath}</dd><dt>Size</dt><dd>{file.byteLength.toLocaleString()} bytes</dd><dt>Type</dt><dd>{file.kind}</dd></dl>
  </section>;
}
