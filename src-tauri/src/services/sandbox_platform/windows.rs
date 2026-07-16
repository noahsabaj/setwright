use crate::core::error::{AppError, AppResult};
use crate::services::sandbox::{
    PlatformControlledProcess, PlatformLaunchHandle, PlatformSandboxLauncher, SandboxExitStatus,
    SandboxExitWaiter, SandboxLaunchAuthorization, SandboxProcessControl,
};
use std::ffi::c_void;
use std::fs;
use std::io::Read;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, FreeSid, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

const PROFILE_NAME: &str = "org.setwright.compiler";
const PROFILE_DESCRIPTION: &str = "Setwright zero-capability TeX compiler";
const TERMINATED_EXIT_CODE: u32 = 0x53_57_43_58;
const HRESULT_ALREADY_EXISTS: i32 = 0x8007_00b7_u32 as i32;

#[derive(Debug, Clone, Default)]
pub struct WindowsAppContainerLauncher;

impl PlatformSandboxLauncher for WindowsAppContainerLauncher {
    fn launch_attested(
        &self,
        authorization: &SandboxLaunchAuthorization,
    ) -> AppResult<PlatformControlledProcess> {
        launch_appcontainer(authorization)
    }
}

fn launch_appcontainer(
    authorization: &SandboxLaunchAuthorization,
) -> AppResult<PlatformControlledProcess> {
    let request = authorization.request();
    let spec = &request.job().spec;
    let runtime_root = canonical_directory(request.runtime_root(), "runtime root")?;
    let stage_root = canonical_directory(&request.job().staged_project, "compile stage")?;
    let output_root = canonical_directory(request.output_directory(), "compile output")?;
    if !output_root.starts_with(&stage_root) {
        return Err(unavailable("compile output escaped the staged project"));
    }

    let executable = canonical_file(&runtime_root.join("bin").join("latexmk.exe"), "latexmk")?;
    if !executable.starts_with(&runtime_root) {
        return Err(unavailable("latexmk escaped the verified runtime"));
    }

    let sid = AppContainerSid::open_or_create()?;
    let mut acl_guards = Vec::new();
    grant_tree(
        &runtime_root,
        sid.as_psid(),
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        &mut acl_guards,
    )?;
    grant_tree(
        &stage_root,
        sid.as_psid(),
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        &mut acl_guards,
    )?;
    grant_tree(
        &output_root,
        sid.as_psid(),
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        &mut acl_guards,
    )?;

    let mut stdout_pipe = Pipe::new()?;
    let mut stderr_pipe = Pipe::new()?;
    let inherited_handles = [stdout_pipe.write, stderr_pipe.write];
    let mut attribute_list = AttributeList::new(2)?;
    attribute_list.update(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        inherited_handles.as_ptr().cast(),
        size_of_val(&inherited_handles),
    )?;
    let security_capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.as_psid(),
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    attribute_list.update(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
        (&raw const security_capabilities).cast(),
        size_of::<SECURITY_CAPABILITIES>(),
    )?;

    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>())
        .map_err(|_| unavailable("STARTUPINFOEXW size overflow"))?;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = null_mut();
    startup.StartupInfo.hStdOutput = stdout_pipe.write;
    startup.StartupInfo.hStdError = stderr_pipe.write;
    startup.lpAttributeList = attribute_list.as_ptr();

    let mut command_line = command_line(&executable, &spec.command.arguments);
    let environment = environment_block(
        &runtime_root,
        &stage_root,
        &output_root,
        &spec.command.environment,
    )?;
    let application_path = process_api_path(&executable)?;
    let current_directory_path = process_api_path(&stage_root)?;
    let application = wide_null(application_path.as_os_str());
    let current_directory = wide_null(current_directory_path.as_os_str());
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            current_directory.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    };
    if created == 0 {
        return Err(last_os_error("create suspended AppContainer process"));
    }
    drop(attribute_list);
    stdout_pipe.close_write();
    stderr_pipe.close_write();

    let job = match JobHandle::new(spec.limits.memory_bytes, spec.limits.max_child_processes) {
        Ok(job) => job,
        Err(error) => {
            unsafe {
                CloseHandle(process.hThread);
                CloseHandle(process.hProcess);
            }
            return Err(error);
        }
    };
    if unsafe { AssignProcessToJobObject(job.raw(), process.hProcess) } == 0 {
        unsafe {
            TerminateJobObject(job.raw(), TERMINATED_EXIT_CODE);
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
        }
        return Err(last_os_error("assign AppContainer process to Job Object"));
    }

    let process_id = process.dwProcessId;
    let state = Arc::new(ProcessState {
        process: process.hProcess as usize,
        initial_thread: process.hThread as usize,
        job: job.into_raw() as usize,
        resumed: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        output_root,
        writable_limit: spec.limits.writable_bytes,
        acl_guards: Mutex::new(acl_guards),
        appcontainer_sid: sid.into_raw() as usize,
    });
    start_writable_limit_watchdog(&state);
    let stdout: Box<dyn Read + Send> = stdout_pipe.into_reader();
    let stderr: Box<dyn Read + Send> = stderr_pipe.into_reader();
    let control: Arc<dyn SandboxProcessControl> = state.clone();
    let waiter: Box<dyn SandboxExitWaiter> = Box::new(WindowsExitWaiter {
        state: Arc::clone(&state),
    });

    PlatformControlledProcess::new(
        PlatformLaunchHandle {
            opaque_id: format!("appcontainer-job:{process_id}"),
        },
        control,
        stdout,
        stderr,
        waiter,
    )
}

struct ProcessState {
    process: usize,
    initial_thread: usize,
    job: usize,
    resumed: AtomicBool,
    finished: AtomicBool,
    output_root: PathBuf,
    writable_limit: u64,
    acl_guards: Mutex<Vec<ScopedAcl>>,
    appcontainer_sid: usize,
}

impl ProcessState {
    fn job(&self) -> HANDLE {
        self.job as HANDLE
    }

    fn process(&self) -> HANDLE {
        self.process as HANDLE
    }

    fn initial_thread(&self) -> HANDLE {
        self.initial_thread as HANDLE
    }
}

impl SandboxProcessControl for ProcessState {
    fn resume(&self) -> AppResult<()> {
        if self.resumed.swap(true, Ordering::AcqRel) {
            return Err(unavailable("AppContainer process was already resumed"));
        }
        let result = unsafe { ResumeThread(self.initial_thread()) };
        if result == u32::MAX {
            return Err(last_os_error("resume AppContainer process"));
        }
        Ok(())
    }

    fn terminate_tree(&self) -> AppResult<()> {
        if self.finished.load(Ordering::Acquire) {
            return Ok(());
        }
        if unsafe { TerminateJobObject(self.job(), TERMINATED_EXIT_CODE) } == 0 {
            let error = std::io::Error::last_os_error();
            if !self.finished.load(Ordering::Acquire) {
                return Err(unavailable(format!(
                    "terminate AppContainer Job Object: {error}"
                )));
            }
        }
        Ok(())
    }
}

impl Drop for ProcessState {
    fn drop(&mut self) {
        unsafe {
            TerminateJobObject(self.job(), TERMINATED_EXIT_CODE);
            CloseHandle(self.initial_thread());
            CloseHandle(self.process());
            CloseHandle(self.job());
        }
        if let Ok(guards) = self.acl_guards.get_mut() {
            while guards.pop().is_some() {}
        }
        if self.appcontainer_sid != 0 {
            unsafe {
                FreeSid(self.appcontainer_sid as PSID);
            }
        }
    }
}

struct WindowsExitWaiter {
    state: Arc<ProcessState>,
}

impl SandboxExitWaiter for WindowsExitWaiter {
    fn wait(self: Box<Self>) -> AppResult<SandboxExitStatus> {
        let waited = unsafe { WaitForSingleObject(self.state.process(), INFINITE) };
        if waited != WAIT_OBJECT_0 {
            return Err(last_os_error("wait for AppContainer process"));
        }
        let mut exit_code = 0_u32;
        if unsafe { GetExitCodeProcess(self.state.process(), &mut exit_code) } == 0 {
            return Err(last_os_error("read AppContainer exit code"));
        }
        self.state.finished.store(true, Ordering::Release);
        Ok(SandboxExitStatus {
            success: exit_code == 0,
            exit_code: Some(exit_code as i32),
        })
    }
}

fn start_writable_limit_watchdog(state: &Arc<ProcessState>) {
    let state = Arc::downgrade(state);
    thread::spawn(move || {
        while let Some(state) = state.upgrade() {
            if state.finished.load(Ordering::Acquire) {
                break;
            }
            if !state.resumed.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            if directory_size(&state.output_root).is_none_or(|size| size > state.writable_limit) {
                unsafe {
                    TerminateJobObject(state.job(), TERMINATED_EXIT_CODE);
                }
                break;
            }
            let wait = unsafe { WaitForSingleObject(state.process(), 10) };
            if wait == WAIT_OBJECT_0 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
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

struct AppContainerSid(PSID);

impl AppContainerSid {
    fn open_or_create() -> AppResult<Self> {
        let name = wide_null(PROFILE_NAME.as_ref());
        let display = wide_null("Setwright compiler".as_ref());
        let description = wide_null(PROFILE_DESCRIPTION.as_ref());
        let mut sid = null_mut();
        let created = unsafe {
            CreateAppContainerProfile(
                name.as_ptr(),
                display.as_ptr(),
                description.as_ptr(),
                null(),
                0,
                &mut sid,
            )
        };
        if created == HRESULT_ALREADY_EXISTS {
            let derived =
                unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            if derived < 0 {
                return Err(unavailable(format!(
                    "derive AppContainer SID failed with HRESULT 0x{:08x}",
                    derived as u32
                )));
            }
        } else if created < 0 {
            return Err(unavailable(format!(
                "create AppContainer profile failed with HRESULT 0x{:08x}",
                created as u32
            )));
        }
        if sid.is_null() {
            return Err(unavailable("AppContainer profile returned a null SID"));
        }
        Ok(Self(sid))
    }

    fn as_psid(&self) -> PSID {
        self.0
    }

    fn into_raw(mut self) -> PSID {
        let sid = self.0;
        self.0 = null_mut();
        sid
    }
}

impl Drop for AppContainerSid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                FreeSid(self.0);
            }
        }
    }
}

struct ScopedAcl {
    path: Vec<u16>,
    original_descriptor: usize,
    original_dacl: usize,
    granted_dacl: usize,
}

impl ScopedAcl {
    fn grant(path: &Path, sid: PSID, permissions: u32) -> AppResult<Self> {
        let path = wide_null(path.as_os_str());
        let mut original_dacl: *mut ACL = null_mut();
        let mut descriptor = null_mut();
        let read = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut original_dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if read != ERROR_SUCCESS {
            return Err(win32_error("read scoped filesystem ACL", read));
        }
        let trustee = TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.cast(),
        };
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: permissions,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: trustee,
        };
        let mut granted_dacl: *mut ACL = null_mut();
        let merged = unsafe { SetEntriesInAclW(1, &entry, original_dacl, &mut granted_dacl) };
        if merged != ERROR_SUCCESS {
            unsafe {
                LocalFree(descriptor as _);
            }
            return Err(win32_error("build scoped filesystem ACL", merged));
        }
        let applied = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr() as *mut u16,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                granted_dacl,
                null_mut(),
            )
        };
        if applied != ERROR_SUCCESS {
            unsafe {
                LocalFree(granted_dacl as _);
                LocalFree(descriptor as _);
            }
            return Err(win32_error("apply scoped filesystem ACL", applied));
        }
        Ok(Self {
            path,
            original_descriptor: descriptor as usize,
            original_dacl: original_dacl as usize,
            granted_dacl: granted_dacl as usize,
        })
    }
}

impl Drop for ScopedAcl {
    fn drop(&mut self) {
        unsafe {
            SetNamedSecurityInfoW(
                self.path.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                self.original_dacl as *mut ACL,
                null_mut(),
            );
            LocalFree(self.granted_dacl as _);
            LocalFree(self.original_descriptor as _);
        }
    }
}

fn grant_tree(
    root: &Path,
    sid: PSID,
    permissions: u32,
    guards: &mut Vec<ScopedAcl>,
) -> AppResult<()> {
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| unavailable(format!("walk ACL scope: {error}")))?;
        if entry.file_type().is_symlink() {
            return Err(unavailable(format!(
                "sandbox ACL scope contains a symlink: {}",
                entry.path().display()
            )));
        }
        guards.push(ScopedAcl::grant(entry.path(), sid, permissions)?);
    }
    Ok(())
}

struct Pipe {
    read: HANDLE,
    write: HANDLE,
}

impl Pipe {
    fn new() -> AppResult<Self> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .map_err(|_| unavailable("SECURITY_ATTRIBUTES size overflow"))?,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let mut read = null_mut();
        let mut write = null_mut();
        if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
            return Err(last_os_error("create sandbox output pipe"));
        }
        if unsafe { SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0) } == 0 {
            unsafe {
                CloseHandle(read);
                CloseHandle(write);
            }
            return Err(last_os_error("restrict sandbox output pipe inheritance"));
        }
        Ok(Self { read, write })
    }

    fn close_write(&mut self) {
        if !self.write.is_null() {
            unsafe {
                CloseHandle(self.write);
            }
            self.write = null_mut();
        }
    }

    fn into_reader(mut self) -> Box<dyn Read + Send> {
        let read = self.read;
        self.read = null_mut();
        self.write = null_mut();
        let file = unsafe { fs::File::from_raw_handle(read as RawHandle) };
        Box::new(file)
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            if !self.read.is_null() {
                CloseHandle(self.read);
            }
            if !self.write.is_null() {
                CloseHandle(self.write);
            }
        }
    }
}

struct AttributeList {
    bytes: Vec<u8>,
}

impl AttributeList {
    fn new(count: u32) -> AppResult<Self> {
        let mut size = 0_usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size);
        }
        if size == 0 {
            return Err(last_os_error("size process attribute list"));
        }
        let mut bytes = vec![0_u8; size];
        if unsafe {
            InitializeProcThreadAttributeList(bytes.as_mut_ptr().cast(), count, 0, &mut size)
        } == 0
        {
            return Err(last_os_error("initialize process attribute list"));
        }
        Ok(Self { bytes })
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.bytes.as_mut_ptr().cast()
    }

    fn update(&mut self, attribute: usize, value: *const c_void, size: usize) -> AppResult<()> {
        if unsafe {
            UpdateProcThreadAttribute(self.as_ptr(), 0, attribute, value, size, null_mut(), null())
        } == 0
        {
            return Err(last_os_error("set process security attribute"));
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.as_ptr());
        }
    }
}

struct JobHandle(HANDLE);

impl JobHandle {
    fn new(memory_bytes: u64, max_children: u16) -> AppResult<Self> {
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() {
            return Err(last_os_error("create compiler Job Object"));
        }
        let memory = usize::try_from(memory_bytes)
            .map_err(|_| unavailable("compile memory limit exceeds this architecture"))?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_JOB_MEMORY;
        limits.BasicLimitInformation.ActiveProcessLimit = u32::from(max_children).saturating_add(1);
        limits.ProcessMemoryLimit = memory;
        limits.JobMemoryLimit = memory;
        if unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .map_err(|_| unavailable("Job Object limit size overflow"))?,
            )
        } == 0
        {
            let error = last_os_error("configure compiler Job Object");
            unsafe {
                CloseHandle(raw);
            }
            return Err(error);
        }
        Ok(Self(raw))
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn into_raw(mut self) -> HANDLE {
        let raw = self.0;
        self.0 = null_mut();
        raw
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
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

fn command_line(executable: &Path, arguments: &[String]) -> Vec<u16> {
    let mut command = quote_windows_argument(&executable.to_string_lossy());
    for argument in arguments {
        command.push(' ');
        command.push_str(&quote_windows_argument(argument));
    }
    wide_null(command.as_ref())
}

fn quote_windows_argument(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'"')
    {
        return argument.into();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0_usize;
    for character in argument.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
            quoted.push('"');
            backslashes = 0;
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
            quoted.push(character);
            backslashes = 0;
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn environment_block(
    runtime_root: &Path,
    stage_root: &Path,
    output_root: &Path,
    fixed: &std::collections::BTreeMap<String, String>,
) -> AppResult<Vec<u16>> {
    const WINDOWS_BASE_ENVIRONMENT: &[&str] = &[
        "ALLUSERSPROFILE",
        "APPDATA",
        "CommonProgramFiles",
        "CommonProgramFiles(x86)",
        "CommonProgramW6432",
        "COMPUTERNAME",
        "ComSpec",
        "DriverData",
        "HOMEDRIVE",
        "HOMEPATH",
        "LOCALAPPDATA",
        "LOGONSERVER",
        "NUMBER_OF_PROCESSORS",
        "OS",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_IDENTIFIER",
        "PROCESSOR_LEVEL",
        "PROCESSOR_REVISION",
        "ProgramData",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "PUBLIC",
        "SystemDrive",
        "SystemRoot",
        "USERDOMAIN",
        "USERNAME",
        "USERPROFILE",
        "windir",
    ];
    let mut environment = std::collections::BTreeMap::new();
    for key in WINDOWS_BASE_ENVIRONMENT {
        if let Some(value) = std::env::var_os(key) {
            environment.insert((*key).to_owned(), value.to_string_lossy().into_owned());
        }
    }
    environment.extend(fixed.clone());
    let runtime_bin = process_api_path(&runtime_root.join("bin"))?;
    let stage = process_api_path(stage_root)?;
    let output = process_api_path(output_root)?;
    environment.insert("PATH".into(), runtime_bin.to_string_lossy().into_owned());
    environment.insert("TEMP".into(), output.to_string_lossy().into_owned());
    environment.insert("TMP".into(), output.to_string_lossy().into_owned());
    if let Some(drive) = stage.to_string_lossy().get(..2)
        && drive.as_bytes().get(1) == Some(&b':')
    {
        environment.insert(format!("={drive}"), stage.to_string_lossy().into_owned());
    }
    let mut environment: Vec<_> = environment.into_iter().collect();
    environment.sort_by(|(left, _), (right, _)| {
        left.to_ascii_uppercase()
            .cmp(&right.to_ascii_uppercase())
            .then_with(|| left.cmp(right))
    });
    let mut block = Vec::new();
    for (key, value) in environment {
        let hidden_drive = key.len() == 3
            && key.starts_with('=')
            && key.as_bytes()[1].is_ascii_alphabetic()
            && key.ends_with(':');
        if key.contains('\0') || (!hidden_drive && key.contains('=')) || value.contains('\0') {
            return Err(unavailable(
                "compile environment contains an invalid name or value",
            ));
        }
        block.extend(format!("{key}={value}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn process_api_path(path: &Path) -> AppResult<PathBuf> {
    let text = path.to_string_lossy();
    let normalized = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(drive) = text.strip_prefix(r"\\?\") {
        drive.into()
    } else {
        text.into_owned()
    };
    if normalized.encode_utf16().count() >= 260 {
        return Err(unavailable(
            "Windows sandbox process path exceeds the supported current-directory length",
        ));
    }
    Ok(PathBuf::from(normalized))
}

fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn unavailable(message: impl Into<String>) -> AppError {
    AppError::CompileUnavailable {
        message: message.into(),
    }
}

fn last_os_error(operation: &str) -> AppError {
    unavailable(format!("{operation}: {}", std::io::Error::last_os_error()))
}

fn win32_error(operation: &str, code: u32) -> AppError {
    unavailable(format!(
        "{operation}: {}",
        std::io::Error::from_raw_os_error(code as i32)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_argument_quoting_handles_spaces_quotes_and_trailing_slashes() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument("two words"), "\"two words\"");
        assert_eq!(quote_windows_argument("a\\\"b"), "\"a\\\\\\\"b\"");
        assert_eq!(
            quote_windows_argument("C:\\space path\\"),
            "\"C:\\space path\\\\\""
        );
    }

    #[test]
    fn environment_block_is_double_terminated_and_case_insensitively_sorted() {
        let runtime = Path::new(r"C:\runtime");
        let output = Path::new(r"C:\stage\output");
        let fixed = std::collections::BTreeMap::from([
            ("openout_any".into(), "p".into()),
            ("HOME".into(), "/nonexistent".into()),
            ("openin_any".into(), "p".into()),
        ]);
        let stage = Path::new(r"C:\stage");
        let block = environment_block(runtime, stage, output, &fixed).unwrap();
        assert_eq!(&block[block.len() - 2..], &[0, 0]);
        let entries: Vec<_> = block[..block.len() - 1]
            .split(|unit| *unit == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| String::from_utf16(entry).unwrap())
            .collect();
        let keys: Vec<_> = entries
            .iter()
            .map(|entry| {
                if entry.starts_with('=') {
                    entry[..3].to_ascii_uppercase()
                } else {
                    entry.split_once('=').unwrap().0.to_ascii_uppercase()
                }
            })
            .collect();
        assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
