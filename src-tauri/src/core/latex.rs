use crate::core::contracts::{FileId, PaperSettingsV1, SourceSpan, VisualNodeKind};
use crate::core::error::{AppError, AppResult};
use crate::core::source::hash_bytes;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use tree_sitter::{InputEdit, Node, Parser, Point, Tree};
use walkdir::WalkDir;

const MAX_INCLUDE_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_INCLUDE_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_INCLUDE_FILES: usize = 1024;
const MAX_INCLUDE_DEPTH: usize = 64;
const MAX_BIBLIOGRAPHY_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BIBLIOGRAPHY_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BIBLIOGRAPHY_FILES: usize = 1024;
const MAX_BIBLIOGRAPHY_WALK_ENTRIES: usize = 100_000;
const MAX_BIBLIOGRAPHY_DEPTH: usize = 64;

pub struct LatexParser {
    parser: Parser,
    trees: HashMap<FileId, Tree>,
}

impl std::fmt::Debug for LatexParser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LatexParser")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LatexAnalysis {
    pub file_id: FileId,
    pub byte_len: usize,
    pub source_hash: String,
    pub has_parse_errors: bool,
    pub includes: Vec<IncludeDirective>,
    pub projection: Vec<ProjectionSegment>,
    pub compatibility: CompatibilityReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IncludeDirective {
    pub span: SourceSpan,
    pub command: String,
    pub raw_path: String,
    pub static_path: Option<String>,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionSegment {
    pub span: SourceSpan,
    pub projection_kind: ProjectionKind,
    pub source_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProjectionKind {
    Supported { node_kind: VisualNodeKind },
    IncludeBoundary { path: String },
    RawInline { reason: String },
    RawBlock { reason: String },
    Trivia,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityReport {
    pub fully_visual: bool,
    pub has_parse_errors: bool,
    pub supported_segments: usize,
    pub include_boundaries: usize,
    pub raw_inline_segments: usize,
    pub raw_block_segments: usize,
    pub raw_reasons: BTreeMap<String, usize>,
}

impl LatexParser {
    pub fn new() -> AppResult<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_latex::LANGUAGE.into())
            .map_err(|error| AppError::Parse {
                message: format!("could not load LaTeX grammar: {error}"),
            })?;
        Ok(Self {
            parser,
            trees: HashMap::new(),
        })
    }

    pub fn parse(&mut self, file_id: FileId, source: &[u8]) -> AppResult<LatexAnalysis> {
        let text = std::str::from_utf8(source).map_err(|_| AppError::InvalidUtf8 {
            path: file_id.to_string(),
        })?;
        let tree = self
            .parser
            .parse(source, None)
            .ok_or_else(|| AppError::Parse {
                message: "Tree-sitter cancelled parsing".into(),
            })?;
        let analysis = analyze_tree(file_id, source, text, &tree);
        self.trees.insert(file_id, tree);
        Ok(analysis)
    }

    /// Incrementally parses a candidate without changing the cached canonical
    /// tree. The caller commits the returned candidate only after every file in
    /// a multi-file edit has validated successfully.
    pub fn parse_candidate(
        &mut self,
        file_id: FileId,
        old_source: &[u8],
        new_source: &[u8],
    ) -> AppResult<LatexParseCandidate> {
        let text = std::str::from_utf8(new_source).map_err(|_| AppError::InvalidUtf8 {
            path: file_id.to_string(),
        })?;
        let mut old_tree = self.trees.get(&file_id).cloned();
        if let Some(tree) = &mut old_tree {
            tree.edit(&single_change_edit(old_source, new_source)?);
        }
        let tree = self
            .parser
            .parse(new_source, old_tree.as_ref())
            .ok_or_else(|| AppError::Parse {
                message: "Tree-sitter cancelled parsing".into(),
            })?;
        let analysis = analyze_tree(file_id, new_source, text, &tree);
        Ok(LatexParseCandidate { analysis, tree })
    }

    pub fn commit_candidate(
        &mut self,
        file_id: FileId,
        candidate: LatexParseCandidate,
    ) -> LatexAnalysis {
        self.trees.insert(file_id, candidate.tree);
        candidate.analysis
    }
}

pub struct LatexParseCandidate {
    analysis: LatexAnalysis,
    tree: Tree,
}

impl LatexParseCandidate {
    #[must_use]
    pub fn analysis(&self) -> &LatexAnalysis {
        &self.analysis
    }
}

fn analyze_tree(file_id: FileId, source: &[u8], text: &str, tree: &Tree) -> LatexAnalysis {
    let has_parse_errors = tree.root_node().has_error();
    let includes = collect_includes(file_id, text, tree);
    let projection = project_tree(file_id, source, tree);
    debug_assert!(projection_covers_source(source.len(), &projection));
    let compatibility = CompatibilityReport::from_projection(has_parse_errors, &projection);
    LatexAnalysis {
        file_id,
        byte_len: source.len(),
        source_hash: hash_bytes(source),
        has_parse_errors,
        includes,
        projection,
        compatibility,
    }
}

fn single_change_edit(old_source: &[u8], new_source: &[u8]) -> AppResult<InputEdit> {
    let old_text = std::str::from_utf8(old_source).map_err(|_| AppError::InvalidUtf8 {
        path: "incremental LaTeX source".into(),
    })?;
    let new_text = std::str::from_utf8(new_source).map_err(|_| AppError::InvalidUtf8 {
        path: "incremental LaTeX source".into(),
    })?;
    let shared_limit = old_source.len().min(new_source.len());
    let mut start = old_source
        .iter()
        .zip(new_source)
        .take_while(|(left, right)| left == right)
        .count();
    while start > 0 && (!old_text.is_char_boundary(start) || !new_text.is_char_boundary(start)) {
        start -= 1;
    }
    let mut suffix = old_source[start..]
        .iter()
        .rev()
        .zip(new_source[start..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count()
        .min(shared_limit.saturating_sub(start));
    while suffix > 0
        && (!old_text.is_char_boundary(old_source.len() - suffix)
            || !new_text.is_char_boundary(new_source.len() - suffix))
    {
        suffix -= 1;
    }
    let old_end_byte = old_source.len() - suffix;
    let new_end_byte = new_source.len() - suffix;
    Ok(InputEdit {
        start_byte: start,
        old_end_byte,
        new_end_byte,
        start_position: byte_point(old_source, start),
        old_end_position: byte_point(old_source, old_end_byte),
        new_end_position: byte_point(new_source, new_end_byte),
    })
}

fn byte_point(source: &[u8], offset: usize) -> Point {
    let before = &source[..offset];
    let row = before.iter().filter(|byte| **byte == b'\n').count();
    let column = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(before.len(), |newline| before.len() - newline - 1);
    Point::new(row, column)
}

impl Default for LatexParser {
    fn default() -> Self {
        Self::new().expect("the pinned LaTeX grammar must be loadable")
    }
}

impl CompatibilityReport {
    fn from_projection(has_parse_errors: bool, projection: &[ProjectionSegment]) -> Self {
        let mut report = Self {
            fully_visual: !has_parse_errors,
            has_parse_errors,
            ..Self::default()
        };
        for segment in projection {
            match &segment.projection_kind {
                ProjectionKind::Supported { .. } => report.supported_segments += 1,
                ProjectionKind::IncludeBoundary { .. } => report.include_boundaries += 1,
                ProjectionKind::RawInline { reason } => {
                    report.raw_inline_segments += 1;
                    *report.raw_reasons.entry(reason.clone()).or_default() += 1;
                }
                ProjectionKind::RawBlock { reason } => {
                    report.raw_block_segments += 1;
                    *report.raw_reasons.entry(reason.clone()).or_default() += 1;
                }
                ProjectionKind::Trivia => {}
            }
        }
        report.fully_visual &= report.raw_inline_segments == 0 && report.raw_block_segments == 0;
        report
    }
}

fn collect_includes(file_id: FileId, source: &str, tree: &Tree) -> Vec<IncludeDirective> {
    let mut directives = Vec::new();
    let mut cursor = tree.walk();
    walk_nodes(tree.root_node(), &mut cursor, &mut |node| {
        if node.kind() != "latex_include" {
            return;
        }
        let command = node
            .child_by_field_name("command")
            .and_then(|child| child.utf8_text(source.as_bytes()).ok())
            .unwrap_or("\\input")
            .trim_start_matches('\\')
            .to_string();
        let raw_group = node
            .child_by_field_name("path")
            .and_then(|child| child.utf8_text(source.as_bytes()).ok())
            .unwrap_or_default();
        let raw_path = strip_outer_group(raw_group).trim().to_string();
        let (static_path, rejection_reason) = validate_static_include_path(&raw_path);
        directives.push(IncludeDirective {
            span: SourceSpan::new(file_id, node.start_byte(), node.end_byte()),
            command,
            raw_path,
            static_path: static_path.map(|path| path.to_string_lossy().replace('\\', "/")),
            rejection_reason,
        });
    });
    directives.sort_by_key(|directive| directive.span.start_byte);
    directives
}

fn project_tree(file_id: FileId, source: &[u8], tree: &Tree) -> Vec<ProjectionSegment> {
    let mut projected = Vec::new();
    project_node(tree.root_node(), file_id, source, &mut projected);
    projected.sort_by_key(|segment| (segment.span.start_byte, segment.span.end_byte));
    coalesce_projection(projected, source)
}

fn project_node(
    node: Node<'_>,
    file_id: FileId,
    source: &[u8],
    projected: &mut Vec<ProjectionSegment>,
) {
    if node.start_byte() == node.end_byte() {
        return;
    }
    if node.is_error() || node.is_missing() || node.kind() == "ERROR" {
        push_projection(
            projected,
            file_id,
            source,
            node.start_byte(),
            node.end_byte(),
            ProjectionKind::RawBlock {
                reason: "parser recovery region".into(),
            },
        );
        return;
    }

    if let Some(projection_kind) = classify_atomic_node(node, source) {
        push_projection(
            projected,
            file_id,
            source,
            node.start_byte(),
            node.end_byte(),
            projection_kind,
        );
        return;
    }

    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    if children.is_empty() {
        push_projection(
            projected,
            file_id,
            source,
            node.start_byte(),
            node.end_byte(),
            ProjectionKind::Trivia,
        );
        return;
    }

    let mut offset = node.start_byte();
    for child in children {
        if child.start_byte() > offset {
            push_projection(
                projected,
                file_id,
                source,
                offset,
                child.start_byte(),
                ProjectionKind::Trivia,
            );
        }
        project_node(child, file_id, source, projected);
        offset = offset.max(child.end_byte());
    }
    if offset < node.end_byte() {
        push_projection(
            projected,
            file_id,
            source,
            offset,
            node.end_byte(),
            ProjectionKind::Trivia,
        );
    }
}

fn classify_atomic_node(node: Node<'_>, source: &[u8]) -> Option<ProjectionKind> {
    let kind = node.kind();
    let raw = node.utf8_text(source).unwrap_or_default();

    let raw_reason = match kind {
        "new_command_definition"
        | "old_command_definition"
        | "let_command_definition"
        | "environment_definition"
        | "theorem_definition"
        | "paired_delimiter_definition" => Some("macro or environment definition"),
        "tikz_environment" | "tikz_library_import" | "asy_environment" | "asydef_environment" => {
            Some("programmatic graphics")
        }
        "luacode_environment"
        | "pycode_environment"
        | "sageblock_environment"
        | "sagesilent_environment" => Some("executable TeX extension"),
        "minted_environment" => Some("minted requires an external cache"),
        _ if contains_tex_conditional(raw) => Some("stateful TeX conditional"),
        _ => None,
    };
    if let Some(reason) = raw_reason {
        return Some(ProjectionKind::RawBlock {
            reason: reason.into(),
        });
    }

    let supported = match kind {
        "latex_include" => {
            let raw_group = node
                .child_by_field_name("path")
                .and_then(|child| child.utf8_text(source).ok())
                .unwrap_or_default();
            let raw_path = strip_outer_group(raw_group).trim();
            let (static_path, rejection_reason) = validate_static_include_path(raw_path);
            return Some(match static_path {
                Some(path) => ProjectionKind::IncludeBoundary {
                    path: path.to_string_lossy().replace('\\', "/"),
                },
                None => ProjectionKind::RawBlock {
                    reason: rejection_reason.unwrap_or_else(|| "dynamic include".into()),
                },
            });
        }
        "title_declaration" | "author_declaration" | "paragraph" | "text_mode" => {
            if subtree_has_unsafe_construct(node, source) {
                return Some(ProjectionKind::RawBlock {
                    reason: "paragraph contains unsupported TeX".into(),
                });
            }
            Some(VisualNodeKind::Paragraph)
        }
        "part" | "chapter" | "section" | "subsection" | "subsubsection" | "subparagraph" => {
            Some(VisualNodeKind::Heading)
        }
        "citation" => Some(VisualNodeKind::Citation),
        "label_reference" | "label_reference_range" | "label_number" => {
            Some(VisualNodeKind::CrossReference)
        }
        "inline_formula" => {
            if safe_math(raw) {
                Some(VisualNodeKind::InlineEquation)
            } else {
                return Some(ProjectionKind::RawInline {
                    reason: "equation is outside the safe visual subset".into(),
                });
            }
        }
        "displayed_equation" | "math_environment" => {
            if safe_math(raw) {
                Some(VisualNodeKind::DisplayEquation)
            } else {
                return Some(ProjectionKind::RawBlock {
                    reason: "equation is outside the safe visual subset".into(),
                });
            }
        }
        "listing_environment" | "verbatim_environment" => Some(VisualNodeKind::CodeListing),
        "generic_environment" => return classify_generic_environment(node, source),
        _ => None,
    };
    supported.map(|node_kind| ProjectionKind::Supported { node_kind })
}

fn classify_generic_environment(node: Node<'_>, source: &[u8]) -> Option<ProjectionKind> {
    let raw = node.utf8_text(source).unwrap_or_default();
    let name = environment_name(raw).unwrap_or_default();
    let supported = match name {
        "document" => return None,
        "abstract" => VisualNodeKind::Paragraph,
        "quote" | "quotation" => VisualNodeKind::Quote,
        "itemize" | "enumerate" | "description" => VisualNodeKind::List,
        "theorem" | "lemma" | "proposition" | "corollary" => VisualNodeKind::Theorem,
        "definition" => VisualNodeKind::Definition,
        "proof" => VisualNodeKind::Proof,
        "figure" if simple_figure(raw) => VisualNodeKind::Figure,
        "figure" => {
            return Some(ProjectionKind::RawBlock {
                reason: "complex figure or subfigure".into(),
            });
        }
        "table" | "tabular" if simple_booktabs_table(raw) => VisualNodeKind::Table,
        "table" | "tabular" => {
            return Some(ProjectionKind::RawBlock {
                reason: "complex table".into(),
            });
        }
        "equation" | "equation*" | "align" | "align*" | "gather" | "gather*" | "multline"
        | "multline*"
            if safe_math(raw) =>
        {
            VisualNodeKind::DisplayEquation
        }
        "equation" | "equation*" | "align" | "align*" | "gather" | "gather*" | "multline"
        | "multline*" => {
            return Some(ProjectionKind::RawBlock {
                reason: "equation is outside the safe visual subset".into(),
            });
        }
        "lstlisting" => VisualNodeKind::CodeListing,
        _ => {
            return Some(ProjectionKind::RawBlock {
                reason: if name.is_empty() {
                    "unrecognized environment".into()
                } else {
                    format!("custom or unsupported environment: {name}")
                },
            });
        }
    };
    Some(ProjectionKind::Supported {
        node_kind: supported,
    })
}

fn subtree_has_unsafe_construct(node: Node<'_>, source: &[u8]) -> bool {
    let mut unsafe_found = false;
    let mut cursor = node.walk();
    walk_nodes(node, &mut cursor, &mut |child| {
        let kind = child.kind();
        let text = child.utf8_text(source).unwrap_or_default();
        unsafe_found |= matches!(
            kind,
            "new_command_definition"
                | "old_command_definition"
                | "let_command_definition"
                | "environment_definition"
                | "generic_environment"
                | "generic_command"
                | "tikz_environment"
                | "luacode_environment"
                | "pycode_environment"
        ) || contains_tex_conditional(text);
    });
    unsafe_found
}

fn contains_tex_conditional(source: &str) -> bool {
    const CONDITIONALS: [&str; 15] = [
        "\\if",
        "\\ifx",
        "\\ifnum",
        "\\ifdim",
        "\\ifodd",
        "\\ifcase",
        "\\ifdefined",
        "\\ifcsname",
        "\\iftrue",
        "\\iffalse",
        "\\unless",
        "\\else",
        "\\or",
        "\\fi",
        "\\csname",
    ];
    CONDITIONALS.iter().any(|needle| {
        source.match_indices(needle).any(|(offset, _)| {
            source[offset + needle.len()..]
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphabetic())
        })
    })
}

fn safe_math(source: &str) -> bool {
    const FORBIDDEN: [&str; 18] = [
        "\\def",
        "\\gdef",
        "\\edef",
        "\\xdef",
        "\\newcommand",
        "\\renewcommand",
        "\\providecommand",
        "\\catcode",
        "\\input",
        "\\include",
        "\\write",
        "\\openout",
        "\\read",
        "\\csname",
        "\\begin{tikz",
        "\\directlua",
        "\\special",
        "\\usepackage",
    ];
    !FORBIDDEN.iter().any(|needle| source.contains(needle))
        && !contains_tex_conditional(source)
        && math_commands_are_safe(source)
}

fn math_commands_are_safe(source: &str) -> bool {
    const ALLOWED_COMMANDS: &[&str] = &[
        "alpha",
        "beta",
        "gamma",
        "delta",
        "epsilon",
        "varepsilon",
        "zeta",
        "eta",
        "theta",
        "vartheta",
        "iota",
        "kappa",
        "lambda",
        "mu",
        "nu",
        "xi",
        "pi",
        "varpi",
        "rho",
        "varrho",
        "sigma",
        "varsigma",
        "tau",
        "upsilon",
        "phi",
        "varphi",
        "chi",
        "psi",
        "omega",
        "Gamma",
        "Delta",
        "Theta",
        "Lambda",
        "Xi",
        "Pi",
        "Sigma",
        "Upsilon",
        "Phi",
        "Psi",
        "Omega",
        "frac",
        "dfrac",
        "tfrac",
        "sqrt",
        "binom",
        "sum",
        "prod",
        "coprod",
        "int",
        "iint",
        "iiint",
        "oint",
        "lim",
        "limsup",
        "liminf",
        "min",
        "max",
        "inf",
        "sup",
        "sin",
        "cos",
        "tan",
        "cot",
        "sec",
        "csc",
        "arcsin",
        "arccos",
        "arctan",
        "sinh",
        "cosh",
        "tanh",
        "log",
        "ln",
        "exp",
        "det",
        "gcd",
        "Pr",
        "left",
        "right",
        "middle",
        "big",
        "Big",
        "bigg",
        "Bigg",
        "bigl",
        "bigr",
        "Bigl",
        "Bigr",
        "biggl",
        "biggr",
        "Biggl",
        "Biggr",
        "text",
        "textrm",
        "textit",
        "textbf",
        "mathrm",
        "mathbf",
        "mathit",
        "mathsf",
        "mathtt",
        "mathcal",
        "mathbb",
        "mathfrak",
        "boldsymbol",
        "operatorname",
        "overline",
        "underline",
        "hat",
        "widehat",
        "bar",
        "vec",
        "dot",
        "ddot",
        "tilde",
        "widetilde",
        "overbrace",
        "underbrace",
        "overset",
        "underset",
        "substack",
        "phantom",
        "hphantom",
        "vphantom",
        "smash",
        "begin",
        "end",
        "label",
        "tag",
        "tag*",
        "nonumber",
        "notag",
        "ref",
        "eqref",
        "quad",
        "qquad",
        "enspace",
        "thinspace",
        "medspace",
        "thickspace",
        "in",
        "notin",
        "ni",
        "subset",
        "subseteq",
        "supset",
        "supseteq",
        "cup",
        "cap",
        "setminus",
        "emptyset",
        "varnothing",
        "forall",
        "exists",
        "nexists",
        "neg",
        "land",
        "lor",
        "top",
        "bot",
        "vdash",
        "dashv",
        "models",
        "le",
        "leq",
        "ge",
        "geq",
        "ne",
        "neq",
        "approx",
        "sim",
        "simeq",
        "cong",
        "equiv",
        "propto",
        "ll",
        "gg",
        "prec",
        "succ",
        "preceq",
        "succeq",
        "parallel",
        "perp",
        "pm",
        "mp",
        "times",
        "div",
        "cdot",
        "ast",
        "star",
        "circ",
        "bullet",
        "oplus",
        "ominus",
        "otimes",
        "oslash",
        "odot",
        "wedge",
        "vee",
        "bigcup",
        "bigcap",
        "bigvee",
        "bigwedge",
        "bigsqcup",
        "bigoplus",
        "bigotimes",
        "partial",
        "nabla",
        "infty",
        "ell",
        "hbar",
        "imath",
        "jmath",
        "Re",
        "Im",
        "aleph",
        "angle",
        "triangle",
        "square",
        "prime",
        "backslash",
        "ldots",
        "cdots",
        "vdots",
        "ddots",
        "dots",
        "rightarrow",
        "leftarrow",
        "leftrightarrow",
        "Rightarrow",
        "Leftarrow",
        "Leftrightarrow",
        "mapsto",
        "longrightarrow",
        "longleftarrow",
        "longleftrightarrow",
        "Longrightarrow",
        "Longleftarrow",
        "Longleftrightarrow",
        "uparrow",
        "downarrow",
        "updownarrow",
        "Uparrow",
        "Downarrow",
        "Updownarrow",
    ];
    const ALLOWED_ENVIRONMENTS: &[&str] = &[
        "equation",
        "equation*",
        "align",
        "align*",
        "aligned",
        "alignedat",
        "gather",
        "gather*",
        "gathered",
        "multline",
        "multline*",
        "split",
        "matrix",
        "pmatrix",
        "bmatrix",
        "Bmatrix",
        "vmatrix",
        "Vmatrix",
        "smallmatrix",
        "cases",
    ];

    for argument in latex_command_arguments(source, "begin")
        .into_iter()
        .chain(latex_command_arguments(source, "end"))
    {
        if !ALLOWED_ENVIRONMENTS.contains(&argument.trim()) {
            return false;
        }
    }

    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        index += 1;
        if index >= bytes.len() {
            break;
        }
        if !bytes[index].is_ascii_alphabetic() {
            // TeX control symbols such as `\\`, `\,`, `\{`, and `\|` are
            // single-character and safe in the visual math subset.
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
            index += 1;
        }
        if bytes.get(index) == Some(&b'*') {
            index += 1;
        }
        let command = &source[start..index];
        if !ALLOWED_COMMANDS.contains(&command) {
            return false;
        }
    }
    true
}

fn latex_command_arguments(source: &str, command: &str) -> Vec<String> {
    let needle = format!("\\{command}{{");
    let mut arguments = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = source[offset..].find(&needle) {
        let start = offset + relative + needle.len();
        let mut depth = 1usize;
        let mut end = start;
        for (relative_index, character) in source[start..].char_indices() {
            if character == '{' {
                depth += 1;
            } else if character == '}' {
                depth -= 1;
                if depth == 0 {
                    end = start + relative_index;
                    break;
                }
            }
        }
        if depth != 0 {
            break;
        }
        arguments.push(source[start..end].to_string());
        offset = end + 1;
    }
    arguments
}

fn simple_figure(source: &str) -> bool {
    let images = source.match_indices("\\includegraphics").count();
    images == 1
        && !source.contains("subfigure")
        && !source.contains("subcaption")
        && !source.contains("minipage")
        && !source.contains("\\input")
        && !source.contains("\\begin{tikz")
}

fn simple_booktabs_table(source: &str) -> bool {
    if source.contains("\\multicolumn")
        || source.contains("\\multirow")
        || source.contains("\\begin{tabularx}")
        || source.contains("\\begin{longtable}")
        || source.contains("\\begin{array}")
        || source.contains("\\input")
    {
        return false;
    }
    let Some(tabular) = source.find("\\begin{tabular}") else {
        return false;
    };
    let after = &source[tabular + "\\begin{tabular}".len()..];
    let Some(open) = after.find('{') else {
        return false;
    };
    let Some(close) = after[open + 1..].find('}') else {
        return false;
    };
    let columns = &after[open + 1..open + 1 + close];
    !columns.is_empty()
        && columns
            .chars()
            .all(|character| matches!(character, 'l' | 'c' | 'r' | ' '))
}

fn environment_name(source: &str) -> Option<&str> {
    let start = source.find("\\begin{")? + "\\begin{".len();
    let end = source[start..].find('}')? + start;
    Some(source[start..end].trim())
}

fn push_projection(
    projected: &mut Vec<ProjectionSegment>,
    file_id: FileId,
    source: &[u8],
    start_byte: usize,
    end_byte: usize,
    projection_kind: ProjectionKind,
) {
    if start_byte >= end_byte || end_byte > source.len() {
        return;
    }
    projected.push(ProjectionSegment {
        span: SourceSpan::new(file_id, start_byte, end_byte),
        projection_kind,
        source_hash: hash_bytes(&source[start_byte..end_byte]),
    });
}

fn coalesce_projection(segments: Vec<ProjectionSegment>, source: &[u8]) -> Vec<ProjectionSegment> {
    let mut result: Vec<ProjectionSegment> = Vec::with_capacity(segments.len());
    for segment in segments {
        if let Some(previous) = result.last_mut()
            && previous.span.end_byte == segment.span.start_byte
            && previous.projection_kind == segment.projection_kind
        {
            previous.span.end_byte = segment.span.end_byte;
            previous.source_hash =
                hash_bytes(&source[previous.span.start_byte..previous.span.end_byte]);
            continue;
        }
        result.push(segment);
    }
    result
}

#[must_use]
pub fn projection_covers_source(byte_len: usize, segments: &[ProjectionSegment]) -> bool {
    let mut expected = 0usize;
    for segment in segments {
        if segment.span.start_byte != expected || segment.span.end_byte < segment.span.start_byte {
            return false;
        }
        expected = segment.span.end_byte;
    }
    expected == byte_len
}

fn walk_nodes<'tree>(
    node: Node<'tree>,
    cursor: &mut tree_sitter::TreeCursor<'tree>,
    visit: &mut impl FnMut(Node<'tree>),
) {
    visit(node);
    if cursor.goto_first_child() {
        loop {
            walk_nodes(cursor.node(), cursor, visit);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

fn strip_outer_group(value: &str) -> &str {
    value
        .strip_prefix('{')
        .and_then(|inner| inner.strip_suffix('}'))
        .unwrap_or(value)
}

fn validate_static_include_path(raw_path: &str) -> (Option<PathBuf>, Option<String>) {
    if raw_path.is_empty() {
        return (None, Some("empty include path".into()));
    }
    if raw_path.contains(['\\', '#', '$', '~']) {
        return (
            None,
            Some("dynamic include path contains TeX expansion".into()),
        );
    }
    let path = Path::new(raw_path);
    if path.is_absolute() || raw_path.starts_with('/') || has_windows_prefix(raw_path) {
        return (None, Some("absolute include path".into()));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return (
            None,
            Some("include path leaves its containing directory".into()),
        );
    }
    let mut path = path.to_path_buf();
    if path.extension().is_none() {
        path.set_extension("tex");
    }
    (Some(path), None)
}

fn has_windows_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[1] == b':') || path.starts_with("//") || path.starts_with("\\\\")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLayout {
    pub root: PathBuf,
    pub main_file: PathBuf,
    pub created_project: bool,
    pub settings: Option<PaperSettingsV1>,
}

impl ProjectLayout {
    pub fn discover(path: impl AsRef<Path>) -> AppResult<Self> {
        let requested = path.as_ref();
        let metadata = std::fs::metadata(requested)
            .map_err(|error| AppError::io("inspect", requested, error))?;
        if metadata.is_file() {
            let main_file = requested
                .canonicalize()
                .map_err(|error| AppError::io("canonicalize", requested, error))?;
            let root = find_matching_settings_root(&main_file)
                .unwrap_or_else(|| main_file.parent().unwrap_or(Path::new(".")).to_path_buf());
            let settings = read_settings_if_present(&root)?;
            return Ok(Self {
                root,
                main_file,
                created_project: settings.is_some(),
                settings,
            });
        }
        if !metadata.is_dir() {
            return Err(AppError::InvalidProject {
                message: format!("{} is neither a file nor directory", requested.display()),
            });
        }
        let root = requested
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", requested, error))?;
        let settings = read_settings_if_present(&root)?;
        let created_project = settings.is_some();
        let main_relative = if let Some(settings) = &settings {
            safe_relative_path(&settings.main_file)?
        } else if root.join("main.tex").is_file() {
            PathBuf::from("main.tex")
        } else {
            let mut candidates = std::fs::read_dir(&root)
                .map_err(|error| AppError::io("read directory", &root, error))?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|entry| {
                    entry.is_file()
                        && entry
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
                })
                .collect::<Vec<_>>();
            candidates.sort();
            if candidates.len() != 1 {
                return Err(AppError::InvalidProject {
                    message: "select a main .tex file (directory has no unambiguous main.tex)"
                        .into(),
                });
            }
            candidates[0]
                .strip_prefix(&root)
                .expect("candidate is under root")
                .to_path_buf()
        };
        let main_candidate = root.join(&main_relative);
        if !main_candidate.is_file() {
            return Err(AppError::FileNotFound {
                path: main_candidate.to_string_lossy().into_owned(),
            });
        }
        let main_file = main_candidate
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", &main_candidate, error))?;
        ensure_within(&root, &main_file)?;
        Ok(Self {
            root,
            main_file,
            created_project,
            settings,
        })
    }

    pub fn main_relative(&self) -> AppResult<PathBuf> {
        self.main_file
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| AppError::PathOutsideRoot {
                path: self.main_file.to_string_lossy().into_owned(),
            })
    }
}

fn find_matching_settings_root(main_file: &Path) -> Option<PathBuf> {
    for ancestor in main_file.parent()?.ancestors().take(8) {
        let settings_path = ancestor.join("paper-settings.json");
        let Ok(bytes) = std::fs::read(&settings_path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_slice::<PaperSettingsV1>(&bytes) else {
            continue;
        };
        let Ok(relative) = safe_relative_path(&settings.main_file) else {
            continue;
        };
        let Ok(candidate) = ancestor.join(relative).canonicalize() else {
            continue;
        };
        if candidate == main_file {
            return ancestor.canonicalize().ok();
        }
    }
    None
}

fn read_settings_if_present(root: &Path) -> AppResult<Option<PaperSettingsV1>> {
    let path = root.join("paper-settings.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|error| AppError::io("read", &path, error))?;
    let settings: PaperSettingsV1 =
        serde_json::from_slice(&bytes).map_err(AppError::serialization)?;
    if !settings.is_valid() {
        return Err(AppError::InvalidProject {
            message: "paper-settings.json violates the canonical V1 contract".into(),
        });
    }
    Ok(Some(settings))
}

pub fn safe_relative_path(path: &str) -> AppResult<PathBuf> {
    if path.is_empty() || has_windows_prefix(path) {
        return Err(AppError::InvalidPath {
            path: path.into(),
            message: "path must be a non-empty project-relative path".into(),
        });
    }
    let path_buf = PathBuf::from(path);
    if path_buf.is_absolute()
        || path_buf.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::PathOutsideRoot { path: path.into() });
    }
    Ok(path_buf)
}

pub fn ensure_within(root: &Path, candidate: &Path) -> AppResult<()> {
    if candidate.starts_with(root) {
        Ok(())
    } else {
        Err(AppError::PathOutsideRoot {
            path: candidate.to_string_lossy().into_owned(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IncludeGraph {
    pub root_file: String,
    pub nodes: BTreeMap<String, IncludeNode>,
    pub edges: Vec<IncludeEdge>,
    pub issues: Vec<IncludeIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IncludeNode {
    pub relative_path: String,
    pub content_hash: String,
    pub source_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IncludeEdge {
    pub from: String,
    pub to: Option<String>,
    pub directive: IncludeDirective,
    pub visually_flattened: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IncludeIssue {
    pub code: String,
    pub message: String,
    pub from: String,
    pub target: Option<String>,
}

pub fn build_include_graph(
    root: &Path,
    main_relative: &Path,
    parser: &mut LatexParser,
) -> AppResult<IncludeGraph> {
    build_include_graph_with_overlays(root, main_relative, parser, &BTreeMap::new())
}

/// Builds the include graph against candidate source buffers while retaining
/// the same bounded, fail-closed traversal for files that are not loaded in a
/// project session. This lets save validation see unsaved include edits before
/// any authoritative project file is replaced on disk.
pub(crate) fn build_include_graph_with_overlays(
    root: &Path,
    main_relative: &Path,
    parser: &mut LatexParser,
    overlays: &BTreeMap<String, Vec<u8>>,
) -> AppResult<IncludeGraph> {
    let root = root
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", root, error))?;
    let main_relative = safe_relative_path(&main_relative.to_string_lossy())?;
    let root_file = normalized_relative(&main_relative);
    let mut graph = IncludeGraph {
        root_file: root_file.clone(),
        nodes: BTreeMap::new(),
        edges: Vec::new(),
        issues: Vec::new(),
    };
    let mut state = IncludeWalkState::default();
    visit_include_file(
        &root,
        &main_relative,
        parser,
        &mut graph,
        &mut state,
        overlays,
        0,
    )?;

    let mut incoming: HashMap<String, usize> = HashMap::new();
    for edge in &graph.edges {
        if let Some(target) = &edge.to {
            *incoming.entry(target.clone()).or_default() += 1;
        }
    }
    for edge in &mut graph.edges {
        edge.visually_flattened &= edge
            .to
            .as_ref()
            .and_then(|target| incoming.get(target))
            .is_some_and(|count| *count == 1);
    }
    for (target, count) in incoming {
        if count > 1 {
            graph.issues.push(IncludeIssue {
                code: "repeatedInclude".into(),
                message: "repeated includes remain raw to avoid duplicated visual authority".into(),
                from: root_file.clone(),
                target: Some(target),
            });
        }
    }
    graph.edges.sort_by(|left, right| {
        (&left.from, left.directive.span.start_byte)
            .cmp(&(&right.from, right.directive.span.start_byte))
    });
    graph.issues.sort_by(|left, right| {
        (&left.from, &left.code, &left.target).cmp(&(&right.from, &right.code, &right.target))
    });
    Ok(graph)
}

/// Finds local bibliography databases without following symlinks. Bibliography
/// files are source-authoritative even when they are not yet referenced, so
/// local citation search works offline across the whole project.
pub fn discover_project_bibliographies(root: &Path) -> AppResult<Vec<PathBuf>> {
    let root = root
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", root, error))?;
    let mut bibliographies = Vec::new();
    let mut visited_entries = 0usize;
    let mut total_bytes = 0u64;
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !matches!(
                    entry.file_name().to_string_lossy().as_ref(),
                    ".git" | ".hg" | ".svn" | "node_modules" | "target"
                )
        })
    {
        let entry = entry.map_err(|error| AppError::InvalidProject {
            message: format!("bibliography discovery failed: {error}"),
        })?;
        visited_entries = visited_entries.saturating_add(1);
        if visited_entries > MAX_BIBLIOGRAPHY_WALK_ENTRIES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "bibliography discovery exceeds the {MAX_BIBLIOGRAPHY_WALK_ENTRIES}-entry safety limit"
                ),
            });
        }
        if entry.depth() > MAX_BIBLIOGRAPHY_DEPTH {
            return Err(AppError::InvalidProject {
                message: format!(
                    "bibliography discovery exceeds the {MAX_BIBLIOGRAPHY_DEPTH}-directory depth safety limit"
                ),
            });
        }
        if !entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bib"))
        {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::InvalidProject {
                message: format!(
                    "bibliography {} is a symbolic link; citation discovery fails closed",
                    entry.path().display()
                ),
            });
        }
        if !entry.file_type().is_file() {
            continue;
        }
        if bibliographies.len() >= MAX_BIBLIOGRAPHY_FILES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "project exceeds the {MAX_BIBLIOGRAPHY_FILES}-bibliography safety limit"
                ),
            });
        }
        let metadata = entry.metadata().map_err(|error| AppError::InvalidProject {
            message: format!(
                "could not inspect bibliography {}: {error}",
                entry.path().display()
            ),
        })?;
        if metadata.len() > MAX_BIBLIOGRAPHY_FILE_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "{} exceeds the {} MiB bibliography-file safety limit",
                    entry.path().display(),
                    MAX_BIBLIOGRAPHY_FILE_BYTES / (1024 * 1024)
                ),
            });
        }
        total_bytes =
            total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| AppError::InvalidProject {
                    message: "bibliography byte count overflowed".into(),
                })?;
        if total_bytes > MAX_BIBLIOGRAPHY_TOTAL_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "project bibliographies exceed the {} MiB aggregate safety limit",
                    MAX_BIBLIOGRAPHY_TOTAL_BYTES / (1024 * 1024)
                ),
            });
        }
        let canonical = entry
            .path()
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", entry.path(), error))?;
        ensure_within(&root, &canonical)?;
        bibliographies.push(
            canonical
                .strip_prefix(&root)
                .expect("checked root prefix")
                .to_path_buf(),
        );
    }
    bibliographies.sort();
    bibliographies.dedup();
    Ok(bibliographies)
}

#[derive(Default)]
struct IncludeWalkState {
    visited: HashSet<PathBuf>,
    active: BTreeSet<PathBuf>,
    total_bytes: u64,
}

impl IncludeWalkState {
    fn reserve_file(&mut self, relative: &Path, announced_size: u64) -> AppResult<()> {
        if self.visited.len() >= MAX_INCLUDE_FILES {
            return Err(AppError::InvalidProject {
                message: format!("include graph exceeds the {MAX_INCLUDE_FILES}-file safety limit"),
            });
        }
        if announced_size > MAX_INCLUDE_FILE_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "{} exceeds the {} MiB include-file safety limit",
                    relative.display(),
                    MAX_INCLUDE_FILE_BYTES / (1024 * 1024)
                ),
            });
        }
        let total = self
            .total_bytes
            .checked_add(announced_size)
            .ok_or_else(|| AppError::InvalidProject {
                message: "include graph byte count overflowed".into(),
            })?;
        if total > MAX_INCLUDE_TOTAL_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "include graph exceeds the {} MiB aggregate safety limit",
                    MAX_INCLUDE_TOTAL_BYTES / (1024 * 1024)
                ),
            });
        }
        self.total_bytes = total;
        Ok(())
    }

    fn adjust_file_size(
        &mut self,
        relative: &Path,
        announced_size: u64,
        actual_size: u64,
    ) -> AppResult<()> {
        if actual_size > MAX_INCLUDE_FILE_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "{} grew beyond the {} MiB include-file safety limit while it was read",
                    relative.display(),
                    MAX_INCLUDE_FILE_BYTES / (1024 * 1024)
                ),
            });
        }
        let total = self
            .total_bytes
            .checked_sub(announced_size)
            .and_then(|value| value.checked_add(actual_size))
            .ok_or_else(|| AppError::InvalidProject {
                message: "include graph byte count became inconsistent".into(),
            })?;
        if total > MAX_INCLUDE_TOTAL_BYTES {
            return Err(AppError::InvalidProject {
                message: format!(
                    "include graph exceeds the {} MiB aggregate safety limit",
                    MAX_INCLUDE_TOTAL_BYTES / (1024 * 1024)
                ),
            });
        }
        self.total_bytes = total;
        Ok(())
    }
}

fn visit_include_file(
    root: &Path,
    relative: &Path,
    parser: &mut LatexParser,
    graph: &mut IncludeGraph,
    state: &mut IncludeWalkState,
    overlays: &BTreeMap<String, Vec<u8>>,
    depth: usize,
) -> AppResult<()> {
    if state.visited.contains(relative) {
        return Ok(());
    }
    if depth > MAX_INCLUDE_DEPTH {
        return Err(AppError::InvalidProject {
            message: format!(
                "include graph exceeds the {MAX_INCLUDE_DEPTH}-level depth safety limit at {}",
                relative.display()
            ),
        });
    }
    let normalized = normalized_relative(relative);
    let candidate = root.join(relative);
    let canonical = candidate
        .canonicalize()
        .map_err(|error| AppError::io("canonicalize", &candidate, error))?;
    ensure_within(root, &canonical)?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| AppError::io("inspect include", &canonical, error))?;
    if !metadata.is_file() {
        return Err(AppError::InvalidProject {
            message: format!(
                "include target {} is not an ordinary file",
                relative.display()
            ),
        });
    }
    let canonical_relative = canonical
        .strip_prefix(root)
        .expect("include was checked to be inside the project root");
    let canonical_normalized = normalized_relative(canonical_relative);
    let bytes = if let Some(candidate) = overlays
        .get(&normalized)
        .or_else(|| overlays.get(&canonical_normalized))
    {
        state.reserve_file(relative, candidate.len() as u64)?;
        Cow::Borrowed(candidate.as_slice())
    } else {
        state.reserve_file(relative, metadata.len())?;
        let file = std::fs::File::open(&canonical)
            .map_err(|error| AppError::io("open include", &canonical, error))?;
        let mut bytes = Vec::new();
        file.take(MAX_INCLUDE_FILE_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| AppError::io("read include", &canonical, error))?;
        state.adjust_file_size(relative, metadata.len(), bytes.len() as u64)?;
        Cow::Owned(bytes)
    };
    state.visited.insert(relative.to_path_buf());
    state.active.insert(relative.to_path_buf());
    let source_only = std::str::from_utf8(bytes.as_ref()).is_err();
    graph.nodes.insert(
        normalized.clone(),
        IncludeNode {
            relative_path: normalized.clone(),
            content_hash: hash_bytes(bytes.as_ref()),
            source_only,
        },
    );
    if source_only {
        graph.issues.push(IncludeIssue {
            code: "nonUtf8Include".into(),
            message: "non-UTF-8 include is available in source mode only".into(),
            from: normalized,
            target: None,
        });
        state.active.remove(relative);
        return Ok(());
    }

    let analysis = parser.parse(FileId::new(), bytes.as_ref())?;
    for directive in analysis.includes {
        let Some(static_path) = directive.static_path.as_deref() else {
            graph.edges.push(IncludeEdge {
                from: normalized.clone(),
                to: None,
                directive: directive.clone(),
                visually_flattened: false,
            });
            graph.issues.push(IncludeIssue {
                code: "dynamicInclude".into(),
                message: directive
                    .rejection_reason
                    .clone()
                    .unwrap_or_else(|| "dynamic include remains raw".into()),
                from: normalized.clone(),
                target: Some(directive.raw_path.clone()),
            });
            continue;
        };
        let containing = relative.parent().unwrap_or(Path::new(""));
        let target_relative = containing.join(static_path);
        let target_normalized = normalized_relative(&target_relative);
        let target_candidate = root.join(&target_relative);
        if !target_candidate.exists() {
            graph.edges.push(IncludeEdge {
                from: normalized.clone(),
                to: Some(target_normalized.clone()),
                directive,
                visually_flattened: false,
            });
            graph.issues.push(IncludeIssue {
                code: "missingInclude".into(),
                message: "included file does not exist".into(),
                from: normalized.clone(),
                target: Some(target_normalized),
            });
            continue;
        }
        let target_canonical = target_candidate
            .canonicalize()
            .map_err(|error| AppError::io("canonicalize", &target_candidate, error))?;
        if !target_canonical.starts_with(root) {
            graph.edges.push(IncludeEdge {
                from: normalized.clone(),
                to: None,
                directive,
                visually_flattened: false,
            });
            graph.issues.push(IncludeIssue {
                code: "outsideRootInclude".into(),
                message: "include resolves outside the project root".into(),
                from: normalized.clone(),
                target: Some(target_candidate.to_string_lossy().into_owned()),
            });
            continue;
        }
        let canonical_relative = target_canonical
            .strip_prefix(root)
            .expect("checked root prefix")
            .to_path_buf();
        let canonical_normalized = normalized_relative(&canonical_relative);
        let cycle = state.active.contains(&canonical_relative);
        graph.edges.push(IncludeEdge {
            from: normalized.clone(),
            to: Some(canonical_normalized.clone()),
            directive,
            visually_flattened: !cycle,
        });
        if cycle {
            graph.issues.push(IncludeIssue {
                code: "cyclicInclude".into(),
                message: "cyclic include remains raw".into(),
                from: normalized.clone(),
                target: Some(canonical_normalized),
            });
        } else {
            visit_include_file(
                root,
                &canonical_relative,
                parser,
                graph,
                state,
                overlays,
                depth.saturating_add(1),
            )?;
        }
    }
    state.active.remove(relative);
    Ok(())
}

#[must_use]
pub fn normalized_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            Component::CurDir => None,
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_owns_every_byte_and_marks_macro_raw() {
        let mut parser = LatexParser::new().unwrap();
        let file_id = FileId::new();
        let source = br#"\documentclass{article}
\newcommand{\danger}[1]{#1}
\begin{document}
\section{Intro}
Hello world.
\end{document}
"#;
        let analysis = parser.parse(file_id, source).unwrap();
        assert!(projection_covers_source(source.len(), &analysis.projection));
        assert!(analysis.projection.iter().any(|segment| {
            matches!(
                &segment.projection_kind,
                ProjectionKind::RawBlock { reason } if reason.contains("macro")
            )
        }));
    }

    #[test]
    fn finds_only_static_includes() {
        let mut parser = LatexParser::new().unwrap();
        let source = br#"\input{sections/intro}
\input{../secret}
\input{\jobname}
% \input{commented}
"#;
        let analysis = parser.parse(FileId::new(), source).unwrap();
        assert_eq!(analysis.includes.len(), 3);
        assert_eq!(
            analysis.includes[0].static_path.as_deref(),
            Some("sections/intro.tex")
        );
        assert!(analysis.includes[1].static_path.is_none());
        assert!(analysis.includes[2].static_path.is_none());
        assert!(analysis.projection.iter().any(|segment| matches!(
            &segment.projection_kind,
            ProjectionKind::IncludeBoundary { path } if path == "sections/intro.tex"
        )));
        assert!(analysis.projection.iter().any(|segment| matches!(
            &segment.projection_kind,
            ProjectionKind::RawBlock { reason } if reason.contains("dynamic") || reason.contains("leaves")
        )));
    }

    #[test]
    fn include_graph_detects_repeat_and_cycle() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("main.tex"),
            "\\input{a}\n\\input{shared}\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("a.tex"),
            "\\input{main}\n\\input{shared}\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("shared.tex"), "Shared\n").unwrap();
        let mut parser = LatexParser::new().unwrap();
        let graph =
            build_include_graph(directory.path(), Path::new("main.tex"), &mut parser).unwrap();
        assert!(
            graph
                .issues
                .iter()
                .any(|issue| issue.code == "cyclicInclude")
        );
        assert!(
            graph
                .issues
                .iter()
                .any(|issue| issue.code == "repeatedInclude")
        );
        assert!(
            graph
                .edges
                .iter()
                .filter(|edge| edge.to.as_deref() == Some("shared.tex"))
                .all(|edge| !edge.visually_flattened)
        );
    }

    #[test]
    fn include_graph_rejects_oversized_main_and_include_before_reading() {
        let main_directory = tempfile::tempdir().unwrap();
        std::fs::File::create(main_directory.path().join("main.tex"))
            .unwrap()
            .set_len(MAX_INCLUDE_FILE_BYTES.saturating_add(1))
            .unwrap();
        let mut parser = LatexParser::new().unwrap();
        assert!(matches!(
            build_include_graph(main_directory.path(), Path::new("main.tex"), &mut parser),
            Err(AppError::InvalidProject { .. })
        ));

        let include_directory = tempfile::tempdir().unwrap();
        std::fs::write(
            include_directory.path().join("main.tex"),
            "\\input{large}\n",
        )
        .unwrap();
        std::fs::File::create(include_directory.path().join("large.tex"))
            .unwrap()
            .set_len(MAX_INCLUDE_FILE_BYTES.saturating_add(1))
            .unwrap();
        let mut parser = LatexParser::new().unwrap();
        assert!(matches!(
            build_include_graph(include_directory.path(), Path::new("main.tex"), &mut parser),
            Err(AppError::InvalidProject { .. })
        ));
    }

    #[test]
    fn include_graph_rejects_excessive_fanout_and_depth() {
        let fanout = tempfile::tempdir().unwrap();
        let mut main = String::new();
        for index in 0..MAX_INCLUDE_FILES {
            main.push_str(&format!("\\input{{part-{index}}}\n"));
            std::fs::write(fanout.path().join(format!("part-{index}.tex")), "part\n").unwrap();
        }
        std::fs::write(fanout.path().join("main.tex"), main).unwrap();
        let mut parser = LatexParser::new().unwrap();
        assert!(matches!(
            build_include_graph(fanout.path(), Path::new("main.tex"), &mut parser),
            Err(AppError::InvalidProject { .. })
        ));

        let depth = tempfile::tempdir().unwrap();
        for index in 0..=MAX_INCLUDE_DEPTH {
            std::fs::write(
                depth.path().join(format!("level-{index}.tex")),
                format!("\\input{{level-{}}}\n", index + 1),
            )
            .unwrap();
        }
        std::fs::write(
            depth
                .path()
                .join(format!("level-{}.tex", MAX_INCLUDE_DEPTH + 1)),
            "end\n",
        )
        .unwrap();
        let mut parser = LatexParser::new().unwrap();
        assert!(matches!(
            build_include_graph(depth.path(), Path::new("level-0.tex"), &mut parser),
            Err(AppError::InvalidProject { .. })
        ));
    }

    #[test]
    fn include_graph_budget_rejects_aggregate_overflow() {
        let mut state = IncludeWalkState::default();
        state
            .reserve_file(Path::new("first.tex"), MAX_INCLUDE_FILE_BYTES)
            .unwrap();
        state
            .reserve_file(Path::new("second.tex"), MAX_INCLUDE_FILE_BYTES)
            .unwrap();
        state
            .reserve_file(Path::new("third.tex"), MAX_INCLUDE_FILE_BYTES)
            .unwrap();
        state
            .reserve_file(Path::new("fourth.tex"), MAX_INCLUDE_FILE_BYTES)
            .unwrap();
        assert!(state.reserve_file(Path::new("overflow.tex"), 1).is_err());
    }

    #[test]
    fn equations_require_the_conservative_command_subset() {
        let mut parser = LatexParser::new().unwrap();
        let analysis = parser
            .parse(
                FileId::new(),
                b"Safe $\\frac{\\alpha}{2}$ and custom $\\myMacro{x}$.\n",
            )
            .unwrap();
        assert!(analysis.projection.iter().any(|segment| matches!(
            segment.projection_kind,
            ProjectionKind::Supported {
                node_kind: VisualNodeKind::InlineEquation
            }
        )));
        assert!(analysis.projection.iter().any(|segment| matches!(
            &segment.projection_kind,
            ProjectionKind::RawInline { reason } if reason.contains("safe visual subset")
        )));
    }

    #[test]
    fn project_discovery_is_non_mutating() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.tex"), "hello").unwrap();
        let before: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let layout = ProjectLayout::discover(directory.path()).unwrap();
        let after: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(layout.main_relative().unwrap(), Path::new("main.tex"));
        assert_eq!(before, after);
        assert!(!layout.created_project);
    }

    #[test]
    fn bibliography_discovery_rejects_oversized_and_aggregate_inputs_before_open() {
        let oversized = tempfile::tempdir().unwrap();
        std::fs::File::create(oversized.path().join("large.bib"))
            .unwrap()
            .set_len(MAX_BIBLIOGRAPHY_FILE_BYTES.saturating_add(1))
            .unwrap();
        assert!(matches!(
            discover_project_bibliographies(oversized.path()),
            Err(AppError::InvalidProject { .. })
        ));

        let aggregate = tempfile::tempdir().unwrap();
        for index in 0..=MAX_BIBLIOGRAPHY_TOTAL_BYTES / MAX_BIBLIOGRAPHY_FILE_BYTES {
            std::fs::File::create(aggregate.path().join(format!("part-{index}.bib")))
                .unwrap()
                .set_len(MAX_BIBLIOGRAPHY_FILE_BYTES)
                .unwrap();
        }
        assert!(matches!(
            discover_project_bibliographies(aggregate.path()),
            Err(AppError::InvalidProject { .. })
        ));
    }

    #[test]
    fn bibliography_discovery_rejects_excessive_count_and_depth() {
        let count = tempfile::tempdir().unwrap();
        for index in 0..=MAX_BIBLIOGRAPHY_FILES {
            std::fs::write(count.path().join(format!("reference-{index}.bib")), b"").unwrap();
        }
        assert!(matches!(
            discover_project_bibliographies(count.path()),
            Err(AppError::InvalidProject { .. })
        ));

        let depth = tempfile::tempdir().unwrap();
        let mut nested = depth.path().to_path_buf();
        for _ in 0..=MAX_BIBLIOGRAPHY_DEPTH {
            nested.push("d");
        }
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("deep.bib"), b"").unwrap();
        assert!(matches!(
            discover_project_bibliographies(depth.path()),
            Err(AppError::InvalidProject { .. })
        ));
    }
}
