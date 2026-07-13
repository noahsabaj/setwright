import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";
import { FileText } from "lucide-react";

export function FileBoundaryNodeView({ node }: NodeViewProps) {
  return (
    <NodeViewWrapper className="file-boundary" contentEditable={false}>
      <span aria-hidden="true" />
      <strong><FileText size={13} aria-hidden="true" /> {String(node.attrs.path)}</strong>
      <span aria-hidden="true" />
    </NodeViewWrapper>
  );
}
