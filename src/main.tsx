import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles/global.css";

const root = document.getElementById("root");

if (!root) {
  throw new Error("Setwright root element was not found");
}

async function bootstrap(): Promise<void> {
  if (import.meta.env.MODE === "pdf-e2e") {
    await import("@wdio/tauri-plugin");
    const { PdfPreviewHarness } = await import("./e2e/PdfPreviewHarness");
    createRoot(root as HTMLElement).render(
      <StrictMode>
        <PdfPreviewHarness />
      </StrictMode>,
    );
    return;
  }

  const { App } = await import("./App");
  createRoot(root as HTMLElement).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void bootstrap();
