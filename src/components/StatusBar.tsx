import { Check, CheckCircle2, CircleAlert, CloudOff, FileClock, LoaderCircle, ShieldAlert, ShieldCheck } from "lucide-react";
import type { ProjectSnapshot, RuntimeReadiness } from "../lib/contracts";
import type { ProjectMetrics } from "../lib/project-metrics";
import { desktopBridge } from "../lib/bridge";
import { useWorkspaceStore } from "../store/workspace-store";

interface StatusBarProps {
  project: ProjectSnapshot;
  metrics: ProjectMetrics;
  runtimeReadiness: RuntimeReadiness | null;
}

export function StatusBar({ project, metrics, runtimeReadiness }: StatusBarProps) {
  const saveState = useWorkspaceStore((state) => state.saveState);
  const compileState = useWorkspaceStore((state) => state.compileState);
  const mode = useWorkspaceStore((state) => state.mode);
  const sourceLine = useWorkspaceStore((state) => state.sourceLine);
  const sourceColumn = useWorkspaceStore((state) => state.sourceColumn);

  const SaveIcon = saveState === "saving" ? LoaderCircle : saveState === "conflict" ? CircleAlert : Check;
  const saveLabel = desktopBridge.runtime === "browserDemo" ? "Demo draft · not written" : ({
    saved: "Saved locally",
    saving: "Saving…",
    dirty: "Unsaved changes",
    conflict: "External change conflict",
  }[saveState]);
  const compilePresentation = compileState === "success"
    ? { label: "PDF up to date", Icon: CheckCircle2 }
    : compileState === "compiling"
      ? { label: "Compiling…", Icon: LoaderCircle }
      : compileState === "failed"
        ? { label: "Compile failed", Icon: ShieldAlert }
        : { label: "Not compiled", Icon: FileClock };
  const compatibilityLabel = `${String(project.compatibility.length)} compatibility ${project.compatibility.length === 1 ? "finding" : "findings"}`;
  const wordLabel = metrics.visualWordCount === null
    ? "Word count unavailable"
    : `${metrics.visualWordCount.toLocaleString()} visual ${metrics.visualWordCount === 1 ? "word" : "words"}`;
  const runtimeReady = runtimeReadiness?.runtimeManifestKeysConfigured === true
    && runtimeReadiness.runtimeInstallAvailable
    && runtimeReadiness.sandboxAttested;
  const RuntimeIcon = runtimeReady ? ShieldCheck : ShieldAlert;
  const runtimeLabel = runtimeReady ? project.settings.runtimeId : "Compiler unavailable";
  const runtimeDetail = runtimeReadiness?.reason ?? "Checking managed runtime and sandbox readiness…";

  return (
    <footer className="status-bar" aria-label="Document status">
      <div className="status-bar__group" aria-live="polite">
        <span className={`status-item status-item--${saveState}`}>
          <SaveIcon size={13} className={saveState === "saving" ? "spin" : ""} aria-hidden="true" />
          {saveLabel}
        </span>
        <span className="status-divider" aria-hidden="true" />
        <span className="status-item"><CloudOff size={13} aria-hidden="true" /> Local only</span>
      </div>

      <div className="status-bar__group status-bar__group--center">
        <span className="status-item">
          <compilePresentation.Icon size={13} className={compileState === "compiling" ? "spin" : ""} aria-hidden="true" />
          {compilePresentation.label}
        </span>
        <span className="status-item" title={runtimeReady ? `Pinned and attested runtime profile: ${project.settings.runtimeId}` : runtimeDetail}>
          <RuntimeIcon size={13} aria-hidden="true" />
          {runtimeLabel}
        </span>
      </div>

      <div className="status-bar__group status-bar__group--right">
        <span className="status-item">{compatibilityLabel}</span>
        <span className="status-divider" aria-hidden="true" />
        <span className="status-item" title="Words in visually supported prose; raw LaTeX, headings, equations, and metadata are excluded.">{wordLabel}</span>
        {mode === "source" ? <span className="status-item status-item--mono">Ln {sourceLine}, Col {sourceColumn}</span> : null}
      </div>
    </footer>
  );
}
