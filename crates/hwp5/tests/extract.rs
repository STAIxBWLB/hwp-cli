//! fixture 기반 통합 테스트: 파싱 무결성 + 텍스트 추출 스냅샷.

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/hwp5")
        .join(name)
}

#[path = "../../hwp-cli/tests/common/fixture_skip.rs"]
mod fixture_skip;

/// Fixture binaries are not in the repository (local only - `fixtures/README.md`). `true` means
/// skip (the test returns), and the skip is counted (#275). Fixtures are provisioned all or
/// nothing, so one representative file is checked.
#[track_caller]
fn skip_if_no_fixtures() -> bool {
    fixture_skip::fixture_missing(&fixture("hello_world.hwp"))
}

const ALL_FIXTURES: &[&str] = &[
    "hello_world.hwp",
    "bookmark.hwp",
    "color_fill.hwp",
    "outline.hwp",
    "work_report.hwp",
    "annual_report.hwp",
];

/// 모든 fixture가 경고 없이 파싱되어야 한다.
/// (경고는 곧 분류표/레이아웃 가정의 오류 신호다)
#[test]
fn 전체_fixture_경고_없이_파싱() {
    if skip_if_no_fixtures() {
        return;
    }
    for name in ALL_FIXTURES {
        let result = hwp5::read_document(&fixture(name)).expect(name);
        assert!(
            result.warnings.is_empty(),
            "{name}에서 경고 발생:\n{}",
            result.warnings.join("\n")
        );
    }
}

/// 소형 fixture의 추출 텍스트 스냅샷.
#[test]
fn 텍스트_추출_스냅샷() {
    if skip_if_no_fixtures() {
        return;
    }
    for name in ["hello_world.hwp", "work_report.hwp"] {
        let result = hwp5::read_document(&fixture(name)).unwrap();
        insta::assert_snapshot!(
            format!("text_{}", name.trim_end_matches(".hwp")),
            result.document.plain_text()
        );
    }
}

/// 빈 문서(컨트롤만 있는 문서)는 빈 텍스트여야 한다 — PrvText로 교차 확인됨.
#[test]
fn 빈_문서_추출() {
    if skip_if_no_fixtures() {
        return;
    }
    for name in ["bookmark.hwp", "color_fill.hwp", "outline.hwp"] {
        let result = hwp5::read_document(&fixture(name)).unwrap();
        assert_eq!(result.document.plain_text().trim(), "", "{name}");
    }
}

/// 정품 bookmark.hwp의 책갈피(bokm) 읽기 — 이름이 정확히 복원돼야 한다(정답지).
#[test]
fn 책갈피_읽기() {
    if skip_if_no_fixtures() {
        return;
    }
    let result = hwp5::read_document(&fixture("bookmark.hwp")).unwrap();
    let bms = hwp_convert::list_bookmarks(&result.document);
    assert_eq!(bms.len(), 1, "책갈피 1개: {bms:?}");
    assert_eq!(bms[0].name, "책갈피테스트");
}

/// 대형 문서: 구조 통계와 핵심 내용 불변식 (전체 스냅샷은 과대).
#[test]
fn annual_report_불변식() {
    if skip_if_no_fixtures() {
        return;
    }
    let result = hwp5::read_document(&fixture("annual_report.hwp")).unwrap();
    let doc = &result.document;
    let text = doc.plain_text();

    assert_eq!(doc.sections.len(), 1);
    assert!(text.contains("Annual Report 2012"), "글상자 안 텍스트 수집");
    assert!(text.contains("Financial Highlights"));
    assert!(text.len() > 5_000, "실제 길이: {}", text.len());
    // 문자 모양/글꼴 테이블이 채워져야 한다
    assert!(!doc.header.char_shapes.is_empty());
    assert!(!doc.header.fonts[0].is_empty());
}

/// 문서 헤더 파싱 검증: hello_world의 알려진 값들.
#[test]
fn hello_world_헤더() {
    if skip_if_no_fixtures() {
        return;
    }
    let result = hwp5::read_document(&fixture("hello_world.hwp")).unwrap();
    let doc = &result.document;

    assert_eq!(doc.meta.source_version, "5.1.0.1");
    assert_eq!(doc.header.properties.section_count, 1);
    // 7개 언어 슬롯 × 글꼴 2종 (실측: FACE_NAME 14개)
    for fonts in &doc.header.fonts {
        assert_eq!(fonts.len(), 2);
    }
    assert_eq!(doc.header.char_shapes.len(), 7);

    // 첫 문단: 구역/단 정의 컨트롤 2개 + "Hello World!" + 문단끝
    let para = &doc.sections[0].paragraphs[0];
    assert_eq!(para.controls.len(), 2);
    assert_eq!(para.controls[0].ctrl_id(), *b"secd");
    assert_eq!(para.controls[1].ctrl_id(), *b"cold");
    assert_eq!(para.plain_text(), "Hello World!");
    // 줄 배치 정보 1줄
    assert_eq!(para.line_segs.len(), 1);

    // PAGE_DEF: A4 (210mm × 297mm)
    let page = doc.sections[0].section_def().unwrap().page.unwrap();
    assert_eq!(page.width.0, 59528);
    assert_eq!(page.height.0, 84186);
}

/// 표 구조 검증: work_report의 표.
#[test]
fn work_report_표_구조() {
    if skip_if_no_fixtures() {
        return;
    }
    let result = hwp5::read_document(&fixture("work_report.hwp")).unwrap();
    let doc = &result.document;

    let tables: Vec<_> = doc
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .filter_map(|c| match c {
            hwp_model::Control::Table(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(!tables.is_empty(), "표가 있어야 한다");

    for t in &tables {
        // 행별 셀 수의 합 == 실제 셀 수 (실측으로 확정한 "Row Size" 해석 검증)
        let expected: u32 = t.row_cell_counts.iter().map(|&c| u32::from(c)).sum();
        assert_eq!(
            t.cells.len() as u32,
            expected,
            "rows={} cols={}",
            t.rows,
            t.cols
        );
        // 모든 셀의 row/col이 표 범위 안
        for cell in &t.cells {
            assert!(cell.row < t.rows && cell.col < t.cols);
        }
    }
}
