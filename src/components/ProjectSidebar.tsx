import { useMemo, useState } from "react";
import type { CSSProperties } from "react";
import { AlertTriangle, BookOpen, Hash, Search } from "lucide-react";
import type { ProjectMetrics } from "../lib/project-metrics";
import type { ProjectSnapshot } from "../lib/contracts";
import { ProjectFileIcon } from "./ProjectFileIcon";

interface ProjectSidebarProps {
  project: ProjectSnapshot;
  metrics: ProjectMetrics;
}

export function ProjectSidebar({ project, metrics }: ProjectSidebarProps) {
  const [query, setQuery] = useState("");
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visibleOutline = useMemo(() => metrics.outline.filter((item) => (
    normalizedQuery === ""
    || item.label?.toLocaleLowerCase().includes(normalizedQuery) === true
    || item.filePath.toLocaleLowerCase().includes(normalizedQuery)
  )), [metrics.outline, normalizedQuery]);
  const visibleFiles = useMemo(() => project.files.filter((file) => (
    normalizedQuery === "" || file.relativePath.toLocaleLowerCase().includes(normalizedQuery)
  )), [normalizedQuery, project.files]);

  return (
    <aside className="project-sidebar" aria-label="Project and document outline">
      <div className="sidebar-search">
        <Search size={14} aria-hidden="true" />
        <input
          aria-label="Filter outline and files"
          placeholder="Filter"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>

      <section className="sidebar-section" aria-labelledby="outline-label">
        <div className="sidebar-section__heading" id="outline-label">
          <span>Outline</span>
          <span className="sidebar-count">{metrics.outline.length}</span>
        </div>
        {metrics.outlineStatus === "unavailable" ? (
          <p className="sidebar-empty">{metrics.outlineNote}</p>
        ) : visibleOutline.length === 0 ? (
          <p className="sidebar-empty">{normalizedQuery === "" ? "No static headings found" : "No matching headings"}</p>
        ) : (
          <ol className="outline-list">
            {visibleOutline.map((item) => (
              <li key={item.id}>
                <div
                  className="outline-entry"
                  style={{ "--outline-depth": item.depth } as CSSProperties}
                  title={item.filePath}
                >
                  <span className="outline-marker" aria-hidden="true">{item.kind === "abstract" ? "A" : "§"}</span>
                  <span>{item.label ?? "Heading title unavailable"}</span>
                </div>
              </li>
            ))}
          </ol>
        )}
        {metrics.outlineStatus === "partial" && metrics.outlineNote !== null ? <p className="sidebar-note">{metrics.outlineNote}</p> : null}
      </section>

      <section className="sidebar-section sidebar-section--files" aria-labelledby="files-label">
        <div className="sidebar-section__heading" id="files-label">
          <span>Project files</span>
          <span className="sidebar-count">{project.files.length}</span>
        </div>
        {visibleFiles.length === 0 ? <p className="sidebar-empty">No matching files</p> : (
          <ul className="file-list">
            {visibleFiles.map((file) => (
              <li key={file.id}>
                <div className={`file-entry${file.id === project.mainFile ? " is-active" : ""}`}>
                  <ProjectFileIcon kind={file.kind} />
                  <span>{file.relativePath}</span>
                  {file.dirty ? <span className="dirty-dot" aria-label="Unsaved" /> : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <div className="sidebar-footer" aria-label="Project source metrics">
        <div className="sidebar-tool" title="Bibliography entries found in UTF-8 project .bib files">
          <BookOpen size={15} aria-hidden="true" />
          <span>References</span>
          <span className="sidebar-count">{metrics.referenceCount ?? "—"}</span>
        </div>
        <div className="sidebar-tool" title="Label declarations found in UTF-8 project .tex files">
          <Hash size={15} aria-hidden="true" />
          <span>Labels</span>
          <span className="sidebar-count">{metrics.labelCount ?? "—"}</span>
        </div>
        <div className={`sidebar-tool${project.compatibility.length > 0 ? " sidebar-tool--warning" : ""}`}>
          <AlertTriangle size={15} aria-hidden="true" />
          <span>Compatibility</span>
          <span className="sidebar-count">{project.compatibility.length}</span>
        </div>
      </div>
    </aside>
  );
}
