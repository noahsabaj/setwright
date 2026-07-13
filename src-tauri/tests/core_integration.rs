// Compile the core independently of the Tauri adapter. This path attribute can
// be removed once `src/lib.rs` publicly declares the same module; keeping it is
// useful because it proves the domain core does not depend on desktop state.
#[path = "../src/core/mod.rs"]
pub mod core;

use core::{FileId, LatexParser, Revision, SourceBuffer, SourceEdit, hash_bytes};

#[test]
fn canonical_source_and_projection_round_trip_without_touching_disk() {
    let file_id = FileId::new();
    let original = b"\\section{Intro}\nHello.\n".to_vec();
    let mut source =
        SourceBuffer::from_bytes(file_id, "main.tex", original.clone(), Revision::INITIAL);
    let mut parser = LatexParser::new().unwrap();
    let before = parser.parse(file_id, source.bytes()).unwrap();
    assert_eq!(before.source_hash, hash_bytes(&original));

    let start = original
        .windows(5)
        .position(|window| window == b"Hello")
        .unwrap();
    source
        .apply_edits(
            &[SourceEdit {
                file_id,
                start_byte: start,
                end_byte: start + 5,
                replacement: "World".into(),
                expected_slice_hash: hash_bytes(b"Hello"),
            }],
            Revision(1),
        )
        .unwrap();
    let after = parser.parse(file_id, source.bytes()).unwrap();
    assert!(core::projection_covers_source(
        source.bytes().len(),
        &after.projection
    ));
    assert_eq!(source.bytes(), b"\\section{Intro}\nWorld.\n");
}
