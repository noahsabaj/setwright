import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useCloseProtection } from "../hooks/useCloseProtection";
import { desktopBridge } from "../lib/bridge";
import type * as BridgeModule from "../lib/bridge";
import { cloneDemoProject } from "../lib/mock-project";
import { WorkspaceSession } from "../lib/workspace-session";
import type { EditResult } from "../lib/contracts";

const native = vi.hoisted(() => ({ listen: vi.fn(), stop: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onCloseRequested: native.listen }) }));
vi.mock("../lib/bridge", async (importOriginal) => {
  const actual = await importOriginal<typeof BridgeModule>();
  return { ...actual, desktopBridge: { ...actual.desktopBridge, runtime: "tauri" } };
});

beforeEach(() => { native.listen.mockReset(); native.stop.mockReset(); });
afterEach(() => { vi.restoreAllMocks(); });

describe("close protection", () => {
  it("blocks native close for a hidden draft before its write is acknowledged and after rejection", async () => {
    let reject!: (error: Error) => void;
    const apply = vi.spyOn(desktopBridge, "applySourceEdits").mockImplementation(() => new Promise((_resolve, no) => { reject = no; }));
    native.listen.mockResolvedValue(native.stop);
    const session = new WorkspaceSession(cloneDemoProject(), desktopBridge);
    const onBlocked = vi.fn();
    const { unmount } = renderHook(() => useCloseProtection(session, onBlocked));
    await waitFor(() => expect(native.listen).toHaveBeenCalled());
    const close = native.listen.mock.calls.at(-1)?.[0] as (event: { preventDefault: () => void }) => void;
    const preventDefault = vi.fn();
    act(() => { session.edit("file-method", "A retained hidden draft"); close({ preventDefault }); });
    expect(preventDefault).toHaveBeenCalledOnce();
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    await waitFor(() => expect(apply).toHaveBeenCalled());
    await act(async () => { reject(new Error("Rejected")); await session.settled(); });
    session.discardDraft("file-method");
    act(() => close({ preventDefault }));
    expect(preventDefault).toHaveBeenCalledTimes(2);
    expect(onBlocked).toHaveBeenCalledTimes(2);
    unmount();
    expect(native.stop).toHaveBeenCalled();
    session.stopAutosave();
  });

  it("allows a clean session to close", async () => {
    native.listen.mockResolvedValue(native.stop);
    const session = new WorkspaceSession(cloneDemoProject(), desktopBridge);
    renderHook(() => useCloseProtection(session, vi.fn()));
    await waitFor(() => expect(native.listen).toHaveBeenCalled());
    const close = native.listen.mock.calls.at(-1)?.[0] as (event: { preventDefault: () => void }) => void;
    const preventDefault = vi.fn();
    close({ preventDefault });
    expect(preventDefault).not.toHaveBeenCalled();
  });

  it.each(["success", "rejection"] as const)("protects an approved conversion before acknowledgement and through %s", async (outcome) => {
    let resolve!: (result: EditResult) => void;
    let reject!: (error: Error) => void;
    const pendingResult = new Promise<EditResult>((yes, no) => { resolve = yes; reject = no; });
    const convert = vi.spyOn(desktopBridge, "convertFileToUtf8").mockReturnValue(pendingResult);
    const initial = cloneDemoProject();
    initial.files.push({ ...initial.files[1]!, id: "legacy", content: null, encoding: "nonUtf8" });
    const session = new WorkspaceSession(initial, desktopBridge);
    native.listen.mockResolvedValue(native.stop);
    const { unmount } = renderHook(() => useCloseProtection(session, vi.fn()));
    await waitFor(() => expect(native.listen).toHaveBeenCalledOnce());
    const close = native.listen.mock.calls[0]?.[0] as (event: { preventDefault: () => void }) => void;
    const preventDefault = vi.fn();
    let conversion!: Promise<void>;
    act(() => {
      conversion = session.convert("legacy", "Approved reviewed source", "original-hash");
      close({ preventDefault });
    });
    expect(preventDefault).toHaveBeenCalledOnce();
    expect(session.getSnapshot().project.dirty).toBe(false);
    expect(session.getSnapshot().drafts).toEqual({});
    await waitFor(() => expect(convert).toHaveBeenCalledOnce());
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);

    if (outcome === "success") {
      await act(async () => {
        resolve({ revision: initial.revision + 1, diagnostics: [], files: initial.files.map((file) => file.id === "legacy" ? { ...file, encoding: "utf8", content: "Approved reviewed source", dirty: true } : file) });
        await conversion;
      });
      expect(session.getSnapshot().pendingConversions).toBe(0);
      act(() => close({ preventDefault }));
      expect(preventDefault).toHaveBeenCalledTimes(2);
      vi.spyOn(desktopBridge, "saveProject").mockResolvedValue({ revision: initial.revision + 1, savedAt: "2026-09-12T12:00:00Z", fileHashes: {} });
      await act(() => session.save());
    } else {
      await act(async () => {
        reject(new Error("Conversion response rejected"));
        await expect(conversion).rejects.toThrow(/Conversion response rejected/);
      });
      expect(session.getSnapshot().pendingConversions).toBe(0);
      expect(session.getSnapshot().paused).toBe(true);
      act(() => close({ preventDefault }));
      expect(preventDefault).toHaveBeenCalledTimes(2);
      vi.spyOn(desktopBridge, "readProject").mockResolvedValue(initial);
      await act(() => session.retrySave());
    }
    act(() => close({ preventDefault }));
    expect(preventDefault).toHaveBeenCalledTimes(2);
    unmount();
    session.stopAutosave();
  });
});
