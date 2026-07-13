import { useState } from "react";
import { ArrowRight, Check, FileText, FolderOpen, ShieldCheck, WifiOff } from "lucide-react";
import type { ProjectSnapshot, TemplateId } from "../lib/contracts";
import { desktopBridge } from "../lib/bridge";
import { BrandMark } from "./BrandMark";

interface WelcomeScreenProps {
  onEnter: (project: ProjectSnapshot) => void;
}

const templates = [
  {
    id: "generic-article",
    label: "Research article",
    detail: "A clean, journal-neutral starting point",
  },
  { id: "acm-acmart", label: "ACM manuscript", detail: "acmart conference review format" },
  { id: "ieee-ieeetran", label: "IEEE paper", detail: "IEEEtran two-column conference format" },
] as const satisfies ReadonlyArray<{ id: TemplateId; label: string; detail: string }>;

export function WelcomeScreen({ onEnter }: WelcomeScreenProps) {
  const [templateId, setTemplateId] = useState<TemplateId>("generic-article");
  const [title, setTitle] = useState("Untitled research paper");
  const [author, setAuthor] = useState("First Author");
  const [folderName, setFolderName] = useState("untitled-paper");
  const [busy, setBusy] = useState<"create" | "open" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleCreate = async () => {
    setBusy("create");
    setError(null);
    try {
      const parentDirectory = await desktopBridge.pickCreateParentDirectory();
      if (parentDirectory === null) return;
      const project = await desktopBridge.createProject({
        parentDirectory,
        folderName,
        title,
        authors: [author.trim()],
        templateId,
        engine: "pdflatex",
      });
      onEnter(project);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The paper could not be created.");
    } finally {
      setBusy(null);
    }
  };

  const handleOpen = async () => {
    setBusy("open");
    setError(null);
    try {
      const projectPath = await desktopBridge.pickProjectPath();
      if (projectPath === null) return;
      onEnter(await desktopBridge.openProject(projectPath));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The project could not be opened.");
    } finally {
      setBusy(null);
    }
  };

  return (
    <main className="welcome" aria-labelledby="welcome-title">
      <section className="welcome__brand">
        <BrandMark inverse />
        <div className="welcome__statement">
          <h1 id="welcome-title">Write papers, not TeX.</h1>
          <p>A local-first, open-source visual editor for LaTeX research papers.</p>
        </div>
        <p className="welcome__edition">Desktop preview · Apache 2.0</p>
      </section>

      <section className="welcome__start" aria-labelledby="start-title">
        <div className="welcome__start-inner">
          <div className="eyebrow">Start writing</div>
          <h2 id="start-title">Your source stays yours.</h2>
          <p className="welcome__intro">
            Setwright works directly with ordinary <code>.tex</code> and <code>.bib</code> files. No account,
            cloud sync, or conversion step.
          </p>

          <fieldset className="template-picker">
            <legend>Choose a template</legend>
            {templates.map((template) => (
              <label className="template-option" key={template.id}>
                <input
                  type="radio"
                  name="template"
                  value={template.id}
                  checked={templateId === template.id}
                  onChange={() => setTemplateId(template.id)}
                />
                <span className="template-option__icon" aria-hidden="true">
                  <FileText size={18} strokeWidth={1.8} />
                </span>
                <span className="template-option__copy">
                  <strong>{template.label}</strong>
                  <span>{template.detail}</span>
                </span>
                <span className="template-option__check" aria-hidden="true">
                  <Check size={14} />
                </span>
              </label>
            ))}
          </fieldset>

          <div className="project-fields">
            <label>
              <span>Paper title</span>
              <input value={title} onChange={(event) => setTitle(event.target.value)} required />
            </label>
            <label>
              <span>Lead author</span>
              <input value={author} onChange={(event) => setAuthor(event.target.value)} autoComplete="name" required />
            </label>
            <label>
              <span>Folder name</span>
              <input value={folderName} onChange={(event) => setFolderName(event.target.value)} required pattern="[^/\\]+" />
            </label>
          </div>

          <button className="primary-button primary-button--large" type="button" onClick={() => void handleCreate()} disabled={busy !== null || title.trim() === "" || author.trim() === "" || folderName.trim() === ""}>
            {busy === "create" ? "Creating paper…" : "Create paper"}
            <ArrowRight size={17} aria-hidden="true" />
          </button>

          <button className="quiet-button quiet-button--large" type="button" onClick={() => void handleOpen()} disabled={busy !== null}>
            <FolderOpen size={17} aria-hidden="true" />
            {busy === "open" ? "Opening…" : "Choose an existing paper's main .tex file"}
          </button>

          {error === null ? null : (
            <p className="inline-error" role="alert">
              {error}
            </p>
          )}

          <div className="welcome__promises" aria-label="Setwright privacy promises">
            <span><WifiOff size={15} aria-hidden="true" /> Works offline</span>
            <span><ShieldCheck size={15} aria-hidden="true" /> No telemetry</span>
          </div>
        </div>
      </section>
    </main>
  );
}
