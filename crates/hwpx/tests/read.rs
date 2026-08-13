//! HWPX reader 테스트: fixture 통합 + 합성 XML 단위.

use std::path::PathBuf;

use hwp_model::{Control, HwpChar, Paragraph};

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

#[test]
fn minimal_추출() {
    if skip_if_no_fixtures() {
        return;
    }
    let result = hwpx::read_document(&fixture("minimal.hwpx")).unwrap();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let doc = &result.document;

    assert_eq!(doc.meta.source_format, "hwpx");
    assert_eq!(doc.sections.len(), 1);
    let text = doc.plain_text();
    assert!(text.contains("hwp-cli 테스트 픽스처입니다."));
    assert!(text.contains("첫 번째 문단: 한글 텍스트와 English text 혼합."));

    // 첫 문단: secd + cold 컨트롤 (hwp5와 동일한 IR 의미)
    let first = &doc.sections[0].paragraphs[0];
    assert_eq!(first.controls.len(), 2);
    assert_eq!(first.controls[0].ctrl_id(), *b"secd");
    assert_eq!(first.controls[1].ctrl_id(), *b"cold");

    // PageDef: A4
    let page = doc.sections[0].section_def().unwrap().page.unwrap();
    assert_eq!(page.width.0, 59528);
    assert_eq!(page.height.0, 84186);

    // lineseg 흡수 확인
    assert!(!first.line_segs.is_empty());

    // 헤더 테이블
    assert_eq!(doc.header.char_shapes.len(), 7);
    assert_eq!(doc.header.fonts[0].len(), 2);
    assert_eq!(doc.header.fonts[0][0].name, "함초롬돋움");
    assert!(doc.header.styles.iter().any(|s| s.name == "바탕글"));
}

#[test]
fn 정품_shapecomment_설명_승격() {
    fn collect(paragraph: &Paragraph, descriptions: &mut Vec<String>) {
        for control in &paragraph.controls {
            match control {
                Control::Picture(picture) => {
                    if let Some(value) = &picture.description {
                        descriptions.push(value.clone());
                    }
                }
                Control::Table(table) => {
                    for cell in &table.cells {
                        for nested in &cell.paragraphs {
                            collect(nested, descriptions);
                        }
                    }
                }
                Control::Generic(generic) => {
                    descriptions.extend(
                        generic
                            .gso_shapes
                            .iter()
                            .filter_map(|shape| shape.description.clone()),
                    );
                    for list in &generic.paragraph_lists {
                        for nested in &list.paragraphs {
                            collect(nested, descriptions);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    let doc = hwpx::read_document(&path).unwrap().document;
    let mut descriptions = Vec::new();
    for section in &doc.sections {
        for paragraph in &section.paragraphs {
            collect(paragraph, &mut descriptions);
        }
    }
    assert!(
        descriptions.iter().any(|value| value == "사각형입니다."),
        "정품 fixture shapeComment: {descriptions:?}"
    );
}

#[test]
fn 합성_헤더_굵게_기울임() {
    let xml = r##"<?xml version="1.0"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head">
  <hh:refList>
    <hh:charProperties itemCnt="2">
      <hh:charPr id="0" height="1000" textColor="#FF0000">
        <hh:fontRef hangul="1" latin="2" hanja="0" japanese="0" other="0" symbol="0" user="0"/>
        <hh:bold/>
      </hh:charPr>
      <hh:charPr id="1" height="1200">
        <hh:italic/>
        <hh:underline type="BOTTOM" shape="SOLID" color="#0000FF"/>
        <hh:strikeout shape="SOLID" color="#000000"/>
      </hh:charPr>
    </hh:charProperties>
    <hh:styles>
      <hh:style id="0" type="PARA" name="개요 1" engName="Outline 1" paraPrIDRef="0" charPrIDRef="0"/>
    </hh:styles>
  </hh:refList>
</hh:head>"##;
    let (header, warnings) = hwpx::read::header::parse_header(xml).unwrap();
    assert!(warnings.is_empty());

    assert_eq!(header.char_shapes.len(), 2);
    let cs0 = &header.char_shapes[0];
    assert!(cs0.is_bold() && !cs0.is_italic());
    assert_eq!(cs0.base_size, 1000);
    assert_eq!(cs0.text_color, 0x0000_00FF); // #FF0000 → BGR
    assert_eq!(cs0.face_ids[0], 1);
    assert_eq!(cs0.face_ids[1], 2);
    let cs1 = &header.char_shapes[1];
    assert!(cs1.is_italic() && !cs1.is_bold());
    assert!(cs1.has_underline() && cs1.has_strike());
    assert_eq!(cs1.underline_color, 0x00FF_0000); // #0000FF → BGR

    assert_eq!(header.styles[0].name, "개요 1");
}

/// 취소선 shape 매핑: SOLID는 취소선, NONE/3D는 비취소선.
/// "3D" 취소선은 한글에서 보이지 않게 렌더되는데(정품 한라대 실측·사용자 확인),
/// 비트18(실선)로 매핑하면 인라인 표 폭에 걸친 가로선이 합성돼 목차에 취소선이 보였다.
#[test]
fn 취소선_3d_shape는_비취소선() {
    let xml = r##"<?xml version="1.0"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head">
  <hh:refList>
    <hh:charProperties itemCnt="3">
      <hh:charPr id="0" height="1000"><hh:strikeout shape="SOLID" color="#000000"/></hh:charPr>
      <hh:charPr id="1" height="1000"><hh:strikeout shape="NONE" color="#000000"/></hh:charPr>
      <hh:charPr id="2" height="1000"><hh:strikeout shape="3D" color="#000000"/></hh:charPr>
    </hh:charProperties>
  </hh:refList>
</hh:head>"##;
    let (header, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert!(header.char_shapes[0].has_strike(), "SOLID은 취소선");
    assert!(!header.char_shapes[1].has_strike(), "NONE은 비취소선");
    assert!(
        !header.char_shapes[2].has_strike(),
        "3D는 비취소선(한글 비가시 렌더 — 가로선 합성 방지)"
    );
}

#[test]
fn 합성_섹션_표와_컨트롤문자() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p paraPrIDRef="3" styleIDRef="1">
    <hp:run charPrIDRef="0"><hp:t>앞</hp:t><hp:tab/><hp:t>뒤</hp:t><hp:lineBreak/><hp:t>둘째 줄 &amp; 이스케이프</hp:t></hp:run>
    <hp:run charPrIDRef="1">
      <hp:tbl rowCnt="2" colCnt="2" borderFillIDRef="3">
        <hp:tr>
          <hp:tc><hp:cellAddr colAddr="0" rowAddr="0"/><hp:cellSpan colSpan="2" rowSpan="1"/><hp:cellSz width="100" height="50"/><hp:subList><hp:p><hp:run charPrIDRef="0"><hp:t>병합 셀</hp:t></hp:run></hp:p></hp:subList></hp:tc>
        </hp:tr>
        <hp:tr>
          <hp:tc><hp:cellAddr colAddr="0" rowAddr="1"/><hp:subList><hp:p><hp:run charPrIDRef="0"><hp:t>가</hp:t></hp:run></hp:p></hp:subList></hp:tc>
          <hp:tc><hp:cellAddr colAddr="1" rowAddr="1"/><hp:subList><hp:p><hp:run charPrIDRef="0"><hp:t>나</hp:t></hp:run></hp:p></hp:subList></hp:tc>
        </hp:tr>
      </hp:tbl>
    </hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, warnings) = hwpx::read::section::parse_section(xml).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(section.paragraphs.len(), 1);
    let para = &section.paragraphs[0];

    // 탭(8 WCHAR)/줄나눔(1)/이스케이프 처리 + 위치 산수
    assert_eq!(
        para.plain_text().trim_end(),
        "앞\t뒤\n둘째 줄 & 이스케이프\n병합 셀\n가\t나"
    );
    assert!(para.chars.contains(&HwpChar::CharCtrl(10)));
    // run 경계: charPrIDRef 0 → 1
    assert_eq!(para.char_shape_runs.len(), 2);
    assert_eq!(para.char_shape_runs[0].0, 0);

    // 표 구조
    let Some(Control::Table(table)) = para.controls.first() else {
        panic!("표 컨트롤이 있어야 한다");
    };
    assert_eq!((table.rows, table.cols), (2, 2));
    assert_eq!(table.cells.len(), 3);
    assert_eq!(table.cells[0].col_span, 2);
    assert_eq!(table.row_cell_counts, vec![1, 2]);
}

/// 정품 형식: `<hp:t>` **안**에 중첩된 `<hp:tab width leader type/>`(mixed content)를
/// InlineCtrl(9)로 읽고, 앞/뒤 텍스트와의 WCHAR 순서가 보존돼야 한다. 정품 한글이 목차
/// 문단을 이 형식(`<hp:t>. 개요<hp:tab width leader type/> 1</hp:t>`)으로 저장한다.
#[test]
fn 정품_중첩탭_hp_t안_인라인컨트롤9() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p paraPrIDRef="0" styleIDRef="0">
    <hp:run charPrIDRef="0"><hp:t>. 개요<hp:tab width="33718" leader="3" type="2"/> 1</hp:t></hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, warnings) = hwpx::read::section::parse_section(xml).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let para = &section.paragraphs[0];
    // 텍스트 순서 보존(탭은 평문에서 '\t').
    assert_eq!(para.plain_text().trim_end(), ". 개요\t 1");
    // 탭이 정확히 1개, InlineCtrl(9)로 복원.
    let tabs = para
        .chars
        .iter()
        .filter(|c| matches!(c, HwpChar::InlineCtrl { code: 9, .. }))
        .count();
    assert_eq!(
        tabs, 1,
        "중첩 탭이 InlineCtrl(9) 1개로 읽혀야: {:?}",
        para.chars
    );
    // 탭 앞에 텍스트 문자(Text), 뒤에도 텍스트가 오는 순서.
    let tab_at = para
        .chars
        .iter()
        .position(|c| matches!(c, HwpChar::InlineCtrl { code: 9, .. }))
        .unwrap();
    assert!(
        matches!(para.chars[tab_at - 1], HwpChar::Text('요')),
        "탭 직전은 '요': {:?}",
        para.chars
    );
}

/// 표 개체 공통 속성(<hp:pos>/<hp:sz>/<hp:outMargin>/zOrder)이 GsoPlacement로 보존되는지.
/// 글자처럼 취급(treatAsChar)을 잃으면 인라인 표가 떠 있는 개체가 돼 본문 흐름에서
/// 빠지고 한글이 재배치(겹침/빈 페이지)한다 — 정품 한라대 실측 기반 회귀 방지.
#[test]
fn 표_배치정보_보존() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p paraPrIDRef="0" styleIDRef="0">
    <hp:run charPrIDRef="0">
      <hp:tbl rowCnt="1" colCnt="1" borderFillIDRef="1" zOrder="8">
        <hp:sz width="18279" widthRelTo="ABSOLUTE" height="3931" heightRelTo="ABSOLUTE"/>
        <hp:pos treatAsChar="1" flowWithText="1" vertRelTo="PARA" horzRelTo="PARA" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/>
        <hp:outMargin left="283" right="283" top="283" bottom="283"/>
        <hp:tr><hp:tc><hp:cellAddr colAddr="0" rowAddr="0"/><hp:subList><hp:p><hp:run charPrIDRef="0"><hp:t>x</hp:t></hp:run></hp:p></hp:subList></hp:tc></hp:tr>
      </hp:tbl>
    </hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, _) = hwpx::read::section::parse_section(xml).unwrap();
    let Some(Control::Table(table)) = section.paragraphs[0].controls.first() else {
        panic!("표 컨트롤이 있어야 한다");
    };
    let pl = table
        .placement
        .as_ref()
        .expect("표 배치정보가 보존돼야 한다");
    assert!(pl.treat_as_char, "글자처럼 취급(인라인) 보존");
    assert!(pl.flow_with_text);
    assert_eq!(pl.vert_rel_to, 2); // PARA
    assert_eq!(pl.horz_rel_to, 3); // PARA
    assert_eq!(pl.z_order, 8);
    assert_eq!(pl.width, 18279); // <hp:sz> — 병합 셀 합산 아님
    assert_eq!(pl.height, 3931);
    assert_eq!(pl.out_margins, [283, 283, 283, 283]);
    // 합성 attr = 정품 인라인 표 값
    assert_eq!(pl.synth_attr(), 0x082a_2311);
}

/// 그림 z-순서(<hp:pic zOrder>)가 보존되는지 — 누락하면 머리말/본문 로고 겹침 순서가
/// 어긋난다(정품 머리말 로고 z=4, 본문 로고 z=7).
#[test]
fn 그림_zorder_보존() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">
  <hp:p paraPrIDRef="0" styleIDRef="0">
    <hp:run charPrIDRef="0">
      <hp:pic zOrder="7" reverse="0">
        <hp:sz width="12299" height="5074"/>
        <hp:pos treatAsChar="1" vertRelTo="PAGE" horzRelTo="PAPER" vertOffset="68401" horzOffset="25510"/>
        <hc:img binaryItemIDRef="image1"/>
      </hp:pic>
    </hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, _) = hwpx::read::section::parse_section(xml).unwrap();
    let Some(Control::Picture(pic)) = section.paragraphs[0].controls.first() else {
        panic!("그림 컨트롤이 있어야 한다");
    };
    assert_eq!(pic.z_order, 7);
    assert!(pic.treat_as_char);
    // 글자처럼 취급이어도 오프셋은 보존돼야 한다(정품 본문 로고 voff=68401).
    assert_eq!(pic.vert_offset, 68401);
    assert_eq!(pic.horz_offset, 25510);
}

/// hwpx 여백류(HWPUNIT)는 hwp5 PARA_SHAPE 단위(2배)로 저장돼야 한다.
/// 정품 한라대 실측: hwpx left=1500 → hwp5 ml=3000. 줄간격은 2배 아님.
#[test]
fn 문단_여백_2배_단위() {
    let xml = r##"<?xml version="1.0"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">
  <hh:refList>
    <hh:paraProperties>
      <hh:paraPr id="0">
        <hh:align horizontal="LEFT"/>
        <hh:margin>
          <hc:intent value="-2248" unit="HWPUNIT"/>
          <hc:left value="1500" unit="HWPUNIT"/>
          <hc:right value="0" unit="HWPUNIT"/>
          <hc:prev value="1416" unit="HWPUNIT"/>
          <hc:next value="0" unit="HWPUNIT"/>
        </hh:margin>
        <hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/>
      </hh:paraPr>
    </hh:paraProperties>
  </hh:refList>
</hh:head>"##;
    let (header, _) = hwpx::read::header::parse_header(xml).unwrap();
    let ps = &header.para_shapes[0];
    assert_eq!(ps.margin_left, 3000, "left 1500 → ×2");
    assert_eq!(ps.indent, -4496, "intent -2248 → ×2");
    assert_eq!(ps.spacing_top, 1416 * 2, "prev → ×2");
    assert_eq!(ps.line_spacing, 160, "줄간격은 2배 아님");
}

#[test]
fn 목록_정의_idref를_0기반_ir로_정규화() {
    let xml = r#"<hh:head><hh:refList>
      <hh:numberings itemCnt="2">
        <hh:numbering id="7"><hh:paraHead level="1" numFormat="ROMAN_CAPITAL">^1.</hh:paraHead></hh:numbering>
        <hh:numbering id="42"><hh:paraHead level="1" numFormat="HANGUL_SYLLABLE">^1.</hh:paraHead></hh:numbering>
      </hh:numberings>
      <hh:bullets itemCnt="2"><hh:bullet id="9" char="•"/><hh:bullet id="31" char="■"/></hh:bullets>
      <hh:paraProperties itemCnt="4">
        <hh:paraPr id="0"><hh:heading type="NUMBER" idRef="7" level="1"/></hh:paraPr>
        <hh:paraPr id="1"><hh:heading type="NUMBER" idRef="42" level="1"/></hh:paraPr>
        <hh:paraPr id="2"><hh:heading type="BULLET" idRef="9" level="1"/></hh:paraPr>
        <hh:paraPr id="3"><hh:heading type="BULLET" idRef="31" level="1"/></hh:paraPr>
      </hh:paraProperties>
    </hh:refList></hh:head>"#;
    let (header, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(header.para_shapes[0].numbering_id, 0);
    assert_eq!(header.para_shapes[1].numbering_id, 1);
    assert_eq!(header.para_shapes[2].numbering_id, 0);
    assert_eq!(header.para_shapes[3].numbering_id, 1);
    assert_eq!(
        header.numbering_levels[0][0].fmt,
        hwp_model::NumFmt::RomanUpper
    );
    assert_eq!(
        header.numbering_levels[1][0].fmt,
        hwp_model::NumFmt::HangulSyllable
    );
    assert_eq!(header.bullet_chars, vec!['•', '■']);
}

/// 미정의 idRef는 야생 파일 관용 — 읽기를 막지 않고 경고 + 첫 정의(0) 폴백.
#[test]
fn 정의되지_않은_목록_idref는_경고_후_폴백() {
    let xml = r#"<hh:head><hh:refList><hh:numberings><hh:numbering id="7"/></hh:numberings><hh:paraProperties><hh:paraPr id="0"><hh:heading type="NUMBER" idRef="42" level="1"/></hh:paraPr></hh:paraProperties></hh:refList></hh:head>"#;
    let (header, warnings) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(header.para_shapes[0].numbering_id, 0, "기본 정의로 폴백");
    assert!(
        warnings.iter().any(|w| w.contains("idRef: 42")),
        "{warnings:?}"
    );
}

/// 개요(OUTLINE) heading은 정규화 없이 원시 idRef를 왕복 보존한다(정품 표본은 idRef=0).
#[test]
fn 개요_heading_idref_왕복_보존() {
    let xml = r#"<hh:head><hh:refList><hh:paraProperties itemCnt="1"><hh:paraPr id="0"><hh:heading type="OUTLINE" idRef="0" level="1"/></hh:paraPr></hh:paraProperties></hh:refList></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h1.para_shapes[0].head_type(), 1, "개요형 머리");
    assert_eq!(h1.para_shapes[0].numbering_id, 0, "원시 idRef 유지");

    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"<hh:heading type="OUTLINE" idRef="0" level="1"/>"#),
        "개요 idRef 원시값 재방출: {out}"
    );
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    assert_eq!(
        h2.para_shapes[0].numbering_id, 0,
        "재읽기에도 드리프트 없음"
    );
}

/// 쪽번호/감추기/새번호 컨트롤이 올바른 ctrl_id로 매핑·보존돼야 한다(드롭 방지).
#[test]
fn 쪽번호_감추기_컨트롤_매핑() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p paraPrIDRef="0">
    <hp:run charPrIDRef="0"><hp:ctrl>
      <hp:pageNum pos="BOTTOM_CENTER" formatType="DIGIT" sideChar="-"/>
      <hp:newNum num="1" numType="PAGE"/>
    </hp:ctrl></hp:run>
  </hp:p>
  <hp:p paraPrIDRef="0">
    <hp:run charPrIDRef="0"><hp:ctrl>
      <hp:pageHiding hideHeader="1" hideFooter="0" hideMasterPage="0" hideBorder="0" hideFill="0" hidePageNum="1"/>
    </hp:ctrl></hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, _) = hwpx::read::section::parse_section(xml).unwrap();
    let ids0: Vec<[u8; 4]> = section.paragraphs[0]
        .controls
        .iter()
        .map(|c| c.ctrl_id())
        .collect();
    assert!(ids0.contains(b"pgnp"), "pageNum → pgnp: {ids0:?}");
    assert!(ids0.contains(b"nwno"), "newNum → nwno: {ids0:?}");
    let ids1: Vec<[u8; 4]> = section.paragraphs[1]
        .controls
        .iter()
        .map(|c| c.ctrl_id())
        .collect();
    assert!(ids1.contains(b"pghd"), "pageHiding → pghd: {ids1:?}");
    // 데이터가 비어 있지 않아야 writer가 드롭하지 않는다.
    for p in &section.paragraphs {
        for c in &p.controls {
            if let Control::Generic(g) = c
                && matches!(&g.ctrl_id, b"pgnp" | b"nwno" | b"pghd")
            {
                assert!(!g.data.is_empty(), "{:?} data 비어있음", g.ctrl_id);
            }
        }
    }
}

/// borderFill의 `<hh:slash>`/`<hh:backSlash>` type≠NONE을 대각선 방향 비트(attr)로
/// 합성해야 한다(렌더러 diagonal_dirs: slash=bit2, backSlash=bit5).
#[test]
fn 테두리채움_대각선_방향_파싱() {
    let xml = r##"<?xml version="1.0"?>
<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head">
  <hh:refList>
    <hh:borderFills itemCnt="3">
      <hh:borderFill id="1" threeD="0" shadow="0">
        <hh:slash type="NONE"/><hh:backSlash type="NONE"/>
        <hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/>
      </hh:borderFill>
      <hh:borderFill id="2" threeD="0" shadow="0">
        <hh:slash type="NONE"/><hh:backSlash type="SOLID"/>
        <hh:diagonal type="SOLID" width="0.1 mm" color="#FF0000"/>
      </hh:borderFill>
      <hh:borderFill id="3" threeD="0" shadow="0">
        <hh:slash type="SOLID"/><hh:backSlash type="SOLID"/>
        <hh:diagonal type="SOLID" width="0.1 mm" color="#0000FF"/>
      </hh:borderFill>
    </hh:borderFills>
  </hh:refList>
</hh:head>"##;
    let (header, _) = hwpx::read::header::parse_header(xml).unwrap();
    let bfs = &header.border_fills;
    assert_eq!(bfs.len(), 3);
    // id=1: 대각선 라인은 있으나 방향 NONE → 방향 비트 0 (그리지 않음).
    assert_eq!(bfs[0].attr & 0x4, 0, "slash off");
    assert_eq!(bfs[0].attr & 0x20, 0, "backSlash off");
    assert!(bfs[0].diagonal.is_visible(), "대각선 스타일은 SOLID");
    // id=2: backSlash만.
    assert_eq!(bfs[1].attr & 0x4, 0);
    assert_ne!(bfs[1].attr & 0x20, 0, "backSlash on");
    // id=3: 둘 다(X).
    assert_ne!(bfs[2].attr & 0x4, 0, "slash on");
    assert_ne!(bfs[2].attr & 0x20, 0, "backSlash on");
}

/// GG-17 parses a `<hp:colLine>` child into `ColumnDef.divider`.
#[test]
fn parses_colpr_divider() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p>
    <hp:run charPrIDRef="0">
      <hp:ctrl><hp:colPr id="" type="BALANCED" layout="LEFT" colCount="2" sameSz="1" sameGap="1417"><hp:colLine type="DOT" width="0.4 mm" color="#FF0000"/></hp:colPr></hp:ctrl>
      <hp:t>본문</hp:t>
    </hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, warnings) = hwpx::read::section::parse_section(xml).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let para = &section.paragraphs[0];
    let Some(Control::Generic(g)) = para.controls.iter().find(|c| c.ctrl_id() == *b"cold") else {
        panic!("expected a cold control");
    };
    let col = g.column_def.as_ref().expect("ColumnDef");
    assert_eq!(col.count, 2);
    assert_eq!(col.kind, 1); // BALANCED
    assert_eq!(col.gap, 1417);
    let d = col.divider.expect("divider");
    assert_eq!(d.line_type, 3, "DOT");
    assert_eq!(d.width, 6, "0.4 mm maps to width index 6");
    assert_eq!(d.color, 0x0000_00FF, "#FF0000 converts to BGR");

    // An empty colPr has no divider and preserves the default document behavior.
    let xml_plain = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p>
    <hp:run charPrIDRef="0">
      <hp:ctrl><hp:colPr id="" type="NEWSPAPER" layout="LEFT" colCount="1" sameSz="1" sameGap="0"/></hp:ctrl>
      <hp:t>본문</hp:t>
    </hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, _) = hwpx::read::section::parse_section(xml_plain).unwrap();
    let Some(Control::Generic(g)) = section.paragraphs[0]
        .controls
        .iter()
        .find(|c| c.ctrl_id() == *b"cold")
    else {
        panic!("expected a cold control");
    };
    assert!(g.column_def.as_ref().unwrap().divider.is_none());
}
