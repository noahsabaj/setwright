import { useState } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SourceEditor } from "../components/SourceEditor";
import type { SourceNavigation } from "../editor/navigation";

const source = "😀 Earlier Unicode\n\\section{Repeated}\nFirst section\n\\section{Repeated}\nSecond section\n";
const offset = source.lastIndexOf("\\section{Repeated}");
const request: SourceNavigation = { fileId: "method", sourceOffset: offset, requestId: 4 };

function sourceView(): EditorView {
  const editor = screen.getByRole("textbox", { name: "method.tex source" });
  const view = EditorView.findFromDOM(editor);
  if (view === null) throw new Error("Source editor is missing");
  return view;
}

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 600));
});
afterEach(() => vi.restoreAllMocks());

describe("Source navigation reveal", () => {
  it("focuses the exact Unicode-safe source offset after drawer focus restoration without changing source", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    function DrawerHarness() {
      const [open, setOpen] = useState(false);
      const [navigation, setNavigation] = useState<SourceNavigation>();
      return <>
        <button type="button" onClick={() => setOpen(true)}>Open outline</button>
        <SourceEditor value={source} fileId="method" fileName="method.tex" navigation={navigation} onChange={onChange} />
        <ModalOverlay isOpen={open} isDismissable onOpenChange={setOpen}>
          <Modal><Dialog aria-label="Outline"><button type="button" onClick={() => { setNavigation(request); setOpen(false); }}>Jump to second section</button></Dialog></Modal>
        </ModalOverlay>
      </>;
    }
    render(<DrawerHarness />);
    await user.click(screen.getByRole("button", { name: "Open outline" }));
    await user.click(screen.getByRole("button", { name: "Jump to second section" }));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "method.tex source" })).toHaveFocus());
    expect(sourceView().state.selection.main.head).toBe(offset);
    expect(sourceView().state.doc.toString()).toBe(source);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("waits until an ancestor becomes visible before completing navigation", async () => {
    const { rerender } = render(<div aria-hidden="true"><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} /></div>);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 55)); });
    const content = document.querySelector<HTMLElement>(".cm-content");
    expect(content).not.toHaveFocus();
    rerender(<div aria-hidden="false"><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} /></div>);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "method.tex source" })).toHaveFocus());
    expect(sourceView().state.selection.main.head).toBe(offset);
  });

  it("cancels pending navigation on a file switch and replays it when that file becomes active", async () => {
    const { rerender } = render(<><button type="button">Other file</button><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} /></>);
    rerender(<><button type="button">Other file</button><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} active={false} /></>);
    const other = screen.getByRole("button", { name: "Other file" });
    other.focus();
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 55)); });
    expect(other).toHaveFocus();
    rerender(<><button type="button">Other file</button><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} active /></>);
    await waitFor(() => expect(screen.getByRole("textbox", { name: "method.tex source" })).toHaveFocus());
    expect(sourceView().state.selection.main.head).toBe(offset);
  });

  it("does not reclaim focus or replay a request after a deliberate click elsewhere", async () => {
    const { rerender } = render(<><button type="button">Another control</button><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} /></>);
    const other = screen.getByRole("button", { name: "Another control" });
    fireEvent.pointerDown(other);
    other.focus();
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 55)); });
    expect(other).toHaveFocus();
    // Switching away and back must not revive the explicitly superseded request.
    rerender(<><button type="button">Another control</button><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} active={false} /></>);
    rerender(<><button type="button">Another control</button><SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} /></>);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 55)); });
    expect(other).toHaveFocus();
  });

  it("preserves a new caret position when the user starts typing before deferred reveal", async () => {
    render(<SourceEditor value={source} fileId="method" fileName="method.tex" navigation={request} />);
    const view = sourceView();
    view.focus();
    fireEvent.keyDown(view.contentDOM, { key: "ArrowRight" });
    view.dispatch({ selection: { anchor: source.length } });
    view.dispatch({ changes: { from: source.length, insert: "New ending" }, selection: { anchor: source.length + 10 } });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 55)); });
    expect(view.state.selection.main.head).toBe(source.length + 10);
    expect(view.state.doc.toString()).toBe(`${source}New ending`);
  });
});
