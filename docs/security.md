# Security model

TeX is a programming language and a paper project can be hostile. Setwright must
not describe compilation as safe until the platform sandbox and malicious-input
suite pass on the actual signed release artifact.

## Assets and adversaries

Protected assets include paper source, files outside the project, credentials,
network identity, app/runtime signing keys, the integrity of generated PDFs and
ZIPs, and the user's expectation that accepted edits target the displayed
revision.

The threat model includes a malicious project/archive, malicious TeX package or
runtime mirror, compromised metadata response, hostile filename or symlink,
stale webview, and malformed review bundle. It does not assume the logged-in OS
account or operating-system kernel is hostile.

## Trust boundaries

- **Webview:** untrusted for authority. It may request typed operations but has
  no general file, shell, database, runtime-download, or network API.
- **Rust broker:** validates session, revision, capability, path, size, schema,
  and state transition before any side effect.
- **Compiler process:** hostile. It receives a staged copy and immutable runtime
  in a platform sandbox, never the original paper directory.
- **Network:** citation lookup and runtime/update download are distinct. Citation
  lookup is an explicit user action through an allowlisted Rust client. Compile
  has no network.
- **Manifests and updates:** data is not trusted merely because it arrived over
  TLS. Release manifests, runtimes, and updates require signature and checksum
  verification rooted in keys shipped with the application.

## Common compiler policy

The broker passes a fixed argument array to `latexmk -norc`; no command string
is interpreted by a shell. pdfLaTeX and XeLaTeX profiles disable shell escape,
enable SyncTeX and recorder output, ignore project/user `.latexmkrc`, and expose
only an allowlisted environment with an empty home/config directory.

The sandbox receives a read-only runtime, a read-only staged input tree, and a
bounded writable output area. It has no network, no outside-root paths, and no
usable escaping symlink. Preview builds are limited to five TeX passes, 60
seconds, 2 GB memory, 1 GB writable output, and a bounded process count. The
broker drains all output while truncating only the UI copy and kills the entire
job tree on cancel or revision replacement.

## Platform enforcement required for release

- **Windows:** a restrictive AppContainer identity, ACL-limited stage/runtime,
  and kill-on-close Job Object.
- **macOS:** a separately signed XPC compiler service inside App Sandbox, with
  no network entitlement and only app-group staging access.
- **Linux:** bundled bubblewrap with user, mount, PID, IPC, and network
  namespaces; dropped capabilities; `no_new_privs`; seccomp; and resource
  limits. If user namespaces are unavailable, compilation is disabled.

Sandbox startup is fail closed. An unavailable policy never falls back to
running TeX with the user's normal privileges.

## Source and archive integrity

All edits carry an expected revision and slice hash. Multi-file writes are
journaled and atomic. Review suggestions must match or unambiguously re-anchor;
otherwise the user resolves a conflict.

Archive extraction rejects absolute paths, parent traversal, device/reserved
names, symlinks, case-fold collisions, duplicate normalized paths, and resource
limits. Export walks an allowlisted dependency closure and never follows a path
outside the canonical project root.

## Release evidence

The security gate includes tests proving hostile TeX cannot read an outside
canary, modify the original project, resolve DNS or HTTP, execute `write18`,
load `.latexmkrc`, escape the process tree, or survive cancellation. The tests
must run on clean machines against the packaged binary, not only unit-test
policy objects.

Current status: this repository does not yet claim that those platform gates,
production signing keys, managed-runtime hosting, or independent security
review are complete. See [release readiness](release-readiness.md).
