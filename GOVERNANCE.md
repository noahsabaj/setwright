# Setwright governance

Setwright uses a maintainer-led, consensus-seeking model. This document applies
to the open-source project; it does not create a promise of hosted services,
support, or release dates.

## Roles

**Contributors** submit issues, documentation, tests, designs, or code under the
project license and DCO.

**Reviewers** are contributors trusted to review a defined area. They may label
and approve changes but cannot merge protected changes unless they are also a
maintainer.

**Maintainers** merge changes, manage releases, set repository policy, and act
as stewards of project security and trademarks. The current maintainer set is
the set of people with the GitHub repository's `Maintain` or `Admin` role; this
repository does not yet publish a separate named roster.

## Decisions

Routine changes require one maintainer approval and passing required checks
when the change author is not the project's only active maintainer. While the
project has only one active maintainer, routine changes instead require a
public pull request, passing required checks, and resolved review threads;
GitHub is configured with zero required approvals because an author cannot
approve their own pull request.
Changes to any of the following require two maintainer approvals when two or
more maintainers are active, and otherwise a public seven-day review window:

- source-authority or byte-preservation invariants;
- compiler sandbox policy or capability boundaries;
- public schemas and their compatibility guarantees;
- signing, update, managed-runtime, or release trust roots;
- licensing, governance, or trademark policy.

Maintainers seek consensus. If consensus cannot be reached, the maintainers may
make a recorded majority decision. A maintainer with a financial, employment,
or personal conflict must disclose it and abstain where it could reasonably
affect the decision.

## Becoming or removing a maintainer

An existing maintainer may nominate a contributor who has demonstrated
sustained, careful work, sound security judgment, and constructive review. The
same approval rule as a protected decision applies. Inactive maintainers may be
moved to emeritus status after six months without project activity, following a
private attempt to contact them and a public seven-day notice.

A maintainer may be removed for serious or repeated policy violations using the
protected-decision rule, excluding the subject from the vote. Urgent repository
access can be suspended while a security or conduct incident is investigated.

## Releases

A maintainer may cut a release only after every gate in
[docs/releasing.md](docs/releasing.md) is evidenced. Local CI success alone is
not enough. Signing/notarization credentials, immutable runtime hosting,
platform sandbox acceptance, and external arXiv rehearsals remain separate
gates.

## Amendments

Governance changes use the protected-decision process and must be recorded in a
pull request. History remains the audit trail.
