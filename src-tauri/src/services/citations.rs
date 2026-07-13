//! Local bibliography operations and explicitly-triggered metadata lookup.
//!
//! BibTeX bytes remain authoritative. Tree-sitter is used only to locate
//! entries and fields; edits are exact, hash-guarded source patches. Network
//! access has no background path: callers must invoke [`CitationLookupService::lookup_explicit`]
//! with an explicit-user-action trigger, and URLs are built internally for one
//! of two allowlisted HTTPS endpoints.

use crate::core::contracts::{FileId, SourceEdit};
use crate::core::source::hash_bytes;
use reqwest::{Client, Url, redirect};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use thiserror::Error;
use tree_sitter::{Node, Parser};

#[path = "../../../vendor/tree-sitter-bibtex/bindings/rust/lib.rs"]
mod bibtex_grammar;

const CROSSREF_ENDPOINT: &str = "https://api.crossref.org/works";
const ARXIV_ENDPOINT: &str = "https://export.arxiv.org/api/query";
const CROSSREF_HOST: &str = "api.crossref.org";
const ARXIV_HOST: &str = "export.arxiv.org";
const MAX_QUERY_CHARS: usize = 512;
const MAX_QUERY_BYTES: usize = 2_048;
const MAX_RESULTS: usize = 20;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str = "Setwright/0.1 (citation metadata lookup; local-first desktop app)";

/// Static node schema for diagnostics and compatibility tooling.
pub const BIBTEX_NODE_TYPES: &str = bibtex_grammar::NODE_TYPES;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextSpan {
    pub start_byte: usize,
    pub end_byte: usize,
}

impl TextSpan {
    #[must_use]
    pub const fn new(start_byte: usize, end_byte: usize) -> Self {
        Self {
            start_byte,
            end_byte,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibField {
    pub name: String,
    /// The exact value token, including braces or quotes.
    pub raw_value: String,
    /// A whitespace-normalized display value. This is never serialized back.
    pub display_value: String,
    pub span: TextSpan,
    pub value_span: TextSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibEntry {
    pub entry_type: String,
    pub key: String,
    pub span: TextSpan,
    pub key_span: TextSpan,
    pub fields: Vec<BibField>,
    pub malformed: bool,
}

impl BibEntry {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&BibField> {
        self.fields
            .iter()
            .find(|field| field.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BibFindingSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibFinding {
    pub severity: BibFindingSeverity,
    pub code: String,
    pub message: String,
    pub entry_key: Option<String>,
    pub span: TextSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibDocument {
    pub entries: Vec<BibEntry>,
    pub findings: Vec<BibFinding>,
    pub source_hash: String,
    pub has_parse_errors: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibSearchResult {
    pub key: String,
    pub entry_type: String,
    pub title: Option<String>,
    pub authors: Option<String>,
    pub year: Option<String>,
    pub score: u32,
    pub span: TextSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibEntryDraft {
    pub entry_type: String,
    pub key: String,
    /// Plain LaTeX values. The formatter wraps each value in braces.
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BibEditPlan {
    pub edits: Vec<SourceEdit>,
    pub inserts_new_entry: bool,
    pub affected_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CitationSourceFile {
    pub file_id: FileId,
    pub relative_path: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CitationKeyRenamePlan {
    pub old_key: String,
    pub new_key: String,
    pub edits_by_file: BTreeMap<FileId, Vec<SourceEdit>>,
    pub citation_occurrences: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetadataProvider {
    Crossref,
    Arxiv,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LookupTrigger {
    ExplicitUserAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataLookupRequest {
    pub provider: MetadataProvider,
    pub query: String,
    pub max_results: usize,
    pub trigger: LookupTrigger,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CitationAuthor {
    pub given: Option<String>,
    pub family: Option<String>,
    pub literal: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CitationWorkType {
    Article,
    ConferencePaper,
    Book,
    Thesis,
    Preprint,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CitationMetadata {
    pub provider: MetadataProvider,
    pub provider_id: String,
    pub work_type: CitationWorkType,
    pub title: String,
    pub authors: Vec<CitationAuthor>,
    pub issued_year: Option<i32>,
    pub venue: Option<String>,
    pub abstract_text: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataLookupResponse {
    pub provider: MetadataProvider,
    pub query: String,
    pub results: Vec<CitationMetadata>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq, Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CitationError {
    #[error("BibTeX parser could not be initialized: {message}")]
    ParserUnavailable { message: String },
    #[error("invalid bibliography edit: {message}")]
    InvalidEdit { message: String },
    #[error("citation key not found: {key}")]
    KeyNotFound { key: String },
    #[error("citation key is ambiguous because it appears more than once: {key}")]
    AmbiguousKey { key: String },
    #[error("citation key already exists: {key}")]
    DuplicateKey { key: String },
    #[error("unsafe citation syntax in {path} at byte {at_byte}")]
    UnsafeCitationSyntax { path: String, at_byte: usize },
    #[error("metadata query is invalid: {message}")]
    InvalidQuery { message: String },
    #[error("metadata endpoint was rejected: {url}")]
    EndpointRejected { url: String },
    #[error("metadata endpoint rejected a redirect to {url}")]
    RedirectRejected { url: String },
    #[error("metadata request failed: {message}")]
    Network { message: String },
    #[error("metadata endpoint returned HTTP {status}")]
    HttpStatus { status: u16 },
    #[error("metadata response exceeded {limit_bytes} bytes")]
    ResponseTooLarge { limit_bytes: usize },
    #[error("metadata response was invalid: {message}")]
    InvalidResponse { message: String },
}

/// Parse BibTeX into tolerant structural locations without normalizing source.
pub fn parse_bibliography(source: &str) -> Result<BibDocument, CitationError> {
    let mut parser = Parser::new();
    let language: tree_sitter::Language = bibtex_grammar::LANGUAGE.into();
    parser
        .set_language(&language)
        .map_err(|error| CitationError::ParserUnavailable {
            message: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| CitationError::ParserUnavailable {
            message: "parser returned no tree".to_owned(),
        })?;
    let root = tree.root_node();
    let mut entries = Vec::new();
    collect_entries(root, source, &mut entries);

    let mut findings = parser_findings(root, source);
    findings.extend(duplicate_findings(&entries));
    findings.extend(missing_field_findings(&entries));
    findings.sort_by_key(|finding| (finding.span.start_byte, finding.code.clone()));

    Ok(BibDocument {
        entries,
        has_parse_errors: root.has_error(),
        findings,
        source_hash: hash_bytes(source.as_bytes()),
    })
}

fn collect_entries(node: Node<'_>, source: &str, entries: &mut Vec<BibEntry>) {
    if node.kind() == "entry" {
        if let Some(entry) = decode_entry(node, source) {
            entries.push(entry);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_entries(child, source, entries);
    }
}

fn decode_entry(node: Node<'_>, source: &str) -> Option<BibEntry> {
    let entry_type_node = node.child_by_field_name("ty")?;
    let key_node = node.child_by_field_name("key")?;
    let entry_type = source
        .get(entry_type_node.byte_range())?
        .trim_start_matches('@')
        .to_ascii_lowercase();
    let key = source.get(key_node.byte_range())?.to_owned();
    let mut fields = Vec::new();
    let mut cursor = node.walk();
    for field_node in node.children_by_field_name("field", &mut cursor) {
        let Some(name_node) = field_node.child_by_field_name("name") else {
            continue;
        };
        let Some(value_node) = field_node.child_by_field_name("value") else {
            continue;
        };
        let Some(name) = source.get(name_node.byte_range()) else {
            continue;
        };
        let Some(raw_value) = source.get(value_node.byte_range()) else {
            continue;
        };
        fields.push(BibField {
            name: name.to_ascii_lowercase(),
            raw_value: raw_value.to_owned(),
            display_value: display_bib_value(raw_value),
            span: TextSpan::new(field_node.start_byte(), field_node.end_byte()),
            value_span: TextSpan::new(value_node.start_byte(), value_node.end_byte()),
        });
    }
    Some(BibEntry {
        entry_type,
        key,
        span: TextSpan::new(node.start_byte(), node.end_byte()),
        key_span: TextSpan::new(key_node.start_byte(), key_node.end_byte()),
        fields,
        malformed: node.has_error(),
    })
}

fn parser_findings(root: Node<'_>, source: &str) -> Vec<BibFinding> {
    let mut findings = Vec::new();
    collect_parser_findings(root, source, &mut findings);
    findings
}

fn collect_parser_findings(node: Node<'_>, source: &str, findings: &mut Vec<BibFinding>) {
    if node.is_error() || node.is_missing() {
        findings.push(BibFinding {
            severity: BibFindingSeverity::Error,
            code: "bibtex-parse-error".to_owned(),
            message: "This region is malformed and will remain byte-preserved source.".to_owned(),
            entry_key: None,
            span: TextSpan::new(node.start_byte(), node.end_byte()),
        });
        return;
    }
    if node.kind() == "junk"
        && source
            .get(node.byte_range())
            .is_some_and(|text| !text.trim().is_empty() && !bibtex_line_comments_only(text))
    {
        findings.push(BibFinding {
            severity: BibFindingSeverity::Warning,
            code: "bibtex-unparsed-source".to_owned(),
            message: "Unrecognized BibTeX source is preserved unchanged.".to_owned(),
            entry_key: None,
            span: TextSpan::new(node.start_byte(), node.end_byte()),
        });
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_parser_findings(child, source, findings);
    }
}

fn bibtex_line_comments_only(source: &str) -> bool {
    source
        .lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('%'))
}

fn duplicate_findings(entries: &[BibEntry]) -> Vec<BibFinding> {
    let mut first_by_key: BTreeMap<&str, &BibEntry> = BTreeMap::new();
    let mut findings = Vec::new();
    for entry in entries {
        if first_by_key.insert(&entry.key, entry).is_some() {
            findings.push(BibFinding {
                severity: BibFindingSeverity::Error,
                code: "duplicate-citation-key".to_owned(),
                message: format!("Citation key `{}` is defined more than once.", entry.key),
                entry_key: Some(entry.key.clone()),
                span: entry.key_span.clone(),
            });
        }
    }
    findings
}

fn missing_field_findings(entries: &[BibEntry]) -> Vec<BibFinding> {
    let mut findings = Vec::new();
    for entry in entries {
        let present = entry
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<BTreeSet<_>>();
        let requirements: &[&[&str]] = match entry.entry_type.as_str() {
            "article" => &[&["author"], &["title"], &["journal"], &["year", "date"]],
            "inproceedings" | "conference" => {
                &[&["author"], &["title"], &["booktitle"], &["year", "date"]]
            }
            "book" => &[
                &["author", "editor"],
                &["title"],
                &["publisher"],
                &["year", "date"],
            ],
            "phdthesis" | "mastersthesis" => {
                &[&["author"], &["title"], &["school"], &["year", "date"]]
            }
            _ => &[],
        };
        for alternatives in requirements {
            if !alternatives.iter().any(|name| present.contains(name)) {
                let label = alternatives.join(" or ");
                findings.push(BibFinding {
                    severity: BibFindingSeverity::Warning,
                    code: "missing-recommended-field".to_owned(),
                    message: format!("`{}` is missing `{label}`.", entry.key),
                    entry_key: Some(entry.key.clone()),
                    span: entry.span.clone(),
                });
            }
        }
    }
    findings
}

/// Search local entries only. No network call can originate from this API.
pub fn search_bibliography(document: &BibDocument, query: &str) -> Vec<BibSearchResult> {
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return document
            .entries
            .iter()
            .take(50)
            .map(|entry| search_result(entry, 0))
            .collect();
    }

    let mut results = document
        .entries
        .iter()
        .filter_map(|entry| {
            let key = entry.key.to_lowercase();
            let title = field_display(entry, "title").unwrap_or_default();
            let authors = field_display(entry, "author").unwrap_or_default();
            let year = field_display(entry, "year").unwrap_or_default();
            let doi = field_display(entry, "doi").unwrap_or_default();
            let haystack = format!("{key}\n{title}\n{authors}\n{year}\n{doi}").to_lowercase();
            if !terms.iter().all(|term| haystack.contains(term)) {
                return None;
            }
            let mut score = 0;
            for term in &terms {
                if key == *term {
                    score += 100;
                } else if key.starts_with(term) {
                    score += 60;
                } else if key.contains(term) {
                    score += 40;
                }
                if title.to_lowercase().contains(term) {
                    score += 20;
                }
                if authors.to_lowercase().contains(term) {
                    score += 10;
                }
            }
            Some(search_result(entry, score))
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.key.cmp(&right.key))
    });
    results.truncate(50);
    results
}

fn search_result(entry: &BibEntry, score: u32) -> BibSearchResult {
    BibSearchResult {
        key: entry.key.clone(),
        entry_type: entry.entry_type.clone(),
        title: field_display(entry, "title"),
        authors: field_display(entry, "author"),
        year: field_display(entry, "year").or_else(|| field_display(entry, "date")),
        score,
        span: entry.span.clone(),
    }
}

fn field_display(entry: &BibEntry, name: &str) -> Option<String> {
    entry.field(name).map(|field| field.display_value.clone())
}

/// Insert a new entry or replace exactly the target entry's source span.
/// Bytes before and after that span are never regenerated.
pub fn plan_upsert_entry(
    file_id: FileId,
    source: &str,
    draft: &BibEntryDraft,
) -> Result<BibEditPlan, CitationError> {
    validate_draft(draft)?;
    let document = parse_bibliography(source)?;
    let matches = document
        .entries
        .iter()
        .filter(|entry| entry.key == draft.key)
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(CitationError::AmbiguousKey {
            key: draft.key.clone(),
        });
    }
    let newline = preferred_newline(source);
    let rendered = render_entry(draft, newline);
    let (start_byte, end_byte, replacement, inserts_new_entry) =
        if let Some(entry) = matches.first() {
            (entry.span.start_byte, entry.span.end_byte, rendered, false)
        } else {
            let separator = append_separator(source, newline);
            (
                source.len(),
                source.len(),
                format!("{separator}{rendered}"),
                true,
            )
        };
    Ok(BibEditPlan {
        edits: vec![source_edit(
            file_id,
            source,
            start_byte,
            end_byte,
            replacement,
        )],
        inserts_new_entry,
        affected_key: draft.key.clone(),
    })
}

pub fn plan_delete_entry(
    file_id: FileId,
    source: &str,
    key: &str,
) -> Result<BibEditPlan, CitationError> {
    let document = parse_bibliography(source)?;
    let matches = document
        .entries
        .iter()
        .filter(|entry| entry.key == key)
        .collect::<Vec<_>>();
    let entry = match matches.as_slice() {
        [] => {
            return Err(CitationError::KeyNotFound {
                key: key.to_owned(),
            });
        }
        [entry] => *entry,
        _ => {
            return Err(CitationError::AmbiguousKey {
                key: key.to_owned(),
            });
        }
    };
    Ok(BibEditPlan {
        edits: vec![source_edit(
            file_id,
            source,
            entry.span.start_byte,
            entry.span.end_byte,
            String::new(),
        )],
        inserts_new_entry: false,
        affected_key: key.to_owned(),
    })
}

/// Plan an exact bibliography-key edit plus exact occurrences in recognized
/// LaTeX citation commands. Ambiguous syntax causes the whole plan to fail.
pub fn plan_citation_key_rename(
    bib_file_id: FileId,
    bib_source: &str,
    tex_files: &[CitationSourceFile],
    old_key: &str,
    new_key: &str,
) -> Result<CitationKeyRenamePlan, CitationError> {
    validate_key(new_key)?;
    if old_key == new_key {
        return Ok(CitationKeyRenamePlan {
            old_key: old_key.to_owned(),
            new_key: new_key.to_owned(),
            edits_by_file: BTreeMap::new(),
            citation_occurrences: 0,
        });
    }
    let document = parse_bibliography(bib_source)?;
    ensure_bibliography_can_be_renamed(&document)?;
    let old_matches = document
        .entries
        .iter()
        .filter(|entry| entry.key == old_key)
        .collect::<Vec<_>>();
    let old_entry = match old_matches.as_slice() {
        [] => {
            return Err(CitationError::KeyNotFound {
                key: old_key.to_owned(),
            });
        }
        [entry] => *entry,
        _ => {
            return Err(CitationError::AmbiguousKey {
                key: old_key.to_owned(),
            });
        }
    };
    if document.entries.iter().any(|entry| entry.key == new_key) {
        return Err(CitationError::DuplicateKey {
            key: new_key.to_owned(),
        });
    }

    let mut edits_by_file = BTreeMap::new();
    edits_by_file.insert(
        bib_file_id,
        vec![source_edit(
            bib_file_id,
            bib_source,
            old_entry.key_span.start_byte,
            old_entry.key_span.end_byte,
            new_key.to_owned(),
        )],
    );
    let mut citation_occurrences = 0;
    for file in tex_files {
        reject_unsafe_citation_contexts(&file.source, old_key, &file.relative_path)?;
        let spans = citation_key_spans(&file.source, old_key, &file.relative_path)?;
        if spans.is_empty() {
            continue;
        }
        citation_occurrences += spans.len();
        let edits = spans
            .into_iter()
            .map(|span| {
                source_edit(
                    file.file_id,
                    &file.source,
                    span.start_byte,
                    span.end_byte,
                    new_key.to_owned(),
                )
            })
            .collect();
        edits_by_file.insert(file.file_id, edits);
    }
    Ok(CitationKeyRenamePlan {
        old_key: old_key.to_owned(),
        new_key: new_key.to_owned(),
        edits_by_file,
        citation_occurrences,
    })
}

/// A key rename must be able to prove that the parser found every possible
/// definition. Editing a partially parsed bibliography could otherwise create
/// a duplicate key hidden in byte-preserved source.
pub(crate) fn ensure_bibliography_can_be_renamed(
    document: &BibDocument,
) -> Result<(), CitationError> {
    if document.has_parse_errors
        || document
            .findings
            .iter()
            .any(|finding| finding.code == "bibtex-unparsed-source")
    {
        return Err(CitationError::InvalidEdit {
            message: "citation keys cannot be renamed while a bibliography contains malformed or unparsed source"
                .to_owned(),
        });
    }
    Ok(())
}

/// Reject citation-shaped text in TeX regions where a lexical replacement is
/// not semantically trustworthy. This is deliberately conservative: these
/// contexts are left byte-authoritative rather than guessed at.
fn reject_unsafe_citation_contexts(
    source: &str,
    target_key: &str,
    path: &str,
) -> Result<(), CitationError> {
    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' && !is_escaped(bytes, cursor) {
            cursor = skip_to_next_line(bytes, cursor + 1);
            continue;
        }
        if bytes[cursor] != b'\\' {
            cursor += 1;
            continue;
        }

        let command_start = cursor;
        let (name, command_end) = control_word(source, command_start);
        if name.is_empty() {
            cursor = command_start + 1;
            continue;
        }

        if is_stateful_tex_command(name) {
            return unsafe_citation(path, command_start);
        }

        if name == "verb" {
            let mut delimiter_at = command_end;
            if bytes.get(delimiter_at) == Some(&b'*') {
                delimiter_at += 1;
            }
            if let Some(&delimiter) = bytes.get(delimiter_at)
                && !delimiter.is_ascii_whitespace()
            {
                let content_start = delimiter_at + 1;
                let content_end = bytes[content_start..]
                    .iter()
                    .position(|byte| *byte == delimiter)
                    .map_or_else(
                        || skip_to_next_line(bytes, content_start),
                        |offset| content_start + offset,
                    );
                if contains_target_citation(source, content_start, content_end, target_key) {
                    return unsafe_citation(path, command_start);
                }
                cursor = content_end.saturating_add(1);
                continue;
            }
        }

        if name == "begin"
            && let Some((environment, content_start)) = braced_text_after(source, command_end)
            && is_literal_environment(environment)
        {
            let terminator = format!("\\end{{{environment}}}");
            let content_end = source[content_start..]
                .find(&terminator)
                .map_or(source.len(), |offset| content_start + offset);
            if contains_target_citation(source, content_start, content_end, target_key) {
                return unsafe_citation(path, command_start);
            }
            cursor = content_end
                .saturating_add(terminator.len())
                .min(source.len());
            continue;
        }

        if is_macro_definition_command(name) {
            let (defined_name, body_ranges) = macro_definition_bodies(source, name, command_end);
            if defined_name
                .as_deref()
                .is_some_and(|defined| is_citation_command(defined.trim_end_matches('*')))
                || body_ranges
                    .iter()
                    .any(|(start, end)| contains_target_citation(source, *start, *end, target_key))
            {
                return unsafe_citation(path, command_start);
            }
        }

        if is_conditional_start(name) {
            let conditional_end =
                matching_conditional_end(source, command_end).unwrap_or(source.len());
            if contains_target_citation(source, command_end, conditional_end, target_key) {
                return unsafe_citation(path, command_start);
            }
            cursor = conditional_end;
            continue;
        }

        cursor = command_end.max(command_start + 1);
    }
    Ok(())
}

fn unsafe_citation<T>(path: &str, at_byte: usize) -> Result<T, CitationError> {
    Err(CitationError::UnsafeCitationSyntax {
        path: path.to_owned(),
        at_byte,
    })
}

fn control_word(source: &str, command_start: usize) -> (&str, usize) {
    let bytes = source.as_bytes();
    let mut cursor = command_start.saturating_add(1);
    let name_start = cursor;
    while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
        cursor += 1;
    }
    (source.get(name_start..cursor).unwrap_or_default(), cursor)
}

fn is_literal_environment(name: &str) -> bool {
    matches!(name, "verbatim" | "verbatim*" | "lstlisting" | "minted")
}

fn is_macro_definition_command(name: &str) -> bool {
    matches!(
        name,
        "def"
            | "gdef"
            | "edef"
            | "xdef"
            | "newcommand"
            | "renewcommand"
            | "providecommand"
            | "DeclareRobustCommand"
            | "newenvironment"
            | "renewenvironment"
            | "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand"
    )
}

fn is_stateful_tex_command(name: &str) -> bool {
    matches!(
        name,
        "catcode"
            | "let"
            | "futurelet"
            | "csname"
            | "endcsname"
            | "expandafter"
            | "scantokens"
            | "toksdef"
            | "chardef"
            | "mathchardef"
            | "countdef"
            | "dimendef"
            | "skipdef"
            | "muskipdef"
            | "read"
            | "openin"
            | "openout"
            | "write"
            | "immediate"
            | "special"
            | "makeatletter"
            | "makeatother"
            | "ExplSyntaxOn"
            | "ExplSyntaxOff"
    )
}

fn is_conditional_start(name: &str) -> bool {
    name == "if" || name.starts_with("if") || name.starts_with("If")
}

fn braced_text_after(source: &str, from: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    let opening = skip_ascii_whitespace(bytes, from);
    if bytes.get(opening) != Some(&b'{') {
        return None;
    }
    let after = scan_balanced(bytes, opening, b'{', b'}')?;
    let closing = after.checked_sub(1)?;
    Some((source.get(opening + 1..closing)?, after))
}

fn macro_definition_bodies(
    source: &str,
    command_name: &str,
    command_end: usize,
) -> (Option<String>, Vec<(usize, usize)>) {
    let bytes = source.as_bytes();
    let mut cursor = skip_ascii_whitespace(bytes, command_end);
    let mut defined_name = None;

    if matches!(command_name, "def" | "gdef" | "edef" | "xdef") {
        if bytes.get(cursor) == Some(&b'\\') {
            let (name, after_name) = control_word(source, cursor);
            defined_name = Some(name.to_owned());
            cursor = after_name;
        }
        while cursor < bytes.len() && bytes[cursor] != b'{' {
            if bytes[cursor] == b'%' && !is_escaped(bytes, cursor) {
                return (defined_name, Vec::new());
            }
            cursor += 1;
        }
        return (
            defined_name,
            braced_range(bytes, cursor).into_iter().collect(),
        );
    }

    let definition_is_unbraced = bytes.get(cursor) == Some(&b'\\');
    if definition_is_unbraced {
        let (name, after_name) = control_word(source, cursor);
        defined_name = Some(name.to_owned());
        cursor = after_name;
        if bytes.get(cursor) == Some(&b'*') {
            cursor += 1;
        }
    }

    let mut groups = Vec::new();
    let mut optional_ranges = Vec::new();
    for _ in 0..8 {
        cursor = skip_ascii_whitespace(bytes, cursor);
        if bytes.get(cursor) == Some(&b'[') {
            let Some(after) = scan_balanced(bytes, cursor, b'[', b']') else {
                break;
            };
            optional_ranges.push((cursor + 1, after - 1));
            cursor = after;
            continue;
        }
        if bytes.get(cursor) != Some(&b'{') {
            break;
        }
        let Some((start, end)) = braced_range(bytes, cursor) else {
            break;
        };
        groups.push((start, end));
        cursor = end.saturating_add(1);
    }
    if !definition_is_unbraced && let Some((first_start, first_end)) = groups.first().copied() {
        let first = source
            .get(first_start..first_end)
            .unwrap_or_default()
            .trim();
        if let Some(stripped) = first.strip_prefix('\\') {
            defined_name = Some(
                stripped
                    .trim_end_matches('*')
                    .chars()
                    .take_while(|character| character.is_ascii_alphabetic())
                    .collect(),
            );
        }
    }
    let mut bodies = if definition_is_unbraced {
        groups
    } else if groups.len() > 1 {
        groups.into_iter().skip(1).collect()
    } else {
        Vec::new()
    };
    bodies.extend(optional_ranges);
    (defined_name, bodies)
}

/// Returns the content range of a balanced braced group.
fn braced_range(bytes: &[u8], opening: usize) -> Option<(usize, usize)> {
    if bytes.get(opening) != Some(&b'{') {
        return None;
    }
    let after = scan_balanced(bytes, opening, b'{', b'}')?;
    Some((opening + 1, after - 1))
}

fn matching_conditional_end(source: &str, from: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = from;
    let mut depth = 1usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' && !is_escaped(bytes, cursor) {
            cursor = skip_to_next_line(bytes, cursor + 1);
            continue;
        }
        if bytes[cursor] != b'\\' {
            cursor += 1;
            continue;
        }
        let (name, after) = control_word(source, cursor);
        if is_conditional_start(name) {
            depth += 1;
        } else if name == "fi" {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(after);
            }
        }
        cursor = after.max(cursor + 1);
    }
    None
}

fn contains_target_citation(source: &str, start: usize, end: usize, target_key: &str) -> bool {
    let bytes = source.as_bytes();
    let end = end.min(bytes.len());
    let mut cursor = start.min(end);
    while cursor < end {
        if bytes[cursor] != b'\\' {
            cursor += 1;
            continue;
        }
        let (name, after_name) = control_word(source, cursor);
        if !is_citation_command(name) {
            cursor = after_name.max(cursor + 1);
            continue;
        }
        let mut argument = after_name;
        if bytes.get(argument) == Some(&b'*') {
            argument += 1;
        }
        argument = skip_ascii_whitespace(bytes, argument);
        for _ in 0..2 {
            if bytes.get(argument) == Some(&b'[') {
                let Some(after) = scan_balanced(bytes, argument, b'[', b']') else {
                    break;
                };
                argument = skip_ascii_whitespace(bytes, after);
            }
        }
        if bytes.get(argument) == Some(&b'{') {
            let argument_end = scan_balanced(bytes, argument, b'{', b'}')
                .map_or(end, |after| after.saturating_sub(1).min(end));
            if key_token_occurs(bytes, argument + 1, argument_end, target_key.as_bytes()) {
                return true;
            }
        }
        cursor = after_name.max(cursor + 1);
    }
    false
}

fn key_token_occurs(bytes: &[u8], start: usize, end: usize, key: &[u8]) -> bool {
    if key.is_empty() || start > end || end > bytes.len() {
        return false;
    }
    bytes[start..end]
        .windows(key.len())
        .enumerate()
        .any(|(offset, candidate)| {
            let key_start = start + offset;
            candidate == key
                && citation_key_boundary(
                    key_start
                        .checked_sub(1)
                        .and_then(|before| bytes.get(before))
                        .copied(),
                )
                && citation_key_boundary(bytes.get(key_start + key.len()).copied())
        })
}

fn citation_key_boundary(byte: Option<u8>) -> bool {
    let Some(byte) = byte else {
        return true;
    };
    matches!(byte, b'{' | b'}' | b',' | b' ' | b'\t' | b'\r' | b'\n')
}

fn citation_key_spans(
    source: &str,
    target_key: &str,
    path: &str,
) -> Result<Vec<TextSpan>, CitationError> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' && !is_escaped(bytes, cursor) {
            cursor = skip_to_next_line(bytes, cursor + 1);
            continue;
        }
        if bytes[cursor] != b'\\' {
            cursor += 1;
            continue;
        }
        let command_start = cursor;
        cursor += 1;
        let name_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
            cursor += 1;
        }
        if cursor < bytes.len() && bytes[cursor] == b'*' {
            cursor += 1;
        }
        let command_name = &source[name_start..cursor];
        if !is_citation_command(command_name.trim_end_matches('*')) {
            cursor = cursor.max(command_start + 1);
            continue;
        }
        cursor = skip_ascii_whitespace(bytes, cursor);
        for _ in 0..2 {
            if cursor < bytes.len() && bytes[cursor] == b'[' {
                cursor = scan_balanced(bytes, cursor, b'[', b']').ok_or_else(|| {
                    CitationError::UnsafeCitationSyntax {
                        path: path.to_owned(),
                        at_byte: command_start,
                    }
                })?;
                cursor = skip_ascii_whitespace(bytes, cursor);
            }
        }
        if cursor >= bytes.len() || bytes[cursor] != b'{' {
            continue;
        }
        let argument_start = cursor + 1;
        let argument_end = scan_flat_braced_argument(bytes, cursor).ok_or_else(|| {
            CitationError::UnsafeCitationSyntax {
                path: path.to_owned(),
                at_byte: command_start,
            }
        })?;
        for (segment_start, segment_end) in
            comma_separated_segments(bytes, argument_start, argument_end)
        {
            let start = skip_ascii_whitespace(bytes, segment_start);
            let end = trim_ascii_whitespace_end(bytes, start, segment_end);
            if source.get(start..end) == Some(target_key) {
                spans.push(TextSpan::new(start, end));
            }
        }
        cursor = argument_end + 1;
    }
    Ok(spans)
}

fn is_citation_command(name: &str) -> bool {
    matches!(
        name,
        "cite"
            | "citep"
            | "citet"
            | "citealp"
            | "citealt"
            | "autocite"
            | "parencite"
            | "textcite"
            | "footcite"
            | "smartcite"
            | "supercite"
            | "nocite"
    )
}

fn scan_flat_braced_argument(bytes: &[u8], opening: usize) -> Option<usize> {
    let mut cursor = opening + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'}' if !is_escaped(bytes, cursor) => return Some(cursor),
            b'{' | b'%' if !is_escaped(bytes, cursor) => return None,
            _ => cursor += 1,
        }
    }
    None
}

fn comma_separated_segments(bytes: &[u8], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut segment_start = start;
    for (offset, byte) in bytes[start..end].iter().enumerate() {
        if *byte == b',' {
            let index = start + offset;
            segments.push((segment_start, index));
            segment_start = index + 1;
        }
    }
    segments.push((segment_start, end));
    segments
}

fn scan_balanced(bytes: &[u8], opening: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut cursor = opening;
    while cursor < bytes.len() {
        if bytes[cursor] == b'%' && !is_escaped(bytes, cursor) {
            cursor = skip_to_next_line(bytes, cursor + 1);
            continue;
        }
        if !is_escaped(bytes, cursor) {
            if bytes[cursor] == open {
                depth += 1;
            } else if bytes[cursor] == close {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
        }
        cursor += 1;
    }
    None
}

fn is_escaped(bytes: &[u8], at: usize) -> bool {
    let mut preceding = 0;
    let mut cursor = at;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        preceding += 1;
        cursor -= 1;
    }
    preceding % 2 == 1
}

fn skip_to_next_line(bytes: &[u8], mut cursor: usize) -> usize {
    while cursor < bytes.len() && bytes[cursor] != b'\n' && bytes[cursor] != b'\r' {
        cursor += 1;
    }
    cursor
}

fn skip_ascii_whitespace(bytes: &[u8], mut cursor: usize) -> usize {
    while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

fn trim_ascii_whitespace_end(bytes: &[u8], start: usize, mut end: usize) -> usize {
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn source_edit(
    file_id: FileId,
    source: &str,
    start_byte: usize,
    end_byte: usize,
    replacement: String,
) -> SourceEdit {
    SourceEdit {
        file_id,
        start_byte,
        end_byte,
        replacement,
        expected_slice_hash: hash_bytes(&source.as_bytes()[start_byte..end_byte]),
    }
}

fn validate_draft(draft: &BibEntryDraft) -> Result<(), CitationError> {
    validate_identifier(&draft.entry_type, "entry type")?;
    validate_key(&draft.key)?;
    if draft.fields.len() > 256 {
        return Err(CitationError::InvalidEdit {
            message: "an entry may contain at most 256 fields".to_owned(),
        });
    }
    for (name, value) in &draft.fields {
        validate_identifier(name, "field name")?;
        if value.len() > 256 * 1024 {
            return Err(CitationError::InvalidEdit {
                message: format!("field `{name}` exceeds 256 KiB"),
            });
        }
        if !braces_are_balanced(value) {
            return Err(CitationError::InvalidEdit {
                message: format!("field `{name}` contains unbalanced braces"),
            });
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), CitationError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':'))
    {
        return Err(CitationError::InvalidEdit {
            message: format!("{label} contains unsupported characters"),
        });
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), CitationError> {
    if key.is_empty()
        || key.len() > 256
        || !key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.' | b'/' | b'+')
        })
    {
        return Err(CitationError::InvalidEdit {
            message: "citation keys must use 1-256 portable ASCII key characters".to_owned(),
        });
    }
    Ok(())
}

fn braces_are_balanced(value: &str) -> bool {
    let mut depth = 0usize;
    for (index, byte) in value.bytes().enumerate() {
        if is_escaped(value.as_bytes(), index) {
            continue;
        }
        match byte {
            b'{' => depth += 1,
            b'}' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
            }
            _ => {}
        }
    }
    depth == 0
}

fn preferred_newline(source: &str) -> &'static str {
    if source.as_bytes().windows(2).any(|window| window == b"\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn append_separator(source: &str, newline: &str) -> String {
    if source.is_empty() || source.ends_with(&format!("{newline}{newline}")) {
        String::new()
    } else if source.ends_with(newline) {
        newline.to_owned()
    } else {
        format!("{newline}{newline}")
    }
}

fn render_entry(draft: &BibEntryDraft, newline: &str) -> String {
    let mut output = format!("@{}{{{},", draft.entry_type, draft.key);
    for (name, value) in &draft.fields {
        output.push_str(newline);
        output.push_str("  ");
        output.push_str(name);
        output.push_str(" = {");
        output.push_str(value);
        output.push_str("},");
    }
    output.push_str(newline);
    output.push('}');
    output
}

fn display_bib_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let unwrapped = if trimmed.len() >= 2
        && ((trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('"') && trimmed.ends_with('"')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    unwrapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone)]
pub struct CitationLookupService {
    client: Client,
}

impl CitationLookupService {
    pub fn new() -> Result<Self, CitationError> {
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(network_error)?;
        Ok(Self { client })
    }

    /// Perform a lookup only in direct response to an explicit UI action.
    /// This service has no timers, tasks, startup hooks, or implicit fallback.
    pub async fn lookup_explicit(
        &self,
        request: MetadataLookupRequest,
    ) -> Result<MetadataLookupResponse, CitationError> {
        validate_lookup_request(&request)?;
        let url = build_lookup_url(&request)?;
        validate_lookup_url(request.provider, &url)?;
        let mut response = self
            .client
            .get(url)
            .header("accept", provider_accept(request.provider))
            .send()
            .await
            .map_err(network_error)?;
        validate_lookup_url(request.provider, response.url())?;
        if response.status().is_redirection() {
            let target = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("unavailable")
                .to_owned();
            return Err(CitationError::RedirectRejected { url: target });
        }
        if !response.status().is_success() {
            return Err(CitationError::HttpStatus {
                status: response.status().as_u16(),
            });
        }
        let body = read_bounded_response(&mut response, MAX_RESPONSE_BYTES).await?;
        let results = match request.provider {
            MetadataProvider::Crossref => parse_crossref_response(&body, request.max_results)?,
            MetadataProvider::Arxiv => parse_arxiv_response(&body, request.max_results)?,
        };
        let truncated = results.len() == request.max_results;
        Ok(MetadataLookupResponse {
            provider: request.provider,
            query: request.query,
            results,
            truncated,
        })
    }
}

fn network_error(error: impl std::fmt::Display) -> CitationError {
    CitationError::Network {
        message: error.to_string(),
    }
}

pub fn validate_lookup_request(request: &MetadataLookupRequest) -> Result<(), CitationError> {
    let query = request.query.trim();
    if query.is_empty() {
        return Err(CitationError::InvalidQuery {
            message: "query cannot be empty".to_owned(),
        });
    }
    if query.chars().count() > MAX_QUERY_CHARS || query.len() > MAX_QUERY_BYTES {
        return Err(CitationError::InvalidQuery {
            message: format!("query exceeds {MAX_QUERY_CHARS} characters"),
        });
    }
    if request.max_results == 0 || request.max_results > MAX_RESULTS {
        return Err(CitationError::InvalidQuery {
            message: format!("maxResults must be between 1 and {MAX_RESULTS}"),
        });
    }
    if query.chars().any(char::is_control) {
        return Err(CitationError::InvalidQuery {
            message: "query contains control characters".to_owned(),
        });
    }
    Ok(())
}

pub fn build_lookup_url(request: &MetadataLookupRequest) -> Result<Url, CitationError> {
    validate_lookup_request(request)?;
    let endpoint = match request.provider {
        MetadataProvider::Crossref => CROSSREF_ENDPOINT,
        MetadataProvider::Arxiv => ARXIV_ENDPOINT,
    };
    let mut url = Url::parse(endpoint).map_err(|error| CitationError::EndpointRejected {
        url: error.to_string(),
    })?;
    match request.provider {
        MetadataProvider::Crossref => {
            url.query_pairs_mut()
                .append_pair("query.bibliographic", request.query.trim())
                .append_pair("rows", &request.max_results.to_string())
                .append_pair("select", "DOI,title,author,issued,published-print,published-online,container-title,type,abstract,URL");
        }
        MetadataProvider::Arxiv => {
            url.query_pairs_mut()
                .append_pair("search_query", &format!("all:{}", request.query.trim()))
                .append_pair("start", "0")
                .append_pair("max_results", &request.max_results.to_string())
                .append_pair("sortBy", "relevance");
        }
    }
    Ok(url)
}

pub fn validate_lookup_url(provider: MetadataProvider, url: &Url) -> Result<(), CitationError> {
    let (host, path) = match provider {
        MetadataProvider::Crossref => (CROSSREF_HOST, "/works"),
        MetadataProvider::Arxiv => (ARXIV_HOST, "/api/query"),
    };
    if url.scheme() != "https"
        || url.host_str() != Some(host)
        || url.port().is_some()
        || url.path() != path
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(CitationError::EndpointRejected {
            url: url.as_str().to_owned(),
        });
    }
    Ok(())
}

#[must_use]
pub fn redirect_target_is_allowed(provider: MetadataProvider, target: &Url) -> bool {
    // The runtime client rejects every redirect. Keeping this pure policy
    // predicate explicit makes the fail-closed behavior testable.
    let _ = provider;
    let _ = target;
    false
}

fn provider_accept(provider: MetadataProvider) -> &'static str {
    match provider {
        MetadataProvider::Crossref => "application/json",
        MetadataProvider::Arxiv => "application/atom+xml",
    }
}

async fn read_bounded_response(
    response: &mut reqwest::Response,
    limit_bytes: usize,
) -> Result<Vec<u8>, CitationError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit_bytes as u64)
    {
        return Err(CitationError::ResponseTooLarge { limit_bytes });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        extend_bounded(&mut body, &chunk, limit_bytes)?;
    }
    Ok(body)
}

fn extend_bounded(
    body: &mut Vec<u8>,
    chunk: &[u8],
    limit_bytes: usize,
) -> Result<(), CitationError> {
    let next_len = body
        .len()
        .checked_add(chunk.len())
        .ok_or(CitationError::ResponseTooLarge { limit_bytes })?;
    if next_len > limit_bytes {
        return Err(CitationError::ResponseTooLarge { limit_bytes });
    }
    body.extend_from_slice(chunk);
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CrossrefEnvelope {
    message: CrossrefMessage,
}

#[derive(Debug, Deserialize)]
struct CrossrefMessage {
    #[serde(default)]
    items: Vec<CrossrefItem>,
}

#[derive(Debug, Deserialize)]
struct CrossrefItem {
    #[serde(rename = "DOI")]
    doi: Option<String>,
    #[serde(default)]
    title: Vec<String>,
    #[serde(default)]
    author: Vec<CrossrefAuthor>,
    #[serde(default)]
    issued: Option<CrossrefDate>,
    #[serde(rename = "published-print", default)]
    published_print: Option<CrossrefDate>,
    #[serde(rename = "published-online", default)]
    published_online: Option<CrossrefDate>,
    #[serde(rename = "container-title", default)]
    container_title: Vec<String>,
    #[serde(rename = "type", default)]
    item_type: String,
    #[serde(rename = "abstract")]
    abstract_text: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CrossrefAuthor {
    given: Option<String>,
    family: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CrossrefDate {
    #[serde(rename = "date-parts", default)]
    date_parts: Vec<Vec<i32>>,
}

fn parse_crossref_response(
    bytes: &[u8],
    max_results: usize,
) -> Result<Vec<CitationMetadata>, CitationError> {
    let envelope: CrossrefEnvelope =
        serde_json::from_slice(bytes).map_err(|error| CitationError::InvalidResponse {
            message: error.to_string(),
        })?;
    Ok(envelope
        .message
        .items
        .into_iter()
        .take(max_results)
        .filter_map(normalize_crossref_item)
        .collect())
}

fn normalize_crossref_item(item: CrossrefItem) -> Option<CitationMetadata> {
    let title = item
        .title
        .into_iter()
        .find(|title| !title.trim().is_empty())?;
    let doi = item.doi.map(|doi| doi.trim().to_owned());
    let provider_id = doi.clone().or_else(|| item.url.clone())?;
    let authors = item
        .author
        .into_iter()
        .take(64)
        .filter_map(|author| {
            let literal = author.name.clone().unwrap_or_else(|| {
                [author.given.as_deref(), author.family.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ")
            });
            (!literal.trim().is_empty()).then_some(CitationAuthor {
                given: author.given,
                family: author.family,
                literal,
            })
        })
        .collect();
    let issued_year = year_from_crossref_date(item.issued.as_ref())
        .or_else(|| year_from_crossref_date(item.published_print.as_ref()))
        .or_else(|| year_from_crossref_date(item.published_online.as_ref()));
    Some(CitationMetadata {
        provider: MetadataProvider::Crossref,
        provider_id,
        work_type: crossref_work_type(&item.item_type),
        title: collapse_whitespace(&strip_xml_tags(&title)),
        authors,
        issued_year,
        venue: item
            .container_title
            .into_iter()
            .find(|venue| !venue.trim().is_empty())
            .map(|venue| collapse_whitespace(&strip_xml_tags(&venue))),
        abstract_text: item
            .abstract_text
            .map(|abstract_text| collapse_whitespace(&strip_xml_tags(&abstract_text)))
            .filter(|abstract_text| !abstract_text.is_empty()),
        doi,
        arxiv_id: None,
        url: item.url.filter(|candidate| safe_result_url(candidate)),
    })
}

fn year_from_crossref_date(date: Option<&CrossrefDate>) -> Option<i32> {
    date?.date_parts.first()?.first().copied()
}

fn crossref_work_type(value: &str) -> CitationWorkType {
    match value {
        "journal-article" => CitationWorkType::Article,
        "proceedings-article" => CitationWorkType::ConferencePaper,
        "book" | "book-chapter" | "edited-book" | "monograph" => CitationWorkType::Book,
        "dissertation" => CitationWorkType::Thesis,
        "posted-content" => CitationWorkType::Preprint,
        _ => CitationWorkType::Other,
    }
}

fn parse_arxiv_response(
    bytes: &[u8],
    max_results: usize,
) -> Result<Vec<CitationMetadata>, CitationError> {
    let source = std::str::from_utf8(bytes).map_err(|error| CitationError::InvalidResponse {
        message: error.to_string(),
    })?;
    if !source.contains("<feed") {
        return Err(CitationError::InvalidResponse {
            message: "arXiv response did not contain an Atom feed".to_owned(),
        });
    }
    Ok(xml_element_bodies(source, "entry")
        .into_iter()
        .take(max_results)
        .filter_map(normalize_arxiv_entry)
        .collect())
}

fn normalize_arxiv_entry(entry: &str) -> Option<CitationMetadata> {
    let id_url = xml_element_text(entry, "id")?;
    let arxiv_id = id_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .trim()
        .to_owned();
    if !safe_arxiv_id(&arxiv_id) {
        return None;
    }
    let title = collapse_whitespace(&xml_element_text(entry, "title")?);
    if title.is_empty() {
        return None;
    }
    let authors = xml_element_bodies(entry, "author")
        .into_iter()
        .take(64)
        .filter_map(|author| xml_element_text(author, "name"))
        .map(|literal| CitationAuthor {
            given: None,
            family: None,
            literal: collapse_whitespace(&literal),
        })
        .collect();
    let issued_year = xml_element_text(entry, "published")
        .and_then(|published| published.get(0..4).map(str::to_owned))
        .and_then(|year| year.parse::<i32>().ok());
    let doi = xml_element_text(entry, "arxiv:doi").filter(|doi| !doi.trim().is_empty());
    Some(CitationMetadata {
        provider: MetadataProvider::Arxiv,
        provider_id: arxiv_id.clone(),
        work_type: CitationWorkType::Preprint,
        title,
        authors,
        issued_year,
        venue: xml_element_text(entry, "arxiv:journal_ref")
            .map(|venue| collapse_whitespace(&venue))
            .filter(|venue| !venue.is_empty()),
        abstract_text: xml_element_text(entry, "summary")
            .map(|summary| collapse_whitespace(&summary))
            .filter(|summary| !summary.is_empty()),
        doi,
        arxiv_id: Some(arxiv_id.clone()),
        url: Some(format!("https://arxiv.org/abs/{arxiv_id}")),
    })
}

fn xml_element_bodies<'a>(source: &'a str, tag: &str) -> Vec<&'a str> {
    let mut bodies = Vec::new();
    let mut cursor = 0;
    let opening = format!("<{tag}");
    let closing = format!("</{tag}>");
    while let Some(relative_start) = source[cursor..].find(&opening) {
        let start = cursor + relative_start;
        let boundary = source.as_bytes().get(start + opening.len()).copied();
        if !matches!(
            boundary,
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')
        ) {
            cursor = start + opening.len();
            continue;
        }
        let Some(open_end_relative) = source[start..].find('>') else {
            break;
        };
        let body_start = start + open_end_relative + 1;
        let Some(close_relative) = source[body_start..].find(&closing) else {
            break;
        };
        let body_end = body_start + close_relative;
        bodies.push(&source[body_start..body_end]);
        cursor = body_end + closing.len();
    }
    bodies
}

fn xml_element_text(source: &str, tag: &str) -> Option<String> {
    xml_element_bodies(source, tag)
        .into_iter()
        .next()
        .map(|body| decode_xml_entities(&strip_xml_tags(body)))
}

fn strip_xml_tags(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut inside_tag = false;
    for character in source.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => output.push(character),
            _ => {}
        }
    }
    output
}

fn decode_xml_entities(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative_ampersand) = source[cursor..].find('&') {
        let ampersand = cursor + relative_ampersand;
        output.push_str(&source[cursor..ampersand]);
        let Some(relative_semicolon) = source[ampersand..].find(';') else {
            output.push_str(&source[ampersand..]);
            return output;
        };
        let semicolon = ampersand + relative_semicolon;
        let entity = &source[ampersand + 1..semicolon];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            numeric if numeric.starts_with("#x") => u32::from_str_radix(&numeric[2..], 16)
                .ok()
                .and_then(char::from_u32),
            numeric if numeric.starts_with('#') => {
                numeric[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(character) = decoded {
            output.push(character);
        } else {
            output.push_str(&source[ampersand..=semicolon]);
        }
        cursor = semicolon + 1;
    }
    output.push_str(&source[cursor..]);
    output
}

fn collapse_whitespace(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn safe_arxiv_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'/' | b'-' | b'_'))
}

fn safe_result_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contracts::Revision;
    use crate::core::source::SourceBuffer;
    use pretty_assertions::assert_eq;
    use reqwest::StatusCode;
    use uuid::Uuid;

    fn file_id(value: u128) -> FileId {
        FileId(Uuid::from_u128(value))
    }

    fn apply(source: &str, file_id: FileId, edits: &[SourceEdit]) -> String {
        let mut buffer = SourceBuffer::from_bytes(
            file_id,
            "references.bib",
            source.as_bytes().to_vec(),
            Revision::INITIAL,
        );
        buffer.apply_edits(edits, Revision(1)).unwrap();
        buffer.text().unwrap().to_owned()
    }

    #[test]
    fn parser_loads_vendored_tree_sitter_grammar() {
        let document = parse_bibliography("@article{key, title={A title}}").unwrap();
        assert_eq!(document.entries.len(), 1);
        assert_eq!(document.entries[0].key, "key");
    }

    #[test]
    fn malformed_bibtex_and_comments_are_preserved_by_neighboring_upsert() {
        let source = "% lead comment\r\n@article{broken, title={Never closes}\r\n\r\n@misc{safe,\r\n  title = {Old},\r\n}\r\n% tail\r\n";
        let draft = BibEntryDraft {
            entry_type: "misc".to_owned(),
            key: "safe".to_owned(),
            fields: BTreeMap::from([("title".to_owned(), "New".to_owned())]),
        };
        let plan = plan_upsert_entry(file_id(1), source, &draft).unwrap();
        let result = apply(source, file_id(1), &plan.edits);
        assert!(result.starts_with("% lead comment\r\n@article{broken"));
        assert!(result.ends_with("\r\n% tail\r\n"));
        assert!(result.contains("@misc{safe,\r\n  title = {New},\r\n}"));
    }

    #[test]
    fn local_search_matches_keys_titles_and_authors_without_network() {
        let source = r#"
@article{vaswani2017attention,
  title = {Attention Is All You Need},
  author = {Vaswani, Ashish and Shazeer, Noam},
  year = {2017},
  journal = {NeurIPS}
}
@misc{other, title={A different paper}}
"#;
        let document = parse_bibliography(source).unwrap();
        let results = search_bibliography(&document, "vaswani attention");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "vaswani2017attention");
    }

    #[test]
    fn duplicate_keys_and_missing_fields_are_reported() {
        let source = "@article{same, title={One}}\n@misc{same, title={Two}}";
        let document = parse_bibliography(source).unwrap();
        assert!(
            document
                .findings
                .iter()
                .any(|finding| finding.code == "duplicate-citation-key")
        );
        assert!(
            document
                .findings
                .iter()
                .any(|finding| finding.code == "missing-recommended-field")
        );
    }

    #[test]
    fn rename_plan_changes_only_exact_key_tokens_and_skips_comments() {
        let bib_id = file_id(1);
        let tex_id = file_id(2);
        let bib = "% keep\n@article{old-key, title={Keep old-key in title}}\n";
        let tex =
            "% \\cite{old-key}\nText \\citep[see][p.~2]{ old-key, other }.\nRaw old-key stays.\n";
        let plan = plan_citation_key_rename(
            bib_id,
            bib,
            &[CitationSourceFile {
                file_id: tex_id,
                relative_path: "main.tex".to_owned(),
                source: tex.to_owned(),
            }],
            "old-key",
            "new-key",
        )
        .unwrap();
        assert_eq!(plan.citation_occurrences, 1);
        let updated_bib = apply(bib, bib_id, &plan.edits_by_file[&bib_id]);
        let updated_tex = apply(tex, tex_id, &plan.edits_by_file[&tex_id]);
        assert_eq!(
            updated_bib,
            "% keep\n@article{new-key, title={Keep old-key in title}}\n"
        );
        assert_eq!(
            updated_tex,
            "% \\cite{old-key}\nText \\citep[see][p.~2]{ new-key, other }.\nRaw old-key stays.\n"
        );
    }

    #[test]
    fn unsafe_nested_citation_argument_rejects_whole_rename() {
        let result = plan_citation_key_rename(
            file_id(1),
            "@misc{old, title={T}}",
            &[CitationSourceFile {
                file_id: file_id(2),
                relative_path: "main.tex".to_owned(),
                source: "\\cite{{old}}".to_owned(),
            }],
            "old",
            "new",
        );
        assert!(matches!(
            result,
            Err(CitationError::UnsafeCitationSyntax { .. })
        ));
    }

    #[test]
    fn rename_rejects_citation_text_in_literal_macro_and_conditional_contexts() {
        let unsafe_sources = [
            "\\begin{verbatim}\\cite{old}\\end{verbatim}",
            "\\begin{lstlisting}\n\\cite{old}\n\\end{lstlisting}",
            "\\begin{minted}{tex}\\cite{old}\\end{minted}",
            "\\verb|\\cite{old}|",
            "\\verb*+\\cite{old}+",
            "\\newcommand{\\wrappedcite}{\\cite{old}}",
            "\\newcommand\\wrappedcite{\\cite{old}}",
            "\\newcommand{\\wrappedcite}[1][\\cite{old}]{#1}",
            "\\renewcommand\\cite[1]{hidden}",
            "\\def\\wrappedcite#1{\\cite{old}}",
            "\\ifdraft\\cite{old}\\else text\\fi",
            "\\ifcase0 text\\or \\cite{old}\\fi",
        ];
        for (index, source) in unsafe_sources.into_iter().enumerate() {
            let result = plan_citation_key_rename(
                file_id(1),
                "@misc{old, title={T}}",
                &[CitationSourceFile {
                    file_id: file_id(100 + index as u128),
                    relative_path: format!("unsafe-{index}.tex"),
                    source: source.to_owned(),
                }],
                "old",
                "new",
            );
            assert!(
                matches!(result, Err(CitationError::UnsafeCitationSyntax { .. })),
                "unsafe context was accepted: {source}"
            );
        }
    }

    #[test]
    fn rename_rejects_stateful_tex_even_when_the_citation_is_elsewhere() {
        for source in [
            "\\catcode`\\@=11 Text \\cite{old}",
            "\\let\\savedcite\\cite Text \\cite{old}",
            "\\csname cite\\endcsname{old}",
            "\\ExplSyntaxOn Text \\cite{old}",
        ] {
            let result = plan_citation_key_rename(
                file_id(1),
                "@misc{old, title={T}}",
                &[CitationSourceFile {
                    file_id: file_id(2),
                    relative_path: "stateful.tex".to_owned(),
                    source: source.to_owned(),
                }],
                "old",
                "new",
            );
            assert!(
                matches!(result, Err(CitationError::UnsafeCitationSyntax { .. })),
                "stateful source was accepted: {source}"
            );
        }
    }

    #[test]
    fn unrelated_macro_definition_does_not_hide_a_safe_citation() {
        let source = "\\newcommand{\\projectname}{Setwright}\nText \\cite{old}.";
        let plan = plan_citation_key_rename(
            file_id(1),
            "@misc{old, title={T}}",
            &[CitationSourceFile {
                file_id: file_id(2),
                relative_path: "main.tex".to_owned(),
                source: source.to_owned(),
            }],
            "old",
            "new",
        )
        .unwrap();
        assert_eq!(plan.citation_occurrences, 1);
    }

    #[test]
    fn rename_rejects_bibliography_with_unparsed_collision_space() {
        let result = plan_citation_key_rename(
            file_id(1),
            "unparsed source\n@misc{old, title={T}}",
            &[],
            "old",
            "new",
        );
        assert!(matches!(result, Err(CitationError::InvalidEdit { .. })));
    }

    #[test]
    fn upsert_preserves_every_byte_outside_target_entry() {
        let source = "preamble junk\n@misc{key, title = \"Old\"} trailing junk\r\n";
        let document = parse_bibliography(source).unwrap();
        let target = &document.entries[0].span;
        let before = &source[..target.start_byte];
        let after = &source[target.end_byte..];
        let draft = BibEntryDraft {
            entry_type: "misc".to_owned(),
            key: "key".to_owned(),
            fields: BTreeMap::from([("title".to_owned(), "New".to_owned())]),
        };
        let plan = plan_upsert_entry(file_id(1), source, &draft).unwrap();
        let updated = apply(source, file_id(1), &plan.edits);
        assert!(updated.starts_with(before));
        assert!(updated.ends_with(after));
    }

    #[test]
    fn endpoint_allowlist_rejects_http_credentials_ports_and_wrong_hosts() {
        let request = MetadataLookupRequest {
            provider: MetadataProvider::Crossref,
            query: "graph neural networks".to_owned(),
            max_results: 5,
            trigger: LookupTrigger::ExplicitUserAction,
        };
        let allowed = build_lookup_url(&request).unwrap();
        assert!(validate_lookup_url(MetadataProvider::Crossref, &allowed).is_ok());
        for rejected in [
            "http://api.crossref.org/works",
            "https://api.crossref.org:444/works",
            "https://user@api.crossref.org/works",
            "https://api.crossref.org.evil.example/works",
            "https://api.crossref.org/other",
        ] {
            assert!(
                validate_lookup_url(MetadataProvider::Crossref, &Url::parse(rejected).unwrap())
                    .is_err(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn redirect_policy_is_fail_closed_for_same_and_cross_host_targets() {
        for target in [
            "https://api.crossref.org/works?rows=1",
            "https://evil.example/steal",
        ] {
            assert!(!redirect_target_is_allowed(
                MetadataProvider::Crossref,
                &Url::parse(target).unwrap()
            ));
        }
    }

    #[test]
    fn query_and_result_limits_are_enforced_before_url_construction() {
        let base = MetadataLookupRequest {
            provider: MetadataProvider::Arxiv,
            query: "transformers".to_owned(),
            max_results: 1,
            trigger: LookupTrigger::ExplicitUserAction,
        };
        assert!(validate_lookup_request(&base).is_ok());
        assert!(
            validate_lookup_request(&MetadataLookupRequest {
                query: "x".repeat(MAX_QUERY_CHARS + 1),
                ..base.clone()
            })
            .is_err()
        );
        assert!(
            validate_lookup_request(&MetadataLookupRequest {
                max_results: MAX_RESULTS + 1,
                ..base
            })
            .is_err()
        );
    }

    #[test]
    fn response_chunks_cannot_cross_the_byte_limit() {
        let mut body = b"1234".to_vec();
        extend_bounded(&mut body, b"56", 6).unwrap();
        assert_eq!(body, b"123456");
        assert!(matches!(
            extend_bounded(&mut body, b"7", 6),
            Err(CitationError::ResponseTooLarge { limit_bytes: 6 })
        ));
        assert_eq!(body, b"123456");
    }

    #[test]
    fn mocked_crossref_json_normalizes_to_public_metadata() {
        let json = br#"{
          "message": {"items": [{
            "DOI": "10.1000/example",
            "title": ["A <i>Useful</i> Paper"],
            "author": [{"given": "Ada", "family": "Lovelace"}],
            "issued": {"date-parts": [[2025, 2, 1]]},
            "container-title": ["Journal of Tests"],
            "type": "journal-article",
            "URL": "https://doi.org/10.1000/example"
          }]}
        }"#;
        let results = parse_crossref_response(json, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "A Useful Paper");
        assert_eq!(results[0].issued_year, Some(2025));
        assert_eq!(results[0].authors[0].literal, "Ada Lovelace");
    }

    #[test]
    fn mocked_arxiv_atom_normalizes_without_live_network() {
        let xml = br#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <entry>
    <id>https://arxiv.org/abs/2501.01234v2</id>
    <published>2025-01-02T00:00:00Z</published>
    <title>  Reliable &amp; Local   Editing </title>
    <summary> A test abstract. </summary>
    <author><name>Ada Lovelace</name></author>
    <author><name>Alan Turing</name></author>
    <arxiv:doi>10.1000/example</arxiv:doi>
  </entry>
</feed>"#;
        let results = parse_arxiv_response(xml, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Reliable & Local Editing");
        assert_eq!(results[0].issued_year, Some(2025));
        assert_eq!(results[0].authors.len(), 2);
        assert_eq!(results[0].arxiv_id.as_deref(), Some("2501.01234v2"));
    }

    #[test]
    fn stale_patch_hash_is_rejected_by_canonical_source_buffer() {
        let source = "@misc{old, title={T}}";
        let plan = plan_citation_key_rename(file_id(1), source, &[], "old", "new").unwrap();
        let mut buffer = SourceBuffer::from_bytes(
            file_id(1),
            "refs.bib",
            source.replace("old", "changed").into_bytes(),
            Revision::INITIAL,
        );
        assert!(
            buffer
                .apply_edits(&plan.edits_by_file[&file_id(1)], Revision(1))
                .is_err()
        );
    }

    #[test]
    fn http_status_type_is_bounded() {
        let status = StatusCode::TOO_MANY_REQUESTS;
        assert_eq!(status.as_u16(), 429);
    }
}
