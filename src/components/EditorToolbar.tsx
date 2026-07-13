import type { Editor } from "@tiptap/core";
import { Bold, BookOpen, Braces, ChevronDown, Code2, Image, Italic, Link2, List, ListOrdered, MessageSquarePlus, Pilcrow, Plus, Redo2, Sigma, Table2, Underline, Undo2 } from "lucide-react";
import type { InsertKind } from "./InsertDialog";

interface EditorToolbarProps {
  editor: Editor;
  onInsert: (kind: InsertKind) => void;
  onComment: () => void;
}

export function EditorToolbar({ editor, onInsert, onComment }: EditorToolbarProps) {
  const setBlock = (value: string) => {
    if (value === "paragraph") editor.chain().focus().setParagraph().run();
    else editor.chain().focus().toggleHeading({ level: Number(value) as 1 | 2 | 3 }).run();
  };

  const addLink = () => {
    const href = window.prompt("Link URL", "https://");
    if (href !== null && href !== "") editor.chain().focus().extendMarkRange("link").setLink({ href }).run();
  };

  return (
    <div className="editor-toolbar" role="toolbar" aria-label="Writing tools">
      <label className="block-select">
        <Pilcrow size={14} aria-hidden="true" />
        <span className="sr-only">Text style</span>
        <select aria-label="Text style" defaultValue="paragraph" onChange={(event) => setBlock(event.target.value)}>
          <option value="paragraph">Paragraph</option>
          <option value="2">Section</option>
          <option value="3">Subsection</option>
        </select>
        <ChevronDown size={13} aria-hidden="true" />
      </label>

      <span className="toolbar-rule" aria-hidden="true" />
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Undo" disabled={!editor.can().undo()} onClick={() => editor.chain().focus().undo().run()}><Undo2 size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Redo" disabled={!editor.can().redo()} onClick={() => editor.chain().focus().redo().run()}><Redo2 size={15} /></button>
      <span className="toolbar-rule" aria-hidden="true" />
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Bold" aria-pressed={editor.isActive("bold")} onClick={() => editor.chain().focus().toggleBold().run()}><Bold size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Italic" aria-pressed={editor.isActive("italic")} onClick={() => editor.chain().focus().toggleItalic().run()}><Italic size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Underline" aria-pressed={editor.isActive("underline")} onClick={() => editor.chain().focus().toggleUnderline().run()}><Underline size={15} /></button>
      <button type="button" className={`toolbar-button toolbar-button--icon ${editor.isActive("code") ? "is-active" : ""}`} aria-label="Inline code" onClick={() => editor.chain().focus().toggleCode().run()}><Code2 size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Link" aria-pressed={editor.isActive("link")} onClick={addLink}><Link2 size={15} /></button>
      <span className="toolbar-rule" aria-hidden="true" />
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Bulleted list" onClick={() => editor.chain().focus().toggleBulletList().run()}><List size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Numbered list" onClick={() => editor.chain().focus().toggleOrderedList().run()}><ListOrdered size={15} /></button>
      <span className="toolbar-rule" aria-hidden="true" />

      <button type="button" className="toolbar-button" onClick={() => onInsert("insert")}><Plus size={15} />Insert</button>
      <button type="button" className="toolbar-button" onClick={() => onInsert("citation")}><BookOpen size={15} />Cite</button>
      <button type="button" className="toolbar-button" onClick={() => onInsert("equation")}><Sigma size={15} />Equation</button>
      <button type="button" className="toolbar-button" onClick={() => onInsert("figure")}><Image size={15} />Figure</button>
      <button type="button" className="toolbar-button" onClick={() => onInsert("table")}><Table2 size={15} />Table</button>
      <button type="button" className="toolbar-button" onClick={onComment}><MessageSquarePlus size={15} />Comments</button>

      <span className="editor-toolbar__spacer" />
      <button type="button" className="toolbar-button toolbar-button--raw" onClick={() => editor.chain().focus().insertContent({ type: "rawBlock", attrs: { environment: "source", source: "" } }).run()}><Braces size={15} />Raw</button>
    </div>
  );
}
