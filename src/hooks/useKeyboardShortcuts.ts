import { useEffect } from "react";
import { useWorkspaceStore } from "../store/workspace-store";
import { hasPrimaryModifier } from "../lib/keyboard";

function isTextInput(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

export function useKeyboardShortcuts({ canWrite = true }: { canWrite?: boolean } = {}): void {
  const setCommandPaletteOpen = useWorkspaceStore((state) => state.setCommandPaletteOpen);
  const setMode = useWorkspaceStore((state) => state.setMode);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      if (hasPrimaryModifier(event) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setCommandPaletteOpen(true);
        return;
      }

      if (useWorkspaceStore.getState().commandPaletteOpen || (event.target instanceof Element && event.target.closest('[role="dialog"]'))) return;

      if (hasPrimaryModifier(event) && !event.altKey && !event.shiftKey) {
        const mode = ({ "1": "write", "2": "source", "3": "preview", "4": "split" } as const)[event.key as "1" | "2" | "3" | "4"];
        if (mode !== undefined) {
          event.preventDefault();
          if (mode !== "write" || canWrite) setMode(mode);
          return;
        }
      }

      if (event.key === "/" && !event.metaKey && !event.ctrlKey && !event.altKey && !isTextInput(event.target)) {
        event.preventDefault();
        setCommandPaletteOpen(true);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [setCommandPaletteOpen, setMode, canWrite]);
}
