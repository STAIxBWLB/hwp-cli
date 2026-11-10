//! #462: an edited HWP5 table keeps the body flow of the paragraphs before it and continues the
//! re-synthesized suffix from that prefix, so the change stays on its page and the content after it
//! keeps clear of the table.
//!
//! What is asserted here is font-free: which caches came from the saved snapshot and which from the
//! synthesizer, and the order of `v_pos` inside one section's body list. Where the lines actually
//! land depends on glyph widths, so the render leg (one page, no overlapping body rows) runs only
//! under `HWP_GOLDEN=1`, per REVIEW.md.

use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::{Document, Paragraph};

/// Body paragraph that owns the table whose cell the tests edit.
const EDITED_PARAGRAPH: usize = 4;

/// Committed public parity source (AGENTS.md's narrow exception), so no fixture-skip accounting.
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pdf-parity/public/source/public-safety-rfp-p1.hwp")
}

fn work_dir() -> PathBuf {
    // Include the PID: concurrent cargo test sessions otherwise share a fixed path.
    let dir = std::env::temp_dir().join(format!("hwp-cli-462-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn input() -> Document {
    let path = fixture();
    assert!(
        path.exists(),
        "committed fixture missing: {} (see fixtures/README.md)",
        path.display()
    );
    hwp5::read_document(&path).unwrap().document
}

fn edit(cell_text: &str, name: &str) -> (PathBuf, Document) {
    let output = work_dir().join(name);
    let result = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("edit")
        .arg(fixture())
        .arg("-o")
        .arg(&output)
        .arg("--set-cell")
        .arg(format!("1:2:2={cell_text}"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "hwp edit failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let document = hwp5::read_document(&output).unwrap().document;
    (output, document)
}

fn body(document: &Document) -> &[Paragraph] {
    &document.sections[0].paragraphs
}

/// The saved flow never steps back inside one section: a paragraph starts at or below the bottom of
/// the last line block before it. A page the paragraph itself starts (its page-break bit, or a saved
/// page-first-line flag) is the one licensed reset.
fn assert_flow_forward(label: &str, paragraphs: &[Paragraph]) {
    for (index, pair) in paragraphs.windows(2).enumerate() {
        let (previous, next) = (&pair[0], &pair[1]);
        let (Some(previous_last), Some(next_first)) =
            (previous.line_segs.last(), next.line_segs.first())
        else {
            continue;
        };
        let previous_bottom = previous_last
            .v_pos
            .saturating_add(previous_last.line_height);
        let starts_page = next.header.break_type & 0x04 != 0
            || next_first.flags & 0x1 != 0
            || next_first.v_pos == 0;
        assert!(
            next_first.v_pos >= previous_bottom || starts_page,
            "{label}: body paragraph {} starts at v_pos {} above the bottom {} of paragraph {} \
             without starting a page — the renderer reads that step back as a new page (#462)",
            index + 1,
            next_first.v_pos,
            previous_bottom,
            index
        );
    }
}

/// The paragraphs before the edit are the saved snapshot, cache included.
fn assert_prefix_restored(label: &str, original: &Document, edited: &Document) {
    for index in 0..EDITED_PARAGRAPH {
        assert_eq!(
            body(edited)[index],
            body(original)[index],
            "{label}: body paragraph {index} precedes the edit, so its saved record (line cache \
             included) has to come back unchanged"
        );
    }
}

/// Every paragraph after the edit is the renderer's, not the snapshot's stale `v_pos`.
fn assert_suffix_resynthesized(label: &str, original: &Document, edited: &Document) {
    for index in EDITED_PARAGRAPH..body(edited).len() {
        assert_ne!(
            body(edited)[index].line_segs,
            body(original)[index].line_segs,
            "{label}: body paragraph {index} moved with the edit, so restoring its saved cache \
             keeps a v_pos that no longer matches the flowed table (#462)"
        );
    }
}

#[test]
fn growing_table_edit_continues_the_kept_prefix() {
    let original = input();
    let long = "대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다. \
                대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다. \
                대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다. \
                대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다.";
    let (_, edited) = edit(long, "grow.hwp");

    assert_prefix_restored("grow", &original, &edited);
    assert_flow_forward("grow", body(&edited));
    assert_suffix_resynthesized("grow", &original, &edited);

    // The seam itself: the edited paragraph follows the kept heading instead of restarting the
    // section at the top, which is what put the table above the heading and onto a second page.
    let heading_bottom = body(&edited)[EDITED_PARAGRAPH - 1]
        .line_segs
        .last()
        .map(|seg| seg.v_pos + seg.line_height)
        .expect("heading cache");
    let table_top = body(&edited)[EDITED_PARAGRAPH].line_segs[0].v_pos;
    assert!(
        table_top >= heading_bottom,
        "grow: the edited paragraph starts at v_pos {table_top}, above the kept heading bottom \
         {heading_bottom} (#462)"
    );
}

#[test]
fn shrinking_table_edit_continues_the_kept_prefix() {
    let original = input();
    let (_, edited) = edit("통합", "shrink.hwp");

    assert_prefix_restored("shrink", &original, &edited);
    assert_flow_forward("shrink", body(&edited));
    assert_suffix_resynthesized("shrink", &original, &edited);
}

/// Render leg: the glyph-driven placement, allowed only under `HWP_GOLDEN=1` (REVIEW.md forbids
/// page-count and glyph assertions in CI).
fn render_layout(source: &Path, name: &str) -> serde_json::Value {
    let layout = work_dir().join(name);
    let result = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("render")
        .arg(source)
        .arg("-o")
        .arg(work_dir().join(format!("{name}.png")))
        .arg("--layout-json")
        .arg(&layout)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "hwp render failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&std::fs::read(&layout).unwrap()).unwrap()
}

fn id_depth(id: &str) -> usize {
    id.split('.').count()
}

#[test]
fn golden_edited_table_renders_on_one_page() {
    if std::env::var_os("HWP_GOLDEN").is_none() {
        eprintln!("SKIP: HWP_GOLDEN is unset; this leg asserts glyph-driven placement (#462)");
        return;
    }
    let long = "대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다. \
                대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다. \
                대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다. \
                대피 경로와 안전지점을 지도 한 장에 통합해 표시하고 상태 변화를 실시간으로 갱신한다.";
    for (label, text) in [("grow", long), ("shrink", "통합")] {
        let (path, _) = edit(text, &format!("golden-{label}.hwp"));
        let layout = render_layout(&path, &format!("golden-{label}-layout.json"));
        let pages = layout["pages"].as_array().expect("pages array");
        assert_eq!(
            pages.len(),
            1,
            "{label}: the edit has to stay on one page (#462)"
        );

        // Body rows only: a table's cell paragraphs live inside the table box and overlap it by
        // design, so the three-segment ids are the body-level rows.
        let body_rows = pages[0]["rows"]
            .as_array()
            .expect("rows array")
            .iter()
            .filter(|row| id_depth(row["id"].as_str().unwrap_or_default()) == 3)
            .collect::<Vec<_>>();
        assert!(
            body_rows.len() > EDITED_PARAGRAPH,
            "{label}: body rows missing"
        );
        for pair in body_rows.windows(2) {
            let previous_bottom = pair[0]["box"]["y1"].as_f64().expect("y1");
            let next_top = pair[1]["box"]["y0"].as_f64().expect("y0");
            assert!(
                next_top >= previous_bottom - 1.0,
                "{label}: body row {} starts at {next_top} inside the row above (bottom \
                 {previous_bottom}) — the edited table overlaps the content after it (#462)",
                pair[1]["id"].as_str().unwrap_or_default()
            );
        }
    }
}
