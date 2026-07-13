import { lazy, Suspense, useEffect, useState } from "react";
import type { ProjectSnapshot } from "./lib/contracts";
import { WelcomeScreen } from "./components/WelcomeScreen";
import { desktopBridge } from "./lib/bridge";

const WorkspaceShell = lazy(async () => {
  const module = await import("./components/WorkspaceShell");
  return { default: module.WorkspaceShell };
});

export function App() {
  const [project, setProject] = useState<ProjectSnapshot | null>(null);
  const [bootstrapSession] = useState(() => new URLSearchParams(window.location.search).get("session"));
  const [bootstrapPending, setBootstrapPending] = useState(bootstrapSession !== null);
  const [bootstrapError, setBootstrapError] = useState<string | null>(null);

  useEffect(() => {
    const sessionId = bootstrapSession;
    if (sessionId === null) return;
    void desktopBridge.readProject(sessionId).then(setProject).catch((cause: unknown) => {
      setBootstrapError(cause instanceof Error ? cause.message : "The project window could not be restored.");
    }).finally(() => setBootstrapPending(false));
  }, [bootstrapSession]);

  if (bootstrapPending) {
    return <main className="app-loading" aria-live="polite">Opening the project in its isolated window…</main>;
  }
  if (bootstrapError !== null) {
    return <main className="app-loading" role="alert">{bootstrapError}</main>;
  }

  return project === null ? <WelcomeScreen onEnter={setProject} /> : (
    <Suspense fallback={<main className="app-loading" aria-live="polite">Preparing your workspace…</main>}>
      <WorkspaceShell project={project} onProjectChange={setProject} />
    </Suspense>
  );
}
