//! #543: `cat --segments v2` names the tab, hyphen, non-breaking space and fixed-width space
//! controls a `run` projects as ordinary text.
//!
//! The document is built as IR, written with the hwp5 writer and read back through the real
//! binary, so the controls go through the same load path a saved file takes. Each published
//! `controls` entry is checked in both coordinate spaces: `at` must index the projected
//! character in the envelope's own `markdown`, and `source` must be the control's WCHAR offset in
//! the paragraph, inside the run's `source_chars`. The envelope must validate against the
//! published schema, and a run with none of the four controls must carry no `controls` key.

use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::header::{CharShape, ParaShape, Style};
use hwp_model::ids::{CharShapeId, ParaShapeId};
use hwp_model::{Document, HwpChar, Paragraph, Section, ctrl_char};

#[path = "common/temp_path.rs"]
mod temp_path;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn document_of(paragraphs: Vec<Paragraph>) -> Document {
    let mut doc = Document {
        sections: vec![Section {
            paragraphs,
            ..Default::default()
        }],
        ..Default::default()
    };
    doc.header.char_shapes = vec![CharShape {
        base_size: 1000,
        ..Default::default()
    }];
    doc.header.para_shapes = vec![ParaShape::default()];
    doc.header.styles = vec![Style {
        name: "바탕글".into(),
        char_shape: CharShapeId(0),
        para_shape: ParaShapeId(0),
        ..Default::default()
    }];
    doc.header.fonts[0] = vec![hwp_model::header::FaceName {
        name: "함초롬바탕".into(),
        ..Default::default()
    }];
    doc
}

/// `앞<TAB>가<HYPHEN>나<NB_SPACE>다<FW_SPACE>뒤`, then a second paragraph with none.
fn controls_document() -> Document {
    let mut chars = vec![HwpChar::Text('앞')];
    chars.push(HwpChar::InlineCtrl {
        code: ctrl_char::TAB,
        payload: vec![0; 12],
    });
    chars.push(HwpChar::Text('가'));
    chars.push(HwpChar::CharCtrl(ctrl_char::HYPHEN));
    chars.push(HwpChar::Text('나'));
    chars.push(HwpChar::CharCtrl(ctrl_char::NB_SPACE));
    chars.push(HwpChar::Text('다'));
    chars.push(HwpChar::CharCtrl(ctrl_char::FW_SPACE));
    chars.push(HwpChar::Text('뒤'));
    let with = Paragraph {
        chars,
        char_shape_runs: vec![(0, CharShapeId(0))],
        ..Default::default()
    };
    let without = Paragraph {
        chars: "보통 a-b".chars().map(HwpChar::Text).collect(),
        char_shape_runs: vec![(0, CharShapeId(0))],
        ..Default::default()
    };
    document_of(vec![with, without])
}

fn write_doc(dir: &Path, doc: &Document) -> PathBuf {
    let path = dir.join("controls.hwp");
    hwp5::write_document(doc, &path, &hwp5::WriteOptions::default()).expect("write the document");
    path
}

fn envelope(file: &Path) -> serde_json::Value {
    let out = hwp()
        .arg("cat")
        .arg(file)
        .args([
            "--format",
            "markdown",
            "--with-segments",
            "--segments",
            "v2",
        ])
        .output()
        .expect("run cat");
    assert!(
        out.status.success(),
        "cat --segments v2 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("the v2 envelope parses")
}

fn validator() -> jsonschema::Validator {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/segment-envelope-v2.schema.json"
    ))
    .expect("schema JSON");
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .expect("schema compiles")
}

fn runs_of_paragraph(env: &serde_json::Value, para: u64) -> Vec<serde_json::Value> {
    env["segments"]
        .as_array()
        .expect("segments")
        .iter()
        .filter(|s| s["kind"] == "run" && s["path"]["indices"][0] == para)
        .cloned()
        .collect()
}

#[test]
fn each_projected_control_is_published_with_both_offsets() {
    let dir = temp_path::TempPath::new("hwp543", "controls");
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    let file = write_doc(&dir, &controls_document());
    let env = envelope(&file);
    if let Err(error) = validator().validate(&env) {
        panic!("the envelope must validate: {error}\n{env:#}");
    }
    let markdown: Vec<char> = env["markdown"].as_str().unwrap().chars().collect();

    let runs = runs_of_paragraph(&env, 0);
    assert_eq!(runs.len(), 1, "{env:#}");
    let run = &runs[0];
    let controls = run["controls"]
        .as_array()
        .unwrap_or_else(|| panic!("the run carries controls (#543): {run:#}"));
    let got: Vec<(String, char, u64)> = controls
        .iter()
        .map(|c| {
            let at = c["at"].as_u64().unwrap() as usize;
            (
                c["kind"].as_str().unwrap().to_owned(),
                markdown[at],
                c["source"].as_u64().unwrap(),
            )
        })
        .collect();
    // Source offsets in UTF-16 units: 앞=0, TAB=1..9, 가=9, HYPHEN=10, 나=11, NB=12, 다=13, FW=14.
    assert_eq!(
        got,
        vec![
            ("tab".to_owned(), ' ', 1),
            ("hyphen".to_owned(), '-', 10),
            ("nb_space".to_owned(), ' ', 12),
            ("fw_space".to_owned(), ' ', 14),
        ],
        "{run:#}"
    );
    let (start, end) = (
        run["char_range"]["start"].as_u64().unwrap(),
        run["char_range"]["end"].as_u64().unwrap(),
    );
    let (src_start, src_end) = (
        run["source_chars"]["start"].as_u64().unwrap(),
        run["source_chars"]["end"].as_u64().unwrap(),
    );
    for c in controls {
        let at = c["at"].as_u64().unwrap();
        let source = c["source"].as_u64().unwrap();
        assert!(start <= at && at < end, "at inside char_range: {c}");
        assert!(
            src_start <= source && source < src_end,
            "source inside source_chars: {c}"
        );
    }

    // A run with none of the four controls carries no key at all - a typed `-` and a typed
    // space are not controls.
    let plain = runs_of_paragraph(&env, 1);
    assert!(!plain.is_empty(), "{env:#}");
    for run in plain {
        assert!(run.get("controls").is_none(), "{run:#}");
    }
}

/// `controls` is a `run`-only field, enforced by `dependentSchemas`.
#[test]
fn the_schema_rejects_controls_on_another_kind() {
    let level = serde_json::json!({
        "char_shape_id": null, "para_shape_id": null, "char": null, "para": null
    });
    let env = serde_json::json!({
        "contract": "hwp-segment-envelope-v2",
        "schema_version": "1.2",
        "markdown": "a b",
        "segments": [{
            "id": "x.0.0",
            "kind": "para",
            "path": {"section": 0, "indices": [0]},
            "char_range": {"start": 0, "end": 3},
            "style": level,
            "direct": level,
            "controls": [{"kind": "nb_space", "at": 1, "source": 1}]
        }]
    });
    assert!(
        !validator().is_valid(&env),
        "a para must not carry controls"
    );
}
