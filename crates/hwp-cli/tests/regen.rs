//! 신규 생성 동등성 테스트 — fixtures/samples/report-tables.hwpx를 JSON IR 경로로
//! 재생성(convert → json → new --from json)해 원본과 동등한지 검증한다(픽스처 하드 의존).
//!
//! 동등성 계약: ① convert/new/validate 정상 종료 ② `hwp cat` stdout 전문 동일(텍스트)
//! ③ 표 지도 동일 — 표별 rows/cols/attr/배치(placement)/안여백/행별 셀 수 + 셀별
//! (row,col) 키의 span·크기·여백·테두리 (구조) ④ secPr 슬라이스 바이트 동일(Gap B)
//! ⑤ tabProperties 슬라이스 바이트 동일(Gap C) ⑥ 각주/미주 정품 형태(Gap A).
//! 범위 외(의도된 차이): linesegarray(한글 재계산), PrvImage/settings(패키지 보조
//! 엔트리 — JSON IR 무손실 범위 밖). 렌더 픽셀 대조는 lineseg 재계산 리플로우로
//! 결정적이지 않아 수동 실기 게이트에 맡긴다.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

fn fixture() -> PathBuf {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    assert!(p.exists(), "커밋된 픽스처 없음: {}", p.display());
    p
}

fn tmp(name: &str) -> PathBuf {
    // 프로세스별 고유 디렉토리 — 병렬 test runner 간 산출물 경합 방지.
    let dir = std::env::temp_dir().join(format!("hwp-cli-regen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn cat(path: &PathBuf) -> String {
    let out = hwp().arg("cat").arg(path).output().unwrap();
    assert!(out.status.success(), "cat 실패: {}", path.display());
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn zip_slice(path: &PathBuf, entry: &str, open: &str, close: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    zip.by_name(entry).unwrap().read_to_string(&mut s).unwrap();
    let i = s
        .find(open)
        .unwrap_or_else(|| panic!("{open} 없음 ({entry})"));
    let j = s
        .find(close)
        .unwrap_or_else(|| panic!("{close} 없음 ({entry})"))
        + close.len();
    s[i..j].to_string()
}

/// 재생성 경로의 최종 산출물 (fixture → json → regen.hwpx). 한 번만 생성해 공유한다.
fn regen() -> &'static (PathBuf, PathBuf) {
    static REGEN: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    REGEN.get_or_init(|| {
        let json = tmp("fixture.json");
        // --embed-bin: 본문 BinData(이미지)까지 JSON에 실어 일반 문서에서도 무손실.
        let c = hwp()
            .arg("convert")
            .arg(fixture())
            .arg("--embed-bin")
            .arg("-o")
            .arg(&json)
            .status()
            .unwrap();
        assert!(c.success(), "convert → json");
        let regen = tmp("regen.hwpx");
        let n = hwp()
            .arg("new")
            .arg("--from")
            .arg(&json)
            .arg("-o")
            .arg(&regen)
            .status()
            .unwrap();
        assert!(n.success(), "new --from json");
        (json, regen)
    })
}

#[test]
fn regen_validate_and_cat_identical() {
    let regen = &regen().1;
    let v = hwp().arg("validate").arg(regen).output().unwrap();
    assert!(
        v.status.success(),
        "regen validate: {}",
        String::from_utf8_lossy(&v.stderr)
    );
    assert_eq!(
        cat(&fixture()),
        cat(regen),
        "hwp cat stdout은 원본과 전문 동일해야"
    );
}

/// 표 하나의 지도 항목 — 구조·배치의 동등성 대상 필드 전부.
/// 셀은 (row,col) 키로 정렬해 위치별 span·크기·여백·테두리까지 비교한다
/// (멀티셋 비교는 배치가 뒤바뀌어도 통과하는 사각지대가 있었다).
type CellEntry = (u16, u16, u16, u16, i32, i32, [u16; 4], u16);

#[derive(Debug, PartialEq)]
struct TableMapEntry {
    rows: u16,
    cols: u16,
    attr: u32,
    cell_spacing: u16,
    inner_margins: [u16; 4],
    row_cell_counts: Vec<u16>,
    border_fill: u16,
    placement: Option<hwp_model::GsoPlacement>,
    /// (row, col, col_span, row_span, width, height, margins, borderFill)
    cells: Vec<CellEntry>,
}

/// 표 지도: 재귀 순서로 TableMapEntry가 동일해야 한다.
#[test]
fn regen_table_map_identical() {
    let regen = &regen().1;
    fn table_map(path: &Path) -> Vec<TableMapEntry> {
        let doc = hwpx::read_document(path).unwrap().document;
        fn walk<'a>(paras: &'a [hwp_model::Paragraph], out: &mut Vec<&'a hwp_model::Table>) {
            for p in paras {
                for c in &p.controls {
                    match c {
                        hwp_model::Control::Table(t) => {
                            out.push(t);
                            for cell in &t.cells {
                                walk(&cell.paragraphs, out);
                            }
                        }
                        hwp_model::Control::Generic(g) => {
                            for l in &g.paragraph_lists {
                                walk(&l.paragraphs, out);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut tables = Vec::new();
        walk(&doc.sections[0].paragraphs, &mut tables);
        tables
            .iter()
            .map(|t| {
                let mut cells: Vec<_> = t
                    .cells
                    .iter()
                    .map(|c| {
                        (
                            c.row,
                            c.col,
                            c.col_span,
                            c.row_span,
                            c.width.0,
                            c.height.0,
                            c.margins,
                            c.border_fill.0,
                        )
                    })
                    .collect();
                cells.sort_unstable();
                TableMapEntry {
                    rows: t.rows,
                    cols: t.cols,
                    attr: t.attr,
                    cell_spacing: t.cell_spacing,
                    inner_margins: t.inner_margins,
                    row_cell_counts: t.row_cell_counts.clone(),
                    border_fill: t.border_fill.0,
                    placement: t.placement.clone(),
                    cells,
                }
            })
            .collect()
    }
    let src_map = table_map(&fixture());
    let regen_map = table_map(regen);
    assert_eq!(src_map.len(), regen_map.len(), "표 개수 동일");
    assert_eq!(
        src_map, regen_map,
        "표 지도(rows/cols·attr·배치·셀 좌표별 구조) 동일"
    );
}

#[test]
fn regen_secpr_and_tabpr_byte_identical() {
    let regen = &regen().1;
    // Gap B 게이트: secPr 슬라이스 바이트 동일.
    assert_eq!(
        zip_slice(
            &fixture(),
            "Contents/section0.xml",
            "<hp:secPr",
            "</hp:secPr>"
        ),
        zip_slice(regen, "Contents/section0.xml", "<hp:secPr", "</hp:secPr>"),
        "secPr 바이트 동일"
    );
    // Gap C 게이트: tabProperties 슬라이스 바이트 동일.
    assert_eq!(
        zip_slice(
            &fixture(),
            "Contents/header.xml",
            "<hh:tabProperties",
            "</hh:tabProperties>"
        ),
        zip_slice(
            regen,
            "Contents/header.xml",
            "<hh:tabProperties",
            "</hh:tabProperties>"
        ),
        "tabProperties 바이트 동일"
    );
}

/// secPr raw 보존 가드: JSON에서 PageDef를 수정하면 raw가 수정값을 조용히 무시하지 않고
/// 의미 경로(write_default_sec_pr)로 폴백해 수정값이 반영돼야 한다.
#[test]
fn regen_pagedef_edit_overrides_raw() {
    let json_path = &regen().0;
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(json_path).unwrap()).unwrap();
    let mut patched = false;
    for para in doc["sections"][0]["paragraphs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
    {
        for ctl in para["controls"].as_array_mut().unwrap().iter_mut() {
            if let Some(sd) = ctl.get_mut("SectionDef")
                && let Some(w) = sd.get_mut("page").and_then(|p| p.get_mut("width"))
            {
                *w = serde_json::json!(50000);
                patched = true;
            }
        }
    }
    assert!(patched, "SectionDef.page.width 패치 실패");
    let edited = tmp("edited.json");
    std::fs::write(&edited, serde_json::to_string(&doc).unwrap()).unwrap();
    let out = tmp("edited.hwpx");
    let n = hwp()
        .arg("new")
        .arg("--from")
        .arg(&edited)
        .arg("-o")
        .arg(&out)
        .status()
        .unwrap();
    assert!(n.success(), "new --from edited.json");
    let secpr = zip_slice(&out, "Contents/section0.xml", "<hp:secPr", "</hp:secPr>");
    assert!(
        secpr.contains(r#"width="50000""#),
        "수정된 페이지 폭이 반영돼야 (raw 무시 금지): {}",
        &secpr[..secpr.len().min(300)]
    );
}

/// Gap A 게이트: 각주/미주가 한글 정품 형태(number/suffixChar/instId + 본문 autoNum)로
/// 재생성돼야 한다 — 이 픽스처는 각주 2·미주 1을 포함한다.
#[test]
fn regen_footnotes_reference_shape() {
    let regen = &regen().1;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(regen).unwrap()).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    for needle in [
        r##"<hp:footNote number="1" suffixChar="41" instId="##,
        r##"<hp:footNote number="2" suffixChar="41" instId="##,
        r##"<hp:endNote number="1" suffixChar="41" instId="##,
        r##"<hp:autoNum num="1" numType="FOOTNOTE">"##,
        r##"<hp:autoNum num="2" numType="FOOTNOTE">"##,
        r##"<hp:autoNum num="1" numType="ENDNOTE">"##,
    ] {
        assert!(xml.contains(needle), "각주/미주 정품 형태: {needle}");
    }
    // 노트 본문 텍스트 왕복.
    let text = cat(regen);
    for t in ["각주 예시 1", "각주 예시 2", "미주 예시 1"] {
        assert!(text.contains(t), "노트 본문: {t}");
    }
}
