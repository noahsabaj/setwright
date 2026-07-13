use crate::core::contracts::{FileId, Revision, SourceEdit, SourceSpan};
use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NewlineStyle {
    Lf,
    CrLf,
    Mixed,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceEncoding {
    Utf8,
    Utf8Bom,
    NonUtf8,
}

/// The authoritative bytes for one project file.
///
/// `bytes` are never normalized. In particular, BOMs, line endings, trailing
/// whitespace, comments, and malformed LaTeX remain byte-identical until a
/// validated edit explicitly touches them.
#[derive(Debug, Clone)]
pub struct SourceBuffer {
    file_id: FileId,
    relative_path: PathBuf,
    bytes: Vec<u8>,
    persisted_hash: String,
    revision: Revision,
    encoding: SourceEncoding,
    newline_style: NewlineStyle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePatchOutcome {
    pub changed: bool,
    pub old_hash: String,
    pub new_hash: String,
    pub touched_spans: Vec<SourceSpan>,
}

impl SourceBuffer {
    #[must_use]
    pub fn from_bytes(
        file_id: FileId,
        relative_path: impl Into<PathBuf>,
        bytes: Vec<u8>,
        revision: Revision,
    ) -> Self {
        let persisted_hash = hash_bytes(&bytes);
        let encoding = detect_encoding(&bytes);
        let newline_style = detect_newline_style(&bytes);
        Self {
            file_id,
            relative_path: relative_path.into(),
            bytes,
            persisted_hash,
            revision,
            encoding,
            newline_style,
        }
    }

    pub fn read_from(
        file_id: FileId,
        root: &Path,
        relative_path: impl Into<PathBuf>,
        revision: Revision,
    ) -> AppResult<Self> {
        let relative_path = relative_path.into();
        let path = root.join(&relative_path);
        let bytes = std::fs::read(&path).map_err(|error| AppError::io("read", &path, error))?;
        Ok(Self::from_bytes(file_id, relative_path, bytes, revision))
    }

    #[must_use]
    pub const fn file_id(&self) -> FileId {
        self.file_id
    }

    #[must_use]
    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn text(&self) -> AppResult<&str> {
        std::str::from_utf8(&self.bytes).map_err(|_| AppError::InvalidUtf8 {
            path: self.relative_path.to_string_lossy().into_owned(),
        })
    }

    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    #[must_use]
    pub const fn encoding(&self) -> SourceEncoding {
        self.encoding
    }

    #[must_use]
    pub const fn newline_style(&self) -> NewlineStyle {
        self.newline_style
    }

    #[must_use]
    pub fn is_source_only(&self) -> bool {
        self.encoding == SourceEncoding::NonUtf8
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.persisted_hash != self.content_hash()
    }

    #[must_use]
    pub fn persisted_hash(&self) -> &str {
        &self.persisted_hash
    }

    #[must_use]
    pub fn content_hash(&self) -> String {
        hash_bytes(&self.bytes)
    }

    pub fn slice(&self, start_byte: usize, end_byte: usize) -> AppResult<&[u8]> {
        validate_bounds(&self.bytes, start_byte, end_byte, &self.relative_path)?;
        Ok(&self.bytes[start_byte..end_byte])
    }

    pub fn slice_hash(&self, start_byte: usize, end_byte: usize) -> AppResult<String> {
        self.slice(start_byte, end_byte).map(hash_bytes)
    }

    /// Applies a set of edits expressed against this exact buffer revision.
    /// Validation happens in full before any candidate bytes are committed.
    pub fn apply_edits(
        &mut self,
        edits: &[SourceEdit],
        new_revision: Revision,
    ) -> AppResult<SourcePatchOutcome> {
        if self.encoding == SourceEncoding::NonUtf8 {
            return Err(AppError::InvalidUtf8 {
                path: self.relative_path.to_string_lossy().into_owned(),
            });
        }

        let mut indexed = Vec::with_capacity(edits.len());
        for edit in edits {
            if edit.file_id != self.file_id {
                return Err(AppError::InvalidEdit {
                    reason: format!(
                        "edit targets {}, but buffer is {}",
                        edit.file_id, self.file_id
                    ),
                });
            }
            validate_bounds(
                &self.bytes,
                edit.start_byte,
                edit.end_byte,
                &self.relative_path,
            )?;
            let actual = hash_bytes(&self.bytes[edit.start_byte..edit.end_byte]);
            if actual != edit.expected_slice_hash {
                return Err(AppError::HashMismatch {
                    expected: edit.expected_slice_hash.clone(),
                    actual,
                });
            }
            indexed.push(edit);
        }

        indexed.sort_by_key(|edit| (edit.start_byte, edit.end_byte));
        for pair in indexed.windows(2) {
            let left = pair[0];
            let right = pair[1];
            let overlaps = left.end_byte > right.start_byte;
            let ambiguous_same_insertion = left.start_byte == left.end_byte
                && right.start_byte == right.end_byte
                && left.start_byte == right.start_byte;
            if overlaps || ambiguous_same_insertion {
                return Err(AppError::InvalidEdit {
                    reason: format!(
                        "overlapping or ambiguous edits at {}..{} and {}..{}",
                        left.start_byte, left.end_byte, right.start_byte, right.end_byte
                    ),
                });
            }
        }

        let old_hash = self.content_hash();
        let mut candidate = self.bytes.clone();
        for edit in indexed.iter().rev() {
            candidate.splice(
                edit.start_byte..edit.end_byte,
                edit.replacement.as_bytes().iter().copied(),
            );
        }

        // Replacements are strings and edit boundaries were verified above,
        // therefore this is an invariant check rather than lossy conversion.
        if std::str::from_utf8(&candidate).is_err() {
            return Err(AppError::InvalidEdit {
                reason: "candidate buffer is not valid UTF-8".into(),
            });
        }

        let new_hash = hash_bytes(&candidate);
        let changed = old_hash != new_hash;
        if changed {
            self.bytes = candidate;
            self.revision = new_revision;
            self.encoding = detect_encoding(&self.bytes);
            self.newline_style = detect_newline_style(&self.bytes);
        }

        Ok(SourcePatchOutcome {
            changed,
            old_hash,
            new_hash,
            touched_spans: indexed
                .into_iter()
                .map(|edit| SourceSpan::new(self.file_id, edit.start_byte, edit.end_byte))
                .collect(),
        })
    }

    /// Replaces a clean buffer with externally changed bytes. Dirty buffers
    /// must be resolved by a three-way merge in the caller.
    pub fn reload_clean(&mut self, bytes: Vec<u8>, new_revision: Revision) -> AppResult<bool> {
        if self.is_dirty() {
            return Err(AppError::ExternalConflict {
                path: self.relative_path.to_string_lossy().into_owned(),
            });
        }
        let changed = hash_bytes(&bytes) != self.content_hash();
        if changed {
            self.bytes = bytes;
            self.persisted_hash = self.content_hash();
            self.revision = new_revision;
            self.encoding = detect_encoding(&self.bytes);
            self.newline_style = detect_newline_style(&self.bytes);
        }
        Ok(changed)
    }

    /// Explicitly converts a non-UTF-8 buffer. The caller must supply the
    /// reviewed UTF-8 replacement; Setwright never guesses an encoding.
    pub fn convert_to_utf8(&mut self, reviewed_text: String, new_revision: Revision) -> bool {
        let candidate = reviewed_text.into_bytes();
        if candidate == self.bytes {
            return false;
        }
        self.bytes = candidate;
        self.revision = new_revision;
        self.encoding = detect_encoding(&self.bytes);
        self.newline_style = detect_newline_style(&self.bytes);
        true
    }

    pub(crate) fn mark_persisted(&mut self) {
        self.persisted_hash = self.content_hash();
    }

    /// Updates the on-disk comparison baseline without replacing canonical
    /// local bytes. Three-way merge resolution uses this to keep the merged
    /// buffer dirty exactly when it differs from the external version that the
    /// user reviewed.
    pub(crate) fn set_persisted_baseline(&mut self, persisted_bytes: &[u8]) {
        self.persisted_hash = hash_bytes(persisted_bytes);
    }
}

#[must_use]
pub fn hash_bytes(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

fn detect_encoding(bytes: &[u8]) -> SourceEncoding {
    if std::str::from_utf8(bytes).is_err() {
        SourceEncoding::NonUtf8
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        SourceEncoding::Utf8Bom
    } else {
        SourceEncoding::Utf8
    }
}

fn detect_newline_style(bytes: &[u8]) -> NewlineStyle {
    let mut lf = 0usize;
    let mut crlf = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            if index > 0 && bytes[index - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
        }
        index += 1;
    }
    match (lf, crlf) {
        (0, 0) => NewlineStyle::None,
        (_, 0) => NewlineStyle::Lf,
        (0, _) => NewlineStyle::CrLf,
        _ => NewlineStyle::Mixed,
    }
}

fn validate_bounds(bytes: &[u8], start_byte: usize, end_byte: usize, path: &Path) -> AppResult<()> {
    if start_byte > end_byte || end_byte > bytes.len() {
        return Err(AppError::InvalidEdit {
            reason: format!(
                "byte range {start_byte}..{end_byte} is outside {} ({} bytes)",
                path.display(),
                bytes.len()
            ),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AppError::InvalidUtf8 {
        path: path.to_string_lossy().into_owned(),
    })?;
    if !text.is_char_boundary(start_byte) || !text.is_char_boundary(end_byte) {
        return Err(AppError::InvalidEdit {
            reason: format!("byte range {start_byte}..{end_byte} splits a UTF-8 character"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_bom_and_crlf_around_edit() {
        let file_id = FileId::new();
        let original = b"\xef\xbb\xbfalpha\r\nbeta\r\n".to_vec();
        let mut source =
            SourceBuffer::from_bytes(file_id, "main.tex", original.clone(), Revision::INITIAL);
        let start = original
            .windows(4)
            .position(|item| item == b"beta")
            .unwrap();
        let edit = SourceEdit {
            file_id,
            start_byte: start,
            end_byte: start + 4,
            replacement: "gamma".into(),
            expected_slice_hash: hash_bytes(b"beta"),
        };
        source.apply_edits(&[edit], Revision(1)).unwrap();
        assert_eq!(source.bytes(), b"\xef\xbb\xbfalpha\r\ngamma\r\n");
        assert_eq!(source.encoding(), SourceEncoding::Utf8Bom);
        assert_eq!(source.newline_style(), NewlineStyle::CrLf);
    }

    #[test]
    fn rejects_hash_mismatch_without_mutation() {
        let file_id = FileId::new();
        let mut source =
            SourceBuffer::from_bytes(file_id, "main.tex", b"hello".to_vec(), Revision::INITIAL);
        let result = source.apply_edits(
            &[SourceEdit {
                file_id,
                start_byte: 0,
                end_byte: 5,
                replacement: "bye".into(),
                expected_slice_hash: hash_bytes(b"stale"),
            }],
            Revision(1),
        );
        assert!(matches!(result, Err(AppError::HashMismatch { .. })));
        assert_eq!(source.bytes(), b"hello");
        assert_eq!(source.revision(), Revision::INITIAL);
    }

    #[test]
    fn identical_replacement_is_a_noop() {
        let file_id = FileId::new();
        let mut source =
            SourceBuffer::from_bytes(file_id, "main.tex", b"hello".to_vec(), Revision::INITIAL);
        let outcome = source
            .apply_edits(
                &[SourceEdit {
                    file_id,
                    start_byte: 0,
                    end_byte: 5,
                    replacement: "hello".into(),
                    expected_slice_hash: hash_bytes(b"hello"),
                }],
                Revision(1),
            )
            .unwrap();
        assert!(!outcome.changed);
        assert_eq!(source.revision(), Revision::INITIAL);
        assert!(!source.is_dirty());
    }

    #[test]
    fn rejects_split_unicode_boundary() {
        let file_id = FileId::new();
        let mut source = SourceBuffer::from_bytes(
            file_id,
            "main.tex",
            "aéb".as_bytes().to_vec(),
            Revision::INITIAL,
        );
        let result = source.apply_edits(
            &[SourceEdit {
                file_id,
                start_byte: 2,
                end_byte: 3,
                replacement: "x".into(),
                expected_slice_hash: hash_bytes(&source.bytes()[2..3]),
            }],
            Revision(1),
        );
        assert!(matches!(result, Err(AppError::InvalidEdit { .. })));
    }
}
