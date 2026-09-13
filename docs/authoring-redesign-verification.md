# Authoring redesign verification

The redesign keeps one active file per editor and one project revision queue. The compilation entrypoint remains `project.mainFile`. No IPC payloads or project file formats changed.

## Automated coverage

On 2026-09-12, all 113 frontend tests across 13 files and all 16 native IPC tests passed. ESLint, TypeScript, the production Vite build, and Rust formatting passed. Vite reports the existing large-chunk advisory for editor/math/PDF bundles.

The frontend suites cover delayed multi-file edits and saves, rejected and hidden drafts, uncertain-response recovery, conversion and restore ordering, native close interception (including queued and in-flight encoding conversions), source/visual undo boundaries, exact Unicode heading destinations, raw disclosures, assets, command-palette keys and focus, contextual formatting, splitter input, history races, and PDF failure states.

Commands use the installed tools directly because this checkout's `pnpm` launcher attempts dependency repair:

```powershell
node node_modules/eslint/bin/eslint.js . --max-warnings 0
node node_modules/vitest/vitest.mjs run
node node_modules/typescript/bin/tsc -b --pretty false
node node_modules/vite/bin/vite.js build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --lib ipc::tests --offline
cargo build --manifest-path src-tauri/Cargo.toml --bin setwright-desktop --offline
```

## Browser inspection

Inspected the running Vite app at 1280px, 700px, and 390px. Confirmed light, dark, and high-contrast treatments; the dedicated narrow mode row; navigation drawers; command focus restoration; selected heading formatting; exact heading focus and scrolling after drawer teardown; compact Editor/PDF switching; and restoration of the side-by-side layout. Pointer resizing changed the ratio from 56 to 63; keyboard input changed it from 54 to 56.

MathLive displayed the sample summation with its actual glyphs and no font-loading console errors. The production output includes the installed KaTeX font assets as local WOFF2 files.

A local, disposable browser fixture under `work/authoring-preview/` exercises the real PDF.js worker and canvas with a two-page PDF. Verified page navigation, stale warnings, a failed compile retaining the previous PDF, and malformed-file reporting. This fixture does not run a compiler or stand in for compiler delivery.

The in-app browser did not apply browser zoom shortcuts. A 640 by 450 CSS-pixel viewport with a device scale factor of 2 exercised the layout equivalent of 200% zoom on a 1280 by 900 display. Temporary browser overrides were reset. Native inspection found zoom hotkeys disabled by the Tauri window defaults; both initial and additional project windows now enable them, with the scoped zoom permission required by the macOS/Linux polyfill. The rebuilt Windows app accepted five Ctrl+plus steps from 100% to 200% and Ctrl+0 restored the original size. At actual 200%, onboarding scrolled without horizontal clipping; the workspace kept its mode row, toolbar, command access, and save/compiler status available. The navigation drawer opened and closed after exact Method heading navigation, and Split offered Editor/PDF tabs. Resetting zoom restored side-by-side panes. Native pointer dragging and Right Arrow both resized the splitter. Zoom was reset to 100% afterward.

## Native boundary and source preservation

The Windows debug app built and launched. Manual file selection exposed an integration bug: the selected filename was passed to an IPC argument that requires a directory. Both open flows now split the selected file into parent directory and basename. The native boundary accepts a selected main file for its immediate containing project without expanding generic filesystem scope; regression tests reject unselected siblings and ancestor roots and verify included-file loading without writes. The user selected the main file in the corrected build and the paper opened successfully in Write mode.

Computer Use then verified included-file heading navigation, expansion and collapse of preserved LaTeX, bibliography opening in Source, Ctrl+K and Enter command execution, creation of a named baseline, a Source edit confined to `sections/method.tex`, navigation away and back with caret position retained, successful local save, and actual summation/fraction glyphs in Write. Native Preview displayed the existing compiler-readiness explanation. Restoring the baseline succeeded, and Alt+F4 closed the clean native window normally.

SHA-256 comparisons of all eight existing files in `work/native-redesign-20260912/` matched the baseline after native navigation and raw disclosure. After the explicit math edit only `sections/method.tex` differed. After native version restoration all eight hashes matched again. Evidence is recorded in `test-results/redesign-native-fixture-before.json`, `redesign-native-navigation-after.json`, `redesign-native-edited.json`, and `redesign-native-restored.json`. History snapshots are intentional new metadata created by the version/save actions. The original `sample-project/` files also remain unchanged.

Native creation also passed: the Research article template created `work/native-created-20260912/` with the entered title `Native creation verification` and author `Test Author`, then opened in Write mode. The user handled the native destination picker. Hashes of all three created project files were identical before and after navigation, zoom, and Split inspection (`test-results/redesign-native-created-before.json` and `redesign-native-created-navigation-after.json`).

The live close check covered a clean saved project. Blocking close during pending writes, rejected hidden drafts, restoration, and encoding conversion is covered by automated tests, rather than an artificially delayed native backend. Native UI inspection was on Windows; macOS/Linux runtime verification remains outside this local run.

Compiler/runtime delivery and release certification remain separate acceptance gates. Compilation stays unavailable when the existing runtime readiness check is not satisfied.
