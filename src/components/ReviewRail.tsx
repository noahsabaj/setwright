import { useEffect, useState } from "react";
import { ArchiveRestore, History, MessageSquareText, Plus, X } from "lucide-react";
import type { HistoryEntry, ProjectSnapshot } from "../lib/contracts";
import { desktopBridge } from "../lib/bridge";
import { useWorkspaceStore } from "../store/workspace-store";

interface ReviewRailProps {
  project: ProjectSnapshot;
  canRestore: boolean;
  onRestoreSnapshot: (snapshotId: string) => Promise<ProjectSnapshot>;
}

export function ReviewRail({ project, canRestore, onRestoreSnapshot }: ReviewRailProps) {
  const reviewPanel = useWorkspaceStore((state) => state.reviewPanel);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const theme = useWorkspaceStore((state) => state.theme);
  const setTheme = useWorkspaceStore((state) => state.setTheme);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (reviewPanel !== "history") return undefined;
    let cancelled = false;
    void desktopBridge.listHistory(project.sessionId).then((entries) => {
      if (!cancelled) setHistory(entries);
    }).catch((cause: unknown) => {
      if (!cancelled) setError(cause instanceof Error ? cause.message : "History could not be read.");
    }).finally(() => {
      if (!cancelled) setBusy(false);
    });
    return () => { cancelled = true; };
  }, [project.sessionId, reviewPanel]);

  if (reviewPanel === null) return null;

  const createVersion = async () => {
    setBusy(true);
    setError(null);
    try {
      const entry = await desktopBridge.createSnapshot(project.sessionId, "Named version");
      setHistory((current) => [entry, ...current]);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The version could not be created.");
    } finally {
      setBusy(false);
    }
  };

  const restoreVersion = async (entry: HistoryEntry) => {
    setBusy(true);
    setError(null);
    try {
      await onRestoreSnapshot(entry.id);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The version could not be restored.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <aside className="review-rail" aria-label="Review and history">
      <header className="review-rail__header">
        <div>
          {reviewPanel === "comments" ? <MessageSquareText size={17} /> : <History size={17} />}
          <strong>{reviewPanel === "comments" ? "Comments" : "Version history"}</strong>
        </div>
        <button className="icon-button" type="button" aria-label="Close review panel" onClick={() => setReviewPanel(null)}><X size={17} /></button>
      </header>

      <div className="review-tabs" role="tablist" aria-label="Review panel">
        <button type="button" role="tab" aria-selected={reviewPanel === "comments"} onClick={() => setReviewPanel("comments")}>Comments</button>
        <button type="button" role="tab" aria-selected={reviewPanel === "history"} onClick={() => setReviewPanel("history")}>History</button>
      </div>

      {error === null ? null : <p className="rail-note" role="alert">{error}</p>}

      {reviewPanel === "comments" ? (
        <div className="review-rail__body" role="tabpanel">
          <button className="new-comment-button" type="button" disabled><Plus size={15} /> Add comment to a source selection</button>
          <div className="suggestion-summary"><MessageSquareText size={16} /><span><strong>No review bundle is open</strong><small>Comments are portable review data and never enter paper source.</small></span></div>
          <p className="rail-note">The exact-anchor review engine is present in the Rust core; this build does not fabricate reviewer threads.</p>
        </div>
      ) : null}

      {reviewPanel === "history" ? (
        <div className="review-rail__body" role="tabpanel" aria-busy={busy}>
          <button className="new-comment-button" type="button" onClick={() => void createVersion()} disabled={busy}><Plus size={15} /> Name current version</button>
          {busy && history.length === 0 ? <p className="rail-note" aria-live="polite">Loading local history…</p> : null}
          <ol className="history-list">
            {history.map((entry) => (
              <li key={entry.id}>
                <span className={`history-dot${entry.kind === "named" || entry.kind === "preRestore" ? " history-dot--named" : ""}`} />
                <div><strong>{entry.label}</strong><time>{new Date(entry.createdAt).toLocaleString()} · revision {entry.revision} · {entry.changedFiles} files</time></div>
                <button
                  type="button"
                  aria-label={`Restore ${entry.label}`}
                  title={canRestore ? undefined : "Save or discard current changes before restoring."}
                  onClick={() => void restoreVersion(entry)}
                  disabled={busy || !canRestore}
                >
                  <ArchiveRestore size={14} />
                </button>
              </li>
            ))}
          </ol>
          {!busy && history.length === 0 ? <p className="rail-note">No snapshots yet. Automatic snapshots are recorded after canonical saves.</p> : null}
          <div className="rail-settings">
            <span className="eyebrow">Appearance</span>
            <div className="theme-switcher" role="group" aria-label="Color theme">
              <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>Light</button>
              <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>Dark</button>
              <button type="button" aria-pressed={theme === "contrast"} onClick={() => setTheme("contrast")}>Contrast</button>
            </div>
          </div>
        </div>
      ) : null}
    </aside>
  );
}
