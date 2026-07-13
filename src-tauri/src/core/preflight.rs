//! Conservative arXiv preflight and deterministic archive primitives.

use crate::core::contracts::{
    ArxivCleanBuild, ArxivExcludedFile, ArxivFinding, ArxivFindingSeverity, ArxivIncludedFile,
    ArxivPolicy, ArxivPreflightReportV1, ArxivRuntime, ArxivSource, ArxivUserApproval, LatexEngine,
    ProjectId, Revision,
};
use crate::core::error::{AppError, AppResult};
use crate::core::latex::{normalized_relative, safe_relative_path};
use crate::core::persistence::atomic_write;
use crate::core::source::hash_bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Cursor, Write};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};
use zip::write::SimpleFileOptions;

const MAX_PREFLIGHT_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreflightContext {
    pub project_id: ProjectId,
    pub revision: Revision,
    pub main_file: String,
    pub runtime_profile_id: String,
    pub runtime_manifest_sha256: String,
    pub engine: LatexEngine,
    pub policy_id: String,
    pub submission_tools_commit: Option<String>,
    /// Paths reported as project inputs by a clean `.fls` recorder. They must
    /// already be project-relative; outside paths are blockers.
    #[serde(default)]
    pub recorder_inputs: Vec<String>,
}

#[derive(Debug, Clone)]
struct InventoryFile {
    bytes: Vec<u8>,
    is_symlink: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveBuild {
    pub archive_path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

pub struct ArxivPreflight;

impl ArxivPreflight {
    /// Performs a conservative source/dependency scan. It deliberately leaves
    /// clean-build and user-approval evidence false; those are recorded only
    /// after compiling the exact generated ZIP in a fresh sandbox.
    pub fn scan(root: &Path, context: PreflightContext) -> AppResult<ArxivPreflightReportV1> {
        validate_context(&context)?;
        let root = root
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", root, error))?;
        let inventory = collect_inventory(&root)?;
        let mut findings = Vec::new();
        let mut excluded_reasons = BTreeMap::<String, String>::new();

        record_case_collisions(&inventory, &mut findings);
        let main_file = normalized_relative(&safe_relative_path(&context.main_file)?);
        if !inventory.contains_key(&main_file) {
            return Err(AppError::FileNotFound {
                path: root.join(&main_file).to_string_lossy().into_owned(),
            });
        }

        let mut required = BTreeSet::from([main_file.clone()]);
        let mut source_queue = VecDeque::from([main_file.clone()]);
        let mut scanned_sources = BTreeSet::new();
        let mut needs_bbl = false;
        let mut needs_ind = false;
        let mut needs_gls = false;
        let mut needs_nls = false;
        let mut uses_minted = false;

        while let Some(relative) = source_queue.pop_front() {
            if !scanned_sources.insert(relative.clone()) {
                continue;
            }
            let Some(file) = inventory.get(&relative) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&file.bytes) else {
                add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    "NON_UTF8_SOURCE",
                    "TeX source must be UTF-8 for deterministic preflight analysis.",
                    Some(relative.clone()),
                    None,
                );
                continue;
            };
            let clean = strip_tex_comments(text);
            scan_unsafe_tex(&relative, &clean, &mut findings);
            uses_minted |=
                clean.contains("\\begin{minted}") || clean.contains("\\usepackage{minted}");
            needs_bbl |= clean.contains("\\bibliography{") || clean.contains("\\addbibresource{");
            needs_ind |= clean.contains("\\makeindex");
            needs_gls |= clean.contains("\\makeglossaries");
            needs_nls |= clean.contains("\\makenomenclature");

            for command in ["input", "include", "subfile", "subfileinclude"] {
                for argument in command_arguments(&clean, command) {
                    resolve_dependency(
                        &relative,
                        &argument,
                        &["tex"],
                        true,
                        &inventory,
                        &mut required,
                        &mut source_queue,
                        &mut findings,
                    );
                }
            }
            for argument in command_arguments(&clean, "includegraphics") {
                resolve_dependency(
                    &relative,
                    &argument,
                    &["pdf", "png", "jpg", "jpeg", "eps"],
                    false,
                    &inventory,
                    &mut required,
                    &mut source_queue,
                    &mut findings,
                );
            }
            for argument in command_arguments(&clean, "bibliography") {
                for entry in argument
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                {
                    resolve_dependency(
                        &relative,
                        entry,
                        &["bib"],
                        false,
                        &inventory,
                        &mut required,
                        &mut source_queue,
                        &mut findings,
                    );
                }
            }
            for argument in command_arguments(&clean, "addbibresource") {
                resolve_dependency(
                    &relative,
                    argument.trim(),
                    &["bib"],
                    false,
                    &inventory,
                    &mut required,
                    &mut source_queue,
                    &mut findings,
                );
            }
            for command in ["documentclass", "usepackage"] {
                let extension = if command == "documentclass" {
                    "cls"
                } else {
                    "sty"
                };
                for argument in command_arguments(&clean, command) {
                    for entry in argument
                        .split(',')
                        .map(str::trim)
                        .filter(|entry| !entry.is_empty())
                    {
                        // Runtime-owned classes/packages are valid. Include a
                        // project-local override only when it exists exactly.
                        if let Some(found) = resolve_optional_local_dependency(
                            &relative,
                            entry,
                            &[extension],
                            &inventory,
                            &mut findings,
                        ) && required.insert(found.clone())
                        {
                            source_queue.push_back(found);
                        }
                    }
                }
            }
        }

        for input in &context.recorder_inputs {
            match safe_relative_path(input) {
                Ok(path) => {
                    let normalized = normalized_relative(&path);
                    if inventory.contains_key(&normalized) {
                        required.insert(normalized);
                    } else {
                        add_finding(
                            &mut findings,
                            ArxivFindingSeverity::Blocker,
                            "RECORDER_INPUT_MISSING",
                            "The clean build recorded a project input that is now missing.",
                            Some(input.clone()),
                            None,
                        );
                    }
                }
                Err(_) => add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    "RECORDER_OUTSIDE_ROOT",
                    "The clean build read an input outside the project root.",
                    None,
                    None,
                ),
            }
        }

        let main_stem = Path::new(&main_file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("main");
        for (needed, extension, code, message) in [
            (
                needs_bbl,
                "bbl",
                "BIBLIOGRAPHY_OUTPUT_MISSING",
                "The submission needs a generated .bbl from the selected runtime profile.",
            ),
            (
                needs_ind,
                "ind",
                "INDEX_OUTPUT_MISSING",
                "The submission needs a generated .ind file.",
            ),
            (
                needs_gls,
                "gls",
                "GLOSSARY_OUTPUT_MISSING",
                "The submission needs a generated .gls file.",
            ),
            (
                needs_nls,
                "nls",
                "NOMENCLATURE_OUTPUT_MISSING",
                "The submission needs a generated .nls file.",
            ),
        ] {
            if !needed {
                continue;
            }
            let generated = Path::new(&main_file)
                .parent()
                .unwrap_or(Path::new(""))
                .join(format!("{main_stem}.{extension}"));
            let generated = normalized_relative(&generated);
            if inventory.contains_key(&generated) {
                required.insert(generated);
            } else {
                add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    code,
                    message,
                    Some(main_file.clone()),
                    None,
                );
            }
        }

        if uses_minted {
            let mut cached = 0usize;
            for path in inventory.keys().filter(|path| {
                path.split('/')
                    .any(|component| component.starts_with("_minted-"))
                    && (path.ends_with(".pygtex") || path.ends_with(".pygstyle"))
            }) {
                required.insert(path.clone());
                cached += 1;
            }
            if cached == 0 {
                add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    "MINTED_CACHE_MISSING",
                    "minted requires an existing frozen cache; shell escape remains disabled.",
                    Some(main_file.clone()),
                    None,
                );
            }
        }

        let mut included_files = Vec::new();
        for (path, file) in &inventory {
            if file.is_symlink {
                excluded_reasons.insert(path.clone(), "symbolic link".into());
                add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    "SYMLINK_NOT_ALLOWED",
                    "Submission candidates cannot contain symbolic links.",
                    Some(path.clone()),
                    None,
                );
                continue;
            }
            if is_always_excluded(path) {
                excluded_reasons.insert(path.clone(), exclusion_reason(path));
                if required.contains(path) {
                    add_finding(
                        &mut findings,
                        ArxivFindingSeverity::Blocker,
                        "FORBIDDEN_METADATA_DEPENDENCY",
                        "A paper source depends on Setwright, review, VCS, or build metadata.",
                        Some(path.clone()),
                        None,
                    );
                }
                continue;
            }
            if !required.contains(path) {
                excluded_reasons
                    .insert(path.clone(), "not in the frozen dependency closure".into());
                continue;
            }
            if !allowed_archive_file(path) {
                excluded_reasons.insert(path.clone(), "unsupported submission file type".into());
                add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    "UNSUPPORTED_FILE_TYPE",
                    "A required dependency has a file type Setwright cannot certify for arXiv.",
                    Some(path.clone()),
                    None,
                );
                continue;
            }
            let extension = Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(extension.as_str(), "ttf" | "otf" | "woff" | "woff2") {
                add_finding(
                    &mut findings,
                    ArxivFindingSeverity::Blocker,
                    "UNSUPPORTED_FONT",
                    "Bundled or system font dependencies are not certified by the MVP arXiv profile.",
                    Some(path.clone()),
                    None,
                );
            }
            if extension == "pdf" {
                scan_pdf_actions(path, &file.bytes, &mut findings);
            }
            included_files.push(ArxivIncludedFile {
                path: path.clone(),
                size_bytes: file.bytes.len() as u64,
                sha256: hash_bytes(&file.bytes),
            });
        }

        included_files.sort_by(|left, right| left.path.cmp(&right.path));
        let excluded_files = excluded_reasons
            .into_iter()
            .map(|(path, reason)| ArxivExcludedFile { path, reason })
            .collect::<Vec<_>>();
        findings.sort_by(|left, right| {
            (&left.severity, &left.code, &left.path, &left.line).cmp(&(
                &right.severity,
                &right.code,
                &right.path,
                &right.line,
            ))
        });
        let source_sha256 = hash_included_inventory(&included_files);
        let now = Utc::now();
        Ok(ArxivPreflightReportV1 {
            schema_version: ArxivPreflightReportV1::SCHEMA_VERSION,
            report_id: Uuid::new_v4(),
            generated_at: now,
            source: ArxivSource {
                project_id: context.project_id,
                revision: context.revision,
                main_file,
                source_sha256,
            },
            runtime: ArxivRuntime {
                profile_id: context.runtime_profile_id,
                manifest_sha256: context.runtime_manifest_sha256,
                engine: context.engine,
            },
            policy: ArxivPolicy {
                policy_id: context.policy_id,
                submission_tools_commit: context.submission_tools_commit,
            },
            findings,
            included_files,
            excluded_files,
            clean_build: ArxivCleanBuild {
                succeeded: false,
                started_at: now,
                finished_at: now,
                pdf_sha256: None,
                page_count: None,
            },
            user_approval: ArxivUserApproval {
                approved: false,
                approved_at: None,
                approved_pdf_sha256: None,
            },
            ready: false,
        })
    }

    /// Builds a stable ZIP: sorted entries, fixed DOS timestamp, fixed Unix
    /// permissions and compression level, no directory or metadata entries.
    pub fn build_submission_zip(
        root: &Path,
        report: &ArxivPreflightReportV1,
        output_path: &Path,
    ) -> AppResult<ArchiveBuild> {
        if report
            .findings
            .iter()
            .any(|finding| finding.severity == ArxivFindingSeverity::Blocker)
        {
            return Err(AppError::PreflightBlocked {
                message: "resolve all blocker findings before building the candidate ZIP".into(),
            });
        }
        let root = root
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", root, error))?;
        if output_path.starts_with(&root) {
            return Err(AppError::InvalidPath {
                path: output_path.to_string_lossy().into_owned(),
                message: "submission ZIP must be written outside the paper project".into(),
            });
        }
        let mut verified = Vec::with_capacity(report.included_files.len());
        for included in &report.included_files {
            let relative = safe_relative_path(&included.path)?;
            let path = root.join(relative);
            let canonical = path
                .canonicalize()
                .map_err(|error| AppError::io("canonicalize", &path, error))?;
            if !canonical.starts_with(&root) {
                return Err(AppError::PathOutsideRoot {
                    path: canonical.to_string_lossy().into_owned(),
                });
            }
            let bytes = std::fs::read(&canonical)
                .map_err(|error| AppError::io("read submission input", &canonical, error))?;
            if bytes.len() as u64 != included.size_bytes || hash_bytes(&bytes) != included.sha256 {
                return Err(AppError::PreflightBlocked {
                    message: format!("{} changed after preflight", included.path),
                });
            }
            verified.push((included.path.clone(), bytes));
        }
        verified.sort_by(|left, right| left.0.cmp(&right.0));
        let current_manifest: Vec<_> = verified
            .iter()
            .map(|(path, bytes)| ArxivIncludedFile {
                path: path.clone(),
                size_bytes: bytes.len() as u64,
                sha256: hash_bytes(bytes),
            })
            .collect();
        if hash_included_inventory(&current_manifest) != report.source.source_sha256 {
            return Err(AppError::PreflightBlocked {
                message: "frozen source manifest no longer matches the report".into(),
            });
        }

        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9))
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for (path, bytes) in verified {
            if path == "00README.json" {
                return Err(AppError::PreflightBlocked {
                    message: "00README.json is intentionally not emitted by the MVP".into(),
                });
            }
            archive
                .start_file(path, options)
                .map_err(|error| AppError::Archive {
                    message: error.to_string(),
                })?;
            archive
                .write_all(&bytes)
                .map_err(|error| AppError::Archive {
                    message: error.to_string(),
                })?;
        }
        let bytes = archive
            .finish()
            .map_err(|error| AppError::Archive {
                message: error.to_string(),
            })?
            .into_inner();
        atomic_write(output_path, &bytes)?;
        Ok(ArchiveBuild {
            archive_path: output_path.to_string_lossy().into_owned(),
            sha256: hash_bytes(&bytes),
            size_bytes: bytes.len() as u64,
        })
    }

    pub fn write_report_beside(
        archive_path: &Path,
        report: &ArxivPreflightReportV1,
    ) -> AppResult<PathBuf> {
        validate_report(report)?;
        let file_name = archive_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("submission");
        let report_path = archive_path.with_file_name(format!("{file_name}.preflight.json"));
        let mut bytes = serde_json::to_vec_pretty(report).map_err(AppError::serialization)?;
        bytes.push(b'\n');
        atomic_write(&report_path, &bytes)?;
        Ok(report_path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanBuildEvidence {
    pub succeeded: bool,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub pdf_sha256: Option<String>,
    pub page_count: Option<u32>,
}

pub fn record_clean_build(
    report: &mut ArxivPreflightReportV1,
    evidence: CleanBuildEvidence,
) -> AppResult<()> {
    if evidence.finished_at < evidence.started_at
        || (evidence.succeeded
            && (evidence
                .pdf_sha256
                .as_deref()
                .is_none_or(|hash| !valid_digest(hash))
                || evidence.page_count.is_none_or(|pages| pages == 0)))
        || (!evidence.succeeded && (evidence.pdf_sha256.is_some() || evidence.page_count.is_some()))
    {
        return Err(AppError::InvalidProject {
            message: "clean-build evidence is internally inconsistent".into(),
        });
    }
    report.clean_build = ArxivCleanBuild {
        succeeded: evidence.succeeded,
        started_at: evidence.started_at,
        finished_at: evidence.finished_at,
        pdf_sha256: evidence.pdf_sha256,
        page_count: evidence.page_count,
    };
    report.user_approval = ArxivUserApproval {
        approved: false,
        approved_at: None,
        approved_pdf_sha256: None,
    };
    report.ready = false;
    Ok(())
}

pub fn approve_preflight_pdf(
    report: &mut ArxivPreflightReportV1,
    approved_at: DateTime<Utc>,
) -> AppResult<()> {
    if !report.clean_build.succeeded
        || report
            .clean_build
            .pdf_sha256
            .as_deref()
            .is_none_or(|hash| !valid_digest(hash))
        || report
            .findings
            .iter()
            .any(|finding| finding.severity == ArxivFindingSeverity::Blocker)
    {
        return Err(AppError::PreflightBlocked {
            message: "only a clean, blocker-free candidate PDF can be approved".into(),
        });
    }
    report.user_approval = ArxivUserApproval {
        approved: true,
        approved_at: Some(approved_at),
        approved_pdf_sha256: report.clean_build.pdf_sha256.clone(),
    };
    report.ready = report.computed_ready();
    Ok(())
}

pub fn validate_report(report: &ArxivPreflightReportV1) -> AppResult<()> {
    if report.schema_version != ArxivPreflightReportV1::SCHEMA_VERSION
        || !valid_digest(&report.source.source_sha256)
        || !valid_digest(&report.runtime.manifest_sha256)
        || !report.readiness_is_consistent()
        || report
            .included_files
            .iter()
            .any(|file| safe_relative_path(&file.path).is_err() || !valid_digest(&file.sha256))
        || report
            .excluded_files
            .iter()
            .any(|file| safe_relative_path(&file.path).is_err() || file.reason.is_empty())
    {
        return Err(AppError::Serialization {
            message: "arXiv preflight report violates the canonical V1 contract".into(),
        });
    }
    Ok(())
}

fn validate_context(context: &PreflightContext) -> AppResult<()> {
    safe_relative_path(&context.main_file)?;
    if !context.main_file.ends_with(".tex")
        || !valid_profile_id(&context.runtime_profile_id)
        || !valid_digest(&context.runtime_manifest_sha256)
        || context.policy_id.is_empty()
        || context.policy_id.len() > 128
        || context
            .submission_tools_commit
            .as_deref()
            .is_some_and(|commit| {
                commit.len() != 40
                    || !commit
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    {
        return Err(AppError::InvalidProject {
            message: "invalid arXiv preflight context".into(),
        });
    }
    Ok(())
}

fn collect_inventory(root: &Path) -> AppResult<BTreeMap<String, InventoryFile>> {
    let mut inventory = BTreeMap::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend)
    {
        let entry = entry.map_err(|error| AppError::InvalidProject {
            message: format!("project traversal failed: {error}"),
        })?;
        if entry.depth() == 0 || entry.file_type().is_dir() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| AppError::PathOutsideRoot {
                path: entry.path().to_string_lossy().into_owned(),
            })?;
        let relative = normalized_relative(relative);
        if entry.file_type().is_symlink() {
            // Represent symlinks with empty bytes so the caller can emit a
            // deterministic blocker/exclusion without following the target.
            inventory.insert(
                relative,
                InventoryFile {
                    bytes: Vec::new(),
                    is_symlink: true,
                },
            );
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| AppError::io("inspect", entry.path(), error))?;
        if metadata.len() > MAX_PREFLIGHT_FILE_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "{} exceeds the preflight file limit",
                    entry.path().display()
                ),
            });
        }
        let bytes = std::fs::read(entry.path())
            .map_err(|error| AppError::io("read", entry.path(), error))?;
        inventory.insert(
            relative.clone(),
            InventoryFile {
                bytes,
                is_symlink: false,
            },
        );
    }
    Ok(inventory)
}

fn should_descend(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry.file_name().to_string_lossy().as_ref(),
        ".git" | ".hg" | ".svn" | "node_modules" | "target"
    )
}

fn record_case_collisions(
    inventory: &BTreeMap<String, InventoryFile>,
    findings: &mut Vec<ArxivFinding>,
) {
    let mut seen = BTreeMap::<String, String>::new();
    for path in inventory.keys() {
        let folded = path.to_lowercase();
        if let Some(previous) = seen.insert(folded, path.clone())
            && previous != *path
        {
            add_finding(
                findings,
                ArxivFindingSeverity::Blocker,
                "CASE_COLLISION",
                "Two project paths differ only by case and are not portable to arXiv workers.",
                Some(path.clone()),
                None,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_dependency(
    containing_file: &str,
    raw: &str,
    extensions: &[&str],
    enqueue_source: bool,
    inventory: &BTreeMap<String, InventoryFile>,
    required: &mut BTreeSet<String>,
    source_queue: &mut VecDeque<String>,
    findings: &mut Vec<ArxivFinding>,
) {
    let Some(found) =
        resolve_existing_dependency(containing_file, raw, extensions, inventory, findings)
    else {
        return;
    };
    if required.insert(found.clone()) && enqueue_source {
        source_queue.push_back(found);
    }
}

fn resolve_existing_dependency(
    containing_file: &str,
    raw: &str,
    extensions: &[&str],
    inventory: &BTreeMap<String, InventoryFile>,
    findings: &mut Vec<ArxivFinding>,
) -> Option<String> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty()
        || raw.contains(['\\', '#', '$', '~', '{', '}'])
        || Path::new(raw).is_absolute()
        || has_parent_or_prefix(raw)
    {
        add_finding(
            findings,
            ArxivFindingSeverity::Blocker,
            "UNSAFE_DEPENDENCY_PATH",
            "A dependency path is dynamic, absolute, or leaves the project root.",
            Some(containing_file.into()),
            None,
        );
        return None;
    }
    let parent = Path::new(containing_file).parent().unwrap_or(Path::new(""));
    let base = parent.join(raw);
    let mut candidates = Vec::new();
    if base.extension().is_some() {
        candidates.push(normalized_relative(&base));
    } else {
        candidates.extend(extensions.iter().map(|extension| {
            let mut candidate = base.clone();
            candidate.set_extension(extension);
            normalized_relative(&candidate)
        }));
    }
    for candidate in &candidates {
        if inventory.contains_key(candidate) {
            return Some(candidate.clone());
        }
    }
    let lower_candidates: BTreeSet<_> = candidates.iter().map(|path| path.to_lowercase()).collect();
    if let Some(actual) = inventory
        .keys()
        .find(|path| lower_candidates.contains(&path.to_lowercase()))
    {
        add_finding(
            findings,
            ArxivFindingSeverity::Blocker,
            "CASE_MISMATCH",
            "A dependency's case does not exactly match its project file path.",
            Some(actual.clone()),
            None,
        );
        return None;
    }
    add_finding(
        findings,
        ArxivFindingSeverity::Blocker,
        "MISSING_DEPENDENCY",
        &format!("Required dependency was not found: {raw}"),
        Some(containing_file.into()),
        None,
    );
    None
}

fn resolve_optional_local_dependency(
    containing_file: &str,
    raw: &str,
    extensions: &[&str],
    inventory: &BTreeMap<String, InventoryFile>,
    findings: &mut Vec<ArxivFinding>,
) -> Option<String> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty()
        || raw.contains(['\\', '#', '$', '~', '{', '}'])
        || Path::new(raw).is_absolute()
        || has_parent_or_prefix(raw)
    {
        add_finding(
            findings,
            ArxivFindingSeverity::Blocker,
            "UNSAFE_DEPENDENCY_PATH",
            "A class or package path is dynamic, absolute, or leaves the project root.",
            Some(containing_file.into()),
            None,
        );
        return None;
    }
    let parent = Path::new(containing_file).parent().unwrap_or(Path::new(""));
    let base = parent.join(raw);
    let candidates: Vec<_> = if base.extension().is_some() {
        vec![normalized_relative(&base)]
    } else {
        extensions
            .iter()
            .map(|extension| {
                let mut candidate = base.clone();
                candidate.set_extension(extension);
                normalized_relative(&candidate)
            })
            .collect()
    };
    candidates
        .into_iter()
        .find(|candidate| inventory.contains_key(candidate))
}

fn scan_unsafe_tex(path: &str, source: &str, findings: &mut Vec<ArxivFinding>) {
    for (needle, code, message) in [
        (
            "\\write18",
            "SHELL_ESCAPE",
            "The source attempts to execute a shell command; shell escape is disabled.",
        ),
        (
            "\\ShellEscape",
            "SHELL_ESCAPE",
            "The source attempts to execute a shell command; shell escape is disabled.",
        ),
        (
            "\\directlua",
            "UNSUPPORTED_ENGINE_FEATURE",
            "Lua execution is outside the certified pdfLaTeX/XeLaTeX profiles.",
        ),
        (
            "\\graphicspath",
            "DYNAMIC_GRAPHICS_PATH",
            "The MVP cannot certify graphicspath resolution; use explicit project-relative figure paths.",
        ),
    ] {
        for (offset, _) in source.match_indices(needle) {
            add_finding(
                findings,
                ArxivFindingSeverity::Blocker,
                code,
                message,
                Some(path.into()),
                Some(line_number(source, offset)),
            );
        }
    }
    for needle in [
        "\\setmainfont",
        "\\setsansfont",
        "\\setmonofont",
        "\\newfontfamily",
    ] {
        for (offset, _) in source.match_indices(needle) {
            add_finding(
                findings,
                ArxivFindingSeverity::Blocker,
                "SYSTEM_FONT_DEPENDENCY",
                "System font discovery is not reproducible in the certified arXiv profile.",
                Some(path.into()),
                Some(line_number(source, offset)),
            );
        }
    }
}

fn scan_pdf_actions(path: &str, bytes: &[u8], findings: &mut Vec<ArxivFinding>) {
    for marker in [
        b"/JavaScript".as_slice(),
        b"/OpenAction",
        b"/Launch",
        b"/AA",
    ] {
        if bytes.windows(marker.len()).any(|window| window == marker) {
            add_finding(
                findings,
                ArxivFindingSeverity::Blocker,
                "PDF_ACTIVE_CONTENT",
                "A bundled PDF contains an active action or JavaScript marker.",
                Some(path.into()),
                None,
            );
            break;
        }
    }
}

fn strip_tex_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let bytes = line.as_bytes();
        let mut comment = None;
        for index in 0..bytes.len() {
            if bytes[index] != b'%' {
                continue;
            }
            let preceding_slashes = bytes[..index]
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count();
            if preceding_slashes % 2 == 0 {
                comment = Some(index);
                break;
            }
        }
        match comment {
            Some(index) => {
                output.push_str(&line[..index]);
                if line.ends_with('\n') {
                    output.push('\n');
                }
            }
            None => output.push_str(line),
        }
    }
    output
}

fn command_arguments(source: &str, command: &str) -> Vec<String> {
    let needle = format!("\\{command}");
    let mut arguments = Vec::new();
    let mut search = 0usize;
    while let Some(relative) = source[search..].find(&needle) {
        let command_start = search + relative;
        let after_command = command_start + needle.len();
        if source[after_command..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphabetic())
        {
            search = after_command;
            continue;
        }
        let mut index = after_command;
        while source
            .as_bytes()
            .get(index)
            .is_some_and(u8::is_ascii_whitespace)
        {
            index += 1;
        }
        if source.as_bytes().get(index) == Some(&b'[')
            && let Some(end) = find_balanced(source.as_bytes(), index, b'[', b']')
        {
            index = end + 1;
            while source
                .as_bytes()
                .get(index)
                .is_some_and(u8::is_ascii_whitespace)
            {
                index += 1;
            }
        }
        if source.as_bytes().get(index) == Some(&b'{')
            && let Some(end) = find_balanced(source.as_bytes(), index, b'{', b'}')
        {
            arguments.push(source[index + 1..end].to_string());
            search = end + 1;
        } else {
            search = after_command;
        }
    }
    arguments
}

fn find_balanced(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for index in start..bytes.len() {
        if bytes[index] == open && !escaped(bytes, index) {
            depth += 1;
        } else if bytes[index] == close && !escaped(bytes, index) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn escaped(bytes: &[u8], index: usize) -> bool {
    bytes[..index]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

fn has_parent_or_prefix(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("//")
        || path.starts_with("\\\\")
        || (path.as_bytes().get(1) == Some(&b':'))
        || Path::new(path).components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn is_always_excluded(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    path.split('/').any(|component| component.starts_with('.'))
        || matches!(lower.as_str(), "paper-settings.json" | "00readme.json")
        || lower.ends_with(".setwright-review")
        || file_name.ends_with('~')
        || [
            ".aux",
            ".log",
            ".out",
            ".toc",
            ".lof",
            ".lot",
            ".fls",
            ".fdb_latexmk",
            ".synctex",
            ".synctex.gz",
            ".bak",
            ".zip",
        ]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn exclusion_reason(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if lower == "paper-settings.json" {
        "Setwright project metadata".into()
    } else if lower.ends_with(".setwright-review") {
        "Setwright review exchange metadata".into()
    } else if path.split('/').any(|component| component.starts_with('.')) {
        "hidden or version-control file".into()
    } else {
        "build output, backup, or archive".into()
    }
}

fn allowed_archive_file(path: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "tex"
            | "ltx"
            | "bib"
            | "bbl"
            | "sty"
            | "cls"
            | "bst"
            | "bbx"
            | "cbx"
            | "def"
            | "fd"
            | "cfg"
            | "clo"
            | "png"
            | "jpg"
            | "jpeg"
            | "pdf"
            | "eps"
            | "ind"
            | "gls"
            | "nls"
            | "ist"
            | "idx"
            | "glo"
            | "nlo"
            | "pygtex"
            | "pygstyle"
            | "ttf"
            | "otf"
            | "woff"
            | "woff2"
    )
}

fn hash_included_inventory(files: &[ArxivIncludedFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update((file.path.len() as u64).to_le_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update(file.size_bytes.to_le_bytes());
        hasher.update(file.sha256.as_bytes());
    }
    hex::encode(hasher.finalize())
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

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn line_number(source: &str, byte_offset: usize) -> u32 {
    source.as_bytes()[..byte_offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count() as u32
        + 1
}

fn add_finding(
    findings: &mut Vec<ArxivFinding>,
    severity: ArxivFindingSeverity,
    code: &str,
    message: &str,
    path: Option<String>,
    line: Option<u32>,
) {
    findings.push(ArxivFinding {
        id: Uuid::new_v4(),
        severity,
        code: code.into(),
        message: message.into(),
        path,
        line,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(main_file: &str) -> PreflightContext {
        PreflightContext {
            project_id: ProjectId::new(),
            revision: Revision(7),
            main_file: main_file.into(),
            runtime_profile_id: "texlive-2025.2025-08-03".into(),
            runtime_manifest_sha256: "a".repeat(64),
            engine: LatexEngine::PdfLatex,
            policy_id: "arxiv-texlive-2025".into(),
            submission_tools_commit: Some("b".repeat(40)),
            recorder_inputs: Vec::new(),
        }
    }

    #[test]
    fn scanner_blocks_escape_shell_and_missing_bbl() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("main.tex"),
            "\\documentclass{article}\n\\input{../secret}\n\\write18{bad}\n\\bibliography{refs}\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("refs.bib"), "@misc{x}").unwrap();
        let report = ArxivPreflight::scan(directory.path(), context("main.tex")).unwrap();
        let codes: BTreeSet<_> = report
            .findings
            .iter()
            .map(|finding| finding.code.as_str())
            .collect();
        assert!(codes.contains("UNSAFE_DEPENDENCY_PATH"));
        assert!(codes.contains("SHELL_ESCAPE"));
        assert!(codes.contains("BIBLIOGRAPHY_OUTPUT_MISSING"));
        assert!(!report.ready);
    }

    #[test]
    fn metadata_and_review_bundles_are_excluded() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), "Hello").unwrap();
        std::fs::write(directory.path().join("paper-settings.json"), "{}").unwrap();
        std::fs::write(directory.path().join("notes.setwright-review"), "{}").unwrap();
        let report = ArxivPreflight::scan(directory.path(), context("main.tex")).unwrap();
        assert_eq!(report.included_files.len(), 1);
        assert!(
            report
                .excluded_files
                .iter()
                .any(|file| file.path == "paper-settings.json")
        );
        assert!(
            report
                .excluded_files
                .iter()
                .any(|file| file.path == "notes.setwright-review")
        );
    }

    #[test]
    fn deterministic_zip_has_sorted_entries_and_stable_bytes() {
        let project = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.tex"), "\\input{z}\n\\input{a}\n").unwrap();
        std::fs::write(project.path().join("z.tex"), "z").unwrap();
        std::fs::write(project.path().join("a.tex"), "a").unwrap();
        let report = ArxivPreflight::scan(project.path(), context("main.tex")).unwrap();
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.severity != ArxivFindingSeverity::Blocker)
        );
        let first_path = output.path().join("first.zip");
        let second_path = output.path().join("second.zip");
        let first =
            ArxivPreflight::build_submission_zip(project.path(), &report, &first_path).unwrap();
        let second =
            ArxivPreflight::build_submission_zip(project.path(), &report, &second_path).unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(
            std::fs::read(first_path).unwrap(),
            std::fs::read(second_path).unwrap()
        );
    }

    #[test]
    fn readiness_requires_clean_build_and_matching_approval() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), "Hello").unwrap();
        let mut report = ArxivPreflight::scan(directory.path(), context("main.tex")).unwrap();
        let now = Utc::now();
        record_clean_build(
            &mut report,
            CleanBuildEvidence {
                succeeded: true,
                started_at: now,
                finished_at: now,
                pdf_sha256: Some("c".repeat(64)),
                page_count: Some(1),
            },
        )
        .unwrap();
        assert!(!report.ready);
        approve_preflight_pdf(&mut report, now).unwrap();
        assert!(report.ready);
        assert!(report.readiness_is_consistent());
    }
}
