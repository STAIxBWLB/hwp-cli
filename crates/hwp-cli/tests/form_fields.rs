//! `hwp slots --forms` and `hwp fill --forms`: Korean form fields ported from kordoc (#364).
//!
//! CI-safe: markdown source and temp dirs only, no fixtures, no fonts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn run(command: &mut Command) -> Output {
    command.output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// A fresh test dir with `form.hwpx` built from `markdown`.
fn template(name: &str, markdown: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("hwp-form-fields-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("form.md");
    std::fs::write(&source, markdown).unwrap();
    let created = dir.join("form.hwpx");
    let output = run(hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&created));
    assert!(output.status.success(), "hwp new: {}", stderr(&output));
    (dir, created)
}

fn text(path: &Path) -> String {
    let output = run(hwp().arg("cat").arg(path));
    assert!(output.status.success(), "hwp cat: {}", stderr(&output));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut entry = archive.by_index(i).unwrap();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            (entry.name().to_string(), bytes)
        })
        .collect()
}

const FORM: &str = "# {{ 제목 }}\n\n담당자: \n\n\
                    | 성명 | | 생년월일 | |\n|---|---|---|---|\n| 주소 | | 연락처 | |\n";

#[test]
fn slots_forms_lists_slots_labels_and_inline_labels() {
    let (dir, form) = template("scan", FORM);
    let output = run(hwp().args(["slots", "--json", "--forms"]).arg(&form));
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["placeholders"][0]["name"], "제목", "{report}");
    let fields = report["fields"].as_array().unwrap();
    let keys: Vec<&str> = fields.iter().map(|f| f["key"].as_str().unwrap()).collect();
    assert_eq!(
        keys,
        ["담당자", "생년월일", "성명", "연락처", "제목", "주소"],
        "sorted by key"
    );
    let get = |key: &str| fields.iter().find(|f| f["key"] == key).unwrap();
    assert_eq!(
        get("성명"),
        &serde_json::json!({"key": "성명", "label": "성명", "source": "formLabel",
                            "confidence": 0.72, "occurrences": 1, "required": false})
    );
    assert_eq!(get("담당자")["source"], "inlineLabel");
    assert_eq!(get("제목")["required"], true);

    // Without --forms the JSON is unchanged.
    let output = run(hwp().args(["slots", "--json"]).arg(&form));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report.get("fields").is_none(), "{report}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_forms_fills_slots_and_form_fields_in_one_pass() {
    let (dir, form) = template("fill", FORM);
    let data = dir.join("data.json");
    std::fs::write(
        &data,
        r#"{"제목": "신청서", "담당자": "이영준", "성명": "홍길동", "연 락 처": "010", "없는키": "x"}"#,
    )
    .unwrap();

    let strict = dir.join("strict.hwpx");
    let output = run(hwp()
        .arg("fill")
        .arg(&form)
        .args(["--forms", "--data"])
        .arg(&data)
        .arg("-o")
        .arg(&strict));
    assert!(!output.status.success(), "an unmatched key fails closed");
    assert!(stderr(&output).contains("없는키"), "{}", stderr(&output));
    assert!(!strict.exists());

    let filled = dir.join("filled.hwpx");
    let output = run(hwp()
        .arg("fill")
        .arg(&form)
        .args(["--forms", "--json", "--allow-partial", "--data"])
        .arg(&data)
        .arg("-o")
        .arg(&filled));
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["mode"], "forms");
    assert_eq!(
        report["counts"],
        serde_json::json!({"제목": 1, "담당자": 1, "성명": 1, "연 락 처": 1, "없는키": 0}),
        "{report}"
    );
    assert_eq!(report["replaced"], 4);
    assert_eq!(report["unmatched"], serde_json::json!(["없는키"]));

    let filled_text = text(&filled);
    assert!(filled_text.contains("신청서"), "{filled_text}");
    assert!(filled_text.contains("담당자: 이영준"), "{filled_text}");
    assert!(
        filled_text.contains("성명\t홍길동\t생년월일"),
        "{filled_text}"
    );
    assert!(filled_text.contains("연락처\t010"), "{filled_text}");

    let output = run(hwp().arg("validate").arg(&filled));
    assert!(output.status.success(), "{}", stderr(&output));
    // Only the section XML is rewritten; every other entry is copied byte for byte.
    let (before, after) = (entries(&form), entries(&filled));
    assert_eq!(
        before.iter().map(|(n, _)| n).collect::<Vec<_>>(),
        after.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        if !name.starts_with("Contents/section") {
            assert_eq!(a, b, "{name} changed");
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_forms_with_no_match_publishes_the_input_under_allow_partial() {
    let (dir, form) = template("zero", FORM);
    let out = dir.join("out.hwpx");
    let output = run(hwp()
        .arg("fill")
        .arg(&form)
        .args([
            "--forms",
            "--allow-partial",
            "--json",
            "--set",
            "없는키=x",
            "-o",
        ])
        .arg(&out));
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["replaced"], 0);
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&form).unwrap());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_forms_refuses_tables_parts_and_hwp_input() {
    let (dir, form) = template("refuse", FORM);
    let data = dir.join("tables.json");
    std::fs::write(&data, r#"{"tables": [{"table": 0, "rows": [["a"]]}]}"#).unwrap();
    let output = run(hwp()
        .arg("fill")
        .arg(&form)
        .args(["--forms", "--data"])
        .arg(&data)
        .arg("-o")
        .arg(dir.join("t.hwpx")));
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--forms"), "{}", stderr(&output));

    let hwp5 = dir.join("form.hwp");
    let output = run(hwp()
        .arg("convert")
        .arg(&form)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5));
    assert!(output.status.success(), "{}", stderr(&output));
    let output = run(hwp()
        .arg("fill")
        .arg(&hwp5)
        .args(["--forms", "--set", "성명=x", "-o"])
        .arg(dir.join("out.hwp")));
    assert!(!output.status.success());
    assert!(stderr(&output).contains("HWPX"), "{}", stderr(&output));

    let _ = std::fs::remove_dir_all(&dir);
}
