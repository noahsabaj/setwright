# Vendored tree-sitter-bibtex

- Package: `tree-sitter-bibtex` 0.1.0 from crates.io
- Upstream commit: `968f8619cc4f42bcb53883d41ae2b2285dda5977`
- Upstream license: MIT (see `LICENSE`; copyright Patrick Förster, 2020)

The generated grammar, queries, parser C source, and Tree-sitter header are
copied byte-for-byte from the crates.io package. The Rust binding alone is
adapted to expose `tree_sitter_language::LanguageFn`, the ABI used by
tree-sitter 0.26, and to use Rust 2024's `unsafe extern` syntax. Setwright's
top-level build script compiles `src/parser.c`; the vendored crate is not added
as a separate Cargo dependency.

The crates.io 0.1.0 archive omitted the license file from its package include
list. `LICENSE` is retained from the upstream repository so redistributed
source contains the complete MIT notice.
