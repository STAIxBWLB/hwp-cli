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
    assert!(
        stderr(&output).contains("양식 채우기(--forms)는 HWPX 입력 전용"),
        "{}",
        stderr(&output)
    );

    // `--forms` writes hwpx only.
    let output = run(hwp()
        .arg("fill")
        .arg(&form)
        .args(["--forms", "--set", "성명=x", "-o"])
        .arg(dir.join("out.hwp")));
    assert!(!output.status.success());
    assert!(stderr(&output).contains(".hwpx"), "{}", stderr(&output));

    let _ = std::fs::remove_dir_all(&dir);
}

/// 32x24 IHDR-only PNG, the shape the insert-image tests use.
fn tiny_png(path: &Path) {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(32u32.to_be_bytes());
    png.extend(24u32.to_be_bytes());
    png.extend([0u8; 8]);
    std::fs::write(path, &png).unwrap();
}

/// A value cell holding a 누름틀 or a picture is never written; the fill says so.
#[test]
fn fill_forms_leaves_cells_with_fields_and_pictures_alone() {
    let (dir, form) = template("controls", "| 성명 | 누름 |\n|---|---|\n| 주소 | 그림 |\n");
    let png = dir.join("tiny.png");
    tiny_png(&png);
    let edited = dir.join("edited.hwpx");
    let output = run(hwp()
        .arg("edit")
        .arg(&form)
        .args(["--create-field", "누름=>이름란", "--insert-image"])
        .arg(format!("그림=>{}", png.display()))
        .arg("-o")
        .arg(&edited));
    assert!(output.status.success(), "{}", stderr(&output));

    let filled = dir.join("filled.hwpx");
    let output = run(hwp()
        .arg("fill")
        .arg(&edited)
        .args([
            "--forms",
            "--json",
            "--allow-partial",
            "--set",
            "성명=홍길동",
            "--set",
            "주소=제주",
            "-o",
        ])
        .arg(&filled));
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["unmatched"],
        serde_json::json!(["성명", "주소"]),
        "{report}"
    );
    let warnings = report["warnings"].to_string();
    assert!(
        warnings.contains("표0 (0,1)") && warnings.contains("표0 (1,1)"),
        "{report}"
    );
    assert_eq!(
        std::fs::read(&filled).unwrap(),
        std::fs::read(&edited).unwrap(),
        "nothing was written, so the input is published unchanged"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Copy an hwpx package, putting an XML comment into section 1. The writer never emits one,
/// so the comment survives only if that section is copied instead of re-serialized.
fn mark_second_section(from: &Path, to: &Path) {
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;
    let mut source = zip::ZipArchive::new(std::fs::File::open(from).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(std::fs::File::create(to).unwrap());
    for i in 0..source.len() {
        let mut entry = source.by_index(i).unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if entry.name() == "Contents/section1.xml" {
            let xml = String::from_utf8(data).unwrap();
            let at = xml.find("?>").unwrap() + 2;
            data = format!("{}<!-- untouched -->{}", &xml[..at], &xml[at..]).into_bytes();
        }
        let method = if entry.name() == "mimetype" {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        out.start_file(
            entry.name(),
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        out.write_all(&data).unwrap();
    }
    out.finish().unwrap();
}

/// Only the sections the fill changed are re-serialized; the others are copied byte for byte.
#[test]
fn fill_forms_rewrites_only_the_changed_sections() {
    let (dir, first) = template("sections", "| 성명 | |\n|---|---|\n");
    let source = dir.join("second.md");
    std::fs::write(&source, "작성자: 미정\n").unwrap();
    let second = dir.join("second.hwpx");
    let output = run(hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&second));
    assert!(output.status.success(), "{}", stderr(&output));
    let merged = dir.join("merged.hwpx");
    let output = run(hwp()
        .arg("merge")
        .arg(&first)
        .arg(&second)
        .arg("-o")
        .arg(&merged));
    assert!(output.status.success(), "{}", stderr(&output));
    let marked = dir.join("marked.hwpx");
    mark_second_section(&merged, &marked);

    let filled = dir.join("filled.hwpx");
    let output = run(hwp()
        .arg("fill")
        .arg(&marked)
        .args(["--forms", "--set", "성명=홍길동", "-o"])
        .arg(&filled));
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(text(&filled).contains("성명\t홍길동"));
    let (before, after) = (entries(&marked), entries(&filled));
    for ((name, a), (_, b)) in before.iter().zip(&after) {
        if name == "Contents/section0.xml" {
            assert_ne!(a, b, "the filled section is rewritten");
        } else {
            assert_eq!(a, b, "{name} changed");
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
