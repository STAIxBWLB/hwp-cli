//! quick-xml helper utilities.
use quick_xml::events::{BytesRef, BytesStart};

/// Looks up an attribute by local name (without its namespace prefix), resolving entity references. Raw
/// escaped values such as `A&amp;B` must be unescaped before entering the IR, or the writer's `esc()`
/// would double-escape bookmark/field names and equation script attributes. Invalid references retain
/// their original spelling.
pub fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = a.key.local_name();
        if key.as_ref() == name {
            let raw = a.value.into_owned();
            Some(
                quick_xml::escape::unescape(&raw)
                    .map(|s| s.into_owned())
                    .unwrap_or(raw),
            )
        } else {
            None
        }
    })
}

/// Turns an entity reference (`&amp;`, `&#x2264;`) back into the character it names.
/// quick-xml does not fold a reference into the surrounding text; it delivers each one as its
/// own `Event::GeneralRef`, so every site that collects element text has to decode them. Without
/// a DTD an external entity cannot be resolved, so only the five predefined names and numeric
/// character references are decoded; an unknown reference yields `None` and the caller drops it
/// instead of passing it through.
pub(crate) fn resolve_entity(r: &BytesRef<'_>) -> Option<char> {
    r.resolve_char_ref()
        .ok()
        .flatten()
        .or_else(|| match r.as_bytes() {
            b"amp" => Some('&'),
            b"lt" => Some('<'),
            b"gt" => Some('>'),
            b"quot" => Some('"'),
            b"apos" => Some('\''),
            _ => None,
        })
}

pub fn attr_u32(e: &BytesStart<'_>, name: &str) -> Option<u32> {
    attr(e, name)?.parse().ok()
}

pub fn attr_i32(e: &BytesStart<'_>, name: &str) -> Option<i32> {
    attr(e, name)?.parse().ok()
}

/// Reads signed 32-bit attributes such as offsets. HWPX stores negatives as unsigned two's-complement
/// decimal (e.g. -77 = "4294967219"); parse as i64, then reinterpret as i32.
pub fn attr_offset_i32(e: &BytesStart<'_>, name: &str) -> Option<i32> {
    attr(e, name)?.parse::<i64>().ok().map(|v| v as i32)
}

pub fn attr_u16(e: &BytesStart<'_>, name: &str) -> Option<u16> {
    attr(e, name)?.parse().ok()
}

/// "#RRGGBB" -> COLORREF(0x00BBGGRR). "none" or a parse failure yields 0xFFFF_FFFF.
pub fn parse_color(s: &str) -> u32 {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() == 6
        && let Ok(rgb) = u32::from_str_radix(hex, 16)
    {
        let r = (rgb >> 16) & 0xFF;
        let g = (rgb >> 8) & 0xFF;
        let b = rgb & 0xFF;
        return (b << 16) | (g << 8) | r;
    }
    0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_resolution_preserves_predefined_and_numeric_references() {
        for (reference, expected) in [
            ("amp", Some('&')),
            ("lt", Some('<')),
            ("gt", Some('>')),
            ("quot", Some('"')),
            ("apos", Some('\'')),
            ("#65", Some('A')),
            ("#x41", Some('A')),
            ("#48124", Some('민')),
            ("#xBBFC", Some('민')),
            ("#128512", Some('😀')),
            ("#x1F600", Some('😀')),
            ("unknown", None),
            ("#invalid", None),
            ("#xZZ", None),
            ("#x110000", None),
            ("#xD800", None),
        ] {
            assert_eq!(
                resolve_entity(&BytesRef::new(reference)),
                expected,
                "{reference}"
            );
        }
    }

    #[test]
    fn 색_변환() {
        assert_eq!(parse_color("#FF0000"), 0x0000_00FF); // Red -> BGR.
        assert_eq!(parse_color("#000000"), 0);
        assert_eq!(parse_color("none"), 0xFFFF_FFFF);
    }

    /// Attribute entities are resolved immediately so the writer's esc() does not accumulate double
    /// escaping (`A&amp;B` -> `A&amp;amp;B`) on each round-trip.
    #[test]
    fn 속성값_엔티티_해석() {
        let el = quick_xml::events::BytesStart::from_content(
            r#"hp:bookmark name="A&amp;B &lt;중&gt; &#48124;" bad="A&nosuch;B""#,
            12,
        );
        assert_eq!(attr(&el, "name").as_deref(), Some("A&B <중> 민")); // &#48124; = U+BBFC
        // Keep unresolved references verbatim so they can be written again without data loss.
        assert_eq!(attr(&el, "bad").as_deref(), Some("A&nosuch;B"));
    }
}
