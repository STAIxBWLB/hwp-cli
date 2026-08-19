//! Corpus-gated proof that the HWPX writer's `hh:breakSetting` fix
//! (`crates/hwpx/src/write/header.rs`) survives conversion of the genuine
//! distribution document that exposed the defect (`dist-01*.hwp`).
//!
//! Lives in `hwp-cli` rather than `hwpx` because it needs both
//! `hwp5::read_document` (parse the real `.hwp` file) and `hwpx` (convert
//! and read back), and `hwp5`/`hwpx` do not depend on each other
//! (hub-and-spoke, CLAUDE.md Invariant 1) - only `hwp-cli` legitimately
//! depends on both. The synthetic tracer test that does not need a real
//! `.hwp` file lives in `crates/hwpx/tests/para_break_roundtrip.rs`.
//!
//! Skips cleanly (never fails) when `HWP_CORPUS_DIR` is unset, mirroring
//! `crates/hwp5/tests/distdoc_corpus.rs`'s idiom. This suite cannot run in
//! CI - the ground-truth corpus is never committed (CLAUDE.md Data policy).
//!
//! ```text
//! HWP_CORPUS_DIR=~/Documents/hwp_samples cargo test -p hwp-cli --test para_break_corpus -- --nocapture
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("HWP_CORPUS_DIR").map(PathBuf::from)
}

/// Mirrors `distdoc_corpus.rs`'s `skip_if_no_corpus` - never panics when the
/// corpus is absent, since CI always runs in that state.
fn skip_if_no_corpus() -> Option<PathBuf> {
    match corpus_dir() {
        Some(dir) if dir.is_dir() => Some(dir),
        _ => {
            eprintln!(
                "skip: HWP_CORPUS_DIR not set - only verifiable against the genuine \
                 distribution-document corpus (see ~/Documents/hwp_samples/README.md)"
            );
            None
        }
    }
}

/// `dist-01*.hwp` inside `dir`, stably ordered. Corpus filenames carry a
/// version and the source document title, so never assume a bare
/// `dist-01.hwp`.
fn dist01_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("HWP_CORPUS_DIR is a readable directory (checked by skip_if_no_corpus)")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("dist-01") && n.ends_with(".hwp"))
        })
        .collect();
    files.sort();
    files
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("hwp-cli-para-break-corpus");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// attr1 bits 5-7 (breakLatinWord + breakNonLatinWord) and 16-19
/// (widowOrphan, keepWithNext, keepLines, pageBreakBefore) - the
/// `breakSetting` group this plan covers. Bits 11 and 13 were also observed
/// lost on this same document but are NOT part of this group; whether they
/// are recoverable through OWPML at all is unestablished, and this test
/// does not check them.
const BREAK_SETTING_MASK: u32 = (0b111 << 5) | (0b1111 << 16);

/// Converts the genuine distribution document that exposed the defect to
/// HWPX and back, and asserts every paragraph's breakSetting bits survive.
#[test]
fn genuine_distribution_document_break_setting_survives_conversion() {
    let Some(dir) = skip_if_no_corpus() else {
        return;
    };
    let files = dist01_files(&dir);
    assert!(
        !files.is_empty(),
        "HWP_CORPUS_DIR is set ({}) but no dist-01*.hwp file was found there",
        dir.display()
    );
    let path = &files[0];

    let orig = hwp5::read_document(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .document;

    let out = tmp("dist-01-roundtrip.hwpx");
    hwpx::write_document(&orig, &out).unwrap();
    let roundtripped = hwpx::read_document(&out).unwrap().document;

    let orig_paras: Vec<_> = orig.sections.iter().flat_map(|s| &s.paragraphs).collect();
    let rt_paras: Vec<_> = roundtripped
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .collect();
    let compared = orig_paras.len().min(rt_paras.len());
    assert!(
        compared > 0,
        "genuine document must have at least one paragraph"
    );

    let default_ps = hwp_model::ParaShape::default();
    let mut distinct_rt_values = BTreeSet::new();
    let mut mismatches = Vec::new();
    for i in 0..compared {
        let orig_bits = orig
            .header
            .para_shapes
            .get(orig_paras[i].para_shape.0 as usize)
            .unwrap_or(&default_ps)
            .attr1
            & BREAK_SETTING_MASK;
        let rt_bits = roundtripped
            .header
            .para_shapes
            .get(rt_paras[i].para_shape.0 as usize)
            .unwrap_or(&default_ps)
            .attr1
            & BREAK_SETTING_MASK;
        distinct_rt_values.insert(rt_bits);
        if orig_bits != rt_bits {
            mismatches.push((i, orig_bits, rt_bits));
        }
    }

    // Prints the masked value(s) so a vacuous pass (round trip happens to
    // always land on the writer's old fixed default, coincidentally
    // matching a uniform source document) is distinguishable from a real
    // one in the output - see the per-paragraph mismatch assertion below
    // for the actual proof, which does not depend on the values varying.
    eprintln!(
        "{compared} paragraphs compared against {} ({} distinct breakSetting value(s) after round trip: {:#x?})",
        path.display(),
        distinct_rt_values.len(),
        distinct_rt_values,
    );

    assert!(
        mismatches.is_empty(),
        "breakSetting bits diverged after round trip: {mismatches:?}"
    );
}
