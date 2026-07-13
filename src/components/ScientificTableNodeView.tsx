import type { NodeViewProps } from "@tiptap/react";
import { NodeViewWrapper } from "@tiptap/react";

export function ScientificTableNodeView({ node, selected }: NodeViewProps) {
  const columns = String(node.attrs.columns).split("|");
  const rows = String(node.attrs.rows)
    .split("\n")
    .map((row) => row.split("|"));
  const caption = String(node.attrs.caption);

  return (
    <NodeViewWrapper className="scientific-table" data-selected={selected} contentEditable={false}>
      <table>
        <caption>{caption}</caption>
        <thead><tr>{columns.map((column) => <th scope="col" key={column}>{column}</th>)}</tr></thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.join("|")}>
              {row.map((cell, cellIndex) => cellIndex === 0 ? <th scope="row" key={cell}>{cell}</th> : <td key={`${row[0] ?? "row"}-${cell}`}>{cell}</td>)}
            </tr>
          ))}
        </tbody>
      </table>
    </NodeViewWrapper>
  );
}
