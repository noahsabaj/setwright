import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { resetMockBridge } from "../lib/bridge";
import { useWorkspaceStore } from "../store/workspace-store";

vi.mock("mathlive", () => ({}));

async function enterDemoWorkspace() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(screen.getByRole("button", { name: /create paper/i }));
  await screen.findByLabelText("Visual paper editor");
  return user;
}

describe("Setwright desktop workspace", () => {
  beforeEach(() => {
    resetMockBridge();
    useWorkspaceStore.setState({
      mode: "split",
      theme: "light",
      saveState: "saved",
      outlineOpen: true,
      reviewPanel: null,
      commandPaletteOpen: false,
      compileState: "idle",
    });
  });

  it("moves from the locked identity screen into the split writing workspace", async () => {
    const user = userEvent.setup();
    render(<App />);

    expect(screen.getByRole("heading", { name: "Write papers, not TeX." })).toBeInTheDocument();
    expect(screen.getByText(/local-first, open-source visual editor/i)).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /research article/i })).toBeChecked();

    await user.click(screen.getByRole("button", { name: /create paper/i }));

    expect(await screen.findByRole("tab", { name: "Split" }, { timeout: 5_000 })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("toolbar", { name: "Writing tools" })).toBeInTheDocument();
    expect(screen.getByLabelText("Compiled PDF preview")).toHaveTextContent("No PDF compiled");
    expect(screen.getByLabelText("Compiled PDF preview")).not.toHaveTextContent("Retrieval-Augmented Models Under Distribution Shift");
    expect(screen.getByText("Demo draft · not written")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /suggestion mode/i })).not.toBeInTheDocument();
    expect(screen.getByText("Method")).toBeInTheDocument();
    const sourceMetrics = screen.getByLabelText("Project source metrics");
    expect(within(sourceMetrics).getByText("References")).toBeInTheDocument();
    expect(within(sourceMetrics).getByText("2")).toBeInTheDocument();
  });

  it("switches between Write, Source, Preview, and Split modes", async () => {
    const user = await enterDemoWorkspace();
    const tabs = screen.getByRole("tablist", { name: "Workspace view" });

    await user.click(within(tabs).getByRole("tab", { name: "Source" }));
    expect(await screen.findByLabelText("LaTeX source editor")).toBeInTheDocument();

    await user.click(within(tabs).getByRole("tab", { name: "Preview" }));
    expect(screen.getByLabelText("Compiled PDF preview")).toBeInTheDocument();

    await user.click(within(tabs).getByRole("tab", { name: "Write" }));
    expect(screen.getByLabelText("Visual paper editor")).toBeInTheDocument();
  });

  it("opens the focus-managed command palette from the keyboard", async () => {
    const user = await enterDemoWorkspace();
    await user.keyboard("{Control>}k{/Control}");

    const palette = screen.getByRole("dialog", { name: "Command palette" });
    expect(within(palette).queryByRole("option", { name: /suggestion/i })).not.toBeInTheDocument();
    expect(within(palette).queryByRole("option", { name: /arxiv/i })).not.toBeInTheDocument();
    const search = within(palette).getByPlaceholderText(/search commands/i);
    expect(search).toHaveFocus();
    await user.type(search, "version history");
    await user.click(within(palette).getByRole("option", { name: /open version history/i }));
    expect(screen.getByRole("complementary", { name: "Review and history" })).toBeInTheDocument();
  });

  it("resizes the split with an accessible keyboard separator", async () => {
    const user = await enterDemoWorkspace();
    const splitter = screen.getByRole("separator", { name: /resize editor and pdf preview/i });
    expect(splitter).toHaveAttribute("aria-valuenow", "54");
    splitter.focus();
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    expect(splitter).toHaveAttribute("aria-valuenow", "50");
  });

  it("exposes canonical raw source alongside safely supported scientific nodes", async () => {
    await enterDemoWorkspace();
    const rawSources = screen.getAllByLabelText(/Raw .* source/i);
    expect(rawSources.some((element) => (element as HTMLTextAreaElement).value.includes("\\begin{table}"))).toBe(true);
    expect(screen.getByLabelText("Editable display equation")).toBeInTheDocument();
    expect(screen.getByText("lewis2020rag")).toBeInTheDocument();
  });
});
