import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { editorExtensions } from "./extensions";
import { insertVisualBlock } from "./insert-visual-block";
import { projectLatex, reconstructLatex } from "./latex-roundtrip";

function cloneDocument(document: JSONContent): JSONContent {
  return structuredClone(document);
}

function blockOfKind(document: JSONContent, kind: string, occurrence = 0): JSONContent {
  const matches = (document.content ?? []).filter((node) => node.attrs?.sourceKind === kind);
  const block = matches[occurrence];
  if (block === undefined) throw new Error(`Missing ${kind} block ${String(occurrence)}`);
  return block;
}

describe("conservative LaTeX projection", () => {
  it.each(["generic", "acm", "ieee"])("keeps the %s first-party template table visually editable", (template) => {
    const path = resolve(process.cwd(), "templates", template, "main.tex");
    const source = readFileSync(path, "utf8");
    const projection = projectLatex(source, `${template}/main.tex`);
    expect(projection.ownership.map((owner) => owner.kind)).toContain("table");
    expect(reconstructLatex(projection, projection.document).source).toBe(source);
  });

  it("reconstructs every source byte exactly when the visual document is unchanged", () => {
    const source = [
      "\uFEFF\\documentclass{article}",
      "\\usepackage{booktabs}",
      "\\title{A \\textbf{careful} paper}",
      "\\author{Ada Lovelace \\and Grace Hopper}",
      "\\begin{document}",
      "\\maketitle",
      "\\begin{abstract}",
      "A short abstract with \\cite{knuth1984}.",
      "\\end{abstract}",
      "\\section{Introduction}",
      "Plain prose with \\textit{emphasis} and Unicode α.",
      "",
      "\\[x = \\sum_{i=1}^{n} i\\]",
      "\\end{document}",
      "",
    ].join("\r\n");
    const projection = projectLatex(source, "main.tex");

    const reconstruction = reconstructLatex(projection, projection.document);

    expect(reconstruction.source).toBe(source);
    expect(reconstruction.changes).toEqual([]);
    expect(projection.ownership.map((owner) => owner.kind)).toEqual(expect.arrayContaining(["title", "author", "abstract", "heading", "paragraph", "equation", "raw"]));
    expect(projection.ownership.at(0)?.startByte).toBe(0);
    expect(projection.ownership.at(-1)?.endByte).toBe(new TextEncoder().encode(source).length);
  });

  it("preserves comments and TikZ exactly while editing an adjacent safe paragraph", () => {
    const tikz = [
      "% keep this comment and spacing exactly", 
      "\\begin{tikzpicture}[scale=.85]",
      "  \\draw[->] (0,0) -- (3,0); % inline comment",
      "\\end{tikzpicture}",
    ].join("\n");
    const source = `\\begin{document}\nOriginal paragraph.\n\n${tikz}\n\nFollowing paragraph.\n\\end{document}\n`;
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    blockOfKind(editedDocument, "paragraph").content = [{ type: "text", text: "Edited paragraph." }];

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.source).toContain(tikz);
    expect(reconstruction.source).toContain("Edited paragraph.");
    expect(reconstruction.source).toContain("Following paragraph.");
    expect(reconstruction.changes).toHaveLength(1);
  });

  it("records UTF-8 byte ownership without splitting Unicode", () => {
    const source = "\\begin{document}\nCafé α and 東京.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const paragraph = blockOfKind(projection.document, "paragraph");
    const owner = projection.ownership.find((candidate) => candidate.ownerId === paragraph.attrs?.sourceOwnerId);
    if (owner === undefined) throw new Error("Missing paragraph owner");
    const expectedStart = new TextEncoder().encode(source.slice(0, source.indexOf("Café"))).length;

    expect(owner.startByte).toBe(expectedStart);
    expect(paragraph.attrs?.sourceStartByte).toBe(expectedStart);
    expect(reconstructLatex(projection, projection.document).source).toBe(source);
  });

  it("changes only the declared source span for one safe paragraph edit", () => {
    const source = "\\begin{document}\nFirst paragraph.\n\nSecond paragraph.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const firstParagraph = blockOfKind(editedDocument, "paragraph", 0);
    const owner = projection.ownership.find((candidate) => candidate.ownerId === firstParagraph.attrs?.sourceOwnerId);
    if (owner === undefined) throw new Error("Missing first paragraph owner");
    firstParagraph.content = [{ type: "text", text: "First paragraph, revised." }];

    const reconstruction = reconstructLatex(projection, editedDocument);
    const expectedStart = owner.startByte + new TextEncoder().encode("First paragraph").length;

    expect(reconstruction.changes).toEqual([expect.objectContaining({
      ownerId: owner.ownerId,
      startByte: expectedStart,
      endByte: expectedStart,
      replacement: ", revised",
    })]);
    expect(reconstruction.source).toBe(source.replace("First paragraph.", "First paragraph, revised."));
    expect(reconstruction.source).toContain("Second paragraph.");
  });

  it("keeps unsupported raw ownership fail-closed when a visual transaction deletes it", () => {
    const source = "\\begin{document}\n\\begin{tikzpicture}\n\\draw (0,0)--(1,1);\n\\end{tikzpicture}\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    editedDocument.content = (editedDocument.content ?? []).filter((node) => node.attrs?.sourceKind !== "raw");

    expect(() => reconstructLatex(projection, editedDocument)).toThrow(/Unsupported LaTeX cannot be deleted/u);
  });

  it("projects only conservative citation, equation, figure, and booktabs forms", () => {
    const source = String.raw`\begin{document}
Evidence supports this claim \citep{smith2024}.

\begin{equation}
x = \frac{1}{n}\sum_{i=1}^{n} y_i
\label{eq:mean}
\end{equation}

\begin{figure}[t]
\centering
\includegraphics[width=0.8\linewidth]{figures/mean.pdf}
\caption{A safe single-image figure.}
\label{fig:mean}
\end{figure}

\begin{table}[t]
\centering
\caption{Safe rectangular results.}
\label{tab:mean}
\begin{tabular}{lcr}
\toprule
Model & Mean & Rank \\
\midrule
Base & 4.1 & 2 \\
Ours & 4.8 & 1 \\
\bottomrule
\end{tabular}
\end{table}
\end{document}
`;
    const projection = projectLatex(source, "main.tex");
    const kinds = projection.ownership.map((owner) => owner.kind);
    const paragraph = blockOfKind(projection.document, "paragraph");

    expect(paragraph.content?.some((node) => node.type === "citation" && node.attrs?.command === "citep")).toBe(true);
    expect(kinds).toContain("equation");
    expect(kinds).toContain("figure");
    expect(kinds).toContain("table");
    expect(reconstructLatex(projection, projection.document).source).toBe(source);
  });

  it("survives Tiptap schema normalization without creating a source edit", () => {
    const source = "\\title{Schema-safe}\n\\author{Ada \\and Grace}\n\\begin{document}\nText with \\textbf{weight} and \\cite{key}.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editor = new Editor({ extensions: editorExtensions, content: projection.document });
    try {
      expect(reconstructLatex(projection, editor.getJSON())).toEqual({ source, changes: [] });
    } finally {
      editor.destroy();
    }
  });

  it("round-trips inline code as texttt instead of rejecting the toolbar mark", () => {
    const source = "\\begin{document}\nOriginal text.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    blockOfKind(editedDocument, "paragraph").content = [
      { type: "text", text: "Inline " },
      { type: "text", text: "code", marks: [{ type: "code" }] },
      { type: "text", text: "." },
    ];

    expect(reconstructLatex(projection, editedDocument).source).toContain("Inline \\texttt{code}.");
  });

  it("round-trips conservative statements, quotes, lists, and listings through the Tiptap schema", () => {
    const source = [
      "\\begin{document}",
      "\\begin{theorem}  [Careful result]",
      "\\label {thm:careful}",
      "  Every safe edit is local.  ",
      "\\end{theorem}   ",
      "\\begin{definition*}",
      "A preserved definition.",
      "\\end{definition*}",
      "\\begin{proof}[Sketch]",
      "Use the invariant.",
      "\\end{proof}",
      "\\begin{quote}",
      "Quoted prose with \\textit{emphasis}.",
      "\\end{quote}",
      "\\begin{itemize}",
      "  \\item   First item.  ",
      "  \\item Second \\textbf{item}.",
      "\\end{itemize}",
      "\\begin{enumerate}",
      "\\item One.",
      "\\item Two.",
      "\\end{enumerate}",
      "\\begin{lstlisting}[language=Python, caption={Tiny loop}, label={lst:tiny}]",
      "for index in range(3):",
      "    print(index)",
      "\\end{lstlisting}",
      "\\end{document}",
      "",
    ].join("\r\n");
    const projection = projectLatex(source, "main.tex");
    const editor = new Editor({ extensions: editorExtensions, content: projection.document });
    try {
      expect(projection.ownership.map((owner) => owner.kind)).toEqual(expect.arrayContaining([
        "theorem", "definition", "proof", "quote", "list", "listing",
      ]));
      expect(reconstructLatex(projection, editor.getJSON())).toEqual({ source, changes: [] });
    } finally {
      editor.destroy();
    }
  });

  it("preserves theorem wrapper trivia while editing only its visual body", () => {
    const source = [
      "\\begin{document}",
      "Before.",
      "",
      "\\begin{theorem}  [Stable title]",
      "\\label {thm:stable}",
      "  Original conclusion.  ",
      "\\end{theorem}   ",
      "After.",
      "\\end{document}",
      "",
    ].join("\r\n");
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const theorem = blockOfKind(editedDocument, "theorem");
    theorem.content = [{ type: "paragraph", content: [{ type: "text", text: "Revised conclusion." }] }];

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.changes).toHaveLength(1);
    expect(reconstruction.source).toContain("\\begin{theorem}  [Stable title]\r\n\\label {thm:stable}\r\n  Revised conclusion.  \r\n\\end{theorem}   \r\n");
    expect(reconstruction.source).toContain("Before.\r\n\r\n");
    expect(reconstruction.source).toContain("After.\r\n\\end{document}");
  });

  it("renames a statement title and label without replacing its environment wrapper", () => {
    const source = "\\begin{document}\n\\begin{theorem}  [Old title]\n  \\label {thm:old}\nStatement body.\n\\end{theorem}   \n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const theorem = blockOfKind(editedDocument, "theorem");
    theorem.attrs = { ...theorem.attrs, title: "New & careful", label: "thm:new" };

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.changes).toHaveLength(1);
    expect(reconstruction.source).toContain("\\begin{theorem}  [New \\& careful]\n  \\label{thm:new}\nStatement body.\n\\end{theorem}   \n");
  });

  it("edits listing code without normalizing options, whitespace, or adjacent unsupported source", () => {
    const source = String.raw`\begin{document}
\begin{lstlisting}[language = {Rust},caption={Exact wrapper},numbers=left,label={lst:exact}]
fn value() -> i32 {
    1
}
\end{lstlisting}  
\begin{tikzpicture}
  \draw (0,0) -- (1,1);
\end{tikzpicture}
\end{document}
`;
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const listing = blockOfKind(editedDocument, "listing");
    const text = listing.content?.[0];
    if (text?.type !== "text") throw new Error("Missing listing text");
    text.text = String(text.text).replace("    1", "    2");

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.changes).toHaveLength(1);
    expect(reconstruction.source).toContain("\\begin{lstlisting}[language = {Rust},caption={Exact wrapper},numbers=left,label={lst:exact}]\nfn value() -> i32 {\n    2\n}\n\\end{lstlisting}  \n");
    expect(reconstruction.source).toContain("\\begin{tikzpicture}\n  \\draw (0,0) -- (1,1);\n\\end{tikzpicture}");
  });

  it("preserves list and quote wrappers during adjacent visual edits", () => {
    const source = String.raw`\begin{document}
\begin{quote}  
  Original quote.  
\end{quote}
\begin{itemize}
  \item   First item.  
  \item Second item.
\end{itemize}
\end{document}
`;
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const quote = blockOfKind(editedDocument, "quote");
    const list = blockOfKind(editedDocument, "list");
    quote.content = [{ type: "paragraph", content: [{ type: "text", text: "Revised quote." }] }];
    const firstItemParagraph = list.content?.[0]?.content?.[0];
    if (firstItemParagraph?.type !== "paragraph") throw new Error("Missing first list item");
    firstItemParagraph.content = [{ type: "text", text: "Revised first item." }];

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.changes).toHaveLength(2);
    expect(reconstruction.source).toContain("\\begin{quote}  \n  Revised quote.  \n\\end{quote}\n");
    expect(reconstruction.source).toContain("\\begin{itemize}\n  \\item   Revised first item.  \n  \\item Second item.\n\\end{itemize}\n");
  });

  it("keeps unsupported statement and list variants as exact raw source", () => {
    const unsupportedStatement = "\\begin{theorem}\nInline math $x$ remains source-only.\n\\end{theorem}\n";
    const unsupportedList = "\\begin{itemize}\n\\item[Key] Optional labels remain source-only.\n\\end{itemize}\n";
    const source = `\\begin{document}\n${unsupportedStatement}${unsupportedList}\\end{document}\n`;
    const projection = projectLatex(source, "main.tex");

    expect(projection.ownership.map((owner) => owner.kind)).not.toContain("theorem");
    expect(projection.ownership.map((owner) => owner.kind)).not.toContain("list");
    expect(reconstructLatex(projection, projection.document)).toEqual({ source, changes: [] });
    expect(projection.ownership.filter((owner) => owner.kind === "raw").map((owner) => owner.original).join("")).toContain(unsupportedStatement);
    expect(projection.ownership.filter((owner) => owner.kind === "raw").map((owner) => owner.original).join("")).toContain(unsupportedList);
  });

  it("serializes newly inserted scientific statements and listings without source metadata", () => {
    const source = "\\begin{document}\nExisting paragraph.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const boundaryIndex = (editedDocument.content ?? []).findIndex((node) => node.attrs?.sourceKind === "raw" && String(node.attrs?.source).includes("\\end{document}"));
    if (boundaryIndex === -1 || editedDocument.content === undefined) throw new Error("Missing document boundary");
    editedDocument.content.splice(
      boundaryIndex,
      0,
      {
        type: "scientificStatement",
        attrs: { kind: "definition", title: "Safe term", label: "def:safe", numbered: true },
        content: [{ type: "paragraph", content: [{ type: "text", text: "A visually inserted definition." }] }],
      },
      {
        type: "listingBlock",
        attrs: { language: "Python", caption: "Tiny example", label: "lst:new" },
        content: [{ type: "text", text: "print(\"safe\")" }],
      },
    );

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.source).toContain("\\begin{definition}[Safe term]\n\\label{def:safe}\nA visually inserted definition.\n\\end{definition}");
    expect(reconstruction.source).toContain("\\begin{lstlisting}[language={Python},caption={Tiny example},label={lst:new}]\nprint(\"safe\")\n\\end{lstlisting}");
    expect(reconstruction.changes.filter((change) => change.ownerId === null)).toHaveLength(2);
  });

  it("keeps source ownership valid when Tiptap inserts a statement from a paragraph selection", () => {
    const source = "\\begin{document}\nExisting paragraph.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editor = new Editor({ extensions: editorExtensions, content: projection.document });
    try {
      const paragraphPositions: number[] = [];
      editor.state.doc.descendants((node, position) => {
        if (paragraphPositions.length === 0 && node.type.name === "paragraph" && node.attrs.sourceKind === "paragraph") paragraphPositions.push(position);
      });
      const paragraphPosition = paragraphPositions[0];
      if (paragraphPosition === undefined) throw new Error("Missing paragraph position");
      editor.commands.setTextSelection(paragraphPosition + 2);
      insertVisualBlock(editor, {
        type: "scientificStatement",
        attrs: { kind: "theorem", title: "", label: "", numbered: true },
        content: [{ type: "paragraph" }],
      });

      const reconstruction = reconstructLatex(projection, editor.getJSON());
      expect(reconstruction.source.match(/Existing paragraph\./gu)).toHaveLength(1);
      expect(reconstruction.source).toContain("\\begin{theorem}\n\n\\end{theorem}");
    } finally {
      editor.destroy();
    }
  });

  it("clamps visual insertion between the document boundaries", () => {
    const source = "\\documentclass{article}\n\\begin{document}\nBody.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editor = new Editor({ extensions: editorExtensions, content: projection.document });
    try {
      let endBoundaryPosition: number | null = null;
      editor.state.doc.forEach((node, position) => {
        if (node.type.name === "rawBlock" && String(node.attrs.source).includes("\\end{document}")) {
          endBoundaryPosition = position;
        }
      });
      if (endBoundaryPosition === null) throw new Error("Missing end document boundary");
      editor.commands.setNodeSelection(endBoundaryPosition);
      expect(insertVisualBlock(editor, {
        type: "scientificStatement",
        attrs: { kind: "theorem", title: "", label: "", numbered: true },
        content: [{ type: "paragraph", content: [{ type: "text", text: "Inside." }] }],
      })).toBe(true);

      const reconstructed = reconstructLatex(projection, editor.getJSON()).source;
      expect(reconstructed.indexOf("\\begin{document}")).toBeLessThan(reconstructed.indexOf("\\begin{theorem}"));
      expect(reconstructed.indexOf("\\begin{theorem}")).toBeLessThan(reconstructed.indexOf("\\end{document}"));
    } finally {
      editor.destroy();
    }
  });

  it("preserves imported prose wrapping and declares only the changed byte", () => {
    const source = "\\begin{document}\nA wrapped\nparagraph stays.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const editedDocument = cloneDocument(projection.document);
    const paragraph = blockOfKind(editedDocument, "paragraph");
    paragraph.content = [{ type: "text", text: "A trapped paragraph stays." }];

    const reconstruction = reconstructLatex(projection, editedDocument);

    expect(reconstruction.source).toContain("A trapped\nparagraph stays.");
    expect(reconstruction.changes).toHaveLength(1);
    expect((reconstruction.changes[0]?.endByte ?? 0) - (reconstruction.changes[0]?.startByte ?? 0)).toBe(1);
    expect(reconstruction.changes[0]?.replacement).toBe("t");
  });

  it("preserves emph as emph and falls back to raw for context-sensitive nested italics", () => {
    const safeSource = "\\begin{document}\nAn \\emph{important} result.\n\\end{document}\n";
    const safeProjection = projectLatex(safeSource, "main.tex");
    const edited = cloneDocument(safeProjection.document);
    const paragraph = blockOfKind(edited, "paragraph");
    const emphasized = paragraph.content?.find((node) => node.marks?.some((mark) => mark.type === "italic"));
    if (emphasized?.type !== "text") throw new Error("Missing emphasized text");
    emphasized.text = "essential";
    expect(reconstructLatex(safeProjection, edited).source).toContain("\\emph{essential}");

    const nestedSource = "\\begin{document}\n\\textit{Italic \\emph{upright} tail}.\n\\end{document}\n";
    const nestedProjection = projectLatex(nestedSource, "main.tex");
    expect(nestedProjection.ownership.map((owner) => owner.kind)).not.toContain("paragraph");
    expect(reconstructLatex(nestedProjection, nestedProjection.document).source).toBe(nestedSource);
  });

  it("preserves exact command spacing and equation label trivia during content edits", () => {
    const source = "\\title  {Old title}\n\\begin{document}\n\\begin{equation}\n x + 1 \\label {eq:spaced}\n\\end{equation}\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const edited = cloneDocument(projection.document);
    blockOfKind(edited, "title").content = [{ type: "text", text: "New title" }];
    blockOfKind(edited, "equation").attrs = {
      ...blockOfKind(edited, "equation").attrs,
      latex: "x + 2",
    };
    const reconstructed = reconstructLatex(projection, edited).source;
    expect(reconstructed).toContain("\\title  {New title}");
    expect(reconstructed).toContain("x + 2 \\label {eq:spaced}");
  });

  it("rejects TeX caret escapes in math and figure paths", () => {
    const source = "\\begin{document}\nSafe paragraph.\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const equationDocument = cloneDocument(projection.document);
    const boundaryIndex = (equationDocument.content ?? []).findIndex((node) => String(node.attrs?.source ?? "").includes("\\end{document}"));
    if (boundaryIndex < 0 || equationDocument.content === undefined) throw new Error("Missing document boundary");
    equationDocument.content.splice(boundaryIndex, 0, { type: "equation", attrs: { latex: "^^5cinput{secret.tex}", numbered: false, label: "" } });
    expect(() => reconstructLatex(projection, equationDocument)).toThrow(/safe visual subset/u);

    const figureDocument = cloneDocument(projection.document);
    figureDocument.content?.splice(boundaryIndex, 0, { type: "figure", attrs: { file: "^^2e^^2e/secret.pdf", caption: "Unsafe", label: "" } });
    expect(() => reconstructLatex(projection, figureDocument)).toThrow(/safe visual subset/u);
  });

  it("rejects listing terminator injection and unsafe optional titles", () => {
    const source = "\\begin{document}\n\\begin{lstlisting}\nprint('safe')\n\\end{lstlisting}\n\\end{document}\n";
    const projection = projectLatex(source, "main.tex");
    const listingDocument = cloneDocument(projection.document);
    const listing = blockOfKind(listingDocument, "listing");
    listing.content = [{ type: "text", text: "\\end{lstlisting}\n\\input{secret.tex}\n\\begin{lstlisting}" }];
    expect(() => reconstructLatex(projection, listingDocument)).toThrow(/listing terminator/u);

    const statementProjection = projectLatex("\\begin{document}\nBody.\n\\end{document}\n", "main.tex");
    const statementDocument = cloneDocument(statementProjection.document);
    const boundaryIndex = (statementDocument.content ?? []).findIndex((node) => String(node.attrs?.source ?? "").includes("\\end{document}"));
    if (boundaryIndex < 0 || statementDocument.content === undefined) throw new Error("Missing document boundary");
    statementDocument.content.splice(boundaryIndex, 0, {
      type: "scientificStatement",
      attrs: { kind: "theorem", title: "A] displaced", label: "", numbered: true },
      content: [{ type: "paragraph", content: [{ type: "text", text: "Claim." }] }],
    });
    expect(() => reconstructLatex(statementProjection, statementDocument)).toThrow(/title or label/u);
  });
});
