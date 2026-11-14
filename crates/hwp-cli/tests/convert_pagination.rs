//! Owner-authored synthetic forms from #410; no private corpus or font golden.
use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::Document;
use hwp_render::segment_map::SegmentMap;
use serde_json::json;

#[allow(dead_code)] // This regression reuses only the shared anchor layer.
#[path = "common/original_form_contract.rs"]
mod original_form_contract;

struct ProbeDir(PathBuf);
impl Drop for ProbeDir {
    fn drop(&mut self) {
        if std::env::var_os("HWP_410_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn run(args: &[&str], dir: &Path) -> Vec<u8> {
    let output = Command::new(env!("CARGO_BIN_EXE_ahwp"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn layout(doc: &Document) -> (SegmentMap, String) {
    let mut store = hwp_render::FontStore::new();
    if let Some(font_dir) = std::env::var_os("HWP_FONT_DIR") {
        store.load_dir(Path::new(&font_dir));
    }
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let (display, map) =
        hwp_render::layout::layout_document_with_segments(doc, &mut store, &mut warnings);
    eprintln!(
        "probe pages={} fonts={:?}",
        display.pages.len(),
        store.resolutions
    );
    assert!(!map.truncated);
    let mut drawn = String::new();
    for item in display.pages.iter().flat_map(|page| &page.items) {
        if let hwp_render::display::Item::Glyphs { run, .. } = item {
            drawn.push_str(&run.text);
        }
    }
    (map, drawn)
}

fn assert_anchors_match(before: &Document, after: &Document) {
    let (before_map, before_text) = layout(before);
    let (after_map, after_text) = layout(after);
    // The CLI text check below covers marker retention without any font. If the
    // host draws glyphs, also forbid suffix duplication in multiline cell layout.
    let rows = before.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            hwp_model::Control::Table(t) => Some(t.rows),
            _ => None,
        })
        .unwrap();
    for drawn in [&before_text, &after_text] {
        if before_text.is_empty() && after_text.is_empty() {
            continue;
        }
        for n in 0..4 {
            for j in 0..rows {
                let marker = format!("Second paragraph with an explicit test marker {n}-{j}");
                let count = drawn
                    .match_indices(&marker)
                    .filter(|(at, _)| {
                        drawn[at + marker.len()..]
                            .chars()
                            .next()
                            .is_none_or(|ch| !ch.is_ascii_digit())
                    })
                    .count();
                assert!(
                    count == 1,
                    "drawn marker must survive exactly once: {marker} count={count}"
                );
            }
        }
    }
    let before_anchors = original_form_contract::anchor_positions(
        before,
        &before_map,
        &original_form_contract::body_paths(before),
    )
    .unwrap();
    let after_anchors = original_form_contract::anchor_positions(
        after,
        &after_map,
        &original_form_contract::body_paths(after),
    )
    .unwrap();
    original_form_contract::compare_anchor_positions(&before_anchors, &after_anchors).unwrap();
    let mut breaks = 0;
    let mut stored_height_checks = 0;
    for (source, output) in before.sections[0]
        .paragraphs
        .iter()
        .zip(&after.sections[0].paragraphs)
    {
        assert_eq!(source.header.break_type & 4, output.header.break_type & 4);
        for (source_ctrl, output_ctrl) in source.controls.iter().zip(&output.controls) {
            if let (hwp_model::Control::Table(a), hwp_model::Control::Table(b)) =
                (source_ctrl, output_ctrl)
            {
                stored_height_checks += 1;
                let source_heights: Vec<_> = a.cells.iter().map(|cell| cell.height).collect();
                let output_heights: Vec<_> = b.cells.iter().map(|cell| cell.height).collect();
                assert_eq!(
                    source_heights, output_heights,
                    "conversion must preserve stored cell minima rather than persisting render-only spacing"
                );
            }
        }
        if source.header.break_type & 4 != 0 {
            breaks += 1;
            assert_eq!(
                output.line_segs.first().unwrap().v_pos,
                0,
                "explicit form-start break must restart the synthesized page-relative anchor"
            );
        }
        let page = before.sections[0].section_def().unwrap().page.unwrap();
        let body_height = page.height.0
            - page.margin_top.0
            - page.margin_bottom.0
            - page.margin_header.0
            - page.margin_footer.0;
        assert!(
            output
                .line_segs
                .iter()
                .all(|seg| seg.v_pos >= 0 && seg.v_pos < body_height),
            "body cache must remain page-relative"
        );
    }
    assert_eq!(breaks, 3);
    assert_eq!(stored_height_checks, 4, "visit every authored form table");
}

fn probe(row_count: usize, merged: bool, following_body: bool) {
    let dir = ProbeDir(std::env::temp_dir().join(format!(
        "hwp410-{}-{row_count}-{merged}",
        std::process::id()
    )));
    std::fs::create_dir_all(&dir.0).unwrap();
    let para = |text: String| json!({"type":"paragraph", "style":"body", "runs":[{"type":"text", "text":text}]});
    let mut blocks = Vec::new();
    for n in 0..4 {
        if n > 0 {
            blocks.push(json!({"type":"break", "kind":"page"}));
        }
        blocks.push(para(format!("Public form {}", n + 1)));
        let rows: Vec<_> = (0..row_count).map(|j| {
            let mut cells = Vec::new();
            if !merged || j != 1 {
                cells.push(json!({"row_span":if merged && j == 0 {2} else {1},
                    "blocks":[para(format!("Field {}", j+1))]}));
            }
            cells.push(json!({"blocks":[para("Synthetic content for deterministic pagination. ".repeat(3)),
                para(format!("Second paragraph with an explicit test marker {n}-{j}"))]}));
            json!({"cells":cells})
        }).collect();
        blocks.push(json!({"type":"table", "width_mm":160,
            "columns":[{"width_mm":35},{"width_mm":125}], "rows": rows}));
        if following_body {
            blocks.push(para(format!("Following body {n}")));
        }
    }
    let spec = json!({"version":"1.0", "metadata":{"title":"Synthetic form pagination probe", "author":"hwp-cli regression probe"},
        "page":{"width_mm":210,"height_mm":297,"margin_left_mm":20,"margin_right_mm":20,"margin_top_mm":20,"margin_bottom_mm":20},
        "styles":{"body":{"font_family":"Noto Sans CJK KR","font_size_pt":10,"align":"left","line_height_percent":120}},
        "sections":[{"blocks":blocks}]});
    std::fs::write(
        dir.0.join("probe.json"),
        serde_json::to_vec_pretty(&spec).unwrap(),
    )
    .unwrap();
    run(&["compose", "probe.json", "-o", "probe.hwpx"], &dir.0);
    run(
        &[
            "edit",
            "probe.hwpx",
            "-o",
            "probe.hwpx",
            "--table-placement",
            "floating",
            "--verify",
        ],
        &dir.0,
    );
    run(
        &[
            "convert",
            "probe.hwpx",
            "-o",
            "probe.hwp",
            "--strict",
            "--loss-report",
            "loss.json",
        ],
        &dir.0,
    );
    let before = hwpx::read_document(&dir.0.join("probe.hwpx"))
        .unwrap()
        .document;
    let after = hwp5::read_document(&dir.0.join("probe.hwp"))
        .unwrap()
        .document;
    // Run the cache assertion first: on pre-fix code it must fail here, not during setup.
    for para in &after.sections[0].paragraphs {
        if para.header.break_type & 4 != 0 {
            assert_eq!(
                para.line_segs.first().unwrap().v_pos,
                0,
                "explicit form-start break must restart the synthesized page-relative anchor"
            );
        }
    }
    assert_anchors_match(&before, &after);
    let source_text = run(&["cat", "probe.hwpx", "--format", "plain"], &dir.0);
    assert_eq!(
        source_text,
        run(&["cat", "probe.hwp", "--format", "plain"], &dir.0)
    );
    let text = String::from_utf8(source_text).unwrap();
    for n in 0..4 {
        for j in 0..row_count {
            let marker = format!("Second paragraph with an explicit test marker {n}-{j}\n");
            assert_eq!(
                text.matches(&marker).count(),
                1,
                "marker must survive exactly once"
            );
        }
    }
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.0.join("loss.json")).unwrap()).unwrap();
    assert_eq!(ledger["events"], json!([]));
    run(
        &["convert", "probe.hwp", "-o", "repeat.hwp", "--strict"],
        &dir.0,
    );
    assert_eq!(
        std::fs::read(dir.0.join("probe.hwp")).unwrap(),
        std::fs::read(dir.0.join("repeat.hwp")).unwrap(),
        "untouched HWP5 conversion must preserve bytes"
    );
    run(
        &[
            "edit",
            "probe.hwp",
            "-o",
            "noop.hwp",
            "--table-placement",
            "floating",
            "--verify",
        ],
        &dir.0,
    );
    let noop = hwp5::read_document(&dir.0.join("noop.hwp"))
        .unwrap()
        .document;
    assert_anchors_match(&before, &noop);
}

#[test]
fn synthetic_form_breaks_restart_page_relative_cache() {
    probe(12, false, false);
}

#[test]
fn short_tables_and_following_body_keep_anchors() {
    probe(1, false, true);
}

#[test]
fn long_tables_and_merged_rows_keep_continuation_anchors() {
    probe(30, true, true);
}

#[test]
fn native_edit_keeps_base_anchor_beside_cached_floating_neighbours() {
    // Owner-authored genuine-style caches, not a genuine Hangul fixture or open receipt.
    let dir =
        ProbeDir(std::env::temp_dir().join(format!("hwp410-native-edit-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let mut doc = hwp_convert::from_markdown("Anchor\n\nEdited paragraph\n\nNeighbour\n");
    assert_eq!(doc.sections[0].paragraphs.len(), 3);
    for shape in &mut doc.header.char_shapes {
        shape.base_size = 1000;
    }
    for shape in &mut doc.header.para_shapes {
        shape.spacing_top = 0;
        shape.spacing_bottom = 0;
        shape.line_spacing_type = 0;
        shape.line_spacing = 160;
        shape.line_spacing_old = 160;
    }
    for control in &mut doc.sections[0].paragraphs[0].controls {
        if let hwp_model::Control::SectionDef(def) = control {
            let page = def.page.as_mut().unwrap();
            page.margin_header = hwp_model::HwpUnit(0);
            page.margin_footer = hwp_model::HwpUnit(0);
        }
    }
    let page = doc.sections[0].section_def().unwrap().page.unwrap();
    let body_width = page.width.0 - page.margin_left.0 - page.margin_right.0;
    let body_height = page.height.0 - page.margin_top.0 - page.margin_bottom.0;
    let line = |v| hwp_model::LineSeg {
        text_start: 0,
        v_pos: v,
        line_height: 1000,
        text_height: 1000,
        baseline_gap: 850,
        line_spacing: 600,
        col_start: 0,
        seg_width: body_width,
        flags: 0x0006_0000,
    };
    let mut cell_para = doc.sections[0].paragraphs[0].clone();
    cell_para.controls.clear();
    cell_para.line_segs = vec![line(0)];
    let tables = hwp_convert::from_markdown("| Cell |\n|---|\n| Value |\n");
    let mut object = tables.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| {
            if let hwp_model::Control::Table(t) = c {
                Some(t.clone())
            } else {
                None
            }
        })
        .unwrap();
    object.rows = 1;
    object.cols = 1;
    object.row_cell_counts = vec![1];
    object.cells.truncate(1);
    object.cells[0].width = hwp_model::HwpUnit(body_width);
    object.cells[0].height = hwp_model::HwpUnit(body_height - 1200);
    object.cells[0].margins = [100; 4];
    object.cells[0].paragraphs = vec![cell_para];
    object.inner_margins = [100; 4];
    object.common_data.clear();
    object.placement = Some(hwp_model::GsoPlacement {
        treat_as_char: false,
        vert_rel_to: 2,
        horz_rel_to: 3,
        width: body_width,
        height: body_height - 1200,
        ..Default::default()
    });
    // Base synthesis advances by 100+100+1000+566, independently of the saved row minimum.
    let base_anchor = 1766;
    for (index, paragraph) in doc.sections[0].paragraphs.iter_mut().enumerate() {
        paragraph.header.instance_id = index as u32 + 1;
        paragraph.line_segs = vec![line(match index {
            0 => 0,
            1 => base_anchor,
            _ => base_anchor + 1600,
        })];
    }
    doc.sections[0].paragraphs[0]
        .controls
        .push(hwp_model::Control::Table(object));
    let input = dir.0.join("source.hwp");
    hwp5::write_document(
        &doc,
        &input,
        &hwp5::WriteOptions {
            preserve_linesegs: true,
            ..Default::default()
        },
    )
    .unwrap();
    let original = hwp5::read_document(&input).unwrap().document;
    assert_eq!(original.meta.source_format, "hwp5");
    assert!(
        original.sections[0].paragraphs[0]
            .controls
            .iter()
            .any(|c| matches!(c,
        hwp_model::Control::Table(t) if t.placement.as_ref().is_some_and(|p| !p.treat_as_char)))
    );
    assert_eq!(
        original.sections[0].paragraphs[1].line_segs[0].v_pos,
        base_anchor
    );
    std::fs::write(
        dir.0.join("ops.json"),
        r#"[{"op":"set_para","address":{"at":{"section":0,"paragraph":1}},"align":"center"}]"#,
    )
    .unwrap();
    run(
        &[
            "edit",
            "source.hwp",
            "-o",
            "edited.hwp",
            "--ops",
            "ops.json",
        ],
        &dir.0,
    );
    let edited = hwp5::read_document(&dir.0.join("edited.hwp"))
        .unwrap()
        .document;
    let changed = &edited.sections[0].paragraphs[1];
    assert_ne!(
        changed.para_shape, original.sections[0].paragraphs[1].para_shape,
        "exercise an actual native paragraph edit"
    );
    assert_eq!(
        changed.line_segs[0].v_pos, base_anchor,
        "native edits must keep the base synthesized anchor, not a cacheless floating prepass anchor"
    );
    assert_eq!(
        changed.line_segs[0].flags & 1,
        0,
        "native edits must not acquire the new planner page-start bit"
    );
    for index in [0, 2] {
        assert_eq!(
            edited.sections[0].paragraphs[index].line_segs,
            original.sections[0].paragraphs[index].line_segs,
            "unchanged native neighbour caches remain authoritative"
        );
    }
}
