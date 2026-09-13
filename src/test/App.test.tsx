import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { desktopBridge, resetMockBridge } from "../lib/bridge";
import { useWorkspaceStore } from "../store/workspace-store";

vi.mock("mathlive", () => ({}));

async function enterDemoWorkspace() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(screen.getByRole("button", { name: /Open a paper/i }));
  await screen.findByRole("region", { name: "Visual paper editor" }, { timeout: 5_000 });
  return user;
}

describe("Setwright authoring workspace", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    resetMockBridge();
    useWorkspaceStore.setState({
      mode: "write", theme: "light", saveState: "saved", outlineOpen: true,
      reviewPanel: null, commandPaletteOpen: false, compileState: "idle", splitRatio: 54,
    });
  });

  it("opens into a quiet Write workspace with collapsed preserved source", async () => {
    await enterDemoWorkspace();
    expect(screen.getByRole("tab", { name: "Write" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("toolbar", { name: "Writing tools" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "Compiled PDF preview" })).not.toBeInTheDocument();
    const raw = screen.getAllByLabelText(/Raw .* source/);
    expect(raw.length).toBeGreaterThan(0);
    expect(raw.every((input) => input.closest("details")?.open === false)).toBe(true);
    expect(screen.getByText("Demo draft · not written")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Comments" })).not.toBeInTheDocument();
  });

  it("switches modes while retaining the same file editor instances", async () => {
    const user = await enterDemoWorkspace();
    const visual = screen.getByRole("region", { name: "Visual paper editor" });
    await user.click(screen.getByRole("tab", { name: "Source" }));
    const source = await screen.findByRole("region", { name: "LaTeX source editor" });
    expect(visual).not.toBeVisible();
    await user.click(screen.getByRole("tab", { name: "Preview" }));
    expect(screen.getByRole("region", { name: "Compiled PDF preview" })).toBeVisible();
    expect(source).not.toBeVisible();
    await user.click(screen.getByRole("tab", { name: "Split" }));
    expect(screen.getByRole("separator", { name: /Resize editor/ })).toBeVisible();
    expect(screen.getByRole("region", { name: "Visual paper editor" })).toBe(visual);
    await user.click(screen.getByRole("tab", { name: "Source" }));
    expect(screen.getByRole("region", { name: "LaTeX source editor" })).toBe(source);
  });

  it("opens and runs a command entirely by keyboard", async () => {
    const user = await enterDemoWorkspace();
    await user.keyboard("{Control>}k{/Control}");
    const palette = screen.getByRole("dialog", { name: "Command palette" });
    const search = within(palette).getByRole("combobox", { name: "Search commands" });
    expect(search).toHaveFocus();
    await user.type(search, "version history");
    await user.keyboard("{Enter}");
    expect(screen.getByRole("complementary", { name: "Version history" })).toBeVisible();
    expect(screen.queryByRole("dialog", { name: "Command palette" })).not.toBeInTheDocument();
  });

  it("navigates included headings and files without writing source", async () => {
    const apply = vi.spyOn(desktopBridge, "applySourceEdits");
    const user = await enterDemoWorkspace();
    const sidebar = screen.getByRole("complementary", { name: "Project and document outline" });
    const mainEditor = screen.getByRole("region", { name: "Visual paper editor" });
    await user.click(within(sidebar).getByRole("button", { name: "Method" }));
    const methodEditor = screen.getByRole("region", { name: "Visual paper editor" });
    expect(within(methodEditor).getByRole("heading", { name: "Method" })).toBeVisible();
    expect(within(sidebar).getByRole("button", { name: "sections/method.tex" })).toHaveAttribute("aria-current", "page");
    await user.click(within(sidebar).getByRole("button", { name: "references.bib" }));
    expect(screen.getByRole("tab", { name: "Write" })).toHaveAttribute("aria-disabled", "true");
    expect(await screen.findByRole("region", { name: "LaTeX source editor" })).toHaveTextContent("references.bib");
    await user.click(within(sidebar).getByRole("button", { name: "figures/shift-overview.pdf" }));
    expect(screen.getByRole("region", { name: "File details" })).toHaveTextContent("Project asset");
    expect(screen.queryByRole("region", { name: "LaTeX source editor" })).not.toBeInTheDocument();
    await user.click(within(sidebar).getByRole("button", { name: "main.tex" }));
    await user.click(screen.getByRole("tab", { name: "Write" }));
    expect(screen.getByRole("region", { name: "Visual paper editor" })).toBe(mainEditor);
    expect(apply).not.toHaveBeenCalled();
  });

  it("edits only the selected file and preserves its undo through file navigation", async () => {
    const apply = vi.spyOn(desktopBridge, "applySourceEdits");
    const user = await enterDemoWorkspace();
    const sidebar = screen.getByRole("complementary", { name: "Project and document outline" });
    await user.click(within(sidebar).getByRole("button", { name: "Method" }));
    const editor = within(screen.getByRole("region", { name: "Visual paper editor" })).getByLabelText("Paper editor");
    const paragraph = within(editor).getByText(/We evaluate three controlled shifts/);
    await user.click(paragraph);
    const range = document.createRange();
    range.selectNodeContents(paragraph);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    await user.keyboard(" Added sentence.");
    await waitFor(() => expect(apply).toHaveBeenCalled());
    expect(apply.mock.calls.every((call) => call[2].every((edit) => edit.fileId === "file-method"))).toBe(true);
    await user.click(within(sidebar).getByRole("button", { name: "main.tex" }));
    await user.click(within(sidebar).getByRole("button", { name: /sections\/method.tex/ }));
    expect(editor).toBeVisible();
    expect(editor).toHaveTextContent("Added sentence.");
    await user.click(screen.getByRole("button", { name: "Undo" }));
    expect(editor).not.toHaveTextContent("Added sentence.");
    await waitFor(() => expect(apply.mock.calls.length).toBeGreaterThan(1));
  });
});
