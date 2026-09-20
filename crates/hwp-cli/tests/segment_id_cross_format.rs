//! Segment ids must not depend on which reader produced the IR.
//!
//! D-02's whole point: an editor persists a segment id and expects it to survive a format
//! change. `fixtures/pdf-parity/public/source/public-safety-rfp-p1.{hwp,hwpx}` is the same
//! one-page document saved by Hangul in both formats, and it is committed, so this test fails
//! loudly rather than skipping.
//!
//! The trap this pins: the two readers store equivalent content in different shapes, so hashing
//! the IR as stored splits the id by source format. Two such shapes exist today.
//!
//! 1. The trailing `CharCtrl(13)` paragraph-break terminator, which HWP5's `PARA_TEXT` stores
//!    and HWPX's `<hp:t>` does not. **This is the one this fixture exercises.**
//! 2. `Paragraph::char_shape_runs`: the HWPX reader suppresses a run whose shape id repeats the
//!    previous one and overwrites a run at the same WCHAR position
//!    (`crates/hwpx/src/read/section.rs`, `if last_shape != Some(id)`), while the HWP5 reader
//!    preserves every `PARA_CHAR_SHAPE` entry verbatim (`crates/hwp5/src/body_text.rs`). This
//!    fixture happens not to contain a redundant run, so the rule is pinned directly by a unit
//!    test in `crates/hwp-convert/src/segment_id.rs` rather than relying on a fixture.
//!
//! Both readers are right about their own format; the id rule is what has to canonicalize,
//! which is why `hwp_convert::segment_id` normalizes before hashing instead of either reader
//! changing.
//!
//! The end-to-end envelope-level hwp5-vs-hwpx assertion belongs to 05-05. This file pins the
//! derivation itself, which is what 05-02 shipped.

use std::path::PathBuf;

use hwp_convert::segment_id::{SegmentPath, canonical_char_shape_runs, paragraph_id, run_id};
use hwp_model::{Document, Paragraph};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pdf-parity/public/source")
        .join(rel)
}

/// Every paragraph id in the document, in document order, plus every run id under it.
fn all_ids(doc: &Document) -> Vec<String> {
    let mut ids = Vec::new();
    for (section, sec) in doc.sections.iter().enumerate() {
        for (index, paragraph) in sec.paragraphs.iter().enumerate() {
            let path = SegmentPath {
                section,
                indices: vec![index],
            };
            ids.push(paragraph_id(&path, paragraph));
            for run in 0..canonical_char_shape_runs(paragraph).len() {
                ids.push(run_id(&path, paragraph, run));
            }
        }
    }
    ids
}

#[test]
fn the_same_document_derives_the_same_ids_from_hwp5_and_hwpx() {
    let hwp5 = hwp5::read_document(&fixture("public-safety-rfp-p1.hwp"))
        .expect("read the committed .hwp")
        .document;
    let hwpx = hwpx::read_document(&fixture("public-safety-rfp-p1.hwpx"))
        .expect("read the committed .hwpx")
        .document;

    let from_hwp5 = all_ids(&hwp5);
    let from_hwpx = all_ids(&hwpx);

    assert!(!from_hwp5.is_empty(), "the fixture must produce segments");
    assert_eq!(
        from_hwp5.len(),
        from_hwpx.len(),
        "the two readers must yield the same number of segments after canonicalization"
    );
    let mismatch = from_hwp5
        .iter()
        .zip(from_hwpx.iter())
        .position(|(a, b)| a != b);
    if let Some(at) = mismatch {
        panic!(
            "segment id {at} differs by source format, so a persisted editor key would not \
             survive an hwp-to-hwpx conversion:\n  from .hwp : {}\n  from .hwpx: {}",
            from_hwp5[at], from_hwpx[at]
        );
    }
}

/// The readers really do produce different IR for this fixture. Without this, the test above
/// could pass for the uninteresting reason that the two IRs happen to be identical, and the
/// canonicalization would be untested here.
///
/// For this fixture the divergence is the trailing `CharCtrl(13)` paragraph-break terminator
/// that HWP5's `PARA_TEXT` stores and HWPX's `<hp:t>` does not. The run-list divergence the
/// readers can also produce is pinned directly as a unit test in
/// `crates/hwp-convert/src/segment_id.rs`, which names the rule instead of relying on a fixture
/// continuing to contain a redundant run.
#[test]
fn the_two_readers_really_do_produce_different_ir_here() {
    let hwp5 = hwp5::read_document(&fixture("public-safety-rfp-p1.hwp"))
        .expect("read the committed .hwp")
        .document;
    let hwpx = hwpx::read_document(&fixture("public-safety-rfp-p1.hwpx"))
        .expect("read the committed .hwpx")
        .document;

    let paragraphs = |doc: &Document| -> Vec<Paragraph> {
        doc.sections
            .iter()
            .flat_map(|s| s.paragraphs.iter().cloned())
            .collect()
    };
    assert_ne!(
        paragraphs(&hwp5),
        paragraphs(&hwpx),
        "this fixture no longer exercises any reader divergence, so the cross-format test \
         above proves nothing; pick a fixture that does"
    );
}
