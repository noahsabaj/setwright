import { beforeEach, describe, expect, it } from "vitest";
import { desktopBridge, resetMockBridge } from "../lib/bridge";
import { demoLatexSource } from "../lib/mock-project";
import { applyValidatedSourceEdits, createDeclaredSourceEdits, createMinimalSourceEdit, hashSourceText } from "../lib/source-edits";

describe("browser demo bridge", () => {
  beforeEach(() => resetMockBridge());

  it("marks the fallback explicitly and advances a validated revision", async () => {
    expect(desktopBridge.runtime).toBe("browserDemo");
    const project = await desktopBridge.openProject("~/Papers/demo");
    const expectedSliceHash = await hashSourceText("article");
    const result = await desktopBridge.applySourceEdits(project.sessionId, project.revision, [
      {
        fileId: project.mainFile,
        startByte: demoLatexSource.indexOf("article"),
        endByte: demoLatexSource.indexOf("article") + "article".length,
        replacement: "report",
        expectedSliceHash,
      },
    ]);

    expect(result.revision).toBe(project.revision + 1);
    expect(result.files.find((file) => file.id === project.mainFile)?.content).toContain("\\documentclass[11pt]{report}");
  });

  it("rejects stale expected source instead of applying approximately", async () => {
    const project = await desktopBridge.openProject("~/Papers/demo");
    await expect(desktopBridge.applySourceEdits(project.sessionId, project.revision, [
      {
        fileId: project.mainFile,
        startByte: 0,
        endByte: 5,
        replacement: "unsafe",
        expectedSliceHash: await hashSourceText("wrong"),
      },
    ])).rejects.toThrow(/expected slice/u);
  });
});

describe("UTF-8 source edit utility", () => {
  it("uses byte offsets without damaging adjacent Unicode", async () => {
    const source = "α paper café";
    const prefixBytes = new TextEncoder().encode("α paper ").length;
    const target = "café";
    const result = await applyValidatedSourceEdits(source, [{
      fileId: "main",
      startByte: prefixBytes,
      endByte: prefixBytes + new TextEncoder().encode(target).length,
      replacement: "résumé",
      expectedSliceHash: await hashSourceText(target),
    }]);
    expect(result).toBe("α paper résumé");
  });

  it("creates a minimal hash-guarded patch without splitting Unicode", async () => {
    const before = "α paper café ☕";
    const after = "α paper résumé ☕";
    const edit = await createMinimalSourceEdit("main", before, after);
    expect(edit).not.toBeNull();
    // The shared trailing `é` stays untouched, so the patch is smaller than
    // replacing the whole word.
    expect(edit?.replacement).toBe("résum");
    await expect(applyValidatedSourceEdits(before, edit === null ? [] : [edit])).resolves.toBe(after);
  });

  it("keeps non-adjacent visual ownership changes as separate byte patches", async () => {
    const before = "α first — untouched middle — second";
    const encoder = new TextEncoder();
    const firstStart = encoder.encode("α ").length;
    const secondStart = encoder.encode("α first — untouched middle — ").length;
    const after = "α FIRST — untouched middle — SECOND";
    const edits = await createDeclaredSourceEdits("main", before, after, [
      { fileId: "main", startByte: firstStart, endByte: firstStart + 5, replacement: "FIRST" },
      { fileId: "main", startByte: secondStart, endByte: secondStart + 6, replacement: "SECOND" },
    ]);

    expect(edits).toHaveLength(2);
    expect(edits?.[0]?.endByte).toBeLessThan(edits?.[1]?.startByte ?? 0);
    await expect(applyValidatedSourceEdits(before, edits ?? [])).resolves.toBe(after);
  });

  it("rejects incomplete declared spans so deletions and moves use a conservative diff", async () => {
    const before = "first\nsecond\nthird\n";
    const after = "third\nfirst\n";

    await expect(createDeclaredSourceEdits("main", before, after, [])).resolves.toBeNull();
    await expect(createDeclaredSourceEdits("main", before, after, [{
      fileId: "main",
      startByte: 0,
      endByte: 6,
      replacement: "third\n",
    }])).resolves.toBeNull();
  });
});
