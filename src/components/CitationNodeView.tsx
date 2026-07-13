import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";
import { BookOpen } from "lucide-react";

export function CitationNodeView({ node, selected }: NodeViewProps) {
  const label = String(node.attrs.label);
  const keys = String(node.attrs.keys);

  return (
    <NodeViewWrapper as="span" className="citation-node" data-selected={selected} title={`Citation key: ${keys}`}>
      <BookOpen size={12} aria-hidden="true" />
      <span>{label}</span>
    </NodeViewWrapper>
  );
}
