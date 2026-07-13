use crate::core::error::{AppError, AppResult};
use crate::core::latex::{ensure_within, normalized_relative, safe_relative_path};
use crate::core::source::hash_bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AtomicWriteOutcome {
    Unchanged,
    Written,
}

/// Writes in the destination directory, flushes file contents, then atomically
/// replaces the destination. Identical bytes are a true no-op: no temporary
/// file is created and the destination timestamp is left untouched.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<AtomicWriteOutcome> {
    if path.is_file() {
        let existing = std::fs::read(path).map_err(|error| AppError::io("read", path, error))?;
        if existing == bytes {
            return Ok(AtomicWriteOutcome::Unchanged);
        }
    }
    let parent = path.parent().ok_or_else(|| AppError::InvalidPath {
        path: path.to_string_lossy().into_owned(),
        message: "destination has no parent directory".into(),
    })?;
    if !parent.is_dir() {
        return Err(AppError::InvalidPath {
            path: parent.to_string_lossy().into_owned(),
            message: "destination directory does not exist".into(),
        });
    }

    let mut temporary = tempfile::Builder::new()
        .prefix(".setwright-save-")
        .tempfile_in(parent)
        .map_err(|error| AppError::io("create temporary file", parent, error))?;
    temporary
        .write_all(bytes)
        .map_err(|error| AppError::io("write temporary file", temporary.path(), error))?;
    temporary
        .flush()
        .map_err(|error| AppError::io("flush temporary file", temporary.path(), error))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| AppError::io("sync temporary file", temporary.path(), error))?;

    if let Ok(metadata) = std::fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| AppError::io("preserve permissions", temporary.path(), error))?;
    }
    temporary
        .persist(path)
        .map_err(|error| AppError::io("replace", path, error.error))?;
    sync_directory(parent)?;
    Ok(AtomicWriteOutcome::Written)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> AppResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AppError::io("sync directory", path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> AppResult<()> {
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionWrite {
    pub relative_path: PathBuf,
    pub bytes: Vec<u8>,
}

impl TransactionWrite {
    pub fn new(relative_path: impl Into<PathBuf>, bytes: Vec<u8>) -> Self {
        Self {
            relative_path: relative_path.into(),
            bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryJournalV1 {
    schema_version: u32,
    root: String,
    created_at: DateTime<Utc>,
    state: RecoveryState,
    entries: Vec<RecoveryEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum RecoveryState {
    Prepared,
    Applying,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryEntry {
    relative_path: String,
    old_bytes: Option<Vec<u8>>,
    old_hash: Option<String>,
    /// `None` represents a deletion in the complete new transaction state.
    new_bytes: Option<Vec<u8>>,
    new_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryChoice {
    RestoreOld,
    RestoreNew,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub journal_path: PathBuf,
    pub project_root: PathBuf,
    pub choice: RecoveryChoice,
    pub restored_files: Vec<String>,
}

/// Atomically writes each member of a multi-file save and records both the old
/// and new complete states before the first project file is replaced.
///
/// The journal path must live in application data, not in the project. It is
/// removed only after all writes complete. If a process is interrupted, the
/// caller can use `recover_transaction` to choose one complete state.
pub fn save_transaction(
    root: &Path,
    journal_path: &Path,
    writes: &[TransactionWrite],
) -> AppResult<Vec<String>> {
    save_transaction_with_deletions(root, journal_path, writes, &[])
}

/// The deletion-capable form used by exact history restoration. Writes and
/// deletions share one journal, so recovery can only expose the complete old
/// manifest or the complete new manifest.
pub fn save_transaction_with_deletions(
    root: &Path,
    journal_path: &Path,
    writes: &[TransactionWrite],
    deletions: &[PathBuf],
) -> AppResult<Vec<String>> {
    if writes.is_empty() && deletions.is_empty() {
        return Ok(Vec::new());
    }
    if journal_path.exists() {
        return Err(AppError::PreflightBlocked {
            message: format!(
                "an unfinished recovery journal must be resolved first: {}",
                journal_path.display()
            ),
        });
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", root, error))?;
    if journal_path.starts_with(&canonical_root) {
        return Err(AppError::InvalidPath {
            path: journal_path.to_string_lossy().into_owned(),
            message: "recovery journals must live outside the paper project".into(),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut entries = Vec::with_capacity(writes.len());
    for write in writes {
        let relative = safe_relative_path(&write.relative_path.to_string_lossy())?;
        let normalized = normalized_relative(&relative);
        if !seen.insert(normalized.clone()) {
            return Err(AppError::InvalidEdit {
                reason: format!("duplicate transaction target: {normalized}"),
            });
        }
        let destination = canonical_root.join(&relative);
        ensure_lexically_within(&canonical_root, &destination)?;
        let old_bytes = if destination.exists() {
            let canonical_destination = destination
                .canonicalize()
                .map_err(|error| AppError::io("canonicalize", &destination, error))?;
            ensure_within(&canonical_root, &canonical_destination)?;
            Some(
                std::fs::read(&canonical_destination)
                    .map_err(|error| AppError::io("read", &canonical_destination, error))?,
            )
        } else {
            let parent = destination.parent().unwrap_or(&canonical_root);
            let canonical_parent = parent
                .canonicalize()
                .map_err(|error| AppError::io("canonicalize", parent, error))?;
            ensure_within(&canonical_root, &canonical_parent)?;
            None
        };
        // Skip per-file no-ops before creating any journal.
        if old_bytes.as_deref() == Some(write.bytes.as_slice()) {
            continue;
        }
        entries.push(RecoveryEntry {
            relative_path: normalized,
            old_hash: old_bytes.as_deref().map(hash_bytes),
            old_bytes,
            new_hash: Some(hash_bytes(&write.bytes)),
            new_bytes: Some(write.bytes.clone()),
        });
    }
    for deletion in deletions {
        let relative = safe_relative_path(&deletion.to_string_lossy())?;
        let normalized = normalized_relative(&relative);
        if !seen.insert(normalized.clone()) {
            return Err(AppError::InvalidEdit {
                reason: format!("duplicate transaction target: {normalized}"),
            });
        }
        let destination = canonical_root.join(&relative);
        ensure_lexically_within(&canonical_root, &destination)?;
        let metadata = match std::fs::symlink_metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(AppError::io("inspect deletion", &destination, error)),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AppError::ExternalConflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        let canonical_destination = destination
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", &destination, error))?;
        ensure_within(&canonical_root, &canonical_destination)?;
        let old_bytes = std::fs::read(&canonical_destination)
            .map_err(|error| AppError::io("read deletion", &canonical_destination, error))?;
        entries.push(RecoveryEntry {
            relative_path: normalized,
            old_hash: Some(hash_bytes(&old_bytes)),
            old_bytes: Some(old_bytes),
            new_hash: None,
            new_bytes: None,
        });
    }
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let parent = journal_path.parent().ok_or_else(|| AppError::InvalidPath {
        path: journal_path.to_string_lossy().into_owned(),
        message: "journal has no parent directory".into(),
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| AppError::io("create recovery directory", parent, error))?;
    let mut journal = RecoveryJournalV1 {
        schema_version: 1,
        root: canonical_root.to_string_lossy().into_owned(),
        created_at: Utc::now(),
        state: RecoveryState::Prepared,
        entries,
    };
    write_journal(journal_path, &journal)?;
    journal.state = RecoveryState::Applying;
    write_journal(journal_path, &journal)?;

    for entry in &journal.entries {
        let destination = canonical_root.join(safe_relative_path(&entry.relative_path)?);
        let apply_result = match &entry.new_bytes {
            Some(bytes) => atomic_write(&destination, bytes).map(|_| ()),
            None => remove_file_if_present(&canonical_root, &destination),
        };
        if let Err(error) = apply_result {
            // Best-effort rollback. The journal remains if rollback itself is
            // incomplete, allowing deterministic recovery on next launch.
            let rollback_ok = restore_entries(
                &canonical_root,
                &journal.entries,
                RecoveryChoice::RestoreOld,
            )
            .is_ok();
            if rollback_ok {
                let _ = std::fs::remove_file(journal_path);
            }
            return Err(error);
        }
    }
    let written = journal
        .entries
        .iter()
        .map(|entry| entry.relative_path.clone())
        .collect();
    std::fs::remove_file(journal_path)
        .map_err(|error| AppError::io("remove recovery journal", journal_path, error))?;
    sync_directory(parent)?;
    Ok(written)
}

pub fn recover_transaction(journal_path: &Path, choice: RecoveryChoice) -> AppResult<Vec<String>> {
    let journal = read_journal(journal_path)?;
    let root = PathBuf::from(&journal.root)
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", Path::new(&journal.root), error))?;
    restore_entries(&root, &journal.entries, choice)?;
    let restored = journal
        .entries
        .iter()
        .map(|entry| entry.relative_path.clone())
        .collect();
    std::fs::remove_file(journal_path)
        .map_err(|error| AppError::io("remove recovery journal", journal_path, error))?;
    if let Some(parent) = journal_path.parent() {
        sync_directory(parent)?;
    }
    Ok(restored)
}

/// Recovers every durable project transaction left by an interrupted process.
///
/// A journal in `prepared` state cannot have touched project files, so the old
/// state wins. Once a journal reaches `applying`, accepted operations are
/// completed to the new state. Recovery first verifies that every destination
/// is still either its recorded old or new value; an unrelated external edit
/// stops recovery before any file is changed.
pub fn recover_pending_transactions(recovery_directory: &Path) -> AppResult<Vec<RecoveryReport>> {
    if !recovery_directory.exists() {
        return Ok(Vec::new());
    }
    let mut journals = std::fs::read_dir(recovery_directory)
        .map_err(|error| AppError::io("read recovery directory", recovery_directory, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::io("read recovery directory", recovery_directory, error))?;
    journals.sort_by_key(std::fs::DirEntry::file_name);

    let mut reports = Vec::new();
    for entry in journals {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| AppError::io("inspect recovery journal", &path, error))?;
        if !metadata.file_type().is_file()
            || path.extension().and_then(|extension| extension.to_str()) != Some("json")
        {
            continue;
        }
        let journal = read_journal(&path)?;
        let choice = match journal.state {
            RecoveryState::Prepared => RecoveryChoice::RestoreOld,
            RecoveryState::Applying => RecoveryChoice::RestoreNew,
        };
        let project_root = PathBuf::from(&journal.root)
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", Path::new(&journal.root), error))?;
        let restored_files = recover_transaction(&path, choice)?;
        reports.push(RecoveryReport {
            journal_path: path,
            project_root,
            choice,
            restored_files,
        });
    }
    Ok(reports)
}

fn read_journal(path: &Path) -> AppResult<RecoveryJournalV1> {
    let bytes =
        std::fs::read(path).map_err(|error| AppError::io("read recovery journal", path, error))?;
    let journal: RecoveryJournalV1 =
        serde_json::from_slice(&bytes).map_err(AppError::serialization)?;
    if journal.schema_version != 1 {
        return Err(AppError::Serialization {
            message: format!("unsupported recovery schema {}", journal.schema_version),
        });
    }
    validate_recovery_entries(&journal.entries)?;
    Ok(journal)
}

fn restore_entries(
    root: &Path,
    entries: &[RecoveryEntry],
    choice: RecoveryChoice,
) -> AppResult<()> {
    validate_recovery_destinations(root, entries)?;
    for entry in entries {
        let destination = root.join(safe_relative_path(&entry.relative_path)?);
        match choice {
            RecoveryChoice::RestoreNew => match &entry.new_bytes {
                Some(bytes) => {
                    atomic_write(&destination, bytes)?;
                }
                None => remove_file_if_present(root, &destination)?,
            },
            RecoveryChoice::RestoreOld => match &entry.old_bytes {
                Some(old_bytes) => {
                    atomic_write(&destination, old_bytes)?;
                }
                None => remove_file_if_present(root, &destination)?,
            },
        }
    }
    Ok(())
}

fn validate_recovery_entries(entries: &[RecoveryEntry]) -> AppResult<()> {
    let mut paths = std::collections::BTreeSet::new();
    for entry in entries {
        let relative = safe_relative_path(&entry.relative_path)?;
        let normalized = normalized_relative(&relative);
        if normalized != entry.relative_path || !paths.insert(normalized.clone()) {
            return Err(AppError::Serialization {
                message: format!(
                    "invalid or duplicate recovery path: {}",
                    entry.relative_path
                ),
            });
        }
        match (&entry.new_bytes, &entry.new_hash) {
            (Some(bytes), Some(hash)) if hash_bytes(bytes) == *hash => {}
            (None, None) => {}
            _ => {
                return Err(AppError::Serialization {
                    message: format!("recovery new-state hash mismatch for {normalized}"),
                });
            }
        }
        match (&entry.old_bytes, &entry.old_hash) {
            (Some(bytes), Some(hash)) if hash_bytes(bytes) == *hash => {}
            (None, None) => {}
            _ => {
                return Err(AppError::Serialization {
                    message: format!("recovery old-state hash mismatch for {normalized}"),
                });
            }
        }
        if entry.old_bytes.is_none() && entry.new_bytes.is_none() {
            return Err(AppError::Serialization {
                message: format!("recovery entry has no old or new file for {normalized}"),
            });
        }
    }
    Ok(())
}

fn validate_recovery_destinations(root: &Path, entries: &[RecoveryEntry]) -> AppResult<()> {
    for entry in entries {
        let destination = root.join(safe_relative_path(&entry.relative_path)?);
        ensure_lexically_within(root, &destination)?;
        let current = match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::ExternalConflict {
                    path: destination.to_string_lossy().into_owned(),
                });
            }
            Ok(metadata) if metadata.is_file() => Some(
                std::fs::read(&destination)
                    .map_err(|error| AppError::io("read recovery target", &destination, error))?,
            ),
            Ok(_) => {
                return Err(AppError::ExternalConflict {
                    path: destination.to_string_lossy().into_owned(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(AppError::io("inspect recovery target", &destination, error));
            }
        };
        let matches_old = match (&entry.old_bytes, &current) {
            (Some(old), Some(current)) => old == current,
            (None, None) => true,
            _ => false,
        };
        let matches_new = match (&entry.new_bytes, &current) {
            (Some(new), Some(current)) => new == current,
            (None, None) => true,
            _ => false,
        };
        if !matches_old && !matches_new {
            return Err(AppError::ExternalConflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(())
}

fn remove_file_if_present(root: &Path, destination: &Path) -> AppResult<()> {
    let metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::io("inspect deletion target", destination, error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AppError::ExternalConflict {
            path: destination.to_string_lossy().into_owned(),
        });
    }
    let canonical = destination
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize deletion target", destination, error))?;
    ensure_within(root, &canonical)?;
    std::fs::remove_file(&canonical)
        .map_err(|error| AppError::io("remove transaction file", &canonical, error))?;
    if let Some(parent) = canonical.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn write_journal(path: &Path, journal: &RecoveryJournalV1) -> AppResult<()> {
    let bytes = serde_json::to_vec(journal).map_err(AppError::serialization)?;
    atomic_write(path, &bytes)?;
    // Re-open with write access to make the durability intent explicit even
    // on platforms where rename durability differs.
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| AppError::io("sync recovery journal", path, error))
}

fn ensure_lexically_within(root: &Path, candidate: &Path) -> AppResult<()> {
    if candidate.starts_with(root) {
        Ok(())
    } else {
        Err(AppError::PathOutsideRoot {
            path: candidate.to_string_lossy().into_owned(),
        })
    }
}

/// Reads an entire file after confirming it is not being grown without bound.
pub fn read_bounded(path: &Path, max_bytes: u64) -> AppResult<Vec<u8>> {
    let file = File::open(path).map_err(|error| AppError::io("open", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| AppError::io("inspect", path, error))?;
    if metadata.len() > max_bytes {
        return Err(AppError::InvalidProject {
            message: format!("{} exceeds the {max_bytes} byte limit", path.display()),
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io("read", path, error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::InvalidProject {
            message: format!(
                "{} changed while reading and exceeds the limit",
                path.display()
            ),
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn atomic_noop_does_not_touch_mtime() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.tex");
        std::fs::write(&path, b"same").unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            atomic_write(&path, b"same").unwrap(),
            AtomicWriteOutcome::Unchanged
        );
        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after);
        assert!(after <= SystemTime::now());
    }

    #[test]
    fn transaction_writes_complete_new_state_and_removes_journal() {
        let project = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.tex"), b"old a").unwrap();
        std::fs::write(project.path().join("b.tex"), b"old b").unwrap();
        let journal = app_data.path().join("save.json");
        let written = save_transaction(
            project.path(),
            &journal,
            &[
                TransactionWrite::new("a.tex", b"new a".to_vec()),
                TransactionWrite::new("b.tex", b"new b".to_vec()),
            ],
        )
        .unwrap();
        assert_eq!(written, ["a.tex", "b.tex"]);
        assert_eq!(
            std::fs::read(project.path().join("a.tex")).unwrap(),
            b"new a"
        );
        assert_eq!(
            std::fs::read(project.path().join("b.tex")).unwrap(),
            b"new b"
        );
        assert!(!journal.exists());
    }

    #[test]
    fn transaction_journals_deletions_with_the_same_complete_state() {
        let project = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.tex"), b"old main").unwrap();
        std::fs::write(project.path().join("extra.tex"), b"old extra").unwrap();
        let journal = app_data.path().join("restore.json");
        let changed = save_transaction_with_deletions(
            project.path(),
            &journal,
            &[TransactionWrite::new("main.tex", b"snapshot main".to_vec())],
            &[PathBuf::from("extra.tex")],
        )
        .unwrap();
        assert_eq!(changed, ["main.tex", "extra.tex"]);
        assert_eq!(
            std::fs::read(project.path().join("main.tex")).unwrap(),
            b"snapshot main"
        );
        assert!(!project.path().join("extra.tex").exists());
        assert!(!journal.exists());
    }

    #[test]
    fn transaction_rejects_escape() {
        let project = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let result = save_transaction(
            project.path(),
            &app_data.path().join("save.json"),
            &[TransactionWrite::new("../escape.tex", b"bad".to_vec())],
        );
        assert!(matches!(result, Err(AppError::PathOutsideRoot { .. })));
    }

    #[test]
    fn transaction_never_overwrites_an_unfinished_journal() {
        let project = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let journal = app_data.path().join("save.json");
        std::fs::write(project.path().join("main.tex"), b"old").unwrap();
        std::fs::write(&journal, b"unfinished").unwrap();
        assert!(matches!(
            save_transaction(
                project.path(),
                &journal,
                &[TransactionWrite::new("main.tex", b"new".to_vec())]
            ),
            Err(AppError::PreflightBlocked { .. })
        ));
        assert_eq!(std::fs::read(journal).unwrap(), b"unfinished");
        assert_eq!(
            std::fs::read(project.path().join("main.tex")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn startup_recovery_finishes_an_applying_transaction() {
        let project = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let recovery = app_data.path().join("recovery");
        std::fs::create_dir(&recovery).unwrap();
        let first = project.path().join("a.tex");
        let second = project.path().join("b.tex");
        std::fs::write(&first, b"old a").unwrap();
        std::fs::write(&second, b"old b").unwrap();
        let journal_path = recovery.join("save-interrupted.json");
        let journal = RecoveryJournalV1 {
            schema_version: 1,
            root: project
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            created_at: Utc::now(),
            state: RecoveryState::Applying,
            entries: vec![
                RecoveryEntry {
                    relative_path: "a.tex".into(),
                    old_bytes: Some(b"old a".to_vec()),
                    old_hash: Some(hash_bytes(b"old a")),
                    new_bytes: Some(b"new a".to_vec()),
                    new_hash: Some(hash_bytes(b"new a")),
                },
                RecoveryEntry {
                    relative_path: "b.tex".into(),
                    old_bytes: Some(b"old b".to_vec()),
                    old_hash: Some(hash_bytes(b"old b")),
                    new_bytes: Some(b"new b".to_vec()),
                    new_hash: Some(hash_bytes(b"new b")),
                },
            ],
        };
        write_journal(&journal_path, &journal).unwrap();
        // Simulate a crash after only the first replacement.
        std::fs::write(&first, b"new a").unwrap();

        let reports = recover_pending_transactions(&recovery).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].choice, RecoveryChoice::RestoreNew);
        assert_eq!(std::fs::read(first).unwrap(), b"new a");
        assert_eq!(std::fs::read(second).unwrap(), b"new b");
        assert!(!journal_path.exists());
    }

    #[test]
    fn startup_recovery_refuses_to_overwrite_an_external_third_state() {
        let project = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let recovery = app_data.path().join("recovery");
        std::fs::create_dir(&recovery).unwrap();
        let target = project.path().join("main.tex");
        std::fs::write(&target, b"externally changed").unwrap();
        let journal_path = recovery.join("save-conflict.json");
        write_journal(
            &journal_path,
            &RecoveryJournalV1 {
                schema_version: 1,
                root: project
                    .path()
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                created_at: Utc::now(),
                state: RecoveryState::Applying,
                entries: vec![RecoveryEntry {
                    relative_path: "main.tex".into(),
                    old_bytes: Some(b"old".to_vec()),
                    old_hash: Some(hash_bytes(b"old")),
                    new_bytes: Some(b"new".to_vec()),
                    new_hash: Some(hash_bytes(b"new")),
                }],
            },
        )
        .unwrap();

        assert!(matches!(
            recover_pending_transactions(&recovery),
            Err(AppError::ExternalConflict { .. })
        ));
        assert_eq!(std::fs::read(target).unwrap(), b"externally changed");
        assert!(journal_path.exists());
    }
}
