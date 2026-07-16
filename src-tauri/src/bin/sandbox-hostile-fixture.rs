//! Desktop-package entry point for the containment probe fixture.
//!
//! The implementation also belongs to a dependency-minimal standalone crate
//! so Linux CI can cross-compile a static musl helper without pulling the
//! Tauri/WebKit dependency graph into the target.

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tools/sandbox/hostile-fixture/src/main.rs"
));
