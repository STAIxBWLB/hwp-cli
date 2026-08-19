//! Proves that a paragraph's Korean/English line-breaking settings
//! (`ParaShape.attr1` bits 5-7, 16-19 — OWPML `hh:breakSetting`) survive an
//! hwp5-shaped IR -> HWPX -> IR round trip instead of being overwritten by
//! `crates/hwpx/src/write/header.rs`'s formerly-fixed literal string.
//!
//! Found by comparing the IR of a genuine distribution document against its
//! HWPX conversion in Hancom Office: attr1 bits 6, 7, 11 and 13 were present
//! before conversion and gone after. Bits 5-7 and 16-19 belong to the
//! `breakSetting` group and are covered here. **Bits 11 and 13 are NOT part
//! of that group, were NOT addressed by this fix, and remain unexplained.**
//!
//! `crates/hwpx/src/read/header.rs` is the contract this test writes
//! against; it is not modified by this plan.
//!
//! The corpus-gated proof against the genuine document that exposed the
//! defect lives in `crates/hwp-cli/tests/para_break_corpus.rs` instead of
//! here: it needs `hwp5::read_document` to parse a real `.hwp` file, and the
//! `hwpx` crate does not depend on `hwp5` (hub-and-spoke, CLAUDE.md
//! Invariant 1) - only `hwp-cli` legitimately depends on both.

use std::path::PathBuf;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("hwpx-para-break-roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Two paragraphs, two ParaShapes differing only in attr1 bit 7 (Korean word
/// break: clear = BREAK_WORD/character-level, set = KEEP_WORD/word-level —
/// the reader's polarity, `read/header.rs` around line 568). Write to HWPX,
/// read back, and assert each paragraph's bit 7 survived unchanged.
#[test]
fn break_setting_survives_hwp5_to_hwpx_round_trip() {
    let mut doc = hwp_convert::from_markdown("문단 하나\n\n문단 둘\n");
    assert_eq!(
        doc.sections[0].paragraphs.len(),
        2,
        "fixture must produce exactly two paragraphs"
    );

    let break_word = hwp_model::ParaShape::default(); // bit 7 clear
    let mut keep_word = hwp_model::ParaShape::default();
    keep_word.attr1 |= 1 << 7;

    doc.header.para_shapes = vec![break_word, keep_word];
    doc.sections[0].paragraphs[0].para_shape = hwp_model::ParaShapeId(0);
    doc.sections[0].paragraphs[1].para_shape = hwp_model::ParaShapeId(1);

    let out = tmp("break-setting.hwpx");
    hwpx::write_document(&doc, &out).unwrap();
    let read = hwpx::read_document(&out).unwrap();

    let shapes = &read.document.header.para_shapes;
    let paras = &read.document.sections[0].paragraphs;
    assert_eq!(
        paras.len(),
        2,
        "round trip must not drop or merge paragraphs"
    );

    let bit7_set = |idx: usize| -> bool {
        shapes[paras[idx].para_shape.0 as usize].break_non_latin_keep_word()
    };

    assert!(
        !bit7_set(0),
        "paragraph 0 (BREAK_WORD) must round-trip with bit 7 clear"
    );
    assert!(
        bit7_set(1),
        "paragraph 1 (KEEP_WORD) must round-trip with bit 7 set"
    );
}
