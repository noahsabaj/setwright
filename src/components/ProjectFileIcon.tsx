import { File, FileImage, FileText, Library } from "lucide-react";
import type { ProjectFile } from "../lib/contracts";

interface ProjectFileIconProps {
  kind: ProjectFile["kind"];
}

export function ProjectFileIcon({ kind }: ProjectFileIconProps) {
  if (kind === "asset") return <FileImage size={14} aria-hidden="true" />;
  if (kind === "bib") return <Library size={14} aria-hidden="true" />;
  if (kind === "tex") return <FileText size={14} aria-hidden="true" />;
  return <File size={14} aria-hidden="true" />;
}
