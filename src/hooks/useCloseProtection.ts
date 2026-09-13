import { useEffect } from "react";
import { desktopBridge } from "../lib/bridge";
import type { WorkspaceSession } from "../lib/workspace-session";

/** Includes draft edits and submitted conversions not yet acknowledged by Rust. */
export function useCloseProtection(session: WorkspaceSession, onBlocked: (cause: unknown) => void) {
  useEffect(() => {
    const mustKeepOpen = () => {
      const state = session.getSnapshot();
      return session.hasChanges || state.paused || state.restoring;
    };
    const protectDrafts = (event: BeforeUnloadEvent) => {
      if (!mustKeepOpen()) return;
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", protectDrafts);
    let disposed = false;
    let stop: (() => void) | undefined;
    if (desktopBridge.runtime === "tauri") {
      void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().onCloseRequested((event) => {
        if (!mustKeepOpen()) return;
        event.preventDefault();
        onBlocked(new Error("This paper has unsaved changes or recovery in progress. Save or recover all files before closing."));
      })).then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      }).catch(onBlocked);
    }
    return () => {
      disposed = true;
      stop?.();
      window.removeEventListener("beforeunload", protectDrafts);
    };
  }, [onBlocked, session]);
}
