import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppHeader } from "../components/AppHeader";
import { CommandPalette } from "../components/CommandPalette";
import { WelcomeScreen } from "../components/WelcomeScreen";
import { useKeyboardShortcuts } from "../hooks/useKeyboardShortcuts";
import { desktopBridge } from "../lib/bridge";
import { cloneDemoProject } from "../lib/mock-project";
import { shortcutLabel } from "../lib/keyboard";
import { useWorkspaceStore } from "../store/workspace-store";

function HeaderHarness({ canWrite = true, onOpenAnother = () => undefined }: { canWrite?: boolean; onOpenAnother?: () => void }) {
  useKeyboardShortcuts({ canWrite });
  return <><AppHeader project={cloneDemoProject()} activeFileName="sections/method.tex" canWrite={canWrite} onOpenAnother={onOpenAnother} /><CommandPalette canWrite={canWrite} /></>;
}

beforeEach(() => {
  useWorkspaceStore.setState({ mode: "write", theme: "light", outlineOpen: true, reviewPanel: null, commandPaletteOpen: false });
});
afterEach(() => vi.restoreAllMocks());

describe("Paper entry and project controls", () => {
  it("keeps creation fields inside the create path and opens a paper through the existing picker", async () => {
    const user = userEvent.setup();
    const onEnter = vi.fn();
    const project = cloneDemoProject();
    const pick = vi.spyOn(desktopBridge, "pickProjectPath").mockResolvedValue("D:/papers/main.tex");
    const open = vi.spyOn(desktopBridge, "openProject").mockResolvedValue(project);
    render(<WelcomeScreen onEnter={onEnter} />);
    expect(screen.queryByRole("textbox", { name: "Paper title" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Open a paper/i }));
    await waitFor(() => expect(onEnter).toHaveBeenCalledWith(project));
    expect(pick).toHaveBeenCalledOnce();
    expect(open).toHaveBeenCalledWith("D:/papers", "main.tex");
  });

  it("creates from the chosen template and entered details after selecting a location", async () => {
    const user = userEvent.setup();
    const onEnter = vi.fn();
    const pick = vi.spyOn(desktopBridge, "pickCreateParentDirectory").mockResolvedValue("D:/papers");
    const create = vi.spyOn(desktopBridge, "createProject").mockResolvedValue(cloneDemoProject());
    render(<WelcomeScreen onEnter={onEnter} />);
    await user.click(screen.getByRole("button", { name: /Create a paper/i }));
    expect(create).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: /Choose location and create/i })).toBeDisabled();
    await user.type(screen.getByRole("textbox", { name: "Lead author" }), "Ada Lovelace");
    await user.clear(screen.getByRole("textbox", { name: "Paper title" }));
    await user.type(screen.getByRole("textbox", { name: "Paper title" }), "Notes on computation");
    await user.click(screen.getByRole("radio", { name: /IEEE paper/i }));
    await user.click(screen.getByRole("button", { name: /Choose location and create/i }));
    expect(pick).toHaveBeenCalledOnce();
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ parentDirectory: "D:/papers", title: "Notes on computation", authors: ["Ada Lovelace"], templateId: "ieee-ieeetran", folderName: "untitled-paper" })));
    expect(onEnter).toHaveBeenCalledOnce();
  });

  it("keeps entered details when the native location picker is cancelled", async () => {
    const user = userEvent.setup();
    vi.spyOn(desktopBridge, "pickCreateParentDirectory").mockResolvedValue(null);
    const create = vi.spyOn(desktopBridge, "createProject");
    render(<WelcomeScreen onEnter={vi.fn()} />);
    await user.click(screen.getByRole("button", { name: /Create a paper/i }));
    await user.type(screen.getByRole("textbox", { name: "Lead author" }), "Ada");
    await user.click(screen.getByRole("button", { name: /Choose location and create/i }));
    await waitFor(() => expect(screen.getByRole("button", { name: /Choose location and create/i })).toBeEnabled());
    expect(screen.getByRole("textbox", { name: "Lead author" })).toHaveValue("Ada");
    expect(create).not.toHaveBeenCalled();
  });

  it("opens a real project menu, presents project details, and routes its actions", async () => {
    const user = userEvent.setup();
    const onOpenAnother = vi.fn();
    render(<HeaderHarness onOpenAnother={onOpenAnother} />);
    expect(screen.queryByRole("button", { name: "Comments" })).not.toBeInTheDocument();
    expect(screen.getByText("sections/method.tex")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Project menu:/ }));
    expect(onOpenAnother).not.toHaveBeenCalled();
    await user.click(screen.getByRole("menuitem", { name: "Project details" }));
    expect(await screen.findByRole("dialog", { name: "Project details" })).toHaveTextContent(cloneDemoProject().rootPath);
    await user.click(screen.getByRole("button", { name: "Close project details" }));
    await user.click(screen.getByRole("button", { name: /Project menu:/ }));
    await user.click(screen.getByRole("menuitem", { name: "Appearance" }));
    expect(useWorkspaceStore.getState().reviewPanel).toBe("appearance");
    await user.click(screen.getByRole("button", { name: /Project menu:/ }));
    await user.click(screen.getByRole("menuitem", { name: "Open another paper" }));
    expect(onOpenAnother).toHaveBeenCalledOnce();
  });
});

describe("Command palette navigation", () => {
  it("moves the selected command with arrows, executes Enter, and restores focus", async () => {
    const user = userEvent.setup();
    render(<HeaderHarness />);
    const trigger = screen.getByRole("button", { name: "Search commands" });
    await user.click(trigger);
    const search = screen.getByRole("combobox", { name: "Search commands" });
    expect(search).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("option", { name: /Switch to Source/i })).toHaveAttribute("aria-selected", "true");
    expect(search).toHaveAttribute("aria-activedescendant", screen.getByRole("option", { name: /Switch to Source/i }).id);
    await user.keyboard("{Enter}");
    expect(useWorkspaceStore.getState().mode).toBe("source");
    expect(screen.queryByRole("dialog", { name: "Command palette" })).not.toBeInTheDocument();
    await waitFor(() => expect(trigger).toHaveFocus());
  });

  it("wraps selection, handles empty search safely, dismisses Escape, and resets each opening", async () => {
    const user = userEvent.setup();
    render(<HeaderHarness />);
    await user.click(screen.getByRole("button", { name: "Search commands" }));
    await user.keyboard("{ArrowUp}");
    expect(screen.getByRole("option", { name: /Use high contrast theme/i })).toHaveAttribute("aria-selected", "true");
    const search = screen.getByRole("combobox");
    await user.type(search, "no-such-command");
    expect(screen.getByText(/No matching commands/)).toBeInTheDocument();
    expect(search).not.toHaveAttribute("aria-activedescendant");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Search commands" }));
    expect(screen.getByRole("combobox")).toHaveValue("");
    expect(screen.getByRole("option", { name: /Switch to Write/i })).toHaveAttribute("aria-selected", "true");
  });

  it("uses platform shortcuts and keeps Split available for non-TeX files", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("Win32");
    const user = userEvent.setup();
    render(<HeaderHarness canWrite={false} />);
    expect(shortcutLabel("K")).toBe("Ctrl+K");
    expect(screen.getByRole("tab", { name: "Write" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("tab", { name: "Split" })).not.toHaveAttribute("aria-disabled", "true");
    await user.keyboard("{Control>}k{/Control}");
    const dialog = screen.getByRole("dialog", { name: "Command palette" });
    expect(within(dialog).queryByRole("option", { name: /Switch to Write/i })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("option", { name: /Switch to Split/i })).toHaveTextContent("Ctrl+4");
    await user.keyboard("{Escape}{Control>}4{/Control}");
    expect(useWorkspaceStore.getState().mode).toBe("split");
    await user.keyboard("{Control>}1{/Control}");
    expect(useWorkspaceStore.getState().mode).toBe("split");
  });

  it("uses Command on Apple platforms without treating Control as the primary shortcut", async () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    const user = userEvent.setup();
    render(<HeaderHarness />);
    expect(shortcutLabel("K")).toBe("⌘K");
    await user.keyboard("{Control>}k{/Control}");
    expect(screen.queryByRole("dialog", { name: "Command palette" })).not.toBeInTheDocument();
    await user.keyboard("{Meta>}k{/Meta}");
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
  });
});
