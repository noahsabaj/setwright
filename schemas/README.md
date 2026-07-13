# Setwright JSON Schemas

These Draft 2020-12 schemas are normative for version 1 on-disk metadata and
exchange files. Internal Rust/TypeScript IPC contracts are generated from Rust
types and are not defined by these schemas.

| Schema | Producers and consumers |
| --- | --- |
| `paper-settings.schema.json` | Setwright-created projects and imported projects explicitly adopted by the user |
| `review-bundle.schema.json` | Explicit `.setwright-review` import/export |
| `runtime-manifest.schema.json` | Signed, immutable managed-runtime distribution |
| `arxiv-preflight-report.schema.json` | Report beside a specific arXiv candidate ZIP |

Version 1 objects reject unknown properties. Producers must not add fields
without a compatible schema/version decision. Consumers reject an unsupported
`schemaVersion` rather than guessing.

Serialized file paths are project-relative, use `/`, and never contain `.` or
`..` segments. SHA-256 values are lowercase hexadecimal over the exact bytes
named by their field. Timestamps are RFC 3339/JSON Schema `date-time` strings.
Byte offsets index UTF-8 source bytes, not Unicode scalar values or UTF-16 code
units.

The examples in [`examples/`](examples/) are illustrative test data. Runtime
signatures, URLs, hashes, and preflight readiness in examples are not trusted
production statements.
