# Security policy

## Supported versions

Setwright is pre-alpha and has no supported public release. Security fixes are
currently made on the default branch. This policy will name supported release
lines before the first stable installer is published.

## Reporting a vulnerability

Use GitHub's **Report a vulnerability** flow in the repository's Security tab
to open a private security advisory. Do not include exploit details, malicious
TeX, credentials, or personal paper content in a public issue.

If private security advisories are unavailable, contact a repository maintainer
privately through their verified GitHub profile. There is not yet a project
security mailbox; do not infer one from the project name.

Include, when safe:

- affected commit or version and operating system;
- a minimal reproduction using non-sensitive test data;
- expected and observed trust-boundary behavior;
- whether files, network, process execution, signatures, updates, or source
  integrity are involved;
- any known mitigations.

Maintainers will acknowledge reports as capacity permits. No response-time or
bounty commitment exists yet. Coordinated disclosure timing will be agreed with
the reporter based on impact and the availability of a tested fix.

## High-priority classes

- sandbox escape, network access, or reads outside the staged project;
- command, TeX, path, archive, or IPC injection;
- source corruption, silent normalization, or stale-revision overwrite;
- malicious runtime/update manifests, signature bypass, or rollback attacks;
- review suggestions applied to the wrong source;
- sensitive document data leaving the local machine unexpectedly.

The design threat model and current non-claims are in
[docs/security.md](docs/security.md).
