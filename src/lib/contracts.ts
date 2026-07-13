import type {
  AppError as GeneratedAppError,
  CompileTicket as GeneratedCompileTicket,
  CreateProjectRequest as GeneratedCreateProjectRequest,
  Diagnostic as GeneratedDiagnostic,
  ExportResult as GeneratedExportResult,
  FileId as GeneratedFileId,
  LatexEngine,
  PaperSettingsV1 as GeneratedPaperSettingsV1,
  ProjectSessionId as GeneratedProjectSessionId,
  Revision as GeneratedRevision,
  RuntimeReadiness as GeneratedRuntimeReadiness,
  SourceEdit as GeneratedSourceEdit,
  SourceSpan as GeneratedSourceSpan,
  TemplateId as GeneratedTemplateId,
  UiCompatibilityFinding,
  UiEditResult,
  UiHistoryEntry,
  UiProjectFile,
  UiProjectSnapshot,
  UiPdfArtifact as GeneratedUiPdfArtifact,
  UiSaveResult,
  Utf8ConversionPreview as GeneratedUtf8ConversionPreview,
} from "./bindings";

// Canonical command and disk-contract types are generated from Rust. The
// aliases below preserve readable UI names without creating a second contract.
export type ProjectSessionId = GeneratedProjectSessionId;
export type FileId = GeneratedFileId;
export type Revision = GeneratedRevision;
export type CompilerEngine = LatexEngine;
export type TemplateId = GeneratedTemplateId;
export type SourceSpan = GeneratedSourceSpan;
export type SourceEdit = GeneratedSourceEdit;
export type ProjectFile = UiProjectFile;
export type CompatibilityFinding = UiCompatibilityFinding;
export type ProjectSnapshot = UiProjectSnapshot;
export type CreateProjectRequest = GeneratedCreateProjectRequest;
export type PaperSettingsV1 = GeneratedPaperSettingsV1;
export type Diagnostic = GeneratedDiagnostic;
export type AppError = GeneratedAppError;
export type EditResult = UiEditResult;
export type SaveResult = UiSaveResult;
export type CompileTicket = GeneratedCompileTicket;
export type HistoryEntry = UiHistoryEntry;
export type ExportResult = GeneratedExportResult;
export type RuntimeReadiness = GeneratedRuntimeReadiness;
export type PdfArtifact = GeneratedUiPdfArtifact;
export type Utf8ConversionPreview = GeneratedUtf8ConversionPreview;

export type SaveState = "saved" | "saving" | "dirty" | "conflict";
export type WorkspaceMode = "write" | "source" | "preview" | "split";
export type ThemePreference = "light" | "dark" | "contrast";

// Visual intent is kept separate from transport until the projection adapter
// resolves it into canonical, expected-hash SourceEdits.
export type DocumentOp =
  | {
      kind: "replaceText";
      span: SourceSpan;
      text: string;
      expectedSliceHash: string;
    }
  | {
      kind: "setMark";
      span: SourceSpan;
      mark: "bold" | "italic" | "underline" | "code";
      enabled: boolean;
    }
  | {
      kind: "insertNode";
      fileId: FileId;
      atByte: number;
      nodeType: string;
      attributes: Record<string, string | number | boolean>;
    }
  | { kind: "setAttributes"; span: SourceSpan; attributes: Record<string, string | number | boolean> }
  | { kind: "moveNode"; span: SourceSpan; targetByte: number }
  | { kind: "deleteNode"; span: SourceSpan };

export interface ReviewerIdentity {
  id: string;
  displayName: string;
  color: string;
}

export interface SourceAnchor {
  span: SourceSpan;
  beforeContext: string;
  expectedSource: string;
  afterContext: string;
}

export interface ReviewReply {
  id: string;
  author: ReviewerIdentity;
  body: string;
  createdAt: string;
}

export interface CommentThread {
  id: string;
  anchor: SourceAnchor;
  author: ReviewerIdentity;
  body: string;
  createdAt: string;
  status: "open" | "resolved";
  replies: ReviewReply[];
}

export interface Suggestion {
  id: string;
  anchor: SourceAnchor;
  author: ReviewerIdentity;
  replacement: string;
  expectedSource: string;
  status: "open" | "accepted" | "rejected" | "conflict";
  createdAt: string;
}

export interface ReviewBundleDraft {
  schemaVersion: number;
  projectId: string;
  baseFileHashes: Record<string, string>;
  reviewer: ReviewerIdentity;
  comments: CommentThread[];
  suggestions: Suggestion[];
  createdAt: string;
}

export interface RuntimeManifestSummary {
  schemaVersion: number;
  profileId: string;
  texLiveSnapshot: string;
  platform: string;
  architecture: string;
  engines: CompilerEngine[];
  archiveChecksum: string;
  signature: string;
  sbomPath: string;
  licenseInventoryPath: string;
}

export type ProjectEvent =
  | { kind: "revisionChanged"; sessionId: ProjectSessionId; revision: Revision }
  | { kind: "saveStateChanged"; sessionId: ProjectSessionId; state: SaveState }
  | { kind: "externalChange"; sessionId: ProjectSessionId; fileId: FileId; conflict: boolean }
  | { kind: "compatibilityChanged"; sessionId: ProjectSessionId; findings: CompatibilityFinding[] };

export type CompileEvent =
  | { kind: "queued"; jobId: string; revision: Revision }
  | { kind: "started"; jobId: string; revision: Revision }
  | { kind: "progress"; jobId: string; pass: number; message: string }
  | { kind: "succeeded"; jobId: string; revision: Revision; pdfHash: string; pageCount: number }
  | { kind: "failed"; jobId: string; revision: Revision; diagnostics: Diagnostic[] }
  | { kind: "cancelled"; jobId: string };

export interface PreflightFinding {
  id: string;
  severity: "info" | "warning" | "blocker";
  title: string;
  detail: string;
  file?: string;
}

/** Compact presentation model derived from the canonical V1 report. */
export interface ArxivPreflightSummary {
  schemaVersion: number;
  sourceHash: string;
  runtimeId: string;
  engine: CompilerEngine;
  findings: PreflightFinding[];
  includedFiles: string[];
  excludedFiles: string[];
  cleanBuildPassed: boolean;
  pdfHash?: string;
  userApproved: boolean;
}
