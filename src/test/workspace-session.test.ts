import { afterEach, describe, expect, it, vi } from "vitest";
import { desktopBridge } from "../lib/bridge";
import type { SetwrightBridge } from "../lib/bridge";
import { cloneDemoProject } from "../lib/mock-project";
import { applyValidatedSourceEdits } from "../lib/source-edits";
import { WorkspaceSession } from "../lib/workspace-session";

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const sessions: WorkspaceSession[] = [];
afterEach(() => { for (const session of sessions) session.stopAutosave(); sessions.length = 0; vi.restoreAllMocks(); });

function fixture(initial = cloneDemoProject()) {
  let canonical = structuredClone(initial);
  const log: string[] = [];
  const bridge = {
    ...desktopBridge,
    readProject: vi.fn<SetwrightBridge["readProject"]>(() => Promise.resolve(structuredClone(canonical))),
    applySourceEdits: vi.fn<SetwrightBridge["applySourceEdits"]>(async (_sessionId, revision, edits) => {
      expect(revision).toBe(canonical.revision);
      log.push(`edit:${edits[0]?.fileId ?? ""}:${revision}`);
      const files = await Promise.all(canonical.files.map(async (file) => {
        const selected = edits.filter((edit) => edit.fileId === file.id);
        if (selected.length === 0 || file.content === null) return file;
        return { ...file, content: await applyValidatedSourceEdits(file.content, selected), dirty: true };
      }));
      canonical = { ...canonical, revision: revision + 1, files, dirty: true };
      return { revision: canonical.revision, files, diagnostics: [] };
    }),
    saveProject: vi.fn<SetwrightBridge["saveProject"]>((_sessionId, revision) => {
      expect(revision).toBe(canonical.revision);
      log.push(`save:${revision}`);
      canonical = { ...canonical, dirty: false, files: canonical.files.map((file) => ({ ...file, dirty: false })) };
      return Promise.resolve({ revision, savedAt: "2026-09-12T12:00:00Z", fileHashes: {} });
    }),
    createSnapshot: vi.fn<SetwrightBridge["createSnapshot"]>((_sessionId, name) => {
      expect(canonical.dirty).toBe(false);
      log.push(`snapshot:${canonical.revision}`);
      return Promise.resolve({ id: "snapshot", revision: canonical.revision, createdAt: "2026-09-12T12:00:00Z", label: name ?? "", kind: "named", changedFiles: 2 });
    }),
    startCompile: vi.fn<SetwrightBridge["startCompile"]>((_sessionId, revision) => {
      expect(revision).toBe(canonical.revision);
      log.push(`compile:${revision}`);
      return Promise.resolve({ jobId: "compile", revision, state: "queued" });
    }),
    convertFileToUtf8: vi.fn<SetwrightBridge["convertFileToUtf8"]>((_sessionId, revision, fileId, reviewedText, hash) => {
      expect(revision).toBe(canonical.revision);
      expect(hash).toBe("reviewed-original");
      log.push(`convert:${fileId}:${revision}`);
      const files = canonical.files.map((file) => file.id === fileId ? { ...file, content: reviewedText, encoding: "utf8" as const, dirty: true } : file);
      canonical = { ...canonical, files, revision: revision + 1, dirty: true };
      return Promise.resolve({ revision: canonical.revision, files, diagnostics: [] });
    }),
    restoreSnapshot: vi.fn<SetwrightBridge["restoreSnapshot"]>(() => {
      log.push("restore");
      canonical = { ...structuredClone(initial), revision: canonical.revision + 1 };
      return Promise.resolve(canonical);
    }),
  } satisfies SetwrightBridge;
  const session = new WorkspaceSession(initial, bridge);
  sessions.push(session);
  return { session, bridge, log };
}

describe("shared project write queue", () => {
  it("waits for every file edit and save before closing the paper for an update", async () => {
    const { session, bridge, log } = fixture();
    bridge.closeProject = vi.fn(() => { log.push("close"); return Promise.resolve(); });
    session.edit("file-main", "first file update");
    session.edit("file-method", "second file update");
    const closing = session.close();
    expect(session.getSnapshot().closing).toBe(true);
    session.edit("file-main", "must not enter after closing begins");
    await closing;
    expect(log.map((entry) => entry.split(":")[0])).toEqual(["edit", "edit", "save", "close"]);
    expect(session.getSnapshot().drafts).toEqual({});
    expect(session.getSnapshot().closing).toBe(false);
  });

  it("keeps the paper open and its draft recoverable if an edit is rejected during close", async () => {
    const { session, bridge } = fixture();
    const closeProject = vi.fn<SetwrightBridge["closeProject"]>();
    bridge.closeProject = closeProject;
    bridge.applySourceEdits.mockRejectedValueOnce(new Error("Source conflict"));
    session.edit("file-method", "my retained draft");
    await expect(session.close()).rejects.toThrow();
    expect(closeProject).not.toHaveBeenCalled();
    expect(session.getSnapshot().drafts["file-method"]?.text).toBe("my retained draft");
    expect(session.getSnapshot().paused).toBe(true);
  });
  it("refreshes accepted source after an uncertain write response before retrying", async () => {
    const { session, bridge } = fixture();
    const apply = bridge.applySourceEdits.bind(bridge);
    bridge.applySourceEdits = vi.fn<SetwrightBridge["applySourceEdits"]>(async (...args) => {
      await apply(...args);
      throw new Error("Response lost after Rust applied the edit");
    });
    session.edit("file-method", "already applied in Rust");
    await session.settled();
    expect(session.getSnapshot().project.revision).toBe(14);
    session.discardDraft("file-method");
    await session.retrySave();
    expect(bridge.readProject).toHaveBeenCalledOnce();
    expect(session.getSnapshot().project.revision).toBe(15);
    expect(session.getSnapshot().project.files.find((file) => file.id === "file-method")?.content).toBe("already applied in Rust");
    expect(bridge.saveProject).toHaveBeenCalledWith("demo-session", 15);
    expect(session.saveState).toBe("saved");
  });

  it("retains drafts entered during recovery and keeps writes paused if refresh fails", async () => {
    const { session, bridge } = fixture();
    bridge.applySourceEdits.mockRejectedValueOnce(new Error("Edit response failed"));
    session.edit("file-main", "first retained draft");
    await session.settled();
    session.discardDraft("file-main");
    const entered = deferred();
    const release = deferred();
    const read = bridge.readProject.bind(bridge);
    bridge.readProject = vi.fn<SetwrightBridge["readProject"]>(async (...args) => { entered.resolve(); await release.promise; return read(...args); });
    const recovery = session.retrySave();
    await entered.promise;
    session.edit("file-method", "new retained draft");
    release.resolve();
    await expect(recovery).rejects.toThrow(/New drafts/);
    expect(session.getSnapshot().drafts["file-method"]?.text).toBe("new retained draft");
    expect(session.saveState).toBe("conflict");
    expect(bridge.saveProject).not.toHaveBeenCalled();
    expect(bridge.applySourceEdits).toHaveBeenCalledOnce();
    session.discardDraft("file-method");
    bridge.readProject.mockRejectedValueOnce(new Error("Accepted source unavailable"));
    await expect(session.retrySave()).rejects.toThrow("Accepted source unavailable");
    expect(session.saveState).toBe("conflict");
    expect(bridge.saveProject).not.toHaveBeenCalled();
  });

  it("requires reconciliation after an uncertain restore response before admitting new writes", async () => {
    const { session, bridge } = fixture();
    const restore = bridge.restoreSnapshot.bind(bridge);
    bridge.restoreSnapshot = vi.fn<SetwrightBridge["restoreSnapshot"]>(async (...args) => {
      await restore(...args);
      throw new Error("Restore applied but response was lost");
    });
    await expect(session.restore("old")).rejects.toThrow(/response was lost/);
    expect(session.getSnapshot().restoring).toBe(false);
    expect(session.saveState).toBe("conflict");
    session.edit("file-method", "draft based on the stale restored view");
    await session.settled();
    expect(bridge.applySourceEdits).not.toHaveBeenCalled();
    expect(session.getSnapshot().drafts["file-method"]?.text).toBe("draft based on the stale restored view");
    session.discardDraft("file-method");
    await session.retrySave();
    expect(session.getSnapshot().project.revision).toBe(15);
    expect(session.saveState).toBe("saved");
  });
  it("captures originating file IDs and keeps newer drafts across delayed acknowledgements", async () => {
    const { session, bridge, log } = fixture();
    const entered = deferred();
    const release = deferred();
    const apply = bridge.applySourceEdits.bind(bridge);
    bridge.applySourceEdits = vi.fn<SetwrightBridge["applySourceEdits"]>(async (...args) => {
      if (args[2][0]?.fileId === "file-main") { entered.resolve(); await release.promise; }
      return apply(...args);
    });
    session.edit("file-main", "first main edit");
    await entered.promise;
    session.edit("file-method", "method edit");
    session.edit("file-main", "newest main edit");
    expect(session.getSnapshot().drafts["file-main"]?.text).toBe("newest main edit");
    release.resolve();
    await session.settled();
    expect(log).toEqual(["edit:file-main:14", "edit:file-method:15", "edit:file-main:16"]);
    expect(session.getSnapshot().project.files.find((file) => file.id === "file-method")?.content).toBe("method edit");
    expect(session.getSnapshot().project.files.find((file) => file.id === "file-main")?.content).toBe("newest main edit");
    expect(session.getSnapshot().drafts).toEqual({});
    expect(session.saveState).toBe("dirty");
    await session.save();
    expect(session.saveState).toBe("saved");
  });

  it("retains both files on rejection, pauses following writes, and never reports saved", async () => {
    const { session, bridge } = fixture();
    const failure = deferred();
    bridge.applySourceEdits = vi.fn<SetwrightBridge["applySourceEdits"]>(async () => { await failure.promise; throw new Error("External edit conflict"); });
    session.edit("file-main", "recover this draft");
    session.edit("file-method", "also recover this draft");
    failure.resolve();
    await session.settled();
    expect(bridge.applySourceEdits).toHaveBeenCalledTimes(1);
    expect(session.saveState).toBe("conflict");
    expect(session.getSnapshot().drafts["file-main"]?.text).toBe("recover this draft");
    expect(session.getSnapshot().drafts["file-method"]?.text).toBe("also recover this draft");
    await expect(session.createSnapshot("unsafe snapshot")).rejects.toThrow(/paused/);
    await expect(session.compile()).rejects.toThrow(/paused/);
    await expect(session.restore("old")).rejects.toThrow(/recover/);
    session.discardDraft("file-main");
    expect(session.getSnapshot().drafts["file-method"]?.text).toBe("also recover this draft");
    await expect(session.retrySave()).rejects.toThrow(/retained draft/);
    session.discardDraft("file-method");
    await session.retrySave();
    expect(session.saveState).toBe("saved");
  });

  it("does not let an older save clear a newer unacknowledged draft", async () => {
    const { session, bridge } = fixture();
    session.edit("file-main", "accepted edit");
    await session.settled();
    const entered = deferred();
    const release = deferred();
    const save = bridge.saveProject.bind(bridge);
    bridge.saveProject = vi.fn<SetwrightBridge["saveProject"]>(async (...args) => { entered.resolve(); await release.promise; return save(...args); });
    const saving = session.save();
    await entered.promise;
    session.edit("file-method", "new pending edit");
    release.resolve();
    await saving;
    expect(session.saveState).toBe("dirty");
    expect(session.hasChanges).toBe(true);
    await session.settled();
    await session.save();
    expect(session.saveState).toBe("saved");
  });

  it("waits for prior edits before compiling, converting, saving and naming a version", async () => {
    const initial = cloneDemoProject();
    initial.files.push({ ...initial.files[1]!, id: "legacy", encoding: "nonUtf8", content: null });
    const { session, bridge, log } = fixture(initial);
    session.edit("file-bib", "@article{new, title={New reference}}\n");
    const compile = session.compile();
    const conversion = session.convert("legacy", "reviewed text", "reviewed-original");
    const snapshot = session.createSnapshot("  Ready for review  ");
    await Promise.all([compile, conversion, snapshot]);
    expect(log).toEqual(["edit:file-bib:14", "compile:15", "convert:legacy:15", "save:16", "snapshot:16"]);
    expect(bridge.createSnapshot).toHaveBeenCalledWith(initial.sessionId, "Ready for review");
    expect(session.getSnapshot().project.mainFile).toBe("file-main");
    expect(session.getSnapshot().project.files.find((file) => file.id === "legacy")?.content).toBe("reviewed text");
  });

  it("tracks every submitted conversion before the queue starts and until all results are acknowledged", async () => {
    const initial = cloneDemoProject();
    initial.files.push(
      { ...initial.files[1]!, id: "legacy-one", encoding: "nonUtf8", content: null },
      { ...initial.files[1]!, id: "legacy-two", encoding: "nonUtf8", content: null },
    );
    const { session, bridge } = fixture(initial);
    const compileEntered = deferred();
    const releaseCompile = deferred();
    const compile = bridge.startCompile.bind(bridge);
    bridge.startCompile = vi.fn<SetwrightBridge["startCompile"]>(async (...args) => {
      compileEntered.resolve();
      await releaseCompile.promise;
      return compile(...args);
    });
    const pendingCompile = session.compile();
    await compileEntered.promise;
    const enteredOne = deferred();
    const enteredTwo = deferred();
    const releaseOne = deferred();
    const releaseTwo = deferred();
    const convert = bridge.convertFileToUtf8.bind(bridge);
    bridge.convertFileToUtf8 = vi.fn<SetwrightBridge["convertFileToUtf8"]>(async (...args) => {
      const first = args[2] === "legacy-one";
      (first ? enteredOne : enteredTwo).resolve();
      await (first ? releaseOne : releaseTwo).promise;
      return convert(...args);
    });
    const firstConversion = session.convert("legacy-one", "First reviewed source", "reviewed-original");
    const secondConversion = session.convert("legacy-two", "Second reviewed source", "reviewed-original");
    expect(bridge.convertFileToUtf8).not.toHaveBeenCalled();
    expect(session.getSnapshot().project.dirty).toBe(false);
    expect(session.getSnapshot().pendingConversions).toBe(2);
    expect(session.hasChanges).toBe(true);
    expect(session.saveState).toBe("dirty");
    await expect(session.restore("old")).rejects.toThrow(/Save or recover/);
    releaseCompile.resolve();
    await pendingCompile;
    await enteredOne.promise;
    expect(session.getSnapshot().pendingConversions).toBe(2);
    releaseOne.resolve();
    await firstConversion;
    await enteredTwo.promise;
    expect(session.getSnapshot().pendingConversions).toBe(1);
    releaseTwo.resolve();
    await secondConversion;
    expect(session.getSnapshot().pendingConversions).toBe(0);
    expect(session.hasChanges).toBe(true);
    await session.save();
    expect(session.hasChanges).toBe(false);
    expect(session.saveState).toBe("saved");
  });

  it("blocks restoration for hidden drafts and resets editor epoch after a clean restore", async () => {
    const { session, bridge } = fixture();
    session.edit("file-method", "hidden draft");
    await expect(session.restore("old")).rejects.toThrow(/Save or recover/);
    expect(bridge.restoreSnapshot).not.toHaveBeenCalled();
    await session.save();
    await session.restore("old");
    expect(session.getSnapshot().epoch).toBe(1);
    expect(session.getSnapshot().drafts).toEqual({});
    expect(session.getSnapshot().project.files.find((file) => file.id === "file-method")?.content).toContain("controlled shifts");
  });

  it("rejects text writes to assets without invoking an edit or conversion command", async () => {
    const { session, bridge } = fixture();
    session.edit("file-figure", "not a text asset");
    await session.settled();
    expect(bridge.applySourceEdits).not.toHaveBeenCalled();
    expect(bridge.convertFileToUtf8).not.toHaveBeenCalled();
    expect(session.getSnapshot().project.files.find((file) => file.id === "file-figure")?.content).toBeNull();
  });

  it.each(["file-figure", "file-main", "missing"])("does not convert an ineligible file %s", async (fileId) => {
    const { session, bridge } = fixture();
    await expect(session.convert(fileId, "reviewed text", "reviewed-original")).rejects.toThrow(/unavailable for encoding conversion/);
    expect(bridge.convertFileToUtf8).not.toHaveBeenCalled();
  });
});
