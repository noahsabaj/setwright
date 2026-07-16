# Release process

Setwright intends to launch Windows, macOS, and Linux together. The GitHub
workflow is a packaging skeleton: it can build draft artifacts after repository
CI passes, but maintainers must supply and validate signing, notarization,
runtime hosting, and external acceptance evidence before publishing.

## Candidate requirements

1. Choose one source commit and version; the tree and lockfiles are clean.
2. Close every applicable item in
   [release-readiness.md](release-readiness.md) with evidence.
3. Revalidate arXiv's current TeX Live/processor policy and the pinned
   submission-tools commit.
4. Reproduce managed runtime archives, verify signatures/SBOM/licenses, and
   exercise fresh and resumed installation.
5. Run CI, source-preservation corpus, malicious-project suite, accessibility
   matrix, and clean-machine workflows on the exact candidate.
6. Build from protected CI with production credentials; never copy signing keys
   into the repository or ordinary build logs.
7. Verify installer and updater signatures on fresh machines before promotion.
8. Attach checksums, SBOM, third-party notices, runtime manifest references,
   known limitations, and security contact instructions.

## Intended artifacts

- Windows 10/11 x64 NSIS installer with Authenticode signature and Evergreen
  WebView2 `125.0.2535.41` or newer.
- macOS 15+ with current system updates, for Intel and Apple Silicon, distributed
  as signed and notarized DMGs.
- Linux x86_64 AppImage and `.deb` built against a fully security-updated Ubuntu 22.04 baseline, with
  checksums, SBOM, and CI provenance.

The macOS Intel CI image is scheduled to leave GitHub-hosted Actions in August
2027. Provision a replacement Intel runner before that date; loss of hosted
capacity is not permission to remove the required Intel preview check or
silently narrow the supported architecture set.

The application installer does not contain TeX. Managed profile manifests and
archives are separately signed release inputs and cannot be silently replaced.

## Workflow behavior

The manual release workflow creates a draft GitHub release and uploads artifacts
from its target matrix. Draft output is for inspection only. Publication is a
separate maintainer action after signature, notarization, update, and
clean-machine evidence has been checked. An absent credential must fail the
corresponding release job rather than yield an apparently official unsigned
installer.

The workflow file intentionally documents secret names without providing
values. Repository administrators must map those names to least-privilege
environment secrets and protect the `release` environment with reviewers.

## Stop-ship defects

Do not publish with a known silent-content-loss, sandbox-escape,
stale-artifact/stale-revision, signature-bypass, or non-reproducible-export
defect. A successful Setwright arXiv Check does not replace review of arXiv's
generated PDF.
