# Development guide

## Toolchain

Use Node.js 22+, pnpm 11, Rust 1.88+, and the Tauri 2 prerequisites for the host
platform. Keep package-manager and Rust lockfiles committed once generated; CI
must install from them without silently upgrading dependencies.

```sh
pnpm install
pnpm dev          # webview only
pnpm tauri dev    # desktop application
pnpm check        # lint, tests, frontend build, Rust tests
```

Rust formatting and strict lint checks:

```sh
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
```

## Repository map

- `src/`: React visual/source/preview UI and generated IPC bindings.
- `src-tauri/`: Rust authority, Tauri commands, permissions, and native broker.
- `schemas/`: normative JSON Schema for versioned exchange formats.
- `templates/`: first-party ordinary-LaTeX starting projects.
- `sample-project/`: multi-file paper used for smoke testing and demonstrations.
- `docs/`: architecture, threat model, runtime, arXiv, and release contracts.

The frontend cannot grow a direct filesystem, shell, SQL, or unrestricted HTTP
dependency. Add a narrowly scoped Rust command and capability instead. Compiler
launch remains exclusively in Rust with fixed arguments.

## Contract changes

Rust `serde` types are authoritative for IPC. Regenerate and commit TypeScript
bindings whenever they change, then run the drift check. On-disk formats also
require a schema update, valid/invalid fixtures, compatibility tests, and a
documented version transition. Version 1 schemas reject unknown properties to
catch misspellings; add fields through a versioned compatibility decision, not
an undocumented producer change.

## Source-preservation tests

Every editing change should cover:

- exact no-op bytes and directory manifest;
- byte-range minimality and expected-slice hash conflicts;
- BOM, LF/CRLF, Unicode, comments, spacing, malformed syntax, and raw nodes;
- edit/undo restoring exact bytes;
- stale revision and external-dirty conflict behavior;
- include boundaries and paths that attempt to escape the project.

Fixtures copied from real papers need a permissive license recorded beside the
fixture. Do not check in author manuscripts merely because they are public.

## Platform checks

Compilation work needs unit policy tests and packaged clean-machine adversarial
tests on Windows, macOS, and Linux. A mocked sandbox or successful local TeX run
does not close the platform acceptance gate. Accessibility work includes native
screen-reader checks listed in [accessibility.md](accessibility.md).
