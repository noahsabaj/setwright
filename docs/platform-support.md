# Platform and PDF preview support

`config/platform-support.json` is the machine-readable compatibility policy.
Tests keep the values below synchronized with Vite, Tauri packaging, release
documentation, and CI.

| Platform | Supported floor | PDF preview enforcement |
| --- | --- | --- |
| Windows | Windows 10/11 x64; Evergreen WebView2 `125.0.2535.41`+ | The NSIS bootstrapper updates an older runtime. Startup diagnostics and the real PDF worker/canvas probe catch a missing, paused, or broken runtime. |
| macOS | macOS 15+ with current system and Safari updates; Intel and Apple Silicon | The bundle and embedded helpers declare deployment target 15.0. Native CI runs on both architectures and the render probe remains authoritative. |
| Linux | Ubuntu 22.04 x86_64 with current security updates | Native CI records WebKitGTK behavior. The real render probe fails only the preview surface if the distribution webview cannot render safely. |

The frontend is compiled for Chromium 125 and Safari 18 syntax. Setwright loads
the PDF.js legacy browser entry and its same-version local worker; it does not
use a CDN, accept PDF.js's in-process fake-worker fallback, fall back to a
mismatched worker, or relax the production CSP.

On first use in a process, PDF preview renders a deterministic two-page vector
fixture and verifies known pixels. A failed probe produces a platform-specific
update message and disables only preview. Editing, saving, retained compilation
artifacts, and the deliberately fail-closed compile boundary remain available.

Windows production packages use automatically serviced Evergreen WebView2.
Fixed WebView2 binaries are useful for isolated compatibility testing but are
not shipped: bundling a privately serviced browser runtime would substantially
increase installer size and transfer browser security patching to Setwright.

The Chromium-floor job uses exact Chrome for Testing `125.0.6422.141` and its
matching ChromeDriver. Google's Chrome for Testing index publishes the official
archive URLs but not SHA-256 values, so Setwright records each archive's SHA-256
alongside its immutable Google Cloud Storage generation, size, MD5, and CRC32C
metadata. CI revalidates the upstream object metadata before downloading, then
checks the local byte size and SHA-256 before execution.

Primary compatibility references:

- [PDF.js 6 release](https://github.com/mozilla/pdf.js/releases/tag/v6.0.227) and [supported environments](https://github.com/mozilla/pdf.js/wiki/frequently-asked-questions#which-browsersenvironments-are-supported)
- [Tauri Windows installer and minimum WebView2 version](https://v2.tauri.app/distribute/windows-installer/)
- [Microsoft Evergreen WebView2 distribution](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)
- [Tauri WebDriver testing](https://v2.tauri.app/develop/tests/webdriver/) and [WebdriverIO's Tauri plugin setup](https://webdriver.io/docs/desktop-testing/tauri/plugin-setup/)
- [Chrome for Testing availability API](https://github.com/GoogleChromeLabs/chrome-for-testing)
