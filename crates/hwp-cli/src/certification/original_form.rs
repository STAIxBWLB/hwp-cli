//! Original-form certification (#419): checks a delivered HWP/HWPX artifact against a local,
//! reviewed expectation plan for a supplied native form.
//!
//! The plan names expected cell values by table/row/column address, required form-start
//! anchors, the stable table grid, and explicit allowances (repeated profile tables, changed
//! year labels). Everything here is read-only: nothing mutates the document or writes into
//! it. The report carries only plan-chosen stable IDs, cause codes and next-step codes; the
//! expected text stays in the local plan, and the optional address map (opt-in) carries
//! addresses, never document text.
//!
//! Render evidence comes from the same isolated, budgeted layout the certification render
//! uses ([`hwp_render::layout_segments_isolated_bounded`]). Fit and clipping are never inferred
//! from text length; they are read from the layout's typed render issues and segment rows.
//! Without a pinned font environment, page placement is reported as limited evidence instead
//! of a pass, and nothing here implies Hancom rendering parity.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{CheckStatus, read_bounded, sha256_hex, validate_relative_asset_path, validate_sha256};

pub const PLAN_CONTRACT: &str = "hwp-original-form-certification-plan-v1";
pub const ADDRESS_MAP_CONTRACT: &str = "hwp-original-form-address-map-v1";
pub const ADDRESS_MAP_PATH: &str = "original-form/address-map.json";
pub const MAX_PLAN_BYTES: u64 = 1024 * 1024;
pub const MAX_EXPECTED_CELLS: usize = 4096;
pub const MAX_FORM_STARTS: usize = 256;
pub const MAX_TABLE_EXPECTATIONS: usize = 512;
pub const MAX_EXPECTED_TEXT_CHARS: usize = 4096;
pub const MAX_FINDINGS: usize = 1024;
const MAX_ID_BYTES: usize = 64;
const MAX_TABLE_INDEX: usize = 65_535;

/// Policy section: `document.original_form`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalFormPolicy {
    /// Path to the expectation plan JSON, relative to the policy file. The plan holds the
    /// expected text and stays local; it is never copied into the report directory.
    pub plan: String,
    /// When true, publish `original-form/address-map.json`: finding IDs mapped to document
    /// addresses (section/paragraph, table/row/column, page). It never carries text.
    #[serde(default)]
    pub address_map: bool,
}

/// The expectation plan (`original-form-certification-plan-v1`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalFormPlan {
    pub contract: String,
    pub version: u32,
    /// Identity of the supplied source form, echoed into the report to bind the result.
    #[serde(default)]
    pub source_sha256: Option<String>,
    /// When present, the delivered artifact must have exactly this SHA-256.
    #[serde(default)]
    pub output_sha256: Option<String>,
    /// Number of tables (recursive walk order, as `--set-cell` counts them) in the source form.
    #[serde(default)]
    pub table_count: Option<usize>,
    #[serde(default)]
    pub form_starts: Vec<FormStart>,
    #[serde(default)]
    pub tables: Vec<TableExpectation>,
    #[serde(default)]
    pub expected_cells: Vec<ExpectedCell>,
    #[serde(default)]
    pub allowances: Allowances,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormStart {
    pub id: String,
    /// The exact text of the body paragraph that opens the form (whitespace-normalized).
    pub text: String,
    /// The form must begin a page: no earlier body paragraph is drawn on its page.
    #[serde(default = "default_true")]
    pub page_start: bool,
    /// Optional 1-based page the form must start on.
    #[serde(default)]
    pub page: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableExpectation {
    pub id: String,
    pub table: usize,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedCell {
    pub id: String,
    pub table: usize,
    pub row: u16,
    pub col: u16,
    pub text: String,
    /// Allowance: four-digit years (1900-2099) may differ between plan and artifact.
    #[serde(default)]
    pub allow_year_change: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowances {
    /// Profile tables the delivered form repeats: `extra_copies` copies follow `table`
    /// directly in table order and must keep its grid.
    #[serde(default)]
    pub repeated_tables: Vec<RepeatedTable>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatedTable {
    pub id: String,
    pub table: usize,
    pub extra_copies: usize,
}

fn default_true() -> bool {
    true
}

/// The plan after validation, with its identity.
#[derive(Debug, Clone)]
pub struct PreparedPlan {
    pub plan: OriginalFormPlan,
    pub plan_sha256: String,
    pub address_map: bool,
}

/// One content-free finding. `id` is the plan entry's own stable ID (or a fixed ID for a
/// document-wide cause), never document text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct OriginalFormFinding {
    pub id: String,
    pub code: &'static str,
    pub next_step: &'static str,
}

/// `checks.original_form` in the certification report.
#[derive(Debug, Clone, Serialize)]
pub struct OriginalFormCheckReport {
    pub status: CheckStatus,
    pub reason_codes: Vec<String>,
    /// True when the result could not use full evidence (see `missing_evidence`). A limited
    /// result never passes certification on its own: the overall result is at most partial.
    pub limited: bool,
    pub missing_evidence: Vec<&'static str>,
    pub plan_sha256: Option<String>,
    pub source_sha256: Option<String>,
    pub output_sha256: String,
    pub expected_cell_count: usize,
    pub form_start_count: usize,
    pub findings: Vec<OriginalFormFinding>,
    pub findings_complete: bool,
    /// The policy requested the local address map.
    pub address_map: bool,
    /// The map file was published. False when not requested, or when the 258-file manifest cap
    /// left no room for it (T11-N2), so a caller never looks for a file that is not there.
    pub address_map_published: bool,
}

/// A located address for the opt-in local map.
#[derive(Debug, Clone, Serialize)]
pub struct MapEntry {
    pub id: String,
    pub section: Option<usize>,
    pub paragraph: Option<usize>,
    pub table: Option<usize>,
    pub row: Option<u16>,
    pub col: Option<u16>,
    pub page: Option<usize>,
}

/// Render-side evidence the certification pass supplies.
pub struct RenderEvidence<'a> {
    pub segments: &'a hwp_render::segment_map::SegmentMap,
    pub issues: &'a hwp_render::RenderIssueReport,
    /// Font manifest supplied, substitution forbidden, and every requested font matched.
    pub fonts_pinned: bool,
    /// Any requested font was substituted or missing.
    pub font_substituted: bool,
    /// The policy forbids substitution (`document.fonts.forbid_substitution`). Only then is a
    /// substitution disallowed; the existing font policy decides, this check does not widen it.
    pub forbid_substitution: bool,
}

/// Loads and validates the plan named by the policy. Any failure is reported as one
/// content-free refusal by the caller.
pub fn prepare(
    policy: &OriginalFormPolicy,
    policy_base: &Path,
) -> std::result::Result<PreparedPlan, PlanRefusal> {
    validate_relative_asset_path(&policy.plan).map_err(|_| PlanRefusal::Unreadable)?;
    let bytes = read_bounded(&policy_base.join(&policy.plan), MAX_PLAN_BYTES)
        .map_err(|_| PlanRefusal::Unreadable)?;
    let plan: OriginalFormPlan =
        serde_json::from_slice(&bytes).map_err(|_| PlanRefusal::NotJson)?;
    validate_plan(&plan)?;
    Ok(PreparedPlan {
        plan,
        plan_sha256: sha256_hex(&bytes),
        address_map: policy.address_map,
    })
}

/// Content-free cause class of a plan refusal (T11-N1): what to check, never the plan's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanRefusal {
    /// The path is invalid, the file is missing or unreadable, or it exceeds the size bound.
    Unreadable,
    /// The bytes are not JSON of the plan's closed shape (unknown or mistyped fields included).
    NotJson,
    /// `contract` or `version` is not `hwp-original-form-certification-plan-v1` version 1.
    Contract,
    /// An entry count, index, page, text length or hash is out of its bound, or nothing is planned.
    Bounds,
    /// An ID is malformed or repeated, or two expected cells address the same cell.
    Ids,
}

impl PlanRefusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unreadable => "original_form_plan_unreadable",
            Self::NotJson => "original_form_plan_not_json",
            Self::Contract => "original_form_plan_contract",
            Self::Bounds => "original_form_plan_bounds",
            Self::Ids => "original_form_plan_ids",
        }
    }

    pub fn next_step(self) -> &'static str {
        match self {
            Self::Unreadable => "check_the_plan_path_and_size",
            Self::NotJson => "fix_the_plan_against_its_schema",
            Self::Contract => "set_the_plan_contract_and_version",
            Self::Bounds => "bring_the_plan_within_its_bounds",
            Self::Ids => "make_plan_ids_and_cells_unique",
        }
    }
}

fn validate_id(id: &str) -> std::result::Result<(), PlanRefusal> {
    if id.is_empty()
        || id.len() > MAX_ID_BYTES
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(PlanRefusal::Ids);
    }
    Ok(())
}

fn validate_text(text: &str) -> std::result::Result<(), PlanRefusal> {
    if text.chars().count() > MAX_EXPECTED_TEXT_CHARS || text.contains('\0') {
        return Err(PlanRefusal::Bounds);
    }
    Ok(())
}

fn validate_plan(plan: &OriginalFormPlan) -> std::result::Result<(), PlanRefusal> {
    if plan.contract != PLAN_CONTRACT || plan.version != 1 {
        return Err(PlanRefusal::Contract);
    }
    if let Some(hash) = &plan.source_sha256 {
        validate_sha256(hash, "original_form source_sha256").map_err(|_| PlanRefusal::Bounds)?;
    }
    if let Some(hash) = &plan.output_sha256 {
        validate_sha256(hash, "original_form output_sha256").map_err(|_| PlanRefusal::Bounds)?;
    }
    if plan.form_starts.len() > MAX_FORM_STARTS
        || plan.expected_cells.len() > MAX_EXPECTED_CELLS
        || plan.tables.len() > MAX_TABLE_EXPECTATIONS
        || plan.allowances.repeated_tables.len() > MAX_TABLE_EXPECTATIONS
    {
        return Err(PlanRefusal::Bounds);
    }
    if plan.form_starts.is_empty() && plan.expected_cells.is_empty() && plan.tables.is_empty() {
        return Err(PlanRefusal::Bounds);
    }
    let mut ids = BTreeSet::new();
    let mut unique = |id: &str| -> std::result::Result<(), PlanRefusal> {
        validate_id(id)?;
        if !ids.insert(id.to_string()) {
            return Err(PlanRefusal::Ids);
        }
        Ok(())
    };
    for start in &plan.form_starts {
        unique(&start.id)?;
        validate_text(&start.text)?;
        if normalize(&start.text).is_empty() {
            return Err(PlanRefusal::Bounds);
        }
        if start
            .page
            .is_some_and(|page| page == 0 || page > super::MAX_PAGE_NUMBER)
        {
            return Err(PlanRefusal::Bounds);
        }
    }
    for table in &plan.tables {
        unique(&table.id)?;
        if table.table > MAX_TABLE_INDEX || table.rows == 0 || table.cols == 0 {
            return Err(PlanRefusal::Bounds);
        }
    }
    let mut cells = BTreeSet::new();
    for cell in &plan.expected_cells {
        unique(&cell.id)?;
        validate_text(&cell.text)?;
        if cell.table > MAX_TABLE_INDEX {
            return Err(PlanRefusal::Bounds);
        }
        if !cells.insert((cell.table, cell.row, cell.col)) {
            return Err(PlanRefusal::Ids);
        }
    }
    let mut repeated = BTreeSet::new();
    for allowance in &plan.allowances.repeated_tables {
        unique(&allowance.id)?;
        if allowance.table > MAX_TABLE_INDEX
            || allowance.extra_copies == 0
            || allowance.extra_copies > MAX_TABLE_EXPECTATIONS
            || !repeated.insert(allowance.table)
        {
            return Err(PlanRefusal::Bounds);
        }
    }
    if plan
        .table_count
        .is_some_and(|count| count > MAX_TABLE_INDEX)
    {
        return Err(PlanRefusal::Bounds);
    }
    Ok(())
}

/// Collapses whitespace runs and trims, so line wrapping and paragraph splits in a cell do not
/// decide equality; every visible character still has to match.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replaces each standalone four-digit year 1900-2099 with a fixed token.
fn mask_years(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let window = chars.get(index..index + 4);
        let boundary_before = index == 0 || !chars[index - 1].is_ascii_digit();
        let boundary_after = chars.get(index + 4).is_none_or(|c| !c.is_ascii_digit());
        if let Some(window) = window
            && boundary_before
            && boundary_after
            && window.iter().all(char::is_ascii_digit)
            && matches!((window[0], window[1]), ('1', '9') | ('2', '0'))
        {
            out.push_str("\u{0}YEAR\u{0}");
            index += 4;
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

fn values_match(expected: &str, actual: &str, allow_year_change: bool) -> bool {
    let (expected, actual) = (normalize(expected), normalize(actual));
    if allow_year_change {
        mask_years(&expected) == mask_years(&actual)
    } else {
        expected == actual
    }
}

fn paragraph_text(paragraph: &hwp_model::Paragraph) -> String {
    paragraph.plain_text()
}

/// A table in recursive walk order (the order `--set-cell` indexes), with the segment path
/// the layout recorder gives it when it can be derived (tables in text boxes cannot).
struct TableRef<'a> {
    table: &'a hwp_model::Table,
    path: Option<hwp_render::segment_id::SegmentPath>,
}

fn child(
    path: &hwp_render::segment_id::SegmentPath,
    index: usize,
) -> hwp_render::segment_id::SegmentPath {
    let mut indices = path.indices.clone();
    indices.push(index);
    hwp_render::segment_id::SegmentPath {
        section: path.section,
        indices,
    }
}

fn collect_tables<'a>(
    paragraph: &'a hwp_model::Paragraph,
    path: Option<hwp_render::segment_id::SegmentPath>,
    out: &mut Vec<TableRef<'a>>,
) {
    for (control_index, control) in paragraph.controls.iter().enumerate() {
        match control {
            hwp_model::Control::Table(table) => {
                let table_path = path.as_ref().map(|path| child(path, control_index));
                out.push(TableRef {
                    table,
                    path: table_path.clone(),
                });
                for (cell_index, cell) in table.cells.iter().enumerate() {
                    let cell_path = table_path.as_ref().map(|path| child(path, cell_index));
                    for (para_index, nested) in cell.paragraphs.iter().enumerate() {
                        collect_tables(
                            nested,
                            cell_path.as_ref().map(|path| child(path, para_index)),
                            out,
                        );
                    }
                }
            }
            hwp_model::Control::Generic(generic) => {
                for list in &generic.paragraph_lists {
                    for nested in &list.paragraphs {
                        collect_tables(nested, None, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn all_tables(document: &hwp_model::Document) -> Vec<TableRef<'_>> {
    let mut out = Vec::new();
    for (section_index, section) in document.sections.iter().enumerate() {
        for (para_index, paragraph) in section.paragraphs.iter().enumerate() {
            collect_tables(
                paragraph,
                Some(hwp_render::segment_id::SegmentPath {
                    section: section_index,
                    indices: vec![para_index],
                }),
                &mut out,
            );
        }
    }
    out
}

fn cell_at(table: &hwp_model::Table, row: u16, col: u16) -> Option<(usize, &hwp_model::Cell)> {
    table.cells.iter().enumerate().find(|(_, cell)| {
        cell.row <= row
            && row < cell.row.saturating_add(cell.row_span.max(1))
            && cell.col <= col
            && col < cell.col.saturating_add(cell.col_span.max(1))
    })
}

fn cell_text(cell: &hwp_model::Cell) -> String {
    cell.paragraphs
        .iter()
        .map(paragraph_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Results of evaluating one plan against one delivered document.
pub struct Evaluation {
    pub report: OriginalFormCheckReport,
    pub map: Vec<MapEntry>,
}

struct Collector {
    findings: BTreeSet<OriginalFormFinding>,
    complete: bool,
    map: BTreeMap<String, MapEntry>,
}

impl Collector {
    fn push(&mut self, id: &str, code: &'static str, next_step: &'static str) {
        if self.findings.len() >= MAX_FINDINGS {
            self.complete = false;
            return;
        }
        self.findings.insert(OriginalFormFinding {
            id: id.to_string(),
            code,
            next_step,
        });
    }

    fn locate(&mut self, entry: MapEntry) {
        if self.map.len() < MAX_FINDINGS {
            self.map.entry(entry.id.clone()).or_insert(entry);
        }
    }

    fn relocate_page(&mut self, id: &str, page: usize) {
        if let Some(entry) = self.map.get_mut(id) {
            entry.page = Some(page);
        }
    }
}

fn empty_entry(id: &str) -> MapEntry {
    MapEntry {
        id: id.to_string(),
        section: None,
        paragraph: None,
        table: None,
        row: None,
        col: None,
        page: None,
    }
}

/// Evaluates the plan. `render` is `None` when the certification render did not produce
/// layout evidence (the render failed or was not run); render-dependent checks are then
/// reported as missing evidence.
pub fn evaluate(
    prepared: &PreparedPlan,
    document: &hwp_model::Document,
    output_sha256: &str,
    render: Option<&RenderEvidence<'_>>,
) -> Evaluation {
    let plan = &prepared.plan;
    let mut out = Collector {
        findings: BTreeSet::new(),
        complete: true,
        map: BTreeMap::new(),
    };
    let mut missing_evidence = BTreeSet::new();

    if plan
        .output_sha256
        .as_deref()
        .is_some_and(|expected| expected != output_sha256)
    {
        out.push(
            "output",
            "output_identity_mismatch",
            "certify_the_reviewed_output",
        );
    }

    let tables = all_tables(document);
    let extra: usize = plan
        .allowances
        .repeated_tables
        .iter()
        .map(|allowance| allowance.extra_copies)
        .sum();
    if let Some(count) = plan.table_count
        && tables.len() != count.saturating_add(extra)
    {
        out.push(
            "tables",
            "table_count_changed",
            "restore_the_form_tables_or_declare_repeats",
        );
    }

    // Grid projection: rows and columns of each named table, and of every declared copy.
    let mut grids: Vec<(String, usize, u16, u16)> = plan
        .tables
        .iter()
        .map(|t| (t.id.clone(), t.table, t.rows, t.cols))
        .collect();
    for allowance in &plan.allowances.repeated_tables {
        let original = tables.get(allowance.table);
        for copy in 1..=allowance.extra_copies {
            if let Some(original) = original {
                grids.push((
                    allowance.id.clone(),
                    allowance.table + copy,
                    original.table.rows,
                    original.table.cols,
                ));
            }
        }
        if original.is_none() {
            out.push(
                &allowance.id,
                "repeated_table_missing",
                "restore_the_form_tables_or_declare_repeats",
            );
        }
    }
    for (id, index, rows, cols) in grids {
        match tables.get(index) {
            None => out.push(
                &id,
                "table_missing",
                "restore_the_form_tables_or_declare_repeats",
            ),
            Some(found) if found.table.rows != rows || found.table.cols != cols => {
                out.push(&id, "table_grid_changed", "restore_the_original_grid");
                out.locate(MapEntry {
                    table: Some(index),
                    ..empty_entry(&id)
                });
            }
            Some(_) => {}
        }
    }

    // Page placement, fragmentation and clipping depend on line breaking, which depends on
    // fonts: without a pinned font environment they are missing evidence, not findings.
    let placement = render.filter(|render| render.fonts_pinned);
    if render.is_some() && placement.is_none() {
        missing_evidence.insert("pinned_font_environment");
    }
    // Rows of segments the layout drew, by id.
    let mut drawn: BTreeMap<&str, Vec<&hwp_render::segment_map::SegmentRow>> = BTreeMap::new();
    if let Some(render) = render {
        for row in &render.segments.rows {
            drawn.entry(row.id.as_str()).or_default().push(row);
        }
        if render.segments.truncated {
            missing_evidence.insert("segment_map_truncated");
        }
    } else {
        missing_evidence.insert("render_layout_unavailable");
    }

    for cell in &plan.expected_cells {
        let Some(table) = tables.get(cell.table) else {
            out.push(&cell.id, "expected_cell_missing", "fill_the_expected_cell");
            continue;
        };
        let Some((cell_index, found)) = cell_at(table.table, cell.row, cell.col) else {
            out.push(&cell.id, "expected_cell_missing", "fill_the_expected_cell");
            out.locate(MapEntry {
                table: Some(cell.table),
                row: Some(cell.row),
                col: Some(cell.col),
                ..empty_entry(&cell.id)
            });
            continue;
        };
        let actual = cell_text(found);
        let mut located_page = None;
        if placement.is_some() {
            match &table.path {
                Some(path) => {
                    let id = hwp_render::segment_id::cell_id(&child(path, cell_index), found);
                    let rows = drawn.get(id.as_str());
                    located_page = rows.and_then(|rows| rows.first()).map(|row| row.page + 1);
                    let has_box =
                        rows.is_some_and(|rows| rows.iter().any(|row| row.bbox.is_some()));
                    if !has_box {
                        out.push(
                            &cell.id,
                            "expected_cell_not_rendered",
                            "check_pagination_and_cell_fragmentation",
                        );
                    }
                }
                None => {
                    missing_evidence.insert("cell_render_location_unavailable");
                }
            }
        }
        if !values_match(&cell.text, &actual, cell.allow_year_change) {
            let code = if normalize(&actual).is_empty() {
                "expected_cell_value_missing"
            } else {
                "expected_cell_value_mismatch"
            };
            out.push(&cell.id, code, "fill_the_expected_cell");
            out.locate(MapEntry {
                table: Some(cell.table),
                row: Some(cell.row),
                col: Some(cell.col),
                page: located_page,
                ..empty_entry(&cell.id)
            });
        }
    }

    // Form-start anchors: exactly one top-level body paragraph with the text.
    for start in &plan.form_starts {
        let wanted = normalize(&start.text);
        let matches: Vec<(usize, usize, &hwp_model::Paragraph)> = document
            .sections
            .iter()
            .enumerate()
            .flat_map(|(section, s)| {
                s.paragraphs
                    .iter()
                    .enumerate()
                    .map(move |(index, p)| (section, index, p))
            })
            .filter(|(_, _, p)| normalize(&paragraph_text(p)) == wanted)
            .collect();
        let (section, paragraph, found) = match matches.as_slice() {
            [] => {
                out.push(&start.id, "form_start_missing", "restore_the_form_start");
                continue;
            }
            [single] => *single,
            _ => {
                out.push(
                    &start.id,
                    "form_start_ambiguous",
                    "make_the_form_start_text_unique",
                );
                continue;
            }
        };
        out.locate(MapEntry {
            section: Some(section),
            paragraph: Some(paragraph),
            ..empty_entry(&start.id)
        });
        let Some(render) = placement else {
            continue;
        };
        let path = hwp_render::segment_id::SegmentPath {
            section,
            indices: vec![paragraph],
        };
        let id = hwp_render::segment_id::paragraph_id(&path, found);
        let first = drawn
            .get(id.as_str())
            .and_then(|rows| rows.iter().find(|row| row.bbox.is_some()))
            .copied();
        let Some(first) = first else {
            out.push(
                &start.id,
                "form_start_not_rendered",
                "check_pagination_and_cell_fragmentation",
            );
            continue;
        };
        let page = first.page + 1;
        out.relocate_page(&start.id, page);
        if start.page.is_some_and(|expected| expected != page) {
            out.push(
                &start.id,
                "form_start_on_wrong_page",
                "restore_the_page_break_before_the_form",
            );
        }
        if start.page_start {
            let earlier_on_page = render.segments.rows.iter().any(|row| {
                row.page == first.page
                    && row.kind == hwp_render::segment_map::kind::PARA
                    && row.bbox.is_some()
                    && row.chars.is_some_and(|chars| chars.end > chars.start)
                    && top_level_paragraph(&row.id)
                        .is_some_and(|(s, p)| (s, p) < (section, paragraph))
            });
            if earlier_on_page {
                out.push(
                    &start.id,
                    "form_start_not_at_page_start",
                    "restore_the_page_break_before_the_form",
                );
            }
        }
    }

    if let Some(render) = placement {
        for issue in &render.issues.issues {
            let code = match issue.code {
                hwp_render::RenderIssueCode::TableRowTooTallClipped => "table_row_clipped",
                hwp_render::RenderIssueCode::TableCellFragmentationIncomplete => {
                    "cell_fragmentation_incomplete"
                }
                _ => continue,
            };
            out.push("render", code, "shorten_the_value_or_allow_row_growth");
        }
    }
    if let Some(render) = render
        && render.forbid_substitution
        && render.font_substituted
    {
        out.push(
            "fonts",
            "disallowed_font_substitution",
            "pin_the_requested_fonts_and_rerender",
        );
    }

    let findings: Vec<OriginalFormFinding> = out.findings.into_iter().collect();
    let mut reason_codes: Vec<String> = findings
        .iter()
        .map(|finding| finding.code.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !out.complete {
        reason_codes.push("original_form_findings_truncated".to_string());
    }
    let status = if reason_codes.is_empty() {
        CheckStatus::Passed
    } else {
        CheckStatus::Failed
    };
    let missing_evidence: Vec<&'static str> = missing_evidence.into_iter().collect();
    // The map locates findings only: an entry whose ID produced no finding is dropped.
    let finding_ids: BTreeSet<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    let map: Vec<MapEntry> = if prepared.address_map {
        out.map
            .into_values()
            .filter(|entry| finding_ids.contains(entry.id.as_str()))
            .collect()
    } else {
        Vec::new()
    };
    Evaluation {
        report: OriginalFormCheckReport {
            status,
            reason_codes,
            limited: !missing_evidence.is_empty(),
            missing_evidence,
            plan_sha256: Some(prepared.plan_sha256.clone()),
            source_sha256: plan.source_sha256.clone(),
            output_sha256: output_sha256.to_string(),
            expected_cell_count: plan.expected_cells.len(),
            form_start_count: plan.form_starts.len(),
            findings,
            findings_complete: out.complete,
            address_map: prepared.address_map,
            address_map_published: false,
        },
        map,
    }
}

/// `(section, paragraph)` of a top-level body paragraph segment id
/// (`<checksum>.<section>.<paragraph>`), or `None` for any nested segment.
fn top_level_paragraph(id: &str) -> Option<(usize, usize)> {
    let mut parts = id.split('.');
    let _checksum = parts.next()?;
    let section = parts.next()?.parse().ok()?;
    let paragraph = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((section, paragraph))
}

/// The report section for a plan that could not be loaded or validated: a content-free
/// refusal with no plan identity.
pub fn invalid_plan_report(refusal: PlanRefusal, output_sha256: &str) -> OriginalFormCheckReport {
    OriginalFormCheckReport {
        status: CheckStatus::Failed,
        reason_codes: vec![refusal.code().to_string()],
        limited: false,
        missing_evidence: Vec::new(),
        plan_sha256: None,
        source_sha256: None,
        output_sha256: output_sha256.to_string(),
        expected_cell_count: 0,
        form_start_count: 0,
        findings: vec![OriginalFormFinding {
            id: "plan".to_string(),
            code: refusal.code(),
            next_step: refusal.next_step(),
        }],
        findings_complete: true,
        address_map: false,
        address_map_published: false,
    }
}

/// The report section when the delivered document could not be parsed, so nothing in the
/// plan can be checked.
pub fn unparsed_report(prepared: &PreparedPlan, output_sha256: &str) -> OriginalFormCheckReport {
    OriginalFormCheckReport {
        status: CheckStatus::Failed,
        reason_codes: vec!["original_form_document_unavailable".to_string()],
        limited: true,
        missing_evidence: vec!["parsed_document"],
        plan_sha256: Some(prepared.plan_sha256.clone()),
        source_sha256: prepared.plan.source_sha256.clone(),
        output_sha256: output_sha256.to_string(),
        expected_cell_count: prepared.plan.expected_cells.len(),
        form_start_count: prepared.plan.form_starts.len(),
        findings: vec![OriginalFormFinding {
            id: "document".to_string(),
            code: "original_form_document_unavailable",
            next_step: "fix_the_package_then_recertify",
        }],
        findings_complete: true,
        address_map: prepared.address_map,
        address_map_published: false,
    }
}

/// The published local address map. Addresses only, never text.
pub fn address_map_bytes(report: &OriginalFormCheckReport, map: &[MapEntry]) -> Result<Vec<u8>> {
    let value = serde_json::json!({
        "contract": ADDRESS_MAP_CONTRACT,
        "schema_version": "1.0",
        "plan_sha256": report.plan_sha256,
        "output_sha256": report.output_sha256,
        "entries": map,
    });
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate_layout(document: &hwp_model::Document, plan: serde_json::Value) -> Evaluation {
        let prepared = PreparedPlan {
            plan: serde_json::from_value(plan).unwrap(),
            plan_sha256: "0".repeat(64),
            address_map: false,
        };
        let mut fonts = hwp_render::FontStore::new();
        let mut issues = hwp_render::RenderIssueAccumulator::new();
        let (_, segments) =
            hwp_render::layout::layout_document_with_segments(document, &mut fonts, &mut issues);
        let issues = issues.finish();
        evaluate(
            &prepared,
            document,
            &"0".repeat(64),
            Some(&RenderEvidence {
                segments: &segments,
                issues: &issues,
                // These tests exercise the evaluator's pinned-evidence branch. Geometry comes
                // from cached lines or the same host layout, never from a font-dependent oracle.
                fonts_pinned: true,
                font_substituted: false,
                forbid_substitution: true,
            }),
        )
    }

    #[test]
    fn expected_cell_text_clipped_behind_a_drawn_border_is_rejected() {
        use hwp_model::{Control, HwpChar};
        // Same cache/flow boundary as segment_map's fully/partially clipped paragraph
        // regressions: a visible first paragraph shifts the cached text past the cell edge.
        for height in [1_800, 3_300, 10_000] {
            let mut document = hwp_convert::from_markdown("| a | b |\n| --- | --- |\n| c | d |\n");
            let table = document.sections[0]
                .paragraphs
                .iter_mut()
                .flat_map(|p| &mut p.controls)
                .find_map(|control| match control {
                    Control::Table(table) => Some(table),
                    _ => None,
                })
                .unwrap();
            let cell = &mut table.cells[0];
            let mut cached = cell.paragraphs[0].clone();
            cached.chars.retain(|ch| !matches!(ch, HwpChar::Text(_)));
            let start = cached.wchar_len();
            cached.chars.extend("firstline".chars().map(HwpChar::Text));
            let line = |text_start, v_pos| hwp_model::paragraph::LineSeg {
                text_start,
                v_pos,
                line_height: 1_000,
                text_height: 1_000,
                baseline_gap: 850,
                line_spacing: 0,
                col_start: 0,
                seg_width: 4_000,
                flags: 0,
            };
            cached.line_segs = vec![line(start, 0), line(start + 5, 1_000)];
            cell.paragraphs.push(cached);
            cell.height = hwp_model::units::HwpUnit(height);
            let expected = cell_text(cell);
            let result = evaluate_layout(
                &document,
                serde_json::json!({
                    "contract": PLAN_CONTRACT, "version": 1,
                    "expected_cells": [{"id":"cell", "table":0, "row":0, "col":0, "text":expected}]
                }),
            );
            let clipped = result
                .report
                .findings
                .iter()
                .any(|f| f.id == "cell" && f.code == "expected_cell_clipped");
            assert_eq!(
                clipped,
                height < 10_000,
                "a drawn border must not hide fully or partially clipped text: height={height}, {:?}",
                result.report.findings
            );
        }
    }

    #[test]
    fn a_table_before_the_anchor_prevents_a_page_start_but_an_empty_row_does_not() {
        let document =
            hwp_convert::from_markdown("| a | b |\n| --- | --- |\n| c | d |\n\nANCHOR\n");
        let result = evaluate_layout(
            &document,
            serde_json::json!({
                "contract": PLAN_CONTRACT, "version":1,
                "form_starts":[{"id":"start", "text":"ANCHOR", "page_start":true}]
            }),
        );
        assert!(
            result
                .report
                .findings
                .iter()
                .any(|f| f.id == "start" && f.code == "form_start_not_at_page_start"),
            "a table-bearing paragraph is earlier visible content: {:?}",
            result.report.findings
        );
        let document = hwp_convert::from_markdown("\n\nANCHOR\n");
        let result = evaluate_layout(
            &document,
            serde_json::json!({
                "contract": PLAN_CONTRACT, "version":1,
                "form_starts":[{"id":"start", "text":"ANCHOR", "page_start":true}]
            }),
        );
        assert!(
            result.report.findings.is_empty(),
            "{:?}",
            result.report.findings
        );
    }

    #[test]
    fn year_allowance_masks_only_standalone_years() {
        assert!(values_match("2025학년도", "2026학년도", true));
        assert!(!values_match("2025학년도", "2026학년도", false));
        assert!(!values_match("12025", "12026", true));
        assert!(values_match(" a  b\nc ", "a b c", false));
    }

    #[test]
    fn top_level_ids_are_recognized() {
        assert_eq!(top_level_paragraph("0123456789abcdef.0.4"), Some((0, 4)));
        assert_eq!(top_level_paragraph("0123456789abcdef.0.4.1.2.0"), None);
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    fn report_schema() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../schemas/certification-report-v1.schema.json"
        ))
        .unwrap()
    }

    /// Every code, next step and missing-evidence value the module can emit is in the published
    /// report schema's enums, so a new code cannot ship without a schema change.
    #[test]
    fn emitted_codes_are_published_in_the_report_schema() {
        let source = include_str!("original_form.rs");
        let schema = report_schema();
        let defs = &schema["$defs"]["originalFormCheck"]["properties"];
        let enumerate = |value: &serde_json::Value| -> BTreeSet<String> {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        let codes = enumerate(&schema["$defs"]["originalFormCode"]["enum"]);
        let steps = enumerate(&defs["findings"]["items"]["properties"]["next_step"]["enum"]);
        let missing = enumerate(&defs["missing_evidence"]["items"]["enum"]);
        let quoted: BTreeSet<String> = source
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
            .map(str::to_string)
            .collect();
        for code in &codes {
            assert!(quoted.contains(code), "schema code {code} is never emitted");
        }
        for step in &steps {
            assert!(
                quoted.contains(step),
                "schema next_step {step} is never emitted"
            );
        }
        for value in &missing {
            assert!(
                quoted.contains(value),
                "schema missing_evidence {value} is never emitted"
            );
        }
        let emitted_steps: BTreeSet<&String> = quoted
            .iter()
            .filter(|s| {
                [
                    "fill_", "restore_", "check_", "make_", "shorten_", "pin_", "fix_", "certify_",
                ]
                .iter()
                .any(|p| s.starts_with(p))
            })
            .collect();
        for step in emitted_steps {
            assert!(
                steps.contains(step),
                "emitted next_step {step} is not in the schema"
            );
        }
    }

    #[test]
    fn plan_bounds_match_the_plan_schema() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../schemas/original-form-certification-plan-v1.schema.json"
        ))
        .unwrap();
        let p = &schema["properties"];
        assert_eq!(p["expected_cells"]["maxItems"], MAX_EXPECTED_CELLS);
        assert_eq!(p["form_starts"]["maxItems"], MAX_FORM_STARTS);
        assert_eq!(p["tables"]["maxItems"], MAX_TABLE_EXPECTATIONS);
        assert_eq!(
            schema["$defs"]["text"]["maxLength"],
            MAX_EXPECTED_TEXT_CHARS
        );
        let report = report_schema();
        assert_eq!(
            report["$defs"]["originalFormCheck"]["properties"]["findings"]["maxItems"],
            MAX_FINDINGS
        );
    }
}
