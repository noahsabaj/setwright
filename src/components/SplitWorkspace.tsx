import { useState } from "react";
import type { KeyboardEvent } from "react";
import type { VisualSourceChange } from "../editor/latex-roundtrip";
import type { CitationSearchResult } from "./InsertDialog";
import { PreviewPane } from "./PreviewPane";
import type { PreviewPaneProps } from "./PreviewPane";
import { VisualEditor } from "./VisualEditor";

interface SplitWorkspaceProps {
  source: string;
  fileId: string;
  fileName?: string | undefined;
  onSourceChange: (nextSource: string, changes?: readonly VisualSourceChange[], basisSource?: string) => void;
  onSearchCitations?: ((query: string) => Promise<CitationSearchResult[]>) | undefined;
  preview: PreviewPaneProps;
}

export function SplitWorkspace({ source, fileId, fileName, onSourceChange, onSearchCitations, preview }: SplitWorkspaceProps) {
  const [editorWidth, setEditorWidth] = useState(54);

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      setEditorWidth((current) => Math.max(35, current - 2));
    }
    if (event.key === "ArrowRight") {
      event.preventDefault();
      setEditorWidth((current) => Math.min(70, current + 2));
    }
    if (event.key === "Home") setEditorWidth(35);
    if (event.key === "End") setEditorWidth(70);
  };

  return (
    <div className="split-workspace">
      <div className="split-workspace__editor" style={{ width: `${String(editorWidth)}%` }}>
        <VisualEditor source={source} fileId={fileId} fileName={fileName} onSourceChange={onSourceChange} onSearchCitations={onSearchCitations} />
      </div>
      <button
        type="button"
        className="splitter"
        role="separator"
        aria-label="Resize editor and PDF preview"
        aria-orientation="vertical"
        aria-valuemin={35}
        aria-valuemax={70}
        aria-valuenow={editorWidth}
        onKeyDown={handleKeyDown}
      ><span /></button>
      <div className="split-workspace__preview"><PreviewPane {...preview} /></div>
    </div>
  );
}
