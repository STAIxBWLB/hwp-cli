//! `hwp merge` / `hwp split` cross-command integration tests (03-03).
//!
//! This is the automated proxy `03-VALIDATION.md` names for the Hancom verdict on
//! FLOW-02: a merge-then-split round-trip that needs no Hancom and no corpus, plus
//! the D-16 all-or-nothing publication guarantee.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "common/fixture_skip.rs"]
mod fixture_skip;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

/// A fresh scratch directory per call — a distinct directory per test (and per
/// call within a test), so concurrent `cargo test` runs never collide.
fn scratch_dir(label: &str) -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "hwp-cli-document-workflows-{label}-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds a genuine `.hwp` file from markdown through the public CLI import
/// path (`hwp new --from`) — no direct `hwp_convert`/writer calls, so this
/// exercises the same input-construction path a real user would use.
fn write_input_hwp(dir: &Path, stem: &str, markdown: &str) -> PathBuf {
    let md = dir.join(format!("{stem}.md"));
    std::fs::write(&md, markdown).unwrap();
    let hwp_path = dir.join(format!("{stem}.hwp"));
    let status = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&hwp_path)
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from {stem}.md 실패");
    hwp_path
}

/// Plain-text content of an HWP/HWPX file via `hwp cat --format plain`.
fn cat_plain(path: &Path) -> String {
    let output = hwp()
        .args(["cat", "--format", "plain"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "hwp cat {} 실패", path.display());
    String::from_utf8(output.stdout).unwrap()
}

/// Includes every modeled paragraph list, including captions and generic-control lists.
fn paragraph_lists<'a>(
    paragraphs: &'a [hwp_model::Paragraph],
    at: String,
    out: &mut Vec<(String, &'a [hwp_model::Paragraph])>,
) {
    out.push((at.clone(), paragraphs));
    for (p, para) in paragraphs.iter().enumerate() {
        for (c, control) in para.controls.iter().enumerate() {
            let caption = match control {
                hwp_model::Control::Table(table) => {
                    for (k, cell) in table.cells.iter().enumerate() {
                        paragraph_lists(
                            &cell.paragraphs,
                            format!("{at} p{p} table{c} cell{k}"),
                            out,
                        );
                    }
                    &table.caption
                }
                hwp_model::Control::Picture(picture) => &picture.caption,
                hwp_model::Control::Generic(generic) => {
                    for (k, list) in generic.paragraph_lists.iter().enumerate() {
                        paragraph_lists(
                            &list.paragraphs,
                            format!("{at} p{p} ctrl{c} list{k}"),
                            out,
                        );
                    }
                    &generic.caption
                }
                hwp_model::Control::SectionDef(_) => continue,
            };
            if let Some(caption) = caption {
                paragraph_lists(
                    &caption.paragraphs,
                    format!("{at} p{p} ctrl{c} caption"),
                    out,
                );
            }
        }
    }
}

/// Produces external-style HWPX input that our writer's defaults would otherwise hide:
/// zero conversion fields and, optionally, a border collection shorter than two entries.
fn write_hwpx_border_fixture(document: &hwp_model::Document, path: &Path, count: Option<usize>) {
    use std::io::{Read, Write};

    hwpx::write_document(document, path).unwrap();
    let original = std::fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(original)).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let references = regex::Regex::new(r#"borderFillIDRef="[0-9]+""#).unwrap();
    let spacing = regex::Regex::new(r#"(<hh:lineSpacing[^>]*value=")[0-9]+(")"#).unwrap();
    let fill_elements =
        regex::Regex::new(r#"(?s)<hh:borderFill id="[0-9]+".*?</hh:borderFill>"#).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let method = entry.compression();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == "Contents/header.xml" || name.starts_with("Contents/section") {
            let xml = String::from_utf8(bytes).unwrap();
            let mut xml = references
                .replace_all(&xml, r#"borderFillIDRef="0""#)
                .into_owned();
            xml = spacing.replace_all(&xml, "${1}0${2}").into_owned();
            if name == "Contents/header.xml"
                && let Some(count) = count
            {
                let begin = xml.find("<hh:borderFills ").unwrap();
                let end = xml.find("</hh:borderFills>").unwrap() + "</hh:borderFills>".len();
                let fills = fill_elements
                    .find_iter(&xml[begin..end])
                    .take(count)
                    .map(|fill| fill.as_str())
                    .collect::<String>();
                xml.replace_range(
                    begin..end,
                    &format!(r#"<hh:borderFills itemCnt="{count}">{fills}</hh:borderFills>"#),
                );
            }
            bytes = xml.into_bytes();
        }
        writer
            .start_file(
                name,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn fragment_paths(dir: &Path, stem: &str, ext: &str) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(&format!("{stem}-")) && name.ends_with(&format!(".{ext}"))
                })
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn merge_then_split_reproduces_each_input_section() {
    let dir = scratch_dir("roundtrip");
    let a = write_input_hwp(&dir, "a", "문서 A의 본문입니다\n");
    let b = write_input_hwp(&dir, "b", "문서 B의 본문입니다\n");
    let merged = dir.join("merged.hwp");

    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&b)
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let out_dir = dir.join("frag");
    let status = hwp()
        .arg("split")
        .arg(&merged)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");

    let fragments = fragment_paths(&out_dir, "merged", "hwp");
    assert_eq!(
        fragments.len(),
        2,
        "조각 두 개가 나와야 합니다: {fragments:?}"
    );

    let fragment_one = cat_plain(&fragments[0]);
    let fragment_two = cat_plain(&fragments[1]);
    let input_one = cat_plain(&a);
    let input_two = cat_plain(&b);

    assert!(
        fragment_one.contains(input_one.trim()),
        "조각 1에 입력 1의 본문이 있어야 합니다: {fragment_one:?}"
    );
    assert!(
        fragment_two.contains(input_two.trim()),
        "조각 2에 입력 2의 본문이 있어야 합니다: {fragment_two:?}"
    );
    assert!(!fragment_one.contains(input_two.trim()));
    assert!(!fragment_two.contains(input_one.trim()));

    std::fs::remove_dir_all(dir).unwrap();
}

/// Merging into `.hwp` writes a NUMBERING and a BULLET record for every definition a paragraph
/// shape references (#377). The second input's definitions sit past the first input's raw
/// records, and the HWP5 writer used to drop them, leaving a dangling numbering reference.
#[test]
fn merge_into_hwp_writes_a_record_for_every_list_definition() {
    let dir = scratch_dir("merge-lists");
    let first = write_input_hwp(&dir, "first", "# 제목\n\n본문\n");
    let md = dir.join("lists.md");
    std::fs::write(&md, "# 둘째\n\n- 가\n- 나\n\n1. 하나\n2. 둘\n").unwrap();
    let lists = dir.join("lists.hwpx");
    let status = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&lists)
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from lists.md 실패");
    let merged = dir.join("merged.hwp");
    let status = hwp()
        .arg("merge")
        .arg(&first)
        .arg(&lists)
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let header = hwp5::read_document(&merged).unwrap().document.header;
    assert!(
        header
            .para_shapes
            .iter()
            .any(|shape| shape.head_type() == 2 && shape.numbering_id > 0),
        "the second input's numbering follows the first input's record"
    );
    for shape in &header.para_shapes {
        let records = match shape.head_type() {
            2 => header.numberings.len(),
            3 => header.bullets.len(),
            _ => continue,
        };
        assert!(
            usize::from(shape.numbering_id) < records,
            "definition id {} has no record ({records} written)",
            shape.numbering_id
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An `.hwp` first input merged with an `.hwpx` input writes the HWP5 paragraph invariants on
/// every paragraph, the HWPX input's included (#381): Hancom refused the file when that input's
/// paragraphs had no terminator, no last-paragraph flag, no section-break bits and instance id 0.
#[test]
fn merge_of_an_hwpx_input_into_hwp_writes_the_paragraph_invariants() {
    let dir = scratch_dir("merge-hwpx-into-hwp");
    let first = write_input_hwp(&dir, "first", "# 제목\n\n본문\n");
    let md = dir.join("second.md");
    std::fs::write(
        &md,
        "둘째 문서\n\n| 가 | 나 |\n|---|---|\n| 다 | 라 |\n\n마지막 문단\n",
    )
    .unwrap();
    let second = dir.join("second.hwpx");
    let status = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&second)
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from second.md 실패");
    let merged = dir.join("merged.hwp");
    let status = hwp()
        .arg("merge")
        .arg(&first)
        .arg(&second)
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let doc = hwp5::read_document(&merged).unwrap().document;
    assert_eq!(doc.sections.len(), 2);
    let mut all = Vec::new();
    for (s, section) in doc.sections.iter().enumerate() {
        paragraph_lists(&section.paragraphs, format!("section {s}"), &mut all);
        assert_eq!(
            section.paragraphs[0].header.break_type & 0x03,
            0x03,
            "section {s}: section-break bits"
        );
    }
    assert!(
        all.iter().any(|(at, _)| at.contains("cell")),
        "the HWPX input's table cells are covered"
    );
    // New instance ids never repeat one in use: the first input, made by hwp-cli, already
    // numbers its paragraphs from 0x10000001, so the HWPX input's must not reuse those.
    let mut ids = std::collections::BTreeSet::new();
    for (at, paragraphs) in &all {
        let last = paragraphs.len() - 1;
        for (p, para) in paragraphs.iter().enumerate() {
            let at = format!("{at} paragraph {p}");
            assert_eq!(
                para.chars.last(),
                Some(&hwp_model::HwpChar::CharCtrl(
                    hwp_model::ctrl_char::PARA_BREAK
                )),
                "{at}: paragraph terminator"
            );
            assert_eq!(
                para.header.chars_flags & 0x80 != 0,
                p == last,
                "{at}: last-paragraph flag"
            );
            assert_ne!(para.header.instance_id, 0, "{at}: instance id");
            assert!(
                ids.insert(para.header.instance_id),
                "{at}: duplicate instance id {:#x}",
                para.header.instance_id
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two generated HWP files already share IDs, including their table-cell paragraphs (#393).
#[test]
fn merge_existing_hwp_paragraph_ids_are_unique_across_sections_and_cells() {
    let dir = scratch_dir("merge-existing-ids");
    let markdown = "본문\n\n| 가 | 나 |\n|---|---|\n| 다 | 라 |\n\n마지막\n";
    let first = write_input_hwp(&dir, "first", markdown);
    let second = write_input_hwp(&dir, "second", markdown);
    let merged = dir.join("merged.hwp");
    let status = hwp()
        .arg("merge")
        .args([&first, &second])
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success());

    let original = hwp5::read_document(&first).unwrap().document;
    let written = hwp5::read_document(&merged).unwrap().document;
    assert_eq!(written.sections.len(), 2);
    let ids = |section: &hwp_model::Section| {
        let mut lists = Vec::new();
        paragraph_lists(&section.paragraphs, "section".to_string(), &mut lists);
        assert!(lists.iter().any(|(at, _)| at.contains("cell")));
        lists
            .into_iter()
            .flat_map(|(_, paragraphs)| {
                paragraphs
                    .iter()
                    .map(|paragraph| paragraph.header.instance_id)
            })
            .collect::<Vec<_>>()
    };
    let first_ids = ids(&original.sections[0]);
    assert_eq!(
        ids(&written.sections[0]),
        first_ids,
        "first occurrences keep their IDs"
    );
    let output_ids = written.sections.iter().flat_map(ids).collect::<Vec<_>>();
    assert!(!output_ids.contains(&0));
    assert_eq!(
        output_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        output_ids.len(),
        "every section and cell paragraph has a distinct ID",
    );
    assert_eq!(
        written.plain_text(),
        format!("{}{}", original.plain_text(), original.plain_text())
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// Conversion defaults must be applied before header offsets, and only for HWP output (#390).
#[test]
fn merge_prepares_converted_shapes_before_grafting_their_border_references() {
    let dir = scratch_dir("merge-shape-provenance");
    let native_path = write_input_hwp(&dir, "native", "원본 문단\n");
    let mut native = hwp5::read_document(&native_path).unwrap().document;
    // A distinct style signature requires the general graft, as a genuine file does.
    native.header.styles[0].name = "native-style".to_string();
    native.header.para_shapes[0].border_fill_id = 0;
    native.header.para_shapes[0].line_spacing_old = 0;
    hwp5::write_document_with_report(
        &native,
        &native_path,
        &hwp5::WriteOptions {
            preserve_linesegs: true,
            ..Default::default()
        },
    )
    .unwrap();

    let converted_path = dir.join("converted.hwpx");
    let mut converted = hwp_convert::from_markdown("변환 문단\n");
    converted.header.para_shapes[0].border_fill_id = 0;
    converted.header.border_fills[1].fill_type = 1;
    converted.header.border_fills[1].bg_color = Some(0x0001_0203);
    write_hwpx_border_fixture(&converted, &converted_path, None);
    let converted = hwpx::read_document(&converted_path).unwrap().document;
    assert_eq!(converted.header.para_shapes[0].border_fill_id, 0);
    assert_eq!(converted.header.para_shapes[0].line_spacing_old, 0);

    for native_first in [true, false] {
        let inputs = if native_first {
            [&native_path, &converted_path]
        } else {
            [&converted_path, &native_path]
        };
        for extension in ["hwp", "hwpx"] {
            let output = dir.join(format!("merged-{native_first}.{extension}"));
            let status = hwp()
                .arg("merge")
                .args(inputs)
                .arg("-o")
                .arg(&output)
                .status()
                .unwrap();
            assert!(status.success());
            let header = if extension == "hwp" {
                hwp5::read_document(&output).unwrap().document.header
            } else {
                hwpx::read_document(&output).unwrap().document.header
            };
            let native_offset = if native_first {
                0
            } else {
                converted.header.para_shapes.len()
            };
            let converted_offset = if native_first {
                native.header.para_shapes.len()
            } else {
                0
            };
            let border_offset = if native_first {
                native.header.border_fills.len()
            } else {
                0
            };
            let genuine_shape = &header.para_shapes[native_offset];
            assert_eq!(
                genuine_shape.border_fill_id,
                if extension == "hwp" { 0 } else { 2 }
            );
            assert_eq!(
                genuine_shape.line_spacing_old,
                if extension == "hwp" {
                    0
                } else {
                    native.header.para_shapes[0].line_spacing
                },
            );
            let converted_shape = &header.para_shapes[converted_offset];
            if extension == "hwp" {
                assert_eq!(converted_shape.line_spacing_old, 160);
                assert_eq!(converted_shape.border_fill_id, border_offset as u16 + 2);
                assert_eq!(
                    header.border_fills[border_offset + 1].bg_color,
                    Some(0x0001_0203)
                );
            } else {
                assert_eq!(
                    converted_shape.border_fill_id, 2,
                    "HWPX keeps its existing zero-reference serialization, without HWP5 pre-graft offsets"
                );
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn merge_converted_inputs_with_missing_border_defaults_own_their_fallback() {
    let dir = scratch_dir("merge-short-border-palette");
    let native_path = write_input_hwp(&dir, "native", "원본 문단\n");
    let mut native = hwp5::read_document(&native_path).unwrap().document;
    native.header.para_shapes[0].border_fill_id = 0;
    hwp5::write_document_with_report(&native, &native_path, &Default::default()).unwrap();
    for original_count in [0, 1] {
        let converted_path = dir.join(format!("converted-{original_count}.hwpx"));
        let mut converted = hwp_convert::from_markdown("변환 문단\n");
        converted.header.border_fills.truncate(original_count);
        for shape in &mut converted.header.para_shapes {
            shape.border_fill_id = 0;
        }
        for shape in &mut converted.header.char_shapes {
            shape.border_fill_id = 0;
        }
        if let Some(fill) = converted.header.border_fills.first_mut() {
            fill.fill_type = 1;
            fill.bg_color = Some(0x0004_0506);
        }
        write_hwpx_border_fixture(&converted, &converted_path, Some(original_count));
        let converted = hwpx::read_document(&converted_path).unwrap().document;
        assert_eq!(converted.header.border_fills.len(), original_count);
        for native_first in [true, false] {
            let inputs = if native_first {
                [&native_path, &converted_path]
            } else {
                [&converted_path, &native_path]
            };
            let output = dir.join(format!("merged-{original_count}-{native_first}.hwp"));
            assert!(
                hwp()
                    .arg("merge")
                    .args(inputs)
                    .arg("-o")
                    .arg(&output)
                    .status()
                    .unwrap()
                    .success()
            );
            let header = hwp5::read_document(&output).unwrap().document.header;
            let (native_offset, converted_offset, border_offset) = if native_first {
                (
                    0,
                    native.header.para_shapes.len(),
                    native.header.border_fills.len(),
                )
            } else {
                (converted.header.para_shapes.len(), 0, 0)
            };
            assert_eq!(header.para_shapes[native_offset].border_fill_id, 0);
            assert_eq!(
                header.para_shapes[converted_offset].border_fill_id,
                border_offset as u16 + 2
            );
            assert_eq!(
                header.border_fills.len(),
                native.header.border_fills.len() + 2
            );
            let fallback = &header.border_fills[border_offset + 1];
            assert_eq!(fallback.fill_type, 0);
            assert!(fallback.sides.iter().all(|side| side.line_type == 0));
            if original_count == 1 {
                assert_eq!(
                    header.border_fills[border_offset].bg_color,
                    Some(0x0004_0506),
                    "existing source fill stays intact"
                );
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// A mixed merge leaves the `.hwp` input's own paragraph shapes as they are (#381 review): a
/// genuine 5.0.2.x file uses `border_fill_id` 0 for "no border", which the conversion defaults
/// would turn into a solid border box around its cell paragraphs.
#[test]
fn a_mixed_merge_keeps_the_hwp_inputs_paragraph_shapes() {
    let genuine =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/hwp5/work_report.hwp");
    if fixture_skip::fixture_missing(&genuine) {
        return;
    }
    let dir = scratch_dir("merge-genuine-shapes");
    let md = dir.join("second.md");
    std::fs::write(&md, "둘째 문서\n").unwrap();
    let second = dir.join("second.hwpx");
    let status = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&second)
        .status()
        .unwrap();
    assert!(status.success(), "hwp new --from second.md 실패");
    let source = hwp5::read_document(&genuine).unwrap().document.header;
    let converted = hwpx::read_document(&second).unwrap().document.header;
    assert!(
        source
            .para_shapes
            .iter()
            .any(|shape| shape.border_fill_id == 0)
    );
    for genuine_first in [true, false] {
        let merged = dir.join(format!("merged-{genuine_first}.hwp"));
        let inputs = if genuine_first {
            [&genuine, &second]
        } else {
            [&second, &genuine]
        };
        let status = hwp()
            .arg("merge")
            .args(inputs)
            .arg("-o")
            .arg(&merged)
            .status()
            .unwrap();
        assert!(status.success(), "hwp merge failed");

        let output = hwp5::read_document(&merged).unwrap().document.header;
        let (shape_offset, border_offset) = if genuine_first {
            (0, 0)
        } else {
            (
                converted.para_shapes.len(),
                converted.border_fills.len() as u16,
            )
        };
        for (index, shape) in source.para_shapes.iter().enumerate() {
            let written = &output.para_shapes[shape_offset + index];
            let border_fill_id = if shape.border_fill_id == 0 {
                0
            } else {
                shape.border_fill_id + border_offset
            };
            assert_eq!(
                (written.border_fill_id, written.line_spacing_old),
                (border_fill_id, shape.line_spacing_old),
                "para shape {index}, genuine first: {genuine_first}",
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn split_publishes_all_fragments_or_none() {
    let dir = scratch_dir("atomicity");
    let a = write_input_hwp(&dir, "a", "문서 A\n");
    let b = write_input_hwp(&dir, "b", "문서 B\n");
    let c = write_input_hwp(&dir, "c", "문서 C\n");
    let input = dir.join("in.hwp");

    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&b)
        .arg(&c)
        .arg("-o")
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let out_dir = dir.join("frag");
    std::fs::create_dir_all(&out_dir).unwrap();
    // Force a failure the transaction cannot recover from: the destination
    // for the third fragment is a directory, not a regular file — the
    // publish precheck refuses to replace it (output.rs's inspect_destination).
    std::fs::create_dir_all(out_dir.join("in-003.hwp")).unwrap();

    let status = hwp()
        .arg("split")
        .arg(&input)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(!status.success(), "강제 실패인데 성공했습니다");

    // D-16: the fragment set is never partially published — none of the
    // three fragment file names exist after the forced failure.
    assert!(!out_dir.join("in-001.hwp").exists());
    assert!(!out_dir.join("in-002.hwp").exists());
    assert!(
        out_dir.join("in-003.hwp").is_dir(),
        "사전에 만든 디렉터리가 그대로 남아 있어야 합니다"
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn split_refuses_loss_report_aliasing_the_input() {
    // Issue #167: `hwp split in.hwp --out-dir frag --loss-report in.hwp` must
    // be refused — the report write must never overwrite the input with JSON.
    let dir = scratch_dir("split-loss-report-alias");
    let input = write_input_hwp(&dir, "in", "본문입니다\n");
    let out_dir = dir.join("frag");
    let original = std::fs::read(&input).unwrap();

    let output = hwp()
        .arg("split")
        .arg(&input)
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--loss-report")
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "입력과 같은 --loss-report 경로는 거부되어야 합니다"
    );
    // The refusal fires before --out-dir is even created, so no fragment set
    // can exist — tolerate the directory simply being absent.
    assert!(
        !out_dir.exists() || fragment_paths(&out_dir, "in", "hwp").is_empty(),
        "거부된 분할이 조각을 게시하면 안 됩니다"
    );
    assert_eq!(
        std::fs::read(&input).unwrap(),
        original,
        "입력 파일이 훼손되면 안 됩니다"
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn 단일_구역_입력은_조각_하나만_낸다() {
    let dir = scratch_dir("single-section");
    let input = write_input_hwp(&dir, "solo", "단일 구역 본문\n");
    let out_dir = dir.join("frag");

    let status = hwp()
        .arg("split")
        .arg(&input)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success());

    assert!(out_dir.join("solo-001.hwp").exists());
    assert!(!out_dir.join("solo-002.hwp").exists());

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn 동일한_두_구역은_서로_다른_조각_파일_두_개가_된다() {
    let dir = scratch_dir("adjacency");
    let a = write_input_hwp(&dir, "same", "같은 내용\n");
    let merged = dir.join("merged.hwp");

    // FLOW-02 adjacency probe: merge the same input with itself so the
    // merged document carries two byte-equal Sections (D-02: sections never
    // fuse), then confirm split never collapses them either.
    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&a)
        .arg("-o")
        .arg(&merged)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");

    let out_dir = dir.join("frag");
    let status = hwp()
        .arg("split")
        .arg(&merged)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");

    let fragments = fragment_paths(&out_dir, "merged", "hwp");
    assert_eq!(fragments.len(), 2);
    assert_ne!(fragments[0], fragments[1]);
    assert_eq!(cat_plain(&fragments[0]), cat_plain(&fragments[1]));

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn split_removes_stale_fragments_from_a_previous_larger_run() {
    // Issue #177: re-running split with fewer fragments must not leave stale
    // stem-NNN files in --out-dir.
    let dir = scratch_dir("stale-fragments");
    let out_dir = dir.join("frag");

    // First run: a two-section input (same stem "in", own directory).
    let dir_a = dir.join("a");
    std::fs::create_dir_all(&dir_a).unwrap();
    let a = write_input_hwp(&dir_a, "part-a", "문서 A\n");
    let b = write_input_hwp(&dir_a, "part-b", "문서 B\n");
    let two_section = dir_a.join("in.hwp");
    let status = hwp()
        .arg("merge")
        .arg(&a)
        .arg(&b)
        .arg("-o")
        .arg(&two_section)
        .status()
        .unwrap();
    assert!(status.success(), "hwp merge 실패");
    let status = hwp()
        .arg("split")
        .arg(&two_section)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");
    assert_eq!(fragment_paths(&out_dir, "in", "hwp").len(), 2);

    // Second run: a single-section input with the same stem, same --out-dir.
    let dir_b = dir.join("b");
    std::fs::create_dir_all(&dir_b).unwrap();
    let one_section = write_input_hwp(&dir_b, "in", "단일 구역\n");
    let status = hwp()
        .arg("split")
        .arg(&one_section)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");

    assert!(out_dir.join("in-001.hwp").exists());
    assert!(
        !out_dir.join("in-002.hwp").exists(),
        "이전 실행의 초과 조각이 남아 있으면 안 됩니다"
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn split_normalizes_the_fragment_extension_to_lowercase() {
    // Issue #177: an uppercase input extension must produce lowercase
    // fragment names, matching the rest of the CLI's casing rule.
    let dir = scratch_dir("lowercase-extension");
    let lower = write_input_hwp(&dir, "in", "본문입니다\n");
    let upper = dir.join("in.HWP");
    std::fs::rename(&lower, &upper).unwrap();
    let out_dir = dir.join("frag");

    let status = hwp()
        .arg("split")
        .arg(&upper)
        .arg("--out-dir")
        .arg(&out_dir)
        .status()
        .unwrap();
    assert!(status.success(), "hwp split 실패");

    let fragments = fragment_paths(&out_dir, "in", "hwp");
    assert_eq!(fragments.len(), 1);
    assert_eq!(
        fragments[0].file_name().and_then(|name| name.to_str()),
        Some("in-001.hwp"),
    );

    std::fs::remove_dir_all(dir).unwrap();
}

/// FLOW-02 `empty` probe (A-04): a document with zero sections is refused
/// rather than published as an empty set.
///
/// No public CLI path can author a genuine zero-section `.hwp`/`.hwpx` file
/// (`hwp new` always emits at least one section, and hand-authoring one
/// through the low-level container writers is undefined territory this
/// crate's own writers never exercise) — so this asserts the exact contract
/// `hwp split` calls into directly: `split_sections` on a document with no
/// sections at all, the same shape `commands::split` would load from any
/// input.
#[test]
fn 구역이_없는_문서는_거부되고_파일이_생기지_않는다() {
    let document = hwp_model::Document::default();
    assert!(document.sections.is_empty());
    let result = hwp_convert::document_split::split_sections(&document);
    assert!(
        result.is_err(),
        "구역이 없는 문서는 분할이 거부되어야 합니다"
    );
}
