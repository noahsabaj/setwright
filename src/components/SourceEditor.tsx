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
import type { SourceNavigation } from "../editor/navigation";

interface SourceEditorProps {
  value: string;
  fileName?: string | undefined;
  onChange?: (value: string) => void;
  authorityState?: "canonical" | "working" | "unavailable" | undefined;
  fileId?: string | undefined;
  language?: "latex" | "text" | undefined;
  navigation?: SourceNavigation | undefined;
  active?: boolean | undefined;
}

export function SourceEditor({ value, fileName = "main.tex", fileId, language = "latex", navigation, active = true, onChange, authorityState = "canonical" }: SourceEditorProps) {
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const initialValueRef = useRef(value);
  const onChangeRef = useRef(onChange);
  const applyingExternalValueRef = useRef(false);
  const extensionsRef = useRef<Extension[] | null>(null);
  const setSourcePosition = useWorkspaceStore((state) => state.setSourcePosition);
  const sourceUnavailable = authorityState === "unavailable";
  const activeRef = useRef(active);
  const navigatedRef = useRef<number | null>(null);

  useEffect(() => { activeRef.current = active; }, [active]);

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
        ...(language === "latex" ? [StreamLanguage.define(stex)] : []),
        EditorView.contentAttributes.of({ "aria-label": `${fileName} source` }),
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
          if (activeRef.current && (update.selectionSet || update.docChanged)) {
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
  }, [fileName, language, setSourcePosition, sourceUnavailable]);

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
      if (activeRef.current) setSourcePosition(1, 1);
    } finally {
      applyingExternalValueRef.current = false;
    }
  }, [setSourcePosition, value]);

  useEffect(() => {
    const view = viewRef.current;
    if (!active || view === null) return undefined;
    view.requestMeasure();
    let stopNavigation: (() => void) | undefined;
    if (navigation !== undefined && navigation.fileId === fileId && navigation.requestId !== navigatedRef.current) {
      const at = Math.max(0, Math.min(view.state.doc.length, navigation.sourceOffset));
      view.dispatch({ selection: { anchor: at } });
      let frame: number | undefined;
      let cancelled = false;
      const interactionEvents = ["pointerdown", "mousedown", "keydown", "beforeinput", "input", "wheel", "touchstart", "compositionstart"] as const;
      const ancestors: HTMLElement[] = [];
      for (let element: HTMLElement | null = view.dom; element !== null; element = element.parentElement) ancestors.push(element);
      const stop = () => {
        cancelled = true;
        if (frame !== undefined) cancelAnimationFrame(frame);
        resizeObserver.disconnect();
        visibilityObserver.disconnect();
        for (const event of interactionEvents) document.removeEventListener(event, cancelForInteraction, true);
      };
      const cancelForInteraction = () => {
        // A later user action supersedes this navigation, even if a drawer
        // animation has not finished revealing its original destination yet.
        navigatedRef.current = navigation.requestId;
        stop();
      };
      const reveal = () => {
        if (cancelled || !activeRef.current || viewRef.current !== view) return;
        if (ancestors.some((element) => element.hidden || element.inert || element.getAttribute("aria-hidden") === "true")
          || view.scrollDOM.getBoundingClientRect().height === 0) return;
        // Drawer teardown restores its trigger's focus on the next frame.
        // Wait for that restoration, then measure and reveal the source caret.
        view.focus();
        view.dispatch({ effects: EditorView.scrollIntoView(view.state.selection.main.head, { y: "center" }) });
        navigatedRef.current = navigation.requestId;
        stop();
      };
      const schedule = () => {
        if (cancelled) return;
        if (frame !== undefined) cancelAnimationFrame(frame);
        frame = requestAnimationFrame(() => { frame = requestAnimationFrame(reveal); });
      };
      const resizeObserver = new ResizeObserver(schedule);
      const visibilityObserver = new MutationObserver(schedule);
      for (const event of interactionEvents) document.addEventListener(event, cancelForInteraction, true);
      resizeObserver.observe(view.scrollDOM);
      for (const element of ancestors) visibilityObserver.observe(element, { attributes: true, attributeFilter: ["hidden", "aria-hidden", "inert"] });
      schedule();
      stopNavigation = stop;
    }
    const head = view.state.selection.main.head;
    const line = view.state.doc.lineAt(head);
    setSourcePosition(line.number, head - line.from + 1);
    return stopNavigation;
  }, [active, fileId, navigation, setSourcePosition, sourceUnavailable]);

  const SourceStatusIcon = authorityState === "canonical" ? Check : CircleAlert;
  const sourceStatus = authorityState === "canonical"
    ? "Source in sync"
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
