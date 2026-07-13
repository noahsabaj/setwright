import type { ChangeEvent } from "react";
import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";
import { FileImage } from "lucide-react";

export function FigureNodeView({ node, selected, updateAttributes }: NodeViewProps) {
  const caption = String(node.attrs.caption);
  const file = String(node.attrs.file);

  const handleCaption = (event: ChangeEvent<HTMLInputElement>) => {
    updateAttributes({ caption: event.target.value });
  };

  return (
    <NodeViewWrapper className="figure-node" data-selected={selected} contentEditable={false}>
      <div className="figure-node__canvas" role="img" aria-label={`Project figure asset ${file}; image bytes are not loaded in the visual editor`}>
        <span className="diagram-node"><FileImage size={18} aria-hidden="true" />Project image asset</span>
        <span className="figure-node__file">{file}</span>
      </div>
      <label className="figure-node__caption">
        <span>Caption</span>
        <input value={caption} onChange={handleCaption} aria-label="Figure caption" />
      </label>
    </NodeViewWrapper>
  );
}
