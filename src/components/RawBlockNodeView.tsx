import type { ChangeEvent } from "react";
import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";
import { Braces, ChevronRight } from "lucide-react";

export function RawBlockNodeView({ node, selected, updateAttributes }: NodeViewProps) {
  const source = String(node.attrs.source);
  const environment = String(node.attrs.environment);

  const handleChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    updateAttributes({ source: event.target.value });
  };

  return (
    <NodeViewWrapper as="details" className="raw-block" data-selected={selected} contentEditable={false}>
      <summary className="raw-block__header">
        <span><Braces size={14} aria-hidden="true" /> {environment === "source" ? "Preserved LaTeX" : `Preserved source · ${environment}`}</span>
        <span className="raw-block__disclosure">Edit source <ChevronRight size={13} aria-hidden="true" /></span>
      </summary>
      <div className="raw-block__body">
        <textarea value={source} onChange={handleChange} aria-label={`Raw ${environment} source`} spellCheck={false} />
        <p>This LaTeX is preserved as written. Edits here change only this block.</p>
      </div>
    </NodeViewWrapper>
  );
}
