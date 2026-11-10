//! Writer-authored HWP picture identities after a merge followed by an image insertion.
//! This synthetic contract checks our serialized records, not native Hangul acceptance.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use hwp5::container::Hwp5Container;
use hwp5::record::{RecordNode, ScanMode, scan_stream, tag};
use serde_json::json;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("hwp-picture-instance-ids-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn collect_picture_records(nodes: &[RecordNode], records: &mut Vec<Vec<u8>>) {
    for node in nodes {
        if node.tag == tag::SHAPE_COMPONENT_PICTURE {
            records.push(node.data.clone());
        }
        collect_picture_records(&node.children, records);
    }
}

fn picture_records(file: &Path) -> Vec<Vec<u8>> {
    let mut container = Hwp5Container::open(file).unwrap();
    let sections = container.body_sections();
    assert!(
        !sections.is_empty(),
        "setup must have serialized body sections"
    );
    let mut records = Vec::new();
    for section in sections {
        let bytes = container.read_record_stream(&section).unwrap();
        let scanned = scan_stream(&bytes, ScanMode::Strict).unwrap();
        assert!(scanned.warnings.is_empty());
        collect_picture_records(&scanned.roots, &mut records);
    }
    records
}

fn picture_id(record: &[u8]) -> u32 {
    // Only writer-authored records are inspected here. build_picture_extras in
    // crates/hwp5/src/write.rs emits its picture instance ID at bytes 74..78.
    assert_eq!(
        record.len(),
        91,
        "setup uses the writer's 91-byte picture layout"
    );
    u32::from_le_bytes(record[74..78].try_into().unwrap())
}

#[test]
fn merged_raw_picture_and_inserted_image_keep_distinct_picture_instance_ids() {
    let fixture = Fixture::new();
    let image = fixture.path("pixel.png");
    // Generate an owner-authored pixel rather than importing an image or native document.
    image::RgbaImage::from_pixel(1, 1, image::Rgba([200, 30, 50, 255]))
        .save(&image)
        .unwrap();

    let first = hwp_convert::from_markdown("Primary anchor.\n\nPrimary tail.");
    let mut picture_source = hwp_convert::from_markdown("Secondary anchor.");
    hwp_convert::insert_image(
        &mut picture_source,
        "Secondary anchor.",
        &image,
        hwp_convert::ImageSize::Mm(10.0, 10.0),
    )
    .unwrap();
    let source_path = fixture.path("picture-source.hwp");
    hwp5::write_document(&picture_source, &source_path, &Default::default()).unwrap();
    let source_records = picture_records(&source_path);
    assert_eq!(
        source_records.len(),
        1,
        "source setup must contain one picture"
    );
    let original_record = &source_records[0];
    let original_id = picture_id(original_record);
    assert_ne!(original_id, 0, "writer-authored source ID must be nonzero");
    hwp5::read_document(&source_path).expect("writer-made source must read successfully");
    let primary_path = fixture.path("primary.hwp");
    hwp5::write_document(&first, &primary_path, &Default::default()).unwrap();
    assert!(
        picture_records(&primary_path).is_empty(),
        "first input must be picture-free"
    );
    let merged_path = fixture.path("merged.hwp");
    let merge = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("merge")
        .arg(&primary_path)
        .arg(&source_path)
        .arg("-o")
        .arg(&merged_path)
        .output()
        .unwrap();
    assert!(
        merge.status.success(),
        "real CLI merge must succeed: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let merged_records = picture_records(&merged_path);
    assert_eq!(
        merged_records, source_records,
        "merge setup must preserve the raw picture record"
    );
    let merged_doc = hwp5::read_document(&merged_path).unwrap().document;
    assert_eq!(
        merged_doc.sections.len(),
        2,
        "CLI merge retains both input sections"
    );
    let merged_picture = merged_doc.sections[1]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("the second section must retain the source picture");
    let common_id = u32::from_le_bytes(merged_picture.common_data[32..36].try_into().unwrap());
    assert_ne!(
        common_id ^ 0x0010_0000,
        original_id,
        "merge setup must separate the common id from the preserved picture-record id"
    );
    let anchor = merged_doc.sections[0]
        .paragraphs
        .iter()
        .position(|paragraph| paragraph.plain_text() == "Primary anchor.")
        .expect("picture-free first input must retain its insertion anchor");

    let ops = fixture.path("ops.json");
    std::fs::write(
        &ops,
        serde_json::to_vec(&json!([{
            "op":"insert_blocks",
            "address":{"at":{"section":0,"paragraph":anchor}},
            "position":"after",
            "missing_style":"create",
            "styles":{},
            "blocks":[{"type":"image","path":"pixel.png","width_mm":10}]
        }]))
        .unwrap(),
    )
    .unwrap();
    let output_path = fixture.path("inserted.hwp");
    let output = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("edit")
        .arg(&merged_path)
        .arg("-o")
        .arg(&output_path)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "real CLI image insertion must succeed before checking identities: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hwp5::read_document(&output_path).expect("inserted HWP must read successfully");
    let records = picture_records(&output_path);
    assert_eq!(
        records.len(),
        2,
        "both the preserved and inserted pictures must be serialized"
    );
    assert!(
        records.contains(original_record),
        "insertion must preserve the original raw picture record"
    );
    let ids: Vec<_> = records.iter().map(|record| picture_id(record)).collect();
    assert!(
        ids.contains(&original_id),
        "the source picture ID must survive"
    );
    assert_eq!(
        ids.iter().copied().collect::<BTreeSet<_>>().len(),
        ids.len(),
        "preserved raw and newly inserted pictures must not share a picture instance ID: {ids:?}"
    );
}
