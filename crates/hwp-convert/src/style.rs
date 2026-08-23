//! Table styling engine (GONG-03, STYL-01): the display-width primitive and `style_table`, the
//! pure function that turns a table's own content into header shading, alignment and
//! content-proportional column widths (D-07). Wired into markdown import from
//! `from_markdown.rs::table_paragraph`, and reused unchanged by the `hwp edit --style-tables`
//! walker (a later phase) so the two call sites cannot drift.
//!
//! D-08 (byte-stable idempotence) is a purity constraint on `style_table`: every value it
//! computes comes from the table's own cells, its header-row count and its total width — nothing
//! else. No marker, no probe of "already styled", no external state. Re-running it on unchanged
//! content must recompute the identical values and, through value-deduped shape allocation
//! (`find_or_insert`/`find_or_insert_para`), append nothing on the second call.

/// Whether `ch` counts as 2 half-width columns (a "wide" character) rather than 1.
///
/// Deliberately a simple per-codepoint range table, not a Unicode East-Asian-Width
/// implementation: it does not handle combining marks, zero-width characters or grapheme
/// clusters, and does not consult font metrics. The corpus evidence behind D-07
/// (`style-patterns.md` §Table conventions) is itself a per-glyph column count — "2타 = one
/// Hangul glyph = two half-width columns" (`korean-official-format.md` §2) — not a typographic
/// measurement, so this ceiling matches what the evidence actually measures.
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF   // Hangul Jamo
        | 0x3130..=0x318F // Hangul Compatibility Jamo
        | 0x3200..=0x32FF // Enclosed CJK letters and months
        | 0x3000..=0x303F // CJK symbols and punctuation
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7A3 // Hangul Syllables
        | 0xFF00..=0xFF60 // Fullwidth forms
        | 0xFFE0..=0xFFE6 // Fullwidth signs
    )
}

/// Half-width column count for `text` — a Hangul syllable (or other wide character) counts 2,
/// everything else (ASCII, digits, plain punctuation, whitespace) counts 1. Total over
/// `text.chars()`: no allocation, no recursion, no `.unwrap()`, no panic path (T-02.4-07). Leading
/// and trailing whitespace are counted, not trimmed, so the measure is stable under the exact
/// cell text.
pub fn display_width(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── display_width ──

    #[test]
    fn display_width_empty() {
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn display_width_ascii() {
        assert_eq!(display_width("abcdef"), 6);
        assert_eq!(display_width("123"), 3);
    }

    #[test]
    fn display_width_hangul_syllables() {
        assert_eq!(display_width("가나다"), 6);
    }

    #[test]
    fn display_width_mixed() {
        assert_eq!(display_width("가a"), 3);
    }

    #[test]
    fn display_width_fullwidth_forms() {
        assert_eq!(display_width("\u{FF21}"), 2); // fullwidth A
        assert_eq!(display_width("\u{FFE5}"), 2); // fullwidth yen sign
    }

    #[test]
    fn display_width_cjk_ideographs() {
        assert_eq!(display_width("\u{4E2D}"), 2); // 中
    }

    #[test]
    fn display_width_hiragana_katakana() {
        assert_eq!(display_width("\u{3042}"), 2); // あ
        assert_eq!(display_width("\u{30A2}"), 2); // ア
    }

    #[test]
    fn display_width_hangul_jamo_and_compat_jamo() {
        assert_eq!(display_width("\u{1100}"), 2); // ᄀ (Hangul Jamo)
        assert_eq!(display_width("\u{3131}"), 2); // ㄱ (Hangul Compatibility Jamo)
    }

    #[test]
    fn display_width_cjk_ext_a_and_enclosed_letters() {
        assert_eq!(display_width("\u{3400}"), 2); // 㐀 (CJK Ext A)
        assert_eq!(display_width("\u{3220}"), 2); // ㈠ (enclosed CJK letter)
    }

    #[test]
    fn display_width_cjk_punctuation() {
        assert_eq!(display_width("\u{3001}"), 2); // 、
    }

    #[test]
    fn display_width_spaces_not_trimmed() {
        assert_eq!(display_width(" a "), 3);
        assert_eq!(display_width("  "), 2);
    }

    #[test]
    fn display_width_total_no_panic_on_any_char() {
        // Total function: every char in the BMP maps to 1 or 2, never panics.
        for c in ['\u{0}', '\u{FFFF}', '\u{10FFFF}'] {
            let _ = display_width(&c.to_string());
        }
    }
}
