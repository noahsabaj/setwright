fn main() {
    let bibtex_source = std::path::Path::new("../vendor/tree-sitter-bibtex/src");
    cc::Build::new()
        .include(bibtex_source)
        .file(bibtex_source.join("parser.c"))
        .warnings(false)
        .compile("tree-sitter-bibtex");
    println!(
        "cargo:rerun-if-changed={}",
        bibtex_source.join("parser.c").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        bibtex_source.join("parser.h").display()
    );

    #[cfg(target_os = "macos")]
    {
        const MACOS_DEPLOYMENT_TARGET: &str = "15.0";
        let bridge = std::path::Path::new("native/macos/setwright_xpc_bridge.m");
        cc::Build::new()
            .file(bridge)
            .flag("-fblocks")
            .flag(&format!("-mmacosx-version-min={MACOS_DEPLOYMENT_TARGET}"))
            .compile("setwright_xpc_bridge");
        println!("cargo:rerun-if-changed={}", bridge.display());
        println!("cargo:rustc-link-arg=-mmacosx-version-min={MACOS_DEPLOYMENT_TARGET}");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Security");
    }

    // `tauri_build` emits the single application resource manifest, including
    // the Common-Controls v6 dependency required by native dialogs. A second
    // linker manifest collides with Tauri's `resource.lib`.
    tauri_build::build();

    // The WDIO 1.2.0 crates currently depend on Tauri with its default
    // features, which enables muda's Common-Controls v6 TaskDialog support.
    // Rust unit-test harnesses do not receive Tauri's application resource, so
    // declare the same side-by-side assembly dependency for the feature-gated
    // graph. LINK.exe deduplicates identical MANIFESTDEPENDENCY directives.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var_os("CARGO_FEATURE_PDF_PREVIEW_E2E").is_some()
    {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' \
             name='Microsoft.Windows.Common-Controls' version='6.0.0.0' \
             processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    // Some Windows SDK / MSVC combinations leave a COFF directive object
    // named `msvcrt.lib` beside the generated resource library. `cc` adds this
    // directory to the native-library search path, so a later test or cdylib
    // link can resolve that 82-byte object instead of the real CRT import
    // library. It is not an application resource and must not survive the
    // build script.
    #[cfg(all(target_os = "windows", target_env = "msvc"))]
    if let Some(out_dir) = std::env::var_os("OUT_DIR") {
        let shadow_crt = std::path::PathBuf::from(out_dir).join("msvcrt.lib");
        if shadow_crt.exists() {
            std::fs::remove_file(&shadow_crt).unwrap_or_else(|error| {
                panic!(
                    "failed to remove shadow CRT library {}: {error}",
                    shadow_crt.display()
                )
            });
        }
    }
}
