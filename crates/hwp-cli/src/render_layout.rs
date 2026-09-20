//! Rendering of the render layout artifact, `schemas/render-layout-v1.schema.json`.
//!
//! It lives in the library rather than beside `commands::render` for the reason 05-05's
//! envelope serializer had to move here: a serializer private to the binary can only ever be
//! tested on the cases some committed fixture happens to contain, and the cases that matter
//! here - a truncated row set, a point segment with no box, a non-finite coordinate - are
//! exactly the ones no fixture carries.
//!
//! Nothing here measures anything. It transcribes the [`SegmentMap`] that
//! [`hwp_render::layout::layout_document_with_segments`] already produced into the published
//! field names. Boxes are not recomputed, ids are not rederived, and neither the chosen output
//! format nor the dpi reaches a single emitted number - which is why the same input yields a
//! byte-identical file under `--format png`, `svg` and `pdf` and under any `--dpi`. That is a
//! property of the source of the numbers: `layout_document_with_segments` takes no dpi
//! argument at all, so the recording pass is format-free and dpi-free BY CONSTRUCTION rather
//! than by a convention someone maintains. If a future change threads a dpi into layout, that
//! property dies quietly and the byte-identity tests would still pass on the day it happened,
//! because both sides of each comparison would move together. Reviewing such a change against
//! this paragraph is the only thing that catches it.

use hwp_render::display::DisplayList;
use hwp_render::segment_map::{BoxPt, SegmentMap, SegmentRow};

/// The version of the layout contract, carried in every file.
pub const RENDER_LAYOUT_SCHEMA_VERSION: &str = "1.0";
/// The contract name, carried in every file.
pub const RENDER_LAYOUT_CONTRACT: &str = "hwp-render-layout-v1";

/// The complete layout artifact for one render invocation.
///
/// `selected` holds 1-based page numbers, the same space `--pages` and the render report use;
/// `map`'s rows carry 0-based indices into `list.pages`, and the conversion happens here so no
/// caller has to know that the two differ. A selected page out of range for `list` is skipped
/// rather than fabricated: it cannot happen through the CLI, where the selection is parsed
/// against the same layout's page count, and inventing a page with no dimensions would publish
/// a measurement nobody made.
pub fn layout_json(list: &DisplayList, map: &SegmentMap, selected: &[usize]) -> serde_json::Value {
    let pages: Vec<serde_json::Value> = selected
        .iter()
        .filter_map(|&page_no| {
            let page = list.pages.get(page_no.checked_sub(1)?)?;
            let rows: Vec<serde_json::Value> = map.page(page_no - 1).map(row_json).collect();
            Some(serde_json::json!({
                "page": page_no,
                "width_pt": page.width_pt,
                "height_pt": page.height_pt,
                "rows": rows,
            }))
        })
        .collect();
    serde_json::json!({
        "contract": RENDER_LAYOUT_CONTRACT,
        "schema_version": RENDER_LAYOUT_SCHEMA_VERSION,
        "truncated": map.truncated,
        "selected_pages": selected,
        "pages": pages,
    })
}

/// Renders one geometry row.
fn row_json(row: &SegmentRow) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "kind": row.kind,
        "box": row.bbox.and_then(box_json),
        "source_chars": row.chars.map(|c| serde_json::json!({
            "start": c.start,
            "end": c.end,
        })),
    })
}

/// A box, or `None` when any coordinate is not finite.
///
/// A non-finite coordinate is a layout bug, not a document property: every path into
/// [`BoxPt`] unions display-item extents that came from finite document units. If one ever
/// appears, `serde_json` would render it as `null` INSIDE the box object, producing a file that
/// silently violates this crate's own published schema. Dropping the whole box instead keeps
/// the file valid and keeps the failure visible as a missing rectangle. It is deliberately not
/// a fabricated zero-extent box: that is the one thing D-08a exists to prevent.
fn box_json(b: BoxPt) -> Option<serde_json::Value> {
    let finite = [b.x0, b.y0, b.x1, b.y1].iter().all(|v| v.is_finite());
    finite.then(|| {
        serde_json::json!({ "x0": b.x0, "y0": b.y0, "x1": b.x1, "y1": b.y1 })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_render::segment_map::{CharRange, kind};

    fn row(kind: &'static str, id: &str, bbox: Option<BoxPt>) -> SegmentRow {
        SegmentRow {
            page: 0,
            kind,
            id: id.to_string(),
            bbox,
            chars: None,
            item_count: 0,
        }
    }

    #[test]
    fn a_non_finite_coordinate_drops_the_box_rather_than_publishing_an_invalid_file() {
        let nan = BoxPt {
            x0: 0.0,
            y0: f32::NAN,
            x1: 10.0,
            y1: 10.0,
        };
        let v = row_json(&row(kind::PARA, "p0", Some(nan)));
        assert!(
            v["box"].is_null(),
            "a non-finite coordinate must drop the whole box, got {}",
            v["box"]
        );
        // And it must not have been replaced by a fabricated rectangle.
        assert_eq!(v["box"], serde_json::Value::Null);
    }

    #[test]
    fn a_point_segment_publishes_a_null_box_and_keeps_its_character_range() {
        let mut r = row(kind::BOOKMARK, "b0", None);
        r.chars = Some(CharRange { start: 3, end: 3 });
        let v = row_json(&r);
        assert_eq!(v["box"], serde_json::Value::Null);
        assert_eq!(v["source_chars"]["start"], 3);
        assert_eq!(v["source_chars"]["end"], 3);
    }

    #[test]
    fn a_table_row_publishes_a_null_character_range_for_a_different_reason() {
        let r = row(
            kind::TABLE,
            "t0",
            Some(BoxPt {
                x0: 1.0,
                y0: 2.0,
                x1: 3.0,
                y1: 4.0,
            }),
        );
        let v = row_json(&r);
        assert_eq!(v["source_chars"], serde_json::Value::Null);
        assert_eq!(v["box"]["x0"], 1.0);
    }

    /// A selected page number the display list does not have is skipped, not invented.
    #[test]
    fn an_out_of_range_selected_page_produces_no_page_object() {
        let list = DisplayList { pages: Vec::new() };
        let v = layout_json(&list, &SegmentMap::default(), &[1]);
        assert_eq!(v["pages"].as_array().expect("pages").len(), 0);
        assert_eq!(v["selected_pages"], serde_json::json!([1]));
    }
}
