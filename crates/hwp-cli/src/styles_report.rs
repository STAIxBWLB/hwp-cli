//! The `hwp styles` listing, `schemas/styles-v1.schema.json` (#427 T2).
//!
//! It lives in the library, like [`crate::segment_envelope`], so the CLI command, the MCP tool
//! and the integration tests drive one serializer. The attributes use the segment envelope's own
//! style-level shape and `used_by` carries the envelope's segment ids, so a consumer joins the
//! two without a second lookup. Nothing here derives an id or an attribute: this is a
//! transcription of `hwp_convert::NamedStyle`.

use hwp_convert::{MarkdownOptions, NamedStyle, list_named_styles, to_markdown_with_segments_v2};
use hwp_model::Document;

use crate::segment_envelope::style_level_json;

/// The contract name, carried in every listing.
pub const STYLES_CONTRACT: &str = "hwp-styles-v1";
/// The version of the listing contract, carried in every listing.
pub const STYLES_SCHEMA_VERSION: &str = "1.0";

/// Lists the document's named styles with default text options, the options the envelope's
/// segment ids are published under.
pub fn list(doc: &Document) -> std::io::Result<Vec<NamedStyle>> {
    let (_, segments) = to_markdown_with_segments_v2(doc, &MarkdownOptions::default())?;
    Ok(list_named_styles(doc, &segments))
}

/// The complete listing document.
pub fn styles_json(styles: &[NamedStyle]) -> serde_json::Value {
    serde_json::json!({
        "contract": STYLES_CONTRACT,
        "schema_version": STYLES_SCHEMA_VERSION,
        "styles": styles.iter().map(style_json).collect::<Vec<_>>(),
    })
}

fn style_json(style: &NamedStyle) -> serde_json::Value {
    serde_json::json!({
        "id": style.id,
        "name": style.name,
        "english_name": style.english_name,
        "kind": style.kind.map(|kind| kind.as_str()),
        "next_style": style.next_style,
        "attrs": style_level_json(&style.attrs),
        "used_by": style.used_by,
    })
}

/// One line per style for a terminal: `id name (english) kind  size pt, N paragraph(s)`.
pub fn styles_text(styles: &[NamedStyle]) -> String {
    if styles.is_empty() {
        return "스타일 없음\n".to_string();
    }
    let mut out = format!("스타일 {}개:\n", styles.len());
    for style in styles {
        let kind = style.kind.map_or("?", |kind| match kind {
            hwp_convert::StyleKind::Paragraph => "문단",
            hwp_convert::StyleKind::Character => "글자",
        });
        let size = style
            .attrs
            .char
            .as_ref()
            .map_or_else(|| "-".to_string(), |c| format!("{}pt", c.size_pt));
        out.push_str(&format!(
            "  [{}] {:?} ({}) {kind} {size} 사용 {}문단\n",
            style.id,
            style.name,
            style.english_name,
            style.used_by.len()
        ));
    }
    out
}
