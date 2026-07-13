import { useEffect, useRef, useState } from "react";
import { EditorContent, useEditor } from "@tiptap/react";
import { FileText } from "lucide-react";
import { editorExtensions } from "../editor/extensions";
import { insertVisualBlock } from "../editor/insert-visual-block";
import { projectLatex, reconstructLatex } from "../editor/latex-roundtrip";
import type { LatexProjection, VisualSourceChange } from "../editor/latex-roundtrip";
import { useWorkspaceStore } from "../store/workspace-store";
import { EditorToolbar } from "./EditorToolbar";
import { InsertDialog } from "./InsertDialog";
import type { CitationSearchResult, InsertKind, InsertPayload } from "./InsertDialog";

interface VisualEditorProps {
  source: string;
  fileId: string;
  fileName?: string | undefined;
  onSourceChange: (nextSource: string, changes?: readonly VisualSourceChange[], basisSource?: string) => void;
  onSearchCitations?: ((query: string) => Promise<CitationSearchResult[]>) | undefined;
}

export function VisualEditor({ source, fileId, fileName = "main.tex", onSourceChange, onSearchCitations }: VisualEditorProps) {
  const [dialog, setDialog] = useState<InsertKind | null>(null);
  const [roundTripError, setRoundTripError] = useState<{ message: string; source: string } | null>(null);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const [initialProjection] = useState<LatexProjection>(() => projectLatex(source, fileId));
  const projectionRef = useRef(initialProjection);
  const onSourceChangeRef = useRef(onSourceChange);
  const applyingCanonicalSourceRef = useRef(false);
  const canonicalSourceRef = useRef({ source, fileId });
  const lastEmittedSourceRef = useRef(source);
  const lastAcceptedDocumentRef = useRef(initialProjection.document);

  const editor = useEditor({
    extensions: editorExtensions,
    content: initialProjection.document,
    immediatelyRender: false,
    editorProps: {
      attributes: {
        class: "visual-editor__content",
        "aria-label": "Paper editor",
        spellcheck: "true",
      },
    },
    onUpdate: ({ editor: currentEditor }) => {
      if (applyingCanonicalSourceRef.current) return;
      try {
        const currentDocument = currentEditor.getJSON();
        const reconstruction = reconstructLatex(projectionRef.current, currentDocument);
        lastAcceptedDocumentRef.current = currentDocument;
        setRoundTripError(null);
        if (reconstruction.source !== lastEmittedSourceRef.current) {
          lastEmittedSourceRef.current = reconstruction.source;
          onSourceChangeRef.current(reconstruction.source, reconstruction.changes, projectionRef.current.source);
        }
      } catch (cause) {
        applyingCanonicalSourceRef.current = true;
        try {
          currentEditor.commands.setContent(lastAcceptedDocumentRef.current, { emitUpdate: false });
        } finally {
          applyingCanonicalSourceRef.current = false;
        }
        setRoundTripError({
          message: cause instanceof Error ? cause.message : "This visual change cannot be represented safely in LaTeX.",
          source: projectionRef.current.source,
        });
      }
    },
  });

  useEffect(() => {
    onSourceChangeRef.current = onSourceChange;
  }, [onSourceChange]);

  useEffect(() => {
    if (editor === null) return;
    const canonical = canonicalSourceRef.current;
    if (canonical.source === source && canonical.fileId === fileId && lastEmittedSourceRef.current === source) return;
    if (canonical.fileId === fileId && source === lastEmittedSourceRef.current) {
      canonicalSourceRef.current = { source, fileId };
      return;
    }
    const projection = projectLatex(source, fileId);
    applyingCanonicalSourceRef.current = true;
    projectionRef.current = projection;
    lastAcceptedDocumentRef.current = projection.document;
    canonicalSourceRef.current = { source, fileId };
    lastEmittedSourceRef.current = source;
    editor.commands.setContent(projection.document, { emitUpdate: false });
    applyingCanonicalSourceRef.current = false;
  }, [editor, fileId, source]);

  if (editor === null) return <div className="editor-loading">Preparing your paper…</div>;

  const handleInsert = (payload: InsertPayload) => {
    if (payload.kind === "citation") {
      editor.chain().focus().insertContent({ type: "citation", attrs: { keys: payload.primary, label: payload.secondary || payload.primary } }).run();
    } else if (payload.kind === "equation") {
      insertVisualBlock(editor, { type: "equation", attrs: { latex: payload.primary, label: payload.secondary, numbered: payload.numbered ?? true } });
    } else if (payload.kind === "figure") {
      insertVisualBlock(editor, { type: "figure", attrs: { file: payload.primary, caption: payload.secondary, label: payload.tertiary ?? "" } });
    } else if (payload.kind === "table") {
      const [header = "", ...body] = payload.primary.trim().split("\n");
      insertVisualBlock(editor, { type: "scientificTable", attrs: { columns: header.split("\t").join("|"), rows: body.map((row) => row.split("\t").join("|")).join("\n"), caption: payload.secondary, label: payload.tertiary ?? "" } });
    } else if (payload.kind === "code") {
      insertVisualBlock(editor, { type: "listingBlock" });
    } else if (payload.kind === "quote") {
      insertVisualBlock(editor, { type: "blockquote", content: [{ type: "paragraph" }] });
    } else if (payload.kind === "theorem" || payload.kind === "definition" || payload.kind === "proof") {
      insertVisualBlock(editor, {
        type: "scientificStatement",
        attrs: { kind: payload.kind, title: "", label: "", numbered: payload.kind !== "proof" },
        content: [{ type: "paragraph" }],
      });
    } else {
      setRoundTripError({ message: "This scientific structure is not connected to a safe LaTeX serializer yet.", source: projectionRef.current.source });
    }
    setDialog(null);
  };

  return (
    <section className="visual-editor" aria-label="Visual paper editor">
      <EditorToolbar editor={editor} onInsert={setDialog} onComment={() => setReviewPanel("comments")} />
      {roundTripError === null || roundTripError.source !== source ? null : <p className="workspace-error" role="alert">{roundTripError.message}</p>}
      <div className="visual-editor__scroller">
        <div className="editor-page">
          <div className="editor-page__meta">
            <span><FileText size={13} aria-hidden="true" /> {fileName}</span>
            <span>Source-backed visual view</span>
          </div>
          <EditorContent editor={editor} />
        </div>
      </div>
      {dialog === null ? null : <InsertDialog kind={dialog} onClose={() => setDialog(null)} onInsert={handleInsert} onSearchCitations={onSearchCitations} />}
    </section>
  );
}
