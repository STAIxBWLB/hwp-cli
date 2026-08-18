//! FIDL-01 CLI-level check: an HWP5 file whose `BorderFill` gradient carries
//! an explicit center/step survives `hwp convert --to hwpx` with those exact
//! values on the emitted `hc:gradation` element, instead of the substituted
//! constants (centerX="0" centerY="0" step="255").

use std::io::Read as _;
use std::path::PathBuf;
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn temp(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("hwp-cli-gradient-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Raw HWP5 gradation block bytes matching the `GradientSpec` below, in the field
/// widths a genuine Hancom-saved file uses: type(u8)=0 (linear), angle(i32)=90,
/// centerX(i32)=25, centerY(i32)=75, spread(i32)=120, num(i32)=2, then two COLORREF
/// u32 stops.
///
/// The earlier version of this helper used i16 fields throughout and claimed to match
/// a genuine file; it did not. A Hancom-saved HWP 5.1.1.0 reference document showed the
/// real layout, and its exact bytes are now pinned by
/// `hwp_model::control::gradient_spec_tests::gradient_spec_parse_hwp5_reads_a_genuine_hancom_block`.
fn gradient_tail_bytes() -> Vec<u8> {
    let mut d = Vec::new();
    d.push(0u8); // type: LINEAR
    d.extend_from_slice(&90i32.to_le_bytes()); // angle
    d.extend_from_slice(&25i32.to_le_bytes()); // centerX
    d.extend_from_slice(&75i32.to_le_bytes()); // centerY
    d.extend_from_slice(&120i32.to_le_bytes()); // spread/step
    d.extend_from_slice(&2i32.to_le_bytes()); // num
    d.extend_from_slice(&0x0000_00FFu32.to_le_bytes());
    d.extend_from_slice(&0x00FF_0000u32.to_le_bytes());
    d
}

#[test]
fn hwp5_gradient_center_and_step_survive_convert_to_hwpx() {
    let mut doc = hwp_convert::from_markdown("그러데이션.\n");
    doc.header.border_fills.push(hwp_model::BorderFill {
        fill_type: 0x4,
        // `tail` carries the raw table-28 bytes verbatim (the hwp5 write path
        // re-emits a non-empty tail unchanged - see write.rs::emit_border_fill),
        // so this is a faithful stand-in for a genuine Hangul-authored file.
        tail: gradient_tail_bytes(),
        gradient: Some(hwp_model::GradientSpec {
            radial: false,
            angle_deg: 90.0,
            center_x: 25,
            center_y: 75,
            step: 120,
            stops: vec![(0.0, 0x0000_00FF), (1.0, 0x00FF_0000)],
        }),
        ..hwp_model::BorderFill::default()
    });

    let input = temp("gradient_source.hwp");
    hwp5::write_document(&doc, &input, &hwp5::WriteOptions::default()).unwrap();

    let output = temp("gradient_output.hwpx");
    let status = hwp()
        .arg("convert")
        .arg(&input)
        .arg("--to")
        .arg("hwpx")
        .arg("-o")
        .arg(&output)
        .status()
        .unwrap();
    assert!(status.success(), "hwp5 -> hwpx convert 실패");

    let bytes = std::fs::read(&output).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/header.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();

    assert!(xml.contains(r#"centerX="25""#), "centerX=25 없음: {xml}");
    assert!(xml.contains(r#"centerY="75""#), "centerY=75 없음: {xml}");
    assert!(xml.contains(r#"step="120""#), "step=120 없음: {xml}");

    std::fs::remove_file(&input).ok();
    std::fs::remove_file(&output).ok();
}
