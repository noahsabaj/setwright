import { useState } from "react";
import { AlertTriangle, CheckCircle2, FileCode2 } from "lucide-react";
import type { ProjectFile, ProjectSnapshot, Utf8ConversionPreview } from "../lib/contracts";
import { desktopBridge } from "../lib/bridge";

interface EncodingConversionPanelProps {
  project: ProjectSnapshot;
  file: ProjectFile;
  onConverted: (project: ProjectSnapshot) => void;
}

export function EncodingConversionPanel({ project, file, onConverted }: EncodingConversionPanelProps) {
  const [preview, setPreview] = useState<Utf8ConversionPreview | null>(null);
  const [reviewedText, setReviewedText] = useState("");
  const [approved, setApproved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const prepare = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await desktopBridge.prepareUtf8Conversion(project.sessionId, file.id);
      setPreview(next);
      setReviewedText(next.reviewedText);
      setApproved(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The conversion preview could not be prepared.");
    } finally {
      setBusy(false);
    }
  };

  const convert = async () => {
    if (preview === null || !approved) return;
    setBusy(true);
    setError(null);
    try {
      await desktopBridge.convertFileToUtf8(
        project.sessionId,
        project.revision,
        file.id,
        reviewedText,
        preview.originalSha256,
      );
      onConverted(await desktopBridge.readProject(project.sessionId));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "The reviewed conversion was not applied.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="encoding-review" aria-labelledby="encoding-review-title">
      <div className="encoding-review__intro">
        <FileCode2 size={24} aria-hidden="true" />
        <div>
          <h2 id="encoding-review-title">Review encoding conversion</h2>
          <p><strong>{file.relativePath}</strong> is preserved as {file.byteLength.toLocaleString()} original bytes. Setwright will not rewrite it until you explicitly approve UTF-8 text.</p>
        </div>
      </div>

      {preview === null ? (
        <button className="primary-button" type="button" disabled={busy} onClick={() => void prepare()}>
          {busy ? "Preparing preview…" : "Create a lossy UTF-8 preview"}
        </button>
      ) : (
        <>
          <div className="encoding-review__facts" role="status">
            <span>Original SHA-256 <code>{preview.originalSha256.slice(0, 16)}…</code></span>
            <span>{preview.replacementCharacterCount} undecodable byte sequence(s) shown as <code>�</code></span>
          </div>
          {preview.replacementCharacterCount > 0 ? (
            <p className="encoding-review__warning"><AlertTriangle size={16} aria-hidden="true" /> Every replacement character requires review; edit the text below before approving.</p>
          ) : null}
          <label className="encoding-review__editor">
            <span>Proposed canonical UTF-8 source</span>
            <textarea value={reviewedText} spellCheck={false} onChange={(event) => { setReviewedText(event.target.value); setApproved(false); }} />
          </label>
          <label className="encoding-review__approval">
            <input type="checkbox" checked={approved} onChange={(event) => setApproved(event.target.checked)} />
            <span>I reviewed the proposed text and understand that saving will replace the original encoded bytes.</span>
          </label>
          <button className="primary-button" type="button" disabled={busy || !approved} onClick={() => void convert()}>
            <CheckCircle2 size={16} aria-hidden="true" />
            {busy ? "Applying reviewed conversion…" : "Use this UTF-8 text"}
          </button>
        </>
      )}
      {error === null ? null : <p className="inline-error" role="alert">{error}</p>}
    </section>
  );
}
