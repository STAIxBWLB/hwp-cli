//! Document-level split — the `hwp split` IR transform (GM-4, D-05/D-07/D-08).
//!
//! **Section-boundary split (default, D-05).** [`split_sections`] divides a document
//! into one fragment per `Section`, each carrying the source's whole `DocHeader`
//! unchanged (D-06: no id remapping, no pruning of fonts/styles/border fills/bin
//! streams — a fragment may carry unused palette entries).
//!
//! **Page-range split (opt-in via `--pages`).** [`split_page_ranges`] uses the page
//! boundaries Hancom itself saved in `LineSeg.flags` bit 0 (never a font-driven
//! layout pass — CI has no bundled fonts and page counts derived from fonts are
//! exactly what this project forbids CI tests from asserting on). A boundary that
//! falls inside a paragraph rounds forward to the start of the next paragraph
//! (D-08), so the straddling paragraph stays whole in the fragment before the
//! boundary and the fragment after it starts clean.
//!
//! This module must not depend on the sibling merge-grafting module: D-06's
//! "whole DocHeader unchanged" is exactly the direction that needs no offset
//! arithmetic.

use hwp_model::{Document, Paragraph, Section};

/// A 1-based, inclusive page range for [`split_page_ranges`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub first: usize,
    pub last: usize,
}

/// The address of a paragraph-relative position: a section index, a paragraph
/// index within that section, and a WCHAR offset within that paragraph (0 when
/// the address sits exactly at the paragraph's start).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParagraphAddress {
    pub section: usize,
    pub paragraph: usize,
    pub wchar_offset: u32,
}

/// A page boundary that fell inside a paragraph and was rounded forward to the
/// next paragraph's start (D-08), recorded so the caller can report and ledger
/// the adjustment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageBoundaryRounding {
    pub range_index: usize,
    pub from: ParagraphAddress,
    pub to: ParagraphAddress,
}

/// Result of [`split_sections`] or [`split_page_ranges`].
#[derive(Debug)]
pub struct SplitOutcome {
    /// The fragments, in document (or requested range) order.
    pub fragments: Vec<Document>,
    /// Every D-08 boundary rounding applied while building the fragments.
    pub roundings: Vec<PageBoundaryRounding>,
}

/// Divides `doc` into one fragment per `Section`, in document order (D-05).
///
/// Each fragment carries the source's whole `DocHeader`, `metadata` and every
/// singular package-passthrough field unchanged (D-06) — only `sections`,
/// `header.properties.section_count` (set to 1) and `header.properties.caret`
/// (zeroed, since it may otherwise reference a list/paragraph id outside the
/// fragment's own body) are fragment-specific.
///
/// Returns `Err` when `doc` has no sections.
pub fn split_sections(doc: &Document) -> Result<SplitOutcome, String> {
    if doc.sections.is_empty() {
        return Err("문서에 구역이 없어 분할할 수 없습니다".to_string());
    }
    let fragments = doc
        .sections
        .iter()
        .map(|section| {
            let mut fragment = doc.clone();
            fragment.sections = vec![section.clone()];
            fragment.header.properties.section_count = 1;
            fragment.header.properties.caret = (0, 0, 0);
            fragment
        })
        .collect();
    Ok(SplitOutcome {
        fragments,
        roundings: Vec::new(),
    })
}

/// Returns the address of every page start Hancom itself saved, in document
/// order: for every paragraph, for every `LineSeg` in it whose bit 0 (first
/// line of a page — `hwp-render/src/layout.rs`, the renderer's own ground-truth
/// read of this bit) is set, one [`ParagraphAddress`] naming that segment's
/// owning section, paragraph and `text_start`.
///
/// This is Hancom's own saved pagination read out of the layout cache; it never
/// runs a font-driven layout pass, so it needs no fonts and is safe to assert on
/// in CI (which bundles none). Returns an empty vector when no paragraph carries
/// line segments.
pub fn page_start_addresses(doc: &Document) -> Vec<ParagraphAddress> {
    let mut addresses = Vec::new();
    for (section_index, section) in doc.sections.iter().enumerate() {
        for (paragraph_index, paragraph) in section.paragraphs.iter().enumerate() {
            for seg in &paragraph.line_segs {
                if seg.flags & 0x1 != 0 {
                    addresses.push(ParagraphAddress {
                        section: section_index,
                        paragraph: paragraph_index,
                        wchar_offset: seg.text_start,
                    });
                }
            }
        }
    }
    addresses
}

/// Rounds `addr` forward to the next paragraph's start when it falls strictly
/// inside a paragraph (D-08). Returns `addr` unchanged, with no rounding, when
/// it already sits at a paragraph start (`wchar_offset == 0`).
fn round_boundary(doc: &Document, addr: ParagraphAddress) -> ParagraphAddress {
    if addr.wchar_offset == 0 {
        return addr;
    }
    let section = &doc.sections[addr.section];
    if addr.paragraph + 1 < section.paragraphs.len() {
        ParagraphAddress {
            section: addr.section,
            paragraph: addr.paragraph + 1,
            wchar_offset: 0,
        }
    } else if addr.section + 1 < doc.sections.len() {
        ParagraphAddress {
            section: addr.section + 1,
            paragraph: 0,
            wchar_offset: 0,
        }
    } else {
        // The straddling paragraph is the document's last; there is nowhere
        // forward to move to but past the end of this section.
        ParagraphAddress {
            section: addr.section,
            paragraph: section.paragraphs.len(),
            wchar_offset: 0,
        }
    }
}

/// Divides `doc` into one fragment per requested page range, in the order the
/// ranges were given (D-05's opt-in page-range mode).
///
/// Page boundaries come only from [`page_start_addresses`] — never a
/// font-driven layout pass. A boundary that falls inside a paragraph rounds
/// forward to the next paragraph's start (D-08): the straddling paragraph stays
/// whole in the fragment before the boundary, and the fragment after it starts
/// at the following paragraph. Every fragment's slice is kept within a single
/// section (the section the range's start address names); when the slice does
/// not already start with that section's first paragraph (which carries its
/// `Control::SectionDef`), that first paragraph is prepended so the fragment
/// stays a well-formed section.
///
/// Returns `Err` for an empty range list, a range with `first > last` or
/// `first == 0`, a range naming a page beyond the count [`page_start_addresses`]
/// implies, or a document with no saved line segments at all.
pub fn split_page_ranges(doc: &Document, ranges: &[PageRange]) -> Result<SplitOutcome, String> {
    if ranges.is_empty() {
        return Err("페이지 범위가 비어 있습니다".to_string());
    }
    for range in ranges {
        if range.first == 0 {
            return Err("페이지 번호는 1부터 시작해야 합니다".to_string());
        }
        if range.first > range.last {
            return Err(format!(
                "페이지 범위가 뒤집혀 있습니다: {}-{}",
                range.first, range.last
            ));
        }
    }

    let addresses = page_start_addresses(doc);
    if addresses.is_empty() {
        return Err(
            "문서에 저장된 줄 배치(PARA_LINE_SEG)가 없어 페이지 경계를 판단할 수 없습니다"
                .to_string(),
        );
    }
    let page_count = addresses.len();
    for range in ranges {
        if range.last > page_count {
            return Err(format!(
                "페이지 범위가 문서의 마지막 페이지({page_count})를 넘습니다: {}-{}",
                range.first, range.last
            ));
        }
    }

    let mut fragments = Vec::with_capacity(ranges.len());
    let mut roundings = Vec::new();

    for (range_index, range) in ranges.iter().enumerate() {
        let raw_start = addresses[range.first - 1];
        let start = round_boundary(doc, raw_start);
        if start != raw_start {
            roundings.push(PageBoundaryRounding {
                range_index,
                from: raw_start,
                to: start,
            });
        }

        let end = if range.last < page_count {
            let raw_end = addresses[range.last];
            let rounded_end = round_boundary(doc, raw_end);
            if rounded_end != raw_end {
                roundings.push(PageBoundaryRounding {
                    range_index,
                    from: raw_end,
                    to: rounded_end,
                });
            }
            Some(rounded_end)
        } else {
            None
        };

        let section_index = start.section;
        let section = doc
            .sections
            .get(section_index)
            .ok_or_else(|| "페이지 경계가 가리키는 구역을 찾을 수 없습니다".to_string())?;
        let end_paragraph = match end {
            Some(end) if end.section == section_index => end.paragraph,
            _ => section.paragraphs.len(),
        };
        let start_paragraph = start.paragraph.min(end_paragraph);

        let mut paragraphs: Vec<Paragraph> =
            section.paragraphs[start_paragraph..end_paragraph].to_vec();
        if start_paragraph != 0
            && let Some(first_paragraph) = section.paragraphs.first()
        {
            paragraphs.insert(0, first_paragraph.clone());
        }

        let mut fragment = doc.clone();
        fragment.sections = vec![Section {
            paragraphs,
            extras: section.extras.clone(),
        }];
        fragment.header.properties.section_count = 1;
        fragment.header.properties.caret = (0, 0, 0);
        fragments.push(fragment);
    }

    Ok(SplitOutcome {
        fragments,
        roundings,
    })
}

/// Whether `paragraph` carries a `Control::SectionDef` — used only by tests to
/// confirm the prepend-guard in [`split_page_ranges`] keeps every fragment's
/// first paragraph well-formed.
#[cfg(test)]
fn has_section_def(paragraph: &Paragraph) -> bool {
    paragraph
        .controls
        .iter()
        .any(|control| matches!(control, hwp_model::Control::SectionDef(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwp_model::LineSeg;

    /// A three-section document sharing one `DocHeader` (a hand-built stand-in
    /// for a multi-section hwp5 document — built directly from three
    /// `from_markdown` sections rather than via the sibling merge-grafting
    /// module, since this module must not depend on that one).
    fn three_section_doc() -> Document {
        let mut doc = crate::from_markdown("첫 구역\n");
        doc.sections
            .push(crate::from_markdown("둘째 구역\n").sections[0].clone());
        doc.sections
            .push(crate::from_markdown("셋째 구역\n").sections[0].clone());
        doc
    }

    fn plain_text(doc: &Document) -> String {
        doc.sections
            .iter()
            .flat_map(|section| &section.paragraphs)
            .flat_map(|paragraph| &paragraph.chars)
            .filter_map(|character| match character {
                hwp_model::HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn 세_구역_문서는_세_조각으로_순서대로_나뉜다() {
        let doc = three_section_doc();
        let outcome = split_sections(&doc).unwrap();
        assert_eq!(outcome.fragments.len(), 3);
        assert!(outcome.roundings.is_empty());
        assert!(plain_text(&outcome.fragments[0]).contains("첫 구역"));
        assert!(plain_text(&outcome.fragments[1]).contains("둘째 구역"));
        assert!(plain_text(&outcome.fragments[2]).contains("셋째 구역"));
    }

    #[test]
    fn 조각마다_docheader_컬렉션_길이가_원본과_같다() {
        let doc = three_section_doc();
        let outcome = split_sections(&doc).unwrap();
        for fragment in &outcome.fragments {
            for lang in 0..hwp_model::LANG_COUNT {
                assert_eq!(
                    fragment.header.fonts[lang].len(),
                    doc.header.fonts[lang].len()
                );
            }
            assert_eq!(
                fragment.header.char_shapes.len(),
                doc.header.char_shapes.len()
            );
            assert_eq!(
                fragment.header.para_shapes.len(),
                doc.header.para_shapes.len()
            );
            assert_eq!(fragment.header.styles.len(), doc.header.styles.len());
            assert_eq!(
                fragment.header.border_fills.len(),
                doc.header.border_fills.len()
            );
            assert_eq!(fragment.header.tab_defs.len(), doc.header.tab_defs.len());
            assert_eq!(
                fragment.header.numberings.len(),
                doc.header.numberings.len()
            );
            assert_eq!(fragment.header.bullets.len(), doc.header.bullets.len());
        }
    }

    #[test]
    fn 조각의_section_count는_1이고_caret은_영이다() {
        let doc = three_section_doc();
        let outcome = split_sections(&doc).unwrap();
        for fragment in &outcome.fragments {
            assert_eq!(fragment.header.properties.section_count, 1);
            assert_eq!(fragment.header.properties.caret, (0, 0, 0));
        }
    }

    #[test]
    fn 단일_구역_문서는_조각_하나를_낸다() {
        let doc = crate::from_markdown("단일 구역\n");
        let outcome = split_sections(&doc).unwrap();
        assert_eq!(outcome.fragments.len(), 1);
    }

    #[test]
    fn 구역이_없는_문서는_거부된다() {
        let doc = Document::default();
        assert!(split_sections(&doc).is_err());
    }

    #[test]
    fn 줄_배치가_없으면_페이지_시작_주소가_비어있다() {
        let doc = crate::from_markdown("본문\n");
        assert!(page_start_addresses(&doc).is_empty());
    }

    fn multi_paragraph_doc() -> Document {
        crate::from_markdown("문단1\n\n문단2\n\n문단3\n")
    }

    fn set_page_start(doc: &mut Document, paragraph_index: usize, text_start: u32) {
        doc.sections[0].paragraphs[paragraph_index]
            .line_segs
            .push(LineSeg {
                text_start,
                v_pos: 0,
                line_height: 100,
                text_height: 100,
                baseline_gap: 0,
                line_spacing: 0,
                col_start: 0,
                seg_width: 1000,
                flags: 0x1,
            });
    }

    #[test]
    fn 문단_경계와_정확히_맞는_페이지_경계는_보정_없이_통과한다() {
        let mut doc = multi_paragraph_doc();
        assert!(doc.sections[0].paragraphs.len() >= 3);
        // Page 1 starts at paragraph 0 (doc start); page 2 starts exactly at
        // paragraph 1's start (offset 0) — no rounding needed.
        set_page_start(&mut doc, 0, 0);
        set_page_start(&mut doc, 1, 0);

        let outcome = split_page_ranges(&doc, &[PageRange { first: 1, last: 1 }]).unwrap();
        assert!(outcome.roundings.is_empty());
        assert_eq!(outcome.fragments.len(), 1);
        assert!(plain_text(&outcome.fragments[0]).contains("문단1"));
        assert!(!plain_text(&outcome.fragments[0]).contains("문단2"));
    }

    #[test]
    fn 문단_중간의_페이지_경계는_한_번_보정되고_걸친_문단은_앞_조각에_남는다() {
        let mut doc = multi_paragraph_doc();
        assert!(doc.sections[0].paragraphs.len() >= 3);
        set_page_start(&mut doc, 0, 0);
        // Page 2 starts strictly inside paragraph 1 (a mid-paragraph page
        // break) — the boundary must round forward to paragraph 2's start.
        set_page_start(&mut doc, 1, 3);

        let outcome = split_page_ranges(&doc, &[PageRange { first: 1, last: 1 }]).unwrap();
        assert_eq!(outcome.roundings.len(), 1);
        assert_eq!(outcome.roundings[0].range_index, 0);
        assert_eq!(outcome.roundings[0].from.wchar_offset, 3);
        assert_eq!(outcome.roundings[0].to.wchar_offset, 0);
        // 문단2 straddled the boundary and must stay whole in the earlier fragment.
        assert!(plain_text(&outcome.fragments[0]).contains("문단1"));
        assert!(plain_text(&outcome.fragments[0]).contains("문단2"));
        assert!(!plain_text(&outcome.fragments[0]).contains("문단3"));
    }

    #[test]
    fn 줄_배치가_없는_문서는_페이지_분할이_거부된다() {
        let doc = crate::from_markdown("본문\n");
        let result = split_page_ranges(&doc, &[PageRange { first: 1, last: 1 }]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("PARA_LINE_SEG"));
    }

    #[test]
    fn 마지막_페이지를_넘는_범위는_거부된다() {
        let mut doc = multi_paragraph_doc();
        set_page_start(&mut doc, 0, 0);
        let result = split_page_ranges(&doc, &[PageRange { first: 1, last: 2 }]);
        assert!(result.is_err());
    }

    #[test]
    fn 빈_범위_목록은_거부된다() {
        let mut doc = multi_paragraph_doc();
        set_page_start(&mut doc, 0, 0);
        let result = split_page_ranges(&doc, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn 조각의_첫_문단은_섹션정의를_보존한다() {
        let mut doc = multi_paragraph_doc();
        assert!(has_section_def(&doc.sections[0].paragraphs[0]));
        set_page_start(&mut doc, 0, 0);
        set_page_start(&mut doc, 2, 0);

        // Second fragment (page 2 → the last page) starts at paragraph 2,
        // which does not itself carry SectionDef, so it must be prepended.
        let outcome = split_page_ranges(&doc, &[PageRange { first: 2, last: 2 }]).unwrap();
        assert!(has_section_def(
            &outcome.fragments[0].sections[0].paragraphs[0]
        ));
    }
}
