//! IR ↔ markdown/JSON 변환.

pub mod base64;
pub mod bookmark;
pub mod csv;
pub mod document_merge;
pub mod document_split;
pub mod docx;
pub mod edit;
pub mod field;
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
pub mod structure;
pub mod style;
pub mod svg;

use hwp_model::Document;

pub use bookmark::{
    BookmarkInfo, bookmark_name, create_bookmark, list_bookmarks, make_bokm_ctrl_data,
};
pub use csv::to_csv;
pub use docx::to_docx;
pub use edit::{
    CloneTextMode, FormCellCandidate, ObjectKind, add_col, add_rows, add_rows_at, add_table,
    add_table_column, add_table_columns, apply_meta, clone_table, delete_object,
    delete_table_column, delete_table_row, find_form_cells_by_label, merge_cells,
    normalize_form_label, replace_text, set_cell, split_cell, table_dims,
};
pub use field::{
    FieldInfo, PlaceholderInfo, create_field, create_hyperlink, hyperlink_url, list_fields,
    scan_placeholders, set_field,
};
pub use format::{
    CharFormat, PageProps, ParaProps, set_char_format, set_page_def, set_para_align, set_para_props,
};
pub use frames::{
    FrameFields, compatibility_warnings, leading_frames, parse_field, parse_frame_fields,
};
pub use from_html::{HtmlImportOptions, from_html, from_html_with};
pub use from_markdown::{
    MarkdownImportOptions, default_header, from_markdown, from_markdown_blocks,
    from_markdown_blocks_report, from_markdown_report, from_markdown_with,
};
pub use html::{to_html, to_html_fragment};
pub use image::{ImageSize, image_kind, image_pixel_size, insert_image, insert_seal};
pub use markdown::{
    MarkdownOptions, MarkdownSegment, to_markdown, to_markdown_with, to_markdown_with_segments,
};
pub use odt::to_odt;
pub use official::{
    OfficialBodyFont, OfficialPreset, OfficialProfile, PageMarginOverrides, profile, profiles,
};
pub use structure::{delete_paragraph, insert_paragraph};
pub use style::{display_width, style_table, style_tables};

/// IR 전체를 JSON으로 직렬화 (구조 검사·디버깅·기계 소비용).
///
/// `embed_bin`이 참이면 첨부 바이너리(`bin_streams[].data`, 기본 JSON 제외)를
/// `data_b64` 필드에 base64로 실어 **자급식 JSON**을 만든다 — [`from_json`]이
/// 다시 읽어 이미지까지 무손실 왕복한다. 거짓이면 이미지 바이트는 빠진다.
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

/// JSON IR을 문서로 역직렬화 ([`to_json`]의 짝).
///
/// `bin_streams[].data_b64`(있을 때)를 디코드해 첨부 바이너리를 복원한다.
/// `data`는 `#[serde(skip)]`이라 base64 필드가 없으면 이미지 바이트는 비어 있다.
pub fn from_json(json: &str) -> Result<Document, String> {
    let mut value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;

    // 역직렬화 전에 data_b64를 분리해 둔다 (Document에는 없는 필드).
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
        // 이미지 바이트 외 IR 전체가 동일해야 한다.
        assert_eq!(doc, back);
    }

    #[test]
    fn embed_bin_이미지_왕복() {
        let mut doc = 표본_문서();
        doc.bin_streams.push(BinStream {
            name: "BIN0001.png".to_string(),
            data: vec![0x89, b'P', b'N', b'G', 1, 2, 3, 255, 0, 42],
        });

        // embed 없이는 이미지 바이트가 빠진다.
        let lean = from_json(&to_json(&doc, false, false).unwrap()).unwrap();
        assert_eq!(lean.bin_streams[0].name, "BIN0001.png");
        assert!(lean.bin_streams[0].data.is_empty());

        // embed면 바이트까지 무손실 왕복.
        let full = from_json(&to_json(&doc, false, true).unwrap()).unwrap();
        assert_eq!(full, doc);
    }
}
