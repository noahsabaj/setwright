import type {
  ArxivPreflightSummary,
  CompileTicket,
  CreateProjectRequest,
  EditResult,
  ExportResult,
  HistoryEntry,
  ProjectSessionId,
  ProjectSnapshot,
  PdfArtifact,
  Revision,
  RuntimeReadiness,
  SaveResult,
  SourceEdit,
  Utf8ConversionPreview,
} from "./contracts";
import { commands } from "./bindings";
import type { LocalCitationSearch, MetadataLookupRequest, MetadataLookupResponse, OpenedProjectWindow } from "./bindings";
import { cloneDemoProject, demoProject } from "./mock-project";
import { applyValidatedSourceEdits } from "./source-edits";

export interface SetwrightBridge {
  readonly runtime: "tauri" | "browserDemo";
  pickCreateParentDirectory(): Promise<string | null>;
  pickProjectPath(): Promise<string | null>;
  pickExportDirectory(): Promise<string | null>;
  createProject(request: CreateProjectRequest): Promise<ProjectSnapshot>;
  openProject(rootPath: string, mainFile?: string): Promise<ProjectSnapshot>;
  openProjectWindow(rootPath: string, mainFile?: string): Promise<OpenedProjectWindow>;
  closeProject(sessionId: ProjectSessionId): Promise<void>;
  readProject(sessionId: ProjectSessionId): Promise<ProjectSnapshot>;
  applySourceEdits(
    sessionId: ProjectSessionId,
    baseRevision: Revision,
    edits: SourceEdit[],
  ): Promise<EditResult>;
  prepareUtf8Conversion(sessionId: ProjectSessionId, fileId: string): Promise<Utf8ConversionPreview>;
  convertFileToUtf8(
    sessionId: ProjectSessionId,
    baseRevision: Revision,
    fileId: string,
    reviewedText: string,
    expectedOriginalSha256: string,
  ): Promise<EditResult>;
  searchLocalCitations(sessionId: ProjectSessionId, fileId: string, query: string): Promise<LocalCitationSearch>;
  lookupCitationMetadata(sessionId: ProjectSessionId, request: MetadataLookupRequest): Promise<MetadataLookupResponse>;
  saveProject(sessionId: ProjectSessionId, expectedRevision: Revision): Promise<SaveResult>;
  getRuntimeReadiness(): Promise<RuntimeReadiness>;
  startCompile(
    sessionId: ProjectSessionId,
    revision: Revision,
    engine: "pdflatex" | "xelatex",
  ): Promise<CompileTicket>;
  cancelCompile(sessionId: ProjectSessionId, jobId: string): Promise<void>;
  readCompilePdf(sessionId: ProjectSessionId): Promise<PdfArtifact>;
  listHistory(sessionId: ProjectSessionId): Promise<HistoryEntry[]>;
  createSnapshot(sessionId: ProjectSessionId, name?: string): Promise<HistoryEntry>;
  restoreSnapshot(sessionId: ProjectSessionId, snapshotId: string): Promise<ProjectSnapshot>;
  runArxivPreflight(
    sessionId: ProjectSessionId,
    revision: Revision,
  ): Promise<ArxivPreflightSummary>;
  exportArxiv(
    sessionId: ProjectSessionId,
    revision: Revision,
    destination: string,
  ): Promise<ExportResult>;
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function commandErrorMessage(error: unknown): string {
  if (typeof error !== "object" || error === null) return "The desktop command failed.";
  const record = error as Record<string, unknown>;
  if (typeof record.message === "string") return record.message;
  if ("error" in record) return commandErrorMessage(record.error);
  if (record.kind === "externalConflict") {
    return `The file changed outside Setwright${typeof record.path === "string" ? `: ${record.path}` : ""}. Saving is paused until you resolve the conflict.`;
  }
  if (record.kind === "revisionConflict") {
    return "The paper changed before this operation completed. Reload the latest revision and try again.";
  }
  if (record.kind === "hashMismatch") {
    return "The selected source no longer matches the displayed text, so Setwright did not apply the edit.";
  }
  if (record.kind === "invalidUtf8") {
    return "This file is not UTF-8. Review a conversion diff in Source mode before editing it visually.";
  }
  if (record.kind === "pathOutsideRoot" || record.kind === "capabilityDenied") {
    return "Setwright blocked an operation outside this window's project boundary.";
  }
  if (record.kind === "sessionClosed") return "This project session is closed.";
  if (record.kind === "fileNotFound") return "A required project file is missing.";
  if (record.kind === "responseTooLarge") return "The metadata service returned more data than Setwright allows.";
  if (record.kind === "network") return "The citation metadata request could not reach its allowlisted provider.";
  return typeof record.kind === "string" ? record.kind : "The desktop command failed.";
}

function unwrapDesktopResult<T, E>(result: { status: "ok"; data: T } | { status: "error"; error: E }): T {
  if (result.status === "ok") return result.data;
  const error = new Error(commandErrorMessage(result.error));
  Object.assign(error, { setwrightError: result.error });
  throw error;
}

let mockProject = cloneDemoProject();

let mockHistory: HistoryEntry[] = [];

export const desktopBridge: SetwrightBridge = {
  runtime: isTauriRuntime() ? "tauri" : "browserDemo",

  async pickCreateParentDirectory() {
    if (!isTauriRuntime()) return "~/Papers";
    const { open } = await import("@tauri-apps/plugin-dialog");
    const result = await open({ directory: true, multiple: false, recursive: false, title: "Choose where to create the paper" });
    return typeof result === "string" ? result : null;
  },

  async pickProjectPath() {
    if (!isTauriRuntime()) return "~/Papers/distribution-shift/main.tex";
    const { open } = await import("@tauri-apps/plugin-dialog");
    const result = await open({
      directory: false,
      multiple: false,
      title: "Choose the project's main LaTeX file",
      filters: [{ name: "LaTeX source", extensions: ["tex"] }],
    });
    return typeof result === "string" ? result : null;
  },

  async pickExportDirectory() {
    if (!isTauriRuntime()) return "~/Desktop";
    const { open } = await import("@tauri-apps/plugin-dialog");
    const result = await open({ directory: true, multiple: false, recursive: false, title: "Choose an export folder" });
    return typeof result === "string" ? result : null;
  },

  async createProject(request) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.createProject(request));
    }
    mockProject = {
      ...cloneDemoProject(),
      sessionId: crypto.randomUUID(),
      title: request.title,
      rootPath: `${request.parentDirectory}/${request.folderName}`,
      settings: {
        ...demoProject.settings,
        projectId: crypto.randomUUID(),
        templateId: request.templateId,
        engine: request.engine,
      },
    };
    return cloneDemoProjectWithState();
  },

  async openProject(rootPath, mainFile) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.openProject(rootPath, mainFile ?? null));
    }
    mockProject = { ...cloneDemoProject(), rootPath };
    return cloneDemoProjectWithState();
  },

  async openProjectWindow(rootPath, mainFile) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.openProjectWindow(rootPath, mainFile ?? null));
    }
    mockProject = { ...cloneDemoProject(), sessionId: crypto.randomUUID(), rootPath };
    return { windowLabel: "browser-demo", sessionId: mockProject.sessionId };
  },

  async closeProject(sessionId) {
    if (isTauriRuntime()) {
      unwrapDesktopResult(await commands.closeProject(sessionId));
    }
  },

  async readProject(sessionId) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.readProject(sessionId));
    }
    return cloneDemoProjectWithState();
  },

  async applySourceEdits(sessionId, baseRevision, edits) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.applySourceEdits(sessionId, baseRevision, edits));
    }
    if (baseRevision !== mockProject.revision) {
      throw new Error(`Revision conflict: expected ${String(mockProject.revision)}, received ${String(baseRevision)}`);
    }
    const nextFiles = structuredClone(mockProject.files);
    for (const file of nextFiles) {
      const fileEdits = edits
        .filter((edit) => edit.fileId === file.id)
        .slice()
        .sort((left, right) => right.startByte - left.startByte);
      if (fileEdits.length === 0 || file.content === null) continue;
      file.content = await applyValidatedSourceEdits(file.content, fileEdits);
      file.dirty = true;
    }
    mockProject = { ...mockProject, revision: mockProject.revision + 1, files: nextFiles };
    return { revision: mockProject.revision, files: structuredClone(nextFiles), diagnostics: [] };
  },

  async prepareUtf8Conversion(sessionId, fileId) {
    if (isTauriRuntime()) return unwrapDesktopResult(await commands.prepareUtf8Conversion(sessionId, fileId));
    throw new Error("The browser preview has no non-UTF-8 source bytes to review.");
  },

  async convertFileToUtf8(sessionId, baseRevision, fileId, reviewedText, expectedOriginalSha256) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.convertFileToUtf8(
        sessionId,
        baseRevision,
        fileId,
        reviewedText,
        expectedOriginalSha256,
      ));
    }
    throw new Error("Encoding conversion is available only for a scoped desktop project.");
  },

  async searchLocalCitations(sessionId, fileId, query) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.searchLocalCitations(sessionId, fileId, query));
    }
    const bibliography = mockProject.files.find((file) => file.id === fileId && file.kind === "bib");
    const source = bibliography?.content ?? "";
    const key = /@[A-Za-z]+\s*\{\s*([^,\s]+)/u.exec(source)?.[1] ?? "";
    const title = /title\s*=\s*\{([^}]*)\}/iu.exec(source)?.[1] ?? null;
    const matches = query.trim() === "" || `${key} ${title ?? ""}`.toLowerCase().includes(query.toLowerCase());
    return {
      fileId,
      sourceHash: "browser-demo-bibliography",
      hasParseErrors: false,
      findings: [],
      results: matches && key !== "" ? [{ key, entryType: "article", title, authors: null, year: "2020", score: 1, span: { startByte: 0, endByte: source.length } }] : [],
    };
  },

  async lookupCitationMetadata(sessionId, request) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.lookupCitationMetadata(sessionId, request));
    }
    throw new Error("Online citation metadata lookup is unavailable in the browser demo.");
  },

  async saveProject(sessionId, expectedRevision) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.saveProject(sessionId, expectedRevision));
    }
    if (expectedRevision !== mockProject.revision) throw new Error("The project changed before it could be saved.");
    mockProject = {
      ...mockProject,
      files: mockProject.files.map((file) => ({ ...file, dirty: false })),
    };
    return { revision: mockProject.revision, savedAt: new Date().toISOString(), fileHashes: {} };
  },

  async getRuntimeReadiness() {
    if (isTauriRuntime()) return commands.getRuntimeReadiness();
    return {
      profileId: mockProject.settings.runtimeId,
      runtimeManifestKeysConfigured: false,
      runtimeInstallAvailable: false,
      sandboxBackend: null,
      sandboxAttested: false,
      reason: "The browser preview has no managed TeX runtime or attested OS sandbox.",
    };
  },

  async startCompile(sessionId, revision, engine) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.startCompile(sessionId, revision, engine));
    }
    throw new Error("Compilation is available only in the desktop app with an attested OS sandbox and a verified managed runtime.");
  },

  async cancelCompile(sessionId, jobId) {
    if (isTauriRuntime()) unwrapDesktopResult(await commands.cancelCompile(sessionId, jobId));
  },

  async readCompilePdf(sessionId) {
    if (isTauriRuntime()) return unwrapDesktopResult(await commands.readCompilePdf(sessionId));
    throw new Error("No compiled PDF artifact exists in the browser preview.");
  },

  async listHistory(sessionId) {
    if (isTauriRuntime()) return unwrapDesktopResult(await commands.listHistory(sessionId));
    return structuredClone(mockHistory);
  },

  async createSnapshot(sessionId, name) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.createSnapshot(sessionId, name ?? null));
    }
    const entry: HistoryEntry = {
      id: crypto.randomUUID(),
      revision: mockProject.revision,
      createdAt: new Date().toISOString(),
      label: name ?? "Named version",
      kind: "named",
      changedFiles: 0,
    };
    mockHistory.unshift(entry);
    return structuredClone(entry);
  },

  async restoreSnapshot(sessionId, snapshotId) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.restoreSnapshot(sessionId, snapshotId));
    }
    mockProject = { ...mockProject, revision: mockProject.revision + 1 };
    return cloneDemoProjectWithState();
  },

  async runArxivPreflight(sessionId, revision) {
    if (isTauriRuntime()) {
      const report = unwrapDesktopResult(await commands.runArxivPreflight(sessionId, revision));
      return {
        schemaVersion: report.schemaVersion,
        sourceHash: report.source.sourceSha256,
        runtimeId: report.runtime.profileId,
        engine: report.runtime.engine,
        findings: report.findings.map((finding) => ({
          id: finding.id,
          severity: finding.severity,
          title: finding.code,
          detail: finding.message,
          ...(finding.path === undefined || finding.path === null ? {} : { file: finding.path }),
        })),
        includedFiles: report.includedFiles.map((file) => file.path),
        excludedFiles: report.excludedFiles.map((file) => file.path),
        cleanBuildPassed: report.cleanBuild.succeeded,
        ...(report.cleanBuild.pdfSha256 === null ? {} : { pdfHash: report.cleanBuild.pdfSha256 }),
        userApproved: report.userApproval.approved,
      };
    }
    return {
      schemaVersion: 1,
      sourceHash: "demo-source-hash",
      runtimeId: mockProject.settings.runtimeId,
      engine: mockProject.settings.engine,
      findings: [
        {
          id: "runtime-demo",
          severity: "info",
          title: "Demo preflight",
          detail: "Connect the managed runtime to perform a clean-room build.",
        },
      ],
      includedFiles: mockProject.files.map((file) => file.relativePath),
      excludedFiles: [],
      cleanBuildPassed: false,
      userApproved: false,
    };
  },

  async exportArxiv(sessionId, revision, destination) {
    if (isTauriRuntime()) {
      return unwrapDesktopResult(await commands.exportArxiv(sessionId, revision, destination));
    }
    throw new Error("arXiv export is unavailable in the browser demo because it cannot run the required clean sandbox build.");
  },
};

function cloneDemoProjectWithState(): ProjectSnapshot {
  return structuredClone(mockProject);
}

export function resetMockBridge(): void {
  mockProject = cloneDemoProject();
  mockHistory = [];
}
