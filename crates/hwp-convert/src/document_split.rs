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
//! boundary and the fragment after it starts clean. A range whose start and end
//! pages live in different sections produces one fragment spanning those
//! sections (intermediate sections carried whole), so no content between the
//! two boundaries is dropped.
//!
//! **Typed range extraction (`--from`/`--to`, #417).** [`extract_range`] takes the paragraphs
//! between two anchors (a unique heading text, a bookmark name, or a body address) instead of
//! a page range, which needs a saved layout cache a composed or freshly edited document does
//! not have. The selection is validated against the source hash and refused before anything
//! is built when an anchor is missing, ambiguous, stale or out of order. The fragment carries
//! the section setup of the source section (page, columns, header and footer), the whole
//! `DocHeader` (D-06, no id remapping), and a resource closure computed by ONE walk over the
//! fragment as built: its body, the setup controls carried in front, every caption and
//! paragraph list, the whole header and the package parts the HWPX writer copies back
//! verbatim. A stream is *reported* as excluded only when that walk proves nothing in the
//! fragment uses it. A reference the IR does not model (a border-fill image fill, a raw HWP5
//! shape, OLE or picture record, raw XML that does not parse, a carried part that is not
//! readable XML) is never read by guessing at bytes: the fragment keeps every stream it cannot
//! prove unused and the report names the site. A picture reference resolves through the OPF
//! manifest first, so an item id that differs from its file name still finds its stream. A
//! reference to a stream the source does not carry is a refusal, so a deliberate exclusion can
//! never be confused with a lost asset. What the OUTPUT format then writes can still be
//! narrower (HWP5 emits only streams its BIN_DATA table names), so the CLI reconciles the
//! report with the published file and lists such streams as dropped by the format. Every kept
//! body paragraph and table gets a source-to-output address so a later binding does not guess
//! shifted indices.
//!
//! This module must not depend on the sibling merge-grafting module: D-06's
//! "whole DocHeader unchanged" is exactly the direction that needs no offset
//! arithmetic.

use std::collections::BTreeSet;

use hwp_model::{BinRef, Control, Document, HwpChar, Paragraph, Section};
use sha2::{Digest as _, Sha256};

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
/// at the following paragraph. A fragment's slice runs from its (rounded)
/// start address through its (rounded) end address across section boundaries:
/// the start section contributes from `start.paragraph` on, intermediate
/// sections are carried whole, and the end section contributes up to
/// `end.paragraph` (a range reaching the document's last page runs to the end
/// of the last section), so no content between the two boundaries is dropped.
/// When the slice does not already start with the start section's first
/// paragraph (which carries its `Control::SectionDef`), that first paragraph
/// is prepended so the fragment's first section stays well-formed; later
/// sections keep their own first paragraphs, so they stay well-formed as-is.
/// The fragment's `section_count` is set to the number of sections it carries.
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

        // D-08 slice, spanning every section from the range's start address
        // to its end address: intermediate sections are taken whole, so no
        // content between the two boundaries is dropped.
        let start_section = start.section;
        let last_section = end.map_or(doc.sections.len() - 1, |end| end.section);

        let mut sections: Vec<Section> = Vec::new();
        for section_index in start_section..=last_section {
            let section = doc
                .sections
                .get(section_index)
                .ok_or_else(|| "페이지 경계가 가리키는 구역을 찾을 수 없습니다".to_string())?;
            let slice_start = if section_index == start_section {
                start.paragraph
            } else {
                0
            };
            let slice_end = match end {
                Some(end) if end.section == section_index => end.paragraph,
                _ => section.paragraphs.len(),
            };
            let slice_start = slice_start.min(slice_end);
            if slice_start == slice_end && !sections.is_empty() {
                // The end boundary sits exactly at this section's start —
                // nothing of this section belongs to the fragment.
                continue;
            }
            let mut paragraphs: Vec<Paragraph> =
                section.paragraphs[slice_start..slice_end].to_vec();
            if slice_start != 0
                && let Some(first_paragraph) = section.paragraphs.first()
            {
                paragraphs.insert(0, first_paragraph.clone());
            }
            sections.push(Section {
                paragraphs,
                extras: section.extras.clone(),
            });
        }

        let mut fragment = doc.clone();
        fragment.header.properties.section_count = sections.len() as u16;
        fragment.sections = sections;
        fragment.header.properties.caret = (0, 0, 0);
        fragments.push(fragment);
    }

    Ok(SplitOutcome {
        fragments,
        roundings,
    })
}

// ───────────────────────── typed range extraction (#417) ─────────────────────────

/// Where a range starts or ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeAnchor {
    /// The one top-level body paragraph whose text equals this string (compared after trimming
    /// surrounding whitespace). Two paragraphs with the same text are an ambiguous anchor.
    Heading(String),
    /// The one top-level body paragraph that holds a bookmark of this name.
    Bookmark(String),
    /// A body paragraph by position: `section.paragraph`, both 0-based.
    Address { section: usize, paragraph: usize },
}

impl RangeAnchor {
    /// The anchor's kind as the range report states it: `heading`, `bookmark` or `address`.
    /// The report never carries the anchor's text, which can be document text.
    pub fn kind(&self) -> &'static str {
        match self {
            RangeAnchor::Heading(_) => "heading",
            RangeAnchor::Bookmark(_) => "bookmark",
            RangeAnchor::Address { .. } => "address",
        }
    }

    fn describe(&self) -> String {
        match self {
            RangeAnchor::Heading(text) => format!("제목 {text:?}"),
            RangeAnchor::Bookmark(name) => format!("책갈피 {name:?}"),
            RangeAnchor::Address { section, paragraph } => {
                format!("주소 {section}.{paragraph}")
            }
        }
    }
}

/// A typed range selection.
///
/// The start paragraph is always included. The end anchor, when given, is included unless
/// `end_inclusive` is false; without an end anchor the range runs to the end of the start's
/// section. The range is a contiguous run of top-level body paragraphs of ONE section, so a
/// table (or any other container) is taken whole or not at all: an anchor inside a table cell
/// is not addressable, and a range can never cut a table in half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeSelector {
    pub start: RangeAnchor,
    pub end: Option<RangeAnchor>,
    pub end_inclusive: bool,
    /// Lower-case hex SHA-256 of the source file the caller selected against. When present it
    /// must equal the hash of the file actually loaded, so a selection made against an older
    /// version of the document is refused instead of silently applied to the new one.
    pub source_sha256: Option<String>,
}

/// Why a range could not be extracted. Each variant names the cause and the next step in
/// [`std::fmt::Display`], and none of them is raised after anything was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeError {
    /// The selector carries a hash and the loaded source has another one.
    StaleSource { expected: String, actual: String },
    /// No body paragraph matches the anchor.
    AnchorMissing { anchor: String },
    /// More than one body paragraph matches the anchor.
    AnchorAmbiguous {
        anchor: String,
        matches: Vec<(usize, usize)>,
    },
    /// The start and end anchors resolve to different sections.
    AcrossSections { start: usize, end: usize },
    /// The end anchor resolves before the start.
    EndBeforeStart,
    /// A kept object points at a binary the source does not carry.
    LostResource { reference: String },
    /// The document has no sections.
    EmptyDocument,
}

impl std::fmt::Display for RangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RangeError::StaleSource { expected, actual } => write!(
                f,
                "선택이 만들어진 원본과 지금 읽은 원본이 다릅니다 (선택 기준 sha256 {expected}, 현재 {actual}). 문서가 바뀌었으니 현재 문서에서 다시 선택하세요"
            ),
            RangeError::AnchorMissing { anchor } => write!(
                f,
                "{anchor}에 해당하는 본문 문단이 없습니다. 표 안의 문단은 지정할 수 없고 제목은 문단 전체 텍스트와 같아야 합니다. `hwp cat`으로 정확한 텍스트나 책갈피 이름을 확인하세요"
            ),
            RangeError::AnchorAmbiguous { anchor, matches } => {
                let places: Vec<String> = matches
                    .iter()
                    .map(|(section, paragraph)| format!("{section}.{paragraph}"))
                    .collect();
                write!(
                    f,
                    "{anchor}이(가) 본문 문단 {}곳({})과 일치합니다. 하나만 가리키도록 주소(구역.문단)로 지정하세요",
                    matches.len(),
                    places.join(", ")
                )
            }
            RangeError::AcrossSections { start, end } => write!(
                f,
                "시작({start}번 구역)과 끝({end}번 구역)이 서로 다른 구역입니다. 범위는 한 구역 안에서만 고를 수 있으니 구역마다 나누어 추출하세요"
            ),
            RangeError::EndBeforeStart => write!(
                f,
                "끝 지점이 시작 지점보다 앞에 있습니다. --from과 --to를 서로 바꾸거나 --to를 생략하세요"
            ),
            RangeError::LostResource { reference } => write!(
                f,
                "선택한 범위가 원본에 없는 바이너리({reference})를 참조합니다. 원본이 불완전합니다. `hwp convert --to json --embed-bin`으로 내보낸 자급식 JSON인지 확인하세요"
            ),
            RangeError::EmptyDocument => write!(
                f,
                "문서에 구역이 없어 범위를 추출할 수 없습니다. 본문이 있는 문서인지 `hwp cat`으로 확인하고, 비어 있다면 다른 파일을 지정하세요"
            ),
        }
    }
}

impl std::error::Error for RangeError {}

/// Where a kept source object ended up in the fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceToOutputAddress {
    pub kind: AddressKind,
    /// Section and body-paragraph index in the source.
    pub source: (usize, usize),
    /// Section and body-paragraph index in the fragment.
    pub output: (usize, usize),
    /// 0-based table ordinal in document order (the numbering `hwp edit --set-cell` uses,
    /// nested tables included), in the source and in the fragment. `None` for paragraphs.
    pub source_table: Option<usize>,
    pub output_table: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressKind {
    Paragraph,
    Table,
}

/// A place in the fragment that may reference a binary stream but whose reference the IR does
/// not model, so the closure cannot resolve it. Content-free: a stable cause code and an
/// address in the FRAGMENT (`header/border_fills/N`, `sS/pP` for the top-level body paragraph,
/// or `package/NAME` for a package part carried verbatim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedSite {
    /// `border_fill_image`, `raw_shape`, `raw_ole`, `raw_picture`, `raw_xml_unparsed` or
    /// `carried_part_unread`.
    pub code: &'static str,
    pub at: String,
}

/// What happened to each binary stream of the source.
///
/// The closure is conservative: a stream is only excluded when the whole fragment, header
/// included, provably does not reference it. "Excluded" therefore never means "lost".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceClosureReport {
    /// Streams the fragment provably references (a picture, or a `binaryItemIDRef` in a
    /// raw-preserved object), so the fragment carries them.
    pub kept: Vec<String>,
    /// Streams the fragment carries because it holds a reference site the closure cannot
    /// resolve ([`UnverifiedSite`]) and so cannot prove them unused. Empty when every
    /// reference site in the fragment was resolved.
    pub kept_unverified: Vec<String>,
    /// The sites that made `kept_unverified` non-empty.
    pub unverified: Vec<UnverifiedSite>,
    /// Streams the fragment provably does not reference. The fragment does not carry them,
    /// and this is a deliberate exclusion, not a loss.
    pub excluded: Vec<String>,
    /// Streams the closure decided to carry that the output FORMAT's writer did not emit (an
    /// HWP5 `/BinData` stream no `BIN_DATA` table row names). Filled by
    /// [`ResourceClosureReport::reconcile_with_output`] after the output is built, so that
    /// `kept` and `kept_unverified` only ever list streams the published file really holds.
    pub dropped_by_format: Vec<String>,
}

impl ResourceClosureReport {
    /// Moves every carried stream the published output does not hold from `kept` /
    /// `kept_unverified` to `dropped_by_format`. `fragment` is the stream list the writer was
    /// given, `output` the streams read back from what it wrote. A stream is held when the
    /// output has a stream with the same bytes that no earlier stream has already claimed.
    pub fn reconcile_with_output(
        &mut self,
        fragment: &[hwp_model::BinStream],
        output: &[hwp_model::BinStream],
    ) {
        let mut unclaimed: Vec<&hwp_model::BinStream> = output.iter().collect();
        let mut dropped = BTreeSet::new();
        for stream in fragment {
            match unclaimed.iter().position(|held| held.data == stream.data) {
                Some(position) => {
                    unclaimed.remove(position);
                }
                None => {
                    dropped.insert(stream.name.clone());
                }
            }
        }
        if dropped.is_empty() {
            return;
        }
        self.kept.retain(|name| !dropped.contains(name));
        self.kept_unverified.retain(|name| !dropped.contains(name));
        // Source order, as the other lists.
        self.dropped_by_format = fragment
            .iter()
            .filter(|stream| dropped.contains(&stream.name))
            .map(|stream| stream.name.clone())
            .collect();
    }
}

/// The extracted fragment and the evidence about it.
#[derive(Debug)]
pub struct RangeOutcome {
    pub fragment: Document,
    pub address_map: Vec<SourceToOutputAddress>,
    pub resources: ResourceClosureReport,
    /// The source paragraph range that was taken: `[start, end)` in `section`.
    pub section: usize,
    pub start: usize,
    pub end: usize,
}

/// Lower-case hex SHA-256 of `bytes`: the source identity a selector is checked against.
pub fn source_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn paragraph_text(paragraph: &Paragraph) -> String {
    paragraph
        .chars
        .iter()
        .filter_map(|ch| match ch {
            HwpChar::Text(c) => Some(*c),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Whether `paragraph` itself (not a nested one) holds a bookmark named `name`.
fn holds_bookmark(paragraph: &Paragraph, name: &str) -> bool {
    paragraph.chars.iter().any(|ch| {
        matches!(
            ch,
            HwpChar::ExtCtrl { code, ctrl_id, ctrl_index: Some(index), .. }
                if *code == hwp_model::ctrl_char::BOOKMARK
                    && ctrl_id == b"bokm"
                    && paragraph
                        .controls
                        .get(*index as usize)
                        .and_then(crate::bookmark::bookmark_name)
                        .as_deref()
                        == Some(name)
        )
    })
}

fn resolve_anchor(doc: &Document, anchor: &RangeAnchor) -> Result<(usize, usize), RangeError> {
    let described = anchor.describe();
    let matches: Vec<(usize, usize)> = match anchor {
        RangeAnchor::Address { section, paragraph } => {
            let exists = doc
                .sections
                .get(*section)
                .is_some_and(|section| *paragraph < section.paragraphs.len());
            return if exists {
                Ok((*section, *paragraph))
            } else {
                Err(RangeError::AnchorMissing { anchor: described })
            };
        }
        RangeAnchor::Heading(text) => {
            let wanted = text.trim();
            body_paragraphs(doc)
                .filter(|(_, _, paragraph)| {
                    !wanted.is_empty() && paragraph_text(paragraph) == wanted
                })
                .map(|(section, index, _)| (section, index))
                .collect()
        }
        RangeAnchor::Bookmark(name) => body_paragraphs(doc)
            .filter(|(_, _, paragraph)| !name.is_empty() && holds_bookmark(paragraph, name))
            .map(|(section, index, _)| (section, index))
            .collect(),
    };
    match matches.as_slice() {
        [] => Err(RangeError::AnchorMissing { anchor: described }),
        [only] => Ok(*only),
        _ => Err(RangeError::AnchorAmbiguous {
            anchor: described,
            matches,
        }),
    }
}

fn body_paragraphs(doc: &Document) -> impl Iterator<Item = (usize, usize, &Paragraph)> {
    doc.sections.iter().enumerate().flat_map(|(section, body)| {
        body.paragraphs
            .iter()
            .enumerate()
            .map(move |(index, paragraph)| (section, index, paragraph))
    })
}

/// HWP5 record tags that can carry a binary reference the IR does not model (한글문서파일형식
/// 5.0 §4.3.9, `HWPTAG_BEGIN` = 0x10): a shape component (its fill may be a picture), an OLE
/// object and a picture component. Local copies because this crate does not depend on `hwp5`;
/// `crates/hwp-cli/tests/split_range.rs` pins them to `hwp5::record::tag`.
pub const TAG_SHAPE_COMPONENT: u16 = 0x10 + 60;
pub const TAG_SHAPE_COMPONENT_OLE: u16 = 0x10 + 68;
pub const TAG_SHAPE_COMPONENT_PICTURE: u16 = 0x10 + 69;

/// Whether a SHAPE_COMPONENT record is long enough to hold a fill block and that block's fill
/// type has the image bit. Only the record layout and the one flag are read, never the BinData
/// id that follows. The offsets are the ones `hwp-render` reads the same block with
/// (`shape_draw.rs::parse_style`, its "Table 28" fill): the object id is stored once (4 bytes)
/// or twice (8 bytes, when the two copies are equal), the transform count is the `u16` at +42,
/// the border starts after that many 96-byte matrix pairs, and the fill type is the `u32` 13
/// bytes into the border block. Measured on the three pictures of the genuine
/// `annual_report.hwp` and `work_report.hwp`: 196 bytes, one transform pair, so the record ends
/// exactly where the border block would begin.
fn shape_component_has_image_fill(data: &[u8]) -> bool {
    let read_u16 = |offset: usize| {
        data.get(offset..offset + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let read_u32 = |offset: usize| {
        data.get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if data.len() < 8 {
        return false;
    }
    let base = if data[0..4] == data[4..8] { 8 } else { 4 };
    let Some(count) = read_u16(base + 42) else {
        return false;
    };
    let border = base + 92 + usize::from(count) * 96;
    read_u32(border + 13).is_some_and(|fill_type| fill_type & 0x2 != 0)
}

/// Every binary reference of a document, and every place whose reference could not be read.
#[derive(Default)]
struct ReferenceScan {
    /// References the IR models or that an XML attribute states outright.
    refs: Vec<BinRef>,
    unverified: Vec<UnverifiedSite>,
}

impl ReferenceScan {
    fn unverified(&mut self, code: &'static str, at: &str) {
        let site = UnverifiedSite {
            code,
            at: at.to_string(),
        };
        if !self.unverified.contains(&site) {
            self.unverified.push(site);
        }
    }

    /// Opaque HWP5 records: a record whose tag can hold a binary id is an unverified site. The
    /// bytes are never parsed. `modeled_picture` is true for the extras of a `Control::Picture`,
    /// whose shape component is its frame and whose picture component is where its `bin_ref`
    /// was read from, so both are already accounted for by the `bin_ref` itself. An OLE record
    /// is never accounted for.
    fn opaque(&mut self, records: &[hwp_model::OpaqueRecord], at: &str, modeled_picture: bool) {
        for record in records {
            match record.tag {
                TAG_SHAPE_COMPONENT if !modeled_picture => self.unverified("raw_shape", at),
                // A modeled picture's shape component is its frame. The genuine pictures
                // measured end it before the border and fill block (see
                // `shape_component_has_image_fill`), so there is nothing to read there. A
                // record that does reach a fill block with the image bit could name a BinData
                // id of its own, which the picture's `bin_ref` does not cover, so it is an
                // unverified site just like a raw shape's.
                TAG_SHAPE_COMPONENT if shape_component_has_image_fill(&record.data) => {
                    self.unverified("raw_shape", at);
                }
                TAG_SHAPE_COMPONENT_OLE => self.unverified("raw_ole", at),
                TAG_SHAPE_COMPONENT_PICTURE if !modeled_picture => {
                    self.unverified("raw_picture", at);
                }
                _ => {}
            }
            self.opaque(&record.children, at, modeled_picture);
        }
    }

    /// Raw-preserved HWPX XML: `binaryItemIDRef` attributes are read as attributes, never by
    /// searching text. XML that does not parse is an unverified site.
    fn xml(&mut self, xml: &str, at: &str) {
        let mut reader = quick_xml::Reader::from_str(xml);
        let mut found = Vec::new();
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(element))
                | Ok(quick_xml::events::Event::Empty(element)) => {
                    for attribute in element.attributes() {
                        let Ok(attribute) = attribute else {
                            self.unverified("raw_xml_unparsed", at);
                            return;
                        };
                        if attribute.key.local_name().as_ref() == b"binaryItemIDRef"
                            && let Ok(value) =
                                attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        {
                            found.push(BinRef::ItemRef(value.into_owned()));
                        }
                    }
                }
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(_) => {
                    self.unverified("raw_xml_unparsed", at);
                    return;
                }
            }
        }
        self.refs.extend(found);
    }

    fn paragraph(&mut self, paragraph: &Paragraph, at: &str) {
        self.opaque(&paragraph.extras, at, false);
        for control in &paragraph.controls {
            match control {
                Control::Picture(picture) => {
                    self.refs.push(picture.bin_ref.clone());
                    self.opaque(&picture.extras, at, true);
                    self.caption(&picture.caption, at);
                }
                Control::Table(table) => {
                    self.opaque(&table.extras, at, false);
                    self.caption(&table.caption, at);
                    for cell in &table.cells {
                        for inner in &cell.paragraphs {
                            self.paragraph(inner, at);
                        }
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        for inner in &list.paragraphs {
                            self.paragraph(inner, at);
                        }
                    }
                    self.caption(&generic.caption, at);
                    self.opaque(&generic.raw_children, at, false);
                    self.opaque(&generic.extras, at, false);
                    if let Some(xml) = &generic.hwpx_raw_xml {
                        self.xml(xml, at);
                    }
                }
                Control::SectionDef(def) => {
                    self.opaque(&def.extras, at, false);
                    for xml in &def.secpr_raw_children {
                        self.xml(xml, at);
                    }
                }
            }
        }
    }

    fn caption(&mut self, caption: &Option<hwp_model::Caption>, at: &str) {
        if let Some(caption) = caption {
            for inner in &caption.paragraphs {
                self.paragraph(inner, at);
            }
        }
    }
}

/// The ONE walk that decides the resource closure. It runs over the fragment as built (the
/// whole header, every section and the setup controls carried in front of the range), so a
/// reference site cannot be seen by one path and missed by another.
fn scan_references(fragment: &Document) -> ReferenceScan {
    let mut scan = ReferenceScan::default();
    for (index, fill) in fragment.header.border_fills.iter().enumerate() {
        // Fill-type bit 1 is an image fill; its BinData id sits unparsed in `tail`.
        if fill.fill_type & 0x2 != 0 {
            scan.unverified("border_fill_image", &format!("header/border_fills/{index}"));
        }
    }
    // Package parts the HWPX writer copies back unchanged (`hwpx_extra_entries`: META-INF/*,
    // DocOptions, scripts, master pages). They are parts of the fragment as much as a section
    // is, so a `binaryItemIDRef` in one counts. A part that is not UTF-8 XML cannot be read and
    // is an unverified site rather than a part assumed to be free of references.
    for (name, bytes) in &fragment.hwpx_extra_entries {
        let at = format!("package/{name}");
        let is_xml = {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".xml") || lower.ends_with(".rdf") || lower.ends_with(".hpf")
        };
        match std::str::from_utf8(bytes) {
            Ok(text) if is_xml => scan.xml(text, &at),
            _ => scan.unverified("carried_part_unread", &at),
        }
    }
    for (section_index, section) in fragment.sections.iter().enumerate() {
        scan.opaque(&section.extras, &format!("s{section_index}"), false);
        for (index, paragraph) in section.paragraphs.iter().enumerate() {
            scan.paragraph(paragraph, &format!("s{section_index}/p{index}"));
        }
    }
    scan
}

/// Tables in `paragraph`, in the order `hwp edit` numbers them: a table, then the tables
/// nested in its cells, then the next control.
fn count_tables(paragraph: &Paragraph) -> usize {
    paragraph
        .controls
        .iter()
        .map(|control| match control {
            Control::Table(table) => {
                1 + table
                    .cells
                    .iter()
                    .flat_map(|cell| &cell.paragraphs)
                    .map(count_tables)
                    .sum::<usize>()
            }
            Control::Generic(generic) => generic
                .paragraph_lists
                .iter()
                .flat_map(|list| &list.paragraphs)
                .map(count_tables)
                .sum(),
            _ => 0,
        })
        .sum()
}

/// Whether a control of the section's first paragraph sets the section up (page, columns,
/// header, footer, page-number position, new number) rather than being content.
///
/// `pghd` (hide) and `pgct` (odd/even adjust) are deliberately NOT carried, although
/// `commands::fill::is_section_scoped` lists them. That list serves a splice that puts new
/// paragraphs back where the anchor was, so the controls still act on the page they sit on.
/// Here the range starts somewhere else: a hide or an odd/even adjust acts on the source page
/// that holds it, and moved onto the fragment's first paragraph it would hide or shift a page
/// the author never marked. When the section's first paragraph holds one, the fragment loses
/// it, and nothing in the fragment says so; the range report does not list it either, because
/// the report is about binaries and addresses.
fn is_section_setup_char(paragraph: &Paragraph, ch: &HwpChar) -> bool {
    let HwpChar::ExtCtrl { ctrl_index, .. } = ch else {
        return false;
    };
    matches!(
        ctrl_index.and_then(|index| paragraph.controls.get(index as usize)),
        Some(Control::SectionDef(_))
    ) || matches!(
        ctrl_index.and_then(|index| paragraph.controls.get(index as usize)),
        Some(Control::Generic(generic))
            if matches!(&generic.ctrl_id, b"cold" | b"head" | b"foot" | b"pgnp" | b"nwno")
    )
}

/// Extracts the paragraphs the selector names from one section of `doc`.
///
/// `actual_sha256` is the identity of the source the caller loaded ([`source_sha256`] of its
/// bytes); it is compared with `selector.source_sha256` when the selector carries one. Every
/// refusal happens before the fragment is built, so a caller that gets an `Err` has nothing
/// to clean up.
pub fn extract_range(
    doc: &Document,
    actual_sha256: &str,
    selector: &RangeSelector,
) -> Result<RangeOutcome, RangeError> {
    if doc.sections.is_empty() {
        return Err(RangeError::EmptyDocument);
    }
    if let Some(expected) = &selector.source_sha256
        && !expected.eq_ignore_ascii_case(actual_sha256)
    {
        return Err(RangeError::StaleSource {
            expected: expected.to_ascii_lowercase(),
            actual: actual_sha256.to_string(),
        });
    }

    let (section_index, start) = resolve_anchor(doc, &selector.start)?;
    let section = &doc.sections[section_index];
    let end = match &selector.end {
        None => section.paragraphs.len(),
        Some(anchor) => {
            let (end_section, end_paragraph) = resolve_anchor(doc, anchor)?;
            if end_section != section_index {
                return Err(RangeError::AcrossSections {
                    start: section_index,
                    end: end_section,
                });
            }
            end_paragraph + usize::from(selector.end_inclusive)
        }
    };
    // One check covers a reversed range and `--end-exclusive` on the start paragraph itself:
    // both leave nothing to extract.
    if end <= start {
        return Err(RangeError::EndBeforeStart);
    }

    // The section setup rides on the first paragraph of the source section. When the range
    // does not start there, that paragraph's setup controls are carried in front, as the
    // page-range split does, but only the setup controls: its text and objects are not in the
    // selection.
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut offset = 0usize;
    if start != 0
        && let Some(first) = section.paragraphs.first()
    {
        let mut setup = first.clone();
        setup.chars.retain(|ch| is_section_setup_char(first, ch));
        let kept: Vec<usize> = setup
            .chars
            .iter()
            .filter_map(|ch| match ch {
                HwpChar::ExtCtrl {
                    ctrl_index: Some(index),
                    ..
                } => Some(*index as usize),
                _ => None,
            })
            .collect();
        setup.controls = kept
            .iter()
            .filter_map(|i| first.controls.get(*i).cloned())
            .collect();
        for (new_index, ch) in setup.chars.iter_mut().enumerate() {
            if let HwpChar::ExtCtrl { ctrl_index, .. } = ch {
                *ctrl_index = Some(new_index as u32);
            }
        }
        // The paragraph keeps no text, line cache or run map of the original.
        setup
            .chars
            .push(HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK));
        setup.line_segs.clear();
        setup.char_shape_runs = vec![(
            0,
            first
                .char_shape_runs
                .first()
                .map_or_else(Default::default, |run| run.1),
        )];
        setup.header.ctrl_mask = 0;
        setup.header.hwp5_child_order.clear();
        paragraphs.push(setup);
        offset = 1;
    }
    paragraphs.extend(section.paragraphs[start..end].iter().cloned());

    // Source-to-output addresses for the kept body paragraphs and the tables they hold.
    let mut address_map = Vec::new();
    let mut source_table = doc
        .sections
        .iter()
        .take(section_index)
        .flat_map(|section| &section.paragraphs)
        .map(count_tables)
        .sum::<usize>()
        + section.paragraphs[..start]
            .iter()
            .map(count_tables)
            .sum::<usize>();
    // Output tables are numbered from the start of the fragment, which has one section.
    let mut output_table = paragraphs[..offset].iter().map(count_tables).sum::<usize>();
    for (position, source_index) in (start..end).enumerate() {
        let paragraph = &section.paragraphs[source_index];
        let output_index = offset + position;
        address_map.push(SourceToOutputAddress {
            kind: AddressKind::Paragraph,
            source: (section_index, source_index),
            output: (0, output_index),
            source_table: None,
            output_table: None,
        });
        let tables = count_tables(paragraph);
        for table in 0..tables {
            address_map.push(SourceToOutputAddress {
                kind: AddressKind::Table,
                source: (section_index, source_index),
                output: (0, output_index),
                source_table: Some(source_table + table),
                output_table: Some(output_table + table),
            });
        }
        source_table += tables;
        output_table += tables;
    }

    let mut fragment = doc.clone();
    fragment.sections = vec![Section {
        paragraphs,
        extras: section.extras.clone(),
    }];
    fragment.header.properties.section_count = 1;
    fragment.header.properties.caret = (0, 0, 0);

    // The closure runs over the fragment as built, with the whole header and the carried setup
    // controls in it. A reference to a binary the source lacks is refused here, still before
    // anything is written.
    let scan = scan_references(&fragment);
    let mut proven = BTreeSet::new();
    for reference in &scan.refs {
        match fragment.bin_stream_index(reference) {
            Some(index) => {
                proven.insert(index);
            }
            None => {
                return Err(RangeError::LostResource {
                    reference: match reference {
                        BinRef::Id(id) => format!("BinData #{}", id.0),
                        BinRef::ItemRef(item) => item.clone(),
                    },
                });
            }
        }
    }
    // Conservative: with an unresolvable reference site in the fragment, a stream that is not
    // proven used is not proven unused either, so it travels and the report says why.
    let keep_unproven = !scan.unverified.is_empty();
    let mut report = ResourceClosureReport {
        unverified: scan.unverified,
        ..ResourceClosureReport::default()
    };
    let mut streams = Vec::new();
    for (index, stream) in doc.bin_streams.iter().enumerate() {
        if proven.contains(&index) {
            report.kept.push(stream.name.clone());
            streams.push(stream.clone());
        } else if keep_unproven {
            report.kept_unverified.push(stream.name.clone());
            streams.push(stream.clone());
        } else {
            report.excluded.push(stream.name.clone());
        }
    }
    fragment.bin_streams = streams;
    // A preview rendered from the whole source would show content the fragment does not have.
    fragment.hwpx_preview_image = None;

    Ok(RangeOutcome {
        fragment,
        address_map,
        resources: report,
        section: section_index,
        start,
        end,
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
        set_section_page_start(doc, 0, paragraph_index, text_start);
    }

    fn set_section_page_start(
        doc: &mut Document,
        section_index: usize,
        paragraph_index: usize,
        text_start: u32,
    ) {
        doc.sections[section_index].paragraphs[paragraph_index]
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

    /// A page start on every section's first paragraph: page 1 = section 0,
    /// page 2 = section 1, page 3 = section 2.
    fn paged_three_section_doc() -> Document {
        let mut doc = three_section_doc();
        set_section_page_start(&mut doc, 0, 0, 0);
        set_section_page_start(&mut doc, 1, 0, 0);
        set_section_page_start(&mut doc, 2, 0, 0);
        doc
    }

    #[test]
    fn 구역을_넘는_페이지_범위는_중간_구역을_모두_담는다() {
        let doc = paged_three_section_doc();

        // Pages 1-2 must carry section 1 whole — the single-section slice
        // used to drop it silently (issue #162).
        let outcome = split_page_ranges(&doc, &[PageRange { first: 1, last: 2 }]).unwrap();
        assert!(outcome.roundings.is_empty());
        assert_eq!(outcome.fragments.len(), 1);
        let fragment = &outcome.fragments[0];
        // The range ends exactly at section 2's start, so section 2 is not
        // part of the fragment at all.
        assert_eq!(fragment.sections.len(), 2);
        assert_eq!(fragment.header.properties.section_count, 2);
        let text = plain_text(fragment);
        assert!(text.contains("첫 구역"));
        assert!(text.contains("둘째 구역"));
        assert!(!text.contains("셋째 구역"));
    }

    #[test]
    fn 마지막_페이지까지의_범위는_뒤따르는_구역을_버리지_않는다() {
        let doc = paged_three_section_doc();

        // A range running to the last page must reach the end of the last
        // section, not the end of the start section.
        let outcome = split_page_ranges(&doc, &[PageRange { first: 1, last: 3 }]).unwrap();
        assert!(outcome.roundings.is_empty());
        let fragment = &outcome.fragments[0];
        assert_eq!(fragment.sections.len(), 3);
        assert_eq!(fragment.header.properties.section_count, 3);
        let text = plain_text(fragment);
        assert!(text.contains("첫 구역"));
        assert!(text.contains("둘째 구역"));
        assert!(text.contains("셋째 구역"));
    }

    #[test]
    fn 구역_중간에서_시작하는_범위도_뒤_구역을_담고_섹션정의를_보존한다() {
        let mut doc = crate::from_markdown("구역1 첫째\n\n구역1 둘째\n");
        doc.sections
            .push(crate::from_markdown("구역2 첫째\n").sections[0].clone());
        // Page 1 = section 0 paragraph 0; page 2 = section 0 paragraph 1
        // (mid-section start); page 3 = section 1 paragraph 0.
        set_section_page_start(&mut doc, 0, 0, 0);
        set_section_page_start(&mut doc, 0, 1, 0);
        set_section_page_start(&mut doc, 1, 0, 0);

        let outcome = split_page_ranges(&doc, &[PageRange { first: 2, last: 3 }]).unwrap();
        assert!(outcome.roundings.is_empty());
        let fragment = &outcome.fragments[0];
        assert_eq!(fragment.sections.len(), 2);
        assert_eq!(fragment.header.properties.section_count, 2);
        // The first section starts mid-way, so its SectionDef paragraph must
        // be prepended (that paragraph carries the section's first text in
        // this IR — the prepend is a duplicate, not a move); the second
        // section keeps its own first paragraph.
        assert!(has_section_def(&fragment.sections[0].paragraphs[0]));
        assert!(has_section_def(&fragment.sections[1].paragraphs[0]));
        assert_eq!(fragment.sections[0].paragraphs.len(), 2);
        let text = plain_text(fragment);
        assert!(text.contains("구역1 둘째"));
        assert!(text.contains("구역2 첫째"));
    }

    #[test]
    fn 구역_끝에_걸친_페이지_경계는_다음_구역_첫_문단으로_보정된다() {
        let mut doc = three_section_doc();
        let last_of_section0 = doc.sections[0].paragraphs.len() - 1;
        set_section_page_start(&mut doc, 0, 0, 0);
        // Page 2 starts strictly inside section 0's last paragraph — the
        // boundary must round forward to section 1's first paragraph.
        set_section_page_start(&mut doc, 0, last_of_section0, 3);

        let outcome = split_page_ranges(&doc, &[PageRange { first: 1, last: 1 }]).unwrap();
        assert_eq!(outcome.roundings.len(), 1);
        assert_eq!(outcome.roundings[0].from.section, 0);
        assert_eq!(outcome.roundings[0].from.paragraph, last_of_section0);
        assert_eq!(outcome.roundings[0].from.wchar_offset, 3);
        assert_eq!(outcome.roundings[0].to.section, 1);
        assert_eq!(outcome.roundings[0].to.paragraph, 0);
        // The straddling paragraph stays whole in the earlier fragment; the
        // next section's content belongs to the following page range.
        let fragment = &outcome.fragments[0];
        assert_eq!(fragment.sections.len(), 1);
        assert!(plain_text(fragment).contains("첫 구역"));
        assert!(!plain_text(fragment).contains("둘째 구역"));
    }

    // ───────────── typed range extraction (#417) ─────────────

    fn plain_text_of(paragraph: &Paragraph) -> String {
        paragraph_text(paragraph)
    }

    /// A body paragraph with only `text`: the markdown converter puts the section setup
    /// controls on the first paragraph of a document, so they are removed here and only
    /// `range_doc`'s first paragraph keeps them.
    fn text_paragraph(text: &str) -> Paragraph {
        let mut paragraph =
            crate::from_markdown(&format!("{text}\n")).sections[0].paragraphs[0].clone();
        paragraph
            .chars
            .retain(|ch| !matches!(ch, HwpChar::ExtCtrl { .. }));
        paragraph.controls.clear();
        paragraph
    }

    fn picture_paragraph(reference: &str) -> Paragraph {
        let mut paragraph = text_paragraph("");
        paragraph
            .controls
            .push(Control::Picture(hwp_model::Picture {
                common_data: Vec::new(),
                width: hwp_model::HwpUnit(100),
                height: hwp_model::HwpUnit(100),
                treat_as_char: true,
                z_order: 0,
                vert_offset: 0,
                horz_offset: 0,
                description: None,
                crop: None,
                flip: 0,
                rotation: None,
                brightness: 0,
                contrast: 0,
                effect_flags: 0,
                effects_raw: Vec::new(),
                caption: None,
                bin_ref: BinRef::ItemRef(reference.to_string()),
                extras: Vec::new(),
            }));
        paragraph
    }

    /// Section 0: `[setup, A, a-body, B, b-picture(b.png), C, c-picture(c.png)]`, two streams
    /// plus an unreferenced third one.
    fn range_doc() -> Document {
        let mut doc = crate::from_markdown("setup\n");
        let section = &mut doc.sections[0];
        section.paragraphs.truncate(1);
        for paragraph in [
            text_paragraph("A"),
            text_paragraph("a body"),
            text_paragraph("B"),
            picture_paragraph("b.png"),
            text_paragraph("C"),
            picture_paragraph("c.png"),
        ] {
            section.paragraphs.push(paragraph);
        }
        doc.bin_streams = ["b.png", "c.png", "unused.png"]
            .iter()
            .map(|name| hwp_model::BinStream {
                name: format!("BinData/{name}"),
                data: name.as_bytes().to_vec(),
            })
            .collect();
        doc
    }

    fn heading(text: &str) -> RangeAnchor {
        RangeAnchor::Heading(text.to_string())
    }

    fn selector(
        start: RangeAnchor,
        end: Option<RangeAnchor>,
        end_inclusive: bool,
    ) -> RangeSelector {
        RangeSelector {
            start,
            end,
            end_inclusive,
            source_sha256: None,
        }
    }

    #[test]
    fn 범위는_시작부터_끝_문단까지_포함하고_주소를_원본에서_출력으로_옮긴다() {
        let doc = range_doc();
        let outcome =
            extract_range(&doc, "h", &selector(heading("A"), Some(heading("B")), true)).unwrap();
        assert_eq!((outcome.section, outcome.start, outcome.end), (0, 1, 4));
        // setup paragraph + A + a body + B
        assert_eq!(outcome.fragment.sections.len(), 1);
        assert_eq!(outcome.fragment.sections[0].paragraphs.len(), 4);
        // The range does not start at the section's first paragraph, so the section setup
        // rides on a carried-in setup paragraph with none of the original's text.
        let carried = &outcome.fragment.sections[0].paragraphs[0];
        assert!(has_section_def(carried));
        assert!(!plain_text_of(carried).contains("setup"));
        let moved: Vec<_> = outcome
            .address_map
            .iter()
            .map(|a| (a.source.1, a.output.1))
            .collect();
        assert_eq!(moved, vec![(1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn 끝_제외는_끝_문단을_범위에서_뺀다() {
        let doc = range_doc();
        let outcome = extract_range(
            &doc,
            "h",
            &selector(heading("A"), Some(heading("B")), false),
        )
        .unwrap();
        assert_eq!((outcome.start, outcome.end), (1, 3));
    }

    #[test]
    fn 끝이_없으면_시작_구역의_끝까지다() {
        let doc = range_doc();
        let outcome = extract_range(&doc, "h", &selector(heading("B"), None, true)).unwrap();
        assert_eq!((outcome.start, outcome.end), (3, 7));
    }

    #[test]
    fn 범위_안_그림의_바이너리만_유지하고_나머지는_제외로_보고한다() {
        let doc = range_doc();
        let outcome = extract_range(
            &doc,
            "h",
            &selector(heading("B"), Some(heading("C")), false),
        )
        .unwrap();
        assert_eq!(outcome.resources.kept, vec!["BinData/b.png"]);
        assert_eq!(
            outcome.resources.excluded,
            vec!["BinData/c.png", "BinData/unused.png"]
        );
        let names: Vec<_> = outcome
            .fragment
            .bin_streams
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["BinData/b.png"]);
        assert_eq!(outcome.fragment.bin_streams[0].data, b"b.png");
    }

    #[test]
    fn 범위_밖_그림만_있는_조각은_바이너리를_하나도_갖지_않는다() {
        let doc = range_doc();
        let outcome = extract_range(
            &doc,
            "h",
            &selector(heading("A"), Some(heading("a body")), true),
        )
        .unwrap();
        assert!(outcome.fragment.bin_streams.is_empty());
        assert!(outcome.resources.kept.is_empty());
        assert_eq!(outcome.resources.excluded.len(), 3);
    }

    #[test]
    fn 원본에_없는_바이너리를_참조하면_아무것도_만들지_않고_거부한다() {
        let mut doc = range_doc();
        doc.bin_streams.retain(|s| s.name != "BinData/b.png");
        let error = extract_range(
            &doc,
            "h",
            &selector(heading("B"), Some(heading("C")), false),
        )
        .unwrap_err();
        assert_eq!(
            error,
            RangeError::LostResource {
                reference: "b.png".into()
            }
        );
    }

    #[test]
    fn 선택_해시가_다르면_낡은_원본으로_거부한다() {
        let doc = range_doc();
        let mut chosen = selector(heading("A"), None, true);
        chosen.source_sha256 = Some("ABCD".into());
        let error = extract_range(&doc, "abcd", &chosen);
        assert!(error.is_ok(), "hash comparison ignores hex case");
        chosen.source_sha256 = Some("abce".into());
        let error = extract_range(&doc, "abcd", &chosen).unwrap_err();
        assert!(matches!(error, RangeError::StaleSource { .. }));
    }

    #[test]
    fn 없는_제목과_중복_제목과_빈_제목은_앵커로_거부한다() {
        let mut doc = range_doc();
        let missing = extract_range(&doc, "h", &selector(heading("없음"), None, true));
        assert!(matches!(missing, Err(RangeError::AnchorMissing { .. })));
        let empty = extract_range(&doc, "h", &selector(heading("  "), None, true));
        assert!(matches!(empty, Err(RangeError::AnchorMissing { .. })));
        doc.sections[0].paragraphs.push(text_paragraph("A"));
        match extract_range(&doc, "h", &selector(heading("A"), None, true)) {
            Err(RangeError::AnchorAmbiguous { matches, .. }) => {
                assert_eq!(matches, vec![(0, 1), (0, 7)]);
            }
            other => panic!("expected an ambiguous anchor, got {other:?}"),
        }
    }

    #[test]
    fn 범위_밖_주소와_구역_밖_주소는_없는_앵커다() {
        let doc = range_doc();
        for (section, paragraph) in [(0, 99), (3, 0)] {
            let result = extract_range(
                &doc,
                "h",
                &selector(RangeAnchor::Address { section, paragraph }, None, true),
            );
            assert!(matches!(result, Err(RangeError::AnchorMissing { .. })));
        }
    }

    #[test]
    fn 시작과_끝이_다른_구역이면_거부한다() {
        let mut doc = range_doc();
        doc.sections.push(Section {
            paragraphs: vec![text_paragraph("Z")],
            extras: Vec::new(),
        });
        let error = extract_range(&doc, "h", &selector(heading("A"), Some(heading("Z")), true))
            .unwrap_err();
        assert_eq!(error, RangeError::AcrossSections { start: 0, end: 1 });
    }

    #[test]
    fn 끝이_시작보다_앞이거나_빈_범위면_거부한다() {
        let doc = range_doc();
        let reversed = extract_range(&doc, "h", &selector(heading("C"), Some(heading("A")), true));
        assert_eq!(reversed.unwrap_err(), RangeError::EndBeforeStart);
        let empty = extract_range(
            &doc,
            "h",
            &selector(heading("A"), Some(heading("A")), false),
        );
        assert_eq!(empty.unwrap_err(), RangeError::EndBeforeStart);
    }

    #[test]
    fn 범위가_첫_문단에서_시작하면_구역_설정_문단을_따로_붙이지_않는다() {
        let doc = range_doc();
        let outcome = extract_range(
            &doc,
            "h",
            &selector(
                RangeAnchor::Address {
                    section: 0,
                    paragraph: 0,
                },
                Some(heading("A")),
                true,
            ),
        )
        .unwrap();
        assert_eq!(outcome.fragment.sections[0].paragraphs.len(), 2);
        assert_eq!(outcome.address_map[0].output, (0, 0));
    }

    #[test]
    fn 원본_문서는_추출해도_바뀌지_않는다() {
        let doc = range_doc();
        let before = format!("{:?}", doc.sections);
        let streams = doc.bin_streams.len();
        extract_range(&doc, "h", &selector(heading("B"), None, true)).unwrap();
        assert_eq!(format!("{:?}", doc.sections), before);
        assert_eq!(doc.bin_streams.len(), streams);
    }

    fn generic(ctrl_id: &[u8; 4]) -> hwp_model::GenericControl {
        hwp_model::GenericControl {
            ctrl_id: *ctrl_id,
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }
    }

    /// Appends `control` to `paragraph` with the extended-control char that places it.
    fn attach(paragraph: &mut Paragraph, ctrl_id: [u8; 4], control: Control) {
        attach_with_code(paragraph, 11, ctrl_id, control);
    }

    fn attach_with_code(paragraph: &mut Paragraph, code: u16, ctrl_id: [u8; 4], control: Control) {
        let index = paragraph.controls.len() as u32;
        paragraph.controls.push(control);
        paragraph.chars.insert(
            0,
            HwpChar::ExtCtrl {
                code,
                ctrl_id,
                payload: Vec::new(),
                ctrl_index: Some(index),
            },
        );
    }

    fn opaque(tag: u16) -> hwp_model::OpaqueRecord {
        hwp_model::OpaqueRecord {
            tag,
            data: vec![0; 4],
            children: Vec::new(),
        }
    }

    /// `range_doc` with `site` applied to the body paragraph `host` (1-based body index of the
    /// range, `0` meaning the first paragraph of the section), then the range `B` only
    /// extracted. `b.png` is referenced by the picture in the range, `c.png` and `unused.png`
    /// by nothing the closure can read.
    fn extract_b_with(site: impl FnOnce(&mut Document)) -> RangeOutcome {
        let mut doc = range_doc();
        site(&mut doc);
        extract_range(
            &doc,
            "h",
            &selector(heading("B"), Some(heading("C")), false),
        )
        .unwrap()
    }

    #[test]
    fn 쪽_배경_그림_채움이_있으면_쓰이지_않는다고_증명할_수_없는_바이너리를_남긴다() {
        let mut existing = 0;
        let outcome = extract_b_with(|doc| {
            existing = doc.header.border_fills.len();
            doc.header.border_fills.push(hwp_model::BorderFill {
                fill_type: 0x2,
                ..Default::default()
            });
        });
        assert_eq!(outcome.resources.kept, vec!["BinData/b.png"]);
        assert_eq!(
            outcome.resources.kept_unverified,
            vec!["BinData/c.png", "BinData/unused.png"]
        );
        assert!(outcome.resources.excluded.is_empty());
        assert_eq!(
            outcome.resources.unverified,
            vec![UnverifiedSite {
                code: "border_fill_image",
                at: format!("header/border_fills/{existing}")
            }]
        );
        assert_eq!(outcome.fragment.bin_streams.len(), 3);
    }

    #[test]
    fn 범위_안_묶음_개체의_원본_레코드는_바이너리를_지우지_않는다() {
        for (tag, code) in [
            (TAG_SHAPE_COMPONENT, "raw_shape"),
            (TAG_SHAPE_COMPONENT_OLE, "raw_ole"),
            (TAG_SHAPE_COMPONENT_PICTURE, "raw_picture"),
        ] {
            let outcome = extract_b_with(|doc| {
                // The grouped object sits in the picture paragraph of the range.
                let mut object = generic(b"gso ");
                object.raw_children.push(hwp_model::OpaqueRecord {
                    tag: 0,
                    data: Vec::new(),
                    children: vec![opaque(tag)],
                });
                attach(
                    &mut doc.sections[0].paragraphs[4],
                    *b"gso ",
                    Control::Generic(object),
                );
            });
            assert!(
                outcome.resources.excluded.is_empty(),
                "{code}: a stream the object may use was excluded"
            );
            assert_eq!(outcome.fragment.bin_streams.len(), 3, "{code}");
            assert_eq!(outcome.resources.unverified.len(), 1, "{code}");
            assert_eq!(outcome.resources.unverified[0].code, code);
            assert_eq!(outcome.resources.unverified[0].at, "s0/p2");
        }
    }

    #[test]
    fn 원문_xml_개체의_binary_item_id_ref는_읽어서_유지한다() {
        let outcome = extract_b_with(|doc| {
            let mut object = generic(b"con ");
            object.hwpx_raw_xml = Some(
                r#"<hp:container><hp:pic><hc:img binaryItemIDRef="c.png"/></hp:pic></hp:container>"#
                    .into(),
            );
            attach(
                &mut doc.sections[0].paragraphs[4],
                *b"con ",
                Control::Generic(object),
            );
        });
        assert_eq!(
            outcome.resources.kept,
            vec!["BinData/b.png", "BinData/c.png"]
        );
        assert_eq!(outcome.resources.excluded, vec!["BinData/unused.png"]);
        assert!(outcome.resources.unverified.is_empty());
    }

    #[test]
    fn 읽을_수_없는_원문_xml은_추측하지_않고_모두_남긴다() {
        let outcome = extract_b_with(|doc| {
            let mut object = generic(b"con ");
            object.hwpx_raw_xml = Some("<hp:container><hp:pic".into());
            attach(
                &mut doc.sections[0].paragraphs[4],
                *b"con ",
                Control::Generic(object),
            );
        });
        assert!(outcome.resources.excluded.is_empty());
        assert_eq!(outcome.resources.unverified[0].code, "raw_xml_unparsed");
        assert_eq!(outcome.fragment.bin_streams.len(), 3);
    }

    #[test]
    fn 앞에_붙는_머리말의_그림은_범위_밖에_있어도_유지한다() {
        // The section's first paragraph carries a header whose paragraph holds c.png. The range
        // starts later, so that paragraph rides in front as the section setup.
        let outcome = extract_b_with(|doc| {
            let mut head = generic(b"head");
            head.paragraph_lists.push(hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![picture_paragraph("c.png")],
            });
            attach(
                &mut doc.sections[0].paragraphs[0],
                *b"head",
                Control::Generic(head),
            );
        });
        assert_eq!(
            outcome.resources.kept,
            vec!["BinData/b.png", "BinData/c.png"]
        );
        assert_eq!(outcome.resources.excluded, vec!["BinData/unused.png"]);
    }

    #[test]
    fn 개체_캡션_안의_그림은_유지한다() {
        let outcome = extract_b_with(|doc| {
            let mut object = generic(b"gso ");
            object.caption = Some(hwp_model::Caption {
                side: hwp_model::CaptionSide::Bottom,
                direction: hwp_model::CaptionDirection::Horizontal,
                gap: 0,
                width: None,
                last_width: 0,
                paragraphs: vec![picture_paragraph("c.png")],
            });
            attach(
                &mut doc.sections[0].paragraphs[4],
                *b"gso ",
                Control::Generic(object),
            );
        });
        assert_eq!(
            outcome.resources.kept,
            vec!["BinData/b.png", "BinData/c.png"]
        );
    }

    #[test]
    fn 그림_자신의_도형_레코드는_추가_불확실성이_아니다() {
        // A modeled picture keeps its shape and picture components as opaque extras. Its
        // bin_ref already says which stream it uses, so those must not turn every other
        // stream into "unverified".
        let outcome = extract_b_with(|doc| {
            if let Control::Picture(picture) = &mut doc.sections[0].paragraphs[4].controls[0] {
                picture.extras = vec![
                    opaque(TAG_SHAPE_COMPONENT),
                    opaque(TAG_SHAPE_COMPONENT_PICTURE),
                ];
            } else {
                panic!("paragraph 4 holds the picture");
            }
        });
        assert!(outcome.resources.unverified.is_empty());
        assert_eq!(
            outcome.resources.excluded,
            vec!["BinData/c.png", "BinData/unused.png"]
        );
    }

    #[test]
    fn 범위_밖의_불확실한_자리는_조각에_없으므로_영향이_없다() {
        // The grouped object is in the C paragraph, outside the B-only range.
        let outcome = extract_b_with(|doc| {
            let mut object = generic(b"gso ");
            object.raw_children.push(opaque(TAG_SHAPE_COMPONENT_OLE));
            attach(
                &mut doc.sections[0].paragraphs[6],
                *b"gso ",
                Control::Generic(object),
            );
        });
        assert!(outcome.resources.unverified.is_empty());
        assert_eq!(outcome.resources.kept, vec!["BinData/b.png"]);
    }

    // ───────────── T9-F8..F12, nits ─────────────

    /// A shape component record: `count` transform pairs, then a border block whose fill type
    /// is `fill_type`, laid out the way `shape_component_has_image_fill` reads it.
    fn shape_component(fill_type: Option<u32>) -> hwp_model::OpaqueRecord {
        let mut data = vec![0u8; 8 + 42];
        data[0..4].copy_from_slice(b"cip$");
        data[4..8].copy_from_slice(b"cip$");
        data.extend_from_slice(&1u16.to_le_bytes()); // one transform pair, at +42 from base 8
        data.resize(8 + 92 + 96, 0); // the matrices end where the border block begins
        if let Some(fill_type) = fill_type {
            data.resize(8 + 92 + 96 + 13, 0);
            data.extend_from_slice(&fill_type.to_le_bytes());
        }
        hwp_model::OpaqueRecord {
            tag: TAG_SHAPE_COMPONENT,
            data,
            children: Vec::new(),
        }
    }

    #[test]
    fn 범위_안_그림의_도형_레코드가_그림_채움을_가리키면_미검증으로_남긴다() {
        let outcome = extract_b_with(|doc| {
            if let Control::Picture(picture) = &mut doc.sections[0].paragraphs[4].controls[0] {
                picture.extras = vec![shape_component(Some(0x2))];
            }
        });
        assert!(outcome.resources.excluded.is_empty());
        assert_eq!(outcome.resources.unverified[0].code, "raw_shape");
        assert_eq!(outcome.fragment.bin_streams.len(), 3);
    }

    #[test]
    fn 범위_안_그림의_도형_레코드가_채움_블록에_닿지_않거나_채움이_그림이_아니면_불확실하지_않다() {
        for record in [
            shape_component(None),      // ends at the border block, like the genuine files
            shape_component(Some(0x1)), // solid fill
            shape_component(Some(0x4)), // gradient
        ] {
            let outcome = extract_b_with(|doc| {
                if let Control::Picture(picture) = &mut doc.sections[0].paragraphs[4].controls[0] {
                    picture.extras = vec![record.clone()];
                }
            });
            assert!(
                outcome.resources.unverified.is_empty(),
                "{:?}",
                record.data.len()
            );
            assert_eq!(
                outcome.resources.excluded,
                vec!["BinData/c.png", "BinData/unused.png"]
            );
        }
    }

    #[test]
    fn 모양_레코드_판독은_짧거나_이상한_레코드에서_멈추고_패닉하지_않는다() {
        for data in [vec![], vec![1, 2, 3], vec![0; 7], vec![0; 8], vec![0; 49]] {
            assert!(!shape_component_has_image_fill(&data));
        }
        // A transform count that would put the border block past the end of the record.
        let mut huge = vec![0u8; 8 + 44];
        huge[8 + 42..8 + 44].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(!shape_component_has_image_fill(&huge));
    }

    #[test]
    fn 조각이_그대로_싣는_패키지_부품의_바이너리_참조도_센다() {
        let outcome = extract_b_with(|doc| {
            doc.hwpx_extra_entries.push((
                "Contents/masterpage0.xml".into(),
                br#"<masterPage><hc:img binaryItemIDRef="c.png"/></masterPage>"#.to_vec(),
            ));
        });
        assert_eq!(
            outcome.resources.kept,
            vec!["BinData/b.png", "BinData/c.png"]
        );
        assert_eq!(outcome.resources.excluded, vec!["BinData/unused.png"]);
    }

    #[test]
    fn 읽을_수_없는_패키지_부품은_모든_바이너리를_남기고_자리를_보고한다() {
        for (name, bytes) in [
            ("Scripts/macro.bin", vec![0xff, 0xfe, 0x00]),
            ("Contents/masterpage0.xml", b"<masterPage><hc:img".to_vec()),
        ] {
            let outcome = extract_b_with(|doc| {
                doc.hwpx_extra_entries.push((name.into(), bytes.clone()));
            });
            assert!(outcome.resources.excluded.is_empty(), "{name}");
            assert_eq!(outcome.resources.unverified.len(), 1, "{name}");
            assert_eq!(
                outcome.resources.unverified[0].at,
                format!("package/{name}")
            );
        }
    }

    #[test]
    fn 읽을_수_있는_정상_패키지_부품은_영향이_없다() {
        // The container and manifest parts every genuine package carries hold no reference.
        let outcome = extract_b_with(|doc| {
            for name in [
                "META-INF/container.xml",
                "META-INF/manifest.xml",
                "META-INF/container.rdf",
            ] {
                doc.hwpx_extra_entries
                    .push((name.into(), b"<container><rootfiles/></container>".to_vec()));
            }
        });
        assert!(outcome.resources.unverified.is_empty());
        assert_eq!(
            outcome.resources.excluded,
            vec!["BinData/c.png", "BinData/unused.png"]
        );
    }

    #[test]
    fn 매니페스트_id가_파일명과_달라도_그림의_바이너리를_찾는다() {
        let mut doc = range_doc();
        doc.bin_streams[0].name = "BinData/p1.png".into();
        doc.hwpx_bin_manifest = vec![("b.png".into(), "BinData/p1.png".into())];
        let outcome = extract_range(
            &doc,
            "h",
            &selector(heading("B"), Some(heading("C")), false),
        )
        .unwrap();
        assert_eq!(outcome.resources.kept, vec!["BinData/p1.png"]);
    }

    #[test]
    fn 출력에_실리지_않은_스트림은_보고서의_유지_목록에서_빠진다() {
        let mut report = ResourceClosureReport {
            kept: vec!["BinData/b.png".into()],
            kept_unverified: vec!["BinData/c.png".into(), "BinData/unused.png".into()],
            ..Default::default()
        };
        let fragment: Vec<_> = ["b.png", "c.png", "unused.png"]
            .iter()
            .map(|name| hwp_model::BinStream {
                name: format!("BinData/{name}"),
                data: name.as_bytes().to_vec(),
            })
            .collect();
        // The output format wrote only b.png; the others were orphans.
        let output = vec![hwp_model::BinStream {
            name: "BIN0001.png".into(),
            data: b"b.png".to_vec(),
        }];
        report.reconcile_with_output(&fragment, &output);
        assert_eq!(report.kept, vec!["BinData/b.png"]);
        assert!(report.kept_unverified.is_empty());
        assert_eq!(
            report.dropped_by_format,
            vec!["BinData/c.png", "BinData/unused.png"]
        );
    }

    #[test]
    fn 출력이_모두_싣고_있으면_보고서는_그대로다() {
        let fragment = vec![hwp_model::BinStream {
            name: "BinData/b.png".into(),
            data: b"b".to_vec(),
        }];
        let mut report = ResourceClosureReport {
            kept: vec!["BinData/b.png".into()],
            ..Default::default()
        };
        let before = report.clone();
        report.reconcile_with_output(&fragment, &fragment.clone());
        assert_eq!(report, before);
    }

    #[test]
    fn 같은_바이트의_스트림_둘은_출력에_하나뿐이면_하나만_빠진다() {
        let fragment: Vec<_> = ["a.png", "b.png"]
            .iter()
            .map(|name| hwp_model::BinStream {
                name: format!("BinData/{name}"),
                data: b"same".to_vec(),
            })
            .collect();
        let mut report = ResourceClosureReport {
            kept: vec!["BinData/a.png".into(), "BinData/b.png".into()],
            ..Default::default()
        };
        report.reconcile_with_output(&fragment, &fragment[..1]);
        assert_eq!(report.kept, vec!["BinData/a.png"]);
        assert_eq!(report.dropped_by_format, vec!["BinData/b.png"]);
    }

    #[test]
    fn 빈_책갈피_이름은_이름_없는_책갈피와도_맞지_않고_앵커로_거부한다() {
        let mut doc = range_doc();
        // A bookmark whose name decodes to the empty string, placed in the paragraph of "B".
        let mut bookmark = generic(b"bokm");
        bookmark.raw_children.push(hwp_model::OpaqueRecord {
            tag: crate::bookmark::CTRL_DATA_TAG,
            data: crate::bookmark::make_bokm_ctrl_data(""),
            children: Vec::new(),
        });
        attach_with_code(
            &mut doc.sections[0].paragraphs[3],
            hwp_model::ctrl_char::BOOKMARK,
            *b"bokm",
            Control::Generic(bookmark),
        );
        // Sanity: the empty name is what the document holds, and `holds_bookmark` finds it
        // when asked for the empty name directly (the guard is in anchor resolution).
        let holder = &doc.sections[0].paragraphs[3];
        assert_eq!(
            holder
                .controls
                .iter()
                .filter_map(crate::bookmark::bookmark_name)
                .collect::<Vec<_>>(),
            vec![String::new()]
        );
        assert!(holds_bookmark(holder, ""));
        let result = extract_range(
            &doc,
            "h",
            &selector(RangeAnchor::Bookmark(String::new()), None, true),
        );
        assert!(matches!(result, Err(RangeError::AnchorMissing { .. })));
    }

    #[test]
    fn 이름이_xml이_아닌_부품은_내용이_xml_같아도_읽지_않고_불확실로_둔다() {
        // `.bin` is not an XML part name, so its bytes are not interpreted even though they
        // happen to be valid UTF-8 with a reference inside.
        let outcome = extract_b_with(|doc| {
            doc.hwpx_extra_entries.push((
                "Scripts/looks-like-xml.bin".into(),
                br#"<a><hc:img binaryItemIDRef="c.png"/></a>"#.to_vec(),
            ));
        });
        assert!(outcome.resources.excluded.is_empty());
        assert_eq!(outcome.resources.unverified[0].code, "carried_part_unread");
        assert!(
            outcome
                .resources
                .kept_unverified
                .contains(&"BinData/c.png".to_string()),
            "{:?}",
            outcome.resources
        );
    }

    #[test]
    fn 앵커_종류는_본문_텍스트_없이_말한다() {
        assert_eq!(heading("비밀 제목").kind(), "heading");
        assert_eq!(RangeAnchor::Bookmark("비밀".into()).kind(), "bookmark");
        assert_eq!(
            RangeAnchor::Address {
                section: 0,
                paragraph: 1
            }
            .kind(),
            "address"
        );
    }

    #[test]
    fn 구역_없는_문서_거부는_다음_행동을_말한다() {
        let mut doc = range_doc();
        doc.sections.clear();
        let error = extract_range(&doc, "h", &selector(heading("A"), None, true)).unwrap_err();
        assert_eq!(error, RangeError::EmptyDocument);
        assert!(error.to_string().contains("hwp cat"), "{error}");
    }

    #[test]
    fn 거부_메시지는_원인과_다음_행동을_말한다() {
        let doc = range_doc();
        let cases = [
            extract_range(&doc, "h", &selector(heading("없음"), None, true)).unwrap_err(),
            extract_range(&doc, "h", &selector(heading("C"), Some(heading("A")), true))
                .unwrap_err(),
        ];
        for error in cases {
            let message = error.to_string();
            assert!(
                message.contains("hwp cat") || message.contains("--from"),
                "{message}"
            );
        }
    }
}
