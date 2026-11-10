//! IR <-> markdown/JSON conversion.

pub mod address;
pub mod base64;
pub mod bookmark;
pub mod csv;
pub mod document_compare;
pub mod document_merge;
pub mod document_split;
pub mod docx;
pub mod edit;
pub mod field;
pub mod form;
pub mod format;
pub mod frames;
pub mod from_html;
pub mod from_markdown;
pub mod gso;
pub mod html;
pub mod image;
pub mod lint;
pub mod markdown;
pub mod merge;
pub mod odt;
pub mod official;
pub mod segment;
pub mod segment_id;
pub mod structure;
pub mod style;
pub mod styles;
pub mod svg;

use hwp_model::Document;

pub use address::{Address, Granularity, ResolveError, ResolvedTarget, TargetKind, resolve};
pub use bookmark::{
    BookmarkInfo, bookmark_name, create_bookmark, list_bookmarks, make_bokm_ctrl_data,
};
pub use csv::to_csv;
pub use docx::to_docx;
pub use edit::{
    CloneTextMode, FormCellCandidate, ObjectKind, add_col, add_rows, add_rows_at, add_table,
    add_table_column, add_table_columns, apply_meta, clone_table, delete_object,
    delete_table_column, delete_table_row, find_form_cells_by_label, merge_cells,
    normalize_form_label, replace_text, replace_text_at, set_cell, set_table_placement, split_cell,
    table_dims, table_placement_stats,
};
pub use field::{
    FieldInfo, PlaceholderInfo, create_field, create_hyperlink, hyperlink_url, list_fields,
    replace_slots, scan_placeholders, set_field,
};
pub use form::{FormField, FormFieldSource, fill_form_fields, scan_form_fields};
pub use format::{
    CharFormat, KEEP_TOGETHER_REFUSAL, PageProps, ParaProps, apply_para_props_at, restyle_range_at,
    set_break_before_at, set_cell_para_props, set_char_format, set_indent_level_at, set_page_def,
    set_para_align, set_para_align_at, set_para_props, shift_head_level_at,
};
pub use frames::{
    FrameFields, compatibility_warnings, leading_frames, parse_field, parse_frame_fields,
};
pub use from_html::{HtmlImportOptions, from_html, from_html_with};
pub use from_markdown::{
    MarkdownImportOptions, TablePlacement, default_header, from_markdown, from_markdown_blocks,
    from_markdown_blocks_report, from_markdown_report, from_markdown_with,
};
pub use html::{to_html, to_html_fragment};
pub use image::{
    ImageSize, SealAnchorMetrics, image_kind, image_pixel_size, insert_image, insert_seal,
};
pub use markdown::{
    MarkdownOptions, MarkdownSegment, to_markdown, to_markdown_with, to_markdown_with_segments,
};
pub use odt::to_odt;
pub use official::{
    HeadingNumbering, OfficialBodyFont, OfficialPreset, OfficialProfile, PageMarginOverrides,
    profile, profiles,
};
pub use segment::{
    CharAttrs, ParaAttrs, Segment, SegmentKind, SegmentStyle, StyleLevel,
    to_markdown_with_segments_v2,
};
pub use segment_id::{
    SegmentPath, canonical_char_shape_runs, cell_id, control_id, paragraph_id, picture_id, run_id,
    table_id,
};
pub use structure::{
    delete_paragraph, delete_paragraph_at, insert_paragraph, insert_paragraph_at,
    insert_paragraph_checked, move_paragraph, text_in_unwritable_object,
};
pub use style::{display_width, style_table, style_tables};
pub use styles::{NamedStyle, StyleKind, StyleTableTooLarge, list_named_styles};

/// Serialize the whole IR as JSON (for structural inspection, debugging and machine consumers).
///
/// With `embed_bin`, attached binaries (`bin_streams[].data`, excluded from the default JSON) are
/// carried as base64 in the `data_b64` field, producing a **self-contained JSON** that
/// [`from_json`] reads back for a lossless round trip including images. Without it the image bytes
/// are omitted.
pub fn to_json(doc: &Document, pretty: bool, embed_bin: bool) -> serde_json::Result<String> {
    if !embed_bin {
        return if pretty {
            serde_json::to_string_pretty(doc)
        } else {
            serde_json::to_string(doc)
        };
    }

    let mut value = serde_json::to_value(doc)?;
    if let Some(arr) = value.get_mut("bin_streams").and_then(|v| v.as_array_mut()) {
        for (item, bin) in arr.iter_mut().zip(&doc.bin_streams) {
            if !bin.data.is_empty()
                && let Some(obj) = item.as_object_mut()
            {
                obj.insert(
                    "data_b64".to_string(),
                    serde_json::Value::String(base64::encode(&bin.data)),
                );
            }
        }
    }
    if pretty {
        serde_json::to_string_pretty(&value)
    } else {
        serde_json::to_string(&value)
    }
}

/// Deserialize a JSON IR into a document (the counterpart of [`to_json`]).
///
/// Decodes `bin_streams[].data_b64` (when present) to restore the attached binaries. `data` is
/// `#[serde(skip)]`, so without the base64 field the image bytes stay empty.
pub fn from_json(json: &str) -> Result<Document, String> {
    let mut value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;

    // Detach data_b64 before deserializing (Document has no such field).
    let mut decoded: Vec<(usize, Vec<u8>)> = Vec::new();
    if let Some(arr) = value.get_mut("bin_streams").and_then(|v| v.as_array_mut()) {
        for (i, item) in arr.iter_mut().enumerate() {
            if let Some(b64) = item.as_object_mut().and_then(|o| o.remove("data_b64")) {
                let s = b64
                    .as_str()
                    .ok_or_else(|| format!("bin_streams[{i}].data_b64는 문자열이어야 합니다"))?;
                let bytes = base64::decode(s).map_err(|e| format!("bin_streams[{i}]: {e}"))?;
                decoded.push((i, bytes));
            }
        }
    }

    let mut doc: Document = serde_json::from_value(value).map_err(|e| e.to_string())?;
    for (i, bytes) in decoded {
        if let Some(bin) = doc.bin_streams.get_mut(i) {
            bin.data = bytes;
        }
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::BinStream;

    fn 표본_문서() -> Document {
        from_markdown("# 제목\n\n본문 문단입니다.\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n")
    }

    #[test]
    fn json_왕복_구조_동일() {
        let doc = 표본_문서();
        let json = to_json(&doc, true, false).unwrap();
        let back = from_json(&json).unwrap();
        // The whole IR except the image bytes must be identical.
        assert_eq!(doc, back);
    }

    #[test]
    fn embed_bin_이미지_왕복() {
        let mut doc = 표본_문서();
        doc.bin_streams.push(BinStream {
            name: "BIN0001.png".to_string(),
            data: vec![0x89, b'P', b'N', b'G', 1, 2, 3, 255, 0, 42],
        });

        // Without embed the image bytes are dropped.
        let lean = from_json(&to_json(&doc, false, false).unwrap()).unwrap();
        assert_eq!(lean.bin_streams[0].name, "BIN0001.png");
        assert!(lean.bin_streams[0].data.is_empty());

        // With embed the round trip is lossless down to the bytes.
        let full = from_json(&to_json(&doc, false, true).unwrap()).unwrap();
        assert_eq!(full, doc);
    }
}
