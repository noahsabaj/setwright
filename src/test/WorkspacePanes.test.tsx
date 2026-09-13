import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PreviewPane } from "../components/PreviewPane";
import { ProjectSidebar } from "../components/ProjectSidebar";
import { ReviewRail } from "../components/ReviewRail";
import { SplitWorkspace } from "../components/SplitWorkspace";
import { desktopBridge } from "../lib/bridge";
import type { HistoryEntry } from "../lib/contracts";
import { cloneDemoProject } from "../lib/mock-project";
import { deriveProjectMetrics } from "../lib/project-metrics";
import { useWorkspaceStore } from "../store/workspace-store";

const originalResizeObserver = globalThis.ResizeObserver;

const historyEntry: HistoryEntry = {
  id: "version-1",
  revision: 1,
  createdAt: "2026-09-10T12:00:00Z",
  label: "Ready for review",
  kind: "named",
  changedFiles: 2,
};

beforeEach(() => {
  useWorkspaceStore.setState({ splitRatio: 54, reviewPanel: null, theme: "light", previewPage: 1, previewZoom: 82, previewScrollTop: 0, compileState: "idle" });
});

afterEach(() => {
  vi.restoreAllMocks();
  globalThis.ResizeObserver = originalResizeObserver;
});

describe("project navigation", () => {
  it("activates the selected file by keyboard without changing the compilation entrypoint", async () => {
    const user = userEvent.setup();
    const project = cloneDemoProject();
    const original = structuredClone(project);
    const bibliography = project.files.find((file) => file.kind === "bib");
    if (bibliography === undefined) throw new Error("Missing bibliography fixture");
    const onSelectFile = vi.fn();
    const onNavigate = vi.fn();
    const onClose = vi.fn();
    const { rerender } = render(<ProjectSidebar project={project} metrics={deriveProjectMetrics(project)} activeFileId={project.mainFile} onSelectFile={onSelectFile} onNavigate={onNavigate} onClose={onClose} />);
    const fileButton = screen.getByRole("button", { name: bibliography.relativePath });
    fileButton.focus();
    await user.keyboard("{Enter}");
    expect(onSelectFile).toHaveBeenCalledWith(bibliography.id);
    rerender(<ProjectSidebar project={project} metrics={deriveProjectMetrics(project)} activeFileId={bibliography.id} onSelectFile={onSelectFile} onNavigate={onNavigate} onClose={onClose} />);
    expect(fileButton).toHaveAttribute("aria-current", "page");
    expect(project).toEqual(original);
    await user.click(screen.getByRole("button", { name: "Close navigation" }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("passes exact outline destinations for duplicate headings after Unicode text", async () => {
    const user = userEvent.setup();
    const project = cloneDemoProject();
    const source = "\\begin{document}\nCafé 📝\n\\section{Results}\nFirst.\n\\section{Results}\nSecond.\n\\end{document}";
    project.files = project.files.map((file) => file.id === project.mainFile ? { ...file, content: source } : file);
    const metrics = deriveProjectMetrics(project);
    const onNavigate = vi.fn();
    render(<ProjectSidebar project={project} metrics={metrics} activeFileId={project.mainFile} onSelectFile={vi.fn()} onNavigate={onNavigate} />);
    await user.type(screen.getByRole("textbox", { name: "Filter outline and files" }), "Results");
    const headings = screen.getAllByRole("button", { name: "Results" });
    headings[1]?.focus();
    await user.keyboard(" ");
    expect(onNavigate).toHaveBeenCalledWith(metrics.outline[1]);
    expect(metrics.outline[1]?.sourceOffset).toBe(source.lastIndexOf("\\section"));
    await user.clear(screen.getByRole("textbox", { name: "Filter outline and files" }));
    await user.type(screen.getByRole("textbox", { name: "Filter outline and files" }), "no matching heading");
    expect(screen.getByText("No matching headings")).toBeInTheDocument();
  });
});

describe("version history and appearance", () => {
  it("waits for loading, submits the supplied name, and keeps it recoverable when saving fails", async () => {
    const user = userEvent.setup();
    let finishLoad: (entries: HistoryEntry[]) => void = () => undefined;
    vi.spyOn(desktopBridge, "listHistory").mockImplementation(() => new Promise((resolve) => { finishLoad = resolve; }));
    const onCreateSnapshot = vi.fn<(name: string) => Promise<HistoryEntry>>().mockRejectedValueOnce(new Error("The draft could not be saved.")).mockResolvedValueOnce(historyEntry);
    useWorkspaceStore.setState({ reviewPanel: "history" });
    render(<ReviewRail project={cloneDemoProject()} canRestore onCreateSnapshot={onCreateSnapshot} onRestoreSnapshot={vi.fn()} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading local history");
    expect(screen.getByRole("textbox", { name: "Name current version" })).toBeDisabled();
    act(() => { finishLoad([]); });
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Name current version" })).toBeEnabled());
    const name = screen.getByRole("textbox", { name: "Name current version" });
    await user.type(name, "  Ready for review  ");
    await user.keyboard("{Enter}");
    expect(onCreateSnapshot).toHaveBeenCalledWith("Ready for review");
    expect(await screen.findByRole("alert")).toHaveTextContent("The draft could not be saved.");
    expect(name).toHaveValue("  Ready for review  ");
    await user.click(screen.getByRole("button", { name: "Save named version" }));
    expect(await screen.findByRole("button", { name: "Restore Ready for review" })).toBeEnabled();
    expect(name).toHaveValue("");
  });

  it("blocks restoration with pending changes and gives appearance its own controls", async () => {
    const user = userEvent.setup();
    vi.spyOn(desktopBridge, "listHistory").mockResolvedValue([historyEntry]);
    useWorkspaceStore.setState({ reviewPanel: "history" });
    const props = { project: cloneDemoProject(), canRestore: false, onCreateSnapshot: vi.fn(), onRestoreSnapshot: vi.fn() };
    render(<ReviewRail {...props} />);
    expect(await screen.findByRole("button", { name: "Restore Ready for review" })).toBeDisabled();
    act(() => useWorkspaceStore.getState().setReviewPanel("appearance"));
    const appearance = screen.getByRole("complementary", { name: "Appearance" });
    await user.click(within(appearance).getByRole("button", { name: "High contrast" }));
    expect(useWorkspaceStore.getState().theme).toBe("contrast");
    expect(within(appearance).getByRole("button", { name: "High contrast" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByText("Comments")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Close panel" }));
    expect(screen.queryByRole("complementary")).not.toBeInTheDocument();
  });

  it("keeps saving busy through panel changes and ignores a history response older than the new version", async () => {
    const user = userEvent.setup();
    let finishLoad: (entries: HistoryEntry[]) => void = () => undefined;
    vi.spyOn(desktopBridge, "listHistory").mockResolvedValueOnce([]).mockImplementation(() => new Promise((resolve) => { finishLoad = resolve; }));
    let finishCreate: (entry: HistoryEntry) => void = () => undefined;
    const onCreateSnapshot = vi.fn<(name: string) => Promise<HistoryEntry>>().mockImplementation(() => new Promise((resolve) => { finishCreate = resolve; }));
    useWorkspaceStore.setState({ reviewPanel: "history" });
    render(<ReviewRail project={cloneDemoProject()} canRestore onCreateSnapshot={onCreateSnapshot} onRestoreSnapshot={vi.fn()} />);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Name current version" })).toBeEnabled());
    await user.type(screen.getByRole("textbox", { name: "Name current version" }), "Ready for review");
    await user.click(screen.getByRole("button", { name: "Save named version" }));
    act(() => useWorkspaceStore.getState().setReviewPanel("appearance"));
    act(() => useWorkspaceStore.getState().setReviewPanel("history"));
    act(() => { finishLoad([]); });
    await waitFor(() => expect(screen.queryByText("Loading local history…")).not.toBeInTheDocument());
    expect(screen.getByRole("button", { name: "Saving version…" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "Name current version" })).toBeDisabled();

    act(() => useWorkspaceStore.getState().setReviewPanel("appearance"));
    act(() => useWorkspaceStore.getState().setReviewPanel("history"));
    act(() => { finishCreate(historyEntry); });
    expect(await screen.findByRole("button", { name: "Restore Ready for review" })).toBeEnabled();
    act(() => { finishLoad([]); });
    await waitFor(() => expect(screen.getByRole("button", { name: "Restore Ready for review" })).toBeEnabled());
    expect(onCreateSnapshot).toHaveBeenCalledOnce();
  });

  it("distinguishes restoration in progress from a draft error when version actions are disabled", async () => {
    const user = userEvent.setup();
    const project = cloneDemoProject();
    vi.spyOn(desktopBridge, "listHistory").mockResolvedValue([historyEntry]);
    let finishRestore: (restored: typeof project) => void = () => undefined;
    const onRestoreSnapshot = vi.fn<(snapshotId: string) => Promise<typeof project>>().mockImplementation(() => new Promise((resolve) => { finishRestore = resolve; }));
    const props = { project, canRestore: true, onCreateSnapshot: vi.fn(), onRestoreSnapshot };
    useWorkspaceStore.setState({ reviewPanel: "history" });
    const { rerender } = render(<ReviewRail {...props} />);
    await user.click(await screen.findByRole("button", { name: "Restore Ready for review" }));
    rerender(<ReviewRail {...props} disabled />);
    expect(screen.getByRole("status")).toHaveTextContent("Restoring version…");
    expect(screen.queryByText("Resolve the draft error before saving a version.")).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Name current version" })).toBeDisabled();
    act(() => { finishRestore(project); });
    await waitFor(() => expect(screen.queryByText("Restoring version…")).not.toBeInTheDocument());
    rerender(<ReviewRail {...props} disabled disabledReason="Resolve the draft error before saving a version." />);
    expect(screen.getByText("Resolve the draft error before saving a version.")).toBeInTheDocument();
  });
});

function observePaneSize() {
  const observers: PaneResizeObserver[] = [];
  class PaneResizeObserver implements ResizeObserver {
    readonly callback: ResizeObserverCallback;
    target: Element | null = null;
    constructor(callback: ResizeObserverCallback) { this.callback = callback; observers.push(this); }
    observe(target: Element) { this.target = target; }
    unobserve() { this.target = null; }
    disconnect() { this.target = null; }
    resize(width: number) {
      if (this.target === null) return;
      const size = { inlineSize: width, blockSize: 700 };
      this.callback([{ target: this.target, contentRect: new DOMRectReadOnly(0, 0, width, 700), contentBoxSize: [size], borderBoxSize: [size], devicePixelContentBoxSize: [size] }], this);
    }
  }
  globalThis.ResizeObserver = PaneResizeObserver;
  return (width: number) => act(() => { for (const observer of observers) observer.resize(width); });
}

describe("responsive split workspace", () => {
  it("resizes by pointer and keyboard within bounds and remembers the ratio after remounting", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<SplitWorkspace editor={<input aria-label="Draft" />} preview={{}} />);
    const splitter = screen.getByRole("separator", { name: "Resize editor and PDF preview" });
    const parent = splitter.parentElement;
    if (parent === null) throw new Error("Missing split container");
    vi.spyOn(parent, "getBoundingClientRect").mockReturnValue(new DOMRect(100, 0, 1000, 700));
    Object.defineProperty(splitter, "setPointerCapture", { value: vi.fn() });
    fireEvent.pointerDown(splitter, { pointerId: 1, button: 0, clientX: 640 });
    fireEvent.pointerMove(splitter, { pointerId: 1, clientX: 750 });
    expect(splitter).toHaveAttribute("aria-valuenow", "65");
    fireEvent.pointerMove(splitter, { pointerId: 1, clientX: 1500 });
    expect(splitter).toHaveAttribute("aria-valuenow", "70");
    fireEvent.pointerUp(splitter, { pointerId: 1 });
    fireEvent.pointerMove(splitter, { pointerId: 1, clientX: 100 });
    expect(splitter).toHaveAttribute("aria-valuenow", "70");
    splitter.focus();
    await user.keyboard("{Home}{ArrowLeft}");
    expect(splitter).toHaveAttribute("aria-valuenow", "35");
    await user.keyboard("{End}{ArrowRight}{ArrowLeft}");
    expect(splitter).toHaveAttribute("aria-valuenow", "68");
    unmount();
    render(<SplitWorkspace editor={<input aria-label="Draft" />} preview={{}} />);
    expect(screen.getByRole("separator")).toHaveAttribute("aria-valuenow", "68");
  });

  it("keeps the same editor mounted through compact PDF toggles and all workspace modes", async () => {
    const user = userEvent.setup();
    const resize = observePaneSize();
    const editor = <input aria-label="Draft" defaultValue="Preserved text" />;
    const { rerender } = render(<SplitWorkspace editor={editor} preview={{}} />);
    const draft = screen.getByRole("textbox", { name: "Draft" });
    await user.type(draft, " with edits");
    resize(899);
    expect(screen.queryByRole("separator")).not.toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "PDF" }));
    expect(draft).not.toBeVisible();
    expect(screen.getByRole("tabpanel", { name: "PDF" })).toBeVisible();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Editor" })).toHaveFocus();
    expect(draft).toBeVisible();
    resize(900);
    expect(screen.getByRole("separator")).toBeInTheDocument();
    expect(screen.queryByRole("tablist", { name: "Split pane" })).not.toBeInTheDocument();
    rerender(<SplitWorkspace editor={editor} preview={{}} mode="preview" />);
    expect(draft).not.toBeVisible();
    rerender(<SplitWorkspace editor={editor} preview={{}} mode="source" />);
    expect(screen.getByRole("textbox", { name: "Draft" })).toBe(draft);
    expect(screen.queryByRole("region", { name: "Compiled PDF preview" })).not.toBeInTheDocument();
    rerender(<SplitWorkspace editor={editor} preview={{}} mode="write" />);
    expect(draft).toHaveValue("Preserved text with edits");
  });

  it("reveals the editor when file or heading navigation changes while compact PDF is selected", async () => {
    const user = userEvent.setup();
    const resize = observePaneSize();
    const editor = <input aria-label="Draft" defaultValue="Keep my draft" />;
    const { rerender } = render(<SplitWorkspace editor={editor} preview={{}} navigationKey="main:1" />);
    resize(700);
    const draft = screen.getByRole("textbox", { name: "Draft" });
    await user.click(screen.getByRole("tab", { name: "PDF" }));
    expect(draft).not.toBeVisible();
    rerender(<SplitWorkspace editor={editor} preview={{}} navigationKey="chapter:2" />);
    expect(screen.getByRole("tab", { name: "Editor" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("textbox", { name: "Draft" })).toBe(draft);
    await user.click(screen.getByRole("tab", { name: "PDF" }));
    expect(draft).not.toBeVisible();
    rerender(<SplitWorkspace editor={editor} preview={{}} navigationKey="chapter:3" />);
    expect(draft).toBeVisible();
  });
});

describe("PDF readiness", () => {
  it("explains the actual unavailable runtime and keeps compile access visible", () => {
    render(<PreviewPane runtimeReason="The compiler runtime has not been installed." projectTitle="A new paper" />);
    expect(screen.getByText("The compiler runtime has not been installed.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Compile unavailable" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Compile unavailable" })).toHaveAttribute("title", "The compiler runtime has not been installed.");
  });

  it("compiles through the supplied callback and reports in-progress state", async () => {
    const user = userEvent.setup();
    const onCompile = vi.fn();
    const { rerender } = render(<PreviewPane projectTitle="A new paper" onCompile={onCompile} />);
    expect(screen.getByText("Compile “A new paper” to see its PDF here.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Compile" }));
    expect(onCompile).toHaveBeenCalledOnce();
    rerender(<PreviewPane onCompile={onCompile} compileStatus="compiling" />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Compiling…" })).toBeDisabled());
  });
});
