//! Versioned native structured-authoring contract for `hwp compose`.
//!
//! The normative machine-readable schema is `schemas/document-spec-v1.schema.json`;
//! this module is its serde implementation. All structs reject unknown fields so a
//! misspelled or future request cannot silently degrade into a different document.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::{Digest as _, Sha256};

pub const MAX_SPEC_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SECTIONS: usize = 64;
pub const MAX_BLOCKS: usize = 20_000;
pub const MAX_RUNS: usize = 100_000;
pub const MAX_TABLE_CELLS: usize = 100_000;
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TOTAL_ASSET_BYTES: u64 = 128 * 1024 * 1024;
const MAX_NESTING: usize = 16;
const MAX_TEXT_CHARS: usize = 2_000_000;
const MAX_NAME_CHARS: usize = 128;
const MAX_SHORT_TEXT_CHARS: usize = 4096;
const MAX_DESCRIPTION_CHARS: usize = 32_768;
const MAX_EQUATION_CHARS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecVersion {
    #[serde(rename = "1.0")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSpec {
    pub version: SpecVersion,
    #[serde(default)]
    pub metadata: MetadataSpec,
    #[serde(default)]
    pub page: PageSpec,
    #[serde(default)]
    pub styles: BTreeMap<String, StyleSpec>,
    #[serde(default)]
    pub lists: BTreeMap<String, ListSpec>,
    pub sections: Vec<SectionSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSpec {
    #[serde(default = "default_page_width")]
    pub width_mm: f32,
    #[serde(default = "default_page_height")]
    pub height_mm: f32,
    #[serde(default)]
    pub orientation: PageOrientation,
    #[serde(default = "default_margin")]
    pub margin_left_mm: f32,
    #[serde(default = "default_margin")]
    pub margin_right_mm: f32,
    #[serde(default = "default_margin")]
    pub margin_top_mm: f32,
    #[serde(default = "default_margin")]
    pub margin_bottom_mm: f32,
    #[serde(default = "default_header_footer_margin")]
    pub margin_header_mm: f32,
    #[serde(default = "default_header_footer_margin")]
    pub margin_footer_mm: f32,
    #[serde(default)]
    pub gutter_mm: f32,
}

const fn default_page_width() -> f32 {
    210.0
}

const fn default_page_height() -> f32 {
    297.0
}

const fn default_margin() -> f32 {
    20.0
}

const fn default_header_footer_margin() -> f32 {
    10.0
}

impl Default for PageSpec {
    fn default() -> Self {
        Self {
            width_mm: default_page_width(),
            height_mm: default_page_height(),
            orientation: PageOrientation::Portrait,
            margin_left_mm: default_margin(),
            margin_right_mm: default_margin(),
            margin_top_mm: default_margin(),
            margin_bottom_mm: default_margin(),
            margin_header_mm: default_header_footer_margin(),
            margin_footer_mm: default_header_footer_margin(),
            gutter_mm: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageOrientation {
    #[default]
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size_pt: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Alignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_left_mm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_right_mm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing_before_pt: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spacing_after_pt: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height_percent: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_with_next: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    Justify,
    Left,
    Right,
    Center,
    Distribute,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListSpec {
    pub kind: ListKind,
    pub levels: Vec<ListLevelSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Ordered,
    Bullet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListLevelSpec {
    pub marker: String,
    #[serde(default = "default_list_start")]
    pub start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

const fn default_list_start() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<PageSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<HeaderFooterSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<HeaderFooterSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_number: Option<PageNumberSpec>,
    pub blocks: Vec<BlockSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderFooterSpec {
    #[serde(default)]
    pub default: Vec<BlockSpec>,
    #[serde(default)]
    pub first: Vec<BlockSpec>,
    #[serde(default)]
    pub odd: Vec<BlockSpec>,
    #[serde(default)]
    pub even: Vec<BlockSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageNumberSpec {
    pub position: PageNumberPosition,
    #[serde(default)]
    pub format: PageNumberFormat,
    #[serde(default = "default_list_start")]
    pub start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageNumberPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    Inside,
    Outside,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageNumberFormat {
    #[default]
    Decimal,
    RomanUpper,
    RomanLower,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockSpec {
    Paragraph {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        list: Option<ListRefSpec>,
        #[serde(default)]
        keep_with_next: bool,
        runs: Vec<RunSpec>,
    },
    Table {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width_mm: Option<f32>,
        columns: Vec<TableColumnSpec>,
        rows: Vec<TableRowSpec>,
    },
    Image {
        path: PathBuf,
        width_mm: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height_mm: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
        #[serde(default)]
        placement: ImagePlacement,
    },
    Equation {
        script: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width_mm: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height_mm: Option<f32>,
    },
    Field {
        name: String,
        #[serde(default)]
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    Break {
        kind: BreakKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRefSpec {
    pub name: String,
    #[serde(default)]
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunSpec {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
        #[serde(default, flatten)]
        format: RunFormatSpec,
    },
    Field {
        name: String,
        #[serde(default)]
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
    Equation {
        script: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width_mm: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height_mm: Option<f32>,
    },
    Image {
        path: PathBuf,
        width_mm: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height_mm: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
    },
    LineBreak,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFormatSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size_pt: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePlacement {
    #[default]
    Inline,
    Floating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakKind {
    Page,
    Column,
    Section,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableColumnSpec {
    pub width_mm: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableRowSpec {
    pub cells: Vec<TableCellSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableCellSpec {
    #[serde(default = "default_span")]
    pub col_span: u16,
    #[serde(default = "default_span")]
    pub row_span: u16,
    #[serde(default)]
    pub blocks: Vec<BlockSpec>,
}

const fn default_span() -> u16 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecInputFormat {
    Json,
    Yaml,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpecIssue {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeReport {
    pub schema_version: SpecVersion,
    pub output: String,
    pub dry_run: bool,
    pub deterministic: bool,
    pub native: bool,
    pub visual_fallback_allowed: bool,
    pub visual_fallback_used: Vec<String>,
    pub sections: usize,
    pub paragraphs: usize,
    pub tables: usize,
    pub images: usize,
    pub equations: usize,
    pub fields: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum ComposeError {
    Parse {
        format: SpecInputFormat,
        message: String,
    },
    Validation {
        issues: Vec<SpecIssue>,
    },
    Asset {
        path: PathBuf,
        message: String,
    },
    Compile {
        path: String,
        message: String,
    },
}

impl ComposeError {
    pub fn issues(&self) -> Vec<SpecIssue> {
        match self {
            Self::Validation { issues } => issues.clone(),
            Self::Parse { format, message } => vec![SpecIssue {
                code: "parse_error".to_string(),
                path: "$".to_string(),
                message: format!("{format:?}: {message}"),
            }],
            Self::Asset { path, message } => vec![SpecIssue {
                code: "asset_error".to_string(),
                path: path.display().to_string(),
                message: message.clone(),
            }],
            Self::Compile { path, message } => vec![SpecIssue {
                code: "compile_error".to_string(),
                path: path.clone(),
                message: message.clone(),
            }],
        }
    }
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let issues = self.issues();
        write!(
            f,
            "{}",
            issues
                .iter()
                .map(|issue| format!("{} at {}: {}", issue.code, issue.path, issue.message))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

impl std::error::Error for ComposeError {}

pub struct CompiledSpec {
    pub document: hwp_model::Document,
    pub report: ComposeReport,
}

pub fn parse_spec(input: &str, format: SpecInputFormat) -> Result<DocumentSpec, ComposeError> {
    if input.len() > MAX_SPEC_BYTES {
        return Err(ComposeError::Validation {
            issues: vec![SpecIssue {
                code: "limit_exceeded".to_string(),
                path: "$".to_string(),
                message: format!("spec is {} bytes; maximum is {MAX_SPEC_BYTES}", input.len()),
            }],
        });
    }
    match format {
        SpecInputFormat::Json => serde_json::from_str(input).map_err(|error| ComposeError::Parse {
            format,
            message: error.to_string(),
        }),
        SpecInputFormat::Yaml => serde_yaml::from_str(input).map_err(|error| ComposeError::Parse {
            format,
            message: error.to_string(),
        }),
    }
}

pub fn infer_input_format(path: &Path) -> Result<SpecInputFormat, ComposeError> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => Ok(SpecInputFormat::Json),
        Some("yaml" | "yml") => Ok(SpecInputFormat::Yaml),
        _ => Err(ComposeError::Validation {
            issues: vec![SpecIssue {
                code: "unsupported_input_format".to_string(),
                path: "$".to_string(),
                message: "spec extension must be .json, .yaml, or .yml".to_string(),
            }],
        }),
    }
}

pub fn validate_spec(spec: &DocumentSpec, _base_dir: &Path) -> Result<(), ComposeError> {
    let mut validator = Validator {
        spec,
        issues: Vec::new(),
        blocks: 0,
        runs: 0,
        cells: 0,
        text_chars: 0,
    };
    validator.validate();
    if validator.issues.is_empty() {
        Ok(())
    } else {
        Err(ComposeError::Validation {
            issues: validator.issues,
        })
    }
}

struct Validator<'a> {
    spec: &'a DocumentSpec,
    issues: Vec<SpecIssue>,
    blocks: usize,
    runs: usize,
    cells: usize,
    text_chars: usize,
}

impl Validator<'_> {
    fn issue(&mut self, code: &str, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(SpecIssue {
            code: code.to_string(),
            path: path.into(),
            message: message.into(),
        });
    }

    fn validate(&mut self) {
        if self.spec.sections.is_empty() {
            self.issue("required", "$.sections", "at least one section is required");
        }
        if self.spec.sections.len() > MAX_SECTIONS {
            self.issue(
                "limit_exceeded",
                "$.sections",
                format!("at most {MAX_SECTIONS} sections are allowed"),
            );
        }
        self.validate_page(&self.spec.page, "$.page");
        for (name, value, maximum) in [
            (
                "title",
                self.spec.metadata.title.as_deref(),
                MAX_SHORT_TEXT_CHARS,
            ),
            (
                "author",
                self.spec.metadata.author.as_deref(),
                MAX_SHORT_TEXT_CHARS,
            ),
            (
                "subject",
                self.spec.metadata.subject.as_deref(),
                MAX_SHORT_TEXT_CHARS,
            ),
            (
                "keywords",
                self.spec.metadata.keywords.as_deref(),
                MAX_SHORT_TEXT_CHARS,
            ),
            (
                "description",
                self.spec.metadata.description.as_deref(),
                MAX_DESCRIPTION_CHARS,
            ),
        ] {
            if let Some(value) = value
                && value.chars().count() > maximum
            {
                self.issue(
                    "limit_exceeded",
                    format!("$.metadata.{name}"),
                    format!("value cannot exceed {maximum} Unicode scalars"),
                );
            }
        }
        if self.spec.styles.len() > 256 {
            self.issue(
                "limit_exceeded",
                "$.styles",
                "at most 256 styles are allowed",
            );
        }
        if self.spec.lists.len() > 256 {
            self.issue("limit_exceeded", "$.lists", "at most 256 lists are allowed");
        }
        for (name, style) in &self.spec.styles {
            let path = format!("$.styles.{name}");
            if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                self.issue(
                    "invalid_name",
                    &path,
                    format!("style name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                );
            }
            if let Some(parent) = &style.based_on {
                if parent.is_empty() || parent.chars().count() > MAX_NAME_CHARS {
                    self.issue(
                        "invalid_name",
                        format!("{path}.based_on"),
                        format!("base style name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                    );
                } else if !self.spec.styles.contains_key(parent) {
                    self.issue(
                        "unknown_reference",
                        format!("{path}.based_on"),
                        format!("style {parent:?} does not exist"),
                    );
                }
            }
            self.validate_style(style, &path);
        }
        self.validate_style_cycles();
        for (name, list) in &self.spec.lists {
            let path = format!("$.lists.{name}");
            if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                self.issue(
                    "invalid_name",
                    &path,
                    format!("list name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                );
            }
            if list.levels.is_empty() || list.levels.len() > 7 {
                self.issue(
                    "invalid_list",
                    &path,
                    "list must define between 1 and 7 levels",
                );
            }
            for (index, level) in list.levels.iter().enumerate() {
                let level_path = format!("{path}.levels[{index}]");
                if level.start == 0 {
                    self.issue(
                        "invalid_value",
                        format!("{level_path}.start"),
                        "start must be at least 1",
                    );
                }
                if list.kind == ListKind::Bullet && level.start != 1 {
                    self.issue(
                        "invalid_value",
                        format!("{level_path}.start"),
                        "bullet list start must be 1 because bullets do not count",
                    );
                }
                if level.marker.is_empty() || level.marker.chars().count() > 64 {
                    self.issue(
                        "invalid_value",
                        format!("{level_path}.marker"),
                        "marker must contain 1..=64 characters",
                    );
                }
                match list.kind {
                    ListKind::Ordered if !level.marker.contains("{n}") => self.issue(
                        "invalid_marker",
                        format!("{level_path}.marker"),
                        "ordered marker must contain {n}",
                    ),
                    ListKind::Bullet if level.marker.chars().count() != 1 => self.issue(
                        "invalid_marker",
                        format!("{level_path}.marker"),
                        "bullet marker must be exactly one Unicode scalar",
                    ),
                    _ => {}
                }
                if let Some(style) = &level.style {
                    self.validate_style_ref(style, format!("{level_path}.style"));
                }
            }
        }
        let mut section_ids = BTreeSet::new();
        for (index, section) in self.spec.sections.iter().enumerate() {
            let path = format!("$.sections[{index}]");
            if let Some(page) = &section.page {
                self.validate_page(page, &format!("{path}.page"));
            }
            if let Some(page_number) = &section.page_number {
                self.validate_page_number(page_number, &format!("{path}.page_number"));
            }
            if let Some(id) = &section.id
                && (id.is_empty() || id.chars().count() > MAX_NAME_CHARS)
            {
                self.issue(
                    "invalid_name",
                    format!("{path}.id"),
                    format!("section id must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                );
            } else if let Some(id) = &section.id
                && !section_ids.insert(id)
            {
                self.issue(
                    "duplicate_id",
                    format!("{path}.id"),
                    format!("section id {id:?} is already used"),
                );
            }
            let page = section.page.as_ref().unwrap_or(&self.spec.page);
            let body_width_mm = writable_page_width(*page);
            if let Some(header) = &section.header {
                self.validate_header_footer(header, &format!("{path}.header"), 0, body_width_mm);
            }
            if let Some(footer) = &section.footer {
                self.validate_header_footer(footer, &format!("{path}.footer"), 0, body_width_mm);
            }
            self.validate_blocks(
                &section.blocks,
                &format!("{path}.blocks"),
                0,
                true,
                body_width_mm,
            );
        }
        if self.blocks > MAX_BLOCKS {
            self.issue(
                "limit_exceeded",
                "$",
                format!(
                    "document contains {} blocks; maximum is {MAX_BLOCKS}",
                    self.blocks
                ),
            );
        }
        if self.runs > MAX_RUNS {
            self.issue(
                "limit_exceeded",
                "$",
                format!(
                    "document contains {} runs; maximum is {MAX_RUNS}",
                    self.runs
                ),
            );
        }
        if self.cells > MAX_TABLE_CELLS {
            self.issue(
                "limit_exceeded",
                "$",
                format!(
                    "document contains {} table cells; maximum is {MAX_TABLE_CELLS}",
                    self.cells
                ),
            );
        }
        if self.text_chars > MAX_TEXT_CHARS {
            self.issue(
                "limit_exceeded",
                "$",
                format!(
                    "document contains {} text characters; maximum is {MAX_TEXT_CHARS}",
                    self.text_chars
                ),
            );
        }
    }

    fn validate_page(&mut self, page: &PageSpec, path: &str) {
        for (name, value) in [("width_mm", page.width_mm), ("height_mm", page.height_mm)] {
            if !value.is_finite() || !(25.0..=2000.0).contains(&value) {
                self.issue(
                    "invalid_dimension",
                    format!("{path}.{name}"),
                    "page dimension must be finite and within 25..=2000 mm",
                );
            }
        }
        for (name, value) in [
            ("margin_left_mm", page.margin_left_mm),
            ("margin_right_mm", page.margin_right_mm),
            ("margin_top_mm", page.margin_top_mm),
            ("margin_bottom_mm", page.margin_bottom_mm),
            ("margin_header_mm", page.margin_header_mm),
            ("margin_footer_mm", page.margin_footer_mm),
            ("gutter_mm", page.gutter_mm),
        ] {
            if !value.is_finite() || !(0.0..=1000.0).contains(&value) {
                self.issue(
                    "invalid_dimension",
                    format!("{path}.{name}"),
                    "margin must be finite and within 0..=1000 mm",
                );
            }
        }
        let (physical_width, physical_height) = physical_page_size(*page);
        if page.margin_left_mm + page.margin_right_mm + page.gutter_mm >= physical_width {
            self.issue(
                "invalid_page",
                path,
                "horizontal margins and gutter leave no writable width",
            );
        }
        if page.margin_top_mm + page.margin_bottom_mm >= physical_height {
            self.issue(
                "invalid_page",
                path,
                "vertical margins leave no writable height",
            );
        }
    }

    fn validate_style(&mut self, style: &StyleSpec, path: &str) {
        if style.keep_with_next == Some(true) {
            self.issue(
                "unsupported_native",
                format!("{path}.keep_with_next"),
                "keep_with_next is not representable by both native writers in v1",
            );
        }
        if let Some(size) = style.font_size_pt
            && (!size.is_finite() || !(1.0..=1000.0).contains(&size))
        {
            self.issue(
                "invalid_value",
                format!("{path}.font_size_pt"),
                "font size must be finite and within 1..=1000 pt",
            );
        }
        if let Some(font) = &style.font_family
            && (font.is_empty() || font.chars().count() > 256)
        {
            self.issue(
                "invalid_value",
                format!("{path}.font_family"),
                "font_family must be 1..=256 Unicode scalars",
            );
        }
        for (name, value) in [
            ("margin_left_mm", style.margin_left_mm),
            ("margin_right_mm", style.margin_right_mm),
            ("spacing_before_pt", style.spacing_before_pt),
            ("spacing_after_pt", style.spacing_after_pt),
        ] {
            if let Some(value) = value
                && (!value.is_finite() || !(0.0..=1000.0).contains(&value))
            {
                self.issue(
                    "invalid_value",
                    format!("{path}.{name}"),
                    "value must be finite and within 0..=1000",
                );
            }
        }
        if let Some(value) = style.line_height_percent
            && !(50..=1000).contains(&value)
        {
            self.issue(
                "invalid_value",
                format!("{path}.line_height_percent"),
                "line height must be within 50..=1000 percent",
            );
        }
        for (name, color) in [
            ("color", style.color.as_deref()),
            ("background", style.background.as_deref()),
        ] {
            if let Some(color) = color
                && parse_color(color).is_none()
            {
                self.issue(
                    "invalid_color",
                    format!("{path}.{name}"),
                    "color must be #RRGGBB",
                );
            }
        }
    }

    fn validate_style_cycles(&mut self) {
        for name in self.spec.styles.keys() {
            let mut seen = BTreeSet::new();
            let mut current = Some(name.as_str());
            while let Some(style_name) = current {
                if !seen.insert(style_name.to_string()) {
                    self.issue(
                        "reference_cycle",
                        format!("$.styles.{name}.based_on"),
                        format!("style inheritance cycle includes {style_name:?}"),
                    );
                    break;
                }
                current = self
                    .spec
                    .styles
                    .get(style_name)
                    .and_then(|style| style.based_on.as_deref());
            }
        }
    }

    fn validate_page_number(&mut self, page_number: &PageNumberSpec, path: &str) {
        if page_number.start == 0 || page_number.start > u32::from(u16::MAX) {
            self.issue(
                "invalid_value",
                format!("{path}.start"),
                format!("start must be within 1..={}", u16::MAX),
            );
        }
        if page_number.format != PageNumberFormat::Decimal {
            self.issue(
                "unsupported_native",
                format!("{path}.format"),
                "v1 native writers currently support decimal page numbers only",
            );
        }
        match (&page_number.prefix, &page_number.suffix) {
            (None, None) => {}
            (Some(prefix), Some(suffix)) if prefix == suffix && prefix.chars().count() == 1 => {}
            _ => self.issue(
                "unsupported_native",
                path,
                "native side characters require equal one-character prefix and suffix",
            ),
        }
    }

    fn validate_header_footer(
        &mut self,
        header_footer: &HeaderFooterSpec,
        path: &str,
        depth: usize,
        body_width_mm: f32,
    ) {
        if !header_footer.first.is_empty() {
            self.issue(
                "unsupported_native",
                format!("{path}.first"),
                "distinct first-page header/footer is not representable by both native writers",
            );
        }
        self.validate_blocks(
            &header_footer.default,
            &format!("{path}.default"),
            depth + 1,
            false,
            body_width_mm,
        );
        self.validate_blocks(
            &header_footer.first,
            &format!("{path}.first"),
            depth + 1,
            false,
            body_width_mm,
        );
        self.validate_blocks(
            &header_footer.odd,
            &format!("{path}.odd"),
            depth + 1,
            false,
            body_width_mm,
        );
        self.validate_blocks(
            &header_footer.even,
            &format!("{path}.even"),
            depth + 1,
            false,
            body_width_mm,
        );
    }

    fn validate_blocks(
        &mut self,
        blocks: &[BlockSpec],
        path: &str,
        depth: usize,
        allow_section_break: bool,
        body_width_mm: f32,
    ) {
        if depth > MAX_NESTING {
            self.issue(
                "limit_exceeded",
                path,
                format!("block nesting exceeds {MAX_NESTING}"),
            );
            return;
        }
        self.blocks = self.blocks.saturating_add(blocks.len());
        for (index, block) in blocks.iter().enumerate() {
            let block_path = format!("{path}[{index}]");
            match block {
                BlockSpec::Paragraph {
                    style,
                    list,
                    keep_with_next,
                    runs,
                } => {
                    if let Some(style) = style {
                        self.validate_style_ref(style, format!("{block_path}.style"));
                    }
                    if *keep_with_next {
                        self.issue(
                            "unsupported_native",
                            format!("{block_path}.keep_with_next"),
                            "keep_with_next is not representable by both native writers in v1",
                        );
                    }
                    if let Some(list_ref) = list {
                        if list_ref.name.is_empty()
                            || list_ref.name.chars().count() > MAX_NAME_CHARS
                        {
                            self.issue(
                                "invalid_name",
                                format!("{block_path}.list.name"),
                                format!("list name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                            );
                        }
                        match self.spec.lists.get(&list_ref.name) {
                            None => self.issue(
                                "unknown_reference",
                                format!("{block_path}.list.name"),
                                format!("list {:?} does not exist", list_ref.name),
                            ),
                            Some(list) if usize::from(list_ref.level) >= list.levels.len() => {
                                self.issue(
                                    "invalid_reference",
                                    format!("{block_path}.list.level"),
                                    format!(
                                        "level {} is outside list {:?}",
                                        list_ref.level, list_ref.name
                                    ),
                                );
                            }
                            _ => {}
                        }
                    }
                    if runs.is_empty() {
                        self.issue(
                            "required",
                            format!("{block_path}.runs"),
                            "paragraph requires at least one run",
                        );
                    }
                    self.runs = self.runs.saturating_add(runs.len());
                    for (run_index, run) in runs.iter().enumerate() {
                        self.validate_run(
                            run,
                            &format!("{block_path}.runs[{run_index}]"),
                            body_width_mm,
                        );
                    }
                }
                BlockSpec::Table {
                    width_mm,
                    columns,
                    rows,
                } => {
                    self.validate_table(
                        *width_mm,
                        columns,
                        rows,
                        &block_path,
                        depth,
                        body_width_mm,
                    );
                }
                BlockSpec::Image {
                    path,
                    width_mm,
                    height_mm,
                    alt,
                    ..
                } => {
                    self.validate_positive_dimension(*width_mm, &format!("{block_path}.width_mm"));
                    if *width_mm > body_width_mm {
                        self.issue(
                            "layout_overflow",
                            format!("{block_path}.width_mm"),
                            format!("image width exceeds body width {body_width_mm:.2} mm"),
                        );
                    }
                    if let Some(height) = height_mm {
                        self.validate_positive_dimension(
                            *height,
                            &format!("{block_path}.height_mm"),
                        );
                    }
                    self.validate_asset(path, &format!("{block_path}.path"));
                    if alt
                        .as_ref()
                        .is_some_and(|alt| alt.chars().count() > MAX_SHORT_TEXT_CHARS)
                    {
                        self.issue(
                            "limit_exceeded",
                            format!("{block_path}.alt"),
                            format!(
                                "alt text cannot exceed {MAX_SHORT_TEXT_CHARS} Unicode scalars"
                            ),
                        );
                    }
                    if alt.as_ref().is_some_and(|alt| !alt.is_empty()) {
                        self.issue(
                            "unsupported_native",
                            format!("{block_path}.alt"),
                            "image alt text is not representable by both native writers in v1",
                        );
                    }
                }
                BlockSpec::Equation {
                    script,
                    width_mm: Some(width_mm),
                    height_mm,
                } => {
                    if script.is_empty() {
                        self.issue(
                            "invalid_value",
                            format!("{block_path}.script"),
                            "equation script cannot be empty",
                        );
                    }
                    if script.chars().count() > MAX_EQUATION_CHARS {
                        self.issue(
                            "limit_exceeded",
                            format!("{block_path}.script"),
                            format!(
                                "equation script cannot exceed {MAX_EQUATION_CHARS} characters"
                            ),
                        );
                    }
                    self.validate_positive_dimension(*width_mm, &format!("{block_path}.width_mm"));
                    if let Some(height) = height_mm {
                        self.validate_positive_dimension(
                            *height,
                            &format!("{block_path}.height_mm"),
                        );
                    }
                }
                BlockSpec::Equation {
                    script,
                    width_mm: None,
                    height_mm,
                } => {
                    if script.is_empty() {
                        self.issue(
                            "invalid_value",
                            format!("{block_path}.script"),
                            "equation script cannot be empty",
                        );
                    }
                    if script.chars().count() > MAX_EQUATION_CHARS {
                        self.issue(
                            "limit_exceeded",
                            format!("{block_path}.script"),
                            format!(
                                "equation script cannot exceed {MAX_EQUATION_CHARS} characters"
                            ),
                        );
                    }
                    if let Some(height) = height_mm {
                        self.validate_positive_dimension(
                            *height,
                            &format!("{block_path}.height_mm"),
                        );
                    }
                }
                BlockSpec::Field { name, value, style } => {
                    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                        self.issue(
                            "invalid_value",
                            format!("{block_path}.name"),
                            format!("field name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                        );
                    }
                    self.text_chars = self.text_chars.saturating_add(value.chars().count());
                    if value.chars().count() > MAX_SHORT_TEXT_CHARS {
                        self.issue(
                            "limit_exceeded",
                            format!("{block_path}.value"),
                            format!(
                                "field value cannot exceed {MAX_SHORT_TEXT_CHARS} Unicode scalars"
                            ),
                        );
                    }
                    if contains_disallowed_text_control(value) {
                        self.issue(
                            "invalid_text",
                            format!("{block_path}.value"),
                            "control characters are not allowed; use explicit break blocks",
                        );
                    }
                    if let Some(style) = style {
                        self.validate_style_ref(style, format!("{block_path}.style"));
                    }
                }
                BlockSpec::Break {
                    kind: BreakKind::Section,
                } if !allow_section_break => self.issue(
                    "unsupported_context",
                    &block_path,
                    "section break is not allowed inside a header, footer, or table cell",
                ),
                BlockSpec::Break { .. } => {}
            }
        }
    }

    fn validate_run(&mut self, run: &RunSpec, path: &str, body_width_mm: f32) {
        match run {
            RunSpec::Text {
                text,
                style,
                format,
            } => {
                self.text_chars = self.text_chars.saturating_add(text.chars().count());
                if text.chars().count() > MAX_TEXT_CHARS {
                    self.issue(
                        "limit_exceeded",
                        format!("{path}.text"),
                        format!("one text run cannot exceed {MAX_TEXT_CHARS} characters"),
                    );
                }
                if contains_disallowed_text_control(text) {
                    self.issue(
                        "invalid_text",
                        format!("{path}.text"),
                        "control characters are not allowed; use line_break runs",
                    );
                }
                if let Some(style) = style {
                    self.validate_style_ref(style, format!("{path}.style"));
                }
                self.validate_run_format(format, path);
            }
            RunSpec::Field { name, value, style } => {
                if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                    self.issue(
                        "invalid_value",
                        format!("{path}.name"),
                        format!("field name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
                    );
                }
                self.text_chars = self.text_chars.saturating_add(value.chars().count());
                if value.chars().count() > MAX_SHORT_TEXT_CHARS {
                    self.issue(
                        "limit_exceeded",
                        format!("{path}.value"),
                        format!("field value cannot exceed {MAX_SHORT_TEXT_CHARS} Unicode scalars"),
                    );
                }
                if contains_disallowed_text_control(value) {
                    self.issue(
                        "invalid_text",
                        format!("{path}.value"),
                        "control characters are not allowed; use line_break runs",
                    );
                }
                if let Some(style) = style {
                    self.validate_style_ref(style, format!("{path}.style"));
                }
            }
            RunSpec::Equation {
                script,
                width_mm,
                height_mm,
            } => {
                if script.is_empty() {
                    self.issue(
                        "invalid_value",
                        format!("{path}.script"),
                        "equation script cannot be empty",
                    );
                }
                if script.chars().count() > MAX_EQUATION_CHARS {
                    self.issue(
                        "limit_exceeded",
                        format!("{path}.script"),
                        format!("equation script cannot exceed {MAX_EQUATION_CHARS} characters"),
                    );
                }
                if let Some(width) = width_mm {
                    self.validate_positive_dimension(*width, &format!("{path}.width_mm"));
                }
                if let Some(height) = height_mm {
                    self.validate_positive_dimension(*height, &format!("{path}.height_mm"));
                }
            }
            RunSpec::Image {
                path: asset,
                width_mm,
                height_mm,
                alt,
            } => {
                self.validate_positive_dimension(*width_mm, &format!("{path}.width_mm"));
                if *width_mm > body_width_mm {
                    self.issue(
                        "layout_overflow",
                        format!("{path}.width_mm"),
                        format!("image width exceeds body width {body_width_mm:.2} mm"),
                    );
                }
                if let Some(height) = height_mm {
                    self.validate_positive_dimension(*height, &format!("{path}.height_mm"));
                }
                self.validate_asset(asset, &format!("{path}.path"));
                if alt
                    .as_ref()
                    .is_some_and(|alt| alt.chars().count() > MAX_SHORT_TEXT_CHARS)
                {
                    self.issue(
                        "limit_exceeded",
                        format!("{path}.alt"),
                        format!("alt text cannot exceed {MAX_SHORT_TEXT_CHARS} Unicode scalars"),
                    );
                }
                if alt.as_ref().is_some_and(|alt| !alt.is_empty()) {
                    self.issue(
                        "unsupported_native",
                        format!("{path}.alt"),
                        "image alt text is not representable by both native writers in v1",
                    );
                }
            }
            RunSpec::LineBreak => {}
        }
    }

    fn validate_run_format(&mut self, format: &RunFormatSpec, path: &str) {
        if let Some(size) = format.font_size_pt
            && (!size.is_finite() || !(1.0..=1000.0).contains(&size))
        {
            self.issue(
                "invalid_value",
                format!("{path}.font_size_pt"),
                "font size must be finite and within 1..=1000 pt",
            );
        }
        if let Some(font) = &format.font_family
            && (font.is_empty() || font.chars().count() > 256)
        {
            self.issue(
                "invalid_value",
                format!("{path}.font_family"),
                "font_family must be 1..=256 Unicode scalars",
            );
        }
        for (name, color) in [
            ("color", format.color.as_deref()),
            ("background", format.background.as_deref()),
        ] {
            if let Some(color) = color
                && parse_color(color).is_none()
            {
                self.issue(
                    "invalid_color",
                    format!("{path}.{name}"),
                    "color must be #RRGGBB",
                );
            }
        }
    }

    fn validate_table(
        &mut self,
        width_mm: Option<f32>,
        columns: &[TableColumnSpec],
        rows: &[TableRowSpec],
        path: &str,
        depth: usize,
        body_width_mm: f32,
    ) {
        if columns.is_empty() || columns.len() > usize::from(u16::MAX) {
            self.issue(
                "invalid_table",
                format!("{path}.columns"),
                "table requires 1..=65535 columns",
            );
            return;
        }
        if rows.is_empty() || rows.len() > usize::from(u16::MAX) {
            self.issue(
                "invalid_table",
                format!("{path}.rows"),
                "table requires 1..=65535 rows",
            );
            return;
        }
        let Some(grid_cells) = rows.len().checked_mul(columns.len()) else {
            self.issue(
                "limit_exceeded",
                path,
                "table grid dimensions overflow addressable memory",
            );
            return;
        };
        if grid_cells > MAX_TABLE_CELLS {
            self.issue(
                "limit_exceeded",
                path,
                format!("table grid has {grid_cells} slots; maximum is {MAX_TABLE_CELLS}"),
            );
            return;
        }
        let mut column_sum = 0.0_f32;
        for (index, column) in columns.iter().enumerate() {
            self.validate_positive_dimension(
                column.width_mm,
                &format!("{path}.columns[{index}].width_mm"),
            );
            column_sum += column.width_mm;
        }
        if !column_sum.is_finite() || column_sum > body_width_mm {
            self.issue(
                "layout_overflow",
                format!("{path}.columns"),
                format!(
                    "column widths total {column_sum:.2} mm; body width is {body_width_mm:.2} mm"
                ),
            );
        }
        if let Some(width) = width_mm {
            self.validate_positive_dimension(width, &format!("{path}.width_mm"));
            if width > body_width_mm {
                self.issue(
                    "layout_overflow",
                    format!("{path}.width_mm"),
                    format!("table width exceeds body width {body_width_mm:.2} mm"),
                );
            }
            if (width - column_sum).abs() > 0.01 {
                self.issue(
                    "inconsistent_width",
                    format!("{path}.width_mm"),
                    format!("table width {width:.2} mm must equal column sum {column_sum:.2} mm"),
                );
            }
        }
        let mut occupied = vec![vec![false; columns.len()]; rows.len()];
        for (row_index, row) in rows.iter().enumerate() {
            let mut cursor = 0usize;
            for (cell_index, cell) in row.cells.iter().enumerate() {
                self.cells = self.cells.saturating_add(1);
                while cursor < columns.len() && occupied[row_index][cursor] {
                    cursor += 1;
                }
                let cell_path = format!("{path}.rows[{row_index}].cells[{cell_index}]");
                let col_span = usize::from(cell.col_span);
                let row_span = usize::from(cell.row_span);
                if col_span == 0 || row_span == 0 {
                    self.issue(
                        "invalid_span",
                        &cell_path,
                        "col_span and row_span must be at least 1",
                    );
                    continue;
                }
                let Some(col_end) = cursor.checked_add(col_span) else {
                    self.issue("invalid_span", &cell_path, "column span overflows");
                    continue;
                };
                let Some(row_end) = row_index.checked_add(row_span) else {
                    self.issue("invalid_span", &cell_path, "row span overflows");
                    continue;
                };
                if col_end > columns.len() || row_end > rows.len() {
                    self.issue(
                        "invalid_span",
                        &cell_path,
                        "cell span exceeds the table grid",
                    );
                    continue;
                }
                let overlaps = occupied[row_index..row_end]
                    .iter()
                    .any(|row| row[cursor..col_end].iter().any(|occupied| *occupied));
                if overlaps {
                    self.issue(
                        "overlapping_span",
                        &cell_path,
                        "cell overlaps an earlier row or column span",
                    );
                    continue;
                }
                for row in &mut occupied[row_index..row_end] {
                    row[cursor..col_end].fill(true);
                }
                self.validate_blocks(
                    &cell.blocks,
                    &format!("{cell_path}.blocks"),
                    depth + 1,
                    false,
                    body_width_mm,
                );
                cursor = col_end;
            }
            if occupied[row_index].iter().any(|occupied| !occupied) {
                self.issue(
                    "incomplete_table_row",
                    format!("{path}.rows[{row_index}]"),
                    "row does not cover every table column",
                );
            }
        }
    }

    fn validate_style_ref(&mut self, style: &str, path: String) {
        if style.is_empty() || style.chars().count() > MAX_NAME_CHARS {
            self.issue(
                "invalid_name",
                path,
                format!("style name must be 1..={MAX_NAME_CHARS} Unicode scalars"),
            );
        } else if !self.spec.styles.contains_key(style) {
            self.issue(
                "unknown_reference",
                path,
                format!("style {style:?} does not exist"),
            );
        }
    }

    fn validate_positive_dimension(&mut self, value: f32, path: &str) {
        if !value.is_finite() || !(0.01..=2000.0).contains(&value) {
            self.issue(
                "invalid_dimension",
                path,
                "dimension must be finite and within 0.01..=2000 mm",
            );
        }
    }

    fn validate_asset(&mut self, asset: &Path, path: &str) {
        if asset.as_os_str().is_empty()
            || asset.to_string_lossy().chars().count() > MAX_SHORT_TEXT_CHARS
        {
            self.issue(
                "invalid_asset",
                path,
                format!("asset path must contain 1..={MAX_SHORT_TEXT_CHARS} Unicode scalars"),
            );
            return;
        }
        if crate::asset_snapshot::validate_relative_path(asset).is_err() {
            self.issue(
                "invalid_asset_path",
                path,
                "asset path must contain only relative normal components",
            );
        }
    }
}

fn parse_color(value: &str) -> Option<u32> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(hex, 16).ok()?;
    let red = (rgb >> 16) & 0xff;
    let green = (rgb >> 8) & 0xff;
    let blue = rgb & 0xff;
    Some((blue << 16) | (green << 8) | red)
}

fn writable_page_width(page: PageSpec) -> f32 {
    let (physical_width, _) = physical_page_size(page);
    physical_width - page.margin_left_mm - page.margin_right_mm - page.gutter_mm
}

fn physical_page_size(page: PageSpec) -> (f32, f32) {
    match page.orientation {
        PageOrientation::Portrait => (
            page.width_mm.min(page.height_mm),
            page.width_mm.max(page.height_mm),
        ),
        PageOrientation::Landscape => (
            page.width_mm.max(page.height_mm),
            page.width_mm.min(page.height_mm),
        ),
    }
}

fn contains_disallowed_text_control(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() && character != '\t')
}

pub fn compile_spec(
    spec: &DocumentSpec,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
    allow_visual_fallback: bool,
) -> Result<CompiledSpec, ComposeError> {
    validate_spec(spec, base_dir)?;
    let compiler = Compiler::new(spec, base_dir, output, dry_run, allow_visual_fallback)?;
    compiler.compile()
}

#[derive(Clone, Copy)]
struct CompiledStyle {
    style_id: hwp_model::StyleId,
    para_shape: hwp_model::ParaShapeId,
    char_shape: hwp_model::CharShapeId,
}

struct CompiledList {
    kind: ListKind,
    definition_ids: Vec<u16>,
}

#[derive(Clone)]
struct EmbeddedAsset {
    item_ref: String,
    pixel_size: (u32, u32),
}

struct Compiler<'a> {
    spec: &'a DocumentSpec,
    base_dir: &'a Path,
    document: hwp_model::Document,
    styles: BTreeMap<String, CompiledStyle>,
    resolved_styles: BTreeMap<String, StyleSpec>,
    lists: BTreeMap<String, CompiledList>,
    list_para_shapes: BTreeMap<(u16, String, u8), hwp_model::ParaShapeId>,
    assets: BTreeMap<String, EmbeddedAsset>,
    asset_paths: BTreeMap<PathBuf, EmbeddedAsset>,
    asset_bytes: u64,
    report: ComposeReport,
}

impl<'a> Compiler<'a> {
    fn new(
        spec: &'a DocumentSpec,
        base_dir: &'a Path,
        output: &Path,
        dry_run: bool,
        allow_visual_fallback: bool,
    ) -> Result<Self, ComposeError> {
        let mut document = hwp_convert::from_markdown("");
        document.sections.clear();
        document.bin_streams.clear();
        document.metadata.title = spec.metadata.title.clone();
        document.metadata.author = spec.metadata.author.clone();
        document.metadata.subject = spec.metadata.subject.clone();
        document.metadata.keywords = spec.metadata.keywords.clone();
        document.metadata.description = spec.metadata.description.clone();
        document.meta.source_format = "document-spec-v1".to_string();
        document.meta.source_version = "1.0".to_string();

        let mut compiler = Self {
            spec,
            base_dir,
            document,
            styles: BTreeMap::new(),
            resolved_styles: BTreeMap::new(),
            lists: BTreeMap::new(),
            list_para_shapes: BTreeMap::new(),
            assets: BTreeMap::new(),
            asset_paths: BTreeMap::new(),
            asset_bytes: 0,
            report: ComposeReport {
                schema_version: SpecVersion::V1,
                output: output.display().to_string(),
                dry_run,
                deterministic: true,
                native: true,
                visual_fallback_allowed: allow_visual_fallback,
                visual_fallback_used: Vec::new(),
                sections: 0,
                paragraphs: 0,
                tables: 0,
                images: 0,
                equations: 0,
                fields: 0,
                warnings: Vec::new(),
            },
        };
        compiler.compile_styles()?;
        compiler.compile_lists()?;
        Ok(compiler)
    }

    fn compile(mut self) -> Result<CompiledSpec, ComposeError> {
        for (index, section) in self.spec.sections.iter().enumerate() {
            let sections = self.compile_section(section, index)?;
            self.document.sections.extend(sections);
        }
        self.document.header.properties.section_count =
            u16::try_from(self.document.sections.len()).unwrap_or(u16::MAX);
        self.report.sections = self.document.sections.len();
        self.report.paragraphs = count_document_paragraphs(&self.document);
        Ok(CompiledSpec {
            document: self.document,
            report: self.report,
        })
    }

    fn compile_styles(&mut self) -> Result<(), ComposeError> {
        for name in self.spec.styles.keys() {
            let resolved = self.resolve_style(name, &mut BTreeSet::new())?;
            self.resolved_styles.insert(name.clone(), resolved);
        }
        for (name, style) in &self.resolved_styles {
            let mut char_shape = self
                .document
                .header
                .char_shapes
                .first()
                .cloned()
                .unwrap_or_default();
            apply_char_style(&mut self.document.header, &mut char_shape, style);
            let char_shape_id = find_or_push_char_shape(&mut self.document.header, char_shape)?;

            let mut para_shape = self
                .document
                .header
                .para_shapes
                .get(2)
                .cloned()
                .or_else(|| self.document.header.para_shapes.first().cloned())
                .unwrap_or_default();
            apply_para_style(&mut para_shape, style);
            let para_shape_id = find_or_push_para_shape(&mut self.document.header, para_shape)?;

            let style_id = u16::try_from(self.document.header.styles.len()).map_err(|_| {
                ComposeError::Compile {
                    path: format!("$.styles.{name}"),
                    message: "style table exceeds u16".to_string(),
                }
            })?;
            self.document.header.styles.push(hwp_model::Style {
                name: name.clone(),
                english_name: name.clone(),
                attr: 0,
                next_style: 0,
                lang_id: 1042,
                para_shape: para_shape_id,
                char_shape: char_shape_id,
                tail: Vec::new(),
            });
            self.styles.insert(
                name.clone(),
                CompiledStyle {
                    style_id: hwp_model::StyleId(style_id),
                    para_shape: para_shape_id,
                    char_shape: char_shape_id,
                },
            );
        }
        Ok(())
    }

    fn resolve_style(
        &self,
        name: &str,
        stack: &mut BTreeSet<String>,
    ) -> Result<StyleSpec, ComposeError> {
        if !stack.insert(name.to_string()) {
            return Err(ComposeError::Compile {
                path: format!("$.styles.{name}.based_on"),
                message: "style inheritance cycle".to_string(),
            });
        }
        let own = self
            .spec
            .styles
            .get(name)
            .ok_or_else(|| ComposeError::Compile {
                path: format!("$.styles.{name}"),
                message: "style not found after validation".to_string(),
            })?;
        let mut resolved = match &own.based_on {
            Some(parent) => self.resolve_style(parent, stack)?,
            None => StyleSpec::default(),
        };
        merge_style(&mut resolved, own);
        resolved.based_on = None;
        stack.remove(name);
        Ok(resolved)
    }

    fn compile_lists(&mut self) -> Result<(), ComposeError> {
        for (name, list) in &self.spec.lists {
            let mut definition_ids = Vec::with_capacity(list.levels.len());
            match list.kind {
                ListKind::Ordered => {
                    let definition_id = u16::try_from(self.document.header.numbering_levels.len())
                        .map_err(|_| ComposeError::Compile {
                            path: format!("$.lists.{name}"),
                            message: "numbering table exceeds u16".to_string(),
                        })?;
                    let mut levels = Vec::with_capacity(7);
                    for level in 0..7 {
                        let value = list.levels.get(level);
                        levels.push(hwp_model::NumLevel {
                            start: value.map_or(1, |value| value.start),
                            fmt: hwp_model::NumFmt::Digit,
                            template: value.map_or_else(
                                || format!("^{}.", level + 1),
                                |value| value.marker.replace("{n}", &format!("^{}", level + 1)),
                            ),
                        });
                    }
                    self.document.header.numbering_levels.push(levels);
                    definition_ids.resize(list.levels.len(), definition_id);
                }
                ListKind::Bullet => {
                    for level in &list.levels {
                        let definition_id = u16::try_from(self.document.header.bullet_chars.len())
                            .map_err(|_| ComposeError::Compile {
                                path: format!("$.lists.{name}"),
                                message: "bullet table exceeds u16".to_string(),
                            })?;
                        self.document
                            .header
                            .bullet_chars
                            .push(level.marker.chars().next().expect("validated marker"));
                        definition_ids.push(definition_id);
                    }
                }
            }
            self.lists.insert(
                name.clone(),
                CompiledList {
                    kind: list.kind,
                    definition_ids,
                },
            );
        }
        Ok(())
    }

    fn compile_section(
        &mut self,
        section: &SectionSpec,
        section_index: usize,
    ) -> Result<Vec<hwp_model::Section>, ComposeError> {
        let page = section.page.unwrap_or(self.spec.page);
        let mut output = Vec::new();
        let mut paragraphs = vec![self.compile_section_anchor(section, page, section_index)?];
        for (block_index, block) in section.blocks.iter().enumerate() {
            if matches!(
                block,
                BlockSpec::Break {
                    kind: BreakKind::Section
                }
            ) {
                output.push(hwp_model::Section {
                    paragraphs,
                    extras: Vec::new(),
                });
                paragraphs = vec![self.compile_section_anchor(section, page, section_index)?];
                continue;
            }
            paragraphs.extend(self.compile_block(
                block,
                &format!("$.sections[{section_index}].blocks[{block_index}]"),
            )?);
        }
        output.push(hwp_model::Section {
            paragraphs,
            extras: Vec::new(),
        });
        Ok(output)
    }

    fn compile_section_anchor(
        &mut self,
        section: &SectionSpec,
        page: PageSpec,
        section_index: usize,
    ) -> Result<hwp_model::Paragraph, ComposeError> {
        let mut paragraph = hwp_model::Paragraph {
            char_shape_runs: vec![(0, hwp_model::CharShapeId(0))],
            header: hwp_model::ParaHeaderInfo {
                break_type: 0x03,
                ..hwp_model::ParaHeaderInfo::default()
            },
            ..hwp_model::Paragraph::default()
        };
        append_control(
            &mut paragraph,
            hwp_model::ctrl_char::SECTION_COLUMN_DEF,
            hwp_model::Control::SectionDef(hwp_model::SectionDef {
                data: Vec::new(),
                page: Some(page_def(page)),
                extras: Vec::new(),
                secpr_raw_children: Vec::new(),
                footnote_shape_raw: None,
                endnote_shape_raw: None,
                page_border_fills_raw: Vec::new(),
            }),
        )?;
        append_control(
            &mut paragraph,
            hwp_model::ctrl_char::SECTION_COLUMN_DEF,
            hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"cold",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: Some(hwp_model::ColumnDef {
                    count: 1,
                    kind: 0,
                    direction: 0,
                    same_width: true,
                    gap: 0,
                    widths: Vec::new(),
                    divider: None,
                }),
            }),
        )?;
        if let Some(header) = &section.header {
            self.append_header_footer(
                &mut paragraph,
                header,
                *b"head",
                &format!("$.sections[{section_index}].header"),
            )?;
        }
        if let Some(footer) = &section.footer {
            self.append_header_footer(
                &mut paragraph,
                footer,
                *b"foot",
                &format!("$.sections[{section_index}].footer"),
            )?;
        }
        if let Some(page_number) = &section.page_number {
            self.document.header.properties.start_numbers[0] =
                u16::try_from(page_number.start).expect("validated page number");
            let mut new_number = Vec::with_capacity(6);
            new_number.extend_from_slice(&0_u32.to_le_bytes());
            new_number.extend_from_slice(
                &u16::try_from(page_number.start)
                    .expect("validated page number")
                    .to_le_bytes(),
            );
            append_control(
                &mut paragraph,
                hwp_model::ctrl_char::PAGE_CONTROL,
                generic_control(*b"nwno", new_number),
            )?;

            let side = page_number
                .suffix
                .as_deref()
                .and_then(|suffix| suffix.encode_utf16().next())
                .unwrap_or(0);
            let position = page_number_position_code(page_number.position);
            let mut data = Vec::with_capacity(12);
            data.extend_from_slice(&(u32::from(position) << 8).to_le_bytes());
            data.extend_from_slice(&[0_u8; 6]);
            data.extend_from_slice(&side.to_le_bytes());
            append_control(
                &mut paragraph,
                hwp_model::ctrl_char::PAGE_CONTROL,
                generic_control(*b"pgnp", data),
            )?;
        }
        Ok(paragraph)
    }

    fn append_header_footer(
        &mut self,
        paragraph: &mut hwp_model::Paragraph,
        spec: &HeaderFooterSpec,
        ctrl_id: [u8; 4],
        path: &str,
    ) -> Result<(), ComposeError> {
        for (apply, label, blocks) in [
            (0_u32, "default", &spec.default),
            (1_u32, "even", &spec.even),
            (2_u32, "odd", &spec.odd),
        ] {
            if blocks.is_empty() {
                continue;
            }
            let mut paragraphs = Vec::new();
            for (index, block) in blocks.iter().enumerate() {
                paragraphs.extend(self.compile_block(block, &format!("{path}.{label}[{index}]"))?);
            }
            if paragraphs.is_empty() {
                paragraphs.push(empty_paragraph());
            }
            let mut data = Vec::with_capacity(8);
            data.extend_from_slice(&apply.to_le_bytes());
            data.extend_from_slice(&0_u32.to_le_bytes());
            append_control(
                paragraph,
                hwp_model::ctrl_char::HEADER_FOOTER,
                hwp_model::Control::Generic(hwp_model::GenericControl {
                    ctrl_id,
                    data,
                    paragraph_lists: vec![hwp_model::ParagraphList {
                        header_data: Vec::new(),
                        paragraphs,
                    }],
                    extras: Vec::new(),
                    raw_children: Vec::new(),
                    gso_shapes: Vec::new(),
                    equation: None,
                    column_def: None,
                }),
            )?;
        }
        Ok(())
    }

    fn compile_block(
        &mut self,
        block: &BlockSpec,
        path: &str,
    ) -> Result<Vec<hwp_model::Paragraph>, ComposeError> {
        match block {
            BlockSpec::Paragraph {
                style, list, runs, ..
            } => Ok(vec![self.compile_paragraph(
                style.as_deref(),
                list.as_ref(),
                runs,
                path,
            )?]),
            BlockSpec::Table {
                width_mm,
                columns,
                rows,
            } => Ok(vec![self.compile_table(*width_mm, columns, rows, path)?]),
            BlockSpec::Image {
                path: asset,
                width_mm,
                height_mm,
                placement,
                ..
            } => {
                let mut paragraph = empty_paragraph();
                self.append_image(
                    &mut paragraph,
                    asset,
                    *width_mm,
                    *height_mm,
                    *placement == ImagePlacement::Inline,
                    path,
                )?;
                self.report.paragraphs += 1;
                Ok(vec![paragraph])
            }
            BlockSpec::Equation {
                script,
                width_mm,
                height_mm,
            } => {
                let mut paragraph = empty_paragraph();
                self.append_equation(&mut paragraph, script, *width_mm, *height_mm, true)?;
                self.report.paragraphs += 1;
                Ok(vec![paragraph])
            }
            BlockSpec::Field { name, value, style } => {
                let mut paragraph = empty_paragraph();
                let shape = self.char_shape_for_style(style.as_deref())?;
                append_field(&mut paragraph, name, value, shape)?;
                self.report.fields += 1;
                self.report.paragraphs += 1;
                Ok(vec![paragraph])
            }
            BlockSpec::Break {
                kind: BreakKind::Page,
            } => {
                let mut paragraph = empty_paragraph();
                paragraph.header.break_type = 0x04;
                self.report.paragraphs += 1;
                Ok(vec![paragraph])
            }
            BlockSpec::Break {
                kind: BreakKind::Column,
            } => {
                let mut paragraph = empty_paragraph();
                paragraph.header.break_type = 0x08;
                self.report.paragraphs += 1;
                Ok(vec![paragraph])
            }
            BlockSpec::Break {
                kind: BreakKind::Section,
            } => Err(ComposeError::Compile {
                path: path.to_string(),
                message: "section break reached nested compiler".to_string(),
            }),
        }
    }

    fn compile_paragraph(
        &mut self,
        style_name: Option<&str>,
        list_ref: Option<&ListRefSpec>,
        runs: &[RunSpec],
        path: &str,
    ) -> Result<hwp_model::Paragraph, ComposeError> {
        let list_level_style = list_ref.and_then(|list_ref| {
            self.spec
                .lists
                .get(&list_ref.name)
                .and_then(|list| list.levels.get(usize::from(list_ref.level)))
                .and_then(|level| level.style.as_deref())
        });
        let style_name = style_name.or(list_level_style);
        let compiled_style = style_name
            .and_then(|name| self.styles.get(name).copied())
            .unwrap_or(CompiledStyle {
                style_id: hwp_model::StyleId(0),
                para_shape: hwp_model::ParaShapeId(2),
                char_shape: hwp_model::CharShapeId(0),
            });
        let para_shape = match list_ref {
            Some(list_ref) => self.list_para_shape(compiled_style.para_shape, list_ref, path)?,
            None => compiled_style.para_shape,
        };
        let mut paragraph = hwp_model::Paragraph {
            para_shape,
            style: compiled_style.style_id,
            char_shape_runs: vec![(0, compiled_style.char_shape)],
            ..hwp_model::Paragraph::default()
        };
        let mut current_shape = compiled_style.char_shape;
        for (run_index, run) in runs.iter().enumerate() {
            let run_path = format!("{path}.runs[{run_index}]");
            match run {
                RunSpec::Text {
                    text,
                    style,
                    format,
                } => {
                    let shape = self.resolve_run_shape(style.as_deref().or(style_name), format)?;
                    set_run_shape(&mut paragraph, shape);
                    current_shape = shape;
                    append_text(&mut paragraph, text);
                }
                RunSpec::Field { name, value, style } => {
                    let shape = self.char_shape_for_style(style.as_deref().or(style_name))?;
                    append_field(&mut paragraph, name, value, shape)?;
                    current_shape = shape;
                    self.report.fields += 1;
                }
                RunSpec::Equation {
                    script,
                    width_mm,
                    height_mm,
                } => {
                    set_run_shape(&mut paragraph, current_shape);
                    self.append_equation(&mut paragraph, script, *width_mm, *height_mm, true)?;
                }
                RunSpec::Image {
                    path: asset,
                    width_mm,
                    height_mm,
                    ..
                } => {
                    set_run_shape(&mut paragraph, current_shape);
                    self.append_image(
                        &mut paragraph,
                        asset,
                        *width_mm,
                        *height_mm,
                        true,
                        &run_path,
                    )?;
                }
                RunSpec::LineBreak => paragraph.chars.push(hwp_model::HwpChar::CharCtrl(
                    hwp_model::ctrl_char::LINE_BREAK,
                )),
            }
        }
        self.report.paragraphs += 1;
        Ok(paragraph)
    }

    fn list_para_shape(
        &mut self,
        base: hwp_model::ParaShapeId,
        list_ref: &ListRefSpec,
        path: &str,
    ) -> Result<hwp_model::ParaShapeId, ComposeError> {
        let key = (base.0, list_ref.name.clone(), list_ref.level);
        if let Some(existing) = self.list_para_shapes.get(&key) {
            return Ok(*existing);
        }
        let list = self
            .lists
            .get(&list_ref.name)
            .ok_or_else(|| ComposeError::Compile {
                path: format!("{path}.list.name"),
                message: "list not found after validation".to_string(),
            })?;
        let definition_id = *list
            .definition_ids
            .get(usize::from(list_ref.level))
            .ok_or_else(|| ComposeError::Compile {
                path: format!("{path}.list.level"),
                message: "list level not found after validation".to_string(),
            })?;
        let mut shape = self
            .document
            .header
            .para_shapes
            .get(usize::from(base.0))
            .cloned()
            .unwrap_or_default();
        shape.attr1 &= !((0x3 << 23) | (0x7 << 25));
        let head_type = match list.kind {
            ListKind::Ordered => 2_u32,
            ListKind::Bullet => 3_u32,
        };
        shape.attr1 |= head_type << 23;
        shape.attr1 |= (u32::from(list_ref.level) + 1) << 25;
        shape.numbering_id = definition_id;
        let id = find_or_push_para_shape(&mut self.document.header, shape)?;
        self.list_para_shapes.insert(key, id);
        Ok(id)
    }

    fn char_shape_for_style(
        &self,
        style: Option<&str>,
    ) -> Result<hwp_model::CharShapeId, ComposeError> {
        match style {
            Some(style) => self
                .styles
                .get(style)
                .map(|compiled| compiled.char_shape)
                .ok_or_else(|| ComposeError::Compile {
                    path: format!("$.styles.{style}"),
                    message: "style not found after validation".to_string(),
                }),
            None => Ok(hwp_model::CharShapeId(0)),
        }
    }

    fn resolve_run_shape(
        &mut self,
        style: Option<&str>,
        format: &RunFormatSpec,
    ) -> Result<hwp_model::CharShapeId, ComposeError> {
        let base_id = self.char_shape_for_style(style)?;
        if *format == RunFormatSpec::default() {
            return Ok(base_id);
        }
        let mut shape = self
            .document
            .header
            .char_shapes
            .get(usize::from(base_id.0))
            .cloned()
            .unwrap_or_default();
        apply_run_format(&mut self.document.header, &mut shape, format);
        find_or_push_char_shape(&mut self.document.header, shape)
    }

    fn append_equation(
        &mut self,
        paragraph: &mut hwp_model::Paragraph,
        script: &str,
        width_mm: Option<f32>,
        height_mm: Option<f32>,
        inline: bool,
    ) -> Result<(), ComposeError> {
        append_control(
            paragraph,
            hwp_model::ctrl_char::OBJECT,
            hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"eqed",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: Some(hwp_model::Equation {
                    script: script.to_string(),
                    width: width_mm.map_or(0, mm_to_hwp),
                    height: height_mm.map_or(0, mm_to_hwp),
                    inline,
                    x: 0,
                    y: 0,
                    raw_attrs: None,
                    raw_props: Vec::new(),
                }),
                column_def: None,
            }),
        )?;
        self.report.equations += 1;
        Ok(())
    }

    fn append_image(
        &mut self,
        paragraph: &mut hwp_model::Paragraph,
        asset: &Path,
        width_mm: f32,
        height_mm: Option<f32>,
        inline: bool,
        path: &str,
    ) -> Result<(), ComposeError> {
        let embedded = self.load_asset(asset, &format!("{path}.path"))?;
        let width = mm_to_hwp(width_mm);
        let height_mm = match height_mm {
            Some(height) => height,
            None => {
                let (pixel_width, pixel_height) = embedded.pixel_size;
                width_mm * (pixel_height as f32 / pixel_width as f32)
            }
        };
        if !height_mm.is_finite() || !(0.01..=2000.0).contains(&height_mm) {
            return Err(ComposeError::Compile {
                path: format!("{path}.height_mm"),
                message: "natural image height must be within 0.01..=2000 mm".to_string(),
            });
        }
        let height = mm_to_hwp(height_mm);
        append_control(
            paragraph,
            hwp_model::ctrl_char::OBJECT,
            hwp_model::Control::Picture(hwp_model::Picture {
                common_data: Vec::new(),
                width: hwp_model::HwpUnit(width),
                height: hwp_model::HwpUnit(height),
                treat_as_char: inline,
                z_order: if inline { 0 } else { 10 },
                vert_offset: 0,
                horz_offset: 0,
                description: None,
                bin_ref: hwp_model::BinRef::ItemRef(embedded.item_ref),
                extras: Vec::new(),
            }),
        )?;
        self.report.images += 1;
        Ok(())
    }

    fn load_asset(&mut self, asset: &Path, path: &str) -> Result<EmbeddedAsset, ComposeError> {
        if let Some(existing) = self.asset_paths.get(asset) {
            return Ok(existing.clone());
        }
        let snapshot = crate::asset_snapshot::read_contained(self.base_dir, asset, MAX_ASSET_BYTES)
            .map_err(|error| ComposeError::Compile {
                path: path.to_string(),
                message: format!("asset_snapshot_{}: {error}", error.code.as_str()),
            })?;
        let data = snapshot.data;
        let (extension, _) = hwp_convert::image_kind(&data);
        if extension == "bin" {
            return Err(ComposeError::Compile {
                path: path.to_string(),
                message: "asset snapshot is not a supported PNG, JPEG, GIF, or BMP image"
                    .to_string(),
            });
        }
        let pixel_size = hwp_convert::image_pixel_size(&data)
            .filter(|(width, height)| *width > 0 && *height > 0);
        let Some(pixel_size) = pixel_size else {
            return Err(ComposeError::Compile {
                path: path.to_string(),
                message: "image header has no valid pixel dimensions".to_string(),
            });
        };
        let digest_hex = snapshot
            .sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if let Some(existing) = self.assets.get(&digest_hex) {
            let embedded = existing.clone();
            self.asset_paths
                .insert(asset.to_path_buf(), embedded.clone());
            return Ok(embedded);
        }
        self.asset_bytes = self.asset_bytes.saturating_add(data.len() as u64);
        if self.asset_bytes > MAX_TOTAL_ASSET_BYTES {
            return Err(ComposeError::Compile {
                path: path.to_string(),
                message: format!("embedded assets exceed {MAX_TOTAL_ASSET_BYTES} total bytes"),
            });
        }
        let name = format!("asset-{}.{}", &digest_hex[..16], extension);
        self.document.bin_streams.push(hwp_model::BinStream {
            name: name.clone(),
            data,
        });
        let embedded = EmbeddedAsset {
            item_ref: name,
            pixel_size,
        };
        self.assets.insert(digest_hex, embedded.clone());
        self.asset_paths
            .insert(asset.to_path_buf(), embedded.clone());
        Ok(embedded)
    }

    fn compile_table(
        &mut self,
        _width_mm: Option<f32>,
        columns: &[TableColumnSpec],
        rows: &[TableRowSpec],
        path: &str,
    ) -> Result<hwp_model::Paragraph, ComposeError> {
        let column_widths = columns
            .iter()
            .map(|column| mm_to_hwp(column.width_mm))
            .collect::<Vec<_>>();
        let mut occupied = vec![vec![false; columns.len()]; rows.len()];
        let mut cells = Vec::new();
        let mut row_cell_counts = Vec::with_capacity(rows.len());
        for (row_index, row) in rows.iter().enumerate() {
            let mut cursor = 0usize;
            let mut anchors = 0_u16;
            for (cell_index, cell) in row.cells.iter().enumerate() {
                while occupied[row_index][cursor] {
                    cursor += 1;
                }
                let col_end = cursor + usize::from(cell.col_span);
                let row_end = row_index + usize::from(cell.row_span);
                for occupied_row in &mut occupied[row_index..row_end] {
                    occupied_row[cursor..col_end].fill(true);
                }
                let mut paragraphs = Vec::new();
                for (block_index, block) in cell.blocks.iter().enumerate() {
                    paragraphs.extend(self.compile_block(
                        block,
                        &format!(
                            "{path}.rows[{row_index}].cells[{cell_index}].blocks[{block_index}]"
                        ),
                    )?);
                }
                if paragraphs.is_empty() {
                    paragraphs.push(empty_paragraph());
                }
                let width = column_widths[cursor..col_end].iter().sum();
                let paragraph_count = i32::try_from(paragraphs.len()).unwrap_or(i32::MAX);
                cells.push(hwp_model::Cell {
                    list_attr: 0x20,
                    col: u16::try_from(cursor).expect("validated table column"),
                    row: u16::try_from(row_index).expect("validated table row"),
                    col_span: cell.col_span,
                    row_span: cell.row_span,
                    width: hwp_model::HwpUnit(width),
                    height: hwp_model::HwpUnit(1700_i32.saturating_mul(paragraph_count.max(1))),
                    margins: [510, 510, 141, 141],
                    border_fill: hwp_model::BorderFillId(3),
                    header_tail: Vec::new(),
                    paragraphs,
                });
                anchors = anchors.saturating_add(1);
                cursor = col_end;
            }
            row_cell_counts.push(anchors);
        }
        let table = hwp_model::Table {
            common_data: Vec::new(),
            placement: Some(hwp_model::GsoPlacement {
                treat_as_char: true,
                affect_line_spacing: true,
                flow_with_text: true,
                hold_anchor: false,
                vert_rel_to: 2,
                horz_rel_to: 3,
                vert_align: 0,
                horz_align: 0,
                vert_offset: 0,
                horz_offset: 0,
                z_order: 0,
                width: column_widths.iter().sum(),
                height: 1700_i32.saturating_mul(i32::try_from(rows.len()).unwrap_or(i32::MAX)),
                out_margins: [0; 4],
            }),
            attr: 0,
            rows: u16::try_from(rows.len()).expect("validated table rows"),
            cols: u16::try_from(columns.len()).expect("validated table columns"),
            cell_spacing: 0,
            inner_margins: [510, 510, 141, 141],
            row_cell_counts,
            border_fill: hwp_model::BorderFillId(3),
            table_tail: Vec::new(),
            cells,
            extras: Vec::new(),
        };
        let mut paragraph = empty_paragraph();
        append_control(
            &mut paragraph,
            hwp_model::ctrl_char::OBJECT,
            hwp_model::Control::Table(table),
        )?;
        self.report.tables += 1;
        self.report.paragraphs += 1;
        Ok(paragraph)
    }
}

fn merge_style(target: &mut StyleSpec, own: &StyleSpec) {
    macro_rules! merge {
        ($field:ident) => {
            if own.$field.is_some() {
                target.$field = own.$field.clone();
            }
        };
    }
    merge!(font_family);
    merge!(font_size_pt);
    merge!(bold);
    merge!(italic);
    merge!(underline);
    merge!(strike);
    merge!(color);
    merge!(background);
    merge!(align);
    merge!(margin_left_mm);
    merge!(margin_right_mm);
    merge!(spacing_before_pt);
    merge!(spacing_after_pt);
    merge!(line_height_percent);
    merge!(keep_with_next);
}

fn apply_char_style(
    header: &mut hwp_model::DocHeader,
    shape: &mut hwp_model::CharShape,
    style: &StyleSpec,
) {
    if let Some(font) = &style.font_family {
        let id = ensure_font(header, font);
        shape.face_ids = [id; hwp_model::LANG_COUNT];
    }
    if let Some(size) = style.font_size_pt {
        shape.base_size = pt_to_hwp(size);
    }
    if let Some(value) = style.bold {
        shape.attr = (shape.attr & !(1 << 1)) | (u32::from(value) << 1);
    }
    if let Some(value) = style.italic {
        shape.attr = (shape.attr & !1) | u32::from(value);
    }
    if let Some(value) = style.underline {
        shape.attr = (shape.attr & !(0x3 << 2)) | (u32::from(value) << 2);
        if value {
            shape.underline_shape = 1;
            if shape.underline_color == 0xFFFF_FFFF {
                shape.underline_color = shape.text_color;
            }
        }
    }
    if let Some(value) = style.strike {
        shape.strike = value;
    }
    if let Some(value) = style.color.as_deref().and_then(parse_color) {
        shape.text_color = value;
    }
    if let Some(value) = style.background.as_deref().and_then(parse_color) {
        shape.shade_color = value;
    }
}

fn apply_run_format(
    header: &mut hwp_model::DocHeader,
    shape: &mut hwp_model::CharShape,
    format: &RunFormatSpec,
) {
    let style = StyleSpec {
        font_family: format.font_family.clone(),
        font_size_pt: format.font_size_pt,
        bold: format.bold,
        italic: format.italic,
        underline: format.underline,
        strike: format.strike,
        color: format.color.clone(),
        background: format.background.clone(),
        ..StyleSpec::default()
    };
    apply_char_style(header, shape, &style);
}

fn apply_para_style(shape: &mut hwp_model::ParaShape, style: &StyleSpec) {
    if let Some(align) = style.align {
        let value = match align {
            Alignment::Justify => 0,
            Alignment::Left => 1,
            Alignment::Right => 2,
            Alignment::Center => 3,
            Alignment::Distribute => 4,
        };
        shape.attr1 = (shape.attr1 & !(0x7 << 2)) | (value << 2);
    }
    if let Some(value) = style.margin_left_mm {
        shape.margin_left = mm_to_hwp(value);
    }
    if let Some(value) = style.margin_right_mm {
        shape.margin_right = mm_to_hwp(value);
    }
    if let Some(value) = style.spacing_before_pt {
        shape.spacing_top = pt_to_hwp(value);
    }
    if let Some(value) = style.spacing_after_pt {
        shape.spacing_bottom = pt_to_hwp(value);
    }
    if let Some(value) = style.line_height_percent {
        shape.line_spacing_type = 0;
        shape.line_spacing = i32::from(value);
        shape.line_spacing_old = i32::from(value);
    }
}

fn ensure_font(header: &mut hwp_model::DocHeader, font: &str) -> u16 {
    if let Some(index) = header.fonts[0].iter().position(|face| face.name == font) {
        return u16::try_from(index).unwrap_or(u16::MAX);
    }
    let id = u16::try_from(header.fonts[0].len()).unwrap_or(u16::MAX);
    for language in &mut header.fonts {
        language.push(hwp_model::FaceName {
            attr: 1,
            name: font.to_string(),
            default_name: Some(font.to_string()),
            ..hwp_model::FaceName::default()
        });
    }
    id
}

fn find_or_push_char_shape(
    header: &mut hwp_model::DocHeader,
    shape: hwp_model::CharShape,
) -> Result<hwp_model::CharShapeId, ComposeError> {
    if let Some(index) = header
        .char_shapes
        .iter()
        .position(|candidate| *candidate == shape)
    {
        return Ok(hwp_model::CharShapeId(
            u16::try_from(index).unwrap_or(u16::MAX),
        ));
    }
    let id = u16::try_from(header.char_shapes.len()).map_err(|_| ComposeError::Compile {
        path: "$.styles".to_string(),
        message: "character shape table exceeds u16".to_string(),
    })?;
    header.char_shapes.push(shape);
    Ok(hwp_model::CharShapeId(id))
}

fn find_or_push_para_shape(
    header: &mut hwp_model::DocHeader,
    shape: hwp_model::ParaShape,
) -> Result<hwp_model::ParaShapeId, ComposeError> {
    if let Some(index) = header
        .para_shapes
        .iter()
        .position(|candidate| *candidate == shape)
    {
        return Ok(hwp_model::ParaShapeId(
            u16::try_from(index).unwrap_or(u16::MAX),
        ));
    }
    let id = u16::try_from(header.para_shapes.len()).map_err(|_| ComposeError::Compile {
        path: "$.styles".to_string(),
        message: "paragraph shape table exceeds u16".to_string(),
    })?;
    header.para_shapes.push(shape);
    Ok(hwp_model::ParaShapeId(id))
}

fn page_def(page: PageSpec) -> hwp_model::PageDef {
    let mut width = mm_to_hwp(page.width_mm);
    let mut height = mm_to_hwp(page.height_mm);
    if page.orientation == PageOrientation::Landscape && width < height {
        std::mem::swap(&mut width, &mut height);
    }
    if page.orientation == PageOrientation::Portrait && width > height {
        std::mem::swap(&mut width, &mut height);
    }
    hwp_model::PageDef {
        width: hwp_model::HwpUnit(width),
        height: hwp_model::HwpUnit(height),
        margin_left: hwp_model::HwpUnit(mm_to_hwp(page.margin_left_mm)),
        margin_right: hwp_model::HwpUnit(mm_to_hwp(page.margin_right_mm)),
        margin_top: hwp_model::HwpUnit(mm_to_hwp(page.margin_top_mm)),
        margin_bottom: hwp_model::HwpUnit(mm_to_hwp(page.margin_bottom_mm)),
        margin_header: hwp_model::HwpUnit(mm_to_hwp(page.margin_header_mm)),
        margin_footer: hwp_model::HwpUnit(mm_to_hwp(page.margin_footer_mm)),
        gutter: hwp_model::HwpUnit(mm_to_hwp(page.gutter_mm)),
        attr: u32::from(page.orientation == PageOrientation::Landscape),
    }
}

fn empty_paragraph() -> hwp_model::Paragraph {
    hwp_model::Paragraph {
        para_shape: hwp_model::ParaShapeId(2),
        char_shape_runs: vec![(0, hwp_model::CharShapeId(0))],
        ..hwp_model::Paragraph::default()
    }
}

fn generic_control(ctrl_id: [u8; 4], data: Vec<u8>) -> hwp_model::Control {
    hwp_model::Control::Generic(hwp_model::GenericControl {
        ctrl_id,
        data,
        paragraph_lists: Vec::new(),
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
    })
}

fn append_control(
    paragraph: &mut hwp_model::Paragraph,
    code: u16,
    control: hwp_model::Control,
) -> Result<(), ComposeError> {
    let index = u32::try_from(paragraph.controls.len()).map_err(|_| ComposeError::Compile {
        path: "$".to_string(),
        message: "paragraph control count exceeds u32".to_string(),
    })?;
    let ctrl_id = control.ctrl_id();
    let mut reverse = ctrl_id;
    reverse.reverse();
    let mut payload = vec![0_u8; 12];
    payload[..4].copy_from_slice(&reverse);
    paragraph.chars.push(hwp_model::HwpChar::ExtCtrl {
        code,
        ctrl_id,
        payload,
        ctrl_index: Some(index),
    });
    paragraph.controls.push(control);
    Ok(())
}

fn append_field(
    paragraph: &mut hwp_model::Paragraph,
    name: &str,
    value: &str,
    shape: hwp_model::CharShapeId,
) -> Result<(), ComposeError> {
    let ctrl_id = *b"%clk";
    let raw_children = vec![hwp_model::OpaqueRecord {
        tag: 0x0057,
        data: hwp_convert::field::make_field_ctrl_data(name),
        children: Vec::new(),
    }];
    append_control(
        paragraph,
        hwp_model::ctrl_char::FIELD_START,
        hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id,
            data: vec![0_u8; 11],
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children,
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
        }),
    )?;
    set_run_shape(paragraph, shape);
    append_text(paragraph, value);
    paragraph.chars.push(hwp_model::HwpChar::InlineCtrl {
        code: hwp_model::ctrl_char::FIELD_END,
        payload: hwp_convert::field::field_end_payload(&ctrl_id),
    });
    Ok(())
}

fn append_text(paragraph: &mut hwp_model::Paragraph, value: &str) {
    for character in value.chars() {
        if character == '\t' {
            paragraph.chars.push(hwp_model::HwpChar::InlineCtrl {
                code: hwp_model::ctrl_char::TAB,
                payload: vec![0; 12],
            });
        } else {
            paragraph.chars.push(hwp_model::HwpChar::Text(character));
        }
    }
}

fn set_run_shape(paragraph: &mut hwp_model::Paragraph, shape: hwp_model::CharShapeId) {
    let position = paragraph.wchar_len();
    if paragraph
        .char_shape_runs
        .last()
        .is_some_and(|(_, current)| *current == shape)
    {
        return;
    }
    if let Some(last) = paragraph.char_shape_runs.last_mut()
        && last.0 == position
    {
        last.1 = shape;
    } else {
        paragraph.char_shape_runs.push((position, shape));
    }
}

fn page_number_position_code(position: PageNumberPosition) -> u8 {
    match position {
        PageNumberPosition::TopLeft => 1,
        PageNumberPosition::TopCenter => 2,
        PageNumberPosition::TopRight => 3,
        PageNumberPosition::BottomLeft => 4,
        PageNumberPosition::BottomCenter => 5,
        PageNumberPosition::BottomRight => 6,
        PageNumberPosition::Outside => 8,
        PageNumberPosition::Inside => 10,
    }
}

fn mm_to_hwp(value: f32) -> i32 {
    (f64::from(value) / 25.4 * f64::from(hwp_model::HwpUnit::PER_INCH)).round() as i32
}

fn pt_to_hwp(value: f32) -> i32 {
    (f64::from(value) * f64::from(hwp_model::HwpUnit::PER_PT)).round() as i32
}

fn count_document_paragraphs(document: &hwp_model::Document) -> usize {
    fn count(paragraph: &hwp_model::Paragraph) -> usize {
        1 + paragraph
            .controls
            .iter()
            .map(|control| match control {
                hwp_model::Control::Table(table) => table
                    .cells
                    .iter()
                    .flat_map(|cell| &cell.paragraphs)
                    .map(count)
                    .sum(),
                hwp_model::Control::Generic(generic) => generic
                    .paragraph_lists
                    .iter()
                    .flat_map(|list| &list.paragraphs)
                    .map(count)
                    .sum(),
                _ => 0,
            })
            .sum::<usize>()
    }
    document
        .sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .map(count)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_spec() -> DocumentSpec {
        DocumentSpec {
            version: SpecVersion::V1,
            metadata: MetadataSpec::default(),
            page: PageSpec::default(),
            styles: BTreeMap::new(),
            lists: BTreeMap::new(),
            sections: vec![SectionSpec {
                id: None,
                page: None,
                header: None,
                footer: None,
                page_number: None,
                blocks: vec![BlockSpec::Paragraph {
                    style: None,
                    list: None,
                    keep_with_next: false,
                    runs: vec![RunSpec::Text {
                        text: "본문".to_string(),
                        style: None,
                        format: RunFormatSpec::default(),
                    }],
                }],
            }],
        }
    }

    fn issue_codes(error: ComposeError) -> Vec<String> {
        error.issues().into_iter().map(|issue| issue.code).collect()
    }

    #[test]
    fn nested_run_unknown_property_is_rejected_in_json_and_yaml() {
        let json = r#"{
          "version": "1.0",
          "sections": [{
            "blocks": [{
              "type": "paragraph",
              "runs": [{"type": "text", "text": "x", "font_weight": 700}]
            }]
          }]
        }"#;
        let yaml = r#"
version: "1.0"
sections:
  - blocks:
      - type: paragraph
        runs:
          - type: text
            text: x
            font_weight: 700
"#;

        assert!(matches!(
            parse_spec(json, SpecInputFormat::Json),
            Err(ComposeError::Parse { .. })
        ));
        assert!(matches!(
            parse_spec(yaml, SpecInputFormat::Yaml),
            Err(ComposeError::Parse { .. })
        ));
    }

    #[test]
    fn unknown_block_and_table_cell_properties_are_rejected() {
        let block = r#"{
          "version": "1.0",
          "sections": [{"blocks": [{
            "type": "break", "kind": "page", "unknown": true
          }]}]
        }"#;
        let cell = r#"
version: "1.0"
sections:
  - blocks:
      - type: table
        columns:
          - width_mm: 20
        rows:
          - cells:
              - unknown: true
"#;
        assert!(matches!(
            parse_spec(block, SpecInputFormat::Json),
            Err(ComposeError::Parse { .. })
        ));
        assert!(matches!(
            parse_spec(cell, SpecInputFormat::Yaml),
            Err(ComposeError::Parse { .. })
        ));
    }

    #[test]
    fn flattened_known_run_format_is_accepted() {
        let json = r#"{
          "version": "1.0",
          "sections": [{
            "blocks": [{
              "type": "paragraph",
              "runs": [{"type": "text", "text": "x", "bold": true}]
            }]
          }]
        }"#;

        let parsed = parse_spec(json, SpecInputFormat::Json).expect("known format property");
        let BlockSpec::Paragraph { runs, .. } = &parsed.sections[0].blocks[0] else {
            panic!("paragraph expected");
        };
        let RunSpec::Text { format, .. } = &runs[0] else {
            panic!("text run expected");
        };
        assert_eq!(format.bold, Some(true));
    }

    #[test]
    fn names_are_bounded_by_unicode_scalars_not_utf8_bytes() {
        let mut accepted = minimal_spec();
        accepted
            .styles
            .insert("한".repeat(MAX_NAME_CHARS), StyleSpec::default());
        validate_spec(&accepted, Path::new(".")).expect("128 Korean scalars are valid");

        let mut rejected = minimal_spec();
        rejected
            .styles
            .insert("한".repeat(MAX_NAME_CHARS + 1), StyleSpec::default());
        let error = validate_spec(&rejected, Path::new(".")).expect_err("129 scalars");
        assert!(issue_codes(error).iter().any(|code| code == "invalid_name"));
    }

    #[test]
    fn section_ids_are_unique_logical_keys() {
        let mut spec = minimal_spec();
        spec.sections[0].id = Some("same".to_string());
        spec.sections.push(spec.sections[0].clone());
        let error = validate_spec(&spec, Path::new(".")).expect_err("duplicate id");
        assert!(issue_codes(error).iter().any(|code| code == "duplicate_id"));
    }

    #[test]
    fn bullet_start_cannot_be_silently_ignored() {
        let mut spec = minimal_spec();
        spec.lists.insert(
            "bullet".to_string(),
            ListSpec {
                kind: ListKind::Bullet,
                levels: vec![ListLevelSpec {
                    marker: "○".to_string(),
                    start: 2,
                    style: None,
                }],
            },
        );
        let error = validate_spec(&spec, Path::new(".")).expect_err("bullet start");
        assert!(error.issues().iter().any(|issue| {
            issue.code == "invalid_value" && issue.path == "$.lists.bullet.levels[0].start"
        }));
    }

    #[test]
    fn oversized_table_grid_is_rejected_before_occupancy_allocation() {
        let mut spec = minimal_spec();
        spec.sections[0].blocks = vec![BlockSpec::Table {
            width_mm: None,
            columns: vec![TableColumnSpec { width_mm: 1.0 }; 101],
            rows: vec![TableRowSpec { cells: Vec::new() }; 1001],
        }];

        let error = validate_spec(&spec, Path::new(".")).expect_err("grid exceeds limit");
        assert!(
            issue_codes(error)
                .iter()
                .any(|code| code == "limit_exceeded")
        );
    }

    #[test]
    fn declared_table_width_must_match_columns_and_page_body() {
        let mut spec = minimal_spec();
        spec.sections[0].blocks = vec![BlockSpec::Table {
            width_mm: Some(50.0),
            columns: vec![
                TableColumnSpec { width_mm: 20.0 },
                TableColumnSpec { width_mm: 20.0 },
            ],
            rows: vec![TableRowSpec {
                cells: vec![
                    TableCellSpec {
                        col_span: 1,
                        row_span: 1,
                        blocks: Vec::new(),
                    },
                    TableCellSpec {
                        col_span: 1,
                        row_span: 1,
                        blocks: Vec::new(),
                    },
                ],
            }],
        }];

        let error = validate_spec(&spec, Path::new(".")).expect_err("width mismatch");
        assert!(
            issue_codes(error)
                .iter()
                .any(|code| code == "inconsistent_width")
        );
    }

    #[test]
    fn compile_maps_tab_to_inline_control() {
        let mut spec = minimal_spec();
        let BlockSpec::Paragraph { runs, .. } = &mut spec.sections[0].blocks[0] else {
            panic!("paragraph expected");
        };
        *runs = vec![RunSpec::Text {
            text: "가\t나".to_string(),
            style: None,
            format: RunFormatSpec::default(),
        }];

        let compiled = compile_spec(&spec, Path::new("."), Path::new("out.hwpx"), true, false)
            .expect("compile");
        let paragraph = &compiled.document.sections[0].paragraphs[1];
        assert!(paragraph.chars.iter().any(|character| matches!(
            character,
            hwp_model::HwpChar::InlineCtrl {
                code: hwp_model::ctrl_char::TAB,
                payload
            } if payload.len() == 12
        )));
        assert!(!paragraph.chars.contains(&hwp_model::HwpChar::Text('\t')));
    }

    #[test]
    fn non_empty_image_alt_fails_closed_for_block_and_run() {
        let root =
            std::env::temp_dir().join(format!("hwp-document-spec-alt-{}", std::process::id()));
        let asset = PathBuf::from("assets/image.gif");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join(&asset), b"GIF89a\x01\x00\x01\x00").unwrap();

        let mut block_spec = minimal_spec();
        block_spec.sections[0].blocks = vec![BlockSpec::Image {
            path: asset.clone(),
            width_mm: 10.0,
            height_mm: None,
            alt: Some("설명".to_string()),
            placement: ImagePlacement::Inline,
        }];
        let error = validate_spec(&block_spec, &root).expect_err("alt unsupported");
        assert!(
            error.issues().iter().any(|issue| {
                issue.code == "unsupported_native" && issue.path.ends_with(".alt")
            })
        );

        let mut run_spec = minimal_spec();
        {
            let BlockSpec::Paragraph { runs, .. } = &mut run_spec.sections[0].blocks[0] else {
                panic!("paragraph expected");
            };
            *runs = vec![RunSpec::Image {
                path: asset.clone(),
                width_mm: 10.0,
                height_mm: None,
                alt: Some(String::new()),
            }];
        }
        validate_spec(&run_spec, &root).expect("empty alt is absent-equivalent");
        let BlockSpec::Paragraph { runs, .. } = &mut run_spec.sections[0].blocks[0] else {
            panic!("paragraph expected");
        };
        let RunSpec::Image { alt, .. } = &mut runs[0] else {
            panic!("image run expected");
        };
        *alt = Some("설명".to_string());
        let error = validate_spec(&run_spec, &root).expect_err("run alt unsupported");
        assert!(
            error.issues().iter().any(|issue| {
                issue.code == "unsupported_native" && issue.path.ends_with(".alt")
            })
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn omitted_image_height_preserves_intrinsic_aspect_ratio() {
        let root =
            std::env::temp_dir().join(format!("hwp-document-spec-ratio-{}", std::process::id()));
        let asset = PathBuf::from("assets/image.gif");
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join(&asset), b"GIF89a\x02\x00\x01\x00").unwrap();
        let mut spec = minimal_spec();
        spec.sections[0].blocks = vec![BlockSpec::Image {
            path: asset.clone(),
            width_mm: 20.0,
            height_mm: None,
            alt: None,
            placement: ImagePlacement::Inline,
        }];

        let compiled =
            compile_spec(&spec, &root, &root.join("out.hwpx"), true, false).expect("compile image");
        let picture = compiled.document.sections[0].paragraphs[1]
            .controls
            .iter()
            .find_map(|control| match control {
                hwp_model::Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .expect("picture");
        assert_eq!(picture.width.0, mm_to_hwp(20.0));
        assert_eq!(picture.height.0, mm_to_hwp(10.0));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_in_examples_parse_and_schema_hash_is_frozen() {
        let json_spec = parse_spec(
            include_str!("../../../examples/document-spec-v1/basic.json"),
            SpecInputFormat::Json,
        )
        .expect("basic JSON");
        parse_spec(
            include_str!("../../../examples/document-spec-v1/comprehensive.yaml"),
            SpecInputFormat::Yaml,
        )
        .expect("comprehensive YAML");
        let yaml = serde_yaml::to_string(&json_spec).unwrap();
        assert_eq!(
            parse_spec(&yaml, SpecInputFormat::Yaml).expect("same model from YAML"),
            json_spec
        );

        let digest: [u8; 32] = Sha256::digest(include_bytes!(
            "../../../schemas/document-spec-v1.schema.json"
        ))
        .into();
        let actual = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            actual,
            "1607cb19c9068306da8c76ba6ebee4ae8e5c6d650490fc0737dadd1a08b9ed1b"
        );
    }
}
