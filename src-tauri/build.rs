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

    // `tauri_build` emits the single application resource manifest, including
    // the Common-Controls v6 dependency required by native dialogs. A second
    // linker manifest collides with Tauri's `resource.lib`.
    tauri_build::build();

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
