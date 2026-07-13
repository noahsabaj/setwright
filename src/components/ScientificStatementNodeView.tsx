import type { NodeViewProps } from "@tiptap/react";
import { NodeViewContent, NodeViewWrapper } from "@tiptap/react";

const STATEMENT_LABELS = {
  theorem: "Theorem",
  definition: "Definition",
  proof: "Proof",
} as const;

export function ScientificStatementNodeView({ node, selected, updateAttributes }: NodeViewProps) {
  const kind = String(node.attrs.kind) as keyof typeof STATEMENT_LABELS;
  const statementLabel = STATEMENT_LABELS[kind] ?? "Statement";
  const title = String(node.attrs.title);
  const label = String(node.attrs.label);

  return (
    <NodeViewWrapper
      className="scientific-statement"
      data-kind={kind}
      data-selected={selected}
      aria-label={`${statementLabel} environment`}
      role="region"
    >
      <header className="scientific-statement__header" contentEditable={false}>
        <strong>{statementLabel}</strong>
        <label>
          <span>Optional title</span>
          <input
            aria-label={`${statementLabel} optional title`}
            value={title}
            onChange={(event) => updateAttributes({ title: event.target.value })}
            placeholder="No title"
          />
        </label>
        <label>
          <span>Label</span>
          <input
            aria-label={`${statementLabel} source label`}
            value={label}
            onChange={(event) => updateAttributes({ label: event.target.value })}
            placeholder="No label"
            spellCheck={false}
          />
        </label>
      </header>
      <NodeViewContent className="scientific-statement__content" />
    </NodeViewWrapper>
  );
}
