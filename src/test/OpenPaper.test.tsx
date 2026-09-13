import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { desktopBridge, resetMockBridge } from "../lib/bridge";
import { cloneDemoProject } from "../lib/mock-project";
import { useWorkspaceStore } from "../store/workspace-store";

vi.mock("mathlive", () => ({}));

beforeEach(() => {
  resetMockBridge();
  useWorkspaceStore.setState({ mode: "write", theme: "light", outlineOpen: true, reviewPanel: null, commandPaletteOpen: false });
});
afterEach(() => vi.restoreAllMocks());

describe("Opening selected LaTeX files", () => {
  it("passes a selected Windows file as directory plus main file during onboarding", async () => {
    const user = userEvent.setup();
    vi.spyOn(desktopBridge, "pickProjectPath").mockResolvedValue(String.raw`D:\Papers\My study\manuscript.tex`);
    const open = vi.spyOn(desktopBridge, "openProject").mockResolvedValue(cloneDemoProject());
    render(<App />);
    await user.click(screen.getByRole("button", { name: /Open a paper/i }));
    await waitFor(() => expect(open).toHaveBeenCalledWith(String.raw`D:\Papers\My study`, "manuscript.tex"));
    expect(await screen.findByRole("region", { name: "Visual paper editor" }, { timeout: 5_000 })).toBeVisible();
  });

  it("passes the selected UNC file to Open another paper without replacing the current paper", async () => {
    const user = userEvent.setup();
    vi.spyOn(desktopBridge, "pickProjectPath")
      .mockResolvedValueOnce(String.raw`D:\Papers\first\main.tex`)
      .mockResolvedValueOnce(String.raw`\\server\papers\Second study\submission.tex`);
    const open = vi.spyOn(desktopBridge, "openProject").mockResolvedValue(cloneDemoProject());
    const openWindow = vi.spyOn(desktopBridge, "openProjectWindow").mockResolvedValue({ sessionId: "second", windowLabel: "project-second" });
    render(<App />);
    await user.click(screen.getByRole("button", { name: /Open a paper/i }));
    await screen.findByRole("region", { name: "Visual paper editor" }, { timeout: 5_000 });
    await user.click(screen.getByRole("button", { name: /Project menu:/ }));
    await user.click(screen.getByRole("menuitem", { name: "Open another paper" }));
    await waitFor(() => expect(openWindow).toHaveBeenCalledWith(String.raw`\\server\papers\Second study`, "submission.tex"));
    expect(open).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: `Project menu: ${cloneDemoProject().title}` })).toBeVisible();
  });

  it("does not issue a project command when either picker is cancelled", async () => {
    const user = userEvent.setup();
    const pick = vi.spyOn(desktopBridge, "pickProjectPath").mockResolvedValueOnce(null);
    const open = vi.spyOn(desktopBridge, "openProject").mockResolvedValue(cloneDemoProject());
    const openWindow = vi.spyOn(desktopBridge, "openProjectWindow");
    render(<App />);
    await user.click(screen.getByRole("button", { name: /Open a paper/i }));
    expect(open).not.toHaveBeenCalled();
    pick.mockResolvedValueOnce("/papers/main.tex");
    await user.click(screen.getByRole("button", { name: /Open a paper/i }));
    await screen.findByRole("region", { name: "Visual paper editor" }, { timeout: 5_000 });
    pick.mockResolvedValueOnce(null);
    await user.click(screen.getByRole("button", { name: /Project menu:/ }));
    await user.click(screen.getByRole("menuitem", { name: "Open another paper" }));
    expect(openWindow).not.toHaveBeenCalled();
  });
});
