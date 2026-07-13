import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { VisualSourceChange } from "../editor/latex-roundtrip";
import type { PdfArtifact, ProjectSnapshot, RuntimeReadiness } from "../lib/contracts";
import { deriveProjectMetrics } from "../lib/project-metrics";
import { useKeyboardShortcuts } from "../hooks/useKeyboardShortcuts";
import { desktopBridge } from "../lib/bridge";
import { events } from "../lib/bindings";
import { createDeclaredSourceEdits, createMinimalSourceEdit } from "../lib/source-edits";
import { useWorkspaceStore } from "../store/workspace-store";
import { AppHeader } from "./AppHeader";
import { CommandPalette } from "./CommandPalette";
import { EncodingConversionPanel } from "./EncodingConversionPanel";
import { PreviewPane } from "./PreviewPane";
import { ProjectSidebar } from "./ProjectSidebar";
import { ReviewRail } from "./ReviewRail";
import { SplitWorkspace } from "./SplitWorkspace";
import { StatusBar } from "./StatusBar";
import { VisualEditor } from "./VisualEditor";

const SourceEditor = lazy(async () => {
  const module = await import("./SourceEditor");
  return { default: module.SourceEditor };
});

interface WorkspaceShellProps {
  project: ProjectSnapshot;
  onProjectChange: (project: ProjectSnapshot) => void;
}

export function WorkspaceShell({ project, onProjectChange }: WorkspaceShellProps) {
  const mainFile = project.files.find((file) => file.id === project.mainFile);
  const projectMetrics = useMemo(() => deriveProjectMetrics(project), [project]);
  const mode = useWorkspaceStore((state) => state.mode);
  const theme = useWorkspaceStore((state) => state.theme);
  const outlineOpen = useWorkspaceStore((state) => state.outlineOpen);
  const reviewPanel = useWorkspaceStore((state) => state.reviewPanel);
  const saveState = useWorkspaceStore((state) => state.saveState);
  const setSaveState = useWorkspaceStore((state) => state.setSaveState);
  const compileState = useWorkspaceStore((state) => state.compileState);
  const setCompileState = useWorkspaceStore((state) => state.setCompileState);
  const [editError, setEditError] = useState<string | null>(null);
  const [runtimeReadiness, setRuntimeReadiness] = useState<RuntimeReadiness | null>(null);
  const [pdfArtifact, setPdfArtifact] = useState<PdfArtifact | null>(null);
  const [sourceDraft, setSourceDraft] = useState(mainFile?.content ?? "");
  const [draftActive, setDraftActive] = useState(false);
  const projectRef = useRef(project);
  const draftActiveRef = useRef(false);
  const operationQueueRef = useRef<Promise<void>>(Promise.resolve());
  const draftSequenceRef = useRef(0);
  const saveTimerRef = useRef<number | null>(null);
  const mountedRef = useRef(true);
  useKeyboardShortcuts();

  useEffect(() => {
    projectRef.current = project;
  }, [project]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (saveTimerRef.current !== null) window.clearTimeout(saveTimerRef.current);
    };
  }, []);

  useEffect(() => {
    const protectWorkingDraft = (event: BeforeUnloadEvent) => {
      if (!draftActive && !project.dirty) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", protectWorkingDraft);
    return () => window.removeEventListener("beforeunload", protectWorkingDraft);
  }, [draftActive, project.dirty]);

  const reportFailure = useCallback((cause: unknown) => {
    if (!mountedRef.current) return;
    setSaveState("conflict");
    setEditError(cause instanceof Error ? cause.message : "The source change could not be applied safely.");
  }, [setSaveState]);

  const reportNonSaveFailure = useCallback((cause: unknown) => {
    if (!mountedRef.current) return;
    setEditError(cause instanceof Error ? cause.message : "The operation could not be completed.");
  }, []);

  useEffect(() => {
    if (desktopBridge.runtime !== "tauri") return undefined;
    let disposed = false;
    let stopListening: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) => listen<string>("setwright-close-blocked", (event) => {
        setEditError(event.payload);
      }))
      .then((unlisten) => {
        if (disposed) unlisten();
        else stopListening = unlisten;
      })
      .catch(reportFailure);
    return () => {
      disposed = true;
      stopListening?.();
    };
  }, [reportFailure]);

  useEffect(() => {
    let cancelled = false;
    void desktopBridge.getRuntimeReadiness().then((readiness) => {
      if (!cancelled) setRuntimeReadiness(readiness);
    }).catch((cause: unknown) => {
      if (!cancelled) reportNonSaveFailure(cause);
    });
    void desktopBridge.readCompilePdf(project.sessionId).then((artifact) => {
      if (!cancelled) {
        setPdfArtifact(artifact);
        setCompileState("success");
      }
    }).catch(() => {
      if (!cancelled) setPdfArtifact(null);
    });
    return () => { cancelled = true; };
  }, [project.sessionId, reportNonSaveFailure, setCompileState]);

  useEffect(() => {
    if (desktopBridge.runtime !== "tauri") return undefined;
    let disposed = false;
    let stopListening: (() => void) | undefined;
    void events.setwrightCompileEvent.listen((message) => {
      const envelope = message.payload;
      if (envelope.sessionId !== projectRef.current.sessionId) return;
      const event = envelope.event;
      if (event.kind === "queued" || event.kind === "started") {
        setCompileState("compiling");
      } else if (event.kind === "finished") {
        if (!event.success) {
          setCompileState("failed");
          setPdfArtifact((current) => current === null ? null : { ...current, stale: true });
          return;
        }
        void desktopBridge.readCompilePdf(envelope.sessionId).then((artifact) => {
          if (!disposed) {
            setPdfArtifact(artifact);
            setCompileState("success");
          }
        }).catch(reportNonSaveFailure);
      } else if (event.kind === "cancelled") {
        setCompileState("idle");
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else stopListening = unlisten;
    }).catch(reportNonSaveFailure);
    return () => {
      disposed = true;
      stopListening?.();
    };
  }, [reportNonSaveFailure, setCompileState]);

  const scheduleSave = useCallback((savingSequence: number) => {
    if (saveTimerRef.current !== null) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null;
      operationQueueRef.current = operationQueueRef.current.then(async () => {
        const current = projectRef.current;
        setSaveState("saving");
        await desktopBridge.saveProject(current.sessionId, current.revision);
        if (!mountedRef.current) return;
        const saved: ProjectSnapshot = {
          ...current,
          files: current.files.map((file) => ({ ...file, dirty: false })),
          dirty: false,
        };
        projectRef.current = saved;
        onProjectChange(saved);
        if (draftSequenceRef.current === savingSequence) {
          draftActiveRef.current = false;
          setDraftActive(false);
        }
        setSaveState("saved");
      }).catch(reportFailure);
    }, 750);
  }, [onProjectChange, reportFailure, setSaveState]);

  const handleSourceChange = useCallback((nextSource: string, declaredChanges?: readonly VisualSourceChange[], basisSource?: string) => {
    const operationSequence = draftSequenceRef.current + 1;
    draftSequenceRef.current = operationSequence;
    setSourceDraft(nextSource);
    draftActiveRef.current = true;
    setDraftActive(true);
    setEditError(null);
    setSaveState("dirty");
    if (saveTimerRef.current !== null) window.clearTimeout(saveTimerRef.current);
    operationQueueRef.current = operationQueueRef.current.then(async () => {
      const current = projectRef.current;
      const mainFile = current.files.find((file) => file.id === current.mainFile);
      if (mainFile === undefined || mainFile.content === null) {
        throw new Error("This file is source-only until its encoding is explicitly converted to UTF-8.");
      }
      const declaredEdits = declaredChanges !== undefined && basisSource === mainFile.content
        ? await createDeclaredSourceEdits(mainFile.id, mainFile.content, nextSource, declaredChanges)
        : null;
      const fallbackEdit = declaredEdits === null
        ? await createMinimalSourceEdit(mainFile.id, mainFile.content, nextSource)
        : null;
      const edits = declaredEdits ?? (fallbackEdit === null ? [] : [fallbackEdit]);
      if (edits.length === 0) {
        if (draftSequenceRef.current === operationSequence && mountedRef.current) {
          draftActiveRef.current = false;
          setDraftActive(false);
          setSaveState(current.dirty ? "dirty" : "saved");
        }
        return;
      }
      const result = await desktopBridge.applySourceEdits(current.sessionId, current.revision, edits);
      if (!mountedRef.current) return;
      const updated: ProjectSnapshot = {
        ...current,
        revision: result.revision,
        files: result.files,
        dirty: true,
      };
      projectRef.current = updated;
      onProjectChange(updated);
      setSaveState("dirty");
      scheduleSave(operationSequence);
    }).catch((cause: unknown) => {
      if (draftSequenceRef.current === operationSequence && mountedRef.current) {
        const current = projectRef.current;
        const canonicalMain = current.files.find((file) => file.id === current.mainFile);
        setSourceDraft(canonicalMain?.content ?? "");
        draftActiveRef.current = false;
        setDraftActive(false);
      }
      reportFailure(cause);
    });
  }, [onProjectChange, reportFailure, scheduleSave, setSaveState]);

  const handleRestoreSnapshot = useCallback(async (snapshotId: string) => {
    if (draftActiveRef.current || projectRef.current.dirty || saveState !== "saved") {
      throw new Error("Save or discard the current changes before restoring a version.");
    }
    if (saveTimerRef.current !== null) {
      window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = null;
    }
    const task = operationQueueRef.current.then(async () => {
      const current = projectRef.current;
      if (draftActiveRef.current || current.dirty) {
        throw new Error("The paper changed before the restore could start. Save it first.");
      }
      const restored = await desktopBridge.restoreSnapshot(current.sessionId, snapshotId);
      const restoredMain = restored.files.find((file) => file.id === restored.mainFile);
      draftSequenceRef.current += 1;
      draftActiveRef.current = false;
      setDraftActive(false);
      setSourceDraft(restoredMain?.content ?? "");
      setEditError(null);
      setSaveState("saved");
      projectRef.current = restored;
      onProjectChange(restored);
      return restored;
    });
    operationQueueRef.current = task.then(() => undefined, () => undefined);
    return task;
  }, [onProjectChange, saveState, setSaveState]);

  const handleEncodingConverted = useCallback((converted: ProjectSnapshot) => {
    const sequence = draftSequenceRef.current + 1;
    draftSequenceRef.current = sequence;
    draftActiveRef.current = false;
    setDraftActive(false);
    const convertedMain = converted.files.find((file) => file.id === converted.mainFile);
    setSourceDraft(convertedMain?.content ?? "");
    setEditError(null);
    projectRef.current = converted;
    onProjectChange(converted);
    setSaveState("dirty");
    scheduleSave(sequence);
  }, [onProjectChange, scheduleSave, setSaveState]);

  const handleOpenAnother = useCallback(() => {
    void (async () => {
      try {
        const projectPath = await desktopBridge.pickProjectPath();
        if (projectPath === null) return;
        await desktopBridge.openProjectWindow(projectPath);
      } catch (cause) {
        reportNonSaveFailure(cause);
      }
    })();
  }, [reportNonSaveFailure]);

  const handleSearchCitations = useCallback(async (query: string) => {
    const current = projectRef.current;
    const bibliography = current.files.find((file) => file.kind === "bib" && file.content !== null);
    if (bibliography === undefined) return [];
    const response = await desktopBridge.searchLocalCitations(current.sessionId, bibliography.id, query);
    return response.results.map((result) => ({
      key: result.key,
      ...(result.title === null ? {} : { title: result.title }),
      ...(result.authors === null ? {} : { authors: result.authors.split(/\s+and\s+/u) }),
      ...(result.year === null ? {} : { year: result.year }),
    }));
  }, []);

  const runtimeReady = runtimeReadiness?.runtimeManifestKeysConfigured === true
    && runtimeReadiness.runtimeInstallAvailable
    && runtimeReadiness.sandboxAttested;
  const handleCompile = useCallback(() => {
    const current = projectRef.current;
    if (!runtimeReady) {
      setEditError(runtimeReadiness?.reason ?? "The managed runtime and OS sandbox are not ready.");
      return;
    }
    setEditError(null);
    setCompileState("compiling");
    void desktopBridge.startCompile(current.sessionId, current.revision, current.settings.engine)
      .catch((cause: unknown) => {
        setCompileState("failed");
        reportNonSaveFailure(cause);
      });
  }, [reportNonSaveFailure, runtimeReadiness, runtimeReady, setCompileState]);

  const source = draftActive ? sourceDraft : (mainFile?.content ?? "");
  const sourceAvailable = mainFile !== undefined && mainFile.content !== null;
  const pdfBytes = useMemo(
    () => pdfArtifact === null ? undefined : Uint8Array.from(pdfArtifact.bytes),
    [pdfArtifact],
  );
  const pdfStale = pdfArtifact !== null && (pdfArtifact.stale || pdfArtifact.revision !== project.revision);
  const previewProps = {
    ...(pdfBytes === undefined ? {} : { pdfBytes }),
    stale: pdfStale,
    compileStatus: runtimeReady ? (compileState === "idle" ? "unavailable" : compileState) : "unavailable" as const,
    ...(runtimeReady ? { onCompile: handleCompile } : {}),
  };

  return (
    <div className="workspace" data-theme={theme}>
      <AppHeader project={project} onOpenAnother={handleOpenAnother} />
      <div className="workspace__body">
        {outlineOpen ? <ProjectSidebar project={project} metrics={projectMetrics} /> : null}
        <main className="workspace__main">
          {editError === null ? null : <p className="workspace-error" role="alert">{editError}</p>}
          {mode === "write" ? sourceAvailable ? <VisualEditor source={source} fileId={project.mainFile} fileName={mainFile.relativePath} onSourceChange={handleSourceChange} onSearchCitations={handleSearchCitations} /> : <SourceOnlyNotice /> : null}
          {mode === "source" && sourceAvailable ? (
            <Suspense fallback={<div className="editor-loading">Loading source editor…</div>}>
              <SourceEditor
                value={source}
                fileName={mainFile?.relativePath}
                authorityState={!sourceAvailable ? "unavailable" : draftActive ? "working" : "canonical"}
                {...(sourceAvailable ? { onChange: handleSourceChange } : {})}
              />
            </Suspense>
          ) : null}
          {mode === "source" && !sourceAvailable && mainFile !== undefined ? (
            <EncodingConversionPanel project={project} file={mainFile} onConverted={handleEncodingConverted} />
          ) : null}
          {mode === "preview" ? <PreviewPane {...previewProps} /> : null}
          {mode === "split" ? sourceAvailable ? <SplitWorkspace source={source} fileId={project.mainFile} fileName={mainFile.relativePath} onSourceChange={handleSourceChange} onSearchCitations={handleSearchCitations} preview={previewProps} /> : <SourceOnlyNotice /> : null}
        </main>
        {reviewPanel === null ? null : (
          <ReviewRail
            project={project}
            canRestore={!draftActive && !project.dirty && saveState === "saved"}
            onRestoreSnapshot={handleRestoreSnapshot}
          />
        )}
      </div>
      <StatusBar project={project} metrics={projectMetrics} runtimeReadiness={runtimeReadiness} />
      <CommandPalette />
    </div>
  );
}

function SourceOnlyNotice() {
  return (
    <section className="source-only-notice" aria-label="Source-only file">
      <strong>Visual editing is unavailable for this file.</strong>
      <p>Its original non-UTF-8 bytes remain untouched. Open Source mode to review and explicitly approve a UTF-8 conversion.</p>
    </section>
  );
}
