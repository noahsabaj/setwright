//! Revision-safe, fail-closed compile scheduling.
//!
//! The scheduler accepts canonical bytes, copies them into a broker-owned job
//! directory, and exposes only a validated fixed compile request to an
//! executor. This module deliberately contains no process-spawning code. The
//! production executor must bridge the request through the attested sandbox
//! service; [`NoCompileExecutor`] is the safe default until that exists.

use crate::core::compile::{CompileJob, CompilePurpose, CompileSpec};
use crate::core::contracts::{
    CompileJobId, Diagnostic, DiagnosticCategory, DiagnosticSeverity, ProjectSessionId, Revision,
};
use crate::core::error::{AppError, AppResult};
use crate::services::runtime::InstalledRuntime;
use crate::services::sandbox::SandboxLaunchRequest;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

const JOB_DIRECTORY: &str = "compile-jobs";
const OUTPUT_DIRECTORY: &str = "output";
const TECHNICAL_LOG: &str = "compile.log";

/// An exact in-memory snapshot of the canonical project buffers.
///
/// No source path is accepted here. That omission is intentional: staging can
/// read only the supplied bytes, so it cannot mutate or accidentally compile
/// the original project directory.
#[derive(Debug, Clone)]
pub struct CanonicalProjectSnapshot {
    pub session_id: ProjectSessionId,
    pub revision: Revision,
    pub spec: CompileSpec,
    pub files: BTreeMap<String, Vec<u8>>,
}

impl CanonicalProjectSnapshot {
    #[must_use]
    pub fn new(
        session_id: ProjectSessionId,
        revision: Revision,
        spec: CompileSpec,
        files: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            session_id,
            revision,
            spec,
            files,
        }
    }
}

/// A process-tree handle installed by an OS-specific executor.
///
/// Implementations must terminate the complete descendant tree (Job Object,
/// XPC service, or sandbox namespace), not just the immediate TeX process.
pub trait ProcessTreeControl: Send + Sync {
    fn terminate_tree(&self) -> AppResult<()>;
}

#[derive(Default)]
struct CancellationState {
    cancelled: AtomicBool,
    process_tree: Mutex<Option<Arc<dyn ProcessTreeControl>>>,
}

/// Cooperative cancellation plus a kill-the-complete-tree hook.
#[derive(Clone, Default)]
pub struct CompileCancellationToken {
    state: Arc<CancellationState>,
}

impl std::fmt::Debug for CompileCancellationToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompileCancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl CompileCancellationToken {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    /// Attaches the platform's complete-process-tree control. If cancellation
    /// won the race, the newly attached tree is terminated immediately.
    pub fn attach_process_tree(&self, tree: Arc<dyn ProcessTreeControl>) -> AppResult<()> {
        let mut process_tree = self.state.process_tree.lock();
        if self.is_cancelled() {
            drop(process_tree);
            tree.terminate_tree()?;
            return Ok(());
        }
        if process_tree.is_some() {
            return Err(AppError::CompileUnavailable {
                message: "compile process-tree control is already attached".into(),
            });
        }
        *process_tree = Some(tree);
        Ok(())
    }

    /// Marks the job cancelled and requests complete process-tree termination.
    pub fn cancel(&self) -> AppResult<bool> {
        let first = !self.state.cancelled.swap(true, Ordering::AcqRel);
        if first && let Some(tree) = self.state.process_tree.lock().take() {
            tree.terminate_tree()?;
        }
        Ok(first)
    }
}

/// A request whose fields cannot be forged outside this module.
///
/// It is created only after exact-profile validation and isolated staging. A
/// real executor can additionally obtain the existing sandbox service's
/// `SandboxLaunchRequest` after supplying a verified installed runtime.
#[derive(Debug, Clone)]
pub struct ValidatedCompileRequest {
    job: CompileJob,
    output_directory: PathBuf,
    technical_log_path: PathBuf,
}

impl ValidatedCompileRequest {
    #[must_use]
    pub fn job(&self) -> &CompileJob {
        &self.job
    }

    #[must_use]
    pub fn stage_directory(&self) -> &Path {
        &self.job.staged_project
    }

    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }

    #[must_use]
    pub fn technical_log_path(&self) -> &Path {
        &self.technical_log_path
    }

    /// Reuses the sandbox boundary's path, runtime-identity, and fixed-profile
    /// validation before any platform launcher is authorized.
    pub fn sandbox_launch_request(
        &self,
        runtime: &InstalledRuntime,
    ) -> AppResult<SandboxLaunchRequest> {
        SandboxLaunchRequest::new(self.job.clone(), runtime, self.output_directory.clone())
    }
}

/// Receives every byte drained from the executor while retaining only a
/// bounded display projection in memory.
pub trait CompileOutputSink {
    fn stdout(&mut self, bytes: &[u8]) -> AppResult<()>;
    fn stderr(&mut self, bytes: &[u8]) -> AppResult<()>;
}

#[derive(Debug, Clone, Default)]
pub struct ExecutorArtifacts {
    pub pdf: Option<Vec<u8>>,
    pub synctex: Option<Vec<u8>>,
    pub dependencies: Vec<String>,
    pub auxiliary_cache: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub struct ExecutorResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub artifacts: ExecutorArtifacts,
}

/// Implemented only by an attested AppContainer/XPC/bubblewrap adapter.
///
/// The trait receives no executable path or arbitrary arguments. It can see
/// only a fixed, validated request and must stream all captured output into the
/// supplied sink so the scheduler can keep a complete technical log.
pub trait CompileExecutor: Send + Sync {
    fn execute(
        &self,
        request: &ValidatedCompileRequest,
        cancellation: &CompileCancellationToken,
        output: &mut dyn CompileOutputSink,
    ) -> AppResult<ExecutorResult>;
}

/// Safe production default. It owns no process API and can never spawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoCompileExecutor;

impl CompileExecutor for NoCompileExecutor {
    fn execute(
        &self,
        _request: &ValidatedCompileRequest,
        _cancellation: &CompileCancellationToken,
        _output: &mut dyn CompileOutputSink,
    ) -> AppResult<ExecutorResult> {
        Err(AppError::CompileUnavailable {
            message: "no attested OS sandbox executor is installed".into(),
        })
    }
}

#[derive(Debug, Clone)]
struct PreparedCompile {
    request: ValidatedCompileRequest,
    cancellation: CompileCancellationToken,
}

#[derive(Debug, Clone)]
struct ActiveCompile {
    job_id: CompileJobId,
    revision: Revision,
    cancellation: CompileCancellationToken,
}

#[derive(Debug, Default)]
struct SessionCompileState {
    active: Option<ActiveCompile>,
    queued: Option<PreparedCompile>,
    last_success: Option<PublishedPdf>,
    latest_attempt: Option<PublishedCompile>,
    known_revision: Revision,
    remove_after_active: bool,
}

#[derive(Debug, Clone)]
pub enum QueueOutcome {
    Queued {
        job_id: CompileJobId,
        revision: Revision,
        superseded_queued_job_id: Option<CompileJobId>,
        cancelled_active_job_id: Option<CompileJobId>,
    },
    IgnoredOlder {
        requested_revision: Revision,
        newest_revision: Revision,
        retained_job_id: CompileJobId,
    },
}

#[derive(Debug, Clone)]
pub struct CancelOutcome {
    pub active_job_id: Option<CompileJobId>,
    pub queued_job_id: Option<CompileJobId>,
}

/// A begin/execute/complete lease. Splitting these phases lets another thread
/// queue a newer revision and cancel the active process tree while execution is
/// in progress.
#[derive(Debug, Clone)]
pub struct CompileLease {
    request: ValidatedCompileRequest,
    cancellation: CompileCancellationToken,
}

impl CompileLease {
    #[must_use]
    pub fn request(&self) -> &ValidatedCompileRequest {
        &self.request
    }

    #[must_use]
    pub fn cancellation(&self) -> &CompileCancellationToken {
        &self.cancellation
    }
}

#[derive(Debug)]
pub struct ExecutedCompile {
    result: AppResult<ExecutorResult>,
    display_log: String,
    display_log_truncated: bool,
    technical_log_path: PathBuf,
}

impl ExecutedCompile {
    #[must_use]
    pub fn display_log(&self) -> &str {
        &self.display_log
    }

    #[must_use]
    pub const fn display_log_truncated(&self) -> bool {
        self.display_log_truncated
    }

    #[must_use]
    pub fn technical_log_path(&self) -> &Path {
        &self.technical_log_path
    }
}

#[derive(Debug, Clone)]
pub struct PublishedPdf {
    pub job_id: CompileJobId,
    pub revision: Revision,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub stale: bool,
}

#[derive(Debug, Clone)]
pub struct PublishedCompile {
    pub job_id: CompileJobId,
    pub revision: Revision,
    pub success: bool,
    pub pdf: Option<PublishedPdf>,
    pub synctex: Option<Vec<u8>>,
    pub dependencies: Vec<String>,
    pub auxiliary_cache: BTreeMap<String, Vec<u8>>,
    pub display_log: String,
    pub display_log_truncated: bool,
    pub technical_log_path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
pub enum CompletionOutcome {
    Published(PublishedCompile),
    Failed {
        publication: PublishedCompile,
        error: AppError,
    },
    Cancelled {
        job_id: CompileJobId,
    },
    DiscardedStale {
        job_id: CompileJobId,
        job_revision: Revision,
        current_revision: Revision,
    },
}

/// Per-project compile scheduler with one active job and one coalesced pending
/// job per session.
#[derive(Debug)]
pub struct CompileScheduler<E = NoCompileExecutor> {
    staging_root: PathBuf,
    executor: E,
    sessions: HashMap<ProjectSessionId, SessionCompileState>,
}

impl CompileScheduler<NoCompileExecutor> {
    pub fn fail_closed(app_data_root: impl AsRef<Path>) -> AppResult<Self> {
        Self::new(app_data_root, NoCompileExecutor)
    }
}

impl<E> CompileScheduler<E>
where
    E: CompileExecutor,
{
    pub fn new(app_data_root: impl AsRef<Path>, executor: E) -> AppResult<Self> {
        let app_data_root = create_real_directory(app_data_root.as_ref(), "app data root")?;
        let staging_root =
            create_real_directory(&app_data_root.join(JOB_DIRECTORY), "compile job root")?;
        if !staging_root.starts_with(&app_data_root) {
            return Err(AppError::PathOutsideRoot {
                path: staging_root.to_string_lossy().into_owned(),
            });
        }
        Ok(Self {
            staging_root,
            executor,
            sessions: HashMap::new(),
        })
    }

    #[must_use]
    pub fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    /// Stages a snapshot and keeps only the newest queued revision. Queueing a
    /// newer revision cancels the active job's complete process tree.
    pub fn queue(&mut self, snapshot: CanonicalProjectSnapshot) -> AppResult<QueueOutcome> {
        let current = self.sessions.get(&snapshot.session_id);
        let newest = current.and_then(|state| {
            [
                state.active.as_ref().map(|job| (job.revision, job.job_id)),
                state
                    .queued
                    .as_ref()
                    .map(|job| (job.request.job.revision, job.request.job.job_id)),
            ]
            .into_iter()
            .flatten()
            .max_by_key(|(revision, _)| *revision)
        });

        if let Some((newest_revision, retained_job_id)) = newest
            && snapshot.revision < newest_revision
        {
            return Ok(QueueOutcome::IgnoredOlder {
                requested_revision: snapshot.revision,
                newest_revision,
                retained_job_id,
            });
        }

        let prepared = self.stage(snapshot)?;
        let job_id = prepared.request.job.job_id;
        let revision = prepared.request.job.revision;
        let staging_root = self.staging_root.clone();
        let state = self
            .sessions
            .entry(prepared.request.job.session_id)
            .or_default();
        state.known_revision = state.known_revision.max(revision);
        mark_pdf_stale_if_needed(state, revision);

        let superseded_queued_job_id = if let Some(previous) = state.queued.take() {
            let previous_job_id = previous.request.job.job_id;
            if let Err(error) = remove_job_stage(&staging_root, previous.request.stage_directory())
            {
                let _ = remove_job_stage(&staging_root, prepared.request.stage_directory());
                return Err(error);
            }
            Some(previous_job_id)
        } else {
            None
        };
        let cancelled_active_job_id = if let Some(active) = &state.active {
            active.cancellation.cancel()?;
            Some(active.job_id)
        } else {
            None
        };
        state.queued = Some(prepared);

        Ok(QueueOutcome::Queued {
            job_id,
            revision,
            superseded_queued_job_id,
            cancelled_active_job_id,
        })
    }

    /// Marks the current canonical revision even when auto-compile is disabled.
    /// Any older successful PDF remains available but visibly stale.
    pub fn note_revision(&mut self, session_id: ProjectSessionId, revision: Revision) {
        let state = self.sessions.entry(session_id).or_default();
        state.known_revision = state.known_revision.max(revision);
        mark_pdf_stale_if_needed(state, revision);
    }

    pub fn begin_next(&mut self, session_id: ProjectSessionId) -> AppResult<Option<CompileLease>> {
        let Some(state) = self.sessions.get_mut(&session_id) else {
            return Ok(None);
        };
        if state.active.is_some() {
            return Err(AppError::CompileUnavailable {
                message: "a compile is already active for this project".into(),
            });
        }
        let Some(prepared) = state.queued.take() else {
            return Ok(None);
        };
        state.active = Some(ActiveCompile {
            job_id: prepared.request.job.job_id,
            revision: prepared.request.job.revision,
            cancellation: prepared.cancellation.clone(),
        });
        Ok(Some(CompileLease {
            request: prepared.request,
            cancellation: prepared.cancellation,
        }))
    }

    /// Runs the configured executor while persisting every output byte. The
    /// scheduler does not publish anything until `complete` revision-checks it.
    #[must_use]
    pub fn execute(&self, lease: &CompileLease) -> ExecutedCompile {
        execute_compile(&self.executor, lease)
    }

    /// Publishes only if both the active job identity and canonical revision
    /// still match. Stale artifacts are discarded wholesale.
    pub fn complete(
        &mut self,
        lease: &CompileLease,
        executed: ExecutedCompile,
        current_revision: Revision,
    ) -> AppResult<CompletionOutcome> {
        let session_id = lease.request.job.session_id;
        let state = self
            .sessions
            .get_mut(&session_id)
            .ok_or(AppError::CompileUnavailable {
                message: "compile session no longer exists".into(),
            })?;
        state.known_revision = state.known_revision.max(current_revision);
        mark_pdf_stale_if_needed(state, current_revision);
        let Some(active) = state.active.take() else {
            return Err(AppError::CompileUnavailable {
                message: "compile lease is not active".into(),
            });
        };
        if active.job_id != lease.request.job.job_id {
            state.active = Some(active);
            return Err(AppError::CompileUnavailable {
                message: "compile lease does not own the active job".into(),
            });
        }

        if state.remove_after_active {
            remove_job_stage(&self.staging_root, lease.request.stage_directory())?;
            self.sessions.remove(&session_id);
            return Ok(CompletionOutcome::Cancelled {
                job_id: active.job_id,
            });
        }

        if active.revision != current_revision {
            remove_job_stage(&self.staging_root, lease.request.stage_directory())?;
            return Ok(CompletionOutcome::DiscardedStale {
                job_id: active.job_id,
                job_revision: active.revision,
                current_revision,
            });
        }
        if active.cancellation.is_cancelled() {
            remove_job_stage(&self.staging_root, lease.request.stage_directory())?;
            return Ok(CompletionOutcome::Cancelled {
                job_id: active.job_id,
            });
        }

        let ExecutedCompile {
            result,
            display_log,
            display_log_truncated,
            technical_log_path,
        } = executed;
        let technical_log = fs::read(&technical_log_path).unwrap_or_default();
        let mut diagnostics = translate_compile_diagnostics(&technical_log);
        match result {
            Ok(result) if result.success && result.artifacts.pdf.is_some() => {
                validate_auxiliary_cache(&result.artifacts.auxiliary_cache)?;
                let pdf_bytes = result.artifacts.pdf.expect("checked above");
                let pdf = PublishedPdf {
                    job_id: active.job_id,
                    revision: active.revision,
                    sha256: sha256_hex(&pdf_bytes),
                    bytes: pdf_bytes,
                    stale: false,
                };
                let publication = PublishedCompile {
                    job_id: active.job_id,
                    revision: active.revision,
                    success: true,
                    pdf: Some(pdf.clone()),
                    synctex: result.artifacts.synctex,
                    dependencies: result.artifacts.dependencies,
                    auxiliary_cache: result.artifacts.auxiliary_cache,
                    display_log,
                    display_log_truncated,
                    technical_log_path,
                    diagnostics,
                    exit_code: result.exit_code,
                };
                state.last_success = Some(pdf);
                replace_latest_attempt(&self.staging_root, state, publication.clone())?;
                Ok(CompletionOutcome::Published(publication))
            }
            Ok(result) => {
                if diagnostics.is_empty() {
                    diagnostics.push(generic_compile_diagnostic(
                        "TeX did not produce a usable PDF",
                        result.exit_code,
                    ));
                }
                if let Some(pdf) = &mut state.last_success {
                    pdf.stale = true;
                }
                let publication = failed_publication(
                    &active,
                    display_log,
                    display_log_truncated,
                    technical_log_path,
                    diagnostics,
                    result.exit_code,
                );
                replace_latest_attempt(&self.staging_root, state, publication.clone())?;
                Ok(CompletionOutcome::Failed {
                    publication,
                    error: AppError::CompileUnavailable {
                        message: "TeX failed or produced no PDF".into(),
                    },
                })
            }
            Err(error) => {
                if diagnostics.is_empty() {
                    diagnostics.push(generic_compile_diagnostic(&error.to_string(), None));
                }
                if let Some(pdf) = &mut state.last_success {
                    pdf.stale = true;
                }
                let publication = failed_publication(
                    &active,
                    display_log,
                    display_log_truncated,
                    technical_log_path,
                    diagnostics,
                    None,
                );
                replace_latest_attempt(&self.staging_root, state, publication.clone())?;
                Ok(CompletionOutcome::Failed { publication, error })
            }
        }
    }

    /// Cancels active work and drops queued work. The active entry remains
    /// until its executor returns, preventing a second simultaneous process.
    pub fn cancel(&mut self, session_id: ProjectSessionId) -> AppResult<CancelOutcome> {
        let Some(state) = self.sessions.get_mut(&session_id) else {
            return Ok(CancelOutcome {
                active_job_id: None,
                queued_job_id: None,
            });
        };
        let active_job_id = if let Some(active) = &state.active {
            active.cancellation.cancel()?;
            Some(active.job_id)
        } else {
            None
        };
        let queued_job_id = if let Some(queued) = state.queued.take() {
            let job_id = queued.request.job.job_id;
            remove_job_stage(&self.staging_root, queued.request.stage_directory())?;
            Some(job_id)
        } else {
            None
        };
        Ok(CancelOutcome {
            active_job_id,
            queued_job_id,
        })
    }

    /// Removes all retained state for a closed project. If an executor is
    /// still unwinding, its process tree is cancelled and the one active stage
    /// is removed atomically by `complete`; no queued or latest-attempt stage
    /// survives project/window closure.
    pub fn remove_session(&mut self, session_id: ProjectSessionId) -> AppResult<CancelOutcome> {
        let Some(mut state) = self.sessions.remove(&session_id) else {
            return Ok(CancelOutcome {
                active_job_id: None,
                queued_job_id: None,
            });
        };
        let active_job_id = state.active.as_ref().map(|active| active.job_id);
        let cancellation = if let Some(active) = &state.active {
            active.cancellation.cancel()
        } else {
            Ok(false)
        };
        let mut queued_job_id = None;
        let cleanup =
            (|| {
                if let Some(queued) = &state.queued {
                    remove_job_stage(&self.staging_root, queued.request.stage_directory())?;
                    queued_job_id = Some(queued.request.job.job_id);
                    state.queued = None;
                }
                if let Some(latest) = &state.latest_attempt {
                    let latest_stage = latest.technical_log_path.parent().ok_or_else(|| {
                        AppError::InvalidPath {
                            path: latest.technical_log_path.to_string_lossy().into_owned(),
                            message: "persisted compile log has no job directory".into(),
                        }
                    })?;
                    remove_job_stage(&self.staging_root, latest_stage)?;
                    state.latest_attempt = None;
                }
                Ok(())
            })();
        if let Err(error) = cleanup {
            self.sessions.insert(session_id, state);
            cancellation?;
            return Err(error);
        }
        if state.active.is_some() {
            state.remove_after_active = true;
            self.sessions.insert(session_id, state);
        }
        cancellation?;
        Ok(CancelOutcome {
            active_job_id,
            queued_job_id,
        })
    }

    /// Cancels exactly one caller-selected job without affecting a newer job
    /// for the same project. This is the safe adapter for an IPC command that
    /// presents both a window-owned session and a typed job identifier.
    pub fn cancel_job(
        &mut self,
        session_id: ProjectSessionId,
        job_id: CompileJobId,
    ) -> AppResult<CancelOutcome> {
        let Some(state) = self.sessions.get_mut(&session_id) else {
            return Err(AppError::CompileUnavailable {
                message: "there is no compile work for this project".into(),
            });
        };
        if let Some(active) = &state.active
            && active.job_id == job_id
        {
            active.cancellation.cancel()?;
            return Ok(CancelOutcome {
                active_job_id: Some(job_id),
                queued_job_id: None,
            });
        }
        if state
            .queued
            .as_ref()
            .is_some_and(|queued| queued.request.job.job_id == job_id)
        {
            let queued = state.queued.take().expect("checked above");
            remove_job_stage(&self.staging_root, queued.request.stage_directory())?;
            return Ok(CancelOutcome {
                active_job_id: None,
                queued_job_id: Some(job_id),
            });
        }
        Err(AppError::CompileUnavailable {
            message: "the requested compile job is no longer active or queued".into(),
        })
    }

    #[must_use]
    pub fn last_successful_pdf(&self, session_id: ProjectSessionId) -> Option<&PublishedPdf> {
        self.sessions
            .get(&session_id)
            .and_then(|state| state.last_success.as_ref())
    }

    #[must_use]
    pub fn latest_attempt(&self, session_id: ProjectSessionId) -> Option<&PublishedCompile> {
        self.sessions
            .get(&session_id)
            .and_then(|state| state.latest_attempt.as_ref())
    }

    fn stage(&self, snapshot: CanonicalProjectSnapshot) -> AppResult<PreparedCompile> {
        validate_exact_spec(&snapshot.spec)?;
        validate_snapshot_paths(&snapshot)?;

        let session_directory = create_child_directory(
            &self.staging_root,
            &snapshot.session_id.to_string(),
            "compile session directory",
        )?;
        let job_directory = create_child_directory(
            &session_directory,
            &Uuid::new_v4().to_string(),
            "compile stage directory",
        )?;
        let staged = (|| {
            let output_directory = create_child_directory(
                &job_directory,
                OUTPUT_DIRECTORY,
                "compile output directory",
            )?;

            for (relative_path, bytes) in &snapshot.files {
                write_staged_file(&job_directory, relative_path, bytes)?;
            }
            let job = CompileJob::new(
                snapshot.session_id,
                snapshot.revision,
                job_directory.clone(),
                snapshot.spec,
            )?;
            let request = ValidatedCompileRequest {
                job,
                output_directory,
                technical_log_path: job_directory.join(TECHNICAL_LOG),
            };
            Ok(PreparedCompile {
                request,
                cancellation: CompileCancellationToken::default(),
            })
        })();
        if staged.is_err() {
            let _ = remove_job_stage(&self.staging_root, &job_directory);
        }
        staged
    }
}

/// Executes a lease without borrowing the scheduler state. IPC workers use
/// this form so a long-running attested executor never holds the scheduler
/// mutex; cancellation and newer-revision coalescing remain addressable while
/// TeX runs.
#[must_use]
pub fn execute_compile<E>(executor: &E, lease: &CompileLease) -> ExecutedCompile
where
    E: CompileExecutor,
{
    let limit = lease.request.job.spec.limits.display_log_bytes;
    let mut collector =
        match PersistentOutputCollector::new(lease.request.technical_log_path.clone(), limit) {
            Ok(collector) => collector,
            Err(error) => {
                return ExecutedCompile {
                    result: Err(error),
                    display_log: String::new(),
                    display_log_truncated: false,
                    technical_log_path: lease.request.technical_log_path.clone(),
                };
            }
        };
    let result = executor.execute(&lease.request, &lease.cancellation, &mut collector);
    let log_result = collector.flush_and_sync();
    let (display_log, display_log_truncated, technical_log_path) = collector.finish();
    ExecutedCompile {
        result: match log_result {
            Ok(()) => result,
            Err(error) => Err(error),
        },
        display_log,
        display_log_truncated,
        technical_log_path,
    }
}

fn validate_exact_spec(spec: &CompileSpec) -> AppResult<()> {
    spec.validate()?;
    let mut expected = match spec.purpose {
        CompilePurpose::Preview | CompilePurpose::ReviewOverlay => CompileSpec::preview(
            spec.runtime_id.clone(),
            spec.engine,
            Path::new(&spec.main_file),
            spec.sandbox.backend,
        )?,
        CompilePurpose::ArxivPreflight => CompileSpec::preflight(
            spec.runtime_id.clone(),
            spec.engine,
            Path::new(&spec.main_file),
            spec.sandbox.backend,
        )?,
    };
    if spec.purpose == CompilePurpose::ReviewOverlay {
        expected.purpose = CompilePurpose::ReviewOverlay;
    }
    if spec == &expected {
        Ok(())
    } else {
        Err(AppError::CompileUnavailable {
            message: "compile request differs from Setwright's exact fixed command profile".into(),
        })
    }
}

fn validate_snapshot_paths(snapshot: &CanonicalProjectSnapshot) -> AppResult<()> {
    if snapshot.files.is_empty() {
        return Err(AppError::InvalidProject {
            message: "compile snapshot has no files".into(),
        });
    }
    let mut normalized = BTreeSet::new();
    for relative_path in snapshot.files.keys() {
        let path = validated_portable_relative_path(relative_path)?;
        let portable = portable_path(&path);
        let portable_lower = portable.to_ascii_lowercase();
        if portable_lower == TECHNICAL_LOG
            || portable_lower == OUTPUT_DIRECTORY
            || portable_lower.starts_with(&format!("{OUTPUT_DIRECTORY}/"))
        {
            return Err(AppError::InvalidProject {
                message: format!("compile snapshot path is reserved by the broker: {portable}"),
            });
        }
        if !normalized.insert(portable_lower) {
            return Err(AppError::InvalidProject {
                message: format!("compile snapshot contains a case-colliding path: {portable}"),
            });
        }
    }
    let main = portable_path(&validated_portable_relative_path(&snapshot.spec.main_file)?);
    if !snapshot.files.contains_key(&main) {
        return Err(AppError::FileNotFound { path: main });
    }
    Ok(())
}

fn validated_portable_relative_path(value: &str) -> AppResult<PathBuf> {
    let invalid_component = value
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..");
    if value.is_empty()
        || value.contains(['\0', '\\'])
        || value.starts_with('/')
        || invalid_component
        || (value.len() >= 2 && value.as_bytes()[1] == b':')
    {
        return Err(AppError::PathOutsideRoot { path: value.into() });
    }
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::PathOutsideRoot { path: value.into() });
    }
    Ok(path)
}

fn portable_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn create_real_directory(path: &Path, label: &str) -> AppResult<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AppError::InvalidPath {
                    path: path.to_string_lossy().into_owned(),
                    message: format!("{label} must be a real directory, not a symlink"),
                });
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .map_err(|error| AppError::io(format!("create {label}"), path, error))?;
            let metadata = fs::symlink_metadata(path)
                .map_err(|error| AppError::io(format!("inspect {label}"), path, error))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AppError::InvalidPath {
                    path: path.to_string_lossy().into_owned(),
                    message: format!("{label} must be a real directory, not a symlink"),
                });
            }
        }
        Err(error) => return Err(AppError::io(format!("inspect {label}"), path, error)),
    }
    fs::canonicalize(path)
        .map_err(|error| AppError::io(format!("canonicalize {label}"), path, error))
}

fn create_child_directory(parent: &Path, component: &str, label: &str) -> AppResult<PathBuf> {
    let relative = validated_portable_relative_path(component)?;
    if relative.components().count() != 1 {
        return Err(AppError::InvalidPath {
            path: component.into(),
            message: format!("{label} must use one path component"),
        });
    }
    let candidate = parent.join(relative);
    let directory = create_real_directory(&candidate, label)?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| AppError::io("canonicalize compile parent", parent, error))?;
    if !directory.starts_with(&canonical_parent) {
        return Err(AppError::PathOutsideRoot {
            path: directory.to_string_lossy().into_owned(),
        });
    }
    Ok(directory)
}

/// Removes exactly one broker-created `<session>/<job>` stage. Both paths are
/// canonicalized and the relative depth is checked before recursive removal,
/// preventing a corrupted request from widening cleanup beyond compile-jobs.
fn remove_job_stage(staging_root: &Path, stage_directory: &Path) -> AppResult<()> {
    let metadata = match fs::symlink_metadata(stage_directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(AppError::io(
                "inspect compile stage for cleanup",
                stage_directory,
                error,
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidPath {
            path: stage_directory.to_string_lossy().into_owned(),
            message: "compile cleanup target must be a real job directory".into(),
        });
    }
    let canonical_root = fs::canonicalize(staging_root)
        .map_err(|error| AppError::io("canonicalize compile cleanup root", staging_root, error))?;
    let canonical_stage = fs::canonicalize(stage_directory).map_err(|error| {
        AppError::io(
            "canonicalize compile stage for cleanup",
            stage_directory,
            error,
        )
    })?;
    let relative =
        canonical_stage
            .strip_prefix(&canonical_root)
            .map_err(|_| AppError::PathOutsideRoot {
                path: canonical_stage.to_string_lossy().into_owned(),
            })?;
    if relative.components().count() != 2 {
        return Err(AppError::InvalidPath {
            path: canonical_stage.to_string_lossy().into_owned(),
            message: "compile cleanup target is not one session/job directory".into(),
        });
    }
    fs::remove_dir_all(&canonical_stage)
        .map_err(|error| AppError::io("remove compile job stage", &canonical_stage, error))?;
    if let Some(session_directory) = canonical_stage.parent()
        && session_directory != canonical_root
    {
        match fs::remove_dir(session_directory) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::NotFound
                ) => {}
            Err(error) => {
                return Err(AppError::io(
                    "remove empty compile session directory",
                    session_directory,
                    error,
                ));
            }
        }
    }
    Ok(())
}

fn replace_latest_attempt(
    staging_root: &Path,
    state: &mut SessionCompileState,
    publication: PublishedCompile,
) -> AppResult<()> {
    if let Some(previous) = &state.latest_attempt {
        let previous_stage =
            previous
                .technical_log_path
                .parent()
                .ok_or_else(|| AppError::InvalidPath {
                    path: previous.technical_log_path.to_string_lossy().into_owned(),
                    message: "persisted compile log has no job directory".into(),
                })?;
        remove_job_stage(staging_root, previous_stage)?;
    }
    state.latest_attempt = Some(publication);
    Ok(())
}

fn write_staged_file(root: &Path, relative_path: &str, bytes: &[u8]) -> AppResult<()> {
    let relative = validated_portable_relative_path(relative_path)?;
    let mut current = root.to_path_buf();
    let components = relative.components().collect::<Vec<_>>();
    for component in &components[..components.len().saturating_sub(1)] {
        let Component::Normal(component) = component else {
            return Err(AppError::PathOutsideRoot {
                path: relative_path.into(),
            });
        };
        current = create_child_directory(
            &current,
            &component.to_string_lossy(),
            "compile source directory",
        )?;
    }
    let target = root.join(&relative);
    if target.exists() || fs::symlink_metadata(&target).is_ok() {
        return Err(AppError::InvalidPath {
            path: target.to_string_lossy().into_owned(),
            message: "compile stage target already exists".into(),
        });
    }
    let canonical_parent = fs::canonicalize(target.parent().unwrap_or(root))
        .map_err(|error| AppError::io("canonicalize compile source parent", &target, error))?;
    if !canonical_parent.starts_with(root) {
        return Err(AppError::PathOutsideRoot {
            path: target.to_string_lossy().into_owned(),
        });
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| AppError::io("create staged compile file", &target, error))?;
    file.write_all(bytes)
        .map_err(|error| AppError::io("write staged compile file", &target, error))?;
    file.sync_all()
        .map_err(|error| AppError::io("flush staged compile file", &target, error))?;
    Ok(())
}

fn validate_auxiliary_cache(cache: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    for path in cache.keys() {
        validated_portable_relative_path(path)?;
    }
    Ok(())
}

fn mark_pdf_stale_if_needed(state: &mut SessionCompileState, revision: Revision) {
    if let Some(pdf) = &mut state.last_success
        && pdf.revision != revision
    {
        pdf.stale = true;
    }
}

fn failed_publication(
    active: &ActiveCompile,
    display_log: String,
    display_log_truncated: bool,
    technical_log_path: PathBuf,
    diagnostics: Vec<Diagnostic>,
    exit_code: Option<i32>,
) -> PublishedCompile {
    PublishedCompile {
        job_id: active.job_id,
        revision: active.revision,
        success: false,
        pdf: None,
        synctex: None,
        dependencies: Vec::new(),
        auxiliary_cache: BTreeMap::new(),
        display_log,
        display_log_truncated,
        technical_log_path,
        diagnostics,
        exit_code,
    }
}

struct PersistentOutputCollector {
    file: File,
    path: PathBuf,
    display: Vec<u8>,
    display_limit: usize,
    truncated: bool,
}

impl PersistentOutputCollector {
    fn new(path: PathBuf, display_limit: usize) -> AppResult<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| AppError::io("create technical compile log", &path, error))?;
        Ok(Self {
            file,
            path,
            display: Vec::with_capacity(display_limit.min(64 * 1024)),
            display_limit,
            truncated: false,
        })
    }

    fn record(&mut self, stream: &[u8], bytes: &[u8]) -> AppResult<()> {
        self.file
            .write_all(stream)
            .and_then(|()| self.file.write_all(bytes))
            .map_err(|error| AppError::io("write technical compile log", &self.path, error))?;
        let remaining = self.display_limit.saturating_sub(self.display.len());
        let retained = remaining.min(bytes.len());
        self.display.extend_from_slice(&bytes[..retained]);
        if retained < bytes.len() {
            self.truncated = true;
        }
        Ok(())
    }

    fn flush_and_sync(&mut self) -> AppResult<()> {
        self.file
            .flush()
            .and_then(|()| self.file.sync_all())
            .map_err(|error| AppError::io("flush technical compile log", &self.path, error))
    }

    fn finish(self) -> (String, bool, PathBuf) {
        (
            String::from_utf8_lossy(&self.display).into_owned(),
            self.truncated,
            self.path,
        )
    }
}

impl CompileOutputSink for PersistentOutputCollector {
    fn stdout(&mut self, bytes: &[u8]) -> AppResult<()> {
        self.record(b"[stdout] ", bytes)
    }

    fn stderr(&mut self, bytes: &[u8]) -> AppResult<()> {
        self.record(b"[stderr] ", bytes)
    }
}

/// Converts common TeX failures to stable diagnostics. The returned entries
/// contain only excerpts; the complete byte log remains at
/// `PublishedCompile::technical_log_path`.
#[must_use]
pub fn translate_compile_diagnostics(log: &[u8]) -> Vec<Diagnostic> {
    let text = String::from_utf8_lossy(log);
    let mut diagnostics = Vec::new();

    if let Some(line) = text.lines().find(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("latex error: file") && lower.contains("not found")
    }) {
        let missing = extract_missing_file(line).unwrap_or("required TeX file");
        diagnostics.push(Diagnostic {
            code: "TEX_MISSING_FILE".into(),
            severity: DiagnosticSeverity::Error,
            category: DiagnosticCategory::MissingDependency,
            title: "A required file is missing".into(),
            message: format!("TeX could not find {missing}. Add it to the project or runtime."),
            span: None,
            source_line: extract_source_line(&text),
            actions: Vec::new(),
            technical_detail: Some(line.trim().into()),
        });
    }

    if let Some(line) = text.lines().find(|line| {
        line.to_ascii_lowercase()
            .contains("undefined control sequence")
    }) {
        diagnostics.push(Diagnostic {
            code: "TEX_UNDEFINED_CONTROL_SEQUENCE".into(),
            severity: DiagnosticSeverity::Error,
            category: DiagnosticCategory::Syntax,
            title: "TeX does not recognize a command".into(),
            message: "Check the command spelling or load the package that defines it.".into(),
            span: None,
            source_line: extract_source_line(&text),
            actions: Vec::new(),
            technical_detail: Some(line.trim().into()),
        });
    }

    if let Some(line) = text.lines().find(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("please (re)run biber")
            || lower.contains("i couldn't open database file")
            || lower.contains("empty bibliography")
            || (lower.contains("citation") && lower.contains("undefined"))
            || lower.contains("there were undefined references")
    }) {
        diagnostics.push(Diagnostic {
            code: "TEX_BIBLIOGRAPHY".into(),
            severity: DiagnosticSeverity::Error,
            category: DiagnosticCategory::Bibliography,
            title: "The bibliography is incomplete".into(),
            message: "Check bibliography files and citation keys, then compile again.".into(),
            span: None,
            source_line: extract_source_line(&text),
            actions: Vec::new(),
            technical_detail: Some(line.trim().into()),
        });
    }

    diagnostics
}

fn extract_missing_file(line: &str) -> Option<&str> {
    let start = line.find('`').or_else(|| line.find('\''))? + 1;
    let rest = &line[start..];
    let end = rest.find('\'').or_else(|| rest.find('`'))?;
    let value = rest[..end].trim();
    (!value.is_empty()).then_some(value)
}

fn extract_source_line(text: &str) -> Option<u32> {
    for line in text.lines() {
        if let Some(index) = line.find(":") {
            let remainder = &line[index + 1..];
            if let Some(end) = remainder.find(':')
                && let Ok(number) = remainder[..end].parse::<u32>()
            {
                return Some(number);
            }
        }
        if let Some(remainder) = line.trim_start().strip_prefix("l.") {
            let digits = remainder.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0
                && let Ok(number) = remainder[..digits].parse::<u32>()
            {
                return Some(number);
            }
        }
    }
    None
}

fn generic_compile_diagnostic(message: &str, exit_code: Option<i32>) -> Diagnostic {
    Diagnostic {
        code: "TEX_COMPILE_FAILED".into(),
        severity: DiagnosticSeverity::Error,
        category: DiagnosticCategory::Compile,
        title: "The paper did not compile".into(),
        message: "Open the technical log for the complete TeX output.".into(),
        span: None,
        source_line: None,
        actions: Vec::new(),
        technical_detail: Some(match exit_code {
            Some(code) => format!("{message} (exit code {code})"),
            None => message.into(),
        }),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::compile::SandboxBackend;
    use crate::core::contracts::LatexEngine;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Clone)]
    struct FakeExecutor {
        calls: Arc<AtomicUsize>,
        outcome: ExecutorResult,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    }

    impl CompileExecutor for FakeExecutor {
        fn execute(
            &self,
            _request: &ValidatedCompileRequest,
            _cancellation: &CompileCancellationToken,
            output: &mut dyn CompileOutputSink,
        ) -> AppResult<ExecutorResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            output.stdout(&self.stdout)?;
            output.stderr(&self.stderr)?;
            Ok(self.outcome.clone())
        }
    }

    #[derive(Debug)]
    struct CountingTree(Arc<AtomicUsize>);

    impl ProcessTreeControl for CountingTree {
        fn terminate_tree(&self) -> AppResult<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn spec() -> CompileSpec {
        CompileSpec::preview(
            "texlive-2025.2025-08-03",
            LatexEngine::PdfLatex,
            Path::new("main.tex"),
            SandboxBackend::WindowsAppContainer,
        )
        .unwrap()
    }

    fn snapshot(session: ProjectSessionId, revision: u64) -> CanonicalProjectSnapshot {
        CanonicalProjectSnapshot::new(
            session,
            Revision(revision),
            spec(),
            BTreeMap::from([
                ("main.tex".into(), b"\\input{sections/body}\n".to_vec()),
                (
                    "sections/body.tex".into(),
                    b"exact canonical bytes\r\n".to_vec(),
                ),
            ]),
        )
    }

    fn success_executor() -> FakeExecutor {
        FakeExecutor {
            calls: Arc::new(AtomicUsize::new(0)),
            outcome: ExecutorResult {
                success: true,
                exit_code: Some(0),
                artifacts: ExecutorArtifacts {
                    pdf: Some(b"%PDF-1.7\nfixture".to_vec()),
                    synctex: Some(b"SyncTeX fixture".to_vec()),
                    dependencies: vec!["main.tex".into(), "sections/body.tex".into()],
                    auxiliary_cache: BTreeMap::from([("main.aux".into(), b"aux".to_vec())]),
                },
            },
            stdout: b"compile output\n".to_vec(),
            stderr: Vec::new(),
        }
    }

    fn staged_job_count(staging_root: &Path, session: ProjectSessionId) -> usize {
        fs::read_dir(staging_root.join(session.to_string()))
            .map(|entries| entries.filter_map(Result::ok).count())
            .unwrap_or(0)
    }

    #[test]
    fn stages_exact_bytes_without_touching_originals_and_rejects_hostile_paths() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.tex");
        fs::write(&original, b"original remains unchanged").unwrap();
        let mut scheduler =
            CompileScheduler::new(directory.path().join("app-data"), success_executor()).unwrap();
        let session = ProjectSessionId::new();
        scheduler.queue(snapshot(session, 1)).unwrap();
        let lease = scheduler.begin_next(session).unwrap().unwrap();
        assert_eq!(
            fs::read(lease.request().stage_directory().join("sections/body.tex")).unwrap(),
            b"exact canonical bytes\r\n"
        );
        assert_eq!(fs::read(&original).unwrap(), b"original remains unchanged");

        for path in ["../escape.tex", "/absolute.tex", "C:/drive.tex", "a\\b.tex"] {
            let mut files = BTreeMap::new();
            files.insert(path.into(), Vec::new());
            let hostile = CanonicalProjectSnapshot::new(session, Revision(2), spec(), files);
            assert!(matches!(
                scheduler.queue(hostile),
                Err(AppError::PathOutsideRoot { .. })
            ));
        }
    }

    #[test]
    fn coalesces_to_newest_revision_and_cancels_complete_process_tree() {
        let directory = tempfile::tempdir().unwrap();
        let mut scheduler =
            CompileScheduler::new(directory.path().join("app-data"), success_executor()).unwrap();
        let session = ProjectSessionId::new();
        scheduler.queue(snapshot(session, 1)).unwrap();
        let first = scheduler.begin_next(session).unwrap().unwrap();
        let terminations = Arc::new(AtomicUsize::new(0));
        first
            .cancellation()
            .attach_process_tree(Arc::new(CountingTree(Arc::clone(&terminations))))
            .unwrap();

        let second = scheduler.queue(snapshot(session, 2)).unwrap();
        let (second_job_id, cancelled_active_job_id) = match second {
            QueueOutcome::Queued {
                job_id,
                cancelled_active_job_id,
                ..
            } => (job_id, cancelled_active_job_id),
            QueueOutcome::IgnoredOlder { .. } => panic!("newer revision was ignored"),
        };
        assert_eq!(cancelled_active_job_id, Some(first.request().job().job_id));
        let third = scheduler.queue(snapshot(session, 3)).unwrap();
        let (third_job_id, superseded_queued_job_id) = match third {
            QueueOutcome::Queued {
                job_id,
                superseded_queued_job_id,
                ..
            } => (job_id, superseded_queued_job_id),
            QueueOutcome::IgnoredOlder { .. } => panic!("newer revision was ignored"),
        };
        assert_eq!(superseded_queued_job_id, Some(second_job_id));
        assert_eq!(staged_job_count(scheduler.staging_root(), session), 2);
        assert!(matches!(
            scheduler.queue(snapshot(session, 2)).unwrap(),
            QueueOutcome::IgnoredOlder {
                newest_revision: Revision(3),
                retained_job_id,
                ..
            } if retained_job_id == third_job_id
        ));
        assert_eq!(terminations.load(Ordering::SeqCst), 1);
        assert!(first.cancellation().is_cancelled());

        let executed = scheduler.execute(&first);
        let completion = scheduler.complete(&first, executed, Revision(3)).unwrap();
        assert!(matches!(
            completion,
            CompletionOutcome::DiscardedStale { .. }
        ));
        assert_eq!(staged_job_count(scheduler.staging_root(), session), 1);
        let newest = scheduler.begin_next(session).unwrap().unwrap();
        assert_eq!(newest.request().job().revision, Revision(3));
        let executed = scheduler.execute(&newest);
        assert!(matches!(
            scheduler.complete(&newest, executed, Revision(3)).unwrap(),
            CompletionOutcome::Published(_)
        ));
        assert_eq!(staged_job_count(scheduler.staging_root(), session), 1);
    }

    #[test]
    fn publishes_only_current_revision_and_marks_last_pdf_stale() {
        let directory = tempfile::tempdir().unwrap();
        let mut scheduler =
            CompileScheduler::new(directory.path().join("app-data"), success_executor()).unwrap();
        let session = ProjectSessionId::new();
        scheduler.queue(snapshot(session, 1)).unwrap();
        let first = scheduler.begin_next(session).unwrap().unwrap();
        let executed = scheduler.execute(&first);
        assert!(matches!(
            scheduler.complete(&first, executed, Revision(1)).unwrap(),
            CompletionOutcome::Published(_)
        ));
        assert_eq!(staged_job_count(scheduler.staging_root(), session), 1);
        assert!(!scheduler.last_successful_pdf(session).unwrap().stale);

        scheduler.note_revision(session, Revision(2));
        assert!(scheduler.last_successful_pdf(session).unwrap().stale);

        scheduler.queue(snapshot(session, 2)).unwrap();
        let second = scheduler.begin_next(session).unwrap().unwrap();
        let executed = scheduler.execute(&second);
        let outcome = scheduler.complete(&second, executed, Revision(3)).unwrap();
        assert!(matches!(outcome, CompletionOutcome::DiscardedStale { .. }));
        assert_eq!(
            scheduler.last_successful_pdf(session).unwrap().revision,
            Revision(1)
        );
    }

    #[test]
    fn failed_current_compile_keeps_previous_pdf_as_stale() {
        let directory = tempfile::tempdir().unwrap();
        let mut scheduler =
            CompileScheduler::new(directory.path().join("app-data"), success_executor()).unwrap();
        let session = ProjectSessionId::new();
        scheduler.queue(snapshot(session, 1)).unwrap();
        let first = scheduler.begin_next(session).unwrap().unwrap();
        let executed = scheduler.execute(&first);
        scheduler.complete(&first, executed, Revision(1)).unwrap();

        scheduler.executor.outcome.success = false;
        scheduler.executor.outcome.artifacts.pdf = None;
        scheduler.executor.stderr = b"! Undefined control sequence.\nl.12 \\badcommand\n".to_vec();
        scheduler.queue(snapshot(session, 2)).unwrap();
        let second = scheduler.begin_next(session).unwrap().unwrap();
        let executed = scheduler.execute(&second);
        let outcome = scheduler.complete(&second, executed, Revision(2)).unwrap();
        assert!(matches!(outcome, CompletionOutcome::Failed { .. }));
        assert_eq!(staged_job_count(scheduler.staging_root(), session), 1);
        assert!(scheduler.last_successful_pdf(session).unwrap().stale);
        assert_eq!(
            scheduler.latest_attempt(session).unwrap().diagnostics[0].code,
            "TEX_UNDEFINED_CONTROL_SEQUENCE"
        );
    }

    #[test]
    fn bounded_display_log_does_not_stop_full_log_drain() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("full.log");
        let mut collector = PersistentOutputCollector::new(path.clone(), 8).unwrap();
        collector.stdout(b"123456").unwrap();
        collector.stderr(b"7890").unwrap();
        collector.flush_and_sync().unwrap();
        let (display, truncated, _) = collector.finish();
        assert_eq!(display.as_bytes(), b"12345678");
        assert!(truncated);
        let full = fs::read(path).unwrap();
        assert!(full.ends_with(b"7890"));
        assert!(full.len() > display.len());
    }

    #[test]
    fn translates_missing_file_undefined_command_and_bibliography_errors() {
        let log = br#"./main.tex:17: LaTeX Error: File `missing.sty' not found.
! Undefined control sequence.
l.17 \unknown
Package biblatex Warning: Please (re)run Biber on the file
"#;
        let diagnostics = translate_compile_diagnostics(log);
        assert_eq!(
            diagnostics
                .iter()
                .map(|item| item.code.as_str())
                .collect::<Vec<_>>(),
            vec![
                "TEX_MISSING_FILE",
                "TEX_UNDEFINED_CONTROL_SEQUENCE",
                "TEX_BIBLIOGRAPHY"
            ]
        );
        assert_eq!(diagnostics[0].source_line, Some(17));
        assert!(diagnostics[0].message.contains("missing.sty"));
    }

    #[test]
    fn default_executor_fails_closed_without_spawning() {
        let directory = tempfile::tempdir().unwrap();
        let mut scheduler =
            CompileScheduler::fail_closed(directory.path().join("app-data")).unwrap();
        let session = ProjectSessionId::new();
        scheduler.queue(snapshot(session, 1)).unwrap();
        let lease = scheduler.begin_next(session).unwrap().unwrap();
        let executed = scheduler.execute(&lease);
        let outcome = scheduler.complete(&lease, executed, Revision(1)).unwrap();
        assert!(matches!(
            outcome,
            CompletionOutcome::Failed {
                error: AppError::CompileUnavailable { .. },
                ..
            }
        ));
    }

    #[test]
    fn typed_job_cancellation_never_cancels_a_different_job() {
        let directory = tempfile::tempdir().unwrap();
        let mut scheduler =
            CompileScheduler::fail_closed(directory.path().join("app-data")).unwrap();
        let session = ProjectSessionId::new();
        let queued = scheduler.queue(snapshot(session, 1)).unwrap();
        let job_id = match queued {
            QueueOutcome::Queued { job_id, .. } => job_id,
            QueueOutcome::IgnoredOlder { .. } => panic!("first job was ignored"),
        };
        assert!(matches!(
            scheduler.cancel_job(session, CompileJobId::new()),
            Err(AppError::CompileUnavailable { .. })
        ));
        let cancelled = scheduler.cancel_job(session, job_id).unwrap();
        assert_eq!(cancelled.queued_job_id, Some(job_id));
        assert!(scheduler.begin_next(session).unwrap().is_none());
        assert_eq!(staged_job_count(scheduler.staging_root(), session), 0);
    }

    #[test]
    fn removing_session_cleans_latest_attempt_and_active_stage_after_unwind() {
        let directory = tempfile::tempdir().unwrap();
        let mut scheduler =
            CompileScheduler::new(directory.path().join("app-data"), success_executor()).unwrap();
        let completed_session = ProjectSessionId::new();
        scheduler.queue(snapshot(completed_session, 1)).unwrap();
        let completed = scheduler.begin_next(completed_session).unwrap().unwrap();
        let executed = scheduler.execute(&completed);
        scheduler
            .complete(&completed, executed, Revision(1))
            .unwrap();
        assert_eq!(
            staged_job_count(scheduler.staging_root(), completed_session),
            1
        );
        scheduler.remove_session(completed_session).unwrap();
        assert_eq!(
            staged_job_count(scheduler.staging_root(), completed_session),
            0
        );
        assert!(scheduler.latest_attempt(completed_session).is_none());

        let active_session = ProjectSessionId::new();
        scheduler.queue(snapshot(active_session, 1)).unwrap();
        let active = scheduler.begin_next(active_session).unwrap().unwrap();
        scheduler.queue(snapshot(active_session, 2)).unwrap();
        let removed = scheduler.remove_session(active_session).unwrap();
        assert_eq!(removed.active_job_id, Some(active.request().job().job_id));
        assert!(removed.queued_job_id.is_some());
        assert_eq!(
            staged_job_count(scheduler.staging_root(), active_session),
            1
        );
        let executed = scheduler.execute(&active);
        assert!(matches!(
            scheduler.complete(&active, executed, Revision(1)).unwrap(),
            CompletionOutcome::Cancelled { .. }
        ));
        assert_eq!(
            staged_job_count(scheduler.staging_root(), active_session),
            0
        );
        assert!(scheduler.begin_next(active_session).unwrap().is_none());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_symlinked_compile_root() {
        let directory = tempfile::tempdir().unwrap();
        let app_data = directory.path().join("app-data");
        let outside = directory.path().join("outside");
        fs::create_dir_all(&app_data).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let link = app_data.join(JOB_DIRECTORY);

        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();

        if linked {
            let result = CompileScheduler::new(&app_data, success_executor());
            assert!(matches!(result, Err(AppError::InvalidPath { .. })));
        }
    }
}
