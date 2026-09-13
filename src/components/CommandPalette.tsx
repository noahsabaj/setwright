import { useEffect, useId, useRef, useState } from "react";
import { Code2, Columns2, Contrast, FilePenLine, FileText, History, Moon, Palette, Search, Sun, X } from "lucide-react";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";
import { shortcutLabel } from "../lib/keyboard";
import { useWorkspaceStore } from "../store/workspace-store";

interface CommandItem {
  id: string;
  label: string;
  detail: string;
  icon: typeof Search;
  shortcut?: string;
  action: () => void;
}

export function CommandPalette({ canWrite = true }: { canWrite?: boolean }) {
  const open = useWorkspaceStore((state) => state.commandPaletteOpen);
  return open ? <OpenCommandPalette canWrite={canWrite} /> : null;
}

function OpenCommandPalette({ canWrite }: { canWrite: boolean }) {
  const setOpen = useWorkspaceStore((state) => state.setCommandPaletteOpen);
  const setMode = useWorkspaceStore((state) => state.setMode);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const setTheme = useWorkspaceStore((state) => state.setTheme);
  const theme = useWorkspaceStore((state) => state.theme);
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  const commands: CommandItem[] = [
    ...(canWrite ? [{ id: "write", label: "Switch to Write", detail: "Focus on your paper", icon: FilePenLine, shortcut: shortcutLabel("1"), action: () => setMode("write") }] : []),
    { id: "source", label: "Switch to Source", detail: "Edit the source file", icon: Code2, shortcut: shortcutLabel("2"), action: () => setMode("source") },
    { id: "preview", label: "Switch to Preview", detail: "Read the latest PDF", icon: FileText, shortcut: shortcutLabel("3"), action: () => setMode("preview") },
    { id: "split", label: "Switch to Split", detail: "Work alongside the PDF", icon: Columns2, shortcut: shortcutLabel("4"), action: () => setMode("split") },
    { id: "history", label: "Open version history", detail: "Name or restore a saved version", icon: History, action: () => setReviewPanel("history") },
    { id: "appearance", label: "Open appearance", detail: "Choose your workspace theme", icon: Palette, action: () => setReviewPanel("appearance") },
    { id: "light", label: "Use light theme", detail: "A bright writing workspace", icon: Sun, action: () => setTheme("light") },
    { id: "dark", label: "Use dark theme", detail: "A quiet workspace for low light", icon: Moon, action: () => setTheme("dark") },
    { id: "contrast", label: "Use high contrast theme", detail: "Stronger borders and text", icon: Contrast, action: () => setTheme("contrast") },
  ];
  const results = commands.filter((command) => `${command.label} ${command.detail}`.toLowerCase().includes(query.trim().toLowerCase()));
  const activeIndex = Math.min(selectedIndex, Math.max(0, results.length - 1));
  const selectedCommand = results[activeIndex];

  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, query]);

  const run = (action: () => void) => {
    action();
    setOpen(false);
  };

  return (
    <ModalOverlay className="modal-layer modal-layer--palette" data-theme={theme} isOpen isDismissable onOpenChange={setOpen}>
      <Modal className="command-palette">
        <Dialog aria-label="Command palette">
          <div className="command-palette__search">
            <Search size={18} aria-hidden="true" />
            <input
              autoFocus aria-label="Search commands" role="combobox" aria-expanded="true" aria-controls={listId}
              aria-autocomplete="list" aria-activedescendant={selectedCommand === undefined ? undefined : `${listId}-${selectedCommand.id}`}
              value={query} placeholder="Search commands…"
              onChange={(event) => { setQuery(event.target.value); setSelectedIndex(0); }}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  if (results.length > 0) setSelectedIndex((activeIndex + (event.key === "ArrowDown" ? 1 : -1) + results.length) % results.length);
                } else if (event.key === "Enter") {
                  event.preventDefault();
                  if (selectedCommand !== undefined) run(selectedCommand.action);
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  event.stopPropagation();
                  setOpen(false);
                }
              }}
            />
            <button type="button" aria-label="Close command palette" onClick={() => setOpen(false)}><X size={16} /></button>
          </div>
          <div className="command-palette__meta"><span role="status">{query === "" ? "Available commands" : `${String(results.length)} results`}</span><span>↑ ↓ to move · Enter to run</span></div>
          <div className="command-list" role="listbox" aria-label="Commands" id={listId} ref={listRef}>
            {results.map((command, index) => {
              const Icon = command.icon;
              return (
                <button type="button" role="option" tabIndex={-1} aria-selected={index === activeIndex} id={`${listId}-${command.id}`} key={command.id}
                  onPointerMove={() => setSelectedIndex(index)} onMouseDown={(event) => event.preventDefault()} onClick={() => run(command.action)}>
                  <span className="command-list__icon"><Icon size={17} aria-hidden="true" /></span>
                  <span><strong>{command.label}</strong><small>{command.detail}</small></span>
                  {command.shortcut === undefined ? null : <kbd>{command.shortcut}</kbd>}
                </button>
              );
            })}
            {results.length === 0 ? <p className="command-empty">No matching commands. Try “source”, “history”, or “theme”.</p> : null}
          </div>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
