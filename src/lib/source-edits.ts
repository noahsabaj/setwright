import type { FileId, SourceEdit } from "./contracts";

interface DeclaredSourceChange {
  fileId: string;
  startByte: number | null;
  endByte: number | null;
  replacement: string;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

export async function hashSourceText(source: string): Promise<string> {
  return hashBytes(encoder.encode(source));
}

export async function applyValidatedSourceEdits(source: string, edits: SourceEdit[]): Promise<string> {
  let bytes = encoder.encode(source);
  const ordered = edits.slice().sort((left, right) => right.startByte - left.startByte);

  for (const edit of ordered) {
    if (edit.startByte < 0 || edit.endByte < edit.startByte || edit.endByte > bytes.length) {
      throw new Error("A source edit falls outside the current UTF-8 buffer.");
    }
    const slice = bytes.slice(edit.startByte, edit.endByte);
    if (edit.expectedSliceHash !== "" && (await hashBytes(slice)) !== edit.expectedSliceHash) {
      throw new Error("Source edit rejected because its expected slice no longer matches.");
    }
    const replacement = encoder.encode(edit.replacement);
    const next = new Uint8Array(edit.startByte + replacement.length + bytes.length - edit.endByte);
    next.set(bytes.slice(0, edit.startByte), 0);
    next.set(replacement, edit.startByte);
    next.set(bytes.slice(edit.endByte), edit.startByte + replacement.length);
    bytes = next;
  }

  return decoder.decode(bytes);
}

/**
 * Produces one minimal contiguous byte patch without splitting a Unicode code
 * point. Rust still validates the expected slice hash and reparses the
 * candidate before accepting it.
 */
export async function createMinimalSourceEdit(
  fileId: FileId,
  before: string,
  after: string,
): Promise<SourceEdit | null> {
  if (before === after) return null;
  const beforePoints = Array.from(before);
  const afterPoints = Array.from(after);
  let prefixPoints = 0;
  while (
    prefixPoints < beforePoints.length
    && prefixPoints < afterPoints.length
    && beforePoints[prefixPoints] === afterPoints[prefixPoints]
  ) {
    prefixPoints += 1;
  }
  let suffixPoints = 0;
  while (
    suffixPoints < beforePoints.length - prefixPoints
    && suffixPoints < afterPoints.length - prefixPoints
    && beforePoints[beforePoints.length - 1 - suffixPoints] === afterPoints[afterPoints.length - 1 - suffixPoints]
  ) {
    suffixPoints += 1;
  }
  const prefix = beforePoints.slice(0, prefixPoints).join("");
  const oldMiddle = beforePoints.slice(prefixPoints, beforePoints.length - suffixPoints).join("");
  const newMiddle = afterPoints.slice(prefixPoints, afterPoints.length - suffixPoints).join("");
  const startByte = encoder.encode(prefix).length;
  return {
    fileId,
    startByte,
    endByte: startByte + encoder.encode(oldMiddle).length,
    replacement: newMiddle,
    expectedSliceHash: await hashSourceText(oldMiddle),
  };
}

/**
 * Converts projection-owned visual changes into independent byte patches.
 * Returning `null` means the reconstruction contains a newly inserted block
 * without an existing owned span, so the caller must compute a conservative
 * source diff instead. Ownership is never guessed.
 */
export async function createDeclaredSourceEdits(
  fileId: FileId,
  before: string,
  after: string,
  changes: readonly DeclaredSourceChange[],
): Promise<SourceEdit[] | null> {
  if (changes.length === 0) return before === after ? [] : null;
  if (changes.some((change) => change.startByte === null || change.endByte === null)) return null;
  const beforeBytes = encoder.encode(before);
  const ordered = changes
    .map((change) => ({ ...change, startByte: change.startByte as number, endByte: change.endByte as number }))
    .sort((left, right) => left.startByte - right.startByte);

  for (const [index, change] of ordered.entries()) {
    if (
      change.fileId !== fileId
      || change.startByte < 0
      || change.endByte < change.startByte
      || change.endByte > beforeBytes.length
      || (index > 0 && ordered[index - 1]!.endByte > change.startByte)
    ) {
      throw new Error("The visual projection declared an invalid or overlapping source span.");
    }
  }

  const edits = await Promise.all(ordered.map(async (change) => ({
    fileId,
    startByte: change.startByte,
    endByte: change.endByte,
    replacement: change.replacement,
    expectedSliceHash: await hashBytes(beforeBytes.slice(change.startByte, change.endByte)),
  })));
  return await applyValidatedSourceEdits(before, edits) === after ? edits : null;
}

async function hashBytes(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new Uint8Array(bytes).buffer);
  return Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
}
