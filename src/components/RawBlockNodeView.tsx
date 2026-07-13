import type { ChangeEvent } from "react";
import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";
import { Braces, LockKeyhole } from "lucide-react";

export function RawBlockNodeView({ node, selected, updateAttributes }: NodeViewProps) {
  const source = String(node.attrs.source);
  const environment = String(node.attrs.environment);

  const handleChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    updateAttributes({ source: event.target.value });
  };

  return (
    <NodeViewWrapper className="raw-block" data-selected={selected} contentEditable={false}>
      <header className="raw-block__header">
        <span><Braces size={14} aria-hidden="true" /> Preserved source · {environment}</span>
        <span title="Setwright will preserve this source exactly"><LockKeyhole size={12} aria-hidden="true" /> byte-safe</span>
      </header>
      <textarea value={source} onChange={handleChange} aria-label={`Raw ${environment} source`} spellCheck={false} />
      <p>Unsupported visual content remains exact LaTeX. Editing here changes only this block.</p>
    </NodeViewWrapper>
  );
}
