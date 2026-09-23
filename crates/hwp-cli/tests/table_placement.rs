//! #296 표 배치 스위치(--table-placement / tables.placement / set_table_placement) 통합 테스트.
//!
//! 커밋된 픽스처 fixtures/samples/report-tables.hwpx의 표 지도(table_edit.rs 헤더 참조):
//! 표 10개 중 3개가 처음부터 부유(treatAsChar=0, flowWithText=1)이고 7개가 인라인이다.
//! 인라인으로 남는 treatAsChar="1" 하나는 표가 아니라 rect 도형의 것이다.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn fixture() -> PathBuf {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    assert!(p.exists(), "커밋된 픽스처가 없습니다: {}", p.display());
    p
}

fn tmp(name: &str) -> PathBuf {
    // PID 포함 — 병렬 cargo test 간 경로 충돌 방지(table_edit.rs와 같은 규칙).
    let dir = std::env::temp_dir().join(format!("hwp-cli-table-placement-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn run(args: &[&str]) -> std::process::Output {
    let out = hwp().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "hwp {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn section_xml(path: &Path) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut buf = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut buf)
        .unwrap();
    buf
}

/// `<hp:{tag}`를 연 뒤 첫 `<hp:pos .../>`를 개체별로 모은다(우리 writer는 pos를 개체
/// 서두에 쓰므로 생성 문서에서는 이 순서가 성립한다).
fn pos_attrs_after(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<hp:{tag} ");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        let pos = after.find("<hp:pos ").expect("개체의 hp:pos");
        let end = after[pos..].find("/>").expect("hp:pos 종료");
        out.push(after[pos..pos + end + 2].to_string());
        rest = &after[pos + end + 2..];
    }
    out
}

/// 1x1 PNG (그림 임베드용 최소 실물 파일).
const PNG_1X1: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 255, 255, 63, 0, 5,
    254, 2, 254, 167, 53, 129, 132, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

/// 표 2개 + 그림 1개를 만드는 markdown 소스를 쓰고 경로를 돌려준다.
fn md_source(name: &str) -> PathBuf {
    let img = tmp(&format!("{name}.png"));
    std::fs::write(&img, PNG_1X1).unwrap();
    let md = tmp(&format!("{name}.md"));
    std::fs::write(
        &md,
        format!(
            "# 제목\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n\n![도장]({})\n\n| A | B |\n|---|---|\n| x | y |\n",
            img.display()
        ),
    )
    .unwrap();
    md
}

#[test]
fn new_floating_표만_부유로_그림은_불변() {
    let md = md_source("floating");
    let out = tmp("floating.hwpx");
    run(&[
        "new",
        "-o",
        out.to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    let xml = section_xml(&out);
    let tables = pos_attrs_after(&xml, "tbl");
    assert_eq!(tables.len(), 2, "표 2개");
    for pos in &tables {
        assert!(pos.contains(r#"treatAsChar="0""#), "{pos}");
        assert!(pos.contains(r#"flowWithText="0""#), "{pos}");
        assert!(pos.contains(r#"allowOverlap="1""#), "{pos}");
        assert!(pos.contains(r#"vertRelTo="PARA""#), "{pos}");
        assert!(pos.contains(r#"horzRelTo="PARA""#), "{pos}");
    }
    let pics = pos_attrs_after(&xml, "pic");
    assert_eq!(pics.len(), 1, "그림 1개");
    assert!(
        pics[0].contains(r#"treatAsChar="1""#),
        "그림 배치는 불변: {}",
        pics[0]
    );
}

#[test]
fn new_기본과_inline은_바이트_동일() {
    let md = md_source("default");
    let default = tmp("default.hwpx");
    let inline = tmp("inline.hwpx");
    run(&[
        "new",
        "-o",
        default.to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
    ]);
    run(&[
        "new",
        "-o",
        inline.to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
        "--table-placement",
        "inline",
    ]);
    assert_eq!(
        std::fs::read(&default).unwrap(),
        std::fs::read(&inline).unwrap(),
        "명시적 inline은 기본 출력과 바이트 동일"
    );
    let xml = section_xml(&default);
    for pos in pos_attrs_after(&xml, "tbl") {
        assert!(pos.contains(r#"treatAsChar="1""#), "기본은 인라인: {pos}");
    }
}

#[test]
fn edit_전체_부유_전환_검증_텍스트_불변() {
    let out = tmp("flip-all.hwpx");
    run(&[
        "edit",
        fixture().to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    // validate 유효.
    let validated = run(&["validate", out.to_str().unwrap()]);
    assert!(
        String::from_utf8_lossy(&validated.stdout).contains("유효"),
        "validate: {}",
        String::from_utf8_lossy(&validated.stdout)
    );
    // 텍스트 추출은 입력과 동일.
    let before = hwp().arg("cat").arg(fixture()).output().unwrap().stdout;
    let after = hwp().arg("cat").arg(&out).output().unwrap().stdout;
    assert_eq!(before, after, "텍스트 추출 동일");
    // 표 10개 모두 부유 — info --body-stats 요약으로 확인.
    let info = run(&["info", out.to_str().unwrap(), "--body-stats"]);
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(
        stdout.contains("표:     10개 (인라인 0, 부유 10)"),
        "info --body-stats: {stdout}"
    );
}

#[test]
fn edit_한_표만_전환() {
    // #9(7x2 단순표)는 인라인 — 그 하나만 부유로.
    let out = tmp("flip-one.hwpx");
    run(&[
        "edit",
        fixture().to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--table-placement",
        "floating",
        "--table",
        "9",
    ]);
    let info = run(&["info", out.to_str().unwrap(), "--body-stats"]);
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(
        stdout.contains("표:     10개 (인라인 6, 부유 4)"),
        "info --body-stats: {stdout}"
    );
}

#[test]
fn edit_재적용은_바이트_동일() {
    let first = tmp("idem-1.hwpx");
    let second = tmp("idem-2.hwpx");
    run(&[
        "edit",
        fixture().to_str().unwrap(),
        "-o",
        first.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    run(&[
        "edit",
        first.to_str().unwrap(),
        "-o",
        second.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "두 번 적용해도 바이트 동일"
    );
}

#[test]
fn edit_ops_json으로_전환() {
    let ops = tmp("ops.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_table_placement","placement":"floating","table":9}]"#,
    )
    .unwrap();
    let out = tmp("ops.hwpx");
    run(&[
        "edit",
        fixture().to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--ops",
        ops.to_str().unwrap(),
    ]);
    let info = run(&["info", out.to_str().unwrap(), "--body-stats"]);
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(
        stdout.contains("표:     10개 (인라인 6, 부유 4)"),
        "info --body-stats: {stdout}"
    );
}

#[test]
fn edit_ops_json_잘못된_placement는_거부() {
    let ops = tmp("ops-bad.json");
    std::fs::write(
        &ops,
        r#"[{"op":"set_table_placement","placement":"hover"}]"#,
    )
    .unwrap();
    let out = tmp("ops-bad.hwpx");
    let result = hwp()
        .args([
            "edit",
            fixture().to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--ops",
            ops.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!result.status.success(), "스키마 거부로 실패해야 한다");
}

#[test]
fn hwp5_왕복() {
    // .hwp 기본 출력은 hwp5 writer의 부유 폴백을 따른다 — inline으로 뒤집으면
    // common_data 비트가 패치돼 hwpx 변환에서 treatAsChar=1로 읽혀야 한다.
    let md = md_source("hwp5");
    let hwp5_out = tmp("hwp5.hwp");
    run(&[
        "new",
        "-o",
        hwp5_out.to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
    ]);
    let flipped = tmp("hwp5-inline.hwp");
    run(&[
        "edit",
        hwp5_out.to_str().unwrap(),
        "-o",
        flipped.to_str().unwrap(),
        "--table-placement",
        "inline",
    ]);
    let as_hwpx = tmp("hwp5-inline.hwpx");
    run(&[
        "convert",
        flipped.to_str().unwrap(),
        "-o",
        as_hwpx.to_str().unwrap(),
    ]);
    let xml = section_xml(&as_hwpx);
    for pos in pos_attrs_after(&xml, "tbl") {
        assert!(
            pos.contains(r#"treatAsChar="1""#),
            "hwp5 common_data 비트 패치가 살아 있어야 한다: {pos}"
        );
    }
}

#[test]
fn compose_v2_tables_placement() {
    let spec = tmp("spec.json");
    std::fs::write(
        &spec,
        serde_json::json!({
            "version": "2.0",
            "document": {
                "version": "1.0",
                "sections": [{"blocks": [
                    {"type": "paragraph", "runs": [{"type": "text", "text": "앞"}]},
                    {"type": "table",
                     "columns": [{"width_mm": 40.0}, {"width_mm": 40.0}],
                     "rows": [{"cells": [
                         {"blocks": [{"type": "paragraph", "runs": [{"type": "text", "text": "1"}]}]},
                         {"blocks": [{"type": "paragraph", "runs": [{"type": "text", "text": "2"}]}]},
                     ]}]},
                ]}],
            },
            "tables": {"placement": "floating"},
        })
        .to_string(),
    )
    .unwrap();
    let out = tmp("spec.hwpx");
    run(&[
        "compose",
        spec.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    let xml = section_xml(&out);
    let tables = pos_attrs_after(&xml, "tbl");
    assert_eq!(tables.len(), 1);
    assert!(tables[0].contains(r#"treatAsChar="0""#), "{}", tables[0]);
    assert!(tables[0].contains(r#"flowWithText="0""#), "{}", tables[0]);
}

#[test]
fn hwp5_출력도_표_치수를_채운다() {
    // Codex review P1: placement가 채워진 표를 .hwp로 쓸 때 width/height 0이 그대로
    // 나가면 한글에서 개체가 접힌다 — writer가 셀 그리드 합산으로 폴백해야 한다.
    let md = md_source("hwp5dims");
    let hwp5_out = tmp("hwp5dims.hwp");
    run(&[
        "new",
        "-o",
        hwp5_out.to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    let as_hwpx = tmp("hwp5dims.hwpx");
    run(&[
        "convert",
        hwp5_out.to_str().unwrap(),
        "-o",
        as_hwpx.to_str().unwrap(),
    ]);
    let xml = section_xml(&as_hwpx);
    let mut rest = xml.as_str();
    let mut checked = 0;
    while let Some(at) = rest.find("<hp:tbl ") {
        let after = &rest[at..];
        let sz = after.find("<hp:sz ").expect("tbl의 hp:sz");
        let end = after[sz..].find("/>").expect("hp:sz 종료");
        let tag = &after[sz..sz + end];
        let width: i64 = tag
            .split(r#"width=""#)
            .nth(1)
            .and_then(|s| s.split('"').next())
            .and_then(|s| s.parse().ok())
            .expect("width 값");
        assert!(width > 0, "0 너비 표 개체 금지: {tag}");
        checked += 1;
        rest = &after[sz + end..];
    }
    assert_eq!(checked, 2, "표 2개 검사");
}

#[test]
fn json_입력도_placement를_적용() {
    // Codex review P2: --from doc.json --table-placement가 조용히 무시되면 안 된다.
    let md = md_source("jsonsrc");
    let json = tmp("jsonsrc.json");
    run(&[
        "new",
        "-o",
        tmp("jsonsrc.hwpx").to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
    ]);
    run(&[
        "convert",
        tmp("jsonsrc.hwpx").to_str().unwrap(),
        "-o",
        json.to_str().unwrap(),
    ]);
    let out = tmp("jsonsrc-floating.hwpx");
    run(&[
        "new",
        "-o",
        out.to_str().unwrap(),
        "--from",
        json.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    let xml = section_xml(&out);
    let tables = pos_attrs_after(&xml, "tbl");
    assert_eq!(tables.len(), 2, "표 2개");
    for pos in &tables {
        assert!(
            pos.contains(r#"treatAsChar="0""#),
            "JSON 입력 표도 전환: {pos}"
        );
    }
}

/// 부유 표 렌더 스모크 — 페이지가 생성되고 빈 페이지가 아니다(전부 흰 픽셀이면 실패).
/// write/section.rs:1350의 blank-page 실패 모드를 잡는다. 폰트 비의존.
#[test]
fn floating_표_렌더_스모크() {
    let md = md_source("render");
    let hwpx = tmp("render.hwpx");
    run(&[
        "new",
        "-o",
        hwpx.to_str().unwrap(),
        "--from",
        md.to_str().unwrap(),
        "--table-placement",
        "floating",
    ]);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/render-raster-formats/table-placement");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let output = dir.join("out.png");
    run(&[
        "render",
        "--dpi",
        "96",
        "-o",
        output.to_str().unwrap(),
        hwpx.to_str().unwrap(),
    ]);
    let page = dir
        .read_dir()
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
        .expect("렌더된 페이지");
    let image = image::open(&page).expect("decode png").to_rgba8();
    assert!(
        image.pixels().any(|p| p.0[..3] != [255, 255, 255]),
        "빈 페이지(전부 흰색)이면 안 된다"
    );
}
