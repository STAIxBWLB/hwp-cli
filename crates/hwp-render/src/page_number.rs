//! Page-number control decoding shared by layout furniture (`pgnp`) and
//! inline automatic numbers (`atno`).
//!
//! HWP 5.0 control payloads keep the number-shape code in different bit
//! ranges, but use the same character decorations.  This module deliberately
//! interprets only rendering semantics; the raw bytes remain owned by the IR.

use hwp_model::NumFmt;

use crate::issues::{RenderIssueAccumulator, RenderIssueCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageNumberPlacement {
    pub position: u8,
    pub format: u8,
    pub user_char: u16,
    pub prefix_char: u16,
    pub suffix_char: u16,
    pub side_char: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AutoNumber {
    pub kind: u8,
    pub format: u8,
    pub superscript: bool,
    pub user_char: u16,
    pub prefix_char: u16,
    pub suffix_char: u16,
}

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
    ]))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
        *data.get(offset + 2)?,
        *data.get(offset + 3)?,
    ]))
}

pub(crate) fn parse_pgnp(data: &[u8]) -> Option<PageNumberPlacement> {
    let props = u32_at(data, 0)?;
    Some(PageNumberPlacement {
        position: ((props >> 8) & 0x0f) as u8,
        format: (props & 0xff) as u8,
        user_char: u16_at(data, 4)?,
        prefix_char: u16_at(data, 6)?,
        suffix_char: u16_at(data, 8)?,
        side_char: u16_at(data, 10)?,
    })
}

pub(crate) fn parse_atno(data: &[u8]) -> Option<AutoNumber> {
    let props = u32_at(data, 0)?;
    Some(AutoNumber {
        kind: (props & 0x0f) as u8,
        format: ((props >> 4) & 0xff) as u8,
        superscript: props & (1 << 12) != 0,
        user_char: u16_at(data, 6)?,
        prefix_char: u16_at(data, 8)?,
        suffix_char: u16_at(data, 10)?,
    })
}

pub(crate) fn parse_nwno_page(data: &[u8]) -> Option<u32> {
    let kind = u32_at(data, 0)?;
    let number = u16_at(data, 4)?;
    (kind == 0).then(|| u32::from(number.max(1)))
}

pub(crate) fn pghd_hides_page_number(data: &[u8]) -> bool {
    u32_at(data, 0).is_some_and(|mask| mask & (1 << 5) != 0)
}

pub(crate) fn is_page_atno(data: &[u8]) -> bool {
    parse_atno(data).is_some_and(|n| n.kind == 0)
}

fn char_text(value: u16) -> String {
    char::from_u32(u32::from(value))
        .filter(|c| *c != '\0')
        .map(String::from)
        .unwrap_or_default()
}

/// Render the number-shape codes already represented by `hwp-model`.
///
/// Other official HWP shapes are kept visible as decimal rather than being
/// dropped. GE-4 continues to track their HWPX serialization loss separately.
pub(crate) fn format_page_number(
    number: u32,
    format: u8,
    user_char: u16,
    prefix_char: u16,
    suffix_char: u16,
    warnings: &mut RenderIssueAccumulator,
) -> String {
    let fmt = match format {
        0 => Some(NumFmt::Digit),
        1 => Some(NumFmt::CircledDigit),
        2 => Some(NumFmt::RomanUpper),
        3 => Some(NumFmt::RomanLower),
        4 => Some(NumFmt::LatinUpper),
        5 => Some(NumFmt::LatinLower),
        8 => Some(NumFmt::HangulSyllable),
        10 => Some(NumFmt::HangulJamo),
        _ => None,
    };
    let number_text = if let Some(fmt) = fmt {
        hwp_model::list::format_number(number, fmt)
    } else if user_char != 0 {
        char_text(user_char)
    } else {
        warnings.push_once(RenderIssueCode::PageNumberFormatFallback, [format]);
        number.to_string()
    };
    format!(
        "{}{}{}",
        char_text(prefix_char),
        number_text,
        char_text(suffix_char)
    )
}

pub(crate) fn format_placement(
    number: u32,
    placement: PageNumberPlacement,
    warnings: &mut RenderIssueAccumulator,
) -> String {
    let inner = format_page_number(
        number,
        placement.format,
        placement.user_char,
        placement.prefix_char,
        placement.suffix_char,
        warnings,
    );
    let side = char_text(placement.side_char);
    if side.is_empty() {
        inner
    } else {
        format!("{side} {inner} {side}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgnp_파싱과_장식() {
        let mut issues = RenderIssueAccumulator::new();
        let mut data = vec![0u8; 12];
        data[..4].copy_from_slice(&(5u32 << 8).to_le_bytes());
        data[10..12].copy_from_slice(&('-' as u16).to_le_bytes());
        let placement = parse_pgnp(&data).expect("valid pgnp");
        assert_eq!(placement.position, 5);
        assert_eq!(format_placement(3, placement, &mut issues), "- 3 -");
    }

    #[test]
    fn atno_페이지_종류와_서식() {
        let mut issues = RenderIssueAccumulator::new();
        let props = (2u32 << 4) | (1 << 12);
        let mut data = vec![0u8; 12];
        data[..4].copy_from_slice(&props.to_le_bytes());
        data[8..10].copy_from_slice(&('(' as u16).to_le_bytes());
        data[10..12].copy_from_slice(&(')' as u16).to_le_bytes());
        let auto = parse_atno(&data).expect("valid atno");
        assert_eq!(auto.kind, 0);
        assert!(auto.superscript);
        assert_eq!(
            format_page_number(
                4,
                auto.format,
                auto.user_char,
                auto.prefix_char,
                auto.suffix_char,
                &mut issues,
            ),
            "(IV)"
        );
    }

    #[test]
    fn 새번호와_숨김() {
        let mut nwno = vec![0u8; 6];
        nwno[4..6].copy_from_slice(&7u16.to_le_bytes());
        assert_eq!(parse_nwno_page(&nwno), Some(7));
        assert!(pghd_hides_page_number(&(1u32 << 5).to_le_bytes()));
        let mut non_page = vec![0u8; 12];
        non_page[..4].copy_from_slice(&1u32.to_le_bytes());
        assert!(!is_page_atno(&non_page));
    }

    #[test]
    fn 미지원_서식은_보이게_폴백하고_한번만_경고() {
        let mut warnings = RenderIssueAccumulator::new();
        assert_eq!(format_page_number(12, 77, 0, 0, 0, &mut warnings), "12");
        assert_eq!(format_page_number(13, 77, 0, 0, 0, &mut warnings), "13");
        assert_eq!(warnings.finish().issue_count, 1);
    }
}
