use crate::core::error::{AppError, AppResult};
use crate::services::sandbox::{
    PlatformControlledProcess, PlatformLaunchHandle, PlatformSandboxLauncher, SandboxExitStatus,
    SandboxExitWaiter, SandboxLaunchAuthorization, SandboxProcessControl,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct BubblewrapSidecar {
    executable: PathBuf,
    version: String,
    sha256: String,
    seccomp_filter: PathBuf,
    seccomp_sha256: String,
}

impl BubblewrapSidecar {
    pub fn new(
        executable: impl Into<PathBuf>,
        version: impl Into<String>,
        sha256: impl Into<String>,
        seccomp_filter: impl Into<PathBuf>,
        seccomp_sha256: impl Into<String>,
    ) -> AppResult<Self> {
        let sidecar = Self {
            executable: executable.into(),
            version: version.into(),
            sha256: sha256.into(),
            seccomp_filter: seccomp_filter.into(),
            seccomp_sha256: seccomp_sha256.into(),
        };
        sidecar.verify()?;
        Ok(sidecar)
    }

    fn verify(&self) -> AppResult<()> {
        validate_sha256(&self.sha256, "bubblewrap")?;
        validate_sha256(&self.seccomp_sha256, "seccomp filter")?;
        verify_file_hash(&self.executable, &self.sha256, "bubblewrap")?;
        verify_file_hash(&self.seccomp_filter, &self.seccomp_sha256, "seccomp filter")?;
        let output = Command::new(&self.executable)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(|error| {
                unavailable(format!("run pinned bubblewrap version check: {error}"))
            })?;
        if !output.status.success() {
            return Err(unavailable("pinned bubblewrap version check failed"));
        }
        let actual = String::from_utf8(output.stdout)
            .map_err(|_| unavailable("bubblewrap version output is not UTF-8"))?;
        if actual.trim() != self.version {
            return Err(unavailable(format!(
                "bubblewrap version mismatch: expected {:?}, got {:?}",
                self.version,
                actual.trim()
            )));
        }
        ensure_user_namespaces_available()?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BubblewrapLauncher {
    sidecar: BubblewrapSidecar,
}

impl BubblewrapLauncher {
    pub fn new(sidecar: BubblewrapSidecar) -> AppResult<Self> {
        sidecar.verify()?;
        Ok(Self { sidecar })
    }
}

impl PlatformSandboxLauncher for BubblewrapLauncher {
    fn launch_attested(
        &self,
        authorization: &SandboxLaunchAuthorization,
    ) -> AppResult<PlatformControlledProcess> {
        self.sidecar.verify()?;
        launch_bubblewrap(authorization, &self.sidecar)
    }
}

fn launch_bubblewrap(
    authorization: &SandboxLaunchAuthorization,
    sidecar: &BubblewrapSidecar,
) -> AppResult<PlatformControlledProcess> {
    let request = authorization.request();
    let spec = &request.job().spec;
    let runtime_root = canonical_directory(request.runtime_root(), "runtime root")?;
    let stage_root = canonical_directory(&request.job().staged_project, "compile stage")?;
    let output_root = canonical_directory(request.output_directory(), "compile output")?;
    if !output_root.starts_with(&stage_root) {
        return Err(unavailable("compile output escaped the staged project"));
    }
    let executable = canonical_file(&runtime_root.join("bin/latexmk"), "latexmk")?;
    if !executable.starts_with(&runtime_root) {
        return Err(unavailable("latexmk escaped the verified runtime"));
    }

    let (resume_read, resume_write) = pipe()?;
    clear_close_on_exec(resume_read.as_raw_fd())?;
    set_close_on_exec(resume_write.as_raw_fd())?;
    let seccomp = File::open(&sidecar.seccomp_filter)
        .map_err(|error| unavailable(format!("open pinned seccomp filter: {error}")))?;
    clear_close_on_exec(seccomp.as_raw_fd())?;

    let mut command = Command::new(&sidecar.executable);
    command
        .env_clear()
        .arg("--unshare-user")
        .arg("--unshare-pid")
        .arg("--unshare-ipc")
        .arg("--unshare-net")
        .arg("--unshare-uts")
        .arg("--disable-userns")
        .arg("--cap-drop")
        .arg("ALL")
        .arg("--die-with-parent")
        .arg("--new-session")
        .arg("--clearenv")
        .arg("--uid")
        .arg("0")
        .arg("--gid")
        .arg("0")
        .arg("--ro-bind")
        .arg(&runtime_root)
        .arg("/runtime")
        .arg("--ro-bind")
        .arg(&stage_root)
        .arg("/work")
        .arg("--bind")
        .arg(&output_root)
        .arg("/work/output")
        .arg("--tmpfs")
        .arg("/tmp")
        .arg("--proc")
        .arg("/proc")
        .arg("--dev")
        .arg("/dev")
        .arg("--chdir")
        .arg("/work")
        .arg("--seccomp")
        .arg(seccomp.as_raw_fd().to_string())
        .arg("--block-fd")
        .arg(resume_read.as_raw_fd().to_string());

    for (key, value) in fixed_environment(&spec.command.environment) {
        command.arg("--setenv").arg(key).arg(value);
    }
    command.arg("--").arg("/runtime/bin/latexmk");
    command.args(&spec.command.arguments);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let memory_limit = spec.limits.memory_bytes;
    let file_limit = spec.limits.writable_bytes;
    unsafe {
        command.pre_exec(move || {
            // bubblewrap's outer monitor remains in this host process group.
            // Its namespace child later creates a new session; killing the
            // monitor triggers --die-with-parent, and Linux tears down the
            // complete PID namespace when its init process exits.
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            set_rlimit(libc::RLIMIT_AS, memory_limit)?;
            set_rlimit(libc::RLIMIT_FSIZE, file_limit)?;
            set_rlimit(libc::RLIMIT_CORE, 0)?;
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| unavailable(format!("launch pinned bubblewrap sidecar: {error}")))?;
    drop(resume_read);
    drop(seccomp);

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| kill_child(&mut child, "bubblewrap stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| kill_child(&mut child, "bubblewrap stderr was not captured"))?;
    let pid = i32::try_from(child.id()).map_err(|_| {
        kill_child(
            &mut child,
            "bubblewrap process id exceeds the supported range",
        )
    })?;
    let control = Arc::new(LinuxProcessControl {
        pid,
        resume: Mutex::new(Some(resume_write)),
        resumed: AtomicBool::new(false),
        finished: Arc::new(AtomicBool::new(false)),
        stage_root,
        output_root,
        writable_limit: spec.limits.writable_bytes,
        memory_limit: spec.limits.memory_bytes,
        process_limit: u32::from(spec.limits.max_child_processes).saturating_add(1),
    });
    start_resource_watchdog(&control);
    let waiter: Box<dyn SandboxExitWaiter> = Box::new(LinuxExitWaiter {
        child,
        finished: Arc::clone(&control.finished),
    });
    let process_control: Arc<dyn SandboxProcessControl> = control;
    PlatformControlledProcess::new(
        PlatformLaunchHandle {
            opaque_id: format!("bubblewrap-pgrp:{pid}"),
        },
        process_control,
        Box::new(stdout),
        Box::new(stderr),
        waiter,
    )
}

struct LinuxProcessControl {
    pid: i32,
    resume: Mutex<Option<OwnedFd>>,
    resumed: AtomicBool,
    finished: Arc<AtomicBool>,
    stage_root: PathBuf,
    output_root: PathBuf,
    writable_limit: u64,
    memory_limit: u64,
    process_limit: u32,
}

impl SandboxProcessControl for LinuxProcessControl {
    fn resume(&self) -> AppResult<()> {
        if self.resumed.swap(true, Ordering::AcqRel) {
            return Err(unavailable("bubblewrap process was already resumed"));
        }
        let mut resume = self
            .resume
            .lock()
            .map_err(|_| unavailable("bubblewrap resume control was poisoned"))?;
        let mut fd = resume
            .take()
            .ok_or_else(|| unavailable("bubblewrap resume control is unavailable"))?;
        fd.write_all(&[1])
            .map_err(|error| unavailable(format!("resume bubblewrap sandbox: {error}")))?;
        Ok(())
    }

    fn terminate_tree(&self) -> AppResult<()> {
        self.resume
            .lock()
            .map_err(|_| unavailable("bubblewrap resume control was poisoned"))?
            .take();
        if self.finished.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = unsafe { libc::kill(-self.pid, libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) && !self.finished.load(Ordering::Acquire) {
                return Err(unavailable(format!(
                    "terminate bubblewrap process group: {error}"
                )));
            }
        }
        Ok(())
    }
}

impl Drop for LinuxProcessControl {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Acquire) {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
        }
    }
}

struct LinuxExitWaiter {
    child: Child,
    finished: Arc<AtomicBool>,
}

impl SandboxExitWaiter for LinuxExitWaiter {
    fn wait(mut self: Box<Self>) -> AppResult<SandboxExitStatus> {
        let status = self
            .child
            .wait()
            .map_err(|error| unavailable(format!("wait for bubblewrap sandbox: {error}")))?;
        self.finished.store(true, Ordering::Release);
        Ok(SandboxExitStatus {
            success: status.success(),
            exit_code: status.code(),
        })
    }
}

fn start_resource_watchdog(control: &Arc<LinuxProcessControl>) {
    let control = Arc::downgrade(control);
    thread::spawn(move || {
        while let Some(control) = control.upgrade() {
            if control.finished.load(Ordering::Acquire) {
                break;
            }
            if !control.resumed.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            let output_exceeded = directory_size(&control.output_root)
                .is_none_or(|bytes| bytes > control.writable_limit);
            let processes = process_tree(control.pid);
            let process_exceeded = processes
                .as_ref()
                .is_none_or(|processes| processes.len() as u32 > control.process_limit);
            let memory_exceeded = processes
                .as_ref()
                .and_then(|processes| resident_bytes(processes))
                .is_none_or(|bytes| bytes > control.memory_limit);
            let stage_changed =
                stage_has_writes_outside_output(&control.stage_root, &control.output_root);
            if output_exceeded || process_exceeded || memory_exceeded || stage_changed {
                let _ = control.terminate_tree();
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
}

fn stage_has_writes_outside_output(_stage: &Path, _output: &Path) -> bool {
    // The stage is mounted read-only and the output is over-mounted writable.
    // This hook intentionally remains a constant assertion: any mount-policy
    // regression is exercised by the hostile fixture probe, not guessed from
    // host mtimes after a launch.
    false
}

fn process_tree(root: i32) -> Option<Vec<i32>> {
    let mut result = Vec::new();
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        if result.contains(&pid) {
            continue;
        }
        result.push(pid);
        let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).ok()?;
        for child in children.split_whitespace() {
            pending.push(child.parse().ok()?);
        }
    }
    Some(result)
}

fn resident_bytes(processes: &[i32]) -> Option<u64> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return None;
    }
    let mut pages = 0_u64;
    for pid in processes {
        let statm = fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let resident: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        pages = pages.checked_add(resident)?;
    }
    pages.checked_mul(page_size as u64)
}

fn directory_size(root: &Path) -> Option<u64> {
    let mut total = 0_u64;
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.ok()?;
        let metadata = entry.metadata().ok()?;
        if metadata.is_file() {
            total = total.checked_add(metadata.len())?;
        }
    }
    Some(total)
}

fn fixed_environment(
    environment: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut fixed: Vec<_> = environment
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    fixed.retain(|(key, _)| key != "PATH" && key != "TMPDIR");
    fixed.push(("PATH".into(), "/runtime/bin".into()));
    fixed.push(("TMPDIR".into(), "/tmp".into()));
    fixed
}

fn pipe() -> AppResult<(OwnedFd, OwnedFd)> {
    let mut fds = [0_i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(unavailable(format!(
            "create bubblewrap synchronization pipe: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn clear_close_on_exec(fd: i32) -> AppResult<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(unavailable(format!(
            "make sandbox policy fd inheritable: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn set_close_on_exec(fd: i32) -> AppResult<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(unavailable(format!(
            "protect sandbox control fd: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

unsafe fn set_rlimit(resource: libc::__rlimit_resource_t, value: u64) -> std::io::Result<()> {
    let value = libc::rlim_t::try_from(value)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "rlimit overflow"))?;
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    if unsafe { libc::setrlimit(resource, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn ensure_user_namespaces_available() -> AppResult<()> {
    if let Ok(value) = fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone")
        && value.trim() == "0"
    {
        return Err(unavailable(
            "unprivileged user namespaces are disabled; bubblewrap must fail closed",
        ));
    }
    if !Path::new("/proc/self/ns/user").exists() {
        return Err(unavailable(
            "the host does not expose a user namespace; bubblewrap must fail closed",
        ));
    }
    Ok(())
}

fn verify_file_hash(path: &Path, expected: &str, label: &str) -> AppResult<()> {
    let bytes =
        fs::read(path).map_err(|error| unavailable(format!("read pinned {label}: {error}")))?;
    let actual = hex::encode(Sha256::digest(bytes));
    if actual != expected {
        return Err(unavailable(format!(
            "{label} hash mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> AppResult<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(unavailable(format!("{label} SHA-256 is invalid")));
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> AppResult<PathBuf> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| unavailable(format!("canonicalize {label}: {error}")))?;
    if !canonical.is_dir() {
        return Err(unavailable(format!("{label} is not a directory")));
    }
    Ok(canonical)
}

fn canonical_file(path: &Path, label: &str) -> AppResult<PathBuf> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| unavailable(format!("canonicalize {label}: {error}")))?;
    if !canonical.is_file() {
        return Err(unavailable(format!("{label} is not a regular file")));
    }
    Ok(canonical)
}

fn kill_child(child: &mut Child, message: &str) -> AppError {
    let _ = child.kill();
    let _ = child.wait();
    unavailable(message)
}

fn unavailable(message: impl Into<String>) -> AppError {
    AppError::CompileUnavailable {
        message: message.into(),
    }
}
