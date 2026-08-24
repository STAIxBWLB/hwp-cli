//! Markdown → IR.
//!
//! Mapping: headings → "개요 N" styles, bold/italic/strikethrough → char-shape variants,
//! GFM tables → Table controls, ordered/bullet lists → head (NUMBER/BULLET) paragraphs,
//! footnotes/endnotes (`[^N]`/`[^eN]`) → fn/en controls, line breaks → CharCtrl(10).
//!
//! Symmetry with the exporter (markdown.rs) is the criterion for round-trip closure:
//! - strikethrough: the exporter reads `CharShape.strike`, so we create strike=true dedicated char shapes.
//! - footnotes/endnotes: the exporter reads `FOOTNOTE_ENDNOTE` ExtCtrl + the `paragraph_lists`
//!   of the `fn `/`en ` GenericControl, so we synthesize exactly that structure.
//! - lists: the exporter draws markers from `ParaShape.head_type()/head_level()/numbering_id`
//!   and `numbering_levels`/`bullet_chars`, so we create those definitions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hwp_model::{
    BinRef, BinStream, BorderFill, BorderFillId, BorderLine, Cell, CharShape, CharShapeId, Control,
    DocMeta, Document, FaceName, GenericControl, HwpChar, HwpUnit, LANG_COUNT, NumLevel, ParaShape,
    ParaShapeId, Paragraph, ParagraphList, Picture, Section, Style, StyleId, Table, ctrl_char,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::frames::FrameFields;
use crate::official::{self, OfficialPreset, PageMarginOverrides};

/// Number of base para shapes (indexes 0~4) created by default_header. List para shapes are
/// appended after these (5~). from_html's list para shapes use the same basis.
pub(crate) const BASE_PARA_SHAPES: u16 = 5;

/// Maximum authored list depth for official-document profiles.
pub(crate) const MAX_OFFICIAL_LIST_DEPTH: u16 = 8;

/// A rejected authored-list depth, shared by Markdown and embedded HTML importers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthoredListDepthError {
    pub observed: u16,
    pub maximum: u16,
}

impl std::fmt::Display for AuthoredListDepthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "authored list depth {} exceeds maximum {}",
            self.observed, self.maximum
        )
    }
}

/// Reject unsupported authored-list depth before creating shapes or definitions.
pub(crate) fn validate_official_list_depth(depth: usize) -> Result<u16, AuthoredListDepthError> {
    let observed = u16::try_from(depth).unwrap_or(u16::MAX);
    if observed > MAX_OFFICIAL_LIST_DEPTH {
        return Err(AuthoredListDepthError {
            observed,
            maximum: MAX_OFFICIAL_LIST_DEPTH,
        });
    }
    Ok(observed)
}

/// Normalizes an authored ordered-list start to the IR's persisted u32 range.
/// HTML and Markdown share this boundary so neither parser can silently narrow
/// a requested marker value.
pub(crate) fn normalize_authored_list_start(start: u64) -> Result<u32, String> {
    let start = start.max(1);
    u32::try_from(start).map_err(|_| {
        format!(
            "authored ordered-list start {start} exceeds maximum {}",
            u32::MAX
        )
    })
}

/// Rejects a list whose later item would require a marker outside the IR's
/// u32 number domain. The visible count is one-based.
pub(crate) fn validate_authored_list_item(start: u32, item: u32) -> Result<(), String> {
    let offset = item.saturating_sub(1);
    if start.checked_add(offset).is_none() {
        return Err(format!(
            "authored ordered-list start {start} with item {item} exceeds maximum {}",
            u32::MAX
        ));
    }
    Ok(())
}

/// Markdown import options.
#[derive(Default)]
pub struct MarkdownImportOptions<'a> {
    /// Base directory for resolving relative-path images (`![](fig.png)`) (location of the md file).
    /// If `None`, relative-path images keep only the alt text after a warning (absolute paths are tried as-is).
    pub base_dir: Option<&'a Path>,
    /// Sandbox roots binding image references (MCP `--root`, #56). Empty disables the check
    /// (CLI behavior — zero change). Roots must be canonical (the MCP server canonicalizes
    /// them at startup). With roots set, an image resolving outside every root is a hard
    /// error: the report variants fail the import instead of degrading to an alt-text warning.
    pub roots: &'a [PathBuf],
    /// Official-document preset — if set, adjusts page margins, fonts, numbering scheme, and
    /// page numbers to the regulation. `None` keeps the existing defaults (no change).
    pub preset: Option<OfficialPreset>,
    /// Side-specific page margins resolved after profile/plain defaults.
    pub page_margins: PageMarginOverrides,
    /// Document frames (`--doc-head`/`--doc-foot`/...), if any were supplied (GONG-03, D-01).
    /// `None`/empty keeps today's output unchanged.
    pub frames: Option<&'a FrameFields>,
}

#[cfg(test)]
mod official_depth_limit_tests {
    use super::{
        MarkdownImportOptions, OfficialPreset, from_markdown_report, normalize_authored_list_start,
        validate_authored_list_item,
    };

    fn nested_markdown(depth: usize) -> String {
        (0..depth)
            .map(|level| format!("{}1. level {}", "   ".repeat(level), level + 1))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn nested_html(depth: usize) -> String {
        let mut html = String::new();
        for level in 1..=depth {
            html.push_str(&format!("<ol><li>level {level}"));
        }
        for _ in 0..depth {
            html.push_str("</li></ol>");
        }
        html
    }

    fn official_options() -> MarkdownImportOptions<'static> {
        MarkdownImportOptions {
            preset: Some(OfficialPreset::Gian),
            ..Default::default()
        }
    }

    #[test]
    fn official_depth_limit() {
        assert!(from_markdown_report(&nested_markdown(8), &official_options()).is_ok());
        assert_eq!(
            from_markdown_report(&nested_markdown(9), &official_options()).unwrap_err(),
            "authored list depth 9 exceeds maximum 8"
        );
    }

    #[test]
    fn embedded_html_official_depth_limit() {
        assert!(from_markdown_report(&nested_html(8), &official_options()).is_ok());
        assert_eq!(
            from_markdown_report(&nested_html(9), &official_options()).unwrap_err(),
            "authored list depth 9 exceeds maximum 8"
        );
    }

    #[test]
    fn ordered_list_start_outside_u32_is_hard_error() {
        assert_eq!(
            normalize_authored_list_start(u64::from(u32::MAX)).unwrap(),
            u32::MAX
        );
        let error = normalize_authored_list_start(u64::from(u32::MAX) + 1)
            .expect_err("ordered-list start above u32 must fail");
        assert!(error.contains("exceeds maximum"), "{error}");
        assert!(validate_authored_list_item(u32::MAX, 1).is_ok());
        assert!(validate_authored_list_item(u32::MAX, 2).is_err());
    }

    #[test]
    fn empty_markdown_and_mixed_html_order_remain_valid() {
        let (empty, _) = from_markdown_report("", &official_options()).unwrap();
        assert_eq!(empty.sections[0].paragraphs.len(), 1);

        let input = "before\n\n<ol><li>inside</li></ol>\n\nafter\n";
        let (document, _) = from_markdown_report(input, &official_options()).unwrap();
        let text: String = document.sections[0]
            .paragraphs
            .iter()
            .flat_map(|paragraph| paragraph.chars.iter())
            .filter_map(|character| match character {
                hwp_model::HwpChar::Text(value) => Some(*value),
                _ => None,
            })
            .collect();
        let before = text.find("before").unwrap();
        let inside = text.find("inside").unwrap();
        let after = text.find("after").unwrap();
        assert!(before < inside && inside < after, "text order: {text}");
    }
}

/// Char shape ID layout (must match default_header). from_html uses the same palette.
pub(crate) mod shapes {
    pub const NORMAL: u16 = 0;
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 2;
    pub const BOLD_ITALIC: u16 = 3;
    /// H1~H6 → 4~9
    pub const HEADING_BASE: u16 = 4;
    /// Hyperlink display text (blue + underline)
    pub const HYPERLINK: u16 = 10;
    /// Strikethrough combinations (body/bold/italic/bold+italic + strike) → 11~14
    pub const STRIKE: u16 = 11;
    pub const BOLD_STRIKE: u16 = 12;
    pub const ITALIC_STRIKE: u16 = 13;
    pub const BOLD_ITALIC_STRIKE: u16 = 14;
    /// Inline code (HCR Dotum + light-gray shading) → 15
    pub const CODE: u16 = 15;
}

/// Index of HCR Dotum in the default_header font table (for inline code). HCR Batang=0.
const FONT_DOTUM: u16 = 1;

/// Border/fill ID layout: 1·2 = no border (default/reference), 3 = solid 0.12mm.
pub(crate) const TABLE_BORDER_FILL: u16 = 3;

/// Cell LIST_HEADER vertical alignment = center (bits5-6=1 → 0x20). Nearly all genuine Hancom
/// table cells (work_report·color_fill·corpus measurements) set this bit. Leaving it 0 (top)
/// makes the hwp5 writer emit it as-is, so cell content sticks to the top (top margin < bottom
/// margin). The hwpx writer emits vertAlign="CENTER" as a constant, so hwpx output is unaffected.
pub(crate) const CELL_VALIGN_CENTER: u32 = 0x20;

/// Body area width (A4 default margins, HWPUNIT). from_html's table width calculation uses the same basis.
pub(crate) const BODY_WIDTH: i32 = 42520;

/// Number of border/fill entries `default_header()` creates (`none, none, solid, blockquote,
/// code-block`). A new entry appended after `default_header()` returns is referenced as this
/// count's 1-based successor (Pitfall 1) — mirrors `BASE_PARA_SHAPES` for `header.border_fills`.
pub(crate) const BASE_BORDER_FILLS: u16 = 5;

/// The base "no spacing" table-cell `ParaShape` (index 0 of `default_header()`'s `para_shapes`,
/// shared by the default paragraph and every table cell). Extracted so `style::style_table` can
/// clone it as the base for centered header/narrow-column `ParaShape`s without needing the
/// fully-assembled document header, which does not exist yet while a table is being built during
/// markdown import.
pub(crate) fn table_cell_para_shape() -> ParaShape {
    ParaShape {
        attr1: 0x180,
        line_spacing_old: 160,
        border_fill_id: 2,
        line_spacing: 160,
        ..ParaShape::default()
    }
}

/// Default document header for `hwp new` — minimal configuration comparable to a blank Hancom document.
pub fn default_header() -> hwp_model::DocHeader {
    // Body HCR Batang 10pt (1000 HWPUNIT). Heading size = body × ratio (1800/1500/1300/1200/1100/1100).
    let body = 1000;
    let h = |factor: i32| (body * factor) / 100;
    let mut header = hwp_model::DocHeader::default();
    for slot in 0..LANG_COUNT {
        header.fonts[slot] = vec![
            FaceName {
                name: "함초롬바탕".to_string(),
                // Hancom's integrity check expects a default font name (attr bit5, 0x20) for font
                // substitution. The healthy sample hello_world.hwp's 'HCR Batang' has
                // default_name="HCR Batang", attr=0x21. attr low 0x01 = font type TTF (table 20).
                // emit_face_name automatically ORs the 0x20 bit.
                attr: 0x01,
                default_name: Some("HCR Batang".to_string()),
                ..FaceName::default()
            },
            // Index 1 = HCR Dotum (gothic/sans-serif) — for inline code. The renderer also draws
            // real glyphs with the bundled fonts/ HCRDotum. Both writers (hwp5 emit_face_name
            // loop, hwpx write_fontfaces) emit the whole per-slot fonts and the ID_MAPPINGS count
            // is also derived from len, so they stay consistent.
            FaceName {
                name: "함초롬돋움".to_string(),
                attr: 0x01,
                default_name: Some("HCR Dotum".to_string()),
                ..FaceName::default()
            },
        ];
    }

    let base = CharShape {
        base_size: 1000,
        ratios: [100; LANG_COUNT],
        rel_sizes: [100; LANG_COUNT],
        // The shade color (shade_color) must be 0xFFFFFFFF = 'none' marker. The default 0 is
        // interpreted by Hancom as 'opaque black shading (character background highlight)',
        // drawing a black bar on every character cell so the (black) text becomes invisible on
        // top of it — the cause of the 'black bar' in round-14 field testing. Healthy samples
        // (ganada.hwp 5.1.1.0, hello_world.hwp 5.1.0.1) all have shade_color=0xFFFFFFFF,
        // shadow_gap=(10,10), shadow_color≈0xC0C0C0.
        // (face_id=0 is harmless — hello_world also has char_shape[0].face_ids=0 and renders fine.)
        shade_color: 0xFFFF_FFFF,
        shadow_color: 0x00C0_C0C0,
        shadow_gap: (10, 10),
        ..CharShape::default()
    };
    let cs = |size: i32, bold: bool, italic: bool| CharShape {
        base_size: size,
        attr: u32::from(bold) << 1 | u32::from(italic),
        ..base.clone()
    };
    header.char_shapes = vec![
        cs(body, false, false),  // 0 body
        cs(body, true, false),   // 1 bold
        cs(body, false, true),   // 2 italic
        cs(body, true, true),    // 3 bold+italic
        cs(h(180), true, false), // 4 H1
        cs(h(150), true, false), // 5 H2
        cs(h(130), true, false), // 6 H3
        cs(h(120), true, false), // 7 H4
        cs(h(110), true, false), // 8 H5
        cs(h(110), true, false), // 9 H6
        // 10 hyperlink: blue (COLORREF 0x00BBGGRR=RGB(0,0,255)) + underline type 1.
        // Same rule as field.rs::hyperlink_char_shape — required for Hancom to recognize/display it as a link.
        CharShape {
            base_size: body,
            text_color: 0x00FF_0000,
            underline_color: 0x00FF_0000,
            attr: 1 << 2,
            ..base.clone()
        },
    ];
    // 11~14 strikethrough combinations. The exporter (markdown.rs) detects strikethrough via
    // CharShape.strike (explicit flag), so a strike=true dedicated char shape is needed for `~~`
    // to round-trip. hwp5 does not write strike as a byte (no effect); hwpx emits <hh:strikeout SOLID>.
    let cs_strike = |bold: bool, italic: bool| CharShape {
        base_size: body,
        attr: u32::from(bold) << 1 | u32::from(italic),
        strike: true,
        ..base.clone()
    };
    header.char_shapes.push(cs_strike(false, false)); // 11 strikethrough
    header.char_shapes.push(cs_strike(true, false)); // 12 bold+strikethrough
    header.char_shapes.push(cs_strike(false, true)); // 13 italic+strikethrough
    header.char_shapes.push(cs_strike(true, true)); // 14 bold+italic+strikethrough
    // 15 inline code: HCR Dotum (face_id=1) + light-gray shading (0xF0F0F0). Hancom draws
    // shade_color as a character background highlight, giving code spans a gray background
    // (contrast with 0xFFFFFFFF='none').
    header.char_shapes.push(CharShape {
        base_size: body,
        face_ids: [FONT_DOTUM; LANG_COUNT],
        shade_color: 0x00F0_F0F0,
        ..base.clone()
    });

    // Tab definitions — Hancom's 3 default left/center/right auto tabs. Healthy samples
    // (hello_world etc. 5.1.0.1) all have these 3, and every PARA_SHAPE references tab_def_id=0.
    // Leaving it empty creates a dangling reference and Hancom rejects the file as
    // 'corrupted/tampered'.
    // 8 bytes each: attr u32(0/1/2) + count i16=0 + reserved u16 (spec table 36, count=0→8B).
    header.tab_defs = vec![
        hwp_model::RawEntry {
            data: vec![0, 0, 0, 0, 0, 0, 0, 0],
            children: Vec::new(),
        },
        hwp_model::RawEntry {
            data: vec![1, 0, 0, 0, 0, 0, 0, 0],
            children: Vec::new(),
        },
        hwp_model::RawEntry {
            data: vec![2, 0, 0, 0, 0, 0, 0, 0],
            children: Vec::new(),
        },
    ];
    // Semantic copy used by the HWPX writer/renderer. Fills left/center/right auto tab attrs in
    // the same order as the raw TAB_DEF so user meaning is not lost to defaults on cross-save.
    header.tab_stops = vec![
        hwp_model::TabDef {
            attr: 0,
            items: Vec::new(),
        },
        hwp_model::TabDef {
            attr: 1,
            items: Vec::new(),
        },
        hwp_model::TabDef {
            attr: 2,
            items: Vec::new(),
        },
    ];

    // 0 default/table cell (justify, no spacing), 1 heading (left + top/bottom spacing),
    // 2 body (justify + bottom spacing).
    //
    // Body paragraphs get bottom spacing (spacing_bottom) so md output looks like a real document
    // with separated paragraphs. Table cells use 0 (no spacing) so cells do not grow
    // unnecessarily — flush_paragraph_inner distinguishes the two by the presence of self.table.
    //
    // PARA_SHAPE[0] of healthy samples (ganada.hwp 5.1.1.0, hello_world.hwp 5.1.0.1) is
    // attr1=0x180 (bit7 Hangul line-break=character + bit8 use line grid), line_spacing_old=160,
    // border_fill_id=2. These are the reference values for when Hancom recomputes body line
    // layout; 0 (our previous value) makes the line grid/line-break basis diverge from healthy
    // samples. The direct cause of the black bar is the char_shape shade color, but we match the
    // healthy sample bytes so Hancom stays safe when it re-lays out lines. (BodyText's
    // PARA_LINE_SEG cache is filled by the synthesizer.)
    let base_para = table_cell_para_shape();
    header.para_shapes = vec![
        base_para.clone(),
        ParaShape {
            attr1: 0x180 | (1 << 2), // healthy attr1 + left align
            spacing_top: 600,
            spacing_bottom: 300,
            ..base_para.clone()
        },
        ParaShape {
            spacing_bottom: 600, // body paragraph bottom spacing
            ..base_para.clone()
        },
        // 3 blockquote: left indent + left bar (border_fill 1-based id 4).
        ParaShape {
            attr1: 0x180 | (1 << 2),
            margin_left: 3000,
            border_fill_id: 4,
            spacing_top: 300,
            spacing_bottom: 300,
            ..base_para.clone()
        },
        // 4 code block: left/right indent + gray background (border_fill 1-based id 5).
        ParaShape {
            attr1: 0x180 | (1 << 2),
            margin_left: 2500,
            margin_right: 2500,
            border_fill_id: 5,
            spacing_top: 300,
            spacing_bottom: 300,
            ..base_para
        },
    ];

    header.styles = vec![Style {
        name: "바탕글".to_string(),
        english_name: "Normal".to_string(),
        ..Style::default()
    }];
    for n in 1..=6u16 {
        header.styles.push(Style {
            name: format!("개요 {n}"),
            english_name: format!("Outline {n}"),
            para_shape: ParaShapeId(1),
            char_shape: CharShapeId(shapes::HEADING_BASE + n - 1),
            ..Style::default()
        });
    }

    let none = BorderFill {
        diagonal: BorderLine {
            line_type: 1,
            width: 0,
            color: 0,
        },
        ..BorderFill::default()
    };
    let solid_line = BorderLine {
        line_type: 1,
        width: 1,
        color: 0,
    }; // solid 0.12mm black
    header.border_fills = vec![
        none.clone(),
        none,
        BorderFill {
            sides: [solid_line; 4],
            diagonal: BorderLine {
                line_type: 1,
                width: 0,
                color: 0,
            },
            ..BorderFill::default()
        },
        // 3 (1-based id 4) blockquote: left gray bar (1.5mm), other sides none. Hancom draws hwpx
        // paragraph borders thinner than hwp5, so raised 1.0mm→1.5mm to stay crisp in hwpx too.
        BorderFill {
            sides: [
                BorderLine {
                    line_type: 1,
                    width: 11,
                    color: 0x0080_8080,
                },
                BorderLine::default(),
                BorderLine::default(),
                BorderLine::default(),
            ],
            ..BorderFill::default()
        },
        // 4 (1-based id 5) code block: light-gray background + thin gray border.
        BorderFill {
            sides: [BorderLine {
                line_type: 1,
                width: 0,
                color: 0x00C0_C0C0,
            }; 4],
            fill_type: 1,
            bg_color: Some(0x00F0_F0F0),
            ..BorderFill::default()
        },
    ];
    header
}

/// Converts markdown text into a document (existing signature — relative-path images keep alt after a warning).
pub fn from_markdown(md: &str) -> Document {
    from_markdown_with(md, &MarkdownImportOptions::default())
}

/// Variant that takes options. If `base_dir` is set, embeds relative-path images (`![](fig.png)`).
/// Remote URLs, missing files, and unsupported formats keep only the alt text in the body after a warning (stderr).
pub fn from_markdown_with(md: &str, opts: &MarkdownImportOptions) -> Document {
    let (doc, warnings, hard_error) = from_markdown_inner(md, opts, true);
    print_warnings(&warnings);
    if let Some(e) = hard_error {
        // Infallible entry point — the sandbox violation still surfaces on stderr, but the
        // document build itself cannot fail here. Fail-closed callers use the report variants.
        print_warnings(&[e]);
    }
    doc
}

/// Variant for part fragments — does not touch section/column definition (secd/cold) injection.
/// The block is grafted into the middle of a composed document, so injecting a section definition
/// would split the document in two. Used by `hwp fill --set name=@part.md` (template + part filling).
pub fn from_markdown_blocks(md: &str, opts: &MarkdownImportOptions) -> Document {
    let (doc, warnings, hard_error) = from_markdown_inner(md, opts, false);
    print_warnings(&warnings);
    if let Some(e) = hard_error {
        // Same policy as from_markdown_with — see there.
        print_warnings(&[e]);
    }
    doc
}

/// Warnings (image failures, etc.) go to stderr (document generation itself succeeds).
fn print_warnings(warnings: &[String]) {
    for w in warnings {
        eprintln!("경고: {w}");
    }
}

/// Like [`from_markdown_with`], but also returns the import warnings (image failures, HTML block
/// contract violations, ...) instead of only printing them to stderr. Lets callers (`hwp new`,
/// MCP `hwp_new`) surface them in their reports. A sandbox violation (image reference outside
/// the `roots` option, #56) is a hard error and fails the import with `Err`.
pub fn from_markdown_report(
    md: &str,
    opts: &MarkdownImportOptions,
) -> Result<(Document, Vec<String>), String> {
    let (doc, warnings, hard_error) = from_markdown_inner(md, opts, true);
    match hard_error {
        Some(e) => Err(e),
        None => Ok((doc, warnings)),
    }
}

/// [`from_markdown_blocks`] variant that also returns the import warnings. A sandbox violation
/// (image reference outside the `roots` option, #56) is a hard error (`Err`).
pub fn from_markdown_blocks_report(
    md: &str,
    opts: &MarkdownImportOptions,
) -> Result<(Document, Vec<String>), String> {
    let (doc, warnings, hard_error) = from_markdown_inner(md, opts, false);
    match hard_error {
        Some(e) => Err(e),
        None => Ok((doc, warnings)),
    }
}

/// Returns the document and the import warnings, plus the first sandbox violation if any.
/// The build never aborts mid-way: a rejected image is dropped like a soft failure and the
/// violation is reported through the third tuple element, so the infallible entry points can
/// still return a document (only reachable with a non-empty `roots` option).
fn from_markdown_inner(
    md: &str,
    opts: &MarkdownImportOptions,
    inject: bool,
) -> (Document, Vec<String>, Option<String>) {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    // Parses strikethrough (`~~`) and footnotes (`[^N]`). Task lists (TASKLISTS) are excluded — no corresponding IR meaning.
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    // The definition body is needed at reference time, so events are collected once and scanned twice.
    let events: Vec<Event> = Parser::new_ext(md, options).collect();

    // 1) Pre-render footnote/endnote definition bodies (reused by references).
    let note_bodies = collect_note_bodies(&events);

    // 2) Body processing.
    let mut b = Builder {
        note_bodies,
        base_dir: opts.base_dir.map(Path::to_path_buf),
        roots: opts.roots.to_vec(),
        ..Builder::default()
    };
    for event in &events {
        b.event(event.clone());
    }
    b.flush_html(); // closes any block HTML left in the buffer
    b.flush_paragraph();

    if b.paragraphs.is_empty() {
        // Close even an empty document with one paragraph. The writer guarantees the paragraph-end char.
        b.paragraphs.push(Paragraph::default());
    }
    // Splice leading frames (두문) in front of the body BEFORE section-control injection, so the
    // 두문 table becomes paragraph 0 and receives secd/cold/pgnp (D-02/D-03; Pattern 5 —
    // inject_section_controls does not inspect the paragraph it decorates).
    if let Some(fields) = opts.frames {
        let leading = crate::frames::leading_frames(fields);
        if !leading.is_empty() {
            b.paragraphs.splice(0..0, leading);
        }
    }
    if inject {
        // Inject section/column definitions into the first paragraph — prerequisite for hwp5/Hancom compatibility
        inject_section_controls(&mut b.paragraphs[0], opts.preset, opts.page_margins);
    }

    // Merges the para shapes and numbering/bullet definitions created for lists into the header.
    let mut header = default_header();
    header.char_shapes.extend(b.extra_char_shapes);
    header.para_shapes.extend(b.extra_para_shapes);
    header.border_fills.extend(b.extra_border_fills);
    for slot in &mut header.fonts {
        slot.extend(b.extra_fonts.iter().cloned());
    }
    header.numbering_levels = b.numbering_levels;
    header.bullet_chars = b.bullet_chars;
    if let Some(preset) = opts.preset {
        official::apply_profile(&mut header, preset);
    }
    // Splice trailing frames (결문) after the body AND after `apply_profile` — `apply_profile`
    // overwrites every existing char shape's `base_size` by table position (official.rs), so a
    // 22pt bold shape allocated before it would be immediately clobbered back to the profile's
    // body size. Allocating (value-deduped) after profile sizing keeps the frame's own typography
    // (D-02/D-03/D-04). The `끝.` guard inspects `b.paragraphs` as assembled so far.
    if let Some(fields) = opts.frames {
        let trailing = crate::frames::trailing_frames(
            fields,
            &b.paragraphs,
            &mut header.para_shapes,
            &mut header.char_shapes,
        );
        b.paragraphs.extend(trailing);
    }

    (
        Document {
            meta: DocMeta {
                source_format: "markdown".to_string(),
                source_version: String::new(),
            },
            metadata: Default::default(),
            header,
            sections: vec![Section {
                paragraphs: b.paragraphs,
                extras: Vec::new(),
            }],
            bin_streams: b.bin_streams,
            hwpx_settings_xml: None,
            hwpx_version_xml: None,
            hwpx_preview_image: None,
            hwp5_xml_template: Vec::new(),
            hwp5_doc_history: Vec::new(),
            hwpx_extra_entries: Vec::new(),
            hwpx_bin_manifest: Vec::new(),
            hwpx_opf_extra_items: Vec::new(),
            hwpx_section_xmlns: Vec::new(),
        },
        b.warnings,
        b.hard_error,
    )
}

/// Whether a footnote/endnote label is an endnote (`eN`) — symmetric with the exporter's convention of writing endnotes as `[^eN]`.
fn is_endnote_label(label: &str) -> bool {
    label
        .strip_prefix('e')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Pre-renders footnote/endnote definition (`[^N]: body`) blocks into label→body paragraphs.
/// Pre-collection is needed because a reference (`[^N]`) can appear before its definition.
fn collect_note_bodies(events: &[Event]) -> HashMap<String, Vec<Paragraph>> {
    let mut map = HashMap::new();
    let mut i = 0;
    while i < events.len() {
        let Event::Start(Tag::FootnoteDefinition(label)) = &events[i] else {
            i += 1;
            continue;
        };
        // Collects inner events up to the matching End (definitions do not nest, but guard by depth).
        let mut depth = 1usize;
        let start = i + 1;
        let mut j = start;
        while j < events.len() {
            match &events[j] {
                Event::Start(Tag::FootnoteDefinition(_)) => depth += 1,
                Event::End(TagEnd::FootnoteDefinition) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let mut sub = Builder::default();
        for ev in &events[start..j] {
            sub.event(ev.clone());
        }
        sub.flush_html();
        sub.flush_paragraph();
        let mut body = sub.paragraphs;
        if body.is_empty() {
            body.push(note_body_para());
        }
        // Note bodies cannot reference list para shapes (output of a sub-builder that is not
        // merged), so reset them to the default body shape (lists inside notes are unsupported in v1 — text only).
        for p in &mut body {
            if p.para_shape.0 >= BASE_PARA_SHAPES {
                p.para_shape = ParaShapeId(2);
            }
        }
        map.insert(label.to_string(), body);
        i = j + 1;
    }
    map
}

/// Empty footnote/endnote body paragraph (satisfies the mandatory 1 char-shape run invariant).
fn note_body_para() -> Paragraph {
    Paragraph {
        char_shape_runs: vec![(0, CharShapeId(0))],
        ..Paragraph::default()
    }
}

/// Builds a footnote/endnote anchor pair — FOOTNOTE_ENDNOTE ExtCtrl (code 17) + fn/en
/// GenericControl carrying the body paragraph list. Shared by the md path (push_footnote) and
/// the HTML fragment path (fnref marker reattachment, #47). `ctrl_index` is the caller's control
/// slot; relink_ctrl_index at paragraph flush does the final reassignment.
pub(crate) fn footnote_anchor(
    label: &str,
    body: Vec<Paragraph>,
    ctrl_index: u32,
) -> (HwpChar, Control) {
    let ctrl_id = if is_endnote_label(label) {
        *b"en  "
    } else {
        *b"fn  "
    };
    // Anchor: ExtCtrl (code 17). First 4B of the 12B payload = reversed ctrl_id (same convention as other anchors).
    let mut payload = vec![0u8; 12];
    let mut rev = ctrl_id;
    rev.reverse();
    payload[..4].copy_from_slice(&rev);
    let ch = HwpChar::ExtCtrl {
        code: ctrl_char::FOOTNOTE_ENDNOTE,
        ctrl_id,
        payload,
        ctrl_index: Some(ctrl_index),
    };
    let control = Control::Generic(GenericControl {
        ctrl_id,
        data: Vec::new(),
        paragraph_lists: vec![ParagraphList {
            header_data: Vec::new(),
            paragraphs: body,
        }],
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    });
    (ch, control)
}

#[derive(Default)]
struct Builder {
    paragraphs: Vec<Paragraph>,
    // current paragraph state
    chars: Vec<HwpChar>,
    runs: Vec<(u32, CharShapeId)>,
    controls: Vec<Control>, // extended controls of the current paragraph (hyperlinks, etc.)
    wchar_pos: u32,
    style: u16,
    bold: bool,
    italic: bool,
    strike: bool,              // strikethrough span (`~~`)
    underline: bool,           // underline span (inline HTML `<u>`)
    sup: bool,                 // superscript span (inline HTML `<sup>`)
    sub: bool,                 // subscript span (inline HTML `<sub>`)
    in_code: bool,             // inline code span (`code` — HCR Dotum + shading)
    in_link: bool,             // hyperlink display text span (blue + underline)
    link_end: Option<HwpChar>, // FIELD_END char to emit when the link ends
    in_blockquote: u32,        // blockquote nesting depth (>0 means a quote paragraph)
    in_codeblock: bool,        // code block span (gray background paragraph)
    heading: Option<u16>,      // 1..=6
    // H1~H3 section number ladder (1. / 1-1. / 1-1-1.) — report standard. If a heading already
    // starts with a number (1. / Ⅰ. / hangul.), the prefix is omitted (prevents double numbering).
    h_counters: [u32; 3],
    pending_heading_num: Option<String>,
    section_para_shape: Option<u16>, // para shape index for H2/H3 indent (left 2000)
    // Outdent para shapes for body paragraphs starting with gaejo-style symbols (□·○) (allocated once per symbol).
    symbol_para_shapes: HashMap<char, u16>,
    // table collection state
    table: Option<TableBuilder>,
    // list state — per-level frame stack (nested); item paragraphs get the head para shape.
    list_stack: Vec<ListFrame>,
    rejected_list_depths: usize,
    // para shapes created for lists (header index BASE_PARA_SHAPES~) and numbering/bullet definitions (0~).
    extra_para_shapes: Vec<ParaShape>,
    // border/fill entries created for table header shading (header index BASE_BORDER_FILLS+1~,
    // 1-based on-disk id — Pitfall 1). Merged into header.border_fills the same way
    // extra_para_shapes merges into header.para_shapes (D-07/D-08).
    extra_border_fills: Vec<BorderFill>,
    numbering_levels: Vec<Vec<NumLevel>>,
    bullet_chars: Vec<char>,
    // footnotes/endnotes: pre-collected definition bodies (label→paragraphs) + definition block skip depth.
    note_bodies: HashMap<String, Vec<Paragraph>>,
    skip_note_def: u32,
    // images: relative-path base directory + embedded binaries + warnings + alt suppression state.
    base_dir: Option<PathBuf>,
    /// Sandbox roots for image containment (MCP `--root`, #56). Empty = no check.
    roots: Vec<PathBuf>,
    /// First sandbox violation (image reference outside `roots`) — a hard import error that
    /// fails the report variants, never degraded to an alt-text warning (#56).
    hard_error: Option<String>,
    bin_streams: Vec<BinStream>,
    warnings: Vec<String>,
    // HTML blocks (tables/images — contract docs/design/18): buffer of consecutive Html events +
    // off-palette char shapes for underline/superscript etc. (allocated once after the palette) + allocation cache.
    html_buf: String,
    extra_char_shapes: Vec<CharShape>,
    html_shape_cache: HashMap<(bool, bool, bool, bool, bool, bool), u16>,
    // Additional fonts restored from `<style>` rules in HTML blocks (contract v2 — extended across all slots).
    extra_fonts: Vec<FaceName>,
    in_image_suppress: bool, // suppresses alt text when image embedding succeeds
}

/// One list level (frame). Created per `Start(List)`; items use this head para shape.
struct ListFrame {
    /// Para shape index this list's item paragraphs reference.
    para_shape_id: u16,
    /// Whether an item at this level is currently open (whether to grant the head on paragraph flush).
    item_open: bool,
    /// `Some` for ordered lists; bullets have no numeric range to validate.
    ordered_start: Option<u32>,
    /// Number of observed items, used to reject starts whose subsequent
    /// markers would exceed the representable u32 domain.
    item_count: u32,
}

#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<Paragraph>>,
    current_row: Vec<Paragraph>,
    in_head: bool,
}

impl Builder {
    fn current_shape(&self) -> u16 {
        // Inline code dominates other formatting with HCR Dotum + shading (highest priority).
        if self.in_code {
            return shapes::CODE;
        }
        if self.in_link {
            return shapes::HYPERLINK;
        }
        if let Some(level) = self.heading {
            return shapes::HEADING_BASE + level - 1;
        }
        match (self.bold, self.italic, self.strike) {
            (false, false, false) => shapes::NORMAL,
            (true, false, false) => shapes::BOLD,
            (false, true, false) => shapes::ITALIC,
            (true, true, false) => shapes::BOLD_ITALIC,
            (false, false, true) => shapes::STRIKE,
            (true, false, true) => shapes::BOLD_STRIKE,
            (false, true, true) => shapes::ITALIC_STRIKE,
            (true, true, true) => shapes::BOLD_ITALIC_STRIKE,
        }
    }

    /// Char shape ID for the current state. With underline/superscript mixed in, palette
    /// combinations are insufficient, so allocate once after the palette (extra_char_shapes) (same rule as from_html).
    fn shape_id(&mut self) -> u16 {
        if !self.underline && !self.sup && !self.sub {
            return self.current_shape();
        }
        if self.in_code {
            return shapes::CODE;
        }
        if self.in_link {
            return shapes::HYPERLINK;
        }
        if let Some(level) = self.heading {
            return shapes::HEADING_BASE + level - 1;
        }
        let key = (
            self.bold,
            self.italic,
            self.underline,
            self.strike,
            self.sup,
            self.sub,
        );
        if let Some(&id) = self.html_shape_cache.get(&key) {
            return id;
        }
        let id = crate::from_html::PALETTE_LEN + self.extra_char_shapes.len() as u16;
        let normal = default_header().char_shapes[shapes::NORMAL as usize].clone();
        let mut attr = u32::from(self.bold) << 1 | u32::from(self.italic);
        if self.underline {
            attr |= 1 << 2; // underline type 1 (under the character)
        }
        if self.sup {
            attr |= 1 << 15;
        }
        if self.sub {
            attr |= 1 << 16;
        }
        self.extra_char_shapes.push(CharShape {
            attr,
            strike: self.strike,
            ..normal
        });
        self.html_shape_cache.insert(key, id);
        id
    }

    fn push_text(&mut self, text: &str) {
        let shape = CharShapeId(self.shape_id());
        if self.runs.last().map(|(_, s)| *s) != Some(shape) {
            self.runs.push((self.wchar_pos, shape));
        }
        for c in text.chars() {
            match c {
                // Tab: HWP stores code 9 as an 8-WCHAR inline control (§3.2.3 table 6).
                // Storing it as Text('\t') (1 WCHAR) breaks both hwp5 PARA_TEXT and hwpx <hp:t>,
                // so store it separately as InlineCtrl per the IR invariant.
                '\t' => {
                    self.wchar_pos += 8;
                    self.chars.push(HwpChar::InlineCtrl {
                        code: hwp_model::ctrl_char::TAB,
                        payload: vec![0; 12],
                    });
                }
                // Other C0 control chars (0x00~0x1F) can corrupt the document, so drop them.
                // Markdown newlines are handled separately as SoftBreak/HardBreak events, so only
                // normal text remains in this Text (code block newlines are also split into CharCtrl by push_code_text).
                c if (c as u32) < 0x20 => {}
                c => {
                    self.wchar_pos += c.len_utf16() as u32;
                    self.chars.push(HwpChar::Text(c));
                }
            }
        }
    }

    /// Code block text: preserves line boundaries `\n` as CharCtrl(10) (line break). One trailing
    /// newline is removed to avoid an empty line at the end of the code box (fenced blocks usually end with `\n`).
    fn push_code_text(&mut self, text: &str) {
        let text = text.strip_suffix('\n').unwrap_or(text);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.chars.push(HwpChar::CharCtrl(10));
                self.wchar_pos += 1;
            }
            if !line.is_empty() {
                self.push_text(line);
            }
        }
    }

    fn flush_paragraph(&mut self) {
        self.flush_paragraph_inner(false);
    }

    /// Closes the paragraph. With `force`, creates an empty paragraph even without content.
    ///
    /// A table cell must have at least 1 paragraph (LIST_HEADER nparas≥1). Dropping an empty
    /// markdown cell (`| |`) would leave the cell with no PARA_HEADER at all, making an nparas=0
    /// cell, which Hancom rejects as 'corrupted'. Called with force=true at cell end so even an
    /// empty cell gets an empty paragraph.
    fn flush_paragraph_inner(&mut self, force: bool) {
        if self.chars.is_empty() && self.runs.is_empty() && !force {
            return;
        }
        // Hancom paragraph invariants — paragraph-end char (0x0d), nchars bit31, char_shape run
        // merging, etc. — are applied uniformly by the hwp5 writer (emit_paragraph) across the
        // whole synthesis path (md+hwpx). However, every paragraph must have at least 1
        // PARA_CHAR_SHAPE (genuine full-coverage: PARA_HEADER count == PARA_CHAR_SHAPE count; even
        // empty cell paragraphs hold one (0,id) run). The writer emits no PARA_CHAR_SHAPE at all
        // when char_shape_runs is empty, so empty paragraphs (empty cells made via force, etc.)
        // get one (0, body shape) run filled in here. Missing it makes Hancom reject the file as
        // 'corrupted' and crashes the pyhwp parser too.
        let mut runs = std::mem::take(&mut self.runs);
        if runs.is_empty() {
            runs.push((0, CharShapeId(self.shape_id())));
        }
        // If a list item is open, the head (NUMBER/BULLET) para shape takes precedence.
        // Otherwise: code block→4 (gray background), quote→3 (indent+bar), heading→1,
        // table cell→0 (no spacing), gaejo-style symbol body (□·○)→outdent shape, body→2.
        let symbol = leading_symbol(&self.chars);
        let para_shape = if let Some(id) = self.active_list_para_shape() {
            id
        } else if self.in_codeblock {
            4
        } else if self.in_blockquote > 0 {
            3
        } else if let Some(h) = self.heading {
            // H2/H3 use the indent (SECTION) para shape; other headings use the default heading shape.
            if (2..=3).contains(&h) {
                self.section_para_shape.unwrap_or(1)
            } else {
                1
            }
        } else if self.table.is_some() {
            0
        } else if let Some(sym) = symbol {
            self.symbol_para_shape(sym)
        } else {
            2
        };
        let mut para = Paragraph {
            para_shape: ParaShapeId(para_shape),
            style: StyleId(self.style),
            chars: std::mem::take(&mut self.chars),
            char_shape_runs: runs,
            controls: std::mem::take(&mut self.controls),
            ..Paragraph::default()
        };
        // Links FIELD_START (hyperlink, etc.) ExtCtrl ↔ controls in order of appearance.
        crate::field::relink_ctrl_index(&mut para);
        self.wchar_pos = 0;
        match &mut self.table {
            Some(tb) => tb.current_row.push(para),
            None => self.paragraphs.push(para),
        }
    }

    /// Head para shape of the currently open list item (None if none).
    fn active_list_para_shape(&self) -> Option<u16> {
        self.list_stack
            .last()
            .filter(|f| f.item_open)
            .map(|f| f.para_shape_id)
    }

    /// List entry — closes the parent item paragraph and creates this level's head para shape/definition.
    fn start_list(&mut self, start: Option<u64>) -> Result<(), String> {
        // Close the parent item's paragraph first (e.g. "second" before a nested list).
        self.flush_paragraph();
        let level = validate_official_list_depth(self.list_stack.len() + 1)
            .map_err(|error| error.to_string())?;
        let para_shape_id = match start {
            // Ordered list: numbering definition (the exporter draws markers from numbering_levels) + NUMBER head.
            Some(s) => {
                let start = normalize_authored_list_start(s)?;
                let def_id = self.numbering_levels.len() as u16;
                let mut levels = vec![NumLevel::default(); 8];
                // Preserves this list level's start number (the exporter reflects start).
                levels[level as usize - 1].start = start;
                self.numbering_levels.push(levels);
                self.push_list_para_shape(2, level, def_id)
            }
            // Bullet list: bullet char + BULLET head. The bullet chars use the bottom two rungs of
            // the gaejo-style ladder (□ → ○ → - → ·) — since this tool targets Korean official
            // documents, level 1 `-` and level 2+ `·` are the defaults (`•` is no longer used).
            None => {
                let def_id = self.bullet_chars.len() as u16;
                self.bullet_chars.push(if level >= 2 { '·' } else { '-' });
                self.push_list_para_shape(3, level, def_id)
            }
        };
        self.list_stack.push(ListFrame {
            para_shape_id,
            item_open: false,
            ordered_start: start.map(normalize_authored_list_start).transpose()?,
            item_count: 0,
        });
        Ok(())
    }

    fn end_list(&mut self) {
        self.flush_paragraph();
        self.list_stack.pop();
    }

    /// Inline HTML tags — toggles contract marks (`u`/`s`/`sup`/`sub`/`strong`/`em`) and `<br/>`.
    /// Other tags (arbitrary `<a>`, `<span>`, etc.) are ignored with only a warning.
    fn inline_html_tag(&mut self, h: &str) {
        let tag = h.trim().trim_end_matches('/').to_ascii_lowercase();
        let tag = tag.trim_end_matches('>');
        match tag {
            "<u" => self.underline = true,
            "</u" => self.underline = false,
            "<s" => self.strike = true,
            "</s" => self.strike = false,
            "<sup" => self.sup = true,
            "</sup" => self.sup = false,
            "<sub" => self.sub = true,
            "</sub" => self.sub = false,
            "<strong" | "<b" => self.bold = true,
            "</strong" | "</b" => self.bold = false,
            "<em" | "<i" => self.italic = true,
            "</em" | "</i" => self.italic = false,
            "<br" => {
                self.chars.push(HwpChar::CharCtrl(10));
                self.wchar_pos += 1;
            }
            _ => self
                .warnings
                .push(format!("인라인 HTML 태그 무시: {}", h.trim())),
        }
    }

    /// Parses the block HTML collected in the buffer and merges it into paragraphs (contract
    /// docs/design/18). A parse failure (contract violation) is left as a warning without aborting
    /// document generation — the existing policy of letting markdown conversion itself succeed (same as image failure warnings).
    /// A sandbox violation (image reference outside `roots`, #56) is the exception: it stays a hard error.
    fn flush_html(&mut self) {
        let html = std::mem::take(&mut self.html_buf);
        if html.trim().is_empty() {
            return;
        }
        self.flush_paragraph();
        let opts = crate::from_html::HtmlImportOptions {
            base_dir: self.base_dir.as_deref(),
            roots: &self.roots,
            bin_seed: self.bin_streams.len(),
            // fnref markers inside the fragment reattach to the pre-collected GFM bodies (#47).
            note_bodies: Some(&self.note_bodies),
        };
        let parsed = crate::from_html::parse_fragment(&html, &opts);
        match parsed {
            Ok(blocks) => self.merge_html_blocks(blocks),
            Err(crate::from_html::FragmentError::Sandbox(e)) => {
                self.hard_error.get_or_insert(e);
            }
            Err(crate::from_html::FragmentError::AuthoredList(error)) => {
                self.hard_error.get_or_insert(error);
            }
            Err(crate::from_html::FragmentError::Contract(e)) => self
                .warnings
                .push(format!("HTML 블록을 무시합니다(계약 위반): {e}")),
        }
    }

    /// Merges from_html output into the current document. Each header collection (para shapes,
    /// numbering/bullet definitions, extra char shapes) indexes from 0, so shift by the offsets before appending.
    fn merge_html_blocks(&mut self, mut blocks: crate::from_html::HtmlBlocks) {
        let ps_off = self.extra_para_shapes.len() as u16;
        let num_off = self.numbering_levels.len() as u16;
        let bul_off = self.bullet_chars.len() as u16;
        let cs_off = self.extra_char_shapes.len() as u16;
        let font_off = self.extra_fonts.len() as u16;
        for mut ps in blocks.extra_para_shapes {
            match (ps.attr1 >> 23) & 0x3 {
                2 => ps.numbering_id += num_off, // numbering definition reference
                3 => ps.numbering_id += bul_off, // bullet definition reference
                _ => {}
            }
            self.extra_para_shapes.push(ps);
        }
        self.numbering_levels.extend(blocks.numbering_levels);
        self.bullet_chars.extend(blocks.bullet_chars);
        // Face id rebase — each fragment's additional fonts start at the same id (default font
        // count~), so they must be shifted by the number of already-merged additional fonts to
        // prevent the second block's shapes from pointing at the first block's fonts (contract v2).
        if font_off > 0 {
            let default_fonts = default_header().fonts[0].len() as u16;
            for shape in &mut blocks.extra_char_shapes {
                for face in &mut shape.face_ids {
                    if *face >= default_fonts {
                        *face += font_off;
                    }
                }
            }
        }
        self.extra_char_shapes.extend(blocks.extra_char_shapes);
        self.extra_fonts.extend(blocks.extra_fonts);
        self.bin_streams.extend(blocks.bin_streams);
        self.warnings.extend(blocks.warnings);
        for mut para in blocks.paragraphs {
            remap_para_ids(&mut para, ps_off, cs_off);
            self.paragraphs.push(para);
        }
    }

    /// Creates a para shape for list items and returns its index.
    /// head_type: 2=number, 3=bullet. HWPX list levels 1..=8 are retained semantically.
    fn push_list_para_shape(&mut self, head_type: u32, level: u16, def_id: u16) -> u16 {
        let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        // Indent per level (HWPUNIT) — makes nesting visible in Hancom. The exporter's nesting
        // detection is head_level based, so this has no effect on round-trip closure (the margin is for on-screen display).
        let step = 2000i32;
        self.extra_para_shapes.push(ParaShape {
            // Healthy body para shape (0x180: Hangul line-break + line grid) + left align + head type/level.
            // HWP5 has only three persisted level bits. HWPX level 8 stays semantic through
            // `list_level`, while the direct HWP5 writer uses its separately evidenced path.
            attr1: 0x180
                | (1 << 2)
                | (head_type << 23)
                | (u32::from(if level > 7 { 7 } else { level }) << 25),
            margin_left: i32::from(level) * step,
            indent: -step, // outdent: aligns marker and body text
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            numbering_id: def_id,
            list_level: (level > 7).then_some(level as u8),
            ..ParaShape::default()
        });
        idx
    }

    /// Creates (once per symbol) an outdent para shape for gaejo-style symbol paragraphs (`□ `·`○ `)
    /// and returns the index. Uses the same margin ladder as push_list_para_shape but does not set
    /// the head (BULLET) bits — the symbol is already in the body text, so Hancom must not draw a marker over it.
    fn symbol_para_shape(&mut self, sym: char) -> u16 {
        if let Some(&id) = self.symbol_para_shapes.get(&sym) {
            return id;
        }
        let step = 2000i32;
        // □=level 1, ○=level 2. The outdent (-step) aligns wrapped lines with the body after the symbol.
        let depth = if sym == '□' { 1 } else { 2 };
        let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        self.extra_para_shapes.push(ParaShape {
            attr1: 0x180 | (1 << 2), // healthy body + left align (no head type/level)
            margin_left: depth * step,
            indent: -step,
            // Paragraph spacing makes □ block boundaries visible — 600 above □ matches the heading (1)
            // spacing_top; the common 300 below matches the heading spacing_bottom convention.
            spacing_top: if depth == 1 { 600 } else { 0 },
            spacing_bottom: 300,
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            ..ParaShape::default()
        });
        self.symbol_para_shapes.insert(sym, idx);
        idx
    }

    /// Plants a footnote/endnote reference in the current paragraph — FOOTNOTE_ENDNOTE ExtCtrl
    /// (anchor) + fn/en GenericControl (body paragraph list). The exporter reads this structure to emit `[^N]`.
    fn push_footnote(&mut self, label: &str) {
        let body = self
            .note_bodies
            .get(label)
            .cloned()
            .unwrap_or_else(|| vec![note_body_para()]);
        let idx = self.controls.len() as u32;
        let (ch, control) = footnote_anchor(label, body, idx);
        self.chars.push(ch);
        self.wchar_pos += 8;
        self.controls.push(control);
    }

    /// Embeds an image reference in the current paragraph — a local file is inserted as
    /// BinStream and inline Picture (as-char, natural size) and alt is suppressed; on
    /// failure (remote/missing/unsupported) the alt text is kept after a warning.
    /// A sandbox violation (outside `roots`, #56) is a hard import error instead.
    fn start_image(&mut self, dest_url: &str) {
        match self.load_image(dest_url) {
            Ok((data, name, w, h)) => {
                let idx = self.controls.len() as u32;
                self.controls.push(Control::Picture(Picture {
                    common_data: Vec::new(),
                    width: HwpUnit(w.max(1)),
                    height: HwpUnit(h.max(1)),
                    treat_as_char: true, // inline (as-char) placement — the writer synthesizes the shape record
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
                    bin_ref: BinRef::ItemRef(name.clone()),
                    extras: Vec::new(),
                }));
                // gso anchor char (code 11) — same convention as insert_image. relink reassigns ctrl_index.
                self.chars.push(HwpChar::ExtCtrl {
                    code: 11,
                    ctrl_id: *b"gso ",
                    payload: crate::field::rev_payload(b"gso "),
                    ctrl_index: Some(idx),
                });
                self.wchar_pos += 8;
                self.bin_streams.push(BinStream { name, data });
                self.in_image_suppress = true;
            }
            Err(crate::image::ImageOpenError::Hard(e)) => {
                // Sandbox violation (#56) — a hard import error, never an alt-text fallback.
                self.hard_error.get_or_insert(e);
                self.in_image_suppress = false; // keep alt text as the fallback
            }
            Err(crate::image::ImageOpenError::Soft(warn)) => {
                self.warnings.push(warn);
                self.in_image_suppress = false; // keep alt text as the fallback
            }
        }
    }

    /// Resolves an image reference to a local path: the `file://` prefix is stripped, absolute
    /// paths are taken as-is, and relative paths are joined onto `base_dir`. Remote URLs and
    /// base-less relative paths are soft rejections (alt text kept after a warning).
    fn resolve_image_path(&self, dest_url: &str) -> Result<PathBuf, String> {
        let lower = dest_url.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Err(format!(
                "원격 이미지 URL은 지원하지 않습니다(alt 보존): {dest_url}"
            ));
        }
        // Strip the file: scheme prefix and treat it as a local path.
        let raw = dest_url.strip_prefix("file://").unwrap_or(dest_url);
        let path = Path::new(raw);
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            match &self.base_dir {
                Some(dir) => Ok(dir.join(path)),
                None => Err(format!(
                    "상대 경로 이미지의 기준 디렉터리를 알 수 없습니다(alt 보존): {dest_url}"
                )),
            }
        }
    }

    /// Resolves and reads an image path. On success returns (bytes, bin name, display width, display height).
    /// Only local paths (absolute + relative to base_dir) are allowed — no network dependence on remote URLs.
    /// With sandbox roots set, containment is verified against the opened file handle and the
    /// bytes are read from that same handle; an outside-root reference is a `Hard` error (#56).
    fn load_image(
        &self,
        dest_url: &str,
    ) -> Result<(Vec<u8>, String, i32, i32), crate::image::ImageOpenError> {
        use crate::image::ImageOpenError;
        let resolved = self
            .resolve_image_path(dest_url)
            .map_err(ImageOpenError::Soft)?;
        let soft =
            |e: std::io::Error| format!("이미지 읽기 실패 {}: {e} (alt 보존)", resolved.display());
        let mut file = crate::image::open_image_under_roots(&resolved, &self.roots, soft)?;
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut data).map_err(|e| {
            ImageOpenError::Soft(format!(
                "이미지 읽기 실패 {}: {e} (alt 보존)",
                resolved.display()
            ))
        })?;
        if data.is_empty() {
            return Err(ImageOpenError::Soft(format!(
                "빈 이미지 파일(alt 보존): {}",
                resolved.display()
            )));
        }
        // Format detection by magic bytes — unknown (.bin) is treated as unsupported (alt preserved).
        let (ext, _) = crate::image::image_kind(&data);
        if ext == "bin" {
            return Err(ImageOpenError::Soft(format!(
                "지원하지 않는 이미지 형식(alt 보존): {}",
                resolved.display()
            )));
        }
        let (w, h) =
            crate::image::display_size(&data, &crate::image::ImageSize::Natural, BODY_WIDTH);
        let name = format!("md_image{}.{ext}", self.bin_streams.len() + 1);
        Ok((data, name, w, h))
    }

    fn event(&mut self, event: Event<'_>) {
        // Footnote/endnote definition blocks are pre-collected by collect_note_bodies, so skip
        // them in the body (depth tracking only). All other events are ignored while skipping.
        if let Event::Start(Tag::FootnoteDefinition(_)) = &event {
            self.skip_note_def += 1;
            return;
        }
        if let Event::End(TagEnd::FootnoteDefinition) = &event {
            self.skip_note_def = self.skip_note_def.saturating_sub(1);
            return;
        }
        if self.skip_note_def > 0 {
            return;
        }
        // The block HTML buffer is closed on non-Html events. pulldown wraps each HTML block in
        // Start/End(HtmlBlock), but consecutive blocks like `<style>`+`<table>` must be collected
        // into one fragment for the <style> rules to apply to the following block (contract v2).
        let is_html_seq = matches!(
            event,
            Event::Html(_) | Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock)
        );
        if !is_html_seq {
            self.flush_html();
        }
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush_paragraph();
                let n = heading_level(level);
                self.heading = Some(n);
                self.style = n; // "개요 N" style
                // H1~H3 section number computation: +1 at this level, reset lower to 0, promote zero upper to 1.
                self.pending_heading_num = if n <= 3 {
                    let i = (n - 1) as usize;
                    self.h_counters[i] += 1;
                    for c in &mut self.h_counters[i + 1..] {
                        *c = 0;
                    }
                    for c in &mut self.h_counters[..i] {
                        if *c == 0 {
                            *c = 1;
                        }
                    }
                    let nums: Vec<String> =
                        self.h_counters[..=i].iter().map(u32::to_string).collect();
                    Some(format!("{}. ", nums.join("-")))
                } else {
                    None
                };
                // H2/H3 indent para shape (allocated once).
                if (2..=3).contains(&n) && self.section_para_shape.is_none() {
                    let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
                    self.extra_para_shapes.push(ParaShape {
                        attr1: 0x180 | (1 << 2),
                        margin_left: 2000,
                        line_spacing_old: 160,
                        line_spacing: 160,
                        border_fill_id: 2,
                        ..ParaShape::default()
                    });
                    self.section_para_shape = Some(idx);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                self.flush_paragraph();
                self.heading = None;
                self.style = 0;
                self.pending_heading_num = None;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => self.flush_paragraph(),
            Event::Start(Tag::Strong) => self.bold = true,
            Event::End(TagEnd::Strong) => self.bold = false,
            Event::Start(Tag::Emphasis) => self.italic = true,
            Event::End(TagEnd::Emphasis) => self.italic = false,
            Event::Start(Tag::Strikethrough) => self.strike = true,
            Event::End(TagEnd::Strikethrough) => self.strike = false,
            Event::Text(t) => {
                // Section number prefix: inserted before the heading's first text (skipped for headings that already have a number).
                if self.heading.is_some()
                    && let Some(num) = self.pending_heading_num.take()
                    && !starts_with_literal_number(&t)
                {
                    self.push_text(&num);
                }
                if self.in_image_suppress {
                    // Image embedded successfully → suppress alt text (the picture replaces it).
                } else if self.in_codeblock {
                    self.push_code_text(&t); // \n in code block text → line break
                } else {
                    self.push_text(&t);
                }
            }
            // ── Inline code (`code`) → HCR Dotum + shading char-shape run ──
            Event::Code(t) => {
                self.in_code = true;
                self.push_text(&t);
                self.in_code = false;
            }
            // ── Image (`![alt](path)`) → inline Picture + BinStream (local paths only) ──
            Event::Start(Tag::Image { dest_url, .. }) => self.start_image(&dest_url),
            Event::End(TagEnd::Image) => self.in_image_suppress = false,
            // ── Footnote/endnote reference (`[^N]`/`[^eN]`) → FOOTNOTE_ENDNOTE ExtCtrl + fn/en control ──
            Event::FootnoteReference(label) => self.push_footnote(&label),
            // ── Hyperlink: [text](url) → %hlk field (FIELD_START + blue-underlined text + FIELD_END) ──
            Event::Start(Tag::Link { dest_url, .. }) => {
                let (start, _end, control) = crate::field::hyperlink_field_parts(&dest_url);
                self.chars.push(start);
                self.wchar_pos += 8; // FIELD_START ExtCtrl = 8 WCHAR
                self.controls.push(control);
                self.in_link = true; // subsequent display text uses the HYPERLINK char shape
                self.link_end = Some(_end);
            }
            Event::End(TagEnd::Link) => {
                if let Some(end) = self.link_end.take() {
                    self.chars.push(end);
                    self.wchar_pos += 8; // FIELD_END InlineCtrl = 8 WCHAR
                }
                self.in_link = false;
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => {
                self.chars.push(HwpChar::CharCtrl(10));
                self.wchar_pos += 1;
            }
            // ── Blockquote (> ) → indent + left-bar paragraph (para_shape 3) ──
            Event::Start(Tag::BlockQuote(_)) => {
                self.flush_paragraph();
                self.in_blockquote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.flush_paragraph();
                self.in_blockquote = self.in_blockquote.saturating_sub(1);
            }
            // ── Code block (```) → gray background paragraph (para_shape 4), line breaks preserved ──
            Event::Start(Tag::CodeBlock(_)) => {
                self.flush_paragraph();
                self.in_codeblock = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                self.flush_paragraph();
                self.in_codeblock = false;
            }
            // ── Ordered/bullet lists → head (NUMBER/BULLET) paragraphs, nesting by level ──
            Event::Start(Tag::List(start)) => match self.start_list(start) {
                Ok(()) => {}
                Err(error) => {
                    self.hard_error.get_or_insert(error.to_string());
                    self.rejected_list_depths += 1;
                }
            },
            Event::End(TagEnd::List(_)) => {
                if self.rejected_list_depths > 0 {
                    self.rejected_list_depths -= 1;
                } else {
                    self.end_list();
                }
            }
            Event::Start(Tag::Item) => {
                if let Some(f) = self.list_stack.last_mut() {
                    if let Some(start) = f.ordered_start {
                        let Some(next_item) = f.item_count.checked_add(1) else {
                            self.hard_error.get_or_insert_with(|| {
                                "authored ordered-list item count exceeds u32 maximum".to_string()
                            });
                            return;
                        };
                        if let Err(error) = validate_authored_list_item(start, next_item) {
                            self.hard_error.get_or_insert(error);
                        } else {
                            f.item_count = next_item;
                        }
                    }
                    f.item_open = true;
                }
            }
            Event::End(TagEnd::Item) => {
                self.flush_paragraph();
                if let Some(f) = self.list_stack.last_mut() {
                    f.item_open = false;
                }
            }
            // ── GFM table ──
            Event::Start(Tag::Table(_)) => {
                self.flush_paragraph();
                self.table = Some(TableBuilder::default());
            }
            Event::Start(Tag::TableHead) => {
                if let Some(tb) = &mut self.table {
                    tb.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(tb) = &mut self.table {
                    let row = std::mem::take(&mut tb.current_row);
                    tb.rows.push(row);
                    tb.in_head = false;
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(tb) = &mut self.table {
                    let row = std::mem::take(&mut tb.current_row);
                    tb.rows.push(row);
                }
            }
            Event::Start(Tag::TableCell) => {
                if self.table.as_ref().is_some_and(|tb| tb.in_head) {
                    self.bold = true;
                }
            }
            Event::End(TagEnd::TableCell) => {
                // Even an empty cell must get 1 paragraph (guarantees nparas≥1 + column count consistency).
                self.flush_paragraph_inner(true);
                self.bold = false;
            }
            Event::End(TagEnd::Table) => {
                if let Some(tb) = self.table.take() {
                    self.paragraphs.push(table_paragraph(
                        tb,
                        &mut self.extra_para_shapes,
                        &mut self.extra_border_fills,
                        &mut self.extra_char_shapes,
                    ));
                }
            }
            // ── Block HTML (tables/images — contract docs/design/18): collect consecutive events in the buffer ──
            Event::Html(h) => self.html_buf.push_str(&h),
            // ── Inline HTML (`<u>`·`<sup>`·`<sub>`·`<s>`·`<br/>`) → mark toggles/line breaks ──
            Event::InlineHtml(h) => self.inline_html_tag(&h),
            _ => {}
        }
    }
}

/// If a paragraph starts with a gaejo-style symbol `□ `/`○ `, returns that symbol. Markdown strips
/// leading line whitespace and flattens the ladder, so only these paragraphs restore the levels
/// via margins (exactly these two prefixes).
/// HTML block merge shift — recursively shifts para shape ids (≥BASE_PARA_SHAPES) and extra char
/// shape ids (≥PALETTE_LEN) (including nested table cells and Generic paragraph lists).
fn remap_para_ids(para: &mut Paragraph, ps_off: u16, cs_off: u16) {
    if para.para_shape.0 >= BASE_PARA_SHAPES {
        para.para_shape.0 += ps_off;
    }
    for (_, id) in &mut para.char_shape_runs {
        if id.0 >= crate::from_html::PALETTE_LEN {
            id.0 += cs_off;
        }
    }
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        remap_para_ids(p, ps_off, cs_off);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_para_ids(p, ps_off, cs_off);
                    }
                }
                // ID 재매핑으로 개체 안 문단의 모양 참조가 바뀐다 — 원문 XML은 stale.
                g.hwpx_raw_xml = None;
            }
            _ => {}
        }
    }
}

fn leading_symbol(chars: &[HwpChar]) -> Option<char> {
    match (chars.first(), chars.get(1)) {
        (Some(HwpChar::Text(sym @ ('□' | '○'))), Some(HwpChar::Text(' '))) => Some(*sym),
        _ => None,
    }
}

/// Whether a heading already carries a number — condition for skipping the automatic section
/// number prefix (prevents double numbering): Arabic digits (`1.`), full-width Roman numerals
/// (`Ⅰ.`·`ⅰ.`), Hangul item markers (syllable + `.`/`)`). Numberless word headings are not caught
/// and receive the automatic number as-is.
fn starts_with_literal_number(t: &str) -> bool {
    let mut chars = t.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_digit()
        || ('\u{2160}'..='\u{217F}').contains(&first)
        || (('\u{AC00}'..='\u{D7A3}').contains(&first) && matches!(chars.next(), Some('.' | ')')))
}

fn heading_level(level: HeadingLevel) -> u16 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Inserts secd/cold extended controls (+ pgnp with a preset) before the first paragraph
/// (including the 8-WCHAR shift per control).
pub(crate) fn inject_section_controls(
    para: &mut Paragraph,
    preset: Option<OfficialPreset>,
    margins: PageMarginOverrides,
) {
    use hwp_model::{Control, GenericControl, SectionDef};
    if para
        .controls
        .iter()
        .any(|c| matches!(c, Control::SectionDef(_)))
    {
        return;
    }
    // Number of extended controls to insert: secd + cold (+ profile page number pgnp).
    let has_page_number = official::has_page_number(preset);
    let n_ctrl = if has_page_number { 3 } else { 2 };
    // Shift existing references
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl {
            ctrl_index: Some(i),
            ..
        } = ch
        {
            *i += n_ctrl;
        }
    }
    for (pos, _) in &mut para.char_shape_runs {
        *pos += n_ctrl * 8;
    }
    for seg in &mut para.line_segs {
        seg.text_start += n_ctrl * 8;
    }
    let first_shape = para
        .char_shape_runs
        .first()
        .map_or(CharShapeId(0), |(_, id)| *id);
    if para.char_shape_runs.first().map(|(p, _)| *p) != Some(0) {
        para.char_shape_runs.insert(0, (0, first_shape));
    }
    // Merging consecutive same-id runs (e.g. the [(0,0),(16,0)] duplication created by secd/cold
    // insertion) is applied by the writer across the whole synthesis path.

    let mut page = official::page_def(preset);
    margins.apply(&mut page);
    para.controls.insert(
        0,
        Control::SectionDef(SectionDef {
            data: Vec::new(),
            page: Some(page),
            extras: Vec::new(),
            secpr_raw_children: Vec::new(),
            footnote_shape_raw: None,
            endnote_shape_raw: None,
            page_border_fills_raw: Vec::new(),
        }),
    );
    para.controls.insert(
        1,
        Control::Generic(GenericControl {
            ctrl_id: *b"cold",
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
        }),
    );
    let ext = |code: u16, ctrl_id: [u8; 4], idx: u32| {
        let mut payload = vec![0u8; 12];
        let mut rev = ctrl_id;
        rev.reverse();
        payload[..4].copy_from_slice(&rev);
        HwpChar::ExtCtrl {
            code,
            ctrl_id,
            payload,
            ctrl_index: Some(idx),
        }
    };
    para.chars.insert(0, ext(2, *b"secd", 0));
    para.chars.insert(1, ext(2, *b"cold", 1));
    if has_page_number {
        // Page number bottom center (pgnp, regulation §10). 12B: props (u32: format DIGIT=0 |
        // position BOTTOM_CENTER=5 <<8) + reserved 6B + sideChar WCHAR — genuine measurement
        // (same layout as hwpx read build_pgnp) has sideChar '-' ("- 1 -" notation).
        let mut data = vec![0u8; 12];
        data[..4].copy_from_slice(&(5u32 << 8).to_le_bytes());
        data[10..12].copy_from_slice(&(u16::from(b'-')).to_le_bytes());
        para.controls.insert(
            2,
            Control::Generic(GenericControl {
                ctrl_id: *b"pgnp",
                data,
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }),
        );
        para.chars.insert(2, ext(21, *b"pgnp", 2));
    }
    // break_type of the section's first paragraph — every single-paragraph sample saved directly
    // by Hancom (ganada·hello_world·outline·bookmark) has 0x03 (bit0 section break + bit1 column
    // break). It is the value Hancom always uses on a 'section first paragraph' carrying secd/cold
    // ExtCtrls; 0x00 breaks header-control consistency and is judged corrupted. (The hwp5
    // round-trip path preserves the original break_type in body_text.rs and does not pass through
    // this function, so the byte-identical gate is unaffected.)
    para.header.break_type = 0x03;
}

/// Turns the collected table into an anchor paragraph (one extended control). `para_shapes`/
/// `border_fills` are the Builder's staging vectors (`extra_para_shapes`/`extra_border_fills`) —
/// merged into `header.para_shapes`/`header.border_fills` after `BASE_PARA_SHAPES`/
/// `BASE_BORDER_FILLS` base entries, which is why those two constants are passed as the offset
/// `style::style_table` needs to compute the final, correct shape/fill ids up front (D-07/D-08).
/// `char_shapes` is the Builder's `extra_char_shapes` staging vector; unlike the para-shape/
/// border-fill offset scheme, `style::style_table` addresses `CharShapeId` with no base offset,
/// so the fixed `PALETTE_LEN`-entry base palette is materialized in front of it before the call
/// and split back off afterward (Pitfall 5: the base palette is always the same fixed entries,
/// so this never duplicates or renumbers anything already relied on elsewhere).
fn table_paragraph(
    tb: TableBuilder,
    para_shapes: &mut Vec<ParaShape>,
    border_fills: &mut Vec<BorderFill>,
    char_shapes: &mut Vec<CharShape>,
) -> Paragraph {
    let rows = tb.rows.len().max(1);
    let cols = tb.rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let row_h = 1700i32; // 10pt text + cell top/bottom margins

    let mut cells = Vec::new();
    for (r, row) in tb.rows.iter().enumerate() {
        for c in 0..cols {
            cells.push(Cell {
                list_attr: CELL_VALIGN_CENTER,
                col: c as u16,
                row: r as u16,
                col_span: 1,
                row_span: 1,
                // Placeholder — style::style_table below overwrites every cell's width with the
                // content-proportional value (D-07 rule 2). table_paragraph itself no longer
                // divides BODY_WIDTH evenly across columns.
                width: HwpUnit(0),
                height: HwpUnit(row_h),
                margins: [510, 510, 141, 141],
                border_fill: BorderFillId(TABLE_BORDER_FILL),
                header_tail: Vec::new(),
                // A cell must have at least 1 paragraph (nparas≥1). Slots missing from short rows
                // are filled with empty paragraphs — an nparas=0 cell is treated as corrupted by
                // Hancom. The fill paragraph must also hold 1 PARA_CHAR_SHAPE run (genuine
                // full-coverage invariant; the writer emits no record when char_shape_runs is empty).
                paragraphs: row.get(c).cloned().map_or_else(
                    || {
                        vec![Paragraph {
                            char_shape_runs: vec![(0, CharShapeId(0))],
                            ..Paragraph::default()
                        }]
                    },
                    |p| vec![p],
                ),
            });
        }
    }
    let mut table = Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: rows as u16,
        cols: cols as u16,
        cell_spacing: 0,
        inner_margins: [510, 510, 141, 141],
        row_cell_counts: vec![cols as u16; rows],
        border_fill: BorderFillId(TABLE_BORDER_FILL),
        table_tail: Vec::new(),
        caption: None,
        cells,
        extras: Vec::new(),
    };

    // D-07: every GFM table gets header shading/centering + content-proportional widths. A GFM
    // table always has exactly one header row (row 0), so header detection is free.
    let mut full_char_shapes = default_header().char_shapes;
    full_char_shapes.extend(char_shapes.iter().cloned());
    crate::style::style_table(
        &mut table,
        1,
        BODY_WIDTH,
        para_shapes,
        BASE_PARA_SHAPES,
        border_fills,
        BASE_BORDER_FILLS,
        &mut full_char_shapes,
    );
    *char_shapes = full_char_shapes.split_off(crate::from_html::PALETTE_LEN as usize);

    let mut payload = vec![0u8; 12];
    payload[..4].copy_from_slice(b" lbt"); // reversed ctrl_id
    Paragraph {
        chars: vec![
            HwpChar::ExtCtrl {
                code: 11,
                ctrl_id: *b"tbl ",
                payload,
                ctrl_index: Some(0),
            },
            HwpChar::CharCtrl(13),
        ],
        char_shape_runs: vec![(0, CharShapeId(0))],
        controls: vec![Control::Table(table)],
        ..Paragraph::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_header_크기() {
        let h = default_header();
        assert_eq!(h.char_shapes[0].base_size, 1000); // body 10pt
        assert_eq!(h.char_shapes[4].base_size, 1800); // H1 = body × 1.8
    }

    #[test]
    fn 표_셀_세로정렬_가운데() {
        // Genuine Hancom table cell default = vertical center (list_attr bits5-6=1=0x20). With 0
        // (top), the hwp5 writer emits it as-is and cell content sticks to the top (top margin < bottom margin).
        use hwp_model::Control;
        let doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음");
        assert!(!table.cells.is_empty());
        for c in &table.cells {
            assert_eq!(
                (c.list_attr >> 5) & 3,
                1,
                "셀 세로정렬 가운데(0x20): {:#x}",
                c.list_attr
            );
        }
    }

    /// GI-1/GI-2 round-trip (a): footnotes, strikethrough, ordered list (start), and nesting are preserved across md → IR → md.
    #[test]
    fn 왕복_각주_취소선_순서목록_중첩() {
        let md = "\
문단에 각주[^1]가 있다.

~~지운 글~~ 과 보통 글.

1. 첫째
2. 둘째
   - 안쪽 가
   - 안쪽 나
3. 셋째

[^1]: 각주 본문이다.
";
        let doc = from_markdown(md);
        let out = crate::markdown::to_markdown(&doc);

        // Footnote: body marker + definition at document end.
        assert!(out.contains("[^1]"), "각주 마커: {out}");
        assert!(out.contains("[^1]: 각주 본문이다."), "각주 정의: {out}");
        // Strikethrough.
        assert!(out.contains("~~지운 글~~"), "취소선: {out}");
        // Ordered list (1./2./3.).
        assert!(out.contains("1. 첫째"), "순서1: {out}");
        assert!(out.contains("2. 둘째"), "순서2: {out}");
        assert!(out.contains("3. 셋째"), "순서3: {out}");
        // Nested bullet list (indented `-`).
        assert!(out.contains("- 안쪽 가"), "중첩 불릿: {out}");
        let idx = out.find("안쪽 가").unwrap();
        let line_start = out[..idx].rfind('\n').map_or(0, |p| p + 1);
        assert!(
            out[line_start..idx].starts_with(' '),
            "중첩은 들여쓰기: {out}"
        );
    }

    /// Ordered list start preservation: starting at `3.` stays `3.` after round-trip.
    #[test]
    fn 왕복_순서목록_start_보존() {
        let doc = from_markdown("3. 셋\n4. 넷\n");
        let out = crate::markdown::to_markdown(&doc);
        assert!(out.contains("3. 셋"), "start=3 보존: {out}");
        assert!(out.contains("4. 넷"), "다음 번호: {out}");
    }

    /// Endnotes (`[^eN]`) round-trip symmetrically too.
    #[test]
    fn 왕복_미주() {
        let doc = from_markdown("본문[^e1] 끝.\n\n[^e1]: 미주 본문.\n");
        let out = crate::markdown::to_markdown(&doc);
        assert!(out.contains("[^e1]"), "미주 마커: {out}");
        assert!(out.contains("[^e1]: 미주 본문."), "미주 정의: {out}");
    }

    /// Whether the footnote control is synthesized as an fn GenericControl + FOOTNOTE_ENDNOTE anchor (structural assertion).
    #[test]
    fn 각주_컨트롤_구조() {
        let doc = from_markdown("가[^1]나\n\n[^1]: 각주.\n");
        let para = &doc.sections[0].paragraphs[0];
        let has_anchor = para.chars.iter().any(|c| {
            matches!(c, HwpChar::ExtCtrl { code, ctrl_id, .. }
                if *code == hwp_model::ctrl_char::FOOTNOTE_ENDNOTE && ctrl_id == b"fn  ")
        });
        assert!(has_anchor, "각주 앵커 존재");
        let has_ctrl = para.controls.iter().any(|c| {
            matches!(c,
            Control::Generic(g) if g.ctrl_id == *b"fn  " && !g.paragraph_lists.is_empty())
        });
        assert!(has_ctrl, "각주 컨트롤+본문 존재");
    }

    /// Writes a minimal PNG (dimensions header only) for tests and returns the path.
    fn write_png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(w.to_be_bytes());
        png.extend(h.to_be_bytes());
        png.extend([0u8; 8]);
        let p = dir.join(name);
        std::fs::write(&p, &png).unwrap();
        p
    }

    /// GI-3: local image `![alt](fig.png)` → inline Picture + BinStream (natural size).
    #[test]
    fn 이미지_로컬_임베드() {
        let dir = std::env::temp_dir().join("hwp-md-img-embed");
        std::fs::create_dir_all(&dir).unwrap();
        write_png(&dir, "fig.png", 96, 48);
        let doc = from_markdown_with(
            "본문\n\n![대체텍스트](fig.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&dir),
                preset: None,
                ..Default::default()
            },
        );
        assert_eq!(doc.bin_streams.len(), 1, "BinStream 1개 임베드");
        let pic = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("Picture 존재");
        assert!(pic.treat_as_char, "인라인(글자처럼) 배치");
        assert!(pic.extras.is_empty(), "writer 합성용 빈 extras");
        assert!(doc.resolve_bin(&pic.bin_ref).is_some(), "bin_ref 해석");
        assert_eq!(pic.width.0, 96 * 7200 / 96, "자연 크기(96px→7200)");
        // The alt text of a successfully embedded image is suppressed.
        assert!(!doc.plain_text().contains("대체텍스트"), "alt 억제");
    }

    /// GI-3: missing files, remote URLs, and relative paths (no base) keep the alt text after a warning.
    #[test]
    fn 이미지_실패는_alt_보존() {
        let dir = std::env::temp_dir().join("hwp-md-img-fail");
        std::fs::create_dir_all(&dir).unwrap();
        // Missing file.
        let d1 = from_markdown_with(
            "![없음alt](nope.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&dir),
                preset: None,
                ..Default::default()
            },
        );
        assert!(d1.bin_streams.is_empty(), "임베드 없음");
        assert!(d1.plain_text().contains("없음alt"), "alt 보존");
        // Remote URL (no network).
        let d2 = from_markdown("![원격alt](https://example.com/a.png)\n");
        assert!(d2.bin_streams.is_empty());
        assert!(d2.plain_text().contains("원격alt"), "원격은 alt 보존");
        // Relative path + no base directory.
        let d3 = from_markdown("![상대alt](fig.png)\n");
        assert!(d3.bin_streams.is_empty());
        assert!(d3.plain_text().contains("상대alt"), "기준없음은 alt 보존");
    }

    /// GI-3 round-trip: image data is preserved on re-export through md(image)→IR→#8 exporter (media_dir).
    #[test]
    fn 이미지_왕복_exporter_데이터보존() {
        let dir = std::env::temp_dir().join("hwp-md-img-rt");
        std::fs::create_dir_all(&dir).unwrap();
        let png_path = write_png(&dir, "rt.png", 32, 32);
        let orig = std::fs::read(&png_path).unwrap();
        let doc = from_markdown_with(
            "![x](rt.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&dir),
                preset: None,
                ..Default::default()
            },
        );
        let media = dir.join("out_media");
        let _ = std::fs::remove_dir_all(&media);
        let md = crate::markdown::to_markdown_with(
            &doc,
            &crate::markdown::MarkdownOptions {
                media_dir: Some(&media),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(md.contains("!["), "이미지 참조 재수출: {md}");
        let extracted = std::fs::read(media.join("image1.png")).expect("추출 이미지");
        assert_eq!(extracted, orig, "추출 이미지 바이트 == 원본(무손실)");
        let _ = std::fs::remove_dir_all(&media);
    }

    /// #56: with sandbox roots set, an image reference resolving outside every root (absolute
    /// path or `../` escape) is a hard import error; inside-root references embed as usual;
    /// empty roots keeps the CLI behavior (an absolute outside path still loads).
    #[test]
    fn 이미지_샌드박스_루트_검사() {
        let base = std::env::temp_dir().join(format!(
            "hwp-md-img-roots-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("sandbox");
        let sub = root.join("sub");
        let outside = base.join("outside");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        write_png(&sub, "in.png", 8, 8);
        let outside_png = write_png(&outside, "out.png", 8, 8);
        // Markdown link destinations treat `\` as an escape — use forward slashes (Windows CI).
        let outside_ref = outside_png.display().to_string().replace('\\', "/");
        // Roots are pre-canonicalized by the caller (mirrors the MCP startup).
        let roots = vec![std::fs::canonicalize(&root).unwrap()];
        // A fn (not a closure) so the two reference lifetimes unify by name.
        fn rooted_opts<'a>(
            base_dir: Option<&'a std::path::Path>,
            roots: &'a [PathBuf],
        ) -> MarkdownImportOptions<'a> {
            MarkdownImportOptions {
                base_dir,
                roots,
                preset: None,
                ..Default::default()
            }
        }

        // Inside the root: embeds as usual.
        let (doc, _) = from_markdown_report("![in](in.png)\n", &rooted_opts(Some(&sub), &roots))
            .expect("루트 안 이미지는 임베드");
        assert_eq!(doc.bin_streams.len(), 1, "루트 안 이미지는 임베드");

        // Absolute path outside the root: hard error (not an alt-text warning).
        let err = from_markdown_report(
            &format!("![out]({outside_ref})\n"),
            &rooted_opts(Some(&sub), &roots),
        )
        .expect_err("루트 밖 절대 경로는 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        // `../` relative escape outside the root: hard error.
        let err = from_markdown_report(
            "![esc](../../outside/out.png)\n",
            &rooted_opts(Some(&sub), &roots),
        )
        .expect_err("'../' 탈출은 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        // Same through an HTML block inside markdown (the flush_html path).
        let err = from_markdown_report(
            &format!("<p><img src=\"{outside_ref}\"/></p>\n"),
            &rooted_opts(Some(&sub), &roots),
        )
        .expect_err("HTML 블록 이미지도 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        // Missing file with roots set: still the soft alt-text fallback, not a hard error.
        let (doc, _) =
            from_markdown_report("![없음alt](nope.png)\n", &rooted_opts(Some(&sub), &roots))
                .expect("없는 파일은 소프트 실패(alt 보존)");
        assert!(doc.bin_streams.is_empty(), "임베드 없음");
        assert!(doc.plain_text().contains("없음alt"), "alt 보존");

        // Empty roots: previous CLI behavior — the absolute outside path still loads.
        let (doc, _) = from_markdown_report(
            &format!("![out]({outside_ref})\n"),
            &MarkdownImportOptions {
                base_dir: Some(&sub),
                preset: None,
                ..Default::default()
            },
        )
        .expect("루트 미설정이면 기존 동작");
        assert_eq!(doc.bin_streams.len(), 1, "루트 미설정이면 절대 경로 로드");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// #56 (handle-bound check): an in-root symlink whose target is outside the roots is a
    /// hard error — the verdict comes from the opened handle, so the escape works even though
    /// the request pathname sits inside the sandbox. An in-root symlink target still loads.
    #[cfg(unix)]
    #[test]
    fn 이미지_샌드박스_심링크_탈출_차단() {
        let base = std::env::temp_dir().join(format!(
            "hwp-md-img-symlink-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("sandbox");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        write_png(&root, "real.png", 8, 8);
        let secret = write_png(&outside, "secret.png", 8, 8);
        std::os::unix::fs::symlink(&secret, root.join("escape.png")).unwrap();
        std::os::unix::fs::symlink(root.join("real.png"), root.join("alias.png")).unwrap();
        let roots = vec![std::fs::canonicalize(&root).unwrap()];

        // The symlink itself is inside the root, but its target is not: hard error.
        let err = from_markdown_report(
            "![x](escape.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&root),
                roots: &roots,
                preset: None,
                ..Default::default()
            },
        )
        .expect_err("루트 밖을 가리키는 심링크는 하드 에러");
        assert!(err.contains("샌드박스"), "{err}");

        // Control: a symlink chain staying inside the root loads as usual.
        let (doc, _) = from_markdown_report(
            "![x](alias.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&root),
                roots: &roots,
                preset: None,
                ..Default::default()
            },
        )
        .expect("루트 안 심링크는 임베드");
        assert_eq!(doc.bin_streams.len(), 1, "루트 안 심링크는 임베드");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// GI-4: inline code `code` → HCR Dotum (face_id=1) + light-gray shading char-shape run.
    #[test]
    fn 인라인_코드_글자모양() {
        let doc = from_markdown("이건 `let x = 1;` 코드다.\n");
        let code_id = shapes::CODE;
        let cs = &doc.header.char_shapes[code_id as usize];
        assert_eq!(cs.face_ids[0], FONT_DOTUM, "함초롬돋움 face_id");
        assert_eq!(cs.shade_color, 0x00F0_F0F0, "연회색 음영");
        // Whether the code text was stored as a CODE char-shape run.
        let para = &doc.sections[0].paragraphs[0];
        let has_code_run = para.char_shape_runs.iter().any(|(_, id)| id.0 == code_id);
        assert!(has_code_run, "CODE run 존재: {:?}", para.char_shape_runs);
        // The HCR Dotum font is in the table.
        assert_eq!(doc.header.fonts[0][FONT_DOTUM as usize].name, "함초롬돋움");
    }

    /// push_text: tab goes to InlineCtrl(9), other C0 control chars are dropped, normal chars become Text.
    #[test]
    fn push_text_탭_인라인컨트롤_제어문자_드롭() {
        let mut b = Builder::default();
        b.push_text("A\tB\u{0001}C");
        let kinds: Vec<_> = b.chars.to_vec();
        assert_eq!(kinds.len(), 4, "A, 탭, B, C (0x01 드롭): {kinds:?}");
        assert!(matches!(kinds[0], HwpChar::Text('A')));
        assert!(matches!(kinds[1], HwpChar::InlineCtrl { code: 9, .. }));
        assert!(matches!(kinds[2], HwpChar::Text('B')));
        assert!(matches!(kinds[3], HwpChar::Text('C')));
        // wchar_pos = 1 + 8 + 1 + 1 (0x01 is not consumed).
        assert_eq!(b.wchar_pos, 11);
    }

    /// H1~H3 section number ladder: counter increment, lower reset, upper 0 promotion + H2/H3 indent shape.
    #[test]
    fn 헤딩_절번호_카운터() {
        let doc = from_markdown("# 서론\n## 배경\n## 목적\n# 본론\n### 세부\n");
        let text = doc.plain_text();
        for want in [
            "1. 서론",
            "1-1. 배경",
            "1-2. 목적",
            "2. 본론",
            "2-1-1. 세부",
        ] {
            assert!(text.contains(want), "{want} 없음: {text}");
        }
        // H2/H3 paragraphs use the SECTION para shape (margin_left 2000).
        let ps_of = |needle: &str| {
            let p = doc.sections[0]
                .paragraphs
                .iter()
                .find(|p| {
                    p.chars.iter().any(|c| matches!(c, HwpChar::Text(_)))
                        && plain_of(p).contains(needle)
                })
                .unwrap();
            p.para_shape.0
        };
        fn plain_of(p: &Paragraph) -> String {
            p.chars
                .iter()
                .filter_map(|c| match c {
                    HwpChar::Text(ch) => Some(*ch),
                    _ => None,
                })
                .collect()
        }
        let section_ps = ps_of("1-1. 배경");
        assert!(section_ps >= BASE_PARA_SHAPES, "SECTION은 확장 모양");
        assert_eq!(ps_of("2-1-1. 세부"), section_ps, "H2/H3 동일 모양");
        assert_eq!(
            doc.header.para_shapes[section_ps as usize].margin_left,
            2000
        );
        assert_eq!(ps_of("1. 서론"), 1, "H1은 기본 제목 모양");
    }

    /// Headings starting with a number skip the section number prefix (prevents double numbering).
    #[test]
    fn 헤딩_이중번호_방지() {
        let doc = from_markdown("# 1. 서론\n");
        let text = doc.plain_text();
        assert!(text.contains("1. 서론"), "{text}");
        assert!(!text.contains("1. 1. 서론"), "이중 번호 금지: {text}");
    }

    /// Gaejo-style literal numbers (full-width Roman numerals, Hangul item markers) also block the
    /// automatic section number. Numberless word headings still receive the automatic number (prevents guard overreach).
    #[test]
    fn 헤딩_리터럴_번호_이중번호_방지() {
        let doc = from_markdown("# Ⅰ. 사업 개요\n## 가. 배경\n## 나) 세부\n");
        let text = doc.plain_text();
        assert!(text.contains("Ⅰ. 사업 개요"), "{text}");
        assert!(!text.contains("1. Ⅰ."), "전각 로마 숫자 이중 번호: {text}");
        assert!(!text.contains("1-1. 가."), "한글 `가.` 이중 번호: {text}");
        assert!(!text.contains("1-2. 나)"), "한글 `나)` 이중 번호: {text}");

        let plain = from_markdown("## 사업 개요\n").plain_text();
        assert!(
            plain.contains("1-1. 사업 개요"),
            "낱말 제목은 자동 번호: {plain}"
        );
    }

    /// Bullet char ladder: level 1 `-`, level 2+ `·` (gaejo-style standard, `•` dropped).
    #[test]
    fn 글머리_사다리_수준별_문자() {
        let doc = from_markdown("- 상위\n  - 하위\n");
        assert_eq!(doc.header.bullet_chars, vec!['-', '·']);
    }

    /// `□ `/`○ ` body paragraphs get the outdent para shape (margin ladder), and the head (BULLET)
    /// bits are not set (the symbol is already in the text). Other paragraphs stay body (2).
    #[test]
    fn 개조식_기호_문단_내어쓰기() {
        let doc = from_markdown("□ 현황\n\n○ 세부\n\n□ 계획\n\n일반 문단\n");
        let ps_of = |needle: &str| {
            doc.sections[0]
                .paragraphs
                .iter()
                .find(|p| {
                    p.chars
                        .iter()
                        .filter_map(|c| match c {
                            HwpChar::Text(ch) => Some(*ch),
                            _ => None,
                        })
                        .collect::<String>()
                        .contains(needle)
                })
                .unwrap_or_else(|| panic!("{needle} 문단 없음"))
                .para_shape
                .0
        };
        let square = ps_of("□ 현황");
        let circle = ps_of("○ 세부");
        assert_eq!(ps_of("□ 계획"), square, "같은 기호는 문단모양 재사용");
        assert_eq!(ps_of("일반 문단"), 2, "기호 없는 본문은 기본 본문 모양");

        let shape = |id: u16| &doc.header.para_shapes[id as usize];
        assert_eq!(
            (shape(square).margin_left, shape(square).indent),
            (2000, -2000)
        );
        assert_eq!(
            (shape(circle).margin_left, shape(circle).indent),
            (4000, -2000)
        );
        assert_eq!(shape(square).head_type(), 0, "머리(BULLET) 비트 없음");
        assert_eq!(shape(circle).head_type(), 0, "머리(BULLET) 비트 없음");
        // Paragraph spacing: 600 above so □ block boundaries are visible, common 300 below (○ gets 0 above).
        assert_eq!(
            (shape(square).spacing_top, shape(square).spacing_bottom),
            (600, 300)
        );
        assert_eq!(
            (shape(circle).spacing_top, shape(circle).spacing_bottom),
            (0, 300)
        );
    }

    /// Table cells, headings, and list items are unaffected by the symbol para shapes.
    #[test]
    fn 개조식_기호_예외_경로() {
        use hwp_model::Control;
        let doc = from_markdown("# □ 제목\n\n- □ 항목\n\n| □ 머리 |\n|----|\n| □ 셀 |\n");
        let heading = &doc.sections[0].paragraphs[0];
        assert_eq!(heading.para_shape.0, 1, "제목은 제목 문단모양");
        let table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음");
        for cell in &table.cells {
            // D-07 table styling now centers this narrow single-column table's cells (a distinct,
            // intentional para shape) — the property this test actually guards is that cells
            // never pick up the □/○ body-paragraph OUTDENT shape (margin_left/indent != 0).
            let shape = &doc.header.para_shapes[cell.paragraphs[0].para_shape.0 as usize];
            assert_eq!(
                (shape.margin_left, shape.indent),
                (0, 0),
                "표 셀은 □/○ 내어쓰기 문단모양을 쓰지 않음"
            );
        }
        let item = doc.sections[0]
            .paragraphs
            .iter()
            .find(|p| p.para_shape.0 >= BASE_PARA_SHAPES)
            .expect("목록 항목 없음");
        assert_eq!(
            doc.header.para_shapes[item.para_shape.0 as usize].head_type(),
            3,
            "목록 항목은 BULLET 머리 유지"
        );
    }

    /// Official profiles apply their locked typography, margins, numbering, and page-number policy.
    #[test]
    fn 공문서_프리셋() {
        use hwp_model::{Control, NumFmt};
        let md = "# 제목\n\n1. 하나\n   1. 둘\n\n본문\n";
        let opts = |p| MarkdownImportOptions {
            base_dir: None,
            preset: p,
            ..Default::default()
        };
        let page_of = |doc: &Document| {
            doc.sections[0].paragraphs[0]
                .controls
                .iter()
                .find_map(|c| match c {
                    Control::SectionDef(sd) => sd.page,
                    _ => None,
                })
                .expect("PageDef 있어야")
        };

        // Default (no preset): existing margins, sizes, and number formats are kept.
        let plain = from_markdown_with(md, &opts(None));
        let p = page_of(&plain);
        assert_eq!(
            (
                p.margin_left.0,
                p.margin_top.0,
                p.margin_right.0,
                p.margin_bottom.0
            ),
            (8504, 5668, 8504, 4252)
        );
        assert_eq!(plain.header.char_shapes[0].base_size, 1000);
        assert!(plain.header.numbering_levels[0][1].template.is_empty());
        assert!(
            !plain.sections[0].paragraphs[0]
                .controls
                .iter()
                .any(|c| matches!(c, Control::Generic(g) if g.ctrl_id == *b"pgnp"))
        );

        // report: margins top20/bottom10/left20/right20mm, HCR Batang 15pt, H1 18pt.
        let report = from_markdown_with(md, &opts(Some(OfficialPreset::Report)));
        let p = page_of(&report);
        assert_eq!(
            (
                p.margin_left.0,
                p.margin_top.0,
                p.margin_right.0,
                p.margin_bottom.0
            ),
            (5668, 5668, 5668, 2834)
        );
        assert_eq!(report.header.fonts[0][0].name, "함초롬바탕");
        assert_eq!(report.header.char_shapes[0].base_size, 1500);
        assert_eq!(report.header.char_shapes[4].base_size, 1800); // H1
        assert_eq!(report.header.char_shapes[15].base_size, 1500); // inline code gets the body size too

        // official: Malgun Gothic 12pt, H1 15pt.
        let official = from_markdown_with(md, &opts(Some(OfficialPreset::Official)));
        assert_eq!(official.header.fonts[0][0].name, "맑은 고딕");
        assert_eq!(
            official.header.fonts[0][1].name, "함초롬돋움",
            "인라인 코드 글꼴 유지"
        );
        assert_eq!(official.header.char_shapes[0].base_size, 1200);
        assert_eq!(official.header.char_shapes[4].base_size, 1500);

        // Statutory eight-level numbering ladder, including the two circled levels.
        let levels = &report.header.numbering_levels[0];
        let fmt_tpl: Vec<(NumFmt, &str)> = levels
            .iter()
            .map(|l| (l.fmt, l.template.as_str()))
            .collect();
        assert_eq!(
            fmt_tpl,
            vec![
                (NumFmt::Digit, "^1."),
                (NumFmt::HangulSyllable, "^2."),
                (NumFmt::Digit, "^3)"),
                (NumFmt::HangulSyllable, "^4)"),
                (NumFmt::Digit, "(^5)"),
                (NumFmt::HangulSyllable, "(^6)"),
                (NumFmt::CircledDigit, "^7"),
                (NumFmt::CircledHangulSyllable, "^8"),
            ]
        );

        // Page number: pgnp (bottom center + sideChar '-') control and ExtCtrl anchor.
        let first = &report.sections[0].paragraphs[0];
        let pgnp = first
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Generic(g) if g.ctrl_id == *b"pgnp" => Some(g),
                _ => None,
            })
            .expect("pgnp 있어야");
        let props = u32::from_le_bytes(pgnp.data[..4].try_into().unwrap());
        assert_eq!((props >> 8) & 0xFF, 5, "BOTTOM_CENTER");
        assert_eq!(
            u16::from_le_bytes(pgnp.data[10..12].try_into().unwrap()),
            u16::from(b'-')
        );
        assert!(
            first
                .chars
                .iter()
                .any(|ch| matches!(ch, HwpChar::ExtCtrl { ctrl_id, .. } if ctrl_id == b"pgnp"))
        );
    }

    #[test]
    fn official_profile_matrix() {
        let matrix = [
            ("official", "맑은 고딕", 1200, 160, 0, false),
            ("report", "함초롬바탕", 1500, 160, 4252, true),
            ("plan", "함초롬바탕", 1500, 160, 4252, true),
            ("notice", "맑은 고딕", 1500, 160, 2834, true),
            ("minutes", "함초롬바탕", 1400, 130, 0, false),
            ("press", "함초롬바탕", 1400, 160, 2834, true),
        ];

        for (name, font, body, spacing, header_footer, page_number) in matrix {
            let preset = OfficialPreset::parse(name).expect("canonical preset");
            let doc = from_markdown_with(
                "1. item\n",
                &MarkdownImportOptions {
                    preset: Some(preset),
                    ..Default::default()
                },
            );
            let page = doc.sections[0].paragraphs[0]
                .controls
                .iter()
                .find_map(|control| match control {
                    Control::SectionDef(section) => section.page,
                    _ => None,
                })
                .expect("page definition");
            assert_eq!(doc.header.fonts[0][0].name, font, "{name}");
            assert_eq!(doc.header.char_shapes[0].base_size, body, "{name}");
            assert!(
                doc.header
                    .para_shapes
                    .iter()
                    .all(|shape| shape.line_spacing == spacing),
                "{name}"
            );
            assert_eq!(page.margin_top.0, 5668, "{name}");
            assert_eq!(page.margin_bottom.0, 2834, "{name}");
            assert_eq!(page.margin_left.0, 5668, "{name}");
            assert_eq!(page.margin_right.0, 5668, "{name}");
            assert_eq!(page.margin_header.0, header_footer, "{name}");
            assert_eq!(page.margin_footer.0, header_footer, "{name}");
            assert_eq!(
                doc.sections[0].paragraphs[0]
                    .controls
                    .iter()
                    .filter(
                        |control| matches!(control, Control::Generic(g) if g.ctrl_id == *b"pgnp")
                    )
                    .count(),
                usize::from(page_number),
                "{name}"
            );
            assert_eq!(doc.header.numbering_levels[0].len(), 8, "{name}");
        }
    }

    // ── md + HTML mixing (contract docs/design/18) ──

    #[test]
    fn html_표_블록_혼합() {
        let doc = from_markdown(
            "본문입니다.\n\n<table>\n<tr><td colspan=\"2\">가로병합</td></tr>\n\
             <tr><td>a</td><td>b</td></tr>\n</table>\n",
        );
        let table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("HTML 표 컨트롤");
        assert_eq!((table.rows, table.cols), (2, 2));
        assert_eq!(table.cells.len(), 3, "앵커 셀만: {}", table.cells.len());
        assert_eq!(table.cells[0].col_span, 2);
        assert_eq!(table.row_cell_counts, vec![1, 2]);
    }

    #[test]
    fn 인라인_html_마크_왕복() {
        // <u>/<sup>/<sub> mixed into md are preserved through IR to md re-export (GH-8 symmetry).
        let doc = from_markdown("이건 <u>밑줄</u>이고 x<sup>2</sup>입니다\n");
        let md = crate::markdown::to_markdown(&doc);
        assert!(md.contains("<u>밑줄</u>"), "밑줄 왕복: {md}");
        assert!(md.contains("<sup>2</sup>"), "위첨자 왕복: {md}");
    }

    #[test]
    fn 계약_위반_html은_경고로_무시() {
        // Blocks with unsupported tags are ignored without breaking document generation (same as the existing image warning policy).
        let doc = from_markdown("본문\n\n<div><p>무시됨</p></div>\n");
        let text: String = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.chars)
            .filter_map(|c| match c {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(text.contains("본문"), "{text}");
        assert!(!text.contains("무시됨"), "{text}");
    }

    #[test]
    fn html_목록_병합_오프셋() {
        // When an HTML ol follows an md list, both numbering definitions must survive (offset merge).
        let doc = from_markdown("1. md 항목\n\n<ol><li>html 항목</li></ol>\n");
        assert_eq!(
            doc.header.numbering_levels.len(),
            2,
            "md+html 번호 정의: {}",
            doc.header.numbering_levels.len()
        );
        let text: String = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.chars)
            .filter_map(|c| match c {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(
            text.contains("md 항목") && text.contains("html 항목"),
            "{text}"
        );
    }

    #[test]
    fn html_두_블록_글꼴_face_리베이스() {
        // Merging two HTML blocks that use different additional fonts must rebase the second
        // block's face ids — otherwise both point at the first font (review regression).
        let md = "본문\n\n\
            <style>.cs0{font-family:\"글꼴가\",serif;font-size:10pt;}</style>\n\
            <table><tr><td><span class=\"cs0\">첫째</span></td></tr></table>\n\n\
            <style>.cs0{font-family:\"글꼴ㄴ\",serif;font-size:10pt;}</style>\n\
            <table><tr><td><span class=\"cs0\">둘째</span></td></tr></table>\n";
        let doc = from_markdown(md);
        let fonts: Vec<&str> = doc.header.fonts[0]
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            fonts.contains(&"글꼴가") && fonts.contains(&"글꼴ㄴ"),
            "두 글꼴 모두 등록: {fonts:?}"
        );
        // Check that each cell paragraph's shape points at the correct font.
        let mut seen = std::collections::BTreeMap::new();
        for para in &doc.sections[0].paragraphs {
            for control in &para.controls {
                let Control::Table(t) = control else {
                    continue;
                };
                for cell in &t.cells {
                    let text: String = cell.paragraphs[0]
                        .chars
                        .iter()
                        .filter_map(|c| match c {
                            HwpChar::Text(c) => Some(*c),
                            _ => None,
                        })
                        .collect();
                    let (_, cs_id) = cell.paragraphs[0].char_shape_runs[0];
                    seen.insert(text, cs_id.0);
                }
            }
        }
        let first = &doc.header.char_shapes[seen["첫째"] as usize];
        let second = &doc.header.char_shapes[seen["둘째"] as usize];
        assert_eq!(
            doc.header.fonts[0][first.face_ids[0] as usize].name,
            "글꼴가"
        );
        assert_eq!(
            doc.header.fonts[0][second.face_ids[0] as usize].name, "글꼴ㄴ",
            "두 번째 블록 face 리베이스"
        );
    }
}
