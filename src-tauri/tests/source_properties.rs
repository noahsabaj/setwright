use setwright_lib::core::{FileId, Revision, SourceBuffer, SourceEdit, hash_bytes};

#[test]
fn ten_thousand_randomized_edit_undo_pairs_restore_exact_bytes() {
    let file_id = FileId::new();
    let original = b"0123456789abcdefghijklmnopqrstuvwxyz\r\n".to_vec();
    let mut source =
        SourceBuffer::from_bytes(file_id, "main.tex", original.clone(), Revision::INITIAL);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut revision = 0u64;

    for iteration in 0..10_000usize {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let length = source.bytes().len();
        let start = (state as usize) % (length + 1);
        state = state.rotate_left(19) ^ iteration as u64;
        let end = start + ((state as usize) % (length - start + 1));
        let removed = source.bytes()[start..end].to_vec();
        let replacement = format!("x{:x}", state & 0xff);

        revision += 1;
        source
            .apply_edits(
                &[SourceEdit {
                    file_id,
                    start_byte: start,
                    end_byte: end,
                    replacement: replacement.clone(),
                    expected_slice_hash: hash_bytes(&removed),
                }],
                Revision(revision),
            )
            .unwrap();

        revision += 1;
        source
            .apply_edits(
                &[SourceEdit {
                    file_id,
                    start_byte: start,
                    end_byte: start + replacement.len(),
                    replacement: String::from_utf8(removed).unwrap(),
                    expected_slice_hash: hash_bytes(replacement.as_bytes()),
                }],
                Revision(revision),
            )
            .unwrap();
        assert_eq!(source.bytes(), original, "failed at iteration {iteration}");
    }
}

#[test]
fn rejected_batch_does_not_partially_apply_an_earlier_valid_edit() {
    let file_id = FileId::new();
    let mut source = SourceBuffer::from_bytes(
        file_id,
        "main.tex",
        b"alpha beta".to_vec(),
        Revision::INITIAL,
    );
    let before = source.bytes().to_vec();
    let result = source.apply_edits(
        &[
            SourceEdit {
                file_id,
                start_byte: 0,
                end_byte: 5,
                replacement: "ALPHA".into(),
                expected_slice_hash: hash_bytes(b"alpha"),
            },
            SourceEdit {
                file_id,
                start_byte: 6,
                end_byte: 10,
                replacement: "BETA".into(),
                expected_slice_hash: hash_bytes(b"stale"),
            },
        ],
        Revision(1),
    );
    assert!(result.is_err());
    assert_eq!(source.bytes(), before);
    assert_eq!(source.revision(), Revision::INITIAL);
}
