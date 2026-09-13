use crate::core::error::{AppError, AppResult};
use crate::services::sandbox::{
    PlatformControlledProcess, PlatformLaunchHandle, PlatformSandboxLauncher, SandboxExitStatus,
    SandboxExitWaiter, SandboxLaunchAuthorization, SandboxProcessControl,
};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

const ERROR_BUFFER_BYTES: usize = 1024;
const JOB_ID_BYTES: usize = 128;
const XPC_LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);
const XPC_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const XPC_WAIT_GRACE: Duration = Duration::from_secs(15);

unsafe extern "C" {
    fn sw_xpc_verify_service(
        bundle_path: *const c_char,
        requirement: *const c_char,
        app_group: *const c_char,
        error: *mut c_char,
        error_length: usize,
    ) -> c_int;
    fn sw_xpc_connect(
        service_name: *const c_char,
        error: *mut c_char,
        error_length: usize,
    ) -> *mut c_void;
    fn sw_xpc_cancel(client: *mut c_void);
    fn sw_xpc_disconnect(client: *mut c_void);
    fn sw_xpc_launch(
        client: *mut c_void,
        runtime_root: *const c_char,
        stage_root: *const c_char,
        output_root: *const c_char,
        arguments: *const *const c_char,
        argument_count: usize,
        environment_keys: *const *const c_char,
        environment_values: *const *const c_char,
        environment_count: usize,
        timeout_ms: u64,
        memory_limit: u64,
        writable_limit: u64,
        process_limit: u32,
        stdout_fd: *mut c_int,
        stderr_fd: *mut c_int,
        root_pid: *mut i32,
        job_id: *mut c_char,
        job_id_length: usize,
        error: *mut c_char,
        error_length: usize,
    ) -> c_int;
    fn sw_xpc_resume(
        client: *mut c_void,
        job_id: *const c_char,
        error: *mut c_char,
        error_length: usize,
    ) -> c_int;
    fn sw_xpc_terminate(
        client: *mut c_void,
        job_id: *const c_char,
        reason: *const c_char,
        error: *mut c_char,
        error_length: usize,
    ) -> c_int;
    fn sw_xpc_wait(
        client: *mut c_void,
        job_id: *const c_char,
        exit_code: *mut i32,
        error: *mut c_char,
        error_length: usize,
    ) -> c_int;
    fn sw_process_tree_resident_bytes(
        root_pid: i32,
        process_limit: u32,
        resident_bytes: *mut u64,
        error: *mut c_char,
        error_length: usize,
    ) -> c_int;
}

#[derive(Debug, Clone)]
pub struct MacosXpcService {
    bundle: PathBuf,
    service_name: CString,
    code_requirement: CString,
    app_group: CString,
    allowed_runtime_root: PathBuf,
    app_group_stage_root: PathBuf,
}

impl MacosXpcService {
    pub fn verify(
        bundle: impl Into<PathBuf>,
        service_name: &str,
        code_requirement: &str,
        app_group: &str,
        allowed_runtime_root: impl Into<PathBuf>,
        app_group_stage_root: impl Into<PathBuf>,
    ) -> AppResult<Self> {
        let service = Self {
            bundle: bundle.into(),
            service_name: CString::new(service_name)
                .map_err(|_| unavailable("XPC service name contains NUL"))?,
            code_requirement: CString::new(code_requirement)
                .map_err(|_| unavailable("XPC code requirement contains NUL"))?,
            app_group: CString::new(app_group)
                .map_err(|_| unavailable("XPC app group contains NUL"))?,
            allowed_runtime_root: canonical_directory(
                &allowed_runtime_root.into(),
                "signed runtime root",
            )?,
            app_group_stage_root: canonical_directory(
                &app_group_stage_root.into(),
                "app-group compile stage",
            )?,
        };
        service.verify_signature_and_entitlements()?;
        Ok(service)
    }

    fn verify_signature_and_entitlements(&self) -> AppResult<()> {
        let bundle = path_cstring(&self.bundle)?;
        let mut error = [0_i8; ERROR_BUFFER_BYTES];
        let result = unsafe {
            sw_xpc_verify_service(
                bundle.as_ptr(),
                self.code_requirement.as_ptr(),
                self.app_group.as_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        ffi_result(result, &error, "verify embedded XPC service")
    }
}

#[derive(Debug, Clone)]
pub struct MacosXpcLauncher {
    service: MacosXpcService,
}

impl MacosXpcLauncher {
    pub fn new(service: MacosXpcService) -> AppResult<Self> {
        service.verify_signature_and_entitlements()?;
        Ok(Self { service })
    }
}

impl PlatformSandboxLauncher for MacosXpcLauncher {
    fn launch_attested(
        &self,
        authorization: &SandboxLaunchAuthorization,
    ) -> AppResult<PlatformControlledProcess> {
        self.service.verify_signature_and_entitlements()?;
        launch_xpc(authorization, &self.service)
    }
}

fn launch_xpc(
    authorization: &SandboxLaunchAuthorization,
    service: &MacosXpcService,
) -> AppResult<PlatformControlledProcess> {
    let request = authorization.request();
    let spec = &request.job().spec;
    let runtime_root = canonical_directory(request.runtime_root(), "runtime root")?;
    let stage_root = canonical_directory(&request.job().staged_project, "compile stage")?;
    let output_root = canonical_directory(request.output_directory(), "compile output")?;
    if !runtime_root.starts_with(&service.allowed_runtime_root) {
        return Err(unavailable(
            "runtime is outside the signed XPC helper runtime root",
        ));
    }
    if !stage_root.starts_with(&service.app_group_stage_root)
        || !output_root.starts_with(&stage_root)
    {
        return Err(unavailable(
            "compile stage is outside the XPC service app-group container",
        ));
    }
    let executable = fs::canonicalize(runtime_root.join("bin/latexmk"))
        .map_err(|error| unavailable(format!("canonicalize signed latexmk helper: {error}")))?;
    if !executable.is_file() || !executable.starts_with(&runtime_root) {
        return Err(unavailable(
            "signed latexmk helper is absent from the fixed runtime layout",
        ));
    }

    let endpoint = XpcEndpoint {
        service_name: service.service_name.clone(),
    };
    let client = endpoint.connect()?;
    let runtime = path_cstring(&runtime_root)?;
    let stage = path_cstring(&stage_root)?;
    let output = path_cstring(&output_root)?;
    let arguments: Vec<_> = spec
        .command
        .arguments
        .iter()
        .map(|argument| CString::new(argument.as_str()))
        .collect::<Result<_, _>>()
        .map_err(|_| unavailable("fixed compile argument contains NUL"))?;
    let argument_pointers: Vec<_> = arguments.iter().map(|value| value.as_ptr()).collect();
    let environment: Vec<_> = spec
        .command
        .environment
        .iter()
        .map(|(key, value)| {
            Ok((
                CString::new(key.as_str())
                    .map_err(|_| unavailable("fixed environment name contains NUL"))?,
                CString::new(value.as_str())
                    .map_err(|_| unavailable("fixed environment value contains NUL"))?,
            ))
        })
        .collect::<AppResult<_>>()?;
    let environment_keys: Vec<_> = environment.iter().map(|(key, _)| key.as_ptr()).collect();
    let environment_values: Vec<_> = environment
        .iter()
        .map(|(_, value)| value.as_ptr())
        .collect();
    let mut stdout_fd = -1;
    let mut stderr_fd = -1;
    let mut root_pid = 0_i32;
    let mut job_id = [0_i8; JOB_ID_BYTES];
    let mut error = [0_i8; ERROR_BUFFER_BYTES];
    let timeout_ms = spec
        .limits
        .timeout_seconds
        .checked_mul(1000)
        .ok_or_else(|| unavailable("XPC compile timeout overflowed milliseconds"))?;
    let launched = client.call_with_deadline(
        XPC_LAUNCH_TIMEOUT,
        "launch sandboxed XPC compile helper",
        |raw| unsafe {
            sw_xpc_launch(
                raw,
                runtime.as_ptr(),
                stage.as_ptr(),
                output.as_ptr(),
                argument_pointers.as_ptr(),
                argument_pointers.len(),
                environment_keys.as_ptr(),
                environment_values.as_ptr(),
                environment.len(),
                timeout_ms,
                spec.limits.memory_bytes,
                spec.limits.writable_bytes,
                u32::from(spec.limits.max_child_processes).saturating_add(1),
                &mut stdout_fd,
                &mut stderr_fd,
                &mut root_pid,
                job_id.as_mut_ptr(),
                job_id.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        },
    )?;
    ffi_result(launched, &error, "launch sandboxed XPC compile helper")?;
    if stdout_fd < 0 || stderr_fd < 0 || root_pid <= 0 {
        return Err(unavailable("XPC service returned invalid stream handles"));
    }
    let job_id = unsafe { CStr::from_ptr(job_id.as_ptr()) }
        .to_str()
        .map_err(|_| unavailable("XPC service returned a non-UTF-8 job id"))?;
    let job_id = CString::new(job_id).map_err(|_| unavailable("XPC job id contains NUL"))?;
    let state = Arc::new(XpcJob {
        endpoint,
        job_id,
        root_pid,
        wall_timeout: spec.limits.timeout(),
        memory_limit: spec.limits.memory_bytes,
        process_limit: u32::from(spec.limits.max_child_processes).saturating_add(1),
        resumed: AtomicBool::new(false),
        finished: AtomicBool::new(false),
    });
    start_memory_watchdog(&state);
    let stdout: Box<dyn Read + Send> = Box::new(unsafe { File::from_raw_fd(stdout_fd) });
    let stderr: Box<dyn Read + Send> = Box::new(unsafe { File::from_raw_fd(stderr_fd) });
    let control: Arc<dyn SandboxProcessControl> = state.clone();
    let waiter: Box<dyn SandboxExitWaiter> = Box::new(XpcExitWaiter {
        state: Arc::clone(&state),
    });
    PlatformControlledProcess::new(
        PlatformLaunchHandle {
            opaque_id: format!("xpc-job:{}", state.job_id.to_string_lossy()),
        },
        control,
        stdout,
        stderr,
        waiter,
    )
}

#[derive(Debug, Clone)]
struct XpcEndpoint {
    service_name: CString,
}

impl XpcEndpoint {
    fn connect(&self) -> AppResult<Arc<XpcClient>> {
        XpcClient::connect(&self.service_name)
    }
}

struct XpcClient {
    raw: usize,
}

impl XpcClient {
    fn connect(service_name: &CStr) -> AppResult<Arc<Self>> {
        let mut error = [0_i8; ERROR_BUFFER_BYTES];
        let raw = unsafe { sw_xpc_connect(service_name.as_ptr(), error.as_mut_ptr(), error.len()) };
        if raw.is_null() {
            return Err(ffi_error(&error, "connect to embedded XPC service"));
        }
        Ok(Arc::new(Self { raw: raw as usize }))
    }

    fn raw(&self) -> *mut c_void {
        self.raw as *mut c_void
    }

    fn call_with_deadline<T>(
        self: &Arc<Self>,
        timeout: Duration,
        label: &str,
        operation: impl FnOnce(*mut c_void) -> T,
    ) -> AppResult<T> {
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let timed_out = Arc::new(AtomicBool::new(false));
        let deadline_timed_out = Arc::clone(&timed_out);
        let deadline_client = Arc::clone(self);
        let deadline = thread::Builder::new()
            .name("setwright-xpc-deadline".into())
            .spawn(move || {
                if matches!(
                    finished_rx.recv_timeout(timeout),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    deadline_timed_out.store(true, Ordering::Release);
                    unsafe {
                        sw_xpc_cancel(deadline_client.raw());
                    }
                }
            })
            .map_err(|error| unavailable(format!("{label}: start XPC deadline: {error}")))?;
        let result = operation(self.raw());
        let _ = finished_tx.send(());
        deadline
            .join()
            .map_err(|_| unavailable(format!("{label}: XPC deadline monitor panicked")))?;
        if timed_out.load(Ordering::Acquire) {
            Err(unavailable(format!(
                "{label}: XPC service did not reply within {} ms",
                timeout.as_millis()
            )))
        } else {
            Ok(result)
        }
    }
}

impl Drop for XpcClient {
    fn drop(&mut self) {
        unsafe {
            sw_xpc_disconnect(self.raw());
        }
    }
}

unsafe impl Send for XpcClient {}
unsafe impl Sync for XpcClient {}

struct XpcJob {
    endpoint: XpcEndpoint,
    job_id: CString,
    root_pid: i32,
    wall_timeout: Duration,
    memory_limit: u64,
    process_limit: u32,
    resumed: AtomicBool,
    finished: AtomicBool,
}

impl XpcJob {
    fn operation(
        &self,
        operation: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_char, usize) -> c_int,
        label: &str,
    ) -> AppResult<()> {
        let client = self.endpoint.connect()?;
        let mut error = [0_i8; ERROR_BUFFER_BYTES];
        let result = client.call_with_deadline(XPC_CONTROL_TIMEOUT, label, |raw| unsafe {
            operation(raw, self.job_id.as_ptr(), error.as_mut_ptr(), error.len())
        })?;
        ffi_result(result, &error, label)
    }

    fn terminate_with_reason(&self, reason: &str, label: &str) -> AppResult<()> {
        let reason =
            CString::new(reason).map_err(|_| unavailable("XPC termination reason contains NUL"))?;
        let client = self.endpoint.connect()?;
        let mut error = [0_i8; ERROR_BUFFER_BYTES];
        let result = client.call_with_deadline(XPC_CONTROL_TIMEOUT, label, |raw| unsafe {
            sw_xpc_terminate(
                raw,
                self.job_id.as_ptr(),
                reason.as_ptr(),
                error.as_mut_ptr(),
                error.len(),
            )
        })?;
        ffi_result(result, &error, label)
    }
}

impl SandboxProcessControl for XpcJob {
    fn resume(&self) -> AppResult<()> {
        if self.resumed.swap(true, Ordering::AcqRel) {
            return Err(unavailable("XPC compiler process was already resumed"));
        }
        self.operation(sw_xpc_resume, "resume XPC compiler process")
    }

    fn terminate_tree(&self) -> AppResult<()> {
        self.terminate_with_reason(
            "cancellation requested",
            "terminate XPC compiler process group",
        )
    }
}

impl Drop for XpcJob {
    fn drop(&mut self) {
        if !self.finished.load(Ordering::Acquire) {
            let _ = self.terminate_with_reason(
                "broker dropped the compile job",
                "terminate dropped XPC compiler job",
            );
        }
    }
}

fn start_memory_watchdog(state: &Arc<XpcJob>) {
    let state = Arc::downgrade(state);
    thread::spawn(move || {
        let mut invalid_samples = 0_u8;
        loop {
            let Some(job) = state.upgrade() else {
                break;
            };
            if job.finished.load(Ordering::Acquire) {
                break;
            }
            if !job.resumed.load(Ordering::Acquire) {
                drop(job);
                thread::sleep(Duration::from_millis(10));
                continue;
            }

            let mut resident_bytes = 0_u64;
            let mut error = [0_i8; ERROR_BUFFER_BYTES];
            let result = unsafe {
                sw_process_tree_resident_bytes(
                    job.root_pid,
                    job.process_limit,
                    &mut resident_bytes,
                    error.as_mut_ptr(),
                    error.len(),
                )
            };
            if result == 0 {
                invalid_samples = 0;
                if resident_bytes > job.memory_limit {
                    let reason = format!(
                        "resident memory {resident_bytes} exceeded limit {}",
                        job.memory_limit
                    );
                    let _ = job.terminate_with_reason(
                        &reason,
                        "terminate XPC compiler after memory limit",
                    );
                    break;
                }
            } else {
                invalid_samples = invalid_samples.saturating_add(1);
                if invalid_samples >= 3 {
                    let detail = ffi_message(&error);
                    let reason = format!("memory inspection failed: {detail}");
                    let _ = job.terminate_with_reason(
                        &reason,
                        "terminate XPC compiler after memory inspection failure",
                    );
                    break;
                }
            }
            drop(job);
            thread::sleep(Duration::from_millis(10));
        }
    });
}

struct XpcExitWaiter {
    state: Arc<XpcJob>,
}

impl SandboxExitWaiter for XpcExitWaiter {
    fn wait(self: Box<Self>) -> AppResult<SandboxExitStatus> {
        let client = self.state.endpoint.connect()?;
        let mut exit_code = 0_i32;
        let mut error = [0_i8; ERROR_BUFFER_BYTES];
        let wait_timeout = self
            .state
            .wall_timeout
            .checked_add(XPC_WAIT_GRACE)
            .ok_or_else(|| unavailable("XPC wait timeout overflowed"))?;
        let result = client.call_with_deadline(
            wait_timeout,
            "wait for XPC compiler process",
            |raw| unsafe {
                sw_xpc_wait(
                    raw,
                    self.state.job_id.as_ptr(),
                    &mut exit_code,
                    error.as_mut_ptr(),
                    error.len(),
                )
            },
        )?;
        ffi_result(result, &error, "wait for XPC compiler process")?;
        self.state.finished.store(true, Ordering::Release);
        Ok(SandboxExitStatus {
            success: exit_code == 0,
            exit_code: Some(exit_code),
        })
    }
}

fn path_cstring(path: &Path) -> AppResult<CString> {
    CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| unavailable("sandbox path contains NUL"))
}

fn canonical_directory(path: &Path, label: &str) -> AppResult<PathBuf> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| unavailable(format!("canonicalize {label}: {error}")))?;
    if !canonical.is_dir() {
        return Err(unavailable(format!("{label} is not a directory")));
    }
    Ok(canonical)
}

fn ffi_result(result: c_int, error: &[c_char], operation: &str) -> AppResult<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(ffi_error(error, operation))
    }
}

fn ffi_error(error: &[c_char], operation: &str) -> AppError {
    let message = ffi_message(error);
    unavailable(format!("{operation}: {message}"))
}

fn ffi_message(error: &[c_char]) -> String {
    if error.first().copied().unwrap_or_default() == 0 {
        "native XPC bridge returned no detail".into()
    } else {
        unsafe { CStr::from_ptr(error.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
}

fn unavailable(message: impl Into<String>) -> AppError {
    AppError::CompileUnavailable {
        message: message.into(),
    }
}
