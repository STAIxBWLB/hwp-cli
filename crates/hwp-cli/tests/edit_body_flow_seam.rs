//! #462: an edited HWP5 table keeps the body flow of the paragraphs before it and continues the
//! re-synthesized suffix from that prefix, so the change stays on its page and the content after it
//! keeps clear of the table. #509 adds the cases the first round missed: insertions and deletions,
//! retained nested caches taller than their synthesis, and an out-of-page saved cursor.
//!
//! What is asserted here is font-free: which caches came from the saved snapshot and which from the
//! synthesizer, and the order of `v_pos` inside one section's body list. Where the lines actually
//! land depends on glyph widths, so the render leg (one page, no overlapping body rows) runs only
//! under `HWP_GOLDEN=1`, per REVIEW.md.

use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::{Control, Document, Paragraph};

/// Body paragraph that owns the table the cell edits target.
const TABLE_PARAGRAPH: usize = 4;

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

fn edit_path(source: &Path, name: &str, arguments: &[&str]) -> (PathBuf, Document) {
    let output = work_dir().join(name);
    let result = Command::new(env!("CARGO_BIN_EXE_ahwp"))
        .arg("edit")
        .arg(source)
        .arg("-o")
        .arg(&output)
        .args(arguments)
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

fn edit(name: &str, arguments: &[&str]) -> (PathBuf, Document) {
    edit_path(&fixture(), name, arguments)
}

fn body(document: &Document) -> &[Paragraph] {
    &document.sections[0].paragraphs
}

fn table(paragraph: &Paragraph, index: usize) -> &hwp_model::Table {
    paragraph
        .controls
        .iter()
        .filter_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .nth(index)
        .expect("table control")
}

fn table_mut(paragraph: &mut Paragraph, index: usize) -> &mut hwp_model::Table {
    paragraph
        .controls
        .iter_mut()
        .filter_map(|control| match control {
            Control::Table(table) => Some(table),
            _ => None,
        })
        .nth(index)
        .expect("table control")
}

fn space_bottom(document: &Document, paragraph: &Paragraph) -> i32 {
    document
        .header
        .para_shapes
        .get(paragraph.para_shape.0 as usize)
        .map_or(0, |shape| shape.spacing_bottom / 2)
}

fn space_top(document: &Document, paragraph: &Paragraph) -> i32 {
    document
        .header
        .para_shapes
        .get(paragraph.para_shape.0 as usize)
        .map_or(0, |shape| shape.spacing_top / 2)
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
fn assert_prefix_restored(label: &str, original: &Document, edited: &Document, count: usize) {
    for index in 0..count {
        assert_eq!(
            body(edited)[index],
            body(original)[index],
            "{label}: body paragraph {index} precedes the edit, so its saved record (line cache \
             included) has to come back unchanged"
        );
    }
}

/// Every body paragraph from `first` on carries the renderer's cache, not the snapshot's.
fn assert_resynthesized(label: &str, original: &Document, edited: &Document, first: usize) {
    let shifted = body(edited).len() as i64 - body(original).len() as i64;
    for index in first..body(edited).len() {
        let original_index = index as i64 - shifted;
        if original_index < 0 || original_index as usize >= body(original).len() {
            continue;
        }
        assert_ne!(
            body(edited)[index].line_segs,
            body(original)[original_index as usize].line_segs,
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
    let (_, edited) = edit("grow.hwp", &["--set-cell", &format!("1:2:2={long}")]);

    assert_prefix_restored("grow", &original, &edited, TABLE_PARAGRAPH);
    assert_flow_forward("grow", body(&edited));
    assert_resynthesized("grow", &original, &edited, TABLE_PARAGRAPH);

    // The seam is the caller's measurement, not a section-top restart: the edited paragraph starts
    // at the saved prefix's own cursor plus its space-before. 0 is the one other legal value,
    // because whether the grown table still fits the page remainder is glyph-driven (#509).
    let heading = &body(&original)[TABLE_PARAGRAPH - 1];
    let prefix_cursor =
        hwp_render::lineseg::saved_body_cursor(heading, space_bottom(&original, heading))
            .expect("heading cache");
    let expected = prefix_cursor + space_top(&edited, &body(&edited)[TABLE_PARAGRAPH]);
    let table_top = body(&edited)[TABLE_PARAGRAPH].line_segs[0].v_pos;
    assert!(
        table_top == expected || table_top == 0,
        "grow: the edited paragraph starts at v_pos {table_top}, but the kept prefix ends at \
         {expected} — a section-top restart lands above it (#462)"
    );
}

#[test]
fn shrinking_table_edit_continues_the_kept_prefix() {
    let original = input();
    let (_, edited) = edit("shrink.hwp", &["--set-cell", "1:2:2=통합"]);

    assert_prefix_restored("shrink", &original, &edited, TABLE_PARAGRAPH);
    assert_flow_forward("shrink", body(&edited));
    assert_resynthesized("shrink", &original, &edited, TABLE_PARAGRAPH);
}

/// #509 finding 1: a body insertion must not cost the untouched prefix its saved records.
#[test]
fn inserted_body_paragraph_keeps_the_untouched_prefix() {
    let original = input();
    let (_, edited) = edit("insert.hwp", &["--insert-para", "1. 추진 배경=>삽입 문단"]);

    assert_eq!(
        body(&edited).len(),
        body(&original).len() + 1,
        "the insert adds exactly one body paragraph"
    );
    assert_prefix_restored("insert", &original, &edited, TABLE_PARAGRAPH);
    assert_flow_forward("insert", body(&edited));
    assert!(
        !body(&edited)[TABLE_PARAGRAPH].line_segs.is_empty(),
        "the inserted paragraph carries a synthesized cache"
    );
    assert_resynthesized("insert", &original, &edited, TABLE_PARAGRAPH + 1);

    // Unchanged paragraphs inside the table are restored even though the table's owner paragraph
    // sits after the edit: an insertion in the body does not invalidate cell-local caches.
    let edited_cells = &table(&body(&edited)[TABLE_PARAGRAPH + 1], 0).cells;
    let original_cells = &table(&body(&original)[TABLE_PARAGRAPH], 0).cells;
    assert_eq!(edited_cells.len(), original_cells.len());
    for (index, (edited_cell, original_cell)) in edited_cells.iter().zip(original_cells).enumerate()
    {
        assert_eq!(
            edited_cell.paragraphs, original_cell.paragraphs,
            "insert: cell {index} was not edited, so its saved paragraphs stay"
        );
    }
}

/// #509 finding 1: a body deletion must not cost the untouched prefix its saved records.
#[test]
fn deleted_body_paragraph_keeps_the_untouched_prefix() {
    let original = input();
    let (_, edited) = edit("delete.hwp", &["--delete-para", "2. 요구사항"]);

    assert_eq!(
        body(&edited).len(),
        body(&original).len() - 1,
        "the delete removes exactly one body paragraph"
    );
    assert_prefix_restored("delete", &original, &edited, TABLE_PARAGRAPH + 1);
    assert_flow_forward("delete", body(&edited));
}

/// #509 finding 1: the same for a list nested in a cell, where the untouched sibling keeps its cache
/// and the inserted paragraph has to start below it.
#[test]
fn inserted_cell_paragraph_keeps_its_sibling_cache() {
    let original = input();
    let (_, edited) = edit(
        "insert-cell.hwp",
        &["--insert-para", "대피 경로와 안전지점 통합 표시=>셀에 삽입"],
    );

    let before = &table(&body(&original)[TABLE_PARAGRAPH], 0).cells[8].paragraphs;
    let after = &table(&body(&edited)[TABLE_PARAGRAPH], 0).cells[8].paragraphs;
    assert_eq!(after.len(), before.len() + 1, "the cell gained a paragraph");
    assert_eq!(
        after[0].line_segs, before[0].line_segs,
        "the cell paragraph the edit did not touch keeps its saved cache"
    );
    let sibling_bottom = after[0].line_segs[0]
        .v_pos
        .saturating_add(after[0].line_segs[0].line_height);
    assert!(
        after[1].line_segs[0].v_pos >= sibling_bottom,
        "the inserted cell paragraph starts at {} inside its kept sibling (bottom {})",
        after[1].line_segs[0].v_pos,
        sibling_bottom
    );
    assert_prefix_restored("insert-cell", &original, &edited, TABLE_PARAGRAPH);
}

/// #509 finding 2: a kept cell whose saved cache is taller than its synthesis must decide where the
/// body after the table starts, not the ephemeral synthesis.
#[test]
fn retained_taller_table_puts_the_following_body_below_it() {
    let mut tall = input();
    let cells = &mut table_mut(&mut tall.sections[0].paragraphs[TABLE_PARAGRAPH], 0).cells;
    let segment = &mut cells[0].paragraphs[0].line_segs;
    assert_eq!(segment.len(), 1, "the first cell holds one saved line");
    segment[0].line_height = 12000;
    segment[0].text_height = 12000;
    let source = work_dir().join("tall.hwp");
    hwp5::write_document(
        &tall,
        &source,
        &hwp5::WriteOptions {
            preserve_linesegs: true,
            ..Default::default()
        },
    )
    .unwrap();

    // A different cell of the same table: the tall first cell is retained, not re-synthesized.
    let (_, edited) = edit_path(&source, "tall-edited.hwp", &["--set-cell", "1:2:2=통합"]);
    let retained =
        table(&body(&edited)[TABLE_PARAGRAPH], 0).cells[0].paragraphs[0].line_segs[0].line_height;
    assert_eq!(
        retained, 12000,
        "the untouched cell keeps the saved cache the input carried"
    );

    assert_body_after_the_table("tall", &edited);
}

/// The paragraph after the table starts at the flow the caches the output keeps actually produce:
/// the table paragraph's own entry, the table height measured from its output caches, and the
/// following paragraph's space-before. 0 is the one other legal value, a page start.
fn assert_body_after_the_table(label: &str, edited: &Document) {
    let table_paragraph = &body(edited)[TABLE_PARAGRAPH];
    let table_bottom = hwp_render::lineseg::saved_body_cursor(
        table_paragraph,
        space_bottom(edited, table_paragraph),
    )
    .expect("table anchor cache");
    let following = &body(edited)[TABLE_PARAGRAPH + 1];
    let expected = table_bottom + space_top(edited, following);
    let following_top = following.line_segs[0].v_pos;
    assert!(
        following_top == expected || following_top == 0,
        "{label}: the paragraph after the table starts at {following_top}, but the table's own \
         caches end at {table_bottom} — the body was placed from a measurement the output does not \
         keep (#509)"
    );
}

/// The fixture's cell 8 with a second paragraph, so a change to the first can strand a retained
/// sibling: the second paragraph's saved `v_pos` was measured against the block the first one had.
fn two_paragraph_cell_source(name: &str) -> (PathBuf, Document) {
    let (path, document) = edit(
        name,
        &["--insert-para", "대피 경로와 안전지점 통합 표시=>둘째 문단"],
    );
    let paragraphs = &table(&body(&document)[TABLE_PARAGRAPH], 0).cells[8].paragraphs;
    assert_eq!(
        paragraphs.len(),
        2,
        "fixture: the cell gained one paragraph"
    );
    let first = &paragraphs[0].line_segs[0];
    assert!(
        paragraphs[1].line_segs[0].v_pos >= first.v_pos.saturating_add(first.line_height),
        "fixture: the two-paragraph source is non-overlapping"
    );
    (path, document)
}

/// Consecutive paragraphs inside one kept list never overlap, and the paragraph named by
/// `sibling_index` carries a re-synthesized cache rather than the stale saved one it arrived with.
fn assert_sibling_resynthesized(
    label: &str,
    paragraphs: &[Paragraph],
    saved_sibling: &[hwp_model::LineSeg],
    sibling_index: usize,
) {
    for pair in paragraphs.windows(2) {
        let bottom = pair[0]
            .line_segs
            .last()
            .map_or(0, |seg| seg.v_pos.saturating_add(seg.line_height));
        assert!(
            pair[1].line_segs[0].v_pos >= bottom,
            "{label}: a cell paragraph starts at {} inside the paragraph above (bottom {bottom})",
            pair[1].line_segs[0].v_pos
        );
    }
    assert_ne!(
        paragraphs[sibling_index].line_segs, saved_sibling,
        "{label}: the cell paragraph below the edit keeps a saved v_pos measured against the block \
         the edit changed (#509)"
    );
}

/// #509 finding: a kept cache stops being valid once an earlier paragraph in the same list moves.
/// The saved sibling sat below an unchanged first paragraph; the edit grows that paragraph to
/// several lines, so both the sibling and the body after the table have to follow the new block.
#[test]
fn cell_growth_resynthesizes_the_sibling_below_it() {
    let (source, original) = two_paragraph_cell_source("sibling-grow-source.hwp");
    let saved = table(&body(&original)[TABLE_PARAGRAPH], 0).cells[8].paragraphs[1]
        .line_segs
        .clone();
    let (_, edited) = edit_path(
        &source,
        "sibling-grow.hwp",
        &[
            "--replace",
            "대피 경로와 안전지점 통합 표시=>첫 줄\n둘째 줄\n셋째 줄",
        ],
    );

    let paragraphs = &table(&body(&edited)[TABLE_PARAGRAPH], 0).cells[8].paragraphs;
    assert_eq!(paragraphs.len(), 2);
    assert!(
        paragraphs[0].line_segs.len() >= 3,
        "fixture: the edited paragraph grew to several lines, got {}",
        paragraphs[0].line_segs.len()
    );
    assert_sibling_resynthesized("sibling-grow", paragraphs, &saved, 1);
    assert_body_after_the_table("sibling-grow", &edited);
}

/// The same staleness reached by inserting a paragraph before the retained sibling: the insertion
/// is cacheless, so everything from it on is re-synthesized, the untouched sibling included.
#[test]
fn cell_insertion_before_a_retained_sibling_takes_the_seam() {
    let (source, original) = two_paragraph_cell_source("sibling-insert-source.hwp");
    let saved = table(&body(&original)[TABLE_PARAGRAPH], 0).cells[8].paragraphs[1]
        .line_segs
        .clone();
    let (_, edited) = edit_path(
        &source,
        "sibling-insert.hwp",
        &["--insert-para", "대피 경로와 안전지점 통합 표시=>삽입 문단"],
    );

    let paragraphs = &table(&body(&edited)[TABLE_PARAGRAPH], 0).cells[8].paragraphs;
    assert_eq!(
        paragraphs.len(),
        3,
        "the cell keeps both saved paragraphs and gains the insertion"
    );
    assert_sibling_resynthesized("sibling-insert", paragraphs, &saved, 2);
    assert_body_after_the_table("sibling-insert", &edited);
}

/// And by deleting the paragraph above it: the sibling becomes the list's first paragraph, so its
/// saved `v_pos` (measured below the deleted one) has to be replaced too.
#[test]
fn cell_deletion_before_a_retained_sibling_takes_the_seam() {
    let (source, original) = two_paragraph_cell_source("sibling-delete-source.hwp");
    let saved = table(&body(&original)[TABLE_PARAGRAPH], 0).cells[8].paragraphs[1]
        .line_segs
        .clone();
    let (_, edited) = edit_path(
        &source,
        "sibling-delete.hwp",
        &["--delete-para", "대피 경로와 안전지점 통합 표시"],
    );

    let paragraphs = &table(&body(&edited)[TABLE_PARAGRAPH], 0).cells[8].paragraphs;
    assert_eq!(
        paragraphs.len(),
        1,
        "the list keeps its remaining paragraph"
    );
    assert_ne!(
        paragraphs[0].line_segs, saved,
        "sibling-delete: the paragraph left in the cell keeps a saved v_pos measured below the \
         paragraph the edit removed (#509)"
    );
    assert_body_after_the_table("sibling-delete", &edited);
}

/// #509 finding 3: a saved cursor outside the page comes from untrusted bytes; it must degrade to a
/// page start instead of panicking (debug) or wrapping (release) inside the walk.
#[test]
fn out_of_page_saved_cursor_does_not_panic() {
    let mut broken = input();
    let heading = &mut broken.sections[0].paragraphs[TABLE_PARAGRAPH - 1];
    heading.line_segs[0].v_pos = i32::MAX - 2;
    let shape = body(&broken)[TABLE_PARAGRAPH].para_shape.0 as usize;
    broken.header.para_shapes[shape].spacing_top = 3;
    let source = work_dir().join("broken-cursor.hwp");
    hwp5::write_document(
        &broken,
        &source,
        &hwp5::WriteOptions {
            preserve_linesegs: true,
            ..Default::default()
        },
    )
    .unwrap();

    // The assertion is the successful exit: the edit used to abort on an arithmetic overflow.
    let (_, edited) = edit_path(
        &source,
        "broken-cursor-edited.hwp",
        &["--set-cell", "1:2:2=통합"],
    );
    assert_eq!(
        body(&edited).len(),
        body(&broken).len(),
        "the edit still publishes a structurally complete document"
    );
}

/// Render leg: the glyph-driven placement, allowed only under `HWP_GOLDEN=1` (REVIEW.md forbids
/// page-count and glyph assertions in CI).
fn render_layout(source: &Path, name: &str) -> serde_json::Value {
    let layout = work_dir().join(name);
    let result = Command::new(env!("CARGO_BIN_EXE_ahwp"))
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
        let (path, _) = edit(
            &format!("golden-{label}.hwp"),
            &["--set-cell", &format!("1:2:2={text}")],
        );
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
            body_rows.len() > TABLE_PARAGRAPH,
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

fn write_cached_geometry_source(document: &mut Document, name: &str) -> (PathBuf, Document) {
    // Recompute from parent geometry before writing, so source caches are valid rather than
    // arbitrary sentinels. Width assertions below depend only on the cell box and its margins.
    let mut fonts = hwp_render::FontStore::new_isolated();
    let mut issues = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(document, &mut fonts, &mut issues);
    let source = work_dir().join(name);
    hwp5::write_document(
        document,
        &source,
        &hwp5::WriteOptions {
            preserve_linesegs: true,
            ..Default::default()
        },
    )
    .unwrap();
    let read = hwp5::read_document(&source).unwrap().document;
    (source, read)
}

fn cell_at(table: &hwp_model::Table, row: u16, col: u16) -> &hwp_model::Cell {
    table
        .cells
        .iter()
        .find(|cell| cell.row == row && cell.col == col)
        .expect("target cell exists")
}

fn cell_content_width(cell: &hwp_model::Cell) -> i32 {
    (cell.width.0 - i32::from(cell.margins[0]) - i32::from(cell.margins[1])).max(1)
}

fn assert_same_paragraph_refs(before: &Paragraph, after: &Paragraph) {
    assert_eq!(
        after.chars, before.chars,
        "geometry edit leaves text unchanged"
    );
    assert_eq!(
        after.style, before.style,
        "geometry edit leaves style IDs unchanged"
    );
    assert_eq!(
        after.para_shape, before.para_shape,
        "geometry edit leaves paragraph shape IDs unchanged"
    );
    assert_eq!(
        after.char_shape_runs, before.char_shape_runs,
        "geometry edit leaves character shape references unchanged"
    );
}

#[test]
fn split_cell_recomputes_the_retained_anchor_cache_for_its_narrower_box() {
    let mut document = input();
    let original_table = table(&body(&document)[TABLE_PARAGRAPH], 0);
    let (row, col) = original_table
        .cells
        .iter()
        .filter(|cell| cell.row_span == 1 && cell.col_span == 1)
        .find_map(|cell| {
            original_table
                .cells
                .iter()
                .any(|next| {
                    next.row == cell.row
                        && next.col == cell.col + 1
                        && next.row_span == 1
                        && next.col_span == 1
                })
                .then_some((cell.row, cell.col))
        })
        .expect("fixture has two adjacent unmerged cells");
    // The body owner at index 4 is document-order table 1; table 0 is the earlier title frame.
    hwp_convert::merge_cells(&mut document, 1, row, col, row, col + 1).unwrap();
    let (source, before) = write_cached_geometry_source(&mut document, "split-width-source.hwp");
    let before_cell = cell_at(table(&body(&before)[TABLE_PARAGRAPH], 0), row, col);
    assert_eq!(
        before_cell.col_span, 2,
        "source must really contain the merged anchor"
    );
    assert!(
        !before_cell.paragraphs[0].line_segs.is_empty(),
        "source anchor has a saved cache"
    );
    let old_width = before_cell.paragraphs[0].line_segs[0].seg_width;
    assert_eq!(
        old_width,
        cell_content_width(before_cell),
        "source cache must match the merged box"
    );

    let (_, edited) = edit_path(
        &source,
        "split-width-output.hwp",
        &["--split-cell", &format!("1:{row}:{col}")],
    );
    let after_cell = cell_at(table(&body(&edited)[TABLE_PARAGRAPH], 0), row, col);
    assert_eq!(
        after_cell.col_span, 1,
        "CLI must actually split the merged anchor"
    );
    assert!(
        after_cell.width.0 < before_cell.width.0,
        "the retained anchor must become narrower"
    );
    assert_same_paragraph_refs(&before_cell.paragraphs[0], &after_cell.paragraphs[0]);
    let expected_width = cell_content_width(after_cell);
    assert_ne!(
        expected_width, old_width,
        "fixture must require a new cache width"
    );
    assert_eq!(
        after_cell.paragraphs[0].line_segs[0].seg_width, expected_width,
        "split-cell must not restore the anchor's cache measured for the old merged width"
    );
}

#[test]
fn width_only_style_tables_recomputes_caches_without_changing_paragraph_refs() {
    let mut document = input();
    let (eligible, _) = hwp_convert::style_tables(&mut document);
    assert!(eligible > 0, "fixture has a styleable table");
    assert_eq!(
        hwp_convert::style_tables(&mut document).1,
        0,
        "settle styles before modifying only geometry"
    );
    let target = table_mut(&mut document.sections[0].paragraphs[TABLE_PARAGRAPH], 0);
    let mut column_widths: Vec<i32> = (0..target.cols)
        .map(|col| {
            target
                .cells
                .iter()
                .find(|cell| cell.col == col && cell.col_span == 1)
                .expect("fixture exposes each grid column")
                .width
                .0
        })
        .collect();
    assert!(column_widths.len() >= 2);
    let original_total: i32 = column_widths.iter().sum();
    let shift = (column_widths[1] / 4).max(1);
    assert!(
        column_widths[1] > shift,
        "redistribution keeps every column positive"
    );
    column_widths[0] += shift;
    column_widths[1] -= shift;
    assert_eq!(
        column_widths.iter().sum::<i32>(),
        original_total,
        "redistribution retains total table width"
    );
    for cell in &mut target.cells {
        cell.width.0 = column_widths
            .iter()
            .skip(cell.col as usize)
            .take(cell.col_span as usize)
            .sum();
    }
    let (row, col) = target
        .cells
        .iter()
        .find(|cell| cell.col == 0 && cell.col_span == 1)
        .map(|cell| (cell.row, cell.col))
        .expect("a single-column anchor observes the changed width");
    let (source, before) = write_cached_geometry_source(&mut document, "restyle-width-source.hwp");
    let before_table = table(&body(&before)[TABLE_PARAGRAPH], 0);
    let before_cell = cell_at(before_table, row, col);
    let old_width = before_cell.paragraphs[0].line_segs[0].seg_width;
    assert_eq!(
        old_width,
        cell_content_width(before_cell),
        "source cache matches the redistributed column width"
    );

    let (_, edited) = edit_path(
        &source,
        "restyle-width-output.hwp",
        &["--style-tables", "official"],
    );
    let after_table = table(&body(&edited)[TABLE_PARAGRAPH], 0);
    assert_eq!(after_table.cells.len(), before_table.cells.len());
    for (before_cell, after_cell) in before_table.cells.iter().zip(&after_table.cells) {
        assert_eq!(
            (
                after_cell.row,
                after_cell.col,
                after_cell.row_span,
                after_cell.col_span
            ),
            (
                before_cell.row,
                before_cell.col,
                before_cell.row_span,
                before_cell.col_span
            )
        );
        assert_eq!(after_cell.paragraphs.len(), before_cell.paragraphs.len());
        for (before_paragraph, after_paragraph) in
            before_cell.paragraphs.iter().zip(&after_cell.paragraphs)
        {
            assert_same_paragraph_refs(before_paragraph, after_paragraph);
        }
    }
    let after_cell = cell_at(after_table, row, col);
    assert!(
        after_cell.width.0 < before_cell.width.0,
        "CLI restyling must restore the narrower official column"
    );
    let expected_width = cell_content_width(after_cell);
    assert_ne!(
        expected_width, old_width,
        "width-only edit must invalidate this cache"
    );
    assert_eq!(
        after_cell.paragraphs[0].line_segs[0].seg_width, expected_width,
        "style-tables must not restore an unchanged paragraph's cache measured for its old parent width"
    );
}
