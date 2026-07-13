//! Durable, project-external history storage.
//!
//! The catalog and object store live entirely below the application-data root
//! supplied to [`DurableHistory::open`]. Snapshot creation accepts bytes rather
//! than project paths, so recording history cannot create metadata in a paper
//! directory. Restores are explicit and use the core recovery journal with the
//! journal itself kept in application data.

use crate::core::contracts::{Revision, SnapshotId};
use crate::core::error::{AppError, AppResult};
use crate::core::persistence::{TransactionWrite, save_transaction_with_deletions};
use crate::core::source::hash_bytes;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;
use walkdir::WalkDir;

const CATALOG_FILE: &str = "history.sqlite3";
const OBJECTS_DIRECTORY: &str = "objects";
const RECOVERY_DIRECTORY: &str = "recovery";
const SCHEMA_VERSION: i64 = 1;
const ZSTD_LEVEL: i32 = 7;

/// Hard bounds applied before bytes enter the history store.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageLimits {
    pub max_files_per_snapshot: usize,
    pub max_file_bytes: u64,
    pub max_snapshot_bytes: u64,
    pub max_relative_path_bytes: usize,
    pub max_project_key_bytes: usize,
    pub max_label_bytes: usize,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            max_files_per_snapshot: 10_000,
            max_file_bytes: 256 * 1024 * 1024,
            max_snapshot_bytes: 2 * 1024 * 1024 * 1024,
            max_relative_path_bytes: 1_024,
            max_project_key_bytes: 512,
            max_label_bytes: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotKind {
    Automatic,
    Named,
    PreRestore,
    PreAccept,
}

impl SnapshotKind {
    fn as_database_value(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Named => "named",
            Self::PreRestore => "pre_restore",
            Self::PreAccept => "pre_accept",
        }
    }

    fn from_database_value(value: &str) -> AppResult<Self> {
        match value {
            "automatic" => Ok(Self::Automatic),
            "named" => Ok(Self::Named),
            "pre_restore" => Ok(Self::PreRestore),
            "pre_accept" => Ok(Self::PreAccept),
            _ => Err(catalog_error(format!(
                "unknown snapshot kind in catalog: {value}"
            ))),
        }
    }

    fn is_retention_protected(self) -> bool {
        matches!(self, Self::Named | Self::PreRestore)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRecord {
    pub snapshot_id: SnapshotId,
    pub project_key: String,
    pub kind: SnapshotKind,
    pub label: Option<String>,
    pub source_revision: Option<Revision>,
    pub manifest_hash: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotOutcome {
    pub record: SnapshotRecord,
    /// `false` only when an automatic snapshot exactly matches the current
    /// history head. Named and safety snapshots still create a catalog entry,
    /// while their content objects remain deduplicated.
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotContents {
    pub record: SnapshotRecord,
    pub files: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicy {
    pub newest: usize,
    pub daily_days: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            newest: 100,
            daily_days: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RetentionReport {
    pub kept: usize,
    pub deleted_snapshot_ids: Vec<SnapshotId>,
    pub deleted_objects: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub snapshot_id: SnapshotId,
    pub written_files: Vec<String>,
    pub deleted_files: Vec<String>,
    pub unchanged_files: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStats {
    pub snapshots: u64,
    pub objects: u64,
    pub referenced_uncompressed_bytes: u64,
    pub stored_compressed_bytes: u64,
}

/// A SQLite catalog plus SHA-256 addressed zstd object store.
///
/// Methods are independent of Tauri and serialized through the connection
/// mutex. The object store is immutable: a snapshot transaction can expose all
/// of its file references or none of them, never a partial manifest.
pub struct DurableHistory {
    root: PathBuf,
    objects_root: PathBuf,
    connection: Mutex<Connection>,
    limits: StorageLimits,
}

impl std::fmt::Debug for DurableHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DurableHistory")
            .field("root", &self.root)
            .field("objects_root", &self.objects_root)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DurableHistory {
    /// Opens or initializes history below an application-data directory.
    /// Nothing outside `app_data_root` is read or written.
    pub fn open(app_data_root: impl AsRef<Path>, limits: StorageLimits) -> AppResult<Self> {
        validate_limits(limits)?;
        let root = app_data_root.as_ref();
        fs::create_dir_all(root)
            .map_err(|error| AppError::io("create history directory", root, error))?;
        let root = root
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize history directory", root, error))?;
        let objects_root = root.join(OBJECTS_DIRECTORY);
        fs::create_dir_all(&objects_root).map_err(|error| {
            AppError::io("create history object directory", &objects_root, error)
        })?;
        let objects_root = objects_root.canonicalize().map_err(|error| {
            AppError::io(
                "canonicalize history object directory",
                &objects_root,
                error,
            )
        })?;
        if !objects_root.starts_with(&root) {
            return Err(AppError::PathOutsideRoot {
                path: objects_root.to_string_lossy().into_owned(),
            });
        }

        let catalog_path = root.join(CATALOG_FILE);
        let mut connection = Connection::open(&catalog_path)
            .map_err(|error| AppError::io("open history catalog", &catalog_path, error))?;
        configure_catalog(&connection)?;
        initialize_schema(&mut connection)?;

        Ok(Self {
            root,
            objects_root,
            connection: Mutex::new(connection),
            limits,
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub const fn limits(&self) -> StorageLimits {
        self.limits
    }

    /// Records a complete relative-path to byte map.
    ///
    /// An identical automatic snapshot is a true catalog no-op. Other kinds
    /// may point at the same immutable blobs so users can name or establish a
    /// safety boundary without duplicating file content.
    pub fn create_snapshot(
        &self,
        project_key: &str,
        kind: SnapshotKind,
        label: Option<&str>,
        source_revision: Option<Revision>,
        files: &BTreeMap<String, Vec<u8>>,
        created_at: DateTime<Utc>,
    ) -> AppResult<SnapshotOutcome> {
        validate_project_key(project_key, self.limits)?;
        let label = validate_label(kind, label, self.limits)?;
        let prepared = PreparedSnapshot::new(files, self.limits)?;

        // Check the head before touching the object directory. Automatic
        // snapshots of an unchanged state leave both catalog and objects alone.
        if kind == SnapshotKind::Automatic {
            let connection = self.lock_connection()?;
            if let Some(head) = latest_snapshot(&connection, project_key)?
                && head.manifest_hash == prepared.manifest_hash
            {
                return Ok(SnapshotOutcome {
                    record: head,
                    created: false,
                });
            }
        }

        let mut stored_objects = BTreeMap::new();
        for file in &prepared.files {
            let compressed_size = self.store_object(&file.object_hash, file.bytes)?;
            stored_objects.insert(
                file.object_hash.clone(),
                (file.bytes.len() as u64, compressed_size),
            );
        }

        let snapshot_id = SnapshotId::new();
        let source_revision_value = source_revision
            .map(|revision| checked_i64(revision.0, "source revision"))
            .transpose()?;
        let file_count = checked_i64(prepared.files.len() as u64, "file count")?;
        let total_bytes = checked_i64(prepared.total_bytes, "snapshot byte count")?;
        let created_at_ms = created_at.timestamp_millis();

        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(catalog_error)?;
        for (hash, (uncompressed_size, compressed_size)) in &stored_objects {
            transaction
                .execute(
                    "INSERT INTO objects(hash, uncompressed_size, compressed_size, created_at_ms) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT(hash) DO NOTHING",
                    params![
                        hash,
                        checked_i64(*uncompressed_size, "object byte count")?,
                        checked_i64(*compressed_size, "compressed object byte count")?,
                        created_at_ms,
                    ],
                )
                .map_err(catalog_error)?;
        }
        transaction
            .execute(
                "INSERT INTO snapshots( \
                    snapshot_id, project_key, kind, label, source_revision, manifest_hash, \
                    file_count, total_bytes, created_at_ms \
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    snapshot_id.to_string(),
                    project_key,
                    kind.as_database_value(),
                    label,
                    source_revision_value,
                    prepared.manifest_hash,
                    file_count,
                    total_bytes,
                    created_at_ms,
                ],
            )
            .map_err(catalog_error)?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO snapshot_files( \
                        snapshot_id, relative_path, object_hash, byte_len \
                     ) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(catalog_error)?;
            for file in &prepared.files {
                statement
                    .execute(params![
                        snapshot_id.to_string(),
                        file.relative_path,
                        file.object_hash,
                        checked_i64(file.bytes.len() as u64, "file byte count")?,
                    ])
                    .map_err(catalog_error)?;
            }
        }
        transaction.commit().map_err(catalog_error)?;

        Ok(SnapshotOutcome {
            record: SnapshotRecord {
                snapshot_id,
                project_key: project_key.to_owned(),
                kind,
                label,
                source_revision,
                manifest_hash: prepared.manifest_hash,
                file_count: prepared.files.len(),
                total_bytes: prepared.total_bytes,
                created_at,
            },
            created: true,
        })
    }

    pub fn list_snapshots(&self, project_key: &str) -> AppResult<Vec<SnapshotRecord>> {
        validate_project_key(project_key, self.limits)?;
        let connection = self.lock_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT snapshot_id, project_key, kind, label, source_revision, \
                        manifest_hash, file_count, total_bytes, created_at_ms \
                 FROM snapshots WHERE project_key = ?1 \
                 ORDER BY created_at_ms DESC, rowid DESC",
            )
            .map_err(catalog_error)?;
        let raw_records = statement
            .query_map([project_key], raw_record_from_row)
            .map_err(catalog_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(catalog_error)?;
        raw_records.into_iter().map(decode_record).collect()
    }

    pub fn get_snapshot(&self, snapshot_id: SnapshotId) -> AppResult<SnapshotRecord> {
        let connection = self.lock_connection()?;
        snapshot_by_id(&connection, snapshot_id)?.ok_or_else(|| AppError::FileNotFound {
            path: format!("history snapshot {snapshot_id}"),
        })
    }

    /// Reads and verifies every object in a snapshot before returning bytes.
    pub fn read_snapshot(&self, snapshot_id: SnapshotId) -> AppResult<SnapshotContents> {
        let (record, files) = {
            let connection = self.lock_connection()?;
            let record = snapshot_by_id(&connection, snapshot_id)?.ok_or_else(|| {
                AppError::FileNotFound {
                    path: format!("history snapshot {snapshot_id}"),
                }
            })?;
            let mut statement = connection
                .prepare(
                    "SELECT relative_path, object_hash, byte_len \
                     FROM snapshot_files WHERE snapshot_id = ?1 \
                     ORDER BY relative_path ASC",
                )
                .map_err(catalog_error)?;
            let rows = statement
                .query_map([snapshot_id.to_string()], |row| {
                    Ok(ObjectReference {
                        relative_path: row.get(0)?,
                        object_hash: row.get(1)?,
                        byte_len: row.get(2)?,
                    })
                })
                .map_err(catalog_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(catalog_error)?;
            (record, rows)
        };

        let mut restored = BTreeMap::new();
        for reference in files {
            let expected_len = checked_u64(reference.byte_len, "stored file byte count")?;
            if expected_len > self.limits.max_file_bytes {
                return Err(catalog_error(format!(
                    "snapshot object exceeds configured file limit: {}",
                    reference.relative_path
                )));
            }
            let bytes = self.read_object(&reference.object_hash, expected_len)?;
            restored.insert(reference.relative_path, bytes);
        }
        let verified = PreparedSnapshot::new(&restored, self.limits)?;
        if verified.manifest_hash != record.manifest_hash
            || verified.total_bytes != record.total_bytes
            || verified.files.len() != record.file_count
        {
            return Err(catalog_error(format!(
                "snapshot manifest verification failed for {snapshot_id}"
            )));
        }
        Ok(SnapshotContents {
            record,
            files: restored,
        })
    }

    pub fn read_snapshot_file(
        &self,
        snapshot_id: SnapshotId,
        relative_path: &str,
    ) -> AppResult<Vec<u8>> {
        let normalized = validate_relative_path(relative_path, self.limits)?;
        let reference = {
            let connection = self.lock_connection()?;
            connection
                .query_row(
                    "SELECT object_hash, byte_len FROM snapshot_files \
                     WHERE snapshot_id = ?1 AND relative_path = ?2",
                    params![snapshot_id.to_string(), normalized],
                    |row| {
                        Ok(ObjectReference {
                            relative_path: normalized.clone(),
                            object_hash: row.get(0)?,
                            byte_len: row.get(1)?,
                        })
                    },
                )
                .optional()
                .map_err(catalog_error)?
                .ok_or_else(|| AppError::FileNotFound {
                    path: format!("history snapshot {snapshot_id}/{normalized}"),
                })?
        };
        let byte_len = checked_u64(reference.byte_len, "stored file byte count")?;
        self.read_object(&reference.object_hash, byte_len)
    }

    /// Restores the snapshot's files through a multi-file recovery journal.
    /// The journal stays below app data and is never placed in the project.
    pub fn restore_snapshot_to_directory(
        &self,
        snapshot_id: SnapshotId,
        project_root: impl AsRef<Path>,
    ) -> AppResult<RestoreReport> {
        let snapshot = self.read_snapshot(snapshot_id)?;
        let project_root = project_root.as_ref();
        let canonical_root = project_root
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize restore root", project_root, error))?;
        if !canonical_root.is_dir() {
            return Err(AppError::InvalidProject {
                message: format!("{} is not a directory", canonical_root.display()),
            });
        }

        let snapshot_paths = snapshot.files.keys().cloned().collect::<BTreeSet<_>>();
        let deletions = restore_deletions(
            &canonical_root,
            &snapshot_paths,
            self.limits.max_files_per_snapshot,
        )?;
        for relative_path in snapshot.files.keys() {
            ensure_restore_parent(&canonical_root, relative_path)?;
        }
        let writes = snapshot
            .files
            .iter()
            .map(|(relative_path, bytes)| TransactionWrite::new(relative_path, bytes.clone()))
            .collect::<Vec<_>>();
        let recovery_root = self.root.join(RECOVERY_DIRECTORY);
        fs::create_dir_all(&recovery_root).map_err(|error| {
            AppError::io("create history recovery directory", &recovery_root, error)
        })?;
        let journal_path =
            recovery_root.join(format!("restore-{}-{}.json", snapshot_id, Uuid::new_v4()));
        let changed =
            save_transaction_with_deletions(&canonical_root, &journal_path, &writes, &deletions)?;
        let deleted_set = deletions
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<BTreeSet<_>>();
        let deleted_files = changed
            .iter()
            .filter(|path| deleted_set.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        let written_files = changed
            .iter()
            .filter(|path| !deleted_set.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        let written = written_files.iter().cloned().collect::<BTreeSet<_>>();
        let unchanged_files = snapshot
            .files
            .keys()
            .filter(|relative_path| !written.contains(*relative_path))
            .cloned()
            .collect();
        Ok(RestoreReport {
            snapshot_id,
            written_files,
            deleted_files,
            unchanged_files,
        })
    }

    /// Applies the MVP policy: retain the newest 100 unprotected snapshots and
    /// one additional snapshot per UTC day for 30 days. Named and pre-restore
    /// snapshots are never removed by retention.
    pub fn enforce_default_retention(
        &self,
        project_key: &str,
        now: DateTime<Utc>,
    ) -> AppResult<RetentionReport> {
        self.enforce_retention(project_key, now, RetentionPolicy::default())
    }

    pub fn enforce_retention(
        &self,
        project_key: &str,
        now: DateTime<Utc>,
        policy: RetentionPolicy,
    ) -> AppResult<RetentionReport> {
        validate_project_key(project_key, self.limits)?;
        let snapshots = self.list_snapshots(project_key)?;
        let cutoff_day = now
            .date_naive()
            .checked_sub_days(chrono::Days::new(u64::from(policy.daily_days)))
            .unwrap_or(chrono::NaiveDate::MIN);
        let mut unprotected_index = 0usize;
        let mut daily_kept = BTreeSet::new();
        let mut delete = Vec::new();

        for snapshot in &snapshots {
            if snapshot.kind.is_retention_protected() {
                continue;
            }
            if unprotected_index < policy.newest {
                unprotected_index += 1;
                daily_kept.insert(snapshot.created_at.date_naive());
                continue;
            }
            unprotected_index += 1;
            let day = snapshot.created_at.date_naive();
            if day >= cutoff_day && daily_kept.insert(day) {
                continue;
            }
            delete.push(snapshot.snapshot_id);
        }

        let deleted_objects = self.delete_snapshots_transactional(&delete)?;
        Ok(RetentionReport {
            kept: snapshots.len().saturating_sub(delete.len()),
            deleted_snapshot_ids: delete,
            deleted_objects,
        })
    }

    pub fn delete_snapshot(&self, snapshot_id: SnapshotId) -> AppResult<bool> {
        let existed = {
            let connection = self.lock_connection()?;
            snapshot_by_id(&connection, snapshot_id)?.is_some()
        };
        if !existed {
            return Ok(false);
        }
        self.delete_snapshots_transactional(&[snapshot_id])?;
        Ok(true)
    }

    pub fn stats(&self) -> AppResult<HistoryStats> {
        let connection = self.lock_connection()?;
        let snapshots = connection
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(catalog_error)?;
        let (objects, uncompressed, compressed) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(uncompressed_size), 0),\
                        COALESCE(SUM(compressed_size), 0) FROM objects",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .map_err(catalog_error)?;
        Ok(HistoryStats {
            snapshots: checked_u64(snapshots, "snapshot count")?,
            objects: checked_u64(objects, "object count")?,
            referenced_uncompressed_bytes: checked_u64(uncompressed, "object byte count")?,
            stored_compressed_bytes: checked_u64(compressed, "compressed object byte count")?,
        })
    }

    fn lock_connection(&self) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| catalog_error("history catalog mutex was poisoned"))
    }

    fn store_object(&self, object_hash: &str, bytes: &[u8]) -> AppResult<u64> {
        let path = self.object_path(object_hash)?;
        if path.is_file() {
            let existing = self.read_object(object_hash, bytes.len() as u64)?;
            if existing != bytes {
                return Err(catalog_error(format!(
                    "history object collision for {object_hash}"
                )));
            }
            return fs::metadata(&path)
                .map(|metadata| metadata.len())
                .map_err(|error| AppError::io("inspect history object", &path, error));
        }
        let parent = path.parent().expect("object path always has a parent");
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("create object shard", parent, error))?;
        let compressed = zstd::stream::encode_all(Cursor::new(bytes), ZSTD_LEVEL)
            .map_err(|error| catalog_error(format!("compress history object: {error}")))?;
        let compressed_size = compressed.len() as u64;
        let mut temporary = tempfile::Builder::new()
            .prefix(".setwright-object-")
            .tempfile_in(parent)
            .map_err(|error| AppError::io("create temporary history object", parent, error))?;
        temporary
            .write_all(&compressed)
            .map_err(|error| AppError::io("write history object", temporary.path(), error))?;
        temporary
            .flush()
            .map_err(|error| AppError::io("flush history object", temporary.path(), error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| AppError::io("sync history object", temporary.path(), error))?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => Ok(compressed_size),
            Err(_error) if path.is_file() => fs::metadata(&path)
                .map(|metadata| metadata.len())
                .map_err(|metadata_error| {
                    AppError::io("inspect concurrent history object", &path, metadata_error)
                }),
            Err(error) => Err(AppError::io("persist history object", &path, error.error)),
        }
    }

    fn read_object(&self, object_hash: &str, expected_len: u64) -> AppResult<Vec<u8>> {
        if expected_len > self.limits.max_file_bytes {
            return Err(catalog_error(format!(
                "history object {object_hash} exceeds the configured file limit"
            )));
        }
        let path = self.object_path(object_hash)?;
        let file = fs::File::open(&path)
            .map_err(|error| AppError::io("open history object", &path, error))?;
        let decoder = zstd::stream::read::Decoder::new(file)
            .map_err(|error| catalog_error(format!("open compressed history object: {error}")))?;
        let bounded_len = expected_len.checked_add(1).ok_or_else(|| {
            catalog_error("history object length overflow while validating object")
        })?;
        let mut bytes = Vec::with_capacity(expected_len.min(16 * 1024 * 1024) as usize);
        decoder
            .take(bounded_len)
            .read_to_end(&mut bytes)
            .map_err(|error| catalog_error(format!("decompress history object: {error}")))?;
        if bytes.len() as u64 != expected_len {
            return Err(catalog_error(format!(
                "history object {object_hash} has length {}, expected {expected_len}",
                bytes.len()
            )));
        }
        let actual_hash = hash_bytes(&bytes);
        if actual_hash != object_hash {
            return Err(AppError::HashMismatch {
                expected: object_hash.to_owned(),
                actual: actual_hash,
            });
        }
        Ok(bytes)
    }

    fn object_path(&self, object_hash: &str) -> AppResult<PathBuf> {
        if object_hash.len() != 64
            || !object_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(catalog_error("invalid object hash in history catalog"));
        }
        Ok(self
            .objects_root
            .join(&object_hash[..2])
            .join(format!("{}.zst", &object_hash[2..])))
    }

    fn delete_snapshots_transactional(&self, snapshot_ids: &[SnapshotId]) -> AppResult<usize> {
        if snapshot_ids.is_empty() {
            return Ok(0);
        }
        let unreferenced = {
            let mut connection = self.lock_connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(catalog_error)?;
            for snapshot_id in snapshot_ids {
                transaction
                    .execute(
                        "DELETE FROM snapshots WHERE snapshot_id = ?1",
                        [snapshot_id.to_string()],
                    )
                    .map_err(catalog_error)?;
            }
            let unreferenced = query_unreferenced_objects(&transaction)?;
            transaction
                .execute(
                    "DELETE FROM objects \
                     WHERE NOT EXISTS ( \
                         SELECT 1 FROM snapshot_files \
                         WHERE snapshot_files.object_hash = objects.hash \
                     )",
                    [],
                )
                .map_err(catalog_error)?;
            transaction.commit().map_err(catalog_error)?;
            unreferenced
        };

        // Catalog integrity does not depend on physical orphan cleanup. If a
        // crash happens here, a later maintenance pass can remove the same
        // content-addressed orphan without affecting any snapshot.
        for object_hash in &unreferenced {
            let path = self.object_path(object_hash)?;
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AppError::io(
                        "remove unreferenced history object",
                        &path,
                        error,
                    ));
                }
            }
        }
        Ok(unreferenced.len())
    }
}

#[derive(Debug)]
struct PreparedFile<'a> {
    relative_path: String,
    object_hash: String,
    bytes: &'a [u8],
}

#[derive(Debug)]
struct PreparedSnapshot<'a> {
    files: Vec<PreparedFile<'a>>,
    manifest_hash: String,
    total_bytes: u64,
}

impl<'a> PreparedSnapshot<'a> {
    fn new(files: &'a BTreeMap<String, Vec<u8>>, limits: StorageLimits) -> AppResult<Self> {
        if files.is_empty() {
            return Err(AppError::InvalidProject {
                message: "a history snapshot must contain at least one file".into(),
            });
        }
        if files.len() > limits.max_files_per_snapshot {
            return Err(AppError::InvalidProject {
                message: format!(
                    "snapshot contains {} files; limit is {}",
                    files.len(),
                    limits.max_files_per_snapshot
                ),
            });
        }
        let mut prepared = Vec::with_capacity(files.len());
        let mut normalized_paths = BTreeSet::new();
        let mut total_bytes = 0u64;
        for (relative_path, bytes) in files {
            let relative_path = validate_relative_path(relative_path, limits)?;
            if !normalized_paths.insert(relative_path.clone()) {
                return Err(AppError::InvalidPath {
                    path: relative_path,
                    message: "two snapshot paths normalize to the same file".into(),
                });
            }
            if bytes.len() as u64 > limits.max_file_bytes {
                return Err(AppError::InvalidProject {
                    message: format!(
                        "{relative_path} is {} bytes; per-file limit is {}",
                        bytes.len(),
                        limits.max_file_bytes
                    ),
                });
            }
            total_bytes = total_bytes.checked_add(bytes.len() as u64).ok_or_else(|| {
                AppError::InvalidProject {
                    message: "snapshot byte count overflow".into(),
                }
            })?;
            if total_bytes > limits.max_snapshot_bytes {
                return Err(AppError::InvalidProject {
                    message: format!(
                        "snapshot is {total_bytes} bytes; limit is {}",
                        limits.max_snapshot_bytes
                    ),
                });
            }
            prepared.push(PreparedFile {
                relative_path,
                object_hash: hash_bytes(bytes),
                bytes,
            });
        }
        prepared.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let manifest_hash = manifest_hash(&prepared);
        Ok(Self {
            files: prepared,
            manifest_hash,
            total_bytes,
        })
    }
}

#[derive(Debug)]
struct ObjectReference {
    relative_path: String,
    object_hash: String,
    byte_len: i64,
}

#[derive(Debug)]
struct RawSnapshotRecord {
    snapshot_id: String,
    project_key: String,
    kind: String,
    label: Option<String>,
    source_revision: Option<i64>,
    manifest_hash: String,
    file_count: i64,
    total_bytes: i64,
    created_at_ms: i64,
}

fn configure_catalog(connection: &Connection) -> AppResult<()> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(catalog_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(catalog_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(catalog_error)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(catalog_error)
}

fn initialize_schema(connection: &mut Connection) -> AppResult<()> {
    let current_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(catalog_error)?;
    if current_version > SCHEMA_VERSION {
        return Err(catalog_error(format!(
            "history schema {current_version} is newer than supported schema {SCHEMA_VERSION}"
        )));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(catalog_error)?;
    transaction
        .execute_batch(
            r#"CREATE TABLE IF NOT EXISTS objects (
                hash TEXT PRIMARY KEY NOT NULL,
                uncompressed_size INTEGER NOT NULL CHECK(uncompressed_size >= 0),
                compressed_size INTEGER NOT NULL CHECK(compressed_size >= 0),
                created_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS snapshots (
                snapshot_id TEXT PRIMARY KEY NOT NULL,
                project_key TEXT NOT NULL,
                kind TEXT NOT NULL CHECK(kind IN ('automatic','named','pre_restore','pre_accept')),
                label TEXT,
                source_revision INTEGER CHECK(source_revision IS NULL OR source_revision >= 0),
                manifest_hash TEXT NOT NULL,
                file_count INTEGER NOT NULL CHECK(file_count > 0),
                total_bytes INTEGER NOT NULL CHECK(total_bytes >= 0),
                created_at_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS snapshots_project_time
                ON snapshots(project_key, created_at_ms DESC);
             CREATE TABLE IF NOT EXISTS snapshot_files (
                snapshot_id TEXT NOT NULL REFERENCES snapshots(snapshot_id) ON DELETE CASCADE,
                relative_path TEXT NOT NULL,
                object_hash TEXT NOT NULL REFERENCES objects(hash),
                byte_len INTEGER NOT NULL CHECK(byte_len >= 0),
                PRIMARY KEY(snapshot_id, relative_path)
             );
             CREATE INDEX IF NOT EXISTS snapshot_files_object
                ON snapshot_files(object_hash);"#,
        )
        .map_err(catalog_error)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(catalog_error)?;
    transaction.commit().map_err(catalog_error)
}

fn latest_snapshot(
    connection: &Connection,
    project_key: &str,
) -> AppResult<Option<SnapshotRecord>> {
    let raw = connection
        .query_row(
            "SELECT snapshot_id, project_key, kind, label, source_revision, manifest_hash, \
                    file_count, total_bytes, created_at_ms \
             FROM snapshots WHERE project_key = ?1 \
             ORDER BY created_at_ms DESC, rowid DESC LIMIT 1",
            [project_key],
            raw_record_from_row,
        )
        .optional()
        .map_err(catalog_error)?;
    raw.map(decode_record).transpose()
}

fn snapshot_by_id(
    connection: &Connection,
    snapshot_id: SnapshotId,
) -> AppResult<Option<SnapshotRecord>> {
    let raw = connection
        .query_row(
            "SELECT snapshot_id, project_key, kind, label, source_revision, manifest_hash, \
                    file_count, total_bytes, created_at_ms \
             FROM snapshots WHERE snapshot_id = ?1",
            [snapshot_id.to_string()],
            raw_record_from_row,
        )
        .optional()
        .map_err(catalog_error)?;
    raw.map(decode_record).transpose()
}

fn raw_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSnapshotRecord> {
    Ok(RawSnapshotRecord {
        snapshot_id: row.get(0)?,
        project_key: row.get(1)?,
        kind: row.get(2)?,
        label: row.get(3)?,
        source_revision: row.get(4)?,
        manifest_hash: row.get(5)?,
        file_count: row.get(6)?,
        total_bytes: row.get(7)?,
        created_at_ms: row.get(8)?,
    })
}

fn decode_record(raw: RawSnapshotRecord) -> AppResult<SnapshotRecord> {
    let snapshot_id = Uuid::parse_str(&raw.snapshot_id)
        .map(SnapshotId::from)
        .map_err(|error| catalog_error(format!("invalid snapshot id in catalog: {error}")))?;
    let created_at =
        DateTime::<Utc>::from_timestamp_millis(raw.created_at_ms).ok_or_else(|| {
            catalog_error(format!(
                "invalid snapshot timestamp in catalog: {}",
                raw.created_at_ms
            ))
        })?;
    Ok(SnapshotRecord {
        snapshot_id,
        project_key: raw.project_key,
        kind: SnapshotKind::from_database_value(&raw.kind)?,
        label: raw.label,
        source_revision: raw
            .source_revision
            .map(|revision| checked_u64(revision, "source revision").map(Revision))
            .transpose()?,
        manifest_hash: raw.manifest_hash,
        file_count: usize::try_from(checked_u64(raw.file_count, "file count")?)
            .map_err(|_| catalog_error("file count does not fit this platform"))?,
        total_bytes: checked_u64(raw.total_bytes, "snapshot byte count")?,
        created_at,
    })
}

fn query_unreferenced_objects(transaction: &Transaction<'_>) -> AppResult<Vec<String>> {
    let mut statement = transaction
        .prepare(
            "SELECT hash FROM objects \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM snapshot_files \
                 WHERE snapshot_files.object_hash = objects.hash \
             )",
        )
        .map_err(catalog_error)?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(catalog_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(catalog_error)
}

fn restore_deletions(
    root: &Path,
    snapshot_paths: &BTreeSet<String>,
    max_files: usize,
) -> AppResult<Vec<PathBuf>> {
    let mut current_files = 0usize;
    let mut deletions = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .path()
                    .strip_prefix(root)
                    .is_ok_and(|relative| !excluded_history_path(relative))
        })
    {
        let entry = entry.map_err(|error| AppError::InvalidProject {
            message: format!("could not inventory project before history restore: {error}"),
        })?;
        if entry.depth() == 0 || entry.file_type().is_dir() {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::ExternalConflict {
                path: entry.path().to_string_lossy().into_owned(),
            });
        }
        if !entry.file_type().is_file() {
            continue;
        }
        current_files = current_files.saturating_add(1);
        if current_files > max_files {
            return Err(AppError::InvalidProject {
                message: format!(
                    "project exceeds the {max_files} file limit for an exact history restore"
                ),
            });
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| AppError::PathOutsideRoot {
                path: entry.path().to_string_lossy().into_owned(),
            })?;
        let portable = relative.to_string_lossy().replace('\\', "/");
        if !snapshot_paths.contains(&portable) {
            deletions.push(PathBuf::from(portable));
        }
    }
    deletions.sort();
    Ok(deletions)
}

/// Paths excluded while capturing history must also be excluded while finding
/// restore deletions. Otherwise a snapshot that intentionally omits a build or
/// dependency tree would make restore destructively delete that tree.
pub(crate) fn excluded_history_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if name.eq_ignore_ascii_case(".git")
                    || name.eq_ignore_ascii_case(".hg")
                    || name.eq_ignore_ascii_case(".svn")
                    || name.eq_ignore_ascii_case("node_modules")
                    || name.eq_ignore_ascii_case("target")
        )
    })
}

fn ensure_restore_parent(root: &Path, relative_path: &str) -> AppResult<()> {
    let path = Path::new(relative_path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(segment) = component else {
            return Err(AppError::PathOutsideRoot {
                path: relative_path.to_owned(),
            });
        };
        current.push(segment);
        if current.exists() {
            let metadata = fs::symlink_metadata(&current)
                .map_err(|error| AppError::io("inspect restore directory", &current, error))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AppError::InvalidPath {
                    path: current.to_string_lossy().into_owned(),
                    message: "restore parent must be a real directory, not a file or symlink"
                        .into(),
                });
            }
        } else {
            fs::create_dir(&current)
                .map_err(|error| AppError::io("create restore directory", &current, error))?;
        }
        let canonical = current
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize restore directory", &current, error))?;
        if !canonical.starts_with(root) {
            return Err(AppError::PathOutsideRoot {
                path: canonical.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(())
}

fn validate_limits(limits: StorageLimits) -> AppResult<()> {
    if limits.max_files_per_snapshot == 0
        || limits.max_file_bytes == 0
        || limits.max_snapshot_bytes == 0
        || limits.max_relative_path_bytes == 0
        || limits.max_project_key_bytes == 0
        || limits.max_label_bytes == 0
    {
        return Err(AppError::InvalidProject {
            message: "history storage limits must all be greater than zero".into(),
        });
    }
    if limits.max_file_bytes > limits.max_snapshot_bytes {
        return Err(AppError::InvalidProject {
            message: "per-file history limit cannot exceed total snapshot limit".into(),
        });
    }
    Ok(())
}

fn validate_project_key(project_key: &str, limits: StorageLimits) -> AppResult<()> {
    if project_key.is_empty()
        || project_key.len() > limits.max_project_key_bytes
        || project_key.contains('\0')
    {
        return Err(AppError::InvalidProject {
            message: "history project key is empty, too long, or contains NUL".into(),
        });
    }
    Ok(())
}

fn validate_label(
    kind: SnapshotKind,
    label: Option<&str>,
    limits: StorageLimits,
) -> AppResult<Option<String>> {
    let normalized = label.map(str::trim).filter(|label| !label.is_empty());
    if kind == SnapshotKind::Named && normalized.is_none() {
        return Err(AppError::InvalidProject {
            message: "named snapshots require a non-empty label".into(),
        });
    }
    if let Some(label) = normalized
        && (label.len() > limits.max_label_bytes || label.contains('\0'))
    {
        return Err(AppError::InvalidProject {
            message: "history snapshot label is too long or contains NUL".into(),
        });
    }
    Ok(normalized.map(ToOwned::to_owned))
}

fn validate_relative_path(path: &str, limits: StorageLimits) -> AppResult<String> {
    if path.is_empty() || path.len() > limits.max_relative_path_bytes || path.contains('\0') {
        return Err(AppError::InvalidPath {
            path: path.to_owned(),
            message: "snapshot path is empty, too long, or contains NUL".into(),
        });
    }
    let portable = path.replace('\\', "/");
    if portable.starts_with('/')
        || portable.starts_with("//")
        || portable
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
    {
        return Err(AppError::PathOutsideRoot {
            path: path.to_owned(),
        });
    }
    let mut components = Vec::new();
    for component in portable.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(AppError::PathOutsideRoot {
                path: path.to_owned(),
            });
        }
        if component
            .bytes()
            .any(|byte| matches!(byte, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*'))
            || component.ends_with('.')
            || component.ends_with(' ')
            || is_windows_reserved_name(component)
        {
            return Err(AppError::InvalidPath {
                path: path.to_owned(),
                message: "snapshot path is not portable across supported platforms".into(),
            });
        }
        components.push(component);
    }
    let normalized = components.join("/");
    if normalized.len() > limits.max_relative_path_bytes {
        return Err(AppError::InvalidPath {
            path: path.to_owned(),
            message: "normalized snapshot path is too long".into(),
        });
    }
    Ok(normalized)
}

fn is_windows_reserved_name(component: &str) -> bool {
    let stem = component
        .split_once('.')
        .map_or(component, |(stem, _extension)| stem)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

fn manifest_hash(files: &[PreparedFile<'_>]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"setwright-history-manifest-v1\0");
    hasher.update((files.len() as u64).to_be_bytes());
    for file in files {
        hasher.update((file.relative_path.len() as u64).to_be_bytes());
        hasher.update(file.relative_path.as_bytes());
        hasher.update((file.bytes.len() as u64).to_be_bytes());
        hasher.update(file.object_hash.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn checked_i64(value: u64, description: &str) -> AppResult<i64> {
    i64::try_from(value)
        .map_err(|_| catalog_error(format!("{description} does not fit the catalog format")))
}

fn checked_u64(value: i64, description: &str) -> AppResult<u64> {
    u64::try_from(value).map_err(|_| catalog_error(format!("negative {description} in catalog")))
}

fn catalog_error(error: impl std::fmt::Display) -> AppError {
    AppError::Serialization {
        message: format!("history catalog: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn history() -> (tempfile::TempDir, DurableHistory) {
        let directory = tempfile::tempdir().unwrap();
        let history = DurableHistory::open(directory.path(), StorageLimits::default()).unwrap();
        (directory, history)
    }

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn files(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([("main.tex".to_owned(), bytes.to_vec())])
    }

    #[test]
    fn content_objects_are_deduplicated_across_snapshots_and_paths() {
        let (_directory, history) = history();
        let same = b"shared bytes".to_vec();
        let first_files = BTreeMap::from([
            ("main.tex".to_owned(), same.clone()),
            ("copy.tex".to_owned(), same.clone()),
        ]);
        history
            .create_snapshot(
                "project-a",
                SnapshotKind::PreAccept,
                None,
                Some(Revision(1)),
                &first_files,
                at(1, 1),
            )
            .unwrap();
        history
            .create_snapshot(
                "project-a",
                SnapshotKind::PreAccept,
                None,
                Some(Revision(2)),
                &first_files,
                at(1, 2),
            )
            .unwrap();
        let stats = history.stats().unwrap();
        assert_eq!(stats.snapshots, 2);
        assert_eq!(stats.objects, 1);
        assert_eq!(stats.referenced_uncompressed_bytes, same.len() as u64);
    }

    #[test]
    fn restore_preserves_bom_crlf_and_binary_bytes_exactly() {
        let (_directory, history) = history();
        let original = BTreeMap::from([
            (
                "main.tex".to_owned(),
                b"\xEF\xBB\xBF\\documentclass{article}\r\n% comment\r\n".to_vec(),
            ),
            (
                "figures/data.bin".to_owned(),
                vec![0, 255, 16, 13, 10, 0, 128, 42],
            ),
        ]);
        let snapshot = history
            .create_snapshot(
                "project-a",
                SnapshotKind::PreRestore,
                None,
                Some(Revision(7)),
                &original,
                at(1, 1),
            )
            .unwrap();
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join("figures")).unwrap();
        fs::create_dir(project.path().join(".git")).unwrap();
        fs::create_dir(project.path().join("node_modules")).unwrap();
        fs::create_dir(project.path().join("target")).unwrap();
        fs::write(project.path().join("main.tex"), b"changed\n").unwrap();
        fs::write(project.path().join("figures/data.bin"), b"changed").unwrap();
        fs::write(
            project.path().join("added-after-snapshot.tex"),
            b"remove me",
        )
        .unwrap();
        fs::write(project.path().join(".git/config"), b"preserve me").unwrap();
        fs::write(
            project.path().join("node_modules/package.js"),
            b"preserve dependency tree",
        )
        .unwrap();
        fs::write(
            project.path().join("target/build.bin"),
            b"preserve build tree",
        )
        .unwrap();

        let report = history
            .restore_snapshot_to_directory(snapshot.record.snapshot_id, project.path())
            .unwrap();
        assert_eq!(report.written_files.len(), 2);
        assert_eq!(report.deleted_files, ["added-after-snapshot.tex"]);
        assert_eq!(
            fs::read(project.path().join("main.tex")).unwrap(),
            original["main.tex"]
        );
        assert_eq!(
            fs::read(project.path().join("figures/data.bin")).unwrap(),
            original["figures/data.bin"]
        );
        assert!(!project.path().join(".setwright").exists());
        assert!(!project.path().join("paper-settings.json").exists());
        assert!(!project.path().join("added-after-snapshot.tex").exists());
        assert_eq!(
            fs::read(project.path().join(".git/config")).unwrap(),
            b"preserve me"
        );
        assert_eq!(
            fs::read(project.path().join("node_modules/package.js")).unwrap(),
            b"preserve dependency tree"
        );
        assert_eq!(
            fs::read(project.path().join("target/build.bin")).unwrap(),
            b"preserve build tree"
        );
    }

    #[test]
    fn identical_automatic_snapshot_is_a_catalog_noop() {
        let (_directory, history) = history();
        let contents = files(b"same bytes");
        let first = history
            .create_snapshot(
                "project-a",
                SnapshotKind::Automatic,
                None,
                Some(Revision(1)),
                &contents,
                at(1, 1),
            )
            .unwrap();
        let second = history
            .create_snapshot(
                "project-a",
                SnapshotKind::Automatic,
                None,
                Some(Revision(2)),
                &contents,
                at(1, 2),
            )
            .unwrap();
        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.record.snapshot_id, second.record.snapshot_id);
        assert_eq!(history.stats().unwrap().snapshots, 1);
    }

    #[test]
    fn retention_keeps_newest_and_one_older_snapshot_per_recent_day() {
        let (_directory, history) = history();
        let moments = [
            at(1, 1),
            at(1, 2),
            at(2, 1),
            at(2, 2),
            at(3, 1),
            at(3, 2),
            at(4, 1),
        ];
        for (index, moment) in moments.into_iter().enumerate() {
            history
                .create_snapshot(
                    "project-a",
                    SnapshotKind::Automatic,
                    None,
                    Some(Revision(index as u64)),
                    &files(format!("version {index}").as_bytes()),
                    moment,
                )
                .unwrap();
        }
        let report = history
            .enforce_retention(
                "project-a",
                at(4, 12),
                RetentionPolicy {
                    newest: 2,
                    daily_days: 2,
                },
            )
            .unwrap();
        // Newest two are day 4 and the newest from day 3. Daily retention adds
        // one from day 2; older duplicates and day 1 snapshots are removed.
        assert_eq!(report.kept, 3);
        let remaining = history.list_snapshots("project-a").unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(
            remaining
                .iter()
                .map(|snapshot| snapshot.created_at.date_naive())
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn named_and_pre_restore_snapshots_survive_retention() {
        let (_directory, history) = history();
        let named = history
            .create_snapshot(
                "project-a",
                SnapshotKind::Named,
                Some("submission draft"),
                None,
                &files(b"named"),
                at(1, 1),
            )
            .unwrap();
        let pre_restore = history
            .create_snapshot(
                "project-a",
                SnapshotKind::PreRestore,
                None,
                None,
                &files(b"restore"),
                at(1, 2),
            )
            .unwrap();
        for index in 0..3 {
            history
                .create_snapshot(
                    "project-a",
                    SnapshotKind::Automatic,
                    None,
                    None,
                    &files(format!("automatic {index}").as_bytes()),
                    at(2, index + 1),
                )
                .unwrap();
        }
        history
            .enforce_retention(
                "project-a",
                at(4, 1),
                RetentionPolicy {
                    newest: 0,
                    daily_days: 0,
                },
            )
            .unwrap();
        let remaining = history.list_snapshots("project-a").unwrap();
        let ids = remaining
            .iter()
            .map(|snapshot| snapshot.snapshot_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(remaining.len(), 2);
        assert!(ids.contains(&named.record.snapshot_id));
        assert!(ids.contains(&pre_restore.record.snapshot_id));
    }

    #[test]
    fn snapshot_creation_never_writes_the_project_directory() {
        let (_directory, history) = history();
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("main.tex"), b"paper").unwrap();
        let before = fs::read_dir(project.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        history
            .create_snapshot(
                "project-a",
                SnapshotKind::Automatic,
                None,
                None,
                &files(b"paper"),
                at(1, 1),
            )
            .unwrap();
        let after = fs::read_dir(project.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(before, after);
    }

    #[test]
    fn paths_and_sizes_are_validated_before_catalog_writes() {
        let directory = tempfile::tempdir().unwrap();
        let history = DurableHistory::open(
            directory.path(),
            StorageLimits {
                max_file_bytes: 4,
                max_snapshot_bytes: 4,
                ..StorageLimits::default()
            },
        )
        .unwrap();
        let escaped = BTreeMap::from([("../escape.tex".to_owned(), b"x".to_vec())]);
        assert!(matches!(
            history.create_snapshot(
                "project-a",
                SnapshotKind::Automatic,
                None,
                None,
                &escaped,
                at(1, 1)
            ),
            Err(AppError::PathOutsideRoot { .. })
        ));
        assert!(
            history
                .create_snapshot(
                    "project-a",
                    SnapshotKind::Automatic,
                    None,
                    None,
                    &files(b"12345"),
                    at(1, 1)
                )
                .is_err()
        );
        let alternate_stream = BTreeMap::from([("main.tex:review".to_owned(), b"x".to_vec())]);
        assert!(matches!(
            history.create_snapshot(
                "project-a",
                SnapshotKind::Automatic,
                None,
                None,
                &alternate_stream,
                at(1, 1)
            ),
            Err(AppError::InvalidPath { .. })
        ));
        assert_eq!(history.stats().unwrap().snapshots, 0);
    }
}
