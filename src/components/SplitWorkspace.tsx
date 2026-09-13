import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import type { WorkspaceMode } from "../lib/contracts";
import { useWorkspaceStore } from "../store/workspace-store";
import { PreviewPane } from "./PreviewPane";
import type { PreviewPaneProps } from "./PreviewPane";

interface SplitWorkspaceProps {
  editor: ReactNode;
  preview: PreviewPaneProps;
  mode?: WorkspaceMode | undefined;
  navigationKey?: string | undefined;
}

export function SplitWorkspace({ editor, preview, mode = "split", navigationKey }: SplitWorkspaceProps) {
  const editorWidth = useWorkspaceStore((state) => state.splitRatio);
  const setEditorWidth = useWorkspaceStore((state) => state.setSplitRatio);
  const [compact, setCompact] = useState(false);
  const [paneSelection, setPaneSelection] = useState<{ navigationKey: string | undefined; pane: "editor" | "preview" }>({ navigationKey, pane: "editor" });
  const [resizing, setResizing] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{ pointerId: number; left: number; width: number } | null>(null);
  const editorTabRef = useRef<HTMLButtonElement>(null);
  const previewTabRef = useRef<HTMLButtonElement>(null);
  const id = useId();
  const activePane = paneSelection.navigationKey === navigationKey ? paneSelection.pane : "editor";
  const setActivePane = (pane: "editor" | "preview") => setPaneSelection({ navigationKey, pane });
  const isSplit = mode === "split";
  const editorHidden = mode === "preview" || (isSplit && compact && activePane !== "editor");
  const previewHidden = mode === "write" || mode === "source" || (isSplit && compact && activePane !== "preview");

  useEffect(() => {
    const container = containerRef.current;
    if (container === null) return undefined;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width;
      if (width !== undefined && width > 0) setCompact(width < 900);
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const next = event.key === "ArrowLeft" ? editorWidth - 2
      : event.key === "ArrowRight" ? editorWidth + 2
        : event.key === "Home" ? 35
          : event.key === "End" ? 70 : null;
    if (next === null) return;
    event.preventDefault();
    setEditorWidth(Math.min(70, Math.max(35, next)));
  };

  const handlePointerDown = (event: PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0 || compact || !isSplit) return;
    const bounds = containerRef.current?.getBoundingClientRect();
    if (bounds === undefined || bounds.width <= 0) return;
    event.preventDefault();
    event.currentTarget.focus();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { pointerId: event.pointerId, left: bounds.left, width: bounds.width };
    setResizing(true);
  };

  const handlePointerMove = (event: PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    setEditorWidth(Math.min(70, Math.max(35, ((event.clientX - drag.left) / drag.width) * 100)));
  };

  const endResize = () => {
    dragRef.current = null;
    setResizing(false);
  };

  const handleTabKey = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const pane = event.key === "Home" ? "editor" : event.key === "End" ? "preview" : activePane === "editor" ? "preview" : "editor";
    setActivePane(pane);
    (pane === "editor" ? editorTabRef : previewTabRef).current?.focus();
  };

  return (
    <div className={`split-workspace${compact || !isSplit ? " split-workspace--compact" : ""}${resizing ? " split-workspace--resizing" : ""}`} ref={containerRef}>
      {compact && isSplit ? (
        <div className="split-workspace__tabs" role="tablist" aria-label="Split pane">
          <button type="button" role="tab" id={`${id}-editor-tab`} aria-controls={`${id}-editor`} aria-selected={activePane === "editor"} tabIndex={activePane === "editor" ? 0 : -1} onKeyDown={handleTabKey} onClick={() => setActivePane("editor")} ref={editorTabRef}>Editor</button>
          <button type="button" role="tab" id={`${id}-preview-tab`} aria-controls={`${id}-preview`} aria-selected={activePane === "preview"} tabIndex={activePane === "preview" ? 0 : -1} onKeyDown={handleTabKey} onClick={() => setActivePane("preview")} ref={previewTabRef}>PDF</button>
        </div>
      ) : null}
      <div className="split-workspace__editor" id={`${id}-editor`} role={compact && isSplit ? "tabpanel" : undefined} aria-labelledby={compact && isSplit ? `${id}-editor-tab` : undefined} hidden={editorHidden} style={{ width: compact || !isSplit ? "100%" : `${String(editorWidth)}%` }}>
        {editor}
      </div>
      {!compact && isSplit ? (
        <button
          type="button"
          className="splitter"
          role="separator"
          aria-label="Resize editor and PDF preview"
          aria-controls={`${id}-editor ${id}-preview`}
          aria-orientation="vertical"
          aria-valuemin={35}
          aria-valuemax={70}
          aria-valuenow={Math.round(editorWidth)}
          aria-valuetext={`Editor ${String(Math.round(editorWidth))}%, PDF ${String(Math.round(100 - editorWidth))}%`}
          onKeyDown={handleKeyDown}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={endResize}
          onPointerCancel={endResize}
          onLostPointerCapture={endResize}
        ><span /></button>
      ) : null}
      <div className="split-workspace__preview" id={`${id}-preview`} role={compact && isSplit ? "tabpanel" : undefined} aria-labelledby={compact && isSplit ? `${id}-preview-tab` : undefined} hidden={previewHidden}><PreviewPane {...preview} /></div>
    </div>
  );
}
