//! Corpus-gated proof that GATE-01's distribution-document read path works
//! against genuine Hancom-authored files, and that `DISTRIBUTE_DOC_DATA`
//! lives where `crates/hwp5/src/distdoc.rs` expects it.
//!
//! These tests read `HWP_CORPUS_DIR`, glob `dist-*.hwp` inside it, and skip
//! cleanly (never fail) when the variable is unset, mirroring `identity.rs`'s
//! `skip_if_no_fixtures()` idiom for `fixtures/hwp5/`. Run locally with:
//!
//! ```text
//! HWP_CORPUS_DIR=~/Documents/hwp_samples cargo test -p hwp5 --test distdoc_corpus
//! ```
//!
//! **This suite cannot run in continuous integration**: the ground-truth
//! corpus lives outside the repository and is never committed (CLAUDE.md
//! §Data policy). Its pass/fail state must be recorded by hand in the phase's
//! verification notes (`02-VALIDATION.md`), since a green CI run alone cannot
//! attest to it.

use std::path::{Path, PathBuf};

use hwp5::record::{RecordHeader, RecordNode, ScanMode, scan_stream, tag};

fn corpus_dir() -> Option<PathBuf> {
    std::env::var_os("HWP_CORPUS_DIR").map(PathBuf::from)
}

/// `HWP_CORPUS_DIR`이 존재하는 디렉터리를 가리키면 그 경로를 반환하고,
/// 아니면 스킵 안내를 stderr에 남기고 `None`을 반환한다. 미설정 상태에서도
/// 패닉하지 않는다 — CI에는 코퍼스가 없어 이 상태로 항상 green이어야 한다.
fn skip_if_no_corpus() -> Option<PathBuf> {
    match corpus_dir() {
        Some(dir) if dir.is_dir() => Some(dir),
        _ => {
            eprintln!(
                "스킵: HWP_CORPUS_DIR 미설정 — 진품 배포용 문서 코퍼스로만 검증 가능 \
                 (~/Documents/hwp_samples/README.md 참고)"
            );
            None
        }
    }
}

/// `dist-*.hwp` 파일을 안정적인 순서로 나열한다.
fn dist_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("HWP_CORPUS_DIR is a readable directory (checked by skip_if_no_corpus)")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("dist-") && n.ends_with(".hwp"))
        })
        .collect();
    files.sort();
    files
}

fn tree_contains_tag(nodes: &[RecordNode], target: u16) -> bool {
    nodes
        .iter()
        .any(|n| n.tag == target || tree_contains_tag(&n.children, target))
}

#[test]
fn every_corpus_distribution_document_reads() {
    let Some(dir) = skip_if_no_corpus() else {
        return;
    };
    let files = dist_files(&dir);
    assert!(
        !files.is_empty(),
        "HWP_CORPUS_DIR is set ({}) but no dist-*.hwp files were found there",
        dir.display()
    );

    for path in &files {
        let name = path.display().to_string();
        let result = hwp5::read_document(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            result.unwrapped_distribution,
            "{name}: expected the distribution flag to be set"
        );
        assert!(
            !result.document.sections.is_empty(),
            "{name}: expected at least one section"
        );
        let text: String = result.document.sections[0]
            .paragraphs
            .iter()
            .map(|p| p.plain_text())
            .collect();
        assert!(
            !text.trim().is_empty(),
            "{name}: expected non-empty text in the first section"
        );
        assert!(
            !result.warnings.iter().any(|w| w.contains("ViewText")),
            "{name}: unexpected ViewText warning(s): {:?}",
            result.warnings
        );
    }
    eprintln!(
        "every_corpus_distribution_document_reads: {} dist-*.hwp files verified: {:?}",
        files.len(),
        files
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy())
            .collect::<Vec<_>>()
    );
}

#[test]
fn distribute_doc_data_lives_at_the_head_of_view_text_not_doc_info() {
    let Some(dir) = skip_if_no_corpus() else {
        return;
    };
    let files = dist_files(&dir);
    assert!(
        !files.is_empty(),
        "HWP_CORPUS_DIR is set ({}) but no dist-*.hwp files were found there",
        dir.display()
    );

    for path in &files {
        let name = path.display().to_string();
        let mut container =
            hwp5::Hwp5Container::open(path).unwrap_or_else(|e| panic!("{name}: {e}"));

        // Every /ViewText/SectionN stream's raw (pre-decompression) bytes
        // begin with the DISTRIBUTE_DOC_DATA record at level 0, size 256.
        let view_text_sections = container.view_text_sections();
        assert!(
            !view_text_sections.is_empty(),
            "{name}: expected at least one /ViewText/SectionN stream"
        );
        for stream_path in &view_text_sections {
            let raw = container
                .read_stream_raw(stream_path)
                .unwrap_or_else(|e| panic!("{name} {stream_path}: {e}"));
            let mut reader = hwp5::codec::ByteReader::new(&raw);
            let header = RecordHeader::decode(&mut reader).unwrap_or_else(|e| {
                panic!("{name} {stream_path}: record header decode failed: {e}")
            });
            assert_eq!(
                header.tag,
                tag::DISTRIBUTE_DOC_DATA,
                "{name} {stream_path}: expected DISTRIBUTE_DOC_DATA (tag 0x{:03X}), observed tag 0x{:03X}",
                tag::DISTRIBUTE_DOC_DATA,
                header.tag
            );
            assert_eq!(
                header.level, 0,
                "{name} {stream_path}: expected level 0, observed {}",
                header.level
            );
            assert_eq!(
                header.size, 256,
                "{name} {stream_path}: expected size 256, observed {}",
                header.size
            );
        }

        // /DocInfo is completely ordinary in a distribution document and
        // never carries this tag — the DISTRIBUTION bit only changes what
        // lives under /ViewText/.
        let doc_info_raw = container
            .read_record_stream("/DocInfo")
            .unwrap_or_else(|e| panic!("{name}: /DocInfo read failed: {e}"));
        let scan = scan_stream(&doc_info_raw, ScanMode::Tolerant)
            .unwrap_or_else(|e| panic!("{name}: /DocInfo scan failed: {e}"));
        assert!(
            !tree_contains_tag(&scan.roots, tag::DISTRIBUTE_DOC_DATA),
            "{name}: /DocInfo unexpectedly contains a DISTRIBUTE_DOC_DATA (tag 0x{:03X}) record",
            tag::DISTRIBUTE_DOC_DATA
        );
    }
}
