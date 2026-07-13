fn main() {
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/lib/bindings.ts");
    setwright_lib::ipc::command_builder()
        .export(specta_typescript::Typescript::default(), &output)
        .expect("failed to export Setwright TypeScript bindings");
    println!("wrote {}", output.display());
}
