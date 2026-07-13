//! Content-addressed snapshot object primitives.
//!
//! SQLite metadata and scheduling belong to the application storage service;
//! this module owns exact-byte hashing, zstd objects, capture manifests, and
//! retention selection so those invariants remain reusable and testable.

use crate::core::contracts::{ProjectId, Revision, SnapshotId};
use crate::core::error::{AppError, AppResult};
use crate::core::latex::{normalized_relative, safe_relative_path};
use crate::core::persistence::atomic_write;
use crate::core::source::hash_bytes;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};

const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotObjectRef {
    pub sha256: String,
    pub byte_length: u64,
    pub compressed_byte_length: u64,
}

#[derive(Debug, Clone)]
pub struct SnapshotObjectStore {
    root: PathBuf,
}

impl SnapshotObjectStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Stores the exact bytes under their SHA-256. No directories are created
    /// until the first object is actually written.
    pub fn put(&self, bytes: &[u8]) -> AppResult<SnapshotObjectRef> {
        let sha256 = hash_bytes(bytes);
        validate_sha256(&sha256)?;
        let path = self.object_path(&sha256)?;
        if path.exists() {
            let decoded = self.get(&sha256)?;
            if decoded != bytes {
                return Err(AppError::Serialization {
                    message: format!("snapshot object collision or corruption: {sha256}"),
                });
            }
            return Ok(SnapshotObjectRef {
                sha256,
                byte_length: bytes.len() as u64,
                compressed_byte_length: std::fs::metadata(&path)
                    .map_err(|error| AppError::io("inspect snapshot object", &path, error))?
                    .len(),
            });
        }
        let compressed =
            zstd::stream::encode_all(Cursor::new(bytes), ZSTD_LEVEL).map_err(|error| {
                AppError::Serialization {
                    message: format!("zstd compression failed: {error}"),
                }
            })?;
        let parent = path.parent().expect("object path has a parent");
        std::fs::create_dir_all(parent)
            .map_err(|error| AppError::io("create object directory", parent, error))?;
        atomic_write(&path, &compressed)?;
        Ok(SnapshotObjectRef {
            sha256,
            byte_length: bytes.len() as u64,
            compressed_byte_length: compressed.len() as u64,
        })
    }

    pub fn get(&self, sha256: &str) -> AppResult<Vec<u8>> {
        let path = self.object_path(sha256)?;
        let compressed = std::fs::read(&path)
            .map_err(|error| AppError::io("read snapshot object", &path, error))?;
        let decoded = zstd::stream::decode_all(Cursor::new(compressed)).map_err(|error| {
            AppError::Serialization {
                message: format!("zstd decompression failed for {sha256}: {error}"),
            }
        })?;
        let actual = hash_bytes(&decoded);
        if actual != sha256 {
            return Err(AppError::HashMismatch {
                expected: sha256.into(),
                actual,
            });
        }
        Ok(decoded)
    }

    pub fn contains(&self, sha256: &str) -> AppResult<bool> {
        Ok(self.object_path(sha256)?.is_file())
    }

    fn object_path(&self, sha256: &str) -> AppResult<PathBuf> {
        validate_sha256(sha256)?;
        Ok(self
            .root
            .join("objects")
            .join(&sha256[..2])
            .join(format!("{}.zst", &sha256[2..])))
    }
}

fn validate_sha256(value: &str) -> AppResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(AppError::InvalidPath {
            path: value.into(),
            message: "expected a lowercase SHA-256 digest".into(),
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotKind {
    Automatic,
    Named,
    PreRestore,
    PreAcceptSuggestion,
}

impl SnapshotKind {
    #[must_use]
    pub const fn is_pinned(self) -> bool {
        matches!(self, Self::Named | Self::PreRestore)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFile {
    pub relative_path: String,
    pub object: SnapshotObjectRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotManifest {
    pub snapshot_id: SnapshotId,
    pub project_id: Option<ProjectId>,
    pub revision: Revision,
    pub kind: SnapshotKind,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub project_hash: String,
    pub files: Vec<SnapshotFile>,
}

impl SnapshotManifest {
    pub fn capture<'a>(
        store: &SnapshotObjectStore,
        project_id: Option<ProjectId>,
        revision: Revision,
        kind: SnapshotKind,
        name: Option<String>,
        files: impl IntoIterator<Item = (&'a Path, &'a [u8])>,
        created_at: DateTime<Utc>,
    ) -> AppResult<Self> {
        if kind == SnapshotKind::Named && name.as_ref().is_none_or(|value| value.trim().is_empty())
        {
            return Err(AppError::InvalidProject {
                message: "named snapshot requires a non-empty name".into(),
            });
        }
        let mut seen = BTreeSet::new();
        let mut captured = Vec::new();
        for (relative_path, bytes) in files {
            let relative = safe_relative_path(&relative_path.to_string_lossy())?;
            let normalized = normalized_relative(&relative);
            if !seen.insert(normalized.clone()) {
                return Err(AppError::InvalidProject {
                    message: format!("duplicate snapshot path: {normalized}"),
                });
            }
            captured.push(SnapshotFile {
                relative_path: normalized,
                object: store.put(bytes)?,
            });
        }
        if captured.is_empty() {
            return Err(AppError::InvalidProject {
                message: "cannot capture an empty project snapshot".into(),
            });
        }
        captured.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let project_hash = hash_snapshot_files(&captured);
        Ok(Self {
            snapshot_id: SnapshotId::new(),
            project_id,
            revision,
            kind,
            name,
            created_at,
            project_hash,
            files: captured,
        })
    }

    pub fn restore_files(
        &self,
        store: &SnapshotObjectStore,
    ) -> AppResult<BTreeMap<String, Vec<u8>>> {
        self.files
            .iter()
            .map(|file| {
                store
                    .get(&file.object.sha256)
                    .map(|bytes| (file.relative_path.clone(), bytes))
            })
            .collect()
    }
}

fn hash_snapshot_files(files: &[SnapshotFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update((file.relative_path.len() as u64).to_le_bytes());
        hasher.update(file.relative_path.as_bytes());
        hasher.update((file.object.byte_length).to_le_bytes());
        hasher.update(file.object.sha256.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Returns automatic snapshot IDs that can be deleted. Pinned kinds are never
/// selected. The newest 100 automatic snapshots and the newest snapshot for
/// each of the last 30 UTC dates are retained.
#[must_use]
pub fn snapshots_to_prune(snapshots: &[SnapshotManifest], now: DateTime<Utc>) -> Vec<SnapshotId> {
    let mut automatic: Vec<_> = snapshots
        .iter()
        .filter(|snapshot| !snapshot.kind.is_pinned())
        .collect();
    automatic.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
    });

    let mut keep = BTreeSet::new();
    keep.extend(
        automatic
            .iter()
            .take(100)
            .map(|snapshot| snapshot.snapshot_id),
    );
    let earliest_date = (now - Duration::days(29)).date_naive();
    let mut kept_dates = BTreeSet::<NaiveDate>::new();
    for snapshot in &automatic {
        let date = snapshot.created_at.date_naive();
        if date >= earliest_date && date <= now.date_naive() && kept_dates.insert(date) {
            keep.insert(snapshot.snapshot_id);
        }
    }
    automatic
        .into_iter()
        .filter(|snapshot| !keep.contains(&snapshot.snapshot_id))
        .map(|snapshot| snapshot.snapshot_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_store_round_trips_and_deduplicates() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotObjectStore::new(directory.path());
        let first = store.put(b"exact bytes\r\n").unwrap();
        let second = store.put(b"exact bytes\r\n").unwrap();
        assert_eq!(first, second);
        assert!(first.compressed_byte_length > 0);
        assert_eq!(store.get(&first.sha256).unwrap(), b"exact bytes\r\n");
    }

    #[test]
    fn manifest_sorts_paths_and_restores_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotObjectStore::new(directory.path());
        let manifest = SnapshotManifest::capture(
            &store,
            None,
            Revision(4),
            SnapshotKind::Automatic,
            None,
            [
                (Path::new("z.tex"), b"z".as_slice()),
                (Path::new("a.tex"), b"a".as_slice()),
            ],
            Utc::now(),
        )
        .unwrap();
        assert_eq!(manifest.files[0].relative_path, "a.tex");
        assert_eq!(manifest.restore_files(&store).unwrap()["z.tex"], b"z");
    }

    #[test]
    fn retention_never_prunes_named_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotObjectStore::new(directory.path());
        let now = Utc::now();
        let mut snapshots = Vec::new();
        for index in 0..110 {
            snapshots.push(
                SnapshotManifest::capture(
                    &store,
                    None,
                    Revision(index),
                    SnapshotKind::Automatic,
                    None,
                    [(Path::new("main.tex"), format!("{index}").as_bytes())],
                    now - Duration::minutes(index as i64),
                )
                .unwrap(),
            );
        }
        let named = SnapshotManifest::capture(
            &store,
            None,
            Revision(0),
            SnapshotKind::Named,
            Some("keep".into()),
            [(Path::new("main.tex"), b"named".as_slice())],
            now - Duration::days(100),
        )
        .unwrap();
        let named_id = named.snapshot_id;
        snapshots.push(named);
        let pruned = snapshots_to_prune(&snapshots, now);
        assert!(!pruned.contains(&named_id));
        assert!(!pruned.is_empty());
    }

    #[test]
    fn pre_accept_suggestion_snapshot_can_age_out() {
        let directory = tempfile::tempdir().unwrap();
        let store = SnapshotObjectStore::new(directory.path());
        let now = Utc::now();
        let template = SnapshotManifest::capture(
            &store,
            None,
            Revision(0),
            SnapshotKind::Automatic,
            None,
            [(Path::new("main.tex"), b"same".as_slice())],
            now,
        )
        .unwrap();
        let mut snapshots = Vec::new();
        for index in 0..101u64 {
            let mut snapshot = template.clone();
            snapshot.snapshot_id = SnapshotId::new();
            snapshot.revision = Revision(index);
            snapshot.created_at = now - Duration::minutes(index as i64);
            snapshots.push(snapshot);
        }
        let mut pre_accept = template;
        pre_accept.snapshot_id = SnapshotId::new();
        pre_accept.kind = SnapshotKind::PreAcceptSuggestion;
        pre_accept.created_at = now - Duration::days(100);
        let pre_accept_id = pre_accept.snapshot_id;
        snapshots.push(pre_accept);
        assert!(snapshots_to_prune(&snapshots, now).contains(&pre_accept_id));
    }
}
