# Architecture

This document is the MVP architecture contract. It describes invariants that a
release must satisfy; it is not evidence that every subsystem is complete.

## System shape

Setwright is a Tauri 2 desktop application with two trust domains:

1. The React webview renders editor projections and sends typed intentions.
2. The Rust core owns project sessions, source buffers, persistence, scoped
   filesystem and network access, compilation, review data, and export.

The webview has no arbitrary filesystem, shell, SQL, or unrestricted HTTP
capability. Each native window has one isolated `ProjectSessionId`; opening a
different project creates a different window and session.

## Source-authority invariant

Ordinary project files are the paper format. Rust holds a revisioned UTF-8 byte
buffer for each file and treats visual documents, parser trees, CodeMirror
state, review overlays, and the PDF as disposable projections.

A visual operation follows this transaction:

1. Resolve a semantic `DocumentOp` against an expected project revision.
2. Produce the smallest set of `SourceEdit` byte replacements.
3. Confirm each edit's expected SHA-256 slice hash and non-overlap.
4. Apply edits to candidate buffers without touching the originals.
5. Incrementally parse candidates and check complete source coverage.
6. Commit all candidate buffers together and advance the revision, or commit
   none and return a structured conflict.

Every source byte must belong to exactly one supported semantic slot, preserved
wrapper/trivia/comment span, or raw node. A parser is a tolerant structural
locator, never a serializer. Ambiguous, malformed, or unsupported syntax is a
`RawInline` or `RawBlock` with exact source text.

Source-mode edits remain authoritative even when they make a region invalid or
unsupported. Such a region falls back to raw source; it is not repaired or
normalized automatically. BOM, newline style, comments, whitespace, and every
untouched byte are preserved. Non-UTF-8 files open source-only until the user
reviews and explicitly accepts a conversion.

A no-op open, mode switch, save, and close cannot write project files or create
project metadata.

## Includes and project boundaries

The include graph accepts only static, unique, acyclic `\input` and `\include`
targets that resolve inside the canonical project root. Those files may appear
in one visual document separated by explicit file-boundary nodes. Dynamic,
repeated, cyclic, missing, symlink-escaping, or outside-root targets remain raw.
Visual operations cannot span a file boundary.

All path policy is evaluated on canonical native paths in Rust. Display paths
are project-relative, use `/` in serialized formats, and never authorize file
access.

## Public contracts

Rust is the source of truth for internal IPC types. Checked-in TypeScript
bindings are generated from Rust `serde` types; CI must fail on binding drift.
The stable concepts are:

- `ProjectSessionId`, `Revision`, `FileId`, and byte-based `SourceSpan`;
- `SourceEdit { fileId, startByte, endByte, replacement,
  expectedSliceHash }`;
- semantic `DocumentOp` variants for text, marks, nodes, attributes, movement,
  and deletion;
- `ProjectEvent`, `CompileEvent`, `Diagnostic`, and structured `AppError`.

Versioned on-disk exchange formats have normative JSON Schemas in
[`schemas/`](../schemas/):

- `PaperSettingsV1` for Setwright-created/adopted projects;
- `ReviewBundleV1` for explicit review import/export;
- `RuntimeManifestV1` for immutable managed runtimes;
- `ArxivPreflightReportV1` for a specific frozen export attempt.

Imported projects do not receive `paper-settings.json` unless the user chooses
to adopt Setwright metadata. Review bundles are never paper source and are
excluded from paper and arXiv exports.

## Persistence and conflicts

Saving is debounced for 750 ms after an edit. Each changed file is written to a
same-directory temporary file, flushed, and atomically replaced. A recovery
journal records a multi-file transaction before replacement so restart can
recover the complete old or complete new state, never a mixture.

App-owned state lives outside the paper directory in SQLite plus a SHA-256,
zstd-compressed object store. Automatic history is taken after 30 seconds idle,
at most once per minute. Retention keeps the newest 100 snapshots and daily
snapshots for 30 days; named and pre-restore snapshots require explicit
deletion.

External changes reload a clean buffer. If a buffer is dirty, Setwright pauses
save and compile for that project and requires a three-way source merge.

Comments and suggestion overlays do not mutate canonical source. Accepting a
suggestion reuses the normal revision/hash-validated patch path and creates a
pre-accept snapshot. Failed re-anchoring is a conflict, never an approximate
application.

## Compilation data flow

Compilation always uses a frozen staged snapshot:

```text
source revision -> isolated stage -> OS sandbox -> latexmk -> candidate output
       ^                                                     |
       +---- publish only if the revision still matches -----+
```

One job may run per session. New edits coalesce to the newest revision and
terminate the stale process tree. Only a current successful result may publish
PDF, SyncTeX, diagnostics, dependency recorder output, or auxiliary cache. A
failed build may leave the previous PDF visible, but it must be labeled stale.

Runtime and platform policy is detailed in [runtimes.md](runtimes.md) and
[security.md](security.md). arXiv export adds a second clean extraction and
compile described in [arxiv.md](arxiv.md).

## Performance budgets

On a 1 MB, 20-file project, the release target is initial projection below one
second; incremental parse/projection below 100 ms p95; key-to-paint below 50 ms
p95; and edit IPC acknowledgement below 100 ms p95. These are acceptance
budgets, not claims about the current scaffold.
