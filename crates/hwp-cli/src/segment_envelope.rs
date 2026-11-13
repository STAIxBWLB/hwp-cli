//! Rendering of the v2 segment envelope, `schemas/segment-envelope-v2.schema.json`.
//!
//! It lives in the library rather than beside `commands::cat` so the integration tests can
//! drive a synthesized document through the real serializer. A contract whose only route to a
//! test is a committed fixture file can only be tested on the cases some fixture happens to
//! contain, and the reserved-alignment case below is exactly one no fixture contains.
//!
//! Nothing here re-derives a kind, an id or a style: this is a transcription of
//! `hwp_convert::Segment` into the published field names.

use hwp_convert::{Segment, StyleLevel};

/// The version of the envelope contract, carried in every envelope.
///
/// `1.1` adds the optional `source_chars` on `run` segments (issue #517), and `1.2` the optional
/// `controls` on `run` segments (issue #543). An additive optional
/// field is a minor bump: a consumer that ignores unknown fields keeps working, and no existing
/// field changed. Removing or renaming one is still a new major.
pub const SEGMENT_ENVELOPE_SCHEMA_VERSION: &str = "1.2";
/// The contract name, carried in every envelope.
pub const SEGMENT_ENVELOPE_CONTRACT: &str = "hwp-segment-envelope-v2";

/// The complete envelope for `--format markdown`, and the base for `--format json`, which adds
/// a `document` sibling.
///
/// `markdown` is a parameter rather than something the caller may attach afterwards: every
/// `char_range` is an offset into it, so an envelope without it would publish offsets into a
/// string it does not carry.
pub fn envelope_v2(markdown: &str, segments: &[Segment]) -> serde_json::Value {
    serde_json::json!({
        "contract": SEGMENT_ENVELOPE_CONTRACT,
        "schema_version": SEGMENT_ENVELOPE_SCHEMA_VERSION,
        "markdown": markdown,
        "segments": segments.iter().map(segment_json).collect::<Vec<_>>(),
    })
}

/// Renders one typed segment.
pub fn segment_json(s: &Segment) -> serde_json::Value {
    let mut v = serde_json::json!({
        "id": s.id,
        "kind": s.kind.as_str(),
        "path": { "section": s.path.section, "indices": s.path.indices },
        "char_range": { "start": s.start, "end": s.end },
        "style": style_level_json(&s.style.style),
        "direct": style_level_json(&s.style.direct),
    });
    // Omitted rather than null when absent: they exist only on field and bookmark segments.
    if let Some(ctrl_id) = &s.ctrl_id {
        v["ctrl_id"] = serde_json::Value::String(ctrl_id.clone());
    }
    if let Some(name) = &s.name {
        v["name"] = serde_json::Value::String(name.clone());
    }
    // Present only on a run piece that emitted source text: a piece with none is not published at
    // all, so there is no empty range here to publish (issue #517).
    if let Some(chars) = &s.source_chars {
        v["source_chars"] = serde_json::json!({
            "start": chars.start,
            "end": chars.end,
        });
    }
    // Omitted rather than empty: only a run that projected a tab, hyphen, non-breaking or
    // fixed-width space control carries it (issue #543).
    if !s.controls.is_empty() {
        v["controls"] = s
            .controls
            .iter()
            .map(|c| {
                serde_json::json!({
                    "kind": c.kind.as_str(),
                    "at": c.at,
                    "source": c.source,
                })
            })
            .collect();
    }
    v
}

/// One style level. A `null` is an explicitly absent block - an id that does not resolve in
/// this document's header table - and never a fabricated default.
pub fn style_level_json(level: &StyleLevel) -> serde_json::Value {
    serde_json::json!({
        "char_shape_id": level.char_shape_id,
        "para_shape_id": level.para_shape_id,
        "char": level.char.as_ref().map(|c| serde_json::json!({
            "face_ids": c.face_ids,
            "faces": c.faces,
            "size_pt": c.size_pt,
            "bold": c.bold,
            "italic": c.italic,
            "color": c.color,
        })),
        "para": level.para.as_ref().map(|p| serde_json::json!({
            "alignment": p.alignment,
            "indent": p.indent,
            "line_spacing_type": p.line_spacing_type,
            "line_spacing": p.line_spacing,
        })),
    })
}
