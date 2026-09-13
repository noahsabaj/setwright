import { useEffect, useRef, useState } from "react";
import { ArchiveRestore, History, Palette, Plus, X } from "lucide-react";
import type { HistoryEntry, ProjectSnapshot } from "../lib/contracts";
import { desktopBridge } from "../lib/bridge";
import { useWorkspaceStore } from "../store/workspace-store";

interface ReviewRailProps {
  project: ProjectSnapshot;
  canRestore: boolean;
  onRestoreSnapshot: (snapshotId: string) => Promise<ProjectSnapshot>;
  onCreateSnapshot: (name: string) => Promise<HistoryEntry>;
  disabled?: boolean | undefined;
  disabledReason?: string | undefined;
}

export function ReviewRail({ project, canRestore, onRestoreSnapshot, onCreateSnapshot, disabled = false, disabledReason }: ReviewRailProps) {
  const reviewPanel = useWorkspaceStore((state) => state.reviewPanel);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const theme = useWorkspaceStore((state) => state.theme);
  const setTheme = useWorkspaceStore((state) => state.setTheme);
  const [history, setHistory] = useState<HistoryEntry[]>([]);
  const [name, setName] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<"creating" | "restoring" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const historyRequest = useRef(0);
  const actionsBlocked = loading || busy !== null || disabled;

  useEffect(() => {
    if (reviewPanel !== "history") return undefined;
    let cancelled = false;
    const request = ++historyRequest.current;
    const loadHistory = async () => {
      setLoading(true);
      setError(null);
      try {
        const entries = await desktopBridge.listHistory(project.sessionId);
        if (!cancelled && request === historyRequest.current) setHistory(entries);
      } catch (cause) {
        if (!cancelled && request === historyRequest.current) setError(cause instanceof Error ? cause.message : "History could not be read.");
      } finally {
        if (!cancelled && request === historyRequest.current) setLoading(false);
      }
    };
    void loadHistory();
    return () => { cancelled = true; };
  }, [project.sessionId, reviewPanel]);

  if (reviewPanel === null) return null;

  const createVersion = async () => {
    const label = name.trim();
    if (label === "" || actionsBlocked) return;
    setBusy("creating");
    setError(null);
    try {
      const entry = await onCreateSnapshot(label);
      historyRequest.current += 1;
      setLoading(false);
      setHistory((current) => [entry, ...current.filter((existing) => existing.id !== entry.id)]);
      setName("");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The version could not be created.");
    } finally {
      setBusy(null);
    }
  };

  const restoreVersion = async (entry: HistoryEntry) => {
    if (actionsBlocked || !canRestore) return;
    setBusy("restoring");
    setError(null);
    try {
      await onRestoreSnapshot(entry.id);
      const entries = await desktopBridge.listHistory(project.sessionId);
      historyRequest.current += 1;
      setLoading(false);
      setHistory(entries);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The version could not be restored.");
    } finally {
      setBusy(null);
    }
  };

  return (
    <aside className="review-rail" aria-label={reviewPanel === "appearance" ? "Appearance" : "Version history"}>
      <header className="review-rail__header">
        <div>
          {reviewPanel === "appearance" ? <Palette size={17} aria-hidden="true" /> : <History size={17} aria-hidden="true" />}
          <strong>{reviewPanel === "appearance" ? "Appearance" : "Version history"}</strong>
        </div>
        <button className="icon-button" type="button" aria-label="Close panel" onClick={() => setReviewPanel(null)}><X size={17} /></button>
      </header>

      {reviewPanel === "history" ? (
        <div className="review-rail__body" aria-busy={loading || busy !== null}>
          <form className="version-form" onSubmit={(event) => { event.preventDefault(); void createVersion(); }}>
            <label htmlFor="version-name">Name current version</label>
            <input id="version-name" value={name} onChange={(event) => setName(event.target.value)} placeholder="e.g. Ready for review" disabled={actionsBlocked} required />
            <button className="new-comment-button" type="submit" disabled={actionsBlocked || name.trim() === ""}>
              <Plus size={15} aria-hidden="true" /> {busy === "creating" ? "Saving version…" : "Save named version"}
            </button>
          </form>
          {disabled && disabledReason !== undefined ? <p className="rail-note">{disabledReason}</p> : null}
          {error === null ? null : <p className="rail-note" role="alert">{error}</p>}
          {loading ? <p className="rail-note" role="status">Loading local history…</p> : null}
          {busy === "restoring" ? <p className="rail-note" role="status">Restoring version…</p> : null}
          <ol className="history-list">
            {history.map((entry) => (
              <li key={entry.id}>
                <span className={`history-dot${entry.kind === "named" || entry.kind === "preRestore" ? " history-dot--named" : ""}`} />
                <div><strong>{entry.label}</strong><time dateTime={entry.createdAt}>{new Date(entry.createdAt).toLocaleString()} · {entry.changedFiles} {entry.changedFiles === 1 ? "file" : "files"}</time></div>
                <button
                  type="button"
                  aria-label={`Restore ${entry.label}`}
                  title={canRestore ? undefined : "Save or discard current changes before restoring."}
                  onClick={() => void restoreVersion(entry)}
                  disabled={actionsBlocked || !canRestore}
                >
                  <ArchiveRestore size={14} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ol>
          {!loading && busy === null && error === null && history.length === 0 ? <p className="rail-note">No versions yet. Versions are also saved automatically when you save your paper.</p> : null}
        </div>
      ) : (
        <div className="review-rail__body">
          <div className="rail-settings">
            <span className="eyebrow">Color theme</span>
            <div className="theme-switcher" role="group" aria-label="Color theme">
              <button type="button" aria-pressed={theme === "light"} onClick={() => setTheme("light")}>Light</button>
              <button type="button" aria-pressed={theme === "dark"} onClick={() => setTheme("dark")}>Dark</button>
              <button type="button" aria-pressed={theme === "contrast"} onClick={() => setTheme("contrast")}>High contrast</button>
            </div>
            <p className="rail-note">Applies to the workspace. Your paper and exported PDF keep their own formatting.</p>
          </div>
        </div>
      )}
    </aside>
  );
}
