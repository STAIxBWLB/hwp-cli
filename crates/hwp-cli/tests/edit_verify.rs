//! `hwp edit --verify` semantic-hash regression for #135.
//!
//! `--verify` compares the semantic hash of the edit result against the same
//! document re-read from disk. Two HWPX writer fidelity gaps made that check
//! fail on genuine Hancom documents whose headers carry (a) an explicit no-fill
//! border fill (`winBrush faceColor="none"`, read as fill bit 0 + the
//! none-sentinel color) and (b) numbering levels with an empty `paraHead`
//! template: the writer dropped the no-fill brush and substituted a default
//! `^N.` template, so the re-read header diverged from the edit result. This
//! test synthesizes exactly those two constructs on top of a generated document
//! — the private fixture that surfaced the issue is gitignored and must not be
//! committed — and requires `--verify` to accept the edit.

use std::path::PathBuf;
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-cli-edit-verify-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A generated (committable) document plus the two header constructs that tripped
/// #135. The body table gives `--style-tables` a real target so the edit publishes.
#[test]
fn verify_passes_with_no_fill_brush_and_empty_numbering_templates() {
    let mut doc = hwp_convert::from_markdown("| 가 | 나 |\n|---|---|\n| 1 | 2 |\n");

    // (a) Explicit no-fill border fill, as genuine files emit it.
    doc.header.border_fills.push(hwp_model::BorderFill {
        fill_type: 0x1,
        bg_color: Some(0xFFFF_FFFF),
        ..Default::default()
    });

    // (b) A 10-level numbering whose top levels have no format template at all
    // (genuine files self-close those paraHead elements).
    doc.header.numbering_levels.push(
        (1..=10u32)
            .map(|level| hwp_model::NumLevel {
                start: 1,
                fmt: hwp_model::NumFmt::Digit,
                template: if level >= 9 {
                    String::new()
                } else {
                    format!("^{level}.")
                },
            })
            .collect(),
    );

    let input = tmp("verify-roundtrip-input.hwpx");
    hwpx::write_document(&doc, &input).expect("write synthetic #135 trigger");
    let output = tmp("verify-roundtrip-output.hwpx");

    let report = hwp()
        .arg("edit")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .args(["--style-tables", "official", "--verify"])
        .output()
        .unwrap();
    assert!(
        report.status.success(),
        "hwp edit --verify must accept a faithful write/re-read cycle (#135): {}",
        String::from_utf8_lossy(&report.stderr)
    );

    // The constructs themselves must survive the cycle, not just the hash: the
    // re-read output still carries the no-fill brush and the empty templates.
    let reread = hwpx::read_document(&output).unwrap().document;
    assert!(
        reread
            .header
            .border_fills
            .iter()
            .any(|bf| bf.fill_type & 0x1 != 0 && bf.bg_color == Some(0xFFFF_FFFF)),
        "the explicit no-fill border fill must survive the edit round trip"
    );
    let numbering = reread
        .header
        .numbering_levels
        .iter()
        .find(|levels| levels.len() == 10)
        .expect("the 10-level numbering must survive the edit round trip");
    assert_eq!(numbering[8].template, "");
    assert_eq!(numbering[9].template, "");
}
