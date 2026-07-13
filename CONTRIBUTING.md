# Contributing to Setwright

Thank you for helping make scientific writing more dependable and accessible.
Contributions are accepted under the Apache License 2.0 and the governance
rules in [GOVERNANCE.md](GOVERNANCE.md).

## Before starting

- For a security vulnerability, follow [SECURITY.md](SECURITY.md) and do not
  open a public issue.
- For behavior changes, open an issue or short design discussion before a large
  implementation. Source preservation, sandbox policy, and public file formats
  require maintainer review.
- Keep external release gates explicit. A local green test does not prove a
  sandbox, code signature, runtime mirror, or arXiv acceptance.

## Developer Certificate of Origin

Every commit must be signed off under [DCO 1.1](DCO.md). Add the following line
using your real name and an email address you control:

```text
Signed-off-by: Your Name <you@example.com>
```

`git commit -s` adds the line automatically. The sign-off certifies that you
have the right to submit the contribution; it is not a cryptographic signature.
Do not sign off for another contributor unless you are legally authorized to do
so.

## Development workflow

1. Create a focused branch from the default branch.
2. Install the prerequisites in [docs/development.md](docs/development.md).
3. Make the smallest coherent change and add tests for behavior.
4. Run `pnpm check` and the platform-specific checks relevant to the change.
5. Confirm all commits carry a DCO sign-off.
6. Open a pull request using the repository template.

Pull requests should explain source-format effects, security-boundary effects,
accessibility impact, and what was not verified. Generated bindings, schemas,
and fixtures must be committed when their source contract changes.

## Compatibility rules

- Never normalize an untouched source span as a side effect of a visual edit.
- Unsupported or ambiguous LaTeX must remain source-editable and byte-preserved.
- Never fall back to unsandboxed compilation.
- File-format changes need backward-compatibility tests and a schema versioning
  note.
- User-visible controls need keyboard behavior, an accessible name, visible
  focus, and a 200% reflow check.

## Licensing and provenance

New dependencies must have licenses compatible with Apache-2.0 and must be
represented in release SBOM and license inventories. Test papers, fonts,
images, and bibliographic data need documented redistribution permission. Do
not copy paper text or visual assets without a compatible license.
