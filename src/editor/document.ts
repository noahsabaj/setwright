import type { JSONContent } from "@tiptap/core";

export const demoVisualDocument: JSONContent = {
  type: "doc",
  content: [
    {
      type: "heading",
      attrs: { level: 1 },
      content: [{ type: "text", text: "Retrieval-Augmented Models Under Distribution Shift" }],
    },
    {
      type: "paragraph",
      content: [
        { type: "text", text: "Maya Chen", marks: [{ type: "bold" }] },
        { type: "text", text: "  ·  Noah Williams  ·  Institute for Language Systems" },
      ],
    },
    {
      type: "heading",
      attrs: { level: 3 },
      content: [{ type: "text", text: "Abstract" }],
    },
    {
      type: "paragraph",
      content: [
        {
          type: "text",
          text: "Retrieval augmentation improves factuality, but its behavior under distribution shift remains poorly understood. We introduce a controlled evaluation spanning temporal, geographic, and lexical shifts.",
        },
      ],
    },
    {
      type: "heading",
      attrs: { level: 2 },
      content: [{ type: "text", text: "1  Introduction" }],
    },
    {
      type: "paragraph",
      content: [
        { type: "text", text: "Large language models can rely on external evidence at inference time " },
        { type: "citation", attrs: { keys: "lewis2020rag", label: "Lewis et al., 2020" } },
        { type: "text", text: ". We ask when retrieval remains useful as the query distribution moves away from the training corpus." },
      ],
    },
    {
      type: "equation",
      attrs: {
        latex: "p(y \\mid x) = \\sum_{z \\in \\mathcal{Z}_k(x)} p_\\eta(z \\mid x) p_\\theta(y \\mid x,z)",
        numbered: true,
        label: "eq:rag-objective",
      },
    },
    { type: "fileBoundary", attrs: { path: "sections/method.tex" } },
    {
      type: "heading",
      attrs: { level: 2 },
      content: [{ type: "text", text: "2  Method" }],
    },
    {
      type: "paragraph",
      content: [
        {
          type: "text",
          text: "We evaluate three controlled shifts across five retrieval corpora. Each example is paired with a timestamped evidence set to prevent future-information leakage.",
        },
      ],
    },
    {
      type: "figure",
      attrs: {
        file: "figures/shift-overview.pdf",
        caption: "Evaluation design. Corpora and queries are shifted independently.",
        label: "fig:shift-overview",
      },
    },
    {
      type: "heading",
      attrs: { level: 3 },
      content: [{ type: "text", text: "2.1  Experimental setup" }],
    },
    {
      type: "paragraph",
      content: [
        { type: "text", text: "Our main result follows from a simple stability argument." },
      ],
    },
    {
      type: "blockquote",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "Theorem 1. " , marks: [{ type: "bold" }] }, { type: "text", text: "If retriever recall decreases by at most ε, expected answer accuracy decreases by at most Lε." }] },
        { type: "paragraph", content: [{ type: "text", text: "Proof. Apply the Lipschitz condition to the evidence-conditioned decoder and sum over the top-k support. ∎" }] },
      ],
    },
    {
      type: "rawBlock",
      attrs: {
        environment: "tikzpicture",
        source: "\\begin{tikzpicture}[scale=.85]\n  \\draw[->] (0,0) -- (3,0) node[right] {shift};\n  \\draw[blue, thick] plot[smooth] coordinates {(0,.3) (1,1.2) (2,.8)};\n\\end{tikzpicture}",
      },
    },
    { type: "fileBoundary", attrs: { path: "main.tex" } },
    {
      type: "heading",
      attrs: { level: 2 },
      content: [{ type: "text", text: "3  Results" }],
    },
    {
      type: "paragraph",
      content: [{ type: "text", text: "Across three shifts, evidence-conditioned models retain 87% of their in-domain performance." }],
    },
    {
      type: "scientificTable",
      attrs: {
        caption: "Accuracy under distribution shift.",
        label: "tab:results",
        columns: "Model|In-domain|Shifted",
        rows: "Parametric|76.4|58.9\nOurs|82.1|71.4",
      },
    },
    {
      type: "heading",
      attrs: { level: 2 },
      content: [{ type: "text", text: "4  Limitations" }],
    },
    {
      type: "paragraph",
      content: [{ type: "text", text: "The benchmark covers English-language corpora and text-only retrieval; multimodal settings remain future work." }],
    },
  ],
};
