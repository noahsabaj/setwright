# Setwright 0.2.0 Windows installation and update verification

Verified locally on Windows on 2026-09-13:

- Production NSIS packaging completed, producing the installer, updater signature and static update manifest. The installer exited successfully.
- The installed executable at `C:\Users\noahs\AppData\Local\Programs\Setwright\setwright-desktop.exe` reports product version 0.2.0. The user's Start menu shortcut targets that file. Windows uninstall registration under the user's SID records the same version and location.
- Computer Use launched the installed executable, opened Updates & changelog, and verified the bundled 0.2.0 and 0.1.0 entries. The absent public feed is reported as a failed check, not as an up-to-date result.
- The automatic-update checkbox persisted through a complete application restart. It was returned to the default off state after inspection. No paper was opened or edited during update-panel verification.
- All 120 frontend tests across 14 files pass. Three release-script tests cover synchronized versions, newer-version ordering including prereleases, changelog requirements, tag matching and manifest prerequisites.
- ESLint, production TypeScript/Vite build, Cargo Clippy with warnings denied, and the full Rust test suite pass. Native tests cover updater-operation exclusion, persisted preferences, and blocking installation when even a clean paper session is open. Frontend tests cover safe notes rendering, failed checks/downloads/preferences, automatic download/install gating, and saving all queued file edits before close, including rejected-draft recovery.

Installer SHA-256: `73e8077d1b81f52d2c132c31c5e8e18f991d84b113d58fd99b488388438d2b98`.

The updater signature is distinct from Authenticode; Windows reports this installer as `NotSigned` for publisher certification. There has not yet been a published feed or real installed N to N+1 download-and-restart exercise. The signature-rejection UI test uses a mocked native rejection; it is not proof of end-to-end cryptographic acceptance. These limits leave the release-readiness gates open.

The source, workflows, installer, manifest and release notes are prepared locally. Automatic approval review rejected the attempted public branch push because explicit authorization to upload this code to `noahsabaj/setwright` is required. No source, installer, tag, release or private signing key was uploaded during this work. Publishing and configuring the GitHub release secret remain pending that authorization and the required repository checks.
