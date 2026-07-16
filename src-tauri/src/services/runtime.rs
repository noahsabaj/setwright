//! Verification and atomic installation of managed TeX Live profiles.
//!
//! A runtime is not trusted because it came from a configured URL. It is
//! trusted only after its manifest has passed schema/target checks, its RFC
//! 8785 representation has passed Ed25519 verification with an embedded key,
//! and the downloaded archive has matched the signed size and SHA-256.

use crate::core::contracts::{
    LatexEngine, LocalDocument, RuntimeArchitecture, RuntimeArtifact, RuntimeManifestV1,
    RuntimePlatform, RuntimeSbom, RuntimeSignature, SbomFormat, TexLiveSnapshot,
};
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

const INSTALL_MARKER: &str = ".setwright-runtime-ready.json";
const INSTALLED_MANIFEST: &str = ".setwright-runtime-manifest.json";
const ED25519_ALGORITHM: &str = "ed25519";
const RFC8785_CANONICALIZATION: &str = "RFC8785";
const SHA256_HEX_LENGTH: usize = 64;

pub type RuntimeResult<T> = Result<T, RuntimeTrustError>;

/// Errors are deliberately specific so an installer UI can distinguish a
/// damaged download from an untrusted manifest or an unsafe archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RuntimeTrustError {
    #[error("invalid runtime manifest: {message}")]
    InvalidManifest { message: String },
    #[error("runtime manifest key is not trusted: {key_id}")]
    UnknownKey { key_id: String },
    #[error("runtime manifest signature is invalid: {message}")]
    InvalidSignature { message: String },
    #[error("runtime archive size mismatch: expected {expected}, got {actual}")]
    ArchiveSizeMismatch { expected: u64, actual: u64 },
    #[error("runtime archive hash mismatch: expected {expected}, got {actual}")]
    ArchiveHashMismatch { expected: String, actual: String },
    #[error("unsafe runtime archive entry {entry}: {reason}")]
    UnsafeArchiveEntry { entry: String, reason: String },
    #[error("runtime archive exceeds {limit}: {actual} > {maximum}")]
    ArchiveLimit {
        limit: String,
        actual: u64,
        maximum: u64,
    },
    #[error("runtime profile id {profile_id} already exists with different state")]
    ImmutableProfileConflict { profile_id: String },
    #[error("installed runtime {profile_id} is corrupt: {message}")]
    CorruptInstallation { profile_id: String, message: String },
    #[error("runtime archive failed: {message}")]
    Archive { message: String },
    #[error("{operation} failed for {path}: {message}")]
    Io {
        operation: String,
        path: String,
        message: String,
    },
}

impl RuntimeTrustError {
    fn io(operation: impl Into<String>, path: &Path, error: impl std::fmt::Display) -> Self {
        Self::Io {
            operation: operation.into(),
            path: path.to_string_lossy().into_owned(),
            message: error.to_string(),
        }
    }

    fn invalid_manifest(message: impl Into<String>) -> Self {
        Self::InvalidManifest {
            message: message.into(),
        }
    }

    fn unsafe_entry(entry: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::UnsafeArchiveEntry {
            entry: entry.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTarget {
    pub platform: RuntimePlatform,
    pub architecture: RuntimeArchitecture,
}

impl RuntimeTarget {
    pub fn current() -> RuntimeResult<Self> {
        let platform = match std::env::consts::OS {
            "windows" => RuntimePlatform::Windows,
            "macos" => RuntimePlatform::Macos,
            "linux" => RuntimePlatform::Linux,
            other => {
                return Err(RuntimeTrustError::invalid_manifest(format!(
                    "unsupported runtime platform {other}"
                )));
            }
        };
        let architecture = match std::env::consts::ARCH {
            "x86_64" => RuntimeArchitecture::X86_64,
            "aarch64" => RuntimeArchitecture::Aarch64,
            other => {
                return Err(RuntimeTrustError::invalid_manifest(format!(
                    "unsupported runtime architecture {other}"
                )));
            }
        };
        Ok(Self {
            platform,
            architecture,
        })
    }
}

/// Application-embedded public keys, indexed by stable manifest key id.
///
/// There is intentionally no network key discovery or manifest-provided key.
#[derive(Debug, Clone, Default)]
pub struct TrustedRuntimeKeys {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustedRuntimeKeys {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, key_id: impl Into<String>, key: [u8; 32]) -> RuntimeResult<()> {
        let key_id = key_id.into();
        validate_identifier(&key_id, "signature key id", 96)?;
        let key = VerifyingKey::from_bytes(&key).map_err(|error| {
            RuntimeTrustError::InvalidSignature {
                message: format!("invalid trusted Ed25519 public key: {error}"),
            }
        })?;
        self.keys.insert(key_id, key);
        Ok(())
    }

    pub fn insert_base64(
        &mut self,
        key_id: impl Into<String>,
        public_key_base64: &str,
    ) -> RuntimeResult<()> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(public_key_base64)
            .map_err(|error| RuntimeTrustError::InvalidSignature {
                message: format!("trusted public key is not valid base64: {error}"),
            })?;
        let bytes: [u8; 32] =
            decoded
                .try_into()
                .map_err(|_| RuntimeTrustError::InvalidSignature {
                    message: "trusted Ed25519 public key must be exactly 32 bytes".into(),
                })?;
        self.insert(key_id, bytes)
    }

    fn get(&self, key_id: &str) -> RuntimeResult<&VerifyingKey> {
        self.keys
            .get(key_id)
            .ok_or_else(|| RuntimeTrustError::UnknownKey {
                key_id: key_id.into(),
            })
    }
}

/// A manifest whose target, schema, and signature have all been checked.
/// Fields are private so callers cannot manufacture this trust boundary.
#[derive(Debug, Clone)]
pub struct VerifiedRuntimeManifest {
    manifest: RuntimeManifestV1,
    canonical_signed_bytes: Vec<u8>,
    manifest_sha256: String,
}

impl VerifiedRuntimeManifest {
    #[must_use]
    pub fn manifest(&self) -> &RuntimeManifestV1 {
        &self.manifest
    }

    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.manifest.profile_id
    }

    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    #[must_use]
    pub fn signed_payload(&self) -> &[u8] {
        &self.canonical_signed_bytes
    }
}

/// Serialize the signed payload exactly as RFC 8785 JCS, with only
/// `signature.value` replaced by the empty string.
pub fn canonical_manifest_payload(manifest: &RuntimeManifestV1) -> RuntimeResult<Vec<u8>> {
    let mut unsigned = manifest.clone();
    unsigned.signature.value.clear();
    serde_json_canonicalizer::to_vec(&unsigned).map_err(|error| {
        RuntimeTrustError::InvalidManifest {
            message: format!("manifest cannot be canonicalized: {error}"),
        }
    })
}

pub fn verify_runtime_manifest(
    manifest: RuntimeManifestV1,
    expected_target: RuntimeTarget,
    trusted_keys: &TrustedRuntimeKeys,
) -> RuntimeResult<VerifiedRuntimeManifest> {
    validate_runtime_manifest(&manifest, expected_target)?;
    let signed_payload = canonical_manifest_payload(&manifest)?;
    let canonical_full = serde_json_canonicalizer::to_vec(&manifest).map_err(|error| {
        RuntimeTrustError::InvalidManifest {
            message: format!("manifest cannot be canonicalized: {error}"),
        }
    })?;
    verify_manifest_signature(manifest, signed_payload, canonical_full, trusted_keys)
}

/// Strict JSON entry point for downloaded manifests. Unlike a direct Serde
/// decode into the shared contract, this rejects duplicate and unknown fields
/// at every level and verifies the signature over the received JSON values.
pub fn verify_runtime_manifest_json(
    bytes: &[u8],
    expected_target: RuntimeTarget,
    trusted_keys: &TrustedRuntimeKeys,
) -> RuntimeResult<VerifiedRuntimeManifest> {
    let strict: StrictRuntimeManifestV1 =
        serde_json::from_slice(bytes).map_err(|error| RuntimeTrustError::InvalidManifest {
            message: format!("manifest JSON does not match RuntimeManifestV1: {error}"),
        })?;
    let manifest = RuntimeManifestV1::from(strict);
    validate_runtime_manifest(&manifest, expected_target)?;

    let mut full_value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| RuntimeTrustError::InvalidManifest {
            message: format!("manifest JSON is invalid: {error}"),
        })?;
    let canonical_full = serde_json_canonicalizer::to_vec(&full_value).map_err(|error| {
        RuntimeTrustError::InvalidManifest {
            message: format!("manifest cannot be canonicalized: {error}"),
        }
    })?;
    let signature_value = full_value
        .get_mut("signature")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|signature| signature.get_mut("value"))
        .ok_or_else(|| RuntimeTrustError::invalid_manifest("signature.value is required"))?;
    *signature_value = serde_json::Value::String(String::new());
    let signed_payload = serde_json_canonicalizer::to_vec(&full_value).map_err(|error| {
        RuntimeTrustError::InvalidManifest {
            message: format!("manifest cannot be canonicalized: {error}"),
        }
    })?;
    verify_manifest_signature(manifest, signed_payload, canonical_full, trusted_keys)
}

fn verify_manifest_signature(
    manifest: RuntimeManifestV1,
    signed_payload: Vec<u8>,
    canonical_full: Vec<u8>,
    trusted_keys: &TrustedRuntimeKeys,
) -> RuntimeResult<VerifiedRuntimeManifest> {
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(&manifest.signature.value)
        .map_err(|error| RuntimeTrustError::InvalidSignature {
            message: format!("signature is not valid base64: {error}"),
        })?;
    let signature = Signature::from_slice(&signature_bytes).map_err(|error| {
        RuntimeTrustError::InvalidSignature {
            message: format!("signature must be a 64-byte Ed25519 signature: {error}"),
        }
    })?;
    trusted_keys
        .get(&manifest.signature.key_id)?
        .verify_strict(&signed_payload, &signature)
        .map_err(|_| RuntimeTrustError::InvalidSignature {
            message: "Ed25519 verification failed".into(),
        })?;

    Ok(VerifiedRuntimeManifest {
        manifest,
        canonical_signed_bytes: signed_payload,
        manifest_sha256: sha256_hex(&canonical_full),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictRuntimeManifestV1 {
    schema_version: u32,
    profile_id: String,
    tex_live_snapshot: StrictTexLiveSnapshot,
    platform: RuntimePlatform,
    architecture: RuntimeArchitecture,
    engines: Vec<LatexEngine>,
    archive: StrictRuntimeArtifact,
    signature: StrictRuntimeSignature,
    sbom: StrictRuntimeSbom,
    license_inventory: StrictLocalDocument,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictTexLiveSnapshot {
    version: u16,
    date: chrono::NaiveDate,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictRuntimeArtifact {
    url: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictRuntimeSignature {
    algorithm: String,
    canonicalization: String,
    key_id: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictRuntimeSbom {
    path: String,
    sha256: String,
    format: SbomFormat,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictLocalDocument {
    path: String,
    sha256: String,
}

impl From<StrictRuntimeManifestV1> for RuntimeManifestV1 {
    fn from(value: StrictRuntimeManifestV1) -> Self {
        Self {
            schema_version: value.schema_version,
            profile_id: value.profile_id,
            tex_live_snapshot: TexLiveSnapshot {
                version: value.tex_live_snapshot.version,
                date: value.tex_live_snapshot.date,
            },
            platform: value.platform,
            architecture: value.architecture,
            engines: value.engines,
            archive: RuntimeArtifact {
                url: value.archive.url,
                size_bytes: value.archive.size_bytes,
                sha256: value.archive.sha256,
            },
            signature: RuntimeSignature {
                algorithm: value.signature.algorithm,
                canonicalization: value.signature.canonicalization,
                key_id: value.signature.key_id,
                value: value.signature.value,
            },
            sbom: RuntimeSbom {
                path: value.sbom.path,
                sha256: value.sbom.sha256,
                format: value.sbom.format,
            },
            license_inventory: LocalDocument {
                path: value.license_inventory.path,
                sha256: value.license_inventory.sha256,
            },
            created_at: value.created_at,
        }
    }
}

pub fn validate_runtime_manifest(
    manifest: &RuntimeManifestV1,
    expected_target: RuntimeTarget,
) -> RuntimeResult<()> {
    if manifest.schema_version != RuntimeManifestV1::SCHEMA_VERSION {
        return Err(RuntimeTrustError::invalid_manifest(format!(
            "schemaVersion must be {}",
            RuntimeManifestV1::SCHEMA_VERSION
        )));
    }
    validate_identifier(&manifest.profile_id, "profile id", 128)?;

    let snapshot = &manifest.tex_live_snapshot;
    let approved = (snapshot.version == 2025
        && snapshot.date == chrono::NaiveDate::from_ymd_opt(2025, 8, 3).expect("valid fixed date"))
        || (snapshot.version == 2023
            && snapshot.date
                == chrono::NaiveDate::from_ymd_opt(2023, 5, 21).expect("valid fixed date"));
    if !approved {
        return Err(RuntimeTrustError::invalid_manifest(format!(
            "TeX Live {} snapshot {} is not an approved Setwright profile",
            snapshot.version, snapshot.date
        )));
    }
    let snapshot_marker = format!("{}-{}", snapshot.version, snapshot.date);
    if !manifest.profile_id.contains(&snapshot.version.to_string())
        || !manifest.profile_id.contains(&snapshot.date.to_string())
        || manifest.profile_id.contains(char::is_whitespace)
        || !manifest.profile_id.contains(&snapshot_marker)
            && !manifest
                .profile_id
                .contains(&format!("{}.{}", snapshot.version, snapshot.date))
    {
        return Err(RuntimeTrustError::invalid_manifest(
            "profile id must bind the TeX Live version and snapshot date",
        ));
    }
    if manifest.platform != expected_target.platform
        || manifest.architecture != expected_target.architecture
    {
        return Err(RuntimeTrustError::invalid_manifest(
            "runtime platform/architecture does not match the requested target",
        ));
    }
    let pdf_latex_count = manifest
        .engines
        .iter()
        .filter(|engine| **engine == LatexEngine::PdfLatex)
        .count();
    let xe_latex_count = manifest
        .engines
        .iter()
        .filter(|engine| **engine == LatexEngine::XeLatex)
        .count();
    if manifest.engines.len() != 2 || pdf_latex_count != 1 || xe_latex_count != 1 {
        return Err(RuntimeTrustError::invalid_manifest(
            "MVP runtime must list pdfLaTeX and XeLaTeX exactly once",
        ));
    }
    if manifest.archive.size_bytes == 0 {
        return Err(RuntimeTrustError::invalid_manifest(
            "archive size must be greater than zero",
        ));
    }
    validate_https_url(&manifest.archive.url)?;
    validate_sha256(&manifest.archive.sha256, "archive sha256")?;
    if manifest.signature.algorithm != ED25519_ALGORITHM
        || manifest.signature.canonicalization != RFC8785_CANONICALIZATION
    {
        return Err(RuntimeTrustError::invalid_manifest(
            "signature must use Ed25519 with RFC8785 canonicalization",
        ));
    }
    validate_identifier(&manifest.signature.key_id, "signature key id", 96)?;
    if manifest.signature.value.is_empty() {
        return Err(RuntimeTrustError::invalid_manifest(
            "manifest signature value is required",
        ));
    }
    validate_declared_path(&manifest.sbom.path, "SBOM path")?;
    validate_sha256(&manifest.sbom.sha256, "SBOM sha256")?;
    validate_declared_path(&manifest.license_inventory.path, "license inventory path")?;
    validate_sha256(
        &manifest.license_inventory.sha256,
        "license inventory sha256",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExtractionLimits {
    pub max_archive_bytes: u64,
    pub max_entries: u64,
    pub max_total_uncompressed_bytes: u64,
    pub max_single_file_bytes: u64,
    pub max_path_bytes: u64,
    pub max_depth: u64,
}

impl Default for ExtractionLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: 16 * 1024 * 1024 * 1024,
            max_entries: 1_000_000,
            max_total_uncompressed_bytes: 40 * 1024 * 1024 * 1024,
            max_single_file_bytes: 8 * 1024 * 1024 * 1024,
            max_path_bytes: 1_024,
            max_depth: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeInstallPhase {
    Validating,
    CopyingArchive,
    Extracting,
    VerifyingPayload,
    Committing,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstallProgress {
    pub profile_id: String,
    pub phase: RuntimeInstallPhase,
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct InstalledRuntime {
    profile_id: String,
    root: PathBuf,
    manifest_sha256: String,
}

impl InstalledRuntime {
    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        profile_id: impl Into<String>,
        root: impl Into<PathBuf>,
        manifest_sha256: impl Into<String>,
    ) -> Self {
        Self {
            profile_id: profile_id.into(),
            root: root.into(),
            manifest_sha256: manifest_sha256.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeInstallOutcome {
    Installed(InstalledRuntime),
    AlreadyInstalled(InstalledRuntime),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RuntimeProfileStatus {
    NotInstalled,
    Ready {
        profile_id: String,
        root: String,
        manifest_sha256: String,
    },
    Corrupt {
        profile_id: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct InstalledMarkerV1 {
    schema_version: u32,
    profile_id: String,
    manifest_sha256: String,
    archive_sha256: String,
    installed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct RuntimeInstaller {
    root: PathBuf,
    limits: ExtractionLimits,
}

impl RuntimeInstaller {
    pub fn new(root: impl Into<PathBuf>) -> RuntimeResult<Self> {
        Self::with_limits(root, ExtractionLimits::default())
    }

    pub fn with_limits(root: impl Into<PathBuf>, limits: ExtractionLimits) -> RuntimeResult<Self> {
        validate_limits(limits)?;
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|error| RuntimeTrustError::io("create runtime root", &root, error))?;
        let root = fs::canonicalize(&root)
            .map_err(|error| RuntimeTrustError::io("canonicalize runtime root", &root, error))?;
        Ok(Self { root, limits })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn install(
        &self,
        verified: &VerifiedRuntimeManifest,
        archive_path: &Path,
    ) -> RuntimeResult<RuntimeInstallOutcome> {
        self.install_with_progress(verified, archive_path, |_| {})
    }

    pub fn install_with_progress<F>(
        &self,
        verified: &VerifiedRuntimeManifest,
        archive_path: &Path,
        mut progress: F,
    ) -> RuntimeResult<RuntimeInstallOutcome>
    where
        F: FnMut(RuntimeInstallProgress),
    {
        let manifest = verified.manifest();
        emit_progress(&mut progress, manifest, RuntimeInstallPhase::Validating, 0);
        if manifest.archive.size_bytes > self.limits.max_archive_bytes {
            return Err(RuntimeTrustError::ArchiveLimit {
                limit: "compressed archive bytes".into(),
                actual: manifest.archive.size_bytes,
                maximum: self.limits.max_archive_bytes,
            });
        }

        let final_root = self.root.join(&manifest.profile_id);
        if final_root.exists() {
            validate_installation_root(&final_root, &manifest.profile_id)?;
            let installed = self.inspect_existing(verified, &final_root)?;
            emit_progress(
                &mut progress,
                manifest,
                RuntimeInstallPhase::Complete,
                manifest.archive.size_bytes,
            );
            return Ok(RuntimeInstallOutcome::AlreadyInstalled(installed));
        }

        let staging = tempfile::Builder::new()
            .prefix(".setwright-install-")
            .tempdir_in(&self.root)
            .map_err(|error| {
                RuntimeTrustError::io("create runtime staging directory", &self.root, error)
            })?;
        let staged_archive = staging.path().join("download.zip");
        emit_progress(
            &mut progress,
            manifest,
            RuntimeInstallPhase::CopyingArchive,
            0,
        );
        copy_and_verify_archive(archive_path, &staged_archive, manifest, |completed| {
            emit_progress(
                &mut progress,
                manifest,
                RuntimeInstallPhase::CopyingArchive,
                completed,
            );
        })?;

        let payload = staging.path().join("payload");
        fs::create_dir(&payload)
            .map_err(|error| RuntimeTrustError::io("create runtime payload", &payload, error))?;
        emit_progress(&mut progress, manifest, RuntimeInstallPhase::Extracting, 0);
        extract_archive(&staged_archive, &payload, self.limits, |completed| {
            emit_progress(
                &mut progress,
                manifest,
                RuntimeInstallPhase::Extracting,
                completed.min(manifest.archive.size_bytes),
            );
        })?;

        emit_progress(
            &mut progress,
            manifest,
            RuntimeInstallPhase::VerifyingPayload,
            manifest.archive.size_bytes,
        );
        verify_payload_document(&payload, &manifest.sbom.path, &manifest.sbom.sha256, "SBOM")?;
        verify_payload_document(
            &payload,
            &manifest.license_inventory.path,
            &manifest.license_inventory.sha256,
            "license inventory",
        )?;

        let manifest_bytes = serde_json_canonicalizer::to_vec(manifest).map_err(|error| {
            RuntimeTrustError::InvalidManifest {
                message: format!("manifest cannot be canonicalized: {error}"),
            }
        })?;
        write_synced_file(&payload.join(INSTALLED_MANIFEST), &manifest_bytes)?;
        let marker = InstalledMarkerV1 {
            schema_version: 1,
            profile_id: manifest.profile_id.clone(),
            manifest_sha256: verified.manifest_sha256.clone(),
            archive_sha256: manifest.archive.sha256.clone(),
            installed_at: chrono::Utc::now(),
        };
        let marker_bytes = serde_json_canonicalizer::to_vec(&marker).map_err(|error| {
            RuntimeTrustError::CorruptInstallation {
                profile_id: manifest.profile_id.clone(),
                message: format!("cannot serialize ready marker: {error}"),
            }
        })?;
        write_synced_file(&payload.join(INSTALL_MARKER), &marker_bytes)?;
        sync_directory(&payload)?;

        emit_progress(
            &mut progress,
            manifest,
            RuntimeInstallPhase::Committing,
            manifest.archive.size_bytes,
        );
        fs::rename(&payload, &final_root).map_err(|error| {
            if final_root.exists() {
                RuntimeTrustError::ImmutableProfileConflict {
                    profile_id: manifest.profile_id.clone(),
                }
            } else {
                RuntimeTrustError::io("atomically commit runtime", &final_root, error)
            }
        })?;
        sync_directory(&self.root)?;

        let installed = InstalledRuntime {
            profile_id: manifest.profile_id.clone(),
            root: final_root,
            manifest_sha256: verified.manifest_sha256.clone(),
        };
        emit_progress(
            &mut progress,
            manifest,
            RuntimeInstallPhase::Complete,
            manifest.archive.size_bytes,
        );
        Ok(RuntimeInstallOutcome::Installed(installed))
    }

    pub fn status(
        &self,
        profile_id: &str,
        expected_target: RuntimeTarget,
        trusted_keys: &TrustedRuntimeKeys,
    ) -> RuntimeProfileStatus {
        if let Err(error) = validate_identifier(profile_id, "profile id", 128) {
            return RuntimeProfileStatus::Corrupt {
                profile_id: profile_id.into(),
                message: error.to_string(),
            };
        }
        let root = self.root.join(profile_id);
        if !root.exists() {
            return RuntimeProfileStatus::NotInstalled;
        }
        match read_and_verify_installation(&root, profile_id, expected_target, trusted_keys) {
            Ok(installed) => RuntimeProfileStatus::Ready {
                profile_id: installed.profile_id,
                root: installed.root.to_string_lossy().into_owned(),
                manifest_sha256: installed.manifest_sha256,
            },
            Err(error) => RuntimeProfileStatus::Corrupt {
                profile_id: profile_id.into(),
                message: error.to_string(),
            },
        }
    }

    /// Re-open an installed profile after application restart. This repeats
    /// signature and declared-document verification before returning the
    /// private-field token accepted by the sandbox launch boundary.
    pub fn load_installed(
        &self,
        profile_id: &str,
        expected_target: RuntimeTarget,
        trusted_keys: &TrustedRuntimeKeys,
    ) -> RuntimeResult<InstalledRuntime> {
        validate_identifier(profile_id, "profile id", 128)?;
        let root = self.root.join(profile_id);
        if !root.exists() {
            return Err(RuntimeTrustError::CorruptInstallation {
                profile_id: profile_id.into(),
                message: "profile is not installed".into(),
            });
        }
        read_and_verify_installation(&root, profile_id, expected_target, trusted_keys)
    }

    fn inspect_existing(
        &self,
        verified: &VerifiedRuntimeManifest,
        final_root: &Path,
    ) -> RuntimeResult<InstalledRuntime> {
        let marker = read_marker(final_root, verified.profile_id())?;
        if marker.profile_id != verified.profile_id()
            || marker.manifest_sha256 != verified.manifest_sha256
            || marker.archive_sha256 != verified.manifest.archive.sha256
        {
            return Err(RuntimeTrustError::ImmutableProfileConflict {
                profile_id: verified.profile_id().into(),
            });
        }
        let stored_manifest = fs::read(final_root.join(INSTALLED_MANIFEST)).map_err(|error| {
            RuntimeTrustError::io(
                "read installed runtime manifest",
                &final_root.join(INSTALLED_MANIFEST),
                error,
            )
        })?;
        let stored_hash = sha256_hex(&stored_manifest);
        if stored_hash != verified.manifest_sha256 {
            return Err(RuntimeTrustError::ImmutableProfileConflict {
                profile_id: verified.profile_id().into(),
            });
        }
        verify_payload_document(
            final_root,
            &verified.manifest.sbom.path,
            &verified.manifest.sbom.sha256,
            "SBOM",
        )?;
        verify_payload_document(
            final_root,
            &verified.manifest.license_inventory.path,
            &verified.manifest.license_inventory.sha256,
            "license inventory",
        )?;
        Ok(InstalledRuntime {
            profile_id: verified.profile_id().into(),
            root: final_root.to_path_buf(),
            manifest_sha256: verified.manifest_sha256.clone(),
        })
    }
}

fn read_and_verify_installation(
    root: &Path,
    profile_id: &str,
    expected_target: RuntimeTarget,
    trusted_keys: &TrustedRuntimeKeys,
) -> RuntimeResult<InstalledRuntime> {
    validate_installation_root(root, profile_id)?;
    let manifest_path = root.join(INSTALLED_MANIFEST);
    let bytes = fs::read(&manifest_path)
        .map_err(|error| RuntimeTrustError::io("read installed manifest", &manifest_path, error))?;
    let verified = verify_runtime_manifest_json(&bytes, expected_target, trusted_keys)?;
    if verified.profile_id() != profile_id {
        return Err(RuntimeTrustError::CorruptInstallation {
            profile_id: profile_id.into(),
            message: "installed manifest profile id does not match its directory".into(),
        });
    }
    let marker = read_marker(root, profile_id)?;
    if marker.schema_version != 1
        || marker.profile_id != profile_id
        || marker.manifest_sha256 != verified.manifest_sha256
        || marker.archive_sha256 != verified.manifest.archive.sha256
    {
        return Err(RuntimeTrustError::CorruptInstallation {
            profile_id: profile_id.into(),
            message: "ready marker does not match the signed manifest".into(),
        });
    }
    verify_payload_document(
        root,
        &verified.manifest.sbom.path,
        &verified.manifest.sbom.sha256,
        "SBOM",
    )?;
    verify_payload_document(
        root,
        &verified.manifest.license_inventory.path,
        &verified.manifest.license_inventory.sha256,
        "license inventory",
    )?;
    Ok(InstalledRuntime {
        profile_id: profile_id.into(),
        root: root.to_path_buf(),
        manifest_sha256: verified.manifest_sha256,
    })
}

fn emit_progress<F>(
    progress: &mut F,
    manifest: &RuntimeManifestV1,
    phase: RuntimeInstallPhase,
    completed_bytes: u64,
) where
    F: FnMut(RuntimeInstallProgress),
{
    progress(RuntimeInstallProgress {
        profile_id: manifest.profile_id.clone(),
        phase,
        completed_bytes,
        total_bytes: manifest.archive.size_bytes,
    });
}

fn copy_and_verify_archive<F>(
    source: &Path,
    destination: &Path,
    manifest: &RuntimeManifestV1,
    mut progress: F,
) -> RuntimeResult<()>
where
    F: FnMut(u64),
{
    let metadata = fs::metadata(source)
        .map_err(|error| RuntimeTrustError::io("inspect runtime archive", source, error))?;
    if !metadata.is_file() {
        return Err(RuntimeTrustError::Archive {
            message: "runtime archive is not a regular file".into(),
        });
    }
    if metadata.len() != manifest.archive.size_bytes {
        return Err(RuntimeTrustError::ArchiveSizeMismatch {
            expected: manifest.archive.size_bytes,
            actual: metadata.len(),
        });
    }
    let mut input = File::open(source)
        .map_err(|error| RuntimeTrustError::io("open runtime archive", source, error))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| {
            RuntimeTrustError::io("create staged runtime archive", destination, error)
        })?;
    let mut hasher = Sha256::new();
    let mut completed = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| RuntimeTrustError::io("read runtime archive", source, error))?;
        if read == 0 {
            break;
        }
        completed =
            completed
                .checked_add(read as u64)
                .ok_or_else(|| RuntimeTrustError::Archive {
                    message: "runtime archive byte count overflow".into(),
                })?;
        if completed > manifest.archive.size_bytes {
            return Err(RuntimeTrustError::ArchiveSizeMismatch {
                expected: manifest.archive.size_bytes,
                actual: completed,
            });
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read]).map_err(|error| {
            RuntimeTrustError::io("write staged runtime archive", destination, error)
        })?;
        progress(completed);
    }
    output.flush().map_err(|error| {
        RuntimeTrustError::io("flush staged runtime archive", destination, error)
    })?;
    output.sync_all().map_err(|error| {
        RuntimeTrustError::io("sync staged runtime archive", destination, error)
    })?;
    if completed != manifest.archive.size_bytes {
        return Err(RuntimeTrustError::ArchiveSizeMismatch {
            expected: manifest.archive.size_bytes,
            actual: completed,
        });
    }
    let actual = hex::encode(hasher.finalize());
    if actual != manifest.archive.sha256 {
        return Err(RuntimeTrustError::ArchiveHashMismatch {
            expected: manifest.archive.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone)]
struct ArchiveEntryPlan {
    index: usize,
    relative: PathBuf,
    normalized_key: String,
    kind: ArchiveEntryKind,
    size: u64,
    unix_mode: Option<u32>,
}

fn extract_archive<F>(
    archive_path: &Path,
    destination: &Path,
    limits: ExtractionLimits,
    mut progress: F,
) -> RuntimeResult<()>
where
    F: FnMut(u64),
{
    let archive_file = File::open(archive_path).map_err(|error| {
        RuntimeTrustError::io("open staged runtime archive", archive_path, error)
    })?;
    let mut archive =
        zip::ZipArchive::new(archive_file).map_err(|error| RuntimeTrustError::Archive {
            message: format!("invalid zip archive: {error}"),
        })?;
    let plans = inspect_archive(&mut archive, limits)?;
    let mut extracted = 0_u64;
    for plan in plans {
        let output_path = destination.join(&plan.relative);
        match plan.kind {
            ArchiveEntryKind::Directory => {
                fs::create_dir_all(&output_path).map_err(|error| {
                    RuntimeTrustError::io("create runtime archive directory", &output_path, error)
                })?;
            }
            ArchiveEntryKind::File => {
                let parent = output_path.parent().ok_or_else(|| {
                    RuntimeTrustError::unsafe_entry(
                        plan.relative.to_string_lossy(),
                        "entry has no parent directory",
                    )
                })?;
                fs::create_dir_all(parent).map_err(|error| {
                    RuntimeTrustError::io("create runtime archive parent", parent, error)
                })?;
                let mut entry =
                    archive
                        .by_index(plan.index)
                        .map_err(|error| RuntimeTrustError::Archive {
                            message: format!("cannot reopen zip entry {}: {error}", plan.index),
                        })?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output_path)
                    .map_err(|error| {
                        RuntimeTrustError::io("create extracted runtime file", &output_path, error)
                    })?;
                let copied = copy_bounded(
                    &mut entry,
                    &mut output,
                    plan.size,
                    &mut extracted,
                    &mut progress,
                )?;
                if copied != plan.size {
                    return Err(RuntimeTrustError::UnsafeArchiveEntry {
                        entry: plan.relative.to_string_lossy().into_owned(),
                        reason: format!(
                            "uncompressed size changed during extraction: expected {}, got {copied}",
                            plan.size
                        ),
                    });
                }
                output.flush().map_err(|error| {
                    RuntimeTrustError::io("flush extracted runtime file", &output_path, error)
                })?;
                output.sync_all().map_err(|error| {
                    RuntimeTrustError::io("sync extracted runtime file", &output_path, error)
                })?;
                apply_safe_permissions(&output_path, plan.unix_mode)?;
            }
        }
    }
    Ok(())
}

fn inspect_archive<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    limits: ExtractionLimits,
) -> RuntimeResult<Vec<ArchiveEntryPlan>> {
    if archive.len() as u64 > limits.max_entries {
        return Err(RuntimeTrustError::ArchiveLimit {
            limit: "entry count".into(),
            actual: archive.len() as u64,
            maximum: limits.max_entries,
        });
    }
    let mut total = 0_u64;
    let mut seen = BTreeMap::<String, ArchiveEntryKind>::new();
    let mut plans = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| RuntimeTrustError::Archive {
                message: format!("cannot inspect zip entry {index}: {error}"),
            })?;
        if entry.encrypted() {
            return Err(RuntimeTrustError::unsafe_entry(
                entry.name(),
                "encrypted entries are not supported",
            ));
        }
        if entry.is_symlink() {
            return Err(RuntimeTrustError::unsafe_entry(
                entry.name(),
                "symbolic links are forbidden",
            ));
        }
        let kind = if entry.is_dir() {
            ArchiveEntryKind::Directory
        } else {
            ArchiveEntryKind::File
        };
        validate_unix_file_type(entry.name(), entry.unix_mode(), kind)?;
        let (relative, normalized_key) = validate_archive_entry_name(entry.name(), limits)?;
        if seen.insert(normalized_key.clone(), kind).is_some() {
            return Err(RuntimeTrustError::unsafe_entry(
                entry.name(),
                "duplicate or case-colliding entry",
            ));
        }
        if kind == ArchiveEntryKind::File && entry.size() > limits.max_single_file_bytes {
            return Err(RuntimeTrustError::ArchiveLimit {
                limit: "single uncompressed file bytes".into(),
                actual: entry.size(),
                maximum: limits.max_single_file_bytes,
            });
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| RuntimeTrustError::ArchiveLimit {
                limit: "total uncompressed bytes".into(),
                actual: u64::MAX,
                maximum: limits.max_total_uncompressed_bytes,
            })?;
        if total > limits.max_total_uncompressed_bytes {
            return Err(RuntimeTrustError::ArchiveLimit {
                limit: "total uncompressed bytes".into(),
                actual: total,
                maximum: limits.max_total_uncompressed_bytes,
            });
        }
        plans.push(ArchiveEntryPlan {
            index,
            relative,
            normalized_key,
            kind,
            size: entry.size(),
            unix_mode: entry.unix_mode(),
        });
    }

    for plan in &plans {
        let components: Vec<_> = plan.normalized_key.split('/').collect();
        for depth in 1..components.len() {
            let ancestor = components[..depth].join("/");
            if seen.get(&ancestor) == Some(&ArchiveEntryKind::File) {
                return Err(RuntimeTrustError::unsafe_entry(
                    &plan.normalized_key,
                    format!("entry is nested beneath file {ancestor}"),
                ));
            }
        }
    }
    Ok(plans)
}

fn validate_archive_entry_name(
    raw_name: &str,
    limits: ExtractionLimits,
) -> RuntimeResult<(PathBuf, String)> {
    if raw_name.is_empty() || raw_name.as_bytes().contains(&0) {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "empty or NUL path",
        ));
    }
    if raw_name.len() as u64 > limits.max_path_bytes {
        return Err(RuntimeTrustError::ArchiveLimit {
            limit: "archive path bytes".into(),
            actual: raw_name.len() as u64,
            maximum: limits.max_path_bytes,
        });
    }
    let normalized_slashes = raw_name.replace('\\', "/");
    let lower = normalized_slashes.to_lowercase();
    if raw_name.contains('\\')
        || normalized_slashes.starts_with('/')
        || lower.starts_with("//?/")
        || lower.starts_with("//./")
        || lower.starts_with("globalroot/")
    {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "absolute, UNC, or device path",
        ));
    }
    let without_directory_suffix = normalized_slashes.trim_end_matches('/');
    if without_directory_suffix.is_empty() || without_directory_suffix.contains("//") {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "empty path component",
        ));
    }
    let components: Vec<_> = without_directory_suffix.split('/').collect();
    if components.len() as u64 > limits.max_depth {
        return Err(RuntimeTrustError::ArchiveLimit {
            limit: "archive path depth".into(),
            actual: components.len() as u64,
            maximum: limits.max_depth,
        });
    }
    for component in &components {
        validate_archive_component(raw_name, component)?;
    }
    let relative: PathBuf = components.iter().collect();
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "path traversal or platform prefix",
        ));
    }
    Ok((relative, components.join("/").to_lowercase()))
}

fn validate_archive_component(raw_name: &str, component: &str) -> RuntimeResult<()> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "empty or traversal component",
        ));
    }
    if component.starts_with('.') {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "hidden paths are forbidden",
        ));
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "trailing dots or spaces are forbidden",
        ));
    }
    if component.chars().any(|character| {
        character <= '\u{1f}' || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
    }) {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "control characters, device syntax, and alternate data streams are forbidden",
        ));
    }
    let device_stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    let numbered_device = (device_stem.starts_with("COM") || device_stem.starts_with("LPT"))
        && device_stem.get(3..).is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        });
    if matches!(
        device_stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$"
    ) || numbered_device
    {
        return Err(RuntimeTrustError::unsafe_entry(
            raw_name,
            "reserved device name",
        ));
    }
    Ok(())
}

fn validate_unix_file_type(
    name: &str,
    unix_mode: Option<u32>,
    kind: ArchiveEntryKind,
) -> RuntimeResult<()> {
    let Some(mode) = unix_mode else {
        return Ok(());
    };
    let file_type = mode & 0o170000;
    let expected = match kind {
        ArchiveEntryKind::File => 0o100000,
        ArchiveEntryKind::Directory => 0o040000,
    };
    if file_type != 0 && file_type != expected {
        return Err(RuntimeTrustError::unsafe_entry(
            name,
            "special files, devices, sockets, and pipes are forbidden",
        ));
    }
    Ok(())
}

fn copy_bounded<R: Read, W: Write, F: FnMut(u64)>(
    reader: &mut R,
    writer: &mut W,
    declared_size: u64,
    total_extracted: &mut u64,
    progress: &mut F,
) -> RuntimeResult<u64> {
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let remaining = declared_size.saturating_sub(copied);
        let allowance = remaining.saturating_add(1).min(buffer.len() as u64) as usize;
        let read =
            reader
                .read(&mut buffer[..allowance])
                .map_err(|error| RuntimeTrustError::Archive {
                    message: format!("cannot decompress runtime archive entry: {error}"),
                })?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| RuntimeTrustError::Archive {
                message: "extracted file byte count overflow".into(),
            })?;
        if copied > declared_size {
            return Err(RuntimeTrustError::Archive {
                message: "archive entry expanded beyond its declared size".into(),
            });
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| RuntimeTrustError::Archive {
                message: format!("cannot write runtime archive entry: {error}"),
            })?;
        *total_extracted =
            total_extracted
                .checked_add(read as u64)
                .ok_or_else(|| RuntimeTrustError::Archive {
                    message: "total extracted byte count overflow".into(),
                })?;
        progress(*total_extracted);
    }
    Ok(copied)
}

fn verify_payload_document(
    payload: &Path,
    relative: &str,
    expected_sha256: &str,
    label: &str,
) -> RuntimeResult<()> {
    let path = payload.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let canonical_payload = fs::canonicalize(payload)
        .map_err(|error| RuntimeTrustError::io("canonicalize runtime payload", payload, error))?;
    let canonical_path =
        fs::canonicalize(&path).map_err(|error| RuntimeTrustError::CorruptInstallation {
            profile_id: payload
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            message: format!("{label} is missing at {relative}: {error}"),
        })?;
    if !canonical_path.starts_with(&canonical_payload) {
        return Err(RuntimeTrustError::CorruptInstallation {
            profile_id: payload
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            message: format!("{label} resolves outside the runtime profile"),
        });
    }
    reject_symlink_components(&canonical_payload, relative, label)?;
    let metadata =
        fs::symlink_metadata(&path).map_err(|error| RuntimeTrustError::CorruptInstallation {
            profile_id: payload
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            message: format!("{label} is missing at {relative}: {error}"),
        })?;
    if !metadata.file_type().is_file() {
        return Err(RuntimeTrustError::CorruptInstallation {
            profile_id: payload
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            message: format!("{label} is not a regular file"),
        });
    }
    let actual = hash_file(&path)?;
    if actual != expected_sha256 {
        return Err(RuntimeTrustError::CorruptInstallation {
            profile_id: payload
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            message: format!("{label} hash mismatch: expected {expected_sha256}, got {actual}"),
        });
    }
    Ok(())
}

fn validate_installation_root(root: &Path, profile_id: &str) -> RuntimeResult<()> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| RuntimeTrustError::io("inspect installed runtime root", root, error))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeTrustError::CorruptInstallation {
            profile_id: profile_id.into(),
            message: "profile root must be a real directory, not a symlink".into(),
        });
    }
    Ok(())
}

fn reject_symlink_components(root: &Path, relative: &str, label: &str) -> RuntimeResult<()> {
    let mut current = root.to_path_buf();
    for component in relative.split('/') {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            RuntimeTrustError::io("inspect runtime document path", &current, error)
        })?;
        if metadata.file_type().is_symlink() {
            return Err(RuntimeTrustError::CorruptInstallation {
                profile_id: root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                message: format!("{label} path contains a symbolic link"),
            });
        }
    }
    Ok(())
}

fn read_marker(root: &Path, profile_id: &str) -> RuntimeResult<InstalledMarkerV1> {
    let marker_path = root.join(INSTALL_MARKER);
    let bytes = fs::read(&marker_path)
        .map_err(|error| RuntimeTrustError::io("read runtime ready marker", &marker_path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| RuntimeTrustError::CorruptInstallation {
        profile_id: profile_id.into(),
        message: format!("ready marker is invalid: {error}"),
    })
}

fn hash_file(path: &Path) -> RuntimeResult<String> {
    let mut file = File::open(path)
        .map_err(|error| RuntimeTrustError::io("open file for hashing", path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| RuntimeTrustError::io("hash file", path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> RuntimeResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| RuntimeTrustError::io("create runtime metadata", path, error))?;
    file.write_all(bytes)
        .map_err(|error| RuntimeTrustError::io("write runtime metadata", path, error))?;
    file.flush()
        .map_err(|error| RuntimeTrustError::io("flush runtime metadata", path, error))?;
    file.sync_all()
        .map_err(|error| RuntimeTrustError::io("sync runtime metadata", path, error))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> RuntimeResult<()> {
    let directory = File::open(path)
        .map_err(|error| RuntimeTrustError::io("open directory for sync", path, error))?;
    directory
        .sync_all()
        .map_err(|error| RuntimeTrustError::io("sync directory", path, error))
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> RuntimeResult<()> {
    use std::os::windows::fs::OpenOptionsExt;

    // FILE_FLAG_BACKUP_SEMANTICS is required to obtain a directory handle.
    // Windows does not guarantee that FlushFileBuffers accepts directory
    // handles, so the durability boundary is each already-synced file plus the
    // same-volume atomic rename below.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let _directory = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|error| RuntimeTrustError::io("open directory barrier", path, error))?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_path: &Path) -> RuntimeResult<()> {
    Ok(())
}

#[cfg(unix)]
fn apply_safe_permissions(path: &Path, unix_mode: Option<u32>) -> RuntimeResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = unix_mode.unwrap_or(0o644) & 0o755;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| RuntimeTrustError::io("set extracted runtime permissions", path, error))
}

#[cfg(not(unix))]
fn apply_safe_permissions(_path: &Path, _unix_mode: Option<u32>) -> RuntimeResult<()> {
    Ok(())
}

fn validate_limits(limits: ExtractionLimits) -> RuntimeResult<()> {
    if limits.max_archive_bytes == 0
        || limits.max_entries == 0
        || limits.max_total_uncompressed_bytes == 0
        || limits.max_single_file_bytes == 0
        || limits.max_single_file_bytes > limits.max_total_uncompressed_bytes
        || limits.max_path_bytes < 16
        || limits.max_depth == 0
    {
        return Err(RuntimeTrustError::invalid_manifest(
            "runtime extraction limits are internally inconsistent",
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str, max_length: usize) -> RuntimeResult<()> {
    let valid = !value.is_empty()
        && value.len() <= max_length
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(RuntimeTrustError::invalid_manifest(format!(
            "{label} has an invalid identifier format"
        )))
    }
}

fn validate_sha256(value: &str, label: &str) -> RuntimeResult<()> {
    if value.len() == SHA256_HEX_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(RuntimeTrustError::invalid_manifest(format!(
            "{label} must be 64 lowercase hexadecimal characters"
        )))
    }
}

fn validate_https_url(value: &str) -> RuntimeResult<()> {
    let url = url::Url::parse(value).map_err(|error| {
        RuntimeTrustError::invalid_manifest(format!("archive URL is invalid: {error}"))
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(RuntimeTrustError::invalid_manifest(
            "archive URL must be an HTTPS origin URL without credentials or fragment",
        ));
    }
    Ok(())
}

fn validate_declared_path(value: &str, label: &str) -> RuntimeResult<()> {
    let limits = ExtractionLimits::default();
    validate_archive_entry_name(value, limits)
        .map(|_| ())
        .map_err(|error| RuntimeTrustError::invalid_manifest(format!("{label}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::{
        LocalDocument, RuntimeArtifact, RuntimeSbom, RuntimeSignature, SbomFormat, TexLiveSnapshot,
    };
    use chrono::TimeZone;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::{Cursor, Seek};
    use zip::write::SimpleFileOptions;

    const KEY_ID: &str = "setwright-test-1";
    const SBOM: &[u8] = br#"{"spdxVersion":"SPDX-2.3"}"#;
    const LICENSES: &[u8] = b"Apache-2.0\n";

    fn target() -> RuntimeTarget {
        RuntimeTarget {
            platform: RuntimePlatform::Windows,
            architecture: RuntimeArchitecture::X86_64,
        }
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            for (path, contents) in entries {
                writer
                    .start_file(*path, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(contents).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.rewind().unwrap();
        bytes.into_inner()
    }

    fn good_archive() -> Vec<u8> {
        archive(&[
            ("docs/runtime.spdx.json", SBOM),
            ("docs/licenses.json", LICENSES),
            ("bin/latexmk.exe", b"test binary"),
        ])
    }

    fn unsigned_manifest(archive: &[u8]) -> RuntimeManifestV1 {
        RuntimeManifestV1 {
            schema_version: 1,
            profile_id: "texlive-2025.2025-08-03-windows-x86_64".into(),
            tex_live_snapshot: TexLiveSnapshot {
                version: 2025,
                date: chrono::NaiveDate::from_ymd_opt(2025, 8, 3).unwrap(),
            },
            platform: RuntimePlatform::Windows,
            architecture: RuntimeArchitecture::X86_64,
            engines: vec![LatexEngine::PdfLatex, LatexEngine::XeLatex],
            archive: RuntimeArtifact {
                url: "https://runtime.setwright.org/test.zip".into(),
                size_bytes: archive.len() as u64,
                sha256: sha256_hex(archive),
            },
            signature: RuntimeSignature {
                algorithm: ED25519_ALGORITHM.into(),
                canonicalization: RFC8785_CANONICALIZATION.into(),
                key_id: KEY_ID.into(),
                value: String::new(),
            },
            sbom: RuntimeSbom {
                path: "docs/runtime.spdx.json".into(),
                sha256: sha256_hex(SBOM),
                format: SbomFormat::SpdxJson23,
            },
            license_inventory: LocalDocument {
                path: "docs/licenses.json".into(),
                sha256: sha256_hex(LICENSES),
            },
            created_at: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    fn sign(mut manifest: RuntimeManifestV1, signing_key: &SigningKey) -> RuntimeManifestV1 {
        let payload = canonical_manifest_payload(&manifest).unwrap();
        manifest.signature.value =
            base64::engine::general_purpose::STANDARD.encode(signing_key.sign(&payload).to_bytes());
        manifest
    }

    fn keys(signing_key: &SigningKey) -> TrustedRuntimeKeys {
        let mut keys = TrustedRuntimeKeys::new();
        keys.insert(KEY_ID, signing_key.verifying_key().to_bytes())
            .unwrap();
        keys
    }

    #[test]
    fn canonical_payload_blanks_only_signature_value() {
        let archive = good_archive();
        let mut manifest = unsigned_manifest(&archive);
        manifest.signature.value = "not-part-of-the-payload".into();
        let payload = String::from_utf8(canonical_manifest_payload(&manifest).unwrap()).unwrap();
        assert!(payload.contains("\"value\":\"\""));
        assert!(!payload.contains("not-part-of-the-payload"));
        assert!(payload.starts_with('{'));
    }

    #[test]
    fn rejects_invalid_signature_and_unknown_key() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let other_key = SigningKey::from_bytes(&[9; 32]);
        let archive = good_archive();
        let manifest = sign(unsigned_manifest(&archive), &other_key);
        assert!(matches!(
            verify_runtime_manifest(manifest.clone(), target(), &keys(&signing_key)),
            Err(RuntimeTrustError::InvalidSignature { .. })
        ));
        assert!(matches!(
            verify_runtime_manifest(manifest, target(), &TrustedRuntimeKeys::new()),
            Err(RuntimeTrustError::UnknownKey { .. })
        ));
    }

    #[test]
    fn strict_json_verification_rejects_unknown_fields_and_signs_received_values() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let archive = good_archive();
        let manifest = unsigned_manifest(&archive);
        let mut value = serde_json::to_value(manifest).unwrap();
        value["createdAt"] = serde_json::Value::String("2026-01-01T00:00:00+00:00".into());
        value["signature"]["value"] = serde_json::Value::String(String::new());
        let payload = serde_json_canonicalizer::to_vec(&value).unwrap();
        value["signature"]["value"] = serde_json::Value::String(
            base64::engine::general_purpose::STANDARD.encode(signing_key.sign(&payload).to_bytes()),
        );
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(
            verify_runtime_manifest_json(&bytes, target(), &keys(&signing_key)).is_ok(),
            "verification must use the received RFC8785 values without timestamp reformatting"
        );

        value["unexpected"] = serde_json::Value::Bool(true);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            verify_runtime_manifest_json(&bytes, target(), &keys(&signing_key)),
            Err(RuntimeTrustError::InvalidManifest { .. })
        ));
    }

    #[test]
    fn rejects_unapproved_snapshot_and_wrong_target() {
        let archive = good_archive();
        let mut manifest = unsigned_manifest(&archive);
        manifest.tex_live_snapshot.date = chrono::NaiveDate::from_ymd_opt(2025, 8, 4).unwrap();
        assert!(matches!(
            validate_runtime_manifest(&manifest, target()),
            Err(RuntimeTrustError::InvalidManifest { .. })
        ));
        let manifest = unsigned_manifest(&archive);
        assert!(matches!(
            validate_runtime_manifest(
                &manifest,
                RuntimeTarget {
                    platform: RuntimePlatform::Linux,
                    architecture: RuntimeArchitecture::X86_64,
                }
            ),
            Err(RuntimeTrustError::InvalidManifest { .. })
        ));
    }

    #[test]
    fn rejects_archive_checksum_before_extraction() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let archive = good_archive();
        let mut manifest = unsigned_manifest(&archive);
        manifest.archive.sha256 = "0".repeat(64);
        let verified =
            verify_runtime_manifest(sign(manifest, &signing_key), target(), &keys(&signing_key))
                .unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(source.path(), &archive).unwrap();
        let root = tempfile::tempdir().unwrap();
        let installer = RuntimeInstaller::new(root.path()).unwrap();
        assert!(matches!(
            installer.install(&verified, source.path()),
            Err(RuntimeTrustError::ArchiveHashMismatch { .. })
        ));
        assert!(!root.path().join(verified.profile_id()).exists());
    }

    #[test]
    fn rejects_traversal_hidden_device_case_collision_and_symlink_entries() {
        let limits = ExtractionLimits::default();
        for malicious in [
            "../escape",
            "/absolute",
            "C:/windows/system32",
            "safe/.hidden",
            "safe/CON.txt",
            "safe/file:stream",
        ] {
            let bytes = archive(&[(malicious, b"bad")]);
            let reader = Cursor::new(bytes);
            let mut zip = zip::ZipArchive::new(reader).unwrap();
            assert!(
                matches!(
                    inspect_archive(&mut zip, limits),
                    Err(RuntimeTrustError::UnsafeArchiveEntry { .. })
                ),
                "{malicious} should be rejected"
            );
        }

        let bytes = archive(&[("bin/Tool.exe", b"a"), ("bin/tool.EXE", b"b")]);
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            inspect_archive(&mut zip, limits),
            Err(RuntimeTrustError::UnsafeArchiveEntry { .. })
        ));

        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .add_symlink(
                    "bin/link",
                    "../../outside",
                    SimpleFileOptions::default().unix_permissions(0o777),
                )
                .unwrap();
            writer.finish().unwrap();
        }
        bytes.rewind().unwrap();
        let mut zip = zip::ZipArchive::new(bytes).unwrap();
        assert!(matches!(
            inspect_archive(&mut zip, limits),
            Err(RuntimeTrustError::UnsafeArchiveEntry { .. })
        ));
    }

    #[test]
    fn installs_atomically_and_reports_ready_only_after_commit() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let archive = good_archive();
        let verified = verify_runtime_manifest(
            sign(unsigned_manifest(&archive), &signing_key),
            target(),
            &keys(&signing_key),
        )
        .unwrap();
        let download = tempfile::NamedTempFile::new().unwrap();
        fs::write(download.path(), &archive).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let installer = RuntimeInstaller::new(app_data.path()).unwrap();
        let mut phases = Vec::new();
        let result = installer
            .install_with_progress(&verified, download.path(), |event| {
                phases.push(event.phase);
            })
            .unwrap();
        let installed = match result {
            RuntimeInstallOutcome::Installed(installed) => installed,
            RuntimeInstallOutcome::AlreadyInstalled(_) => panic!("new install reported existing"),
        };
        assert!(installed.root().join("bin/latexmk.exe").is_file());
        assert_eq!(phases.last(), Some(&RuntimeInstallPhase::Complete));
        assert!(matches!(
            installer.status(verified.profile_id(), target(), &keys(&signing_key)),
            RuntimeProfileStatus::Ready { .. }
        ));
        let reopened = installer
            .load_installed(verified.profile_id(), target(), &keys(&signing_key))
            .unwrap();
        assert_eq!(reopened.manifest_sha256(), verified.manifest_sha256());

        assert!(matches!(
            installer.install(&verified, download.path()).unwrap(),
            RuntimeInstallOutcome::AlreadyInstalled(_)
        ));
    }

    #[test]
    fn failed_reinstall_never_replaces_an_existing_profile() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let archive = good_archive();
        let verified = verify_runtime_manifest(
            sign(unsigned_manifest(&archive), &signing_key),
            target(),
            &keys(&signing_key),
        )
        .unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let existing = app_data.path().join(verified.profile_id());
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("sentinel"), b"old state").unwrap();
        let download = tempfile::NamedTempFile::new().unwrap();
        fs::write(download.path(), &archive).unwrap();
        let installer = RuntimeInstaller::new(app_data.path()).unwrap();
        assert!(installer.install(&verified, download.path()).is_err());
        assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"old state");
        assert!(!existing.join("bin/latexmk.exe").exists());
    }
}
