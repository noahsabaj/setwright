import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { VisualEditor } from "../components/VisualEditor";

vi.mock("mathlive", () => ({}));

describe("VisualEditor source synchronization", () => {
  it("emits an inverse source change when Tiptap undo returns to the projection source", async () => {
    const user = userEvent.setup();
    const onSourceChange = vi.fn();
    render(<VisualEditor source="alpha" fileId="main" onSourceChange={onSourceChange} />);

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
