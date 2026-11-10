//! Opaque picture-caption LIST_HEADER preservation during a real CLI style update.
//! Sentinel bytes are synthetic preservation probes, with no native-layout validity claim.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::{Compression, write::DeflateEncoder};
use hwp_model::{
    Caption, CaptionDirection, CaptionSide, Control, Document, HwpChar, Paragraph, Picture,
};
use hwp5::container::Hwp5Container;
use hwp5::record::{RecordNode, ScanMode, scan_stream, tag};
use serde_json::json;

#[path = "common/temp_path.rs"]
mod temp_path;

fn has_picture_record(nodes: &[RecordNode]) -> bool {
    nodes
        .iter()
        .any(|node| node.tag == tag::SHAPE_COMPONENT_PICTURE || has_picture_record(&node.children))
}

fn picture_node(nodes: &[RecordNode]) -> Option<&RecordNode> {
    for node in nodes {
        if node.tag == tag::CTRL_HEADER
            && node.data.get(..4) == Some(b" osg")
            && has_picture_record(&node.children)
        {
            return Some(node);
        }
        if let Some(found) = picture_node(&node.children) {
            return Some(found);
        }
    }
    None
}

fn picture_node_mut(nodes: &mut [RecordNode]) -> Option<&mut RecordNode> {
    for node in nodes {
        if node.tag == tag::CTRL_HEADER
            && node.data.get(..4) == Some(b" osg")
            && has_picture_record(&node.children)
        {
            return Some(node);
        }
        if let Some(found) = picture_node_mut(&mut node.children) {
            return Some(found);
        }
    }
    None
}

fn picture(document: &Document) -> &Picture {
    document
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("setup must retain the picture owner")
}

fn caption_header(file: &Path) -> Vec<u8> {
    let mut container = Hwp5Container::open(file).unwrap();
    let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
    let roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;
    picture_node(&roots)
        .expect("serialized picture control")
        .children
        .iter()
        .find(|child| child.tag == tag::LIST_HEADER)
        .expect("serialized picture caption header")
        .data
        .clone()
}

fn patch_caption_header(file: &Path, tail: &[u8]) -> Vec<u8> {
    let mut container = Hwp5Container::open(file).unwrap();
    let compressed = container.file_header().is_compressed();
    let bytes = container.read_record_stream("/BodyText/Section0").unwrap();
    drop(container);
    let mut roots = scan_stream(&bytes, ScanMode::Strict).unwrap().roots;
    let header = picture_node_mut(&mut roots)
        .expect("writer-made picture control")
        .children
        .iter_mut()
        .find(|child| child.tag == tag::LIST_HEADER)
        .expect("writer-made caption header");
    assert_eq!(
        header.data.len(),
        22,
        "setup uses the writer-authored caption header"
    );
    let flags = u32::from_le_bytes(header.data[4..8].try_into().unwrap());
    // Bits 3-6 and appended bytes are opaque sentinels, not measured format semantics.
    header.data[4..8].copy_from_slice(&((flags & !0x78) | 0x68).to_le_bytes());
    header.data.extend_from_slice(tail);
    let expected = header.data.clone();
    let serialized = RecordNode::serialize_forest(&roots);
    let encoded = if compressed {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&serialized).unwrap();
        encoder.finish().unwrap()
    } else {
        serialized
    };
    let mut cfb = cfb::open_rw(file).unwrap();
    cfb.create_stream("/BodyText/Section0")
        .unwrap()
        .write_all(&encoded)
        .unwrap();
    cfb.flush().unwrap();
    expected
}

fn restyle_picture_caption(label: &str, tail: &[u8]) {
    let dir = temp_path::TempPath::new("hwp448", label);
    std::fs::create_dir_all(&*dir).unwrap();
    let image = dir.join("pixel.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([20, 80, 160, 255]))
        .save(&image)
        .unwrap();
    let image_bytes = std::fs::read(&image).unwrap();
    let mut document = hwp_convert::from_markdown("Picture anchor.");
    hwp_convert::insert_image(
        &mut document,
        "Picture anchor.",
        &image,
        hwp_convert::ImageSize::Mm(10.0, 10.0),
    )
    .unwrap();
    let style = document.header.styles[0].clone();
    let caption_paragraph = Paragraph {
        chars: "Caption style probe.".chars().map(HwpChar::Text).collect(),
        style: hwp_model::StyleId(0),
        para_shape: style.para_shape,
        char_shape_runs: vec![(0, style.char_shape)],
        ..Paragraph::default()
    };
    let owner = document.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .unwrap();
    owner.caption = Some(Caption {
        side: CaptionSide::Bottom,
        direction: CaptionDirection::Horizontal,
        gap: 250,
        width: None,
        last_width: 0,
        paragraphs: vec![caption_paragraph],
    });
    let source = dir.join("source.hwp");
    hwp5::write_document(&document, &source, &Default::default()).unwrap();
    let expected_header = patch_caption_header(&source, tail);
    assert_eq!(
        caption_header(&source),
        expected_header,
        "setup must persist the opaque header sentinels"
    );
    let before = hwp5::read_document(&source).unwrap().document;
    let before_picture = picture(&before);
    let before_caption = before_picture
        .caption
        .as_ref()
        .expect("reader must retain picture caption");
    assert_eq!(before_caption.paragraphs.len(), 1);
    let before_paragraph = &before_caption.paragraphs[0];
    assert_eq!(before_paragraph.plain_text(), "Caption style probe.");
    assert_eq!(before_paragraph.style.0, 0, "caption inherits style zero");
    assert_eq!(
        before_paragraph.char_shape_runs[0].1, before.header.styles[0].char_shape,
        "caption has no direct character-shape override"
    );
    assert_ne!(
        before.header.char_shapes[before_paragraph.char_shape_runs[0].1.0 as usize].base_size, 1400,
        "setup must require a real caption restyle"
    );
    assert_eq!(
        before.resolve_bin(&before_picture.bin_ref),
        Some(image_bytes.as_slice())
    );
    let source_bytes = std::fs::read(&source).unwrap();
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        serde_json::to_vec(&json!([{"op":"update_style","style":0,"size":"14pt"}])).unwrap(),
    )
    .unwrap();
    let output = dir.join("updated.hwp");
    let result = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "CLI update_style must succeed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let after = hwp5::read_document(&output).unwrap().document;
    let after_picture = picture(&after);
    let after_caption = after_picture
        .caption
        .as_ref()
        .expect("updated picture retains its caption");
    assert_eq!(after_caption.paragraphs.len(), 1);
    let after_paragraph = &after_caption.paragraphs[0];
    assert_eq!(after_paragraph.plain_text(), "Caption style probe.");
    assert_eq!(
        after.header.char_shapes[after_paragraph.char_shape_runs[0].1.0 as usize].base_size, 1400,
        "caption must actually be restyled before preservation is checked"
    );
    assert_eq!(
        after_picture.common_data, before_picture.common_data,
        "untargeted picture common attributes must stay unchanged"
    );
    assert_eq!(
        after_picture.extras, before_picture.extras,
        "untargeted picture shape records must stay unchanged"
    );
    assert_eq!(
        after.resolve_bin(&after_picture.bin_ref),
        Some(image_bytes.as_slice())
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        source_bytes,
        "edit must leave the source unchanged"
    );
    let actual_header = caption_header(&output);
    assert_eq!(
        u32::from_le_bytes(actual_header[4..8].try_into().unwrap()) & 0x78,
        0x68,
        "caption restyling must preserve opaque LIST_HEADER bits 3-6"
    );
    assert_eq!(
        actual_header, expected_header,
        "caption restyling must preserve the entire original LIST_HEADER payload"
    );
}

#[test]
fn update_style_preserves_picture_caption_opaque_header_flags() {
    restyle_picture_caption("flags", &[]);
}

#[test]
fn update_style_preserves_picture_caption_opaque_header_tail() {
    restyle_picture_caption("tail", b"owner-authored-caption-tail");
}
