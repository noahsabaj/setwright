use crate::core::contracts::{
    Diagnostic, DocumentOp, FileId, PaperSettingsV1, ProjectEvent, ProjectFile, ProjectSessionId,
    Revision, SourceEdit, SourceSpan, TemplateId, TextMark,
};
use crate::core::error::{AppError, AppResult};
use crate::core::latex::{
    CompatibilityReport, IncludeGraph, LatexAnalysis, LatexParser, ProjectLayout,
    build_include_graph, build_include_graph_with_overlays, discover_project_bibliographies,
    ensure_within, normalized_relative, projection_covers_source, safe_relative_path,
};
use crate::core::persistence::{TransactionWrite, save_transaction};
use crate::core::source::{SourceBuffer, hash_bytes};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_SOURCE_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
    pub session_id: ProjectSessionId,
    pub root_path: String,
    pub main_file_id: FileId,
    pub revision: Revision,
    pub files: Vec<ProjectFile>,
    pub include_graph: IncludeGraph,
    pub include_graph_stale: bool,
    pub compatibility: BTreeMap<FileId, CompatibilityReport>,
    pub settings: Option<PaperSettingsV1>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileContents {
    pub file: ProjectFile,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditResult {
    pub revision: Revision,
    pub changed: bool,
    pub changed_files: Vec<FileId>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub revision: Revision,
    pub saved_files: Vec<FileId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExternalChangeResult {
    Unchanged,
    Reloaded {
        revision: Revision,
        file_id: FileId,
    },
    Conflict {
        file_id: FileId,
        relative_path: String,
        base_bytes: Vec<u8>,
        local_bytes: Vec<u8>,
        external_bytes: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewProjectSpec {
    pub settings: PaperSettingsV1,
    pub title: String,
    pub authors: Vec<String>,
}

/// One native-window project session. All edit methods require `&mut self`,
/// making the command adapter the single serialization point for a project.
#[derive(Debug)]
pub struct ProjectSession {
    session_id: ProjectSessionId,
    root: PathBuf,
    main_file_id: FileId,
    revision: Revision,
    files: BTreeMap<FileId, SourceBuffer>,
    path_to_id: BTreeMap<String, FileId>,
    persisted_bytes: BTreeMap<FileId, Vec<u8>>,
    pending_external_bytes: BTreeMap<FileId, Vec<u8>>,
    analyses: BTreeMap<FileId, LatexAnalysis>,
    include_graph: IncludeGraph,
    include_graph_stale: bool,
    settings: Option<PaperSettingsV1>,
    parser: LatexParser,
    closed: bool,
}

#[derive(Debug)]
struct PreparedSource {
    normalized_path: String,
    file_id: FileId,
    buffer: SourceBuffer,
    analysis: Option<LatexAnalysis>,
}

#[derive(Debug)]
struct PreparedIncludeGraph {
    graph: IncludeGraph,
    new_sources: Vec<PreparedSource>,
}

impl ProjectSession {
    /// Opens without writing project files or application metadata.
    pub fn open(layout: ProjectLayout) -> AppResult<Self> {
        let mut parser = LatexParser::new()?;
        let main_relative = layout.main_relative()?;
        let include_graph = build_include_graph(&layout.root, &main_relative, &mut parser)?;
        let mut files = BTreeMap::new();
        let mut path_to_id = BTreeMap::new();
        let mut persisted_bytes = BTreeMap::new();
        let mut analyses = BTreeMap::new();

        let mut source_paths: std::collections::BTreeSet<String> =
            include_graph.nodes.keys().cloned().collect();
        source_paths.extend(
            discover_project_bibliographies(&layout.root)?
                .iter()
                .map(|path| normalized_relative(path)),
        );
        for relative_string in &source_paths {
            let relative = safe_relative_path(relative_string)?;
            let file_id = FileId::new();
            let bytes = read_source_file_bounded(&layout.root, &relative)?;
            let buffer = SourceBuffer::from_bytes(file_id, relative, bytes, Revision::INITIAL);
            if !buffer.is_source_only()
                && buffer
                    .relative_path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
            {
                analyses.insert(file_id, parser.parse(file_id, buffer.bytes())?);
            }
            persisted_bytes.insert(file_id, buffer.bytes().to_vec());
            path_to_id.insert(relative_string.clone(), file_id);
            files.insert(file_id, buffer);
        }
        let main_key = normalized_relative(&main_relative);
        let main_file_id = *path_to_id
            .get(&main_key)
            .ok_or_else(|| AppError::InvalidProject {
                message: "main file was not loaded into the include graph".into(),
            })?;
        Ok(Self {
            session_id: ProjectSessionId::new(),
            root: layout.root,
            main_file_id,
            revision: Revision::INITIAL,
            files,
            path_to_id,
            persisted_bytes,
            pending_external_bytes: BTreeMap::new(),
            analyses,
            include_graph,
            include_graph_stale: false,
            settings: layout.settings,
            parser,
            closed: false,
        })
    }

    pub fn open_path(path: impl AsRef<Path>) -> AppResult<Self> {
        Self::open(ProjectLayout::discover(path)?)
    }

    /// Creates a Setwright-owned project and then opens it. Imported projects
    /// never call this path and therefore never receive `paper-settings.json`.
    pub fn create(
        root: impl AsRef<Path>,
        spec: NewProjectSpec,
        recovery_directory: &Path,
    ) -> AppResult<Self> {
        let requested_root = root.as_ref();
        if !spec.settings.is_valid() {
            return Err(AppError::InvalidProject {
                message: "new project settings violate the canonical V1 contract".into(),
            });
        }
        let main_relative = safe_relative_path(&spec.settings.main_file)?;
        // Resolve and validate every generated byte before creating the target
        // directory. Invalid onboarding metadata must remain a true no-op.
        let source = render_project_template(&spec)?;
        let bibliography = checked_in_template(spec.settings.template_id)
            .bibliography
            .as_bytes()
            .to_vec();
        let mut settings_bytes =
            serde_json::to_vec_pretty(&spec.settings).map_err(AppError::serialization)?;
        settings_bytes.push(b'\n');
        let requested_parent = requested_root
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let canonical_parent = requested_parent.canonicalize().map_err(|error| {
            AppError::io("canonicalize project parent", requested_parent, error)
        })?;

        match std::fs::symlink_metadata(requested_root) {
            Ok(metadata) => {
                ensure_plain_directory(requested_root, &metadata)?;
                let mut entries = std::fs::read_dir(requested_root).map_err(|error| {
                    AppError::io("read project directory", requested_root, error)
                })?;
                if entries.next().is_some() {
                    return Err(AppError::InvalidProject {
                        message: "new project directory must be empty".into(),
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // `create_dir` is intentionally atomic and non-recursive: the
                // already-canonical parent is the creation authority, and a
                // racing replacement cannot be silently accepted.
                std::fs::create_dir(requested_root).map_err(|error| {
                    AppError::io("create project directory", requested_root, error)
                })?;
            }
            Err(error) => {
                return Err(AppError::io(
                    "inspect project directory",
                    requested_root,
                    error,
                ));
            }
        }
        let root = verify_creation_root(requested_root, &canonical_parent)?;
        if let Some(parent) = main_relative
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(root.join(parent)).map_err(|error| {
                AppError::io("create source directory", &root.join(parent), error)
            })?;
        }
        let verified_root = verify_creation_root(requested_root, &canonical_parent)?;
        if verified_root != root {
            return Err(AppError::ExternalConflict {
                path: requested_root.to_string_lossy().into_owned(),
            });
        }
        let journal = recovery_directory.join(format!(
            "create-{}-{}.json",
            spec.settings.project_id,
            uuid::Uuid::new_v4()
        ));
        save_transaction(
            &root,
            &journal,
            &[
                TransactionWrite::new(main_relative, source.into_bytes()),
                TransactionWrite::new("references.bib", bibliography),
                TransactionWrite::new("paper-settings.json", settings_bytes),
            ],
        )?;
        Self::open_path(&root)
    }

    #[must_use]
    pub const fn session_id(&self) -> ProjectSessionId {
        self.session_id
    }

    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    #[must_use]
    pub const fn main_file_id(&self) -> FileId {
        self.main_file_id
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn settings(&self) -> Option<&PaperSettingsV1> {
        self.settings.as_ref()
    }

    pub fn file(&self, file_id: FileId) -> AppResult<&SourceBuffer> {
        self.ensure_open()?;
        self.files
            .get(&file_id)
            .ok_or_else(|| AppError::UnknownFile {
                file_id: file_id.to_string(),
            })
    }

    pub fn files(&self) -> AppResult<Vec<ProjectFile>> {
        self.ensure_open()?;
        Ok(self.files.values().map(project_file_descriptor).collect())
    }

    pub fn read_file(&self, file_id: FileId) -> AppResult<ProjectFileContents> {
        let source = self.file(file_id)?;
        Ok(ProjectFileContents {
            file: project_file_descriptor(source),
            bytes: source.bytes().to_vec(),
        })
    }

    pub fn projection(&self, file_id: FileId) -> AppResult<&LatexAnalysis> {
        self.ensure_open()?;
        self.analyses.get(&file_id).ok_or_else(|| {
            if self
                .files
                .get(&file_id)
                .is_some_and(SourceBuffer::is_source_only)
            {
                AppError::InvalidUtf8 {
                    path: self.files[&file_id]
                        .relative_path()
                        .to_string_lossy()
                        .into_owned(),
                }
            } else if let Some(source) = self.files.get(&file_id) {
                AppError::InvalidProject {
                    message: format!(
                        "visual LaTeX projection is unavailable for {}",
                        source.relative_path().display()
                    ),
                }
            } else {
                AppError::UnknownFile {
                    file_id: file_id.to_string(),
                }
            }
        })
    }

    pub fn compatibility_report(&self, file_id: FileId) -> AppResult<&CompatibilityReport> {
        Ok(&self.projection(file_id)?.compatibility)
    }

    pub fn snapshot(&self) -> AppResult<ProjectSnapshot> {
        self.ensure_open()?;
        Ok(ProjectSnapshot {
            session_id: self.session_id,
            root_path: self.root.to_string_lossy().into_owned(),
            main_file_id: self.main_file_id,
            revision: self.revision,
            files: self.files.values().map(project_file_descriptor).collect(),
            include_graph: self.include_graph.clone(),
            include_graph_stale: self.include_graph_stale,
            compatibility: self
                .analyses
                .iter()
                .map(|(file_id, analysis)| (*file_id, analysis.compatibility.clone()))
                .collect(),
            settings: self.settings.clone(),
            dirty: self.files.values().any(SourceBuffer::is_dirty),
        })
    }

    pub fn apply_source_edits(
        &mut self,
        base_revision: Revision,
        edits: Vec<SourceEdit>,
    ) -> AppResult<EditResult> {
        self.ensure_open()?;
        self.ensure_revision(base_revision)?;
        if edits.is_empty() {
            return Ok(EditResult {
                revision: self.revision,
                changed: false,
                changed_files: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
        let mut grouped: BTreeMap<FileId, Vec<SourceEdit>> = BTreeMap::new();
        for edit in edits {
            grouped.entry(edit.file_id).or_default().push(edit);
        }
        let next_revision = self.revision.next();
        let mut candidates = BTreeMap::new();
        let mut candidate_analyses = BTreeMap::new();
        let mut changed_files = Vec::new();
        let mut include_graph_changed = false;

        for (file_id, file_edits) in grouped {
            let source = self
                .files
                .get(&file_id)
                .ok_or_else(|| AppError::UnknownFile {
                    file_id: file_id.to_string(),
                })?;
            let old_includes = self
                .analyses
                .get(&file_id)
                .map(|analysis| analysis.includes.clone());
            let mut candidate = source.clone();
            let outcome = candidate.apply_edits(&file_edits, next_revision)?;
            if !outcome.changed {
                continue;
            }
            if is_tex_source(&candidate) {
                let parsed =
                    self.parser
                        .parse_candidate(file_id, source.bytes(), candidate.bytes())?;
                let analysis = parsed.analysis();
                if !projection_covers_source(candidate.bytes().len(), &analysis.projection) {
                    return Err(AppError::Parse {
                        message: format!("projection did not own every byte in {file_id}"),
                    });
                }
                if old_includes.as_ref() != Some(&analysis.includes) {
                    include_graph_changed = true;
                }
                candidate_analyses.insert(file_id, parsed);
            }
            candidates.insert(file_id, candidate);
            changed_files.push(file_id);
        }

        if changed_files.is_empty() {
            return Ok(EditResult {
                revision: self.revision,
                changed: false,
                changed_files,
                diagnostics: Vec::new(),
            });
        }
        for (file_id, candidate) in candidates {
            self.files.insert(file_id, candidate);
        }
        for (file_id, parsed) in candidate_analyses {
            let analysis = self.parser.commit_candidate(file_id, parsed);
            self.analyses.insert(file_id, analysis);
        }
        self.include_graph_stale |= include_graph_changed;
        self.revision = next_revision;
        changed_files.sort();
        Ok(EditResult {
            revision: self.revision,
            changed: true,
            changed_files,
            diagnostics: Vec::new(),
        })
    }

    pub fn apply_document_op(
        &mut self,
        base_revision: Revision,
        operation: DocumentOp,
    ) -> AppResult<EditResult> {
        self.ensure_open()?;
        self.ensure_revision(base_revision)?;
        let edits = match operation {
            DocumentOp::InsertText {
                file_id,
                at_byte,
                text,
            } => vec![self.insertion_edit(file_id, at_byte, text)?],
            DocumentOp::ReplaceText {
                span,
                replacement,
                expected_slice_hash,
            } => vec![SourceEdit {
                file_id: span.file_id,
                start_byte: span.start_byte,
                end_byte: span.end_byte,
                replacement,
                expected_slice_hash,
            }],
            DocumentOp::Delete {
                span,
                expected_slice_hash,
            } => vec![SourceEdit {
                file_id: span.file_id,
                start_byte: span.start_byte,
                end_byte: span.end_byte,
                replacement: String::new(),
                expected_slice_hash,
            }],
            DocumentOp::InsertNode {
                file_id,
                at_byte,
                node,
            } => vec![self.insertion_edit(file_id, at_byte, node.latex)?],
            DocumentOp::SetMark {
                span,
                mark,
                enabled,
            } => vec![self.mark_edit(span, mark, enabled)?],
            DocumentOp::Move {
                span,
                destination_byte,
                expected_slice_hash,
            } => self.move_edits(span, destination_byte, expected_slice_hash)?,
            DocumentOp::SetAttribute { .. } => {
                return Err(AppError::InvalidEdit {
                    reason: "attribute changes must be resolved by the typed projection adapter"
                        .into(),
                });
            }
        };
        self.apply_source_edits(base_revision, edits)
    }

    pub fn save(&mut self, recovery_directory: &Path) -> AppResult<SaveResult> {
        self.ensure_open()?;
        let dirty: Vec<_> = self
            .files
            .iter()
            .filter(|(_, source)| source.is_dirty())
            .map(|(file_id, source)| {
                (
                    *file_id,
                    TransactionWrite::new(source.relative_path(), source.bytes().to_vec()),
                )
            })
            .collect();
        if dirty.is_empty() {
            return Ok(SaveResult {
                revision: self.revision,
                saved_files: Vec::new(),
            });
        }
        // A watcher improves responsiveness, but correctness cannot depend on
        // one having delivered an event before the autosave timer fires. Check
        // every loaded destination against the exact bytes observed at open or
        // the last successful save before beginning any multi-file write.
        self.ensure_disk_matches_persisted()?;

        // Include traversal and newly discovered source parsing are fallible.
        // Complete them against the candidate in-memory buffers before the
        // transaction can replace even one authoritative project file.
        let prepared_graph = if self.include_graph_stale {
            Some(self.prepare_include_graph()?)
        } else {
            None
        };
        // Preparation can involve bounded disk reads. Recheck both existing
        // baselines and newly discovered files immediately before committing.
        self.ensure_disk_matches_persisted()?;
        if let Some(prepared) = &prepared_graph {
            self.ensure_prepared_sources_unchanged(prepared)?;
        }
        let journal =
            recovery_directory.join(format!("save-{}-{}.json", self.session_id, self.revision.0));
        let writes: Vec<_> = dirty.iter().map(|(_, write)| write.clone()).collect();
        let written_paths = save_transaction(&self.root, &journal, &writes)?;
        let written_set: std::collections::BTreeSet<_> = written_paths.into_iter().collect();
        let mut saved_files = Vec::new();
        for (file_id, source) in &mut self.files {
            let relative = normalized_relative(source.relative_path());
            if written_set.contains(&relative) {
                source.mark_persisted();
                self.persisted_bytes
                    .insert(*file_id, source.bytes().to_vec());
                saved_files.push(*file_id);
            }
        }
        saved_files.sort();
        if let Some(prepared) = prepared_graph {
            self.apply_prepared_include_graph(prepared);
        }
        Ok(SaveResult {
            revision: self.revision,
            saved_files,
        })
    }

    pub fn handle_external_change(
        &mut self,
        file_id: FileId,
        external_bytes: Vec<u8>,
    ) -> AppResult<ExternalChangeResult> {
        self.ensure_open()?;
        let (same_bytes, dirty, relative_path, local_bytes) = {
            let source = self
                .files
                .get(&file_id)
                .ok_or_else(|| AppError::UnknownFile {
                    file_id: file_id.to_string(),
                })?;
            (
                source.bytes() == external_bytes,
                source.is_dirty(),
                normalized_relative(source.relative_path()),
                source.bytes().to_vec(),
            )
        };
        if same_bytes {
            self.persisted_bytes.insert(file_id, external_bytes.clone());
            self.pending_external_bytes.remove(&file_id);
            self.files
                .get_mut(&file_id)
                .expect("source existence checked above")
                .set_persisted_baseline(&external_bytes);
            return Ok(ExternalChangeResult::Unchanged);
        }
        if dirty {
            self.pending_external_bytes
                .insert(file_id, external_bytes.clone());
            return Ok(ExternalChangeResult::Conflict {
                file_id,
                relative_path,
                base_bytes: self
                    .persisted_bytes
                    .get(&file_id)
                    .cloned()
                    .unwrap_or_default(),
                local_bytes,
                external_bytes,
            });
        }

        let next_revision = self.revision.next();
        let source = self.files.get_mut(&file_id).expect("checked above");
        source.reload_clean(external_bytes.clone(), next_revision)?;
        self.persisted_bytes.insert(file_id, external_bytes);
        self.pending_external_bytes.remove(&file_id);
        if source.is_source_only() || !is_tex_source(source) {
            self.analyses.remove(&file_id);
        } else {
            self.analyses
                .insert(file_id, self.parser.parse(file_id, source.bytes())?);
        }
        self.revision = next_revision;
        self.include_graph_stale = true;
        Ok(ExternalChangeResult::Reloaded {
            revision: self.revision,
            file_id,
        })
    }

    /// A synchronous correctness guard for actions such as compilation that
    /// cannot rely on filesystem watcher delivery ordering. Any mismatch is
    /// reported before canonical bytes are staged or project files are saved.
    pub fn ensure_disk_matches_persisted(&self) -> AppResult<()> {
        self.ensure_open()?;
        for file_id in self.files.keys().copied() {
            self.ensure_file_unchanged_on_disk(file_id)?;
        }
        Ok(())
    }

    pub fn resolve_external_change(
        &mut self,
        file_id: FileId,
        base_revision: Revision,
        resolved_text: String,
    ) -> AppResult<EditResult> {
        self.ensure_open()?;
        self.ensure_revision(base_revision)?;
        let expected_external = self
            .pending_external_bytes
            .get(&file_id)
            .cloned()
            .ok_or_else(|| AppError::InvalidProject {
                message: "there is no pending external conflict for this file".into(),
            })?;
        let source = self.file(file_id)?;
        let destination = self.root.join(source.relative_path());
        let metadata = std::fs::symlink_metadata(&destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::ExternalConflict {
                    path: destination.to_string_lossy().into_owned(),
                }
            } else {
                AppError::io("inspect external merge target", &destination, error)
            }
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AppError::ExternalConflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        let on_disk = std::fs::read(&destination)
            .map_err(|error| AppError::io("read external merge target", &destination, error))?;
        if on_disk != expected_external {
            return Err(AppError::ExternalConflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        let edit = SourceEdit {
            file_id,
            start_byte: 0,
            end_byte: source.bytes().len(),
            replacement: resolved_text,
            expected_slice_hash: hash_bytes(source.bytes()),
        };
        let result = self.apply_source_edits(base_revision, vec![edit])?;
        self.persisted_bytes
            .insert(file_id, expected_external.clone());
        self.files
            .get_mut(&file_id)
            .expect("source existence checked above")
            .set_persisted_baseline(&expected_external);
        self.pending_external_bytes.remove(&file_id);
        Ok(result)
    }

    pub fn convert_file_to_utf8(
        &mut self,
        file_id: FileId,
        base_revision: Revision,
        reviewed_text: String,
    ) -> AppResult<EditResult> {
        self.ensure_open()?;
        self.ensure_revision(base_revision)?;
        let next_revision = self.revision.next();
        let source = self
            .files
            .get_mut(&file_id)
            .ok_or_else(|| AppError::UnknownFile {
                file_id: file_id.to_string(),
            })?;
        let changed = source.convert_to_utf8(reviewed_text, next_revision);
        if !changed {
            return Ok(EditResult {
                revision: self.revision,
                changed: false,
                changed_files: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
        if is_tex_source(source) {
            let analysis = self.parser.parse(file_id, source.bytes())?;
            self.analyses.insert(file_id, analysis);
        } else {
            self.analyses.remove(&file_id);
        }
        self.revision = next_revision;
        Ok(EditResult {
            revision: self.revision,
            changed: true,
            changed_files: vec![file_id],
            diagnostics: Vec::new(),
        })
    }

    pub fn close(&mut self) -> AppResult<ProjectEvent> {
        self.ensure_open()?;
        self.closed = true;
        Ok(ProjectEvent::Closed)
    }

    #[must_use]
    pub fn project_hash(&self) -> String {
        let mut hasher = Sha256::new();
        for (path, file_id) in &self.path_to_id {
            if let Some(source) = self.files.get(file_id) {
                hasher.update((path.len() as u64).to_le_bytes());
                hasher.update(path.as_bytes());
                hasher.update((source.bytes().len() as u64).to_le_bytes());
                hasher.update(source.bytes());
            }
        }
        hex::encode(hasher.finalize())
    }

    fn ensure_open(&self) -> AppResult<()> {
        if self.closed {
            Err(AppError::SessionClosed)
        } else {
            Ok(())
        }
    }

    fn ensure_revision(&self, expected: Revision) -> AppResult<()> {
        if self.revision == expected {
            Ok(())
        } else {
            Err(AppError::RevisionConflict {
                expected: expected.0,
                actual: self.revision.0,
            })
        }
    }

    fn ensure_file_unchanged_on_disk(&self, file_id: FileId) -> AppResult<()> {
        let source = self.file(file_id)?;
        let destination = self.root.join(source.relative_path());
        let metadata = std::fs::symlink_metadata(&destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::ExternalConflict {
                    path: destination.to_string_lossy().into_owned(),
                }
            } else {
                AppError::io("inspect external state", &destination, error)
            }
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AppError::ExternalConflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        let on_disk = std::fs::read(&destination)
            .map_err(|error| AppError::io("read external state", &destination, error))?;
        if self.persisted_bytes.get(&file_id) != Some(&on_disk) {
            return Err(AppError::ExternalConflict {
                path: destination.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    }

    fn insertion_edit(
        &self,
        file_id: FileId,
        at_byte: usize,
        replacement: String,
    ) -> AppResult<SourceEdit> {
        let source = self.file(file_id)?;
        source.slice(at_byte, at_byte)?;
        Ok(SourceEdit {
            file_id,
            start_byte: at_byte,
            end_byte: at_byte,
            replacement,
            expected_slice_hash: hash_bytes([]),
        })
    }

    fn mark_edit(&self, span: SourceSpan, mark: TextMark, enabled: bool) -> AppResult<SourceEdit> {
        let source = self.file(span.file_id)?;
        let selected = source.slice(span.start_byte, span.end_byte)?;
        let selected_text = std::str::from_utf8(selected).map_err(|_| AppError::InvalidUtf8 {
            path: source.relative_path().to_string_lossy().into_owned(),
        })?;
        let command = mark_command(mark);
        let replacement = if enabled {
            format!("\\{command}{{{selected_text}}}")
        } else {
            let prefix = format!("\\{command}{{");
            selected_text
                .strip_prefix(&prefix)
                .and_then(|inner| inner.strip_suffix('}'))
                .ok_or_else(|| AppError::InvalidEdit {
                    reason: format!("selected source is not an exact {command} wrapper"),
                })?
                .to_string()
        };
        Ok(SourceEdit {
            file_id: span.file_id,
            start_byte: span.start_byte,
            end_byte: span.end_byte,
            replacement,
            expected_slice_hash: hash_bytes(selected),
        })
    }

    fn move_edits(
        &self,
        span: SourceSpan,
        destination_byte: usize,
        expected_slice_hash: String,
    ) -> AppResult<Vec<SourceEdit>> {
        let source = self.file(span.file_id)?;
        source.slice(destination_byte, destination_byte)?;
        let selected = source.slice(span.start_byte, span.end_byte)?;
        let actual = hash_bytes(selected);
        if actual != expected_slice_hash {
            return Err(AppError::HashMismatch {
                expected: expected_slice_hash,
                actual,
            });
        }
        if destination_byte > span.start_byte && destination_byte < span.end_byte {
            return Err(AppError::InvalidEdit {
                reason: "move destination lies inside the moved span".into(),
            });
        }
        let selected_text = std::str::from_utf8(selected)
            .expect("UTF-8 source and validated character boundaries")
            .to_string();
        Ok(vec![
            SourceEdit {
                file_id: span.file_id,
                start_byte: span.start_byte,
                end_byte: span.end_byte,
                replacement: String::new(),
                expected_slice_hash: hash_bytes(selected),
            },
            SourceEdit {
                file_id: span.file_id,
                start_byte: destination_byte,
                end_byte: destination_byte,
                replacement: selected_text,
                expected_slice_hash: hash_bytes([]),
            },
        ])
    }

    fn prepare_include_graph(&mut self) -> AppResult<PreparedIncludeGraph> {
        let main_relative = self
            .files
            .get(&self.main_file_id)
            .expect("main file always exists")
            .relative_path()
            .to_path_buf();
        let overlays = self
            .files
            .values()
            .map(|source| {
                (
                    normalized_relative(source.relative_path()),
                    source.bytes().to_vec(),
                )
            })
            .collect();
        let graph = build_include_graph_with_overlays(
            &self.root,
            &main_relative,
            &mut self.parser,
            &overlays,
        )?;
        let mut source_paths: std::collections::BTreeSet<String> =
            graph.nodes.keys().cloned().collect();
        source_paths.extend(
            discover_project_bibliographies(&self.root)?
                .iter()
                .map(|path| normalized_relative(path)),
        );
        let mut new_sources = Vec::new();
        for relative_string in &source_paths {
            if self.path_to_id.contains_key(relative_string) {
                continue;
            }
            let relative = safe_relative_path(relative_string)?;
            let bytes = read_source_file_bounded(&self.root, &relative)?;
            if let Some(node) = graph.nodes.get(relative_string) {
                let actual_hash = hash_bytes(&bytes);
                if actual_hash != node.content_hash {
                    return Err(AppError::ExternalConflict {
                        path: self.root.join(&relative).to_string_lossy().into_owned(),
                    });
                }
            }
            let file_id = FileId::new();
            let buffer = SourceBuffer::from_bytes(file_id, relative, bytes, self.revision);
            let analysis = if !buffer.is_source_only()
                && buffer
                    .relative_path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
            {
                Some(self.parser.parse(file_id, buffer.bytes())?)
            } else {
                None
            };
            new_sources.push(PreparedSource {
                normalized_path: relative_string.clone(),
                file_id,
                buffer,
                analysis,
            });
        }
        Ok(PreparedIncludeGraph { graph, new_sources })
    }

    fn ensure_prepared_sources_unchanged(&self, prepared: &PreparedIncludeGraph) -> AppResult<()> {
        for source in &prepared.new_sources {
            let on_disk = read_source_file_bounded(&self.root, source.buffer.relative_path())?;
            if on_disk != source.buffer.bytes() {
                return Err(AppError::ExternalConflict {
                    path: self
                        .root
                        .join(source.buffer.relative_path())
                        .to_string_lossy()
                        .into_owned(),
                });
            }
        }
        Ok(())
    }

    fn apply_prepared_include_graph(&mut self, prepared: PreparedIncludeGraph) {
        for source in prepared.new_sources {
            if let Some(analysis) = source.analysis {
                self.analyses.insert(source.file_id, analysis);
            }
            self.persisted_bytes
                .insert(source.file_id, source.buffer.bytes().to_vec());
            self.path_to_id
                .insert(source.normalized_path, source.file_id);
            self.files.insert(source.file_id, source.buffer);
        }
        self.include_graph = prepared.graph;
        self.include_graph_stale = false;
    }
}

fn ensure_plain_directory(path: &Path, metadata: &std::fs::Metadata) -> AppResult<()> {
    if metadata_is_link_or_reparse(metadata) {
        return Err(AppError::InvalidPath {
            path: path.to_string_lossy().into_owned(),
            message: "project directory must not be a symlink, junction, or reparse point".into(),
        });
    }
    if !metadata.is_dir() {
        return Err(AppError::InvalidProject {
            message: format!("{} is not a directory", path.display()),
        });
    }
    Ok(())
}

fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn verify_creation_root(requested_root: &Path, canonical_parent: &Path) -> AppResult<PathBuf> {
    let metadata = std::fs::symlink_metadata(requested_root)
        .map_err(|error| AppError::io("inspect project directory", requested_root, error))?;
    ensure_plain_directory(requested_root, &metadata)?;
    let canonical_root = requested_root
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize project directory", requested_root, error))?;
    if canonical_root.parent() != Some(canonical_parent) {
        return Err(AppError::InvalidPath {
            path: requested_root.to_string_lossy().into_owned(),
            message: format!(
                "canonical project directory must remain directly beneath {}",
                canonical_parent.display()
            ),
        });
    }
    Ok(canonical_root)
}

fn read_source_file_bounded(root: &Path, relative: &Path) -> AppResult<Vec<u8>> {
    let path = root.join(relative);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| AppError::io("inspect source", &path, error))?;
    if !metadata.is_file() || metadata_is_link_or_reparse(&metadata) {
        return Err(AppError::InvalidProject {
            message: format!("{} is not an ordinary source file", path.display()),
        });
    }
    if metadata.len() > MAX_SOURCE_FILE_BYTES {
        return Err(AppError::InvalidProject {
            message: format!("{} exceeds the source file size limit", path.display()),
        });
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", root, error))?;
    let canonical = path
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", &path, error))?;
    ensure_within(&canonical_root, &canonical)?;
    let file = std::fs::File::open(&canonical)
        .map_err(|error| AppError::io("open source", &canonical, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io("read source", &canonical, error))?;
    if bytes.len() as u64 > MAX_SOURCE_FILE_BYTES {
        return Err(AppError::InvalidProject {
            message: format!("{} grew beyond the source file size limit", path.display()),
        });
    }
    Ok(bytes)
}

fn project_file_descriptor(source: &SourceBuffer) -> ProjectFile {
    ProjectFile {
        file_id: source.file_id(),
        relative_path: normalized_relative(source.relative_path()),
        revision: source.revision(),
        dirty: source.is_dirty(),
        byte_len: source.bytes().len(),
        content_hash: source.content_hash(),
        source_only: source.is_source_only(),
    }
}

fn is_tex_source(source: &SourceBuffer) -> bool {
    source
        .relative_path()
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
}

fn mark_command(mark: TextMark) -> &'static str {
    match mark {
        TextMark::Bold => "textbf",
        TextMark::Italic => "emph",
        TextMark::Monospace => "texttt",
        TextMark::Underline => "underline",
        TextMark::Strike => "sout",
        TextMark::Superscript => "textsuperscript",
        TextMark::Subscript => "textsubscript",
    }
}

const TEMPLATE_TITLE_PLACEHOLDER: &str = "A Clear and Specific Paper Title";
const MAX_PROJECT_TITLE_CHARS: usize = 512;
const MAX_PROJECT_AUTHORS: usize = 64;
const MAX_PROJECT_AUTHOR_CHARS: usize = 256;

#[derive(Debug, Clone, Copy)]
struct CheckedInTemplate {
    main: &'static str,
    bibliography: &'static str,
    author_placeholder: &'static str,
}

fn checked_in_template(template_id: TemplateId) -> CheckedInTemplate {
    match template_id {
        TemplateId::GenericArticle => CheckedInTemplate {
            main: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../templates/generic/main.tex"
            )),
            bibliography: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../templates/generic/references.bib"
            )),
            author_placeholder: "First Author",
        },
        TemplateId::AcmAcMart => CheckedInTemplate {
            main: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../templates/acm/main.tex"
            )),
            bibliography: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../templates/acm/references.bib"
            )),
            author_placeholder: "Anonymous Author(s)",
        },
        TemplateId::IeeeTran => CheckedInTemplate {
            main: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../templates/ieee/main.tex"
            )),
            bibliography: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../templates/ieee/references.bib"
            )),
            author_placeholder: "First Author",
        },
    }
}

fn render_project_template(spec: &NewProjectSpec) -> AppResult<String> {
    let title = validated_metadata_text("paper title", &spec.title, MAX_PROJECT_TITLE_CHARS)?;
    if spec.authors.len() > MAX_PROJECT_AUTHORS {
        return Err(AppError::InvalidProject {
            message: format!("a project may contain at most {MAX_PROJECT_AUTHORS} authors"),
        });
    }
    let authors = spec
        .authors
        .iter()
        .map(|author| {
            validated_metadata_text("author name", author, MAX_PROJECT_AUTHOR_CHARS)
                .map(escape_latex_text)
        })
        .collect::<AppResult<Vec<_>>>()?;
    let template = checked_in_template(spec.settings.template_id);
    let author_text = if authors.is_empty() {
        template.author_placeholder.to_owned()
    } else {
        // The public V1 creation contract contains names, not per-author
        // affiliations. Keep the checked-in template's syntactically valid
        // affiliation/contact placeholder and substitute the names as one
        // author group until the richer metadata adapter is introduced.
        authors.join(", ")
    };
    substitute_exact_template_tokens(
        template.main,
        &[
            (TEMPLATE_TITLE_PLACEHOLDER, escape_latex_text(title)),
            (template.author_placeholder, author_text),
        ],
    )
}

fn validated_metadata_text<'a>(
    label: &str,
    value: &'a str,
    max_chars: usize,
) -> AppResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::InvalidProject {
            message: format!("{label} must not be empty"),
        });
    }
    if value.chars().count() > max_chars {
        return Err(AppError::InvalidProject {
            message: format!("{label} exceeds the {max_chars}-character limit"),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(AppError::InvalidProject {
            message: format!("{label} must not contain control characters"),
        });
    }
    Ok(value)
}

fn substitute_exact_template_tokens(
    source: &str,
    replacements: &[(&str, String)],
) -> AppResult<String> {
    let mut spans = Vec::with_capacity(replacements.len());
    for (token, replacement) in replacements {
        let matches = source.match_indices(token).collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(AppError::InvalidProject {
                message: format!(
                    "checked-in template token {token:?} must occur exactly once, found {}",
                    matches.len()
                ),
            });
        }
        let start = matches[0].0;
        spans.push((start, start + token.len(), replacement));
    }
    spans.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut rendered = source.to_owned();
    for (start, end, replacement) in spans {
        rendered.replace_range(start..end, replacement);
    }
    Ok(rendered)
}

fn escape_latex_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\textbackslash{}"),
            '{' => escaped.push_str("\\{"),
            '}' => escaped.push_str("\\}"),
            '#' => escaped.push_str("\\#"),
            '$' => escaped.push_str("\\$"),
            '%' => escaped.push_str("\\%"),
            '&' => escaped.push_str("\\&"),
            '_' => escaped.push_str("\\_"),
            '^' => escaped.push_str("\\textasciicircum{}"),
            '~' => escaped.push_str("\\textasciitilde{}"),
            other => escaped.push(other),
        }
    }
    escaped
}

pub type SharedProjectSession = Arc<Mutex<ProjectSession>>;

#[derive(Debug, Default)]
pub struct ProjectRegistry {
    sessions: RwLock<HashMap<ProjectSessionId, SharedProjectSession>>,
}

impl ProjectRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, session: ProjectSession) -> ProjectSessionId {
        let session_id = session.session_id();
        self.sessions
            .write()
            .insert(session_id, Arc::new(Mutex::new(session)));
        session_id
    }

    pub fn open_path(&self, path: impl AsRef<Path>) -> AppResult<ProjectSnapshot> {
        let session = ProjectSession::open_path(path)?;
        let snapshot = session.snapshot()?;
        self.insert(session);
        Ok(snapshot)
    }

    pub fn get(&self, session_id: ProjectSessionId) -> AppResult<SharedProjectSession> {
        self.sessions
            .read()
            .get(&session_id)
            .cloned()
            .ok_or(AppError::SessionClosed)
    }

    pub fn remove(&self, session_id: ProjectSessionId) -> AppResult<SharedProjectSession> {
        self.sessions
            .write()
            .remove(&session_id)
            .ok_or(AppError::SessionClosed)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.read().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_fixture(source: &[u8]) -> (tempfile::TempDir, ProjectSession) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), source).unwrap();
        let session = ProjectSession::open_path(directory.path()).unwrap();
        (directory, session)
    }

    #[test]
    fn open_and_close_are_non_mutating() {
        let (directory, mut session) = open_fixture(b"hello\r\n");
        let before = std::fs::read(directory.path().join("main.tex")).unwrap();
        let before_entries = std::fs::read_dir(directory.path()).unwrap().count();
        session.close().unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("main.tex")).unwrap(),
            before
        );
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            before_entries
        );
    }

    #[test]
    fn revision_and_hash_guard_atomic_multi_file_edits() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), "\\input{section}\nmain").unwrap();
        std::fs::write(directory.path().join("section.tex"), "section").unwrap();
        let mut session = ProjectSession::open_path(directory.path()).unwrap();
        let snapshot = session.snapshot().unwrap();
        let main = snapshot.main_file_id;
        let section = snapshot
            .files
            .iter()
            .find(|file| file.relative_path == "section.tex")
            .unwrap()
            .file_id;
        let main_bytes = session.file(main).unwrap().bytes().to_vec();
        let section_bytes = session.file(section).unwrap().bytes().to_vec();
        let result = session.apply_source_edits(
            Revision::INITIAL,
            vec![
                SourceEdit {
                    file_id: main,
                    start_byte: main_bytes.len() - 4,
                    end_byte: main_bytes.len(),
                    replacement: "MAIN".into(),
                    expected_slice_hash: hash_bytes(b"main"),
                },
                SourceEdit {
                    file_id: section,
                    start_byte: 0,
                    end_byte: section_bytes.len(),
                    replacement: "SECTION".into(),
                    expected_slice_hash: hash_bytes(b"wrong"),
                },
            ],
        );
        assert!(matches!(result, Err(AppError::HashMismatch { .. })));
        assert_eq!(session.file(main).unwrap().bytes(), main_bytes);
        assert_eq!(session.file(section).unwrap().bytes(), section_bytes);
        assert_eq!(session.revision(), Revision::INITIAL);
    }

    #[test]
    fn successful_edit_is_one_global_revision_and_save_is_atomic() {
        let (directory, mut session) = open_fixture(b"hello");
        let app_data = tempfile::tempdir().unwrap();
        let file_id = session.main_file_id();
        let edit = SourceEdit {
            file_id,
            start_byte: 0,
            end_byte: 5,
            replacement: "goodbye".into(),
            expected_slice_hash: hash_bytes(b"hello"),
        };
        let result = session
            .apply_source_edits(Revision::INITIAL, vec![edit])
            .unwrap();
        assert_eq!(result.revision, Revision(1));
        assert_eq!(
            std::fs::read(directory.path().join("main.tex")).unwrap(),
            b"hello"
        );
        let saved = session.save(app_data.path()).unwrap();
        assert_eq!(saved.saved_files, [file_id]);
        assert_eq!(
            std::fs::read(directory.path().join("main.tex")).unwrap(),
            b"goodbye"
        );
        assert_eq!(std::fs::read_dir(app_data.path()).unwrap().count(), 0);
    }

    #[test]
    fn external_dirty_change_returns_three_way_inputs() {
        let (_directory, mut session) = open_fixture(b"base");
        let file_id = session.main_file_id();
        session
            .apply_source_edits(
                Revision::INITIAL,
                vec![SourceEdit {
                    file_id,
                    start_byte: 0,
                    end_byte: 4,
                    replacement: "ours".into(),
                    expected_slice_hash: hash_bytes(b"base"),
                }],
            )
            .unwrap();
        let result = session
            .handle_external_change(file_id, b"theirs".to_vec())
            .unwrap();
        assert!(matches!(
            result,
            ExternalChangeResult::Conflict {
                base_bytes,
                local_bytes,
                external_bytes,
                ..
            } if base_bytes == b"base" && local_bytes == b"ours" && external_bytes == b"theirs"
        ));
    }

    #[test]
    fn resolved_external_merge_updates_baseline_and_then_saves() {
        let (directory, mut session) = open_fixture(b"base");
        let recovery = tempfile::tempdir().unwrap();
        let file_id = session.main_file_id();
        session
            .apply_source_edits(
                Revision::INITIAL,
                vec![SourceEdit {
                    file_id,
                    start_byte: 0,
                    end_byte: 4,
                    replacement: "ours".into(),
                    expected_slice_hash: hash_bytes(b"base"),
                }],
            )
            .unwrap();
        std::fs::write(directory.path().join("main.tex"), b"theirs").unwrap();
        assert!(matches!(
            session
                .handle_external_change(file_id, b"theirs".to_vec())
                .unwrap(),
            ExternalChangeResult::Conflict { .. }
        ));
        let resolved = session
            .resolve_external_change(file_id, Revision(1), "merged".into())
            .unwrap();
        assert_eq!(resolved.revision, Revision(2));
        assert!(session.file(file_id).unwrap().is_dirty());
        session.save(recovery.path()).unwrap();
        assert_eq!(
            std::fs::read(directory.path().join("main.tex")).unwrap(),
            b"merged"
        );
        assert!(!session.file(file_id).unwrap().is_dirty());
    }

    #[test]
    fn resolved_external_merge_rejects_a_newer_unreviewed_disk_state() {
        let (directory, mut session) = open_fixture(b"base");
        let file_id = session.main_file_id();
        session
            .apply_source_edits(
                Revision::INITIAL,
                vec![SourceEdit {
                    file_id,
                    start_byte: 0,
                    end_byte: 4,
                    replacement: "ours".into(),
                    expected_slice_hash: hash_bytes(b"base"),
                }],
            )
            .unwrap();
        std::fs::write(directory.path().join("main.tex"), b"theirs").unwrap();
        session
            .handle_external_change(file_id, b"theirs".to_vec())
            .unwrap();
        std::fs::write(directory.path().join("main.tex"), b"newer").unwrap();
        assert!(matches!(
            session.resolve_external_change(file_id, Revision(1), "merged".into()),
            Err(AppError::ExternalConflict { .. })
        ));
        assert_eq!(session.file(file_id).unwrap().bytes(), b"ours");
        assert_eq!(session.revision(), Revision(1));
    }

    #[test]
    fn save_refuses_to_overwrite_an_unreported_external_change() {
        let (directory, mut session) = open_fixture(b"base");
        let recovery = tempfile::tempdir().unwrap();
        let file_id = session.main_file_id();
        session
            .apply_source_edits(
                Revision::INITIAL,
                vec![SourceEdit {
                    file_id,
                    start_byte: 0,
                    end_byte: 4,
                    replacement: "ours".into(),
                    expected_slice_hash: hash_bytes(b"base"),
                }],
            )
            .unwrap();
        std::fs::write(directory.path().join("main.tex"), b"theirs").unwrap();

        assert!(matches!(
            session.save(recovery.path()),
            Err(AppError::ExternalConflict { .. })
        ));
        assert_eq!(
            std::fs::read(directory.path().join("main.tex")).unwrap(),
            b"theirs"
        );
        assert!(session.snapshot().unwrap().dirty);
    }

    #[test]
    fn save_validates_new_includes_before_mutating_disk_or_persisted_baseline() {
        let original = b"\\documentclass{article}\n\\begin{document}\nbase\n\\end{document}\n";
        let (directory, mut session) = open_fixture(original);
        let recovery = tempfile::tempdir().unwrap();
        let file_id = session.main_file_id();
        let replacement = "\\input{large}\n";
        session
            .apply_source_edits(
                Revision::INITIAL,
                vec![SourceEdit {
                    file_id,
                    start_byte: 0,
                    end_byte: original.len(),
                    replacement: replacement.into(),
                    expected_slice_hash: hash_bytes(original),
                }],
            )
            .unwrap();
        let oversized = std::fs::File::create(directory.path().join("large.tex")).unwrap();
        oversized.set_len(MAX_SOURCE_FILE_BYTES + 1).unwrap();
        drop(oversized);

        assert!(matches!(
            session.save(recovery.path()),
            Err(AppError::InvalidProject { .. })
        ));
        assert_eq!(
            std::fs::read(directory.path().join("main.tex")).unwrap(),
            original
        );
        let source = session.file(file_id).unwrap();
        assert_eq!(source.bytes(), replacement.as_bytes());
        assert_eq!(source.persisted_hash(), hash_bytes(original));
        assert_eq!(session.persisted_bytes.get(&file_id).unwrap(), original);
        assert!(source.is_dirty());
        assert!(session.include_graph_stale);
        assert_eq!(std::fs::read_dir(recovery.path()).unwrap().count(), 0);
    }

    #[test]
    fn compile_guard_detects_external_changes_even_when_buffer_is_clean() {
        let (directory, session) = open_fixture(b"base");
        std::fs::write(directory.path().join("main.tex"), b"theirs").unwrap();
        assert!(matches!(
            session.ensure_disk_matches_persisted(),
            Err(AppError::ExternalConflict { .. })
        ));
    }

    #[test]
    fn project_creation_uses_each_checked_in_template_exactly_and_parses_it() {
        let parent = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        for (index, template_id) in [
            TemplateId::GenericArticle,
            TemplateId::AcmAcMart,
            TemplateId::IeeeTran,
        ]
        .into_iter()
        .enumerate()
        {
            let root = parent.path().join(format!("paper-{index}"));
            let settings = PaperSettingsV1::new(
                "main.tex",
                template_id,
                "texlive-2025",
                crate::core::contracts::LatexEngine::PdfLatex,
            );
            let spec = NewProjectSpec {
                settings: settings.clone(),
                title: "Safe & Exact Study".into(),
                authors: vec!["Ada_L".into(), "Grace & Hopper".into()],
            };
            let template = checked_in_template(template_id);
            let expected = template
                .main
                .replacen(
                    TEMPLATE_TITLE_PLACEHOLDER,
                    &escape_latex_text("Safe & Exact Study"),
                    1,
                )
                .replacen(
                    template.author_placeholder,
                    &[
                        escape_latex_text("Ada_L"),
                        escape_latex_text("Grace & Hopper"),
                    ]
                    .join(", "),
                    1,
                );

            let session = ProjectSession::create(&root, spec, app_data.path()).unwrap();
            let generated = std::fs::read_to_string(root.join("main.tex")).unwrap();
            assert_eq!(generated, expected);
            assert_eq!(
                std::fs::read_to_string(root.join("references.bib")).unwrap(),
                template.bibliography
            );
            let stored_settings: PaperSettingsV1 =
                serde_json::from_slice(&std::fs::read(root.join("paper-settings.json")).unwrap())
                    .unwrap();
            assert_eq!(stored_settings, settings);
            assert!(stored_settings.is_valid());
            assert_eq!(session.settings(), Some(&settings));

            let mut parser = LatexParser::new().unwrap();
            let analysis = parser.parse(FileId::new(), generated.as_bytes()).unwrap();
            assert!(projection_covers_source(
                generated.len(),
                &analysis.projection
            ));
        }
    }

    #[test]
    fn project_creation_supplies_default_author_and_rejects_invalid_metadata_without_writes() {
        for template_id in [
            TemplateId::GenericArticle,
            TemplateId::AcmAcMart,
            TemplateId::IeeeTran,
        ] {
            let settings = PaperSettingsV1::new(
                "main.tex",
                template_id,
                "texlive-2025",
                crate::core::contracts::LatexEngine::PdfLatex,
            );
            let rendered = render_project_template(&NewProjectSpec {
                settings,
                title: "Untitled draft".into(),
                authors: Vec::new(),
            })
            .unwrap();
            assert!(rendered.contains(checked_in_template(template_id).author_placeholder));
            assert!(!rendered.contains("\\author{}"));
        }

        let parent = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let root = parent.path().join("invalid-paper");
        let settings = PaperSettingsV1::new(
            "main.tex",
            TemplateId::GenericArticle,
            "texlive-2025",
            crate::core::contracts::LatexEngine::PdfLatex,
        );
        assert!(matches!(
            ProjectSession::create(
                &root,
                NewProjectSpec {
                    settings,
                    title: "Valid title".into(),
                    authors: vec!["   ".into()],
                },
                app_data.path(),
            ),
            Err(AppError::InvalidProject { .. })
        ));
        assert!(!root.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn project_creation_rejects_a_symlink_or_reparse_root() {
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let root = parent.path().join("linked-paper");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &root).unwrap();
        #[cfg(windows)]
        if let Err(error) = std::os::windows::fs::symlink_dir(outside.path(), &root) {
            // Directory junctions do not require the symlink privilege and
            // exercise the broader Windows reparse-point rejection path.
            if error.raw_os_error() == Some(1314) {
                let status = std::process::Command::new("cmd.exe")
                    .args(["/D", "/C", "mklink", "/J"])
                    .arg(&root)
                    .arg(outside.path())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap();
                assert!(status.success(), "failed to create junction fixture");
            } else {
                panic!("failed to create directory symlink fixture: {error}");
            }
        }
        let settings = PaperSettingsV1::new(
            "main.tex",
            TemplateId::GenericArticle,
            "texlive-2025",
            crate::core::contracts::LatexEngine::PdfLatex,
        );

        assert!(matches!(
            ProjectSession::create(
                &root,
                NewProjectSpec {
                    settings,
                    title: "Safe root".into(),
                    authors: vec!["Author".into()],
                },
                app_data.path(),
            ),
            Err(AppError::InvalidPath { .. })
        ));
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(app_data.path()).unwrap().count(), 0);
    }

    #[test]
    fn created_project_has_settings_and_escaped_metadata() {
        let parent = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let root = parent.path().join("paper");
        let settings = PaperSettingsV1::new(
            "main.tex",
            TemplateId::GenericArticle,
            "texlive-2025",
            crate::core::contracts::LatexEngine::PdfLatex,
        );
        let session = ProjectSession::create(
            &root,
            NewProjectSpec {
                settings,
                title: "A&B".into(),
                authors: vec!["Ada_L".into()],
            },
            app_data.path(),
        )
        .unwrap();
        let text = std::fs::read_to_string(root.join("main.tex")).unwrap();
        assert!(text.contains("A\\&B"));
        assert!(text.contains("Ada\\_L"));
        assert!(text.contains("\\bibliography{references}"));
        assert!(root.join("references.bib").is_file());
        assert!(
            session
                .snapshot()
                .unwrap()
                .files
                .iter()
                .any(|file| { file.relative_path == "references.bib" && file.byte_len > 0 })
        );
        assert!(session.settings().is_some());
    }

    #[test]
    fn local_bibliographies_are_canonical_offline_sources() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), "Hello").unwrap();
        std::fs::create_dir(directory.path().join("refs")).unwrap();
        std::fs::write(
            directory.path().join("refs").join("library.bib"),
            "@misc{key, title={Old}}\n",
        )
        .unwrap();
        let mut session = ProjectSession::open_path(directory.path()).unwrap();
        let bibliography = session
            .snapshot()
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.relative_path == "refs/library.bib")
            .unwrap();
        let bytes = session.file(bibliography.file_id).unwrap().bytes().to_vec();
        let start = bytes
            .windows(3)
            .position(|window| window == b"Old")
            .unwrap();
        session
            .apply_source_edits(
                Revision::INITIAL,
                vec![SourceEdit {
                    file_id: bibliography.file_id,
                    start_byte: start,
                    end_byte: start + 3,
                    replacement: "New".into(),
                    expected_slice_hash: hash_bytes(b"Old"),
                }],
            )
            .unwrap();
        assert!(session.projection(bibliography.file_id).is_err());
        assert!(
            std::str::from_utf8(session.file(bibliography.file_id).unwrap().bytes())
                .unwrap()
                .contains("New")
        );
    }
}
