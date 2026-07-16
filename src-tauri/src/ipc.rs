//! Typed, window-scoped Tauri command boundary.
//!
//! The webview never receives a general filesystem or process primitive. Paths
//! entering this module must have been granted to the invoking native window by
//! the dialog plugin, and every project session is owned by exactly one window.

use crate::core::compile::{CompilePurpose, CompileSpec};
use crate::core::contracts::{
    ArxivFinding, ArxivFindingSeverity, CompileEvent, CompileJobId, Diagnostic, DiagnosticCategory,
    DiagnosticSeverity, DocumentOp, FileId, LatexEngine, PaperSettingsV1, ProjectId,
    ProjectSessionId, Revision, SnapshotId, SourceEdit, SourceSpan, TemplateId,
};
use crate::core::error::{AppError, AppResult};
use crate::core::latex::{discover_project_bibliographies, normalized_relative};
use crate::core::persistence::recover_pending_transactions;
use crate::core::preflight::{ArxivPreflight, PreflightContext, validate_report};
use crate::core::project::{NewProjectSpec, ProjectRegistry, ProjectSession};
use crate::core::source::hash_bytes;
use crate::services::citations::{
    BibEntryDraft, BibFinding, BibSearchResult, CitationError, CitationLookupService,
    CitationSourceFile, MetadataLookupRequest, MetadataLookupResponse,
    ensure_bibliography_can_be_renamed, parse_bibliography, plan_citation_key_rename,
    plan_delete_entry, plan_upsert_entry, search_bibliography,
};
use crate::services::compiler::{
    CanonicalProjectSnapshot, CompileExecutor, CompileLease, CompileScheduler, CompletionOutcome,
    QueueOutcome, SandboxedCompileExecutor, cancelled_compile, execute_compile,
};
use crate::services::runtime::InstalledRuntime;
use crate::services::sandbox::{FailClosedSandboxBroker, SandboxBroker, SandboxReadiness};
use crate::services::storage::{
    DurableHistory, SnapshotKind, SnapshotRecord, StorageLimits, excluded_history_path,
};
use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{Manager, State, Window};
use tauri_plugin_fs::FsExt;
use tauri_specta::Event as _;
use walkdir::WalkDir;

pub const DEFAULT_RUNTIME_PROFILE: &str = "texlive-2025-2025-08-03";
const MAX_CAPTURED_PROJECT_FILES: usize = 10_000;
const MAX_CAPTURED_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CAPTURED_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CITATION_RENAME_FILES: usize = 2_048;
const MAX_CITATION_RENAME_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CITATION_RENAME_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
struct SessionContext {
    owner_window: String,
    title: String,
    settings: PaperSettingsV1,
    project_key: String,
}

/// Process state deliberately contains no globally-addressable current
/// project. A command must present both a session ID and the native window that
/// owns it.
pub struct DesktopState {
    registry: ProjectRegistry,
    contexts: RwLock<HashMap<ProjectSessionId, SessionContext>>,
    history: DurableHistory,
    citation_lookup: CitationLookupService,
    sandbox_broker: Arc<dyn SandboxBroker>,
    compiler: Mutex<CompileScheduler>,
    automatic_compile_generations: RwLock<HashMap<ProjectSessionId, u64>>,
    snapshot_generations: RwLock<HashMap<ProjectSessionId, u64>>,
    last_automatic_snapshots: RwLock<HashMap<String, chrono::DateTime<Utc>>>,
    recovery_directory: PathBuf,
}

impl DesktopState {
    pub fn open(app_data_directory: impl AsRef<Path>) -> AppResult<Self> {
        let app_data_directory = app_data_directory.as_ref();
        std::fs::create_dir_all(app_data_directory).map_err(|error| {
            AppError::io(
                "create application data directory",
                app_data_directory,
                error,
            )
        })?;
        let recovery_directory = app_data_directory.join("recovery");
        std::fs::create_dir_all(&recovery_directory).map_err(|error| {
            AppError::io("create recovery directory", &recovery_directory, error)
        })?;
        recover_pending_transactions(&recovery_directory)?;
        let history =
            DurableHistory::open(app_data_directory.join("history"), StorageLimits::default())?;
        let citation_lookup =
            CitationLookupService::new().map_err(|error| AppError::InvalidProject {
                message: format!("could not initialize citation metadata broker: {error}"),
            })?;
        let sandbox_broker: Arc<dyn SandboxBroker> = Arc::new(FailClosedSandboxBroker::default());
        // Genuine runtime signing keys and packaged probe evidence are not yet
        // configured. The selector is nevertheless executable: when startup
        // eventually supplies both private-field trust tokens, it installs the
        // sandbox executor; every absent/mismatched state retains NoCompile.
        let compiler = select_startup_compile_scheduler(
            app_data_directory.join("compiler"),
            None,
            Arc::clone(&sandbox_broker),
        )?;
        Ok(Self {
            registry: ProjectRegistry::new(),
            contexts: RwLock::new(HashMap::new()),
            history,
            citation_lookup,
            sandbox_broker,
            compiler: Mutex::new(compiler),
            automatic_compile_generations: RwLock::new(HashMap::new()),
            snapshot_generations: RwLock::new(HashMap::new()),
            last_automatic_snapshots: RwLock::new(HashMap::new()),
            recovery_directory,
        })
    }

    fn register(
        &self,
        window_label: &str,
        session: ProjectSession,
        title: String,
        settings: PaperSettingsV1,
        project_key: String,
    ) -> AppResult<ProjectSessionId> {
        let session_id = session.session_id();
        let replaced = self.registry.insert(session);
        debug_assert_eq!(replaced, session_id);
        let previous = self.contexts.write().insert(
            session_id,
            SessionContext {
                owner_window: window_label.to_owned(),
                title,
                settings,
                project_key,
            },
        );
        if previous.is_some() {
            let _ = self.registry.remove(session_id);
            return Err(AppError::InvalidProject {
                message: "project session identifier collision".into(),
            });
        }
        Ok(session_id)
    }

    fn context_for(
        &self,
        window_label: &str,
        session_id: ProjectSessionId,
    ) -> AppResult<SessionContext> {
        let context = self
            .contexts
            .read()
            .get(&session_id)
            .cloned()
            .ok_or(AppError::SessionClosed)?;
        if context.owner_window != window_label {
            return Err(AppError::CapabilityDenied {
                capability: "project-session".into(),
                message: "this project belongs to a different native window".into(),
            });
        }
        Ok(context)
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn window_has_dirty_project(&self, window_label: &str) -> bool {
        let session_ids = self
            .contexts
            .read()
            .iter()
            .filter_map(|(session_id, context)| {
                (context.owner_window == window_label).then_some(*session_id)
            })
            .collect::<Vec<_>>();
        session_ids.into_iter().any(|session_id| {
            self.registry
                .get(session_id)
                .ok()
                .and_then(|shared| shared.lock().snapshot().ok())
                .is_some_and(|snapshot| snapshot.dirty)
        })
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn close_window_sessions(&self, window_label: &str) {
        let session_ids = self
            .contexts
            .read()
            .iter()
            .filter_map(|(session_id, context)| {
                (context.owner_window == window_label).then_some(*session_id)
            })
            .collect::<Vec<_>>();
        for session_id in session_ids {
            let _ = self.compiler.lock().remove_session(session_id);
            self.contexts.write().remove(&session_id);
            if let Ok(shared) = self.registry.remove(session_id) {
                let _ = shared.lock().close();
            }
        }
    }

    fn schedule_automatic_snapshot(
        &self,
        app_handle: tauri::AppHandle,
        session_id: ProjectSessionId,
    ) {
        let generation = {
            let mut generations = self.snapshot_generations.write();
            let generation = generations.entry(session_id).or_default();
            *generation = generation.saturating_add(1);
            *generation
        };
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            loop {
                let state = app_handle.state::<DesktopState>();
                if state.snapshot_generations.read().get(&session_id) != Some(&generation) {
                    return;
                }
                let Some(context) = state.contexts.read().get(&session_id).cloned() else {
                    return;
                };
                let now = Utc::now();
                let remaining_throttle = state
                    .last_automatic_snapshots
                    .read()
                    .get(&context.project_key)
                    .map(|last| 60 - now.signed_duration_since(*last).num_seconds())
                    .filter(|seconds| *seconds > 0);
                if let Some(seconds) = remaining_throttle {
                    tokio::time::sleep(std::time::Duration::from_secs(seconds as u64)).await;
                    continue;
                }
                let Ok(shared) = state.registry.get(session_id) else {
                    return;
                };
                let snapshot = {
                    let session = shared.lock();
                    capture_project_files(&session).map(|files| (session.revision(), files))
                };
                let Ok((revision, files)) = snapshot else {
                    return;
                };
                if state
                    .history
                    .create_snapshot(
                        &context.project_key,
                        SnapshotKind::Automatic,
                        None,
                        Some(revision),
                        &files,
                        now,
                    )
                    .and_then(|_| {
                        state
                            .history
                            .enforce_default_retention(&context.project_key, now)
                    })
                    .is_ok()
                {
                    state
                        .last_automatic_snapshots
                        .write()
                        .insert(context.project_key, now);
                }
                return;
            }
        });
    }

    fn invalidate_automatic_compile(&self, session_id: ProjectSessionId) {
        let mut generations = self.automatic_compile_generations.write();
        let generation = generations.entry(session_id).or_default();
        *generation = generation.saturating_add(1);
    }

    fn schedule_automatic_compile(
        &self,
        app_handle: tauri::AppHandle,
        owner_window: String,
        session_id: ProjectSessionId,
    ) {
        let generation = {
            let mut generations = self.automatic_compile_generations.write();
            let generation = generations.entry(session_id).or_default();
            *generation = generation.saturating_add(1);
            *generation
        };
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            let state = app_handle.state::<DesktopState>();
            if state.automatic_compile_generations.read().get(&session_id) != Some(&generation) {
                return;
            }
            let Ok(context) = state.context_for(&owner_window, session_id) else {
                return;
            };
            let Ok(shared) = state.registry.get(session_id) else {
                return;
            };
            let revision = shared.lock().revision();
            let Ok(queued) = queue_compile_for_owner(
                &state,
                &owner_window,
                session_id,
                revision,
                context.settings.engine,
            ) else {
                return;
            };
            emit_compile_events(&app_handle, &owner_window, session_id, &queued.events);
            if let Some(lease) = queued.initial_lease {
                spawn_compile_worker(app_handle, owner_window, session_id, lease);
            }
        });
    }
}

fn select_startup_compile_scheduler(
    compiler_root: impl AsRef<Path>,
    runtime: Option<InstalledRuntime>,
    broker: Arc<dyn SandboxBroker>,
) -> AppResult<CompileScheduler> {
    if let Some(runtime) = runtime
        && let Ok(executor) = SandboxedCompileExecutor::new(runtime, broker)
    {
        let executor: Arc<dyn CompileExecutor> = Arc::new(executor);
        return CompileScheduler::with_executor(compiler_root, executor);
    }
    CompileScheduler::fail_closed(compiler_root)
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub parent_directory: String,
    pub folder_name: String,
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub template_id: TemplateId,
    pub engine: LatexEngine,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiProjectFileKind {
    Tex,
    Bib,
    Asset,
    Style,
    Other,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiSourceEncoding {
    Utf8,
    NonUtf8,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiProjectFile {
    pub id: FileId,
    pub relative_path: String,
    pub kind: UiProjectFileKind,
    pub encoding: UiSourceEncoding,
    pub content: Option<String>,
    pub dirty: bool,
    pub byte_length: usize,
    pub sha256: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Utf8ConversionPreview {
    pub file_id: FileId,
    pub relative_path: String,
    pub original_sha256: String,
    pub original_byte_length: usize,
    pub reviewed_text: String,
    pub replacement_character_count: usize,
    pub utf8_byte_length: usize,
    pub utf8_sha256: String,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CompatibilitySeverity {
    Info,
    Warning,
    Blocked,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiCompatibilityFinding {
    pub id: String,
    pub severity: CompatibilitySeverity,
    pub title: String,
    pub detail: String,
    pub span: Option<SourceSpan>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiProjectSnapshot {
    pub session_id: ProjectSessionId,
    pub revision: Revision,
    pub root_path: String,
    pub title: String,
    pub main_file: FileId,
    pub files: Vec<UiProjectFile>,
    pub settings: PaperSettingsV1,
    pub compatibility: Vec<UiCompatibilityFinding>,
    pub dirty: bool,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiEditResult {
    pub revision: Revision,
    pub files: Vec<UiProjectFile>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiSaveResult {
    pub revision: Revision,
    pub saved_at: chrono::DateTime<Utc>,
    pub file_hashes: BTreeMap<FileId, String>,
}

#[derive(specta::Type, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiHistoryKind {
    Automatic,
    Named,
    PreRestore,
    PreAccept,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiHistoryEntry {
    pub id: SnapshotId,
    pub revision: Revision,
    pub created_at: chrono::DateTime<Utc>,
    pub label: String,
    pub kind: UiHistoryKind,
    pub changed_files: usize,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompileTicket {
    pub job_id: CompileJobId,
    pub revision: Revision,
    pub state: String,
}

#[derive(
    specta::Type, tauri_specta::Event, Debug, Clone, Serialize, Deserialize, PartialEq, Eq,
)]
#[serde(rename_all = "camelCase")]
#[tauri_specta(event_name = "setwright-compile-event")]
pub struct CompileEventEnvelope {
    pub session_id: ProjectSessionId,
    pub event: CompileEvent,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiPdfArtifact {
    pub job_id: CompileJobId,
    pub revision: Revision,
    pub sha256: String,
    pub stale: bool,
    /// Typed JSON byte array for the current command surface. A raw binary IPC
    /// response will replace this field when the frontend bridge consumes it.
    pub bytes: Vec<u8>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub archive_path: String,
    pub report_path: String,
    pub archive_hash: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CitationCommandError {
    #[error("{error}")]
    App { error: AppError },
    #[error("{error}")]
    Citation { error: CitationError },
}

impl From<AppError> for CitationCommandError {
    fn from(error: AppError) -> Self {
        Self::App { error }
    }
}

impl From<CitationError> for CitationCommandError {
    fn from(error: CitationError) -> Self {
        Self::Citation { error }
    }
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalCitationSearch {
    pub file_id: FileId,
    pub source_hash: String,
    pub has_parse_errors: bool,
    pub findings: Vec<BibFinding>,
    pub results: Vec<BibSearchResult>,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeReadiness {
    pub profile_id: String,
    pub runtime_manifest_keys_configured: bool,
    pub runtime_install_available: bool,
    pub sandbox_backend: Option<String>,
    pub sandbox_attested: bool,
    pub reason: String,
}

#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenedProjectWindow {
    pub window_label: String,
    pub session_id: ProjectSessionId,
}

#[tauri::command]
#[specta::specta]
pub fn create_project(
    window: Window,
    state: State<'_, DesktopState>,
    request: CreateProjectRequest,
) -> AppResult<UiProjectSnapshot> {
    validate_folder_name(&request.folder_name)?;
    let parent = canonical_scoped_directory(&window, Path::new(&request.parent_directory))?;
    let root = parent.join(&request.folder_name);
    if !root.starts_with(&parent) {
        return Err(AppError::PathOutsideRoot {
            path: root.to_string_lossy().into_owned(),
        });
    }
    let settings = PaperSettingsV1::new(
        "main.tex",
        request.template_id,
        DEFAULT_RUNTIME_PROFILE,
        request.engine,
    );
    let title = request.title.trim().to_owned();
    if title.is_empty() {
        return Err(AppError::InvalidProject {
            message: "a paper title is required".into(),
        });
    }
    let session = ProjectSession::create(
        &root,
        NewProjectSpec {
            settings: settings.clone(),
            title: title.clone(),
            authors: request.authors,
        },
        &state.recovery_directory,
    )?;
    let project_key = format!("project:{}", settings.project_id);
    let session_id = state.register(window.label(), session, title, settings, project_key)?;
    project_snapshot(&state, window.label(), session_id)
}

#[tauri::command]
#[specta::specta]
pub fn open_project(
    window: Window,
    state: State<'_, DesktopState>,
    root_path: String,
    main_file: Option<String>,
) -> AppResult<UiProjectSnapshot> {
    let root = canonical_scoped_directory(&window, Path::new(&root_path))?;
    let open_path = match main_file {
        Some(relative) => {
            let relative = validate_relative_source_path(&relative)?;
            root.join(relative)
        }
        None => root.clone(),
    };
    let session = ProjectSession::open_path(&open_path)?;
    let core_snapshot = session.snapshot()?;
    let main_relative = session
        .file(session.main_file_id())?
        .relative_path()
        .to_string_lossy()
        .replace('\\', "/");
    let settings = core_snapshot
        .settings
        .clone()
        .unwrap_or_else(|| imported_project_settings(&root, main_relative));
    let title = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Imported paper")
        .to_owned();
    let project_key = core_snapshot
        .settings
        .as_ref()
        .map(|stored| format!("project:{}", stored.project_id))
        .unwrap_or_else(|| imported_project_key(&root));
    let session_id = state.register(window.label(), session, title, settings, project_key)?;
    project_snapshot(&state, window.label(), session_id)
}

#[tauri::command]
#[specta::specta]
pub fn open_project_window(
    window: Window,
    state: State<'_, DesktopState>,
    root_path: String,
    main_file: Option<String>,
) -> AppResult<OpenedProjectWindow> {
    let root = canonical_scoped_directory(&window, Path::new(&root_path))?;
    let open_path = match main_file {
        Some(relative) => root.join(validate_relative_source_path(&relative)?),
        None => root.clone(),
    };
    let session = ProjectSession::open_path(&open_path)?;
    let core_snapshot = session.snapshot()?;
    let main_relative = session
        .file(session.main_file_id())?
        .relative_path()
        .to_string_lossy()
        .replace('\\', "/");
    let settings = core_snapshot
        .settings
        .clone()
        .unwrap_or_else(|| imported_project_settings(&root, main_relative));
    let title = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Imported paper")
        .to_owned();
    let project_key = core_snapshot
        .settings
        .as_ref()
        .map(|stored| format!("project:{}", stored.project_id))
        .unwrap_or_else(|| imported_project_key(&root));
    let window_label = format!("project-{}", uuid::Uuid::new_v4().simple());
    let session_id =
        state.register(&window_label, session, title.clone(), settings, project_key)?;
    let url = tauri::WebviewUrl::App(format!("index.html?session={session_id}").into());
    if let Err(error) = tauri::WebviewWindowBuilder::new(window.app_handle(), &window_label, url)
        .title(format!("{title} — Setwright"))
        .inner_size(1440.0, 920.0)
        .min_inner_size(980.0, 680.0)
        .build()
    {
        state.contexts.write().remove(&session_id);
        if let Ok(registered) = state.registry.remove(session_id) {
            let _ = registered.lock().close();
        }
        return Err(AppError::InvalidProject {
            message: format!("could not create a native project window: {error}"),
        });
    }
    Ok(OpenedProjectWindow {
        window_label,
        session_id,
    })
}

#[tauri::command]
#[specta::specta]
pub fn read_project(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
) -> AppResult<UiProjectSnapshot> {
    project_snapshot(&state, window.label(), session_id)
}

#[tauri::command]
#[specta::specta]
pub fn close_project(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
) -> AppResult<()> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    if shared.lock().snapshot()?.dirty {
        return Err(AppError::InvalidProject {
            message: "save or discard the unsaved changes before closing".into(),
        });
    }
    state.compiler.lock().remove_session(session_id)?;
    let shared = state.registry.remove(session_id)?;
    shared.lock().close()?;
    state.contexts.write().remove(&session_id);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn apply_source_edits(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    base_revision: Revision,
    edits: Vec<SourceEdit>,
) -> AppResult<UiEditResult> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    let result = session.apply_source_edits(base_revision, edits)?;
    let files = project_files(&session)?;
    let changed = result.changed;
    let response = UiEditResult {
        revision: result.revision,
        files,
        diagnostics: result.diagnostics,
    };
    drop(session);
    if changed {
        state
            .compiler
            .lock()
            .note_revision(session_id, response.revision);
        let app_handle = window.app_handle().clone();
        state.schedule_automatic_snapshot(app_handle.clone(), session_id);
        state.schedule_automatic_compile(app_handle, window.label().to_owned(), session_id);
    }
    Ok(response)
}

#[tauri::command]
#[specta::specta]
pub fn apply_document_op(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    base_revision: Revision,
    operation: DocumentOp,
) -> AppResult<UiEditResult> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    let result = session.apply_document_op(base_revision, operation)?;
    let files = project_files(&session)?;
    let changed = result.changed;
    let response = UiEditResult {
        revision: result.revision,
        files,
        diagnostics: result.diagnostics,
    };
    drop(session);
    if changed {
        state
            .compiler
            .lock()
            .note_revision(session_id, response.revision);
        let app_handle = window.app_handle().clone();
        state.schedule_automatic_snapshot(app_handle.clone(), session_id);
        state.schedule_automatic_compile(app_handle, window.label().to_owned(), session_id);
    }
    Ok(response)
}

#[tauri::command]
#[specta::specta]
pub fn prepare_utf8_conversion(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    file_id: FileId,
) -> AppResult<Utf8ConversionPreview> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let session = shared.lock();
    let descriptor = session.file(file_id)?;
    if !descriptor.is_source_only() {
        return Err(AppError::InvalidProject {
            message: "the selected file is already canonical UTF-8".into(),
        });
    }
    let contents = session.read_file(file_id)?;
    let original_sha256 = hash_bytes(&contents.bytes);
    let reviewed_text = String::from_utf8_lossy(&contents.bytes).into_owned();
    let replacement_character_count = reviewed_text.matches('\u{FFFD}').count();
    Ok(Utf8ConversionPreview {
        file_id,
        relative_path: descriptor.relative_path().to_string_lossy().into_owned(),
        original_sha256,
        original_byte_length: contents.bytes.len(),
        utf8_byte_length: reviewed_text.len(),
        utf8_sha256: hash_bytes(reviewed_text.as_bytes()),
        reviewed_text,
        replacement_character_count,
    })
}

#[tauri::command]
#[specta::specta]
pub fn convert_file_to_utf8(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    base_revision: Revision,
    file_id: FileId,
    reviewed_text: String,
    expected_original_sha256: String,
) -> AppResult<UiEditResult> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    let actual = session.file(file_id)?.content_hash().to_owned();
    if actual != expected_original_sha256 {
        return Err(AppError::HashMismatch {
            expected: expected_original_sha256,
            actual,
        });
    }
    let result = session.convert_file_to_utf8(file_id, base_revision, reviewed_text)?;
    let files = project_files(&session)?;
    let changed = result.changed;
    let response = UiEditResult {
        revision: result.revision,
        files,
        diagnostics: result.diagnostics,
    };
    drop(session);
    if changed {
        state
            .compiler
            .lock()
            .note_revision(session_id, response.revision);
        let app_handle = window.app_handle().clone();
        state.schedule_automatic_snapshot(app_handle.clone(), session_id);
        state.schedule_automatic_compile(app_handle, window.label().to_owned(), session_id);
    }
    Ok(response)
}

#[tauri::command]
#[specta::specta]
pub fn save_project(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    expected_revision: Revision,
) -> AppResult<UiSaveResult> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    if session.revision() != expected_revision {
        return Err(AppError::RevisionConflict {
            expected: expected_revision.0,
            actual: session.revision().0,
        });
    }
    session.save(&state.recovery_directory)?;
    let saved_at = Utc::now();
    let file_hashes = session
        .files()?
        .into_iter()
        .map(|file| (file.file_id, file.content_hash))
        .collect();
    Ok(UiSaveResult {
        revision: session.revision(),
        saved_at,
        file_hashes,
    })
}

#[tauri::command]
#[specta::specta]
pub fn search_local_citations(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    file_id: FileId,
    query: String,
) -> Result<LocalCitationSearch, CitationCommandError> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let session = shared.lock();
    ensure_bibliography_file(&session, file_id)?;
    let contents = session.read_file(file_id)?;
    let source = String::from_utf8(contents.bytes).map_err(|_| AppError::InvalidUtf8 {
        path: contents.file.relative_path,
    })?;
    let document = parse_bibliography(&source)?;
    let results = search_bibliography(&document, &query);
    Ok(LocalCitationSearch {
        file_id,
        source_hash: document.source_hash,
        has_parse_errors: document.has_parse_errors,
        findings: document.findings,
        results,
    })
}

#[tauri::command]
#[specta::specta]
pub fn upsert_bibliography_entry(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    base_revision: Revision,
    file_id: FileId,
    draft: BibEntryDraft,
) -> Result<UiEditResult, CitationCommandError> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    ensure_bibliography_file(&session, file_id)?;
    let contents = session.read_file(file_id)?;
    let source = String::from_utf8(contents.bytes).map_err(|_| AppError::InvalidUtf8 {
        path: contents.file.relative_path,
    })?;
    let plan = plan_upsert_entry(file_id, &source, &draft)?;
    let response = citation_edit_result(&mut session, base_revision, plan.edits)?;
    drop(session);
    state
        .compiler
        .lock()
        .note_revision(session_id, response.revision);
    let app_handle = window.app_handle().clone();
    state.schedule_automatic_snapshot(app_handle.clone(), session_id);
    state.schedule_automatic_compile(app_handle, window.label().to_owned(), session_id);
    Ok(response)
}

#[tauri::command]
#[specta::specta]
pub fn delete_bibliography_entry(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    base_revision: Revision,
    file_id: FileId,
    key: String,
) -> Result<UiEditResult, CitationCommandError> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    ensure_bibliography_file(&session, file_id)?;
    let contents = session.read_file(file_id)?;
    let source = String::from_utf8(contents.bytes).map_err(|_| AppError::InvalidUtf8 {
        path: contents.file.relative_path,
    })?;
    let plan = plan_delete_entry(file_id, &source, &key)?;
    let response = citation_edit_result(&mut session, base_revision, plan.edits)?;
    drop(session);
    state
        .compiler
        .lock()
        .note_revision(session_id, response.revision);
    let app_handle = window.app_handle().clone();
    state.schedule_automatic_snapshot(app_handle.clone(), session_id);
    state.schedule_automatic_compile(app_handle, window.label().to_owned(), session_id);
    Ok(response)
}

#[tauri::command]
#[specta::specta]
pub fn rename_citation_key(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    base_revision: Revision,
    bibliography_file_id: FileId,
    old_key: String,
    new_key: String,
) -> Result<UiEditResult, CitationCommandError> {
    state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let mut session = shared.lock();
    ensure_bibliography_file(&session, bibliography_file_id)?;
    if old_key == new_key {
        return citation_edit_result(&mut session, base_revision, Vec::new());
    }
    let (bibliography_source, tex_files) =
        citation_rename_sources(&session, bibliography_file_id, &old_key, &new_key)?;
    let plan = plan_citation_key_rename(
        bibliography_file_id,
        &bibliography_source,
        &tex_files,
        &old_key,
        &new_key,
    )?;
    let edits = plan.edits_by_file.into_values().flatten().collect();
    let response = citation_edit_result(&mut session, base_revision, edits)?;
    drop(session);
    state
        .compiler
        .lock()
        .note_revision(session_id, response.revision);
    let app_handle = window.app_handle().clone();
    state.schedule_automatic_snapshot(app_handle.clone(), session_id);
    state.schedule_automatic_compile(app_handle, window.label().to_owned(), session_id);
    Ok(response)
}

#[tauri::command]
#[specta::specta]
pub async fn lookup_citation_metadata(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    request: MetadataLookupRequest,
) -> Result<MetadataLookupResponse, CitationCommandError> {
    state.context_for(window.label(), session_id)?;
    Ok(state.citation_lookup.lookup_explicit(request).await?)
}

#[tauri::command]
#[specta::specta]
pub fn get_runtime_readiness(state: State<'_, DesktopState>) -> RuntimeReadiness {
    match state.sandbox_broker.readiness() {
        SandboxReadiness::Unavailable { backend, reason } => RuntimeReadiness {
            profile_id: DEFAULT_RUNTIME_PROFILE.into(),
            runtime_manifest_keys_configured: false,
            runtime_install_available: false,
            sandbox_backend: backend.map(sandbox_backend_name).map(str::to_owned),
            sandbox_attested: false,
            reason: format!(
                "runtime hosting/signing keys are not configured; sandbox unavailable: {reason}"
            ),
        },
        SandboxReadiness::Attested { backend, .. } => RuntimeReadiness {
            profile_id: DEFAULT_RUNTIME_PROFILE.into(),
            runtime_manifest_keys_configured: false,
            runtime_install_available: false,
            sandbox_backend: Some(sandbox_backend_name(backend).into()),
            sandbox_attested: true,
            reason: "sandbox evidence exists, but runtime hosting/signing keys are not configured"
                .into(),
        },
    }
}

#[tauri::command]
#[specta::specta]
pub fn start_compile(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    revision: Revision,
    engine: LatexEngine,
) -> AppResult<CompileTicket> {
    state.context_for(window.label(), session_id)?;
    state.invalidate_automatic_compile(session_id);
    let queued = queue_compile_for_owner(&state, window.label(), session_id, revision, engine)?;
    for event in &queued.events {
        let _ = CompileEventEnvelope {
            session_id,
            event: event.clone(),
        }
        .emit(&window);
    }
    if let Some(lease) = queued.initial_lease {
        spawn_compile_worker(
            window.app_handle().clone(),
            window.label().to_owned(),
            session_id,
            lease,
        );
    }
    Ok(queued.ticket)
}

#[tauri::command]
#[specta::specta]
pub fn cancel_compile(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    job_id: CompileJobId,
) -> AppResult<()> {
    state.context_for(window.label(), session_id)?;
    let cancelled = state.compiler.lock().cancel_job(session_id, job_id)?;
    // Queued work has no worker to report its terminal state. Active work
    // emits Cancelled only after its process tree has actually returned.
    if cancelled.queued_job_id.is_some() {
        let _ = CompileEventEnvelope {
            session_id,
            event: CompileEvent::Cancelled { job_id },
        }
        .emit(&window);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn read_compile_pdf(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
) -> AppResult<UiPdfArtifact> {
    state.context_for(window.label(), session_id)?;
    let compiler = state.compiler.lock();
    let pdf = compiler
        .last_successful_pdf(session_id)
        .ok_or(AppError::CompileUnavailable {
            message: "this project has no successful sandboxed PDF artifact".into(),
        })?;
    Ok(UiPdfArtifact {
        job_id: pdf.job_id,
        revision: pdf.revision,
        sha256: pdf.sha256.clone(),
        stale: pdf.stale,
        bytes: pdf.bytes.clone(),
    })
}

#[tauri::command]
#[specta::specta]
pub fn list_history(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
) -> AppResult<Vec<UiHistoryEntry>> {
    let context = state.context_for(window.label(), session_id)?;
    state
        .history
        .list_snapshots(&context.project_key)?
        .into_iter()
        .map(history_entry)
        .collect()
}

#[tauri::command]
#[specta::specta]
pub fn create_snapshot(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    name: Option<String>,
) -> AppResult<UiHistoryEntry> {
    let context = state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let session = shared.lock();
    let files = capture_project_files(&session)?;
    let name = name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let outcome = state.history.create_snapshot(
        &context.project_key,
        SnapshotKind::Named,
        name.or(Some("Named version")),
        Some(session.revision()),
        &files,
        Utc::now(),
    )?;
    history_entry(outcome.record)
}

#[tauri::command]
#[specta::specta]
pub fn restore_snapshot(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    snapshot_id: SnapshotId,
) -> AppResult<UiProjectSnapshot> {
    let context = state.context_for(window.label(), session_id)?;
    let shared = state.registry.get(session_id)?;
    let (root, main_relative, current_revision, current_files) = {
        let session = shared.lock();
        (
            session.root().to_path_buf(),
            session
                .file(session.main_file_id())?
                .relative_path()
                .to_path_buf(),
            session.revision(),
            capture_project_files(&session)?,
        )
    };
    let target = state.history.get_snapshot(snapshot_id)?;
    if target.project_key != context.project_key {
        return Err(AppError::CapabilityDenied {
            capability: "history-snapshot".into(),
            message: "the snapshot belongs to another project".into(),
        });
    }
    let pre_restore = state.history.create_snapshot(
        &context.project_key,
        SnapshotKind::PreRestore,
        Some("Before history restore"),
        Some(current_revision),
        &current_files,
        Utc::now(),
    )?;
    state
        .history
        .restore_snapshot_to_directory(snapshot_id, &root)?;

    // Re-open the exact selected main file before swapping registry state. An
    // imported project may have multiple top-level TeX files and no settings,
    // so rediscovery from the root is not equivalent. If rebuilding projections
    // fails, roll the disk back to the pre-restore snapshot and leave the live
    // session/context intact.
    let reopened = match ProjectSession::open_path(root.join(&main_relative)) {
        Ok(session) => session,
        Err(error) => {
            state
                .history
                .restore_snapshot_to_directory(pre_restore.record.snapshot_id, &root)?;
            return Err(error);
        }
    };
    if let Err(error) = state.compiler.lock().remove_session(session_id) {
        state
            .history
            .restore_snapshot_to_directory(pre_restore.record.snapshot_id, &root)?;
        return Err(error);
    }
    let new_id = reopened.session_id();
    state.registry.insert(reopened);
    {
        let mut contexts = state.contexts.write();
        contexts.insert(new_id, context);
        contexts.remove(&session_id);
    }
    let previous = state.registry.remove(session_id)?;
    previous.lock().close()?;
    project_snapshot(&state, window.label(), new_id)
}

/// The runtime service owns network/process access. Until its signed profile
/// and platform sandbox attestation are wired, the public boundary fails
/// closed rather than producing a misleading artifact.
#[tauri::command]
#[specta::specta]
pub fn run_arxiv_preflight(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    revision: Revision,
) -> AppResult<crate::core::contracts::ArxivPreflightReportV1> {
    static_preflight_for_owner(&state, window.label(), session_id, revision)
}

#[tauri::command]
#[specta::specta]
pub fn export_arxiv(
    window: Window,
    state: State<'_, DesktopState>,
    session_id: ProjectSessionId,
    revision: Revision,
    destination: String,
) -> AppResult<ExportResult> {
    state.context_for(window.label(), session_id)?;
    let destination = canonical_scoped_directory(&window, Path::new(&destination))?;
    let shared = state.registry.get(session_id)?;
    let session = shared.lock();
    if session.revision() != revision {
        return Err(AppError::RevisionConflict {
            expected: revision.0,
            actual: session.revision().0,
        });
    }
    let _ = destination;
    Err(AppError::CompileUnavailable {
        message: "export is blocked until the exact ZIP passes a second clean sandbox build and PDF approval".into(),
    })
}

#[derive(Debug)]
struct QueuedCompileInvocation {
    ticket: CompileTicket,
    events: Vec<CompileEvent>,
    initial_lease: Option<CompileLease>,
}

fn queue_compile_for_owner(
    state: &DesktopState,
    window_label: &str,
    session_id: ProjectSessionId,
    revision: Revision,
    engine: LatexEngine,
) -> AppResult<QueuedCompileInvocation> {
    let (snapshot, _) = canonical_compile_snapshot(
        state,
        window_label,
        session_id,
        revision,
        engine,
        CompilePurpose::Preview,
    )?;
    let mut compiler = state.compiler.lock();
    let (job_id, job_revision, superseded_queued_job_id, cancelled_active_job_id) =
        match compiler.queue(snapshot)? {
            QueueOutcome::Queued {
                job_id,
                revision,
                superseded_queued_job_id,
                cancelled_active_job_id,
            } => (
                job_id,
                revision,
                superseded_queued_job_id,
                cancelled_active_job_id,
            ),
            QueueOutcome::IgnoredOlder {
                requested_revision,
                newest_revision,
                ..
            } => {
                return Err(AppError::RevisionConflict {
                    expected: requested_revision.0,
                    actual: newest_revision.0,
                });
            }
        };
    let initial_lease = if cancelled_active_job_id.is_none() {
        compiler.begin_next(session_id)?
    } else {
        None
    };
    let mut events = vec![CompileEvent::Queued {
        job_id,
        revision: job_revision,
    }];
    if let Some(superseded) = superseded_queued_job_id {
        events.push(CompileEvent::Cancelled { job_id: superseded });
    }
    if initial_lease.is_some() {
        events.push(CompileEvent::Started {
            job_id,
            revision: job_revision,
        });
    }
    Ok(QueuedCompileInvocation {
        ticket: CompileTicket {
            job_id,
            revision: job_revision,
            state: if initial_lease.is_some() {
                "started".into()
            } else {
                "queued".into()
            },
        },
        events,
        initial_lease,
    })
}

fn spawn_compile_worker(
    app_handle: tauri::AppHandle,
    owner_window: String,
    session_id: ProjectSessionId,
    initial_lease: CompileLease,
) {
    tauri::async_runtime::spawn(async move {
        let mut next_lease = Some(initial_lease);
        while let Some(lease) = next_lease.take() {
            let job_id = lease.request().job().job_id;
            let job_revision = lease.request().job().revision;
            let execution_lease = lease.clone();
            let executor = app_handle
                .state::<DesktopState>()
                .compiler
                .lock()
                .executor_handle();
            let blocking_executor = Arc::clone(&executor);
            let executed = tauri::async_runtime::spawn_blocking(move || {
                execute_compile(blocking_executor.as_ref(), &execution_lease)
            })
            .await;

            let mut worker_failure = None;
            let executed = match executed {
                Ok(executed) => executed,
                Err(error) => {
                    worker_failure = Some(format!("compile worker failed: {error}"));
                    let _ = lease.cancellation().cancel();
                    execute_compile(executor.as_ref(), &lease)
                }
            };

            let state = app_handle.state::<DesktopState>();
            let current_revision = state
                .registry
                .get(session_id)
                .ok()
                .map(|shared| shared.lock().revision())
                .unwrap_or_else(|| {
                    let _ = lease.cancellation().cancel();
                    job_revision
                });
            let (mut events, following_lease) = {
                let mut compiler = state.compiler.lock();
                let events = match compiler.complete(&lease, executed, current_revision) {
                    Ok(completion) => compile_completion_events(&compiler, session_id, completion),
                    Err(error) => internal_compile_failure_events(
                        job_id,
                        job_revision,
                        &format!("could not finalize compile: {error}"),
                    ),
                };
                let following = match compiler.begin_next(session_id) {
                    Ok(lease) => lease,
                    Err(error) => {
                        worker_failure = Some(format!("could not drain compile queue: {error}"));
                        None
                    }
                };
                (events, following)
            };
            if let Some(message) = worker_failure {
                events.insert(0, internal_compile_diagnostic(job_id, &message));
            }
            emit_compile_events(&app_handle, &owner_window, session_id, &events);
            if let Some(following) = &following_lease {
                emit_compile_events(
                    &app_handle,
                    &owner_window,
                    session_id,
                    &[CompileEvent::Started {
                        job_id: following.request().job().job_id,
                        revision: following.request().job().revision,
                    }],
                );
            }
            next_lease = following_lease;
        }
    });
}

fn compile_completion_events(
    compiler: &CompileScheduler,
    session_id: ProjectSessionId,
    completion: CompletionOutcome,
) -> Vec<CompileEvent> {
    match completion {
        CompletionOutcome::Published(publication) => {
            let job_id = publication.job_id;
            let revision = publication.revision;
            let mut events = publication
                .diagnostics
                .into_iter()
                .map(|diagnostic| CompileEvent::Diagnostic { job_id, diagnostic })
                .collect::<Vec<_>>();
            events.push(CompileEvent::Finished {
                job_id,
                revision,
                success: true,
                pdf_hash: publication.pdf.map(|pdf| pdf.sha256),
                stale: false,
            });
            events
        }
        CompletionOutcome::Failed { publication, .. } => {
            let job_id = publication.job_id;
            let revision = publication.revision;
            let mut events = publication
                .diagnostics
                .into_iter()
                .map(|diagnostic| CompileEvent::Diagnostic { job_id, diagnostic })
                .collect::<Vec<_>>();
            let stale = compiler
                .last_successful_pdf(session_id)
                .is_some_and(|pdf| pdf.stale);
            events.push(CompileEvent::Finished {
                job_id,
                revision,
                success: false,
                pdf_hash: None,
                stale,
            });
            events
        }
        CompletionOutcome::Cancelled { job_id } => vec![CompileEvent::Cancelled { job_id }],
        CompletionOutcome::DiscardedStale {
            job_id,
            job_revision,
            ..
        } => vec![CompileEvent::Finished {
            job_id,
            revision: job_revision,
            success: false,
            pdf_hash: None,
            stale: true,
        }],
    }
}

fn internal_compile_failure_events(
    job_id: CompileJobId,
    revision: Revision,
    message: &str,
) -> Vec<CompileEvent> {
    vec![
        internal_compile_diagnostic(job_id, message),
        CompileEvent::Finished {
            job_id,
            revision,
            success: false,
            pdf_hash: None,
            stale: true,
        },
    ]
}

fn internal_compile_diagnostic(job_id: CompileJobId, message: &str) -> CompileEvent {
    CompileEvent::Diagnostic {
        job_id,
        diagnostic: Diagnostic {
            code: "COMPILE_WORKER_FAILURE".into(),
            severity: DiagnosticSeverity::Error,
            category: DiagnosticCategory::Internal,
            title: "Compile worker failed".into(),
            message: message.into(),
            span: None,
            source_line: None,
            actions: Vec::new(),
            technical_detail: Some(message.into()),
        },
    }
}

fn emit_compile_events(
    app_handle: &tauri::AppHandle,
    owner_window: &str,
    session_id: ProjectSessionId,
    events: &[CompileEvent],
) {
    let Some(window) = app_handle.get_webview_window(owner_window) else {
        return;
    };
    for event in events {
        let _ = CompileEventEnvelope {
            session_id,
            event: event.clone(),
        }
        .emit(&window);
    }
}

#[cfg(test)]
#[derive(Debug)]
struct CompileInvocation {
    ticket: CompileTicket,
    events: Vec<CompileEvent>,
    error: Option<AppError>,
}

#[cfg(test)]
fn run_compile_for_owner(
    state: &DesktopState,
    window_label: &str,
    session_id: ProjectSessionId,
    revision: Revision,
    engine: LatexEngine,
) -> AppResult<CompileInvocation> {
    let (snapshot, _) = canonical_compile_snapshot(
        state,
        window_label,
        session_id,
        revision,
        engine,
        CompilePurpose::Preview,
    )?;
    let (job_id, job_revision, lease) = {
        let mut compiler = state.compiler.lock();
        let queued = compiler.queue(snapshot)?;
        let (job_id, job_revision) = match queued {
            QueueOutcome::Queued {
                job_id, revision, ..
            } => (job_id, revision),
            QueueOutcome::IgnoredOlder {
                requested_revision,
                newest_revision,
                ..
            } => {
                return Err(AppError::RevisionConflict {
                    expected: requested_revision.0,
                    actual: newest_revision.0,
                });
            }
        };
        let lease = compiler
            .begin_next(session_id)?
            .ok_or(AppError::CompileUnavailable {
                message: "the compile queue did not produce an executable job".into(),
            })?;
        (job_id, job_revision, lease)
    };
    let mut events = vec![CompileEvent::Queued {
        job_id,
        revision: job_revision,
    }];
    events.push(CompileEvent::Started {
        job_id,
        revision: job_revision,
    });
    // A real attested executor may run for minutes. It must never borrow the
    // scheduler mutex: newer revisions and typed cancellation need to reach
    // the active process-tree token while execution is in progress.
    let executor = state.compiler.lock().executor_handle();
    let executed = execute_compile(executor.as_ref(), &lease);
    let current_revision = state.registry.get(session_id)?.lock().revision();
    let mut compiler = state.compiler.lock();
    let completion = compiler.complete(&lease, executed, current_revision)?;
    let mut ticket = CompileTicket {
        job_id,
        revision: job_revision,
        state: "finished".into(),
    };
    let error = match completion {
        CompletionOutcome::Published(publication) => {
            for diagnostic in publication.diagnostics {
                events.push(CompileEvent::Diagnostic { job_id, diagnostic });
            }
            let pdf_hash = publication.pdf.as_ref().map(|pdf| pdf.sha256.clone());
            events.push(CompileEvent::Finished {
                job_id,
                revision: job_revision,
                success: true,
                pdf_hash,
                stale: false,
            });
            None
        }
        CompletionOutcome::Failed { publication, error } => {
            ticket.state = "failed".into();
            for diagnostic in publication.diagnostics {
                events.push(CompileEvent::Diagnostic { job_id, diagnostic });
            }
            let stale = compiler
                .last_successful_pdf(session_id)
                .is_some_and(|pdf| pdf.stale);
            events.push(CompileEvent::Finished {
                job_id,
                revision: job_revision,
                success: false,
                pdf_hash: None,
                stale,
            });
            Some(error)
        }
        CompletionOutcome::Cancelled { .. } => {
            ticket.state = "cancelled".into();
            events.push(CompileEvent::Cancelled { job_id });
            Some(AppError::CompileUnavailable {
                message: "the compile was cancelled".into(),
            })
        }
        CompletionOutcome::DiscardedStale {
            current_revision, ..
        } => {
            ticket.state = "discardedStale".into();
            events.push(CompileEvent::Finished {
                job_id,
                revision: job_revision,
                success: false,
                pdf_hash: None,
                stale: true,
            });
            Some(AppError::RevisionConflict {
                expected: job_revision.0,
                actual: current_revision.0,
            })
        }
    };
    Ok(CompileInvocation {
        ticket,
        events,
        error,
    })
}

fn canonical_compile_snapshot(
    state: &DesktopState,
    window_label: &str,
    session_id: ProjectSessionId,
    revision: Revision,
    engine: LatexEngine,
    purpose: CompilePurpose,
) -> AppResult<(CanonicalProjectSnapshot, SessionContext)> {
    let context = state.context_for(window_label, session_id)?;
    let backend = match state.sandbox_broker.readiness() {
        SandboxReadiness::Unavailable {
            backend: Some(backend),
            ..
        }
        | SandboxReadiness::Attested { backend, .. } => backend,
        SandboxReadiness::Unavailable {
            backend: None,
            reason,
        } => {
            return Err(AppError::CompileUnavailable { message: reason });
        }
    };
    let shared = state.registry.get(session_id)?;
    let session = shared.lock();
    session.ensure_disk_matches_persisted()?;
    if session.revision() != revision {
        return Err(AppError::RevisionConflict {
            expected: revision.0,
            actual: session.revision().0,
        });
    }
    let main_file = session
        .file(session.main_file_id())?
        .relative_path()
        .to_string_lossy()
        .replace('\\', "/");
    let mut spec = match purpose {
        CompilePurpose::Preview | CompilePurpose::ReviewOverlay => CompileSpec::preview(
            context.settings.runtime_id.clone(),
            engine,
            Path::new(&main_file),
            backend,
        )?,
        CompilePurpose::ArxivPreflight => CompileSpec::preflight(
            context.settings.runtime_id.clone(),
            engine,
            Path::new(&main_file),
            backend,
        )?,
    };
    if purpose == CompilePurpose::ReviewOverlay {
        spec.purpose = CompilePurpose::ReviewOverlay;
    }
    let files = capture_compile_files(&session, &main_file)?;
    Ok((
        CanonicalProjectSnapshot::new(session_id, revision, spec, files),
        context,
    ))
}

fn static_preflight_for_owner(
    state: &DesktopState,
    window_label: &str,
    session_id: ProjectSessionId,
    revision: Revision,
) -> AppResult<crate::core::contracts::ArxivPreflightReportV1> {
    let context = state.context_for(window_label, session_id)?;
    let (snapshot, _) = canonical_compile_snapshot(
        state,
        window_label,
        session_id,
        revision,
        context.settings.engine,
        CompilePurpose::ArxivPreflight,
    )?;
    let (job_id, lease) = {
        let mut compiler = state.compiler.lock();
        let job_id = match compiler.queue(snapshot)? {
            QueueOutcome::Queued { job_id, .. } => job_id,
            QueueOutcome::IgnoredOlder {
                requested_revision,
                newest_revision,
                ..
            } => {
                return Err(AppError::RevisionConflict {
                    expected: requested_revision.0,
                    actual: newest_revision.0,
                });
            }
        };
        let lease = compiler
            .begin_next(session_id)?
            .ok_or(AppError::CompileUnavailable {
                message: "the preflight staging queue did not produce a job".into(),
            })?;
        (job_id, lease)
    };
    let scan = ArxivPreflight::scan(
        lease.request().stage_directory(),
        PreflightContext {
            project_id: context.settings.project_id,
            revision,
            main_file: lease.request().job().spec.main_file.clone(),
            runtime_profile_id: context.settings.runtime_id,
            runtime_manifest_sha256: "0".repeat(64),
            engine: context.settings.engine,
            policy_id: "setwright-arxiv-static-v1".into(),
            submission_tools_commit: None,
            recorder_inputs: Vec::new(),
        },
    );

    // Clear the staging lease through the same scheduler lifecycle without
    // consulting the production executor or introducing a process launch.
    lease.cancellation().cancel()?;
    let executed = cancelled_compile(&lease);
    let mut compiler = state.compiler.lock();
    let _ = compiler.complete(&lease, executed, revision)?;
    drop(compiler);

    let mut report = scan?;
    report.findings.extend([
        preflight_blocker(
            "RUNTIME_SANDBOX_UNATTESTED",
            "No signed managed runtime and hostile-project sandbox attestation is available; no TeX process was launched.",
        ),
        preflight_blocker(
            "CLEAN_ARCHIVE_BUILD_NOT_RUN",
            "The exact candidate archive has not been re-extracted and compiled in a second clean sandbox.",
        ),
        preflight_blocker(
            "ARXIV_ORACLE_UNPINNED",
            "The official arXiv submission-tools oracle commit is not pinned in this build.",
        ),
    ]);
    report.ready = false;
    debug_assert_eq!(lease.request().job().job_id, job_id);
    validate_report(&report)?;
    Ok(report)
}

fn preflight_blocker(code: &str, message: &str) -> ArxivFinding {
    ArxivFinding {
        id: uuid::Uuid::new_v4(),
        severity: ArxivFindingSeverity::Blocker,
        code: code.into(),
        message: message.into(),
        path: None,
        line: None,
    }
}

pub fn command_builder() -> tauri_specta::Builder<tauri::Wry> {
    // Public byte offsets are capped at 64 MiB and a revision cannot approach
    // JavaScript's 2^53 exact-integer limit in a real project session.
    tauri_specta::Builder::<tauri::Wry>::new()
        .dangerously_cast_bigints_to_number()
        .commands(tauri_specta::collect_commands![
            create_project,
            open_project,
            open_project_window,
            read_project,
            close_project,
            apply_source_edits,
            apply_document_op,
            prepare_utf8_conversion,
            convert_file_to_utf8,
            save_project,
            search_local_citations,
            upsert_bibliography_entry,
            delete_bibliography_entry,
            rename_citation_key,
            lookup_citation_metadata,
            get_runtime_readiness,
            start_compile,
            cancel_compile,
            read_compile_pdf,
            list_history,
            create_snapshot,
            restore_snapshot,
            run_arxiv_preflight,
            export_arxiv,
        ])
        .events(tauri_specta::collect_events![CompileEventEnvelope])
        .typ::<crate::core::contracts::DocumentOp>()
        .typ::<crate::core::contracts::ProjectEvent>()
        .typ::<crate::core::contracts::CompileEvent>()
        .typ::<crate::core::contracts::ReviewBundleV1>()
        .typ::<crate::core::contracts::RuntimeManifestV1>()
}

fn project_snapshot(
    state: &DesktopState,
    window_label: &str,
    session_id: ProjectSessionId,
) -> AppResult<UiProjectSnapshot> {
    let context = state.context_for(window_label, session_id)?;
    let shared = state.registry.get(session_id)?;
    let session = shared.lock();
    let snapshot = session.snapshot()?;
    let files = project_files(&session)?;
    let mut compatibility = Vec::new();
    for file in &files {
        if file.encoding == UiSourceEncoding::NonUtf8 {
            compatibility.push(UiCompatibilityFinding {
                id: format!("non-utf8:{}", file.id),
                severity: CompatibilitySeverity::Blocked,
                title: "Source-only file".into(),
                detail: format!(
                    "{} is not UTF-8 and will remain source-only until an explicit reviewed conversion.",
                    file.relative_path
                ),
                span: None,
            });
            continue;
        }
        if file.kind != UiProjectFileKind::Tex {
            continue;
        }
        let report = session.compatibility_report(file.id)?;
        if report.has_parse_errors {
            compatibility.push(UiCompatibilityFinding {
                id: format!("parse-recovery:{}", file.id),
                severity: CompatibilitySeverity::Blocked,
                title: "Parser recovery region".into(),
                detail: format!(
                    "{} contains malformed or incomplete LaTeX; affected bytes remain raw.",
                    file.relative_path
                ),
                span: None,
            });
        }
        for (reason, count) in &report.raw_reasons {
            compatibility.push(UiCompatibilityFinding {
                id: format!("raw:{}:{}", file.id, hash_bytes(reason.as_bytes())),
                severity: CompatibilitySeverity::Warning,
                title: "Preserved raw LaTeX".into(),
                detail: format!(
                    "{} has {count} exact source region(s) preserved as raw: {reason}.",
                    file.relative_path
                ),
                span: None,
            });
        }
    }
    if snapshot.include_graph_stale {
        compatibility.push(UiCompatibilityFinding {
            id: "include-graph-stale".into(),
            severity: CompatibilitySeverity::Info,
            title: "Include graph will refresh on save".into(),
            detail: "Visual operations remain inside the currently loaded file boundaries.".into(),
            span: None,
        });
    }
    for (index, issue) in snapshot.include_graph.issues.iter().enumerate() {
        compatibility.push(UiCompatibilityFinding {
            id: format!("include:{}:{index}", issue.code),
            severity: CompatibilitySeverity::Warning,
            title: "Include kept source-only".into(),
            detail: format!(
                "{} (from {}{})",
                issue.message,
                issue.from,
                issue
                    .target
                    .as_deref()
                    .map(|target| format!(", target {target}"))
                    .unwrap_or_default()
            ),
            span: None,
        });
    }
    Ok(UiProjectSnapshot {
        session_id,
        revision: session.revision(),
        root_path: session.root().to_string_lossy().into_owned(),
        title: context.title,
        main_file: session.main_file_id(),
        files,
        settings: context.settings,
        compatibility,
        dirty: snapshot.dirty,
    })
}

fn project_files(session: &ProjectSession) -> AppResult<Vec<UiProjectFile>> {
    session
        .files()?
        .into_iter()
        .map(|descriptor| {
            let contents = session.read_file(descriptor.file_id)?;
            let content = String::from_utf8(contents.bytes).ok();
            Ok(UiProjectFile {
                id: descriptor.file_id,
                relative_path: descriptor.relative_path.clone(),
                kind: file_kind(Path::new(&descriptor.relative_path)),
                encoding: if descriptor.source_only {
                    UiSourceEncoding::NonUtf8
                } else {
                    UiSourceEncoding::Utf8
                },
                content,
                dirty: descriptor.dirty,
                byte_length: descriptor.byte_len,
                sha256: descriptor.content_hash,
            })
        })
        .collect()
}

fn ensure_bibliography_file(session: &ProjectSession, file_id: FileId) -> AppResult<()> {
    let file = session.file(file_id)?;
    if !file
        .relative_path()
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bib"))
    {
        return Err(AppError::InvalidPath {
            path: file.relative_path().to_string_lossy().into_owned(),
            message: "a local .bib file is required".into(),
        });
    }
    Ok(())
}

/// Freeze a complete, bounded rename scope. A key rename is rejected whenever
/// the include graph or bibliography inventory is stale/incomplete, any source
/// changed externally, or another bibliography can hide a key collision.
fn citation_rename_sources(
    session: &ProjectSession,
    selected_bibliography: FileId,
    old_key: &str,
    new_key: &str,
) -> Result<(String, Vec<CitationSourceFile>), CitationCommandError> {
    let snapshot = session.snapshot()?;
    if snapshot.include_graph_stale {
        return Err(AppError::InvalidProject {
            message: "citation keys cannot be renamed while the include graph is stale".into(),
        }
        .into());
    }
    if !snapshot.include_graph.issues.is_empty() {
        return Err(AppError::InvalidProject {
            message: format!(
                "citation keys cannot be renamed until {} include-graph issue(s) are resolved",
                snapshot.include_graph.issues.len()
            ),
        }
        .into());
    }

    let descriptors = session.files()?;
    let loaded_bibliographies = descriptors
        .iter()
        .filter(|file| extension_is(&file.relative_path, "bib"))
        .map(|file| (file.relative_path.replace('\\', "/"), file.file_id))
        .collect::<BTreeMap<_, _>>();
    let discovered_bibliographies = discover_project_bibliographies(session.root())?
        .into_iter()
        .map(|path| normalized_relative(&path))
        .collect::<BTreeSet<_>>();
    let loaded_bibliography_paths = loaded_bibliographies
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if loaded_bibliography_paths != discovered_bibliographies {
        return Err(AppError::InvalidProject {
            message: "the bibliography inventory changed on disk; reopen the project before renaming a citation key"
                .into(),
        }
        .into());
    }

    let graph_tex_paths = snapshot
        .include_graph
        .nodes
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let loaded_tex = descriptors
        .iter()
        .filter(|file| extension_is(&file.relative_path, "tex"))
        .map(|file| (file.relative_path.replace('\\', "/"), file.file_id))
        .collect::<BTreeMap<_, _>>();
    if !graph_tex_paths
        .iter()
        .all(|path| loaded_tex.contains_key(path))
    {
        return Err(AppError::InvalidProject {
            message: "the include graph references a TeX file that is not loaded".into(),
        }
        .into());
    }

    let source_count = graph_tex_paths.len() + loaded_bibliographies.len();
    if source_count > MAX_CITATION_RENAME_FILES {
        return Err(AppError::InvalidProject {
            message: format!(
                "citation rename scope exceeds the {MAX_CITATION_RENAME_FILES}-file safety limit"
            ),
        }
        .into());
    }

    let mut total_bytes = 0u64;
    let mut selected_source = None;
    let mut old_definitions = 0usize;
    let mut selected_old_definitions = 0usize;
    let mut new_definitions = 0usize;
    for (path, file_id) in &loaded_bibliographies {
        let source = session.file(*file_id)?;
        reserve_citation_rename_bytes(&mut total_bytes, path, source.bytes().len() as u64)?;
        ensure_citation_source_unchanged_on_disk(session.root(), path, source.persisted_hash())?;
        let text = source.text()?;
        let document = parse_bibliography(text)?;
        ensure_bibliography_can_be_renamed(&document)?;
        let old_in_file = document
            .entries
            .iter()
            .filter(|entry| entry.key == old_key)
            .count();
        old_definitions += old_in_file;
        new_definitions += document
            .entries
            .iter()
            .filter(|entry| entry.key == new_key)
            .count();
        if *file_id == selected_bibliography {
            selected_old_definitions = old_in_file;
            selected_source = Some(text.to_owned());
        }
    }

    match old_definitions {
        0 => {
            return Err(CitationError::KeyNotFound {
                key: old_key.to_owned(),
            }
            .into());
        }
        1 => {}
        _ => {
            return Err(CitationError::AmbiguousKey {
                key: old_key.to_owned(),
            }
            .into());
        }
    }
    if selected_old_definitions != 1 {
        return Err(CitationError::InvalidEdit {
            message: format!(
                "citation key `{old_key}` is not uniquely defined in the selected bibliography"
            ),
        }
        .into());
    }
    if new_definitions > 0 {
        return Err(CitationError::DuplicateKey {
            key: new_key.to_owned(),
        }
        .into());
    }

    let mut tex_files = Vec::with_capacity(graph_tex_paths.len());
    for path in graph_tex_paths {
        let file_id = loaded_tex[&path];
        let source = session.file(file_id)?;
        reserve_citation_rename_bytes(&mut total_bytes, &path, source.bytes().len() as u64)?;
        ensure_citation_source_unchanged_on_disk(session.root(), &path, source.persisted_hash())?;
        tex_files.push(CitationSourceFile {
            file_id,
            relative_path: path,
            source: source.text()?.to_owned(),
        });
    }

    let selected_source = selected_source.ok_or_else(|| AppError::UnknownFile {
        file_id: selected_bibliography.to_string(),
    })?;
    Ok((selected_source, tex_files))
}

fn extension_is(path: &str, expected: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn reserve_citation_rename_bytes(total: &mut u64, path: &str, size: u64) -> AppResult<()> {
    if size > MAX_CITATION_RENAME_FILE_BYTES {
        return Err(AppError::InvalidProject {
            message: format!(
                "{path} exceeds the {} MiB citation-rename file limit",
                MAX_CITATION_RENAME_FILE_BYTES / (1024 * 1024)
            ),
        });
    }
    *total = total
        .checked_add(size)
        .ok_or_else(|| AppError::InvalidProject {
            message: "citation rename byte count overflowed".into(),
        })?;
    if *total > MAX_CITATION_RENAME_TOTAL_BYTES {
        return Err(AppError::InvalidProject {
            message: format!(
                "citation rename scope exceeds the {} MiB aggregate safety limit",
                MAX_CITATION_RENAME_TOTAL_BYTES / (1024 * 1024)
            ),
        });
    }
    Ok(())
}

fn ensure_citation_source_unchanged_on_disk(
    root: &Path,
    relative_path: &str,
    expected_hash: &str,
) -> AppResult<()> {
    let path = root.join(relative_path);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| AppError::io("inspect citation source", &path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::InvalidProject {
            message: format!("citation source {} is not an ordinary file", path.display()),
        });
    }
    if metadata.len() > MAX_CITATION_RENAME_FILE_BYTES {
        return Err(AppError::InvalidProject {
            message: format!("{} exceeds the citation-rename file limit", path.display()),
        });
    }
    let file = std::fs::File::open(&path)
        .map_err(|error| AppError::io("read citation source", &path, error))?;
    let mut reader = file.take(MAX_CITATION_RENAME_FILE_BYTES.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut read_bytes = 0u64;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut chunk)
            .map_err(|error| AppError::io("read citation source", &path, error))?;
        if count == 0 {
            break;
        }
        read_bytes = read_bytes.saturating_add(count as u64);
        if read_bytes > MAX_CITATION_RENAME_FILE_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "{} grew beyond the citation-rename file limit while it was read",
                    path.display()
                ),
            });
        }
        hasher.update(&chunk[..count]);
    }
    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != expected_hash {
        return Err(AppError::ExternalConflict {
            path: relative_path.to_owned(),
        });
    }
    Ok(())
}

fn citation_edit_result(
    session: &mut ProjectSession,
    base_revision: Revision,
    edits: Vec<SourceEdit>,
) -> Result<UiEditResult, CitationCommandError> {
    let result = session.apply_source_edits(base_revision, edits)?;
    Ok(UiEditResult {
        revision: result.revision,
        files: project_files(session)?,
        diagnostics: result.diagnostics,
    })
}

fn file_kind(path: &Path) -> UiProjectFileKind {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "tex" => UiProjectFileKind::Tex,
        "bib" => UiProjectFileKind::Bib,
        "cls" | "sty" | "bst" => UiProjectFileKind::Style,
        "png" | "jpg" | "jpeg" | "pdf" | "eps" | "svg" => UiProjectFileKind::Asset,
        _ => UiProjectFileKind::Other,
    }
}

const fn sandbox_backend_name(backend: crate::core::compile::SandboxBackend) -> &'static str {
    match backend {
        crate::core::compile::SandboxBackend::WindowsAppContainer => "windowsAppContainer",
        crate::core::compile::SandboxBackend::MacosXpcAppSandbox => "macosXpcAppSandbox",
        crate::core::compile::SandboxBackend::LinuxBubblewrap => "linuxBubblewrap",
    }
}

fn canonical_scoped_directory(window: &Window, path: &Path) -> AppResult<PathBuf> {
    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize selected directory", path, error))?;
    if !canonical.is_dir() {
        return Err(AppError::InvalidPath {
            path: canonical.to_string_lossy().into_owned(),
            message: "a directory is required".into(),
        });
    }
    if !window.fs_scope().is_allowed(&canonical) {
        return Err(AppError::CapabilityDenied {
            capability: "selected-directory".into(),
            message: "select this directory through the native file dialog first".into(),
        });
    }
    Ok(canonical)
}

fn validate_folder_name(folder_name: &str) -> AppResult<()> {
    let path = Path::new(folder_name);
    let mut components = path.components();
    let only = components.next();
    if folder_name.trim().is_empty()
        || components.next().is_some()
        || !matches!(only, Some(Component::Normal(_)))
        || folder_name.contains(['/', '\\'])
    {
        return Err(AppError::InvalidPath {
            path: folder_name.into(),
            message: "project folder must be one non-empty directory name".into(),
        });
    }
    #[cfg(windows)]
    {
        let stem = folder_name
            .trim_end_matches(['.', ' '])
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let reserved = matches!(
            stem.as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        );
        if reserved || folder_name.ends_with(['.', ' ']) {
            return Err(AppError::InvalidPath {
                path: folder_name.into(),
                message: "project folder name is reserved on Windows".into(),
            });
        }
    }
    Ok(())
}

fn validate_relative_source_path(path: &str) -> AppResult<PathBuf> {
    let relative = Path::new(path);
    // IPC paths use portable `/` separators. Relying only on the host's
    // `Path::components` would treat Windows drive, rooted, and parent paths
    // as ordinary file names on Unix hosts.
    let has_non_portable_windows_syntax = path.contains(['\\', ':']);
    let has_portable_parent = path.split('/').any(|component| component == "..");
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || has_non_portable_windows_syntax
        || has_portable_parent
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::InvalidPath {
            path: path.into(),
            message: "main file must be a project-relative path".into(),
        });
    }
    Ok(relative.to_path_buf())
}

fn imported_project_key(root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    format!("imported:{}", hex::encode(hasher.finalize()))
}

/// Imported projects do not receive `paper-settings.json`, but their app-data
/// identity must survive reopen. The version-8 UUID is a domain-separated
/// digest of the already-canonical project root; it is never written into the
/// paper unless the user explicitly adopts Setwright project metadata.
fn stable_imported_project_id(root: &Path) -> ProjectId {
    let identity = root.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let identity = identity.to_ascii_lowercase();

    let mut hasher = Sha256::new();
    hasher.update(b"setwright-imported-project-v1\0");
    hasher.update(identity.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 9562 custom/version-8 UUID with the RFC variant.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ProjectId::from(uuid::Uuid::from_bytes(bytes))
}

fn imported_project_settings(root: &Path, main_file: String) -> PaperSettingsV1 {
    let mut settings = PaperSettingsV1::new(
        main_file,
        TemplateId::GenericArticle,
        DEFAULT_RUNTIME_PROFILE,
        LatexEngine::PdfLatex,
    );
    settings.project_id = stable_imported_project_id(root);
    settings
}

#[derive(Debug, Default)]
struct ProjectInventoryBudget {
    file_count: usize,
    total_bytes: u64,
}

impl ProjectInventoryBudget {
    fn add(&mut self, path: &Path, size: u64) -> AppResult<()> {
        self.validate_file_size(path, size)?;
        let file_count = self.file_count.saturating_add(1);
        if file_count > MAX_CAPTURED_PROJECT_FILES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "project inventory exceeds the {MAX_CAPTURED_PROJECT_FILES}-file safety limit"
                ),
            });
        }
        let total_bytes =
            self.total_bytes
                .checked_add(size)
                .ok_or_else(|| AppError::InvalidProject {
                    message: "project inventory byte count overflowed".into(),
                })?;
        if total_bytes > MAX_CAPTURED_TOTAL_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "project inventory exceeds the {} MiB aggregate safety limit",
                    MAX_CAPTURED_TOTAL_BYTES / (1024 * 1024)
                ),
            });
        }
        self.file_count = file_count;
        self.total_bytes = total_bytes;
        Ok(())
    }

    fn replace(&mut self, path: &Path, previous_size: u64, size: u64) -> AppResult<()> {
        self.validate_file_size(path, size)?;
        let total_bytes = self
            .total_bytes
            .checked_sub(previous_size)
            .and_then(|total| total.checked_add(size))
            .ok_or_else(|| AppError::InvalidProject {
                message: "project inventory byte count became inconsistent".into(),
            })?;
        if total_bytes > MAX_CAPTURED_TOTAL_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "project inventory exceeds the {} MiB aggregate safety limit",
                    MAX_CAPTURED_TOTAL_BYTES / (1024 * 1024)
                ),
            });
        }
        self.total_bytes = total_bytes;
        Ok(())
    }

    fn validate_file_size(&self, path: &Path, size: u64) -> AppResult<()> {
        if size > MAX_CAPTURED_FILE_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "{} exceeds the {} MiB per-file inventory safety limit",
                    path.display(),
                    MAX_CAPTURED_FILE_BYTES / (1024 * 1024)
                ),
            });
        }
        Ok(())
    }
}

fn read_inventory_file(path: &Path, operation: &str) -> AppResult<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|error| AppError::io(operation, path, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_CAPTURED_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(operation, path, error))?;
    if bytes.len() as u64 > MAX_CAPTURED_FILE_BYTES {
        return Err(AppError::InvalidProject {
            message: format!(
                "{} grew beyond the {} MiB per-file inventory safety limit while it was read",
                path.display(),
                MAX_CAPTURED_FILE_BYTES / (1024 * 1024)
            ),
        });
    }
    Ok(bytes)
}

/// Captures every ordinary project input needed by TeX into an in-memory map,
/// then overlays the Rust-owned source buffers. The scheduler can stage only
/// this map, so it never reads from or writes to the original project while a
/// compile runs.
fn capture_compile_files(
    session: &ProjectSession,
    main_file: &str,
) -> AppResult<BTreeMap<String, Vec<u8>>> {
    let root = session.root();
    let main_pdf = Path::new(main_file).with_extension("pdf");
    let mut files = BTreeMap::new();
    let mut budget = ProjectInventoryBudget::default();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .path()
                    .strip_prefix(root)
                    .is_ok_and(|relative| !excluded_compile_tree(relative))
        });

    for entry in walker {
        let entry = entry.map_err(|error| AppError::InvalidProject {
            message: format!("could not inventory compile inputs: {error}"),
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| AppError::PathOutsideRoot {
                path: entry.path().to_string_lossy().into_owned(),
            })?;
        if excluded_compile_tree(relative) {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::InvalidProject {
                message: format!(
                    "compile input {} is a symbolic link; compilation fails closed instead of following it",
                    relative.display()
                ),
            });
        }
        if entry.file_type().is_dir() {
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(AppError::InvalidProject {
                message: format!(
                    "compile input {} is not an ordinary file",
                    relative.display()
                ),
            });
        }
        if excluded_compile_file(relative, &main_pdf) {
            continue;
        }
        let key = relative
            .to_str()
            .ok_or_else(|| AppError::InvalidProject {
                message: format!(
                    "compile input path {} is not valid Unicode",
                    relative.display()
                ),
            })?
            .replace('\\', "/");
        let metadata = std::fs::metadata(entry.path())
            .map_err(|error| AppError::io("inspect compile input", entry.path(), error))?;
        budget.add(relative, metadata.len())?;
        let bytes = read_inventory_file(entry.path(), "read compile input")?;
        budget.replace(relative, metadata.len(), bytes.len() as u64)?;
        files.insert(key, bytes);
    }

    // Loaded TeX/Bib buffers are authoritative even when dirty and therefore
    // supersede their persisted filesystem copies byte-for-byte.
    for descriptor in session.files()? {
        let key = descriptor.relative_path.replace('\\', "/");
        let bytes = session.read_file(descriptor.file_id)?.bytes;
        if let Some(previous) = files.get(&key) {
            budget.replace(Path::new(&key), previous.len() as u64, bytes.len() as u64)?;
        } else {
            budget.add(Path::new(&key), bytes.len() as u64)?;
        }
        files.insert(key, bytes);
    }
    Ok(files)
}

fn excluded_compile_tree(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy();
        name.eq_ignore_ascii_case(".git")
            || name.eq_ignore_ascii_case(".hg")
            || name.eq_ignore_ascii_case(".svn")
            || name.eq_ignore_ascii_case("node_modules")
            || name.eq_ignore_ascii_case("target")
    })
}

fn excluded_compile_file(path: &Path, main_pdf: &Path) -> bool {
    let portable = path.to_string_lossy().replace('\\', "/");
    let lower = portable.to_ascii_lowercase();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let main_pdf = main_pdf
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if lower == main_pdf
        || file_name == "paper-settings.json"
        || file_name.ends_with(".setwright-review")
        || file_name.ends_with('~')
        || (file_name.starts_with('#') && file_name.ends_with('#'))
        || [".bak", ".backup", ".tmp", ".swp", ".swo"]
            .iter()
            .any(|suffix| file_name.ends_with(suffix))
        || file_name.ends_with(".synctex")
        || file_name.ends_with(".synctex.gz")
        || file_name.ends_with(".run.xml")
    {
        return true;
    }
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| {
            matches!(
                extension.as_str(),
                "aux"
                    | "bcf"
                    | "blg"
                    | "fdb_latexmk"
                    | "fls"
                    | "lof"
                    | "log"
                    | "lot"
                    | "nav"
                    | "out"
                    | "snm"
                    | "toc"
                    | "vrb"
            )
        })
}

fn capture_project_files(session: &ProjectSession) -> AppResult<BTreeMap<String, Vec<u8>>> {
    let root = session.root();
    let mut files = BTreeMap::new();
    let mut budget = ProjectInventoryBudget::default();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .path()
                    .strip_prefix(root)
                    .is_ok_and(|relative| !excluded_history_path(relative))
        });
    for entry in walker {
        let entry = entry.map_err(|error| AppError::InvalidProject {
            message: format!("could not inventory project history: {error}"),
        })?;
        if entry.file_type().is_symlink() || !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| AppError::PathOutsideRoot {
                path: entry.path().to_string_lossy().into_owned(),
            })?;
        if excluded_history_path(relative) {
            continue;
        }
        let key = relative.to_string_lossy().replace('\\', "/");
        let metadata = std::fs::metadata(entry.path())
            .map_err(|error| AppError::io("inspect history input", entry.path(), error))?;
        budget.add(relative, metadata.len())?;
        let bytes = read_inventory_file(entry.path(), "read history input")?;
        budget.replace(relative, metadata.len(), bytes.len() as u64)?;
        files.insert(key, bytes);
    }
    // Dirty authoritative buffers supersede the persisted filesystem view.
    for descriptor in session.files()? {
        let key = descriptor.relative_path;
        let bytes = session.read_file(descriptor.file_id)?.bytes;
        if let Some(previous) = files.get(&key) {
            budget.replace(Path::new(&key), previous.len() as u64, bytes.len() as u64)?;
        } else {
            budget.add(Path::new(&key), bytes.len() as u64)?;
        }
        files.insert(key, bytes);
    }
    Ok(files)
}

fn history_entry(record: SnapshotRecord) -> AppResult<UiHistoryEntry> {
    let kind = match record.kind {
        SnapshotKind::Automatic => UiHistoryKind::Automatic,
        SnapshotKind::Named => UiHistoryKind::Named,
        SnapshotKind::PreRestore => UiHistoryKind::PreRestore,
        SnapshotKind::PreAccept => UiHistoryKind::PreAccept,
    };
    let label = record.label.clone().unwrap_or_else(|| match kind {
        UiHistoryKind::Automatic => "Automatic snapshot".into(),
        UiHistoryKind::Named => "Named version".into(),
        UiHistoryKind::PreRestore => "Before restore".into(),
        UiHistoryKind::PreAccept => "Before accepting suggestion".into(),
    });
    Ok(UiHistoryEntry {
        id: record.snapshot_id,
        revision: record.source_revision.unwrap_or(Revision::INITIAL),
        created_at: record.created_at,
        label,
        kind,
        changed_files: record.file_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered_fixture(source: &str) -> (tempfile::TempDir, DesktopState, ProjectSessionId) {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("paper");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("main.tex"), source.as_bytes()).unwrap();
        let state = DesktopState::open(directory.path().join("app-data")).unwrap();
        let session = ProjectSession::open_path(&project).unwrap();
        let settings = PaperSettingsV1::new(
            "main.tex",
            TemplateId::GenericArticle,
            DEFAULT_RUNTIME_PROFILE,
            LatexEngine::PdfLatex,
        );
        let project_key = format!("project:{}", settings.project_id);
        let session_id = state
            .register(
                "owner-window",
                session,
                "Fixture paper".into(),
                settings,
                project_key,
            )
            .unwrap();
        (directory, state, session_id)
    }

    #[test]
    fn project_folder_is_one_safe_component() {
        assert!(validate_folder_name("distribution-shift").is_ok());
        assert!(validate_folder_name("").is_err());
        assert!(validate_folder_name("../paper").is_err());
        assert!(validate_folder_name("nested/paper").is_err());
        #[cfg(windows)]
        assert!(validate_folder_name("CON.txt").is_err());
    }

    #[test]
    fn main_file_must_stay_relative() {
        assert_eq!(
            validate_relative_source_path("sections/method.tex").unwrap(),
            PathBuf::from("sections/method.tex")
        );
        assert!(validate_relative_source_path("../outside.tex").is_err());
        assert!(validate_relative_source_path("C:\\outside.tex").is_err());
        assert!(validate_relative_source_path("C:/outside.tex").is_err());
        assert!(validate_relative_source_path("..\\outside.tex").is_err());
        assert!(validate_relative_source_path("\\outside.tex").is_err());
        assert!(validate_relative_source_path("main.tex:stream").is_err());
    }

    #[test]
    fn history_excludes_vcs_data_only_by_component() {
        assert!(excluded_history_path(Path::new(".git/objects/a")));
        assert!(excluded_history_path(Path::new(
            "node_modules/pkg/index.js"
        )));
        assert!(excluded_history_path(Path::new("nested/target/output.bin")));
        assert!(!excluded_history_path(Path::new("figure.git.png")));
    }

    #[test]
    fn project_inventory_budget_caps_files_individual_bytes_and_total_bytes() {
        let path = Path::new("asset.bin");
        let mut count = ProjectInventoryBudget::default();
        for _ in 0..MAX_CAPTURED_PROJECT_FILES {
            count.add(path, 0).unwrap();
        }
        assert!(count.add(path, 0).is_err());

        let mut individual = ProjectInventoryBudget::default();
        assert!(
            individual
                .add(path, MAX_CAPTURED_FILE_BYTES.saturating_add(1))
                .is_err()
        );

        let mut aggregate = ProjectInventoryBudget::default();
        aggregate.add(path, MAX_CAPTURED_FILE_BYTES).unwrap();
        aggregate.add(path, MAX_CAPTURED_FILE_BYTES).unwrap();
        assert!(aggregate.add(path, 1).is_err());
    }

    #[test]
    fn compile_inventory_rejects_oversized_sparse_asset_before_reading_it() {
        let source = "\\documentclass{article}\n\\begin{document}A\\end{document}\n";
        let (directory, state, session_id) = registered_fixture(source);
        let oversized = directory.path().join("paper/oversized.bin");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_CAPTURED_FILE_BYTES.saturating_add(1))
            .unwrap();
        assert!(matches!(
            canonical_compile_snapshot(
                &state,
                "owner-window",
                session_id,
                Revision::INITIAL,
                LatexEngine::PdfLatex,
                CompilePurpose::Preview,
            ),
            Err(AppError::InvalidProject { .. })
        ));
    }

    #[test]
    fn imported_project_identity_is_stable_per_canonical_root() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_root = first.path().canonicalize().unwrap();
        let second_root = second.path().canonicalize().unwrap();
        let first_id = stable_imported_project_id(&first_root);
        assert_eq!(first_id, stable_imported_project_id(&first_root));
        assert_ne!(first_id, stable_imported_project_id(&second_root));
        assert_eq!(first_id.0.get_version_num(), 8);
    }

    #[test]
    fn compile_ipc_uses_dirty_canonical_bytes_and_fails_closed_with_events() {
        let original = "\\documentclass{article}\n\\begin{document}\nDraft\n\\end{document}\n";
        let (directory, state, session_id) = registered_fixture(original);
        let revision = {
            let shared = state.registry.get(session_id).unwrap();
            let mut session = shared.lock();
            let main_file = session.main_file_id();
            let at_byte = session.read_file(main_file).unwrap().bytes.len();
            session
                .apply_document_op(
                    Revision::INITIAL,
                    DocumentOp::InsertText {
                        file_id: main_file,
                        at_byte,
                        text: "% canonical unsaved edit\n".into(),
                    },
                )
                .unwrap()
                .revision
        };
        state.compiler.lock().note_revision(session_id, revision);

        let (snapshot, _) = canonical_compile_snapshot(
            &state,
            "owner-window",
            session_id,
            revision,
            LatexEngine::PdfLatex,
            CompilePurpose::Preview,
        )
        .unwrap();
        assert!(
            snapshot.files["main.tex"]
                .windows(b"canonical unsaved edit".len())
                .any(|window| window == b"canonical unsaved edit")
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("paper/main.tex")).unwrap(),
            original
        );
        assert!(matches!(
            canonical_compile_snapshot(
                &state,
                "different-window",
                session_id,
                revision,
                LatexEngine::PdfLatex,
                CompilePurpose::Preview,
            ),
            Err(AppError::CapabilityDenied { .. })
        ));

        let outcome = run_compile_for_owner(
            &state,
            "owner-window",
            session_id,
            revision,
            LatexEngine::PdfLatex,
        )
        .unwrap();
        assert_eq!(outcome.ticket.state, "failed");
        assert!(matches!(
            outcome.error,
            Some(AppError::CompileUnavailable { .. })
        ));
        assert!(matches!(
            outcome.events.first(),
            Some(CompileEvent::Queued { .. })
        ));
        assert!(
            outcome
                .events
                .iter()
                .any(|event| matches!(event, CompileEvent::Started { .. }))
        );
        assert!(outcome.events.iter().any(|event| {
            matches!(
                event,
                CompileEvent::Diagnostic {
                    diagnostic: Diagnostic { code, .. },
                    ..
                } if code == "TEX_COMPILE_FAILED"
            )
        }));
        assert!(
            outcome
                .events
                .iter()
                .any(|event| { matches!(event, CompileEvent::Finished { success: false, .. }) })
        );
        assert!(
            state
                .compiler
                .lock()
                .last_successful_pdf(session_id)
                .is_none()
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("paper/main.tex")).unwrap(),
            original
        );
    }

    #[test]
    fn static_preflight_scans_filtered_isolated_inventory() {
        let source = "\\documentclass{article}\n\\usepackage{graphicx}\n\\begin{document}\n\\includegraphics{secret}\n\\end{document}\n";
        let (directory, state, session_id) = registered_fixture(source);
        let project = directory.path().join("paper");
        std::fs::write(project.join("secret.png"), b"not a real png").unwrap();
        std::fs::write(project.join("figure.pdf"), b"figure asset").unwrap();
        std::fs::write(project.join("main.pdf"), b"generated paper").unwrap();
        std::fs::write(project.join("main.log"), b"old log").unwrap();
        std::fs::write(project.join("paper-settings.json"), b"{}\n").unwrap();
        std::fs::write(project.join("notes.setwright-review"), b"{}\n").unwrap();
        std::fs::create_dir(project.join(".git")).unwrap();
        std::fs::write(project.join(".git/config"), b"private git data").unwrap();

        let (snapshot, _) = canonical_compile_snapshot(
            &state,
            "owner-window",
            session_id,
            Revision::INITIAL,
            LatexEngine::PdfLatex,
            CompilePurpose::ArxivPreflight,
        )
        .unwrap();
        assert!(snapshot.files.contains_key("secret.png"));
        assert!(snapshot.files.contains_key("figure.pdf"));
        for excluded in [
            "main.pdf",
            "main.log",
            "paper-settings.json",
            "notes.setwright-review",
            ".git/config",
        ] {
            assert!(
                !snapshot.files.contains_key(excluded),
                "staged excluded input {excluded}"
            );
        }

        let report =
            static_preflight_for_owner(&state, "owner-window", session_id, Revision::INITIAL)
                .unwrap();
        let codes = report
            .findings
            .iter()
            .map(|finding| finding.code.as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"RUNTIME_SANDBOX_UNATTESTED"));
        assert!(codes.contains(&"CLEAN_ARCHIVE_BUILD_NOT_RUN"));
        assert!(codes.contains(&"ARXIV_ORACLE_UNPINNED"));
        assert!(!report.ready);
        assert!(!report.clean_build.succeeded);
        assert!(!report.user_approval.approved);
        assert!(
            report
                .included_files
                .iter()
                .any(|file| file.path == "secret.png")
        );
    }

    #[test]
    fn compile_snapshot_rejects_unreported_external_source_changes() {
        let source = "\\documentclass{article}\n\\begin{document}A\\end{document}\n";
        let (directory, state, session_id) = registered_fixture(source);
        std::fs::write(
            directory.path().join("paper/main.tex"),
            b"\\documentclass{article}\n\\begin{document}external\\end{document}\n",
        )
        .unwrap();
        assert!(matches!(
            canonical_compile_snapshot(
                &state,
                "owner-window",
                session_id,
                Revision::INITIAL,
                LatexEngine::PdfLatex,
                CompilePurpose::Preview,
            ),
            Err(AppError::ExternalConflict { .. })
        ));
    }

    #[test]
    fn citation_rename_checks_key_collisions_in_every_bibliography() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("paper");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("main.tex"),
            b"\\documentclass{article}\n\\begin{document}\\cite{old}\\end{document}\n",
        )
        .unwrap();
        std::fs::write(project.join("selected.bib"), b"@misc{old, title={Old}}\n").unwrap();
        std::fs::write(project.join("other.bib"), b"@misc{new, title={New}}\n").unwrap();
        let session = ProjectSession::open_path(&project).unwrap();
        let selected = session
            .files()
            .unwrap()
            .into_iter()
            .find(|file| file.relative_path == "selected.bib")
            .unwrap()
            .file_id;

        assert!(matches!(
            citation_rename_sources(&session, selected, "old", "new"),
            Err(CitationCommandError::Citation {
                error: CitationError::DuplicateKey { .. }
            })
        ));
    }

    #[test]
    fn citation_rename_rejects_changed_inventory_and_external_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("paper");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("main.tex"),
            b"\\documentclass{article}\n\\begin{document}\\cite{old}\\end{document}\n",
        )
        .unwrap();
        std::fs::write(project.join("selected.bib"), b"@misc{old, title={Old}}\n").unwrap();
        let session = ProjectSession::open_path(&project).unwrap();
        let selected = session
            .files()
            .unwrap()
            .into_iter()
            .find(|file| file.relative_path == "selected.bib")
            .unwrap()
            .file_id;

        std::fs::write(project.join("late.bib"), b"@misc{late, title={Late}}\n").unwrap();
        assert!(matches!(
            citation_rename_sources(&session, selected, "old", "new"),
            Err(CitationCommandError::App {
                error: AppError::InvalidProject { .. }
            })
        ));
        std::fs::remove_file(project.join("late.bib")).unwrap();
        std::fs::write(
            project.join("selected.bib"),
            b"@misc{old, title={Externally changed}}\n",
        )
        .unwrap();
        assert!(matches!(
            citation_rename_sources(&session, selected, "old", "new"),
            Err(CitationCommandError::App {
                error: AppError::ExternalConflict { .. }
            })
        ));
    }

    #[test]
    fn citation_rename_rejects_problematic_include_graph() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("paper");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("main.tex"),
            b"\\documentclass{article}\n\\input{\\jobname}\n\\begin{document}\\cite{old}\\end{document}\n",
        )
        .unwrap();
        std::fs::write(project.join("selected.bib"), b"@misc{old, title={Old}}\n").unwrap();
        let session = ProjectSession::open_path(&project).unwrap();
        let selected = session
            .files()
            .unwrap()
            .into_iter()
            .find(|file| file.relative_path == "selected.bib")
            .unwrap()
            .file_id;

        assert!(matches!(
            citation_rename_sources(&session, selected, "old", "new"),
            Err(CitationCommandError::App {
                error: AppError::InvalidProject { .. }
            })
        ));
    }

    #[test]
    fn generated_surface_contains_semantic_review_runtime_and_compile_contracts() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("bindings.ts");
        command_builder()
            .export(specta_typescript::Typescript::default(), &output)
            .unwrap();
        let bindings = std::fs::read_to_string(output).unwrap();
        for contract in [
            "DocumentOp",
            "ProjectEvent",
            "CompileEvent",
            "ReviewBundleV1",
            "RuntimeManifestV1",
            "CompileEventEnvelope",
        ] {
            assert!(bindings.contains(contract), "missing {contract}");
        }
        assert!(bindings.contains("applyDocumentOp"));
        assert!(bindings.contains("prepareUtf8Conversion"));
        assert!(bindings.contains("convertFileToUtf8"));
        assert!(bindings.contains("readCompilePdf"));
        assert!(bindings.contains("setwright-compile-event"));
    }
}
