import { ChevronDown, Command, History, MessageSquareText, PanelLeftClose, PanelLeftOpen, Search } from "lucide-react";
import type { ProjectSnapshot } from "../lib/contracts";
import { useWorkspaceStore } from "../store/workspace-store";
import { BrandMark } from "./BrandMark";
import { IconButton } from "./IconButton";
import { ModeSwitcher } from "./ModeSwitcher";

interface AppHeaderProps {
  project: ProjectSnapshot;
  onOpenAnother: () => void;
}

export function AppHeader({ project, onOpenAnother }: AppHeaderProps) {
  const outlineOpen = useWorkspaceStore((state) => state.outlineOpen);
  const toggleOutline = useWorkspaceStore((state) => state.toggleOutline);
  const reviewPanel = useWorkspaceStore((state) => state.reviewPanel);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const setCommandPaletteOpen = useWorkspaceStore((state) => state.setCommandPaletteOpen);
  const mainFile = project.files.find((file) => file.id === project.mainFile);

  return (
    <header className="app-header">
      <div className="app-header__left">
        <BrandMark compact />
        <span className="app-header__rule" aria-hidden="true" />
        <IconButton label={outlineOpen ? "Hide project outline" : "Show project outline"} onPress={toggleOutline}>
          {outlineOpen ? <PanelLeftClose size={17} /> : <PanelLeftOpen size={17} />}
        </IconButton>
        <button className="project-menu" type="button" title={`${project.rootPath} · Open another project in a new window`} onClick={onOpenAnother}>
          <span className="project-menu__title">{project.title}</span>
          <span className="project-menu__path">{mainFile?.relativePath ?? "Main source unavailable"}</span>
          <ChevronDown size={14} aria-hidden="true" />
        </button>
      </div>

      <ModeSwitcher />

      <div className="app-header__actions">
        <button className="command-button" type="button" onClick={() => setCommandPaletteOpen(true)}>
          <Search size={15} aria-hidden="true" />
          <span>Search or run a command</span>
          <kbd><Command size={11} />K</kbd>
        </button>
        <IconButton
          label="Comments"
          active={reviewPanel === "comments"}
          onPress={() => setReviewPanel(reviewPanel === "comments" ? null : "comments")}
        >
          <MessageSquareText size={17} />
        </IconButton>
        <IconButton
          label="Version history"
          active={reviewPanel === "history"}
          onPress={() => setReviewPanel(reviewPanel === "history" ? null : "history")}
        >
          <History size={17} />
        </IconButton>
      </div>
    </header>
  );
}
