import { useEffect } from "react";
import type { FormEvent } from "react";
import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";

export function EquationNodeView({ node, selected, updateAttributes }: NodeViewProps) {
  const latex = String(node.attrs.latex);
  const numbered = Boolean(node.attrs.numbered);
  const label = String(node.attrs.label);

  useEffect(() => {
    void import("mathlive");
  }, []);

  const handleInput = (event: FormEvent<HTMLElement>) => {
    const target = event.currentTarget as HTMLElement & { value?: string };
    updateAttributes({ latex: target.value ?? target.textContent ?? latex });
  };

  return (
    <NodeViewWrapper className="equation-node" data-selected={selected} contentEditable={false}>
      <span className="equation-node__gutter" aria-hidden="true">ƒ</span>
      <math-field
        className="equation-node__field"
        value={latex}
        virtual-keyboard-mode="manual"
        smart-mode
        aria-label="Editable display equation"
        onInput={handleInput}
      />
      {numbered ? <span className="equation-node__number" title={label === "" ? "Number assigned during compilation" : `${label} · number assigned during compilation`}>Numbered</span> : null}
    </NodeViewWrapper>
  );
}
