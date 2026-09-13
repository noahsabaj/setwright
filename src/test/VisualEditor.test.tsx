import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { VisualEditor } from "../components/VisualEditor";
import type { SourceNavigation } from "../editor/navigation";

vi.mock("mathlive", () => ({}));
afterEach(() => vi.restoreAllMocks());

describe("VisualEditor source synchronization", () => {
  it("opens and closes preserved source without emitting an edit", async () => {
    const user = userEvent.setup();
    const onSourceChange = vi.fn();
    render(<VisualEditor source={"\\unknown{keep exactly}\n"} fileId="main" onSourceChange={onSourceChange} />);
    const editor = await screen.findByLabelText("Paper editor");
    const details = editor.querySelector("details");
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
    const summary = details!.querySelector("summary")!;
    await user.click(summary);
    expect(details).toHaveAttribute("open");
    expect(within(details!).getByRole("textbox")).toHaveValue("\\unknown{keep exactly}\n");
    await user.click(summary);
    expect(details).not.toHaveAttribute("open");
    expect(onSourceChange).not.toHaveBeenCalled();
  });

  it("navigates duplicate headings by current source range after an earlier Unicode edit", async () => {
    const user = userEvent.setup();
    const source = "α😀 introduction\n\n\\section{Same}\nFirst.\n\n\\section{Same}\nSecond.\n";
    const onSourceChange = vi.fn();
    const onNavigationFallback = vi.fn();
    const props = { source, fileId: "main", onSourceChange, onNavigationFallback };
    const { rerender } = render(<VisualEditor {...props} />);
    const editor = await screen.findByLabelText("Paper editor");
    await user.click(editor);
    const paragraph = editor.querySelector("p")!;
    const range = document.createRange();
    range.selectNodeContents(paragraph);
    range.collapse(false);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    await user.keyboard(" café😀");
    await waitFor(() => expect(onSourceChange).toHaveBeenCalled());
    const currentSource = onSourceChange.mock.lastCall![0] as string;
    onSourceChange.mockClear();
    const navigation: SourceNavigation = { fileId: "main", sourceOffset: currentSource.lastIndexOf("\\section{Same}"), requestId: 1 };
    rerender(<VisualEditor {...props} source={currentSource} navigation={navigation} />);
    const lastHeading = editor.querySelectorAll("h2")[1]!;
    await waitFor(() => expect(lastHeading.contains(window.getSelection()?.anchorNode ?? null)).toBe(true));
    expect(onNavigationFallback).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox", { name: "Text style" })).toHaveValue("2");
    expect(screen.getByRole("button", { name: "Undo" })).toBeEnabled();
    expect(onSourceChange).not.toHaveBeenCalled();
  });

  it("falls back for source-only navigation and waits while the editor is inactive", async () => {
    const onNavigationFallback = vi.fn();
    const props = { source: "\\unknown{source}\n", fileId: "main", onSourceChange: vi.fn(), onNavigationFallback };
    const navigation = { fileId: "main", sourceOffset: 0, requestId: 1 };
    const { rerender } = render(<VisualEditor {...props} navigation={navigation} active={false} />);
    await screen.findByLabelText("Paper editor");
    expect(onNavigationFallback).not.toHaveBeenCalled();
    rerender(<VisualEditor {...props} navigation={navigation} active />);
    await waitFor(() => expect(onNavigationFallback).toHaveBeenCalledExactlyOnceWith(navigation));
  });

  it("waits for a drawer to release focus, then reveals the heading in its own scroller", async () => {
    const frames = new Map<number, FrameRequestCallback>();
    let nextFrameId = 0;
    vi.spyOn(globalThis, "requestAnimationFrame").mockImplementation((callback) => {
      const id = ++nextFrameId;
      frames.set(id, callback);
      return id;
    });
    vi.spyOn(globalThis, "cancelAnimationFrame").mockImplementation((id) => { frames.delete(id); });
    const nextFrame = async () => {
      await act(async () => {
        const pending = [...frames.values()];
        frames.clear();
        for (const callback of pending) callback(performance.now());
        await Promise.resolve();
      });
    };
    const onSourceChange = vi.fn();
    const navigation = { fileId: "main", sourceOffset: 0, requestId: 42 };
    const view = (hidden: boolean, target?: SourceNavigation) => <>
      <button type="button">Outline</button>
      <div aria-hidden={hidden}>
        <VisualEditor source={"\\section{Target}\nProse."} fileId="main" onSourceChange={onSourceChange} navigation={target} />
      </div>
    </>;
    const { rerender } = render(view(true));
    const editor = await screen.findByLabelText("Paper editor");
    const scroller = editor.closest(".visual-editor__scroller") as HTMLDivElement;
    const heading = editor.querySelector("h2")!;
    vi.spyOn(scroller, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 100, 600, 400));
    vi.spyOn(heading, "getBoundingClientRect").mockImplementation(() => new DOMRect(0, 800 - scroller.scrollTop, 500, 30));
    const scrollTo = vi.fn<(options: ScrollToOptions) => void>((options) => { scroller.scrollTop = options.top ?? 0; });
    Object.defineProperty(scroller, "scrollTo", { value: scrollTo });
    rerender(view(true, navigation));
    await nextFrame();
    await nextFrame();
    expect(scrollTo).not.toHaveBeenCalled();
    rerender(view(false, navigation));
    await act(async () => { await Promise.resolve(); });
    await nextFrame();
    screen.getByRole("button", { name: "Outline" }).focus();
    await nextFrame();
    expect(editor).toHaveFocus();
    expect(scrollTo).toHaveBeenCalledExactlyOnceWith({ top: 676, behavior: "instant" });
    expect(heading.getBoundingClientRect().top).toBe(124);
    expect(onSourceChange).not.toHaveBeenCalled();
    scroller.scrollTop = 200;
    rerender(view(true, navigation));
    rerender(view(false, navigation));
    await nextFrame();
    await nextFrame();
    expect(scroller.scrollTop).toBe(200);
    expect(scrollTo).toHaveBeenCalledOnce();

    // A newer deliberate interaction must win over a pending automatic reveal.
    rerender(view(false, { ...navigation, requestId: 43 }));
    const trigger = screen.getByRole("button", { name: "Outline" });
    fireEvent.pointerDown(trigger);
    trigger.focus();
    await nextFrame();
    await nextFrame();
    expect(trigger).toHaveFocus();
    expect(scroller.scrollTop).toBe(200);
    expect(scrollTo).toHaveBeenCalledOnce();
  });

  it("updates formatting state and clears undo on an external canonical replacement", async () => {
    const user = userEvent.setup();
    const onSourceChange = vi.fn();
    const { rerender } = render(<VisualEditor source="alpha" fileId="main" onSourceChange={onSourceChange} />);
    const editor = await screen.findByLabelText("Paper editor");
    await user.click(editor);
    await user.click(screen.getByRole("button", { name: "Bold" }));
    expect(screen.getByRole("button", { name: "Bold" })).toHaveAttribute("aria-pressed", "true");
    await user.keyboard("bold");
    await waitFor(() => expect(onSourceChange).toHaveBeenCalled());
    expect(screen.getByRole("button", { name: "Undo" })).toBeEnabled();
    rerender(<VisualEditor source="External restored text" fileId="main" onSourceChange={onSourceChange} />);
    await waitFor(() => expect(editor).toHaveTextContent("External restored text"));
    expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
    onSourceChange.mockClear();
    await act(async () => { await user.keyboard("{Control>}z{/Control}"); });
    expect(editor).toHaveTextContent("External restored text");
    expect(onSourceChange).not.toHaveBeenCalled();
  });

  it("keeps secondary formatting and figure insertion reachable from the compact toolbar", async () => {
    const user = userEvent.setup();
    const onSourceChange = vi.fn();
    render(<VisualEditor source="alpha" fileId="main" onSourceChange={onSourceChange} />);
    const editor = await screen.findByLabelText("Paper editor");
    await user.click(editor);
    await user.click(screen.getByRole("button", { name: "More formatting" }));
    await user.click(await screen.findByRole("menuitem", { name: "Underline" }));
    await waitFor(() => expect(editor).toHaveFocus());
    await user.keyboard("underlined");
    await waitFor(() => expect(onSourceChange).toHaveBeenCalled());
    expect(onSourceChange.mock.lastCall![0]).toContain("\\underline{underlined}");
    await user.click(screen.getByRole("button", { name: "Insert" }));
    await user.click(screen.getByRole("button", { name: "Figure" }));
    expect(screen.getByRole("dialog", { name: "Figure" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Project path" })).toBeInTheDocument();
  });

  it("keeps undo after an accepted source echo and switching the editor inactive", async () => {
    const user = userEvent.setup();
    const onSourceChange = vi.fn();
    const { rerender } = render(<VisualEditor source="alpha" fileId="main" onSourceChange={onSourceChange} />);

    const editor = await screen.findByLabelText("Paper editor");
    await user.click(editor);
    const text = editor.querySelector("p")?.firstChild;
    if (text === null || text === undefined) throw new Error("Expected the projected paragraph text node.");
    const range = document.createRange();
    range.setStart(text, text.textContent?.length ?? 0);
    range.collapse(true);
    window.getSelection()?.removeAllRanges();
    window.getSelection()?.addRange(range);
    await user.keyboard(" beta");
    await waitFor(() => expect(onSourceChange).toHaveBeenCalled());
    expect(onSourceChange.mock.lastCall?.[0]).not.toBe("alpha");
    expect(onSourceChange.mock.lastCall?.[2]).toBe("alpha");

    const accepted = onSourceChange.mock.lastCall![0] as string;
    rerender(<VisualEditor source={accepted} fileId="main" onSourceChange={onSourceChange} active={false} />);
    rerender(<VisualEditor source={accepted} fileId="main" onSourceChange={onSourceChange} active />);

    await user.keyboard("{Control>}z{/Control}");
    await waitFor(() => expect(onSourceChange).toHaveBeenLastCalledWith(
      "alpha",
      expect.any(Array),
      "alpha",
    ));
  });

  it("rolls an unsafe scientific-node transaction back instead of leaving an unsaved visual draft", async () => {
    const onSourceChange = vi.fn();
    const source = "\\begin{document}\n\\begin{equation}\nx\n\\end{equation}\n\\end{document}\n";
    render(<VisualEditor source={source} fileId="main" onSourceChange={onSourceChange} />);

    const equation = await screen.findByLabelText("Editable display equation");
    Object.defineProperty(equation, "value", { value: "\\input{outside}", writable: true, configurable: true });
    fireEvent.input(equation);

    expect(await screen.findByRole("alert")).toHaveTextContent(/safe math subset/u);
    expect(onSourceChange).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByLabelText("Editable display equation")).toHaveAttribute("value", "x"));
  });
});
