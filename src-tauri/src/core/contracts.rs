use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            specta::Type,
            Debug,
            Clone,
            Copy,
            Serialize,
            Deserialize,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }
    };
}

uuid_id!(ProjectSessionId);
uuid_id!(ProjectId);
uuid_id!(FileId);
uuid_id!(CompileJobId);
uuid_id!(SnapshotId);
uuid_id!(CommentThreadId);
uuid_id!(CommentId);
uuid_id!(SuggestionId);

#[derive(
    specta::Type,
    Debug,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
#[serde(transparent)]
pub struct Revision(pub u64);

impl Revision {
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceSpan {
    pub file_id: FileId,
    pub start_byte: usize,
    pub end_byte: usize,
}

impl SourceSpan {
    #[must_use]
    pub const fn new(file_id: FileId, start_byte: usize, end_byte: usize) -> Self {
        Self {
            file_id,
            start_byte,
            end_byte,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.end_byte.saturating_sub(self.start_byte)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start_byte == self.end_byte
    }
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceEdit {
    pub file_id: FileId,
    pub start_byte: usize,
    pub end_byte: usize,
    pub replacement: String,
    pub expected_slice_hash: String,
}

impl SourceEdit {
    #[must_use]
    pub fn span(&self) -> SourceSpan {
        SourceSpan::new(self.file_id, self.start_byte, self.end_byte)
    }
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TextMark {
    Bold,
    Italic,
    Monospace,
    Underline,
    Strike,
    Superscript,
    Subscript,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VisualNodeKind {
    Paragraph,
    Heading,
    Quote,
    List,
    Footnote,
    Theorem,
    Definition,
    Proof,
    CodeListing,
    Citation,
    CrossReference,
    InlineEquation,
    DisplayEquation,
    Figure,
    Table,
    RawInline,
    RawBlock,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentNode {
    pub node_kind: VisualNodeKind,
    /// Canonical LaTeX for a newly inserted node. Existing source is never
    /// regenerated from this representation.
    pub latex: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// Semantic intent sent by the visual editor. The command adapter resolves an
/// operation into one or more `SourceEdit`s and then submits those edits to the
/// revision-checked canonical source engine.
#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DocumentOp {
    InsertText {
        file_id: FileId,
        at_byte: usize,
        text: String,
    },
    ReplaceText {
        span: SourceSpan,
        replacement: String,
        expected_slice_hash: String,
    },
    Delete {
        span: SourceSpan,
        expected_slice_hash: String,
    },
    SetMark {
        span: SourceSpan,
        mark: TextMark,
        enabled: bool,
    },
    InsertNode {
        file_id: FileId,
        at_byte: usize,
        node: DocumentNode,
    },
    SetAttribute {
        span: SourceSpan,
        name: String,
        value: Option<String>,
    },
    Move {
        span: SourceSpan,
        destination_byte: usize,
        expected_slice_hash: String,
    },
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectFile {
    pub file_id: FileId,
    pub relative_path: String,
    pub revision: Revision,
    pub dirty: bool,
    pub byte_len: usize,
    pub content_hash: String,
    pub source_only: bool,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProjectEvent {
    Opened {
        session_id: ProjectSessionId,
        root: String,
        main_file_id: FileId,
        revision: Revision,
    },
    RevisionChanged {
        revision: Revision,
        files: Vec<FileId>,
    },
    Saved {
        revision: Revision,
        files: Vec<FileId>,
    },
    ExternalReloaded {
        file_id: FileId,
        revision: Revision,
    },
    ExternalConflict {
        file_id: FileId,
        relative_path: String,
    },
    Closed,
}

#[derive(
    specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
    Blocker,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticCategory {
    Syntax,
    MissingDependency,
    UnsafePath,
    UnsupportedFeature,
    Compile,
    Bibliography,
    ArxivPolicy,
    Internal,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub category: DiagnosticCategory,
    pub title: String,
    pub message: String,
    pub span: Option<SourceSpan>,
    pub source_line: Option<u32>,
    #[serde(default)]
    pub actions: Vec<DiagnosticAction>,
    pub technical_detail: Option<String>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticAction {
    pub action_id: String,
    pub label: String,
    pub safe: bool,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CompileEvent {
    Queued {
        job_id: CompileJobId,
        revision: Revision,
    },
    Started {
        job_id: CompileJobId,
        revision: Revision,
    },
    Log {
        job_id: CompileJobId,
        stream: LogStream,
        text: String,
        truncated: bool,
    },
    Diagnostic {
        job_id: CompileJobId,
        diagnostic: Diagnostic,
    },
    Finished {
        job_id: CompileJobId,
        revision: Revision,
        success: bool,
        pdf_hash: Option<String>,
        stale: bool,
    },
    Cancelled {
        job_id: CompileJobId,
    },
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LatexEngine {
    #[serde(rename = "pdflatex")]
    PdfLatex,
    #[serde(rename = "xelatex")]
    XeLatex,
}

impl LatexEngine {
    #[must_use]
    pub const fn executable(self) -> &'static str {
        match self {
            Self::PdfLatex => "pdflatex",
            Self::XeLatex => "xelatex",
        }
    }
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TemplateId {
    #[serde(rename = "generic-article")]
    GenericArticle,
    #[serde(rename = "acm-acmart")]
    AcmAcMart,
    #[serde(rename = "ieee-ieeetran")]
    IeeeTran,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaperSettingsV1 {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub main_file: String,
    pub template_id: TemplateId,
    pub runtime_id: String,
    pub engine: LatexEngine,
}

impl PaperSettingsV1 {
    pub const SCHEMA_VERSION: u32 = 1;

    #[must_use]
    pub fn new(
        main_file: impl Into<String>,
        template_id: TemplateId,
        runtime_id: impl Into<String>,
        engine: LatexEngine,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            project_id: ProjectId::new(),
            main_file: main_file.into(),
            template_id,
            runtime_id: runtime_id.into(),
            engine,
        }
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        let main = Path::new(&self.main_file);
        self.schema_version == Self::SCHEMA_VERSION
            && (5..=1024).contains(&self.main_file.len())
            && self.main_file.ends_with(".tex")
            && !self.main_file.contains(['\\', '\0'])
            && !main.is_absolute()
            && main
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            && valid_profile_id(&self.runtime_id)
    }
}

fn valid_profile_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

// The following V1 structs intentionally mirror the normative files in
// `schemas/` exactly. Do not add IPC-only convenience fields to these types.

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewIdentity {
    pub id: Uuid,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewBaseFile {
    pub file_id: String,
    pub path: String,
    pub sha256: String,
    pub byte_length: usize,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewAnchor {
    pub file_id: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub base_file_hash: String,
    pub expected_source: String,
    pub prefix_context: String,
    pub suffix_context: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewMessage {
    pub id: Uuid,
    pub author: ReviewIdentity,
    pub created_at: DateTime<Utc>,
    pub body: String,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReviewThreadStatus {
    Open,
    Resolved,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewThread {
    pub id: Uuid,
    pub anchor: ReviewAnchor,
    pub status: ReviewThreadStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
    pub messages: Vec<ReviewMessage>,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionStatus {
    Pending,
    Accepted,
    Rejected,
    Conflict,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewSuggestion {
    pub id: Uuid,
    pub order: u32,
    pub author: ReviewIdentity,
    pub created_at: DateTime<Utc>,
    pub anchor: ReviewAnchor,
    pub replacement: String,
    pub status: SuggestionStatus,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewBundleV1 {
    pub schema_version: u32,
    pub bundle_id: Uuid,
    pub project_id: ProjectId,
    pub project_hash: String,
    pub base_files: Vec<ReviewBaseFile>,
    pub reviewer: ReviewIdentity,
    pub exported_at: DateTime<Utc>,
    pub threads: Vec<ReviewThread>,
    pub suggestions: Vec<ReviewSuggestion>,
}

impl ReviewBundleV1 {
    pub const SCHEMA_VERSION: u32 = 1;
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TexLiveSnapshot {
    pub version: u16,
    pub date: chrono::NaiveDate,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimePlatform {
    Windows,
    Macos,
    Linux,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeArchitecture {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64")]
    Aarch64,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeArtifact {
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSignature {
    pub algorithm: String,
    pub canonicalization: String,
    pub key_id: String,
    pub value: String,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SbomFormat {
    #[serde(rename = "SPDX-JSON-2.3")]
    SpdxJson23,
    #[serde(rename = "CycloneDX-JSON-1.6")]
    CycloneDxJson16,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSbom {
    pub path: String,
    pub sha256: String,
    pub format: SbomFormat,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalDocument {
    pub path: String,
    pub sha256: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeManifestV1 {
    pub schema_version: u32,
    pub profile_id: String,
    pub tex_live_snapshot: TexLiveSnapshot,
    pub platform: RuntimePlatform,
    pub architecture: RuntimeArchitecture,
    pub engines: Vec<LatexEngine>,
    pub archive: RuntimeArtifact,
    pub signature: RuntimeSignature,
    pub sbom: RuntimeSbom,
    pub license_inventory: LocalDocument,
    pub created_at: DateTime<Utc>,
}

impl RuntimeManifestV1 {
    pub const SCHEMA_VERSION: u32 = 1;
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivSource {
    pub project_id: ProjectId,
    pub revision: Revision,
    pub main_file: String,
    pub source_sha256: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivRuntime {
    pub profile_id: String,
    pub manifest_sha256: String,
    pub engine: LatexEngine,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivPolicy {
    pub policy_id: String,
    pub submission_tools_commit: Option<String>,
}

#[derive(
    specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "camelCase")]
pub enum ArxivFindingSeverity {
    Info,
    Warning,
    Blocker,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivFinding {
    pub id: Uuid,
    pub severity: ArxivFindingSeverity,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivIncludedFile {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivExcludedFile {
    pub path: String,
    pub reason: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivCleanBuild {
    pub succeeded: bool,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub pdf_sha256: Option<String>,
    pub page_count: Option<u32>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivUserApproval {
    pub approved: bool,
    pub approved_at: Option<DateTime<Utc>>,
    pub approved_pdf_sha256: Option<String>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArxivPreflightReportV1 {
    pub schema_version: u32,
    pub report_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub source: ArxivSource,
    pub runtime: ArxivRuntime,
    pub policy: ArxivPolicy,
    pub findings: Vec<ArxivFinding>,
    pub included_files: Vec<ArxivIncludedFile>,
    pub excluded_files: Vec<ArxivExcludedFile>,
    pub clean_build: ArxivCleanBuild,
    pub user_approval: ArxivUserApproval,
    pub ready: bool,
}

impl ArxivPreflightReportV1 {
    pub const SCHEMA_VERSION: u32 = 1;

    #[must_use]
    pub fn computed_ready(&self) -> bool {
        self.clean_build.succeeded
            && self.clean_build.pdf_sha256.is_some()
            && self.user_approval.approved
            && self.user_approval.approved_at.is_some()
            && self.user_approval.approved_pdf_sha256 == self.clean_build.pdf_sha256
            && !self
                .findings
                .iter()
                .any(|finding| finding.severity == ArxivFindingSeverity::Blocker)
    }

    #[must_use]
    pub fn readiness_is_consistent(&self) -> bool {
        self.ready == self.computed_ready()
    }
}
