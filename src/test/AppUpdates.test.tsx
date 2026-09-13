import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { UpdateCenter } from "../components/UpdateCenter";
import { appUpdates, type AppUpdateStatus } from "../lib/app-updates";

const native = vi.hoisted(() => ({ handler: null as null | ((event: { payload: AppUpdateStatus }) => void), stop: vi.fn() }));
vi.mock("../lib/bridge", () => ({ desktopBridge: { runtime: "tauri" } }));
vi.mock("../lib/app-updates", () => ({ appUpdates: { status: vi.fn(), check: vi.fn(), download: vi.fn(), install: vi.fn(), setAutomatic: vi.fn() } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn((_name: string, handler: NonNullable<typeof native.handler>) => { native.handler = handler; return Promise.resolve(native.stop); }) }));
const base: AppUpdateStatus = { currentVersion: "0.2.0", phase: "current", version: null, notes: null, automatic: false, downloadedBytes: 0, totalBytes: null, error: null };
beforeEach(() => {
  vi.clearAllMocks(); native.handler = null;
  vi.mocked(appUpdates.status).mockResolvedValue(base);
  vi.mocked(appUpdates.check).mockResolvedValue(base);
  vi.mocked(appUpdates.install).mockResolvedValue();
});
async function status(next: Partial<AppUpdateStatus>) {
  await waitFor(() => expect(native.handler).not.toBeNull());
  act(() => native.handler?.({ payload: { ...base, ...next } }));
}
describe("application updates", () => {
  it("shows installed changelog offline and never interprets remote HTML", async () => {
    render(<UpdateCenter paperOpen={false} />);
    await status({ phase: "available", version: "0.3.0", notes: '<img src="https://tracker.invalid/a" onerror="alert(1)">\n- Better editing' });
    fireEvent.click(screen.getByRole("button", { name: "Update available" }));
    expect(screen.getByRole("heading", { name: "Changelog" })).toBeInTheDocument();
    expect(screen.getByText(/Better editing/)).toBeInTheDocument();
    expect(document.querySelector('img[src*="tracker.invalid"]')).toBeNull();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    expect(appUpdates.download).not.toHaveBeenCalled();
  });
  it("downloads automatically when opted in but waits for the paper to close before installing", async () => {
    vi.mocked(appUpdates.download).mockResolvedValue({ ...base, phase: "ready", version: "0.3.0", automatic: true });
    const { rerender } = render(<UpdateCenter paperOpen />);
    await status({ phase: "available", version: "0.3.0", automatic: true });
    await waitFor(() => expect(appUpdates.download).toHaveBeenCalledOnce());
    expect(appUpdates.install).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Update ready" }));
    expect(screen.getByRole("button", { name: "Install update and restart" })).toBeDisabled();
    rerender(<UpdateCenter paperOpen={false} />);
    await waitFor(() => expect(appUpdates.install).toHaveBeenCalledOnce());
  });
  it("does not enable automatic updates if saving the preference fails", async () => {
    vi.mocked(appUpdates.setAutomatic).mockRejectedValue(new Error("Preference could not be saved"));
    render(<UpdateCenter paperOpen={false} />);
    await status({});
    fireEvent.click(screen.getByRole("button", { name: "Updates & changelog" }));
    fireEvent.click(screen.getByRole("checkbox"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Preference could not be saved");
    expect(screen.getByRole("checkbox")).not.toBeChecked();
  });
  it("keeps download errors retryable and does not install an unverified update", async () => {
    vi.mocked(appUpdates.download).mockRejectedValue(new Error("Signature verification failed"));
    render(<UpdateCenter paperOpen={false} />);
    await status({ phase: "available", version: "0.3.0" });
    fireEvent.click(screen.getByRole("button", { name: "Update available" }));
    fireEvent.click(screen.getByRole("button", { name: "Download update" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Signature verification failed");
    expect(screen.getByRole("button", { name: "Download update" })).toBeEnabled();
    expect(appUpdates.install).not.toHaveBeenCalled();
  });
  it("reports unavailable checks honestly and tears down its native subscription", async () => {
    vi.mocked(appUpdates.status).mockResolvedValue({ ...base, phase: "idle" });
    vi.mocked(appUpdates.check).mockRejectedValue(new Error("Release feed unavailable"));
    const { unmount } = render(<UpdateCenter paperOpen={false} />);
    await waitFor(() => expect(appUpdates.check).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "Updates & changelog" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Release feed unavailable");
    expect(screen.queryByText("You’re up to date.")).toBeNull();
    unmount(); expect(native.stop).toHaveBeenCalledOnce();
  });
});
