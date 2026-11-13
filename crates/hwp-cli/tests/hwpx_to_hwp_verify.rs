//! HWP5-target verification must compare represented semantics without accepting real loss.
//! Public inputs are generated at test time; no private document or binary fixture is copied.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "common/fixture_skip.rs"]
mod fixture_skip;
#[path = "common/temp_path.rs"]
mod temp_path;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

fn workspace(label: &str) -> temp_path::TempPath {
    let dir = temp_path::TempPath::new("hwp444", label);
    std::fs::create_dir_all(&*dir).unwrap();
    dir
}

fn metadata_edit(input: &Path, output: &Path) -> Output {
    command()
        .arg("edit")
        .arg(input)
        .args(["--set-meta", "title=t", "--verify", "-o"])
        .arg(output)
        .output()
        .unwrap()
}

fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[test]
fn synthetic_hwpx_metadata_edit_verifies_in_hwp() {
    for table in [false, true] {
        let dir = workspace(if table { "table" } else { "body" });
        let mut source = hwp_convert::from_markdown(if table {
            "Baseline input.\n\n| Field | Value |\n| --- | --- |\n| alpha | 1 |\n"
        } else {
            "Baseline input.\n"
        });
        // CHAR_SHAPE underline bits are already supported by the writer (§4.2.6).
        source.header.char_shapes[0].attr |= (1 << 2) | (2 << 4);
        source.header.char_shapes[0].underline_shape = 3;
        let input = dir.join("source.hwpx");
        hwpx::write_document(&source, &input).unwrap();
        let bytes = std::fs::read(&input).unwrap();
        let output = dir.join("edited.hwp");
        let result = metadata_edit(&input, &output);
        assert!(
            result.status.success(),
            "faithful HWPX metadata edit must pass HWP5 verification (table={table}): {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let reread = hwp5::read_document(&output).unwrap().document;
        assert_eq!(reread.metadata.title.as_deref(), Some("t"));
        assert!(reread.plain_text().contains("Baseline input."));
        assert_eq!(reread.header.char_shapes[0].underline_shape_code(), 2);
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
    }
}

#[test]
fn composed_hwpx_text_edit_verifies_in_hwp() {
    let dir = workspace("compose");
    let spec = dir.join("source.json");
    std::fs::write(
        &spec,
        serde_json::to_vec(&serde_json::json!({
            "version":"1.0",
            "sections":[{"blocks":[{"type":"paragraph","runs":[
                {"type":"text","text":"Baseline input."}
            ]}]}]
        }))
        .unwrap(),
    )
    .unwrap();
    let input = dir.join("composed.hwpx");
    let composed = command()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        composed.status.success(),
        "compose setup: {}",
        String::from_utf8_lossy(&composed.stderr)
    );
    let output = dir.join("edited.hwp");
    let result = command()
        .arg("edit")
        .arg(&input)
        .args([
            "--replace",
            "Baseline input.=>Baseline output.",
            "--verify",
            "-o",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "compose HWPX text edit must pass HWP5 verification: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        hwp5::read_document(&output)
            .unwrap()
            .document
            .plain_text()
            .contains("Baseline output.")
    );
}

#[test]
fn genuine_minimal_hwpx_metadata_edit_verifies_in_hwp() {
    let input = fixture("fixtures/hwpx/minimal.hwpx");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = workspace("minimal");
    let output = dir.join("edited.hwp");
    let result = metadata_edit(&input, &output);
    assert!(
        result.status.success(),
        "minimal HWPX must pass HWP5 verification: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn decorative_shape_loss_still_refuses_publication() {
    let input = fixture("fixtures/samples/report-tables.hwpx");
    let dir = workspace("decorative-loss");
    let output = dir.join("refused.hwp");
    let result = metadata_edit(&input, &output);
    assert!(
        !result.status.success(),
        "GE-1 decorative shape loss must refuse"
    );
    assert!(
        !output.exists(),
        "lossy decorative-shape output must not publish"
    );
}

#[test]
fn opaque_equation_loss_still_refuses_publication() {
    let input = fixture("fixtures/hwpx/equation-ampersand-groundtruth.hwpx");
    if fixture_skip::fixture_missing(&input) {
        return;
    }
    let dir = workspace("equation-loss");
    let output = dir.join("refused.hwp");
    let result = metadata_edit(&input, &output);
    assert!(!result.status.success(), "opaque equation loss must refuse");
    assert!(String::from_utf8_lossy(&result.stderr).contains("opaque_control_unrepresentable"));
    assert!(!output.exists(), "lossy equation output must not publish");
}
