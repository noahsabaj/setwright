use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

/// A stable, structured error contract suitable for the Tauri boundary.
///
/// Variants intentionally carry strings instead of `std::io::Error` so the
/// value can be serialized without losing its category.  Internal errors are
/// converted at the point where enough operation/path context is available.
#[derive(specta::Type, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AppError {
    #[error("{operation} failed for {path}: {message}")]
    Io {
        operation: String,
        path: String,
        message: String,
    },
    #[error("file is not valid UTF-8: {path}")]
    InvalidUtf8 { path: String },
    #[error("invalid path {path}: {message}")]
    InvalidPath { path: String, message: String },
    #[error("path escapes the project root: {path}")]
    PathOutsideRoot { path: String },
    #[error("capability {capability} denied: {message}")]
    CapabilityDenied { capability: String, message: String },
    #[error("file not found: {path}")]
    FileNotFound { path: String },
    #[error("unknown file id: {file_id}")]
    UnknownFile { file_id: String },
    #[error("project session is closed")]
    SessionClosed,
    #[error("revision conflict: expected {expected}, current {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("source slice changed: expected {expected}, actual {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("invalid source edit: {reason}")]
    InvalidEdit { reason: String },
    #[error("invalid project: {message}")]
    InvalidProject { message: String },
    #[error("parse failed: {message}")]
    Parse { message: String },
    #[error("serialization failed: {message}")]
    Serialization { message: String },
    #[error("external change conflict in {path}")]
    ExternalConflict { path: String },
    #[error("preflight is blocked: {message}")]
    PreflightBlocked { message: String },
    #[error("compile unavailable: {message}")]
    CompileUnavailable { message: String },
    #[error("archive failed: {message}")]
    Archive { message: String },
}

impl AppError {
    pub fn io(operation: impl Into<String>, path: &Path, error: impl std::fmt::Display) -> Self {
        Self::Io {
            operation: operation.into(),
            path: path.to_string_lossy().into_owned(),
            message: error.to_string(),
        }
    }

    pub fn serialization(error: impl std::fmt::Display) -> Self {
        Self::Serialization {
            message: error.to_string(),
        }
    }
}
