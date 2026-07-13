import type { NodeViewProps } from "@tiptap/react";
import { NodeViewContent, NodeViewWrapper } from "@tiptap/react";
import { Code2 } from "lucide-react";

export function ListingBlockNodeView({ node, selected }: NodeViewProps) {
  const language = String(node.attrs.language);
  const caption = String(node.attrs.caption);
  const label = String(node.attrs.label);
  const details = [language === "" ? "No language" : language, caption, label].filter(Boolean).join(" · ");

  return (
    <NodeViewWrapper className="listing-block" data-selected={selected} aria-label="Listings code block">
      <header className="listing-block__header" contentEditable={false}>
        <span><Code2 size={14} aria-hidden="true" /> Listings code</span>
        <span>{details}</span>
      </header>
      <NodeViewContent
        className="listing-block__content"
        aria-label="Editable listing source"
        role="textbox"
        spellCheck={false}
      />
    </NodeViewWrapper>
  );
}
