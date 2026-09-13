import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Dialog, Heading, Modal, ModalOverlay } from "react-aria-components";
import { Download, X } from "lucide-react";
import { appUpdates, type AppUpdateStatus } from "../lib/app-updates";
import { desktopBridge } from "../lib/bridge";
import { useWorkspaceStore } from "../store/workspace-store";
import changelog from "../../CHANGELOG.md?raw";
import packageInfo from "../../package.json";

const initial: AppUpdateStatus = { currentVersion: packageInfo.version, phase: "idle", version: null, notes: null, automatic: false, downloadedBytes: 0, totalBytes: null, error: null };

export function UpdateCenter({ paperOpen }: { paperOpen: boolean }) {
  const [state, setState] = useState(initial);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const theme = useWorkspaceStore((store) => store.theme);
  const native = desktopBridge.runtime === "tauri";
  const automaticAttempt = useRef<string | null>(null);
  const busy = ["checking", "downloading", "installing"].includes(state.phase);
  const run = useCallback(async (action: () => Promise<AppUpdateStatus | void>) => {
    setError(null);
    try { const next = await action(); if (next) setState(next); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }, []);

  useEffect(() => {
    if (!native) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event").then(async ({ listen }) => {
      const stop = await listen<AppUpdateStatus>("setwright-update-status", ({ payload }) => { if (!disposed) setState(payload); });
      if (disposed) { stop(); return; }
      unlisten = stop;
      const current = await appUpdates.status();
      if (!disposed) { setState(current); if (current.phase === "idle") void run(appUpdates.check); }
    }).catch((cause: unknown) => { if (!disposed) setError(String(cause)); });
    const interval = window.setInterval(() => { void run(appUpdates.check); }, 6 * 60 * 60 * 1000);
    return () => { disposed = true; unlisten?.(); window.clearInterval(interval); };
  }, [native, run]);

  useEffect(() => {
    if (!native || !state.automatic || state.version === null) return;
    const action = state.phase === "available" ? "download" : state.phase === "ready" && !paperOpen ? "install" : null;
    if (action === null) return;
    const attempt = `${action}:${state.version}`;
    if (automaticAttempt.current === attempt) return;
    automaticAttempt.current = attempt;
    void run(action === "download" ? appUpdates.download : appUpdates.install);
  }, [native, paperOpen, run, state.automatic, state.phase, state.version]);

  const available = state.version !== null;
  const label = state.phase === "downloading" ? "Downloading update…" : state.phase === "ready" ? "Update ready" : available ? "Update available" : "Updates & changelog";
  return <>
    <button type="button" data-theme={theme} className={`update-launcher${available ? " update-launcher--available" : ""}`} onClick={() => setOpen(true)} aria-label={label}>
      <Download size={14} aria-hidden="true" /><span>{label}</span>
    </button>
    <ModalOverlay className="modal-layer" data-theme={theme} isOpen={open || state.phase === "installing"} isDismissable={state.phase !== "installing"} onOpenChange={setOpen}>
      <Modal className="update-dialog"><Dialog aria-label="Setwright updates and changelog">
        <header><Heading slot="title">Setwright updates</Heading><Button className="icon-button" aria-label="Close updates" isDisabled={state.phase === "installing"} onPress={() => setOpen(false)}><X size={18} /></Button></header>
        <p>Installed version <strong>{state.currentVersion}</strong></p>
        {!native ? <p>Install the desktop app to check for updates.</p> : <>
          <p role="status">{state.phase === "current" ? "You’re up to date." : state.phase === "checking" ? "Checking for updates…" : state.phase === "installing" ? "Installing update. Setwright will restart…" : available ? `Version ${state.version} is available.` : "Check for the latest Setwright release."}</p>
          {state.phase === "downloading" ? <><progress aria-label="Update download progress" value={state.totalBytes ? state.downloadedBytes : undefined} max={state.totalBytes ?? 1} /><p>{(state.downloadedBytes / 1048576).toFixed(1)} MB downloaded</p></> : null}
          <div className="update-actions">
            <button type="button" className="secondary-button" disabled={busy} onClick={() => void run(appUpdates.check)}>Check for updates</button>
            {available && state.phase !== "ready" ? <button type="button" className="primary-button" disabled={busy} onClick={() => void run(appUpdates.download)}>Download update</button> : null}
            {state.phase === "ready" ? <button type="button" className="primary-button" disabled={paperOpen} onClick={() => void run(appUpdates.install)}>Install update and restart</button> : null}
          </div>
          <label className="update-option"><input type="checkbox" checked={state.automatic} disabled={busy} onChange={(event) => { automaticAttempt.current = null; void run(() => appUpdates.setAutomatic(event.target.checked)); }} />Automatically download and install updates</label>
          <p className="update-help">Updates install only when every paper is closed. Use “Save and close paper” in the project menu; any unsaved or rejected edits must be resolved first.</p>
          {error || state.error ? <p className="inline-error" role="alert">{error ?? state.error}</p> : null}
        </>}
        {state.notes ? <section><h3>What’s new in {state.version}</h3><ReleaseNotes text={state.notes} /></section> : null}
        <section><h3>Changelog</h3><ReleaseNotes text={changelog} /></section>
      </Dialog></Modal>
    </ModalOverlay>
  </>;
}

// Release notes are untrusted text. Render a small readable Markdown subset,
// never remote HTML, embedded images, scripts, or tracking resources.
function ReleaseNotes({ text }: { text: string }) {
  return <div className="release-notes">{text.split(/\r?\n/).map((line, index) => {
    if (line.startsWith("# ")) return null;
    if (line.startsWith("## ")) return <h4 key={index}>{line.slice(3)}</h4>;
    if (line.startsWith("- ")) return <p className="release-notes__item" key={index}>• {line.slice(2)}</p>;
    return line.trim() ? <p key={index}>{line}</p> : null;
  })}</div>;
}
