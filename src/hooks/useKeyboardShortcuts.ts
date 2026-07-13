import { useEffect } from "react";
import { useWorkspaceStore } from "../store/workspace-store";

function isTextInput(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

export function useKeyboardShortcuts(): void {
  const setCommandPaletteOpen = useWorkspaceStore((state) => state.setCommandPaletteOpen);
  const setMode = useWorkspaceStore((state) => state.setMode);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setCommandPaletteOpen(true);
        return;
      }

      if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey) {
        const mode = ({ "1": "write", "2": "source", "3": "preview", "4": "split" } as const)[event.key as "1" | "2" | "3" | "4"];
        if (mode !== undefined) {
          event.preventDefault();
          setMode(mode);
          return;
        }
      }

      if (event.key === "/" && !isTextInput(event.target)) {
        event.preventDefault();
        setCommandPaletteOpen(true);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [setCommandPaletteOpen, setMode]);
}
