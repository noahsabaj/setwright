import type { Editor, JSONContent } from "@tiptap/core";

export function insertVisualBlock(editor: Editor, content: JSONContent): boolean {
  const { selection } = editor.state;
  let position = selection.$from.depth > 0 ? selection.$from.after(1) : selection.to;
  const documentStarts: number[] = [];
  const documentEnds: number[] = [];
  editor.state.doc.forEach((node, offset) => {
    if (node.type.name !== "rawBlock") return;
    const source = String(node.attrs.source ?? "");
    if (/\\begin\s*\{document\}/u.test(source)) documentStarts.push(offset + node.nodeSize);
    if (/\\end\s*\{document\}/u.test(source)) documentEnds.push(offset);
  });
  if (documentStarts.length !== documentEnds.length || documentStarts.length > 1) return false;
  if (documentStarts.length === 1) {
    const documentStart = documentStarts[0];
    const documentEnd = documentEnds[0];
    if (documentStart === undefined || documentEnd === undefined || documentStart > documentEnd) return false;
    position = Math.min(documentEnd, Math.max(documentStart, position));
  }
  return editor.chain().focus().insertContentAt(position, content).run();
}
