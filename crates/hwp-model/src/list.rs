//! 목록(번호매기기/글머리표) 마커 — ParaShape 머리 종류/수준과
//! `numbering_levels`/`bullet_chars`로 문단 머리 마커 문자열을 만든다.
//!
//! 원래 hwp-render 전용이었으나 markdown 내보내기(hwp-convert)도 같은 규칙이
//! 필요해 hwp-model로 이동했다(허브-스포크: render·convert 모두 model만 의존).

use std::collections::HashMap;

use crate::{Document, NumFmt, Paragraph};

/// Section list counters (per numbering definition, levels 1 through 10).
#[derive(Default, Clone)]
pub struct ListState {
    counters: HashMap<u16, [u32; 11]>,
    /// Counter family dedicated to outline paragraphs (`head_type == 1`). Outline
    /// `numbering_id` values are unnormalized raw references (zero in genuine samples), so they do
    /// not select entries from the normal numbering-definition counter map.
    outline_counters: [u32; 11],
}

#[cfg(test)]
mod official_counter_continuation_tests {
    use super::format_number;
    use crate::NumFmt;

    #[test]
    fn hangul_single_vowel_continuation() {
        assert_eq!(format_number(1, NumFmt::HangulSyllable), "가");
        assert_eq!(format_number(14, NumFmt::HangulSyllable), "하");
        assert_eq!(format_number(15, NumFmt::HangulSyllable), "거");
        assert_eq!(format_number(28, NumFmt::HangulSyllable), "허");
        assert_eq!(format_number(29, NumFmt::HangulSyllable), "고");
    }

    #[test]
    fn circled_hangul_evidence_boundary() {
        assert_eq!(format_number(1, NumFmt::CircledHangulSyllable), "㉮");
        assert_eq!(format_number(14, NumFmt::CircledHangulSyllable), "㉻");
        assert_eq!(format_number(15, NumFmt::CircledHangulSyllable), "거");
    }
}

impl ListState {
    /// 이 문단의 머리 마커 문자열(불릿 문자 또는 "1.", "1.1."). 목록이 아니면 None.
    pub fn marker(&mut self, doc: &Document, para: &Paragraph) -> Option<String> {
        let ps = doc.header.para_shapes.get(para.para_shape.0 as usize)?;
        let ty = ps.head_type();
        // Text conversion emits markers only for numbering (2) and bullets (3). Outline (1)
        // remains heading structure here; renderer-only outline markers use marker_for_render().
        if ty != 2 && ty != 3 {
            return None;
        }
        let id = ps.numbering_id as usize;
        if ty == 3 {
            return Some(bullet_char(doc, id).to_string()); // 불릿
        }
        // 번호: 수준 카운터 증가 + 더 깊은 수준 리셋.
        let level = ps.head_level() as usize; // 1..=10
        let counters = self.counters.entry(ps.numbering_id).or_default();
        counters[level] = counters[level].saturating_add(1);
        for c in &mut counters[level + 1..] {
            *c = 0;
        }
        let levels = numbering_levels(doc, id);
        // 최심 수준에 형식 템플릿이 있으면 적용("(^5)"→"(5)", "제^1조"→"제1조", "^1.^2."→"1.1.").
        if let Some(tmpl) = levels
            .and_then(|l| l.get(level - 1))
            .map(|nl| nl.template.as_str())
            && !tmpl.is_empty()
        {
            return Some(apply_template(tmpl, levels, counters));
        }
        // 템플릿 없음: 기존 "1.", "1.1." 폴백.
        let parts: Vec<String> = (1..=level)
            .map(|lv| {
                let fmt = levels
                    .and_then(|l| l.get(lv - 1))
                    .map_or(NumFmt::Digit, |nl| nl.fmt);
                let start = levels.and_then(|l| l.get(lv - 1)).map_or(1, |nl| nl.start);
                let n = u64::from(counters[lv].max(1)) + u64::from(start.saturating_sub(1));
                format_number_wide(n, fmt)
            })
            .collect();
        Some(format!("{}.", parts.join(".")))
    }

    /// Renderer marker including the default per-level outline (`head_type == 1`) formats.
    /// The eight official levels are `1.` / `가.` / `1)` / `가)` / `(1)` / `(가)` / `①` / `㉮`.
    pub fn marker_for_render(&mut self, doc: &Document, para: &Paragraph) -> Option<String> {
        let ps = doc.header.para_shapes.get(para.para_shape.0 as usize)?;
        if ps.head_type() == 1 {
            return Some(self.outline_marker(ps.head_level()));
        }
        self.marker(doc, para)
    }

    /// Advance one outline level, reset deeper levels, and apply the level's default format.
    fn outline_marker(&mut self, level: u8) -> String {
        let level = level.clamp(1, 8) as usize;
        self.outline_counters[level] = self.outline_counters[level].saturating_add(1);
        for c in &mut self.outline_counters[level + 1..] {
            *c = 0;
        }
        let n = self.outline_counters[level].max(1);
        match level {
            1 => format!("{}.", format_number(n, NumFmt::Digit)),
            2 => format!("{}.", format_number(n, NumFmt::HangulSyllable)),
            3 => format!("{})", format_number(n, NumFmt::Digit)),
            4 => format!("{})", format_number(n, NumFmt::HangulSyllable)),
            5 => format!("({})", format_number(n, NumFmt::Digit)),
            6 => format!("({})", format_number(n, NumFmt::HangulSyllable)),
            7 => format_number(n, NumFmt::CircledDigit),
            _ => format_number(n, NumFmt::CircledHangulSyllable),
        }
    }
}

/// Replaces each `^K` (K=1..=10) in a template with its level counter.
fn apply_template(tmpl: &str, levels: Option<&[crate::NumLevel]>, counters: &[u32; 11]) -> String {
    let mut out = String::new();
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^'
            && let Some(k) = chars.peek().and_then(|d| d.to_digit(10))
            && (1..=10).contains(&k)
        {
            chars.next(); // 숫자 소비
            let k = k as usize;
            let nl = levels.and_then(|l| l.get(k - 1));
            let fmt = nl.map_or(NumFmt::Digit, |n| n.fmt);
            let start = nl.map_or(1, |n| n.start);
            let n = u64::from(counters[k].max(1)) + u64::from(start.saturating_sub(1));
            out.push_str(&format_number_wide(n, fmt));
        } else {
            out.push(c);
        }
    }
    out
}

fn bullet_char(doc: &Document, id: usize) -> char {
    doc.header.bullet_chars.get(id).copied().unwrap_or('•')
}

fn numbering_levels(doc: &Document, id: usize) -> Option<&[crate::NumLevel]> {
    doc.header.numbering_levels.get(id).map(Vec::as_slice)
}

/// 번호 n(1부터)을 형식에 맞게 표기.
pub fn format_number(n: u32, fmt: NumFmt) -> String {
    match fmt {
        NumFmt::Digit => n.to_string(),
        NumFmt::HangulSyllable => hangul_syllable(n),
        NumFmt::HangulJamo => cycle("ㄱㄴㄷㄹㅁㅂㅅㅇㅈㅊㅋㅌㅍㅎ", n),
        NumFmt::CircledDigit => {
            if (1..=20).contains(&n) {
                char::from_u32(0x245F + n).map_or_else(|| n.to_string(), |c| c.to_string())
            } else {
                n.to_string()
            }
        }
        NumFmt::CircledHangulSyllable => {
            if (1..=14).contains(&n) {
                char::from_u32(0x326D + n).map_or_else(|| n.to_string(), |c| c.to_string())
            } else {
                // Unicode has precomposed circled Hangul syllables only through
                // ㉻. Native HWP counter rendering supplies the circle for later
                // values, so the IR deliberately keeps the underlying syllable
                // instead of inventing a combining enclosure or lookalike text.
                hangul_syllable(n)
            }
        }
        NumFmt::LatinUpper => latin(n, b'A'),
        NumFmt::LatinLower => latin(n, b'a'),
        NumFmt::RomanUpper => roman(n).to_uppercase(),
        NumFmt::RomanLower => roman(n),
    }
}

/// Formats the complete mathematical list marker without narrowing. Numbering
/// schemes without an unbounded Unicode representation fall back to decimal
/// text above `u32::MAX`, which keeps malformed/read documents printable
/// instead of panicking or wrapping their marker.
fn format_number_wide(n: u64, fmt: NumFmt) -> String {
    match u32::try_from(n) {
        Ok(n) => format_number(n, fmt),
        Err(_) => n.to_string(),
    }
}

/// 문자열에서 (n-1)%len 위치 글자(반복). 큰 n은 순환.
fn cycle(set: &str, n: u32) -> String {
    let chars: Vec<char> = set.chars().collect();
    let i = (n.max(1) - 1) as usize % chars.len();
    chars[i].to_string()
}

/// Korean official-list syllables continue by the single-vowel sequence after
/// 하: 가...하, 거...허, 고...호, 구...후, 그...흐, 기...히.
fn hangul_syllable(n: u32) -> String {
    const INITIALS: [u32; 14] = [0, 2, 3, 5, 6, 7, 9, 11, 12, 14, 15, 16, 17, 18];
    const VOWELS: [u32; 6] = [0, 4, 8, 13, 18, 20];
    let index = n.saturating_sub(1) as usize;
    let Some(&initial) = INITIALS.get(index % INITIALS.len()) else {
        return n.to_string();
    };
    let Some(&vowel) = VOWELS.get(index / INITIALS.len()) else {
        return n.to_string();
    };
    char::from_u32(0xAC00 + initial * 588 + vowel * 28)
        .map_or_else(|| n.to_string(), |character| character.to_string())
}

/// A, B, … Z, AA, AB … (1부터).
fn latin(n: u32, base: u8) -> String {
    let mut n = n.max(1);
    let mut out = String::new();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        out.insert(0, (base + rem) as char);
        n = (n - 1) / 26;
    }
    out
}

/// 로마 숫자(소문자). 1~3999, 범위 밖은 십진.
fn roman(n: u32) -> String {
    if !(1..=3999).contains(&n) {
        return n.to_string();
    }
    const VALS: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (v, s) in VALS {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 마커_카운터() {
        use crate::{NumLevel, ParaShape, ParaShapeId};
        let mut doc = Document::default();
        let mk = |ty: u32, lv: u32| ParaShape {
            attr1: (ty << 23) | (lv << 25),
            numbering_id: 0,
            ..ParaShape::default()
        };
        doc.header.para_shapes = vec![mk(2, 1), mk(2, 2), mk(3, 1)];
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        doc.header.bullet_chars = vec!['•'];
        let mut st = ListState::default();
        let p = |id| Paragraph {
            para_shape: ParaShapeId(id),
            ..Paragraph::default()
        };
        assert_eq!(st.marker(&doc, &p(0)).as_deref(), Some("1."));
        assert_eq!(st.marker(&doc, &p(0)).as_deref(), Some("2."));
        assert_eq!(st.marker(&doc, &p(1)).as_deref(), Some("2.1.")); // 수준2
        assert_eq!(st.marker(&doc, &p(2)).as_deref(), Some("•")); // 불릿
        // 비목록 문단은 None.
        doc.header.para_shapes.push(mk(0, 0));
        assert_eq!(st.marker(&doc, &p(3)), None);
    }

    #[test]
    fn 마커_템플릿() {
        use crate::{NumLevel, ParaShape, ParaShapeId};
        let mut doc = Document::default();
        let mk = |ty: u32, lv: u32| ParaShape {
            attr1: (ty << 23) | (lv << 25),
            numbering_id: 0,
            ..ParaShape::default()
        };
        doc.header.para_shapes = vec![mk(2, 1), mk(2, 2), mk(2, 3)];
        let lvl = |t: &str| NumLevel {
            start: 1,
            fmt: NumFmt::Digit,
            template: t.to_string(),
        };
        // 수준1 "^1.", 수준2 "^1.^2." (중첩), 수준3 "제^3조"
        doc.header.numbering_levels = vec![vec![lvl("^1."), lvl("^1.^2."), lvl("제^3조")]];
        let mut st = ListState::default();
        let p = |id| Paragraph {
            para_shape: ParaShapeId(id),
            ..Paragraph::default()
        };
        assert_eq!(st.marker(&doc, &p(0)).as_deref(), Some("1.")); // ^1.
        assert_eq!(st.marker(&doc, &p(0)).as_deref(), Some("2.")); // ^1.
        assert_eq!(st.marker(&doc, &p(1)).as_deref(), Some("2.1.")); // ^1.^2. 중첩
        assert_eq!(st.marker(&doc, &p(2)).as_deref(), Some("제1조")); // 접두/접미
        // 빈 템플릿이면 기존 폴백("1.").
        doc.header.numbering_levels = vec![vec![NumLevel::default(); 7]];
        let mut st2 = ListState::default();
        assert_eq!(st2.marker(&doc, &p(0)).as_deref(), Some("1."));
    }

    #[test]
    fn 번호정의별_카운터_독립() {
        use crate::{NumLevel, ParaShape, ParaShapeId};
        let mut doc = Document::default();
        let mk = |id: u16| ParaShape {
            attr1: (2 << 23) | (1 << 25),
            numbering_id: id,
            ..ParaShape::default()
        };
        doc.header.para_shapes = vec![mk(0), mk(1)];
        doc.header.numbering_levels =
            vec![vec![NumLevel::default(); 7], vec![NumLevel::default(); 7]];
        let p = |id| Paragraph {
            para_shape: ParaShapeId(id),
            ..Paragraph::default()
        };
        let mut st = ListState::default();
        assert_eq!(st.marker(&doc, &p(0)).as_deref(), Some("1."));
        assert_eq!(st.marker(&doc, &p(1)).as_deref(), Some("1."));
        assert_eq!(st.marker(&doc, &p(0)).as_deref(), Some("2."));
    }

    #[test]
    fn 최대_시작값_뒤의_마커는_패닉하지_않는다() {
        use crate::{NumLevel, ParaShape, ParaShapeId};
        let mut doc = Document::default();
        doc.header.para_shapes = vec![ParaShape {
            attr1: (2 << 23) | (1 << 25),
            numbering_id: 0,
            ..ParaShape::default()
        }];
        doc.header.numbering_levels = vec![vec![NumLevel {
            start: u32::MAX,
            fmt: NumFmt::Digit,
            template: "^1.".to_string(),
        }]];
        let paragraph = Paragraph {
            para_shape: ParaShapeId(0),
            ..Paragraph::default()
        };
        let mut state = ListState::default();
        assert_eq!(
            state.marker(&doc, &paragraph).as_deref(),
            Some("4294967295.")
        );
        assert_eq!(
            state.marker(&doc, &paragraph).as_deref(),
            Some("4294967296.")
        );
    }

    #[test]
    fn 개요_마커() {
        use crate::{ParaShape, ParaShapeId};
        let mut doc = Document::default();
        let mk = |lv: u32| ParaShape {
            attr1: (1 << 23) | (lv << 25),
            ..ParaShape::default()
        };
        doc.header.para_shapes = vec![mk(1), mk(2), mk(3), mk(5), mk(6), mk(7)];
        let p = |id| Paragraph {
            para_shape: ParaShapeId(id),
            ..Paragraph::default()
        };
        // Text conversion keeps outlines as heading structure without markers.
        let mut st = ListState::default();
        assert_eq!(st.marker(&doc, &p(0)), None);

        let mut st = ListState::default();
        assert_eq!(st.marker_for_render(&doc, &p(0)).as_deref(), Some("1."));
        assert_eq!(st.marker_for_render(&doc, &p(0)).as_deref(), Some("2."));
        assert_eq!(st.marker_for_render(&doc, &p(1)).as_deref(), Some("가."));
        assert_eq!(st.marker_for_render(&doc, &p(1)).as_deref(), Some("나."));
        assert_eq!(st.marker_for_render(&doc, &p(2)).as_deref(), Some("1)"));
        // Returning to a shallower level resets deeper counters and continues its own counter.
        assert_eq!(st.marker_for_render(&doc, &p(0)).as_deref(), Some("3."));
        assert_eq!(st.marker_for_render(&doc, &p(1)).as_deref(), Some("가."));
        // Fixed formats for levels 5 through 7.
        assert_eq!(st.marker_for_render(&doc, &p(3)).as_deref(), Some("(1)"));
        assert_eq!(st.marker_for_render(&doc, &p(4)).as_deref(), Some("(가)"));
        assert_eq!(st.marker_for_render(&doc, &p(5)).as_deref(), Some("①"));

        // Outline and regular numbering counters are independent.
        let mk2 = || ParaShape {
            attr1: (2 << 23) | (1 << 25),
            numbering_id: 0,
            ..ParaShape::default()
        };
        doc.header.para_shapes.push(mk2());
        doc.header.numbering_levels = vec![vec![crate::NumLevel::default(); 7]];
        let mut st = ListState::default();
        assert_eq!(st.marker_for_render(&doc, &p(0)).as_deref(), Some("1."));
        assert_eq!(st.marker_for_render(&doc, &p(6)).as_deref(), Some("1."));
        assert_eq!(st.marker_for_render(&doc, &p(0)).as_deref(), Some("2."));
    }

    #[test]
    fn 번호_형식() {
        assert_eq!(format_number(3, NumFmt::Digit), "3");
        assert_eq!(format_number(1, NumFmt::HangulSyllable), "가");
        assert_eq!(format_number(3, NumFmt::HangulSyllable), "다");
        assert_eq!(format_number(14, NumFmt::HangulSyllable), "하");
        assert_eq!(format_number(15, NumFmt::HangulSyllable), "거");
        assert_eq!(format_number(1, NumFmt::CircledDigit), "①");
        assert_eq!(format_number(1, NumFmt::CircledHangulSyllable), "㉮");
        assert_eq!(format_number(14, NumFmt::CircledHangulSyllable), "㉻");
        assert_eq!(format_number(15, NumFmt::CircledHangulSyllable), "거");
        assert_eq!(format_number(1, NumFmt::LatinUpper), "A");
        assert_eq!(format_number(27, NumFmt::LatinUpper), "AA");
        assert_eq!(format_number(4, NumFmt::RomanUpper), "IV");
        assert_eq!(format_number(9, NumFmt::RomanLower), "ix");
    }
}
