// Host-native hostile helper used only by cross-platform containment probes.
//
// CI installs this binary at the managed runtime's fixed `bin/latexmk`
// location. It deliberately ignores TeX arguments and reads its probe mode
// from the staged `probe.json`, so the production launcher still supplies
// only Setwright's exact fixed command profile.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeConfig {
    mode: String,
    outside_canary: Option<PathBuf>,
    original_project_canary: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
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

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--fixture-child") {
        child_mode();
        return;
    }
    let config: ProbeConfig = serde_json::from_slice(
        &fs::read("probe.json").expect("hostile fixture requires staged probe.json"),
    )
    .expect("probe.json must match the hostile fixture schema");
    match config.mode.as_str() {
        "boundary" => boundary_mode(&config),
        "cancellation" => cancellation_mode(),
        "memory" => memory_mode(),
        "process-count" => process_count_mode(),
        "writable-output" => writable_output_mode(),
        other => panic!("unknown hostile fixture mode: {other}"),
    }
}

fn boundary_mode(config: &ProbeConfig) {
    let current_exe = std::env::current_exe().expect("locate hostile fixture");
    let runtime_probe = current_exe
        .parent()
        .expect("fixture runtime bin directory")
        .join("runtime-write-probe");
    let arguments: Vec<String> = std::env::args().collect();
    let outside_read_denied = config
        .outside_canary
        .as_deref()
        .is_some_and(|path| fs::read(path).is_err());
    let outside_write_denied = config.outside_canary.as_deref().is_some_and(write_denied);
    let original_project_write_denied = config
        .original_project_canary
        .as_deref()
        .is_some_and(write_denied);
    let result = BoundaryResult {
        runtime_read_only: write_denied(&runtime_probe),
        stage_read_only: write_denied(Path::new("stage-write-probe")),
        outside_read_denied,
        outside_write_denied,
        original_project_write_denied,
        dns_denied: ("example.com", 443).to_socket_addrs().is_err(),
        http_denied: "1.1.1.1:80".parse().ok().is_none_or(|address| {
            TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err()
        }),
        empty_home_and_config: ["HOME", "TEXMFHOME", "TEXMFCONFIG"]
            .iter()
            .all(|key| std::env::var(key).ok().as_deref() == Some("/nonexistent")),
        shell_escape_profile_locked: arguments
            .iter()
            .any(|argument| argument.contains("no-shell-escape"))
            && !arguments.iter().any(|argument| {
                argument.contains("shell-escape") && !argument.contains("no-shell-escape")
            }),
        latexmkrc_ignored: arguments.iter().any(|argument| argument == "-norc"),
    };
    let bytes = serde_json::to_vec(&result).expect("serialize boundary result");
    fs::write("output/probe-result.json", bytes).expect("write boundary result");
}

fn write_denied(path: &Path) -> bool {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(b"sandbox escape"))
        .is_err()
}

fn cancellation_mode() {
    fs::write("output/parent.pid", std::process::id().to_string())
        .expect("write hostile parent pid");
    let mut child = Command::new(std::env::current_exe().expect("locate hostile fixture"))
        .arg("--fixture-child")
        .spawn()
        .expect("spawn hostile child");
    fs::write("output/child.pid", child.id().to_string()).expect("write hostile child pid");
    loop {
        if child.try_wait().expect("poll hostile child").is_some() {
            panic!("hostile child exited before cancellation");
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn child_mode() {
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn memory_mode() {
    let mut allocations: Vec<Vec<u8>> = Vec::new();
    loop {
        let mut chunk = vec![0_u8; 64 * 1024 * 1024];
        for byte in chunk.iter_mut().step_by(4096) {
            *byte = 0x5a;
        }
        allocations.push(chunk);
        std::hint::black_box(&allocations);
    }
}

fn process_count_mode() {
    let executable = std::env::current_exe().expect("locate hostile fixture");
    let mut children = Vec::new();
    loop {
        match Command::new(&executable).arg("--fixture-child").spawn() {
            Ok(child) => {
                children.push(child);
                // Stay hostile and exceed the 32-child policy quickly, while
                // leaving enough scheduler time for a polling native watchdog
                // to terminate the tree before the CI account exhausts PIDs.
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                let _ = fs::write("output/process-limit.txt", error.to_string());
                loop {
                    thread::sleep(Duration::from_secs(60));
                }
            }
        }
    }
}

fn writable_output_mode() {
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open("output/oversized.bin")
        .expect("create writable-limit probe");
    file.set_len(1024 * 1024 * 1024 + 1)
        .expect("grow writable-limit probe");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
