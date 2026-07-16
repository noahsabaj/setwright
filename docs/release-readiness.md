# Release readiness ledger

This ledger prevents a green local build from being mistaken for a releasable
desktop product. Evidence links replace `Open` as gates are independently
completed; a claim without reproducible evidence does not close a gate.

| Gate | Status | Evidence required |
| --- | --- | --- |
| Byte-preserving editor corpus | Open | No-op and randomized edit/undo results over synthetic fixtures and at least 25 permissively licensed papers |
| Windows compiler containment | Open | Packaged AppContainer adversarial run on supported Windows versions |
| macOS compiler containment | Open | Signed XPC/App Sandbox adversarial run on Intel and Apple Silicon |
| Linux compiler containment | Open | Packaged bubblewrap adversarial run and fail-closed user-namespace case |
| Managed TeX Live 2025 profile | Open | Reproducible archives, signatures, SBOM/licenses, clean install on every target |
| Managed TeX Live 2023 profile | Open | Reproducible archives, signatures, SBOM/licenses, clean install on every target |
| Durable runtime hosting | Open | Production origin, access controls, retention, range/resume, rollback exercise |
| Production signing/notarization | Open | Authenticode, Apple notarization, Linux provenance, updater signature verification |
| Accessibility matrix | Open | NVDA, VoiceOver, and Orca reports for the packaged release candidate |
| arXiv oracle | Open | Reviewed immutable submission-tools commit and passing archive comparisons |
| Real arXiv rehearsal | Open | Actual submission-generated PDF reviewed for each certified engine/template path |
| Signed update N to N+1 | Open | Clean-machine update, rollback/recovery, and signature-failure tests |
| Third-party compliance | Open | Release SBOM, complete binary/runtime license inventory, NOTICE review |
| Independent security review | Open | Resolved findings or documented accepted risks for the release candidate |
| Name and mark clearance | Open | Counsel-reviewed clearance search, ownership decision, and any desired filings for Setwright and the tagline |

There are currently no signed release artifacts. Do not change this document to
`Complete` merely because a workflow uploaded an unsigned artifact or a local
machine had TeX installed.

The `Sandbox containment` CI matrix is deliberately non-closing evidence. It
runs a test-signed hostile native fixture through AppContainer, an embedded XPC
service, or pinned bubblewrap and uploads the existing probe schema with all
real-TeX workflow fields false. The test also requires attestation rejection.
The three containment rows remain `Open` until signed TeX Live profiles pass on
clean machines using packaged release-candidate binaries.
