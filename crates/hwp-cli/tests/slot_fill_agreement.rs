//! `hwp slots` and `hwp fill` must agree about which `{{slot}}` a document has.
//!
//! They read the document two different ways: `slots` walks the IR, where a paragraph's
//! characters are already joined, while `fill` rewrites the raw section XML. Inline formatting
//! inside a slot name splits it across text runs, and before #145 that split was invisible to
//! `slots` and fatal to `fill` — the name was listed and then refused.
//!
//! CI-safe: markdown source and temp dirs only, no fixtures, no fonts.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-slot-fill-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Slot names `hwp slots` reports, one `name<TAB>count` line each.
fn reported_slots(path: &Path) -> BTreeSet<String> {
    let output = hwp().arg("slots").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "hwp slots failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').next().unwrap_or(line).to_string())
        .collect()
}

fn document_text(path: &Path) -> String {
    let output = hwp().arg("cat").arg(path).output().unwrap();
    assert!(
        output.status.success(),
        "hwp cat failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Every slot `hwp slots` lists must be fillable, including the ones inline formatting split
/// across runs. `hwp fill` is fail-closed, so a name it cannot find aborts the whole command —
/// which makes a single fill of all reported names the sharpest form of this assertion.
#[test]
fn every_reported_slot_is_fillable() {
    let dir = test_dir("agreement");
    let source = dir.join("source.md");
    std::fs::write(
        &source,
        // 제목: whole in one run. 이름/기관명: split by emphasis and by bold.
        // 연락처: split twice over. 비고: formatting around, not inside, the name.
        "제목  {{제목}}\n\n\
         이름: {{이*름*}}\n\n\
         기관: {{기**관**명}}\n\n\
         연락: {{연*락*처}}\n\n\
         비고: *{{비고}}*\n",
    )
    .unwrap();

    let created = dir.join("doc.hwpx");
    let run = hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&created)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "hwp new failed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let reported = reported_slots(&created);
    let expected: BTreeSet<String> = ["제목", "이름", "기관명", "연락처", "비고"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "hwp slots must report every slot, split across runs or not"
    );

    // One fill of all of them. Without --allow-partial this exits non-zero if any name is
    // unfillable, so success is the agreement assertion.
    let filled = dir.join("filled.hwpx");
    let mut fill = hwp();
    fill.arg("fill").arg(&created).arg("-o").arg(&filled);
    for name in &reported {
        fill.arg("--set").arg(format!("{name}=값-{name}"));
    }
    let run = fill.output().unwrap();
    assert!(
        run.status.success(),
        "every reported slot must be fillable, but fill refused\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    // Nothing left behind, and the values actually landed.
    assert!(
        reported_slots(&filled).is_empty(),
        "no slot may survive a fill of every reported name"
    );
    let text = document_text(&filled);
    for name in &reported {
        assert!(
            text.contains(&format!("값-{name}")),
            "value for {name} missing from the filled document:\n{text}"
        );
    }

    let run = hwp().arg("validate").arg(&filled).output().unwrap();
    assert!(
        run.status.success(),
        "the filled document must validate\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Text around a split placeholder must survive the coalescing untouched — the failure mode of
/// a run-range rewrite is eating a neighbouring character, which no slot-name check would catch.
#[test]
fn coalescing_a_split_slot_preserves_its_neighbouring_text() {
    let dir = test_dir("neighbours");
    let source = dir.join("source.md");
    std::fs::write(&source, "앞말 {{이*름*}} 뒷말\n").unwrap();

    let created = dir.join("doc.hwpx");
    hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&created)
        .output()
        .unwrap();

    let filled = dir.join("filled.hwpx");
    let run = hwp()
        .arg("fill")
        .arg(&created)
        .arg("--set")
        .arg("이름=홍길동")
        .arg("-o")
        .arg(&filled)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "fill failed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(document_text(&filled).trim(), "앞말 홍길동 뒷말");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Build `doc.hwpx` from markdown in a fresh test dir.
fn template(name: &str, markdown: &str) -> (PathBuf, PathBuf) {
    let dir = test_dir(name);
    let source = dir.join("source.md");
    std::fs::write(&source, markdown).unwrap();
    let created = dir.join("doc.hwpx");
    let run = hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&created)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "hwp new failed\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    (dir, created)
}

/// A padded `{{ name }}` is one slot: `slots` lists it trimmed and `fill` fills it, padded-only,
/// mixed with the unpadded spelling, split across runs, and inside a table cell (#362).
#[test]
fn padded_slots_are_listed_and_filled() {
    let (dir, created) = template(
        "padded",
        "제목  {{ 제목 }}\n\n\
         이름: {{ 이*름* }}\n\n\
         기관: {{기관}} 그리고 {{ 기관 }}\n\n\
         | 부서 | {{ 부서 }} |\n|---|---|\n| 가 | 나 |\n",
    );
    let reported = reported_slots(&created);
    let expected: BTreeSet<String> = ["제목", "이름", "기관", "부서"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(reported, expected);

    let filled = dir.join("filled.hwpx");
    let mut fill = hwp();
    fill.arg("fill")
        .arg(&created)
        .arg("-o")
        .arg(&filled)
        .arg("--json");
    for name in &reported {
        fill.arg("--set").arg(format!("{name}=값-{name}"));
    }
    let run = fill.output().unwrap();
    assert!(
        run.status.success(),
        "fill refused a padded slot\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["counts"]["기관"], 2, "both spellings: {report}");
    assert_eq!(report["replaced"], 5, "{report}");

    assert!(reported_slots(&filled).is_empty());
    let text = document_text(&filled);
    assert!(text.contains("기관: 값-기관 그리고 값-기관"), "{text}");
    assert!(text.contains("이름: 값-이름"), "{text}");
    assert!(!text.contains("{{"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The IR fill paths (`--data` with `tables`) use the same grammar.
#[test]
fn table_fill_fills_a_padded_slot() {
    let (dir, created) = template(
        "padded-table",
        "부서: {{ 부서 }}\n\n| 품목 | 수량 |\n|---|---|\n| | |\n",
    );
    let data = dir.join("data.json");
    std::fs::write(
        &data,
        r#"{"fields": {"부서": "기획팀"}, "tables": [{"table": 0, "rows": [["노트북", "5"]]}]}"#,
    )
    .unwrap();
    let filled = dir.join("filled.hwpx");
    let run = hwp()
        .arg("fill")
        .arg(&created)
        .arg("--data")
        .arg(&data)
        .arg("-o")
        .arg(&filled)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "table fill refused a padded slot\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(document_text(&filled).contains("부서: 기획팀"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// With `--allow-partial`, a request where no name is a slot publishes the input unchanged and
/// reports zero counts; without it the fill still fails closed and publishes nothing (#362).
#[test]
fn allow_partial_publishes_a_zero_match_fill() {
    let (dir, created) = template("zero", "제목  {{제목}}\n");

    let partial = dir.join("partial.hwpx");
    let run = hwp()
        .arg("fill")
        .arg(&created)
        .args(["--set", "성명=홍길동", "--set", "소속=기획팀"])
        .arg("-o")
        .arg(&partial)
        .args(["--json", "--allow-partial"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "--allow-partial must publish a zero-match fill\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["replaced"], 0, "{report}");
    assert_eq!(
        report["counts"],
        serde_json::json!({"성명": 0, "소속": 0}),
        "{report}"
    );
    assert_eq!(
        std::fs::read(&partial).unwrap(),
        std::fs::read(&created).unwrap(),
        "a zero-match fill publishes the input unchanged"
    );

    let strict = dir.join("strict.hwpx");
    let run = hwp()
        .arg("fill")
        .arg(&created)
        .args(["--set", "성명=홍길동"])
        .arg("-o")
        .arg(&strict)
        .output()
        .unwrap();
    assert!(!run.status.success(), "without --allow-partial it fails");
    assert!(!strict.exists(), "a failed fill publishes nothing");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Run `hwp fill <template> <args> -o <out>` and return the output.
fn fill(template: &Path, out: &Path, args: &[&str]) -> std::process::Output {
    hwp()
        .arg("fill")
        .arg(template)
        .args(args)
        .arg("-o")
        .arg(out)
        .output()
        .unwrap()
}

/// A name is any text without braces or control characters, the rule TemplateSpec bindings
/// accept, and requested keys are trimmed like names (#362).
#[test]
fn names_with_spaces_and_punctuation_are_listed_and_filled() {
    let (dir, created) = template(
        "wide-names",
        "{{성 명}} / {{사업명(국문)}} / {{가·나}} / {{기간: 시작}} / {{a/b}} / {{ 제목 }}\n",
    );
    let reported = reported_slots(&created);
    let expected: BTreeSet<String> = [
        "성 명",
        "사업명(국문)",
        "가·나",
        "기간: 시작",
        "a/b",
        "제목",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    assert_eq!(reported, expected);

    let data = dir.join("data.json");
    std::fs::write(
        &data,
        r#"{"성 명": "1", "사업명(국문)": "2", "가·나": "3", "기간: 시작": "4", "a/b": "5", " 제목 ": "6"}"#,
    )
    .unwrap();
    let filled = dir.join("filled.hwpx");
    let data_arg = data.display().to_string();
    let run = fill(&created, &filled, &["--data", &data_arg, "--json"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(
        report["counts"][" 제목 "], 1,
        "counted under the caller's key: {report}"
    );
    assert_eq!(document_text(&filled).trim(), "1 / 2 / 3 / 4 / 5 / 6");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Values are literal on every path: a value that spells a slot is neither filled again nor
/// reported as a slot left behind, and a field value spelling a part anchor is not one (#362).
#[test]
fn values_are_literal_on_every_path() {
    // Default (raw XML) path.
    let (dir, created) = template("literal", "{{a}} {{b}}\n\n| 품목 |\n|---|\n| |\n");
    let out = dir.join("default.hwpx");
    let run = fill(&created, &out, &["--set", "a={{b}}", "--set", "b=B"]);
    assert!(
        run.status.success(),
        "default path refused a literal value\nstderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(document_text(&out).contains("{{b}} B"));

    // Tables (IR) path.
    let data = dir.join("tables.json");
    std::fs::write(
        &data,
        r#"{"fields": {"a": "{{b}}", "b": "B"}, "tables": [{"table": 0, "rows": [["노트북"]]}]}"#,
    )
    .unwrap();
    let out = dir.join("tables.hwpx");
    let data_arg = data.display().to_string();
    let run = fill(&created, &out, &["--data", &data_arg]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(document_text(&out).contains("{{b}} B"));

    // Parts (IR) path.
    let (parts_dir, parts_template) =
        template("literal-parts", "{{a}} {{b}}\n\n{{c}}\n\n{{본문}}\n");
    let part = parts_dir.join("part.md");
    std::fs::write(&part, "부분 본문\n").unwrap();
    let out = parts_dir.join("parts.hwpx");
    let part_arg = format!("본문=@{}", part.display());
    let run = fill(
        &parts_template,
        &out,
        &[
            "--set",
            "a={{b}}",
            "--set",
            "b=B",
            "--set",
            "c={{본문}}",
            "--set",
            &part_arg,
        ],
    );
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = document_text(&out);
    assert!(text.contains("{{b}} B"), "{text}");
    assert!(
        text.contains("{{본문}}") && text.matches("부분 본문").count() == 1,
        "a value spelling the anchor stays literal: {text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&parts_dir);
}

/// `--allow-partial` publishes the input unchanged on a zero total on the IR paths too (#362).
#[test]
fn allow_partial_publishes_a_zero_match_ir_fill() {
    let (dir, created) = template("zero-ir", "제목\n\n| 품목 |\n|---|\n| |\n");

    let data = dir.join("tables.json");
    std::fs::write(
        &data,
        r#"{"없음": "x", "tables": [{"table": 0, "rows": []}]}"#,
    )
    .unwrap();
    let data_arg = data.display().to_string();
    let out = dir.join("tables.hwpx");
    let run = fill(&created, &out, &["--data", &data_arg]);
    assert!(!run.status.success(), "without --allow-partial it fails");
    assert!(!out.exists());
    let run = fill(&created, &out, &["--data", &data_arg, "--allow-partial"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(&created).unwrap()
    );

    let part = dir.join("part.md");
    std::fs::write(&part, "부분\n").unwrap();
    let part_arg = format!("본문=@{}", part.display());
    let out = dir.join("parts.hwpx");
    let run = fill(&created, &out, &["--set", &part_arg]);
    assert!(!run.status.success(), "without --allow-partial it fails");
    let run = fill(&created, &out, &["--set", &part_arg, "--allow-partial"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(&created).unwrap()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Copy an hwpx package with `find` replaced by `replace` in `Contents/section0.xml`.
fn rewrite_section0(from: &Path, to: &Path, find: &str, replace: &str) {
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;
    let mut source = zip::ZipArchive::new(std::fs::File::open(from).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(std::fs::File::create(to).unwrap());
    for i in 0..source.len() {
        let mut entry = source.by_index(i).unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if entry.name() == "Contents/section0.xml" {
            let xml = String::from_utf8(data).unwrap();
            assert!(xml.contains(find), "{find} not in the section");
            data = xml.replace(find, replace).into_bytes();
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

/// A name `hwp slots` lists is filled although the XML escapes it: `&`, `<`, `>`, `"` and a
/// numeric character reference (#363 review).
#[test]
fn escaped_names_are_listed_and_filled() {
    let (dir, created) = template("escaped", "{{R&D 과제명}} / {{a<1> \"b\"}} / {{가나}}\n");
    let marked = dir.join("marked.hwpx");
    rewrite_section0(&created, &marked, "{{가나}}", "{{&#44032;나}}");
    let reported = reported_slots(&marked);
    let expected: BTreeSet<String> = ["R&D 과제명", "a<1> \"b\"", "가나"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(reported, expected);

    let data = dir.join("data.json");
    std::fs::write(
        &data,
        serde_json::json!({"R&D 과제명": "1", "a<1> \"b\"": "2", "가나": "3"}).to_string(),
    )
    .unwrap();
    let filled = dir.join("filled.hwpx");
    let data_arg = data.display().to_string();
    let run = fill(&marked, &filled, &["--data", &data_arg]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(document_text(&filled).trim(), "1 / 2 / 3");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two keys that trim to one slot are accepted with equal values, each counted once for the
/// token, and refused with different values.
#[test]
fn keys_naming_one_slot_need_equal_values() {
    let (dir, created) = template("same-slot", "{{제목}}\n");
    let data = dir.join("same.json");
    std::fs::write(&data, r#"{" 제목": "A", "제목": "A"}"#).unwrap();
    let data_arg = data.display().to_string();
    let out = dir.join("same.hwpx");
    let run = fill(&created, &out, &["--data", &data_arg, "--json"]);
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["counts"], serde_json::json!({" 제목": 1, "제목": 1}));
    assert_eq!(report["replaced"], 1, "one token: {report}");

    std::fs::write(&data, r#"{" 제목": "A", "제목": "B"}"#).unwrap();
    let run = fill(&created, &dir.join("differ.hwpx"), &["--data", &data_arg]);
    assert!(!run.status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Every numbering and bullet id a paragraph shape references has a record in the header.
fn assert_list_definitions_resolve(header: &hwp_model::DocHeader) {
    for shape in &header.para_shapes {
        let (kind, records) = match shape.head_type() {
            2 => ("NUMBERING", header.numberings.len()),
            3 => ("BULLET", header.bullets.len()),
            _ => continue,
        };
        assert!(
            usize::from(shape.numbering_id) < records,
            "{kind} id {} has no record ({records} written)",
            shape.numbering_id
        );
    }
}

/// A part fills into a `.hwp` template, lists included (#377). The part's numbering and bullet
/// definitions sit past the template's own records, and the HWP5 writer used to drop them, so
/// every part fill into `.hwp` failed its output verification.
#[test]
fn a_part_with_lists_fills_into_hwp() {
    // The template's own bullet list gives it raw BULLET records, so the part's bullet is
    // grafted past them.
    let (dir, created) = template("part-hwp", "# 제목\n\n- 기존\n\n{{본문}}\n\n메일: {{x}}\n");
    let hwp5 = dir.join("template.hwp");
    let run = hwp()
        .arg("convert")
        .arg(&created)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let part = dir.join("part.md");
    std::fs::write(&part, "부분\n\n- 가\n- 나\n\n1. 하나\n2. 둘\n").unwrap();
    let out = dir.join("out.hwp");
    let set_part = format!("본문=@{}", part.display());
    let run = fill(&hwp5, &out, &["--set", &set_part, "--set", "x=v"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let validate = hwp().arg("validate").arg(&out).output().unwrap();
    assert!(
        validate.status.success(),
        "{}",
        String::from_utf8_lossy(&validate.stdout)
    );
    let text = document_text(&out);
    for expected in ["부분", "가", "하나", "메일: v"] {
        assert!(
            text.contains(expected),
            "{expected:?} missing from {text:?}"
        );
    }
    let header = hwp5::read_document(&out).unwrap().document.header;
    assert_list_definitions_resolve(&header);
    assert!(
        header.bullets.len() == 2 && header.numberings.len() > 1,
        "the part's definitions follow the template's own"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Parts keep their controls through same-format fills and strict template conversion (#391,
/// #392). The assertions inspect content and styling, not only command exit status.
#[test]
fn parts_keep_links_and_notes_in_every_native_format_pair() {
    let (dir, hwpx) = template("parts-format-matrix", "# 제목\n\n{{본문}}\n\n메일: {{x}}\n");
    let hwp5 = dir.join("template.hwp");
    let conversion = hwp()
        .arg("convert")
        .arg(&hwpx)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        conversion.status.success(),
        "{}",
        String::from_utf8_lossy(&conversion.stderr)
    );

    for (name, markdown) in [
        ("plain", "부분\n"),
        ("link", "[링크](https://example.com) 부분\n"),
        (
            "notes",
            "부분[^1] 미주[^e1]\n\n[^1]: **각주 내용**\n\n[^e1]: 미주 내용\n",
        ),
    ] {
        let part = dir.join(format!("{name}.md"));
        std::fs::write(&part, markdown).unwrap();
        let set_part = format!("본문=@{}", part.display());
        for (source, input) in [("hwp", &hwp5), ("hwpx", &hwpx)] {
            for target in ["hwp", "hwpx"] {
                // The requested filename may equal the private converted template's filename.
                let output_dir = dir.join(format!("{name}-{source}-{target}"));
                std::fs::create_dir(&output_dir).unwrap();
                let output = output_dir.join(format!("template.{target}"));
                let run = fill(
                    input,
                    &output,
                    &["--set", &set_part, "--set", "x=v", "--json"],
                );
                assert!(
                    run.status.success(),
                    "{name} {source} -> {target}: {}",
                    String::from_utf8_lossy(&run.stderr)
                );
                let report: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
                assert_eq!(report["output"], output.display().to_string());
                assert_eq!(report["replaced"], 2);
                let validation = hwp().arg("validate").arg(&output).output().unwrap();
                assert!(
                    validation.status.success(),
                    "{}",
                    String::from_utf8_lossy(&validation.stdout)
                );
                let doc = if target == "hwp" {
                    hwp5::read_document(&output).unwrap().document
                } else {
                    hwpx::read_document(&output).unwrap().document
                };
                assert!(doc.plain_text().contains("부분"));
                assert!(doc.plain_text().contains("메일: v"));
                let controls: Vec<_> = doc
                    .sections
                    .iter()
                    .flat_map(|section| &section.paragraphs)
                    .flat_map(|paragraph| &paragraph.controls)
                    .collect();
                if name == "link" {
                    let urls: Vec<_> = controls
                        .iter()
                        .filter_map(|control| hwp_convert::hyperlink_url(control))
                        .collect();
                    assert_eq!(urls, ["https://example.com"]);
                    assert!(doc.header.char_shapes.iter().any(|shape| shape.text_color
                        == 0x00ff_0000
                        && shape.underline_kind() == 1));
                }
                if name == "notes" {
                    for (id, text) in [(*b"fn  ", "각주 내용"), (*b"en  ", "미주 내용")] {
                        let note = controls
                            .iter()
                            .find_map(|control| match control {
                                hwp_model::Control::Generic(g) if g.ctrl_id == id => Some(g),
                                _ => None,
                            })
                            .expect("note control survives");
                        let list = &note.paragraph_lists[0];
                        if target == "hwp" {
                            assert_eq!(
                                list.header_data,
                                hwp5::write::synthesized_note_list_header(list.paragraphs.len())
                            );
                        }
                        let paragraph = &list.paragraphs[0];
                        let actual: String = paragraph
                            .chars
                            .iter()
                            .filter_map(|ch| match ch {
                                hwp_model::HwpChar::Text(ch) => Some(*ch),
                                _ => None,
                            })
                            .collect();
                        assert_eq!(actual, text);
                        if id == *b"fn  " {
                            let shape = paragraph.char_shape_runs[0].1.0 as usize;
                            assert!(
                                doc.header.char_shapes[shape].is_bold(),
                                "note formatting survives"
                            );
                        }
                    }
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cross_format_part_failures_preserve_destination_and_clean_private_staging() {
    let (dir, hwpx) = template("parts-cross-failure", "# 제목\n\n{{본문}}\n");
    let hwp5 = dir.join("template.hwp");
    let conversion = hwp()
        .arg("convert")
        .arg(&hwpx)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        conversion.status.success(),
        "{}",
        String::from_utf8_lossy(&conversion.stderr)
    );
    let part = dir.join("part.md");
    std::fs::write(&part, "부분\n").unwrap();
    let missing_anchor = format!("없는앵커=@{}", part.display());
    let output_hwp = dir.join("out.hwp");
    let output_hwpx = dir.join("out.hwpx");
    for output in [&output_hwp, &output_hwpx] {
        std::fs::write(output, b"ORIGINAL").unwrap();
    }
    let entries = || -> BTreeSet<_> {
        std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    };
    let before = entries();
    for (input, output) in [(&hwpx, &output_hwp), (&hwp5, &output_hwpx)] {
        let run = fill(input, output, &["--set", &missing_anchor]);
        assert!(
            !run.status.success(),
            "unmatched anchor must fail after conversion"
        );
        assert!(String::from_utf8_lossy(&run.stderr).contains("부분 앵커를 찾지 못했습니다"));
        assert_eq!(std::fs::read(output).unwrap(), b"ORIGINAL");
        assert_eq!(entries(), before, "no public intermediate output remains");
    }

    let mut opaque = hwpx::read_document(&hwpx).unwrap().document;
    opaque
        .hwpx_extra_entries
        .push(("SyntheticOpaque/entry.bin".into(), b"opaque".to_vec()));
    let source = dir.join("opaque.hwpx");
    hwpx::write_document(&opaque, &source).unwrap();
    let set_part = format!("본문=@{}", part.display());
    let before = entries();
    let run = fill(&source, &output_hwp, &["--set", &set_part]);
    assert!(
        !run.status.success(),
        "strict template conversion must reject package loss"
    );
    assert!(String::from_utf8_lossy(&run.stderr).contains("hwpx_package_entry_removed"));
    assert_eq!(std::fs::read(&output_hwp).unwrap(), b"ORIGINAL");
    assert_eq!(entries(), before);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A part fills into an official-profile `.hwp` template that has its own numbered list (#379
/// review). The template's eight official NUMBERING records are followed by the part's own, and
/// the re-read must still restore the official list levels, or the verification refuses.
#[test]
fn a_part_fills_into_an_official_hwp_template_with_a_numbered_list() {
    let dir = test_dir("part-official");
    let source = dir.join("source.md");
    std::fs::write(&source, "# 제목\n\n1. 하나\n    1. 가\n\n{{본문}}\n").unwrap();
    let hwp5 = dir.join("template.hwp");
    let run = hwp()
        .args(["new", "--preset", "official", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let part = dir.join("part.md");
    std::fs::write(&part, "부분\n\n1. 셋\n").unwrap();
    let out = dir.join("out.hwp");
    let set_part = format!("본문=@{}", part.display());
    let run = fill(&hwp5, &out, &["--set", &set_part]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let levels = |path: &Path| -> Vec<Option<u8>> {
        let header = hwp5::read_document(path).unwrap().document.header;
        header
            .para_shapes
            .iter()
            .filter(|shape| shape.head_type() == 2 && shape.numbering_id < 8)
            .map(|shape| shape.list_level)
            .collect()
    };
    let template_levels = levels(&hwp5);
    assert!(template_levels.iter().any(Option::is_some));
    assert_eq!(
        levels(&out),
        template_levels,
        "official list levels survive"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A numbered list that starts at a number other than 1 cannot be written to `.hwp` yet: the
/// synthesized NUMBERING record counts from 1, so the output verification refuses the fill
/// rather than renumber the list (#377 review). `.hwpx` keeps the start number.
#[test]
fn a_part_list_starting_past_one_is_refused_for_hwp() {
    let (dir, created) = template("part-start", "# 제목\n\n{{본문}}\n");
    let hwp5 = dir.join("template.hwp");
    let run = hwp()
        .arg("convert")
        .arg(&created)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let part = dir.join("part.md");
    std::fs::write(&part, "3. 셋째\n4. 넷째\n").unwrap();
    let set_part = format!("본문=@{}", part.display());

    let out = dir.join("out.hwp");
    let run = fill(&hwp5, &out, &["--set", &set_part]);
    assert!(!run.status.success(), "a renumbered list must not publish");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("의미 불변식"),
        "refused by the output verification: {stderr}"
    );
    assert!(!out.exists());

    let out = dir.join("out.hwpx");
    let run = fill(&created, &out, &["--set", &set_part]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first paragraph's section-scoped controls: the section and column definitions and the
/// page number position, in order.
fn section_controls(path: &Path) -> Vec<hwp_model::Control> {
    let doc = match path.extension().and_then(|extension| extension.to_str()) {
        Some("hwp") => hwp5::read_document(path).unwrap().document,
        _ => hwpx::read_document(path).unwrap().document,
    };
    doc.sections[0].paragraphs[0]
        .controls
        .iter()
        .filter(|control| match control {
            hwp_model::Control::SectionDef(_) => true,
            hwp_model::Control::Generic(g) => matches!(&g.ctrl_id, b"cold" | b"pgnp"),
            _ => false,
        })
        .cloned()
        .collect()
}

/// A part anchor in the document's first paragraph fills (#376). That paragraph holds the
/// section definition; the fill carries it onto the part's first paragraph, so the output keeps
/// the template's page settings (a 30 mm top margin, not the writer's default A4) and its page
/// number.
#[test]
fn a_part_anchor_in_the_first_paragraph_keeps_the_section_definition() {
    let dir = test_dir("part-first");
    let source = dir.join("source.md");
    std::fs::write(&source, "{{본문}}\n\n메일: {{x}}\n").unwrap();
    let hwpx = dir.join("template.hwpx");
    let run = hwp()
        .args(["new", "--preset", "report", "--margin-top", "30", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&hwpx)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let hwp5 = dir.join("template.hwp");
    let run = hwp()
        .arg("convert")
        .arg(&hwpx)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let part = dir.join("part.md");
    std::fs::write(&part, "부분\n").unwrap();
    let set_part = format!("본문=@{}", part.display());

    for (template, out) in [(&hwpx, dir.join("out.hwpx")), (&hwp5, dir.join("out.hwp"))] {
        let run = fill(template, &out, &["--set", &set_part, "--set", "x=v"]);
        assert!(
            run.status.success(),
            "{}: {}",
            template.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        let validate = hwp().arg("validate").arg(&out).output().unwrap();
        assert!(
            validate.status.success(),
            "{}",
            String::from_utf8_lossy(&validate.stdout)
        );
        let text = document_text(&out);
        assert!(
            text.contains("부분") && text.contains("메일: v"),
            "{text:?}"
        );
        let expected = section_controls(template);
        assert_eq!(expected.len(), 3, "secd, cold and pgnp in the template");
        assert_eq!(section_controls(&out), expected, "{}", out.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--allow-partial` with nothing to change on the IR path: `.hwp` to `.hwp` publishes the input
/// byte for byte; a different output format still goes through the writer, which converts.
#[test]
fn zero_match_ir_fill_publishes_hwp_unchanged_or_converts() {
    let (dir, created) = template("zero-hwp", "제목\n\n| 품목 |\n|---|\n| |\n");
    let hwp5 = dir.join("template.hwp");
    let run = hwp()
        .arg("convert")
        .arg(&created)
        .args(["--to", "hwp", "-o"])
        .arg(&hwp5)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let data = dir.join("tables.json");
    std::fs::write(
        &data,
        r#"{"없음": "x", "tables": [{"table": 0, "rows": []}]}"#,
    )
    .unwrap();
    let data_arg = data.display().to_string();

    let same = dir.join("same.hwp");
    let run = fill(&hwp5, &same, &["--data", &data_arg, "--allow-partial"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(std::fs::read(&same).unwrap(), std::fs::read(&hwp5).unwrap());

    let converted = dir.join("converted.hwp");
    let run = fill(
        &created,
        &converted,
        &["--data", &data_arg, "--allow-partial"],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let info = hwp()
        .args(["info", "--json"])
        .arg(&converted)
        .output()
        .unwrap();
    let info: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(
        info["format"], "hwp5",
        "the writer converted the hwpx input"
    );
    assert_eq!(
        document_text(&converted).trim(),
        document_text(&created).trim()
    );

    // Like the writer path, a zero-match fill writes only .hwp or .hwpx.
    let markdown = dir.join("converted.md");
    let run = fill(
        &created,
        &markdown,
        &["--data", &data_arg, "--allow-partial"],
    );
    assert!(!run.status.success(), "a .md output must be refused");
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("fill 출력은 .hwp 또는 .hwpx만"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!markdown.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

/// `@@` in a `--set` value is a literal `@`, on the default path and beside a part.
#[test]
fn a_doubled_at_sign_is_a_literal_at_sign() {
    let (dir, created) = template("at-escape", "# 제목\n\n{{본문}}\n\n메일: {{x}}\n");

    let plain = dir.join("plain.hwpx");
    let run = fill(
        &created,
        &plain,
        &["--set", "본문=본문", "--set", "x=@@example"],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = document_text(&plain);
    assert!(text.contains("메일: @example"), "{text}");
    assert!(!text.contains("@@"), "{text}");

    let part = dir.join("part.md");
    std::fs::write(&part, "부분\n").unwrap();
    let part_arg = format!("본문=@{}", part.display());
    let with_part = dir.join("with-part.hwpx");
    let run = fill(
        &created,
        &with_part,
        &["--set", &part_arg, "--set", "x=@@example"],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = document_text(&with_part);
    assert!(text.contains("메일: @example"), "{text}");
    assert!(!text.contains("@@"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}
