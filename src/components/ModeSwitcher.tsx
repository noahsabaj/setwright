import { Code2, Columns2, FilePenLine, FileText } from "lucide-react";
import { Tab, TabList, Tabs } from "react-aria-components";
import type { WorkspaceMode } from "../lib/contracts";
import { useWorkspaceStore } from "../store/workspace-store";

const modes = [
  { id: "write", label: "Write", icon: FilePenLine },
  { id: "source", label: "Source", icon: Code2 },
  { id: "preview", label: "Preview", icon: FileText },
  { id: "split", label: "Split", icon: Columns2 },
] as const satisfies ReadonlyArray<{ id: WorkspaceMode; label: string; icon: typeof FilePenLine }>;

export function ModeSwitcher() {
  const mode = useWorkspaceStore((state) => state.mode);
  const setMode = useWorkspaceStore((state) => state.setMode);

  return (
    <Tabs selectedKey={mode} onSelectionChange={(key) => setMode(String(key) as WorkspaceMode)}>
      <TabList className="mode-switcher" aria-label="Workspace view">
        {modes.map(({ id, label, icon: Icon }) => (
          <Tab className="mode-switcher__button" data-active={mode === id} id={id} key={id}>
            <Icon size={14} strokeWidth={1.9} aria-hidden="true" />
            <span>{label}</span>
          </Tab>
        ))}
      </TabList>
    </Tabs>
  );
}
