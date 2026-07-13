import { useEffect, useRef, useState } from "react";
import { Code2, Columns2, FilePenLine, FileText, History, MessageSquareText, Moon, Search, Sun, X } from "lucide-react";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";
import { useWorkspaceStore } from "../store/workspace-store";

interface CommandItem {
  id: string;
  label: string;
  detail: string;
  icon: typeof Search;
  shortcut?: string;
  action: () => void;
}

export function CommandPalette() {
  const open = useWorkspaceStore((state) => state.commandPaletteOpen);
  const setOpen = useWorkspaceStore((state) => state.setCommandPaletteOpen);
  const setMode = useWorkspaceStore((state) => state.setMode);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const setTheme = useWorkspaceStore((state) => state.setTheme);
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return undefined;
    inputRef.current?.focus();
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [open, setOpen]);

  if (!open) return null;

  const run = (action: () => void) => {
    action();
    setOpen(false);
    setQuery("");
  };

  const commands: CommandItem[] = [
    { id: "write", label: "Switch to Write", detail: "Visual scientific editor", icon: FilePenLine, shortcut: "⌘1", action: () => setMode("write") },
    { id: "source", label: "Switch to Source", detail: "Edit authoritative LaTeX", icon: Code2, shortcut: "⌘2", action: () => setMode("source") },
    { id: "preview", label: "Switch to Preview", detail: "Show the latest compiled PDF or its unavailable state", icon: FileText, shortcut: "⌘3", action: () => setMode("preview") },
    { id: "split", label: "Switch to Split", detail: "Write beside the PDF preview", icon: Columns2, shortcut: "⌘4", action: () => setMode("split") },
    { id: "comments", label: "Open comments", detail: "Review bundle status", icon: MessageSquareText, action: () => setReviewPanel("comments") },
    { id: "history", label: "Open version history", detail: "Local recoverable snapshots", icon: History, action: () => setReviewPanel("history") },
    { id: "light", label: "Use light theme", detail: "Paper-white workspace", icon: Sun, action: () => setTheme("light") },
    { id: "dark", label: "Use dark theme", detail: "Low-light workspace", icon: Moon, action: () => setTheme("dark") },
  ];
  const results = commands.filter((command) => `${command.label} ${command.detail}`.toLowerCase().includes(query.toLowerCase()));

  return (
    <ModalOverlay className="modal-layer modal-layer--palette" isOpen={open} isDismissable onOpenChange={setOpen}>
      <Modal className="command-palette">
        <Dialog aria-label="Command palette">
        <label className="command-palette__search">
          <Search size={18} aria-hidden="true" />
          <input ref={inputRef} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search commands…" />
          <button type="button" aria-label="Close command palette" onClick={() => setOpen(false)}><X size={16} /></button>
        </label>
        <div className="command-palette__meta"><span>{query === "" ? "Available commands" : `${String(results.length)} results`}</span><span>Type to filter · choose a command</span></div>
        <div className="command-list" role="listbox" aria-label="Commands">
          {results.map((command, index) => {
            const Icon = command.icon;
            return (
              <button type="button" role="option" aria-selected={index === 0} key={command.id} onClick={() => run(command.action)}>
                <span className="command-list__icon"><Icon size={17} /></span>
                <span><strong>{command.label}</strong><small>{command.detail}</small></span>
                {command.shortcut === undefined ? null : <kbd>{command.shortcut}</kbd>}
              </button>
            );
          })}
          {results.length === 0 ? <p className="command-empty">No matching available commands.</p> : null}
        </div>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
