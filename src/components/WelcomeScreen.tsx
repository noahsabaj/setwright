import { useState } from "react";
import { ArrowLeft, ArrowRight, Check, FilePlus2, FileText, FolderOpen } from "lucide-react";
import type { ProjectSnapshot, TemplateId } from "../lib/contracts";
import { desktopBridge } from "../lib/bridge";
import { selectedProjectLocation } from "../lib/project-location";
import { BrandMark } from "./BrandMark";

interface WelcomeScreenProps {
  onEnter: (project: ProjectSnapshot) => void;
}

const templates = [
  { id: "generic-article", label: "Research article", detail: "A simple starting point for any journal" },
  { id: "acm-acmart", label: "ACM manuscript", detail: "ACM conference review format" },
  { id: "ieee-ieeetran", label: "IEEE paper", detail: "IEEE two-column conference format" },
] as const satisfies ReadonlyArray<{ id: TemplateId; label: string; detail: string }>;

export function WelcomeScreen({ onEnter }: WelcomeScreenProps) {
  const [creating, setCreating] = useState(false);
  const [templateId, setTemplateId] = useState<TemplateId>("generic-article");
  const [title, setTitle] = useState("Untitled research paper");
  const [author, setAuthor] = useState("");
  const [folderName, setFolderName] = useState("untitled-paper");
  const [busy, setBusy] = useState<"create" | "open" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleCreate = async () => {
    setBusy("create");
    setError(null);
    try {
      const parentDirectory = await desktopBridge.pickCreateParentDirectory();
      if (parentDirectory === null) return;
      onEnter(await desktopBridge.createProject({
        parentDirectory, folderName: folderName.trim(), title: title.trim(),
        authors: [author.trim()], templateId, engine: "pdflatex",
      }));
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
      if (projectPath !== null) {
        const { rootPath, mainFile } = selectedProjectLocation(projectPath);
        onEnter(await desktopBridge.openProject(rootPath, mainFile));
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The paper could not be opened.");
    } finally {
      setBusy(null);
    }
  };

  return (
    <main className="welcome" aria-labelledby="welcome-title">
      <header className="welcome__header"><BrandMark /><span>Your writing, at home.</span></header>
      <section className="welcome__start">
        <div className="welcome__start-inner">
          {creating ? (
            <form onSubmit={(event) => { event.preventDefault(); void handleCreate(); }}>
              <button className="welcome__back" type="button" disabled={busy !== null} onClick={() => { setCreating(false); setError(null); }}>
                <ArrowLeft size={16} aria-hidden="true" /> Back
              </button>
              <h1 id="welcome-title">Create a paper</h1>
              <p className="welcome__intro">Choose a starting point. You can make it your own as you write.</p>
              <fieldset className="template-picker" disabled={busy !== null}>
                <legend>Template</legend>
                {templates.map((template) => (
                  <label className="template-option" key={template.id}>
                    <input type="radio" name="template" value={template.id} checked={templateId === template.id} onChange={() => setTemplateId(template.id)} />
                    <span className="template-option__icon" aria-hidden="true"><FileText size={18} /></span>
                    <span className="template-option__copy"><strong>{template.label}</strong><span>{template.detail}</span></span>
                    <span className="template-option__check" aria-hidden="true"><Check size={14} /></span>
                  </label>
                ))}
              </fieldset>
              <div className="project-fields">
                <label><span>Paper title</span><input value={title} onChange={(event) => setTitle(event.target.value)} required disabled={busy !== null} /></label>
                <label><span>Lead author</span><input value={author} onChange={(event) => setAuthor(event.target.value)} placeholder="Your name" autoComplete="name" required disabled={busy !== null} /></label>
                <label><span>Folder name</span><input value={folderName} onChange={(event) => setFolderName(event.target.value)} required pattern="[^/\\]+" disabled={busy !== null} /></label>
              </div>
              <p className="welcome__folder-hint">Next, choose where to save this folder on your computer.</p>
              <button className="primary-button primary-button--large" type="submit" disabled={busy !== null || !title.trim() || !author.trim() || !folderName.trim()}>
                {busy === "create" ? "Creating paper…" : "Choose location and create"}<ArrowRight size={17} aria-hidden="true" />
              </button>
            </form>
          ) : (
            <>
              <h1 id="welcome-title">A place for your next paper.</h1>
              <p className="welcome__intro">Open a paper you’re working on, or begin with a fresh page.</p>
              <div className="welcome__paths">
                <button className="welcome-path" type="button" disabled={busy !== null} onClick={() => void handleOpen()}>
                  <FolderOpen size={24} aria-hidden="true" />
                  <span><strong>{busy === "open" ? "Opening paper…" : "Open a paper"}</strong><small>Choose your main .tex file</small></span>
                  <ArrowRight size={18} aria-hidden="true" />
                </button>
                <button className="welcome-path" type="button" disabled={busy !== null} onClick={() => { setCreating(true); setError(null); }}>
                  <FilePlus2 size={24} aria-hidden="true" />
                  <span><strong>Create a paper</strong><small>Start with an article or conference template</small></span>
                  <ArrowRight size={18} aria-hidden="true" />
                </button>
              </div>
            </>
          )}
          {error === null ? null : <p className="inline-error" role="alert">{error}</p>}
          <p className="welcome__privacy">Your files stay on your computer. No account required.</p>
        </div>
      </section>
      <footer className="welcome__footer">Setwright · A local-first editor for LaTeX papers</footer>
    </main>
  );
}
