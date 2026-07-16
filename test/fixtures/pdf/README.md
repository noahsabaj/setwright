# Representative PDF fixture

`sample-project.pdf` is the deterministic rendered form of the repository-owned
[`sample-project`](../../../sample-project) source. It exercises paper text,
equations, tables, source listings, cross-references, and a bibliography through
the same PDF preview boundary as production.

The adjacent manifest pins the exact TeX container digest, build command,
`SOURCE_DATE_EPOCH`, byte length, page count, and SHA-256 digest. CI rebuilds the
fixture in a clean directory and requires both a matching digest and a byte-for-byte
comparison before accepting changes.

The source and generated fixture are covered by this repository's Apache-2.0
license. Update the source, PDF, and manifest together; never replace the image
digest with a mutable tag.
