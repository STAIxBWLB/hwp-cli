//! Ordered-list preservation at the compose/convert boundaries (#486).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

#[path = "common/temp_path.rs"]
mod temp_path;

const CUSTOM: [&str; 7] = ["{n}.", "{n})", "({n})", "{n}-", "[{n}]", "#{n}", "{n}:"];
const DEFAULT: [&str; 7] = ["{n}.", "{n}.", "{n})", "{n})", "({n})", "({n})", "{n}"];

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn workspace(name: &str) -> temp_path::TempPath {
    let dir = temp_path::TempPath::new("hwp-list-markers-486", name);
    std::fs::create_dir_all(&*dir).unwrap();
    dir
}

fn list_spec(markers: &[&str], levels: &[usize], location: &str) -> Value {
    let paragraphs: Vec<_> = levels
        .iter()
        .flat_map(|level| {
            (0..2).map(move |item| {
                json!({
                    "type":"paragraph", "style":"body", "list":{"name":"o", "level":level},
                    "runs":[{"type":"text", "text":format!("level-{level}-item-{item}")}]
                })
            })
        })
        .collect();
    let mut section = json!({"blocks":paragraphs});
    match location {
        "body" => {}
        "cell" => {
            section["blocks"] = json!([{"type":"table", "width_mm":80,
            "columns":[{"width_mm":80}], "rows":[{"cells":[{"blocks":paragraphs}]}]}])
        }
        "header" | "footer" => {
            section["blocks"] = json!([{"type":"paragraph", "style":"body", "runs":[{"type":"text","text":"body"}]}]);
            section[location] = json!({"default":paragraphs});
        }
        _ => panic!("unknown fixture location"),
    }
    json!({"version":"1.0", "styles":{"body":{"font_size_pt":10}},
        "lists":{"o":{"kind":"ordered", "levels":markers.iter().map(|marker|json!({"marker":marker,"start":1})).collect::<Vec<_>>()}},
        "sections":[section]})
}

fn compose_hwpx(dir: &Path) -> PathBuf {
    let spec = dir.join("spec.json");
    std::fs::write(
        &spec,
        list_spec(&CUSTOM, &[0, 1, 2, 3, 4, 5, 6], "body").to_string(),
    )
    .unwrap();
    let output = dir.join("custom.hwpx");
    let run = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "HWPX preserves the authored templates: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    output
}

#[test]
fn compose_hwp_refuses_custom_ordered_markers_before_publication() {
    let dir = workspace("custom-compose");
    let spec = dir.join("spec.json");
    std::fs::write(
        &spec,
        list_spec(&CUSTOM, &[0, 1, 2, 3, 4, 5, 6], "body").to_string(),
    )
    .unwrap();
    let output = dir.join("out.hwp");
    std::fs::write(&output, b"existing output").unwrap();
    let run = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        !run.status.success(),
        "custom list markers must not silently become the HWP default ladder"
    );
    let error = String::from_utf8_lossy(&run.stderr);
    assert!(
        error.contains("marker/start") && error.contains("HWPX"),
        "{error}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
}

#[test]
fn compose_hwp_refuses_used_non_digit_default_slots_in_every_block_scope() {
    let dir = workspace("used-format");
    for location in ["body", "cell", "header", "footer"] {
        let spec = dir.join(format!("{location}.json"));
        std::fs::write(&spec, list_spec(&DEFAULT, &[1], location).to_string()).unwrap();
        let output = dir.join(format!("{location}.hwp"));
        let run = hwp()
            .arg("compose")
            .arg(&spec)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            !run.status.success(),
            "{location}: default marker text does not make a Digit request preservable at a Hangul-format slot"
        );
        let error = String::from_utf8_lossy(&run.stderr);
        assert!(
            error.contains("번호 형식(Digit)") && error.contains("HWPX"),
            "{location}: {error}"
        );
        assert!(!output.exists());
    }
}

#[test]
fn convert_hwp_warns_instead_of_silently_substituting_custom_numbering() {
    let dir = workspace("cli-convert");
    let input = compose_hwpx(&dir);
    let output = dir.join("converted.hwp");
    let run = hwp()
        .arg("convert")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "conversion remains allowed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let warning = String::from_utf8_lossy(&run.stderr);
    assert!(
        warning.contains("numbering_default_substitution"),
        "conversion must disclose numbering substitution: {warning}"
    );
    assert!(
        warning.contains("definition=0") && warning.contains("HWPX"),
        "{warning}"
    );
    assert!(output.is_file());
}

#[test]
fn convert_sample_default_ladder_does_not_warn_about_unused_extended_levels() {
    let dir = workspace("default-sample-convert");
    let input =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    let run = hwp()
        .arg("convert")
        .arg(input)
        .arg("-o")
        .arg(dir.join("out.hwp"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(
        !stderr.contains("numbering_default_substitution"),
        "unused HWPX extension levels must not warn: {stderr}"
    );
}

#[test]
fn new_markdown_default_list_does_not_warn_about_unused_eighth_level() {
    let dir = workspace("default-markdown-new");
    let input = dir.join("list.md");
    std::fs::write(&input, "1. First item\n2. Second item\n").unwrap();
    let run = hwp()
        .args(["new", "--from"])
        .arg(input)
        .arg("-o")
        .arg(dir.join("out.hwp"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(
        !stderr.contains("numbering_default_substitution"),
        "unused markdown extension level must not warn: {stderr}"
    );
}

#[test]
fn mcp_convert_returns_numbering_substitution_in_its_warning_contract() {
    let dir = workspace("mcp-convert");
    let input = compose_hwpx(&dir);
    let output = dir.join("converted.hwp");
    let mut child = hwp()
        .args(["mcp", "--root"])
        .arg(&*dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for message in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"list-marker-regression","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"hwp_convert","arguments":{"input":input,"output":output}}}),
        ] {
            writeln!(stdin, "{message}").unwrap();
        }
    }
    let run = child.wait_with_output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let response: Value = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["id"] == 2)
        .unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_ne!(response["result"]["isError"], true, "{response}");
    let payload: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let warnings = payload["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap()
            .contains("numbering_default_substitution")),
        "MCP ConvertReport.warnings must disclose substitution: {payload}"
    );
    assert!(output.is_file());
}
