import { Extension, mergeAttributes, Node } from "@tiptap/core";
import Link from "@tiptap/extension-link";
import Placeholder from "@tiptap/extension-placeholder";
import Underline from "@tiptap/extension-underline";
import { ReactNodeViewRenderer } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { CitationNodeView } from "../components/CitationNodeView";
import { EquationNodeView } from "../components/EquationNodeView";
import { FigureNodeView } from "../components/FigureNodeView";
import { FileBoundaryNodeView } from "../components/FileBoundaryNodeView";
import { RawBlockNodeView } from "../components/RawBlockNodeView";
import { ListingBlockNodeView } from "../components/ListingBlockNodeView";
import { ScientificStatementNodeView } from "../components/ScientificStatementNodeView";
import { ScientificTableNodeView } from "../components/ScientificTableNodeView";

const SourceOwnershipAttributes = Extension.create({
  name: "sourceOwnershipAttributes",
  addGlobalAttributes() {
    return [{
      types: ["paragraph", "heading", "blockquote", "bulletList", "orderedList", "equation", "figure", "listingBlock", "scientificStatement", "scientificTable", "rawBlock"],
      attributes: {
        sourceOwnerId: { default: null, rendered: false },
        sourceFileId: { default: null, rendered: false },
        sourceStartByte: { default: null, rendered: false },
        sourceEndByte: { default: null, rendered: false },
        sourceKind: { default: null, rendered: false },
      },
    }];
  },
});

const InlineLatexAttributes = Extension.create({
  name: "inlineLatexAttributes",
  addGlobalAttributes() {
    return [
      {
        types: ["hardBreak"],
        attributes: {
          latexCommand: { default: "\\\\", rendered: false },
        },
      },
      {
        types: ["italic"],
        attributes: {
          latexCommand: { default: "textit", rendered: false },
        },
      },
    ];
  },
});

const CitationNode = Node.create({
  name: "citation",
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  addAttributes() {
    return { keys: { default: "" }, label: { default: "Citation" }, command: { default: "cite", rendered: false } };
  },
  parseHTML() {
    return [{ tag: "span[data-citation]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["span", mergeAttributes(HTMLAttributes, { "data-citation": "" }), String(HTMLAttributes.label)];
  },
  addNodeView() {
    return ReactNodeViewRenderer(CitationNodeView);
  },
});

const EquationNode = Node.create({
  name: "equation",
  group: "block",
  atom: true,
  selectable: true,
  addAttributes() {
    return { latex: { default: "" }, numbered: { default: false }, label: { default: "" } };
  },
  parseHTML() {
    return [{ tag: "div[data-equation]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-equation": "" })];
  },
  addNodeView() {
    return ReactNodeViewRenderer(EquationNodeView);
  },
});

const RawBlockNode = Node.create({
  name: "rawBlock",
  group: "block",
  atom: true,
  selectable: true,
  isolating: true,
  addAttributes() {
    return { source: { default: "" }, environment: { default: "source" } };
  },
  parseHTML() {
    return [{ tag: "pre[data-raw-block]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["pre", mergeAttributes(HTMLAttributes, { "data-raw-block": "" }), String(HTMLAttributes.source)];
  },
  addNodeView() {
    return ReactNodeViewRenderer(RawBlockNodeView);
  },
});

const FigureNode = Node.create({
  name: "figure",
  group: "block",
  atom: true,
  selectable: true,
  addAttributes() {
    return { file: { default: "" }, caption: { default: "" }, label: { default: "" } };
  },
  parseHTML() {
    return [{ tag: "figure[data-setwright-figure]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["figure", mergeAttributes(HTMLAttributes, { "data-setwright-figure": "" })];
  },
  addNodeView() {
    return ReactNodeViewRenderer(FigureNodeView);
  },
});

const ScientificTableNode = Node.create({
  name: "scientificTable",
  group: "block",
  atom: true,
  selectable: true,
  addAttributes() {
    return {
      caption: { default: "" },
      label: { default: "" },
      columns: { default: "Column 1|Column 2" },
      rows: { default: "Value|Value" },
      alignment: { default: "", rendered: false },
    };
  },
  parseHTML() {
    return [{ tag: "div[data-scientific-table]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-scientific-table": "" })];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ScientificTableNodeView);
  },
});

const ScientificStatementNode = Node.create({
  name: "scientificStatement",
  group: "block",
  content: "paragraph+",
  defining: true,
  isolating: true,
  addAttributes() {
    return {
      kind: { default: "theorem" },
      title: { default: "" },
      label: { default: "" },
      numbered: { default: true },
    };
  },
  parseHTML() {
    return [{ tag: "section[data-scientific-statement]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["section", mergeAttributes(HTMLAttributes, { "data-scientific-statement": "" }), 0];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ScientificStatementNodeView);
  },
});

const ListingBlockNode = Node.create({
  name: "listingBlock",
  group: "block",
  content: "text*",
  marks: "",
  code: true,
  defining: true,
  isolating: true,
  addAttributes() {
    return {
      language: { default: "" },
      caption: { default: "" },
      label: { default: "" },
    };
  },
  parseHTML() {
    return [{ tag: "pre[data-listing-block]", preserveWhitespace: "full" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["pre", mergeAttributes(HTMLAttributes, { "data-listing-block": "" }), ["code", 0]];
  },
  addNodeView() {
    return ReactNodeViewRenderer(ListingBlockNodeView);
  },
});

const FileBoundaryNode = Node.create({
  name: "fileBoundary",
  group: "block",
  atom: true,
  selectable: false,
  isolating: true,
  addAttributes() {
    return { path: { default: "main.tex" } };
  },
  parseHTML() {
    return [{ tag: "div[data-file-boundary]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes(HTMLAttributes, { "data-file-boundary": "" })];
  },
  addNodeView() {
    return ReactNodeViewRenderer(FileBoundaryNodeView);
  },
});

export const editorExtensions = [
  StarterKit.configure({ heading: { levels: [1, 2, 3] }, link: false, underline: false }),
  Underline,
  Link.configure({ openOnClick: false, autolink: true, HTMLAttributes: { rel: "noopener noreferrer" } }),
  Placeholder.configure({ placeholder: "Write something, or type / for commands…" }),
  CitationNode,
  EquationNode,
  RawBlockNode,
  FigureNode,
  ScientificStatementNode,
  ListingBlockNode,
  ScientificTableNode,
  FileBoundaryNode,
  SourceOwnershipAttributes,
  InlineLatexAttributes,
];
