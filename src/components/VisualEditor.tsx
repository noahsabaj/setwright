import { useEffect, useRef, useState } from "react";
import { EditorContent, useEditor } from "@tiptap/react";
import { EditorState, Selection } from "@tiptap/pm/state";
import { FileText } from "lucide-react";
import { editorExtensions } from "../editor/extensions";
import { insertVisualBlock } from "../editor/insert-visual-block";
import { projectLatex, reconstructLatex } from "../editor/latex-roundtrip";
import type { LatexProjection, VisualSourceChange } from "../editor/latex-roundtrip";
import type { SourceNavigation } from "../editor/navigation";
import { EditorToolbar } from "./EditorToolbar";
import { InsertDialog } from "./InsertDialog";
import type { CitationSearchResult, InsertKind, InsertPayload } from "./InsertDialog";

interface VisualEditorProps {
  source: string;
  fileId: string;
  fileName?: string | undefined;
  onSourceChange: (nextSource: string, changes?: readonly VisualSourceChange[], basisSource?: string) => void;
  onSearchCitations?: ((query: string) => Promise<CitationSearchResult[]>) | undefined;
  navigation?: SourceNavigation | undefined;
  onNavigationFallback?: ((request: SourceNavigation) => void) | undefined;
  active?: boolean | undefined;
}

export function VisualEditor({ source, fileId, fileName = "main.tex", onSourceChange, onSearchCitations, navigation, onNavigationFallback, active = true }: VisualEditorProps) {
  const [dialog, setDialog] = useState<InsertKind | null>(null);
  const [roundTripError, setRoundTripError] = useState<{ message: string; source: string } | null>(null);
  const [initialProjection] = useState<LatexProjection>(() => projectLatex(source, fileId));
  const projectionRef = useRef(initialProjection);
  const onSourceChangeRef = useRef(onSourceChange);
  const applyingCanonicalSourceRef = useRef(false);
  const canonicalSourceRef = useRef({ source, fileId });
  const lastEmittedSourceRef = useRef(source);
  const lastAcceptedDocumentRef = useRef(initialProjection.document);
  const handledNavigationRef = useRef<number | null>(null);
  const scrollerRef = useRef<HTMLDivElement>(null);

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
    // setContent suppresses the update callback, but still adds a replacement
    // to ProseMirror history. A new canonical epoch must discard that history.
    editor.view.updateState(EditorState.create({
      schema: editor.schema,
      doc: editor.schema.nodeFromJSON(projection.document),
      plugins: editor.state.plugins,
    }));
    editor.view.dispatch(editor.state.tr.setMeta("addToHistory", false));
    applyingCanonicalSourceRef.current = false;
  }, [editor, fileId, source]);

  useEffect(() => {
    if (editor === null || !active || navigation === undefined || navigation.fileId !== fileId || handledNavigationRef.current === navigation.requestId) return;
    try {
      const current = reconstructLatex(projectionRef.current, editor.getJSON());
      const range = current.ranges.find((candidate) => navigation.sourceOffset >= candidate.startOffset && navigation.sourceOffset < candidate.endOffset);
      if (current.source !== source || range === undefined) {
        handledNavigationRef.current = navigation.requestId;
        onNavigationFallback?.(navigation);
        return;
      }
      const node = editor.state.doc.child(range.nodeIndex);
      // An outline can name a source-only construct; leave those to Source.
      if (node.type.name !== "heading" && node.attrs.sourceKind !== "abstract") {
        handledNavigationRef.current = navigation.requestId;
        onNavigationFallback?.(navigation);
        return;
      }
      let position = 0;
      for (let index = 0; index < range.nodeIndex; index += 1) position += editor.state.doc.child(index).nodeSize;
      const selection = Selection.near(editor.state.doc.resolve(position + 1));
      editor.view.dispatch(editor.state.tr.setSelection(selection));
      const target = editor.view.nodeDOM(position);
      const scroller = scrollerRef.current;
      if (!(target instanceof HTMLElement) || scroller === null) {
        handledNavigationRef.current = navigation.requestId;
        onNavigationFallback?.(navigation);
        return;
      }
      // The selection has been applied once. Subsequent source acknowledgements
      // must not reapply it while the deferred viewport reveal is still pending.
      handledNavigationRef.current = navigation.requestId;

      let frame: number | undefined;
      let disposed = false;
      const interactionEvents = ["pointerdown", "mousedown", "keydown", "beforeinput", "input", "wheel", "touchstart", "compositionstart"] as const;
      const stop = () => {
        disposed = true;
        if (frame !== undefined) cancelAnimationFrame(frame);
        resizeObserver.disconnect();
        visibilityObserver.disconnect();
        for (const event of interactionEvents) document.removeEventListener(event, stop, true);
      };
      const reveal = () => {
        frame = undefined;
        if (disposed || editor.isDestroyed || !target.isConnected || scroller.closest('[hidden], [aria-hidden="true"], [inert]') !== null || scroller.getBoundingClientRect().height === 0) return;
        // Focus can restore the old scroll position. Measure and scroll only
        // afterwards, and after a closing drawer has restored its trigger focus.
        editor.view.focus();
        const top = scroller.scrollTop + target.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 24;
        scroller.scrollTo({ top: Math.max(0, top), behavior: "instant" });
        stop();
      };
      const scheduleReveal = () => {
        if (disposed) return;
        if (frame !== undefined) cancelAnimationFrame(frame);
        frame = requestAnimationFrame(() => { frame = requestAnimationFrame(reveal); });
      };
      const resizeObserver = new ResizeObserver(scheduleReveal);
      const visibilityObserver = new MutationObserver(scheduleReveal);
      resizeObserver.observe(scroller);
      for (let ancestor: HTMLElement | null = scroller; ancestor !== null; ancestor = ancestor.parentElement) {
        visibilityObserver.observe(ancestor, { attributes: true, attributeFilter: ["hidden", "aria-hidden", "inert"] });
      }
      // A later click, keystroke, or scroll takes precedence over navigation.
      // Programmatic focus restoration during drawer teardown does not.
      for (const event of interactionEvents) document.addEventListener(event, stop, true);
      scheduleReveal();
      return stop;
    } catch {
      handledNavigationRef.current = navigation.requestId;
      onNavigationFallback?.(navigation);
    }
  }, [active, editor, fileId, navigation, onNavigationFallback, source]);

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
      <EditorToolbar editor={editor} onInsert={setDialog} />
      {roundTripError === null || roundTripError.source !== source ? null : <p className="workspace-error" role="alert">{roundTripError.message}</p>}
      <div className="visual-editor__scroller" ref={scrollerRef}>
        <div className="editor-page">
          <div className="editor-page__meta">
            <span><FileText size={13} aria-hidden="true" /> {fileName}</span>
            <span>Draft</span>
          </div>
          <EditorContent editor={editor} />
        </div>
      </div>
      {dialog === null || !active ? null : <InsertDialog kind={dialog} onClose={() => setDialog(null)} onInsert={handleInsert} onSearchCitations={onSearchCitations} />}
    </section>
  );
}
