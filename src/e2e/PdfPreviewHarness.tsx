import { useEffect, useState } from "react";
import { PreviewPane } from "../components/PreviewPane";
import { desktopBridge } from "../lib/bridge";
import type { WebviewRuntimeInfo } from "../lib/contracts";
import { pdfEngine } from "../lib/pdf-engine";
import { createPdfProbeFixture, PDF_PROBE_FIXTURE_SHA256 } from "../lib/pdf-probe-fixture";
import sampleProjectDataUrl from "../../test/fixtures/pdf/sample-project.pdf?inline";
import representativeManifest from "../../test/fixtures/pdf/sample-project.manifest.json";

interface RepresentativeFixture {
  bytes: Uint8Array;
  sha256: string;
}

let representativeFixturePromise: Promise<RepresentativeFixture> | null = null;

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new Uint8Array(bytes).buffer);
  return Array.from(new Uint8Array(digest), (value) => value.toString(16).padStart(2, "0")).join("");
}

function loadRepresentativeFixture(): Promise<RepresentativeFixture> {
  representativeFixturePromise ??= Promise.resolve().then(async () => {
    const marker = ";base64,";
    const markerIndex = sampleProjectDataUrl.indexOf(marker);
    if (!sampleProjectDataUrl.startsWith("data:application/pdf") || markerIndex < 0) {
      throw new Error("The representative fixture was not embedded as a PDF data URL.");
    }
    const binary = window.atob(sampleProjectDataUrl.slice(markerIndex + marker.length));
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    if (bytes.byteLength !== representativeManifest.byteLength) {
      throw new Error("The representative fixture byte length does not match its manifest.");
    }
    const sha256 = await sha256Hex(bytes);
    if (sha256 !== representativeManifest.sha256) {
      throw new Error("The representative fixture SHA-256 does not match its manifest.");
    }
    return { bytes, sha256 };
  });
  return representativeFixturePromise;
}

function waitForReplacementBoundary(): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, 20));
}

export function PdfPreviewHarness() {
  const [pdfBytes, setPdfBytes] = useState<Uint8Array>(() => createPdfProbeFixture());
  const [fixture, setFixture] = useState("probe");
  const [mounted, setMounted] = useState(true);
  const [busy, setBusy] = useState(false);
  const [harnessError, setHarnessError] = useState<string | null>(null);
  const [runtime, setRuntime] = useState<WebviewRuntimeInfo | null>(null);
  const [representativeSha256, setRepresentativeSha256] = useState("unverified");

  useEffect(() => {
    let current = true;
    void desktopBridge.getWebviewRuntimeInfo().then((value) => {
      if (current) setRuntime(value);
    }).catch(() => {
      if (current) {
        setRuntime({ engine: "unknown", detectedVersion: null, minimumVersion: null, comparison: "not-comparable" });
      }
    });
    return () => {
      current = false;
    };
  }, []);

  const showProbe = () => {
    setFixture("probe");
    setHarnessError(null);
    setPdfBytes(createPdfProbeFixture());
  };

  const showRepresentative = async () => {
    setBusy(true);
    setHarnessError(null);
    try {
      const fixtureData = await loadRepresentativeFixture();
      setRepresentativeSha256(fixtureData.sha256);
      setFixture("representative");
      setPdfBytes(fixtureData.bytes.slice());
    } catch (cause: unknown) {
      setHarnessError(cause instanceof Error ? cause.message : "Representative fixture failed to load.");
    } finally {
      setBusy(false);
    }
  };

  const replaceRapidly = async () => {
    setBusy(true);
    setHarnessError(null);
    try {
      const representative = await loadRepresentativeFixture();
      setRepresentativeSha256(representative.sha256);
      setFixture("rapid-probe");
      setPdfBytes(createPdfProbeFixture());
      await waitForReplacementBoundary();
      setFixture("rapid-representative");
      setPdfBytes(representative.bytes.slice());
    } catch (cause: unknown) {
      setHarnessError(cause instanceof Error ? cause.message : "Rapid replacement failed.");
    } finally {
      setBusy(false);
    }
  };

  const showCorrupt = () => {
    setFixture("corrupt");
    setHarnessError(null);
    setPdfBytes(new TextEncoder().encode("%PDF-1.7\ncorrupt fixture\n%%EOF\n"));
  };

  return (
    <main
      data-testid="pdf-preview-e2e-harness"
      style={{ display: "grid", gridTemplateRows: "auto minmax(0, 1fr)", width: "100vw", height: "100vh" }}
    >
      <header style={{ display: "flex", alignItems: "center", flexWrap: "wrap", gap: 8, padding: 10, borderBottom: "1px solid var(--border)", background: "var(--surface)", overflow: "hidden" }}>
        <button type="button" onClick={showProbe}>Probe fixture</button>
        <button type="button" disabled={busy} onClick={() => void showRepresentative()}>Representative fixture</button>
        <button type="button" disabled={busy} onClick={() => void replaceRapidly()}>Rapid replacement</button>
        <button type="button" onClick={showCorrupt}>Corrupt input</button>
        <button type="button" onClick={() => setMounted((value) => !value)}>{mounted ? "Unmount preview" : "Restore preview"}</button>
        <output data-testid="pdf-e2e-fixture">{fixture}</output>
        <output data-testid="pdf-e2e-pdfjs-version" data-value={pdfEngine.pdfJsVersion} hidden />
        <output data-testid="pdf-e2e-probe-sha256" data-value={PDF_PROBE_FIXTURE_SHA256} hidden />
        <output data-testid="pdf-e2e-representative-sha256" data-value={representativeSha256} hidden />
        <output data-testid="pdf-e2e-runtime" data-value={runtime === null ? "detecting" : JSON.stringify(runtime)} hidden />
        {harnessError === null ? null : <output role="alert">{harnessError}</output>}
      </header>
      <div style={{ minWidth: 0, minHeight: 0 }}>
        {mounted ? (
          <PreviewPane pdfBytes={pdfBytes} compileStatus="success" />
        ) : (
          <p data-testid="pdf-e2e-unmounted">PDF preview unmounted</p>
        )}
      </div>
    </main>
  );
}
