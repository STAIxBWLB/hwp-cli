//! HWPX writer 테스트: 왕복 + 패키지 규칙.

use std::io::Read as _;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/hwpx")
        .join(name)
}

/// fixture 바이너리는 저장소에서 제외된다(로컬 전용). 없으면 `true`(스킵).
fn skip_if_no_fixtures() -> bool {
    if fixture("minimal.hwpx").exists() {
        return false;
    }
    eprintln!("스킵: fixtures 없음 (fixtures/hwpx/) — fixtures/README.md 참고");
    true
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("hwpx-write-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A complete 1x1 transparent PNG, including valid CRCs and an IEND chunk.
const VALID_PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x01, 0xe2, 0x21, 0xbc, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

/// hwpx → IR → hwpx → IR 왕복: 의미 동등성.
#[test]
fn 왕복_의미_동등() {
    if skip_if_no_fixtures() {
        return;
    }
    let original = hwpx::read_document(&fixture("minimal.hwpx"))
        .unwrap()
        .document;
    let out = tmp("roundtrip.hwpx");
    let warnings = hwpx::write_document(&original, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let reread = hwpx::read_document(&out).unwrap();
    assert!(reread.warnings.is_empty(), "{:?}", reread.warnings);
    let doc = reread.document;

    assert_eq!(doc.plain_text(), original.plain_text());
    assert_eq!(doc.sections.len(), original.sections.len());
    assert_eq!(
        doc.header.char_shapes.len(),
        original.header.char_shapes.len()
    );
    assert_eq!(
        doc.header
            .styles
            .iter()
            .map(|s| &s.name)
            .collect::<Vec<_>>(),
        original
            .header
            .styles
            .iter()
            .map(|s| &s.name)
            .collect::<Vec<_>>(),
    );
    // PageDef 보존
    let (a, b) = (
        original.sections[0].section_def().unwrap().page.unwrap(),
        doc.sections[0].section_def().unwrap().page.unwrap(),
    );
    assert_eq!(
        (a.width, a.height, a.margin_left),
        (b.width, b.height, b.margin_left)
    );
}

/// 패키지 규칙: mimetype이 첫 엔트리 + 무압축.
#[test]
fn 패키지_mimetype_규칙() {
    if skip_if_no_fixtures() {
        return;
    }
    let doc = hwpx::read_document(&fixture("minimal.hwpx"))
        .unwrap()
        .document;
    let out = tmp("package.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    let file = std::fs::File::open(&out).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let first = zip.by_index(0).unwrap();
    assert_eq!(first.name(), "mimetype");
    assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    drop(first);

    let mut mime = String::new();
    zip.by_name("mimetype")
        .unwrap()
        .read_to_string(&mut mime)
        .unwrap();
    assert_eq!(mime, "application/hwp+zip");
}

/// 같은 IR은 실행 시각과 출력 경로가 달라도 byte-identical HWPX여야 한다.
/// ZIP writer 기본값이 현재 시각으로 바뀌어도 회귀하지 않도록 DOS epoch도 직접 확인한다.
#[test]
fn 새_패키지는_시간과_경로에_무관하게_결정적() {
    let doc = hwp_convert::from_markdown("# 결정적 합성\n\n같은 입력은 같은 출력이어야 한다.\n");
    let first = tmp("deterministic-first.hwpx");
    let second = tmp("deterministic-second.hwpx");

    hwpx::write_document(&doc, &first).unwrap();
    // DOS timestamp 해상도(2초)를 넘겨 현재 시각 기반 기본값의 false green을 막는다.
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    hwpx::write_document(&doc, &second).unwrap();

    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "동일 IR의 HWPX package bytes"
    );

    for path in [&first, &second] {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        for index in 0..archive.len() {
            let entry = archive.by_index(index).unwrap();
            assert_eq!(
                entry.last_modified(),
                Some(zip::DateTime::default()),
                "{} timestamp",
                entry.name()
            );
        }
    }
}

/// markdown → hwpx → markdown 왕복: 구조 보존.
#[test]
fn markdown_생성_왕복() {
    let md = "# 제목\n\n본문 **굵게** 그리고 *기울임*.\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n";
    let doc = hwp_convert::from_markdown(md);
    let out = tmp("from_md.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let text = reread.plain_text();
    assert!(text.contains("제목"));
    assert!(text.contains("본문 굵게 그리고 기울임."));
    assert!(text.contains("1\t2"), "표 셀: {text:?}");

    // 헤딩 스타일과 서식 스팬이 md로 되돌아온다
    let md_out = hwp_convert::to_markdown(&reread);
    assert!(md_out.contains("# "), "{md_out}");
    assert!(md_out.contains("**굵게**"), "{md_out}");
    assert!(md_out.contains("*기울임*"), "{md_out}");
    assert!(md_out.contains("| 1 | 2 |"), "{md_out}");
}

#[test]
fn table_cell_header_and_vertical_alignment_round_trip() {
    let mut doc = hwp_convert::from_markdown("| Header |\n| --- |\n| Value |\n");
    let table = doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("generated table");
    table.cells[0].list_attr = (1 << 18) | (2 << 5);

    let out = tmp("table-cell-attributes.hwpx");
    hwpx::write_document(&doc, &out).unwrap();
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    let mut xml = String::new();
    archive
        .by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains(r#"header="1""#), "{xml}");
    assert!(xml.contains(r#"vertAlign="BOTTOM""#), "{xml}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let cell = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => table.cells.first(),
            _ => None,
        })
        .expect("round-tripped cell");
    assert!(cell.is_header());
    assert_eq!(cell.vert_align(), 2);
}

/// GI-3/GI-4 왕복: md(이미지+인라인 코드) → hwpx 저장 → 재읽기에서 Picture·bin·코드 글자모양 생존.
#[test]
fn md_이미지_코드_hwpx_왕복() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join("hwpx-md-imgcode");
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(16u32.to_be_bytes());
    png.extend(16u32.to_be_bytes());
    png.extend([0u8; 8]);
    let fig = dir.join("f.png");
    std::fs::File::create(&fig)
        .unwrap()
        .write_all(&png)
        .unwrap();

    let doc = hwp_convert::from_markdown_with(
        "본문 `let x = 1;` 코드와 이미지.\n\n![alt](f.png)\n",
        &hwp_convert::MarkdownImportOptions {
            base_dir: Some(&dir),
            preset: None,
            ..Default::default()
        },
    );
    let out = tmp("md_imgcode.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let has_pic = reread.sections[0].paragraphs.iter().any(|p| {
        p.controls
            .iter()
            .any(|c| matches!(c, hwp_model::Control::Picture(_)))
    });
    assert!(has_pic, "이미지 Picture 왕복");
    assert!(!reread.bin_streams.is_empty(), "bin_streams 왕복");
    let code_ids: std::collections::HashSet<u16> = reread
        .header
        .char_shapes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.face_ids[0] == 1)
        .map(|(i, _)| i as u16)
        .collect();
    assert!(!code_ids.is_empty(), "코드 글자모양(함초롬돋움) 왕복");
    assert!(
        reread.sections[0].paragraphs.iter().any(|p| p
            .char_shape_runs
            .iter()
            .any(|(_, id)| code_ids.contains(&id.0))),
        "코드 run 왕복"
    );
}

/// GE-9: 부유(글 앞) 그림의 세로/가로 오프셋·z-order가 hwpx `<hp:pos>`/zOrder에
/// 방출된다. hwp5 read가 실값을 승계하면(parse_picture_gso) writer가 그대로 쓴다 —
/// 이전엔 0 하드코딩이라 떠 있는 그림이 좌상단(offset 0)에 뭉쳤다.
#[test]
fn 부유_그림_배치_hwpx_방출() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join("hwpx-ge9-float");
    std::fs::create_dir_all(&dir).unwrap();
    let fig = dir.join("g.png");
    std::fs::File::create(&fig)
        .unwrap()
        .write_all(VALID_PNG_1X1)
        .unwrap();

    let mut doc = hwp_convert::from_markdown_with(
        "이미지.\n\n![alt](g.png)\n",
        &hwp_convert::MarkdownImportOptions {
            base_dir: Some(&dir),
            preset: None,
            ..Default::default()
        },
    );
    // 그림을 부유(글 앞) + 오프셋·z-order로 바꾼다(hwp5 read가 승계했을 값을 모사).
    for para in &mut doc.sections[0].paragraphs {
        for c in &mut para.controls {
            if let hwp_model::Control::Picture(p) = c {
                p.treat_as_char = false;
                p.vert_offset = 5000;
                p.horz_offset = 3000;
                p.z_order = 7;
                p.description = Some("제목 & <대체> 😀".to_string());
            }
        }
    }
    let out = tmp("ge9_float.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut raw = Vec::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_end(&mut raw)
        .unwrap();
    let xml = String::from_utf8(raw).unwrap();
    // 부유 <hp:pic zOrder="7"> + <hp:pos ... vertOffset="5000" horzOffset="3000">.
    assert!(xml.contains(r#"zOrder="7""#), "z-order 방출: {xml}");
    assert!(
        xml.contains(r#"vertOffset="5000""#),
        "세로 오프셋 방출: {xml}"
    );
    assert!(
        xml.contains(r#"horzOffset="3000""#),
        "가로 오프셋 방출: {xml}"
    );
    assert!(
        xml.contains(r#"treatAsChar="0""#),
        "부유(treatAsChar=0) 방출: {xml}"
    );
    let picture_comment = "<hp:shapeComment>제목 &amp; &lt;대체&gt; 😀</hp:shapeComment>";
    let comment_at = xml.find(picture_comment).expect("그림 설명 XML escape");
    let out_margin_at = xml[..comment_at]
        .rfind("<hp:outMargin ")
        .expect("그림 outMargin");
    let close_at = comment_at + xml[comment_at..].find("</hp:pic>").expect("그림 닫는 태그");
    assert!(out_margin_at < comment_at && comment_at < close_at);

    let reread = hwpx::read_document(&out).unwrap().document;
    let description = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(picture) => picture.description.as_deref(),
            _ => None,
        });
    assert_eq!(description, Some("제목 & <대체> 😀"));
}

/// GI-1/GI-2 왕복 (b): md(각주·취소선·순서목록·중첩) → hwpx 저장 → 재읽기 → md.
#[test]
fn markdown_각주_취소선_목록_hwpx_완전왕복() {
    let md = "\
문단에 각주[^1]가 있다.

~~지운 글~~ 과 보통 글.

1. 첫째
2. 둘째
   - 안쪽 가
   - 안쪽 나
3. 셋째

[^1]: 각주 본문이다.
";
    let doc = hwp_convert::from_markdown(md);
    let out = tmp("from_md_notes.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    // 각주·목록은 DROP 경고 없이 방출돼야 한다.
    assert!(
        !warnings.iter().any(|w| w.contains("DROP")),
        "DROP 경고: {warnings:?}"
    );

    let reread = hwpx::read_document(&out).unwrap().document;
    // 각주 컨트롤이 hwpx 왕복에서 fn GenericControl로 되살아난다.
    let has_fn = reread.sections[0].paragraphs.iter().any(|p| {
        p.controls.iter().any(|c| matches!(c,
            hwp_model::Control::Generic(g) if g.ctrl_id == *b"fn  " && !g.paragraph_lists.is_empty()))
    });
    assert!(has_fn, "각주 컨트롤 왕복");

    let md_out = hwp_convert::to_markdown(&reread);
    assert!(md_out.contains("[^1]"), "각주 마커: {md_out}");
    assert!(
        md_out.contains("[^1]: 각주 본문이다."),
        "각주 정의: {md_out}"
    );
    assert!(md_out.contains("~~지운 글~~"), "취소선: {md_out}");
    assert!(md_out.contains("1. 첫째"), "순서1: {md_out}");
    assert!(md_out.contains("3. 셋째"), "순서3: {md_out}");
    assert!(md_out.contains("- 안쪽 가"), "중첩 불릿: {md_out}");
}

/// 머리말/꼬리말 적용쪽(applyPageType)이 hwpx write→read 왕복에서 보존된다(GE-α9).
/// 이전엔 writer가 `applyPageType="BOTH"` 상수라 짝수/홀수 구분 머리말이 항상
/// "양쪽"으로 뭉개졌다. `g.data` 선두 u32(적용쪽)를 read의 역매핑으로 방출한다.
#[test]
fn 머리말_적용쪽_hwpx_왕복() {
    use hwp_model::{Control, GenericControl, HwpChar, Paragraph, ParagraphList};

    for (apply, want, tag) in [(1u32, "EVEN", 1u8), (2u32, "ODD", 2u8)] {
        let mut doc = hwp_convert::from_markdown("본문");
        // head_foot_data 레이아웃: apply(u32)@0 + id(u32)@4.
        let mut data = Vec::with_capacity(8);
        data.extend_from_slice(&apply.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let g = GenericControl {
            ctrl_id: *b"head",
            data,
            paragraph_lists: vec![ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![Paragraph::default()],
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
        };
        // 머리말 컨트롤을 문단에 ExtCtrl(코드 16)로 부착.
        let para = &mut doc.sections[0].paragraphs[0];
        let idx = para.controls.len() as u32;
        para.chars.push(HwpChar::ExtCtrl {
            code: 16,
            ctrl_id: *b"head",
            payload: hwp_convert::field::rev_payload(b"head"),
            ctrl_index: Some(idx),
        });
        para.controls.push(Control::Generic(g));

        let out = tmp(&format!("hf_{apply}.hwpx"));
        let warnings = hwpx::write_document(&doc, &out).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");

        // 방출 XML에 상수 BOTH가 아니라 실제 적용쪽이 들어간다.
        let bytes = std::fs::read(&out).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut raw = Vec::new();
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_end(&mut raw)
            .unwrap();
        let xml = String::from_utf8(raw).unwrap();
        assert!(
            xml.contains(&format!(r#"applyPageType="{want}""#)),
            "applyPageType={want} 방출: {xml}"
        );

        // 되읽은 head 컨트롤의 g.data 선두 u32가 적용쪽을 보존.
        let reread = hwpx::read_document(&out).unwrap().document;
        let apply_byte = reread.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Generic(g) if g.ctrl_id == *b"head" => Some(g.data.first().copied()),
                _ => None,
            });
        assert_eq!(apply_byte, Some(Some(tag)), "적용쪽 왕복 (apply={apply})");
    }
}

/// 본문 탭이 hwpx에서 `<hp:t>` **안**의 중첩 `<hp:tab width leader type/>`(정품 mixed
/// content)로 방출되고 raw 0x09가 절대 없어야 한다. t 밖 형제 bare 탭은 한글이 폭 0으로
/// 무시하고(D3 밀착), raw 0x09를 t 안에 그대로 두면 한글이 파일을 열지 못한다(D3 먹통).
#[test]
fn 본문_탭_hwpx_tab요소_raw없음() {
    let doc = hwp_convert::from_markdown("앞\t뒤\n");
    let out = tmp("tab.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut raw = Vec::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_end(&mut raw)
        .unwrap();
    let xml = String::from_utf8(raw).unwrap();
    // 속성 있는 탭 요소(<hp:tab · width/leader/type)로 방출.
    assert!(
        xml.contains("<hp:tab ")
            && xml.contains("width=\"")
            && xml.contains("leader=\"")
            && xml.contains("type=\""),
        "탭이 속성 있는 <hp:tab …/>로 방출돼야: {xml}"
    );
    // 탭 정의 없는 문단 → 정품 기본 탭(왼쪽·채움없음): type="1" leader="0".
    assert!(
        xml.contains(r#"leader="0" type="1""#),
        "기본 탭은 leader=0 type=1이어야: {xml}"
    );
    // <hp:t>…</hp:t> 블록을 지운 뒤 <hp:tab이 남으면 t 밖 형제로 방출된 것(먹통 원인).
    let stripped = regex_lite_strip_t(&xml);
    assert!(
        !stripped.contains("<hp:tab"),
        "bare <hp:tab>(hp:t 밖 형제 — 한글 무시)가 방출됨: {xml}"
    );
    assert!(
        !xml.as_bytes().contains(&0x09),
        "section XML에 raw 0x09가 있으면 안 됨"
    );

    // 재읽기: 탭이 InlineCtrl(9)로 복원되고 텍스트 순서 보존.
    let reread = hwpx::read_document(&out).unwrap().document;
    assert!(reread.plain_text().contains("앞\t뒤"), "탭 왕복");
    let tabs = reread.sections[0].paragraphs[0]
        .chars
        .iter()
        .filter(|c| matches!(c, hwp_model::HwpChar::InlineCtrl { code: 9, .. }))
        .count();
    assert_eq!(tabs, 1, "재읽기 본문 탭 InlineCtrl(9) 1개");
}

/// `<hp:t …>…</hp:t>` 블록을 모두 제거한다(비탐욕, 정규식 없이 문자열 스캔). 남은 문자열에
/// `<hp:tab`이 있으면 탭이 t 바깥 형제로 방출된 것이다.
fn regex_lite_strip_t(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(open) = rest.find("<hp:t ").or_else(|| rest.find("<hp:t>")) {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        if let Some(close) = after.find("</hp:t>") {
            rest = &after[close + "</hp:t>".len()..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// 필드(누름틀) hwpx 왕복: create_field → write → read → list_fields로 이름·값 복원.
#[test]
fn 필드_생성_hwpx_왕복() {
    let mut doc = hwp_convert::from_markdown("수신: 부서명");
    assert!(hwp_convert::create_field(&mut doc, "수신:", "수신처", ""));
    assert_eq!(hwp_convert::set_field(&mut doc, "수신처", "홍길동"), 1);

    let out = tmp("field.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    // 쓴 XML에 fieldBegin/fieldEnd가 있다.
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    {
        use std::io::Read as _;
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
    }
    assert!(
        xml.contains(r#"type="CLICK_HERE""#),
        "fieldBegin CLICK_HERE 없음"
    );
    assert!(xml.contains(r#"name="수신처""#), "필드 이름 없음");
    assert!(xml.contains("<hp:fieldEnd"), "fieldEnd 없음");

    // 재읽기 → list_fields로 이름·종류·값 복원.
    let reread = hwpx::read_document(&out).unwrap().document;
    let fields = hwp_convert::list_fields(&reread);
    assert_eq!(fields.len(), 1, "{fields:?}");
    assert_eq!(fields[0].ctrl_id, "%clk");
    assert_eq!(fields[0].name.as_deref(), Some("수신처"));
    assert_eq!(fields[0].value, "홍길동");
}

/// GF-4: 표 128의 확장 필드(변경추적·차례·개인정보)가 hwpx write에서 DROP되지
/// 않고 fieldBegin/End·내용 텍스트로 방출된다. OWPML type은 근거 없어 UNKNOWN 폴백
/// (라벨은 잃어도 필드·내용 보존 — GF-1 등급). 재읽기 시 %unk로 되살아나 왕복 성립.
#[test]
fn 확장필드_hwp5출신_hwpx_방출() {
    use hwp_model::{Control, GenericControl, HwpChar};

    // 대표 3종: 변경추적(삭제, %%*d)·차례(%toc)·개인정보(%cpr). 각 필드를 한
    // 문단에 FIELD_START(코드3)+내용+FIELD_END(코드4)로 배치.
    let fields: [(&[u8; 4], &str); 3] = [
        (b"%%*d", "삭제된 문장"),
        (b"%toc", "제1장 서론"),
        (b"%cpr", "홍**"),
    ];
    let mut doc = hwp_convert::from_markdown("본문");
    let para = &mut doc.sections[0].paragraphs[0];
    para.chars.clear();
    para.controls.clear();
    for (i, (id, text)) in fields.iter().enumerate() {
        para.chars.push(HwpChar::ExtCtrl {
            code: 3, // FIELD_START
            ctrl_id: **id,
            payload: hwp_convert::field::rev_payload(id),
            ctrl_index: Some(i as u32),
        });
        para.chars.extend(text.chars().map(HwpChar::Text));
        para.chars.push(HwpChar::InlineCtrl {
            code: 4, // FIELD_END
            payload: hwp_convert::field::field_end_payload(id),
        });
        para.controls.push(Control::Generic(GenericControl {
            ctrl_id: **id,
            data: vec![0u8; 11], // 속성4 기타1 len2=0 id4
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
        }));
    }
    para.header.ctrl_mask = 0;

    let out = tmp("ext_fields.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    // 확장 필드는 DROP 경고 없이 방출돼야 한다.
    assert!(
        !warnings.iter().any(|w| w.contains("DROP")),
        "DROP 경고: {warnings:?}"
    );

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut raw = Vec::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_end(&mut raw)
        .unwrap();
    let xml = String::from_utf8(raw).unwrap();
    assert_eq!(
        xml.matches("<hp:fieldBegin").count(),
        3,
        "fieldBegin 3개: {xml}"
    );
    assert_eq!(
        xml.matches("<hp:fieldEnd").count(),
        3,
        "fieldEnd 3개: {xml}"
    );
    for (_, text) in &fields {
        assert!(xml.contains(text), "내용 텍스트 보존({text}): {xml}");
    }
    // 차례(%toc)는 코퍼스 실측 OWPML type으로, 변경추적·개인정보는 UNKNOWN 폴백으로.
    assert!(
        xml.contains(r#"type="TABLEOFCONTENTS""#),
        "%toc→TABLEOFCONTENTS: {xml}"
    );
    assert_eq!(
        xml.matches(r#"type="UNKNOWN""#).count(),
        2,
        "%%*d·%cpr→UNKNOWN 2개: {xml}"
    );

    // 재읽기: 필드가 되살아나고 내용 텍스트가 보존된다(왕복 성립). %toc는 종류까지
    // 왕복(%toc), 변경추적·개인정보는 %unk로 하향(내용은 보존).
    let reread = hwpx::read_document(&out).unwrap().document;
    let got = hwp_convert::list_fields(&reread);
    assert_eq!(got.len(), 3, "필드 3개 왕복: {got:?}");
    let by_value: std::collections::HashMap<&str, &str> = got
        .iter()
        .map(|f| (f.value.as_str(), f.ctrl_id.as_str()))
        .collect();
    assert_eq!(
        by_value.get("제1장 서론"),
        Some(&"%toc"),
        "%toc 종류 왕복: {got:?}"
    );
    assert_eq!(
        by_value.get("삭제된 문장"),
        Some(&"%unk"),
        "변경추적→%unk: {got:?}"
    );
    assert_eq!(
        by_value.get("홍**"),
        Some(&"%unk"),
        "개인정보→%unk: {got:?}"
    );
}

/// 책갈피(bokm) hwpx 왕복: create_bookmark → write → `<hp:bookmark name>` → read → list_bookmarks.
#[test]
fn 책갈피_생성_hwpx_왕복() {
    let mut doc = hwp_convert::from_markdown("제목 문단\n\n본문");
    assert!(hwp_convert::create_bookmark(
        &mut doc,
        "제목",
        "책갈피테스트"
    ));

    let out = tmp("bookmark.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    // 쓴 XML에 <hp:bookmark name="…"/>가 있다.
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    {
        use std::io::Read as _;
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
    }
    assert!(
        xml.contains(r#"<hp:bookmark name="책갈피테스트""#),
        "hp:bookmark 없음: {xml}"
    );

    // 재읽기 → list_bookmarks로 이름 복원.
    let reread = hwpx::read_document(&out).unwrap().document;
    let bms = hwp_convert::list_bookmarks(&reread);
    assert_eq!(bms.len(), 1, "{bms:?}");
    assert_eq!(bms[0].name, "책갈피테스트");
}

/// 하이퍼링크(%hlk) hwpx 왕복: create_hyperlink → write → fieldBegin HYPERLINK+Command → read.
#[test]
fn 하이퍼링크_생성_hwpx_왕복() {
    let mut doc = hwp_convert::from_markdown("문서: 참고");
    assert!(hwp_convert::create_hyperlink(
        &mut doc,
        "문서:",
        "https://example.com/a",
        "여기"
    ));

    let out = tmp("hyperlink.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    // 쓴 XML에 fieldBegin type=HYPERLINK + Command(URL)가 있다.
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    {
        use std::io::Read as _;
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
    }
    assert!(xml.contains(r#"type="HYPERLINK""#), "HYPERLINK 없음: {xml}");
    assert!(xml.contains("example.com"), "Command URL 없음: {xml}");

    // 재읽기 → list_fields로 종류·값·command 복원.
    let reread = hwpx::read_document(&out).unwrap().document;
    let fields = hwp_convert::list_fields(&reread);
    let hlk: Vec<_> = fields.iter().filter(|f| f.ctrl_id == "%hlk").collect();
    assert_eq!(hlk.len(), 1, "{fields:?}");
    assert_eq!(hlk[0].value, "여기");
    assert_eq!(
        hlk[0].command.as_deref(),
        Some("https\\://example.com/a;1;0;0;")
    );
}

/// 요약정보 8필드가 hwpx 패키지 왕복(write_document → read_document)에서 보존된다.
///
/// 정품 content.hpf 형식(creator/subject/keyword/description/lastsaveby meta +
/// CreatedDate/ModifiedDate ISO-8601)을 방출·재파싱한다. FILETIME은 **초 단위 정밀도**로
/// 왕복하며, 하위 100ns(FT_PER_SEC 미만)는 ISO 초 절사로 소실되므로 여기선 초 경계 값을
/// 사용한다. 이 테스트는 fixtures가 필요 없다(합성 문서).
#[test]
fn 요약정보_8필드_hwpx_패키지_왕복() {
    use hwp_model::iso8601_utc_to_filetime as iso2ft;

    let mut doc = hwp_convert::from_markdown("요약정보 검증 본문\n");
    // 모두 초 경계(FT_PER_SEC 배수)라 왕복 무손실.
    let created = iso2ft("2025-09-17T04:32:50Z").unwrap();
    let modified = iso2ft("2025-09-17T04:33:13Z").unwrap();
    doc.metadata = hwp_model::Metadata {
        title: Some("실기 검증 요약정보 문서".into()),
        author: Some("홍길동".into()),
        subject: Some("글자효과 및 요약정보 검증".into()),
        keywords: Some("hwp, 실기검증, 요약정보".into()),
        description: Some("C 시리즈 요약정보 검증용 문서입니다.".into()),
        last_saved_by: Some("검증 담당자".into()),
        create_time: Some(created),
        modify_time: Some(modified),
    };

    let out = tmp("summary_meta.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    let reread = hwpx::read_document(&out).unwrap().document;
    let m = &reread.metadata;
    assert_eq!(m.title, doc.metadata.title);
    assert_eq!(m.author, doc.metadata.author);
    assert_eq!(m.subject, doc.metadata.subject);
    assert_eq!(m.keywords, doc.metadata.keywords);
    assert_eq!(m.description, doc.metadata.description);
    assert_eq!(m.last_saved_by, doc.metadata.last_saved_by);
    // FILETIME은 초 정밀도로 보존(하위 100ns는 애초에 없음).
    assert_eq!(m.create_time, Some(created));
    assert_eq!(m.modify_time, Some(modified));
}

/// GE-β5: settings.xml·version.xml 원문 pass-through.
///
/// 커스텀 settings.xml(앱 설정·캐럿 위치)·version.xml(버전 메타)을 슬롯에 담아
/// write→read 왕복하면 원문이 상수로 대체되지 않고 그대로 보존된다. 슬롯이 없는
/// (hwp5 출신 등) 문서는 기존 기본 상수 출력이 바이트 동일하게 유지된다.
#[test]
fn ge_b5_settings_version_원문_passthrough_왕복() {
    // 정품과 다른 값(캐럿 위치·appVersion)을 넣어 상수 대체가 아님을 검증한다.
    let settings = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/app" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0"><ha:CaretPosition listIDRef="3" paraIDRef="7" pos="42"/></ha:HWPApplicationSetting>"##;
    let version = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="1" buildNumber="0" os="1" xmlVersion="1.5" application="Hancom Office Hangul" appVersion="9.1.1.4321"/>"##;

    let mut doc = hwp_convert::from_markdown("원문 pass-through 검증 본문\n");
    doc.hwpx_settings_xml = Some(settings.to_string());
    doc.hwpx_version_xml = Some(version.to_string());

    let out = tmp("passthrough_parts.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    // ZIP에서 두 파트의 바이트가 원문과 동일한지 직접 확인.
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let read_part = |zip: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str| -> String {
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    assert_eq!(
        read_part(&mut zip, "settings.xml"),
        settings,
        "settings 원문 보존"
    );
    assert_eq!(
        read_part(&mut zip, "version.xml"),
        version,
        "version 원문 보존"
    );

    // read 왕복에서도 슬롯이 원문 그대로 복원된다.
    let reread = hwpx::read_document(&out).unwrap().document;
    assert_eq!(reread.hwpx_settings_xml.as_deref(), Some(settings));
    assert_eq!(reread.hwpx_version_xml.as_deref(), Some(version));

    // JSON 왕복에서도 슬롯이 보존된다.
    let json = hwp_convert::to_json(&reread, false, false).unwrap();
    let back = hwp_convert::from_json(&json).unwrap();
    assert_eq!(back.hwpx_settings_xml.as_deref(), Some(settings));
    assert_eq!(back.hwpx_version_xml.as_deref(), Some(version));
}

/// GE-β5 보강: 슬롯이 None이면 두 파트가 기존 기본 상수와 바이트 동일하게 방출된다
/// (구형 IR·hwp5 출신 문서의 출력 불변).
#[test]
fn ge_b5_슬롯_없으면_기본상수_불변() {
    let doc = hwp_convert::from_markdown("기본 상수 경로 본문\n");
    assert!(doc.hwpx_settings_xml.is_none());
    assert!(doc.hwpx_version_xml.is_none());

    let out = tmp("passthrough_default.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut settings = String::new();
    zip.by_name("settings.xml")
        .unwrap()
        .read_to_string(&mut settings)
        .unwrap();
    let mut version = String::new();
    zip.by_name("version.xml")
        .unwrap()
        .read_to_string(&mut version)
        .unwrap();

    // 기존 상수(concat! 결과 포함)와 바이트 동일.
    assert!(settings.contains(r##"<ha:CaretPosition listIDRef="0" paraIDRef="0" pos="0"/>"##));
    assert!(version.contains(r##"application="hwp-cli""##));
    assert!(version.contains(&format!(r##"appVersion="{}""##, env!("CARGO_PKG_VERSION"))));
}

/// GC-5: hwpx `<hp:secPr>` 미해석 자식(grid/startNum/visibility/lineNumberShape/
/// footNotePr/pageBorderFill 등) 원문 pass-through.
///
/// 정품과 다른 사용자 값(startNum·grid·visibility·pageBorderFill)을 담은 secPr을
/// parse→write→parse 왕복하면, 원문 자식이 상수 템플릿으로 대체되지 않고 등장 순서대로
/// 그대로 보존된다. pagePr만 페이지 정의로 재생성된다(같은 자리).
#[test]
fn gc5_secpr_자식_원문_passthrough_왕복() {
    // 상수 템플릿과 다른 사용자 값(grid charGrid=7, startNum page=3, visibility HIDE_ALL,
    // lineNumberShape restartType=2, pageBorderFill borderFillIDRef=2)을 넣는다.
    let sec_pr = r##"<hp:secPr id="" textDirection="HORIZONTAL" spaceColumns="1134" tabStop="8000" tabStopVal="4000" tabStopUnit="HWPUNIT" outlineShapeIDRef="1" memoShapeIDRef="0" textVerticalWidthHead="0" masterPageCnt="0"><hp:grid lineGrid="5" charGrid="7" wonggojiFormat="1"/><hp:startNum pageStartsOn="ODD" page="3" pic="2" tbl="4" equation="1"/><hp:visibility hideFirstHeader="1" hideFirstFooter="1" hideFirstMasterPage="0" border="HIDE_ALL" fill="SHOW_ALL" hideFirstPageNum="1" hideFirstEmptyLine="0" showLineNumber="1"/><hp:lineNumberShape restartType="2" countBy="3" distance="100" startNumber="9"/><hp:pagePr landscape="WIDELY" width="59528" height="84186" gutterType="LEFT_ONLY"><hp:margin header="4252" footer="4252" gutter="0" left="8504" right="8504" top="5668" bottom="4252"/></hp:pagePr><hp:footNotePr><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/></hp:footNotePr><hp:pageBorderFill type="BOTH" borderFillIDRef="2" textBorder="PAPER" headerInside="0" footerInside="0" fillArea="PAPER"><hp:offset left="1417" right="1417" top="1417" bottom="1417"/></hp:pageBorderFill></hp:secPr>"##;
    let xml = format!(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hp:p id="1" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0">{sec_pr}</hp:run></hp:p></hs:sec>"##
    );

    // read: 미해석 자식이 원문으로 캡처되고 pagePr 자리엔 마커.
    let (section, warns) = hwpx::read::section::parse_section(&xml).unwrap();
    assert!(warns.is_empty(), "{warns:?}");
    let def = section.section_def().expect("secPr");
    let raw = &def.secpr_raw_children;
    assert!(
        raw.iter().any(|c| c.contains(r#"charGrid="7""#)),
        "grid 원문 캡처: {raw:?}"
    );
    assert!(
        raw.iter().any(|c| c.contains(r#"page="3""#)),
        "startNum 원문 캡처"
    );
    assert!(
        raw.iter().any(|c| c.contains(r#"border="HIDE_ALL""#)),
        "visibility 원문 캡처"
    );
    assert!(
        raw.iter().any(|c| c.contains(r#"borderFillIDRef="2""#)),
        "pageBorderFill 원문 캡처"
    );
    assert!(
        raw.iter().any(|c| c == hwp_model::SECPR_PAGEPR_SLOT),
        "pagePr 자리 마커"
    );
    // pagePr은 원문이 아니라 페이지 정의로 파싱(원문 목록에 pagePr 태그가 없어야 함).
    assert!(
        !raw.iter().any(|c| c.contains("<hp:pagePr")),
        "pagePr는 원문 아닌 페이지 정의"
    );
    assert_eq!(def.page.expect("페이지 정의").width.0, 59528);

    // write: 원문 자식이 순서대로 그대로, pagePr만 페이지 정의로 재생성.
    let doc = hwp_model::Document::default();
    let mut bins = hwpx::write::section::BinCollector::default();
    let mut wwarn = Vec::new();
    let out = hwpx::write::section::write_section(&doc, &section, false, &mut bins, &mut wwarn);
    assert!(
        out.contains(r##"<hp:grid lineGrid="5" charGrid="7" wonggojiFormat="1"/>"##),
        "grid 원문 방출"
    );
    assert!(
        out.contains(
            r##"<hp:startNum pageStartsOn="ODD" page="3" pic="2" tbl="4" equation="1"/>"##
        ),
        "startNum 원문 방출"
    );
    assert!(
        out.contains(r##"border="HIDE_ALL""##),
        "visibility 원문 방출"
    );
    assert!(
        out.contains(
            r##"<hp:lineNumberShape restartType="2" countBy="3" distance="100" startNumber="9"/>"##
        ),
        "lineNumberShape 원문 방출"
    );
    assert!(
        out.contains(r##"borderFillIDRef="2""##),
        "pageBorderFill 원문 방출"
    );
    assert!(
        out.contains(r##"<hp:pagePr landscape="WIDELY" width="59528" height="84186""##),
        "pagePr 재생성"
    );
    // 상수 템플릿 값(charGrid="0", page="0")으로 덮이지 않았다.
    assert!(
        !out.contains(r##"charGrid="0""##),
        "상수 grid로 대체되지 않음"
    );
    assert!(
        !out.contains(r##"pageStartsOn="BOTH""##),
        "상수 startNum으로 대체되지 않음"
    );

    // 순서 보존: grid < startNum < pagePr < footNotePr < pageBorderFill.
    let pos = |needle: &str| {
        out.find(needle)
            .unwrap_or_else(|| panic!("미발견: {needle}"))
    };
    assert!(
        pos("<hp:grid ") < pos("<hp:startNum "),
        "grid→startNum 순서"
    );
    assert!(
        pos("<hp:startNum ") < pos("<hp:pagePr "),
        "startNum→pagePr 순서"
    );
    assert!(
        pos("<hp:pagePr ") < pos("<hp:footNotePr>"),
        "pagePr→footNotePr 순서"
    );
    assert!(
        pos("<hp:footNotePr>") < pos(r#"<hp:pageBorderFill type="BOTH""#),
        "footNotePr→pageBorderFill 순서"
    );

    // 재파싱 왕복: 원문 보존과 페이지 정의가 유지된다.
    let (section2, _) = hwpx::read::section::parse_section(&out).unwrap();
    let def2 = section2.section_def().expect("secPr 2회차");
    assert!(
        def2.secpr_raw_children
            .iter()
            .any(|c| c.contains(r#"charGrid="7""#)),
        "왕복 후 grid 원문 유지"
    );
    assert!(
        def2.secpr_raw_children
            .iter()
            .any(|c| c.contains(r#"borderFillIDRef="2""#)),
        "왕복 후 pageBorderFill 원문 유지"
    );
    assert_eq!(def2.page.unwrap().width.0, 59528);
}

/// GC-5 보강: secpr_raw_children가 비면(hwp5 출신·구형 IR) 기존 상수 템플릿이
/// 방출된다 — 출력 불변(hwp5→hwpx 합성 경로 회귀 방지).
#[test]
fn gc5_슬롯_없으면_기본상수_불변() {
    use hwp_model::{Control, HwpChar, HwpUnit, PageDef, Paragraph, Section, SectionDef};
    let page = PageDef {
        width: HwpUnit(59528),
        height: HwpUnit(84186),
        margin_left: HwpUnit(8504),
        margin_right: HwpUnit(8504),
        margin_top: HwpUnit(5668),
        margin_bottom: HwpUnit(4252),
        margin_header: HwpUnit(4252),
        margin_footer: HwpUnit(4252),
        gutter: HwpUnit(0),
        attr: 0,
    };
    let mut para = Paragraph::default();
    para.chars.push(HwpChar::ExtCtrl {
        code: 2,
        ctrl_id: *b"secd",
        payload: vec![0u8; 12],
        ctrl_index: Some(0),
    });
    para.controls.push(Control::SectionDef(SectionDef {
        data: Vec::new(),
        page: Some(page),
        extras: Vec::new(),
        secpr_raw_children: Vec::new(), // 슬롯 없음(hwp5 출신)
        footnote_shape_raw: None,
        endnote_shape_raw: None,
        page_border_fills_raw: Vec::new(),
    }));
    let section = Section {
        paragraphs: vec![para],
        extras: Vec::new(),
    };

    let doc = hwp_model::Document::default();
    let mut bins = hwpx::write::section::BinCollector::default();
    let mut warns = Vec::new();
    let out = hwpx::write::section::write_section(&doc, &section, false, &mut bins, &mut warns);

    // 상수 템플릿의 표준 자식이 그대로 방출된다.
    assert!(
        out.contains(r##"<hp:grid lineGrid="0" charGrid="0" wonggojiFormat="0"/>"##),
        "상수 grid"
    );
    assert!(
        out.contains(
            r##"<hp:startNum pageStartsOn="BOTH" page="0" pic="0" tbl="0" equation="0"/>"##
        ),
        "상수 startNum"
    );
    assert!(
        out.contains(r##"<hp:pageBorderFill type="BOTH""##),
        "상수 pageBorderFill BOTH"
    );
    assert!(
        out.contains(r##"<hp:pageBorderFill type="EVEN""##),
        "상수 pageBorderFill EVEN"
    );
    assert!(
        out.contains(r##"<hp:pageBorderFill type="ODD""##),
        "상수 pageBorderFill ODD"
    );
    // 페이지 정의는 반영된다.
    assert!(
        out.contains(r##"<hp:pagePr landscape="WIDELY" width="59528" height="84186""##),
        "pagePr 반영"
    );
}

/// GI-XC(교차변환 손실 차단): hwp5 출신 구역의 FOOTNOTE_SHAPE/PAGE_BORDER_FILL raw가
/// 있으면 hwpx writer가 상수 대신 실측 값을 재구성한다.
///
/// raw 바이트는 **정답지 실측**이다(11.19 제안요청서 hwp, gc23 조사 보고서 확정 레이아웃):
/// PAGE_BORDER_FILL 3종이 순서로 BOTH(테두리ID=7 실선)/EVEN(1)/ODD(1)이고, FOOTNOTE_SHAPE
/// 2개가 각주/미주다. BOTH의 borderFillIDRef가 실테두리(BF#7)를 승계하는지 단언한다.
#[test]
fn gixc_hwp5_raw_secpr_실측값_방출_정답지대조() {
    use hwp_model::{Control, HwpChar, HwpUnit, PageDef, Paragraph, Section, SectionDef};
    let page = PageDef {
        width: HwpUnit(59528),
        height: HwpUnit(84186),
        margin_left: HwpUnit(8504),
        margin_right: HwpUnit(8504),
        margin_top: HwpUnit(5668),
        margin_bottom: HwpUnit(4252),
        margin_header: HwpUnit(4252),
        margin_footer: HwpUnit(4252),
        gutter: HwpUnit(0),
        attr: 0,
    };
    // 정답지 실측 raw (dump BodyText/Section0 --raw).
    let foot: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xff, 0xff, 0xff,
        0xff, 0x52, 0x03, 0x37, 0x02, 0x1b, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];
    let end: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xf8, 0x2f, 0xe0,
        0x00, 0x52, 0x03, 0x37, 0x02, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];
    // BOTH=테두리ID 7(실선), EVEN/ODD=1(무테두리). gap 0x0589=1417.
    let pbf_both: Vec<u8> = vec![
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x07, 0x00,
    ];
    let pbf_even: Vec<u8> = vec![
        0x01, 0x00, 0x00, 0x00, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x89, 0x05, 0x01, 0x00,
    ];
    let pbf_odd = pbf_even.clone();

    let mut para = Paragraph::default();
    para.chars.push(HwpChar::ExtCtrl {
        code: 2,
        ctrl_id: *b"secd",
        payload: vec![0u8; 12],
        ctrl_index: Some(0),
    });
    para.controls.push(Control::SectionDef(SectionDef {
        data: Vec::new(),
        page: Some(page),
        extras: Vec::new(),
        secpr_raw_children: Vec::new(),
        footnote_shape_raw: Some(foot),
        endnote_shape_raw: Some(end),
        page_border_fills_raw: vec![pbf_both, pbf_even, pbf_odd],
    }));
    let section = Section {
        paragraphs: vec![para],
        extras: Vec::new(),
    };

    let doc = hwp_model::Document::default();
    let mut bins = hwpx::write::section::BinCollector::default();
    let mut warns = Vec::new();
    let out = hwpx::write::section::write_section(&doc, &section, false, &mut bins, &mut warns);

    // 핵심: BOTH가 실테두리(BF#7)를 승계한다 — 상수 "1"로 대체되지 않았다.
    assert!(
        out.contains(
            r##"<hp:pageBorderFill type="BOTH" borderFillIDRef="7" textBorder="PAPER" headerInside="0" footerInside="0" fillArea="PAPER"><hp:offset left="1417" right="1417" top="1417" bottom="1417"/></hp:pageBorderFill>"##
        ),
        "BOTH가 실테두리 id=7 승계: {out}"
    );
    assert!(
        out.contains(r##"<hp:pageBorderFill type="EVEN" borderFillIDRef="1""##),
        "EVEN=무테두리 id=1"
    );
    assert!(
        out.contains(r##"<hp:pageBorderFill type="ODD" borderFillIDRef="1""##),
        "ODD=무테두리 id=1"
    );

    // 각주/미주 실측값 재구성(구분선 길이가 각주 -1, 미주 14692344로 구분됨).
    assert!(
        out.contains(
            r##"<hp:footNotePr><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/><hp:noteLine length="-1" type="SOLID" width="0.12 mm" color="#000000"/><hp:noteSpacing betweenNotes="283" belowLine="567" aboveLine="850"/><hp:numbering type="CONTINUOUS" newNum="1"/><hp:placement place="EACH_COLUMN" beneathText="0"/></hp:footNotePr>"##
        ),
        "footNotePr 실측 재구성: {out}"
    );
    assert!(
        out.contains(r##"<hp:endNotePr><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/><hp:noteLine length="14692344" type="SOLID" width="0.12 mm" color="#000000"/><hp:noteSpacing betweenNotes="0" belowLine="567" aboveLine="850"/><hp:numbering type="CONTINUOUS" newNum="1"/><hp:placement place="END_OF_DOCUMENT" beneathText="0"/></hp:endNotePr>"##),
        "endNotePr 실측 재구성: {out}"
    );
    // secPr 유효성: 파싱 왕복이 성공하고 페이지 정의가 유지된다.
    let (section2, w2) = hwpx::read::section::parse_section(&out).unwrap();
    assert!(w2.is_empty(), "{w2:?}");
    assert_eq!(section2.section_def().unwrap().page.unwrap().width.0, 59528);
}

/// 문단 끝에 gso 컨트롤(ExtCtrl 코드 11 + Generic)을 부착한다.
fn attach_gso(para: &mut hwp_model::Paragraph, g: hwp_model::GenericControl) {
    use hwp_model::HwpChar;
    let idx = para.controls.len() as u32;
    para.chars.push(HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: g.ctrl_id,
        payload: hwp_convert::field::rev_payload(&g.ctrl_id),
        ctrl_index: Some(idx),
    });
    para.controls.push(hwp_model::Control::Generic(g));
    para.header.ctrl_mask = 0;
}

/// 적대적 점검(codex) 반영 ①: hwpx 출신 수식의 **비기본 속성**이 왕복에서 보존된다.
/// 이전엔 read가 script/sz/pos만 취해 zOrder·textWrap·baseUnit·글자색·수식 글꼴·PAGE
/// 기준 배치가 전부 기본값으로 재작성됐다(22pt 빨간 수식 → 10pt 검정). 원문 pass-through.
/// ②: `lineMode`는 한컴 모델 열거값(LINE|CHAR)이라 합성 경로가 `"0"`을 쓰면 스키마 위반.
#[test]
fn 수식_속성_원문_보존() {
    use hwp_model::Control;

    // 정품 형태의 비기본 속성 수식 — 우리가 합성하는 값과 전부 다르게 잡았다.
    let src = concat!(
        r##"<hs:sec xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section"><hp:p><hp:run charPrIDRef="0">"##,
        r##"<hp:equation id="9" zOrder="7" numberingType="EQUATION" textWrap="BEHIND_TEXT" textFlow="LEFT_ONLY" lock="1" version="Equation Version 60" baseLine="70" textColor="#FF0000" baseUnit="2200" lineMode="LINE" font="맑은 고딕">"##,
        r##"<hp:sz width="4000" widthRelTo="ABSOLUTE" height="1200" heightRelTo="ABSOLUTE" protect="0"/>"##,
        r##"<hp:pos treatAsChar="0" affectLSpacing="1" flowWithText="0" allowOverlap="0" holdAnchorAndSO="0" vertRelTo="PAGE" horzRelTo="PAGE" vertAlign="TOP" horzAlign="LEFT" vertOffset="1000" horzOffset="2000"/>"##,
        r##"<hp:outMargin left="56" right="56" top="56" bottom="56"/><hp:script>x &lt; y</hp:script></hp:equation>"##,
        r##"</hp:run></hp:p></hs:sec>"##,
    );
    let (section, w) = hwpx::read::section::parse_section(src).unwrap();
    assert!(w.is_empty(), "{w:?}");
    let mut doc = hwp_convert::from_markdown("본문");
    doc.sections = vec![section];

    let out = tmp("equation_attrs.hwpx");
    assert!(
        !hwpx::write_document(&doc, &out)
            .unwrap()
            .iter()
            .any(|w| w.contains("DROP"))
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    for keep in [
        r#"zOrder="7""#,
        r#"textWrap="BEHIND_TEXT""#,
        r#"lock="1""#,
        r#"baseLine="70""#,
        r##"textColor="#FF0000""##,
        r#"baseUnit="2200""#,
        r#"lineMode="LINE""#,
        r#"font="맑은 고딕""#,
        r#"vertRelTo="PAGE""#,
        r#"horzOffset="2000""#,
        r#"<hp:outMargin left="56""#,
    ] {
        assert!(xml.contains(keep), "속성 유실: {keep}\n{xml}");
    }
    // 스크립트는 여전히 IR 정본에서 나온다(원문 pass-through 대상이 아님).
    let eq = hwpx::read_document(&out).unwrap().document.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::Generic(g) => g.equation.clone(),
            _ => None,
        })
        .expect("수식 왕복");
    assert_eq!(eq.script, "x < y");

    // 합성 경로(원문 없음)는 스키마 유효한 lineMode 열거값을 써야 한다.
    let mut synth = hwp_convert::from_markdown("본문\n\n둘째");
    attach_gso(
        &mut synth.sections[0].paragraphs[1],
        eqed_control(hwp_model::Equation {
            script: "x".to_string(),
            ..Default::default()
        }),
    );
    let out2 = tmp("equation_synth.hwpx");
    hwpx::write_document(&synth, &out2).unwrap();
    let mut zip2 =
        zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out2).unwrap())).unwrap();
    let mut xml2 = String::new();
    zip2.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml2)
        .unwrap();
    assert!(xml2.contains(r#"lineMode="CHAR""#), "합성 lineMode: {xml2}");
    assert!(
        !xml2.contains(r#"lineMode="0""#),
        "스키마 밖 lineMode: {xml2}"
    );
}

/// 적대적 점검(codex) 반영 ③: hwp5 출신 부유 수식의 배치(PAGE 기준·오프셋·z-order)가
/// gso 공통 헤더에서 복원된다. 이전엔 PARA/PARA·zOrder 0 상수라 페이지 기준으로 놓인
/// 수식이 문단 기준으로 옮겨갔다(그림 GE-9와 같은 결함).
#[test]
fn 수식_hwp5출신_배치_복원() {
    // gso 공통 헤더 24B: attr(bit0=인라인 off, bits3-4=vertRel PAGE, bits8-9=horzRel PAGE)
    // + voff + hoff + width + height + zorder.
    let mut data = Vec::new();
    let attr: u32 = (1 << 3) | (1 << 8); // vertRelTo=PAGE(1), horzRelTo=PAGE(1), 부유
    data.extend(attr.to_le_bytes());
    data.extend(1000i32.to_le_bytes()); // voff
    data.extend(2000i32.to_le_bytes()); // hoff
    data.extend(4000i32.to_le_bytes()); // width
    data.extend(1200i32.to_le_bytes()); // height
    data.extend(7i32.to_le_bytes()); // z-order

    let mut doc = hwp_convert::from_markdown("본문\n\n둘째");
    let mut g = eqed_control(hwp_model::Equation {
        script: "x over y".to_string(),
        width: 4000,
        height: 1200,
        inline: false,
        x: 2000,
        y: 1000,
        ..Default::default()
    });
    g.data = data;
    attach_gso(&mut doc.sections[0].paragraphs[1], g);

    let out = tmp("equation_hwp5_pos.hwpx");
    hwpx::write_document(&doc, &out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    let eq = &xml[xml.find("<hp:equation").expect("수식 방출")..];
    for keep in [
        r#"zOrder="7""#,
        r#"vertRelTo="PAGE""#,
        r#"horzRelTo="PAGE""#,
        r#"vertOffset="1000""#,
        r#"horzOffset="2000""#,
        r#"treatAsChar="0""#,
    ] {
        assert!(eq.contains(keep), "배치 유실: {keep}\n{eq}");
    }
}

/// eqed 컨트롤(수식) 하나를 만든다.
fn eqed_control(equation: hwp_model::Equation) -> hwp_model::GenericControl {
    hwp_model::GenericControl {
        ctrl_id: *b"eqed",
        data: Vec::new(),
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: Vec::new(),
        equation: Some(equation),
        column_def: None,
    }
}

/// GE-14: 수식이 `<hp:equation>`으로 방출되고 스크립트·크기·배치가 왕복에서 살아남는다.
/// 이전엔 writer arm이 없어 통째로 DROP — 각주 든 문서를 편집만 해도 수식이 사라졌다.
/// XML 특수문자(`<`·`&`) 케이스는 writer `esc()` ↔ reader 엔티티 해석의 짝을 고정한다.
#[test]
fn 수식_hwpx_왕복() {
    use hwp_model::{Control, Equation, GenericControl};

    let equations = [
        Equation {
            script: "a over b = c_1 ^2".to_string(),
            width: 4000,
            height: 1200,
            inline: true,
            x: 0,
            y: 0,
            ..Equation::default()
        },
        // 특수문자: esc()가 &lt;·&amp;로 쓰고 reader가 되돌리지 못하면 글자가 사라진다.
        Equation {
            script: "x < y & y > z".to_string(),
            width: 2000,
            height: 900,
            inline: false,
            x: 3000,
            y: 5000,
            ..Equation::default()
        },
    ];
    let mut doc = hwp_convert::from_markdown("본문\n\n둘째");
    for eq in equations.iter().cloned() {
        attach_gso(
            &mut doc.sections[0].paragraphs[1],
            GenericControl {
                ctrl_id: *b"eqed",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: Some(eq),
                column_def: None,
            },
        );
    }

    let out = tmp("equation.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    let reread = hwpx::read_document(&out).unwrap();
    assert!(
        !reread.warnings.iter().any(|w| w.contains("DROP")),
        "{:?}",
        reread.warnings
    );
    let got: Vec<_> = reread.document.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .filter_map(|c| match c {
            Control::Generic(g) => g.equation.as_ref(),
            _ => None,
        })
        .collect();
    assert_eq!(got.len(), 2, "수식 2개 왕복");
    for (got, want) in got.iter().zip(&equations) {
        assert_eq!(got.script, want.script, "스크립트 원문");
        assert_eq!((got.width, got.height), (want.width, want.height), "크기");
        assert_eq!(got.inline, want.inline, "글자처럼 취급");
    }
    // 부유 수식의 오프셋은 <hp:pos>에 실려야 한다(0으로 뭉개면 좌상단에 몰린다).
    assert_eq!((got[1].x, got[1].y), (3000, 5000), "부유 오프셋");
}

/// hwp5-출신 글상자(gso + 문단)가 hwpx `<hp:rect>+<hp:drawText>` 왕복을 통과한다 —
/// 이전엔 통째로 드롭돼 안의 텍스트가 소실됐다.
#[test]
fn 글상자_hwp5출신_hwpx_왕복() {
    use hwp_model::{CharShapeId, GenericControl, HwpChar, Paragraph, ParagraphList};

    let mut doc = hwp_convert::from_markdown("본문 문단\n\n둘째 문단");
    // hwp5형 gso: 40B 공통 헤더(attr bit0=글자처럼, 크기 4000x2000) + 글상자 문단 1개.
    let mut data = vec![0u8; 40];
    data[0] = 1; // treatAsChar
    data[12..16].copy_from_slice(&4000i32.to_le_bytes());
    data[16..20].copy_from_slice(&2000i32.to_le_bytes());
    let boxed = Paragraph {
        chars: "상자속글".chars().map(HwpChar::Text).collect(),
        char_shape_runs: vec![(0, CharShapeId(0))],
        ..Default::default()
    };
    let gso = GenericControl {
        ctrl_id: *b"gso ",
        data,
        paragraph_lists: vec![ParagraphList {
            header_data: Vec::new(),
            paragraphs: vec![boxed],
        }],
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
    };
    attach_gso(&mut doc.sections[0].paragraphs[1], gso);

    let out = tmp("gso_textbox.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(
        !warnings.iter().any(|w| w.contains("gso")),
        "gso 드롭 경고가 없어야: {warnings:?}"
    );

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    {
        use std::io::Read as _;
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
    }
    assert!(xml.contains("<hp:rect "), "hp:rect 없음: {xml}");
    assert!(xml.contains("<hp:drawText "), "hp:drawText 없음: {xml}");
    assert!(xml.contains("상자속글"), "글상자 텍스트 없음: {xml}");
    assert!(
        xml.contains(r#"treatAsChar="1""#),
        "treatAsChar 보존: {xml}"
    );

    // 재읽기: 텍스트 보존 + 도형 기하 복원(rect 4000x2000).
    let reread = hwpx::read_document(&out).unwrap().document;
    assert!(
        reread.plain_text().contains("상자속글"),
        "재읽기 텍스트: {}",
        reread.plain_text()
    );
    let shape = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            hwp_model::Control::Generic(g) if !g.gso_shapes.is_empty() => Some(&g.gso_shapes[0]),
            _ => None,
        })
        .expect("재읽기 도형");
    assert_eq!(shape.kind, hwp_model::ShapeKind::Rect);
    assert_eq!((shape.w, shape.h), (4000, 2000));
}

/// hwpx-출신 구조화 도형(ShapeGeom)이 쓰기→읽기 왕복에서 기하·스타일을 보존한다 —
/// 이전엔 드롭. Polygon 점(pt0..)·Rect 채움/테두리 색 왕복 확인.
#[test]
fn 도형_shapegeom_hwpx_왕복() {
    use hwp_model::{GenericControl, ShapeGeom, ShapeKind};

    let mut doc = hwp_convert::from_markdown("본문\n\n둘째");
    let rect = ShapeGeom {
        kind: ShapeKind::Rect,
        x: 1000,
        y: 2000,
        w: 5000,
        h: 3000,
        points: Vec::new(),
        fill: 0x00CC8040, // BGR
        fill_gradient: None,
        border_color: 0x000000FF, // 빨강(BGR)
        border_width: 40,
        round_ratio: 10,
        border_style: 1, // DASH
        arrow_start: 0,
        arrow_end: 0,
        anchored: false,
        description: Some("사각형 설명".to_string()),
    };
    let poly = ShapeGeom {
        kind: ShapeKind::Polygon,
        x: 0,
        y: 0,
        w: 200,
        h: 100,
        points: vec![(0, 0), (100, 50), (200, 0)],
        fill: 0xFFFF_FFFF,
        fill_gradient: None,
        border_color: 0,
        border_width: 12,
        round_ratio: 0,
        border_style: 0,
        arrow_start: 0,
        arrow_end: 0,
        anchored: false,
        description: Some("다각형 설명".to_string()),
    };
    let gso = GenericControl {
        ctrl_id: *b"rect",
        data: Vec::new(),
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: vec![rect.clone(), poly.clone()],
        equation: None,
        column_def: None,
    };
    attach_gso(&mut doc.sections[0].paragraphs[1], gso);

    let out = tmp("gso_shapes.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    for (comment, close) in [
        (
            "<hp:shapeComment>사각형 설명</hp:shapeComment>",
            "</hp:rect>",
        ),
        (
            "<hp:shapeComment>다각형 설명</hp:shapeComment>",
            "</hp:polygon>",
        ),
    ] {
        let comment_at = xml.find(comment).expect("도형 shapeComment");
        let out_margin_at = xml[..comment_at]
            .rfind("<hp:outMargin ")
            .expect("도형 outMargin");
        let close_at = comment_at + xml[comment_at..].find(close).expect("도형 닫는 태그");
        assert!(out_margin_at < comment_at && comment_at < close_at);
    }

    let reread = hwpx::read_document(&out).unwrap().document;
    let shapes: Vec<&ShapeGeom> = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .filter_map(|c| match c {
            hwp_model::Control::Generic(g) if !g.gso_shapes.is_empty() => Some(&g.gso_shapes[0]),
            _ => None,
        })
        .collect();
    assert_eq!(shapes.len(), 2, "도형 2개 왕복");
    let r = shapes.iter().find(|s| s.kind == ShapeKind::Rect).unwrap();
    assert_eq!((r.x, r.y, r.w, r.h), (1000, 2000, 5000, 3000));
    assert_eq!(r.fill, rect.fill);
    assert_eq!(r.border_color, rect.border_color);
    assert_eq!(r.border_width, rect.border_width);
    assert_eq!(r.border_style, rect.border_style);
    assert_eq!(r.round_ratio, rect.round_ratio);
    assert_eq!(r.description, rect.description);
    let p = shapes
        .iter()
        .find(|s| s.kind == ShapeKind::Polygon)
        .unwrap();
    assert_eq!(p.points, poly.points, "폴리곤 점 왕복");
    assert_eq!(p.description, poly.description);
}

/// hwp5-출신 장식 도형(텍스트 없는 gso)이 hwpx 도형 요소로 왕복된다 — 실쌍 바이트
/// (코퍼스 원본.hwp의 SHAPE_COMPONENT+SC_LINE; 한글 export와 lineShape width=32 등 일치 검증됨).
#[test]
fn 장식_도형_hwp5출신_hwpx_왕복() {
    use hwp_model::opaque::OpaqueRecord;
    use hwp_model::{GenericControl, ShapeKind};

    // 실쌍 SHAPE_COMPONENT(252B) + SC_LINE(20B) — hwp-convert/src/gso.rs 테스트와 동일 출처.
    const LINE_SC: &str = "6e696c246e696c240000000000000000000001006400000064000000c8c1000004000000000000000000e4600000020000000100000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000e17a14ae47017f400000000000000000000000000000000000000000000000007b14ae47e17aa43f0000000000000000000000000000f03f000000000000008000000000000000000000000000000000000000000000f03f00000000000000000000000020000000410000c000010000000000000000000000ffffffff00000000000000000000000000000000000000000001e76b390000";
    const LINE_GEOM: &str = "0000000000000000640000006400000000000000";
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    let mut doc = hwp_convert::from_markdown("본문\n\n둘째");
    // gso 40B 공통 헤더: attr=0x042a2211(글자처럼·PARA/COLUMN), 49608×4 — 실쌍 값.
    let mut data = vec![0u8; 40];
    data[0..4].copy_from_slice(&0x042a_2211u32.to_le_bytes());
    data[12..16].copy_from_slice(&49608i32.to_le_bytes());
    data[16..20].copy_from_slice(&4i32.to_le_bytes());
    let description = "HWP 장식 & <선> 😀";
    let encoded = description.encode_utf16().collect::<Vec<_>>();
    data.extend_from_slice(&(encoded.len() as u16).to_le_bytes());
    for unit in encoded {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    let gso = GenericControl {
        ctrl_id: *b"gso ",
        data,
        paragraph_lists: Vec::new(), // 텍스트 없음 = 장식 도형
        extras: Vec::new(),
        raw_children: vec![OpaqueRecord {
            tag: 0x4C,
            data: hex(LINE_SC),
            children: vec![OpaqueRecord {
                tag: 0x4E,
                data: hex(LINE_GEOM),
                children: Vec::new(),
            }],
        }],
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
    };
    attach_gso(&mut doc.sections[0].paragraphs[1], gso);

    let out = tmp("gso_deco.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    // 쓴 XML이 한글 export와 동형: hp:line + lineShape width=32 + treatAsChar=1 PARA/COLUMN.
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    {
        use std::io::Read as _;
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
    }
    assert!(xml.contains("<hp:line "), "hp:line 없음: {xml}");
    assert!(
        xml.contains(r#"width="32" style="SOLID""#),
        "lineShape 스타일: {xml}"
    );
    assert!(
        xml.contains(r#"vertRelTo="PARA" horzRelTo="COLUMN""#),
        "배치 역매핑: {xml}"
    );
    assert!(
        xml.contains("<hp:shapeComment>HWP 장식 &amp; &lt;선&gt; 😀</hp:shapeComment>"),
        "HWP GSO 설명 승격: {xml}"
    );

    // 재읽기 → 도형 기하 복원.
    let reread = hwpx::read_document(&out).unwrap().document;
    let s = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            hwp_model::Control::Generic(g) if !g.gso_shapes.is_empty() => Some(&g.gso_shapes[0]),
            _ => None,
        })
        .expect("도형 재읽기");
    assert_eq!(s.kind, ShapeKind::Line);
    assert_eq!((s.w, s.h), (49608, 4));
    assert_eq!(s.border_width, 32);
    assert_eq!(s.description.as_deref(), Some(description));
    // 글자처럼취급(gso attr bit0=1)이 anchored로 복원 — 재렌더 시 흐름 위치 배치의 근거.
    assert!(s.anchored, "treatAsChar=1 → anchored");
}

// ── GE-α1~α7: 글자효과·번호형식 write 대칭(hwpx→IR→hwpx→IR 왕복 보존) ─────────
//
// fixture 불필요: 각 속성을 켠 합성 header XML을 parse_header로 읽고, write_header로
// 되쓴 뒤 다시 parse_header 하여 속성이 살아남는지 단언한다. write가 상수/미방출로
// 누르던 회귀(글자 그림자·외곽선·양각/음각·첨자·밑줄 모양·번호 형식)를 잡는다.

/// charPr 자식 XML을 감싸 read→write→read 왕복하고 (원본 CharShape, 재읽기 CharShape)를 돌려준다.
fn 왕복_charpr(inner: &str) -> (hwp_model::CharShape, hwp_model::CharShape) {
    let xml = format!(
        r##"<hh:head><hh:charProperties itemCnt="1"><hh:charPr id="0" height="1000" textColor="#000000" shadeColor="#FFFFFF">{inner}</hh:charPr></hh:charProperties></hh:head>"##
    );
    let (h1, _) = hwpx::read::header::parse_header(&xml).unwrap();
    let out = hwpx::write::header::write_header(&h1, 1);
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    (h1.char_shapes[0].clone(), h2.char_shapes[0].clone())
}

/// GE-α1 글자 그림자: type/색/간격이 왕복에서 보존된다(이전엔 상수 NONE).
#[test]
fn ge_a1_글자_그림자_왕복() {
    let (cs1, cs2) =
        왕복_charpr(r##"<hh:shadow type="DROP" color="#FF0000" offsetX="7" offsetY="9"/>"##);
    assert!(cs1.has_shadow(), "원본 그림자 파싱");
    assert!(cs2.has_shadow(), "재읽기 그림자 보존");
    assert_eq!(cs2.shadow_gap, cs1.shadow_gap, "그림자 간격 보존");
    assert_eq!(cs2.shadow_gap, (7, 9));
    assert_eq!(cs2.shadow_color, cs1.shadow_color, "그림자 색 보존");
}

/// GE-α2 외곽선: 유무가 왕복에서 보존된다(이전엔 상수 NONE).
#[test]
fn ge_a2_외곽선_왕복() {
    let (cs1, cs2) = 왕복_charpr(r#"<hh:outline type="SOLID"/>"#);
    assert!(cs1.has_outline(), "원본 외곽선 파싱");
    assert!(cs2.has_outline(), "재읽기 외곽선 보존");
    // 외곽선 없음도 여전히 없음으로 유지.
    let (_, cs_none) = 왕복_charpr(r#"<hh:outline type="NONE"/>"#);
    assert!(!cs_none.has_outline(), "NONE은 외곽선 없음 유지");
}

/// GE-α3 양각: 왕복에서 보존된다(이전엔 미방출).
#[test]
fn ge_a3_양각_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:emboss/>");
    assert!(cs1.is_emboss(), "원본 양각 파싱");
    assert!(cs2.is_emboss(), "재읽기 양각 보존");
    assert!(!cs2.is_engrave(), "음각은 켜지지 않음");
}

/// GE-α3 음각: 왕복에서 보존된다(이전엔 미방출).
#[test]
fn ge_a3_음각_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:engrave/>");
    assert!(cs1.is_engrave(), "원본 음각 파싱");
    assert!(cs2.is_engrave(), "재읽기 음각 보존");
    assert!(!cs2.is_emboss(), "양각은 켜지지 않음");
}

/// GE-α4 위첨자: 왕복에서 보존된다(이전엔 미방출).
#[test]
fn ge_a4_위첨자_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:supscript/>");
    assert!(cs1.is_superscript(), "원본 위첨자 파싱");
    assert!(cs2.is_superscript(), "재읽기 위첨자 보존");
    assert!(!cs2.is_subscript(), "아래첨자는 켜지지 않음");
}

/// GE-α4 아래첨자: 왕복에서 보존된다(이전엔 미방출).
#[test]
fn ge_a4_아래첨자_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:subscript/>");
    assert!(cs1.is_subscript(), "원본 아래첨자 파싱");
    assert!(cs2.is_subscript(), "재읽기 아래첨자 보존");
    assert!(!cs2.is_superscript(), "위첨자는 켜지지 않음");
}

/// GE-alpha5: a non-solid underline shape survives an HWPX round trip.
#[test]
fn ge_a5_underline_shape_roundtrip() {
    let (cs1, cs2) =
        왕복_charpr(r##"<hh:underline type="BOTTOM" shape="DASH" color="#000000"/>"##);
    assert_eq!(cs1.underline_shape_code(), 1, "parsed DASH shape");
    assert_eq!(cs2.underline_shape_code(), 1, "preserved DASH shape");
    assert_eq!(cs2.underline_kind(), 1, "preserved bottom underline kind");
    // The serialized XML contains the normalized shape name.
    let xml = r##"<hh:head><hh:charProperties itemCnt="1"><hh:charPr id="0" height="1000"><hh:underline type="BOTTOM" shape="DASH" color="#000000"/></hh:charPr></hh:charProperties></hh:head>"##;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(out.contains(r#"shape="DASH""#), "serialized DASH: {out}");
}

/// HWP5-style normalized underline bits serialize without the HWPX-only legacy field.
#[test]
fn normalized_underline_shape_writes_without_legacy_field() {
    for (code, expected) in [(1u32, "DASH"), (11, "WAVE"), (12, "DOUBLE_WAVE")] {
        let xml = r##"<hh:head><hh:charProperties itemCnt="1"><hh:charPr id="0" height="1000"><hh:underline type="BOTTOM" shape="SOLID" color="#000000"/></hh:charPr></hh:charProperties></hh:head>"##;
        let (mut header, _) = hwpx::read::header::parse_header(xml).unwrap();
        let shape = &mut header.char_shapes[0];
        shape.attr = (shape.attr & !(0xFu32 << 4)) | (code << 4);
        shape.underline_shape = 0;

        let out = hwpx::write::header::write_header(&header, 1);

        assert!(
            out.contains(&format!(
                r#"<hh:underline type="BOTTOM" shape="{expected}""#
            )),
            "serialized {expected}: {out}"
        );
        let (roundtripped, _) = hwpx::read::header::parse_header(&out).unwrap();
        assert_eq!(
            u32::from(roundtripped.char_shapes[0].underline_shape_code()),
            code,
            "round-tripped {expected}"
        );
    }
}

/// GG-8 emphasis bits serialize as `symMark` names and survive a round trip.
#[test]
fn gg8_symmark_roundtrip() {
    let xml = r##"<hh:head><hh:charProperties itemCnt="2"><hh:charPr id="0" height="1000" symMark="DOT_ABOVE"/><hh:charPr id="1" height="1000" symMark="COLON"/></hh:charProperties></hh:head>"##;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h1.char_shapes[0].emphasis_kind(), 1);
    assert_eq!(h1.char_shapes[1].emphasis_kind(), 6);
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"symMark="DOT_ABOVE""#),
        "serialized XML: {out}"
    );
    assert!(out.contains(r#"symMark="COLON""#), "serialized XML: {out}");
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    assert_eq!(
        h2.char_shapes[0].emphasis_kind(),
        1,
        "round-trip preservation"
    );
    assert_eq!(
        h2.char_shapes[1].emphasis_kind(),
        6,
        "round-trip preservation"
    );
    // No emphasis remains NONE.
    let (_, cs_none) = 왕복_charpr("");
    assert_eq!(cs_none.emphasis_kind(), 0);
}

/// GG-10: a non-solid strike shape survives an HWPX round trip.
#[test]
fn gg10_strike_shape_roundtrip() {
    let (cs1, cs2) = 왕복_charpr(r##"<hh:strikeout shape="DASH_DOT" color="#000000"/>"##);
    assert!(cs1.has_strike(), "parsed strike");
    assert_eq!(cs1.strike_shape_code(), 3, "DASH_DOT maps to code 3");
    assert!(cs2.has_strike(), "preserved strike");
    assert_eq!(cs2.strike_shape_code(), 3, "round-trip shape code");
    // The serialized XML contains shape="DASH_DOT".
    let xml = r##"<hh:head><hh:charProperties itemCnt="1"><hh:charPr id="0" height="1000"><hh:strikeout shape="DASH_DOT" color="#000000"/></hh:charPr></hh:charProperties></hh:head>"##;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"<hh:strikeout shape="DASH_DOT""#),
        "serialized XML: {out}"
    );
    // No strike remains NONE.
    let (_, cs_plain) = 왕복_charpr("");
    assert!(!cs_plain.has_strike());
}

/// GE-α7 문단번호 형식: 수준별 start/numFormat/템플릿이 왕복에서 보존된다(이전엔 상수 ^{{level}}.).
#[test]
fn ge_a7_번호_형식_왕복() {
    let xml = r#"<hh:head><hh:numberings itemCnt="1"><hh:numbering id="1" start="0"><hh:paraHead start="3" level="1" numFormat="ROMAN_CAPITAL">제^1조</hh:paraHead><hh:paraHead start="1" level="2" numFormat="HANGUL_SYLLABLE">(^2)</hh:paraHead></hh:numbering></hh:numberings></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    let out = hwpx::write::header::write_header(&h1, 1);
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();

    let lv1 = &h1.numbering_levels[0][0];
    let lv2 = &h1.numbering_levels[0][1];
    assert_eq!(lv1.start, 3);
    assert_eq!(lv1.fmt, hwp_model::NumFmt::RomanUpper);
    assert_eq!(lv1.template, "제^1조");
    assert_eq!(lv2.fmt, hwp_model::NumFmt::HangulSyllable);
    assert_eq!(lv2.template, "(^2)");

    // 재읽기에서 동일 값 보존.
    let r1 = &h2.numbering_levels[0][0];
    let r2 = &h2.numbering_levels[0][1];
    assert_eq!(r1.start, lv1.start, "시작 번호 보존");
    assert_eq!(r1.fmt, lv1.fmt, "번호 형식 보존");
    assert_eq!(r1.template, lv1.template, "번호 템플릿 보존");
    assert_eq!(r2.fmt, lv2.fmt);
    assert_eq!(r2.template, lv2.template);
}

/// GE-α8: 문단 paraPr에 걸린 heading(목록 링크)이 write→re-read 왕복에서 보존된다.
/// 이전엔 write가 heading을 상수 NONE으로 고정해, 한글에서 번호 정의는 남지만 문단에
/// 번호가 표시되지 않았다(C6_번호형식). read가 인코딩한 head_type/level/numbering_id를
/// write가 역방출하는지 단정한다.
#[test]
fn ge_a8_문단머리_heading_왕복() {
    let xml = r#"<hh:head><hh:numberings itemCnt="2"><hh:numbering id="7" start="0"><hh:paraHead start="1" level="1" numFormat="ROMAN_CAPITAL">^1.</hh:paraHead></hh:numbering><hh:numbering id="42" start="0"><hh:paraHead start="1" level="1" numFormat="DIGIT">^1.</hh:paraHead></hh:numbering></hh:numberings><hh:paraProperties itemCnt="1"><hh:paraPr id="0" tabPrIDRef="0"><hh:align horizontal="JUSTIFY" vertical="BASELINE"/><hh:heading type="NUMBER" idRef="42" level="3"/></hh:paraPr></hh:paraProperties></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();

    // 외부 idRef=42는 두 번째 정의인 IR index 1로 정규화된다.
    let ps1 = &h1.para_shapes[0];
    assert_eq!(ps1.head_type(), 2, "번호형 머리");
    assert_eq!(ps1.head_level(), 3, "수준 3");
    assert_eq!(ps1.numbering_id, 1, "번호정의 링크");

    // write → re-read.
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"<hh:heading type="NUMBER" idRef="2" level="3"/>"#),
        "write가 heading을 역방출해야 함: {out}"
    );
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();

    // 재읽기에서 heading 링크 보존.
    let ps2 = &h2.para_shapes[0];
    assert_eq!(ps2.head_type(), 2, "재읽기 번호형 머리 보존");
    assert_eq!(ps2.head_level(), 3, "재읽기 수준 보존");
    assert_eq!(ps2.numbering_id, 1, "재읽기 번호정의 링크 보존");

    // 번호 정의도 함께 보존.
    assert_eq!(h2.numbering_levels[1][0].fmt, hwp_model::NumFmt::Digit);
}

#[test]
fn 글머리표_definition_id_왕복() {
    let xml = r#"<hh:head><hh:bullets itemCnt="1"><hh:bullet id="9" char="■" useImage="0"/></hh:bullets><hh:paraProperties itemCnt="1"><hh:paraPr id="0"><hh:heading type="BULLET" idRef="9" level="1"/></hh:paraPr></hh:paraProperties></hh:head>"#;
    let (header, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(header.para_shapes[0].numbering_id, 0);
    assert_eq!(header.bullet_chars, vec!['■']);

    let out = hwpx::write::header::write_header(&header, 1);
    assert!(
        out.contains(r#"<hh:bullet id="1" char="■" useImage="0"/>"#),
        "글머리표 정의 방출: {out}"
    );
    assert!(
        out.contains(r#"<hh:heading type="BULLET" idRef="1" level="1"/>"#),
        "글머리표 참조 방출: {out}"
    );
    let (reread, _) = hwpx::read::header::parse_header(&out).unwrap();
    assert_eq!(reread.para_shapes[0].numbering_id, 0);
    assert_eq!(reread.bullet_chars, vec!['■']);
}

/// GE-α8 보강: heading이 없는(기본) paraPr은 write에서 여전히 NONE으로 나가
/// 기본 출력 바이트가 변하지 않는다.
#[test]
fn ge_a8_기본문단_heading_none_유지() {
    let xml = r#"<hh:head><hh:paraProperties itemCnt="1"><hh:paraPr id="0" tabPrIDRef="0"><hh:align horizontal="JUSTIFY" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/></hh:paraPr></hh:paraProperties></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"<hh:heading type="NONE" idRef="0" level="0"/>"#),
        "기본 paraPr은 NONE 유지: {out}"
    );
}

/// GC-4 탭 정의: tabPr/tabItem(위치·종류·채움)이 hwpx read→write→re-read 왕복에서
/// 보존된다(이전엔 read가 tabPrIDRef만 읽고 tabItem을 버렸으며 write는 빈 상수만 냈다).
#[test]
fn gc4_탭정의_hwpx_왕복() {
    // 탭 2개 정의: 자동왼탭 + 항목 2개(오른/소수점, 채움 DASH/DOT), 그리고 자동오른탭 빈 정의.
    let xml = r#"<hh:head><hh:tabProperties itemCnt="2"><hh:tabPr id="0" autoTabLeft="1" autoTabRight="0"><hh:tabItem pos="4000" type="RIGHT" leader="DASH"/><hh:tabItem pos="8000" type="DECIMAL" leader="DOT"/></hh:tabPr><hh:tabPr id="1" autoTabLeft="0" autoTabRight="1"/></hh:tabProperties></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();

    // read 파싱 확인.
    assert_eq!(h1.tab_stops.len(), 2, "탭 정의 2개");
    let t0 = &h1.tab_stops[0];
    assert!(t0.auto_tab_left() && !t0.auto_tab_right(), "0번 자동왼탭");
    assert_eq!(t0.items.len(), 2, "0번 항목 2개");
    assert_eq!(t0.items[0].pos, 4000);
    assert_eq!(t0.items[0].kind, 1, "오른쪽 탭");
    assert_eq!(t0.items[0].fill, 2, "DASH 채움");
    assert_eq!(t0.items[1].pos, 8000);
    assert_eq!(t0.items[1].kind, 3, "소수점 탭");
    assert_eq!(t0.items[1].fill, 3, "DOT 채움");
    let t1 = &h1.tab_stops[1];
    assert!(!t1.auto_tab_left() && t1.auto_tab_right(), "1번 자동오른탭");
    assert!(t1.items.is_empty(), "1번은 항목 없음");

    // write → 방출 XML에 항목이 정품 구조(hp:switch/case[unit=HWPUNIT,pos=X]/
    // default[pos=2X])로 실린다. naked tabItem은 한글 먹통 원인이므로 금지.
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(
            r#"<hp:case hp:required-namespace="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar"><hh:tabItem pos="4000" type="RIGHT" leader="DASH" unit="HWPUNIT"/></hp:case><hp:default><hh:tabItem pos="8000" type="RIGHT" leader="DASH"/></hp:default>"#
        ),
        "방출 XML에 switch로 감싼 탭 항목(case pos=X·default pos=2X): {out}"
    );
    assert!(
        !out.contains(r#"<hh:tabItem pos="4000" type="RIGHT" leader="DASH"/>"#),
        "naked tabItem(먹통 원인) 방출 금지: {out}"
    );
    assert!(
        out.contains(r#"<hh:tabPr id="1" autoTabLeft="0" autoTabRight="1"/>"#),
        "자동오른탭 빈 정의 보존: {out}"
    );

    // re-read → case값만 취하고 default는 무시해 중복 없이 동일 값 보존.
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    assert_eq!(h2.tab_stops[0].items.len(), 2, "재읽기 항목 중복 수집 없음");
    assert_eq!(h2.tab_stops, h1.tab_stops, "왕복 후 탭 정의 완전 보존");
}

/// GC-4 하위호환: 정품 hp:switch 구조를 읽을 때 case(HwpUnitChar, pos=X)만 취하고
/// default(pos=2X)는 버려 항목이 중복 수집되지 않는다.
#[test]
fn gc4_정품_switch_구조_case만_읽음() {
    let xml = r#"<hh:head><hh:tabProperties itemCnt="1"><hh:tabPr id="0" autoTabLeft="0" autoTabRight="0"><hp:switch><hp:case hp:required-namespace="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar"><hh:tabItem pos="2900" type="LEFT" leader="NONE" unit="HWPUNIT"/></hp:case><hp:default><hh:tabItem pos="5800" type="LEFT" leader="NONE"/></hp:default></hp:switch><hp:switch><hp:case hp:required-namespace="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar"><hh:tabItem pos="44026" type="RIGHT" leader="DASH" unit="HWPUNIT"/></hp:case><hp:default><hh:tabItem pos="88052" type="RIGHT" leader="DASH"/></hp:default></hp:switch></hh:tabPr></hh:tabProperties></hh:head>"#;
    let (h, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h.tab_stops.len(), 1, "탭 정의 1개");
    let t = &h.tab_stops[0];
    assert_eq!(t.items.len(), 2, "switch 항목 2개(default 중복 배제)");
    // case의 HWPUNIT pos(X)만 취한다 — default의 2X가 아니어야.
    assert_eq!(t.items[0].pos, 2900, "첫 항목 case pos=X");
    assert_eq!(t.items[0].kind, 0, "LEFT 탭");
    assert_eq!(t.items[0].fill, 0, "NONE 채움");
    assert_eq!(t.items[1].pos, 44026, "둘째 항목 case pos=X");
    assert_eq!(t.items[1].kind, 1, "RIGHT 탭");
    assert_eq!(t.items[1].fill, 2, "DASH 채움");
}

/// GC-4 하위호환: switch 없는 naked tabItem(구형 출력·타 구현체)도 그대로 읽는다.
#[test]
fn gc4_naked_tabitem_하위호환_읽기() {
    let xml = r#"<hh:head><hh:tabProperties itemCnt="1"><hh:tabPr id="0" autoTabLeft="0" autoTabRight="0"><hh:tabItem pos="8504" type="LEFT" leader="DASH"/></hh:tabPr></hh:tabProperties></hh:head>"#;
    let (h, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h.tab_stops.len(), 1);
    let t = &h.tab_stops[0];
    assert_eq!(t.items.len(), 1, "naked 항목 1개");
    assert_eq!(t.items[0].pos, 8504);
    assert_eq!(t.items[0].fill, 2, "DASH 채움");
}

/// GC-4 회귀 방어: 탭 정의가 없으면 write_tab_properties 출력이 기존 빈 상수와 바이트 동일.
#[test]
fn gc4_탭정의_없으면_기본상수_불변() {
    let h = hwp_model::DocHeader::default();
    let out = hwpx::write::header::write_header(&h, 1);
    assert!(
        out.contains(
            r#"<hh:tabProperties itemCnt="1"><hh:tabPr id="0" autoTabLeft="0" autoTabRight="0"/></hh:tabProperties>"#
        ),
        "빈 탭 상수 불변: {out}"
    );
}

/// 쪽 컨트롤(쪽번호/감추기/새번호/자동번호)이 hwpx 왕복에서 hwp5 페이로드를 바이트
/// 동일하게 복원한다 — writer(속성 방출)와 reader(build_*)가 정확한 역쌍임을 단정.
/// 이전엔 writer가 전부 드롭(코퍼스 87건).
#[test]
fn 쪽_컨트롤_hwpx_페이로드_왕복() {
    use hwp_model::{Control, GenericControl, HwpChar};

    // (ctrl_id, code, payload) — reader build_* 레이아웃/실측 표준값.
    let mut pgnp = vec![0u8; 12];
    pgnp[0..4].copy_from_slice(&(5u32 << 8).to_le_bytes()); // BOTTOM_CENTER
    pgnp[10..12].copy_from_slice(&(u16::from(b'-')).to_le_bytes());
    let pghd = 0x21u32.to_le_bytes().to_vec(); // 머리말+쪽번호 감춤(정품 실측 표지값)
    let mut nwno = vec![0u8; 6];
    nwno[4..6].copy_from_slice(&7u16.to_le_bytes());
    let atno = {
        let mut v = Vec::new();
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    };
    let specs: Vec<([u8; 4], u16, Vec<u8>)> = vec![
        (*b"pgnp", 21, pgnp),
        (*b"pghd", 21, pghd),
        (*b"nwno", 21, nwno),
        (*b"atno", 18, atno),
    ];

    let mut doc = hwp_convert::from_markdown("본문\n\n둘째");
    let para = &mut doc.sections[0].paragraphs[1];
    for (cid, code, data) in &specs {
        let idx = para.controls.len() as u32;
        para.chars.push(HwpChar::ExtCtrl {
            code: *code,
            ctrl_id: *cid,
            payload: vec![0u8; 12],
            ctrl_index: Some(idx),
        });
        para.controls.push(Control::Generic(GenericControl {
            ctrl_id: *cid,
            data: data.clone(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
        }));
    }
    para.header.ctrl_mask = 0;

    let out = tmp("page_ctrls.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    // XML 정답지 형식 확인.
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    {
        use std::io::Read as _;
        zip.by_name("Contents/section0.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
    }
    assert!(
        xml.contains(r#"<hp:pageNum pos="BOTTOM_CENTER" formatType="DIGIT" sideChar="-"/>"#),
        "pageNum: {xml}"
    );
    assert!(
        xml.contains(r#"hideHeader="1""#) && xml.contains(r#"hidePageNum="1""#),
        "pageHiding: {xml}"
    );
    assert!(
        xml.contains(r#"<hp:newNum num="7" numType="PAGE"/>"#),
        "newNum: {xml}"
    );
    assert!(xml.contains("<hp:autoNum "), "autoNum: {xml}");

    // 재읽기 → 페이로드 바이트 동일.
    let reread = hwpx::read_document(&out).unwrap().document;
    for (cid, _, want) in &specs {
        let got = reread.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Generic(g) if g.ctrl_id == *cid => Some(&g.data),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} 재읽기 실패", String::from_utf8_lossy(cid)));
        assert_eq!(got, want, "{} 페이로드 왕복", String::from_utf8_lossy(cid));
    }
}

/// 스타일 사다리 hwpx 왕복: H1~H3 절 번호 접두와 H2/H3 SECTION 들여쓰기 모양이
/// 보존되고, 목록은 네이티브 번호/글머리 정의(head_type)로 왕복된다.
#[test]
fn 왕복_스타일_사다리() {
    let doc = hwp_convert::from_markdown("# 제목\n\n## 절\n\n- 항목\n  - 하위\n\n1. 첫\n");
    let out = tmp("ladder.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let reread = hwpx::read_document(&out).unwrap().document;

    // 텍스트 보존(절 번호 접두 포함).
    assert_eq!(reread.plain_text(), doc.plain_text());
    assert!(reread.plain_text().contains("1. 제목"));
    assert!(reread.plain_text().contains("1-1. 절"));
    // 네이티브 목록 정의 왕복: 글머리 정의 + 머리(head_type) 문단모양.
    assert!(!reread.header.bullet_chars.is_empty(), "글머리 정의 왕복");
    assert!(
        reread
            .header
            .para_shapes
            .iter()
            .any(|ps| ps.head_type() == 3),
        "BULLET 머리 문단모양"
    );
    assert!(
        reread
            .header
            .para_shapes
            .iter()
            .any(|ps| ps.head_type() == 2),
        "NUMBER 머리 문단모양"
    );
    // H2/H3 SECTION 들여쓰기(margin_left 2000, 머리 없음) 모양 왕복.
    assert!(
        reread
            .header
            .para_shapes
            .iter()
            .any(|ps| ps.margin_left == 2000 && ps.head_type() == 0),
        "SECTION 문단모양(margin_left 2000)"
    );
}

/// secPr/tabPr 왕복: secPr 자식 pass-through(`secpr_raw_children`) + 의미 탭 정의
/// (`tab_stops`)로 hwpx read → write가 원문과 슬라이스 수준 동등해야 한다 (Gap B/C).
#[test]
fn 왕복_secpr_tabpr_raw() {
    use hwp_model::Control;
    let src =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    assert!(src.exists(), "커밋된 픽스처 없음: {}", src.display());
    let doc = hwpx::read_document(&src).unwrap().document;

    // 읽기 단계 캡처: 탭 정의 의미 파싱 + secPr 자식 원문 보존.
    assert_eq!(doc.header.tab_stops.len(), 5, "tabPr 5종 의미 파싱");
    let secpr_children = doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            Control::SectionDef(def) => Some(def.secpr_raw_children.clone()),
            _ => None,
        })
        .expect("SectionDef");
    assert!(!secpr_children.is_empty(), "secPr 자식 pass-through 캡처");

    // 쓰기 → 재읽기 자기일관.
    let out = tmp("secpr_tabpr_raw.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let reread = hwpx::read_document(&out).unwrap().document;
    assert_eq!(reread.header.tab_stops, doc.header.tab_stops);

    // zip 슬라이스 수준: 입력과 출력의 secPr/tabProperties가 바이트 동일.
    let slice = |path: &PathBuf, entry: &str, open: &str, close: &str| {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut s = String::new();
        zip.by_name(entry).unwrap().read_to_string(&mut s).unwrap();
        let i = s.find(open).unwrap();
        let j = s.find(close).unwrap() + close.len();
        s[i..j].to_string()
    };
    assert_eq!(
        slice(&src, "Contents/section0.xml", "<hp:secPr", "</hp:secPr>"),
        slice(&out, "Contents/section0.xml", "<hp:secPr", "</hp:secPr>"),
        "secPr 슬라이스 바이트 동일"
    );
    assert_eq!(
        slice(
            &src,
            "Contents/header.xml",
            "<hh:tabProperties",
            "</hh:tabProperties>"
        ),
        slice(
            &out,
            "Contents/header.xml",
            "<hh:tabProperties",
            "</hh:tabProperties>"
        ),
        "tabProperties 슬라이스 바이트 동일"
    );
}

/// 각주/미주 write arm: fn/en이 `<hp:footNote>`/`<hp:endNote>`로 방출되고
/// 노트 문단이 왕복된다 (Gap A — 기존에는 DROP arm으로 드롭).
#[test]
fn 각주_미주_왕복() {
    use hwp_model::{Control, GenericControl, HwpChar, Paragraph, ParagraphList};
    let mut doc = hwp_convert::from_markdown("본문 문장입니다.\n");
    let note = |id: &[u8; 4], txt: &str| {
        // 정품 IR 형태: 노트 본문 첫 run에 자동 번호(atno) 컨트롤 + 텍스트.
        let mut chars = vec![HwpChar::ExtCtrl {
            code: 18,
            ctrl_id: *b"atno",
            payload: vec![],
            ctrl_index: Some(0),
        }];
        chars.push(HwpChar::Text(' '));
        chars.extend(txt.chars().map(HwpChar::Text));
        Control::Generic(GenericControl {
            ctrl_id: *id,
            data: vec![],
            paragraph_lists: vec![ParagraphList {
                header_data: vec![],
                paragraphs: vec![Paragraph {
                    chars,
                    controls: vec![Control::Generic(GenericControl {
                        ctrl_id: *b"atno",
                        data: vec![],
                        paragraph_lists: vec![],
                        extras: vec![],
                        raw_children: vec![],
                        gso_shapes: vec![],
                        equation: None,
                        column_def: None,
                    })],
                    ..Paragraph::default()
                }],
            }],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: None,
            column_def: None,
        })
    };
    let anchor = |idx: u32, id: &[u8; 4]| HwpChar::ExtCtrl {
        code: 17,
        ctrl_id: *id,
        payload: vec![],
        ctrl_index: Some(idx),
    };
    let p0 = &mut doc.sections[0].paragraphs[0];
    let i0 = p0.controls.len() as u32;
    p0.controls.push(note(b"fn  ", "각주 내용"));
    p0.controls.push(note(b"en  ", "미주 내용"));
    p0.chars.push(anchor(i0, b"fn  "));
    p0.chars.push(anchor(i0 + 1, b"en  "));

    let out = tmp("footnote_endnote.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(
        warnings.iter().all(|w| !w.contains("DROP")),
        "드롭 없음: {warnings:?}"
    );
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains("<hp:footNote"), "footNote 방출: {xml}");
    assert!(xml.contains("<hp:endNote"), "endNote 방출: {xml}");
    // 한글 저장본 실측 형태: number/suffixChar/instId 속성 + 본문 autoNum(종류·번호).
    assert!(
        xml.contains(r##"<hp:footNote number="1" suffixChar="41" instId="##),
        "footNote 속성 형태: {xml}"
    );
    assert!(
        xml.contains(r##"<hp:endNote number="1" suffixChar="41" instId="##),
        "endNote 속성 형태: {xml}"
    );
    assert!(
        xml.contains(r##"<hp:autoNum num="1" numType="FOOTNOTE"><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/></hp:autoNum>"##),
        "각주 본문 autoNum: {xml}"
    );
    assert!(
        xml.contains(r##"<hp:autoNum num="1" numType="ENDNOTE"><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/></hp:autoNum>"##),
        "미주 본문 autoNum: {xml}"
    );

    // 재읽기 — 노트 문단이 paragraph_lists로 복원되고 plain_text에 포함.
    let reread = hwpx::read_document(&out).unwrap().document;
    let text = reread.plain_text();
    assert!(text.contains("각주 내용"), "각주 본문 왕복: {text}");
    assert!(text.contains("미주 내용"), "미주 본문 왕복: {text}");
}

/// GG-15: HWPX round-trips picture transforms and pixel adjustments.
#[test]
fn 그림_변환_보정_속성_hwpx_왕복() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join("hwpx-gg15-pic-fx");
    std::fs::create_dir_all(&dir).unwrap();
    let fig = dir.join("g.png");
    std::fs::File::create(&fig)
        .unwrap()
        .write_all(VALID_PNG_1X1)
        .unwrap();

    let mut doc = hwp_convert::from_markdown_with(
        "이미지.\n\n![alt](g.png)\n",
        &hwp_convert::MarkdownImportOptions {
            base_dir: Some(&dir),
            preset: None,
            ..Default::default()
        },
    );
    for para in &mut doc.sections[0].paragraphs {
        for c in &mut para.controls {
            if let hwp_model::Control::Picture(p) = c {
                p.flip = 1; // 가로 뒤집기
                p.rotation = Some(30.0);
                p.crop = Some([100.0, 200.0, 3900.0, 2800.0]);
                p.brightness = 30;
                p.contrast = -20;
            }
        }
    }
    let out = tmp("gg15_pic_fx.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut raw = Vec::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_end(&mut raw)
        .unwrap();
    let xml = String::from_utf8(raw).unwrap();
    assert!(
        xml.contains(r#"<hp:flip horizontal="1" vertical="0"/>"#),
        "flip 방출: {xml}"
    );
    assert!(
        xml.contains(r#"rotationInfo angle="30""#),
        "회전 방출: {xml}"
    );
    assert!(
        xml.contains(r#"<hp:imgClip left="100" right="3900" top="200" bottom="2800"/>"#),
        "자르기 방출: {xml}"
    );
    assert!(xml.contains(r#"bright="30""#), "밝기 방출: {xml}");
    assert!(xml.contains(r#"contrast="-20""#), "명암 방출: {xml}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let pic = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            hwp_model::Control::Picture(p) => Some(p),
            _ => None,
        })
        .expect("그림 개체");
    assert_eq!(pic.flip, 1);
    assert_eq!(pic.rotation, Some(30.0));
    assert_eq!(pic.crop, Some([100.0, 200.0, 3900.0, 2800.0]));
    assert_eq!(pic.brightness, 30);
    assert_eq!(pic.contrast, -20);

    for para in &mut doc.sections[0].paragraphs {
        for control in &mut para.controls {
            if let hwp_model::Control::Picture(picture) = control {
                picture.brightness = i8::MAX;
                picture.contrast = i8::MIN;
            }
        }
    }
    let clamped_out = tmp("gg15_pic_fx_clamped.hwpx");
    hwpx::write_document(&doc, &clamped_out).unwrap();
    let clamped = hwpx::read_document(&clamped_out).unwrap().document;
    let pic = clamped.sections[0]
        .paragraphs
        .iter()
        .flat_map(|para| &para.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("picture");
    assert_eq!(pic.brightness, 100);
    assert_eq!(pic.contrast, -100);
}

/// GG-7: HWPX round-trips border-fill gradients through `header.xml`.
#[test]
fn 셀배경_그러데이션_hwpx_왕복() {
    let mut doc = hwp_convert::from_markdown("그러데이션.\n");
    doc.header.border_fills.push(hwp_model::BorderFill {
        fill_type: 0x4,
        gradient: Some(hwp_model::GradientSpec {
            radial: false,
            angle_deg: 90.0,
            stops: vec![(0.0, 0x0000_00FF), (1.0, 0x00FF_0000)],
        }),
        ..hwp_model::BorderFill::default()
    });
    let out = tmp("gg7_grad.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut raw = Vec::new();
    zip.by_name("Contents/header.xml")
        .unwrap()
        .read_to_end(&mut raw)
        .unwrap();
    let xml = String::from_utf8(raw).unwrap();
    assert!(
        xml.contains(r#"<hc:gradation type="LINEAR" angle="90""#),
        "그러데이션 방출: {xml}"
    );

    let reread = hwpx::read_document(&out).unwrap().document;
    let g = reread
        .header
        .border_fills
        .iter()
        .find_map(|bf| bf.gradient.as_ref())
        .expect("그러데이션 재읽기");
    assert!(!g.radial);
    assert_eq!(g.angle_deg, 90.0);
    assert_eq!(g.stops.len(), 2);
    assert_eq!(g.stops[0].1, 0x0000_00FF);
    assert_eq!(g.stops[1].1, 0x00FF_0000);
}
