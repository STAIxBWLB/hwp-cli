//! Tab stops — the semantic model (`DocHeader::tab_stops`) is the source of truth for a
//! paragraph's explicit tab stops, since both the hwp5 and hwpx readers populate it. The raw
//! `TAB_DEF` bytes (`DocHeader::tab_defs`, hwp5-only) are a fallback, used only when the semantic
//! definition for the same `tab_def_id` is empty — the case where an hwp5 document's semantic
//! parse (`hwp5::doc_info::parse_tab_def`) failed but the raw record still carries the data. Each
//! stop carries its kind and leader (fill) code alongside its position.

use hwp_model::{Document, Paragraph};

/// One tab stop: position in points, tab kind, and leader (fill) code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabStop {
    /// Tab position in points.
    pub pos_pt: f32,
    /// Tab kind: 0 left, 1 right, 2 center, 3 decimal.
    pub kind: u8,
    /// Leader/fill line-type code (border-line-style family). 0 means no leader.
    pub fill: u8,
}

/// A paragraph's explicit tab stops (pt, ascending). Empty means "use the default interval".
pub fn tab_stops(doc: &Document, para: &Paragraph) -> Vec<TabStop> {
    let pid = doc
        .header
        .para_shapes
        .get(para.para_shape.0 as usize)
        .map_or(0, |p| p.tab_def_id);
    let mut stops: Vec<TabStop> = match doc.header.tab_stops.get(pid as usize) {
        Some(def) if !def.items.is_empty() => def
            .items
            .iter()
            .filter(|item| item.pos > 0)
            .map(|item| TabStop {
                pos_pt: item.pos as f32 / 100.0,
                kind: item.kind,
                fill: item.fill,
            })
            .collect(),
        // Semantic definition missing or empty: fall back to the raw hwp5 bytes for the same
        // tab_def_id (covers an hwp5 document whose semantic parse produced an empty TabDef while
        // tab_defs still carries the original bytes).
        _ => match doc.header.tab_defs.get(pid as usize) {
            Some(entry) => parse_tab_stops(&entry.data),
            None => Vec::new(),
        },
    };
    stops.sort_by(|a, b| {
        a.pos_pt
            .partial_cmp(&b.pos_pt)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    stops
}

/// TAB_DEF raw: `u32 attr, i32 count, count×(i32 pos HWPUNIT, u8 type, u8 fill, u16 resv)`.
/// Used only as the hwp5-origin fallback when the semantic parse produced an empty definition.
fn parse_tab_stops(raw: &[u8]) -> Vec<TabStop> {
    if raw.len() < 8 {
        return Vec::new();
    }
    let count = i32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]).max(0) as usize;
    let mut out = Vec::with_capacity(count.min(64));
    for i in 0..count {
        let base = 8 + i * 8;
        if base + 4 > raw.len() {
            break;
        }
        let pos = i32::from_le_bytes([raw[base], raw[base + 1], raw[base + 2], raw[base + 3]]);
        if pos <= 0 {
            continue;
        }
        // The type/fill bytes are bounds-checked separately from the position; a truncated
        // entry that has a valid position but no room for them just falls back to 0/0.
        let (kind, fill) = if base + 6 <= raw.len() {
            (raw[base + 4], raw[base + 5])
        } else {
            (0, 0)
        };
        out.push(TabStop {
            pos_pt: pos as f32 / 100.0,
            kind,
            fill,
        });
    }
    out
}

/// Current position `rel` (pt, relative to line start) -> next tab position, plus the stop
/// landed on when it was an explicit one (`None` when the default grid interval was used).
///
/// Explicit stops win: the first stop strictly greater than `rel` (with the existing epsilon) is
/// selected, so a tab landing exactly on a stop advances past it rather than standing still.
pub fn next_tab_at(tabs: &[TabStop], rel: f32, default_interval: f32) -> (f32, Option<TabStop>) {
    if let Some(&stop) = tabs.iter().find(|t| t.pos_pt > rel + 0.01) {
        (stop.pos_pt, Some(stop))
    } else {
        (
            (rel / default_interval).floor() * default_interval + default_interval,
            None,
        )
    }
}

/// Current position `rel` (pt, relative to line start) -> next tab position. Explicit stops win,
/// falling back to the default grid interval.
pub fn next_tab(tabs: &[TabStop], rel: f32, default_interval: f32) -> f32 {
    next_tab_at(tabs, rel, default_interval).0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// attr(4) + count=2, two tabs (100pt left, 200pt center).
    #[test]
    fn 탭_파싱() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&0u32.to_le_bytes()); // attr
        raw.extend_from_slice(&2i32.to_le_bytes()); // count
        for (pos, ty) in [(10000i32, 0u8), (20000, 2)] {
            raw.extend_from_slice(&pos.to_le_bytes());
            raw.push(ty);
            raw.push(0); // fill
            raw.extend_from_slice(&0u16.to_le_bytes()); // reserved
        }
        let stops = parse_tab_stops(&raw);
        assert_eq!(
            stops,
            vec![
                TabStop {
                    pos_pt: 100.0,
                    kind: 0,
                    fill: 0
                },
                TabStop {
                    pos_pt: 200.0,
                    kind: 2,
                    fill: 0
                },
            ]
        ); // HWPUNIT/100 = pt
    }

    #[test]
    fn 빈_정의는_빈_스톱() {
        assert!(parse_tab_stops(&[]).is_empty());
        assert!(parse_tab_stops(&0u32.to_le_bytes()).is_empty()); // fewer than 8 bytes
    }

    #[test]
    fn next_tab_명시_우선_기본_폴백() {
        let tabs = [
            TabStop {
                pos_pt: 100.0,
                kind: 0,
                fill: 0,
            },
            TabStop {
                pos_pt: 200.0,
                kind: 0,
                fill: 0,
            },
        ];
        assert_eq!(next_tab(&tabs, 50.0, 40.0), 100.0); // next explicit stop
        assert_eq!(next_tab(&tabs, 150.0, 40.0), 200.0);
        // Past the last stop -> default interval.
        assert_eq!(next_tab(&tabs, 250.0, 40.0), 280.0); // floor(250/40)*40+40
        // No explicit stops -> default interval throughout.
        assert_eq!(next_tab(&[], 10.0, 40.0), 40.0);
    }
}
