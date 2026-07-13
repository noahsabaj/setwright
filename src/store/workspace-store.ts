import { create } from "zustand";
import type { SaveState, ThemePreference, WorkspaceMode } from "../lib/contracts";

export type ReviewPanel = "comments" | "history" | null;

interface WorkspaceState {
  mode: WorkspaceMode;
  theme: ThemePreference;
  saveState: SaveState;
  outlineOpen: boolean;
  reviewPanel: ReviewPanel;
  commandPaletteOpen: boolean;
  compileState: "idle" | "compiling" | "success" | "failed";
  sourceLine: number;
  sourceColumn: number;
  previewPage: number;
  previewZoom: number;
  previewScrollTop: number;
  setMode: (mode: WorkspaceMode) => void;
  setTheme: (theme: ThemePreference) => void;
  setSaveState: (saveState: SaveState) => void;
  toggleOutline: () => void;
  setReviewPanel: (reviewPanel: ReviewPanel) => void;
  setCommandPaletteOpen: (open: boolean) => void;
  setCompileState: (compileState: WorkspaceState["compileState"]) => void;
  setSourcePosition: (line: number, column: number) => void;
  setPreviewPage: (page: number) => void;
  setPreviewZoom: (zoom: number) => void;
  setPreviewScrollTop: (scrollTop: number) => void;
}

export const useWorkspaceStore = create<WorkspaceState>((set) => ({
  mode: "split",
  theme: "light",
  saveState: "saved",
  outlineOpen: true,
  reviewPanel: null,
  commandPaletteOpen: false,
  compileState: "idle",
  sourceLine: 1,
  sourceColumn: 1,
  previewPage: 1,
  previewZoom: 82,
  previewScrollTop: 0,
  setMode: (mode) => set({ mode }),
  setTheme: (theme) => set({ theme }),
  setSaveState: (saveState) => set({ saveState }),
  toggleOutline: () => set((state) => ({ outlineOpen: !state.outlineOpen })),
  setReviewPanel: (reviewPanel) => set({ reviewPanel }),
  setCommandPaletteOpen: (commandPaletteOpen) => set({ commandPaletteOpen }),
  setCompileState: (compileState) => set({ compileState }),
  setSourcePosition: (sourceLine, sourceColumn) => set({ sourceLine, sourceColumn }),
  setPreviewPage: (previewPage) => set({ previewPage }),
  setPreviewZoom: (previewZoom) => set({ previewZoom }),
  setPreviewScrollTop: (previewScrollTop) => set({ previewScrollTop }),
}));
