//! HWPX writer tests: round trips and package rules.

use std::io::Read as _;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/hwpx")
        .join(name)
}

#[path = "../../hwp-cli/tests/common/fixture_skip.rs"]
mod fixture_skip;

/// Fixture binaries are not in the repository (local only). `true` means skip, and the skip is
/// counted (#275).
#[track_caller]
fn skip_if_no_fixtures() -> bool {
    fixture_skip::fixture_missing(&fixture("minimal.hwpx"))
}

#[path = "../../hwp-cli/tests/common/temp_path.rs"]
mod temp_path;

fn tmp(name: &str) -> temp_path::TempPath {
    temp_path::TempPath::new("hwpx-write-tests", name)
}

/// A complete 1x1 transparent PNG, including valid CRCs and an IEND chunk.
const VALID_PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x01, 0xe2, 0x21, 0xbc, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

fn append_control(doc: &mut hwp_model::Document, control: hwp_model::Control) {
    let para = &mut doc.sections[0].paragraphs[0];
    let index = para.controls.len() as u32;
    para.chars.push(hwp_model::HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: control.ctrl_id(),
        payload: Vec::new(),
        ctrl_index: Some(index),
    });
    para.controls.push(control);
}

#[test]
fn typed_report_records_unsupported_control_without_warning_parsing() {
    let mut doc = hwp_convert::from_markdown("본문\n");
    append_control(
        &mut doc,
        hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"ole ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }),
    );

    let out = tmp("typed-report-unsupported-control.hwpx");
    let report = hwpx::write_document_with_report(&doc, &out).unwrap();

    assert!(
        report.warnings.is_empty(),
        "ordinary warnings: {:?}",
        report.warnings
    );
    assert_eq!(report.preservation.events.len(), 1);
    let event = &report.preservation.events[0];
    assert_eq!(
        event.code,
        hwp_model::PreservationCode::OpaqueControlUnrepresentable
    );
    assert_eq!(event.resource, hwp_model::PreservationResourceKind::Control);
    assert_eq!(
        event.disposition,
        hwp_model::PreservationDisposition::Unrepresentable
    );
    assert_eq!(event.count, 1);
}

#[test]
fn typed_report_records_missing_picture_binary_as_asset_loss() {
    let mut doc = hwp_convert::from_markdown("본문\n");
    append_control(
        &mut doc,
        hwp_model::Control::Picture(hwp_model::Picture {
            common_data: Vec::new(),
            width: hwp_model::HwpUnit(1000),
            height: hwp_model::HwpUnit(500),
            treat_as_char: true,
            z_order: 0,
            vert_offset: 0,
            horz_offset: 0,
            description: None,
            crop: None,
            flip: 0,
            rotation: None,
            brightness: 0,
            contrast: 0,
            effect_flags: 0,
            effects_raw: Vec::new(),
            caption: None,
            bin_ref: hwp_model::BinRef::ItemRef("missing-image".to_string()),
            extras: Vec::new(),
        }),
    );

    let out = tmp("typed-report-missing-image.hwpx");
    let report = hwpx::write_document_with_report(&doc, &out).unwrap();

    assert!(
        report.warnings.is_empty(),
        "ordinary warnings: {:?}",
        report.warnings
    );
    assert_eq!(report.preservation.events.len(), 1);
    let event = &report.preservation.events[0];
    assert_eq!(event.code, hwp_model::PreservationCode::BinaryAssetRemoved);
    assert_eq!(
        event.resource,
        hwp_model::PreservationResourceKind::BinaryAsset
    );
    assert_eq!(
        event.disposition,
        hwp_model::PreservationDisposition::Removed
    );
    assert_eq!(event.count, 1);

    let legacy =
        hwpx::write_document(&doc, &tmp("typed-report-missing-image-legacy.hwpx")).unwrap();
    assert_eq!(legacy.len(), 1);
    assert!(
        legacy[0].starts_with("DROP: binary_asset_removed"),
        "{legacy:?}"
    );
}

#[test]
fn typed_report_records_both_gso_drop_paths() {
    let generic_gso = |data: Vec<u8>| {
        hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data,
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    };

    let mut invalid_header = hwp_convert::from_markdown("본문\n");
    append_control(&mut invalid_header, generic_gso(Vec::new()));
    let header_report =
        hwpx::write_document_with_report(&invalid_header, &tmp("typed-report-gso-header.hwpx"))
            .unwrap();
    assert_eq!(header_report.preservation.events.len(), 1);
    assert_eq!(
        header_report.preservation.events[0].code,
        hwp_model::PreservationCode::GsoHeaderUnrepresentable
    );

    let mut invalid_shapes = hwp_convert::from_markdown("본문\n");
    append_control(&mut invalid_shapes, generic_gso(vec![0; 24]));
    let shape_report =
        hwpx::write_document_with_report(&invalid_shapes, &tmp("typed-report-gso-shape.hwpx"))
            .unwrap();
    assert_eq!(shape_report.preservation.events.len(), 1);
    assert_eq!(
        shape_report.preservation.events[0].code,
        hwp_model::PreservationCode::GsoShapeUnrepresentable
    );
}

/// hwpx-to-IR-to-hwpx-to-IR round trip: semantic equivalence.
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
    // Preserve PageDef.
    let (a, b) = (
        original.sections[0].section_def().unwrap().page.unwrap(),
        doc.sections[0].section_def().unwrap().page.unwrap(),
    );
    assert_eq!(
        (a.width, a.height, a.margin_left),
        (b.width, b.height, b.margin_left)
    );
}

/// Package rule: mimetype is first and uncompressed.
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
    assert_eq!(first.name().unwrap(), "mimetype");
    assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    drop(first);

    let mut mime = String::new();
    zip.by_name("mimetype")
        .unwrap()
        .read_to_string(&mut mime)
        .unwrap();
    assert_eq!(mime, "application/hwp+zip");
}

/// The same IR must produce byte-identical HWPX regardless of run time or output path.
/// Also pin DOS epoch timestamps against a ZIP writer default changing to current time.
#[test]
fn 새_패키지는_시간과_경로에_무관하게_결정적() {
    let doc = hwp_convert::from_markdown("# 결정적 합성\n\n같은 입력은 같은 출력이어야 한다.\n");
    let first = tmp("deterministic-first.hwpx");
    let second = tmp("deterministic-second.hwpx");

    hwpx::write_document(&doc, &first).unwrap();
    // Exceed DOS timestamp resolution (two seconds) to reject time-dependent false greens.
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
                entry.name().unwrap()
            );
        }
    }
}

/// Markdown-to-HWPX-to-Markdown round trip: preserve structure.
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

    // Heading styles and formatted spans return in Markdown.
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

/// GB-13: table, picture, and shape captions preserve side/gap/fullSz/text
/// through HWPX write and read.
#[test]
fn caption_round_trip() {
    use hwp_model::{Caption, CaptionDirection, CaptionSide, Paragraph};

    let caption_para = |text: &str| Paragraph {
        chars: text.chars().map(hwp_model::HwpChar::Text).collect(),
        ..Paragraph::default()
    };

    // Bottom table caption with implicit full-size width.
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
    table.caption = Some(Caption {
        side: CaptionSide::Bottom,
        direction: CaptionDirection::Horizontal,
        gap: 850,
        width: None,
        last_width: 0,
        paragraphs: vec![caption_para("표 1. 캡션")],
    });

    let out = tmp("caption-table.hwpx");
    hwpx::write_document(&doc, &out).unwrap();
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    let mut xml = String::new();
    archive
        .by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(
        xml.contains(r#"<hp:caption side="BOTTOM" fullSz="1" width="0" gap="850""#),
        "{xml}"
    );
    assert!(xml.contains("표 1. 캡션"), "{xml}");
    // hwpxlib orders a table caption after outMargin and before inMargin.
    let cap_at = xml.find("<hp:caption").unwrap();
    assert!(xml[..cap_at].contains("<hp:outMargin"), "{xml}");
    assert!(xml[cap_at..].contains("<hp:inMargin"), "{xml}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let caption = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => table.caption.as_ref(),
            _ => None,
        })
        .expect("round-tripped caption");
    assert_eq!(caption.side, CaptionSide::Bottom);
    assert_eq!(caption.direction, CaptionDirection::Horizontal);
    assert_eq!(caption.gap, 850);
    assert_eq!(caption.width, None, "fullSz=1 → width None");
    assert!(
        caption
            .paragraphs
            .first()
            .is_some_and(|p| p.plain_text() == "표 1. 캡션"),
        "{:?}",
        caption.paragraphs
    );

    // Left picture caption with an explicit width.
    let mut doc = hwp_convert::from_markdown("본문\n");
    let picture = hwp_model::Picture {
        common_data: Vec::new(),
        width: hwp_model::HwpUnit(1000),
        height: hwp_model::HwpUnit(500),
        treat_as_char: true,
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: None,
        crop: None,
        flip: 0,
        rotation: None,
        brightness: 0,
        contrast: 0,
        effect_flags: 0,
        effects_raw: Vec::new(),
        caption: Some(Caption {
            side: CaptionSide::Left,
            direction: CaptionDirection::Vertical,
            gap: 300,
            width: Some(2000),
            last_width: 0,
            paragraphs: vec![caption_para("그림 1")],
        }),
        bin_ref: hwp_model::BinRef::ItemRef("img".to_string()),
        extras: Vec::new(),
    };
    doc.bin_streams.push(hwp_model::BinStream {
        name: "img".to_string(),
        data: VALID_PNG_1X1.to_vec(),
    });
    let para = &mut doc.sections[0].paragraphs[0];
    para.chars.push(hwp_model::HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: *b"gso ",
        payload: Vec::new(),
        ctrl_index: Some(para.controls.len() as u32),
    });
    para.controls.push(hwp_model::Control::Picture(picture));

    let out = tmp("caption-picture.hwpx");
    hwpx::write_document(&doc, &out).unwrap();
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    let mut xml = String::new();
    archive
        .by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(
        xml.contains(r#"<hp:caption side="LEFT" fullSz="0" width="2000" gap="300""#),
        "{xml}"
    );
    assert!(xml.contains(r#"textDirection="VERTICAL""#), "{xml}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let caption = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(pic) => pic.caption.as_ref(),
            _ => None,
        })
        .expect("round-tripped picture caption");
    assert_eq!(caption.side, CaptionSide::Left);
    assert_eq!(caption.direction, CaptionDirection::Vertical);
    assert_eq!(caption.gap, 300);
    assert_eq!(caption.width, Some(2000));
    assert!(
        caption
            .paragraphs
            .first()
            .is_some_and(|p| p.plain_text() == "그림 1")
    );

    let mut doc = hwp_convert::from_markdown("Body\n");
    let generic = hwp_model::GenericControl {
        ctrl_id: *b"gso ",
        data: Vec::new(),
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: vec![hwp_model::ShapeGeom {
            kind: hwp_model::ShapeKind::Rect,
            x: 100,
            y: 200,
            w: 3000,
            h: 1500,
            points: Vec::new(),
            fill: 0xFFFF_FFFF,
            fill_gradient: None,
            border_color: 0,
            border_width: 10,
            round_ratio: 0,
            border_style: 0,
            arrow_start: 0,
            arrow_end: 0,
            anchored: true,
            description: None,
        }],
        equation: None,
        column_def: None,
        caption: Some(Caption {
            side: CaptionSide::Bottom,
            direction: CaptionDirection::Horizontal,
            gap: 250,
            width: None,
            last_width: 3000,
            paragraphs: vec![caption_para("Shape 1")],
        }),
        hwpx_raw_xml: None,
        container_box: None,
    };
    let para = &mut doc.sections[0].paragraphs[0];
    para.chars.push(hwp_model::HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: *b"gso ",
        payload: Vec::new(),
        ctrl_index: Some(para.controls.len() as u32),
    });
    para.controls.push(hwp_model::Control::Generic(generic));

    let out = tmp("caption-shape.hwpx");
    hwpx::write_document(&doc, &out).unwrap();
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    let mut xml = String::new();
    archive
        .by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    let rect_at = xml.find("<hp:rect").unwrap();
    let caption_at = xml[rect_at..].find("<hp:caption").unwrap() + rect_at;
    let rect_end = xml[rect_at..].find("</hp:rect>").unwrap() + rect_at;
    assert!(
        caption_at < rect_end,
        "caption must remain inside the shape: {xml}"
    );
    assert!(xml.contains("Shape 1"), "{xml}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let caption = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Generic(generic) => generic.caption.as_ref(),
            _ => None,
        })
        .expect("round-tripped shape caption");
    assert_eq!(caption.paragraphs[0].plain_text(), "Shape 1");
}

/// HWPX's 32-bit UINT_MAX margin sentinel is represented by u16::MAX in the
/// shared IR, but must expand back to 4294967295 when serialized as HWPX.
#[test]
fn margin_sentinel_hwpx_round_trip() {
    let xml = r##"<?xml version="1.0"?>
<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph">
  <hp:p paraPrIDRef="0" styleIDRef="0">
    <hp:run charPrIDRef="0">
      <hp:tbl rowCnt="1" colCnt="1">
        <hp:inMargin left="4294967295" right="510" top="4294967295" bottom="141"/>
        <hp:tr><hp:tc>
          <hp:subList><hp:p><hp:run><hp:t>x</hp:t></hp:run></hp:p></hp:subList>
          <hp:cellAddr colAddr="0" rowAddr="0"/>
          <hp:cellMargin left="4294967295" right="510" top="4294967295" bottom="141"/>
        </hp:tc></hp:tr>
      </hp:tbl>
    </hp:run>
  </hp:p>
</hs:sec>"##;
    let (section, warnings) = hwpx::read::section::parse_section(xml).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let doc = hwp_model::Document::default();
    let mut bins = hwpx::write::section::BinCollector::default();
    let mut write_warnings = Vec::new();
    let written =
        hwpx::write::section::write_section(&doc, &section, false, &mut bins, &mut write_warnings);
    assert!(write_warnings.is_empty(), "{write_warnings:?}");
    assert!(
        written.contains(
            r#"<hp:inMargin left="4294967295" right="510" top="4294967295" bottom="141"/>"#
        ),
        "table margin sentinel was not expanded: {written}"
    );
    assert!(
        written.contains(
            r#"<hp:cellMargin left="4294967295" right="510" top="4294967295" bottom="141"/>"#
        ),
        "cell margin sentinel was not expanded: {written}"
    );

    let (reread, warnings) = hwpx::read::section::parse_section(&written).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let Some(hwp_model::Control::Table(table)) = reread
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find(|control| matches!(control, hwp_model::Control::Table(_)))
    else {
        panic!("table control after write/read: {:?}", reread.paragraphs);
    };
    assert_eq!(table.inner_margins, [u16::MAX, 510, u16::MAX, 141]);
    assert_eq!(table.cells[0].margins, [u16::MAX, 510, u16::MAX, 141]);
}

/// GI-3/GI-4 round trip: Markdown images/inline code retain Picture, binary and code character shapes.
#[test]
fn md_이미지_코드_hwpx_왕복() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!("hwpx-md-imgcode-{}", std::process::id()));
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
    let _ = std::fs::remove_dir_all(&dir);
}

/// GE-9: floating front-of-text picture offsets and z-order appear in HWPX hp:pos/zOrder.
/// The writer retains actual values inherited through parse_picture_gso on HWP5 read;
/// previously hardcoded zeros crowded floating pictures into the top-left corner.
#[test]
fn 부유_그림_배치_hwpx_방출() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!("hwpx-ge9-float-{}", std::process::id()));
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
    // Make the picture floating with offsets/z-order, simulating inherited HWP5 values.
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
    // Floating hp:pic zOrder=7 and hp:pos vertOffset=5000, horzOffset=3000.
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
    let _ = std::fs::remove_dir_all(&dir);
}

/// A negative `<hp:pos>` offset is written in Hancom's unsigned 32-bit form (-2051 ->
/// 4294965245, the D1 seal case), a non-negative one is unchanged, and reading the file
/// back gives the signed value again.
#[test]
fn floating_picture_negative_offset_emits_unsigned_and_round_trips() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!("hwpx-neg-pos-offset-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::File::create(dir.join("g.png"))
        .unwrap()
        .write_all(VALID_PNG_1X1)
        .unwrap();
    let mut doc = hwp_convert::from_markdown_with(
        "![alt](g.png)\n",
        &hwp_convert::MarkdownImportOptions {
            base_dir: Some(&dir),
            preset: None,
            ..Default::default()
        },
    );
    for para in &mut doc.sections[0].paragraphs {
        for c in &mut para.controls {
            if let hwp_model::Control::Picture(p) = c {
                p.treat_as_char = false;
                p.vert_offset = -2051;
                p.horz_offset = 1984;
            }
        }
    }
    let out = tmp("neg_pos_offset.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains(r#"vertOffset="4294965245""#), "{xml}");
    assert!(xml.contains(r#"horzOffset="1984""#), "{xml}");
    assert!(!xml.contains(r#"Offset="-"#), "no signed offsets: {xml}");

    let reread = hwpx::read_document(&out).unwrap().document;
    let offsets = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(p) => Some((p.vert_offset, p.horz_offset)),
            _ => None,
        });
    assert_eq!(offsets, Some((-2051, 1984)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// GI-1/GI-2 round trip (b): Markdown footnotes, strikeout, ordered/nested lists through HWPX.
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
    // Emit footnotes and lists without DROP warnings.
    assert!(
        !warnings.iter().any(|w| w.contains("DROP")),
        "DROP 경고: {warnings:?}"
    );

    let reread = hwpx::read_document(&out).unwrap().document;
    // Footnote controls reread from HWPX as fn GenericControl.
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

/// Header/footer applyPageType survives HWPX write/read (GE-α9).
/// Previously a constant BOTH collapsed even/odd headers into both-page headers.
/// Reverse the read mapping to emit the leading g.data u32 apply-page value.
#[test]
fn 머리말_적용쪽_hwpx_왕복() {
    use hwp_model::{Control, GenericControl, HwpChar, Paragraph, ParagraphList};

    for (apply, want, tag) in [(1u32, "EVEN", 1u8), (2u32, "ODD", 2u8)] {
        let mut doc = hwp_convert::from_markdown("본문");
        // head_foot_data layout: apply(u32)@0 + id(u32)@4.
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
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        };
        // Attach the header control to the paragraph as ExtCtrl code 16.
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

        // Output XML contains the actual apply-page value rather than constant BOTH.
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

        // The leading g.data u32 of the reread head control retains the apply-page value.
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

/// Emit body tabs as nested hp:tab width/leader/type inside hp:t, matching genuine mixed
/// content, never raw 0x09. Hangul ignores sibling tabs outside t as zero-width (D3 crowding),
/// and cannot open files containing raw 0x09 inside t (D3 hang).
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
    // Emit a tab element with width/leader/type attributes.
    assert!(
        xml.contains("<hp:tab ")
            && xml.contains("width=\"")
            && xml.contains("leader=\"")
            && xml.contains("type=\""),
        "탭이 속성 있는 <hp:tab …/>로 방출돼야: {xml}"
    );
    // Without a tab definition, use the genuine default left tab with no leader: type=1, leader=0.
    assert!(
        xml.contains(r#"leader="0" type="1""#),
        "기본 탭은 leader=0 type=1이어야: {xml}"
    );
    // A remaining hp:tab after removing hp:t blocks indicates an invalid sibling tab outside t.
    let stripped = regex_lite_strip_t(&xml);
    assert!(
        !stripped.contains("<hp:tab"),
        "bare <hp:tab>(hp:t 밖 형제 — 한글 무시)가 방출됨: {xml}"
    );
    assert!(
        !xml.as_bytes().contains(&0x09),
        "section XML에 raw 0x09가 있으면 안 됨"
    );

    // Reread tabs as InlineCtrl(9), preserving text order.
    let reread = hwpx::read_document(&out).unwrap().document;
    assert!(reread.plain_text().contains("앞\t뒤"), "탭 왕복");
    let tabs = reread.sections[0].paragraphs[0]
        .chars
        .iter()
        .filter(|c| matches!(c, hwp_model::HwpChar::InlineCtrl { code: 9, .. }))
        .count();
    assert_eq!(tabs, 1, "재읽기 본문 탭 InlineCtrl(9) 1개");
}

/// Remove every hp:t block using a nongreedy string scan without regex. If hp:tab remains,
/// the writer emitted a tab outside t as a sibling.
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

/// Form-field HWPX round trip: create_field, write, read, then list_fields restores names/values.
#[test]
fn 필드_생성_hwpx_왕복() {
    let mut doc = hwp_convert::from_markdown("수신: 부서명");
    assert!(hwp_convert::create_field(&mut doc, "수신:", "수신처", ""));
    assert_eq!(hwp_convert::set_field(&mut doc, "수신처", "홍길동"), 1);

    let out = tmp("field.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    // Written XML contains fieldBegin/fieldEnd.
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

    // Reread list_fields restores names, kinds and values.
    let reread = hwpx::read_document(&out).unwrap().document;
    let fields = hwp_convert::list_fields(&reread);
    assert_eq!(fields.len(), 1, "{fields:?}");
    assert_eq!(fields[0].ctrl_id, "%clk");
    assert_eq!(fields[0].name.as_deref(), Some("수신처"));
    assert_eq!(fields[0].value, "홍길동");
}

/// GF-4: extended table-128 fields (change tracking, TOC and privacy) emit fieldBegin/End
/// and content text rather than DROP. Unevidenced OWPML types fall back to UNKNOWN,
/// retaining fields/content at GF-1 fidelity; rereading restores %unk.
#[test]
fn 확장필드_hwp5출신_hwpx_방출() {
    use hwp_model::{Control, GenericControl, HwpChar};

    // Three representative fields: deletion tracking (%%*d), TOC (%toc), privacy (%cpr).
    // Place FIELD_START(code 3), content and FIELD_END(code 4) in each paragraph.
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
            data: vec![0u8; 11], // attr4, other1, len2=0, id4
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));
    }
    para.header.ctrl_mask = 0;

    let out = tmp("ext_fields.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    // Extended fields must emit without DROP warnings.
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
    // TOC (%toc) uses the corpus-observed OWPML type; tracking/privacy fall back to UNKNOWN.
    assert!(
        xml.contains(r#"type="TABLEOFCONTENTS""#),
        "%toc→TABLEOFCONTENTS: {xml}"
    );
    assert_eq!(
        xml.matches(r#"type="UNKNOWN""#).count(),
        2,
        "%%*d·%cpr→UNKNOWN 2개: {xml}"
    );

    // Reread fields and their content; %toc retains its kind while tracking/privacy
    // degrade to %unk with their content preserved.
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

/// Bookmark HWPX round trip: create_bookmark, write hp:bookmark name, read, list_bookmarks.
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

    // Written XML contains hp:bookmark with its name attribute.
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

    // Reread list_bookmarks restores the name.
    let reread = hwpx::read_document(&out).unwrap().document;
    let bms = hwp_convert::list_bookmarks(&reread);
    assert_eq!(bms.len(), 1, "{bms:?}");
    assert_eq!(bms[0].name, "책갈피테스트");
}

/// Hyperlink (%hlk) round trip: create_hyperlink, write fieldBegin HYPERLINK/Command, read.
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

    // Written XML contains fieldBegin type=HYPERLINK and the Command URL.
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

    // Reread list_fields restores kind, value and command.
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

/// Preserve eight summary fields across write_document/read_document package round trips.
///
/// Emit and reparse genuine content.hpf creator/subject/keyword/description/lastsaveby metadata
/// and ISO-8601 CreatedDate/ModifiedDate. FILETIME round trips at second precision;
/// subsecond 100 ns ticks below FT_PER_SEC are lost through ISO second truncation, so use
/// second-boundary values here. This synthetic-document test needs no fixtures.
#[test]
fn 요약정보_8필드_hwpx_패키지_왕복() {
    use hwp_model::iso8601_utc_to_filetime as iso2ft;

    let mut doc = hwp_convert::from_markdown("요약정보 검증 본문\n");
    // All timestamps lie on second boundaries (multiples of FT_PER_SEC), making the round trip lossless.
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
    // Retain FILETIME to second precision; these inputs have no subsecond ticks.
    assert_eq!(m.create_time, Some(created));
    assert_eq!(m.modify_time, Some(modified));
}

/// GE-β5: pass through original settings.xml/version.xml.
///
/// Put custom app settings/caret positions and version metadata in their slots, then
/// write/read them without replacement by constants. Documents without those slots,
/// such as HWP5 sources, retain byte-identical default-constant output.
#[test]
fn ge_b5_settings_version_원문_passthrough_왕복() {
    // Use nondefault caret/appVersion values to detect replacement with constants.
    let settings = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/app" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0"><ha:CaretPosition listIDRef="3" paraIDRef="7" pos="42"/></ha:HWPApplicationSetting>"##;
    let version = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="1" buildNumber="0" os="1" xmlVersion="1.5" application="Hancom Office Hangul" appVersion="9.1.1.4321"/>"##;

    let mut doc = hwp_convert::from_markdown("원문 pass-through 검증 본문\n");
    doc.hwpx_settings_xml = Some(settings.to_string());
    doc.hwpx_version_xml = Some(version.to_string());

    let out = tmp("passthrough_parts.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    // Directly compare both ZIP parts against the original bytes.
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

    // The read round trip also restores the slots unchanged.
    let reread = hwpx::read_document(&out).unwrap().document;
    assert_eq!(reread.hwpx_settings_xml.as_deref(), Some(settings));
    assert_eq!(reread.hwpx_version_xml.as_deref(), Some(version));

    // The JSON round trip preserves the slots too.
    let json = hwp_convert::to_json(&reread, false, false).unwrap();
    let back = hwp_convert::from_json(&json).unwrap();
    assert_eq!(back.hwpx_settings_xml.as_deref(), Some(settings));
    assert_eq!(back.hwpx_version_xml.as_deref(), Some(version));
}

#[test]
fn preview_image_bytes_are_preserved_on_same_format_rewrite() {
    let mut doc = hwp_convert::from_markdown("preview pass-through\n");
    doc.hwpx_preview_image = Some(VALID_PNG_1X1.to_vec());

    let out = tmp("preview-image-passthrough.hwpx");
    let report = hwpx::write_document_with_report(&doc, &out).unwrap();
    assert!(report.preservation.is_lossless());

    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut preview = Vec::new();
    zip.by_name("Preview/PrvImage.png")
        .unwrap()
        .read_to_end(&mut preview)
        .unwrap();
    assert_eq!(preview, VALID_PNG_1X1);

    let reread = hwpx::read_document(&out).unwrap().document;
    assert_eq!(reread.hwpx_preview_image.as_deref(), Some(VALID_PNG_1X1));
}

/// GE-β5 reinforcement: absent slots emit both parts byte-identically to existing defaults,
/// retaining output for old IR and HWP5-source documents.
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

    // Byte-identical to existing constants, including concat! results.
    assert!(settings.contains(r##"<ha:CaretPosition listIDRef="0" paraIDRef="0" pos="0"/>"##));
    assert!(version.contains(r##"application="hwp-cli""##));
    assert!(version.contains(&format!(r##"appVersion="{}""##, env!("CARGO_PKG_VERSION"))));
}

/// GC-5: pass through unparsed hp:secPr children, including grid/startNum/visibility/
/// lineNumberShape/footNotePr/pageBorderFill.
///
/// Use nondefault startNum/grid/visibility/pageBorderFill values in secPr, then parse/write/parse.
/// Retain original children in source order without replacing them with constant templates.
/// Only pagePr is regenerated from PageDef at its original position.
#[test]
fn gc5_secpr_자식_원문_passthrough_왕복() {
    // Use values unlike the template: grid charGrid=7, startNum page=3, visibility HIDE_ALL,
    // lineNumberShape restartType=2 and pageBorderFill borderFillIDRef=2.
    let sec_pr = r##"<hp:secPr id="" textDirection="HORIZONTAL" spaceColumns="1134" tabStop="8000" tabStopVal="4000" tabStopUnit="HWPUNIT" outlineShapeIDRef="1" memoShapeIDRef="0" textVerticalWidthHead="0" masterPageCnt="0"><hp:grid lineGrid="5" charGrid="7" wonggojiFormat="1"/><hp:startNum pageStartsOn="ODD" page="3" pic="2" tbl="4" equation="1"/><hp:visibility hideFirstHeader="1" hideFirstFooter="1" hideFirstMasterPage="0" border="HIDE_ALL" fill="SHOW_ALL" hideFirstPageNum="1" hideFirstEmptyLine="0" showLineNumber="1"/><hp:lineNumberShape restartType="2" countBy="3" distance="100" startNumber="9"/><hp:pagePr landscape="WIDELY" width="59528" height="84186" gutterType="LEFT_ONLY"><hp:margin header="4252" footer="4252" gutter="0" left="8504" right="8504" top="5668" bottom="4252"/></hp:pagePr><hp:footNotePr><hp:autoNumFormat type="DIGIT" userChar="" prefixChar="" suffixChar=")" supscript="0"/></hp:footNotePr><hp:pageBorderFill type="BOTH" borderFillIDRef="2" textBorder="PAPER" headerInside="0" footerInside="0" fillArea="PAPER"><hp:offset left="1417" right="1417" top="1417" bottom="1417"/></hp:pageBorderFill></hp:secPr>"##;
    let xml = format!(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core"><hp:p id="1" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0">{sec_pr}</hp:run></hp:p></hs:sec>"##
    );

    // Read captures unparsed children as raw XML and marks the pagePr position.
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
    // Parse pagePr into PageDef, not raw children; the raw list must contain no pagePr tag.
    assert!(
        !raw.iter().any(|c| c.contains("<hp:pagePr")),
        "pagePr는 원문 아닌 페이지 정의"
    );
    assert_eq!(def.page.expect("페이지 정의").width.0, 59528);

    // Write raw children unchanged in order, regenerating only pagePr from PageDef.
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
    // Template constants charGrid=0 and page=0 did not overwrite the values.
    assert!(
        !out.contains(r##"charGrid="0""##),
        "상수 grid로 대체되지 않음"
    );
    assert!(
        !out.contains(r##"pageStartsOn="BOTH""##),
        "상수 startNum으로 대체되지 않음"
    );

    // Preserve order: grid < startNum < pagePr < footNotePr < pageBorderFill.
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

    // Reparse round trip retains raw XML and PageDef.
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

/// GC-5 reinforcement: empty secpr_raw_children (HWP5 sources/old IR) emits the existing
/// constant template unchanged, protecting the HWP5-to-HWPX synthesis path.
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
        secpr_raw_children: Vec::new(), // No slot (HWP5 source).
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

    // Emit the constant template's standard children unchanged.
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
    // Reflect PageDef.
    assert!(
        out.contains(r##"<hp:pagePr landscape="WIDELY" width="59528" height="84186""##),
        "pagePr 반영"
    );
}

/// GI-XC cross-conversion loss prevention: when HWP5 section raw FOOTNOTE_SHAPE/
/// PAGE_BORDER_FILL data exists, reconstruct observed values instead of constants.
///
/// These raw bytes come from the existing oracle measurement (11.19 RFP HWP, gc23 final layout):
/// three PAGE_BORDER_FILL records in BOTH(border 7, solid)/EVEN(1)/ODD(1) order and two
/// FOOTNOTE_SHAPE records for footnotes/endnotes. BOTH must inherit actual border BF#7.
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
    // Existing oracle-measured raw data from dump BodyText/Section0 --raw.
    let foot: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xff, 0xff, 0xff,
        0xff, 0x52, 0x03, 0x37, 0x02, 0x1b, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];
    let end: Vec<u8> = vec![
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x00, 0x01, 0x00, 0xf8, 0x2f, 0xe0,
        0x00, 0x52, 0x03, 0x37, 0x02, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];
    // BOTH uses solid border 7; EVEN/ODD use no-border ID 1. gap 0x0589=1417.
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

    // BOTH inherits actual border BF#7 instead of being replaced by constant 1.
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

    // Reconstruct observed footnote/endnote values: separator lengths -1 and 14692344 respectively.
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
    // secPr validity: parsing round trip succeeds and retains PageDef.
    let (section2, w2) = hwpx::read::section::parse_section(&out).unwrap();
    assert!(w2.is_empty(), "{w2:?}");
    assert_eq!(section2.section_def().unwrap().page.unwrap().width.0, 59528);
}

/// Attach a gso control (ExtCtrl code 11 + Generic) at the paragraph end.
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

/// Adversarial review (codex), item 1: nondefault HWPX equation attributes survive round trips.
/// Previously only script/sz/pos were read; zOrder, textWrap, baseUnit, color, font and PAGE
/// placement reset to defaults, changing a red 22 pt equation to black 10 pt. Pass raw XML through.
/// Item 2: lineMode is a Hancom model enum (LINE|CHAR); synthetic value 0 violates the schema.
#[test]
fn 수식_속성_원문_보존() {
    use hwp_model::Control;

    // Genuine-shaped equation with nondefault attributes, all different from synthesized defaults.
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
    // Script still comes from canonical IR and is excluded from raw pass-through.
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

    // Without raw XML, synthesis must emit a schema-valid lineMode enum.
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

/// Adversarial review (codex), item 3: restore floating HWP5 equations' PAGE-relative placement,
/// offsets and z-order from the gso common header. PARA/PARA and zero zOrder previously
/// moved page-relative equations to paragraph-relative positions, like picture defect GE-9.
#[test]
fn 수식_hwp5출신_배치_복원() {
    // 24-byte gso common header: attr bit0=not inline, bits3-4=vertical PAGE, bits8-9=horizontal PAGE.
    // + voff + hoff + width + height + zorder.
    let mut data = Vec::new();
    let attr: u32 = (1 << 3) | (1 << 8); // vertRelTo=PAGE(1), horzRelTo=PAGE(1), floating.
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

/// Build one eqed equation control.
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
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    }
}

/// GE-14: emit equations as hp:equation and preserve script/size/placement through round trips.
/// Previously no writer arm existed, dropping equations even when merely editing a document.
/// XML special characters < and & pin the writer esc()/reader entity-decoding pair.
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
        // Characters disappear if esc() emits &lt;/&amp; but the reader fails to decode them.
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
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
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
    // Floating equation offsets belong in hp:pos; zeroing them crowds equations into the top-left corner.
    assert_eq!((got[1].x, got[1].y), (3000, 5000), "부유 오프셋");
}

/// HWP5 text boxes (gso and paragraphs) survive HWPX hp:rect/hp:drawText round trips;
/// previously they were dropped with their text.
#[test]
fn 글상자_hwp5출신_hwpx_왕복() {
    use hwp_model::{CharShapeId, GenericControl, HwpChar, Paragraph, ParagraphList};

    let mut doc = hwp_convert::from_markdown("본문 문단\n\n둘째 문단");
    // HWP5-shaped gso: 40-byte header (attr bit0=inline, size 4000x2000) plus one text-box paragraph.
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
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
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

    // Reread preserves text and restores rectangle geometry, 4000x2000.
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

/// Structured HWPX shapes (ShapeGeom) retain geometry/style across write/read;
/// previously dropped. Check polygon points and rectangle fill/border-color round trips.
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
        border_color: 0x000000FF, // Red (BGR).
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
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
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

/// HWP5 decorative shapes (gso without text) round-trip through HWPX shape elements using existing
/// paired corpus SHAPE_COMPONENT/SC_LINE bytes, checked against Hangul export including lineShape width=32.
#[test]
fn 장식_도형_hwp5출신_hwpx_왕복() {
    use hwp_model::opaque::OpaqueRecord;
    use hwp_model::{GenericControl, ShapeKind};

    // Paired SHAPE_COMPONENT(252B)/SC_LINE(20B) bytes, from the same source as hwp-convert/src/gso.rs tests.
    const LINE_SC: &str = "6e696c246e696c240000000000000000000001006400000064000000c8c1000004000000000000000000e4600000020000000100000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000e17a14ae47017f400000000000000000000000000000000000000000000000007b14ae47e17aa43f0000000000000000000000000000f03f000000000000008000000000000000000000000000000000000000000000f03f00000000000000000000000020000000410000c000010000000000000000000000ffffffff00000000000000000000000000000000000000000001e76b390000";
    const LINE_GEOM: &str = "0000000000000000640000006400000000000000";
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    let mut doc = hwp_convert::from_markdown("본문\n\n둘째");
    // 40-byte gso header: attr=0x042a2211 (inline, PARA/COLUMN), size 49608x4, from the existing pair.
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
        paragraph_lists: Vec::new(), // No text: decorative shape.
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
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    };
    attach_gso(&mut doc.sections[0].paragraphs[1], gso);

    let out = tmp("gso_deco.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    // Written XML matches Hangul export structurally: hp:line, lineShape width=32, treatAsChar=1, PARA/COLUMN.
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

    // Reread restores shape geometry.
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
    // Inline treatment (gso attr bit0=1) restores anchored placement, determining flow position on rerender.
    assert!(s.anchored, "treatAsChar=1 → anchored");
}

// GE-α1 through α7: character effects/number formats, symmetric HWPX-to-IR-to-HWPX-to-IR preservation.
//
// No fixture needed: parse synthetic header XML with each property, write_header, then parse_header
// again and assert preservation. Catch constant/omitted-output regressions in shadows, outlines,
// emboss/engrave, scripts, underline shapes and numbering formats.

/// Wrap charPr child XML, round-trip read/write/read and return original/reread CharShape values.
fn 왕복_charpr(inner: &str) -> (hwp_model::CharShape, hwp_model::CharShape) {
    let xml = format!(
        r##"<hh:head><hh:charProperties itemCnt="1"><hh:charPr id="0" height="1000" textColor="#000000" shadeColor="#FFFFFF">{inner}</hh:charPr></hh:charProperties></hh:head>"##
    );
    let (h1, _) = hwpx::read::header::parse_header(&xml).unwrap();
    let out = hwpx::write::header::write_header(&h1, 1);
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    (h1.char_shapes[0].clone(), h2.char_shapes[0].clone())
}

/// GE-α1 text shadow: preserve type/color/spacing, previously constant NONE.
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

/// GE-α2 outline: preserve presence, previously constant NONE.
#[test]
fn ge_a2_외곽선_왕복() {
    let (cs1, cs2) = 왕복_charpr(r#"<hh:outline type="SOLID"/>"#);
    assert!(cs1.has_outline(), "원본 외곽선 파싱");
    assert!(cs2.has_outline(), "재읽기 외곽선 보존");
    // An absent outline remains absent.
    let (_, cs_none) = 왕복_charpr(r#"<hh:outline type="NONE"/>"#);
    assert!(!cs_none.has_outline(), "NONE은 외곽선 없음 유지");
}

/// GE-α3 emboss survives the round trip, previously omitted.
#[test]
fn ge_a3_양각_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:emboss/>");
    assert!(cs1.is_emboss(), "원본 양각 파싱");
    assert!(cs2.is_emboss(), "재읽기 양각 보존");
    assert!(!cs2.is_engrave(), "음각은 켜지지 않음");
}

/// GE-α3 engrave survives the round trip, previously omitted.
#[test]
fn ge_a3_음각_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:engrave/>");
    assert!(cs1.is_engrave(), "원본 음각 파싱");
    assert!(cs2.is_engrave(), "재읽기 음각 보존");
    assert!(!cs2.is_emboss(), "양각은 켜지지 않음");
}

/// GE-α4 superscript survives the round trip, previously omitted.
#[test]
fn ge_a4_위첨자_왕복() {
    let (cs1, cs2) = 왕복_charpr("<hh:supscript/>");
    assert!(cs1.is_superscript(), "원본 위첨자 파싱");
    assert!(cs2.is_superscript(), "재읽기 위첨자 보존");
    assert!(!cs2.is_subscript(), "아래첨자는 켜지지 않음");
}

/// GE-α4 subscript survives the round trip, previously omitted.
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

/// An IR that sets only the legacy `underline_shape` field (the JSON IR surgery
/// path of `tools/gen_effects_cases.py`) keeps its shape through the HWPX write
/// (#236 dotted, #237 double).
#[test]
fn legacy_underline_shape_field_alone_survives_roundtrip() {
    for (field, expected) in [(3u8, "DOT"), (7, "CIRCLE"), (8, "DOUBLE_SLIM")] {
        let xml = r##"<hh:head><hh:charProperties itemCnt="1"><hh:charPr id="0" height="1000"><hh:underline type="BOTTOM" shape="SOLID" color="#000000"/></hh:charPr></hh:charProperties></hh:head>"##;
        let (mut header, _) = hwpx::read::header::parse_header(xml).unwrap();
        let shape = &mut header.char_shapes[0];
        shape.attr &= !(0xFu32 << 4);
        shape.underline_shape = field;

        let out = hwpx::write::header::write_header(&header, 1);
        assert!(
            out.contains(&format!(
                r#"<hh:underline type="BOTTOM" shape="{expected}""#
            )),
            "serialized {expected}: {out}"
        );
        let (roundtripped, _) = hwpx::read::header::parse_header(&out).unwrap();
        let cs = &roundtripped.char_shapes[0];
        assert_eq!(cs.underline_kind(), 1, "underline kind survives");
        assert_eq!(
            cs.underline_shape, field,
            "legacy field round-trips {expected}"
        );
        assert_eq!(
            cs.underline_shape_code(),
            field - 1,
            "normalized code {expected}"
        );
    }
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

/// GE-α7 paragraph numbering: retain per-level start/numFormat/template, formerly constant ^{{level}}.
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

    // Reread retains the same values.
    let r1 = &h2.numbering_levels[0][0];
    let r2 = &h2.numbering_levels[0][1];
    assert_eq!(r1.start, lv1.start, "시작 번호 보존");
    assert_eq!(r1.fmt, lv1.fmt, "번호 형식 보존");
    assert_eq!(r1.template, lv1.template, "번호 템플릿 보존");
    assert_eq!(r2.fmt, lv2.fmt);
    assert_eq!(r2.template, lv2.template);
}

/// GE-α8: paraPr heading/list links survive write/reread. Constant NONE previously left numbering
/// definitions intact but hid paragraph numbers in Hangul (C6 numbering-format case).
/// Assert the writer reverses the reader's encoded head_type/level/numbering_id
/// values into the output.
#[test]
fn ge_a8_문단머리_heading_왕복() {
    let xml = r#"<hh:head><hh:numberings itemCnt="2"><hh:numbering id="7" start="0"><hh:paraHead start="1" level="1" numFormat="ROMAN_CAPITAL">^1.</hh:paraHead></hh:numbering><hh:numbering id="42" start="0"><hh:paraHead start="1" level="1" numFormat="DIGIT">^1.</hh:paraHead></hh:numbering></hh:numberings><hh:paraProperties itemCnt="1"><hh:paraPr id="0" tabPrIDRef="0"><hh:align horizontal="JUSTIFY" vertical="BASELINE"/><hh:heading type="NUMBER" idRef="42" level="2"/></hh:paraPr></hh:paraProperties></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();

    // Normalize external idRef=42 to the second definition, IR index 1.
    let ps1 = &h1.para_shapes[0];
    assert_eq!(ps1.head_type(), 2, "번호형 머리");
    assert_eq!(ps1.head_level(), 3, "수준 3");
    assert_eq!(ps1.numbering_id, 1, "번호정의 링크");

    // write → re-read.
    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"<hh:heading type="NUMBER" idRef="2" level="2"/>"#),
        "write가 heading을 역방출해야 함: {out}"
    );
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();

    // Reread retains the heading link.
    let ps2 = &h2.para_shapes[0];
    assert_eq!(ps2.head_type(), 2, "재읽기 번호형 머리 보존");
    assert_eq!(ps2.head_level(), 3, "재읽기 수준 보존");
    assert_eq!(ps2.numbering_id, 1, "재읽기 번호정의 링크 보존");

    // Retain numbering definitions too.
    assert_eq!(h2.numbering_levels[1][0].fmt, hwp_model::NumFmt::Digit);
}

#[test]
fn 글머리표_definition_id_왕복() {
    let xml = r#"<hh:head><hh:bullets itemCnt="1"><hh:bullet id="9" char="■" useImage="0"/></hh:bullets><hh:paraProperties itemCnt="1"><hh:paraPr id="0"><hh:heading type="BULLET" idRef="9" level="0"/></hh:paraPr></hh:paraProperties></hh:head>"#;
    let (header, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(header.para_shapes[0].numbering_id, 0);
    assert_eq!(header.bullet_chars, vec!['■']);

    let out = hwpx::write::header::write_header(&header, 1);
    assert!(
        out.contains(r#"<hh:bullet id="1" char="■" useImage="0"/>"#),
        "글머리표 정의 방출: {out}"
    );
    assert!(
        out.contains(r#"<hh:heading type="BULLET" idRef="1" level="0"/>"#),
        "글머리표 참조 방출: {out}"
    );
    let (reread, _) = hwpx::read::header::parse_header(&out).unwrap();
    assert_eq!(reread.para_shapes[0].numbering_id, 0);
    assert_eq!(reread.bullet_chars, vec!['■']);
}

/// GE-α8 reinforcement: paraPr without a heading still emits NONE,
/// keeping default output bytes unchanged.
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

#[test]
fn official_eight_level_hwpx_round_trip() {
    let markdown = "1. level 1\n   1. level 2\n      1. level 3\n         1. level 4\n            1. level 5\n               1. level 6\n                  1. level 7\n                     1. level 8\n";
    let doc = hwp_convert::from_markdown_with(
        markdown,
        &hwp_convert::MarkdownImportOptions {
            preset: Some(hwp_convert::OfficialPreset::Gian),
            ..Default::default()
        },
    );
    let out = tmp("official-eight-level.hwpx");
    hwpx::write_document(&doc, &out).unwrap();

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut header_xml = String::new();
    archive
        .by_name("Contents/header.xml")
        .unwrap()
        .read_to_string(&mut header_xml)
        .unwrap();
    for (level, format, template) in [
        (1, "DIGIT", "^1."),
        (2, "HANGUL_SYLLABLE", "^2."),
        (3, "DIGIT", "^3)"),
        (4, "HANGUL_SYLLABLE", "^4)"),
        (5, "DIGIT", "(^5)"),
        (6, "HANGUL_SYLLABLE", "(^6)"),
        (7, "CIRCLED_DIGIT", "^7"),
        (8, "CIRCLED_HANGUL_SYLLABLE", "^8"),
    ] {
        assert!(
            header_xml.contains(&format!(
                r#"level="{level}" align="LEFT" useInstWidth="1" autoIndent="1" widthAdjust="0" textOffsetType="PERCENT" textOffset="50" numFormat="{format}" charPrIDRef="4294967295" checkable="0">{template}</hh:paraHead>"#
            )),
            "missing level {level}: {header_xml}"
        );
    }
    assert!(
        !header_xml.contains(r#"level="9""#),
        "an official definition must stop at level 8: {header_xml}"
    );
    for (id_ref, level) in (1..=8).zip(0..=7) {
        assert!(
            header_xml.contains(&format!(
                r#"<hh:heading type="NUMBER" idRef="{id_ref}" level="{level}"/>"#
            )),
            "missing zero-based heading level {level}: {header_xml}"
        );
    }
    assert!(
        !header_xml.contains(r#"<hh:heading type="NUMBER" idRef="8" level="8"/>"#),
        "the eighth heading must use Hancom level 7: {header_xml}"
    );

    let reread = hwpx::read_document(&out).unwrap().document;
    assert_eq!(reread.plain_text(), doc.plain_text(), "paragraph order");
    let level8_paragraph = reread
        .sections
        .iter()
        .flat_map(|section| section.paragraphs.iter())
        .find(|paragraph| paragraph.plain_text() == "level 8")
        .expect("generated HWPX must retain the level 8 paragraph");
    let level8_shape = &reread.header.para_shapes[level8_paragraph.para_shape.0 as usize];
    assert_eq!(level8_shape.head_type(), 2, "level 8 is a NUMBER paragraph");
    assert_eq!(
        level8_shape.head_level(),
        8,
        "generated HWPX must retain its internal level-8 paragraph link"
    );
    let level8 = reread
        .header
        .numbering_levels
        .iter()
        .flat_map(|levels| levels.iter())
        .find(|level| level.template == "^8")
        .expect("generated HWPX must retain a level-8 definition");
    assert_eq!(format!("{:?}", level8.fmt), "CircledHangulSyllable");
    assert!(
        reread.header.numbering_levels.iter().all(|levels| {
            levels.len() == 8
                && levels[0].template == "^1."
                && levels[1].template == "^2."
                && levels[2].template == "^3)"
                && levels[3].template == "^4)"
                && levels[4].template == "(^5)"
                && levels[5].template == "(^6)"
                && levels[6].template == "^7"
                && levels[7].template == "^8"
        }),
        "reread must preserve all eight definitions"
    );
}

#[test]
fn existing_seven_level_hwpx_does_not_gain_a_synthetic_eighth_definition() {
    let header = hwp_model::DocHeader {
        numbering_levels: vec![vec![hwp_model::NumLevel::default(); 7]],
        ..Default::default()
    };

    let out = hwpx::write::header::write_header(&header, 1);
    assert!(
        out.contains(r#"level="7""#),
        "seven levels must be emitted: {out}"
    );
    assert!(
        !out.contains(r#"level="8""#),
        "existing seven-level definitions must not gain level 8: {out}"
    );
}

/// GC-4: tabPr/tabItem positions, types and leaders survive HWPX read/write/reread. Previously
/// read retained only tabPrIDRef and discarded tabItem; write emitted empty constants.
#[test]
fn gc4_탭정의_hwpx_왕복() {
    // Two tab definitions: automatic left with right/decimal DASH/DOT stops, and empty automatic right.
    let xml = r#"<hh:head><hh:tabProperties itemCnt="2"><hh:tabPr id="0" autoTabLeft="1" autoTabRight="0"><hh:tabItem pos="4000" type="RIGHT" leader="DASH"/><hh:tabItem pos="8000" type="DECIMAL" leader="DOT"/></hh:tabPr><hh:tabPr id="1" autoTabLeft="0" autoTabRight="1"/></hh:tabProperties></hh:head>"#;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();

    // Check read-side parsing.
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

    // Write items in the genuine hp:switch/case[unit=HWPUNIT,pos=X]/default[pos=2X] structure.
    // Bare tabItem can make Hangul hang and is forbidden.
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

    // Reread only case values, ignoring default to avoid duplicates while retaining values.
    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    assert_eq!(h2.tab_stops[0].items.len(), 2, "재읽기 항목 중복 수집 없음");
    assert_eq!(h2.tab_stops, h1.tab_stops, "왕복 후 탭 정의 완전 보존");
}

/// GC-4 compatibility: read only case(HwpUnitChar,pos=X) from genuine hp:switch,
/// discarding default(pos=2X) so items are not collected twice.
#[test]
fn gc4_정품_switch_구조_case만_읽음() {
    let xml = r#"<hh:head><hh:tabProperties itemCnt="1"><hh:tabPr id="0" autoTabLeft="0" autoTabRight="0"><hp:switch><hp:case hp:required-namespace="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar"><hh:tabItem pos="2900" type="LEFT" leader="NONE" unit="HWPUNIT"/></hp:case><hp:default><hh:tabItem pos="5800" type="LEFT" leader="NONE"/></hp:default></hp:switch><hp:switch><hp:case hp:required-namespace="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar"><hh:tabItem pos="44026" type="RIGHT" leader="DASH" unit="HWPUNIT"/></hp:case><hp:default><hh:tabItem pos="88052" type="RIGHT" leader="DASH"/></hp:default></hp:switch></hh:tabPr></hh:tabProperties></hh:head>"#;
    let (h, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h.tab_stops.len(), 1, "탭 정의 1개");
    let t = &h.tab_stops[0];
    assert_eq!(t.items.len(), 2, "switch 항목 2개(default 중복 배제)");
    // Use the case HWPUNIT position X, never default's 2X.
    assert_eq!(t.items[0].pos, 2900, "첫 항목 case pos=X");
    assert_eq!(t.items[0].kind, 0, "LEFT 탭");
    assert_eq!(t.items[0].fill, 0, "NONE 채움");
    assert_eq!(t.items[1].pos, 44026, "둘째 항목 case pos=X");
    assert_eq!(t.items[1].kind, 1, "RIGHT 탭");
    assert_eq!(t.items[1].fill, 2, "DASH 채움");
}

/// GC-4 compatibility: also read bare tabItem without switch from old output/other implementations.
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

/// GC-4 regression: with no tab definitions, write_tab_properties remains byte-identical to the empty constant.
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

/// Page controls (page number/hide/new number/automatic number) restore byte-identical HWP5 payloads
/// through HWPX round trips, asserting writer attributes and reader build_* are exact inverses.
/// Previously all were dropped by the writer (87 corpus cases).
#[test]
fn 쪽_컨트롤_hwpx_페이로드_왕복() {
    use hwp_model::{Control, GenericControl, HwpChar};

    // (ctrl_id, code, payload): reader build_* layout and existing measured standard values.
    let mut pgnp = vec![0u8; 12];
    pgnp[0..4].copy_from_slice(&(5u32 << 8).to_le_bytes()); // BOTTOM_CENTER
    pgnp[10..12].copy_from_slice(&(u16::from(b'-')).to_le_bytes());
    let pghd = 0x21u32.to_le_bytes().to_vec(); // Hide header and page number (existing observed genuine-cover value).
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
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));
    }
    para.header.ctrl_mask = 0;

    let out = tmp("page_ctrls.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(!warnings.iter().any(|w| w.contains("DROP")), "{warnings:?}");

    // Check the oracle XML format.
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

    // Reread payload bytes are identical.
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

/// Style ladder HWPX round trip: retain H1-H3 section-number prefixes and H2/H3 SECTION indentation;
/// lists round-trip as native numbering/bullet definitions via head_type.
#[test]
fn 왕복_스타일_사다리() {
    let doc = hwp_convert::from_markdown("# 제목\n\n## 절\n\n- 항목\n  - 하위\n\n1. 첫\n");
    let out = tmp("ladder.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let reread = hwpx::read_document(&out).unwrap().document;

    // Preserve text, including section-number prefixes.
    assert_eq!(reread.plain_text(), doc.plain_text());
    assert!(reread.plain_text().contains("1. 제목"));
    assert!(reread.plain_text().contains("1-1. 절"));
    // Native list definitions round-trip: bullet definitions and head_type paragraph shapes.
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
    // H2/H3 SECTION indentation (margin_left 2000, no heading) survives the round trip.
    assert!(
        reread
            .header
            .para_shapes
            .iter()
            .any(|ps| ps.margin_left == 2000 && ps.head_type() == 0),
        "SECTION 문단모양(margin_left 2000)"
    );
}

/// secPr/tabPr round trip: raw secpr_raw_children and semantic tab_stops retain slice-level
/// equivalence between source and HWPX read/write output (Gap B/C).
#[test]
fn 왕복_secpr_tabpr_raw() {
    use hwp_model::Control;
    let src =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    assert!(src.exists(), "커밋된 픽스처 없음: {}", src.display());
    let doc = hwpx::read_document(&src).unwrap().document;

    // Read captures semantic tab definitions and original secPr child XML.
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

    // Write/reread self-consistency.
    let out = tmp("secpr_tabpr_raw.hwpx");
    let warnings = hwpx::write_document(&doc, &out).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let reread = hwpx::read_document(&out).unwrap().document;
    assert_eq!(reread.header.tab_stops, doc.header.tab_stops);

    // ZIP slices: input/output secPr and tabProperties are byte-identical.
    let slice = |path: &std::path::Path, entry: &str, open: &str, close: &str| {
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

/// Footnote/endnote writer arm emits fn/en as hp:footNote/hp:endNote,
/// with note paragraphs surviving the round trip (Gap A, previously dropped).
#[test]
fn 각주_미주_왕복() {
    use hwp_model::{Control, GenericControl, HwpChar, Paragraph, ParagraphList};
    let mut doc = hwp_convert::from_markdown("본문 문장입니다.\n");
    let note = |id: &[u8; 4], txt: &str| {
        // Genuine IR layout: the note body's first run contains automatic-number atno and text.
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
                        caption: None,
                        hwpx_raw_xml: None,
                        container_box: None,
                    })],
                    ..Paragraph::default()
                }],
            }],
            extras: vec![],
            raw_children: vec![],
            gso_shapes: vec![],
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
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
    // Observed Hangul-saved layout: number/suffixChar/instId attributes and body autoNum kind/number.
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

    // Reread restores note paragraphs in paragraph_lists and includes them in plain_text.
    let reread = hwpx::read_document(&out).unwrap().document;
    let text = reread.plain_text();
    assert!(text.contains("각주 내용"), "각주 본문 왕복: {text}");
    assert!(text.contains("미주 내용"), "미주 본문 왕복: {text}");
}

/// GG-15: HWPX round-trips picture transforms and pixel adjustments.
#[test]
fn 그림_변환_보정_속성_hwpx_왕복() {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!("hwpx-gg15-pic-fx-{}", std::process::id()));
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
                p.flip = 1; // Horizontal flip.
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
    let _ = std::fs::remove_dir_all(&dir);
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
            center_x: 0,
            center_y: 0,
            step: 255,
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

/// #135: a genuine Hancom header can self-close a `paraHead` (no format template at
/// all). The writer used to substitute the default `^N.` template for such levels,
/// so a write/re-read cycle fabricated a template the source never had.
#[test]
fn empty_numbering_template_round_trips_as_empty() {
    let xml = r##"<hh:head><hh:numberings itemCnt="1"><hh:numbering id="1" start="0"><hh:paraHead start="1" level="1" numFormat="DIGIT">^1.</hh:paraHead><hh:paraHead start="1" level="9" numFormat="HANGUL_JAMO"/><hh:paraHead start="1" level="10" numFormat="ROMAN_SMALL"/></hh:numbering></hh:numberings></hh:head>"##;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h1.numbering_levels[0].len(), 10);
    assert_eq!(h1.numbering_levels[0][8].template, "");
    assert_eq!(h1.numbering_levels[0][9].template, "");

    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"level="9" align="LEFT" useInstWidth="1" autoIndent="1" widthAdjust="0" textOffsetType="PERCENT" textOffset="50" numFormat="HANGUL_JAMO" charPrIDRef="4294967295" checkable="0"/>"#),
        "an empty template must self-close instead of materializing a default: {out}"
    );

    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    let levels = &h2.numbering_levels[0];
    assert_eq!(levels.len(), 10, "level count must survive the round trip");
    assert_eq!(levels[0].template, "^1.");
    assert_eq!(levels[8].template, "", "level 9 template must stay empty");
    assert_eq!(levels[9].template, "", "level 10 template must stay empty");
    assert_eq!(levels[8].fmt, hwp_model::NumFmt::HangulJamo);
    assert_eq!(levels[9].fmt, hwp_model::NumFmt::RomanLower);
}

/// #135: genuine Hancom files carry an explicit no-fill brush
/// (`winBrush faceColor="none"`), which the reader maps to fill bit 0 plus the
/// none-sentinel color. The writer used to drop the brush for the none-sentinel,
/// so the re-read fill lost the bit entirely.
#[test]
fn no_fill_win_brush_round_trips() {
    let xml = r##"<hh:head><hh:borderFills itemCnt="2"><hh:borderFill id="1" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/><hh:leftBorder type="NONE" width="0.1 mm" color="#000000"/><hh:rightBorder type="NONE" width="0.1 mm" color="#000000"/><hh:topBorder type="NONE" width="0.1 mm" color="#000000"/><hh:bottomBorder type="NONE" width="0.1 mm" color="#000000"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/></hh:borderFill><hh:borderFill id="2" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0"><hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/><hh:leftBorder type="NONE" width="0.1 mm" color="#000000"/><hh:rightBorder type="NONE" width="0.1 mm" color="#000000"/><hh:topBorder type="NONE" width="0.1 mm" color="#000000"/><hh:bottomBorder type="NONE" width="0.1 mm" color="#000000"/><hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/><hc:fillBrush><hc:winBrush faceColor="none" hatchColor="#999999" alpha="0"/></hc:fillBrush></hh:borderFill></hh:borderFills></hh:head>"##;
    let (h1, _) = hwpx::read::header::parse_header(xml).unwrap();
    assert_eq!(h1.border_fills.len(), 2);
    assert_eq!(h1.border_fills[1].fill_type & 0x1, 0x1);
    assert_eq!(h1.border_fills[1].bg_color, Some(0xFFFF_FFFF));

    let out = hwpx::write::header::write_header(&h1, 1);
    assert!(
        out.contains(r#"<hc:winBrush faceColor="none""#),
        "the explicit no-fill brush must be emitted back: {out}"
    );

    let (h2, _) = hwpx::read::header::parse_header(&out).unwrap();
    let bf = &h2.border_fills[1];
    assert_eq!(
        bf.fill_type & 0x1,
        0x1,
        "fill bit 0 must survive the round trip"
    );
    assert_eq!(bf.bg_color, Some(0xFFFF_FFFF));
}
