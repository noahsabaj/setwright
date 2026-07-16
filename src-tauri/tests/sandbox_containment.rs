use base64::Engine as _;
use chrono::Utc;
use ed25519_dalek::{Signer, SigningKey};
use serde::Deserialize;
use setwright_lib::core::compile::{CompileJob, CompileSpec, SandboxBackend};
use setwright_lib::core::contracts::{
    LatexEngine, LocalDocument, ProjectSessionId, Revision, RuntimeArtifact, RuntimeManifestV1,
    RuntimeSbom, RuntimeSignature, SbomFormat, TexLiveSnapshot,
};
use setwright_lib::services::runtime::{
    RuntimeInstallOutcome, RuntimeInstaller, RuntimeTarget, TrustedRuntimeKeys,
    canonical_manifest_payload, verify_runtime_manifest,
};
use setwright_lib::services::sandbox::{
    CommonSandboxProbeEvidence, LinuxBubblewrapEvidence, MacosXpcEvidence, PlatformSandboxLauncher,
    SandboxAttestation, SandboxExitStatus, SandboxLaunchRequest, SandboxProbeEvidence,
    SandboxProbeRunner, WindowsAppContainerEvidence, current_sandbox_backend,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

const KEY_ID: &str = "setwright-containment-spike-test-key";
const SBOM: &[u8] = br#"{"spdxVersion":"SPDX-2.3","name":"hostile fixture"}"#;
const LICENSES: &[u8] = b"Apache-2.0\n";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoundaryResult {
    runtime_read_only: bool,
    stage_read_only: bool,
    outside_read_denied: bool,
    outside_write_denied: bool,
    original_project_write_denied: bool,
    dns_denied: bool,
    http_denied: bool,
    empty_home_and_config: bool,
    shell_escape_profile_locked: bool,
    latexmkrc_ignored: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessCountResult {
    successful_children: usize,
    error: Option<String>,
}

struct ProbeRun {
    stage: PathBuf,
    status: SandboxExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    trigger_observed: bool,
}

#[test]
fn native_hostile_fixture_containment_remains_incomplete_attestation() {
    if std::env::var_os("SETWRIGHT_PREPARE_FIXTURE_RUNTIME").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        let preparation = TempDir::new().expect("create fixture preparation root");
        let runtime = install_signed_fixture_runtime(preparation.path());
        println!(
            "prepared signed fixture runtime: {}",
            runtime.root().display()
        );
        return;
    }
    if std::env::var_os("SETWRIGHT_RUN_NATIVE_SANDBOX_PROBE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        eprintln!("native containment probe skipped outside the dedicated signed-fixture job");
        return;
    }

    #[cfg(target_os = "macos")]
    let root = tempfile::Builder::new()
        .prefix("setwright-containment-")
        .tempdir_in(required_env_path("SETWRIGHT_XPC_STAGE_ROOT"))
        .expect("create containment probe root in the app-group container");
    #[cfg(not(target_os = "macos"))]
    let root = TempDir::new().expect("create containment probe root");
    let canary_root = TempDir::new().expect("create outside-canary root");
    let runtime = install_signed_fixture_runtime(root.path());

    #[cfg(target_os = "windows")]
    run_suite(
        root.path(),
        canary_root.path(),
        &runtime,
        setwright_lib::services::sandbox_platform::WindowsAppContainerLauncher,
    );

    #[cfg(target_os = "linux")]
    {
        use setwright_lib::services::sandbox_platform::{BubblewrapLauncher, BubblewrapSidecar};
        let sidecar = BubblewrapSidecar::new(
            required_env_path("SETWRIGHT_BWRAP_PATH"),
            required_env("SETWRIGHT_BWRAP_VERSION"),
            required_env("SETWRIGHT_BWRAP_SHA256"),
            required_env_path("SETWRIGHT_SECCOMP_PATH"),
            required_env("SETWRIGHT_SECCOMP_SHA256"),
        )
        .expect("verify pinned bubblewrap sidecar and seccomp filter");
        run_suite(
            root.path(),
            canary_root.path(),
            &runtime,
            BubblewrapLauncher::new(sidecar).expect("create bubblewrap launcher"),
        );
    }

    #[cfg(target_os = "macos")]
    {
        use setwright_lib::services::sandbox_platform::{MacosXpcLauncher, MacosXpcService};
        let service = MacosXpcService::verify(
            required_env_path("SETWRIGHT_XPC_BUNDLE"),
            &required_env("SETWRIGHT_XPC_SERVICE_NAME"),
            &required_env("SETWRIGHT_XPC_REQUIREMENT"),
            &required_env("SETWRIGHT_XPC_APP_GROUP"),
            runtime.root(),
            required_env_path("SETWRIGHT_XPC_STAGE_ROOT"),
        )
        .expect("verify signed XPC service");
        run_suite(
            root.path(),
            canary_root.path(),
            &runtime,
            MacosXpcLauncher::new(service).expect("create XPC launcher"),
        );
    }
}

fn run_suite<L>(
    root: &Path,
    canary_root: &Path,
    runtime: &setwright_lib::services::runtime::InstalledRuntime,
    launcher: L,
) where
    L: PlatformSandboxLauncher,
{
    let outside = canary_root.join("outside-canary.txt");
    let original = canary_root.join("original-project-canary.txt");
    fs::write(&outside, b"outside secret").unwrap();
    fs::write(&original, b"original paper").unwrap();
    let runner = SandboxProbeRunner::new(launcher);

    let boundary = run_mode(&runner, runtime, root, "boundary", &outside, &original);
    assert!(
        boundary.status.success,
        "boundary fixture failed: {}",
        display_run(&boundary)
    );
    let boundary_result: BoundaryResult = serde_json::from_slice(
        &fs::read(boundary.stage.join("output/probe-result.json"))
            .expect("boundary fixture emitted evidence"),
    )
    .expect("decode boundary evidence");
    assert_eq!(fs::read(&outside).unwrap(), b"outside secret");
    assert_eq!(fs::read(&original).unwrap(), b"original paper");

    let cancellation = run_mode(&runner, runtime, root, "cancellation", &outside, &original);
    assert!(
        cancellation.trigger_observed && !cancellation.status.success,
        "process-tree cancellation was not observed: {}",
        display_run(&cancellation)
    );

    let memory = run_mode(&runner, runtime, root, "memory", &outside, &original);
    assert!(
        !memory.status.success,
        "memory limit did not terminate the hostile fixture: {}",
        display_run(&memory)
    );
    let process_count = run_mode(&runner, runtime, root, "process-count", &outside, &original);
    assert!(
        process_count.trigger_observed && !process_count.status.success,
        "process limit did not terminate the hostile fixture: {}",
        display_run(&process_count)
    );
    let writable = run_mode(
        &runner,
        runtime,
        root,
        "writable-output",
        &outside,
        &original,
    );
    assert!(
        !writable.status.success,
        "writable limit did not terminate the hostile fixture: {}",
        display_run(&writable)
    );

    assert!(boundary_result.runtime_read_only);
    #[cfg(not(target_os = "macos"))]
    assert!(boundary_result.stage_read_only);
    assert!(boundary_result.outside_read_denied);
    assert!(boundary_result.outside_write_denied);
    assert!(boundary_result.original_project_write_denied);
    assert!(boundary_result.dns_denied);
    assert!(boundary_result.http_denied);
    assert!(boundary_result.empty_home_and_config);
    assert!(boundary_result.shell_escape_profile_locked);
    assert!(boundary_result.latexmkrc_ignored);

    let common = CommonSandboxProbeEvidence {
        schema_version: 1,
        policy_version: 1,
        profile_id: runtime.profile_id().into(),
        runtime_manifest_sha256: runtime.manifest_sha256().into(),
        broker_build_sha256: hash_file(
            &std::env::current_exe().expect("locate containment probe executable"),
        ),
        probed_at: Utc::now(),
        sandbox_started: true,
        runtime_read_only: boundary_result.runtime_read_only,
        staged_project_only: boundary_result.original_project_write_denied
            && (cfg!(target_os = "macos") || boundary_result.stage_read_only),
        empty_home_and_config: boundary_result.empty_home_and_config,
        outside_canary_denied: boundary_result.outside_read_denied
            && boundary_result.outside_write_denied,
        dns_denied: boundary_result.dns_denied,
        http_denied: boundary_result.http_denied,
        shell_escape_denied: boundary_result.shell_escape_profile_locked,
        latexmkrc_ignored: boundary_result.latexmkrc_ignored,
        process_tree_killed: cancellation.trigger_observed && !cancellation.status.success,
        memory_limit_enforced: !memory.status.success,
        writable_limit_enforced: !writable.status.success,
        child_limit_enforced: process_count.trigger_observed && !process_count.status.success,
        // A hostile native helper is not a TeX Live workflow attestation.
        pdflatex_passed: false,
        xelatex_passed: false,
        bibtex_passed: false,
        biber_passed: false,
        synctex_passed: false,
    };
    let evidence = platform_evidence(common);
    let evidence_path = std::env::var_os("SETWRIGHT_PROBE_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("sandbox-probe-evidence.json"));
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize probe evidence"),
    )
    .expect("write probe evidence");
    println!("containment evidence: {}", evidence_path.display());
    assert!(
        SandboxAttestation::verify_for_current_host(evidence).is_err(),
        "containment-only fixture evidence must not unlock production compilation"
    );
}

fn run_mode<L>(
    runner: &SandboxProbeRunner<L>,
    runtime: &setwright_lib::services::runtime::InstalledRuntime,
    root: &Path,
    mode: &str,
    outside: &Path,
    original: &Path,
) -> ProbeRun
where
    L: PlatformSandboxLauncher,
{
    let stage = root.join(format!("stage-{mode}"));
    let output = stage.join("output");
    fs::create_dir_all(&output).unwrap();
    fs::write(stage.join("main.tex"), b"fixture").unwrap();
    let backend = current_sandbox_backend().unwrap();
    let spec = CompileSpec::preview(
        runtime.profile_id(),
        LatexEngine::PdfLatex,
        Path::new("main.tex"),
        backend,
    )
    .unwrap();
    fs::write(
        stage.join("probe.json"),
        serde_json::to_vec(&serde_json::json!({
            "mode": mode,
            "outsideCanary": outside,
            "originalProjectCanary": original,
            "maxChildProcesses": spec.limits.max_child_processes,
        }))
        .unwrap(),
    )
    .unwrap();
    let max_child_processes = spec.limits.max_child_processes;
    let job = CompileJob::new(
        ProjectSessionId::new(),
        Revision::INITIAL,
        fs::canonicalize(&stage).unwrap(),
        spec,
    )
    .unwrap();
    let request =
        SandboxLaunchRequest::new(job, runtime, fs::canonicalize(&output).unwrap()).unwrap();
    let process = runner.launch_fixture(&request).unwrap();
    let control = process.control();
    process.resume().unwrap();
    let (mut stdout, mut stderr, waiter) = process.into_io();
    let stdout_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let (status_tx, status_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = status_tx.send(waiter.wait());
    });

    let mut trigger_observed = false;
    let mut early_status = None;
    if mode == "cancellation" {
        let marker = stage.join("output/child.pid");
        let deadline = Instant::now() + Duration::from_secs(45);
        while Instant::now() < deadline {
            if marker.exists() {
                trigger_observed = true;
                control.terminate_tree().unwrap();
                break;
            }
            match status_rx.try_recv() {
                Ok(status) => {
                    early_status = Some(status.unwrap());
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("sandbox waiter disconnected"),
                Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(25)),
            }
        }
        if !trigger_observed && early_status.is_none() {
            control.terminate_tree().unwrap();
        }
    } else if mode == "process-count" {
        let armed_marker = stage.join("output/process-limit-armed.json");
        let blocked_marker = stage.join("output/process-spawn-blocked.json");
        let breached_marker = stage.join("output/process-limit-breached.json");
        let deadline = Instant::now() + Duration::from_secs(45);
        while Instant::now() < deadline {
            if blocked_marker.exists() {
                let armed: ProcessCountResult = serde_json::from_slice(
                    &fs::read(&armed_marker).expect("read armed process-limit evidence"),
                )
                .expect("decode armed process-limit evidence");
                let result: ProcessCountResult = serde_json::from_slice(
                    &fs::read(&blocked_marker).expect("read process rejection evidence"),
                )
                .expect("decode process rejection evidence");
                assert_eq!(
                    armed.successful_children,
                    usize::from(max_child_processes),
                    "process-limit probe was armed at the wrong threshold"
                );
                assert_eq!(
                    result.successful_children,
                    usize::from(max_child_processes),
                    "native process limit rejected a child at the wrong threshold"
                );
                assert!(
                    result
                        .error
                        .as_deref()
                        .is_some_and(|error| !error.is_empty()),
                    "native process-limit rejection omitted its OS error"
                );
                trigger_observed = true;
                control.terminate_tree().unwrap();
                break;
            }
            match status_rx.try_recv() {
                Ok(status) => {
                    let status = status.unwrap();
                    if armed_marker.exists() {
                        let armed: ProcessCountResult = serde_json::from_slice(
                            &fs::read(&armed_marker).expect("read armed process-limit evidence"),
                        )
                        .expect("decode armed process-limit evidence");
                        assert_eq!(
                            armed.successful_children,
                            usize::from(max_child_processes),
                            "process-limit probe was armed at the wrong threshold"
                        );
                        trigger_observed = !status.success;
                    }
                    early_status = Some(status);
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("sandbox waiter disconnected"),
                Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(25)),
            }
        }
        if !trigger_observed && early_status.is_none() {
            let detail = if breached_marker.exists() {
                let result: ProcessCountResult = serde_json::from_slice(
                    &fs::read(&breached_marker).expect("read process-limit breach evidence"),
                )
                .expect("decode process-limit breach evidence");
                format!(
                    "watchdog left {} children alive after the configured limit was exceeded",
                    result.successful_children
                )
            } else if !armed_marker.exists() {
                "hostile fixture exited before reaching the configured process limit".to_owned()
            } else {
                "hostile fixture reached the configured process limit but never crossed it"
                    .to_owned()
            };
            control.terminate_tree().unwrap();
            eprintln!("process-count containment failure: {detail}");
        }
    }

    let status = early_status.unwrap_or_else(|| {
        status_rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| {
                let _ = control.terminate_tree();
                panic!("sandbox mode {mode} did not exit after its resource limit")
            })
            .unwrap()
    });
    drop(control);
    let run = ProbeRun {
        stage,
        status,
        stdout: stdout_thread.join().unwrap().unwrap(),
        stderr: stderr_thread.join().unwrap().unwrap(),
        trigger_observed,
    };
    println!("sandbox mode {mode}: {}", display_run(&run));
    run
}

fn install_signed_fixture_runtime(
    root: &Path,
) -> setwright_lib::services::runtime::InstalledRuntime {
    let target = RuntimeTarget::current().unwrap();
    let platform = match target.platform {
        setwright_lib::core::contracts::RuntimePlatform::Windows => "windows",
        setwright_lib::core::contracts::RuntimePlatform::Macos => "macos",
        setwright_lib::core::contracts::RuntimePlatform::Linux => "linux",
    };
    let architecture = match target.architecture {
        setwright_lib::core::contracts::RuntimeArchitecture::X86_64 => "x86_64",
        setwright_lib::core::contracts::RuntimeArchitecture::Aarch64 => "aarch64",
    };
    let profile_id = format!("texlive-2025.2025-08-03-{platform}-{architecture}");
    let signing_key = SigningKey::from_bytes(&[0x53; 32]);
    let mut keys = TrustedRuntimeKeys::new();
    keys.insert(KEY_ID, signing_key.verifying_key().to_bytes())
        .unwrap();
    #[cfg(target_os = "macos")]
    let install_root = std::env::var_os("SETWRIGHT_XPC_RUNTIME_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("installed-runtimes"));
    #[cfg(not(target_os = "macos"))]
    let install_root = root.join("installed-runtimes");
    let installer = RuntimeInstaller::new(install_root).unwrap();
    if std::env::var_os("SETWRIGHT_USE_PREINSTALLED_RUNTIME").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        return installer
            .load_installed(&profile_id, target, &keys)
            .expect("reopen preinstalled signed fixture runtime");
    }

    let fixture = std::env::var_os("SETWRIGHT_HOSTILE_FIXTURE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_sandbox-hostile-fixture")));
    let fixture_bytes = fs::read(&fixture).expect("read hostile fixture binary");
    let tool_path = if cfg!(target_os = "windows") {
        "bin/latexmk.exe"
    } else {
        "bin/latexmk"
    };
    let mut archive_bytes = Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut archive_bytes);
        let file_options = SimpleFileOptions::default().unix_permissions(0o755);
        archive.start_file(tool_path, file_options).unwrap();
        archive.write_all(&fixture_bytes).unwrap();
        archive
            .start_file(
                "docs/runtime.spdx.json",
                SimpleFileOptions::default().unix_permissions(0o644),
            )
            .unwrap();
        archive.write_all(SBOM).unwrap();
        archive
            .start_file(
                "docs/licenses.json",
                SimpleFileOptions::default().unix_permissions(0o644),
            )
            .unwrap();
        archive.write_all(LICENSES).unwrap();
        archive.finish().unwrap();
    }
    archive_bytes.rewind().unwrap();
    let bytes = archive_bytes.into_inner();
    let archive_path = root.join("hostile-fixture.zip");
    fs::write(&archive_path, &bytes).unwrap();
    let mut manifest = RuntimeManifestV1 {
        schema_version: 1,
        profile_id,
        tex_live_snapshot: TexLiveSnapshot {
            version: 2025,
            date: chrono::NaiveDate::from_ymd_opt(2025, 8, 3).unwrap(),
        },
        platform: target.platform,
        architecture: target.architecture,
        engines: vec![LatexEngine::PdfLatex, LatexEngine::XeLatex],
        archive: RuntimeArtifact {
            url: "https://runtime.setwright.invalid/hostile-fixture.zip".into(),
            size_bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(&bytes)),
        },
        signature: RuntimeSignature {
            algorithm: "ed25519".into(),
            canonicalization: "RFC8785".into(),
            key_id: KEY_ID.into(),
            value: String::new(),
        },
        sbom: RuntimeSbom {
            path: "docs/runtime.spdx.json".into(),
            sha256: hex::encode(Sha256::digest(SBOM)),
            format: SbomFormat::SpdxJson23,
        },
        license_inventory: LocalDocument {
            path: "docs/licenses.json".into(),
            sha256: hex::encode(Sha256::digest(LICENSES)),
        },
        created_at: Utc::now(),
    };
    let payload = canonical_manifest_payload(&manifest).unwrap();
    manifest.signature.value =
        base64::engine::general_purpose::STANDARD.encode(signing_key.sign(&payload).to_bytes());
    let verified = verify_runtime_manifest(manifest, target, &keys).unwrap();
    match installer.install(&verified, &archive_path).unwrap() {
        RuntimeInstallOutcome::Installed(runtime)
        | RuntimeInstallOutcome::AlreadyInstalled(runtime) => runtime,
    }
}

fn platform_evidence(common: CommonSandboxProbeEvidence) -> SandboxProbeEvidence {
    match current_sandbox_backend().unwrap() {
        SandboxBackend::WindowsAppContainer => {
            SandboxProbeEvidence::WindowsAppContainer(WindowsAppContainerEvidence {
                common,
                appcontainer_sid: "org.setwright.compiler".into(),
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
                common,
                service_code_requirement: required_env("SETWRIGHT_XPC_REQUIREMENT"),
                app_sandbox_enabled: true,
                network_entitlement_absent: true,
                app_group_stage_only: true,
                xpc_peer_requirement_enforced: true,
                process_limits_enforced: true,
            })
        }
        SandboxBackend::LinuxBubblewrap => {
            SandboxProbeEvidence::LinuxBubblewrap(LinuxBubblewrapEvidence {
                common,
                bubblewrap_sha256: required_env("SETWRIGHT_BWRAP_SHA256"),
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

fn display_run(run: &ProbeRun) -> String {
    format!(
        "status={:?}, stdout={}, stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    )
}

fn hash_file(path: &Path) -> String {
    let mut file = File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    hex::encode(hasher.finalize())
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required by the native probe job"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn required_env_path(name: &str) -> PathBuf {
    PathBuf::from(required_env(name))
}
