//! Fail-closed compile sandbox boundary.
//!
//! This module contains no process-spawning fallback. The default broker always
//! returns `CompileUnavailable`. A platform launcher can be wrapped only after
//! typed hostile-project probe evidence has been validated for the current OS,
//! and every launch request is compared with the core's exact fixed compile
//! profile before the launcher is called.

use crate::core::compile::{CompileJob, CompilePurpose, CompileSpec, SandboxBackend};
use crate::core::contracts::CompileJobId;
use crate::core::error::{AppError, AppResult};
use crate::services::runtime::InstalledRuntime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PROBE_SCHEMA_VERSION: u32 = 1;
const SANDBOX_POLICY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommonSandboxProbeEvidence {
    pub schema_version: u32,
    pub policy_version: u32,
    pub profile_id: String,
    pub runtime_manifest_sha256: String,
    pub broker_build_sha256: String,
    pub probed_at: chrono::DateTime<chrono::Utc>,
    pub sandbox_started: bool,
    pub runtime_read_only: bool,
    pub staged_project_only: bool,
    pub empty_home_and_config: bool,
    pub outside_canary_denied: bool,
    pub dns_denied: bool,
    pub http_denied: bool,
    pub shell_escape_denied: bool,
    pub latexmkrc_ignored: bool,
    pub process_tree_killed: bool,
    pub memory_limit_enforced: bool,
    pub writable_limit_enforced: bool,
    pub child_limit_enforced: bool,
    pub pdflatex_passed: bool,
    pub xelatex_passed: bool,
    pub bibtex_passed: bool,
    pub biber_passed: bool,
    pub synctex_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowsAppContainerEvidence {
    pub common: CommonSandboxProbeEvidence,
    pub appcontainer_sid: String,
    pub restricted_token: bool,
    pub zero_network_capabilities: bool,
    pub runtime_acl_read_only: bool,
    pub stage_acl_scoped: bool,
    pub job_object_kill_on_close: bool,
    pub job_object_limits: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MacosXpcEvidence {
    pub common: CommonSandboxProbeEvidence,
    pub service_code_requirement: String,
    pub app_sandbox_enabled: bool,
    pub network_entitlement_absent: bool,
    pub app_group_stage_only: bool,
    pub xpc_peer_requirement_enforced: bool,
    pub process_limits_enforced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxBubblewrapEvidence {
    pub common: CommonSandboxProbeEvidence,
    pub bubblewrap_sha256: String,
    pub user_namespace: bool,
    pub mount_namespace: bool,
    pub pid_namespace: bool,
    pub ipc_namespace: bool,
    pub network_namespace: bool,
    pub capabilities_dropped: bool,
    pub no_new_privileges: bool,
    pub seccomp_filter: bool,
    pub rlimits_enforced: bool,
}

/// Backend-specific evidence emitted by the release-risk probe. Deserializing
/// evidence does not attest it; `SandboxAttestation::verify_for_current_host`
/// is the only constructor for an attestation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "backend", rename_all = "camelCase")]
pub enum SandboxProbeEvidence {
    WindowsAppContainer(WindowsAppContainerEvidence),
    MacosXpcAppSandbox(MacosXpcEvidence),
    LinuxBubblewrap(LinuxBubblewrapEvidence),
}

impl SandboxProbeEvidence {
    #[must_use]
    pub const fn backend(&self) -> SandboxBackend {
        match self {
            Self::WindowsAppContainer(_) => SandboxBackend::WindowsAppContainer,
            Self::MacosXpcAppSandbox(_) => SandboxBackend::MacosXpcAppSandbox,
            Self::LinuxBubblewrap(_) => SandboxBackend::LinuxBubblewrap,
        }
    }

    #[must_use]
    pub fn common(&self) -> &CommonSandboxProbeEvidence {
        match self {
            Self::WindowsAppContainer(evidence) => &evidence.common,
            Self::MacosXpcAppSandbox(evidence) => &evidence.common,
            Self::LinuxBubblewrap(evidence) => &evidence.common,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxAttestation {
    backend: SandboxBackend,
    attestation_id: String,
    profile_id: String,
    runtime_manifest_sha256: String,
    probed_at: chrono::DateTime<chrono::Utc>,
}

impl SandboxAttestation {
    pub fn verify_for_current_host(evidence: SandboxProbeEvidence) -> AppResult<Self> {
        let current = current_sandbox_backend()?;
        if evidence.backend() != current {
            return Err(compile_unavailable(format!(
                "sandbox probe targets {:?}, but this process requires {:?}",
                evidence.backend(),
                current
            )));
        }
        validate_probe_evidence(&evidence)?;
        let canonical = serde_json_canonicalizer::to_vec(&evidence).map_err(|error| {
            compile_unavailable(format!(
                "sandbox probe evidence cannot be canonicalized: {error}"
            ))
        })?;
        let common = evidence.common();
        Ok(Self {
            backend: evidence.backend(),
            attestation_id: hex::encode(Sha256::digest(&canonical)),
            profile_id: common.profile_id.clone(),
            runtime_manifest_sha256: common.runtime_manifest_sha256.clone(),
            probed_at: common.probed_at,
        })
    }

    #[must_use]
    pub const fn backend(&self) -> SandboxBackend {
        self.backend
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.attestation_id
    }

    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    #[must_use]
    pub fn runtime_manifest_sha256(&self) -> &str {
        &self.runtime_manifest_sha256
    }

    #[must_use]
    pub const fn probed_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.probed_at
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SandboxReadiness {
    Unavailable {
        backend: Option<SandboxBackend>,
        reason: String,
    },
    Attested {
        backend: SandboxBackend,
        attestation_id: String,
        profile_id: String,
        runtime_manifest_sha256: String,
        probed_at: chrono::DateTime<chrono::Utc>,
    },
}

impl From<&SandboxAttestation> for SandboxReadiness {
    fn from(attestation: &SandboxAttestation) -> Self {
        Self::Attested {
            backend: attestation.backend,
            attestation_id: attestation.attestation_id.clone(),
            profile_id: attestation.profile_id.clone(),
            runtime_manifest_sha256: attestation.runtime_manifest_sha256.clone(),
            probed_at: attestation.probed_at,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxLaunchRequest {
    job: CompileJob,
    runtime_root: PathBuf,
    runtime_manifest_sha256: String,
    output_directory: PathBuf,
}

impl SandboxLaunchRequest {
    pub fn new(
        job: CompileJob,
        runtime: &InstalledRuntime,
        output_directory: impl Into<PathBuf>,
    ) -> AppResult<Self> {
        let request = Self {
            job,
            runtime_root: runtime.root().to_path_buf(),
            runtime_manifest_sha256: runtime.manifest_sha256().into(),
            output_directory: output_directory.into(),
        };
        if request.job.spec.runtime_id != runtime.profile_id() {
            return Err(compile_unavailable(
                "compile runtime id does not match the verified installation",
            ));
        }
        validate_launch_request(&request)?;
        Ok(request)
    }

    #[must_use]
    pub fn job(&self) -> &CompileJob {
        &self.job
    }

    #[must_use]
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    #[must_use]
    pub fn runtime_manifest_sha256(&self) -> &str {
        &self.runtime_manifest_sha256
    }

    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }
}

/// An unforgeable launch token passed to the platform-specific launcher only
/// after the broker validates evidence, paths, profile identity, and the exact
/// fixed command model.
#[derive(Debug, Clone)]
pub struct SandboxLaunchAuthorization {
    request: SandboxLaunchRequest,
    backend: SandboxBackend,
    attestation_id: String,
    authority: LaunchAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchAuthority {
    ReleaseAttestation,
    ContainmentProbe,
}

impl SandboxLaunchAuthorization {
    #[must_use]
    pub fn request(&self) -> &SandboxLaunchRequest {
        &self.request
    }

    #[must_use]
    pub const fn backend(&self) -> SandboxBackend {
        self.backend
    }

    #[must_use]
    pub fn attestation_id(&self) -> &str {
        &self.attestation_id
    }

    /// Probe launches exercise the real native launcher but can never produce
    /// an attestation-bound receipt or implement the production broker trait.
    #[must_use]
    pub const fn is_containment_probe(&self) -> bool {
        matches!(self.authority, LaunchAuthority::ContainmentProbe)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformLaunchHandle {
    pub opaque_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SandboxLaunchReceipt {
    pub job_id: CompileJobId,
    pub backend: SandboxBackend,
    pub attestation_id: String,
    pub platform_handle: PlatformLaunchHandle,
}

/// Complete control of a sandboxed process tree while its initial process is
/// still suspended. Platform launchers must not resume the process themselves:
/// the compile executor first installs this control in the cancellation token.
pub trait SandboxProcessControl: Send + Sync {
    fn resume(&self) -> AppResult<()>;
    fn terminate_tree(&self) -> AppResult<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxExitStatus {
    pub success: bool,
    pub exit_code: Option<i32>,
}

/// One-shot blocking waiter owned by the common executor's wait thread.
pub trait SandboxExitWaiter: Send {
    fn wait(self: Box<Self>) -> AppResult<SandboxExitStatus>;
}

/// The platform-owned half of a controlled launch. Its handle is opaque to the
/// common executor; the broker turns it into an attestation-bound receipt.
pub struct PlatformControlledProcess {
    platform_handle: PlatformLaunchHandle,
    control: Arc<dyn SandboxProcessControl>,
    stdout: Box<dyn Read + Send>,
    stderr: Box<dyn Read + Send>,
    exit_waiter: Box<dyn SandboxExitWaiter>,
}

impl std::fmt::Debug for PlatformControlledProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformControlledProcess")
            .field("platform_handle", &self.platform_handle)
            .finish_non_exhaustive()
    }
}

impl PlatformControlledProcess {
    pub fn new(
        platform_handle: PlatformLaunchHandle,
        control: Arc<dyn SandboxProcessControl>,
        stdout: Box<dyn Read + Send>,
        stderr: Box<dyn Read + Send>,
        exit_waiter: Box<dyn SandboxExitWaiter>,
    ) -> AppResult<Self> {
        if platform_handle.opaque_id.trim().is_empty() {
            return Err(compile_unavailable(
                "platform sandbox launcher returned an empty job handle",
            ));
        }
        Ok(Self {
            platform_handle,
            control,
            stdout,
            stderr,
            exit_waiter,
        })
    }

    #[must_use]
    pub fn platform_handle(&self) -> &PlatformLaunchHandle {
        &self.platform_handle
    }

    #[must_use]
    pub fn control(&self) -> Arc<dyn SandboxProcessControl> {
        Arc::clone(&self.control)
    }

    pub fn resume(&self) -> AppResult<()> {
        self.control.resume()
    }

    pub fn into_io(
        self,
    ) -> (
        Box<dyn Read + Send>,
        Box<dyn Read + Send>,
        Box<dyn SandboxExitWaiter>,
    ) {
        (self.stdout, self.stderr, self.exit_waiter)
    }
}

/// An attestation-bound, initially suspended child. All process authority and
/// inherited I/O is explicit and owned; dropping an arbitrary `Child` handle
/// can never silently detach a compiler process.
pub struct ControlledSandboxChild {
    receipt: SandboxLaunchReceipt,
    control: Arc<dyn SandboxProcessControl>,
    stdout: Box<dyn Read + Send>,
    stderr: Box<dyn Read + Send>,
    exit_waiter: Box<dyn SandboxExitWaiter>,
}

impl std::fmt::Debug for ControlledSandboxChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlledSandboxChild")
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}

impl ControlledSandboxChild {
    #[must_use]
    pub fn receipt(&self) -> &SandboxLaunchReceipt {
        &self.receipt
    }

    #[must_use]
    pub fn control(&self) -> Arc<dyn SandboxProcessControl> {
        Arc::clone(&self.control)
    }

    pub fn resume(&self) -> AppResult<()> {
        self.control.resume()
    }

    pub fn into_io(
        self,
    ) -> (
        Box<dyn Read + Send>,
        Box<dyn Read + Send>,
        Box<dyn SandboxExitWaiter>,
    ) {
        (self.stdout, self.stderr, self.exit_waiter)
    }
}

/// Implemented only by an OS-specific AppContainer/XPC/bubblewrap service.
/// The frontend cannot provide executable paths or invoke this trait.
pub trait PlatformSandboxLauncher: Send + Sync {
    fn launch_attested(
        &self,
        authorization: &SandboxLaunchAuthorization,
    ) -> AppResult<PlatformControlledProcess>;
}

pub trait SandboxBroker: Send + Sync {
    fn readiness(&self) -> SandboxReadiness;
    fn launch(&self, request: &SandboxLaunchRequest) -> AppResult<ControlledSandboxChild>;
}

/// Bootstrap path for hostile-fixture containment probes. This validates the
/// same launch request and calls the same platform launcher as production, but
/// deliberately returns only the platform-owned child. It does not implement
/// [`SandboxBroker`], cannot mint a [`SandboxLaunchReceipt`], and therefore
/// cannot be installed in [`crate::services::compiler::SandboxedCompileExecutor`].
#[derive(Debug)]
pub struct SandboxProbeRunner<L> {
    launcher: L,
}

impl<L> SandboxProbeRunner<L>
where
    L: PlatformSandboxLauncher,
{
    #[must_use]
    pub const fn new(launcher: L) -> Self {
        Self { launcher }
    }

    pub fn launch_fixture(
        &self,
        request: &SandboxLaunchRequest,
    ) -> AppResult<PlatformControlledProcess> {
        validate_launch_request(request)?;
        let backend = current_sandbox_backend()?;
        if request.job.spec.sandbox.backend != backend {
            return Err(compile_unavailable(
                "containment probe request targets a different sandbox backend",
            ));
        }
        self.launcher.launch_attested(&SandboxLaunchAuthorization {
            request: request.clone(),
            backend,
            attestation_id: format!(
                "containment-probe-v{PROBE_SCHEMA_VERSION}:{}",
                request.runtime_manifest_sha256
            ),
            authority: LaunchAuthority::ContainmentProbe,
        })
    }
}

/// Production-safe default until a target's release-risk sandbox spike has
/// supplied evidence and a platform launcher. It has no process API at all.
#[derive(Debug, Clone)]
pub struct FailClosedSandboxBroker {
    backend: Option<SandboxBackend>,
    reason: String,
}

impl Default for FailClosedSandboxBroker {
    fn default() -> Self {
        match current_sandbox_backend() {
            Ok(backend) => Self {
                backend: Some(backend),
                reason: "sandbox broker has no accepted platform probe attestation".into(),
            },
            Err(error) => Self {
                backend: None,
                reason: error.to_string(),
            },
        }
    }
}

impl FailClosedSandboxBroker {
    #[must_use]
    pub fn with_reason(reason: impl Into<String>) -> Self {
        Self {
            backend: current_sandbox_backend().ok(),
            reason: reason.into(),
        }
    }
}

impl SandboxBroker for FailClosedSandboxBroker {
    fn readiness(&self) -> SandboxReadiness {
        SandboxReadiness::Unavailable {
            backend: self.backend,
            reason: self.reason.clone(),
        }
    }

    fn launch(&self, _request: &SandboxLaunchRequest) -> AppResult<ControlledSandboxChild> {
        Err(compile_unavailable(self.reason.clone()))
    }
}

/// Guard around a platform launcher. Construction validates evidence for the
/// current host; launch revalidates the complete fixed request every time.
#[derive(Debug)]
pub struct AttestedSandboxBroker<L> {
    attestation: SandboxAttestation,
    launcher: L,
}

impl<L> AttestedSandboxBroker<L>
where
    L: PlatformSandboxLauncher,
{
    pub fn new(evidence: SandboxProbeEvidence, launcher: L) -> AppResult<Self> {
        Ok(Self {
            attestation: SandboxAttestation::verify_for_current_host(evidence)?,
            launcher,
        })
    }

    #[must_use]
    pub fn attestation(&self) -> &SandboxAttestation {
        &self.attestation
    }
}

impl<L> SandboxBroker for AttestedSandboxBroker<L>
where
    L: PlatformSandboxLauncher,
{
    fn readiness(&self) -> SandboxReadiness {
        SandboxReadiness::from(&self.attestation)
    }

    fn launch(&self, request: &SandboxLaunchRequest) -> AppResult<ControlledSandboxChild> {
        validate_launch_request(request)?;
        if request.job.spec.sandbox.backend != self.attestation.backend
            || request.job.spec.runtime_id != self.attestation.profile_id
            || request.runtime_manifest_sha256 != self.attestation.runtime_manifest_sha256
        {
            return Err(compile_unavailable(
                "compile request is not covered by the accepted sandbox attestation",
            ));
        }
        let authorization = SandboxLaunchAuthorization {
            request: request.clone(),
            backend: self.attestation.backend,
            attestation_id: self.attestation.attestation_id.clone(),
            authority: LaunchAuthority::ReleaseAttestation,
        };
        let platform = self.launcher.launch_attested(&authorization)?;
        let receipt = SandboxLaunchReceipt {
            job_id: request.job.job_id,
            backend: self.attestation.backend,
            attestation_id: self.attestation.attestation_id.clone(),
            platform_handle: platform.platform_handle,
        };
        Ok(ControlledSandboxChild {
            receipt,
            control: platform.control,
            stdout: platform.stdout,
            stderr: platform.stderr,
            exit_waiter: platform.exit_waiter,
        })
    }
}

pub fn current_sandbox_backend() -> AppResult<SandboxBackend> {
    match std::env::consts::OS {
        "windows" => Ok(SandboxBackend::WindowsAppContainer),
        "macos" => Ok(SandboxBackend::MacosXpcAppSandbox),
        "linux" => Ok(SandboxBackend::LinuxBubblewrap),
        other => Err(compile_unavailable(format!(
            "Setwright has no sandbox backend for {other}"
        ))),
    }
}

fn validate_probe_evidence(evidence: &SandboxProbeEvidence) -> AppResult<()> {
    let common = evidence.common();
    if common.schema_version != PROBE_SCHEMA_VERSION
        || common.policy_version != SANDBOX_POLICY_VERSION
    {
        return Err(compile_unavailable(
            "sandbox probe schema or policy version is not supported",
        ));
    }
    validate_runtime_id(&common.profile_id)?;
    validate_sha256(&common.runtime_manifest_sha256, "runtime manifest")?;
    validate_sha256(&common.broker_build_sha256, "sandbox broker build")?;
    let common_controls = [
        common.sandbox_started,
        common.runtime_read_only,
        common.staged_project_only,
        common.empty_home_and_config,
        common.outside_canary_denied,
        common.dns_denied,
        common.http_denied,
        common.shell_escape_denied,
        common.latexmkrc_ignored,
        common.process_tree_killed,
        common.memory_limit_enforced,
        common.writable_limit_enforced,
        common.child_limit_enforced,
        common.pdflatex_passed,
        common.xelatex_passed,
        common.bibtex_passed,
        common.biber_passed,
        common.synctex_passed,
    ];
    if common_controls.contains(&false) {
        return Err(compile_unavailable(
            "sandbox probe did not pass every common isolation and TeX workflow check",
        ));
    }

    match evidence {
        SandboxProbeEvidence::WindowsAppContainer(windows) => {
            if windows.appcontainer_sid.trim().is_empty()
                || !windows.restricted_token
                || !windows.zero_network_capabilities
                || !windows.runtime_acl_read_only
                || !windows.stage_acl_scoped
                || !windows.job_object_kill_on_close
                || !windows.job_object_limits
            {
                return Err(compile_unavailable(
                    "AppContainer probe did not pass every token, ACL, network, and Job Object check",
                ));
            }
        }
        SandboxProbeEvidence::MacosXpcAppSandbox(macos) => {
            if macos.service_code_requirement.trim().is_empty()
                || !macos.app_sandbox_enabled
                || !macos.network_entitlement_absent
                || !macos.app_group_stage_only
                || !macos.xpc_peer_requirement_enforced
                || !macos.process_limits_enforced
            {
                return Err(compile_unavailable(
                    "XPC probe did not pass every code-signing, entitlement, app-group, and limit check",
                ));
            }
        }
        SandboxProbeEvidence::LinuxBubblewrap(linux) => {
            validate_sha256(&linux.bubblewrap_sha256, "bubblewrap binary")?;
            if !linux.user_namespace
                || !linux.mount_namespace
                || !linux.pid_namespace
                || !linux.ipc_namespace
                || !linux.network_namespace
                || !linux.capabilities_dropped
                || !linux.no_new_privileges
                || !linux.seccomp_filter
                || !linux.rlimits_enforced
            {
                return Err(compile_unavailable(
                    "bubblewrap probe did not pass every namespace, privilege, seccomp, and limit check",
                ));
            }
        }
    }
    Ok(())
}

fn validate_launch_request(request: &SandboxLaunchRequest) -> AppResult<()> {
    let spec = &request.job.spec;
    spec.validate()?;
    let expected = exact_compile_spec(spec)?;
    if spec != &expected {
        return Err(compile_unavailable(
            "compile request differs from Setwright's exact fixed command profile",
        ));
    }
    validate_sha256(&request.runtime_manifest_sha256, "runtime manifest")?;

    let stage = canonical_directory(&request.job.staged_project, "compile stage")?;
    let runtime = canonical_directory(&request.runtime_root, "runtime root")?;
    let output = canonical_directory(&request.output_directory, "compile output")?;
    if !output.starts_with(&stage) {
        return Err(AppError::PathOutsideRoot {
            path: output.to_string_lossy().into_owned(),
        });
    }
    if runtime.starts_with(&stage) || stage.starts_with(&runtime) {
        return Err(compile_unavailable(
            "read-only runtime and writable compile stage must be disjoint",
        ));
    }
    Ok(())
}

fn exact_compile_spec(spec: &CompileSpec) -> AppResult<CompileSpec> {
    let main = Path::new(&spec.main_file);
    let mut expected = match spec.purpose {
        CompilePurpose::Preview | CompilePurpose::ReviewOverlay => CompileSpec::preview(
            spec.runtime_id.clone(),
            spec.engine,
            main,
            spec.sandbox.backend,
        )?,
        CompilePurpose::ArxivPreflight => CompileSpec::preflight(
            spec.runtime_id.clone(),
            spec.engine,
            main,
            spec.sandbox.backend,
        )?,
    };
    if spec.purpose == CompilePurpose::ReviewOverlay {
        expected.purpose = CompilePurpose::ReviewOverlay;
    }
    Ok(expected)
}

fn canonical_directory(path: &Path, label: &str) -> AppResult<PathBuf> {
    if !path.is_absolute() {
        return Err(AppError::InvalidPath {
            path: path.to_string_lossy().into_owned(),
            message: format!("{label} must be an absolute broker-owned path"),
        });
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| AppError::io(format!("inspect {label}"), path, error))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(AppError::InvalidPath {
            path: path.to_string_lossy().into_owned(),
            message: format!("{label} must be a real directory, not a symlink"),
        });
    }
    fs::canonicalize(path)
        .map_err(|error| AppError::io(format!("canonicalize {label}"), path, error))
}

fn validate_runtime_id(value: &str) -> AppResult<()> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(compile_unavailable(
            "sandbox probe runtime id has an invalid identifier format",
        ))
    }
}

fn validate_sha256(value: &str, label: &str) -> AppResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(compile_unavailable(format!(
            "{label} SHA-256 must be 64 lowercase hexadecimal characters"
        )))
    }
}

fn compile_unavailable(message: impl Into<String>) -> AppError {
    AppError::CompileUnavailable {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::compile::SandboxPolicy;
    use crate::core::contracts::{LatexEngine, ProjectSessionId, Revision};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn common() -> CommonSandboxProbeEvidence {
        CommonSandboxProbeEvidence {
            schema_version: PROBE_SCHEMA_VERSION,
            policy_version: SANDBOX_POLICY_VERSION,
            profile_id: "texlive-2025.2025-08-03".into(),
            runtime_manifest_sha256: "a".repeat(64),
            broker_build_sha256: "b".repeat(64),
            probed_at: chrono::Utc::now(),
            sandbox_started: true,
            runtime_read_only: true,
            staged_project_only: true,
            empty_home_and_config: true,
            outside_canary_denied: true,
            dns_denied: true,
            http_denied: true,
            shell_escape_denied: true,
            latexmkrc_ignored: true,
            process_tree_killed: true,
            memory_limit_enforced: true,
            writable_limit_enforced: true,
            child_limit_enforced: true,
            pdflatex_passed: true,
            xelatex_passed: true,
            bibtex_passed: true,
            biber_passed: true,
            synctex_passed: true,
        }
    }

    fn current_evidence() -> SandboxProbeEvidence {
        match current_sandbox_backend().unwrap() {
            SandboxBackend::WindowsAppContainer => {
                SandboxProbeEvidence::WindowsAppContainer(WindowsAppContainerEvidence {
                    common: common(),
                    appcontainer_sid: "S-1-15-2-test".into(),
                    restricted_token: true,
                    zero_network_capabilities: true,
                    runtime_acl_read_only: true,
                    stage_acl_scoped: true,
                    job_object_kill_on_close: true,
                    job_object_limits: true,
                })
            }
            SandboxBackend::MacosXpcAppSandbox => {
                SandboxProbeEvidence::MacosXpcAppSandbox(MacosXpcEvidence {
                    common: common(),
                    service_code_requirement: "anchor apple generic".into(),
                    app_sandbox_enabled: true,
                    network_entitlement_absent: true,
                    app_group_stage_only: true,
                    xpc_peer_requirement_enforced: true,
                    process_limits_enforced: true,
                })
            }
            SandboxBackend::LinuxBubblewrap => {
                SandboxProbeEvidence::LinuxBubblewrap(LinuxBubblewrapEvidence {
                    common: common(),
                    bubblewrap_sha256: "c".repeat(64),
                    user_namespace: true,
                    mount_namespace: true,
                    pid_namespace: true,
                    ipc_namespace: true,
                    network_namespace: true,
                    capabilities_dropped: true,
                    no_new_privileges: true,
                    seccomp_filter: true,
                    rlimits_enforced: true,
                })
            }
        }
    }

    #[test]
    fn missing_control_keeps_probe_unattested() {
        let mut evidence = current_evidence();
        match &mut evidence {
            SandboxProbeEvidence::WindowsAppContainer(value) => {
                value.common.outside_canary_denied = false;
            }
            SandboxProbeEvidence::MacosXpcAppSandbox(value) => {
                value.common.outside_canary_denied = false;
            }
            SandboxProbeEvidence::LinuxBubblewrap(value) => {
                value.common.outside_canary_denied = false;
            }
        }
        assert!(matches!(
            SandboxAttestation::verify_for_current_host(evidence),
            Err(AppError::CompileUnavailable { .. })
        ));
    }

    #[test]
    fn default_broker_is_always_fail_closed() {
        let broker = FailClosedSandboxBroker::default();
        assert!(matches!(
            broker.readiness(),
            SandboxReadiness::Unavailable { .. }
        ));
        let (_root, request) = request_fixture();
        let result = broker.launch(&request);
        assert!(matches!(result, Err(AppError::CompileUnavailable { .. })));
    }

    #[test]
    fn containment_only_evidence_cannot_unlock_compilation() {
        let mut evidence = current_evidence();
        match &mut evidence {
            SandboxProbeEvidence::WindowsAppContainer(evidence) => {
                evidence.common.pdflatex_passed = false;
                evidence.common.xelatex_passed = false;
                evidence.common.bibtex_passed = false;
                evidence.common.biber_passed = false;
                evidence.common.synctex_passed = false;
            }
            SandboxProbeEvidence::MacosXpcAppSandbox(evidence) => {
                evidence.common.pdflatex_passed = false;
                evidence.common.xelatex_passed = false;
                evidence.common.bibtex_passed = false;
                evidence.common.biber_passed = false;
                evidence.common.synctex_passed = false;
            }
            SandboxProbeEvidence::LinuxBubblewrap(evidence) => {
                evidence.common.pdflatex_passed = false;
                evidence.common.xelatex_passed = false;
                evidence.common.bibtex_passed = false;
                evidence.common.biber_passed = false;
                evidence.common.synctex_passed = false;
            }
        }
        assert!(matches!(
            SandboxAttestation::verify_for_current_host(evidence),
            Err(AppError::CompileUnavailable { .. })
        ));
    }

    #[derive(Debug, Clone)]
    struct RecordingLauncher {
        calls: Arc<AtomicUsize>,
    }

    #[derive(Debug)]
    struct RecordingControl;

    impl SandboxProcessControl for RecordingControl {
        fn resume(&self) -> AppResult<()> {
            Ok(())
        }

        fn terminate_tree(&self) -> AppResult<()> {
            Ok(())
        }
    }

    struct CompletedWaiter;

    impl SandboxExitWaiter for CompletedWaiter {
        fn wait(self: Box<Self>) -> AppResult<SandboxExitStatus> {
            Ok(SandboxExitStatus {
                success: true,
                exit_code: Some(0),
            })
        }
    }

    impl PlatformSandboxLauncher for RecordingLauncher {
        fn launch_attested(
            &self,
            _authorization: &SandboxLaunchAuthorization,
        ) -> AppResult<PlatformControlledProcess> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            PlatformControlledProcess::new(
                PlatformLaunchHandle {
                    opaque_id: "test-handle".into(),
                },
                Arc::new(RecordingControl),
                Box::new(std::io::Cursor::new(Vec::<u8>::new())),
                Box::new(std::io::Cursor::new(Vec::<u8>::new())),
                Box::new(CompletedWaiter),
            )
        }
    }

    fn request_fixture() -> (tempfile::TempDir, SandboxLaunchRequest) {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let stage = root.path().join("stage");
        let output = stage.join("out");
        fs::create_dir(&runtime).unwrap();
        fs::create_dir(&stage).unwrap();
        fs::create_dir(&output).unwrap();
        let backend = current_sandbox_backend().unwrap();
        let spec = CompileSpec::preview(
            "texlive-2025.2025-08-03",
            LatexEngine::PdfLatex,
            Path::new("main.tex"),
            backend,
        )
        .unwrap();
        let job = CompileJob::new(ProjectSessionId::new(), Revision(1), stage, spec).unwrap();
        let request = SandboxLaunchRequest {
            job,
            runtime_root: runtime,
            runtime_manifest_sha256: "a".repeat(64),
            output_directory: output,
        };
        validate_launch_request(&request).unwrap();
        (root, request)
    }

    #[test]
    fn attested_broker_allows_only_the_exact_fixed_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let broker = AttestedSandboxBroker::new(
            current_evidence(),
            RecordingLauncher {
                calls: Arc::clone(&calls),
            },
        )
        .unwrap();
        let (_root, request) = request_fixture();
        let child = broker.launch(&request).unwrap();
        assert_eq!(child.receipt().job_id, request.job().job_id);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut weakened = request.clone();
        weakened.job.spec.sandbox = SandboxPolicy {
            network_allowed: true,
            ..weakened.job.spec.sandbox.clone()
        };
        assert!(matches!(
            broker.launch(&weakened),
            Err(AppError::CompileUnavailable { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn attestation_is_bound_to_runtime_manifest_hash() {
        let calls = Arc::new(AtomicUsize::new(0));
        let broker = AttestedSandboxBroker::new(
            current_evidence(),
            RecordingLauncher {
                calls: Arc::clone(&calls),
            },
        )
        .unwrap();
        let (_root, mut request) = request_fixture();
        request.runtime_manifest_sha256 = "d".repeat(64);
        assert!(matches!(
            broker.launch(&request),
            Err(AppError::CompileUnavailable { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
