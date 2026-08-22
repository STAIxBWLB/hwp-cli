//! End-to-end coverage for the official-document profile surface.
//!
//! These tests deliberately invoke the shipped `hwp` binary, then reread the native
//! output. They assert structural document properties only, never renderer output.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use hwp_model::{Control, Document, NumFmt, PageDef};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hwp-cli-official-presets-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn command_output(command: &mut Command, label: &str) -> Output {
    command
        .output()
        .unwrap_or_else(|error| panic!("{label}: {error}"))
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn reread(path: &Path) -> Document {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("hwp") => hwp5::read_document(path).unwrap().document,
        Some("hwpx") => hwpx::read_document(path).unwrap().document,
        other => panic!("unsupported test extension: {other:?}"),
    }
}

fn page_def(document: &Document) -> PageDef {
    document.sections[0].paragraphs[0]
        .controls
        .iter()
        .find_map(|control| match control {
            Control::SectionDef(section) => section.page,
            _ => None,
        })
        .expect("section must carry a page definition")
}

fn page_number(document: &Document) -> Option<&hwp_model::GenericControl> {
    document.sections[0].paragraphs[0]
        .controls
        .iter()
        .find_map(|control| match control {
            Control::Generic(generic) if generic.ctrl_id == *b"pgnp" => Some(generic),
            _ => None,
        })
}

fn semantic_digest(path: &Path) -> serde_json::Value {
    let output = command_output(
        hwp().args(["cat", "--format", "json"]).arg(path),
        "hwp cat --format json",
    );
    assert_success(&output, "hwp cat --format json");
    let mut value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    if let Some(meta) = value
        .get_mut("meta")
        .and_then(serde_json::Value::as_object_mut)
    {
        meta.remove("source_version");
    }
    value
}

fn hwpx_entry(path: &Path, name: &str) -> String {
    let file = fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(name).unwrap();
    let mut xml = String::new();
    entry.read_to_string(&mut xml).unwrap();
    xml
}

fn nested_markdown(depth: usize) -> String {
    (0..depth)
        .map(|level| format!("{}1. level {}", "   ".repeat(level), level + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn nested_html(depth: usize) -> String {
    let mut html = String::new();
    for level in 1..=depth {
        html.push_str(&format!("<ol><li>level {level}"));
    }
    for _ in 0..depth {
        html.push_str("</li></ol>");
    }
    html
}

#[test]
fn canonical_profiles_round_trip_in_hwp_and_hwpx() {
    let root = temp_dir("canonical-matrix");
    let input = root.join("input.md");
    fs::write(
        &input,
        "source first\n\n1. numbered item\n   1. nested item\n\nsource last\n",
    )
    .unwrap();

    let profiles = [
        ("official", "맑은 고딕", 1200, 160, 0, false),
        ("report", "함초롬바탕", 1500, 160, 4252, true),
        ("plan", "함초롬바탕", 1500, 160, 4252, true),
        ("notice", "맑은 고딕", 1500, 160, 2834, true),
        ("minutes", "함초롬바탕", 1400, 130, 0, false),
        ("gaejosik", "맑은 고딕", 1500, 160, 4252, true),
        ("press", "함초롬바탕", 1400, 160, 2834, true),
    ];

    for (profile, font, body_size, line_spacing, header_footer, has_page_number) in profiles {
        for extension in ["hwp", "hwpx"] {
            let output = root.join(format!("{profile}.{extension}"));
            let created = command_output(
                hwp()
                    .args(["new", "--from"])
                    .arg(&input)
                    .args(["--preset", profile, "--output"])
                    .arg(&output),
                &format!("hwp new {profile} {extension}"),
            );
            assert_success(&created, &format!("hwp new {profile} {extension}"));
            assert!(output.exists(), "{profile}/{extension} must publish");

            let document = reread(&output);
            let page = page_def(&document);
            assert_eq!(
                document.header.fonts[0][0].name, font,
                "{profile}/{extension}"
            );
            assert_eq!(
                document.header.char_shapes[0].base_size, body_size,
                "{profile}/{extension}"
            );
            assert!(
                document
                    .header
                    .para_shapes
                    .iter()
                    .all(|shape| shape.line_spacing == line_spacing),
                "{profile}/{extension}"
            );
            assert_eq!(page.margin_top.0, 5668, "{profile}/{extension}");
            assert_eq!(page.margin_bottom.0, 2834, "{profile}/{extension}");
            assert_eq!(page.margin_left.0, 5668, "{profile}/{extension}");
            assert_eq!(page.margin_right.0, 5668, "{profile}/{extension}");
            assert_eq!(page.margin_header.0, header_footer, "{profile}/{extension}");
            assert_eq!(page.margin_footer.0, header_footer, "{profile}/{extension}");

            let levels = &document.header.numbering_levels[0];
            assert_eq!(levels.len(), 8, "{profile}/{extension}");
            assert_eq!(levels[0].fmt, NumFmt::Digit, "{profile}/{extension}");
            assert_eq!(
                levels[1].fmt,
                NumFmt::HangulSyllable,
                "{profile}/{extension}"
            );
            assert_eq!(levels[6].fmt, NumFmt::CircledDigit, "{profile}/{extension}");
            assert_eq!(
                levels[7].fmt,
                NumFmt::CircledHangulSyllable,
                "{profile}/{extension}"
            );

            if extension == "hwp" {
                for paragraph in &document.sections[0].paragraphs {
                    let Some(shape) = document
                        .header
                        .para_shapes
                        .get(paragraph.para_shape.0 as usize)
                    else {
                        continue;
                    };
                    if shape.head_type() != 2 {
                        continue;
                    }
                    assert_eq!(
                        shape.tail.len(),
                        16,
                        "{profile}/{extension}: official HWP5 list shape tail"
                    );
                    assert_eq!(
                        u32::from_le_bytes(shape.tail[12..16].try_into().unwrap()),
                        u32::from(shape.head_level() - 1),
                        "{profile}/{extension}: reread paragraph retains native list-level binding"
                    );
                    assert_eq!(
                        u32::from_le_bytes(shape.tail[8..12].try_into().unwrap()),
                        u32::try_from(line_spacing).unwrap(),
                        "{profile}/{extension}: native list shape retains profile line spacing"
                    );
                }
            }

            let pgnp = page_number(&document);
            assert_eq!(pgnp.is_some(), has_page_number, "{profile}/{extension}");
            if let Some(pgnp) = pgnp {
                let properties = u32::from_le_bytes(pgnp.data[..4].try_into().unwrap());
                assert_eq!((properties >> 8) & 0xff, 5, "{profile}/{extension}");
                assert_eq!(
                    u16::from_le_bytes(pgnp.data[10..12].try_into().unwrap()),
                    u16::from(b'-'),
                    "{profile}/{extension}"
                );
            }

            let plain = command_output(hwp().args(["cat"]).arg(&output), "hwp cat");
            assert_success(&plain, "hwp cat");
            let plain = String::from_utf8_lossy(&plain.stdout);
            let first = plain.find("source first").expect("first source paragraph");
            let middle = plain
                .find("numbered item")
                .expect("numbered source paragraph");
            let last = plain.find("source last").expect("last source paragraph");
            assert!(
                first < middle && middle < last,
                "{profile}/{extension}: {plain}"
            );

            if extension == "hwpx" {
                let header = hwpx_entry(&output, "Contents/header.xml");
                let section = hwpx_entry(&output, "Contents/section0.xml");
                assert!(
                    header.contains("CIRCLED_HANGUL_SYLLABLE"),
                    "{profile} must serialize level-eight numbering"
                );
                assert!(
                    section.contains("<hp:pagePr "),
                    "{profile} must serialize PageDef"
                );
                assert_eq!(
                    section.contains(
                        "<hp:pageNum pos=\"BOTTOM_CENTER\" formatType=\"DIGIT\" sideChar=\"-\"/>"
                    ),
                    has_page_number,
                    "{profile} page-number XML"
                );
            }
        }
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn official_hwp_rejects_independent_ordered_lists_before_publication() {
    let root = temp_dir("independent-official-lists");
    let input = root.join("input.md");
    let output = root.join("independent.hwp");
    fs::write(&input, "1. first list\n\nplain gap\n\n1. second list\n").unwrap();

    let result = command_output(
        hwp()
            .args(["new", "--from"])
            .arg(&input)
            .args(["--preset", "official", "--output"])
            .arg(&output),
        "independent official ordered lists",
    );

    assert!(
        !result.status.success(),
        "unproven topology must fail closed"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("직접 HWP5 공식 번호 체계 구조"),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !output.exists(),
        "rejected topology must not publish an HWP"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn official_hwp_rejects_explicit_ordered_list_start_but_hwpx_preserves_it() {
    let root = temp_dir("explicit-official-list-start");
    let input = root.join("input.md");
    let hwp_output = root.join("explicit-start.hwp");
    let hwpx_output = root.join("explicit-start.hwpx");
    fs::write(&input, "3. explicit start\n").unwrap();

    let hwp_result = command_output(
        hwp()
            .args(["new", "--from"])
            .arg(&input)
            .args(["--preset", "official", "--output"])
            .arg(&hwp_output),
        "explicit official HWP list start",
    );
    assert!(
        !hwp_result.status.success(),
        "unproven explicit start must fail closed for HWP"
    );
    assert!(
        String::from_utf8_lossy(&hwp_result.stderr).contains("직접 HWP5 공식 번호 체계 구조"),
        "stderr: {}",
        String::from_utf8_lossy(&hwp_result.stderr)
    );
    assert!(
        !hwp_output.exists(),
        "rejected explicit start must not publish an HWP"
    );

    let hwpx_result = command_output(
        hwp()
            .args(["new", "--from"])
            .arg(&input)
            .args(["--preset", "official", "--output"])
            .arg(&hwpx_output),
        "explicit official HWPX list start",
    );
    assert_success(&hwpx_result, "explicit official HWPX list start");
    let document = reread(&hwpx_output);
    assert_eq!(document.header.numbering_levels[0][0].start, 3);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn aliases_margins_and_input_failures_are_shipped_binary_gates() {
    let root = temp_dir("edge-matrix");
    let input = root.join("input.md");
    fs::write(&input, "1. item\n   1. nested\n").unwrap();

    let aliases = [
        (
            "official",
            [
                "official",
                "gian",
                "gongmun",
                "기안",
                "기안문",
                "공문",
                "공문서",
            ],
        ),
        (
            "report",
            ["report", "bogoseo", "보고", "보고서", "", "", ""],
        ),
        (
            "plan",
            ["plan", "계획", "계획서", "사업계획", "사업계획서", "", ""],
        ),
        ("notice", ["notice", "공고", "공고문", "고시", "", "", ""]),
        ("minutes", ["minutes", "회의록", "회의기록", "", "", "", ""]),
        ("gaejosik", ["gaejosik", "개조식", "", "", "", "", ""]),
        ("press", ["press", "보도", "보도자료", "", "", "", ""]),
    ];
    for (canonical, names) in aliases {
        let canonical_output = root.join(format!("canonical-{canonical}.hwpx"));
        let canonical_run = command_output(
            hwp()
                .args(["new", "--from"])
                .arg(&input)
                .args(["--preset", canonical, "--output"])
                .arg(&canonical_output),
            "canonical alias baseline",
        );
        assert_success(&canonical_run, "canonical alias baseline");
        let expected = semantic_digest(&canonical_output);
        for (index, alias) in names.iter().filter(|alias| !alias.is_empty()).enumerate() {
            let output = root.join(format!("alias-{canonical}-{index}.hwpx"));
            let result = command_output(
                hwp()
                    .args(["new", "--from"])
                    .arg(&input)
                    .args(["--preset", alias, "--output"])
                    .arg(&output),
                "hwp new alias",
            );
            assert_success(&result, &format!("hwp new --preset {alias}"));
            assert_eq!(semantic_digest(&output), expected, "alias {alias}");
        }
    }

    let margins = [
        ("--margin-top", "25", "margin_top", 7087),
        ("--margin-bottom", "15", "margin_bottom", 4252),
        ("--margin-left", "30", "margin_left", 8504),
        ("--margin-right", "35", "margin_right", 9921),
    ];
    for (flag, value, side, expected) in margins {
        let output = root.join(format!("{side}.hwpx"));
        let result = command_output(
            hwp()
                .args(["new", "--from"])
                .arg(&input)
                .args(["--preset", "official", flag, value, "--output"])
                .arg(&output),
            "hwp new margin override",
        );
        assert_success(&result, flag);
        let page = page_def(&reread(&output));
        let actual = match side {
            "margin_top" => page.margin_top.0,
            "margin_bottom" => page.margin_bottom.0,
            "margin_left" => page.margin_left.0,
            "margin_right" => page.margin_right.0,
            _ => unreachable!(),
        };
        assert_eq!(actual, expected, "{flag}");
    }

    let invalid_cases: Vec<(&str, Vec<&str>)> = vec![
        ("unknown-preset", vec!["--preset", "unknown"]),
        ("range", vec!["--preset", "official", "--margin-top", "201"]),
        (
            "sum",
            vec![
                "--preset",
                "official",
                "--margin-left",
                "200",
                "--margin-right",
                "200",
            ],
        ),
    ];
    for (name, args) in invalid_cases {
        let output = root.join(format!("rejected-{name}.hwpx"));
        let result = command_output(
            hwp().arg("new").args(args).args(["--output"]).arg(&output),
            &format!("invalid {name}"),
        );
        assert!(!result.status.success(), "{name} must fail");
        assert!(!output.exists(), "{name} must not publish output");
    }

    let json_input = root.join("input.json");
    fs::write(
        &json_input,
        serde_json::to_vec(&semantic_digest(&root.join("canonical-official.hwpx"))).unwrap(),
    )
    .unwrap();
    for (name, args) in [
        ("json-preset", vec!["--preset", "official"]),
        ("json-margin", vec!["--margin-top", "25"]),
    ] {
        let output = root.join(format!("rejected-{name}.hwpx"));
        let result = command_output(
            hwp()
                .args(["new", "--from"])
                .arg(&json_input)
                .args(args)
                .args(["--output"])
                .arg(&output),
            name,
        );
        assert!(!result.status.success(), "{name} must fail");
        assert!(!output.exists(), "{name} must not publish output");
    }

    for (name, contents) in [
        ("markdown-depth9", nested_markdown(9)),
        ("html-depth9", nested_html(9)),
    ] {
        let source = root.join(format!("{name}.md"));
        let output = root.join(format!("rejected-{name}.hwpx"));
        fs::write(&source, contents).unwrap();
        let result = command_output(
            hwp()
                .args(["new", "--from"])
                .arg(&source)
                .args(["--preset", "official", "--output"])
                .arg(&output),
            name,
        );
        assert!(!result.status.success(), "{name} must fail closed");
        assert!(!output.exists(), "{name} must not publish output");
    }

    let empty = root.join("empty.hwp");
    let empty_run = command_output(
        hwp()
            .args(["new", "--preset", "report", "--output"])
            .arg(&empty),
        "empty profile input",
    );
    assert_success(&empty_run, "empty profile input");
    assert!(empty.exists(), "empty input must remain a valid document");
    assert!(
        page_number(&reread(&empty)).is_some(),
        "empty report keeps page number"
    );

    fs::remove_dir_all(root).unwrap();
}
