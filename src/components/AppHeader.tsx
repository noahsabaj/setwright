import { useState } from "react";
import { ChevronDown, FolderOpen, History, Info, Palette, PanelLeftClose, PanelLeftOpen, Search, X } from "lucide-react";
import { Button, Dialog, Heading, Menu, MenuItem, MenuTrigger, Modal, ModalOverlay, Popover } from "react-aria-components";
import type { ProjectSnapshot } from "../lib/contracts";
import { shortcutLabel } from "../lib/keyboard";
import { useWorkspaceStore } from "../store/workspace-store";
import { BrandMark } from "./BrandMark";
import { IconButton } from "./IconButton";
import { ModeSwitcher } from "./ModeSwitcher";

interface AppHeaderProps {
  project: ProjectSnapshot;
  onOpenAnother: () => void;
  onClosePaper?: (() => void) | undefined;
  activeFileName?: string;
  canWrite?: boolean;
}

export function AppHeader({ project, onOpenAnother, onClosePaper, activeFileName, canWrite = true }: AppHeaderProps) {
  const outlineOpen = useWorkspaceStore((state) => state.outlineOpen);
  const toggleOutline = useWorkspaceStore((state) => state.toggleOutline);
  const setReviewPanel = useWorkspaceStore((state) => state.setReviewPanel);
  const setCommandPaletteOpen = useWorkspaceStore((state) => state.setCommandPaletteOpen);
  const theme = useWorkspaceStore((state) => state.theme);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const mainFile = project.files.find((file) => file.id === project.mainFile);

  return (
    <header className="app-header">
      <div className="app-header__left">
        <BrandMark compact />
        <span className="app-header__rule" aria-hidden="true" />
        <IconButton label={outlineOpen ? "Hide project outline" : "Show project outline"} onPress={toggleOutline}>
          {outlineOpen ? <PanelLeftClose size={17} /> : <PanelLeftOpen size={17} />}
        </IconButton>
        <MenuTrigger>
          <Button className="project-menu" aria-label={`Project menu: ${project.title}`}>
            <span className="project-menu__title">{project.title}</span>
            <span className="project-menu__path">{activeFileName ?? mainFile?.relativePath ?? "Paper"}</span>
            <ChevronDown size={14} aria-hidden="true" />
          </Button>
          <Popover className="workspace-popover" data-theme={theme} placement="bottom start">
            <Menu className="project-actions" aria-label="Project actions">
              <MenuItem id="details" onAction={() => setDetailsOpen(true)} textValue="Project details"><Info size={16} />Project details</MenuItem>
              <MenuItem id="open" onAction={onOpenAnother} textValue="Open another paper"><FolderOpen size={16} />Open another paper</MenuItem>
              {onClosePaper ? <MenuItem id="close" onAction={onClosePaper} textValue="Save and close paper"><X size={16} />Save and close paper</MenuItem> : null}
              <MenuItem id="history" onAction={() => setReviewPanel("history")} textValue="Version history"><History size={16} />Version history</MenuItem>
              <MenuItem id="appearance" onAction={() => setReviewPanel("appearance")} textValue="Appearance"><Palette size={16} />Appearance</MenuItem>
            </Menu>
          </Popover>
        </MenuTrigger>
      </div>
      <ModeSwitcher canWrite={canWrite} />
      <div className="app-header__actions">
        <button className="command-button" type="button" aria-label="Search commands" title={`Search commands (${shortcutLabel("K")})`} onClick={() => setCommandPaletteOpen(true)}>
          <Search size={16} aria-hidden="true" /><span>Search commands</span><kbd>{shortcutLabel("K")}</kbd>
        </button>
      </div>
      <ModalOverlay className="modal-layer" data-theme={theme} isOpen={detailsOpen} isDismissable onOpenChange={setDetailsOpen}>
        <Modal className="project-details">
          <Dialog aria-label="Project details">
            <header><Heading slot="title">Project details</Heading><Button className="icon-button" aria-label="Close project details" onPress={() => setDetailsOpen(false)}><X size={18} /></Button></header>
            <dl><dt>Paper</dt><dd>{project.title}</dd><dt>Folder</dt><dd>{project.rootPath}</dd><dt>Main file</dt><dd>{mainFile?.relativePath ?? "Unavailable"}</dd><dt>Files</dt><dd>{project.files.length}</dd><dt>Compiler</dt><dd>{project.settings.engine}</dd></dl>
          </Dialog>
        </Modal>
      </ModalOverlay>
    </header>
  );
}
