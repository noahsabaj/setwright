import { useEffect, useRef } from "react";
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, defaultHighlightStyle, foldGutter, foldKeymap, indentOnInput, StreamLanguage, syntaxHighlighting } from "@codemirror/language";
import { stex } from "@codemirror/legacy-modes/mode/stex";
import { highlightSelectionMatches, openSearchPanel, searchKeymap } from "@codemirror/search";
import { EditorState } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import { crosshairCursor, drawSelection, dropCursor, EditorView, highlightActiveLine, highlightActiveLineGutter, highlightSpecialChars, keymap, lineNumbers, rectangularSelection } from "@codemirror/view";
import { Check, CircleAlert, FileCode2, Search } from "lucide-react";
import { useWorkspaceStore } from "../store/workspace-store";

interface SourceEditorProps {
  value: string;
  fileName?: string | undefined;
  onChange?: (value: string) => void;
  authorityState?: "canonical" | "working" | "unavailable" | undefined;
}

export function SourceEditor({ value, fileName = "main.tex", onChange, authorityState = "canonical" }: SourceEditorProps) {
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const initialValueRef = useRef(value);
  const onChangeRef = useRef(onChange);
  const applyingExternalValueRef = useRef(false);
  const extensionsRef = useRef<Extension[] | null>(null);
  const setSourcePosition = useWorkspaceStore((state) => state.setSourcePosition);
  const sourceUnavailable = authorityState === "unavailable";

  useEffect(() => {
    onChangeRef.current = onChange;
  }, [onChange]);

  useEffect(() => {
    if (mountRef.current === null) return undefined;
    const extensions: Extension[] = [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        foldGutter(),
        drawSelection(),
        dropCursor(),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        bracketMatching(),
        closeBrackets(),
        autocompletion(),
        rectangularSelection(),
        crosshairCursor(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        StreamLanguage.define(stex),
        EditorView.lineWrapping,
        keymap.of([
          indentWithTab,
          ...closeBracketsKeymap,
          ...defaultKeymap,
          ...searchKeymap,
          ...historyKeymap,
          ...foldKeymap,
          ...completionKeymap,
        ]),
        EditorView.updateListener.of((update) => {
          if (update.selectionSet || update.docChanged) {
            const head = update.state.selection.main.head;
            const line = update.state.doc.lineAt(head);
            setSourcePosition(line.number, head - line.from + 1);
          }
          if (update.docChanged && !applyingExternalValueRef.current) {
            onChangeRef.current?.(update.state.doc.toString());
          }
        }),
        EditorView.theme({
          "&": { height: "100%" },
          ".cm-scroller": { overflow: "auto", fontFamily: "var(--font-mono)" },
          ".cm-content": { padding: "22px 0 80px", caretColor: "var(--accent)" },
          ".cm-gutters": { backgroundColor: "var(--source-gutter)", color: "var(--text-tertiary)", border: "none" },
          ".cm-activeLine": { backgroundColor: "var(--source-active)" },
          ".cm-activeLineGutter": { backgroundColor: "var(--source-active)", color: "var(--text-secondary)" },
          ".cm-selectionBackground": { backgroundColor: "var(--selection) !important" },
          ".cm-line": { paddingLeft: "18px" },
        }),
      ];
    extensionsRef.current = extensions;
    const state = EditorState.create({ doc: initialValueRef.current, extensions });
    const view = new EditorView({ state, parent: mountRef.current });
    viewRef.current = view;
    return () => {
      viewRef.current = null;
      extensionsRef.current = null;
      view.destroy();
    };
  }, [setSourcePosition, sourceUnavailable]);

  useEffect(() => {
    const view = viewRef.current;
    if (view === null || view.state.doc.toString() === value) return;
    const extensions = extensionsRef.current;
    if (extensions === null) return;
    applyingExternalValueRef.current = true;
    try {
      // A restore, external reload, or rejected draft starts a new canonical
      // editing epoch. Replacing the state clears undo entries that could
      // otherwise resurrect bytes from the previous epoch.
      view.setState(EditorState.create({ doc: value, extensions }));
      setSourcePosition(1, 1);
    } finally {
      applyingExternalValueRef.current = false;
    }
  }, [setSourcePosition, value]);

  const SourceStatusIcon = authorityState === "canonical" ? Check : CircleAlert;
  const sourceStatus = authorityState === "canonical"
    ? "Canonical Rust buffer"
    : authorityState === "working"
      ? "Working source · not saved"
      : "Source-only encoding";

  return (
    <section className="source-editor" aria-label="LaTeX source editor">
      <div className="source-toolbar">
        <div className="source-toolbar__file"><FileCode2 size={15} /> <strong>{fileName}</strong><span>{sourceUnavailable ? "Original bytes" : "UTF-8"}</span></div>
        <div className="source-toolbar__actions">
          <button type="button" disabled={sourceUnavailable} onClick={() => { if (viewRef.current !== null) openSearchPanel(viewRef.current); }}><Search size={14} /> Find</button>
          <span className="source-toolbar__valid"><SourceStatusIcon size={13} /> {sourceStatus}</span>
        </div>
      </div>
      {sourceUnavailable ? (
        <div className="source-editor__unavailable" role="status">
          <CircleAlert size={24} aria-hidden="true" />
          <strong>This file is not UTF-8.</strong>
          <p>Setwright will not decode or rewrite it until a reviewed conversion is explicitly accepted.</p>
        </div>
      ) : <div className="source-editor__mount" ref={mountRef} />}
    </section>
  );
}
