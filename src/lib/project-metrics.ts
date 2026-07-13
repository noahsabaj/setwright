import type { JSONContent } from "@tiptap/core";
import { projectLatex } from "../editor/latex-roundtrip";
import type { ProjectFile, ProjectSnapshot } from "./contracts";

export interface ProjectOutlineItem {
  id: string;
  label: string | null;
  depth: number;
  fileId: string;
  filePath: string;
  sourceOffset: number;
  kind: "abstract" | "heading";
}

export interface ProjectMetrics {
  outline: readonly ProjectOutlineItem[];
  outlineStatus: "available" | "partial" | "unavailable";
  outlineNote: string | null;
  referenceCount: number | null;
  labelCount: number | null;
  visualWordCount: number | null;
}

type OutlineEvent =
  | { type: "abstract"; offset: number }
  | { type: "heading"; offset: number; depth: number; title: string | null }
  | { type: "include"; offset: number; target: string | null };

interface OutlineScan {
  events: OutlineEvent[];
  partial: boolean;
}

export function deriveProjectMetrics(project: ProjectSnapshot): ProjectMetrics {
  const texFiles = project.files.filter((file) => file.kind === "tex");
  const bibFiles = project.files.filter((file) => file.kind === "bib");
  const labelsAvailable = texFiles.every((file) => file.content !== null);
  const referencesAvailable = bibFiles.every((file) => file.content !== null);
  const labelCount = labelsAvailable
    ? texFiles.reduce((count, file) => count + countLatexLabels(file.content ?? ""), 0)
    : null;
  const referenceCount = referencesAvailable
    ? bibFiles.reduce((count, file) => count + countBibliographyEntries(file.content ?? ""), 0)
    : null;

  const mainFile = project.files.find((file) => file.id === project.mainFile);
  if (mainFile === undefined || mainFile.kind !== "tex" || mainFile.content === null) {
    return {
      outline: [],
      outlineStatus: "unavailable",
      outlineNote: "The main source is unavailable as UTF-8.",
      referenceCount,
      labelCount,
      visualWordCount: null,
    };
  }

  const filesByPath = new Map<string, ProjectFile>();
  for (const file of texFiles) {
    const normalized = normalizeProjectPath(file.relativePath);
    if (normalized !== null) filesByPath.set(normalized, file);
  }

  const outline: ProjectOutlineItem[] = [];
  const reachedFiles: ProjectFile[] = [];
  const visited = new Set<string>();
  let partial = false;

  const visit = (file: ProjectFile, stack: ReadonlySet<string>) => {
    const normalizedPath = normalizeProjectPath(file.relativePath);
    if (normalizedPath === null || visited.has(file.id)) {
      partial = true;
      return;
    }
    if (file.content === null) {
      partial = true;
      return;
    }
    visited.add(file.id);
    reachedFiles.push(file);
    const nextStack = new Set(stack);
    nextStack.add(file.id);
    const scan = scanOutlineEvents(file.content);
    partial ||= scan.partial;
    for (const event of scan.events) {
      if (event.type === "abstract") {
        outline.push({
          id: `${file.id}:abstract:${String(event.offset)}`,
          label: "Abstract",
          depth: 0,
          fileId: file.id,
          filePath: file.relativePath,
          sourceOffset: event.offset,
          kind: "abstract",
        });
        continue;
      }
      if (event.type === "heading") {
        outline.push({
          id: `${file.id}:heading:${String(event.offset)}`,
          label: event.title,
          depth: event.depth,
          fileId: file.id,
          filePath: file.relativePath,
          sourceOffset: event.offset,
          kind: "heading",
        });
        continue;
      }
      if (event.target === null) {
        partial = true;
        continue;
      }
      const resolvedPath = resolveIncludePath(normalizedPath, event.target);
      const includedFile = resolvedPath === null ? undefined : filesByPath.get(resolvedPath);
      if (includedFile === undefined || nextStack.has(includedFile.id) || visited.has(includedFile.id)) {
        partial = true;
        continue;
      }
      visit(includedFile, nextStack);
    }
  };

  visit(mainFile, new Set());
  const visualWordCount = reachedFiles.reduce((count, file) => (
    count + countVisualWords(file.content ?? "", file.id)
  ), 0);

  return {
    outline,
    outlineStatus: partial ? "partial" : "available",
    outlineNote: partial ? "Only static, unique, in-project includes are shown." : null,
    referenceCount,
    labelCount,
    visualWordCount,
  };
}

function scanOutlineEvents(source: string): OutlineScan {
  const events: OutlineEvent[] = [];
  let partial = false;
  let braceDepth = 0;
  let conditionalDepth = 0;

  for (let index = 0; index < source.length;) {
    const character = source[index];
    if (character === "%" && !isEscaped(source, index)) {
      const newline = source.indexOf("\n", index);
      index = newline === -1 ? source.length : newline + 1;
      continue;
    }
    if (character === "{" && !isEscaped(source, index)) {
      braceDepth += 1;
      index += 1;
      continue;
    }
    if (character === "}" && !isEscaped(source, index)) {
      braceDepth = Math.max(0, braceDepth - 1);
      index += 1;
      continue;
    }
    if (character !== "\\" || braceDepth !== 0) {
      index += 1;
      continue;
    }

    const command = parseCommand(source, index);
    if (command === null) {
      index += 2;
      continue;
    }
    if (command.name.startsWith("if")) {
      conditionalDepth += 1;
      partial = true;
      index = command.end;
      continue;
    }
    if (command.name === "fi") {
      conditionalDepth = Math.max(0, conditionalDepth - 1);
      index = command.end;
      continue;
    }
    if (conditionalDepth > 0) {
      index = command.end;
      continue;
    }

    if (command.name === "begin") {
      const environment = parseRequiredGroup(source, command.end);
      if (environment !== null && environment.value.trim() === "abstract") {
        events.push({ type: "abstract", offset: index });
        index = environment.end;
        continue;
      }
    }
    if (["section", "subsection", "subsubsection"].includes(command.name)) {
      let cursor = command.end;
      if (source[cursor] === "*") cursor += 1;
      const optional = parseOptionalGroup(source, cursor);
      if (optional !== null) cursor = optional.end;
      const title = parseRequiredGroup(source, cursor);
      if (title === null) {
        partial = true;
        index = command.end;
        continue;
      }
      events.push({
        type: "heading",
        offset: index,
        depth: command.name === "section" ? 0 : command.name === "subsection" ? 1 : 2,
        title: decodeHeadingTitle(title.value),
      });
      index = title.end;
      continue;
    }
    if (command.name === "input" || command.name === "include") {
      const target = parseRequiredGroup(source, command.end);
      events.push({
        type: "include",
        offset: index,
        target: target === null ? null : staticIncludeTarget(target.value),
      });
      if (target === null) partial = true;
      index = target?.end ?? command.end;
      continue;
    }
    index = command.end;
  }

  if (conditionalDepth !== 0) partial = true;
  return { events, partial };
}

function countVisualWords(source: string, fileId: string): number {
  const projection = projectLatex(source, fileId);
  let count = 0;
  for (const node of projection.document.content ?? []) {
    if (node.attrs?.sourceKind !== "paragraph" && node.attrs?.sourceKind !== "abstract") continue;
    count += countTextWords(node);
  }
  return count;
}

function countTextWords(node: JSONContent): number {
  if (node.type === "text") {
    return node.text?.match(/[\p{L}\p{N}]+(?:[’'-][\p{L}\p{N}]+)*/gu)?.length ?? 0;
  }
  return (node.content ?? []).reduce((count, child) => count + countTextWords(child), 0);
}

function countLatexLabels(source: string): number {
  return stripLatexComments(source).match(/\\label\s*\{[^{}]+\}/gu)?.length ?? 0;
}

function countBibliographyEntries(source: string): number {
  let count = 0;
  for (let index = 0; index < source.length;) {
    if (source[index] === "%" && !isEscaped(source, index)) {
      const newline = source.indexOf("\n", index);
      index = newline === -1 ? source.length : newline + 1;
      continue;
    }
    if (source[index] !== "@") {
      index += 1;
      continue;
    }
    let typeEnd = index + 1;
    while (typeEnd < source.length && /[A-Za-z]/u.test(source[typeEnd] ?? "")) typeEnd += 1;
    if (typeEnd === index + 1) {
      index += 1;
      continue;
    }
    const opening = skipWhitespace(source, typeEnd);
    const open = source[opening];
    if (open !== "{" && open !== "(") {
      index = typeEnd;
      continue;
    }
    const entryType = source.slice(index + 1, typeEnd).toLowerCase();
    if (entryType !== "comment" && entryType !== "preamble" && entryType !== "string") count += 1;
    const entry = parseBalancedGroup(source, opening, open, open === "{" ? "}" : ")");
    index = entry?.end ?? typeEnd;
  }
  return count;
}

function stripLatexComments(source: string): string {
  let result = "";
  for (let index = 0; index < source.length;) {
    if (source[index] === "%" && !isEscaped(source, index)) {
      const newline = source.indexOf("\n", index);
      if (newline === -1) break;
      result += "\n";
      index = newline + 1;
      continue;
    }
    result += source[index];
    index += 1;
  }
  return result;
}

function decodeHeadingTitle(value: string): string | null {
  let result = "";
  for (let index = 0; index < value.length;) {
    const character = value[index];
    if (character === "~") {
      result += " ";
      index += 1;
      continue;
    }
    if (character === "\r" || character === "\n" || character === "\t") {
      result += " ";
      index += character === "\r" && value[index + 1] === "\n" ? 2 : 1;
      continue;
    }
    if (["$", "%", "#", "_", "^", "{", "}"].includes(character ?? "")) return null;
    if (character !== "\\") {
      result += character;
      index += 1;
      continue;
    }
    const escaped = value[index + 1];
    if (escaped !== undefined && "%$&#_{}".includes(escaped)) {
      result += escaped;
      index += 2;
      continue;
    }
    const command = parseCommand(value, index);
    if (command === null) return null;
    if (["textbf", "textit", "emph", "underline", "texttt", "mbox"].includes(command.name)) {
      const group = parseRequiredGroup(value, command.end);
      if (group === null) return null;
      const nested = decodeHeadingTitle(group.value);
      if (nested === null) return null;
      result += nested;
      index = group.end;
      continue;
    }
    if (command.name === "href") {
      const url = parseRequiredGroup(value, command.end);
      const label = url === null ? null : parseRequiredGroup(value, url.end);
      if (label === null) return null;
      const nested = decodeHeadingTitle(label.value);
      if (nested === null) return null;
      result += nested;
      index = label.end;
      continue;
    }
    if (command.name === "LaTeX" || command.name === "TeX") {
      result += command.name;
      index = command.end;
      continue;
    }
    return null;
  }
  const normalized = result.replace(/\s+/gu, " ").trim();
  return normalized === "" ? null : normalized;
}

function staticIncludeTarget(value: string): string | null {
  const target = value.trim();
  if (target === "" || /[\\{}%]/u.test(target) || /^(?:[A-Za-z]:|\/)/u.test(target)) return null;
  return target;
}

function resolveIncludePath(includingPath: string, target: string): string | null {
  const baseParts = includingPath.split("/").slice(0, -1);
  const targetWithExtension = /\.[^/]+$/u.test(target) ? target : `${target}.tex`;
  return normalizeProjectPath([...baseParts, ...targetWithExtension.split("/")].join("/"));
}

function normalizeProjectPath(value: string): string | null {
  if (value === "" || value.includes("\\") || /^(?:[A-Za-z]:|\/)/u.test(value)) return null;
  const parts: string[] = [];
  for (const part of value.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
    } else {
      parts.push(part);
    }
  }
  return parts.length === 0 ? null : parts.join("/");
}

function parseCommand(source: string, start: number): { name: string; end: number } | null {
  if (source[start] !== "\\") return null;
  const first = source[start + 1];
  if (first === undefined) return null;
  if (!/[A-Za-z@]/u.test(first)) return { name: first, end: start + 2 };
  let end = start + 2;
  while (end < source.length && /[A-Za-z@]/u.test(source[end] ?? "")) end += 1;
  return { name: source.slice(start + 1, end), end };
}

function parseRequiredGroup(source: string, start: number): { value: string; end: number } | null {
  const opening = skipWhitespace(source, start);
  return source[opening] === "{" ? parseBalancedGroup(source, opening, "{", "}") : null;
}

function parseOptionalGroup(source: string, start: number): { value: string; end: number } | null {
  const opening = skipWhitespace(source, start);
  return source[opening] === "[" ? parseBalancedGroup(source, opening, "[", "]") : null;
}

function parseBalancedGroup(source: string, opening: number, open: string, close: string): { value: string; end: number } | null {
  let depth = 1;
  for (let index = opening + 1; index < source.length; index += 1) {
    if (source[index] === open && !isEscaped(source, index)) depth += 1;
    else if (source[index] === close && !isEscaped(source, index)) {
      depth -= 1;
      if (depth === 0) return { value: source.slice(opening + 1, index), end: index + 1 };
    }
  }
  return null;
}

function skipWhitespace(source: string, start: number): number {
  let index = start;
  while (index < source.length && /[\t\n\r ]/u.test(source[index] ?? "")) index += 1;
  return index;
}

function isEscaped(source: string, index: number): boolean {
  let slashCount = 0;
  for (let cursor = index - 1; cursor >= 0 && source[cursor] === "\\"; cursor -= 1) slashCount += 1;
  return slashCount % 2 === 1;
}
