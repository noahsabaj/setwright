# Setwright

[![CI](https://github.com/noahsabaj/setwright/actions/workflows/ci.yml/badge.svg)](https://github.com/noahsabaj/setwright/actions/workflows/ci.yml)
[![DCO](https://github.com/noahsabaj/setwright/actions/workflows/dco.yml/badge.svg)](https://github.com/noahsabaj/setwright/actions/workflows/dco.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-black.svg)](LICENSE)

**Write papers, not TeX.**

Setwright is a local-first, open-source desktop editor for LaTeX research
papers. It is designed to give authors a structured visual writing surface
without replacing the ordinary `.tex`, `.bib`, and asset files that make up a
paper.

> [!IMPORTANT]
> Setwright is a pre-alpha engineering project. The repository contains an MVP
> scaffold and contracts for the intended architecture; it is not yet approved
> for processing untrusted TeX or producing arXiv-ready submissions. There are
> no signed public installers or hosted TeX runtime profiles yet.

## Product principles

- **Source stays authoritative.** Visual editing is expressed as small,
  revision-checked source patches. Unsupported LaTeX stays visible and
  byte-preserved as raw source.
- **Local first.** Papers and review data remain on the author's computer. No
  account, telemetry, hosted document service, or AI writing service is part of
  the MVP.
- **Fail closed.** Compilation must run in an OS sandbox with no network and no
  access outside an isolated staging directory. A platform without a proven
  sandbox gets editing, not an unsandboxed fallback compiler.
- **Honest export.** Setwright can preflight a submission, but the PDF generated
  by arXiv remains the external acceptance gate.

## Intended MVP

The application uses a Tauri 2 and Rust core with a React 19/Vite frontend. The
target experience includes Write, Source, Preview, and Split modes; a
conservative visual subset for scientific writing; PDF preview; citations,
math, tables, and figures; comments and suggestions; revision history; managed
TeX Live profiles; and a reproducible arXiv export check.

The architecture, file formats, and release gates are described in:

- [Architecture](docs/architecture.md)
- [Security model](docs/security.md)
- [Managed runtimes](docs/runtimes.md)
- [arXiv preflight and export](docs/arxiv.md)
- [Release process](docs/releasing.md)

## Implemented pre-alpha vertical slice

The repository currently includes the native Tauri shell, the four editing
modes, source-authoritative project sessions, conservative LaTeX projection
with exact raw fallbacks, revision/hash-guarded byte patches, atomic multi-file
saves and recovery, local snapshots, external-change merge primitives, the
three checked-in templates, local and explicitly-triggered citation services,
generated Rust/TypeScript contracts, PDF preview plumbing, and a coalescing
compile scheduler. Runtime manifests, staged compilation, sandbox attestations,
review bundles, and arXiv preflight also have typed fail-closed cores and tests.

Compilation remains deliberately unavailable in the shipped shell until real
AppContainer, XPC/App Sandbox, and bubblewrap executors pass the release-risk
spike with signed managed TeX profiles. Full review UX, SyncTeX, clean-room
arXiv export/oracle execution, signed updates/installers, the real-paper corpus,
and platform accessibility/security rehearsals remain release work. The
[release-readiness ledger](docs/release-readiness.md) is authoritative.

## Development

Prerequisites:

- Node.js 22 or newer and pnpm 11
- Rust 1.95 or newer
- The [Tauri 2 system prerequisites](https://v2.tauri.app/start/prerequisites/)
  for the host platform

```sh
pnpm install
pnpm tauri dev
```

Run the repository checks with:

```sh
pnpm check
```

See [the development guide](docs/development.md) for repository conventions and
platform notes. The example paper in [`sample-project/`](sample-project/) and
the Generic, ACM, and IEEE starting points in [`templates/`](templates/) are
ordinary LaTeX projects and can also be opened in other editors.

## Contributing

Setwright is licensed under Apache License 2.0. Contributions require a
Developer Certificate of Origin sign-off. Read [CONTRIBUTING.md](CONTRIBUTING.md),
[GOVERNANCE.md](GOVERNANCE.md), and [SECURITY.md](SECURITY.md) before opening a
change.

The Setwright name and visual identity are not granted by the source license;
see [TRADEMARK.md](TRADEMARK.md).
