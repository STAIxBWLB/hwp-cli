//! Proven HWP5 NUMBERING layout used for generated official eight-level lists.

use hwp_model::{NumFmt, NumLevel};

use crate::codec::{ByteReader, ByteWriter};

#[derive(Clone, Copy)]
struct Slot {
    selector: u32,
    format: NumFmt,
    template: &'static str,
}

const OFFICIAL_SLOTS: [Slot; 8] = [
    Slot {
        selector: 0x0000_000c,
        format: NumFmt::Digit,
        template: "^1.",
    },
    Slot {
        selector: 0x0000_010c,
        format: NumFmt::HangulSyllable,
        template: "^2.",
    },
    Slot {
        selector: 0x0000_000c,
        format: NumFmt::Digit,
        template: "^3)",
    },
    Slot {
        selector: 0x0000_010c,
        format: NumFmt::HangulSyllable,
        template: "^4)",
    },
    Slot {
        selector: 0x0000_000c,
        format: NumFmt::Digit,
        template: "(^5)",
    },
    Slot {
        selector: 0x0000_010c,
        format: NumFmt::HangulSyllable,
        template: "(^6)",
    },
    Slot {
        selector: 0x0000_002c,
        format: NumFmt::CircledDigit,
        template: "^7",
    },
    Slot {
        selector: 0x0000_012c,
        format: NumFmt::CircledHangulSyllable,
        template: "^8",
    },
];

const EMPTY_EXTENSION_SELECTOR: u32 = 0x0000_0008;
const WIDTH: u16 = 0;
const BODY_DISTANCE: u16 = 50;
const NO_CHAR_SHAPE: u32 = 0xffff_ffff;

/// Returns true only for the complete semantic contract proven by the private
/// Hancom-saved source. Partial or differently configured lists must not use
/// the direct HWP5 path.
pub fn is_official_eight_level_contract(levels: &[Vec<NumLevel>]) -> bool {
    !levels.is_empty()
        && levels.iter().all(|definition| {
            definition.len() == OFFICIAL_SLOTS.len()
                && definition.iter().zip(OFFICIAL_SLOTS).all(|(level, slot)| {
                    level.start == 1 && level.fmt == slot.format && level.template == slot.template
                })
        })
}

/// Materializes one NUMBERING record from the direct evidence contract.
///
/// The record has seven legacy slots, a reserved u16, and seven starts, then
/// three observed extension slots and starts. All ten persisted starts are one.
pub fn official_eight_level_data() -> Vec<u8> {
    let mut writer = ByteWriter::new();
    for slot in OFFICIAL_SLOTS.into_iter().take(7) {
        write_slot(&mut writer, slot.selector, slot.template);
    }
    writer.write_u16(0);
    for _ in 0..7 {
        writer.write_u32(1);
    }
    write_slot(
        &mut writer,
        OFFICIAL_SLOTS[7].selector,
        OFFICIAL_SLOTS[7].template,
    );
    for _ in 0..2 {
        write_slot(&mut writer, EMPTY_EXTENSION_SELECTOR, "");
    }
    for _ in 0..3 {
        writer.write_u32(1);
    }
    let data = writer.into_bytes();
    debug_assert_eq!(data.len(), 230);
    data
}

/// Recognizes only the fully observed direct record and projects its eight
/// visible slots into the semantic model. Unknown NUMBERING records retain the
/// existing conservative parser.
pub fn parse_official_eight_level_data(data: &[u8]) -> Option<Vec<NumLevel>> {
    let mut reader = ByteReader::new(data);
    for slot in OFFICIAL_SLOTS.into_iter().take(7) {
        if reader.read_u32().ok()? != slot.selector
            || reader.read_u16().ok()? != WIDTH
            || reader.read_u16().ok()? != BODY_DISTANCE
            || reader.read_u32().ok()? != NO_CHAR_SHAPE
            || reader.read_hwp_string().ok()? != slot.template
        {
            return None;
        }
    }
    if reader.read_u16().ok()? != 0 {
        return None;
    }
    let mut levels = Vec::with_capacity(OFFICIAL_SLOTS.len());
    for slot in OFFICIAL_SLOTS.into_iter().take(7) {
        let start = reader.read_u32().ok()?;
        if start != 1 {
            return None;
        }
        levels.push(NumLevel {
            start,
            fmt: slot.format,
            template: slot.template.to_string(),
        });
    }
    let slot = OFFICIAL_SLOTS[7];
    if reader.read_u32().ok()? != slot.selector
        || reader.read_u16().ok()? != WIDTH
        || reader.read_u16().ok()? != BODY_DISTANCE
        || reader.read_u32().ok()? != NO_CHAR_SHAPE
        || reader.read_hwp_string().ok()? != slot.template
    {
        return None;
    }
    for _ in 0..2 {
        if reader.read_u32().ok()? != EMPTY_EXTENSION_SELECTOR
            || reader.read_u16().ok()? != WIDTH
            || reader.read_u16().ok()? != BODY_DISTANCE
            || reader.read_u32().ok()? != NO_CHAR_SHAPE
            || !reader.read_hwp_string().ok()?.is_empty()
        {
            return None;
        }
    }
    let start = reader.read_u32().ok()?;
    if start != 1 {
        return None;
    }
    levels.push(NumLevel {
        start,
        fmt: slot.format,
        template: slot.template.to_string(),
    });
    for _ in 0..2 {
        let start = reader.read_u32().ok()?;
        if start != 1 {
            return None;
        }
    }
    reader.is_empty().then_some(levels)
}

pub fn is_official_eight_level_data(data: &[u8]) -> bool {
    parse_official_eight_level_data(data).is_some()
}

fn write_slot(writer: &mut ByteWriter, selector: u32, template: &str) {
    writer.write_u32(selector);
    writer.write_u16(WIDTH);
    writer.write_u16(BODY_DISTANCE);
    writer.write_u32(NO_CHAR_SHAPE);
    let units: Vec<_> = template.encode_utf16().collect();
    writer.write_u16(units.len() as u16);
    for unit in units {
        writer.write_u16(unit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default NUMBERING record's level selectors are the official slots' selectors for levels
    /// 1 to 7, so the formats it draws are `write::DEFAULT_NUMBERING_FORMATS` (#379 review). Each
    /// level is attr u32 (the selector), width u16, distance u16, char shape u32, then the
    /// template as a length-prefixed WCHAR string.
    #[test]
    fn default_numbering_formats_follow_the_default_record_selectors() {
        let data = &crate::write::DEFAULT_NUMBERING_DATA;
        let mut offset = 0;
        for (index, format) in crate::write::DEFAULT_NUMBERING_FORMATS.iter().enumerate() {
            let selector = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            let slot = OFFICIAL_SLOTS
                .iter()
                .find(|slot| slot.selector == selector)
                .unwrap_or_else(|| panic!("level {}: unknown selector {selector:#x}", index + 1));
            assert_eq!(slot.format, *format, "level {}", index + 1);
            let len = usize::from(u16::from_le_bytes([data[offset + 12], data[offset + 13]]));
            offset += 14 + 2 * len;
        }
    }
}
