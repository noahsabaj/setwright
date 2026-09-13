# Installing and updating Setwright

Run the Windows NSIS installer. It installs for the current user and adds **Setwright** to the Start menu. Open Windows search and type Setwright. No development server or terminal is needed. Windows publisher certification is separate from the updater signature; the current authoring preview is not Authenticode certified.

The bottom **Updates & changelog** button shows the installed version, bundled changelog (available offline), manual update check, download progress, and optional automatic updates. Automatic updates default off. Checks run at startup and every six hours while the app is open. A failed check is reported as an error, never as “up to date.” Release notes are rendered as text without executing HTML or fetching embedded resources.

An update must pass Tauri's signature verification before it can be installed. Installation is blocked in Rust while **any paper in any window** is open, because even a clean native session may have pending editor drafts. Use the project menu's **Save and close paper** action: it waits for the shared write queue and successful saving. Rejected edits keep the paper open. With automatic updates enabled, a verified download installs after every paper closes; otherwise click **Install update and restart**. Setwright must be running to check or install updates.

## Version policy

Use SemVer: PATCH for fixes, MINOR for compatible features, MAJOR for incompatible public behavior or project format changes. During 0.x development, document any breaking changes explicitly and increment MINOR. This release is 0.2.0. Keep package.json, Cargo.toml, Cargo.lock and tauri.conf.json synchronized:

```powershell
node scripts/release.mjs prepare 0.2.1
# Add a dated 0.2.1 entry to CHANGELOG.md describing the actual changes.
node scripts/release.mjs check
pnpm check
```

The build refuses mismatched versions or missing release notes. Preparation refuses equal or older versions. The in-app changelog is bundled from CHANGELOG.md; update notes in latest.json come from that version's entry.

## Signing and packaging

Keep the original updater key. Changing it would prevent installed copies from verifying future updates. The local key lives outside the repository at `%LOCALAPPDATA%\SetwrightReleaseKeys\updater.key`, with restricted file access. Back it up securely. Its public key is embedded in tauri.conf.json. Never commit the private key or print it in logs.

```powershell
powershell -NoProfile -File scripts/package-windows.ps1
```

This produces the NSIS installer, its `.sig`, and `latest.json` under `src-tauri/target/release/bundle/nsis`. Set `TAURI_SIGNING_PRIVATE_KEY` to the same key (contents or file path) when building elsewhere. An encrypted key additionally requires `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

## Release workflow

1. Prepare the version and changelog, pass checks, and merge through the required pull request checks. Do not bypass main's protection.
2. Create the matching `vMAJOR.MINOR.PATCH` tag on the reviewed commit.
3. Configure the `release` environment's `TAURI_SIGNING_PRIVATE_KEY` secret with the existing key, plus its password secret if needed.
4. Run **Windows authoring preview** for that tag. It verifies the selected commit, runs tests, signs updater artifacts, and creates a draft with release notes. This Windows preview does not assert production certification or compiler availability. The separate full-platform release workflow still requires Authenticode and Apple credentials.
5. Review the draft artifacts and known limitations. Publishing a non-prerelease as GitHub's latest release activates the installed app's feed: `https://github.com/noahsabaj/setwright/releases/latest/download/latest.json`. Drafts and prereleases do not supply this stable feed. Keep the installer, signature and manifest together and retain previous releases.
6. Exercise an installed N to N+1 upgrade and recovery on a clean machine before closing the corresponding release-readiness gate. Never replace an existing version's assets with different bytes; publish a new version for fixes.

Until the first release is published, update checks report that the feed is unavailable. Local installation and the offline changelog work independently. The application does not run in the background after exit and does not provide an automatic downgrade mechanism. Compiler runtime delivery, full platform certification and clean-machine update acceptance remain tracked in docs/release-readiness.md.
