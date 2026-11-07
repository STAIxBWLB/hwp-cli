//! Owner-authored synthetic forms from #410; no private corpus or font golden.
use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::Document;
use hwp_render::segment_map::SegmentMap;
use serde_json::json;

struct ProbeDir(PathBuf);
impl Drop for ProbeDir {
    fn drop(&mut self) {
        if std::env::var_os("HWP_410_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn run(args: &[&str], dir: &Path) -> Vec<u8> {
    let output = Command::new(env!("CARGO_BIN_EXE_hwp"))
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

fn layout(doc: &Document) -> SegmentMap {
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
    map
}

fn assert_anchors_match(before: &Document, after: &Document) {
    let before_map = layout(before);
    let after_map = layout(after);
    for (pi, paragraph) in before.sections[0].paragraphs.iter().enumerate() {
        let address = hwp_render::segment_id::SegmentPath {
            section: 0,
            indices: vec![pi],
        };
        let id = hwp_render::segment_id::paragraph_id(&address, paragraph);
        let source_pages: Vec<_> = before_map
            .rows
            .iter()
            .filter(|row| row.id == id)
            .map(|row| row.page)
            .collect();
        let output_pages: Vec<_> = after_map
            .rows
            .iter()
            .filter(|row| row.id == id)
            .map(|row| row.page)
            .collect();
        assert_eq!(
            source_pages, output_pages,
            "body paragraph {pi} must retain its page anchor"
        );
    }
    let tables = |map: SegmentMap| {
        map.rows
            .into_iter()
            .filter(|row| row.kind == "table")
            .map(|row| (row.page, row.bbox))
            .collect::<Vec<_>>()
    };
    let source = tables(before_map);
    let output = tables(after_map);
    assert_eq!(source.len(), output.len(), "table fragments must agree");
    for ((page, bbox), (output_page, output_bbox)) in source.iter().zip(&output) {
        assert_eq!(page, output_page, "table fragment page anchors must agree");
        let (a, b) = (bbox.unwrap(), output_bbox.unwrap());
        for (x, y) in [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)] {
            assert!(
                (x - y).abs() < 0.05,
                "table anchor geometry drift: {a:?} != {b:?}"
            );
        }
    }
    let mut breaks = 0;
    for (source, output) in before.sections[0]
        .paragraphs
        .iter()
        .zip(&after.sections[0].paragraphs)
    {
        assert_eq!(source.header.break_type & 4, output.header.break_type & 4);
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
