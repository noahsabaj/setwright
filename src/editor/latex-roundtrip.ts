import type { JSONContent } from "@tiptap/core";

export type SourceOwnershipKind =
  | "abstract"
  | "author"
  | "equation"
  | "figure"
  | "heading"
  | "list"
  | "listing"
  | "paragraph"
  | "proof"
  | "quote"
  | "raw"
  | "table"
  | "theorem"
  | "definition"
  | "title";

type SerializationMetadata =
  | { type: "abstract"; opening: string; closing: string; innerPrefix: string; innerSuffix: string; suffix: string }
  | { type: "command"; opening: string; closing: string; suffix: string }
  | { type: "equation"; opening: string; closing: string; innerPrefix: string; innerSuffix: string; numbered: boolean; originalBody: string; originalLatex: string; originalLabel: string; labelToken: string; suffix: string }
  | { type: "figure"; opening: string; closing: string; placement: string; graphicsOptions: string; centered: boolean; suffix: string }
  | { type: "list"; environment: "itemize" | "enumerate"; opening: string; closing: string; itemPrefixes: string[]; itemSuffixes: string[]; suffix: string }
  | { type: "listing"; opening: string; closing: string; originalLanguage: string; originalCaption: string; originalLabel: string; suffix: string }
  | { type: "paragraph"; prefix: string; suffix: string }
  | { type: "quote"; opening: string; closing: string; innerPrefix: string; innerSuffix: string; suffix: string }
  | { type: "raw" }
  | { type: "statement"; environment: "theorem" | "theorem*" | "definition" | "definition*" | "proof"; openingBase: string; originalOpeningExtra: string; leadingBeforeTitle: string; originalTitle: string; closing: string; innerPrefix: string; innerSuffix: string; originalLabel: string; labelToken: string; labelPlacement: "prefix" | "suffix" | "none"; suffix: string }
  | { type: "table"; opening: string; closing: string; placement: string; alignment: string; centered: boolean; suffix: string };

export interface SourceOwnership {
  ownerId: string;
  fileId: string;
  startByte: number;
  endByte: number;
  kind: SourceOwnershipKind;
  original: string;
  semanticFingerprint: string;
  metadata: SerializationMetadata;
}

export interface LatexProjection {
  source: string;
  fileId: string;
  document: JSONContent;
  ownership: readonly SourceOwnership[];
  newline: "\n" | "\r\n";
}

export interface VisualSourceChange {
  ownerId: string | null;
  fileId: string;
  startByte: number | null;
  endByte: number | null;
  replacement: string;
}

export interface LatexReconstruction {
  source: string;
  changes: readonly VisualSourceChange[];
}

interface BlockDraft {
  start: number;
  end: number;
  kind: SourceOwnershipKind;
  node: JSONContent;
  metadata: SerializationMetadata;
}

interface ParsedGroup {
  value: string;
  start: number;
  end: number;
  raw: string;
}

interface ParsedCommand {
  name: string;
  nameEnd: number;
}

const SOURCE_ATTRIBUTE_NAMES = [
  "sourceOwnerId",
  "sourceFileId",
  "sourceStartByte",
  "sourceEndByte",
  "sourceKind",
] as const;

const INLINE_MARK_COMMANDS = new Map<string, "bold" | "italic" | "underline" | "code">([
  ["textbf", "bold"],
  ["textit", "italic"],
  ["emph", "italic"],
  ["underline", "underline"],
  ["texttt", "code"],
] as const);

const CITATION_COMMANDS = new Set(["cite", "citep", "citet", "parencite", "textcite", "autocite"]);
const SAFE_MATH_ENVIRONMENTS = new Set(["matrix", "pmatrix", "bmatrix", "vmatrix", "Vmatrix", "cases", "aligned", "split", "gathered"]);
const SAFE_MATH_COMMANDS = new Set([
  "alpha", "beta", "gamma", "delta", "epsilon", "varepsilon", "zeta", "eta", "theta", "vartheta", "iota", "kappa", "lambda",
  "mu", "nu", "xi", "pi", "varpi", "rho", "varrho", "sigma", "varsigma", "tau", "upsilon", "phi", "varphi", "chi", "psi", "omega",
  "Gamma", "Delta", "Theta", "Lambda", "Xi", "Pi", "Sigma", "Upsilon", "Phi", "Psi", "Omega",
  "frac", "dfrac", "tfrac", "sqrt", "sum", "prod", "coprod", "int", "iint", "iiint", "oint", "lim", "min", "max", "inf", "sup",
  "sin", "cos", "tan", "log", "ln", "exp", "det", "gcd", "Pr", "mathbb", "mathbf", "mathrm", "mathit", "mathsf", "mathtt", "mathcal",
  "left", "right", "middle", "big", "Big", "bigg", "Bigg", "bigl", "bigr", "Bigl", "Bigr", "biggl", "biggr", "Biggl", "Biggr",
  "cdot", "times", "div", "pm", "mp", "ast", "star", "circ", "bullet", "oplus", "otimes", "le", "leq", "ge", "geq", "ne", "neq",
  "approx", "sim", "simeq", "equiv", "propto", "in", "notin", "ni", "subset", "subseteq", "supset", "supseteq", "cup", "cap", "setminus",
  "land", "lor", "neg", "forall", "exists", "nexists", "to", "rightarrow", "leftarrow", "leftrightarrow", "Rightarrow", "Leftarrow", "Leftrightarrow",
  "mapsto", "infty", "partial", "nabla", "ell", "Re", "Im", "emptyset", "varnothing", "top", "bot", "angle", "mid", "vert", "Vert",
  "langle", "rangle", "lceil", "rceil", "lfloor", "rfloor", "underbrace", "overbrace", "overline", "underline", "hat", "widehat", "bar", "vec",
  "dot", "ddot", "text", "operatorname", "begin", "end", "quad", "qquad", "enspace", "label",
]);

const textEncoder = new TextEncoder();

export function projectLatex(source: string, fileId: string): LatexProjection {
  const newline = source.includes("\r\n") ? "\r\n" : "\n";
  const drafts = scanSource(source);
  const completeDrafts = drafts.length === 0
    ? [paragraphDraft(0, 0, "", "", [])]
    : mergeAdjacentRawDrafts(drafts, source);
  const ownership: SourceOwnership[] = [];
  const content = completeDrafts.map((draft) => {
    const startByte = utf8Length(source.slice(0, draft.start));
    const endByte = startByte + utf8Length(source.slice(draft.start, draft.end));
    const ownerId = `${fileId}:${String(startByte)}:${String(endByte)}:${draft.kind}`;
    const node = withOwnershipAttributes(draft.node, ownerId, fileId, startByte, endByte, draft.kind);
    ownership.push({
      ownerId,
      fileId,
      startByte,
      endByte,
      kind: draft.kind,
      original: source.slice(draft.start, draft.end),
      semanticFingerprint: semanticFingerprint(node),
      metadata: draft.metadata,
    });
    return node;
  });

  return {
    source,
    fileId,
    document: { type: "doc", content },
    ownership,
    newline,
  };
}

export function reconstructLatex(projection: LatexProjection, document: JSONContent): LatexReconstruction {
  const owners = new Map(projection.ownership.map((owner) => [owner.ownerId, owner]));
  const seenOwners = new Set<string>();
  const changes: VisualSourceChange[] = [];
  const fragments: string[] = [];

  for (const node of document.content ?? []) {
    const ownerId = sourceAttribute(node, "sourceOwnerId");
    if (ownerId === null) {
      const replacement = `${serializeNewBlock(node, projection.newline)}${projection.newline}${projection.newline}`;
      fragments.push(replacement);
      changes.push({ ownerId: null, fileId: projection.fileId, startByte: null, endByte: null, replacement });
      continue;
    }

    if (seenOwners.has(ownerId)) {
      throw new Error("A visual block duplicated its source ownership; the source was left unchanged.");
    }
    const owner = owners.get(ownerId);
    if (owner === undefined || sourceAttribute(node, "sourceFileId") !== projection.fileId) {
      throw new Error("A visual block no longer matches the canonical source file; the source was left unchanged.");
    }
    seenOwners.add(ownerId);

    let replacement = semanticFingerprint(node) === owner.semanticFingerprint
      ? owner.original
      : serializeOwnedBlock(node, owner, projection.newline);
    if (replacement !== owner.original && preservesTextualTrivia(owner.kind)) {
      replacement = preserveEquivalentWhitespace(owner.original, replacement);
    }
    fragments.push(replacement);
    if (replacement !== owner.original) {
      changes.push(minimalOwnedChange(owner, replacement));
    }
  }

  for (const owner of projection.ownership) {
    if (owner.kind === "raw" && !seenOwners.has(owner.ownerId)) {
      throw new Error("Unsupported LaTeX cannot be deleted from the visual projection; edit its source card explicitly.");
    }
  }

  return { source: fragments.join(""), changes };
}

function scanSource(source: string): BlockDraft[] {
  const drafts: BlockDraft[] = [];
  let segmentStart = 0;
  let index = 0;
  let braceDepth = 0;
  let conditionalDepth = 0;
  let insideDocument = !/\\begin\s*\{document\}/u.test(source);

  const flushGap = (end: number, visualText: boolean) => {
    if (end <= segmentStart) return;
    drafts.push(...projectTextGap(source, segmentStart, end, visualText));
  };

  const emit = (draft: BlockDraft) => {
    drafts.push(draft);
    index = draft.end;
    segmentStart = draft.end;
  };

  while (index < source.length) {
    const character = source[index];
    if (character === "%" && !isEscaped(source, index)) {
      const newlineIndex = source.indexOf("\n", index);
      index = newlineIndex === -1 ? source.length : newlineIndex + 1;
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

    if (source.startsWith("\\[", index) && insideDocument && conditionalDepth === 0) {
      const closing = findUnescapedToken(source, "\\]", index + 2);
      if (closing === -1) {
        index += 2;
        continue;
      }
      flushGap(index, true);
      const coreEnd = closing + 2;
      const end = consumeWhitespace(source, coreEnd);
      emit(parseDisplayEquation(source, index, coreEnd, end));
      continue;
    }

    const command = parseCommand(source, index);
    if (command === null) {
      index += 2;
      continue;
    }

    if (command.name.startsWith("if")) {
      conditionalDepth += 1;
      index = command.nameEnd;
      continue;
    }
    if (command.name === "fi") {
      conditionalDepth = Math.max(0, conditionalDepth - 1);
      index = command.nameEnd;
      continue;
    }

    if ((command.name === "begin" || command.name === "end") && conditionalDepth === 0) {
      const environmentGroup = parseRequiredGroup(source, command.nameEnd);
      if (environmentGroup === null) {
        index = command.nameEnd;
        continue;
      }
      const environment = environmentGroup.value.trim();
      if (environment === "document") {
        flushGap(index, insideDocument);
        const coreEnd = environmentGroup.end;
        const end = consumeWhitespace(source, coreEnd);
        emit(rawDraft(source, index, end, "document boundary"));
        insideDocument = command.name === "begin";
        continue;
      }
      if (command.name === "begin") {
        const endMatch = findEnvironmentEnd(source, environmentGroup.end, environment);
        if (endMatch === null) {
          index = command.nameEnd;
          continue;
        }
        flushGap(index, insideDocument);
        const coreEnd = endMatch.end;
        const end = consumeWhitespace(source, coreEnd);
        const draft = insideDocument
          ? parseEnvironment(source, index, environmentGroup.end, endMatch.start, coreEnd, end, environment)
          : rawDraft(source, index, end, environment);
        emit(draft);
        continue;
      }
    }

    const structural = conditionalDepth === 0 && (
      command.name === "title"
      || command.name === "author"
      || (insideDocument && ["section", "subsection", "subsubsection"].includes(command.name))
    );
    if (structural) {
      const parsed = parseStructuralCommand(source, index, command);
      if (parsed !== null) {
        flushGap(index, insideDocument);
        emit(parsed);
        continue;
      }
    }

    index = command.nameEnd;
  }

  flushGap(source.length, insideDocument);
  return drafts;
}

function projectTextGap(source: string, start: number, end: number, visualText: boolean): BlockDraft[] {
  if (!visualText) return [rawDraft(source, start, end, "source")];
  const drafts: BlockDraft[] = [];
  const gap = source.slice(start, end);
  const separator = /(?:\r?\n[\t ]*){2,}/gu;
  let relativeStart = 0;
  for (const match of gap.matchAll(separator)) {
    const matchIndex = match.index;
    const relativeEnd = matchIndex + match[0].length;
    drafts.push(projectParagraphPart(source, start + relativeStart, start + relativeEnd));
    relativeStart = relativeEnd;
  }
  if (relativeStart < gap.length) drafts.push(projectParagraphPart(source, start + relativeStart, end));
  if (gap.length === 0) return drafts;
  return drafts;
}

function projectParagraphPart(source: string, start: number, end: number): BlockDraft {
  const part = source.slice(start, end);
  const leadingLength = part.match(/^[\t\n\r ]*/u)?.[0].length ?? 0;
  const trailingLength = part.match(/[\t\n\r ]*$/u)?.[0].length ?? 0;
  const coreEnd = Math.max(leadingLength, part.length - trailingLength);
  const core = part.slice(leadingLength, coreEnd);
  if (core === "") return rawDraft(source, start, end, "spacing");
  const inline = parseInlineLatex(core);
  if (inline === null) return rawDraft(source, start, end, detectRawEnvironment(core));
  return paragraphDraft(
    start,
    end,
    part.slice(0, leadingLength),
    part.slice(coreEnd),
    inline,
  );
}

function paragraphDraft(start: number, end: number, prefix: string, suffix: string, content: JSONContent[]): BlockDraft {
  return {
    start,
    end,
    kind: "paragraph",
    node: { type: "paragraph", content },
    metadata: { type: "paragraph", prefix, suffix },
  };
}

function parseStructuralCommand(source: string, start: number, command: ParsedCommand): BlockDraft | null {
  let cursor = command.nameEnd;
  if (source[cursor] === "*") {
    cursor += 1;
  }
  const optional = parseOptionalGroup(source, cursor);
  if (optional !== null) cursor = optional.end;
  const argument = parseRequiredGroup(source, cursor);
  if (argument === null) return null;
  const inline = parseInlineLatex(argument.value);
  const coreEnd = argument.end;
  const end = consumeWhitespace(source, coreEnd);
  if (inline === null) return rawDraft(source, start, end, command.name);

  const opening = source.slice(start, argument.start + 1);
  const closing = source.slice(argument.end - 1, coreEnd);
  const suffix = source.slice(coreEnd, end);
  if (command.name === "title") {
    return {
      start,
      end,
      kind: "title",
      node: { type: "heading", attrs: { level: 1 }, content: inline },
      metadata: { type: "command", opening, closing, suffix },
    };
  }
  if (command.name === "author") {
    return {
      start,
      end,
      kind: "author",
      node: { type: "paragraph", content: inline },
      metadata: { type: "command", opening, closing, suffix },
    };
  }
  const level = command.name === "section" ? 2 : 3;
  return {
    start,
    end,
    kind: "heading",
    node: { type: "heading", attrs: { level }, content: inline },
    metadata: { type: "command", opening, closing, suffix },
  };
}

function parseEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  environment: string,
): BlockDraft {
  if (environment === "abstract") return parseAbstract(source, start, bodyStart, bodyEnd, coreEnd, end);
  if (["equation", "equation*", "displaymath"].includes(environment)) {
    return parseEquationEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end, environment);
  }
  if (environment === "figure" || environment === "figure*") {
    return parseFigureEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end, environment);
  }
  if (environment === "table" || environment === "table*") {
    return parseTableEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end, environment);
  }
  if (["theorem", "theorem*", "definition", "definition*", "proof"].includes(environment)) {
    return parseStatementEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end, environment);
  }
  if (environment === "lstlisting") {
    return parseListingEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end);
  }
  if (environment === "quote") {
    return parseQuoteEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end);
  }
  if (environment === "itemize" || environment === "enumerate") {
    return parseListEnvironment(source, start, bodyStart, bodyEnd, coreEnd, end, environment);
  }
  return rawDraft(source, start, end, environment);
}

function parseStatementEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  environmentValue: string,
): BlockDraft {
  const environment = environmentValue as "theorem" | "theorem*" | "definition" | "definition*" | "proof";
  const body = source.slice(bodyStart, bodyEnd);
  const optionalStart = skipWhitespace(body, 0);
  const optionalTitle = parseOptionalGroup(body, optionalStart);
  const title = optionalTitle === null ? "" : decodePlainLatex(optionalTitle.value);
  if (title === null) return rawDraft(source, start, end, environment);
  const contentStart = optionalTitle?.end ?? 0;
  const content = body.slice(contentStart);
  const labels = [...content.matchAll(/\\label\s*\{([^{}]+)\}/gu)];
  if (labels.length > 1) return rawDraft(source, start, end, environment);

  const labelMatch = labels[0];
  const label = labelMatch?.[1]?.trim() ?? "";
  if (!isSafeLabel(label)) return rawDraft(source, start, end, environment);
  let labelPlacement: "prefix" | "suffix" | "none" = "none";
  let labelToken = "";
  let contentCoreStart = 0;
  let contentCoreEnd = content.length;

  if (labelMatch !== undefined && labelMatch.index !== undefined) {
    const labelStart = labelMatch.index;
    const labelEnd = labelStart + labelMatch[0].length;
    const beforeLabel = content.slice(0, labelStart);
    const afterLabel = content.slice(labelEnd);
    labelToken = labelMatch[0];
    if (beforeLabel.trim() === "") {
      labelPlacement = "prefix";
      const bounds = whitespaceBounds(afterLabel);
      contentCoreStart = labelEnd + bounds.start;
      contentCoreEnd = labelEnd + bounds.end;
    } else if (afterLabel.trim() === "") {
      labelPlacement = "suffix";
      const bounds = whitespaceBounds(beforeLabel);
      contentCoreStart = bounds.start;
      contentCoreEnd = bounds.end;
    } else {
      return rawDraft(source, start, end, environment);
    }
  } else {
    const bounds = whitespaceBounds(content);
    contentCoreStart = bounds.start;
    contentCoreEnd = bounds.end;
  }

  const paragraphs = splitVisualParagraphs(content.slice(contentCoreStart, contentCoreEnd));
  if (paragraphs === null) return rawDraft(source, start, end, environment);
  const statementKind = environment.startsWith("theorem") ? "theorem" : environment.startsWith("definition") ? "definition" : "proof";
  return {
    start,
    end,
    kind: statementKind,
    node: {
      type: "scientificStatement",
      attrs: {
        kind: statementKind,
        title,
        label,
        numbered: statementKind !== "proof" && !environment.endsWith("*"),
      },
      content: paragraphs.map((paragraph) => ({ type: "paragraph", content: paragraph })),
    },
    metadata: {
      type: "statement",
      environment,
      openingBase: source.slice(start, bodyStart),
      originalOpeningExtra: body.slice(0, contentStart),
      leadingBeforeTitle: optionalTitle === null ? "" : body.slice(0, optionalStart),
      originalTitle: title,
      closing: source.slice(bodyEnd, coreEnd),
      innerPrefix: content.slice(0, contentCoreStart),
      innerSuffix: content.slice(contentCoreEnd),
      originalLabel: label,
      labelToken,
      labelPlacement,
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseListingEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
): BlockDraft {
  const body = source.slice(bodyStart, bodyEnd);
  const optionalStart = skipWhitespace(body, 0);
  const options = parseOptionalGroup(body, optionalStart);
  const codeStart = options?.end ?? 0;
  const optionValues = options === null ? new Map<string, string>() : parseListingOptions(options.value);
  const language = optionValues.get("language") ?? "";
  const captionValue = optionValues.get("caption") ?? "";
  const caption = captionValue === "" ? "" : decodePlainLatex(stripOuterBraces(captionValue));
  const label = stripOuterBraces(optionValues.get("label") ?? "").trim();
  if (caption === null || !isSafeLabel(label)) return rawDraft(source, start, end, "lstlisting");
  const code = body.slice(codeStart);
  if (!isSafeListingBody(code)) return rawDraft(source, start, end, "lstlisting");
  return {
    start,
    end,
    kind: "listing",
    node: {
      type: "listingBlock",
      attrs: { language: stripOuterBraces(language), caption, label },
      ...(code === "" ? {} : { content: [{ type: "text", text: code }] }),
    },
    metadata: {
      type: "listing",
      opening: `${source.slice(start, bodyStart)}${body.slice(0, codeStart)}`,
      closing: source.slice(bodyEnd, coreEnd),
      originalLanguage: stripOuterBraces(language),
      originalCaption: caption,
      originalLabel: label,
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseQuoteEnvironment(source: string, start: number, bodyStart: number, bodyEnd: number, coreEnd: number, end: number): BlockDraft {
  const body = source.slice(bodyStart, bodyEnd);
  const bounds = whitespaceBounds(body);
  const paragraphs = splitVisualParagraphs(body.slice(bounds.start, bounds.end));
  if (paragraphs === null) return rawDraft(source, start, end, "quote");
  return {
    start,
    end,
    kind: "quote",
    node: { type: "blockquote", content: paragraphs.map((paragraph) => ({ type: "paragraph", content: paragraph })) },
    metadata: {
      type: "quote",
      opening: source.slice(start, bodyStart),
      closing: source.slice(bodyEnd, coreEnd),
      innerPrefix: body.slice(0, bounds.start),
      innerSuffix: body.slice(bounds.end),
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseListEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  environment: "itemize" | "enumerate",
): BlockDraft {
  const body = source.slice(bodyStart, bodyEnd);
  if (containsUnescapedComment(body)) return rawDraft(source, start, end, environment);
  const itemStarts = findTopLevelCommands(body, "item");
  const firstItemStart = itemStarts[0];
  if (firstItemStart === undefined || body.slice(0, firstItemStart).trim() !== "") return rawDraft(source, start, end, environment);
  const itemPrefixes: string[] = [];
  const itemSuffixes: string[] = [];
  const items: JSONContent[] = [];

  for (let itemIndex = 0; itemIndex < itemStarts.length; itemIndex += 1) {
    const itemStart = itemStarts[itemIndex] ?? 0;
    const command = parseCommand(body, itemStart);
    if (command?.name !== "item" || parseOptionalGroup(body, command.nameEnd) !== null) return rawDraft(source, start, end, environment);
    const itemEnd = itemStarts[itemIndex + 1] ?? body.length;
    const contentStart = skipWhitespace(body, command.nameEnd);
    const itemBody = body.slice(contentStart, itemEnd);
    const bounds = whitespaceBounds(itemBody);
    const inline = parseInlineLatex(itemBody.slice(bounds.start, bounds.end));
    if (inline === null) return rawDraft(source, start, end, environment);
    itemPrefixes.push(body.slice(itemStart, contentStart + bounds.start));
    itemSuffixes.push(itemBody.slice(bounds.end));
    items.push({ type: "listItem", content: [{ type: "paragraph", content: inline }] });
  }

  return {
    start,
    end,
    kind: "list",
    node: {
      type: environment === "itemize" ? "bulletList" : "orderedList",
      ...(environment === "enumerate" ? { attrs: { start: 1 } } : {}),
      content: items,
    },
    metadata: {
      type: "list",
      environment,
      opening: `${source.slice(start, bodyStart)}${body.slice(0, firstItemStart)}`,
      closing: source.slice(bodyEnd, coreEnd),
      itemPrefixes,
      itemSuffixes,
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseListingOptions(value: string): Map<string, string> {
  const options = new Map<string, string>();
  for (const segment of splitTopLevel(value, ",")) {
    const trimmed = segment.trim();
    if (trimmed === "") continue;
    const equals = findTopLevelCharacter(trimmed, "=");
    const key = (equals === -1 ? trimmed : trimmed.slice(0, equals)).trim();
    if (!/^[A-Za-z][A-Za-z0-9_-]*$/u.test(key)) continue;
    options.set(key, equals === -1 ? "" : trimmed.slice(equals + 1).trim());
  }
  return options;
}

function stripOuterBraces(value: string): string {
  const trimmed = value.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) return trimmed;
  const group = parseBalancedGroup(trimmed, 0, "{", "}");
  return group?.end === trimmed.length ? group.value : trimmed;
}

function splitTopLevel(value: string, separator: string): string[] {
  const parts: string[] = [];
  let start = 0;
  let braceDepth = 0;
  let bracketDepth = 0;
  for (let index = 0; index < value.length; index += 1) {
    const character = value[index];
    if (character === "{" && !isEscaped(value, index)) braceDepth += 1;
    else if (character === "}" && !isEscaped(value, index)) braceDepth = Math.max(0, braceDepth - 1);
    else if (character === "[" && !isEscaped(value, index)) bracketDepth += 1;
    else if (character === "]" && !isEscaped(value, index)) bracketDepth = Math.max(0, bracketDepth - 1);
    else if (character === separator && braceDepth === 0 && bracketDepth === 0 && !isEscaped(value, index)) {
      parts.push(value.slice(start, index));
      start = index + 1;
    }
  }
  parts.push(value.slice(start));
  return parts;
}

function findTopLevelCharacter(value: string, target: string): number {
  let braceDepth = 0;
  let bracketDepth = 0;
  for (let index = 0; index < value.length; index += 1) {
    const character = value[index];
    if (character === "{" && !isEscaped(value, index)) braceDepth += 1;
    else if (character === "}" && !isEscaped(value, index)) braceDepth = Math.max(0, braceDepth - 1);
    else if (character === "[" && !isEscaped(value, index)) bracketDepth += 1;
    else if (character === "]" && !isEscaped(value, index)) bracketDepth = Math.max(0, bracketDepth - 1);
    else if (character === target && braceDepth === 0 && bracketDepth === 0 && !isEscaped(value, index)) return index;
  }
  return -1;
}

function containsUnescapedComment(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === "%" && !isEscaped(value, index)) return true;
  }
  return false;
}

function findTopLevelCommands(value: string, name: string): number[] {
  const starts: number[] = [];
  let braceDepth = 0;
  for (let index = 0; index < value.length;) {
    const character = value[index];
    if (character === "%" && !isEscaped(value, index)) {
      const newline = value.indexOf("\n", index);
      index = newline === -1 ? value.length : newline + 1;
      continue;
    }
    if (character === "{" && !isEscaped(value, index)) {
      braceDepth += 1;
      index += 1;
      continue;
    }
    if (character === "}" && !isEscaped(value, index)) {
      braceDepth = Math.max(0, braceDepth - 1);
      index += 1;
      continue;
    }
    if (character !== "\\" || braceDepth !== 0) {
      index += 1;
      continue;
    }
    const command = parseCommand(value, index);
    if (command === null) {
      index += 1;
      continue;
    }
    if (command.name === name) starts.push(index);
    index = command.nameEnd;
  }
  return starts;
}

function parseAbstract(source: string, start: number, bodyStart: number, bodyEnd: number, coreEnd: number, end: number): BlockDraft {
  const body = source.slice(bodyStart, bodyEnd);
  const bounds = whitespaceBounds(body);
  const core = body.slice(bounds.start, bounds.end);
  const paragraphs = splitVisualParagraphs(core);
  if (paragraphs === null) return rawDraft(source, start, end, "abstract");
  return {
    start,
    end,
    kind: "abstract",
    node: {
      type: "blockquote",
      content: paragraphs.map((content) => ({ type: "paragraph", content })),
    },
    metadata: {
      type: "abstract",
      opening: source.slice(start, bodyStart),
      closing: source.slice(bodyEnd, coreEnd),
      innerPrefix: body.slice(0, bounds.start),
      innerSuffix: body.slice(bounds.end),
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseDisplayEquation(source: string, start: number, coreEnd: number, end: number): BlockDraft {
  const bodyStart = start + 2;
  const bodyEnd = coreEnd - 2;
  return equationDraft(source, start, bodyStart, bodyEnd, coreEnd, end, "\\[", "\\]", false);
}

function parseEquationEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  environment: string,
): BlockDraft {
  return equationDraft(
    source,
    start,
    bodyStart,
    bodyEnd,
    coreEnd,
    end,
    source.slice(start, bodyStart),
    source.slice(bodyEnd, coreEnd),
    environment === "equation",
  );
}

function equationDraft(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  opening: string,
  closing: string,
  numbered: boolean,
): BlockDraft {
  const body = source.slice(bodyStart, bodyEnd);
  const labels = [...body.matchAll(/\\label\s*\{([^{}]+)\}/gu)];
  if (labels.length > 1) return rawDraft(source, start, end, "equation");
  const label = labels[0]?.[1]?.trim() ?? "";
  const withoutLabel = body.replace(/\\label\s*\{[^{}]+\}/u, "");
  const bounds = whitespaceBounds(withoutLabel);
  const latex = withoutLabel.slice(bounds.start, bounds.end);
  if (!isSafeMath(latex) || !isSafeLabel(label)) return rawDraft(source, start, end, "equation");
  return {
    start,
    end,
    kind: "equation",
    node: { type: "equation", attrs: { latex, numbered, label } },
    metadata: {
      type: "equation",
      opening,
      closing,
      innerPrefix: withoutLabel.slice(0, bounds.start),
      innerSuffix: withoutLabel.slice(bounds.end),
      numbered,
      originalBody: body,
      originalLatex: latex,
      originalLabel: label,
      labelToken: labels[0]?.[0] ?? "",
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseFigureEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  environment: string,
): BlockDraft {
  const opening = source.slice(start, bodyStart);
  const body = source.slice(bodyStart, bodyEnd);
  let cursor = skipWhitespace(body, 0);
  const placementGroup = parseOptionalGroup(body, cursor);
  const placement = placementGroup?.raw ?? "";
  if (placementGroup !== null) cursor = skipWhitespace(body, placementGroup.end);
  let centered = false;
  if (body.startsWith("\\centering", cursor)) {
    centered = true;
    cursor = skipWhitespace(body, cursor + "\\centering".length);
  }
  const include = parseNamedCommandWithArgument(body, cursor, "includegraphics", true);
  if (include === null) return rawDraft(source, start, end, environment);
  cursor = skipWhitespace(body, include.end);
  const caption = parseNamedCommandWithArgument(body, cursor, "caption", false);
  if (caption === null) return rawDraft(source, start, end, environment);
  const captionText = decodePlainLatex(caption.argument.value);
  if (captionText === null) return rawDraft(source, start, end, environment);
  cursor = skipWhitespace(body, caption.end);
  const labelCommand = parseNamedCommandWithArgument(body, cursor, "label", false);
  const label = labelCommand?.argument.value.trim() ?? "";
  if (labelCommand !== null) cursor = skipWhitespace(body, labelCommand.end);
  if (cursor !== body.length || !isSafeLabel(label) || !isSafeAssetPath(include.argument.value)) {
    return rawDraft(source, start, end, environment);
  }
  return {
    start,
    end,
    kind: "figure",
    node: { type: "figure", attrs: { file: include.argument.value, caption: captionText, label } },
    metadata: {
      type: "figure",
      opening,
      closing: source.slice(bodyEnd, coreEnd),
      placement,
      graphicsOptions: include.optional?.raw ?? "",
      centered,
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseTableEnvironment(
  source: string,
  start: number,
  bodyStart: number,
  bodyEnd: number,
  coreEnd: number,
  end: number,
  environment: string,
): BlockDraft {
  const body = source.slice(bodyStart, bodyEnd);
  let cursor = skipWhitespace(body, 0);
  const placementGroup = parseOptionalGroup(body, cursor);
  const placement = placementGroup?.raw ?? "";
  if (placementGroup !== null) cursor = skipWhitespace(body, placementGroup.end);
  let centered = false;
  if (body.startsWith("\\centering", cursor)) {
    centered = true;
    cursor = skipWhitespace(body, cursor + "\\centering".length);
  }
  const caption = parseNamedCommandWithArgument(body, cursor, "caption", false);
  if (caption === null) return rawDraft(source, start, end, environment);
  const captionText = decodePlainLatex(caption.argument.value);
  if (captionText === null) return rawDraft(source, start, end, environment);
  cursor = skipWhitespace(body, caption.end);
  const labelCommand = parseNamedCommandWithArgument(body, cursor, "label", false);
  const label = labelCommand?.argument.value.trim() ?? "";
  if (labelCommand !== null) cursor = skipWhitespace(body, labelCommand.end);
  const tabularBegin = parseEnvironmentOpening(body, cursor, "tabular");
  if (tabularBegin === null) return rawDraft(source, start, end, environment);
  const alignmentGroup = parseRequiredGroup(body, tabularBegin.end);
  if (alignmentGroup === null) return rawDraft(source, start, end, environment);
  const alignment = alignmentGroup.value.replace(/\s/gu, "");
  if (!/^[lcr]+$/u.test(alignment)) return rawDraft(source, start, end, environment);
  const tabularEnd = findEnvironmentEnd(body, alignmentGroup.end, "tabular");
  if (tabularEnd === null || skipWhitespace(body, tabularEnd.end) !== body.length) return rawDraft(source, start, end, environment);
  const parsedRows = parseBooktabsRows(body.slice(alignmentGroup.end, tabularEnd.start), alignment.length);
  if (parsedRows === null || !isSafeLabel(label)) return rawDraft(source, start, end, environment);
  return {
    start,
    end,
    kind: "table",
    node: {
      type: "scientificTable",
      attrs: {
        caption: captionText,
        label,
        alignment,
        columns: parsedRows.header.join("|"),
        rows: parsedRows.rows.map((row) => row.join("|")).join("\n"),
      },
    },
    metadata: {
      type: "table",
      opening: source.slice(start, bodyStart),
      closing: source.slice(bodyEnd, coreEnd),
      placement,
      alignment,
      centered,
      suffix: source.slice(coreEnd, end),
    },
  };
}

function parseBooktabsRows(body: string, columnCount: number): { header: string[]; rows: string[][] } | null {
  const normalized = body.trim()
    .replace(/^\\toprule\s*/u, "")
    .replace(/\s*\\bottomrule$/u, "");
  const midruleParts = normalized.split(/\\midrule/u);
  if (midruleParts.length !== 2) return null;
  const headerRows = splitLatexRows(midruleParts[0] ?? "");
  const dataRows = splitLatexRows(midruleParts[1] ?? "");
  if (headerRows.length !== 1 || dataRows.length === 0) return null;
  const allRows = [...headerRows, ...dataRows].map((row) => row.split("&").map((cell) => decodePlainLatex(cell.trim())));
  if (allRows.some((row) => row.length !== columnCount || row.some((cell) => cell === null))) return null;
  const values = allRows.map((row) => row as string[]);
  return { header: values[0] ?? [], rows: values.slice(1) };
}

function splitLatexRows(value: string): string[] {
  return value.split(/\\\\(?:\[[^\]]*\])?/u).map((row) => row.trim()).filter(Boolean);
}

function serializeOwnedBlock(node: JSONContent, owner: SourceOwnership, newline: string): string {
  if (node.type === "rawBlock") return String(node.attrs?.source ?? "");
  const metadata = owner.metadata;
  if (metadata.type === "paragraph") {
    return `${metadata.prefix}${serializeBlockCore(node, newline)}${metadata.suffix}`;
  }
  if (metadata.type === "command" && (node.type === "heading" || node.type === "paragraph")) {
    return `${metadata.opening}${serializeInline(node.content ?? [])}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "abstract" && node.type === "blockquote") {
    const body = serializeParagraphChildren(node, newline + newline);
    return `${metadata.opening}${metadata.innerPrefix}${body}${metadata.innerSuffix}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "quote" && node.type === "blockquote") {
    const body = serializeParagraphChildren(node, newline + newline);
    return `${metadata.opening}${metadata.innerPrefix}${body}${metadata.innerSuffix}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "statement" && node.type === "scientificStatement") {
    const kind = String(node.attrs?.kind ?? "");
    const expectedKind = metadata.environment.startsWith("theorem")
      ? "theorem"
      : metadata.environment.startsWith("definition") ? "definition" : "proof";
    const expectedNumbered = expectedKind !== "proof" && !metadata.environment.endsWith("*");
    if (kind !== expectedKind || Boolean(node.attrs?.numbered) !== expectedNumbered) {
      throw new Error("Changing a theorem environment kind or numbering is source-only.");
    }
    const title = String(node.attrs?.title ?? "");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeLabel(label) || !isSafeOptionalText(title)) throw new Error("The statement title or label is not safe LaTeX and was not written.");
    const openingExtra = title === metadata.originalTitle
      ? metadata.originalOpeningExtra
      : title === "" ? metadata.leadingBeforeTitle : `${metadata.leadingBeforeTitle}[${escapeLatexText(title)}]`;
    let innerPrefix = metadata.innerPrefix;
    let innerSuffix = metadata.innerSuffix;
    if (label !== metadata.originalLabel) {
      const labelSource = label === "" ? "" : `\\label{${label}}`;
      if (metadata.labelPlacement === "prefix") innerPrefix = replaceOnce(innerPrefix, metadata.labelToken, labelSource);
      else if (metadata.labelPlacement === "suffix") innerSuffix = replaceOnce(innerSuffix, metadata.labelToken, labelSource);
      else if (label !== "") innerPrefix = `${innerPrefix}${labelSource}${newline}`;
    }
    const body = serializeParagraphChildren(node, newline + newline);
    return `${metadata.openingBase}${openingExtra}${innerPrefix}${body}${innerSuffix}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "listing" && node.type === "listingBlock") {
    const language = String(node.attrs?.language ?? "");
    const caption = String(node.attrs?.caption ?? "");
    const label = String(node.attrs?.label ?? "");
    if (language !== metadata.originalLanguage || caption !== metadata.originalCaption || label !== metadata.originalLabel) {
      throw new Error("Editing listings options is source-only; the code body remains visually editable.");
    }
    const body = plainTextContent(node);
    if (!isSafeListingBody(body)) throw new Error("The code contains a TeX listing terminator and was not written.");
    return `${metadata.opening}${body}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "list" && (node.type === "bulletList" || node.type === "orderedList")) {
    const expectedType = metadata.environment === "itemize" ? "bulletList" : "orderedList";
    if (node.type !== expectedType) throw new Error("Changing a list environment kind is source-only.");
    if (node.type === "orderedList" && Number(node.attrs?.start ?? 1) !== 1) {
      throw new Error("Only lists beginning at one are in the safe visual subset.");
    }
    const items = node.content ?? [];
    if (items.length !== metadata.itemPrefixes.length || items.length !== metadata.itemSuffixes.length) {
      const trailing = metadata.itemSuffixes.at(-1) ?? newline;
      const body = items.map((item, index) => `${index === 0 ? "\\item " : `${newline}\\item `}${serializeListItem(item, newline)}`).join("");
      return `${metadata.opening}${body}${trailing}${metadata.closing}${metadata.suffix}`;
    }
    const body = items.map((item, index) => `${metadata.itemPrefixes[index] ?? "\\item "}${serializeListItem(item, newline)}${metadata.itemSuffixes[index] ?? ""}`).join("");
    return `${metadata.opening}${body}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "equation" && node.type === "equation") {
    const latex = String(node.attrs?.latex ?? "");
    if (!isSafeMath(latex)) throw new Error("This equation left Setwright's safe math subset and was not written.");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeLabel(label)) throw new Error("The equation label is not safe LaTeX and was not written.");
    const numbered = Boolean(node.attrs?.numbered);
    const opening = numbered === metadata.numbered ? metadata.opening : `\\begin{${numbered ? "equation" : "equation*"}}`;
    const closing = numbered === metadata.numbered ? metadata.closing : `\\end{${numbered ? "equation" : "equation*"}}`;
    if (label === metadata.originalLabel) {
      const body = replaceOnce(metadata.originalBody, metadata.originalLatex, latex);
      return `${opening}${body}${closing}${metadata.suffix}`;
    }
    const labelSource = label === "" ? "" : `${newline}\\label{${label}}`;
    return `${opening}${metadata.innerPrefix}${latex}${labelSource}${metadata.innerSuffix}${closing}${metadata.suffix}`;
  }
  if (metadata.type === "figure" && node.type === "figure") {
    const file = String(node.attrs?.file ?? "");
    const caption = String(node.attrs?.caption ?? "");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeAssetPath(file) || !isSafeLabel(label)) throw new Error("The figure path or label is outside the safe visual subset.");
    const centered = metadata.centered ? `\\centering${newline}` : "";
    const labelSource = label === "" ? "" : `${newline}\\label{${label}}`;
    return `${metadata.opening}${metadata.placement}${newline}${centered}\\includegraphics${metadata.graphicsOptions}{${file}}${newline}\\caption{${escapeLatexText(caption)}}${labelSource}${newline}${metadata.closing}${metadata.suffix}`;
  }
  if (metadata.type === "table" && node.type === "scientificTable") {
    return `${serializeTable(node, metadata.alignment, metadata.centered, newline, `${metadata.opening}${metadata.placement}`, metadata.closing)}${metadata.suffix}`;
  }
  return `${serializeBlockCore(node, newline)}${metadata.type === "raw" ? "" : metadata.suffix}`;
}

function serializeNewBlock(node: JSONContent, newline: string): string {
  return serializeBlockCore(node, newline);
}

function serializeBlockCore(node: JSONContent, newline: string): string {
  if (node.type === "paragraph") return serializeInline(node.content ?? []);
  if (node.type === "heading") {
    const level = Number(node.attrs?.level ?? 2);
    const command = level <= 1 ? "section" : level === 2 ? "section" : "subsection";
    return `\\${command}{${serializeInline(node.content ?? [])}}`;
  }
  if (node.type === "blockquote") {
    return `\\begin{quote}${newline}${serializeParagraphChildren(node, newline + newline)}${newline}\\end{quote}`;
  }
  if (node.type === "equation") {
    const latex = String(node.attrs?.latex ?? "");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeMath(latex) || !isSafeLabel(label)) throw new Error("The equation is outside Setwright's safe visual subset.");
    const numbered = Boolean(node.attrs?.numbered);
    const environment = numbered ? "equation" : "equation*";
    const labelSource = label === "" ? "" : `${newline}\\label{${label}}`;
    return `\\begin{${environment}}${newline}${latex}${labelSource}${newline}\\end{${environment}}`;
  }
  if (node.type === "figure") {
    const file = String(node.attrs?.file ?? "");
    const caption = String(node.attrs?.caption ?? "");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeAssetPath(file) || !isSafeLabel(label)) throw new Error("The figure is outside Setwright's safe visual subset.");
    const labelSource = label === "" ? "" : `${newline}\\label{${label}}`;
    return `\\begin{figure}${newline}\\centering${newline}\\includegraphics{${file}}${newline}\\caption{${escapeLatexText(caption)}}${labelSource}${newline}\\end{figure}`;
  }
  if (node.type === "scientificTable") return serializeTable(node, String(node.attrs?.alignment ?? ""), true, newline, "\\begin{table}", "\\end{table}");
  if (node.type === "scientificStatement") {
    const kind = String(node.attrs?.kind ?? "");
    if (kind !== "theorem" && kind !== "definition" && kind !== "proof") {
      throw new Error("This scientific statement kind cannot be serialized safely.");
    }
    const title = String(node.attrs?.title ?? "");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeLabel(label) || !isSafeOptionalText(title)) throw new Error("The statement title or label is not safe LaTeX and was not written.");
    const numbered = Boolean(node.attrs?.numbered);
    if (kind === "proof" && numbered) throw new Error("Proof environments cannot be numbered visually.");
    const environment = kind === "proof" ? "proof" : `${kind}${numbered ? "" : "*"}`;
    const titleSource = title === "" ? "" : `[${escapeLatexText(title)}]`;
    const labelSource = label === "" ? "" : `${newline}\\label{${label}}`;
    const body = serializeParagraphChildren(node, newline + newline);
    return `\\begin{${environment}}${titleSource}${labelSource}${newline}${body}${newline}\\end{${environment}}`;
  }
  if (node.type === "listingBlock") {
    const language = String(node.attrs?.language ?? "");
    const caption = String(node.attrs?.caption ?? "");
    const label = String(node.attrs?.label ?? "");
    if (!isSafeListingLanguage(language) || !isSafeLabel(label)) throw new Error("The code listing options are outside the safe visual subset.");
    const options = [
      ...(language === "" ? [] : [`language={${language}}`]),
      ...(caption === "" ? [] : [`caption={${escapeLatexText(caption)}}`]),
      ...(label === "" ? [] : [`label={${label}}`]),
    ];
    const body = plainTextContent(node);
    if (!isSafeListingBody(body)) throw new Error("The code contains a TeX listing terminator and was not written.");
    const opening = `\\begin{lstlisting}${options.length === 0 ? "" : `[${options.join(",")}]`}`;
    return `${opening}${newline}${ensureTrailingNewline(body, newline)}\\end{lstlisting}`;
  }
  if (node.type === "rawBlock") return String(node.attrs?.source ?? "");
  if (node.type === "codeBlock") {
    const body = plainTextContent(node);
    if (!isSafeListingBody(body)) throw new Error("The code contains a TeX listing terminator and was not written.");
    return `\\begin{lstlisting}${newline}${ensureTrailingNewline(body, newline)}\\end{lstlisting}`;
  }
  if (node.type === "bulletList" || node.type === "orderedList") {
    if (node.type === "orderedList" && Number(node.attrs?.start ?? 1) !== 1) {
      throw new Error("Only lists beginning at one are in the safe visual subset.");
    }
    const environment = node.type === "bulletList" ? "itemize" : "enumerate";
    const items = (node.content ?? []).map((item) => `\\item ${serializeListItem(item, newline)}`).join(newline);
    return `\\begin{${environment}}${newline}${items}${newline}\\end{${environment}}`;
  }
  throw new Error(`The ${node.type ?? "unknown"} block cannot be serialized safely and was not written.`);
}

function serializeTable(node: JSONContent, requestedAlignment: string, centered: boolean, newline: string, opening: string, closing: string): string {
  const header = String(node.attrs?.columns ?? "").split("|");
  const rows = String(node.attrs?.rows ?? "").split("\n").filter((row) => row !== "").map((row) => row.split("|"));
  if (header.length === 0 || rows.some((row) => row.length !== header.length)) {
    throw new Error("Only rectangular tables can be serialized visually.");
  }
  const alignment = requestedAlignment.length === header.length && /^[lcr]+$/u.test(requestedAlignment)
    ? requestedAlignment
    : "l".repeat(header.length);
  const caption = escapeLatexText(String(node.attrs?.caption ?? ""));
  const label = String(node.attrs?.label ?? "");
  if (!isSafeLabel(label)) throw new Error("The table label is not safe LaTeX and was not written.");
  const centeredSource = centered ? `\\centering${newline}` : "";
  const labelSource = label === "" ? "" : `${newline}\\label{${label}}`;
  const headerSource = header.map(escapeLatexText).join(" & ");
  const rowsSource = rows.map((row) => `${row.map(escapeLatexText).join(" & ")} \\\\`).join(newline);
  return `${opening}${newline}${centeredSource}\\caption{${caption}}${labelSource}${newline}\\begin{tabular}{${alignment}}${newline}\\toprule${newline}${headerSource} \\\\${newline}\\midrule${newline}${rowsSource}${newline}\\bottomrule${newline}\\end{tabular}${newline}${closing}`;
}

function serializeParagraphChildren(node: JSONContent, separator: string): string {
  return (node.content ?? []).map((child) => child.type === "paragraph" ? serializeInline(child.content ?? []) : serializeBlockCore(child, "\n")).join(separator);
}

function serializeListItem(item: JSONContent, newline: string): string {
  if (item.type !== "listItem") throw new Error("Only ordinary list items are in the safe visual subset.");
  return serializeParagraphChildren(item, newline + newline);
}

function serializeInline(content: readonly JSONContent[]): string {
  return content.map((node) => {
    if (node.type === "text") {
      let value = escapeLatexText(node.text ?? "");
      for (const mark of node.marks ?? []) {
        if (mark.type === "bold") value = `\\textbf{${value}}`;
        else if (mark.type === "italic") {
          const command = mark.attrs?.latexCommand === "emph" ? "emph" : "textit";
          value = `\\${command}{${value}}`;
        }
        else if (mark.type === "underline") value = `\\underline{${value}}`;
        else if (mark.type === "code") value = `\\texttt{${value}}`;
        else if (mark.type === "link") {
          const href = String(mark.attrs?.href ?? "");
          if (!isSafeUrl(href)) throw new Error("The link target is not safe LaTeX and was not written.");
          value = `\\href{${href}}{${value}}`;
        } else {
          throw new Error(`The ${mark.type} mark cannot be serialized safely.`);
        }
      }
      return value;
    }
    if (node.type === "citation") {
      const command = String(node.attrs?.command ?? "cite");
      const keys = String(node.attrs?.keys ?? "");
      if (!CITATION_COMMANDS.has(command) || !isSafeCitationKeys(keys)) throw new Error("The citation is outside Setwright's safe subset.");
      return `\\${command}{${keys}}`;
    }
    if (node.type === "hardBreak") return node.attrs?.latexCommand === "and" ? "\\and" : "\\\\";
    throw new Error(`The ${node.type ?? "unknown"} inline node cannot be serialized safely.`);
  }).join("");
}

function parseInlineLatex(value: string): JSONContent[] | null {
  const nodes: JSONContent[] = [];
  let text = "";
  const flushText = () => {
    if (text === "") return;
    appendInlineNode(nodes, { type: "text", text });
    text = "";
  };

  for (let index = 0; index < value.length;) {
    const character = value[index];
    if (character === "\r" || character === "\n" || character === "\t") {
      text += " ";
      index += character === "\r" && value[index + 1] === "\n" ? 2 : 1;
      continue;
    }
    if (character === "~") {
      text += "\u00a0";
      index += 1;
      continue;
    }
    if (["%", "$", "&", "#", "_", "^", "{", "}"].includes(character ?? "")) return null;
    if (character !== "\\") {
      text += character;
      index += 1;
      continue;
    }

    const escaped = value[index + 1];
    if (escaped !== undefined && "%$&#_{}".includes(escaped)) {
      text += escaped;
      index += 2;
      continue;
    }
    if (escaped === " ") {
      text += " ";
      index += 2;
      continue;
    }
    const command = parseCommand(value, index);
    if (command === null) return null;
    const markName = INLINE_MARK_COMMANDS.get(command.name);
    if (markName !== undefined) {
      const argument = parseRequiredGroup(value, command.nameEnd);
      if (argument === null) return null;
      const nested = parseInlineLatex(argument.value);
      if (
        nested === null
        || nested.some((nestedNode) => nestedNode.type !== "text")
        || nested.some((nestedNode) => nestedNode.marks?.some((mark) => mark.type === markName))
      ) return null;
      flushText();
      const mark = markName === "italic"
        ? { type: markName, attrs: { latexCommand: command.name } }
        : { type: markName };
      for (const nestedNode of nested) appendInlineNode(nodes, addMark(nestedNode, mark));
      index = argument.end;
      continue;
    }
    if (command.name === "href") {
      const href = parseRequiredGroup(value, command.nameEnd);
      const label = href === null ? null : parseRequiredGroup(value, href.end);
      if (href === null || label === null || !isSafeUrl(href.value)) return null;
      const nested = parseInlineLatex(label.value);
      if (nested === null || nested.some((nestedNode) => nestedNode.type !== "text")) return null;
      flushText();
      for (const nestedNode of nested) appendInlineNode(nodes, addMark(nestedNode, { type: "link", attrs: { href: href.value } }));
      index = label.end;
      continue;
    }
    if (CITATION_COMMANDS.has(command.name)) {
      const argument = parseRequiredGroup(value, command.nameEnd);
      if (argument === null || !isSafeCitationKeys(argument.value)) return null;
      flushText();
      appendInlineNode(nodes, {
        type: "citation",
        attrs: { keys: argument.value.trim(), label: argument.value.trim(), command: command.name },
      });
      index = argument.end;
      continue;
    }
    if (command.name === "and" || command.name === "\\") {
      flushText();
      appendInlineNode(nodes, { type: "hardBreak", attrs: { latexCommand: command.name } });
      index = command.nameEnd;
      continue;
    }
    if (["textbackslash", "textasciitilde", "textasciicircum"].includes(command.name)) {
      const emptyArgument = parseRequiredGroup(value, command.nameEnd);
      if (emptyArgument === null || emptyArgument.value !== "") return null;
      text += command.name === "textbackslash" ? "\\" : command.name === "textasciitilde" ? "~" : "^";
      index = emptyArgument.end;
      continue;
    }
    return null;
  }
  flushText();
  return nodes;
}

function splitVisualParagraphs(value: string): JSONContent[][] | null {
  if (value === "") return [[]];
  const parts = value.split(/(?:\r?\n[\t ]*){2,}/u);
  const result: JSONContent[][] = [];
  for (const part of parts) {
    const inline = parseInlineLatex(part.trim());
    if (inline === null) return null;
    result.push(inline);
  }
  return result;
}

function semanticFingerprint(node: JSONContent): string {
  if (node.type === "paragraph" || node.type === "heading") {
    return JSON.stringify([node.type, node.type === "heading" ? Number(node.attrs?.level ?? 2) : null, serializeInline(node.content ?? [])]);
  }
  if (node.type === "blockquote") {
    return JSON.stringify([node.type, (node.content ?? []).map((child) => serializeInline(child.content ?? []))]);
  }
  if (node.type === "scientificStatement") {
    return JSON.stringify([
      node.type,
      String(node.attrs?.kind ?? ""),
      String(node.attrs?.title ?? ""),
      String(node.attrs?.label ?? ""),
      Boolean(node.attrs?.numbered),
      (node.content ?? []).map((child) => serializeInline(child.content ?? [])),
    ]);
  }
  if (node.type === "listingBlock") {
    return JSON.stringify([
      node.type,
      String(node.attrs?.language ?? ""),
      String(node.attrs?.caption ?? ""),
      String(node.attrs?.label ?? ""),
      plainTextContent(node),
    ]);
  }
  if (node.type === "bulletList" || node.type === "orderedList") {
    return JSON.stringify([node.type, Number(node.attrs?.start ?? 1), (node.content ?? []).map((item) => serializeListItem(item, "\n"))]);
  }
  if (node.type === "equation") {
    return JSON.stringify([node.type, String(node.attrs?.latex ?? ""), Boolean(node.attrs?.numbered), String(node.attrs?.label ?? "")]);
  }
  if (node.type === "figure") {
    return JSON.stringify([node.type, String(node.attrs?.file ?? ""), String(node.attrs?.caption ?? ""), String(node.attrs?.label ?? "")]);
  }
  if (node.type === "scientificTable") {
    return JSON.stringify([node.type, String(node.attrs?.columns ?? ""), String(node.attrs?.rows ?? ""), String(node.attrs?.caption ?? ""), String(node.attrs?.label ?? ""), String(node.attrs?.alignment ?? "")]);
  }
  if (node.type === "rawBlock") return JSON.stringify([node.type, String(node.attrs?.source ?? "")]);
  return JSON.stringify(stripSourceAttributes(node));
}

function stripSourceAttributes(node: JSONContent): JSONContent {
  const attrs = Object.fromEntries(Object.entries(node.attrs ?? {}).filter(([key]) => !SOURCE_ATTRIBUTE_NAMES.includes(key as typeof SOURCE_ATTRIBUTE_NAMES[number])));
  return {
    ...node,
    ...(Object.keys(attrs).length === 0 ? { attrs: undefined } : { attrs }),
    ...(node.content === undefined ? {} : { content: node.content.map(stripSourceAttributes) }),
  };
}

function withOwnershipAttributes(node: JSONContent, ownerId: string, fileId: string, startByte: number, endByte: number, kind: SourceOwnershipKind): JSONContent {
  return {
    ...node,
    attrs: {
      ...node.attrs,
      sourceOwnerId: ownerId,
      sourceFileId: fileId,
      sourceStartByte: startByte,
      sourceEndByte: endByte,
      sourceKind: kind,
    },
  };
}

function sourceAttribute(node: JSONContent, name: "sourceOwnerId" | "sourceFileId"): string | null {
  const value: unknown = node.attrs?.[name];
  return typeof value === "string" && value !== "" ? value : null;
}

function rawDraft(source: string, start: number, end: number, environment: string): BlockDraft {
  return {
    start,
    end,
    kind: "raw",
    node: { type: "rawBlock", attrs: { source: source.slice(start, end), environment } },
    metadata: { type: "raw" },
  };
}

function mergeAdjacentRawDrafts(drafts: readonly BlockDraft[], source: string): BlockDraft[] {
  const merged: BlockDraft[] = [];
  for (const draft of drafts) {
    const previous = merged.at(-1);
    if (previous?.kind === "raw" && draft.kind === "raw" && previous.end === draft.start) {
      previous.end = draft.end;
      previous.node = { type: "rawBlock", attrs: { source: source.slice(previous.start, draft.end), environment: "source" } };
    } else {
      merged.push({ ...draft });
    }
  }
  return merged;
}

function parseCommand(source: string, start: number): ParsedCommand | null {
  if (source[start] !== "\\") return null;
  const first = source[start + 1];
  if (first === undefined) return null;
  if (!/[A-Za-z@]/u.test(first)) return { name: first, nameEnd: start + 2 };
  let end = start + 2;
  while (end < source.length && /[A-Za-z@]/u.test(source[end] ?? "")) end += 1;
  return { name: source.slice(start + 1, end), nameEnd: end };
}

function parseRequiredGroup(source: string, start: number): ParsedGroup | null {
  const opening = skipWhitespace(source, start);
  if (source[opening] !== "{") return null;
  return parseBalancedGroup(source, opening, "{", "}");
}

function parseOptionalGroup(source: string, start: number): ParsedGroup | null {
  const opening = skipWhitespace(source, start);
  if (source[opening] !== "[") return null;
  return parseBalancedGroup(source, opening, "[", "]");
}

function parseBalancedGroup(source: string, opening: number, open: string, close: string): ParsedGroup | null {
  let depth = 1;
  for (let index = opening + 1; index < source.length; index += 1) {
    if (source[index] === "%" && !isEscaped(source, index)) {
      const newlineIndex = source.indexOf("\n", index);
      if (newlineIndex === -1) return null;
      index = newlineIndex;
      continue;
    }
    if (source[index] === open && !isEscaped(source, index)) depth += 1;
    else if (source[index] === close && !isEscaped(source, index)) {
      depth -= 1;
      if (depth === 0) {
        return {
          value: source.slice(opening + 1, index),
          start: opening,
          end: index + 1,
          raw: source.slice(opening, index + 1),
        };
      }
    }
  }
  return null;
}

function parseNamedCommandWithArgument(source: string, start: number, name: string, optionalAllowed: boolean): { argument: ParsedGroup; optional: ParsedGroup | null; end: number } | null {
  const cursor = skipWhitespace(source, start);
  const command = parseCommand(source, cursor);
  if (command?.name !== name) return null;
  const optional = optionalAllowed ? parseOptionalGroup(source, command.nameEnd) : null;
  const argument = parseRequiredGroup(source, optional?.end ?? command.nameEnd);
  if (argument === null) return null;
  return { argument, optional, end: argument.end };
}

function parseEnvironmentOpening(source: string, start: number, environment: string): { end: number } | null {
  const cursor = skipWhitespace(source, start);
  const command = parseCommand(source, cursor);
  if (command?.name !== "begin") return null;
  const group = parseRequiredGroup(source, command.nameEnd);
  return group?.value.trim() === environment ? { end: group.end } : null;
}

function findEnvironmentEnd(source: string, start: number, environment: string): { start: number; end: number } | null {
  let depth = 1;
  for (let index = start; index < source.length;) {
    if (source[index] === "%" && !isEscaped(source, index)) {
      const newlineIndex = source.indexOf("\n", index);
      index = newlineIndex === -1 ? source.length : newlineIndex + 1;
      continue;
    }
    if (source[index] !== "\\") {
      index += 1;
      continue;
    }
    const command = parseCommand(source, index);
    if (command === null || (command.name !== "begin" && command.name !== "end")) {
      index += 2;
      continue;
    }
    const group = parseRequiredGroup(source, command.nameEnd);
    if (group === null || group.value.trim() !== environment) {
      index = command.nameEnd;
      continue;
    }
    if (command.name === "begin") depth += 1;
    else depth -= 1;
    if (depth === 0) return { start: index, end: group.end };
    index = group.end;
  }
  return null;
}

function findUnescapedToken(source: string, token: string, start: number): number {
  let index = source.indexOf(token, start);
  while (index !== -1 && isEscaped(source, index)) index = source.indexOf(token, index + token.length);
  return index;
}

function appendInlineNode(nodes: JSONContent[], node: JSONContent): void {
  const previous = nodes.at(-1);
  if (previous?.type === "text" && node.type === "text" && JSON.stringify(previous.marks ?? []) === JSON.stringify(node.marks ?? [])) {
    previous.text = `${previous.text ?? ""}${node.text ?? ""}`;
    return;
  }
  nodes.push(node);
}

function addMark(node: JSONContent, mark: NonNullable<JSONContent["marks"]>[number]): JSONContent {
  if (node.type !== "text") return node;
  return { ...node, marks: [...(node.marks ?? []), mark] };
}

function decodePlainLatex(value: string): string | null {
  const nodes = parseInlineLatex(value);
  if (nodes === null || nodes.some((node) => node.type !== "text" || (node.marks?.length ?? 0) > 0)) return null;
  return nodes.map((node) => node.text ?? "").join("");
}

function escapeLatexText(value: string): string {
  return value.replace(/[\\%$&#_{}~^\u00a0]/gu, (character) => {
    if (character === "\\") return "\\textbackslash{}";
    if (character === "~") return "\\textasciitilde{}";
    if (character === "\u00a0") return "~";
    if (character === "^") return "\\textasciicircum{}";
    return `\\${character}`;
  });
}

function preservesTextualTrivia(kind: SourceOwnershipKind): boolean {
  return kind === "paragraph";
}

function preserveEquivalentWhitespace(original: string, replacement: string): string {
  const originalRuns = [...original.matchAll(/[\t\n\r ]+/gu)].map((match) => match[0]);
  const replacementRuns = [...replacement.matchAll(/[\t\n\r ]+/gu)];
  if (originalRuns.length !== replacementRuns.length) return replacement;
  let cursor = 0;
  let preserved = "";
  replacementRuns.forEach((match, index) => {
    const at = match.index;
    preserved += replacement.slice(cursor, at);
    const originalRun = originalRuns[index] ?? match[0];
    preserved += match[0] === " " && /[\t\n\r]/u.test(originalRun) ? originalRun : match[0];
    cursor = at + match[0].length;
  });
  return `${preserved}${replacement.slice(cursor)}`;
}

function minimalOwnedChange(owner: SourceOwnership, replacement: string): VisualSourceChange {
  const beforePoints = Array.from(owner.original);
  const afterPoints = Array.from(replacement);
  let prefix = 0;
  while (prefix < beforePoints.length && prefix < afterPoints.length && beforePoints[prefix] === afterPoints[prefix]) prefix += 1;
  let suffix = 0;
  while (
    suffix < beforePoints.length - prefix
    && suffix < afterPoints.length - prefix
    && beforePoints[beforePoints.length - 1 - suffix] === afterPoints[afterPoints.length - 1 - suffix]
  ) suffix += 1;
  const unchangedPrefix = beforePoints.slice(0, prefix).join("");
  const oldMiddle = beforePoints.slice(prefix, beforePoints.length - suffix).join("");
  const newMiddle = afterPoints.slice(prefix, afterPoints.length - suffix).join("");
  const startByte = owner.startByte + utf8Length(unchangedPrefix);
  return {
    ownerId: owner.ownerId,
    fileId: owner.fileId,
    startByte,
    endByte: startByte + utf8Length(oldMiddle),
    replacement: newMiddle,
  };
}

function isSafeMath(value: string): boolean {
  if (value.trim() === "" || value.includes("%") || containsTexCaretEscape(value)) return false;
  let braceDepth = 0;
  for (let index = 0; index < value.length;) {
    const character = value[index];
    if (character === "{" && !isEscaped(value, index)) braceDepth += 1;
    else if (character === "}" && !isEscaped(value, index)) {
      braceDepth -= 1;
      if (braceDepth < 0) return false;
    } else if (character === "\\") {
      const command = parseCommand(value, index);
      if (command === null) return false;
      if (command.name.length === 1 && ",;! \\|:{}[]()".includes(command.name)) {
        index = command.nameEnd;
        continue;
      }
      if (!SAFE_MATH_COMMANDS.has(command.name)) return false;
      if (command.name === "begin" || command.name === "end") {
        const group = parseRequiredGroup(value, command.nameEnd);
        if (group === null || !SAFE_MATH_ENVIRONMENTS.has(group.value.trim())) return false;
        index = group.end;
        continue;
      }
      index = command.nameEnd;
      continue;
    }
    index += 1;
  }
  return braceDepth === 0 && !/\\(?:def|gdef|edef|xdef|newcommand|renewcommand|input|include|write|catcode|csname|usepackage|documentclass|special|directlua|openin|read)\b/u.test(value);
}

function isSafeCitationKeys(value: string): boolean {
  return !containsTexCaretEscape(value) && /^[A-Za-z0-9_.:+/-]+(?:\s*,\s*[A-Za-z0-9_.:+/-]+)*$/u.test(value.trim());
}

function isSafeLabel(value: string): boolean {
  return !containsTexCaretEscape(value) && (value === "" || /^[A-Za-z0-9_.:+/-]+$/u.test(value));
}

function isSafeListingLanguage(value: string): boolean {
  return !containsTexCaretEscape(value) && (value === "" || /^[A-Za-z0-9_+.#-]+$/u.test(value));
}

function isSafeAssetPath(value: string): boolean {
  if (
    value === ""
    || containsTexCaretEscape(value)
    || value.includes("\\")
    || /^(?:[A-Za-z]:|\/)/u.test(value)
    || !/^[A-Za-z0-9._/-]+$/u.test(value)
  ) return false;
  return value.split("/").every((segment) => segment !== "" && segment !== "." && segment !== "..");
}

function isSafeUrl(value: string): boolean {
  return !containsTexCaretEscape(value)
    && /^(?:https?:\/\/|mailto:)[-A-Za-z0-9./:?@+,;=]+$/u.test(value);
}

function isSafeListingBody(value: string): boolean {
  return !containsTexCaretEscape(value) && !/\\end\s*\{\s*lstlisting\s*\}/iu.test(value);
}

function isSafeOptionalText(value: string): boolean {
  return !containsTexCaretEscape(value) && !value.includes("[") && !value.includes("]");
}

function containsTexCaretEscape(value: string): boolean {
  return value.includes("^^");
}

function plainTextContent(node: JSONContent): string {
  if (node.type === "text") return node.text ?? "";
  return (node.content ?? []).map(plainTextContent).join("");
}

function ensureTrailingNewline(value: string, newline: string): string {
  if (value === "") return "";
  return value.endsWith("\n") || value.endsWith("\r") ? value : `${value}${newline}`;
}

function replaceOnce(value: string, search: string, replacement: string): string {
  if (search === "") return value;
  const index = value.indexOf(search);
  if (index === -1) throw new Error("The source wrapper changed and this visual edit cannot be applied safely.");
  return `${value.slice(0, index)}${replacement}${value.slice(index + search.length)}`;
}

function detectRawEnvironment(value: string): string {
  return value.match(/\\begin\s*\{([^{}]+)\}/u)?.[1] ?? (value.includes("%") ? "comments" : "source");
}

function whitespaceBounds(value: string): { start: number; end: number } {
  const start = value.match(/^[\t\n\r ]*/u)?.[0].length ?? 0;
  const trailing = value.match(/[\t\n\r ]*$/u)?.[0].length ?? 0;
  return { start, end: Math.max(start, value.length - trailing) };
}

function consumeWhitespace(source: string, start: number): number {
  let end = start;
  while (end < source.length && /[\t\n\r ]/u.test(source[end] ?? "")) end += 1;
  return end;
}

function skipWhitespace(source: string, start: number): number {
  return consumeWhitespace(source, start);
}

function isEscaped(source: string, index: number): boolean {
  let slashes = 0;
  for (let cursor = index - 1; cursor >= 0 && source[cursor] === "\\"; cursor -= 1) slashes += 1;
  return slashes % 2 === 1;
}

function utf8Length(value: string): number {
  return textEncoder.encode(value).length;
}
