//! Portable review and suggestion primitives.

use crate::core::contracts::{
    FileId, ProjectId, ReviewAnchor, ReviewBaseFile, ReviewBundleV1, ReviewIdentity, ReviewMessage,
    ReviewSuggestion, ReviewThread, ReviewThreadStatus, SourceEdit, SuggestionStatus,
};
use crate::core::error::{AppError, AppResult};
use crate::core::latex::{normalized_relative, safe_relative_path};
use crate::core::persistence::atomic_write;
use crate::core::source::hash_bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const CONTEXT_BYTES: usize = 96;
const MAX_REVIEW_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFileInput {
    pub file_id: String,
    pub relative_path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ReviewWorkspace {
    bundle: ReviewBundleV1,
    base_sources: BTreeMap<String, Vec<u8>>,
}

impl ReviewWorkspace {
    pub fn new(
        project_id: ProjectId,
        reviewer: ReviewIdentity,
        files: Vec<ReviewFileInput>,
        exported_at: DateTime<Utc>,
    ) -> AppResult<Self> {
        if files.is_empty() {
            return Err(AppError::InvalidProject {
                message: "review bundle requires at least one base file".into(),
            });
        }
        let mut sources = BTreeMap::new();
        let mut base_files = Vec::with_capacity(files.len());
        let mut project_hasher = Sha256::new();
        let mut files = files;
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        for file in files {
            let relative = safe_relative_path(&file.relative_path.to_string_lossy())?;
            let path = normalized_relative(&relative);
            if file.file_id.is_empty() || file.file_id.len() > 128 {
                return Err(AppError::InvalidProject {
                    message: "review file id must contain 1 to 128 characters".into(),
                });
            }
            if sources
                .insert(file.file_id.clone(), file.bytes.clone())
                .is_some()
            {
                return Err(AppError::InvalidProject {
                    message: format!("duplicate review file id: {}", file.file_id),
                });
            }
            project_hasher.update((path.len() as u64).to_le_bytes());
            project_hasher.update(path.as_bytes());
            project_hasher.update((file.bytes.len() as u64).to_le_bytes());
            project_hasher.update(&file.bytes);
            base_files.push(ReviewBaseFile {
                file_id: file.file_id,
                path,
                sha256: hash_bytes(&file.bytes),
                byte_length: file.bytes.len(),
            });
        }
        let bundle = ReviewBundleV1 {
            schema_version: ReviewBundleV1::SCHEMA_VERSION,
            bundle_id: Uuid::new_v4(),
            project_id,
            project_hash: hex::encode(project_hasher.finalize()),
            base_files,
            reviewer,
            exported_at,
            threads: Vec::new(),
            suggestions: Vec::new(),
        };
        validate_review_bundle(&bundle)?;
        Ok(Self {
            bundle,
            base_sources: sources,
        })
    }

    #[must_use]
    pub fn bundle(&self) -> &ReviewBundleV1 {
        &self.bundle
    }

    pub fn create_anchor(
        &self,
        file_id: &str,
        start_byte: usize,
        end_byte: usize,
    ) -> AppResult<ReviewAnchor> {
        let source = self
            .base_sources
            .get(file_id)
            .ok_or_else(|| AppError::UnknownFile {
                file_id: file_id.into(),
            })?;
        let text = std::str::from_utf8(source).map_err(|_| AppError::InvalidUtf8 {
            path: file_id.into(),
        })?;
        if start_byte > end_byte
            || end_byte > source.len()
            || !text.is_char_boundary(start_byte)
            || !text.is_char_boundary(end_byte)
        {
            return Err(AppError::InvalidEdit {
                reason: format!("invalid review anchor {start_byte}..{end_byte}"),
            });
        }
        let prefix_start = previous_char_boundary(text, start_byte.saturating_sub(CONTEXT_BYTES));
        let suffix_end = next_char_boundary(text, (end_byte + CONTEXT_BYTES).min(source.len()));
        let base_file = self
            .bundle
            .base_files
            .iter()
            .find(|file| file.file_id == file_id)
            .expect("source and base file maps are built together");
        Ok(ReviewAnchor {
            file_id: file_id.into(),
            start_byte,
            end_byte,
            base_file_hash: base_file.sha256.clone(),
            expected_source: text[start_byte..end_byte].into(),
            prefix_context: text[prefix_start..start_byte].into(),
            suffix_context: text[end_byte..suffix_end].into(),
        })
    }

    pub fn add_thread(
        &mut self,
        anchor: ReviewAnchor,
        author: ReviewIdentity,
        body: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> AppResult<Uuid> {
        self.validate_anchor_against_base(&anchor)?;
        let body = body.into();
        validate_message_body(&body)?;
        let id = Uuid::new_v4();
        self.bundle.threads.push(ReviewThread {
            id,
            anchor,
            status: ReviewThreadStatus::Open,
            created_at,
            resolved_at: None,
            messages: vec![ReviewMessage {
                id: Uuid::new_v4(),
                author,
                created_at,
                body,
            }],
        });
        Ok(id)
    }

    pub fn reply(
        &mut self,
        thread_id: Uuid,
        author: ReviewIdentity,
        body: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> AppResult<Uuid> {
        let body = body.into();
        validate_message_body(&body)?;
        let thread = self
            .bundle
            .threads
            .iter_mut()
            .find(|thread| thread.id == thread_id)
            .ok_or_else(|| AppError::InvalidProject {
                message: format!("unknown review thread {thread_id}"),
            })?;
        let id = Uuid::new_v4();
        thread.messages.push(ReviewMessage {
            id,
            author,
            created_at,
            body,
        });
        Ok(id)
    }

    pub fn set_thread_resolved(
        &mut self,
        thread_id: Uuid,
        resolved: bool,
        changed_at: DateTime<Utc>,
    ) -> AppResult<()> {
        let thread = self
            .bundle
            .threads
            .iter_mut()
            .find(|thread| thread.id == thread_id)
            .ok_or_else(|| AppError::InvalidProject {
                message: format!("unknown review thread {thread_id}"),
            })?;
        thread.status = if resolved {
            ReviewThreadStatus::Resolved
        } else {
            ReviewThreadStatus::Open
        };
        thread.resolved_at = resolved.then_some(changed_at);
        Ok(())
    }

    pub fn add_suggestion(
        &mut self,
        anchor: ReviewAnchor,
        author: ReviewIdentity,
        replacement: String,
        created_at: DateTime<Utc>,
    ) -> AppResult<Uuid> {
        self.validate_anchor_against_base(&anchor)?;
        if replacement.len() > 1024 * 1024 {
            return Err(AppError::InvalidEdit {
                reason: "review suggestion exceeds 1 MiB".into(),
            });
        }
        let id = Uuid::new_v4();
        let order = self
            .bundle
            .suggestions
            .iter()
            .map(|suggestion| suggestion.order)
            .max()
            .map_or(0, |order| order.saturating_add(1));
        self.bundle.suggestions.push(ReviewSuggestion {
            id,
            order,
            author,
            created_at,
            anchor,
            replacement,
            status: SuggestionStatus::Pending,
        });
        Ok(id)
    }

    pub fn set_suggestion_status(
        &mut self,
        suggestion_id: Uuid,
        status: SuggestionStatus,
    ) -> AppResult<()> {
        let suggestion = self
            .bundle
            .suggestions
            .iter_mut()
            .find(|suggestion| suggestion.id == suggestion_id)
            .ok_or_else(|| AppError::InvalidProject {
                message: format!("unknown review suggestion {suggestion_id}"),
            })?;
        suggestion.status = status;
        Ok(())
    }

    pub fn export(&self, path: &Path) -> AppResult<()> {
        validate_review_path(path)?;
        validate_review_bundle(&self.bundle)?;
        let mut bytes = serde_json::to_vec_pretty(&self.bundle).map_err(AppError::serialization)?;
        bytes.push(b'\n');
        atomic_write(path, &bytes)?;
        Ok(())
    }

    fn validate_anchor_against_base(&self, anchor: &ReviewAnchor) -> AppResult<()> {
        let source =
            self.base_sources
                .get(&anchor.file_id)
                .ok_or_else(|| AppError::UnknownFile {
                    file_id: anchor.file_id.clone(),
                })?;
        let base = self
            .bundle
            .base_files
            .iter()
            .find(|file| file.file_id == anchor.file_id)
            .expect("base source has a descriptor");
        if anchor.base_file_hash != base.sha256
            || anchor.start_byte > anchor.end_byte
            || anchor.end_byte > source.len()
            || source[anchor.start_byte..anchor.end_byte] != *anchor.expected_source.as_bytes()
        {
            return Err(AppError::InvalidEdit {
                reason: "review anchor does not match the exact base source".into(),
            });
        }
        Ok(())
    }
}

pub fn import_review_bundle(path: &Path) -> AppResult<ReviewBundleV1> {
    validate_review_path(path)?;
    let metadata = std::fs::metadata(path).map_err(|error| AppError::io("inspect", path, error))?;
    if metadata.len() > MAX_REVIEW_FILE_BYTES {
        return Err(AppError::InvalidProject {
            message: "review bundle exceeds 64 MiB".into(),
        });
    }
    let bytes = std::fs::read(path).map_err(|error| AppError::io("read", path, error))?;
    let bundle: ReviewBundleV1 = serde_json::from_slice(&bytes).map_err(AppError::serialization)?;
    validate_review_bundle(&bundle)?;
    Ok(bundle)
}

pub fn validate_review_bundle(bundle: &ReviewBundleV1) -> AppResult<()> {
    if bundle.schema_version != ReviewBundleV1::SCHEMA_VERSION {
        return Err(AppError::Serialization {
            message: format!("unsupported review schema {}", bundle.schema_version),
        });
    }
    validate_digest(&bundle.project_hash)?;
    if bundle.base_files.is_empty() {
        return Err(AppError::Serialization {
            message: "review bundle has no base files".into(),
        });
    }
    let mut file_ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut hashes = BTreeMap::new();
    for file in &bundle.base_files {
        safe_relative_path(&file.path)?;
        validate_digest(&file.sha256)?;
        if file.file_id.is_empty()
            || file.file_id.len() > 128
            || !file_ids.insert(file.file_id.clone())
            || !paths.insert(file.path.to_ascii_lowercase())
        {
            return Err(AppError::Serialization {
                message: "review base file ids and paths must be unique".into(),
            });
        }
        hashes.insert(file.file_id.clone(), (&file.sha256, file.byte_length));
    }
    let mut ids = BTreeSet::new();
    for thread in &bundle.threads {
        if !ids.insert(thread.id) || thread.messages.is_empty() {
            return Err(AppError::Serialization {
                message: "review thread ids must be unique and messages non-empty".into(),
            });
        }
        validate_anchor_shape(&thread.anchor, &hashes)?;
        if (thread.status == ReviewThreadStatus::Resolved) != thread.resolved_at.is_some() {
            return Err(AppError::Serialization {
                message: "resolved thread status and resolvedAt disagree".into(),
            });
        }
        for message in &thread.messages {
            if !ids.insert(message.id) {
                return Err(AppError::Serialization {
                    message: "review message ids must be unique".into(),
                });
            }
            validate_message_body(&message.body)?;
        }
    }
    let mut orders = BTreeSet::new();
    for suggestion in &bundle.suggestions {
        if !ids.insert(suggestion.id) || !orders.insert(suggestion.order) {
            return Err(AppError::Serialization {
                message: "suggestion ids and orders must be unique".into(),
            });
        }
        validate_anchor_shape(&suggestion.anchor, &hashes)?;
        if suggestion.replacement.len() > 1024 * 1024 {
            return Err(AppError::Serialization {
                message: "suggestion replacement exceeds 1 MiB".into(),
            });
        }
    }
    Ok(())
}

fn validate_anchor_shape(
    anchor: &ReviewAnchor,
    hashes: &BTreeMap<String, (&String, usize)>,
) -> AppResult<()> {
    let Some((base_hash, byte_length)) = hashes.get(&anchor.file_id) else {
        return Err(AppError::Serialization {
            message: format!("anchor references unknown file {}", anchor.file_id),
        });
    };
    validate_digest(&anchor.base_file_hash)?;
    if anchor.base_file_hash.as_str() != base_hash.as_str()
        || anchor.start_byte > anchor.end_byte
        || anchor.end_byte > *byte_length
        || anchor.expected_source.len() > 1024 * 1024
        || anchor.prefix_context.len() > 4096
        || anchor.suffix_context.len() > 4096
    {
        return Err(AppError::Serialization {
            message: "review anchor violates its base-file bounds or hash".into(),
        });
    }
    Ok(())
}

fn validate_digest(value: &str) -> AppResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(AppError::Serialization {
            message: format!("invalid lowercase SHA-256: {value}"),
        })
    }
}

fn validate_message_body(body: &str) -> AppResult<()> {
    if body.is_empty() || body.len() > 65_536 {
        Err(AppError::InvalidEdit {
            reason: "review message must contain 1 to 65,536 UTF-8 bytes".into(),
        })
    } else {
        Ok(())
    }
}

fn validate_review_path(path: &Path) -> AppResult<()> {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".setwright-review"))
    {
        Ok(())
    } else {
        Err(AppError::InvalidPath {
            path: path.to_string_lossy().into_owned(),
            message: "review bundle must use the .setwright-review extension".into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AnchorResolution {
    Exact { start_byte: usize, end_byte: usize },
    Reanchored { start_byte: usize, end_byte: usize },
    Conflict { reason: String },
}

pub fn resolve_review_anchor(anchor: &ReviewAnchor, current: &[u8]) -> AnchorResolution {
    let Ok(text) = std::str::from_utf8(current) else {
        return AnchorResolution::Conflict {
            reason: "current file is not UTF-8".into(),
        };
    };
    if hash_bytes(current) == anchor.base_file_hash
        && anchor.start_byte <= anchor.end_byte
        && anchor.end_byte <= current.len()
        && text.is_char_boundary(anchor.start_byte)
        && text.is_char_boundary(anchor.end_byte)
        && current[anchor.start_byte..anchor.end_byte] == *anchor.expected_source.as_bytes()
    {
        return AnchorResolution::Exact {
            start_byte: anchor.start_byte,
            end_byte: anchor.end_byte,
        };
    }

    let needle = format!(
        "{}{}{}",
        anchor.prefix_context, anchor.expected_source, anchor.suffix_context
    );
    if needle.is_empty() {
        return AnchorResolution::Conflict {
            reason: "empty anchor has no unique context".into(),
        };
    }
    let matches: Vec<_> = text
        .match_indices(&needle)
        .map(|(offset, _)| offset)
        .collect();
    if matches.len() != 1 {
        return AnchorResolution::Conflict {
            reason: if matches.is_empty() {
                "exact anchor context no longer exists".into()
            } else {
                "anchor context is no longer unique".into()
            },
        };
    }
    let start_byte = matches[0] + anchor.prefix_context.len();
    AnchorResolution::Reanchored {
        start_byte,
        end_byte: start_byte + anchor.expected_source.len(),
    }
}

pub fn suggestion_to_source_edit(
    suggestion: &ReviewSuggestion,
    canonical_file_id: FileId,
    current: &[u8],
) -> AppResult<SourceEdit> {
    if suggestion.status != SuggestionStatus::Pending {
        return Err(AppError::InvalidEdit {
            reason: "only a pending suggestion can be accepted".into(),
        });
    }
    let (start_byte, end_byte) = match resolve_review_anchor(&suggestion.anchor, current) {
        AnchorResolution::Exact {
            start_byte,
            end_byte,
        }
        | AnchorResolution::Reanchored {
            start_byte,
            end_byte,
        } => (start_byte, end_byte),
        AnchorResolution::Conflict { reason } => {
            return Err(AppError::InvalidEdit {
                reason: format!("review suggestion conflicts: {reason}"),
            });
        }
    };
    Ok(SourceEdit {
        file_id: canonical_file_id,
        start_byte,
        end_byte,
        replacement: suggestion.replacement.clone(),
        expected_slice_hash: hash_bytes(&current[start_byte..end_byte]),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewOverlay {
    pub files: BTreeMap<String, Vec<u8>>,
    pub conflicted_suggestions: Vec<Uuid>,
}

pub fn apply_suggestion_overlay(
    bundle: &ReviewBundleV1,
    current_files: &BTreeMap<String, Vec<u8>>,
) -> AppResult<ReviewOverlay> {
    validate_review_bundle(bundle)?;
    let mut output = current_files.clone();
    let mut conflicts = BTreeSet::new();
    let mut by_file: BTreeMap<String, Vec<(usize, usize, u32, &ReviewSuggestion)>> =
        BTreeMap::new();
    for suggestion in bundle
        .suggestions
        .iter()
        .filter(|suggestion| suggestion.status == SuggestionStatus::Pending)
    {
        let Some(current) = current_files.get(&suggestion.anchor.file_id) else {
            conflicts.insert(suggestion.id);
            continue;
        };
        match resolve_review_anchor(&suggestion.anchor, current) {
            AnchorResolution::Exact {
                start_byte,
                end_byte,
            }
            | AnchorResolution::Reanchored {
                start_byte,
                end_byte,
            } => by_file
                .entry(suggestion.anchor.file_id.clone())
                .or_default()
                .push((start_byte, end_byte, suggestion.order, suggestion)),
            AnchorResolution::Conflict { .. } => {
                conflicts.insert(suggestion.id);
            }
        }
    }

    for (file_id, mut resolved) in by_file {
        resolved.sort_by_key(|(start, end, order, _)| (*start, *end, *order));
        for left_index in 0..resolved.len() {
            for right_index in left_index + 1..resolved.len() {
                let left = resolved[left_index];
                let right = resolved[right_index];
                if right.0 >= left.1 {
                    break;
                }
                let both_insertions = left.0 == left.1 && right.0 == right.1;
                if !both_insertions {
                    conflicts.insert(left.3.id);
                    conflicts.insert(right.3.id);
                }
            }
        }
        let current = output.get_mut(&file_id).expect("resolved from current map");
        resolved.retain(|(_, _, _, suggestion)| !conflicts.contains(&suggestion.id));
        // Descending offsets preserve base coordinates. For equal insertion
        // points, descending order yields ascending suggestion order in output.
        resolved.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.2.cmp(&left.2)));
        for (start, end, _, suggestion) in resolved {
            current.splice(
                start..end,
                suggestion.replacement.as_bytes().iter().copied(),
            );
        }
    }
    Ok(ReviewOverlay {
        files: output,
        conflicted_suggestions: conflicts.into_iter().collect(),
    })
}

fn previous_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(name: &str) -> ReviewIdentity {
        ReviewIdentity {
            id: Uuid::new_v4(),
            display_name: name.into(),
            email: None,
        }
    }

    fn workspace(source: &[u8]) -> ReviewWorkspace {
        ReviewWorkspace::new(
            ProjectId::new(),
            identity("Reviewer"),
            vec![ReviewFileInput {
                file_id: "main".into(),
                relative_path: "main.tex".into(),
                bytes: source.to_vec(),
            }],
            Utc::now(),
        )
        .unwrap()
    }

    #[test]
    fn unique_exact_context_reanchors_without_approximation() {
        let source = b"prefix unique target suffix";
        let workspace = workspace(source);
        let start = source
            .windows(6)
            .position(|window| window == b"target")
            .unwrap();
        let anchor = workspace.create_anchor("main", start, start + 6).unwrap();
        let current = b"inserted prefix unique target suffix";
        assert!(matches!(
            resolve_review_anchor(&anchor, current),
            AnchorResolution::Reanchored { start_byte, .. } if start_byte > start
        ));
    }

    #[test]
    fn ambiguous_context_is_a_conflict() {
        let source = b"same target same";
        let workspace = workspace(source);
        let anchor = workspace.create_anchor("main", 5, 11).unwrap();
        let current = b"same target samesame target same";
        assert!(matches!(
            resolve_review_anchor(&anchor, current),
            AnchorResolution::Conflict { .. }
        ));
    }

    #[test]
    fn accepting_suggestion_produces_hash_guarded_edit() {
        let source = b"hello world";
        let mut workspace = workspace(source);
        let anchor = workspace.create_anchor("main", 6, 11).unwrap();
        workspace
            .add_suggestion(anchor, identity("Reviewer"), "paper".into(), Utc::now())
            .unwrap();
        let edit =
            suggestion_to_source_edit(&workspace.bundle().suggestions[0], FileId::new(), source)
                .unwrap();
        assert_eq!(edit.replacement, "paper");
        assert_eq!(edit.expected_slice_hash, hash_bytes(b"world"));
    }

    #[test]
    fn review_export_uses_canonical_schema_shape() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("review.setwright-review");
        let workspace = workspace(b"hello");
        workspace.export(&path).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(value.get("baseFiles").is_some());
        assert!(value.get("threads").is_some());
        assert!(value.get("baseFileHashes").is_none());
        assert_eq!(import_review_bundle(&path).unwrap(), *workspace.bundle());
    }
}
