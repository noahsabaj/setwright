import { useId, useState } from "react";
import { BookOpen, Braces, Code2, Footprints, Image, Quote, Sigma, Table2, X } from "lucide-react";
import { Dialog, Modal, ModalOverlay } from "react-aria-components";

export type InsertKind = "insert" | "citation" | "equation" | "figure" | "table";

export interface InsertPayload {
  kind: InsertKind | "theorem" | "definition" | "proof" | "code" | "quote" | "footnote";
  primary: string;
  secondary: string;
  tertiary?: string;
  numbered?: boolean;
}

export interface CitationSearchResult {
  key: string;
  title?: string | undefined;
  authors?: string[] | undefined;
  year?: string | undefined;
}

interface InsertDialogProps {
  kind: InsertKind;
  onClose: () => void;
  onInsert: (payload: InsertPayload) => void;
  onSearchCitations?: ((query: string) => Promise<CitationSearchResult[]>) | undefined;
}

export function InsertDialog({ kind, onClose, onInsert, onSearchCitations }: InsertDialogProps) {
  const titleId = useId();
  const [primary, setPrimary] = useState("");
  const [secondary, setSecondary] = useState("");
  const [tertiary, setTertiary] = useState("");
  const [numbered, setNumbered] = useState(true);
  const [citationQuery, setCitationQuery] = useState("");
  const [citationResults, setCitationResults] = useState<CitationSearchResult[]>([]);
  const [citationSearchState, setCitationSearchState] = useState<"idle" | "searching" | "complete" | "error">("idle");

  const insertSimple = (simpleKind: InsertPayload["kind"], label: string) => {
    onInsert({ kind: simpleKind, primary: label, secondary: "" });
  };

  const submit = () => {
    onInsert({
      kind,
      primary,
      secondary,
      ...(tertiary === "" ? {} : { tertiary }),
      ...(kind === "equation" ? { numbered } : {}),
    });
  };

  const searchCitations = async () => {
    if (onSearchCitations === undefined || citationQuery.trim() === "") return;
    setCitationSearchState("searching");
    try {
      setCitationResults(await onSearchCitations(citationQuery.trim()));
      setCitationSearchState("complete");
    } catch {
      setCitationSearchState("error");
    }
  };

  return (
    <ModalOverlay className="modal-layer" isOpen isDismissable onOpenChange={(open) => { if (!open) onClose(); }}>
      <Modal className="insert-dialog">
        <Dialog aria-labelledby={titleId}>
        <header className="insert-dialog__header">
          <div>
            <span className="eyebrow">Insert</span>
            <h2 id={titleId}>
              {kind === "insert" ? "Scientific structure" : kind.charAt(0).toUpperCase() + kind.slice(1)}
            </h2>
          </div>
          <button className="icon-button" type="button" aria-label="Close dialog" onClick={onClose}><X size={18} /></button>
        </header>

        {kind === "insert" ? (
          <div className="insert-grid">
            <button type="button" onClick={() => insertSimple("theorem", "")}><Sigma /><strong>Theorem</strong><span>Numbered statement</span></button>
            <button type="button" onClick={() => insertSimple("definition", "")}><BookOpen /><strong>Definition</strong><span>Numbered definition</span></button>
            <button type="button" onClick={() => insertSimple("proof", "")}><Footprints /><strong>Proof</strong><span>Proof environment</span></button>
            <button type="button" onClick={() => insertSimple("code", "Code listing")}><Code2 /><strong>Code listing</strong><span>Listings-compatible</span></button>
            <button type="button" onClick={() => insertSimple("quote", "Quote")}><Quote /><strong>Block quote</strong><span>Long quotation</span></button>
            <button type="button" disabled title="Visual footnote insertion is not connected yet."><Braces /><strong>Footnote</strong><span>Unavailable</span></button>
          </div>
        ) : null}

        {kind === "citation" ? (
          <div className="dialog-form">
            <label>
              <span>Search references</span>
              <input autoFocus value={citationQuery} onChange={(event) => setCitationQuery(event.target.value)} placeholder="Author, title, DOI, or citation key" />
            </label>
            {onSearchCitations === undefined ? null : (
              <button
                className="quiet-button"
                type="button"
                disabled={citationQuery.trim() === "" || citationSearchState === "searching"}
                onClick={() => { void searchCitations(); }}
              >
                {citationSearchState === "searching" ? "Searching references…" : "Search local bibliography"}
              </button>
            )}
            {citationSearchState === "error" ? <p role="alert">The local bibliography search failed.</p> : null}
            <div className="citation-results" role="listbox" aria-label="Citation results">
              {citationResults.map((reference) => {
                const authorLabel = reference.authors?.join(", ") ?? reference.key;
                const title = reference.title ?? reference.key;
                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={primary === reference.key}
                    key={`metadata-${reference.key}`}
                    onClick={() => { setPrimary(reference.key); setSecondary(authorLabel); }}
                  >
                    <BookOpen size={16} />
                    <span><strong>{title}</strong><small>{reference.key} · {authorLabel}</small></span>
                    {reference.year === undefined ? null : <time>{reference.year}</time>}
                  </button>
                );
              })}
            </div>
            {citationSearchState === "complete" && citationResults.length === 0 ? <p className="dialog-hint">No matching entries were found in the project bibliography.</p> : null}
            {onSearchCitations === undefined ? <p className="dialog-hint">Add a local <code>.bib</code> file to enable citation search.</p> : null}
            <p className="dialog-hint">Local search works offline. Crossref/arXiv lookup is not exposed by this interface yet.</p>
          </div>
        ) : null}

        {kind === "equation" ? (
          <div className="dialog-form">
            <label><span>LaTeX math</span><textarea autoFocus value={primary} onChange={(event) => setPrimary(event.target.value)} placeholder="p(y \\mid x) = \\sum_z p(z \\mid x)" spellCheck={false} /></label>
            <div className="equation-preview" aria-label="Equation source"><Sigma size={18} /> {primary === "" ? "Enter an equation above" : primary}</div>
            <div className="form-row">
              <label><span>Label</span><input value={secondary} onChange={(event) => setSecondary(event.target.value)} placeholder="eq:objective" /></label>
              <label className="checkbox-field"><input type="checkbox" checked={numbered} onChange={(event) => setNumbered(event.target.checked)} /><span>Number equation</span></label>
            </div>
          </div>
        ) : null}

        {kind === "figure" ? (
          <div className="dialog-form">
            <div className="asset-drop"><Image size={24} /><strong>Project image path</strong><span>Reference an existing PDF, PNG, JPG, or EPS asset</span></div>
            <label><span>Project path</span><input value={primary} onChange={(event) => setPrimary(event.target.value)} placeholder="figures/overview.pdf" /></label>
            <label><span>Caption</span><textarea value={secondary} onChange={(event) => setSecondary(event.target.value)} placeholder="Describe what the figure shows" /></label>
            <label><span>Label</span><input value={tertiary} onChange={(event) => setTertiary(event.target.value)} placeholder="fig:overview" /></label>
          </div>
        ) : null}

        {kind === "table" ? (
          <div className="dialog-form">
            <div className="table-setup"><Table2 size={22} /><span><strong>Rectangular booktabs table</strong><small>Paste tab-separated spreadsheet cells below.</small></span></div>
            <label><span>Cells</span><textarea autoFocus value={primary} onChange={(event) => setPrimary(event.target.value)} placeholder={"Model\tIn-domain\tShifted\nBaseline\t76.4\t58.9\nOurs\t82.1\t71.4"} /></label>
            <label><span>Caption</span><input value={secondary} onChange={(event) => setSecondary(event.target.value)} placeholder="Accuracy under distribution shift." /></label>
            <label><span>Label</span><input value={tertiary} onChange={(event) => setTertiary(event.target.value)} placeholder="tab:results" /></label>
          </div>
        ) : null}

        {kind === "insert" ? null : (
          <footer className="insert-dialog__footer">
            <button className="quiet-button" type="button" onClick={onClose}>Cancel</button>
            <button className="primary-button" type="button" onClick={submit} disabled={primary.trim() === ""}>Insert {kind}</button>
          </footer>
        )}
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}
