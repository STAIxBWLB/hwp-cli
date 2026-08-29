//! Canonical Korean official-document profile policy.

use hwp_model::{FaceName, HwpUnit, NumFmt, PageDef};

use crate::from_markdown::shapes;

/// A canonical official-document profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OfficialPreset {
    Official,
    Report,
    Plan,
    Notice,
    Minutes,
    Press,
}

/// Retired in 0.9.0: `gaejosik` named a writing style, not a document class.
const GAEJOSIK_IS_NOT_A_PROFILE: &str = "개조식은 문체이며 공문서 프로필이 아닙니다. 보고서·계획서는 --preset report|plan을 쓰고, 명사형 종결은 본문에서 적용하세요.";

/// Heading number ladder selected for markdown import (#125).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HeadingNumbering {
    /// Section ladder (pre-#125 behavior): `#`~`###` get `1.` / `1-1.` / `1-1-1.` — a
    /// dotted join of the ancestor counters. A downstream project (RISE annual reports)
    /// depends on exactly this, so it stays the default.
    #[default]
    Section,
    /// Korean official-document ladder: `#` is the document title and gets no number;
    /// `##`~`#####` get `Ⅰ.` / `1.` / `가.` / `1)`, each level counting independently.
    Official,
}

impl OfficialPreset {
    /// Heading number ladder this preset selects for markdown import (#125): official and
    /// report use the official ladder; every other profile keeps the section ladder.
    pub const fn heading_numbering(self) -> HeadingNumbering {
        match self {
            Self::Official | Self::Report => HeadingNumbering::Official,
            _ => HeadingNumbering::Section,
        }
    }

    /// Source-compatible legacy spelling; new callers should use [`Self::Official`].
    #[allow(non_upper_case_globals)]
    pub const Gian: Self = Self::Official;

    /// Parse a canonical name or supported legacy/Korean alias into one canonical profile.
    pub fn parse(value: &str) -> Result<Self, String> {
        let ascii = value.to_ascii_lowercase();
        match ascii.as_str() {
            "official" | "gian" | "gongmun" => Ok(Self::Official),
            "report" | "bogoseo" => Ok(Self::Report),
            "plan" => Ok(Self::Plan),
            "notice" => Ok(Self::Notice),
            "minutes" => Ok(Self::Minutes),
            "press" => Ok(Self::Press),
            _ => match value {
                "기안" | "기안문" | "공문" | "공문서" => Ok(Self::Official),
                "보고" | "보고서" => Ok(Self::Report),
                "계획" | "계획서" | "사업계획" | "사업계획서" => Ok(Self::Plan),
                "공고" | "공고문" | "고시" => Ok(Self::Notice),
                "회의록" | "회의기록" => Ok(Self::Minutes),
                "보도" | "보도자료" => Ok(Self::Press),
                // 개조식 is a sentence-ending style, not a document class, so it has no
                // profile row. Name it explicitly instead of leaving it to the catch-all.
                "개조식" => Err(GAEJOSIK_IS_NOT_A_PROFILE.to_string()),
                _ if ascii == "gaejosik" => Err(GAEJOSIK_IS_NOT_A_PROFILE.to_string()),
                _ => Err(format!("알 수 없는 공문서 프리셋: {value}")),
            },
        }
    }

    /// Stable canonical CLI/MCP name.
    pub const fn name(self) -> &'static str {
        profile(self).name
    }
}

/// Font family selected by a profile body style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficialBodyFont {
    MalgunGothic,
    HcrBatang,
}

/// Immutable policy row for one official profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfficialProfile {
    pub preset: OfficialPreset,
    pub name: &'static str,
    pub body_font: OfficialBodyFont,
    /// HWP's 1/100 point unit.
    pub body_size: i32,
    pub line_spacing: i32,
    /// Header and footer margins in HWPUNIT.
    pub header_footer: i32,
    pub page_number: bool,
}

const MM_10: i32 = 2834;
const MM_15: i32 = 4252;
const MM_20: i32 = 5668;

const PROFILES: [OfficialProfile; 6] = [
    OfficialProfile {
        preset: OfficialPreset::Official,
        name: "official",
        body_font: OfficialBodyFont::MalgunGothic,
        body_size: 1200,
        line_spacing: 160,
        header_footer: 0,
        page_number: false,
    },
    OfficialProfile {
        preset: OfficialPreset::Report,
        name: "report",
        body_font: OfficialBodyFont::HcrBatang,
        body_size: 1500,
        line_spacing: 160,
        header_footer: MM_15,
        page_number: true,
    },
    OfficialProfile {
        preset: OfficialPreset::Plan,
        name: "plan",
        body_font: OfficialBodyFont::HcrBatang,
        body_size: 1500,
        line_spacing: 160,
        header_footer: MM_15,
        page_number: true,
    },
    OfficialProfile {
        preset: OfficialPreset::Notice,
        name: "notice",
        body_font: OfficialBodyFont::MalgunGothic,
        body_size: 1500,
        line_spacing: 160,
        header_footer: MM_10,
        page_number: true,
    },
    OfficialProfile {
        preset: OfficialPreset::Minutes,
        name: "minutes",
        body_font: OfficialBodyFont::HcrBatang,
        body_size: 1400,
        line_spacing: 130,
        header_footer: 0,
        page_number: false,
    },
    OfficialProfile {
        preset: OfficialPreset::Press,
        name: "press",
        body_font: OfficialBodyFont::HcrBatang,
        body_size: 1400,
        line_spacing: 160,
        header_footer: MM_10,
        page_number: true,
    },
];

/// Return the complete immutable profile registry.
pub const fn profiles() -> &'static [OfficialProfile; 6] {
    &PROFILES
}

/// Return one profile row by canonical preset.
pub const fn profile(preset: OfficialPreset) -> &'static OfficialProfile {
    match preset {
        OfficialPreset::Official => &PROFILES[0],
        OfficialPreset::Report => &PROFILES[1],
        OfficialPreset::Plan => &PROFILES[2],
        OfficialPreset::Notice => &PROFILES[3],
        OfficialPreset::Minutes => &PROFILES[4],
        OfficialPreset::Press => &PROFILES[5],
    }
}

/// Per-side page margin overrides in HWPUNIT, applied only after defaults are resolved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageMarginOverrides {
    pub top: Option<HwpUnit>,
    pub bottom: Option<HwpUnit>,
    pub left: Option<HwpUnit>,
    pub right: Option<HwpUnit>,
}

impl PageMarginOverrides {
    /// Replace only explicitly supplied page margins.
    pub fn apply(self, page: &mut PageDef) {
        if let Some(value) = self.top {
            page.margin_top = value;
        }
        if let Some(value) = self.bottom {
            page.margin_bottom = value;
        }
        if let Some(value) = self.left {
            page.margin_left = value;
        }
        if let Some(value) = self.right {
            page.margin_right = value;
        }
    }
}

/// The plain document's existing page defaults.
pub const fn plain_page_def() -> PageDef {
    PageDef {
        width: HwpUnit(59528),
        height: HwpUnit(84186),
        margin_left: HwpUnit(8504),
        margin_right: HwpUnit(8504),
        margin_top: HwpUnit(MM_20),
        margin_bottom: HwpUnit(MM_15),
        margin_header: HwpUnit(MM_15),
        margin_footer: HwpUnit(MM_15),
        gutter: HwpUnit(0),
        attr: 0,
    }
}

/// Resolve profile or plain page defaults before applying side-specific overrides.
pub const fn page_def(preset: Option<OfficialPreset>) -> PageDef {
    match preset {
        None => plain_page_def(),
        Some(preset) => {
            let profile = profile(preset);
            PageDef {
                width: HwpUnit(59528),
                height: HwpUnit(84186),
                margin_left: HwpUnit(MM_20),
                margin_right: HwpUnit(MM_20),
                margin_top: HwpUnit(MM_20),
                margin_bottom: HwpUnit(MM_10),
                margin_header: HwpUnit(profile.header_footer),
                margin_footer: HwpUnit(profile.header_footer),
                gutter: HwpUnit(0),
                attr: 0,
            }
        }
    }
}

/// Apply a profile's typography and eight-level numbering to a default-header palette.
pub fn apply_profile(header: &mut hwp_model::DocHeader, preset: OfficialPreset) {
    let profile = profile(preset);
    if profile.body_font == OfficialBodyFont::MalgunGothic {
        for slot in &mut header.fonts {
            slot[0] = FaceName {
                name: "맑은 고딕".to_string(),
                attr: 0x01,
                default_name: Some("Malgun Gothic".to_string()),
                ..FaceName::default()
            };
        }
    }

    let headings = if profile.body_size >= 1500 {
        [1800, 1700, 1600, 1550, 1500, 1500]
    } else {
        [1500, 1400, 1300, 1200, profile.body_size, profile.body_size]
    };
    for (index, shape) in header.char_shapes.iter_mut().enumerate() {
        shape.base_size = match index.checked_sub(shapes::HEADING_BASE as usize) {
            Some(level) if level < headings.len() => headings[level],
            _ => profile.body_size,
        };
    }
    for shape in &mut header.para_shapes {
        shape.line_spacing_old = profile.line_spacing;
        shape.line_spacing = profile.line_spacing;
    }
    for levels in &mut header.numbering_levels {
        for (index, level) in levels.iter_mut().enumerate() {
            let (format, template) = match index {
                0 => (NumFmt::Digit, "^1."),
                1 => (NumFmt::HangulSyllable, "^2."),
                2 => (NumFmt::Digit, "^3)"),
                3 => (NumFmt::HangulSyllable, "^4)"),
                4 => (NumFmt::Digit, "(^5)"),
                5 => (NumFmt::HangulSyllable, "(^6)"),
                6 => (NumFmt::CircledDigit, "^7"),
                7 => (NumFmt::CircledHangulSyllable, "^8"),
                _ => break,
            };
            level.fmt = format;
            level.template = template.to_string();
        }
    }
}

/// Whether a canonical profile includes the proven bottom-center page-number control.
pub const fn has_page_number(preset: Option<OfficialPreset>) -> bool {
    match preset {
        Some(preset) => profile(preset).page_number,
        None => false,
    }
}
