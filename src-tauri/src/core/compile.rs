//! Compile orchestration models.
//!
//! This module never launches a process. A platform broker must turn a
//! validated `CompileJob` into an AppContainer/XPC/bubblewrap invocation and
//! must fail closed if it cannot apply the requested sandbox policy.

use crate::core::contracts::{CompileJobId, LatexEngine, ProjectSessionId, Revision};
use crate::core::error::{AppError, AppResult};
use crate::core::latex::{normalized_relative, safe_relative_path};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CompilePurpose {
    Preview,
    ReviewOverlay,
    ArxivPreflight,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SandboxBackend {
    WindowsAppContainer,
    MacosXpcAppSandbox,
    LinuxBubblewrap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPolicy {
    pub backend: SandboxBackend,
    pub fail_closed: bool,
    pub network_allowed: bool,
    pub runtime_read_only: bool,
    pub stage_writable: bool,
    pub outside_root_allowed: bool,
    pub follow_outside_symlinks: bool,
    pub empty_home: bool,
    pub drop_capabilities: bool,
    pub no_new_privileges: bool,
}

impl SandboxPolicy {
    #[must_use]
    pub const fn strict(backend: SandboxBackend) -> Self {
        Self {
            backend,
            fail_closed: true,
            network_allowed: false,
            runtime_read_only: true,
            stage_writable: true,
            outside_root_allowed: false,
            follow_outside_symlinks: false,
            empty_home: true,
            drop_capabilities: true,
            no_new_privileges: true,
        }
    }

    pub fn validate(&self) -> AppResult<()> {
        if !self.fail_closed
            || self.network_allowed
            || !self.runtime_read_only
            || !self.stage_writable
            || self.outside_root_allowed
            || self.follow_outside_symlinks
            || !self.empty_home
        {
            return Err(AppError::CompileUnavailable {
                message: "compile policy is weaker than Setwright's required sandbox".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompileLimits {
    pub timeout_seconds: u64,
    pub max_tex_passes: u8,
    pub memory_bytes: u64,
    pub writable_bytes: u64,
    pub max_child_processes: u16,
    pub display_log_bytes: usize,
}

impl CompileLimits {
    pub const PREVIEW: Self = Self {
        timeout_seconds: 60,
        max_tex_passes: 5,
        memory_bytes: 2 * 1024 * 1024 * 1024,
        writable_bytes: 1024 * 1024 * 1024,
        max_child_processes: 32,
        display_log_bytes: 2 * 1024 * 1024,
    };

    pub const PREFLIGHT: Self = Self {
        timeout_seconds: 10 * 60,
        ..Self::PREVIEW
    };

    #[must_use]
    pub const fn timeout(self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeTool {
    LatexMk,
    PdfLatex,
    XeLatex,
    BibTex,
    Biber,
    SyncTex,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandModel {
    /// Logical runtime-owned tool. No caller-supplied executable path is
    /// accepted at the IPC boundary.
    pub tool: RuntimeTool,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompileSpec {
    pub runtime_id: String,
    pub engine: LatexEngine,
    pub main_file: String,
    pub purpose: CompilePurpose,
    pub sandbox: SandboxPolicy,
    pub limits: CompileLimits,
    pub command: CommandModel,
}

impl CompileSpec {
    pub fn preview(
        runtime_id: impl Into<String>,
        engine: LatexEngine,
        main_file: &Path,
        backend: SandboxBackend,
    ) -> AppResult<Self> {
        Self::new(
            runtime_id,
            engine,
            main_file,
            CompilePurpose::Preview,
            backend,
        )
    }

    pub fn preflight(
        runtime_id: impl Into<String>,
        engine: LatexEngine,
        main_file: &Path,
        backend: SandboxBackend,
    ) -> AppResult<Self> {
        Self::new(
            runtime_id,
            engine,
            main_file,
            CompilePurpose::ArxivPreflight,
            backend,
        )
    }

    fn new(
        runtime_id: impl Into<String>,
        engine: LatexEngine,
        main_file: &Path,
        purpose: CompilePurpose,
        backend: SandboxBackend,
    ) -> AppResult<Self> {
        let runtime_id = runtime_id.into();
        validate_runtime_id(&runtime_id)?;
        let main_file = safe_relative_path(&main_file.to_string_lossy())?;
        if !main_file
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
        {
            return Err(AppError::InvalidPath {
                path: main_file.to_string_lossy().into_owned(),
                message: "compile main file must have a .tex extension".into(),
            });
        }
        let main_file = normalized_relative(&main_file);
        let limits = match purpose {
            CompilePurpose::ArxivPreflight => CompileLimits::PREFLIGHT,
            CompilePurpose::Preview | CompilePurpose::ReviewOverlay => CompileLimits::PREVIEW,
        };
        let sandbox = SandboxPolicy::strict(backend);
        let command = fixed_latexmk_command(engine, &main_file);
        let spec = Self {
            runtime_id,
            engine,
            main_file,
            purpose,
            sandbox,
            limits,
            command,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> AppResult<()> {
        validate_runtime_id(&self.runtime_id)?;
        safe_relative_path(&self.main_file)?;
        self.sandbox.validate()?;
        if self.command.tool != RuntimeTool::LatexMk
            || self.command.working_directory != "."
            || self
                .command
                .environment
                .get("openin_any")
                .map(String::as_str)
                != Some("p")
            || self
                .command
                .environment
                .get("openout_any")
                .map(String::as_str)
                != Some("p")
            || !self
                .command
                .arguments
                .iter()
                .any(|argument| argument == "-norc")
            || self.command.arguments.iter().any(|argument| {
                argument.contains("shell-escape") && !argument.contains("no-shell-escape")
            })
        {
            return Err(AppError::CompileUnavailable {
                message: "compile command violates the fixed safe profile".into(),
            });
        }
        Ok(())
    }
}

fn fixed_latexmk_command(engine: LatexEngine, main_file: &str) -> CommandModel {
    let engine_mode = match engine {
        LatexEngine::PdfLatex => "-pdf",
        LatexEngine::XeLatex => "-xelatex",
    };
    let engine_template = match engine {
        LatexEngine::PdfLatex => {
            "-pdflatex=pdflatex -no-shell-escape -interaction=nonstopmode -halt-on-error -file-line-error -synctex=1 -recorder %O %S"
        }
        LatexEngine::XeLatex => {
            "-xelatex=xelatex -no-shell-escape -interaction=nonstopmode -halt-on-error -file-line-error -synctex=1 -recorder %O %S"
        }
    };
    CommandModel {
        tool: RuntimeTool::LatexMk,
        arguments: vec![
            "-norc".into(),
            engine_mode.into(),
            engine_template.into(),
            "-interaction=nonstopmode".into(),
            "-halt-on-error".into(),
            "-file-line-error".into(),
            "-synctex=1".into(),
            "-recorder".into(),
            "-outdir=output".into(),
            format!("-max_repeat={}", CompileLimits::PREVIEW.max_tex_passes),
            main_file.into(),
        ],
        working_directory: ".".into(),
        environment: BTreeMap::from([
            ("HOME".into(), "/nonexistent".into()),
            ("TEXMFHOME".into(), "/nonexistent".into()),
            ("TEXMFCONFIG".into(), "/nonexistent".into()),
            ("TEXMFVAR".into(), "/tmp/texmf-var".into()),
            ("openin_any".into(), "p".into()),
            ("openout_any".into(), "p".into()),
        ]),
    }
}

fn validate_runtime_id(runtime_id: &str) -> AppResult<()> {
    let valid = !runtime_id.is_empty()
        && runtime_id.len() <= 128
        && runtime_id
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && runtime_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::CompileUnavailable {
            message: "runtime id does not match the signed-manifest identifier format".into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompileJob {
    pub job_id: CompileJobId,
    pub session_id: ProjectSessionId,
    pub revision: Revision,
    pub staged_project: PathBuf,
    pub spec: CompileSpec,
}

impl CompileJob {
    pub fn new(
        session_id: ProjectSessionId,
        revision: Revision,
        staged_project: PathBuf,
        spec: CompileSpec,
    ) -> AppResult<Self> {
        if !staged_project.is_absolute() {
            return Err(AppError::InvalidPath {
                path: staged_project.to_string_lossy().into_owned(),
                message: "compile stage must be an absolute broker-owned path".into(),
            });
        }
        spec.validate()?;
        Ok(Self {
            job_id: CompileJobId::new(),
            session_id,
            revision,
            staged_project,
            spec,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueueDecision {
    pub queued: CompileJob,
    pub superseded_job_id: Option<CompileJobId>,
}

/// Models one active compile per project and revision-gated publication.
#[derive(Debug, Default)]
pub struct CompileCoordinator {
    active: HashMap<ProjectSessionId, CompileJob>,
}

impl CompileCoordinator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn queue(&mut self, job: CompileJob) -> QueueDecision {
        let superseded_job_id = self
            .active
            .insert(job.session_id, job.clone())
            .map(|previous| previous.job_id);
        QueueDecision {
            queued: job,
            superseded_job_id,
        }
    }

    #[must_use]
    pub fn active(&self, session_id: ProjectSessionId) -> Option<&CompileJob> {
        self.active.get(&session_id)
    }

    pub fn cancel(&mut self, session_id: ProjectSessionId) -> Option<CompileJobId> {
        self.active.remove(&session_id).map(|job| job.job_id)
    }

    /// Returns true only when artifacts belong to the still-active job and the
    /// caller's current canonical revision. Stale jobs are discarded.
    pub fn accept_artifacts(
        &mut self,
        session_id: ProjectSessionId,
        job_id: CompileJobId,
        current_revision: Revision,
    ) -> bool {
        let accepted = self
            .active
            .get(&session_id)
            .is_some_and(|job| job.job_id == job_id && job.revision == current_revision);
        if self
            .active
            .get(&session_id)
            .is_some_and(|job| job.job_id == job_id)
        {
            self.active.remove(&session_id);
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> SandboxBackend {
        SandboxBackend::WindowsAppContainer
    }

    #[test]
    fn fixed_profile_disables_rc_shell_and_network() {
        let spec = CompileSpec::preview(
            "texlive-2025.2025-08-03",
            LatexEngine::PdfLatex,
            Path::new("main.tex"),
            backend(),
        )
        .unwrap();
        assert!(
            spec.command
                .arguments
                .iter()
                .any(|argument| argument == "-norc")
        );
        assert!(
            spec.command
                .arguments
                .iter()
                .any(|argument| argument.contains("-no-shell-escape"))
        );
        assert!(!spec.sandbox.network_allowed);
        assert!(spec.sandbox.fail_closed);
    }

    #[test]
    fn rejects_main_path_escape() {
        let result = CompileSpec::preview(
            "texlive-2025",
            LatexEngine::PdfLatex,
            Path::new("../main.tex"),
            backend(),
        );
        assert!(matches!(result, Err(AppError::PathOutsideRoot { .. })));
    }

    #[test]
    fn coordinator_rejects_stale_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let session_id = ProjectSessionId::new();
        let spec = CompileSpec::preview(
            "texlive-2025",
            LatexEngine::PdfLatex,
            Path::new("main.tex"),
            backend(),
        )
        .unwrap();
        let first = CompileJob::new(
            session_id,
            Revision(1),
            directory.path().to_path_buf(),
            spec.clone(),
        )
        .unwrap();
        let second = CompileJob::new(
            session_id,
            Revision(2),
            directory.path().to_path_buf(),
            spec,
        )
        .unwrap();
        let mut coordinator = CompileCoordinator::new();
        coordinator.queue(first.clone());
        let decision = coordinator.queue(second.clone());
        assert_eq!(decision.superseded_job_id, Some(first.job_id));
        assert!(!coordinator.accept_artifacts(session_id, first.job_id, Revision(2)));
        assert!(coordinator.accept_artifacts(session_id, second.job_id, Revision(2)));
    }
}
