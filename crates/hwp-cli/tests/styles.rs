//! `hwp styles` (#427 T2): the named-style listing, `schemas/styles-v1.schema.json`.
//!
//! AC1: every named style is listed with resolved attributes, and `used_by` holds the segment
//! ids `hwp cat --with-segments --segments v2` publishes. The envelope does not carry a
//! paragraph's style id (it is closed), so the oracle here is the document IR that the json
//! envelope carries beside its segments: each `para` segment's path is walked down the IR to its
//! paragraph, and that paragraph's own `style` field says which listing row must hold the id.

#[path = "common/fixture_skip.rs"]
mod fixture_skip;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn repo(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// A committed document with nested paragraphs (tables, text boxes) and several named styles.
fn sample() -> PathBuf {
    repo("fixtures/samples/report-tables.hwpx")
}

fn run_json(args: &[&str], file: &PathBuf) -> Value {
    let out = hwp().args(args).arg(file).output().expect("run hwp");
    assert!(
        out.status.success(),
        "hwp {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON value")
}

fn styles(file: &PathBuf) -> Value {
    run_json(&["styles", "--format", "json"], file)
}

fn envelope(file: &PathBuf) -> Value {
    // `cat <file> --format json ...`: the file argument is positional, so it goes first.
    let out = hwp()
        .arg("cat")
        .arg(file)
        .args(["--format", "json", "--with-segments", "--segments", "v2"])
        .output()
        .expect("run hwp cat");
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).expect("the envelope parses")
}

fn validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/styles-v1.schema.json"))
            .expect("the schema is valid JSON");
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .expect("the schema compiles as Draft 2020-12")
}

/// Walks a segment path down the IR JSON to the paragraph it names: `indices[0]` is the
/// top-level paragraph; below it a table steps `[control, cell, paragraph]` and a generic
/// control `[control, flat paragraph index across its lists]`.
fn paragraph_at<'d>(document: &'d Value, section: usize, indices: &[usize]) -> &'d Value {
    let mut para = &document["sections"][section]["paragraphs"][indices[0]];
    let mut rest = &indices[1..];
    while !rest.is_empty() {
        let control = &para["controls"][rest[0]];
        if let Some(table) = control.get("Table") {
            para = &table["cells"][rest[1]]["paragraphs"][rest[2]];
            rest = &rest[3..];
        } else {
            let generic = control.get("Generic").expect("a table or generic control");
            let mut remaining = rest[1];
            let mut found = None;
            for list in generic["paragraph_lists"].as_array().unwrap() {
                let paragraphs = list["paragraphs"].as_array().unwrap();
                if remaining < paragraphs.len() {
                    found = Some(&paragraphs[remaining]);
                    break;
                }
                remaining -= paragraphs.len();
            }
            para = found.expect("the generic control has that paragraph");
            rest = &rest[2..];
        }
    }
    para
}

/// AC1, checked per document: the listing validates, every table row is listed once in table
/// order, and each paragraph segment's id sits in the row its paragraph's style names.
fn assert_listing_matches_the_envelope(file: &PathBuf) {
    let listing = styles(file);
    let errors: Vec<String> = validator()
        .iter_errors(&listing)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "listing violates its schema: {errors:?}");

    let env = envelope(file);
    let table = env["document"]["header"]["styles"].as_array().unwrap();
    let rows = listing["styles"].as_array().unwrap();
    assert_eq!(rows.len(), table.len(), "one row per style table entry");
    for (index, (row, style)) in rows.iter().zip(table).enumerate() {
        assert_eq!(row["id"], index, "rows are in table order");
        assert_eq!(row["name"], style["name"]);
        assert_eq!(row["english_name"], style["english_name"]);
        assert_eq!(row["next_style"], style["next_style"]);
    }

    let mut expected: Vec<BTreeSet<String>> = vec![BTreeSet::new(); rows.len()];
    let mut para_segments = 0;
    for segment in env["segments"].as_array().unwrap() {
        if segment["kind"] != "para" {
            continue;
        }
        para_segments += 1;
        let indices: Vec<usize> = segment["path"]["indices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_u64().unwrap() as usize)
            .collect();
        let section = segment["path"]["section"].as_u64().unwrap() as usize;
        let paragraph = paragraph_at(&env["document"], section, &indices);
        let style = paragraph["style"].as_u64().unwrap() as usize;
        assert!(style < rows.len(), "the sample names only listed styles");
        expected[style].insert(segment["id"].as_str().unwrap().to_string());
    }
    assert!(para_segments > 0, "the sample must have paragraph segments");

    let mut listed = 0;
    for (row, want) in rows.iter().zip(&expected) {
        let got: Vec<&str> = row["used_by"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect();
        listed += got.len();
        let got_set: BTreeSet<String> = got.iter().map(|id| id.to_string()).collect();
        assert_eq!(
            got_set.len(),
            got.len(),
            "style {} lists an id twice",
            row["id"]
        );
        assert_eq!(&got_set, want, "used_by of style {}", row["id"]);
    }
    assert_eq!(
        listed, para_segments,
        "every paragraph segment is listed once"
    );
}

#[test]
fn the_listing_joins_the_segment_envelope_on_nested_paragraphs() {
    assert_listing_matches_the_envelope(&sample());
}

#[test]
fn the_listing_joins_the_envelope_for_hwp5_and_hwpx_alike() {
    for ext in ["hwp", "hwpx"] {
        assert_listing_matches_the_envelope(&repo(&format!(
            "fixtures/pdf-parity/public/source/public-safety-rfp-p1.{ext}"
        )));
    }
}

/// The sample spreads its paragraphs over several styles, so the join above is not satisfied by
/// everything landing in row 0.
#[test]
fn the_sample_uses_more_than_one_style() {
    let listing = styles(&sample());
    let used: Vec<_> = listing["styles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| !s["used_by"].as_array().unwrap().is_empty())
        .collect();
    assert!(used.len() >= 5, "styles in use: {}", used.len());
}

#[test]
fn attrs_are_the_envelopes_style_level_for_a_paragraph_of_that_style() {
    let file = sample();
    let listing = styles(&file);
    let env = envelope(&file);
    let mut checked = 0;
    for segment in env["segments"].as_array().unwrap() {
        if segment["kind"] != "para" {
            continue;
        }
        let id = segment["id"].as_str().unwrap();
        let row = listing["styles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["used_by"].as_array().unwrap().iter().any(|u| u == id))
            .expect("every sample paragraph names a listed style");
        assert_eq!(row["attrs"], segment["style"], "attrs of segment {id}");
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn kind_is_stated_for_hwp5_and_unknown_for_hwpx() {
    let hwp5 = styles(&repo(
        "fixtures/pdf-parity/public/source/public-safety-rfp-p1.hwp",
    ));
    assert!(
        hwp5["styles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["kind"] == "paragraph")
    );
    // The HWPX reader never reads `hh:style@type` (GE-13): null, not a guessed "paragraph".
    let hwpx = styles(&sample());
    assert!(
        hwpx["styles"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["kind"].is_null())
    );
}

/// A genuine file saved by Hangul has one character style, `쪽 번호`.
#[test]
fn a_genuine_character_style_is_listed_as_one() {
    let file = repo("fixtures/hwp5/bookmark.hwp");
    if fixture_skip::fixture_missing(&file) {
        return;
    }
    let listing = styles(&file);
    let kinds: Vec<(&str, &str)> = listing["styles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["kind"].as_str().unwrap()))
        .collect();
    assert!(kinds.contains(&("쪽 번호", "character")), "{kinds:?}");
    assert_eq!(
        kinds.iter().filter(|(_, k)| *k == "character").count(),
        1,
        "{kinds:?}"
    );
}

#[test]
fn the_schema_is_closed_and_rejects_an_unknown_field() {
    let mut listing = styles(&sample());
    assert!(validator().is_valid(&listing));
    listing["styles"][0]["attrs"]["char"]["size_pnt"] = serde_json::json!(10.0);
    assert!(
        !validator().is_valid(&listing),
        "a misspelled field must fail"
    );
    let mut listing = styles(&sample());
    listing["styles"][0]["extra"] = serde_json::json!(1);
    assert!(!validator().is_valid(&listing));
    let mut listing = styles(&sample());
    listing["contract"] = serde_json::json!("hwp-segment-envelope-v2");
    assert!(!validator().is_valid(&listing));
}

#[test]
fn the_text_form_lists_every_style() {
    let out = hwp()
        .args(["styles"])
        .arg(sample())
        .output()
        .expect("run hwp styles");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("스타일 30개:"), "{text}");
    assert!(text.contains("\"바탕글\""), "{text}");
    assert_eq!(text.lines().count(), 31);
}

/// MCP parity (AC6 for the read surface): the tool returns the same document as the CLI.
#[test]
fn the_mcp_tool_returns_the_cli_listing() {
    use std::io::Write as _;
    use std::process::Stdio;

    let file = sample();
    let mut child = hwp()
        .arg("mcp")
        .current_dir(repo("."))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hwp mcp");
    let mut stdin = child.stdin.take().unwrap();
    for message in [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"styles-test","version":"0"}}}),
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"hwp_styles","arguments":{"path": file.to_string_lossy()}}}),
    ] {
        writeln!(stdin, "{message}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let reply: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|v| v["id"] == 2)
        .expect("a tools/call reply");
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let from_mcp: Value = serde_json::from_str(text).unwrap();
    assert_eq!(from_mcp, styles(&file));
}

/// A style table longer than a paragraph's `StyleId` can address is refused with its cause and
/// the next step, over the JSON IR loader that accepts an unbounded `header.styles` (T15-F1).
#[test]
fn an_oversized_style_table_is_refused_instead_of_wrapping_ids() {
    let out = hwp()
        .args(["cat", "--format", "json"])
        .arg(sample())
        .output()
        .expect("run hwp cat");
    assert!(out.status.success());
    let mut ir: Value = serde_json::from_slice(&out.stdout).unwrap();
    let template = ir["header"]["styles"][0].clone();
    ir["header"]["styles"] = Value::Array(vec![template; 65_537]);
    let dir = std::env::temp_dir().join(format!("hwp-styles-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oversized.json");
    std::fs::write(&path, serde_json::to_vec(&ir).unwrap()).unwrap();

    let run = hwp()
        .arg("styles")
        .arg(&path)
        .output()
        .expect("run hwp styles");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!run.status.success(), "an oversized table must be refused");
    assert!(
        run.stdout.is_empty(),
        "nothing is listed before the refusal"
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("65537") && stderr.contains("65536") && stderr.contains("다시 변환"),
        "the refusal states the cause and the next step: {stderr}"
    );
}
