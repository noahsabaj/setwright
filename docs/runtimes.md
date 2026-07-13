# Managed TeX runtimes

Setwright keeps TeX out of the desktop installer. The first compile installs a
complete, immutable offline profile after showing its multi-gigabyte size and
required free space. A partial or unverifiable runtime is never executable.

## Initial target profiles

| Profile ID | Snapshot | Engines certified by Setwright |
| --- | --- | --- |
| `texlive-2025-2025-08-03` | TeX Live 2025, 2025-08-03 | pdfLaTeX, XeLaTeX |
| `texlive-2023-2023-05-21` | TeX Live 2023, 2023-05-21 | pdfLaTeX, XeLaTeX |

The 2025 profile is the default for new Setwright papers. These are release
targets derived from the planned arXiv compatibility matrix, not a permanent
claim about arXiv. Maintainers must re-check the official
[arXiv TeX Live policy](https://info.arxiv.org/help/faq/texlive.html) and
processor support immediately before each release.

Plain TeX, pdfTeX, LaTeX/DVI, LuaLaTeX, and a user-installed system TeX may be
opened in source mode, but the MVP does not label their output arXiv-ready.

## Manifest and installation

Every platform/architecture archive has a `RuntimeManifestV1` conforming to
[`runtime-manifest.schema.json`](../schemas/runtime-manifest.schema.json). The
manifest fixes profile ID, TeX snapshot, platform, architecture, supported
engines, archive size and SHA-256, signature, SBOM, and license inventory.

Installation proceeds as follows:

1. Fetch a signed manifest from a configured HTTPS origin.
2. Verify it with a trust root embedded in the signed Setwright application.
3. Check disk space and obtain explicit user confirmation.
4. Download to app-owned storage with range-based resume.
5. Verify exact byte count, SHA-256, and detached manifest signature.
6. Extract to a new directory with path and size limits.
7. self-test pdfLaTeX, XeLaTeX, BibTeX, Biber, `latexmk`, and SyncTeX inside the
   target platform sandbox.
8. Atomically publish the profile directory or delete it on failure.

Profiles are content-immutable. An update gets a new profile ID and directory;
projects never change pins automatically. Rollback selects an already verified
profile rather than mutating one in place.

## Reproducible builds and distribution

Runtime build inputs, scripts, upstream checksums, patches, SBOM, and complete
license inventory must be recorded per profile. Build workers are separated
from signing keys. At least two maintainers verify the final manifest and a
clean-machine install before publication.

No production download origin, signing trust root, multi-platform runtime
archive, or hosting durability policy exists in this scaffold. Those are open
external release gates; example manifests must never be treated as trusted
production metadata.

## Compile profiles

The broker invokes `latexmk -norc` with a fixed, code-owned argument list. User
and project `.latexmkrc` files do not load. Shell escape stays disabled.
Compilation occurs from a staged snapshot under the policy in
[security.md](security.md), and output is published only if its source revision
is still current.
