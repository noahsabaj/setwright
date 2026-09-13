import type { Editor } from "@tiptap/core";
import { useEditorState } from "@tiptap/react";
import { Bold, BookOpen, Braces, Check, ChevronDown, Code2, Italic, Link2, List, ListOrdered, MoreHorizontal, Pilcrow, Plus, Redo2, Sigma, Underline, Undo2 } from "lucide-react";
import { Button, Menu, MenuItem, MenuTrigger, Popover } from "react-aria-components";
import type { InsertKind } from "./InsertDialog";
import { useWorkspaceStore } from "../store/workspace-store";

interface EditorToolbarProps {
  editor: Editor;
  onInsert: (kind: InsertKind) => void;
}

export function EditorToolbar({ editor, onInsert }: EditorToolbarProps) {
  const theme = useWorkspaceStore((store) => store.theme);
  const state = useEditorState({
    editor,
    selector: ({ editor: current }) => ({
      block: current.isActive("heading", { level: 2 }) ? "2" : current.isActive("heading", { level: 3 }) ? "3" : "paragraph",
      bold: current.isActive("bold"),
      italic: current.isActive("italic"),
      underline: current.isActive("underline"),
      code: current.isActive("code"),
      link: current.isActive("link"),
      bulletList: current.isActive("bulletList"),
      orderedList: current.isActive("orderedList"),
      canUndo: current.can().undo(),
      canRedo: current.can().redo(),
    }),
  });

  const setBlock = (value: string) => {
    if (value === "paragraph") editor.chain().focus().setParagraph().run();
    else editor.chain().focus().setHeading({ level: Number(value) as 2 | 3 }).run();
  };

  const addLink = () => {
    const href = window.prompt("Link URL", String(editor.getAttributes("link").href ?? "https://"));
    if (href === null) return;
    if (href.trim() === "") editor.chain().focus().extendMarkRange("link").unsetLink().run();
    else editor.chain().focus().extendMarkRange("link").setLink({ href: href.trim() }).run();
  };

  const moreCommands = [
    { id: "underline", label: "Underline", Icon: Underline, active: state.underline, action: () => editor.chain().focus().toggleUnderline().run() },
    { id: "code", label: "Inline code", Icon: Code2, active: state.code, action: () => editor.chain().focus().toggleCode().run() },
    { id: "link", label: "Link", Icon: Link2, active: state.link, action: addLink },
    { id: "bullet", label: "Bulleted list", Icon: List, active: state.bulletList, action: () => editor.chain().focus().toggleBulletList().run() },
    { id: "ordered", label: "Numbered list", Icon: ListOrdered, active: state.orderedList, action: () => editor.chain().focus().toggleOrderedList().run() },
    { id: "raw", label: "Raw LaTeX", Icon: Braces, active: false, action: () => editor.chain().focus().insertContent({ type: "rawBlock", attrs: { environment: "source", source: "" } }).run() },
  ];

  return (
    <div className="editor-toolbar" role="toolbar" aria-label="Writing tools">
      <label className="block-select">
        <Pilcrow size={14} aria-hidden="true" />
        <span className="sr-only">Text style</span>
        <select aria-label="Text style" value={state.block} onChange={(event) => setBlock(event.target.value)}>
          <option value="paragraph">Paragraph</option>
          <option value="2">Section</option>
          <option value="3">Subsection</option>
        </select>
        <ChevronDown size={13} aria-hidden="true" />
      </label>
      <span className="toolbar-rule" aria-hidden="true" />
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Undo" title="Undo" disabled={!state.canUndo} onClick={() => editor.chain().focus().undo().run()}><Undo2 size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Redo" title="Redo" disabled={!state.canRedo} onClick={() => editor.chain().focus().redo().run()}><Redo2 size={15} /></button>
      <span className="toolbar-rule" aria-hidden="true" />
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Bold" title="Bold" aria-pressed={state.bold} onClick={() => editor.chain().focus().toggleBold().run()}><Bold size={15} /></button>
      <button type="button" className="toolbar-button toolbar-button--icon" aria-label="Italic" title="Italic" aria-pressed={state.italic} onClick={() => editor.chain().focus().toggleItalic().run()}><Italic size={15} /></button>
      <MenuTrigger>
        <Button className="toolbar-button toolbar-button--icon toolbar-more" aria-label="More formatting"><MoreHorizontal size={17} /></Button>
        <Popover className="toolbar-menu" data-theme={theme} placement="bottom start">
          <Menu aria-label="More formatting" onAction={(key) => moreCommands.find((command) => command.id === key)?.action()}>
            {moreCommands.map(({ id, label, Icon, active }) => (
              <MenuItem className="toolbar-menu__item" id={id} key={id} textValue={label}>
                <Icon size={15} aria-hidden="true" /><span>{label}</span>{active ? <Check size={14} aria-label="Active" /> : null}
              </MenuItem>
            ))}
          </Menu>
        </Popover>
      </MenuTrigger>
      <span className="editor-toolbar__spacer" />
      <button type="button" className="toolbar-button" onClick={() => onInsert("citation")}><BookOpen size={15} />Cite</button>
      <button type="button" className="toolbar-button" onClick={() => onInsert("equation")}><Sigma size={15} />Equation</button>
      <button type="button" className="toolbar-button toolbar-button--insert" onClick={() => onInsert("insert")}><Plus size={15} />Insert</button>
    </div>
  );
}
