//! `hwp edit` — 기존 문서를 인메모리로 편집해 다시 쓴다.
//!
//! 원본을 IR로 읽어(이미지·opaque 보존) 텍스트 치환·표 셀 설정을 적용한 뒤
//! 출력 포맷으로 저장한다. hwp 출력은 합성 경로(`write_hwp_edited`)를 거쳐
//! 편집으로 낡은 줄 배치·문단 불변식을 다시 세운다. 같은 포맷 hwpx→hwpx는
//! 패키지 외과 수술 경로(`hwpx::patch::rewrite_document_staged`)로, 편집된
//! 콘텐츠 엔트리만 재직렬화하고 나머지 엔트리는 raw 복사로 바이트 보존한다.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::Context;
use hwp_cli::cli::EditArgs;
use hwp_convert::{CharFormat, ImageSize};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::commands::cat::load_document;

enum EditOperation {
    Replace(Vec<String>),
    SetCell(Vec<String>),
    SetCellByLabel {
        specs: Vec<String>,
        table: Option<usize>,
    },
    CreateField(Vec<String>),
    CreateBookmark(Vec<String>),
    CreateHyperlink(Vec<String>),
    InsertImage(Vec<String>),
    Seal(Vec<String>),
    SetField(Vec<String>),
    SetMeta(Vec<String>),
    SetFormat(Vec<String>),
    SetAlign(Vec<String>),
    InsertParaBefore(Vec<String>),
    InsertPara(Vec<String>),
    DeletePara(Vec<String>),
    AddRow(Vec<String>),
    AddCol(Vec<String>),
    DeleteRow(Vec<String>),
    DeleteCol(Vec<String>),
    MergeCells(Vec<String>),
    SplitCell(Vec<String>),
    AddTable(Vec<String>),
    CloneTable(Vec<String>),
    SetPara(Vec<String>),
    SetCellPara(Vec<String>),
    SetPage(Vec<String>),
    DeleteImage(Vec<String>),
    DeleteTable(Vec<String>),
    DeleteField(Vec<String>),
    DeleteBookmark(Vec<String>),
    StyleTables(hwp_convert::OfficialPreset),
    SetTablePlacement {
        placement: hwp_convert::TablePlacement,
        table: Option<usize>,
    },
}

/// MCP처럼 이미 구조화된 호출자가 CLI mini-language를 거치지 않고 전달하는 편집.
///
/// 문자열 안의 `=>`, `=`, `:`, `@`는 데이터 그대로 유지된다. CLI 전용 문자열
/// 파서는 `EditOperation`에만 남기고, JSON/MCP 경계에서는 이 타입만 사용한다.
pub(crate) enum TypedEditOperation {
    Replace {
        from: String,
        to: String,
        /// Narrows the from/to match from the whole document (default) to inside one paragraph
        /// (D-12) — additive, not an alternative to `from` (unlike `pattern` on
        /// `SetPara`/`SetAlign`): `from` is always the matched substring here.
        address: Option<hwp_convert::address::Address>,
    },
    SetCell {
        table: usize,
        row: u16,
        col: u16,
        text: String,
    },
    SetCellByLabel {
        label: String,
        text: String,
        table: Option<usize>,
    },
    CreateField {
        anchor: String,
        name: String,
        value: String,
    },
    CreateBookmark {
        anchor: String,
        name: String,
    },
    CreateHyperlink {
        anchor: String,
        display: String,
        url: String,
    },
    InsertImage {
        anchor: String,
        path: std::path::PathBuf,
        size_mm: Option<(f32, f32)>,
    },
    Seal {
        anchor: String,
        path: std::path::PathBuf,
        size_mm: Option<f32>,
    },
    SetField {
        name: String,
        value: String,
    },
    SetMeta {
        key: String,
        value: String,
    },
    SetFormat {
        pattern: String,
        format: CharFormat,
        /// Address selector (D-12), alternative to `pattern`. `None` when the op used
        /// `pattern`; the pattern-only path is unchanged in that case.
        address: Option<hwp_convert::address::Address>,
    },
    SetAlign {
        pattern: String,
        align: u8,
        /// Address selector (D-12), alternative to `pattern`.
        address: Option<hwp_convert::address::Address>,
    },
    InsertPara {
        anchor: String,
        text: String,
        before: bool,
        /// Address selector (D-12), alternative to `anchor`.
        address: Option<hwp_convert::address::Address>,
        /// Inline paragraph-shape properties for the new paragraph itself (D-08) — reuses
        /// `SetPara`'s `ParaProps`, not a new property vocabulary.
        style: Option<hwp_convert::ParaProps>,
        /// Inline char-format properties for the new paragraph's own run (D-08) — reuses
        /// `SetFormat`'s `CharFormat`.
        char: Option<CharFormat>,
    },
    DeletePara {
        matching: String,
        /// Address selector (D-12), alternative to `matching`.
        address: Option<hwp_convert::address::Address>,
    },
    /// Moves the paragraph at `address` to before/after the paragraph named by `to_address`
    /// (D-17: both must resolve to the same section — checked during preflight, not here).
    MoveParagraph {
        address: hwp_convert::address::Address,
        to_address: hwp_convert::address::Address,
        /// `true` = before `to_address`'s paragraph, `false` = after.
        before: bool,
    },
    /// Raises the addressed paragraph's list level by one, within its existing numbering or
    /// bullet definition (A3). Address-only — there is no anchor/pattern alternative.
    IndentPara {
        address: hwp_convert::address::Address,
    },
    /// Lowers the addressed paragraph's list level by one, within its existing numbering or
    /// bullet definition (A3). Address-only — there is no anchor/pattern alternative.
    OutdentPara {
        address: hwp_convert::address::Address,
    },
    AddRow {
        table: usize,
        at: Option<u16>,
        count: usize,
        template_row: Option<u16>,
    },
    AddCol {
        table: usize,
        at: Option<u16>,
        count: u16,
    },
    DeleteRow {
        table: usize,
        row: u16,
    },
    DeleteCol {
        table: usize,
        col: u16,
    },
    MergeCells {
        table: usize,
        r1: u16,
        c1: u16,
        r2: u16,
        c2: u16,
    },
    SplitCell {
        table: usize,
        row: u16,
        col: u16,
    },
    AddTable {
        anchor: String,
        rows: Vec<Vec<String>>,
    },
    CloneTable {
        source_table: usize,
        anchor: String,
        text_mode: hwp_convert::CloneTextMode,
    },
    SetPara {
        pattern: String,
        /// Paragraph shape converted up to HWPUNIT/pt×100 units (same units as the CLI `parse_para_props`).
        props: hwp_convert::ParaProps,
        /// Address selector (D-12), alternative to `pattern`.
        address: Option<hwp_convert::address::Address>,
    },
    /// Paragraph shape for every paragraph of one cell, addressed like `set_cell` (0-based).
    SetCellPara {
        table: usize,
        row: u16,
        col: u16,
        /// Same units as `SetPara` - both go through hwp-convert's shared scaling.
        props: hwp_convert::ParaProps,
    },
    SetPage {
        /// Page setup converted up to HWPUNIT units (same units as the CLI `apply_page_prop`).
        props: hwp_convert::PageProps,
    },
    DeleteImage {
        anchor: String,
    },
    DeleteTable {
        /// 0-based table index. Mutually exclusive with anchor (exactly one is Some) — enforced at the MCP boundary.
        index: Option<usize>,
        /// The table of the paragraph containing the anchor text. Mutually exclusive with index.
        anchor: Option<String>,
    },
    DeleteField {
        name: String,
    },
    DeleteBookmark {
        name: String,
    },
    /// Constructed via both the CLI path (`EditOperation::StyleTables`) and the MCP `tool_edit`
    /// `style_tables` argument (D-09, plan 06). `preset` is accepted (and parsed) at both
    /// boundaries for symmetry with `hwp new --preset`, but `hwp_convert::style_tables` itself
    /// is purely content-driven (D-07/D-08) and never reads it.
    StyleTables {
        #[allow(dead_code)]
        preset: hwp_convert::OfficialPreset,
    },
    /// #296: 표 배치 전환(글자처럼 취급 on/off). `table`은 0-기반 재귀 표 인덱스,
    /// None이면 문서의 모든 표.
    SetTablePlacement {
        placement: hwp_convert::TablePlacement,
        table: Option<usize>,
    },
}

impl TypedEditOperation {
    fn is_structural(&self) -> bool {
        // Keeps the same classification as the legacy EditOperation::is_structural (the same
        // operation must take the same write path whether it comes via CLI or MCP).
        matches!(
            self,
            Self::InsertImage { .. }
                | Self::Seal { .. }
                | Self::InsertPara { .. }
                | Self::DeletePara { .. }
                | Self::MoveParagraph { .. }
                | Self::AddRow { .. }
                | Self::AddCol { .. }
                | Self::DeleteRow { .. }
                | Self::DeleteCol { .. }
                | Self::MergeCells { .. }
                | Self::SplitCell { .. }
                | Self::AddTable { .. }
                | Self::CloneTable { .. }
                | Self::DeleteImage { .. }
                | Self::DeleteTable { .. }
                | Self::DeleteField { .. }
                | Self::DeleteBookmark { .. }
                // 배치 전환은 레이아웃을 바꾸므로 합성 쓰기 경로를 강제한다.
                | Self::SetTablePlacement { .. }
        )
    }
}

impl EditOperation {
    fn is_structural(&self) -> bool {
        match self {
            Self::InsertImage(_)
            | Self::Seal(_)
            | Self::InsertParaBefore(_)
            | Self::InsertPara(_)
            | Self::DeletePara(_)
            | Self::AddRow(_)
            | Self::AddCol(_)
            | Self::DeleteRow(_)
            | Self::DeleteCol(_)
            | Self::MergeCells(_)
            | Self::SplitCell(_)
            | Self::AddTable(_)
            | Self::CloneTable(_)
            | Self::DeleteImage(_)
            | Self::DeleteTable(_)
            | Self::DeleteField(_)
            | Self::DeleteBookmark(_)
            | Self::SetTablePlacement { .. } => true,
            Self::Replace(_)
            | Self::SetCell(_)
            | Self::SetCellByLabel { .. }
            | Self::CreateField(_)
            | Self::CreateBookmark(_)
            | Self::CreateHyperlink(_)
            | Self::SetField(_)
            | Self::SetMeta(_)
            | Self::SetFormat(_)
            | Self::SetAlign(_)
            | Self::SetPara(_)
            | Self::SetCellPara(_)
            | Self::SetPage(_)
            | Self::StyleTables(_) => false,
        }
    }
}

/// CLI 편집 인자를 실행 순서의 타입화된 작업 목록으로 정규화한다.
///
/// `EditArgs`를 `..` 없이 해체하고 `EditOperation`을 실행할 때도 전수 매칭하므로,
/// 새 편집 플래그나 작업 종류를 추가하면 계획·실행을 함께 갱신할 때까지 컴파일되지 않는다.
pub struct EditPlan {
    operations: Vec<EditOperation>,
    typed_operations: Vec<TypedEditOperation>,
    verify: bool,
    allow_partial: bool,
    /// EDT-06 (D-14): when set, the write-dispatch step swaps `write_validated`/
    /// `write_with_private_input_snapshot(..., Publish)` for `validate_without_publish`/
    /// `..., ValidateOnly`. Every other step (preflight, the apply loop, report construction)
    /// runs identically, which is what makes the dry-run report truthful. `false` for every
    /// caller that never asked for it.
    dry_run: bool,
    /// EDT-06 (D-15): when set, `run()` writes the `edit-report-v1` JSON here — including on
    /// an aborted run (see [`EditAbort`]), so a caller can diagnose why nothing applied instead
    /// of receiving only an error string. The MCP `tool_edit` boundary (D-12) writes the same
    /// artifact through [`emit_edit_report`].
    report: Option<std::path::PathBuf>,
}

struct ResolvedLabelEdit {
    text: String,
    candidate: hwp_convert::FormCellCandidate,
    request: String,
}

struct LabelEditRequest {
    label: String,
    text: String,
    table: Option<usize>,
    request: String,
}

struct LabelPreflight {
    resolved: Vec<Option<ResolvedLabelEdit>>,
    unapplied: Vec<String>,
}

#[derive(Debug)]
pub struct EditReport {
    pub output: String,
    pub applied: usize,
    pub warnings: Vec<String>,
    pub preservation: hwp_model::PreservationReport,
    /// EDT-06: one outcome per `plan.typed_operations` entry, in array order (D-01). Empty for
    /// a run that used only the individual CLI edit flags (`plan.operations`) — the report's
    /// per-op detail is anchored to the `--ops` typed channel, where "op" and segment-id
    /// concepts are already well-defined (D-02/D-13); the legacy flags have no equivalent.
    pub ops: Vec<OpOutcome>,
    /// D-14: true for a `--dry-run` report. A dry-run and a real run of the same batch produce
    /// reports that differ only in this field.
    pub dry_run: bool,
}

/// Which of the `edit-ops-v1` `op` enum this outcome describes — spelled the same way
/// `OpsEntry`'s `#[serde(tag = "op", rename_all = "snake_case")]` spells it (D-03 field-name
/// symmetry), so a consumer never has to learn a second vocabulary for the same op kinds.
fn typed_op_kind(operation: &TypedEditOperation) -> &'static str {
    match operation {
        TypedEditOperation::Replace { .. } => "replace",
        TypedEditOperation::SetCell { .. } => "set_cell",
        TypedEditOperation::SetCellByLabel { .. } => "set_cell_by_label",
        TypedEditOperation::CreateField { .. } => "create_field",
        TypedEditOperation::CreateBookmark { .. } => "create_bookmark",
        TypedEditOperation::CreateHyperlink { .. } => "create_hyperlink",
        TypedEditOperation::InsertImage { .. } => "insert_image",
        TypedEditOperation::Seal { .. } => "seal",
        TypedEditOperation::SetField { .. } => "set_field",
        TypedEditOperation::SetMeta { .. } => "set_meta",
        TypedEditOperation::SetFormat { .. } => "set_format",
        TypedEditOperation::SetAlign { .. } => "set_align",
        TypedEditOperation::InsertPara { .. } => "insert_para",
        TypedEditOperation::DeletePara { .. } => "delete_para",
        TypedEditOperation::MoveParagraph { .. } => "move_para",
        TypedEditOperation::IndentPara { .. } => "indent_para",
        TypedEditOperation::OutdentPara { .. } => "outdent_para",
        TypedEditOperation::AddRow { .. } => "add_row",
        TypedEditOperation::AddCol { .. } => "add_col",
        TypedEditOperation::DeleteRow { .. } => "delete_row",
        TypedEditOperation::DeleteCol { .. } => "delete_col",
        TypedEditOperation::MergeCells { .. } => "merge_cells",
        TypedEditOperation::SplitCell { .. } => "split_cell",
        TypedEditOperation::AddTable { .. } => "add_table",
        TypedEditOperation::CloneTable { .. } => "clone_table",
        TypedEditOperation::SetPara { .. } => "set_para",
        TypedEditOperation::SetCellPara { .. } => "set_cell_para",
        TypedEditOperation::SetPage { .. } => "set_page",
        TypedEditOperation::DeleteImage { .. } => "delete_image",
        TypedEditOperation::DeleteTable { .. } => "delete_table",
        TypedEditOperation::DeleteField { .. } => "delete_field",
        TypedEditOperation::DeleteBookmark { .. } => "delete_bookmark",
        TypedEditOperation::StyleTables { .. } => "style_tables",
        TypedEditOperation::SetTablePlacement { .. } => "set_table_placement",
    }
}

/// One `edit-report-v1` `changed` entry (D-13): both ids per change, `null` on either side
/// marking a creation or a removal.
#[derive(Debug, Clone, Serialize)]
pub struct IdPair {
    before: Option<String>,
    after: Option<String>,
}

/// `edit-report-v1` `ops[].status` (D-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpStatus {
    Applied,
    Failed,
}

/// One `edit-report-v1` `ops[]` entry: what one addressed or unaddressed op in the batch did.
#[derive(Debug, Clone, Serialize)]
pub struct OpOutcome {
    index: usize,
    op: String,
    status: OpStatus,
    pieces_touched: usize,
    changed: Vec<IdPair>,
    reason: Option<String>,
}

/// `edit-report-v1`'s wire shape (schema_version/contract first, D-15's house convention) — a
/// thin serialization view over [`EditReport`], which also carries fields (`warnings`,
/// `preservation`) that are not part of the published report contract.
#[derive(Debug, Serialize)]
struct EditReportV1<'a> {
    schema_version: &'static str,
    contract: &'static str,
    output: &'a str,
    dry_run: bool,
    ops: &'a [OpOutcome],
    applied_count: usize,
    failed_count: usize,
}

impl EditReportV1<'_> {
    fn from_report(report: &EditReport) -> EditReportV1<'_> {
        let applied_count = report
            .ops
            .iter()
            .filter(|op| op.status == OpStatus::Applied)
            .count();
        let failed_count = report.ops.len() - applied_count;
        EditReportV1 {
            schema_version: "1.0",
            contract: "hwp-edit-report-v1",
            output: &report.output,
            dry_run: report.dry_run,
            ops: &report.ops,
            applied_count,
            failed_count,
        }
    }
}

/// EDT-06: carries a fully-populated [`EditReport`] alongside an aborted `execute()` run
/// (unapplied requests without `--allow-partial`, or zero applicable edits) so `run()` can still
/// write `--report`'s file — a caller diagnosing why nothing applied needs the `ops` array, not
/// only an error string. `Display` reproduces exactly the message the old bare `anyhow::bail!`
/// produced, so existing callers that match on the error text see no change. `pub(crate)` since
/// D-12 (Task 2 decision, Option A, 2026-09-24): the MCP `tool_edit` error path downcasts to
/// this and writes the same report artifact the CLI writes on an aborted batch.
#[derive(Debug)]
pub(crate) struct EditAbort {
    pub(crate) report: EditReport,
    reason: String,
}

impl std::fmt::Display for EditAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for EditAbort {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputFormat {
    Hwp,
    Hwpx,
    Json,
    Markdown,
}

impl OutputFormat {
    fn from_path(output: &Path) -> anyhow::Result<Self> {
        match output
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("hwp") => Ok(Self::Hwp),
            Some("hwpx") => Ok(Self::Hwpx),
            Some("json") => Ok(Self::Json),
            Some("md") | Some("markdown") => Ok(Self::Markdown),
            other => anyhow::bail!("출력 포맷을 추론할 수 없습니다 (확장자: {other:?})"),
        }
    }

    fn supports_verify(self) -> bool {
        matches!(self, Self::Hwp | Self::Hwpx)
    }
}

impl EditPlan {
    pub fn from_args(args: EditArgs) -> (std::path::PathBuf, std::path::PathBuf, Self) {
        let EditArgs {
            input,
            output,
            // --ops는 from_ops 전용 — 개별 플래그 경로에서는 무의미하다.
            ops: _,
            replace,
            set_cell,
            set_cell_by_label,
            label_table,
            set_field,
            set_meta,
            create_field,
            create_bookmark,
            create_hyperlink,
            insert_image,
            seal,
            set_format,
            set_align,
            insert_para,
            insert_para_before,
            delete_para,
            add_row,
            add_col,
            delete_row,
            delete_col,
            merge_cells,
            split_cell,
            add_table,
            clone_table,
            set_para,
            set_cell_para,
            set_page,
            delete_image,
            delete_table,
            delete_field,
            delete_bookmark,
            style_tables,
            table_placement,
            table,
            verify,
            allow_partial,
            report,
            dry_run,
        } = args;

        let mut operations = Vec::new();
        macro_rules! add {
            ($variant:ident, $specs:ident) => {
                if !$specs.is_empty() {
                    operations.push(EditOperation::$variant($specs));
                }
            };
        }
        add!(Replace, replace);
        add!(SetCell, set_cell);
        if !set_cell_by_label.is_empty() {
            operations.push(EditOperation::SetCellByLabel {
                specs: set_cell_by_label,
                table: label_table,
            });
        }
        add!(CreateField, create_field);
        add!(CreateBookmark, create_bookmark);
        add!(CreateHyperlink, create_hyperlink);
        add!(InsertImage, insert_image);
        add!(Seal, seal);
        add!(SetField, set_field);
        add!(SetMeta, set_meta);
        add!(SetFormat, set_format);
        add!(SetAlign, set_align);
        add!(InsertParaBefore, insert_para_before);
        add!(InsertPara, insert_para);
        add!(DeletePara, delete_para);
        add!(AddRow, add_row);
        add!(AddCol, add_col);
        add!(DeleteRow, delete_row);
        add!(DeleteCol, delete_col);
        add!(MergeCells, merge_cells);
        add!(SplitCell, split_cell);
        add!(AddTable, add_table);
        add!(CloneTable, clone_table);
        add!(SetPara, set_para);
        // After SetPara and therefore after SetCell: in one invocation --set-cell-para
        // restyles the paragraphs the same invocation just created.
        add!(SetCellPara, set_cell_para);
        add!(SetPage, set_page);
        add!(DeleteImage, delete_image);
        add!(DeleteTable, delete_table);
        add!(DeleteField, delete_field);
        add!(DeleteBookmark, delete_bookmark);
        if let Some(preset) = style_tables {
            operations.push(EditOperation::StyleTables(preset.canonical()));
        }
        if let Some(placement) = table_placement {
            operations.push(EditOperation::SetTablePlacement {
                placement: placement.canonical(),
                table,
            });
        }

        (
            input,
            output,
            Self {
                operations,
                typed_operations: Vec::new(),
                verify,
                allow_partial,
                dry_run,
                report,
            },
        )
    }

    /// `--ops` 편집 계획: 연산 파일을 읽어 타입화된 작업 목록으로 정규화한다.
    ///
    /// 파일 읽기·스키마 검증·의미 제약 위반은 [`crate::edit_ops::load_ops`]가
    /// 한국어 오류로 거부한다. `verify`/`allow_partial`는 개별 플래그 값을 그대로
    /// 쓴다. `ops`는 `--ops`가 주어진 호출에서만 만들어지므로 Some이 보장된다.
    pub fn from_ops(
        args: EditArgs,
    ) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf, Self)> {
        let EditArgs {
            input,
            output,
            ops,
            verify,
            allow_partial,
            report,
            dry_run,
            ..
        } = args;
        let ops = ops.expect("from_ops is only called when --ops is set");
        let typed_operations = crate::edit_ops::load_ops(&ops)?;
        Ok((
            input,
            output,
            Self {
                operations: Vec::new(),
                typed_operations,
                verify,
                allow_partial,
                dry_run,
                report,
            },
        ))
    }

    /// Used by the MCP `tool_edit` boundary only. Since D-12 (Phase 9) the MCP surface carries
    /// `dry_run` and `report` with the same semantics as the CLI `--ops` channel, so both come
    /// in as parameters threaded into the struct literal exactly the way [`Self::from_ops`]
    /// threads them — one plan type, one execution path, no MCP-only fields.
    pub(crate) fn from_typed(
        operations: Vec<TypedEditOperation>,
        verify: bool,
        allow_partial: bool,
        dry_run: bool,
        report: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            operations: Vec::new(),
            typed_operations: operations,
            verify,
            allow_partial,
            dry_run,
            report,
        }
    }

    fn replacement_pairs(&self) -> anyhow::Result<Option<Vec<(String, String)>>> {
        if self.typed_operations.is_empty() {
            let [EditOperation::Replace(specs)] = self.operations.as_slice() else {
                return Ok(None);
            };
            let pairs = specs
                .iter()
                .map(|spec| {
                    let (from, to) = spec.split_once("=>").with_context(|| {
                        format!("--replace 형식은 \"찾기=>바꾸기\" 입니다: {spec:?}")
                    })?;
                    Ok((from.to_string(), to.to_string()))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            return Ok(Some(pairs));
        }
        // T-07-06: an addressed replace must never take this package-preserving fast path — it
        // never loads the document, so it cannot run the address preflight or scope the rewrite
        // to one paragraph. Only an all-pattern-form batch is eligible.
        if self.operations.is_empty()
            && self.typed_operations.iter().all(|operation| {
                matches!(operation, TypedEditOperation::Replace { address: None, .. })
            })
        {
            return Ok(Some(
                self.typed_operations
                    .iter()
                    .map(|operation| match operation {
                        TypedEditOperation::Replace {
                            from,
                            to,
                            address: None,
                        } => (from.clone(), to.clone()),
                        _ => unreachable!("all로 Replace(주소 없음) 여부를 확인함"),
                    })
                    .collect(),
            ));
        }
        Ok(None)
    }
}

/// Render an optional insertion boundary for progress output.
fn fmt_at(at: Option<u16>) -> String {
    at.map(|a| a.to_string())
        .unwrap_or_else(|| "end".to_string())
}

/// Parsed `--add-row` spec: `TABLE[:AT[:COUNT[:TEMPLATE_ROW]]]` (#77).
struct AddRowSpec {
    table: usize,
    at: Option<u16>,
    count: usize,
    template_row: Option<u16>,
}

/// Parsed `--add-col` spec: `TABLE[:AT[:COUNT]]` (#77).
struct AddColSpec {
    table: usize,
    at: Option<u16>,
    count: u16,
}

/// Parse the insertion-boundary field: omitted, empty, or `end` means append.
fn parse_insert_at(field: Option<&str>, flag: &str) -> anyhow::Result<Option<u16>> {
    match field.map(str::trim) {
        None | Some("") | Some("end") => Ok(None),
        Some(s) => s
            .parse::<u16>()
            .map(Some)
            .with_context(|| format!("{flag} 위치는 숫자 또는 \"end\" 입니다: {s:?}")),
    }
}

fn parse_add_row_spec(spec: &str) -> anyhow::Result<AddRowSpec> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() > 4 || parts.first().is_none_or(|p| p.trim().is_empty()) {
        anyhow::bail!("--add-row 형식은 \"표[:위치[:개수[:템플릿행]]]\" 입니다: {spec:?}");
    }
    let table: usize = parts[0].trim().parse().context("표 인덱스")?;
    let at = parse_insert_at(parts.get(1).copied(), "--add-row")?;
    let count = match parts.get(2).map(|s| s.trim()) {
        None | Some("") => 1,
        Some(s) => s.parse::<usize>().context("개수")?,
    };
    if count == 0 {
        anyhow::bail!("--add-row 개수는 1 이상이어야 합니다: {spec:?}");
    }
    let template_row = match parts.get(3).map(|s| s.trim()) {
        None | Some("") => None,
        Some(s) => Some(s.parse::<u16>().context("템플릿 행")?),
    };
    Ok(AddRowSpec {
        table,
        at,
        count,
        template_row,
    })
}

fn parse_add_col_spec(spec: &str) -> anyhow::Result<AddColSpec> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() > 3 || parts.first().is_none_or(|p| p.trim().is_empty()) {
        anyhow::bail!("--add-col 형식은 \"표[:위치[:개수]]\" 입니다: {spec:?}");
    }
    let table: usize = parts[0].trim().parse().context("표 인덱스")?;
    let at = parse_insert_at(parts.get(1).copied(), "--add-col")?;
    let count = match parts.get(2).map(|s| s.trim()) {
        None | Some("") => 1,
        Some(s) => s.parse::<u16>().context("개수")?,
    };
    if count == 0 {
        anyhow::bail!("--add-col 개수는 1 이상이어야 합니다: {spec:?}");
    }
    Ok(AddColSpec { table, at, count })
}

/// Parsed `--clone-table` spec: `SOURCE_TABLE=>ANCHOR[=>blank|keep]` (#78).
struct CloneTableSpec {
    source_table: usize,
    anchor: String,
    text_mode: hwp_convert::CloneTextMode,
}

fn parse_clone_table_spec(spec: &str) -> anyhow::Result<CloneTableSpec> {
    let mut parts = spec.splitn(3, "=>");
    let source = parts.next().unwrap_or_default().trim();
    let anchor = parts.next().map(str::trim).unwrap_or_default();
    if source.is_empty() || anchor.is_empty() {
        anyhow::bail!("--clone-table 형식은 \"표=>앵커[=>blank|keep]\" 입니다: {spec:?}");
    }
    let source_table: usize = source.parse().context("표 인덱스")?;
    let text_mode = match parts.next().map(str::trim) {
        None | Some("") | Some("blank") => hwp_convert::CloneTextMode::Blank,
        Some("keep") => hwp_convert::CloneTextMode::Keep,
        Some(other) => anyhow::bail!("--clone-table 모드는 blank|keep 입니다: {other:?}"),
    };
    Ok(CloneTableSpec {
        source_table,
        anchor: anchor.to_string(),
        text_mode,
    })
}

pub fn run(input: &Path, output: &Path, plan: &EditPlan) -> anyhow::Result<()> {
    match execute(input, output, plan) {
        Ok(report) => {
            emit_edit_report(plan, &report)?;
            crate::commands::convert::print_warnings(&report.warnings);
            crate::commands::preservation::print_report(&report.preservation);
            if plan.dry_run {
                eprintln!(
                    "편집 dry-run 완료: {} (출력 파일은 게시하지 않음)",
                    input.display()
                );
            } else {
                eprintln!("편집 완료: {} → {}", input.display(), output.display());
            }
            Ok(())
        }
        Err(err) => {
            // EDT-06/D-14: an aborted run still leaves a diagnosable artifact when `--report`
            // was given — write it, then propagate the SAME error `err` carries (EditAbort's
            // Display reproduces the original bail! message verbatim).
            if let Some(abort) = err.downcast_ref::<EditAbort>() {
                emit_edit_report(plan, &abort.report)?;
            }
            Err(err)
        }
    }
}

/// Writes `--report <path>`'s file (staged, per `write_loss_report`'s precedent — T-07-24), or
/// prints the report to stdout when `--dry-run` was given without `--report`, mirroring
/// `template.rs`'s `print_report || dry_run` idiom. A no-op when neither was requested. The MCP
/// `tool_edit` boundary (D-12) calls this only when its `report` argument was given, so the
/// stdout branch never fires there — stdout is the MCP protocol channel.
pub(crate) fn emit_edit_report(plan: &EditPlan, report: &EditReport) -> anyhow::Result<()> {
    let report_v1 = EditReportV1::from_report(report);
    if let Some(path) = &plan.report {
        let bytes = serde_json::to_vec_pretty(&report_v1)?;
        crate::commands::output::write_validated(
            path,
            None,
            |staged| {
                std::fs::write(staged, &bytes)?;
                Ok(())
            },
            |staged, _| {
                let written = std::fs::read(staged)?;
                if written != bytes {
                    anyhow::bail!("편집 보고서 검증 중 바이트 불일치: {}", staged.display());
                }
                Ok(())
            },
        )?;
    } else if plan.dry_run {
        println!("{}", serde_json::to_string_pretty(&report_v1)?);
    }
    Ok(())
}

pub fn execute(input: &Path, output: &Path, plan: &EditPlan) -> anyhow::Result<EditReport> {
    let output_format = OutputFormat::from_path(output)?;
    if plan.verify && !output_format.supports_verify() {
        anyhow::bail!(
            "--verify는 HWP/HWPX 출력에서만 지원합니다. JSON/Markdown 출력은 --verify 없이 사용하세요"
        );
    }

    // 고속 경로: hwpx→hwpx이고 --replace뿐이면 패키지 보존 패치로 처리한다(IR 재작성 시
    // 미리보기·hp:switch 호환 블록·미모델 엔트리가 손실). 한계: <hp:t> 런 분절을
    // 가로지르는 문자열은 매칭되지 않는다(경고 출력).
    let replacement_pairs = plan.replacement_pairs()?;
    if let Some(pairs) = replacement_pairs
        && output_format == OutputFormat::Hwpx
        && matches!(
            crate::format::detect(input)?,
            crate::format::FileFormat::Hwpx
        )
    {
        let writer = |staged: &Path| patch_replacements_staged(input, staged, output, &pairs, plan);
        let verifier = |staged: &Path, _: &_| {
            if plan.verify {
                verify_output(staged, None)?;
            }
            Ok(())
        };
        // D-14: this fast path publishes on its own (it never goes through the write-dispatch
        // section below), so it needs its own dry-run gate.
        let report = if plan.dry_run {
            crate::commands::output::validate_without_publish(
                output,
                Some(input),
                writer,
                verifier,
            )?
        } else {
            crate::commands::output::write_validated(output, Some(input), writer, verifier)?
        };
        for (entry, n) in &report.counts {
            eprintln!("치환(패키지 보존): {entry} ({n}건)");
        }
        return Ok(EditReport {
            output: output.display().to_string(),
            applied: report.applied_requests,
            warnings: report.warnings,
            preservation: hwp_model::PreservationReport::new(),
            // One outcome per typed replace, equal to the apply loop's for the same batch (#332).
            ops: report.ops,
            dry_run: plan.dry_run,
        });
    }

    let mut doc = load_document(input)?;
    let original_doc = doc.clone();
    // Label lookup is a preflight, not another mutation primitive: every request
    // observes the original document and errors before a staged output exists.
    let label_preflight = preflight_label_edits(plan, &mut doc)?;
    if !label_preflight.unapplied.is_empty() && !plan.allow_partial {
        anyhow::bail!(
            "적용되지 않은 편집 요청이 있습니다: {} (--allow-partial로 일치한 요청만 적용 가능)",
            label_preflight.unapplied.join(", ")
        );
    }
    // D-06: resolves every address against the document AS LOADED, before op 1 applies. Always
    // aborts on any failure (Task 1 decision 6) — no `&& !plan.allow_partial` here, unlike the
    // label preflight above.
    let resolved_addresses = preflight_addressed_ops(plan, &doc)?;
    // Sibling pass: resolves move_para's DESTINATION reference (preflight_addressed_ops already
    // covers its SOURCE address above), same D-04/D-06 always-abort discipline.
    let resolved_move_destinations = preflight_move_destinations(plan, &doc)?;
    let conflicts = detect_conflicts(plan, &doc, &resolved_addresses, &resolved_move_destinations);
    if !conflicts.is_empty() {
        anyhow::bail!(
            "주소 지정 편집이 서로 충돌합니다:\n{}",
            conflicts
                .iter()
                .map(|conflict| match conflict.kind {
                    ConflictKind::LengthChange => format!(
                        "op[{}]이 문단 {}의 길이를 바꿀 수 있어 op[{}]의 고정된 run 범위가 무효화됩니다",
                        conflict.earlier_index, conflict.paragraph, conflict.later_index
                    ),
                    ConflictKind::Removal => format!(
                        "op[{}]이 문단 {}을(를) 제거하여 op[{}]이 같은 주소(또는 그 하위 경로)를 가리킬 수 없습니다",
                        conflict.earlier_index, conflict.paragraph, conflict.later_index
                    ),
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    // EDT-06: kept alongside the consuming iterators below for post-hoc report construction —
    // one random-access slot per `plan.typed_operations` entry, `Some` only where preflight
    // resolved a target, exactly mirroring `resolved_addresses`/`resolved_move_destinations`.
    let resolved_addresses_for_report = resolved_addresses.clone();
    let resolved_move_destinations_for_report = resolved_move_destinations.clone();
    let mut resolved_addresses = resolved_addresses.into_iter();
    let mut resolved_move_destinations = resolved_move_destinations.into_iter();
    let mut resolved_label_edits = label_preflight.resolved.into_iter();
    let mut edits = 0usize;
    let mut unapplied = label_preflight.unapplied;
    // Index-drift tracker (planner decision 3, #358): threaded through the apply loop below so
    // every addressed op maps its preflight-resolved ORIGINAL path through whatever paragraphs
    // earlier addressed insert_para/delete_para/move_para ops in this batch inserted or removed.
    let mut offsets = IndexOffsets::default();
    // EDT-06/D-13: the document state addresses were resolved against (D-06) — the "before"
    // reference for re-deriving every touched paragraph/run's id once the typed-op loop below
    // has mutated `doc`. Only cloned when there is a typed op to report on; cloned ONCE (not
    // per-op) and read via `&mut` reborrows since `paragraph_at_mut` needs mutable access even
    // though every use here is read-only.
    let mut doc_before_typed_ops = (!plan.typed_operations.is_empty()).then(|| doc.clone());
    // 구조 편집(문단/행 추가·삭제·이미지 삽입)은 합성 경로로 써야 한다 — 삽입 문단/행
    // 불변식 + 그림 도형 레코드 합성(빈-extras Picture)이 적용되도록.
    let structural = plan.operations.iter().any(EditOperation::is_structural)
        || plan
            .typed_operations
            .iter()
            .any(TypedEditOperation::is_structural);
    // D-08: re-styling an already-correctly-styled document (every GFM table is styled at
    // import time regardless of preset, per plan 03) is a legitimate no-op, not a failure — its
    // output is EXPECTED to equal its input byte-for-byte. The generic "no visible effect"
    // publish guard below must not treat that as an error the way it does for a --replace/
    // --set-* request that silently matched nothing. `--table-placement` (#296) follows the
    // same contract: reapplying an already-applied placement is a byte-identical no-op.
    let requested_idempotent_table_op = plan.operations.iter().any(|op| {
        matches!(
            op,
            EditOperation::StyleTables(_) | EditOperation::SetTablePlacement { .. }
        )
    }) || plan.typed_operations.iter().any(|op| {
        matches!(
            op,
            TypedEditOperation::StyleTables { .. } | TypedEditOperation::SetTablePlacement { .. }
        )
    });

    for operation in &plan.operations {
        match operation {
            EditOperation::Replace(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (from, to) = spec.split_once("=>").with_context(|| {
                        format!("--replace 형식은 \"찾기=>바꾸기\" 입니다: {spec:?}")
                    })?;
                    if from.is_empty() || from == to {
                        unapplied.push(format!("--replace {spec:?}"));
                        continue;
                    }
                    let n = hwp_convert::replace_text(&mut doc, from, to, true);
                    eprintln!("치환: {from:?} → {to:?} ({n}건)");
                    record_effect(
                        &before,
                        &doc,
                        format!("--replace {spec:?}"),
                        &mut edits,
                        &mut unapplied,
                    );
                }
            }
            EditOperation::SetCell(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (loc, text) = spec.split_once('=').with_context(|| {
                        format!("--set-cell 형식은 \"표:행:열=값\" 입니다: {spec:?}")
                    })?;
                    let (ti, r, c) = parse_cell_loc(loc, "--set-cell")?;
                    hwp_convert::set_cell(&mut doc, ti, r, c, text)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("셀 설정: 표{ti} ({r},{c}) = {text:?}");
                    record_effect(
                        &before,
                        &doc,
                        format!("--set-cell {spec:?}"),
                        &mut edits,
                        &mut unapplied,
                    );
                }
            }
            EditOperation::SetCellByLabel { specs, .. } => {
                for _ in specs {
                    let resolved = resolved_label_edits
                        .next()
                        .expect("label edits are resolved during preflight");
                    let Some(resolved) = resolved else {
                        continue;
                    };
                    let before = doc.clone();
                    hwp_convert::set_cell(
                        &mut doc,
                        resolved.candidate.table,
                        resolved.candidate.row,
                        resolved.candidate.col,
                        &resolved.text,
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                    eprintln!(
                        "양식 셀 설정: 표{} ({},{})",
                        resolved.candidate.table, resolved.candidate.row, resolved.candidate.col
                    );
                    record_effect(&before, &doc, resolved.request, &mut edits, &mut unapplied);
                }
            }
            // 누름틀 생성은 set_field보다 먼저 — 같은 호출에서 생성한 필드를 바로 채울 수 있게.
            EditOperation::CreateField(specs) => {
                for spec in specs {
                    let (anchor, rest) = spec.split_once("=>").with_context(|| {
                        format!(
                            "--create-field 형식은 \"앵커=>이름\" 또는 \"앵커=>이름=값\" 입니다: {spec:?}"
                        )
                    })?;
                    let (name, value) = rest.split_once('=').unwrap_or((rest, ""));
                    if hwp_convert::create_field(&mut doc, anchor, name, value) {
                        eprintln!("누름틀 생성: {anchor:?} 뒤에 이름={name:?} 값={value:?}");
                        edits += 1;
                    } else {
                        eprintln!("경고: 앵커 {anchor:?}를 찾지 못했습니다");
                        unapplied.push(format!("--create-field {spec:?}"));
                    }
                }
            }
            EditOperation::CreateBookmark(specs) => {
                for spec in specs {
                    let (anchor, name) = spec.split_once("=>").with_context(|| {
                        format!("--create-bookmark 형식은 \"앵커=>이름\" 입니다: {spec:?}")
                    })?;
                    if hwp_convert::create_bookmark(&mut doc, anchor, name) {
                        eprintln!("책갈피 생성: {anchor:?} 뒤에 이름={name:?}");
                        edits += 1;
                    } else {
                        eprintln!("경고: 앵커 {anchor:?}를 찾지 못했습니다");
                        unapplied.push(format!("--create-bookmark {spec:?}"));
                    }
                }
            }
            EditOperation::CreateHyperlink(specs) => {
                for spec in specs {
                    // "앵커=>URL"(표시=URL) 또는 "앵커=>표시=>URL". URL 쿼리의 '='와 충돌 없게 "=>"로 분할.
                    let parts: Vec<&str> = spec.split("=>").collect();
                    let (anchor, display, url) = match parts.as_slice() {
                        [a, u] => (*a, *u, *u),
                        [a, d, u] => (*a, *d, *u),
                        _ => anyhow::bail!(
                            "--create-hyperlink 형식은 \"앵커=>URL\" 또는 \"앵커=>표시=>URL\" 입니다: {spec:?}"
                        ),
                    };
                    if hwp_convert::create_hyperlink(&mut doc, anchor, url, display) {
                        eprintln!("하이퍼링크 생성: {anchor:?} 뒤에 표시={display:?} URL={url:?}");
                        edits += 1;
                    } else {
                        eprintln!("경고: 앵커 {anchor:?}를 찾지 못했습니다");
                        unapplied.push(format!("--create-hyperlink {spec:?}"));
                    }
                }
            }
            EditOperation::InsertImage(specs) => {
                for spec in specs {
                    let (anchor, rhs) = spec.split_once("=>").with_context(|| {
                        format!("--insert-image 형식은 \"앵커=>경로\" 또는 \"앵커=>경로@너비x높이\"(mm) 입니다: {spec:?}")
                    })?;
                    let (path, size) = parse_image_size(rhs)?;
                    hwp_convert::insert_image(&mut doc, anchor, Path::new(path), size)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("이미지 삽입: {anchor:?} 뒤에 {path:?}");
                    edits += 1;
                }
            }
            EditOperation::Seal(specs) => {
                for spec in specs {
                    let (anchor, rhs) = spec.split_once("=>").with_context(|| {
                        format!("--seal 형식은 \"앵커=>경로\" 또는 \"앵커=>경로@크기mm\" 입니다: {spec:?}")
                    })?;
                    let (path, size_mm) = parse_seal_size(rhs);
                    let measure = seal_measurer(&doc);
                    hwp_convert::insert_seal(&mut doc, anchor, Path::new(path), size_mm, measure)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("도장 날인: {anchor:?} 위에 {path:?}");
                    edits += 1;
                }
            }
            EditOperation::SetField(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (name, value) = spec.split_once('=').with_context(|| {
                        format!("--set-field 형식은 \"이름=값\" 입니다: {spec:?}")
                    })?;
                    let n = hwp_convert::set_field(&mut doc, name, value);
                    if n == 0 {
                        eprintln!("경고: 필드 {name:?}를 찾지 못했습니다 (hwp fields로 이름 확인)");
                        unapplied.push(format!("--set-field {spec:?}"));
                    } else {
                        eprintln!("필드 설정: {name:?} = {value:?} ({n}건)");
                    }
                    if n > 0 {
                        record_effect(
                            &before,
                            &doc,
                            format!("--set-field {spec:?}"),
                            &mut edits,
                            &mut unapplied,
                        );
                    }
                }
            }
            EditOperation::SetMeta(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    hwp_convert::apply_meta(&mut doc, spec).map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("메타데이터 설정: {spec}");
                    record_effect(
                        &before,
                        &doc,
                        format!("--set-meta {spec:?}"),
                        &mut edits,
                        &mut unapplied,
                    );
                }
            }
            EditOperation::SetFormat(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (pattern, attrs) = spec.split_once(':').with_context(|| {
                        format!("--set-format 형식은 \"찾기:속성=값,…\" 입니다: {spec:?}")
                    })?;
                    let fmt = parse_char_format(attrs)?;
                    let n = hwp_convert::set_char_format(&mut doc, pattern, &fmt);
                    if n == 0 {
                        eprintln!("경고: 서식 대상 {pattern:?}를 찾지 못했습니다");
                        unapplied.push(format!("--set-format {spec:?}"));
                    } else {
                        eprintln!("글자 서식: {pattern:?} ({n}건)");
                    }
                    if n > 0 {
                        record_effect(
                            &before,
                            &doc,
                            format!("--set-format {spec:?}"),
                            &mut edits,
                            &mut unapplied,
                        );
                    }
                }
            }
            EditOperation::SetAlign(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (pattern, name) = spec.split_once('=').with_context(|| {
                        format!("--set-align 형식은 \"찾기=정렬\" 입니다: {spec:?}")
                    })?;
                    let align = parse_align(name)?;
                    let n = hwp_convert::set_para_align(&mut doc, pattern, align);
                    if n == 0 {
                        eprintln!("경고: 정렬 대상 {pattern:?}를 찾지 못했습니다");
                        unapplied.push(format!("--set-align {spec:?}"));
                    } else {
                        eprintln!("문단 정렬: {pattern:?} = {name:?} ({n}건)");
                    }
                    if n > 0 {
                        record_effect(
                            &before,
                            &doc,
                            format!("--set-align {spec:?}"),
                            &mut edits,
                            &mut unapplied,
                        );
                    }
                }
            }
            EditOperation::InsertParaBefore(specs) => {
                for spec in specs {
                    let (anchor, text) = spec.split_once("=>").with_context(|| {
                        format!("--insert-para-before 형식은 \"앵커=>텍스트\" 입니다: {spec:?}")
                    })?;
                    if hwp_convert::insert_paragraph(&mut doc, anchor, text, true) {
                        eprintln!("문단 삽입(앞): {anchor:?} 앞에 {text:?}");
                        edits += 1;
                    } else {
                        eprintln!("{}", paragraph_miss_message(&doc, anchor, "앵커"));
                        unapplied.push(format!("--insert-para-before {spec:?}"));
                    }
                }
            }
            EditOperation::InsertPara(specs) => {
                for spec in specs {
                    let (anchor, text) = spec.split_once("=>").with_context(|| {
                        format!("--insert-para 형식은 \"앵커=>텍스트\" 입니다: {spec:?}")
                    })?;
                    if hwp_convert::insert_paragraph(&mut doc, anchor, text, false) {
                        eprintln!("문단 삽입(뒤): {anchor:?} 뒤에 {text:?}");
                        edits += 1;
                    } else {
                        eprintln!("{}", paragraph_miss_message(&doc, anchor, "앵커"));
                        unapplied.push(format!("--insert-para {spec:?}"));
                    }
                }
            }
            EditOperation::DeletePara(specs) => {
                for matching in specs {
                    let n = hwp_convert::delete_paragraph(&mut doc, matching);
                    if n == 0 {
                        eprintln!(
                            "{}",
                            paragraph_miss_message(&doc, matching, "삭제 대상 문단")
                        );
                        unapplied.push(format!("--delete-para {matching:?}"));
                    } else {
                        eprintln!("문단 삭제: {matching:?} ({n}건)");
                    }
                    edits += n;
                }
            }
            EditOperation::AddRow(specs) => {
                for spec in specs {
                    let s = parse_add_row_spec(spec)?;
                    hwp_convert::add_rows_at(&mut doc, s.table, s.at, s.count, s.template_row)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!(
                        "표 행 추가: 표{} 위치{} 개수{} 템플릿{}",
                        s.table,
                        fmt_at(s.at),
                        s.count,
                        fmt_at(s.template_row)
                    );
                    edits += 1;
                }
            }
            EditOperation::AddCol(specs) => {
                for spec in specs {
                    let s = parse_add_col_spec(spec)?;
                    hwp_convert::add_table_columns(&mut doc, s.table, s.at, s.count)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!(
                        "표 열 추가: 표{} 위치{} 개수{} (전체 폭 유지)",
                        s.table,
                        fmt_at(s.at),
                        s.count
                    );
                    edits += 1;
                }
            }
            EditOperation::DeleteRow(specs) => {
                for spec in specs {
                    let (t, r) = spec.split_once(':').with_context(|| {
                        format!("--delete-row 형식은 \"표:행\" 입니다: {spec:?}")
                    })?;
                    let ti: usize = t.trim().parse().context("표 인덱스")?;
                    let row: u16 = r.trim().parse().context("행 번호")?;
                    hwp_convert::delete_table_row(&mut doc, ti, row)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("표 행 삭제: 표{ti} 행{row}");
                    edits += 1;
                }
            }
            EditOperation::DeleteCol(specs) => {
                for spec in specs {
                    let (t, c) = spec.split_once(':').with_context(|| {
                        format!("--delete-col 형식은 \"표:열\" 입니다: {spec:?}")
                    })?;
                    let ti: usize = t.trim().parse().context("표 인덱스")?;
                    let col: u16 = c.trim().parse().context("열 번호")?;
                    hwp_convert::delete_table_column(&mut doc, ti, col)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("표 열 삭제: 표{ti} 열{col} (전체 폭 유지)");
                    edits += 1;
                }
            }
            EditOperation::MergeCells(specs) => {
                for spec in specs {
                    let parts: Vec<&str> = spec.split(':').collect();
                    if parts.len() != 5 {
                        anyhow::bail!("--merge-cells 형식은 \"표:r1:c1:r2:c2\" 입니다: {spec:?}");
                    }
                    let ti: usize = parts[0].trim().parse().context("표 인덱스")?;
                    let r1: u16 = parts[1].trim().parse().context("r1")?;
                    let c1: u16 = parts[2].trim().parse().context("c1")?;
                    let r2: u16 = parts[3].trim().parse().context("r2")?;
                    let c2: u16 = parts[4].trim().parse().context("c2")?;
                    hwp_convert::merge_cells(&mut doc, ti, r1, c1, r2, c2)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("셀 병합: 표{ti} ({r1},{c1})-({r2},{c2})");
                    edits += 1;
                }
            }
            EditOperation::SplitCell(specs) => {
                for spec in specs {
                    let parts: Vec<&str> = spec.split(':').collect();
                    if parts.len() != 3 {
                        anyhow::bail!("--split-cell 형식은 \"표:행:열\" 입니다: {spec:?}");
                    }
                    let ti: usize = parts[0].trim().parse().context("표 인덱스")?;
                    let r: u16 = parts[1].trim().parse().context("행 번호")?;
                    let c: u16 = parts[2].trim().parse().context("열 번호")?;
                    hwp_convert::split_cell(&mut doc, ti, r, c).map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("셀 분할: 표{ti} ({r},{c})");
                    edits += 1;
                }
            }
            EditOperation::AddTable(specs) => {
                for spec in specs {
                    let (anchor, json) = spec.split_once("=>").with_context(|| {
                        format!("--add-table 형식은 \"앵커=>행JSON\" 입니다: {spec:?}")
                    })?;
                    let rows: Vec<Vec<String>> = serde_json::from_str(json).with_context(|| {
                        format!("--add-table 행 데이터는 문자열 배열의 배열이어야 합니다: {json:?}")
                    })?;
                    hwp_convert::add_table(&mut doc, anchor, &rows)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!(
                        "표 삽입: {anchor:?} 뒤 ({}x{})",
                        rows.len(),
                        rows.first().map_or(0, Vec::len)
                    );
                    edits += 1;
                }
            }
            EditOperation::CloneTable(specs) => {
                for spec in specs {
                    let s = parse_clone_table_spec(spec)?;
                    hwp_convert::clone_table(&mut doc, s.source_table, &s.anchor, s.text_mode)
                        .map_err(|e| anyhow::anyhow!(e))?;
                    let mode = match s.text_mode {
                        hwp_convert::CloneTextMode::Blank => "blank",
                        hwp_convert::CloneTextMode::Keep => "keep",
                    };
                    eprintln!("표 복제: 표{} → {:?} 뒤 ({mode})", s.source_table, s.anchor);
                    edits += 1;
                }
            }
            EditOperation::SetPara(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (pattern, kv) = spec.split_once("=>").with_context(|| {
                        format!("--set-para 형식은 \"찾기=>키:값\" 입니다: {spec:?}")
                    })?;
                    let props = parse_para_props(kv, "--set-para")?;
                    let n = hwp_convert::set_para_props(&mut doc, pattern, &props);
                    if n == 0 {
                        eprintln!("경고: 문단모양 대상 {pattern:?}를 찾지 못했습니다");
                        unapplied.push(format!("--set-para {spec:?}"));
                    } else {
                        eprintln!("문단 모양: {pattern:?} {kv} ({n}건)");
                        record_effect(
                            &before,
                            &doc,
                            format!("--set-para {spec:?}"),
                            &mut edits,
                            &mut unapplied,
                        );
                    }
                }
            }
            EditOperation::SetCellPara(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (loc, kv) = spec.split_once("=>").with_context(|| {
                        format!("--set-cell-para 형식은 \"표:행:열=>키:값\" 입니다: {spec:?}")
                    })?;
                    let (ti, r, c) = parse_cell_loc(loc, "--set-cell-para")?;
                    let props = parse_para_props(kv, "--set-cell-para")?;
                    let n = hwp_convert::set_cell_para_props(&mut doc, ti, r, c, &props)
                        .map_err(|e| anyhow::anyhow!("--set-cell-para {loc:?}: {e}"))?;
                    if n == 0 {
                        eprintln!("경고: 셀 문단모양 대상 {loc:?}에 적용할 속성이 없습니다");
                        unapplied.push(format!("--set-cell-para {spec:?}"));
                    } else {
                        eprintln!("셀 문단 모양: 표{ti} ({r},{c}) {kv} ({n}건)");
                        record_effect(
                            &before,
                            &doc,
                            format!("--set-cell-para {spec:?}"),
                            &mut edits,
                            &mut unapplied,
                        );
                    }
                }
            }
            EditOperation::SetPage(specs) => {
                let before = doc.clone();
                let mut props = hwp_convert::PageProps::default();
                for spec in specs {
                    let (key, value) = spec
                        .split_once(':')
                        .with_context(|| format!("--set-page 형식은 \"키:값\" 입니다: {spec:?}"))?;
                    apply_page_prop(&mut props, key.trim(), value.trim())?;
                }
                let n = hwp_convert::set_page_def(&mut doc, &props);
                if n == 0 {
                    eprintln!("경고: 구역 정의를 찾지 못했습니다");
                    unapplied.push("--set-page".to_string());
                } else {
                    eprintln!("페이지 설정: {}건", specs.len());
                    record_effect(
                        &before,
                        &doc,
                        "--set-page".to_string(),
                        &mut edits,
                        &mut unapplied,
                    );
                }
            }
            EditOperation::DeleteImage(specs) => {
                for anchor in specs {
                    let n = hwp_convert::delete_object(
                        &mut doc,
                        hwp_convert::ObjectKind::Image,
                        anchor,
                    );
                    if n == 0 {
                        eprintln!("경고: 그림을 찾지 못했습니다 (앵커 {anchor:?})");
                        unapplied.push(format!("--delete-image {anchor:?}"));
                    } else {
                        eprintln!("그림 삭제: {anchor:?} ({n}건)");
                        edits += n;
                    }
                }
            }
            EditOperation::DeleteTable(specs) => {
                for spec in specs {
                    let n = if let Ok(nth) = spec.trim().parse::<usize>() {
                        hwp_convert::delete_object(
                            &mut doc,
                            hwp_convert::ObjectKind::TableNth(nth),
                            "",
                        )
                    } else {
                        hwp_convert::delete_object(&mut doc, hwp_convert::ObjectKind::Table, spec)
                    };
                    if n == 0 {
                        eprintln!("경고: 표를 찾지 못했습니다 ({spec:?})");
                        unapplied.push(format!("--delete-table {spec:?}"));
                    } else {
                        eprintln!("표 삭제: {spec:?} ({n}건)");
                        edits += n;
                    }
                }
            }
            EditOperation::DeleteField(specs) => {
                for name in specs {
                    let n =
                        hwp_convert::delete_object(&mut doc, hwp_convert::ObjectKind::Field, name);
                    if n == 0 {
                        eprintln!("경고: 필드를 찾지 못했습니다 ({name:?})");
                        unapplied.push(format!("--delete-field {name:?}"));
                    } else {
                        eprintln!("필드 삭제: {name:?} ({n}건)");
                        edits += n;
                    }
                }
            }
            EditOperation::DeleteBookmark(specs) => {
                for name in specs {
                    let n = hwp_convert::delete_object(
                        &mut doc,
                        hwp_convert::ObjectKind::Bookmark,
                        name,
                    );
                    if n == 0 {
                        eprintln!("경고: 책갈피를 찾지 못했습니다 ({name:?})");
                        unapplied.push(format!("--delete-bookmark {name:?}"));
                    } else {
                        eprintln!("책갈피 삭제: {name:?} ({n}건)");
                        edits += n;
                    }
                }
            }
            // `preset` does not yet select divergent values (D-07's values are uniform across
            // all six profiles today) - it validates and records intent for a future divergence.
            EditOperation::StyleTables(_preset) => {
                // (eligible, changed). Only "no styleable table at all" is an unapplied edit;
                // "every table is already styled" is a successful no-op, which is exactly what
                // D-08's byte-stability promises on a second run.
                let (eligible, changed) = hwp_convert::style_tables(&mut doc);
                if eligible == 0 {
                    eprintln!("경고: 스타일링 대상 표를 찾지 못했습니다(1열 표는 건너뜀)");
                    unapplied.push("--style-tables".to_string());
                } else if changed == 0 {
                    eprintln!("표 스타일링: 이미 적용되어 있습니다({eligible}개 확인)");
                } else {
                    eprintln!("표 스타일링: {changed}개");
                    edits += changed;
                }
            }
            EditOperation::SetTablePlacement { placement, table } => {
                apply_table_placement_op(&mut doc, *placement, *table, &mut edits, &mut unapplied);
            }
        }
    }
    // EDT-06: one outcome per `plan.typed_operations` entry, populated as this loop runs —
    // reusing the SAME `edits`/`unapplied` accounting the stderr summary uses (before/after
    // deltas around each call), not a second, independently-derived count.
    let mut ops_outcomes: Vec<OpOutcome> = Vec::with_capacity(plan.typed_operations.len());
    for (index, operation) in plan.typed_operations.iter().enumerate() {
        let target_before = resolved_addresses_for_report[index].clone();
        let to_target_before = resolved_move_destinations_for_report[index].clone();
        // Snapshot BEFORE calling apply_typed_operation: for a structural op (insert_para/
        // move_para), this call's own `offsets.record_insert`/`record_move` below would
        // otherwise self-shift the very anchor/reference path we need — capturing it now is
        // the SAME state `apply_typed_operation` itself reads internally when it runs this op.
        let target_current_path = target_before
            .as_ref()
            .and_then(|target| offsets.current_path(&target.path).ok());
        let to_target_current_path = to_target_before
            .as_ref()
            .and_then(|target| offsets.current_path(&target.path).ok());
        // #358: an op with no address (a pattern, anchor or index form) is invisible to
        // `offsets`. If it adds or removes anything along a later addressed op's path, that
        // address would silently land on a different paragraph, so the batch is refused instead.
        let later_shapes = if target_before.is_none() {
            later_address_shapes(
                index,
                &resolved_addresses_for_report,
                &resolved_move_destinations_for_report,
                &offsets,
                &doc,
            )?
        } else {
            Vec::new()
        };
        let edits_before = edits;
        let unapplied_before = unapplied.len();
        apply_typed_operation(
            operation,
            &mut doc,
            &mut edits,
            &mut unapplied,
            &mut resolved_label_edits,
            &mut resolved_addresses,
            &mut resolved_move_destinations,
            &mut offsets,
        )?;
        let op = typed_op_kind(operation).to_string();
        if let Some(moved) = later_shapes
            .iter()
            .find(|later| path_shape(&doc, &later.current) != later.shape)
        {
            anyhow::bail!(
                "op[{index}] {op}이(가) op[{}] {}의 주소({})가 지나는 문단·개체 목록을 바꿉니다. \
                 주소는 배치 적용 전 문서 기준이라 이 순서로는 다른 문단을 편집하게 됩니다 \
                 (주소 없는 연산을 뒤로 옮기거나, 주소 형식으로 쓰거나, 두 번의 hwp edit로 나누세요)",
                moved.later,
                typed_op_kind(&plan.typed_operations[moved.later]),
                moved.original
            );
        }
        if unapplied.len() > unapplied_before {
            ops_outcomes.push(OpOutcome {
                index,
                op,
                status: OpStatus::Failed,
                pieces_touched: 0,
                changed: Vec::new(),
                reason: Some(failure_reason(operation)),
            });
            continue;
        }
        if edits == edits_before {
            // `set_cell_by_label` whose label was already found unresolvable during
            // `preflight_label_edits` (which runs BEFORE this loop, so its miss is already
            // baked into `unapplied_before`) returns early without touching `edits` or
            // `unapplied` on ITS OWN call — the only op kind not covered by the
            // "advanced edits xor pushed unapplied" invariant every other arm keeps. Report it
            // honestly as failed rather than silently claiming success.
            ops_outcomes.push(OpOutcome {
                index,
                op,
                status: OpStatus::Failed,
                pieces_touched: 0,
                changed: Vec::new(),
                reason: Some("적용되지 않음 (사전 검증 단계에서 이미 확인됨)".to_string()),
            });
            continue;
        }
        let changed = report_changed_ids(
            operation,
            target_before.as_ref(),
            target_current_path.as_ref(),
            to_target_current_path.as_ref(),
            doc_before_typed_ops.as_mut(),
            &mut doc,
        );
        let pieces_touched = changed.len().max(1);
        ops_outcomes.push(OpOutcome {
            index,
            op,
            status: OpStatus::Applied,
            pieces_touched,
            changed,
            reason: None,
        });
    }

    if !unapplied.is_empty() && !plan.allow_partial {
        let reason = format!(
            "적용되지 않은 편집 요청이 있습니다: {} (--allow-partial로 일치한 요청만 적용 가능)",
            unapplied.join(", ")
        );
        return Err(EditAbort {
            report: EditReport {
                output: output.display().to_string(),
                applied: edits,
                warnings: Vec::new(),
                preservation: hwp_model::PreservationReport::new(),
                ops: ops_outcomes,
                dry_run: plan.dry_run,
            },
            reason,
        }
        .into());
    }
    // `--style-tables` on an already-styled document legitimately produces zero edits and a
    // document equal to the original: that IS D-08's guarantee, and refusing to publish would
    // make the second of two identical runs fail. Every other operation still has to change
    // something to earn an output.
    if !requested_idempotent_table_op && (edits == 0 || doc == original_doc) {
        // EDT-06: still leave a diagnosable artifact when `--report`/`--dry-run` was given (a
        // caller needs `ops` to see WHY nothing applied, not just this error string) — write
        // it BEFORE this guard aborts, per the plan's own instruction, rather than after
        // `execute()` has already returned an opaque `Err`.
        return Err(EditAbort {
            report: EditReport {
                output: output.display().to_string(),
                applied: edits,
                warnings: Vec::new(),
                preservation: hwp_model::PreservationReport::new(),
                ops: ops_outcomes,
                dry_run: plan.dry_run,
            },
            reason: "적용 가능한 편집이 없어 출력을 게시하지 않습니다 \
             (--replace/--set-cell/--set-field/--set-meta 등 요청 확인)"
                .to_string(),
        }
        .into());
    }

    let mut warnings = unapplied
        .iter()
        .map(|request| format!("미적용 편집 요청: {request}"))
        .collect::<Vec<_>>();
    let write_staged = |source: &Path, staged: &Path| {
        let mut report = write_output(
            &doc,
            staged,
            structural,
            output_format,
            Some((source, &original_doc)),
        )?;
        if output_format.supports_verify() {
            let removed_binary_allowance =
                crate::commands::preservation::intentional_removed_binary_assets(
                    &original_doc,
                    &doc,
                );
            report.preservation.extend(
                crate::commands::preservation::inspect_same_format_container_with_binary_allowance(
                    source,
                    staged,
                    removed_binary_allowance,
                )?,
            );
        }
        Ok(report)
    };
    let verify_staged = |staged: &Path, writer_report: &hwp_model::WriteReport| {
        if output_format.supports_verify() {
            crate::commands::reject_preservation_loss("edit", &writer_report.preservation)?;
        }
        if plan.verify {
            verify_output(staged, Some(&doc))?;
        }
        Ok(())
    };
    // D-14: dry-run applies the WHOLE batch above exactly like a real run, then routes the
    // write through the same staged verifier `hwp compose --dry-run`/`hwp template --dry-run`
    // already use, discarding instead of publishing — every other step is unchanged, which is
    // what makes the dry-run report truthful about target misses and resulting ids.
    let snapshot_mode = if plan.dry_run {
        crate::commands::output::SnapshotOutputMode::ValidateOnly
    } else {
        crate::commands::output::SnapshotOutputMode::Publish
    };
    let writer_report =
        if output_format == OutputFormat::Hwp && original_doc.meta.source_format == "hwp5" {
            let (_, report) = crate::commands::output::write_with_private_input_snapshot(
                output,
                input,
                hwp_cli::certification::MAX_INPUT_BYTES,
                snapshot_mode,
                |snapshot, staged, _| write_staged(snapshot, staged),
                verify_staged,
            )?;
            report
        } else if plan.dry_run {
            crate::commands::output::validate_without_publish(
                output,
                Some(input),
                |staged| write_staged(input, staged),
                verify_staged,
            )?
        } else {
            crate::commands::output::write_validated(
                output,
                Some(input),
                |staged| write_staged(input, staged),
                verify_staged,
            )?
        };
    warnings.extend(writer_report.warnings);
    Ok(EditReport {
        output: output.display().to_string(),
        applied: edits,
        warnings,
        preservation: writer_report.preservation,
        ops: ops_outcomes,
        dry_run: plan.dry_run,
    })
}

fn preflight_label_edits(
    plan: &EditPlan,
    doc: &mut hwp_model::Document,
) -> anyhow::Result<LabelPreflight> {
    let mut requests = Vec::new();
    for operation in &plan.operations {
        let EditOperation::SetCellByLabel { specs, table } = operation else {
            continue;
        };
        for spec in specs {
            let (label, text) = spec.split_once('=').with_context(|| {
                format!("--set-cell-by-label 형식은 \"레이블=값\" 입니다: {spec:?}")
            })?;
            let normalized = hwp_convert::normalize_form_label(label);
            if normalized.is_empty() {
                anyhow::bail!("--set-cell-by-label 레이블은 비어 있을 수 없습니다");
            }
            requests.push(LabelEditRequest {
                label: normalized,
                text: text.to_string(),
                table: *table,
                request: "set_cell_by_label".to_string(),
            });
        }
    }
    for operation in &plan.typed_operations {
        let TypedEditOperation::SetCellByLabel { label, text, table } = operation else {
            continue;
        };
        let normalized = hwp_convert::normalize_form_label(label);
        if normalized.is_empty() {
            anyhow::bail!("set_cell_by_label 레이블은 비어 있을 수 없습니다");
        }
        requests.push(LabelEditRequest {
            label: normalized,
            text: text.clone(),
            table: *table,
            request: "set_cell_by_label".to_string(),
        });
    }

    let mut resolved = Vec::with_capacity(requests.len());
    let mut unapplied = Vec::new();
    let mut targeted = BTreeSet::new();
    for request in requests {
        if let Some(table) = request.table
            && hwp_convert::table_dims(doc, table).is_none()
        {
            anyhow::bail!("set_cell_by_label의 table 범위가 유효하지 않습니다");
        }
        let candidates = hwp_convert::find_form_cells_by_label(doc, &request.label, request.table);
        match candidates.as_slice() {
            [] => {
                unapplied.push(request.request);
                resolved.push(None);
            }
            [candidate] => {
                if !targeted.insert(*candidate) {
                    anyhow::bail!(
                        "양식 레이블 대상이 중복됩니다: 표{} ({},{})",
                        candidate.table,
                        candidate.row,
                        candidate.col
                    );
                }
                resolved.push(Some(ResolvedLabelEdit {
                    text: request.text,
                    candidate: *candidate,
                    request: "set_cell_by_label".to_string(),
                }));
            }
            many => anyhow::bail!(
                "양식 레이블 대상이 모호합니다: {}",
                many.iter()
                    .map(|candidate| format!(
                        "표{} ({},{})",
                        candidate.table, candidate.row, candidate.col
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    Ok(LabelPreflight {
        resolved,
        unapplied,
    })
}

/// Resolves every addressed target in `plan.typed_operations` against `doc` **as loaded** (D-06:
/// never against a state any earlier op in the batch may have produced), returning one slot per
/// entry so the apply loop can consume it in lockstep with `plan.typed_operations`, exactly as it
/// consumes `resolved_label_edits`. Sibling of `preflight_label_edits`, same insertion point
/// (right after `load_document`/`original_doc`), but never softened by `--allow-partial`:
/// staleness (D-03), an unresolvable path (D-04) and an out-of-range `chars` (A5) are Phase 6
/// D-09's structural layer.
fn preflight_addressed_ops(
    plan: &EditPlan,
    doc: &hwp_model::Document,
) -> anyhow::Result<Vec<Option<hwp_convert::address::ResolvedTarget>>> {
    let mut resolved = Vec::with_capacity(plan.typed_operations.len());
    let mut failures = Vec::new();
    for (index, operation) in plan.typed_operations.iter().enumerate() {
        let target = match operation {
            TypedEditOperation::SetFormat {
                address: Some(address),
                ..
            } => Some((address, hwp_convert::address::Granularity::Run)),
            TypedEditOperation::SetPara {
                address: Some(address),
                ..
            }
            | TypedEditOperation::SetAlign {
                address: Some(address),
                ..
            }
            | TypedEditOperation::Replace {
                address: Some(address),
                ..
            }
            | TypedEditOperation::InsertPara {
                address: Some(address),
                ..
            }
            | TypedEditOperation::DeletePara {
                address: Some(address),
                ..
            }
            | TypedEditOperation::MoveParagraph { address, .. }
            | TypedEditOperation::IndentPara { address }
            | TypedEditOperation::OutdentPara { address } => {
                Some((address, hwp_convert::address::Granularity::Paragraph))
            }
            _ => None,
        };
        let Some((address, granularity)) = target else {
            resolved.push(None);
            continue;
        };
        match hwp_convert::address::resolve(doc, address, granularity) {
            Ok(target) => resolved.push(Some(target)),
            Err(error) => {
                failures.push(format!(
                    "op[{index}] {} — {error}",
                    describe_address(address)
                ));
                resolved.push(None);
            }
        }
    }
    if !failures.is_empty() {
        anyhow::bail!(
            "주소를 해석할 수 없는 편집 연산이 있습니다:\n{}",
            failures.join("\n")
        );
    }
    Ok(resolved)
}

/// Resolves `move_para`'s DESTINATION reference address (`to_address`) for every op in the
/// batch — a sibling pass to [`preflight_addressed_ops`], which already covers `move_para`'s
/// SOURCE `address`. One slot per `plan.typed_operations` entry, `Some` only for
/// `MoveParagraph`, consumed in the same lockstep the apply loop already uses for
/// `resolved_addresses`/`resolved_label_edits`. Same D-04/D-06 discipline: resolved against
/// `doc` as loaded, always aborts on any failure.
fn preflight_move_destinations(
    plan: &EditPlan,
    doc: &hwp_model::Document,
) -> anyhow::Result<Vec<Option<hwp_convert::address::ResolvedTarget>>> {
    let mut resolved = Vec::with_capacity(plan.typed_operations.len());
    let mut failures = Vec::new();
    for (index, operation) in plan.typed_operations.iter().enumerate() {
        let TypedEditOperation::MoveParagraph {
            address,
            to_address,
            ..
        } = operation
        else {
            resolved.push(None);
            continue;
        };
        // D-17: source and destination must name the same section — rejected here, never
        // attempted (it would inherit the G1 section re-emission failure mode nothing tests).
        if address.section != to_address.section {
            failures.push(format!(
                "op[{index}] move_para: 원본 구역({})과 대상 구역({})이 다릅니다 — 구역 간 이동은 지원하지 않습니다",
                address.section, to_address.section
            ));
            resolved.push(None);
            continue;
        }
        match hwp_convert::address::resolve(
            doc,
            to_address,
            hwp_convert::address::Granularity::Paragraph,
        ) {
            Ok(target) => resolved.push(Some(target)),
            Err(error) => {
                failures.push(format!(
                    "op[{index}] to.{} — {error}",
                    describe_address(to_address)
                ));
                resolved.push(None);
            }
        }
    }
    if !failures.is_empty() {
        anyhow::bail!(
            "이동 대상 주소를 해석할 수 없는 편집 연산이 있습니다:\n{}",
            failures.join("\n")
        );
    }
    Ok(resolved)
}

/// One paragraph splice an addressed structural op made, as the path of the paragraph inserted
/// or removed in the document at that moment.
enum Splice {
    Inserted(hwp_convert::SegmentPath),
    Removed(hwp_convert::SegmentPath),
}

/// Index-drift tracker for addressed structural ops (planner decision 3, T-07-10/T-07-11, #358):
/// every paragraph an addressed `insert_para`/`delete_para`/`move_para` inserted or removed, in
/// batch order. Owned by `execute()`, threaded through the apply loop. A later addressed op maps
/// its preflight path through the splices in order: a splice shifts the path only in the list it
/// happened in, only when the path passes through that list at or after the splice, and at any
/// depth (deleting a body paragraph shifts the cell paragraphs of every table below it).
///
/// Ops without an address are never recorded here. `execute()` refuses a batch in which one of
/// them changes the structure along a later addressed op's path (see [`path_shape`]).
#[derive(Default)]
struct IndexOffsets(Vec<Splice>);

impl IndexOffsets {
    /// Where the paragraph preflight resolved at `original_path` (against the pre-batch document)
    /// is now. Checked (T-07-11): a path whose paragraph, or an ancestor of it, an earlier op
    /// removed aborts the run rather than landing somewhere else. The removal clause of
    /// `detect_conflicts` rejects such a batch during preflight, so this is an internal error.
    fn current_path(
        &self,
        original_path: &hwp_convert::SegmentPath,
    ) -> anyhow::Result<hwp_convert::SegmentPath> {
        let mut path = original_path.clone();
        if !shift_through_splices(&self.0, path.section, &mut path.indices) {
            anyhow::bail!(
                "내부 오류: 앞선 연산이 제거한 문단을 가리키는 주소입니다 (원래={original_path})"
            );
        }
        Ok(path)
    }

    fn record_insert(&mut self, inserted: &hwp_convert::SegmentPath) {
        self.0.push(Splice::Inserted(inserted.clone()));
    }

    fn record_delete(&mut self, removed: &hwp_convert::SegmentPath) {
        self.0.push(Splice::Removed(removed.clone()));
    }

    /// A move is a removal at `from` and an insertion where [`moved_paragraph_path`] says.
    fn record_move(
        &mut self,
        from: &hwp_convert::SegmentPath,
        to_ref: &hwp_convert::SegmentPath,
        to_index: usize,
    ) {
        self.record_delete(from);
        self.record_insert(&moved_paragraph_path(from, to_ref, to_index));
    }
}

/// Where `hwp_convert::move_paragraph` puts the paragraph it moves from `from` to `to_index` of
/// the list `to_ref` belongs to. It finds that list after the removal, so the list's own path
/// shifts with the removal first; `to_index` already counts it (see [`move_to_index`]).
fn moved_paragraph_path(
    from: &hwp_convert::SegmentPath,
    to_ref: &hwp_convert::SegmentPath,
    to_index: usize,
) -> hwp_convert::SegmentPath {
    let mut indices = list_prefix(to_ref).to_vec();
    // move_paragraph refuses a destination inside the moved paragraph, so this cannot fail.
    shift_through_splices(
        &[Splice::Removed(from.clone())],
        to_ref.section,
        &mut indices,
    );
    indices.push(to_index);
    hwp_convert::SegmentPath {
        section: to_ref.section,
        indices,
    }
}

/// Moves `indices` (a path in `section`) through `splices` in order. Returns false when a splice
/// removed the paragraph `indices` names or one of its ancestors.
fn shift_through_splices(splices: &[Splice], section: usize, indices: &mut [usize]) -> bool {
    for splice in splices {
        let (at, inserted) = match splice {
            Splice::Inserted(at) => (at, true),
            Splice::Removed(at) => (at, false),
        };
        let depth = list_prefix(at).len();
        if at.section != section
            || indices.len() <= depth
            || indices[..depth] != at.indices[..depth]
        {
            continue;
        }
        let spliced = at.indices[depth];
        let index = &mut indices[depth];
        if inserted {
            if *index >= spliced {
                *index += 1;
            }
        } else if *index > spliced {
            *index -= 1;
        } else if *index == spliced {
            return false;
        }
    }
    true
}

/// #358: the length of every sequence `path` indexes into, from the section's paragraph list down
/// to the list holding the paragraph itself (a paragraph's controls, a table's cells, a cell's
/// paragraphs, a generic control's flat paragraph sequence), following `address::resolve`'s
/// descent. An op that adds to or removes from any of them changes the shape (no edit op adds and
/// removes in one sequence at once), so an unchanged shape means the path still names the same
/// paragraph. A change after the path's own index changes it too: the check is conservative.
fn path_shape(doc: &hwp_model::Document, path: &hwp_convert::SegmentPath) -> Vec<usize> {
    use hwp_model::Control;

    let mut shape = Vec::new();
    let Some(section) = doc.sections.get(path.section) else {
        return shape;
    };
    shape.push(section.paragraphs.len());
    let mut para = path
        .indices
        .first()
        .and_then(|&index| section.paragraphs.get(index));
    let mut i = 1;
    while let Some(current) = para
        && i < path.indices.len()
    {
        shape.push(current.controls.len());
        para = match current.controls.get(path.indices[i]) {
            Some(Control::Table(table)) => {
                shape.push(table.cells.len());
                let cell = path
                    .indices
                    .get(i + 1)
                    .and_then(|&index| table.cells.get(index));
                shape.extend(cell.map(|cell| cell.paragraphs.len()));
                let next = path.indices.get(i + 2);
                i += 3;
                cell.zip(next)
                    .and_then(|(cell, &index)| cell.paragraphs.get(index))
            }
            Some(Control::Generic(generic)) if generic.raw_children.is_empty() => {
                let mut flat = generic
                    .paragraph_lists
                    .iter()
                    .flat_map(|list| &list.paragraphs);
                shape.push(flat.clone().count());
                let next = path.indices.get(i + 1);
                i += 2;
                next.and_then(|&index| flat.nth(index))
            }
            _ => None,
        };
    }
    shape
}

struct LaterAddressShape {
    later: usize,
    original: hwp_convert::SegmentPath,
    current: hwp_convert::SegmentPath,
    shape: Vec<usize>,
}

/// #358: for each list a later addressed op (after `index`) points into, the op that points there
/// first, its preflight path, the path it names now, and that path's [`path_shape`]. One entry per
/// list: every path into the same list has the same shape.
fn later_address_shapes(
    index: usize,
    resolved: &[Option<hwp_convert::address::ResolvedTarget>],
    resolved_move_destinations: &[Option<hwp_convert::address::ResolvedTarget>],
    offsets: &IndexOffsets,
    doc: &hwp_model::Document,
) -> anyhow::Result<Vec<LaterAddressShape>> {
    // ponytail: rescans every later op for each op with no address, quadratic in a mixed batch's
    // length; keep a per-list suffix index if 10,000-op mixed batches get slow.
    let mut lists = std::collections::HashSet::new();
    let mut shapes = Vec::new();
    for later in (index + 1)..resolved.len() {
        for target in [&resolved[later], &resolved_move_destinations[later]]
            .into_iter()
            .flatten()
        {
            let current = offsets.current_path(&target.path)?;
            if lists.insert((current.section, list_prefix(&current).to_vec())) {
                let shape = path_shape(doc, &current);
                shapes.push(LaterAddressShape {
                    later,
                    original: target.path.clone(),
                    current,
                    shape,
                });
            }
        }
    }
    Ok(shapes)
}

/// The list a path belongs to: section plus every index except the paragraph's own trailing
/// one. Two paths with the same identity name the same containing list.
fn list_prefix(path: &hwp_convert::SegmentPath) -> &[usize] {
    let n = path.indices.len();
    &path.indices[..n.saturating_sub(1)]
}

/// The insertion index `hwp_convert::move_paragraph` expects for `to_ref`'s reference paragraph
/// and `before`/`after` positioning, given that the SAME call is about to remove `from`.
/// `move_paragraph` interprets its `to_index` against the destination list state AFTER the
/// source's removal (structure.rs Task 1), so a same-list move whose source sits before the
/// reference must pre-compensate by one: the reference's own live position shifts down when the
/// paragraph ahead of it disappears.
fn move_to_index(
    from: &hwp_convert::SegmentPath,
    to_ref: &hwp_convert::SegmentPath,
    before: bool,
) -> usize {
    let same_list = from.section == to_ref.section && list_prefix(from) == list_prefix(to_ref);
    let src_idx = *from.indices.last().expect("non-empty path");
    let ref_idx = *to_ref.indices.last().expect("non-empty path");
    let adjusted_ref_idx = if same_list && src_idx < ref_idx {
        ref_idx - 1
    } else {
        ref_idx
    };
    if before {
        adjusted_ref_idx
    } else {
        adjusted_ref_idx + 1
    }
}

/// The `(ParaShapeId, StyleId, CharShapeId)` template an addressed `insert_para` inherits by
/// default when the op carries no `style`/`char` override — the anchor paragraph's OWN current
/// shape, mirroring what the legacy pattern-form `insert_paragraph` already inherits via
/// `structure::para_template`. `None` only if `path` no longer resolves (should not happen
/// post-preflight; the caller treats it as an internal error).
fn default_shape_at(
    doc: &mut hwp_model::Document,
    path: &hwp_convert::SegmentPath,
) -> Option<(
    hwp_model::ParaShapeId,
    hwp_model::StyleId,
    hwp_model::CharShapeId,
)> {
    let para = hwp_convert::address::paragraph_at_mut(doc, path)?;
    Some((
        para.para_shape,
        para.style,
        para.char_shape_runs
            .first()
            .map_or(hwp_model::CharShapeId(0), |run| run.1),
    ))
}

/// EDT-06/D-13: the `changed` id pairs for one successfully-applied addressed op, computed right
/// after it mutated `doc`. `target_before`/`to_target_before` are the op's preflight-resolved
/// target(s) (`None` for an op with no address, which reports no ids — there is nothing to
/// re-key). `target_current_path`/`to_target_current_path` are `IndexOffsets::current_path`
/// snapshots taken BEFORE this op's own call to `apply_typed_operation` — required, not merely
/// convenient: a structural op's own `record_insert`/`record_move` (called INSIDE that same
/// `apply_typed_operation` call) would otherwise have already self-shifted the very anchor/
/// reference path being resolved here, corrupting exactly the lookup this function needs.
/// `doc_before_typed_ops` is the document state addresses were resolved against (D-06), needed
/// to re-derive the pre-batch run-id list for the run-split cascade (Pitfall 2).
///
/// Structural ops (`insert_para`/`delete_para`/`move_para`) are handled directly here, using the
/// SAME private helpers (`move_to_index`, `moved_paragraph_path`) their own
/// `apply_typed_operation` arms use; the ids are the ones right after this op. Every
/// non-structural addressed kind uses
/// `target_current_path` directly (already correct for any EARLIER structural op in the batch,
/// since none of these kinds change list length themselves).
fn report_changed_ids(
    operation: &TypedEditOperation,
    target_before: Option<&hwp_convert::address::ResolvedTarget>,
    target_current_path: Option<&hwp_convert::SegmentPath>,
    to_target_current_path: Option<&hwp_convert::SegmentPath>,
    doc_before_typed_ops: Option<&mut hwp_model::Document>,
    doc: &mut hwp_model::Document,
) -> Vec<IdPair> {
    let Some(target) = target_before else {
        return Vec::new();
    };
    match operation {
        TypedEditOperation::DeletePara {
            address: Some(_), ..
        } => {
            // The removed paragraph no longer exists anywhere to re-derive an id from — D-13's
            // null-after case, reported directly rather than through a (necessarily failing)
            // lookup.
            vec![IdPair {
                before: Some(target.before_id.clone()),
                after: None,
            }]
        }
        TypedEditOperation::InsertPara {
            address: Some(_),
            before,
            ..
        } => {
            let Some(anchor_path) = target_current_path else {
                return Vec::new();
            };
            let Some(&anchor_idx) = anchor_path.indices.last() else {
                return Vec::new();
            };
            let new_idx = if *before { anchor_idx } else { anchor_idx + 1 };
            let mut new_indices = anchor_path.indices.clone();
            *new_indices.last_mut().expect("non-empty path") = new_idx;
            let new_path = hwp_convert::SegmentPath {
                section: anchor_path.section,
                indices: new_indices,
            };
            let Some(paragraph) = hwp_convert::address::paragraph_at_mut(doc, &new_path) else {
                return Vec::new();
            };
            // D-13's null-before case: the created paragraph did not exist before the batch.
            vec![IdPair {
                before: None,
                after: Some(hwp_convert::paragraph_id(&new_path, paragraph)),
            }]
        }
        TypedEditOperation::MoveParagraph { before, .. } => {
            let (Some(from_current), Some(to_ref_current)) =
                (target_current_path, to_target_current_path)
            else {
                return Vec::new();
            };
            let to_index = move_to_index(from_current, to_ref_current, *before);
            let moved_path = moved_paragraph_path(from_current, to_ref_current, to_index);
            let Some(paragraph) = hwp_convert::address::paragraph_at_mut(doc, &moved_path) else {
                return Vec::new();
            };
            // The paragraph itself is spliced, not re-created (structure::move_paragraph never
            // clones), so its instance_id and content are unchanged — only its path, and
            // therefore its id STRING, moves.
            vec![IdPair {
                before: Some(target.before_id.clone()),
                after: Some(hwp_convert::paragraph_id(&moved_path, paragraph)),
            }]
        }
        _ => {
            // Every other addressed kind (SetFormat, SetPara, SetAlign, Replace(addressed),
            // IndentPara, OutdentPara): none of these change list length, so the paragraph's
            // OWN position is unaffected by THIS op — `target_current_path` already reflects
            // drift from any earlier structural op in the batch, which is exactly correct here.
            let Some(current_path) = target_current_path else {
                return Vec::new();
            };
            let Some(paragraph) = hwp_convert::address::paragraph_at_mut(doc, current_path) else {
                return Vec::new();
            };
            let after_para_id = hwp_convert::paragraph_id(current_path, paragraph);
            let mut changed = Vec::new();
            if after_para_id != target.before_id {
                changed.push(IdPair {
                    before: Some(target.before_id.clone()),
                    after: Some(after_para_id),
                });
            }
            // Pitfall 2 / must_haves.truths: a run-range op cascades id changes to every LATER
            // run in the paragraph, not only the one it targeted, because a run split shifts
            // every later run's canonical index. Zipped by position, null-padded on whichever
            // side is shorter (a split GROWS the run count; nothing here ever shrinks it).
            if let hwp_convert::address::TargetKind::Run { .. } = target.kind {
                let before_runs: Vec<String> = doc_before_typed_ops
                    .and_then(|before_doc| {
                        hwp_convert::address::paragraph_at_mut(before_doc, &target.path)
                    })
                    .map(|p| {
                        hwp_convert::canonical_char_shape_runs(p)
                            .iter()
                            .enumerate()
                            .map(|(i, _)| hwp_convert::run_id(&target.path, p, i))
                            .collect()
                    })
                    .unwrap_or_default();
                let after_runs: Vec<String> = hwp_convert::canonical_char_shape_runs(paragraph)
                    .iter()
                    .enumerate()
                    .map(|(i, _)| hwp_convert::run_id(current_path, paragraph, i))
                    .collect();
                for i in 0..before_runs.len().max(after_runs.len()) {
                    let b = before_runs.get(i).cloned();
                    let a = after_runs.get(i).cloned();
                    if b != a {
                        changed.push(IdPair {
                            before: b,
                            after: a,
                        });
                    }
                }
            }
            changed
        }
    }
}

/// A pair the widened preflight rejects (07-02 length-change clause, T-07-26; 07-03 removal
/// clause, T-07-12): `earlier_index` either changes `paragraph`'s length or removes it outright,
/// and `later_index` targets `paragraph` (or, for the removal clause, a path beneath it) in a
/// way whose fixed offsets or existence assumption D-06 froze against the PRE-BATCH document and
/// that no longer holds once `earlier_index` applies.
struct LengthChangeConflict {
    earlier_index: usize,
    later_index: usize,
    paragraph: hwp_convert::SegmentPath,
    kind: ConflictKind,
}

/// Which of [`detect_conflicts`]'s two clauses produced a [`LengthChangeConflict`] — reported
/// separately only so `execute()`'s single shared error block can phrase each line accurately;
/// both kinds still share the one `anyhow::bail!` call site (planner decision 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConflictKind {
    LengthChange,
    Removal,
}

/// Widens the preflight to reject an incoherent batch. **Two clauses, one function, one error
/// path** (planner decision 2) — do not split this into a second conflict mechanism:
///
/// 1. **Length-change clause (07-02, T-07-26):** a length-changing op on a paragraph (an
///    addressed `replace`, or a pattern-form `replace` whose `from` string occurs in that
///    paragraph in the ORIGINAL document — the pattern-form arm is a deliberate
///    over-approximation, since a batch could in principle be harmless if an even earlier op
///    already removed the match) paired with a LATER run-range op inside that same paragraph.
/// 2. **Removal clause (07-03, T-07-12):** an op that REMOVES its target (`delete_para`, or
///    `move_para`'s source) paired with a LATER op whose resolved target is the SAME path or a
///    path BENEATH it (nested inside the removed paragraph, e.g. a table cell it contained).
///
/// Both share the SAME always-abort behavior (D-07): a batch tripping either clause is rejected
/// during preflight, 0 ops applied, no output file, `--allow-partial` ignored. Two
/// NON-destructive ops on one address that cannot change its length (e.g. `set_align` then
/// `set_para`) are NOT a conflict under either clause — they compose in array order, Phase 6
/// D-01's existing semantics; do not widen either clause to cover that case.
fn detect_conflicts(
    plan: &EditPlan,
    doc: &hwp_model::Document,
    resolved: &[Option<hwp_convert::address::ResolvedTarget>],
    resolved_move_destinations: &[Option<hwp_convert::address::ResolvedTarget>],
) -> Vec<LengthChangeConflict> {
    let mut conflicts = Vec::new();
    for (earlier_index, operation) in plan.typed_operations.iter().enumerate() {
        let TypedEditOperation::Replace { from, to, address } = operation else {
            continue;
        };
        if from.is_empty() || from == to {
            // A no-op replace never reaches hwp_convert::replace_text/replace_text_at (see the
            // apply arm) — it cannot change any paragraph's length.
            continue;
        }
        let length_changing_paths: Vec<hwp_convert::SegmentPath> = if address.is_some() {
            resolved
                .get(earlier_index)
                .and_then(|target| target.as_ref())
                .map(|target| vec![target.path.clone()])
                .unwrap_or_default()
        } else {
            paragraph_paths_containing(doc, from)
        };
        if length_changing_paths.is_empty() {
            continue;
        }
        for (later_index, later_operation) in plan
            .typed_operations
            .iter()
            .enumerate()
            .skip(earlier_index + 1)
        {
            let TypedEditOperation::SetFormat {
                address: Some(_), ..
            } = later_operation
            else {
                continue;
            };
            let Some(target) = resolved.get(later_index).and_then(|t| t.as_ref()) else {
                continue;
            };
            if length_changing_paths.contains(&target.path) {
                conflicts.push(LengthChangeConflict {
                    earlier_index,
                    later_index,
                    paragraph: target.path.clone(),
                    kind: ConflictKind::LengthChange,
                });
            }
        }
    }

    // Removal clause (07-03, T-07-12, planner decision 2): an op that removes its target
    // (`delete_para`'s address, or `move_para`'s source) paired with a LATER op whose resolved
    // target is that same path or a path beneath it. A later `move_para` counts on EITHER its
    // source or its destination reference — either one landing on removed content is the same
    // composition bug.
    for (earlier_index, operation) in plan.typed_operations.iter().enumerate() {
        let removes = matches!(
            operation,
            TypedEditOperation::DeletePara {
                address: Some(_),
                ..
            } | TypedEditOperation::MoveParagraph { .. }
        );
        if !removes {
            continue;
        }
        let Some(removed_path) = resolved.get(earlier_index).and_then(|t| t.as_ref()) else {
            continue;
        };
        for later_index in (earlier_index + 1)..plan.typed_operations.len() {
            let later_targets = [
                resolved.get(later_index).and_then(|t| t.as_ref()),
                resolved_move_destinations
                    .get(later_index)
                    .and_then(|t| t.as_ref()),
            ];
            for target in later_targets.into_iter().flatten() {
                if path_is_same_or_beneath(&removed_path.path, &target.path) {
                    conflicts.push(LengthChangeConflict {
                        earlier_index,
                        later_index,
                        paragraph: removed_path.path.clone(),
                        kind: ConflictKind::Removal,
                    });
                }
            }
        }
    }

    conflicts
}

/// `other` is `removed`'s own path, or nested beneath it (removed's index chain is a strict or
/// non-strict prefix of `other`'s, in the same section) — the removal clause's "same path or a
/// path beneath it" test.
fn path_is_same_or_beneath(
    removed: &hwp_convert::SegmentPath,
    other: &hwp_convert::SegmentPath,
) -> bool {
    removed.section == other.section
        && removed.indices.len() <= other.indices.len()
        && other.indices[..removed.indices.len()] == removed.indices[..]
}

/// Every paragraph path (mirroring `address::resolve`'s path convention), anywhere in `doc`
/// (body, table cells, generic-control paragraph lists), whose OWN text — never a descendant
/// paragraph's, which lives in a separate `Paragraph` value under `para.controls` — contains
/// `pattern`. Read-only counterpart to `hwp_convert::replace_text`'s recursion, used only by
/// `detect_conflicts`'s pattern-form over-approximation arm.
fn paragraph_paths_containing(
    doc: &hwp_model::Document,
    pattern: &str,
) -> Vec<hwp_convert::SegmentPath> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for (section_index, section) in doc.sections.iter().enumerate() {
        for (para_index, para) in section.paragraphs.iter().enumerate() {
            collect_paragraph_paths_containing(
                para,
                pattern,
                hwp_convert::SegmentPath {
                    section: section_index,
                    indices: vec![para_index],
                },
                &mut paths,
            );
        }
    }
    paths
}

fn collect_paragraph_paths_containing(
    para: &hwp_model::Paragraph,
    pattern: &str,
    path: hwp_convert::SegmentPath,
    out: &mut Vec<hwp_convert::SegmentPath>,
) {
    if paragraph_own_text_contains(para, pattern) {
        out.push(path.clone());
    }
    for (ctrl_index, ctrl) in para.controls.iter().enumerate() {
        match ctrl {
            hwp_model::Control::Table(table) => {
                for (cell_index, cell) in table.cells.iter().enumerate() {
                    for (p_index, p) in cell.paragraphs.iter().enumerate() {
                        let mut child = path.clone();
                        child.indices.extend([ctrl_index, cell_index, p_index]);
                        collect_paragraph_paths_containing(p, pattern, child, out);
                    }
                }
            }
            hwp_model::Control::Generic(generic) if generic.raw_children.is_empty() => {
                let mut seq = 0usize;
                for list in &generic.paragraph_lists {
                    for p in &list.paragraphs {
                        let mut child = path.clone();
                        child.indices.extend([ctrl_index, seq]);
                        collect_paragraph_paths_containing(p, pattern, child, out);
                        seq += 1;
                    }
                }
            }
            _ => {}
        }
    }
}

/// A conservative, own-text-only substring check for one paragraph (never recurses into nested
/// controls): concatenates this paragraph's own `Text` characters and looks for `pattern` inside
/// them. Coarser than `hwp_convert`'s internal `find_match` (may match across what that treats as
/// a control-character boundary) but never leaks a descendant paragraph's text into its
/// ancestor's check — the over-approximation planner decision 2 accepts is deliberately
/// per-paragraph, not per-subtree.
fn paragraph_own_text_contains(para: &hwp_model::Paragraph, pattern: &str) -> bool {
    let mut text = String::new();
    for ch in &para.chars {
        if let hwp_model::HwpChar::Text(c) = ch {
            text.push(*c);
        }
    }
    text.contains(pattern)
}

/// Op-index-and-address prefix for a preflight failure message (D-04): reconstructs the id/at
/// form the caller wrote, so an error names the op index, the address and the reason without
/// echoing any document text (T-07-04).
fn describe_address(address: &hwp_convert::address::Address) -> String {
    let path = std::iter::once(address.section.to_string())
        .chain(address.indices.iter().map(ToString::to_string))
        .collect::<Vec<_>>()
        .join(".");
    match &address.checksum {
        Some(checksum) => format!("id {checksum}.{path}"),
        None => format!("at {path}"),
    }
}

struct PatchReport {
    counts: BTreeMap<String, usize>,
    applied_requests: usize,
    warnings: Vec<String>,
    ops: Vec<OpOutcome>,
}

/// The unapplied-request label a pattern-form typed `replace` records, for the stderr summary
/// and the abort message. The report's `reason` comes from [`failure_reason`] instead.
fn replace_request(from: &str, to: &str) -> String {
    format!("replace from={from:?} to={to:?}")
}

/// #348: a failed op's `edit-report-v1` `reason`: the op kind and a fixed cause, never the
/// request's patterns, anchors, names, urls or text, so the report stays content-free
/// (T-07-22). The fast path and the apply loop both call this, so they report the same string
/// (#332). `unapplied` keeps the full request for the operator-facing stderr and abort messages.
fn failure_reason(operation: &TypedEditOperation) -> String {
    use TypedEditOperation as Op;
    let cause = match operation {
        Op::Replace { from, to, .. } if from.is_empty() || from == to => {
            "empty pattern or identical replacement"
        }
        Op::Replace {
            address: Some(_), ..
        } => "no match at the address",
        Op::Replace { .. }
        | Op::DeletePara { .. }
        | Op::DeleteImage { .. }
        | Op::DeleteTable { .. }
        | Op::DeleteField { .. }
        | Op::DeleteBookmark { .. } => "no match",
        Op::CreateField { .. }
        | Op::CreateBookmark { .. }
        | Op::CreateHyperlink { .. }
        | Op::InsertPara { .. } => "anchor not found",
        Op::SetAlign {
            address: Some(_), ..
        } => "paragraph not found at the address",
        Op::SetPara {
            address: Some(_), ..
        } => "paragraph not found at the address, or no change",
        Op::SetField { .. } | Op::SetFormat { .. } | Op::SetAlign { .. } | Op::SetPara { .. } => {
            "no match or no change"
        }
        Op::StyleTables { .. } => "no styleable table",
        Op::SetTablePlacement { .. } => "table not found",
        Op::SetCell { .. }
        | Op::SetCellByLabel { .. }
        | Op::SetMeta { .. }
        | Op::SetCellPara { .. }
        | Op::SetPage { .. } => "no change",
        // These fail with an error rather than an unapplied request; listed so a new op kind
        // has to pick its cause here.
        Op::InsertImage { .. }
        | Op::Seal { .. }
        | Op::MoveParagraph { .. }
        | Op::IndentPara { .. }
        | Op::OutdentPara { .. }
        | Op::AddRow { .. }
        | Op::AddCol { .. }
        | Op::DeleteRow { .. }
        | Op::DeleteCol { .. }
        | Op::MergeCells { .. }
        | Op::SplitCell { .. }
        | Op::AddTable { .. }
        | Op::CloneTable { .. } => "not applied",
    };
    format!("{}: {cause}", typed_op_kind(operation))
}

/// #332: the fast path's `edit-report-v1` ops, one per typed replace, equal to what the apply
/// loop reports for a pattern-form replace: no resolved address, so `changed` is empty and an
/// applied op touches `changed.len().max(1)` = 1 piece. The per-entry match counts are not
/// used, since the apply loop does not report them either. Empty for the legacy `--replace`
/// flags, which carry no per-op outcomes on either path (`typed_operations` is empty there).
fn fast_path_outcomes(plan: &EditPlan, matched: &[bool]) -> Vec<OpOutcome> {
    plan.typed_operations
        .iter()
        .zip(matched)
        .enumerate()
        .map(|(index, (operation, &matched))| OpOutcome {
            index,
            op: "replace".to_string(),
            status: if matched {
                OpStatus::Applied
            } else {
                OpStatus::Failed
            },
            pieces_touched: usize::from(matched),
            changed: Vec::new(),
            reason: (!matched).then(|| failure_reason(operation)),
        })
        .collect()
}

fn patch_replacements_staged(
    input: &Path,
    staged: &Path,
    output: &Path,
    pairs: &[(String, String)],
    plan: &EditPlan,
) -> anyhow::Result<PatchReport> {
    let parent = staged.parent().context("임시 출력 작업공간이 없습니다")?;
    let mut current = input.to_path_buf();
    let mut current_is_temporary = false;
    let mut applied_requests = 0usize;
    let mut totals = BTreeMap::new();
    let mut warnings = Vec::new();
    // One flag per pair. Without --allow-partial the first unapplied request still decides the
    // abort message, but the loop runs every pair so the abort report has every op's status,
    // as the apply loop's EditAbort report does.
    let mut matched = Vec::with_capacity(pairs.len());
    let mut first_unapplied: Option<String> = None;

    for (index, (from, to)) in pairs.iter().enumerate() {
        if from.is_empty() || from == to {
            matched.push(false);
            if plan.allow_partial {
                warnings.push(format!("미적용 편집 요청: --replace {from:?}=>{to:?}"));
            } else {
                first_unapplied.get_or_insert_with(|| {
                    format!(
                        "적용되지 않은 편집 요청이 있습니다: --replace {from:?}=>{to:?} \
                         (--allow-partial로 일치한 요청만 적용 가능)"
                    )
                });
            }
            continue;
        }
        let next = parent.join(format!(".hwp-replace-step-{index}.hwpx"));
        let counts = hwpx::patch::replace_texts(&current, &next, &[(from.clone(), to.clone())])?;
        let matches = counts
            .iter()
            .filter(|(entry, _)| entry.starts_with("Contents/section") && entry.ends_with(".xml"))
            .map(|(_, count)| *count)
            .sum::<usize>();
        matched.push(matches > 0);
        if matches == 0 {
            let _ = fs::remove_file(&next);
            if plan.allow_partial {
                warnings.push(format!("미적용 편집 요청: --replace {from:?}=>{to:?}"));
            } else {
                first_unapplied.get_or_insert_with(|| {
                    format!(
                        "적용되지 않은 편집 요청이 있습니다: --replace {from:?}=>{to:?} \
                         (런 분절 교차 매칭은 미지원, --allow-partial로 일치한 요청만 적용 가능)"
                    )
                });
            }
            continue;
        }
        for (entry, count) in counts {
            *totals.entry(entry).or_insert(0) += count;
        }
        if current_is_temporary {
            let _ = fs::remove_file(&current);
        }
        current = next;
        current_is_temporary = true;
        applied_requests += 1;
    }

    let ops = fast_path_outcomes(plan, &matched);
    // EDT-06/D-14 on the fast path: every abort below carries the report, so `--report` (CLI)
    // and `report` (MCP) still get a diagnosable artifact, as on the apply-loop path.
    let abort = |reason: String| -> anyhow::Error {
        EditAbort {
            report: EditReport {
                output: output.display().to_string(),
                applied: applied_requests,
                warnings: Vec::new(),
                preservation: hwp_model::PreservationReport::new(),
                ops: ops.clone(),
                dry_run: plan.dry_run,
            },
            reason,
        }
        .into()
    };
    if let Some(reason) = first_unapplied {
        return Err(abort(reason));
    }
    if applied_requests == 0 {
        return Err(abort(
            "적용 가능한 편집이 없어 출력을 게시하지 않습니다".to_string(),
        ));
    }
    let original = load_document(input)?;
    let final_doc = load_document(&current)?;
    if semantic_signature(&original) == semantic_signature(&final_doc) {
        return Err(abort(
            "순차 치환의 최종 결과가 원문과 같아 출력을 게시하지 않습니다 \
             (상쇄되는 --replace 요청 확인)"
                .to_string(),
        ));
    }
    fs::rename(&current, staged).with_context(|| {
        format!(
            "순차 치환 결과를 최종 임시 파일로 옮기지 못했습니다: {}",
            staged.display()
        )
    })?;
    Ok(PatchReport {
        counts: totals,
        applied_requests,
        warnings,
        ops,
    })
}

fn record_effect(
    before: &hwp_model::Document,
    after: &hwp_model::Document,
    request: String,
    edits: &mut usize,
    unapplied: &mut Vec<String>,
) {
    if before == after {
        unapplied.push(request);
    } else {
        *edits += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_typed_operation(
    operation: &TypedEditOperation,
    doc: &mut hwp_model::Document,
    edits: &mut usize,
    unapplied: &mut Vec<String>,
    resolved_label_edits: &mut dyn Iterator<Item = Option<ResolvedLabelEdit>>,
    resolved_addresses: &mut dyn Iterator<Item = Option<hwp_convert::address::ResolvedTarget>>,
    resolved_move_destinations: &mut dyn Iterator<
        Item = Option<hwp_convert::address::ResolvedTarget>,
    >,
    offsets: &mut IndexOffsets,
) -> anyhow::Result<()> {
    // One slot per `plan.typed_operations` entry (`preflight_addressed_ops`), consumed in
    // lockstep with this loop regardless of operation kind so the position alignment holds.
    let resolved_target = resolved_addresses
        .next()
        .expect("resolved_addresses is aligned 1:1 with typed_operations");
    // Same lockstep contract, for move_para's destination reference (preflight_move_destinations).
    let resolved_move_destination = resolved_move_destinations
        .next()
        .expect("resolved_move_destinations is aligned 1:1 with typed_operations");
    match operation {
        TypedEditOperation::Replace { from, to, address } => {
            if from.is_empty() || from == to {
                unapplied.push(replace_request(from, to));
                return Ok(());
            }
            if address.is_some() {
                let target = resolved_target
                    .expect("preflight_addressed_ops resolved every addressed replace op");
                // Structural drift (planner decision 3): an earlier insert/delete/move in this
                // batch may have shifted this paragraph's position since preflight resolved it.
                let current_path = offsets.current_path(&target.path)?;
                let count = hwp_convert::replace_text_at(doc, &current_path, from, to);
                if count == 0 {
                    unapplied.push(format!(
                        "replace address={current_path} from={from:?} to={to:?}"
                    ));
                } else {
                    eprintln!("치환(주소): {current_path} {from:?} → {to:?} ({count}건)");
                    *edits += 1;
                }
            } else {
                let before = doc.clone();
                let count = hwp_convert::replace_text(doc, from, to, true);
                eprintln!("치환: {from:?} → {to:?} ({count}건)");
                record_effect(&before, doc, replace_request(from, to), edits, unapplied);
            }
        }
        TypedEditOperation::SetCell {
            table,
            row,
            col,
            text,
        } => {
            let before = doc.clone();
            hwp_convert::set_cell(doc, *table, *row, *col, text)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("셀 설정: 표{table} ({row},{col}) = {text:?}");
            record_effect(
                &before,
                doc,
                format!("set_cell table={table} row={row} col={col}"),
                edits,
                unapplied,
            );
        }
        TypedEditOperation::SetCellByLabel { .. } => {
            let resolved = resolved_label_edits
                .next()
                .expect("label edits are resolved during preflight");
            let Some(resolved) = resolved else {
                return Ok(());
            };
            let before = doc.clone();
            hwp_convert::set_cell(
                doc,
                resolved.candidate.table,
                resolved.candidate.row,
                resolved.candidate.col,
                &resolved.text,
            )
            .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!(
                "양식 셀 설정: 표{} ({},{})",
                resolved.candidate.table, resolved.candidate.row, resolved.candidate.col
            );
            record_effect(&before, doc, resolved.request, edits, unapplied);
        }
        TypedEditOperation::CreateField {
            anchor,
            name,
            value,
        } => {
            if hwp_convert::create_field(doc, anchor, name, value) {
                eprintln!("누름틀 생성: {anchor:?} 뒤에 이름={name:?} 값={value:?}");
                *edits += 1;
            } else {
                unapplied.push(format!("create_field anchor={anchor:?} name={name:?}"));
            }
        }
        TypedEditOperation::CreateBookmark { anchor, name } => {
            if hwp_convert::create_bookmark(doc, anchor, name) {
                eprintln!("책갈피 생성: {anchor:?} 뒤에 이름={name:?}");
                *edits += 1;
            } else {
                unapplied.push(format!("create_bookmark anchor={anchor:?} name={name:?}"));
            }
        }
        TypedEditOperation::CreateHyperlink {
            anchor,
            display,
            url,
        } => {
            if hwp_convert::create_hyperlink(doc, anchor, url, display) {
                eprintln!("하이퍼링크 생성: {anchor:?} 뒤에 표시={display:?} URL={url:?}");
                *edits += 1;
            } else {
                unapplied.push(format!("create_hyperlink anchor={anchor:?} url={url:?}"));
            }
        }
        TypedEditOperation::InsertImage {
            anchor,
            path,
            size_mm,
        } => {
            let size = size_mm
                .map(|(width, height)| ImageSize::Mm(width, height))
                .unwrap_or(ImageSize::Natural);
            hwp_convert::insert_image(doc, anchor, path, size)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("이미지 삽입: {anchor:?} 뒤에 {}", path.display());
            *edits += 1;
        }
        TypedEditOperation::Seal {
            anchor,
            path,
            size_mm,
        } => {
            let measure = seal_measurer(doc);
            hwp_convert::insert_seal(doc, anchor, path, *size_mm, measure)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("도장 날인: {anchor:?} 위에 {}", path.display());
            *edits += 1;
        }
        TypedEditOperation::SetField { name, value } => {
            let before = doc.clone();
            let count = hwp_convert::set_field(doc, name, value);
            if count == 0 {
                unapplied.push(format!("set_field name={name:?}"));
            } else {
                eprintln!("필드 설정: {name:?} = {value:?} ({count}건)");
                record_effect(
                    &before,
                    doc,
                    format!("set_field name={name:?}"),
                    edits,
                    unapplied,
                );
            }
        }
        TypedEditOperation::SetMeta { key, value } => {
            let before = doc.clone();
            let value = (!value.is_empty()).then(|| value.clone());
            match key.trim() {
                "title" => doc.metadata.title = value,
                "author" => doc.metadata.author = value,
                "subject" => doc.metadata.subject = value,
                "keywords" => doc.metadata.keywords = value,
                other => {
                    anyhow::bail!("메타데이터 키는 title|author|subject|keywords 입니다: {other:?}")
                }
            }
            record_effect(
                &before,
                doc,
                format!("set_meta key={key:?}"),
                edits,
                unapplied,
            );
        }
        TypedEditOperation::SetFormat {
            pattern,
            format,
            address,
        } => {
            if address.is_some() {
                let target = resolved_target
                    .expect("preflight_addressed_ops resolved every addressed set_format op");
                let hwp_convert::address::TargetKind::Run { w_start, w_end, .. } = target.kind
                else {
                    anyhow::bail!("set_format 주소는 run 단위여야 합니다");
                };
                // Structural drift (planner decision 3): the paragraph's own list position may
                // have shifted; w_start/w_end stay valid, they are content offsets WITHIN it.
                let current_path = offsets.current_path(&target.path)?;
                hwp_convert::restyle_range_at(doc, &current_path, w_start, w_end, format)
                    .map_err(|error| anyhow::anyhow!(error))?;
                eprintln!("글자 서식(주소): {current_path} [{w_start}, {w_end})");
                *edits += 1;
            } else {
                let before = doc.clone();
                let count = hwp_convert::set_char_format(doc, pattern, format);
                if count == 0 {
                    unapplied.push(format!("set_format pattern={pattern:?}"));
                } else {
                    eprintln!("글자 서식: {pattern:?} ({count}건)");
                    record_effect(
                        &before,
                        doc,
                        format!("set_format pattern={pattern:?}"),
                        edits,
                        unapplied,
                    );
                }
            }
        }
        TypedEditOperation::SetAlign {
            pattern,
            align,
            address,
        } => {
            if address.is_some() {
                let target = resolved_target
                    .expect("preflight_addressed_ops resolved every addressed set_align op");
                let current_path = offsets.current_path(&target.path)?;
                if hwp_convert::set_para_align_at(doc, &current_path, *align) {
                    eprintln!("문단 정렬(주소): {current_path} = {align}");
                    *edits += 1;
                } else {
                    unapplied.push(format!("set_align address={current_path} align={align}"));
                }
            } else {
                let before = doc.clone();
                let count = hwp_convert::set_para_align(doc, pattern, *align);
                if count == 0 {
                    unapplied.push(format!("set_align pattern={pattern:?}"));
                } else {
                    eprintln!("문단 정렬: {pattern:?} = {align} ({count}건)");
                    record_effect(
                        &before,
                        doc,
                        format!("set_align pattern={pattern:?}"),
                        edits,
                        unapplied,
                    );
                }
            }
        }
        TypedEditOperation::InsertPara {
            anchor,
            text,
            before,
            address,
            style,
            char,
        } => {
            if address.is_some() {
                let target = resolved_target
                    .expect("preflight_addressed_ops resolved every addressed insert_para op");
                let current_path = offsets.current_path(&target.path)?;
                let base_shape = default_shape_at(doc, &current_path).ok_or_else(|| {
                    anyhow::anyhow!("내부 오류: 삽입 앵커 문단을 찾을 수 없습니다: {current_path}")
                })?;
                hwp_convert::insert_paragraph_at(doc, &current_path, *before, text, base_shape)
                    .map_err(|error| anyhow::anyhow!(error))?;
                let anchor_idx = *current_path.indices.last().expect("non-empty path");
                let new_idx = if *before { anchor_idx } else { anchor_idx + 1 };
                let mut new_indices = current_path.indices.clone();
                *new_indices.last_mut().expect("non-empty path") = new_idx;
                let new_path = hwp_convert::SegmentPath {
                    section: current_path.section,
                    indices: new_indices,
                };
                if let Some(style) = style {
                    hwp_convert::apply_para_props_at(doc, &new_path, style);
                }
                if let Some(char_fmt) = char {
                    let wlen = hwp_convert::address::paragraph_at_mut(doc, &new_path)
                        .map(|p| p.wchar_len())
                        .unwrap_or(0);
                    if wlen > 0 {
                        hwp_convert::restyle_range_at(doc, &new_path, 0, wlen, char_fmt)
                            .map_err(|error| anyhow::anyhow!(error))?;
                    }
                }
                eprintln!("문단 삽입(주소): {current_path} before={before} text={text:?}");
                offsets.record_insert(&new_path);
                *edits += 1;
            } else if hwp_convert::insert_paragraph(doc, anchor, text, *before) {
                eprintln!("문단 삽입: {anchor:?}, before={before}, text={text:?}");
                *edits += 1;
            } else {
                eprintln!("{}", paragraph_miss_message(doc, anchor, "앵커"));
                unapplied.push(format!("insert_para anchor={anchor:?}"));
            }
        }
        TypedEditOperation::DeletePara { matching, address } => {
            if address.is_some() {
                let target = resolved_target
                    .expect("preflight_addressed_ops resolved every addressed delete_para op");
                let current_path = offsets.current_path(&target.path)?;
                hwp_convert::delete_paragraph_at(doc, &current_path)
                    .map_err(|error| anyhow::anyhow!(error))?;
                eprintln!("문단 삭제(주소): {current_path}");
                offsets.record_delete(&current_path);
                *edits += 1;
            } else {
                let count = hwp_convert::delete_paragraph(doc, matching);
                if count == 0 {
                    eprintln!(
                        "{}",
                        paragraph_miss_message(doc, matching, "삭제 대상 문단")
                    );
                    unapplied.push(format!("delete_para matching={matching:?}"));
                } else {
                    eprintln!("문단 삭제: {matching:?} ({count}건)");
                    *edits += count;
                }
            }
        }
        TypedEditOperation::MoveParagraph { before, .. } => {
            let target = resolved_target
                .expect("preflight_addressed_ops resolved every move_para source address");
            let to_target = resolved_move_destination
                .expect("preflight_move_destinations resolved every move_para destination");
            let from_current = offsets.current_path(&target.path)?;
            let to_ref_current = offsets.current_path(&to_target.path)?;
            let to_index = move_to_index(&from_current, &to_ref_current, *before);
            let to_list = hwp_convert::SegmentPath {
                section: to_ref_current.section,
                indices: to_ref_current.indices.clone(),
            };
            hwp_convert::move_paragraph(doc, &from_current, &to_list, to_index)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("문단 이동(주소): {from_current} → {to_list} index={to_index}");
            offsets.record_move(&from_current, &to_list, to_index);
            *edits += 1;
        }
        TypedEditOperation::IndentPara { .. } => {
            let target =
                resolved_target.expect("preflight_addressed_ops resolved every indent_para op");
            let current_path = offsets.current_path(&target.path)?;
            let new_level = hwp_convert::shift_head_level_at(doc, &current_path, 1)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("문단 들여쓰기(주소): {current_path} → 수준 {new_level}");
            *edits += 1;
        }
        TypedEditOperation::OutdentPara { .. } => {
            let target =
                resolved_target.expect("preflight_addressed_ops resolved every outdent_para op");
            let current_path = offsets.current_path(&target.path)?;
            let new_level = hwp_convert::shift_head_level_at(doc, &current_path, -1)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("문단 내어쓰기(주소): {current_path} → 수준 {new_level}");
            *edits += 1;
        }
        TypedEditOperation::AddRow {
            table,
            at,
            count,
            template_row,
        } => {
            hwp_convert::add_rows_at(doc, *table, *at, *count, *template_row)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!(
                "표 행 추가: 표{table} 위치={} 개수={count} 템플릿={}",
                fmt_at(*at),
                fmt_at(*template_row)
            );
            *edits += 1;
        }
        TypedEditOperation::AddCol { table, at, count } => {
            hwp_convert::add_table_columns(doc, *table, *at, *count)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!(
                "표 열 추가: 표{table} 위치={} 개수={count} (전체 폭 유지)",
                fmt_at(*at)
            );
            *edits += 1;
        }
        TypedEditOperation::DeleteRow { table, row } => {
            hwp_convert::delete_table_row(doc, *table, *row)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("표 행 삭제: 표{table} 행{row}");
            *edits += 1;
        }
        TypedEditOperation::DeleteCol { table, col } => {
            hwp_convert::delete_table_column(doc, *table, *col)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("표 열 삭제: 표{table} 열{col}");
            *edits += 1;
        }
        TypedEditOperation::MergeCells {
            table,
            r1,
            c1,
            r2,
            c2,
        } => {
            hwp_convert::merge_cells(doc, *table, *r1, *c1, *r2, *c2)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("셀 병합: 표{table} ({r1},{c1})-({r2},{c2})");
            *edits += 1;
        }
        TypedEditOperation::SplitCell { table, row, col } => {
            hwp_convert::split_cell(doc, *table, *row, *col)
                .map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("셀 분할: 표{table} ({row},{col})");
            *edits += 1;
        }
        TypedEditOperation::AddTable { anchor, rows } => {
            hwp_convert::add_table(doc, anchor, rows).map_err(|error| anyhow::anyhow!(error))?;
            eprintln!(
                "표 삽입: {anchor:?} 뒤 ({}x{})",
                rows.len(),
                rows.first().map_or(0, Vec::len)
            );
            *edits += 1;
        }
        TypedEditOperation::CloneTable {
            source_table,
            anchor,
            text_mode,
        } => {
            hwp_convert::clone_table(doc, *source_table, anchor, *text_mode)
                .map_err(|error| anyhow::anyhow!(error))?;
            let mode = match text_mode {
                hwp_convert::CloneTextMode::Blank => "blank",
                hwp_convert::CloneTextMode::Keep => "keep",
            };
            eprintln!("표 복제: 표{source_table} → {anchor:?} 뒤 ({mode})");
            *edits += 1;
        }
        TypedEditOperation::SetPara {
            pattern,
            props,
            address,
        } => {
            if address.is_some() {
                let target = resolved_target
                    .expect("preflight_addressed_ops resolved every addressed set_para op");
                let current_path = offsets.current_path(&target.path)?;
                if hwp_convert::apply_para_props_at(doc, &current_path, props) {
                    eprintln!("문단 모양(주소): {current_path}");
                    *edits += 1;
                } else {
                    unapplied.push(format!("set_para address={current_path}"));
                }
            } else {
                let before = doc.clone();
                let count = hwp_convert::set_para_props(doc, pattern, props);
                if count == 0 {
                    unapplied.push(format!("set_para pattern={pattern:?}"));
                } else {
                    eprintln!("문단 모양: {pattern:?} ({count}건)");
                    record_effect(
                        &before,
                        doc,
                        format!("set_para pattern={pattern:?}"),
                        edits,
                        unapplied,
                    );
                }
            }
        }
        TypedEditOperation::SetCellPara {
            table,
            row,
            col,
            props,
        } => {
            let before = doc.clone();
            let count = hwp_convert::set_cell_para_props(doc, *table, *row, *col, props)
                .map_err(|error| anyhow::anyhow!(error))?;
            if count == 0 {
                unapplied.push(format!("set_cell_para table={table} row={row} col={col}"));
            } else {
                eprintln!("셀 문단 모양: 표{table} ({row},{col}) ({count}건)");
                record_effect(
                    &before,
                    doc,
                    format!("set_cell_para table={table} row={row} col={col}"),
                    edits,
                    unapplied,
                );
            }
        }
        TypedEditOperation::SetPage { props } => {
            let before = doc.clone();
            let count = hwp_convert::set_page_def(doc, props);
            if count == 0 {
                unapplied.push("set_page".to_string());
            } else {
                eprintln!("페이지 설정: {count}구역");
                record_effect(&before, doc, "set_page".to_string(), edits, unapplied);
            }
        }
        TypedEditOperation::DeleteImage { anchor } => {
            let count = hwp_convert::delete_object(doc, hwp_convert::ObjectKind::Image, anchor);
            if count == 0 {
                unapplied.push(format!("delete_image anchor={anchor:?}"));
            } else {
                eprintln!("그림 삭제: {anchor:?} ({count}건)");
                *edits += count;
            }
        }
        TypedEditOperation::DeleteTable { index, anchor } => {
            // index/anchor mutual exclusion is enforced at the MCP boundary — here we only check defensively.
            let (kind, selector) = match (index, anchor) {
                (Some(nth), None) => (hwp_convert::ObjectKind::TableNth(*nth), ""),
                (None, Some(anchor)) => (hwp_convert::ObjectKind::Table, anchor.as_str()),
                _ => anyhow::bail!("delete_table은 index와 anchor 중 하나만 지정해야 합니다"),
            };
            let count = hwp_convert::delete_object(doc, kind, selector);
            if count == 0 {
                unapplied.push(format!("delete_table index={index:?} anchor={anchor:?}"));
            } else {
                eprintln!("표 삭제: index={index:?} anchor={anchor:?} ({count}건)");
                *edits += count;
            }
        }
        TypedEditOperation::DeleteField { name } => {
            let count = hwp_convert::delete_object(doc, hwp_convert::ObjectKind::Field, name);
            if count == 0 {
                unapplied.push(format!("delete_field name={name:?}"));
            } else {
                eprintln!("필드 삭제: {name:?} ({count}건)");
                *edits += count;
            }
        }
        TypedEditOperation::DeleteBookmark { name } => {
            let count = hwp_convert::delete_object(doc, hwp_convert::ObjectKind::Bookmark, name);
            if count == 0 {
                unapplied.push(format!("delete_bookmark name={name:?}"));
            } else {
                eprintln!("책갈피 삭제: {name:?} ({count}건)");
                *edits += count;
            }
        }
        TypedEditOperation::StyleTables { preset: _ } => {
            // See the CLI arm: an already-styled document is a no-op, not an unapplied request.
            let (eligible, changed) = hwp_convert::style_tables(doc);
            if eligible == 0 {
                unapplied.push("style_tables".to_string());
            } else if changed == 0 {
                eprintln!("표 스타일링: 이미 적용되어 있습니다({eligible}개 확인)");
            } else {
                eprintln!("표 스타일링: {changed}개");
                *edits += changed;
            }
        }
        TypedEditOperation::SetTablePlacement { placement, table } => {
            apply_table_placement_op(doc, *placement, *table, edits, unapplied);
        }
    }
    Ok(())
}

/// #296 표 배치 전환의 공용 적용 — CLI(`--table-placement`)와 typed(--ops/MCP) 경로가 같은
/// 계정 규칙을 쓴다: 대상 표가 없으면 미적용, 이미 요청 배치면 성공 no-op(재적용 바이트 안정).
fn apply_table_placement_op(
    doc: &mut hwp_model::Document,
    placement: hwp_convert::TablePlacement,
    table: Option<usize>,
    edits: &mut usize,
    unapplied: &mut Vec<String>,
) {
    let name = match placement {
        hwp_convert::TablePlacement::Inline => "inline",
        hwp_convert::TablePlacement::Floating => "floating",
    };
    match hwp_convert::set_table_placement(doc, table, placement) {
        None => {
            eprintln!("경고: 배치를 바꿀 표를 찾지 못했습니다 (table={table:?})");
            unapplied.push(format!(
                "set_table_placement placement={name} table={table:?}"
            ));
        }
        Some(0) => eprintln!("표 배치({name}): 이미 적용되어 있습니다"),
        Some(changed) => {
            eprintln!("표 배치({name}): {changed}개");
            *edits += changed;
        }
    }
}

fn write_output(
    doc: &hwp_model::Document,
    output: &Path,
    structural: bool,
    output_format: OutputFormat,
    source: Option<(&Path, &hwp_model::Document)>,
) -> anyhow::Result<hwp_model::WriteReport> {
    match output_format {
        OutputFormat::Hwp
            if source.is_some_and(|(_, original)| original.meta.source_format == "hwp5") =>
        {
            let (source, original) = source.expect("guarded source");
            crate::commands::convert::write_hwp_preserving_source(
                source,
                original,
                doc,
                output,
                !structural,
                structural,
            )
        }
        // 구조 편집은 삽입 문단/행에 불변식을 세우려 합성 경로를 강제한다.
        OutputFormat::Hwp if structural => {
            crate::commands::convert::write_hwp_structural(doc, output)
        }
        OutputFormat::Hwp => crate::commands::convert::write_hwp_edited(doc, output),
        OutputFormat::Hwpx => {
            // 같은 포맷(hwpx→hwpx)이면 패키지 외과 수술 경로: dirty 콘텐츠 엔트리만
            // IR에서 재직렬화하고 나머지 엔트리(BinData·META-INF·미리보기·DocOptions 등)는
            // 원본 패키지에서 raw 복사로 바이트 보존한다.
            if let Some((source_path, original)) = source
                && original.meta.source_format == "hwpx"
            {
                // dirty 판정은 편집 전후 IR 비교로 계산한다(op별 수동 매핑보다
                // 누락 위험이 없다). 섹션 지정은 앵커/패턴/전역 표 인덱스 기반 op의
                // 대상 섹션을 값싸게 증명할 수 없어 항상 전체 섹션을 dirty로 둔다.
                let dirty = hwpx::patch::DirtyEntries {
                    sections: None,
                    header: doc.header != original.header,
                    content_hpf: doc.metadata != original.metadata,
                };
                return Ok(hwpx::patch::rewrite_document_staged(
                    source_path,
                    output,
                    doc,
                    &dirty,
                    &hwpx::PackageLimits::default(),
                )?);
            }
            Ok(hwpx::write_document_with_report(doc, output)?)
        }
        OutputFormat::Json => {
            fs::write(output, hwp_convert::to_json(doc, true, true)?)?;
            Ok(hwp_model::WriteReport::new())
        }
        OutputFormat::Markdown => {
            fs::write(output, hwp_convert::to_markdown(doc))?;
            Ok(hwp_model::WriteReport::new())
        }
    }
}

/// "bold=on,size=16,color=#FF0000" → CharFormat.
fn parse_char_format(attrs: &str) -> anyhow::Result<CharFormat> {
    let mut fmt = CharFormat::default();
    for kv in attrs.split(',') {
        let kv = kv.trim();
        if kv.is_empty() {
            continue;
        }
        let (k, v) = kv.split_once('=').unwrap_or((kv, "on"));
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "bold" | "굵게" => fmt.bold = Some(parse_on(v)),
            "italic" | "기울임" => fmt.italic = Some(parse_on(v)),
            "underline" | "밑줄" => fmt.underline = Some(parse_on(v)),
            "strike" | "취소선" => fmt.strike = Some(parse_on(v)),
            "size" | "크기" => {
                fmt.size_pt = Some(v.parse().with_context(|| format!("size 값: {v:?}"))?);
            }
            "color" | "색" => {
                fmt.color = Some(parse_color(v).with_context(|| format!("color 값: {v:?}"))?);
            }
            "font" | "글꼴" => {
                if v.is_empty() {
                    anyhow::bail!("font 값이 비어 있습니다");
                }
                fmt.font = Some(v.to_string());
            }
            other => anyhow::bail!("알 수 없는 서식 속성: {other:?}"),
        }
    }
    Ok(fmt)
}

fn parse_on(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "on" | "true" | "1" | "yes" | "y"
    )
}

/// "#RRGGBB" 또는 색 이름 → COLORREF(0x00BBGGRR).
pub(crate) fn parse_color(s: &str) -> Option<u32> {
    let s = s.trim();
    let rgb = match s.to_ascii_lowercase().as_str() {
        "red" | "빨강" => (0xFF, 0x00, 0x00),
        "green" | "초록" => (0x00, 0x80, 0x00),
        "blue" | "파랑" => (0x00, 0x00, 0xFF),
        "black" | "검정" => (0x00, 0x00, 0x00),
        "white" | "흰색" => (0xFF, 0xFF, 0xFF),
        "yellow" | "노랑" => (0xFF, 0xFF, 0x00),
        _ => {
            let hex = s.strip_prefix('#').unwrap_or(s);
            if hex.len() != 6 {
                return None;
            }
            let v = u32::from_str_radix(hex, 16).ok()?;
            ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
        }
    };
    let (r, g, b) = rgb;
    Some((b << 16) | (g << 8) | r)
}

/// "경로" 또는 "경로@너비x높이"(mm) → (경로, ImageSize).
/// `@` 뒤가 "너비x높이"로 파싱될 때만 크기로 보고, 아니면 경로 일부(자연 크기)로 둔다.
fn parse_image_size(rhs: &str) -> anyhow::Result<(&str, ImageSize)> {
    if let Some((path, dims)) = rhs.rsplit_once('@')
        && let Some((w, h)) = dims.split_once(['x', 'X'])
        && let (Ok(w), Ok(h)) = (w.trim().parse::<f32>(), h.trim().parse::<f32>())
    {
        return Ok((path, ImageSize::Mm(w, h)));
    }
    Ok((rhs, ImageSize::Natural))
}

/// "경로" 또는 "경로@크기mm"(또는 "경로@크기") → (경로, Option<f32> mm).
/// `@` 뒤가 수치로 파싱될 때만 크기로 보고, 아니면 경로 일부(기본 20mm)로 둔다.
fn parse_seal_size(rhs: &str) -> (&str, Option<f32>) {
    if let Some((path, raw)) = rhs.rsplit_once('@') {
        let num = raw.trim().strip_suffix("mm").unwrap_or(raw.trim());
        if let Ok(mm) = num.trim().parse::<f32>() {
            return (path, Some(mm));
        }
    }
    (rhs, None)
}

/// Builds the anchor-measurement callback `insert_seal` calls on the paragraph that
/// receives the seal (D-06). `hwp-convert` does not depend on `hwp-render` (invariant 1),
/// so the shaping happens here and only the resulting numbers cross the boundary.
///
/// Host independence: the store sees system fonts plus the explicit font directory
/// (`HWP_FONT_DIR`, default `fonts/`), but a measurement is kept only when every face used
/// to shape the anchor line is the document's requested face. On any substitution,
/// coverage fallback or missing face the callback returns `None` and `insert_seal` uses
/// its constant fallback, so a host's substitute font never reaches the serialized seal
/// offset, while a machine that has the document's own fonts installed still gets the
/// measured placement (#250).
fn seal_measurer(
    doc: &hwp_model::Document,
) -> impl FnMut(&hwp_model::Paragraph, (u32, u32)) -> Option<hwp_convert::SealAnchorMetrics> + use<>
{
    let font_dir =
        std::path::PathBuf::from(std::env::var("HWP_FONT_DIR").unwrap_or_else(|_| "fonts".into()));
    seal_measurer_in(doc, hwp_render::FontStore::new(), &font_dir)
}

fn seal_measurer_in(
    doc: &hwp_model::Document,
    mut store: hwp_render::FontStore,
    font_dir: &std::path::Path,
) -> impl FnMut(&hwp_model::Paragraph, (u32, u32)) -> Option<hwp_convert::SealAnchorMetrics> + use<>
{
    // Shaping reads only the header (fonts, char shapes); a header-only document avoids
    // borrowing `doc` while `insert_seal` mutates it.
    let shaping_doc = hwp_model::Document {
        header: doc.header.clone(),
        ..Default::default()
    };
    store.load_dir(font_dir);
    move |para, range| measure_seal_anchor(&mut store, &shaping_doc, para, range)
}

/// Shapes the anchor's paragraph up to the anchor and the anchor itself with the
/// renderer's own entry point (`shape_range`: active char-shape runs, per-language
/// faces). Line height is the base size of the char shape active at the anchor start.
fn measure_seal_anchor(
    store: &mut hwp_render::FontStore,
    doc: &hwp_model::Document,
    para: &hwp_model::Paragraph,
    (start, end): (u32, u32),
) -> Option<hwp_convert::SealAnchorMetrics> {
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let before = seal_inline_width(hwp_render::shape::shape_range(
        store,
        doc,
        para,
        (0, start),
        &mut warnings,
    ))?;
    let width = seal_inline_width(hwp_render::shape::shape_range(
        store,
        doc,
        para,
        (start, end),
        &mut warnings,
    ))?;
    // Any shaping issue (e.g. a piece that failed to shape) means the widths undercount.
    if !warnings.finish().issues.is_empty() || !seal_faces_exact(store) {
        return None;
    }
    let shape_id = para
        .char_shape_runs
        .iter()
        .rev()
        .find(|(pos, _)| *pos <= start)
        .map(|(_, id)| *id)?;
    let base = doc.header.char_shapes.get(shape_id.0 as usize)?.base_size;
    // pt -> HWPUNIT: 1pt = 100 HWPUNIT.
    Some(hwp_convert::SealAnchorMetrics {
        anchor_start: (before * 100.0).round() as i32,
        anchor_width: (width * 100.0).round() as i32,
        line_height: if base > 0 { base } else { 1000 },
    })
}

/// Sum of run advances; `None` for a tab or line break, whose position needs full layout.
fn seal_inline_width(items: Vec<hwp_render::shape::InlineItem>) -> Option<f32> {
    items.iter().try_fold(0.0, |w, item| match item {
        hwp_render::shape::InlineItem::Run(run) => Some(w + run.width_pt),
        _ => None,
    })
}

/// True only when every face resolution so far matched the requested family exactly.
fn seal_faces_exact(store: &hwp_render::FontStore) -> bool {
    store.resolutions_complete
        && store
            .resolutions
            .iter()
            .all(|r| r.outcome == hwp_render::FontResolutionOutcome::Matched)
}

/// 정렬 이름 → 코드(0=양쪽,1=왼쪽,2=오른쪽,3=가운데,4=배분,5=나눔).
pub(crate) fn parse_align(name: &str) -> anyhow::Result<u8> {
    Ok(match name.trim().to_ascii_lowercase().as_str() {
        "left" | "왼쪽" => 1,
        "right" | "오른쪽" => 2,
        "center" | "가운데" => 3,
        "justify" | "both" | "양쪽" => 0,
        "distribute" | "배분" => 4,
        "divide" | "나눔" => 5,
        other => anyhow::bail!("알 수 없는 정렬: {other:?} (left/right/center/justify/distribute)"),
    })
}

/// mm → HWPUNIT (1mm = 7200/25.4). Callers with already-parsed numbers, like the MCP, use the same conversion.
pub(crate) fn mm_to_hwpunit(mm: f32) -> i32 {
    (mm * 7200.0 / 25.4).round() as i32
}

/// mm string → HWPUNIT (1mm = 7200/25.4).
fn parse_mm(value: &str) -> anyhow::Result<i32> {
    let mm: f32 = value
        .trim()
        .trim_end_matches("mm")
        .parse()
        .with_context(|| format!("mm 값이 숫자가 아닙니다: {value:?}"))?;
    Ok(mm_to_hwpunit(mm))
}

/// Parses a paragraph-shape property list, "key:value[,key:value]", into ParaProps.
/// Keys: line-spacing (ratio % or fixed Npt), indent, left, right, top, bottom (mm),
/// align (left/right/center/justify/distribute). Shared by `--set-para` and
/// `--set-cell-para`; `flag` names the caller in every error message.
///
/// A list with no recognised pairs yields empty props, which every entry point treats as a
/// no-op rather than as a rewrite with defaults.
pub(crate) fn parse_para_props(kv: &str, flag: &str) -> anyhow::Result<hwp_convert::ParaProps> {
    let mut props = hwp_convert::ParaProps::default();
    for pair in kv.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair
            .split_once(':')
            .with_context(|| format!("{flag} 형식은 \"키:값\" 입니다: {pair:?}"))?;
        apply_para_prop(&mut props, key.trim(), value.trim(), flag)?;
    }
    Ok(props)
}

/// Applies one "key:value" pair to `props`. `flag` names the caller in error messages.
fn apply_para_prop(
    props: &mut hwp_convert::ParaProps,
    key: &str,
    value: &str,
    flag: &str,
) -> anyhow::Result<()> {
    match key {
        "line-spacing" => {
            // One message for both branches: the caller cannot tell which one it missed.
            let bad = || {
                format!(
                    "{flag} 줄간격 값이 올바르지 않습니다: {value:?} (허용 형식: 150%, 150, 15pt)"
                )
            };
            if let Some(pt) = value.strip_suffix("pt") {
                let pt: f32 = pt.parse().with_context(bad)?;
                props.line_spacing = Some((1, (pt * 100.0).round() as i32));
            } else {
                // The help has always documented the ratio form as "%"; strip exactly one
                // trailing ASCII U+0025 and nothing else (no width folding, no arbitrary
                // trailing characters), so a full-width "％" still fails the integer parse.
                let pct: i32 = value
                    .strip_suffix('%')
                    .unwrap_or(value)
                    .parse()
                    .with_context(bad)?;
                props.line_spacing = Some((0, pct));
            }
        }
        "indent" => props.indent = Some(parse_mm(value)?),
        "left" => props.margin_left = Some(parse_mm(value)?),
        "right" => props.margin_right = Some(parse_mm(value)?),
        "top" => props.spacing_top = Some(parse_mm(value)?),
        "bottom" => props.spacing_bottom = Some(parse_mm(value)?),
        "align" => props.align = Some(parse_align(value)?),
        other => anyhow::bail!(
            "{flag} 알 수 없는 문단모양 키: {other:?} (line-spacing/indent/left/right/top/bottom/align)"
        ),
    }
    Ok(())
}

/// Explains a paragraph insert/delete that matched nothing writable. When the text does live
/// in the document but only inside an object the hwp5 writer re-emits from its original record
/// bytes (a text box, a header/footer), editing the IR there would be dropped on save, so the
/// walker never enters it - say that instead of the misleading "the anchor was not found".
fn paragraph_miss_message(doc: &hwp_model::Document, text: &str, subject: &str) -> String {
    if hwp_convert::text_in_unwritable_object(doc, text) {
        format!(
            "경고: {subject} {text:?}가 원본 레코드를 그대로 보존하는 개체(글상자·머리말 등) 안에만 있습니다 \
             — 그 안의 문단은 편집할 수 없어 적용하지 않았습니다"
        )
    } else {
        format!("경고: {subject} {text:?}를 찾지 못했습니다")
    }
}

/// Parses a `"table:row:col"` cell address (0-based). `flag` names the caller in errors.
pub(crate) fn parse_cell_loc(loc: &str, flag: &str) -> anyhow::Result<(usize, u16, u16)> {
    let parts: Vec<&str> = loc.split(':').collect();
    if parts.len() != 3 {
        anyhow::bail!("{flag} 위치는 \"표:행:열\" 형식입니다: {loc:?}");
    }
    let table: usize = parts[0].trim().parse().context("표 인덱스")?;
    let row: u16 = parts[1].trim().parse().context("행 번호")?;
    let col: u16 = parts[2].trim().parse().context("열 번호")?;
    Ok((table, row, col))
}

/// Applies one `--set-page` "key:value" to PageProps.
fn apply_page_prop(
    props: &mut hwp_convert::PageProps,
    key: &str,
    value: &str,
) -> anyhow::Result<()> {
    match key {
        "width" => props.width = Some(parse_mm(value)?),
        "height" => props.height = Some(parse_mm(value)?),
        "margin-left" => props.margin_left = Some(parse_mm(value)?),
        "margin-right" => props.margin_right = Some(parse_mm(value)?),
        "margin-top" => props.margin_top = Some(parse_mm(value)?),
        "margin-bottom" => props.margin_bottom = Some(parse_mm(value)?),
        "orientation" => {
            props.landscape = Some(match value.to_ascii_lowercase().as_str() {
                "landscape" | "가로" => true,
                "portrait" | "세로" => false,
                other => anyhow::bail!("알 수 없는 용지 방향: {other:?} (portrait/landscape)"),
            })
        }
        other => anyhow::bail!(
            "알 수 없는 페이지 키: {other:?} (width/height/margin-left/margin-right/margin-top/margin-bottom/orientation)"
        ),
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SemanticCounts {
    sections: usize,
    paragraphs: usize,
    tables: usize,
    pictures: usize,
    generic_controls: usize,
    fields: usize,
    bookmarks: usize,
    hyperlinks: usize,
    bin_streams: usize,
    char_shapes: usize,
    para_shapes: usize,
    styles: usize,
    text_chars: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct SemanticSignature {
    /// writer가 의도적으로 재계산하는 캐시/포맷 출처만 정규화한 전체 IR.
    /// PageDef/SectionDef, first·odd·even 머리말/꼬리말, 모든 header resource,
    /// section/paragraph/control opaque extras, BinData, settings/version pass-through를
    /// 포함하므로 부분 필드 목록이 새 모델 필드를 조용히 빠뜨리지 않는다.
    /// 전체 clone을 signature에 붙잡아 두거나 오류에 Debug 출력하지 않고, streaming
    /// JSON 직렬화를 SHA-256으로 요약한다. 따라서 BinData/opaque 본문도 비교하되
    /// mismatch 응답에는 원문 byte/string이 노출되지 않는다.
    canonical_sha256: [u8; 32],
    counts: SemanticCounts,
}

#[derive(Clone, Copy)]
enum SemanticTarget {
    Hwp,
    Hwpx,
}

fn canonical_document(
    doc: &hwp_model::Document,
    target: Option<SemanticTarget>,
) -> hwp_model::Document {
    fn binary_semantic_id(bytes: &[u8]) -> String {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let mut out = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// Collects the semantic ids of every stream referenced via resolve_bin by any Picture
    /// in the document (body, table cells, and text boxes, recursively).
    fn collect_referenced_bin_ids(
        paragraphs: &[hwp_model::Paragraph],
        doc: &hwp_model::Document,
        out: &mut Vec<String>,
    ) {
        for paragraph in paragraphs {
            for control in &paragraph.controls {
                match control {
                    hwp_model::Control::Picture(picture) => {
                        if let Some(bytes) = doc.resolve_bin(&picture.bin_ref) {
                            out.push(binary_semantic_id(bytes));
                        }
                    }
                    hwp_model::Control::Table(table) => {
                        for cell in &table.cells {
                            collect_referenced_bin_ids(&cell.paragraphs, doc, out);
                        }
                    }
                    hwp_model::Control::Generic(generic) => {
                        for list in &generic.paragraph_lists {
                            collect_referenced_bin_ids(&list.paragraphs, doc, out);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn is_default_column_def(column: &hwp_model::ColumnDef) -> bool {
        column.count == 1
            && column.kind == 0
            && column.direction == 0
            && column.same_width
            && column.gap == 0
            && column.widths.is_empty()
            && column.divider.is_none()
    }

    fn canonicalize_paragraph(
        paragraph: &mut hwp_model::Paragraph,
        target: Option<SemanticTarget>,
        source_doc: &hwp_model::Document,
    ) {
        // 줄 배치와 PARA_HEADER 캐시는 편집 후 writer가 재계산하는 비의미 상태다.
        paragraph.line_segs.clear();
        paragraph.header.chars_flags = 0;
        paragraph.header.ctrl_mask = 0;
        paragraph.header.instance_id = 0;
        paragraph.header.tail.clear();
        paragraph.header.hwp5_child_order.clear();
        // HWP5 writer는 모든 문단을 PARA_BREAK로 닫지만 HWPX는 문단 경계 자체가
        // 같은 의미를 표현하고 reader가 이 문자를 만들지 않는다. 포맷 간 검증에서
        // 이 writer 정규화만 의미 차이로 보지 않는다.
        if paragraph.chars.last()
            == Some(&hwp_model::HwpChar::CharCtrl(
                hwp_model::ctrl_char::PARA_BREAK,
            ))
        {
            paragraph.chars.pop();
        }
        if matches!(target, Some(SemanticTarget::Hwpx)) {
            // OWPML p는 pageBreak/columnBreak만 표현한다. hwp5 합성 첫 문단의
            // 구역/다단 표식 bits0-1은 secPr/colPr 자체가 같은 의미를 보존한다.
            paragraph.header.break_type &= 0x0c;

            // HWPX writer는 charPr run 경계를 실제 Text를 방출하는 시점에만 연다.
            // 필드/책갈피/그림 같은 폭 8의 제어문자 사이에 있는 경계는 다음 보이는
            // Text 위치로 이동하고, 뒤따르는 Text가 없으면 사라진다. 같은 투영을
            // 적용하되 Text에 실제 적용되는 shape ID 변화는 그대로 비교한다.
            let source_runs = paragraph.char_shape_runs.clone();
            let first_shape = source_runs.first().map(|(_, id)| *id).unwrap_or_default();
            let shape_at = |position: u32| {
                source_runs
                    .iter()
                    .rev()
                    .find(|(start, _)| *start <= position)
                    .map(|(_, id)| *id)
                    .unwrap_or_default()
            };
            let mut projected = vec![(0, first_shape)];
            let mut current = first_shape;
            let mut wchar_pos = 0_u32;
            for ch in &paragraph.chars {
                if matches!(ch, hwp_model::HwpChar::Text(_)) {
                    let shape = shape_at(wchar_pos);
                    if shape != current {
                        projected.push((wchar_pos, shape));
                        current = shape;
                    }
                }
                wchar_pos = wchar_pos.saturating_add(ch.wchar_width());
            }
            paragraph.char_shape_runs = projected;
        }
        for control in &mut paragraph.controls {
            match control {
                hwp_model::Control::Table(table) => {
                    if matches!(target, Some(SemanticTarget::Hwpx))
                        && table.common_data.is_empty()
                        && table.placement.is_none()
                    {
                        // The HWPX writer emits a synthetic table (empty common_data/placement)
                        // split per cell with header-row repeat (attr=6) and the inline default
                        // placement, and the reader returns it verbatim. Project the same values
                        // as write_table's fallback, with width/height from the writer's grid
                        // estimation (sum of max single-span cells).
                        table.attr = 6;
                        let cols = table.cols.max(1) as usize;
                        let rows = table.rows.max(1) as usize;
                        let mut col_w = vec![0_i64; cols];
                        let mut row_h = vec![0_i64; rows];
                        for cell in &table.cells {
                            let (col, row) = (cell.col as usize, cell.row as usize);
                            if cell.col_span == 1 && col < cols {
                                col_w[col] = col_w[col].max(i64::from(cell.width.0));
                            }
                            if cell.row_span == 1 && row < rows {
                                row_h[row] = row_h[row].max(i64::from(cell.height.0));
                            }
                        }
                        table.placement = Some(hwp_model::GsoPlacement {
                            treat_as_char: true,
                            flow_with_text: true,
                            vert_rel_to: 2, // PARA
                            horz_rel_to: 3, // PARA
                            width: col_w.iter().sum::<i64>() as i32,
                            height: row_h.iter().sum::<i64>() as i32,
                            out_margins: [283; 4],
                            ..Default::default()
                        });
                    }
                    for cell in &mut table.cells {
                        if matches!(target, Some(SemanticTarget::Hwp))
                            && cell.header_tail.is_empty()
                        {
                            // The hwp5 writer synthesizes width + 8 reserved zero bytes
                            // for an empty LIST_HEADER tail, and the reader returns them
                            // verbatim. Project the same bytes so newly created cells
                            // (add-col/split-cell/add-table) match their re-read form.
                            cell.header_tail =
                                hwp5::write::synthesized_cell_header_tail(cell.width.0);
                        }
                        for paragraph in &mut cell.paragraphs {
                            canonicalize_paragraph(paragraph, target, source_doc);
                        }
                    }
                }
                hwp_model::Control::Generic(generic) => {
                    if matches!(target, Some(SemanticTarget::Hwpx)) {
                        if (generic.ctrl_id == *b"head" || generic.ctrl_id == *b"foot")
                            && generic.data.len() == 8
                        {
                            // HWPX writer는 머리말/꼬리말 id를 문서 전역 순번으로
                            // 재부여한다. 적용쪽(data[0..4])과 본문은 의미지만 뒤의
                            // writer-generated id는 아니므로 정확한 8B 형식에서만 0으로
                            // 정규화한다.
                            generic.data[4..8].fill(0);
                        }
                        if generic.ctrl_id == *b"cold"
                            && generic
                                .column_def
                                .as_ref()
                                .is_some_and(is_default_column_def)
                        {
                            generic.column_def = None;
                        }
                        if hwp_convert::field::is_field_ctrl_id(&generic.ctrl_id) {
                            // command 없는 합성 필드의 11-byte HWP5 header 기본값은
                            // HWPX fieldBegin에 대응 필드가 없어 reader가 빈 Vec로 돌려준다.
                            if generic.data == vec![0_u8; 11] {
                                generic.data.clear();
                            }
                            // HWPX fieldBegin은 name=""도 명시하므로 reader가 빈 이름의
                            // CTRL_DATA를 합성한다. 정확한 합성 레코드만 제거하고 비어 있지
                            // 않은 이름 및 다른 opaque child는 그대로 검증한다.
                            let empty_name = hwp_convert::field::make_field_ctrl_data("");
                            if generic.raw_children.len() == 1 {
                                let child = &generic.raw_children[0];
                                if child.tag == 0x0057
                                    && child.data == empty_name
                                    && child.children.is_empty()
                                {
                                    generic.raw_children.clear();
                                }
                            }
                        }
                        let generated_equation =
                            generic.equation.as_ref().is_some_and(|equation| {
                                hwpx::write::section::is_materialized_generated_equation(
                                    generic, equation,
                                )
                            });
                        if generated_equation {
                            let equation = generic.equation.as_mut().expect("is_some predicate");
                            equation.raw_attrs = None;
                            equation.raw_props.clear();
                        }
                    } else if matches!(target, Some(SemanticTarget::Hwp))
                        && hwp5::write::is_materialized_default_column_def(generic)
                    {
                        generic.data.clear();
                    }
                    for list in &mut generic.paragraph_lists {
                        for paragraph in &mut list.paragraphs {
                            canonicalize_paragraph(paragraph, target, source_doc);
                        }
                    }
                }
                hwp_model::Control::SectionDef(def) => match target {
                    Some(SemanticTarget::Hwpx)
                        if hwpx::write::section::is_generated_default_secpr_children(
                            &def.secpr_raw_children,
                        ) =>
                    {
                        def.secpr_raw_children.clear();
                    }
                    Some(SemanticTarget::Hwp)
                        if hwp5::write::is_materialized_default_section_def(def) =>
                    {
                        def.data.clear();
                        def.extras.clear();
                        def.footnote_shape_raw = None;
                        def.endnote_shape_raw = None;
                        def.page_border_fills_raw.clear();
                    }
                    _ => {}
                },
                hwp_model::Control::Picture(picture) => {
                    if let Some(bytes) = source_doc.resolve_bin(&picture.bin_ref) {
                        let writer_generated = matches!(target, Some(SemanticTarget::Hwp))
                            && hwp5::write::is_materialized_generated_picture(picture, bytes);
                        picture.bin_ref = hwp_model::BinRef::ItemRef(binary_semantic_id(bytes));
                        if writer_generated {
                            // HWP writer materializes only container scaffolding here.
                            // Size, placement, z-order, object description, and media-bytes
                            // reference remain in the canonical Picture and are compared.
                            picture.common_data.clear();
                            picture.extras.clear();
                        }
                    }
                }
            }
        }
    }

    let mut canonical = doc.clone();
    canonical.meta = hwp_model::DocMeta::default();
    canonical.header.id_mappings_counts.clear();
    canonical.header.properties.section_count =
        u16::try_from(canonical.sections.len()).unwrap_or(u16::MAX);
    if matches!(target, Some(SemanticTarget::Hwp)) {
        // HWP5 writer가 합성 문서를 유효한 5.1.x 파일로 만들며 채우는 기본값을
        // writer와 동일한 projection으로 정규화한다. raw payload/tail은 정확한
        // 생성 바이트와 일치할 때만 제거하므로 사용자 opaque 데이터는 보존된다.
        for start in &mut canonical.header.properties.start_numbers {
            *start = (*start).max(1);
        }
        for language in &mut canonical.header.fonts {
            for font in language {
                if font.alt_name.is_some() {
                    font.attr |= 0x80;
                }
                if font.panose.is_some() {
                    font.attr |= 0x40;
                }
                if font.default_name.is_some() {
                    font.attr |= 0x20;
                }
            }
        }
        canonical
            .header
            .extras
            .retain(|record| !hwp5::write::is_materialized_compatible_document(record));
        // Embedded BIN_DATA rows are writer-assigned storage bookkeeping. Only
        // the exact modeled embedded form is projected away; linked, storage,
        // tailed, or otherwise custom entries stay active in the digest.
        canonical.header.bin_data.retain(|item| {
            !(item.attr == 1
                && item.link_abs.is_none()
                && item.link_rel.is_none()
                && item.storage_id.is_some()
                && item.extension.is_some()
                && item.tail.is_empty())
        });
        for stream in &mut canonical.bin_streams {
            stream.name = binary_semantic_id(&stream.data);
        }
        canonical.bin_streams.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.data.cmp(&right.data))
        });
        if hwp5::write::is_materialized_default_tab_defs(
            &canonical.header.tab_defs,
            &canonical.header.tab_stops,
        ) {
            canonical.header.tab_defs.clear();
            canonical.header.tab_stops.clear();
        }
        if hwp5::write::is_materialized_default_numberings(
            &canonical.header.numberings,
            &canonical.header.numbering_levels,
        ) {
            canonical.header.numberings.clear();
            canonical.header.numbering_levels.clear();
        }
        if hwp5::write::is_materialized_generated_bullets(
            &canonical.header.bullets,
            &canonical.header.bullet_chars,
        ) {
            canonical.header.bullets.clear();
        }
        for fill in &mut canonical.header.border_fills {
            if hwp5::write::is_materialized_generated_border_fill_tail(fill) {
                fill.tail.clear();
            }
            if fill.fill_type & 0x1 != 0 && fill.bg_color.is_none() {
                fill.bg_color = Some(0xFFFF_FFFF);
            }
        }
        for shape in &mut canonical.header.char_shapes {
            let generated_tail = shape.tail.is_empty()
                || hwp5::write::is_materialized_generated_char_shape_tail(shape);
            if generated_tail {
                shape.tail.clear();
                shape.border_fill_id = shape.border_fill_id.max(2);
            }
            if shape.strike {
                shape.attr |= 1 << 18;
            }
            shape.strike = false;
            if shape.underline_kind() == 0 {
                shape.underline_shape = 0;
            }
        }
        for shape in &mut canonical.header.para_shapes {
            // emit_para_shape folds line_spacing_type back into attr1 bits 0..1 and rewrites the
            // 5.0.2.5+ spacing window at tail[8..12]; project the same writes before classifying
            // the tail, or an edited spacing leaves the two sides holding different bytes.
            shape.attr1 = (shape.attr1 & !0x3) | (u32::from(shape.line_spacing_type) & 0x3);
            if shape.tail.len() >= 12 {
                shape.tail[8..12].copy_from_slice(&shape.line_spacing.to_le_bytes());
            }
            let generated_tail = shape.tail.is_empty()
                || hwp5::write::is_materialized_generated_para_shape_tail(shape);
            if generated_tail {
                shape.tail.clear();
                shape.line_spacing = if shape.line_spacing > 0 {
                    shape.line_spacing
                } else {
                    160
                };
            }
        }
        for style in &mut canonical.header.styles {
            if hwp5::write::is_materialized_generated_style_tail(style) {
                style.tail.clear();
            }
        }
    } else if matches!(target, Some(SemanticTarget::Hwpx)) {
        // HWPX header writer는 beginNum의 0 값을 1로 materialize한다. reader가
        // beginNum을 의미 파싱하므로 동일한 exact writer projection을 양쪽에 적용한다.
        for start in &mut canonical.header.properties.start_numbers {
            *start = (*start).max(1);
        }
        // The HWPX writer emits an explicit no-fill brush (`winBrush faceColor="none"`)
        // whenever fill bit 0 is set, and the reader returns the none-sentinel color for
        // it. An hwp5-sourced fill may carry the bit with no color at all; project the
        // same materialization as the writer so both sides agree.
        for fill in &mut canonical.header.border_fills {
            if fill.fill_type & 0x1 != 0 && fill.bg_color.is_none() {
                fill.bg_color = Some(0xFFFF_FFFF);
            }
        }
        // The HWPX writer bundles only streams referenced by a Picture (BinCollector).
        // Unreferenced streams (leftovers of deleted objects, etc.) do not exist on disk,
        // so both sides exclude them by the same rule — writer loss of referenced streams
        // is still detected.
        let mut referenced_bins = Vec::new();
        for section in &canonical.sections {
            collect_referenced_bin_ids(&section.paragraphs, &canonical, &mut referenced_bins);
        }
        for stream in &mut canonical.bin_streams {
            stream.name = binary_semantic_id(&stream.data);
        }
        canonical
            .bin_streams
            .retain(|stream| referenced_bins.contains(&stream.name));
        canonical.bin_streams.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.data.cmp(&right.data))
        });
        // HWPX writer는 같은 bytes를 한 package item으로 재사용한다. 이름/등장
        // Name/appearance order is not meaningful, so bytes are compared after sorting and deduplication.
        canonical
            .bin_streams
            .dedup_by(|left, right| left.data == right.data);
        canonical
            .hwpx_settings_xml
            .get_or_insert_with(|| hwpx::DEFAULT_SETTINGS_XML.to_string());
        canonical
            .hwpx_version_xml
            .get_or_insert_with(|| hwpx::DEFAULT_VERSION_XML.to_string());
        // 매니페스트 (id, href)는 writer가 bin_streams에서 결정론적으로 재배정하는
        // 이름 메타다(편집으로 bin이 추가/삭제되면 원본 슬롯과 달라진다). 바이트
        // 정체성은 위의 content-hash bin_streams 비교가 담당하므로 digest에서는 제외.
        canonical.hwpx_bin_manifest.clear();
        canonical
            .metadata
            .author
            .get_or_insert_with(|| "hwp-cli".to_string());
        for language in &mut canonical.header.fonts {
            for font in language {
                // HWPX writer는 name과 OWPML typeInfo만 방출한다. 나머지는 HWP5
                // FACE_NAME 전용 payload라 HWPX 재읽기에서 존재하지 않는다.
                font.attr = 0;
                font.alt_kind = None;
                font.alt_name = None;
                font.panose = None;
                font.default_name = None;
                font.tail.clear();
            }
        }
        for shape in &mut canonical.header.char_shapes {
            // HWPX writer가 raw attr를 그대로 쓰지 않고 각 의미 태그로 재구성한다.
            let mut attr = 0_u32;
            attr |= u32::from(shape.is_italic());
            attr |= u32::from(shape.is_bold()) << 1;
            attr |= (u32::from(matches!(shape.underline_kind(), 1 | 3))
                * u32::from(shape.underline_kind()))
                << 2;
            attr |= u32::from(shape.has_outline()) << 8;
            attr |= u32::from(shape.has_shadow()) << 11;
            attr |= u32::from(shape.is_emboss()) << 13;
            attr |= u32::from(shape.is_engrave()) << 14;
            attr |= u32::from(shape.is_superscript()) << 15;
            attr |= u32::from(shape.is_subscript()) << 16;
            attr |= u32::from(shape.strike) << 18;
            // Preserve decoration bits overwritten by the new accessors: underline shape
            // (4..=7), emphasis (21..=24), and strike shape (26..=29).
            attr |= u32::from(shape.underline_shape_code()) << 4;
            attr |= u32::from(shape.emphasis_kind()) << 21;
            attr |= u32::from(shape.strike_shape_code()) << 26;
            attr |= ((shape.attr >> 25) & 1) << 25;
            attr |= ((shape.attr >> 30) & 1) << 30;
            shape.attr = attr;

            // 스키마 필수 기본을 materialize하는 항목. reader가 돌려주는 값으로
            // 맞추되 활성 underline/shadow의 실제 색·간격은 그대로 유지한다.
            if shape.border_fill_id == 0 {
                shape.border_fill_id = 2;
            }
            for ratio in &mut shape.ratios {
                *ratio = (*ratio).max(1);
            }
            for relative_size in &mut shape.rel_sizes {
                *relative_size = (*relative_size).max(1);
            }
            if shape.underline_shape == 0 {
                shape.underline_shape = 1;
            }
            if shape.underline_color == 0xFFFF_FFFF {
                shape.underline_color = 0;
            }
            if !shape.has_shadow() {
                shape.shadow_color = 0;
                shape.shadow_gap = (0, 0);
            }
        }
        let numbering_count = canonical
            .header
            .numbering_levels
            .len()
            .max(canonical.header.numberings.len())
            .max(1);
        // HWP5 reader가 raw와 modeled 수준을 병렬로 모두 채운 경우 raw는 중복
        // 표현이다. modeled 정의가 모자라 writer가 raw-only custom 내용을 기본값으로
        // 잃게 되는 경우에는 raw를 남겨 semantic mismatch가 드러나게 한다.
        if canonical.header.numbering_levels.len() >= canonical.header.numberings.len() {
            canonical.header.numberings.clear();
        }
        canonical
            .header
            .numbering_levels
            .resize_with(numbering_count, || {
                (1..=7)
                    .map(|level| hwp_model::NumLevel {
                        start: 1,
                        fmt: hwp_model::NumFmt::Digit,
                        template: format!("^{level}."),
                    })
                    .collect()
            });
        for levels in &mut canonical.header.numbering_levels {
            while levels.len() < 7 {
                let level = levels.len() + 1;
                levels.push(hwp_model::NumLevel {
                    start: 1,
                    fmt: hwp_model::NumFmt::Digit,
                    template: format!("^{level}."),
                });
            }
        }
        let tab_count = canonical
            .header
            .tab_stops
            .len()
            .max(canonical.header.tab_defs.len())
            .max(1);
        // numbering과 동일하게 모든 raw 탭에 modeled 짝이 있을 때만 중복 raw를
        // 제거한다. raw-only 사용자 탭은 HWPX writer 손실 검증 대상이다.
        if canonical.header.tab_stops.len() >= canonical.header.tab_defs.len() {
            canonical.header.tab_defs.clear();
        }
        canonical
            .header
            .tab_stops
            .resize_with(tab_count, hwp_model::TabDef::default);
        for style in &mut canonical.header.styles {
            style.attr = 0;
            style.tail.clear();
            if style.lang_id <= 0 {
                // writer는 0/음수 언어 ID를 한국어(1042)로 materialize한다.
                style.lang_id = 1042;
            }
        }
        for shape in &mut canonical.header.para_shapes {
            let alignment = (shape.attr1 >> 2) & 0x7;
            let heading_type = (shape.attr1 >> 23) & 0x3;
            let heading_level = if heading_type == 0 {
                0
            } else {
                ((shape.attr1 >> 25) & 0x7).clamp(1, 7)
            };
            shape.attr1 =
                (1 << 8) | (alignment << 2) | (heading_type << 23) | (heading_level << 25);
            shape.tab_def_id = (shape.tab_def_id as usize).min(tab_count - 1) as u16;
            if shape.border_fill_id == 0 {
                shape.border_fill_id = 2;
            }
            shape.border_offsets = [0; 4];
            shape.indent = shape.indent / 2 * 2;
            shape.margin_left = shape.margin_left / 2 * 2;
            shape.margin_right = shape.margin_right / 2 * 2;
            shape.spacing_top = shape.spacing_top / 2 * 2;
            shape.spacing_bottom = shape.spacing_bottom / 2 * 2;
            if shape.line_spacing > 0 {
                if shape.line_spacing_type != 0 {
                    shape.line_spacing = shape.line_spacing / 2 * 2;
                }
            } else {
                shape.line_spacing = 160;
            }
            if shape.line_spacing_type > 3 {
                shape.line_spacing_type = 0;
            }
        }
    }
    for section in &mut canonical.sections {
        for paragraph in &mut section.paragraphs {
            canonicalize_paragraph(paragraph, target, doc);
        }
    }
    canonical
}

fn semantic_signature(doc: &hwp_model::Document) -> SemanticSignature {
    semantic_signature_for(doc, None)
}

fn semantic_signature_for(
    doc: &hwp_model::Document,
    target: Option<SemanticTarget>,
) -> SemanticSignature {
    fn visit_paragraph(paragraph: &hwp_model::Paragraph, counts: &mut SemanticCounts) {
        counts.paragraphs += 1;
        for control in &paragraph.controls {
            match control {
                hwp_model::Control::Table(table) => {
                    counts.tables += 1;
                    for cell in &table.cells {
                        for paragraph in &cell.paragraphs {
                            visit_paragraph(paragraph, counts);
                        }
                    }
                }
                hwp_model::Control::Picture(_) => counts.pictures += 1,
                hwp_model::Control::Generic(generic) => {
                    counts.generic_controls += 1;
                    if hwp_convert::hyperlink_url(control).is_some() {
                        counts.hyperlinks += 1;
                    }
                    for list in &generic.paragraph_lists {
                        for paragraph in &list.paragraphs {
                            visit_paragraph(paragraph, counts);
                        }
                    }
                }
                hwp_model::Control::SectionDef(_) => {}
            }
        }
    }

    let canonical = canonical_document(doc, target);
    let mut counts = SemanticCounts {
        sections: canonical.sections.len(),
        bin_streams: canonical.bin_streams.len(),
        char_shapes: canonical.header.char_shapes.len(),
        para_shapes: canonical.header.para_shapes.len(),
        styles: canonical.header.styles.len(),
        text_chars: canonical.plain_text().chars().count(),
        ..SemanticCounts::default()
    };
    for section in &canonical.sections {
        for paragraph in &section.paragraphs {
            visit_paragraph(paragraph, &mut counts);
        }
    }
    counts.fields = hwp_convert::list_fields(&canonical).len();
    counts.bookmarks = hwp_convert::list_bookmarks(&canonical).len();

    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, &canonical)
        .expect("hwp-model Document의 JSON 직렬화는 실패하지 않음");
    // BinStream.data는 일반 JSON 출력 비대 방지를 위해 serde(skip)이다. 검증 digest에는
    // 반드시 포함해 이미지/첨부 byte 손실도 semantic mismatch로 잡는다.
    for stream in &canonical.bin_streams {
        writer.0.update((stream.name.len() as u64).to_le_bytes());
        writer.0.update(stream.name.as_bytes());
        writer.0.update((stream.data.len() as u64).to_le_bytes());
        writer.0.update(&stream.data);
    }
    SemanticSignature {
        canonical_sha256: writer.0.finalize().into(),
        counts,
    }
}

fn semantic_mismatch_summary(expected: &SemanticSignature, actual: &SemanticSignature) -> String {
    fn digest_hex(digest: &[u8; 32]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    let fields = [
        ("sections", expected.counts.sections, actual.counts.sections),
        (
            "paragraphs",
            expected.counts.paragraphs,
            actual.counts.paragraphs,
        ),
        ("tables", expected.counts.tables, actual.counts.tables),
        ("pictures", expected.counts.pictures, actual.counts.pictures),
        (
            "generic_controls",
            expected.counts.generic_controls,
            actual.counts.generic_controls,
        ),
        ("fields", expected.counts.fields, actual.counts.fields),
        (
            "bookmarks",
            expected.counts.bookmarks,
            actual.counts.bookmarks,
        ),
        (
            "hyperlinks",
            expected.counts.hyperlinks,
            actual.counts.hyperlinks,
        ),
        (
            "bin_streams",
            expected.counts.bin_streams,
            actual.counts.bin_streams,
        ),
        (
            "char_shapes",
            expected.counts.char_shapes,
            actual.counts.char_shapes,
        ),
        (
            "para_shapes",
            expected.counts.para_shapes,
            actual.counts.para_shapes,
        ),
        ("styles", expected.counts.styles, actual.counts.styles),
        (
            "text_chars",
            expected.counts.text_chars,
            actual.counts.text_chars,
        ),
    ];
    let differences = fields
        .into_iter()
        .filter(|(_, expected, actual)| expected != actual)
        .map(|(name, expected, actual)| format!("{name}={expected}->{actual}"))
        .collect::<Vec<_>>();
    let counts = if differences.is_empty() {
        "count 차이 없음".to_string()
    } else {
        differences.join(", ")
    };
    format!(
        "expected_sha256={}, actual_sha256={}, {counts}",
        digest_hex(&expected.canonical_sha256),
        digest_hex(&actual.canonical_sha256)
    )
}

/// 쓰기 후 재읽기로 자기 검증하고, 요청 결과의 핵심 의미 불변식도 대조한다.
fn verify_output(output: &Path, expected: Option<&hwp_model::Document>) -> anyhow::Result<()> {
    verify_output_with_success_log(output, expected, true)
}

fn verify_output_with_success_log(
    output: &Path,
    expected: Option<&hwp_model::Document>,
    print_success: bool,
) -> anyhow::Result<()> {
    let doc =
        load_document(output).with_context(|| format!("검증 재읽기 실패: {}", output.display()))?;
    if let Some(expected) = expected {
        let target = match OutputFormat::from_path(output)? {
            OutputFormat::Hwp => Some(SemanticTarget::Hwp),
            OutputFormat::Hwpx => Some(SemanticTarget::Hwpx),
            OutputFormat::Json | OutputFormat::Markdown => None,
        };
        let expected = semantic_signature_for(expected, target);
        let actual = semantic_signature_for(&doc, target);
        if actual != expected {
            anyhow::bail!(
                "재읽은 문서의 의미 불변식이 편집 결과와 다릅니다 ({})",
                semantic_mismatch_summary(&expected, &actual)
            );
        }
    }
    if print_success {
        let text_len = doc.plain_text().chars().count();
        let paras: usize = doc.sections.iter().map(|s| s.paragraphs.len()).sum();
        eprintln!("검증: 재읽기 OK ({paras}문단, 본문 {text_len}자)");
    }
    Ok(())
}

pub fn verify_document(output: &Path, expected: &hwp_model::Document) -> anyhow::Result<()> {
    verify_output(output, Some(expected))
}

pub(crate) fn verify_document_quiet(
    output: &Path,
    expected: &hwp_model::Document,
) -> anyhow::Result<()> {
    verify_output_with_success_log(output, Some(expected), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #224: the three spellings the help documents all parse, and the two ratio spellings
    /// produce the same tuple - only the `pt` suffix selects the fixed-spacing type.
    #[test]
    fn parse_para_props_accepts_percent_bare_and_pt_line_spacing() {
        let spacing = |kv: &str| parse_para_props(kv, "--set-para").unwrap().line_spacing;
        assert_eq!(spacing("line-spacing:150%"), Some((0, 150)));
        assert_eq!(spacing("line-spacing:150"), Some((0, 150)));
        assert_eq!(spacing("line-spacing:150%"), spacing("line-spacing: 150 "));
        // set_para_props doubles length kinds on the way into IR units, so 15000 becomes 30000.
        assert_eq!(spacing("line-spacing:150pt"), Some((1, 15000)));
    }

    /// #224: every refused spelling gets one error that names the flag, quotes the offending
    /// value and lists the accepted forms. Full-width digits and a full-width percent sign are
    /// refused rather than normalized - no NFKC, no width folding.
    #[test]
    fn parse_para_props_refuses_bad_line_spacing_naming_the_flag() {
        for kv in [
            "line-spacing:",
            "line-spacing:abc",
            "line-spacing:１５０％",
            "line-spacing:150 %",
            "line-spacing:150pt%",
        ] {
            let rendered = format!("{:#}", parse_para_props(kv, "--set-para").unwrap_err());
            assert!(rendered.contains("--set-para"), "{kv}: {rendered}");
            assert!(rendered.contains("150%"), "{kv}: {rendered}");
            assert!(rendered.contains("15pt"), "{kv}: {rendered}");
        }
    }

    /// #221: both paragraph-shape flags share one parser, so `align` and comma-separated
    /// key lists work for `--set-para` as well as for `--set-cell-para`.
    #[test]
    fn parse_para_props_accepts_align_and_comma_lists() {
        let props = parse_para_props(
            "line-spacing:150%,indent:-12mm,align:center",
            "--set-cell-para",
        )
        .unwrap();
        assert_eq!(props.line_spacing, Some((0, 150)));
        assert_eq!(props.indent, Some(mm_to_hwpunit(-12.0)));
        assert_eq!(props.align, Some(3));

        // justify=0 left=1 right=2 center=3 distribute=4 - the existing name map.
        for (name, code) in [
            ("justify", 0u8),
            ("left", 1),
            ("right", 2),
            ("center", 3),
            ("distribute", 4),
        ] {
            let props = parse_para_props(&format!("align:{name}"), "--set-para").unwrap();
            assert_eq!(props.align, Some(code), "{name}");
        }

        // A list with no pairs is empty props, which every entry point treats as a no-op.
        assert!(parse_para_props("", "--set-cell-para").unwrap().is_empty());

        // Errors name the caller's flag, not the other one.
        let error = format!(
            "{:#}",
            parse_para_props("align:sideways", "--set-cell-para").unwrap_err()
        );
        assert!(error.contains("알 수 없는 정렬"), "{error}");
        let error = format!(
            "{:#}",
            parse_para_props("nope:1", "--set-cell-para").unwrap_err()
        );
        assert!(error.contains("--set-cell-para"), "{error}");
    }

    /// #221: the cell address parser names the caller's flag and rejects a malformed address.
    #[test]
    fn parse_cell_loc_names_the_flag() {
        assert_eq!(parse_cell_loc("0:1:2", "--set-cell").unwrap(), (0, 1, 2));
        assert_eq!(
            parse_cell_loc(" 3 : 4 : 5 ", "--set-cell").unwrap(),
            (3, 4, 5)
        );
        for loc in ["0:1", "0:1:2:3", "a:1:2"] {
            let error = format!("{:#}", parse_cell_loc(loc, "--set-cell-para").unwrap_err());
            assert!(
                error.contains("--set-cell-para") || error.contains("인덱스"),
                "{loc}: {error}"
            );
        }
    }

    #[test]
    fn canonical_semantics_cover_page_sections_resources_and_pass_through() {
        let base = hwp_convert::from_markdown("본문");

        let mut changed = base.clone();
        let section_def = changed.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| &mut paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::SectionDef(section_def) => Some(section_def),
                _ => None,
            })
            .expect("markdown 문서는 secd를 가짐");
        section_def.page.as_mut().unwrap().margin_left.0 += 1;
        assert_ne!(semantic_signature(&base), semantic_signature(&changed));

        let mut changed = base.clone();
        changed
            .header
            .border_fills
            .push(hwp_model::BorderFill::default());
        assert_ne!(semantic_signature(&base), semantic_signature(&changed));

        let mut changed = base.clone();
        changed.sections.push(hwp_model::Section::default());
        assert_ne!(semantic_signature(&base), semantic_signature(&changed));

        let mut changed = base.clone();
        changed.hwpx_settings_xml = Some("<ha:configItemSet/>".to_string());
        assert_ne!(semantic_signature(&base), semantic_signature(&changed));

        let mut changed = base.clone();
        changed.sections[0].extras.push(hwp_model::OpaqueRecord {
            tag: 0x3ff,
            data: vec![1, 2, 3],
            children: Vec::new(),
        });
        assert_ne!(semantic_signature(&base), semantic_signature(&changed));
    }

    #[test]
    fn canonical_semantics_ignore_only_writer_recomputed_paragraph_caches() {
        let base = hwp_convert::from_markdown("본문");
        let mut changed = base.clone();
        let paragraph = &mut changed.sections[0].paragraphs[0];
        paragraph.line_segs.push(hwp_model::LineSeg {
            text_start: 0,
            v_pos: 1,
            line_height: 2,
            text_height: 3,
            baseline_gap: 4,
            line_spacing: 5,
            col_start: 6,
            seg_width: 7,
            flags: 8,
        });
        paragraph.header.instance_id = 1234;
        paragraph.header.ctrl_mask = 42;
        paragraph.header.chars_flags = 1;
        paragraph.header.tail = vec![9, 9];
        assert_eq!(semantic_signature(&base), semantic_signature(&changed));
    }

    #[test]
    fn hwp_canonical_semantics_project_writer_synthesized_cell_header_tail() {
        // New cells (add-col/split-cell/add-table) carry an empty LIST_HEADER tail;
        // the hwp5 writer synthesizes width + 8 zero bytes and the reader returns
        // them verbatim, so the canonicalizer projects the same bytes for Hwp.
        let base = hwp_convert::from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let signature = |doc| semantic_signature_for(doc, Some(SemanticTarget::Hwp));
        let with_table = |doc: &mut hwp_model::Document,
                          f: &mut dyn FnMut(&mut hwp_model::Table)| {
            for section in &mut doc.sections {
                for paragraph in &mut section.paragraphs {
                    for control in &mut paragraph.controls {
                        if let hwp_model::Control::Table(table) = control {
                            f(table);
                            return;
                        }
                    }
                }
            }
        };
        let mut projected = base.clone();
        with_table(&mut projected, &mut |table| {
            for cell in &mut table.cells {
                cell.header_tail = hwp5::write::synthesized_cell_header_tail(cell.width.0);
            }
        });
        assert_eq!(signature(&base), signature(&projected));
        // A genuinely different tail is document data and must stay compared.
        let mut real = base.clone();
        with_table(&mut real, &mut |table| {
            table.cells[0].header_tail = vec![0xaa; 12];
        });
        assert_ne!(signature(&base), signature(&real));
    }

    #[test]
    fn hwpx_canonical_semantics_keep_active_format_and_opaque_mutations() {
        let mut base = hwp_convert::from_markdown("본문");
        let shape = &mut base.header.char_shapes[0];
        shape.attr = (shape.attr & !(0x3 << 2)) | (1 << 2) | (1 << 11);
        shape.underline_color = 0x0011_2233;
        shape.shadow_color = 0x0044_5566;
        shape.shadow_gap = (2, 3);
        shape.border_fill_id = 3;

        let signature = |doc| semantic_signature_for(doc, Some(SemanticTarget::Hwpx));

        let mut changed = base.clone();
        changed.header.char_shapes[0].underline_color ^= 0x0000_00ff;
        assert_ne!(signature(&base), signature(&changed));

        let mut changed = base.clone();
        changed.header.char_shapes[0].shadow_color ^= 0x0000_ff00;
        assert_ne!(signature(&base), signature(&changed));

        let mut changed = base.clone();
        changed.header.char_shapes[0].shadow_gap.0 += 1;
        assert_ne!(signature(&base), signature(&changed));

        let mut changed = base.clone();
        changed.header.char_shapes[0].border_fill_id = 4;
        assert_ne!(signature(&base), signature(&changed));

        let mut changed = base.clone();
        let section_def = changed.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| &mut paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::SectionDef(section_def) => Some(section_def),
                _ => None,
            })
            .expect("markdown 문서는 secd를 가짐");
        section_def
            .secpr_raw_children
            .push("<hp:extension value=\"opaque\"/>".to_string());
        assert_ne!(signature(&base), signature(&changed));
    }

    #[test]
    fn hwpx_canonical_semantics_ignore_only_inactive_format_defaults() {
        let base = hwp_convert::from_markdown("본문");
        let mut materialized = base.clone();
        let shape = &mut materialized.header.char_shapes[0];
        assert!(!shape.has_shadow());
        shape.shadow_color = 0x0011_2233;
        shape.shadow_gap = (7, 9);
        if shape.underline_shape == 0 {
            shape.underline_shape = 1;
        }
        if shape.underline_color == 0xFFFF_FFFF {
            shape.underline_color = 0;
        }
        materialized.hwpx_settings_xml = Some(hwpx::DEFAULT_SETTINGS_XML.to_string());
        materialized.hwpx_version_xml = Some(hwpx::DEFAULT_VERSION_XML.to_string());
        materialized.metadata.author = Some("hwp-cli".to_string());

        assert_eq!(
            semantic_signature_for(&base, Some(SemanticTarget::Hwpx)),
            semantic_signature_for(&materialized, Some(SemanticTarget::Hwpx))
        );
    }

    #[test]
    fn hwpx_canonical_semantics_keep_unmodeled_raw_numbering_and_tabs() {
        let mut left = hwp_convert::from_markdown("본문");
        left.header.numberings.push(hwp_model::RawEntry {
            data: vec![1, 2, 3],
            children: Vec::new(),
        });
        let mut right = left.clone();
        right.header.numberings[0].data[0] ^= 0xff;
        assert_ne!(
            semantic_signature_for(&left, Some(SemanticTarget::Hwpx)),
            semantic_signature_for(&right, Some(SemanticTarget::Hwpx))
        );

        left.header.tab_stops.clear();
        left.header.tab_defs[0].data[0] = 7;
        let mut right = left.clone();
        right.header.tab_defs[0].data[0] = 8;
        assert_ne!(
            semantic_signature_for(&left, Some(SemanticTarget::Hwpx)),
            semantic_signature_for(&right, Some(SemanticTarget::Hwpx))
        );
    }

    #[test]
    fn hwpx_canonical_semantics_count_deduplicated_binary_content() {
        let mut duplicated = hwp_convert::from_markdown("본문");
        duplicated.bin_streams = vec![
            hwp_model::BinStream {
                name: "first.png".to_string(),
                data: vec![1, 2, 3],
            },
            hwp_model::BinStream {
                name: "second.png".to_string(),
                data: vec![1, 2, 3],
            },
        ];
        // The canonicalizer keeps only streams referenced by a control, like the HWPX
        // writer (BinCollector) — to exercise deduplication, both streams must be referenced
        // by a Picture. The writer reuses one entry for identical bytes, so both point at
        // the same entry.
        for _ in 0..2 {
            duplicated.sections[0].paragraphs[0]
                .controls
                .push(hwp_model::Control::Picture(hwp_model::Picture {
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
                    bin_ref: hwp_model::BinRef::ItemRef("first.png".to_string()),
                    extras: Vec::new(),
                }));
        }
        let mut single = duplicated.clone();
        single.bin_streams.pop();

        let duplicated = semantic_signature_for(&duplicated, Some(SemanticTarget::Hwpx));
        let single = semantic_signature_for(&single, Some(SemanticTarget::Hwpx));
        assert_eq!(duplicated, single);
        assert_eq!(duplicated.counts.bin_streams, 1);
    }

    #[test]
    fn hwpx_canonical_semantics_drop_unreferenced_binary_content() {
        // The HWPX writer does not bundle unreferenced streams (leftovers of deleted
        // objects, etc.), so the canonicalizer excludes them by the same rule — otherwise
        // the reread verification would expect streams that cannot exist on disk and always fail.
        let mut doc = hwp_convert::from_markdown("본문");
        doc.bin_streams.push(hwp_model::BinStream {
            name: "orphan.png".to_string(),
            data: vec![1, 2, 3],
        });
        let signature = semantic_signature_for(&doc, Some(SemanticTarget::Hwpx));
        assert_eq!(signature.counts.bin_streams, 0);
    }

    #[test]
    fn semantic_mismatch_diagnostic_is_bounded_and_does_not_expose_document_content() {
        let secret = "PRIVATE-CONTENT-THAT-MUST-NOT-LEAK";
        let expected = hwp_convert::from_markdown(secret);
        let mut actual = expected.clone();
        actual.sections.push(hwp_model::Section::default());
        actual.bin_streams.push(hwp_model::BinStream {
            name: "secret.bin".to_string(),
            data: secret.repeat(10_000).into_bytes(),
        });
        let summary =
            semantic_mismatch_summary(&semantic_signature(&expected), &semantic_signature(&actual));
        assert!(
            summary.len() < 512,
            "진단이 bounded여야 함: {}",
            summary.len()
        );
        assert!(!summary.contains(secret));
        assert!(summary.contains("expected_sha256="));
        assert!(summary.contains("sections=1->2"));
    }

    #[test]
    fn hwpx_generated_equation_raw_is_ignored_but_custom_raw_is_kept() {
        fn signature(doc: &hwp_model::Document) -> SemanticSignature {
            semantic_signature_for(doc, Some(SemanticTarget::Hwpx))
        }

        let input = r#"{
          "version":"1.0",
          "sections":[{"blocks":[{"type":"paragraph","runs":[
            {"type":"text","text":"수식 "},
            {"type":"equation","script":"a^2+b^2=c^2","width_mm":35,"height_mm":8}
          ]}]}]
        }"#;
        let spec = hwp_cli::document_spec::parse_spec(
            input,
            hwp_cli::document_spec::SpecInputFormat::Json,
        )
        .unwrap();
        let compiled = hwp_cli::document_spec::compile_spec(
            &spec,
            std::path::Path::new("."),
            std::path::Path::new("out.hwpx"),
            false,
            false,
            &[],
        )
        .unwrap();
        let output = std::env::temp_dir().join(format!(
            "hwp-cli-equation-canonical-{}.hwpx",
            std::process::id()
        ));
        hwpx::write_document(&compiled.document, &output).unwrap();
        let mut materialized = hwpx::read_document(&output).unwrap().document;
        let _ = std::fs::remove_file(output);

        assert_eq!(signature(&compiled.document), signature(&materialized));

        let equation = materialized.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| &mut paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::Generic(generic) => generic.equation.as_mut(),
                _ => None,
            })
            .expect("materialized equation");
        equation
            .raw_attrs
            .as_mut()
            .expect("writer-generated attributes")
            .push_str(r#" custom="1""#);
        assert_ne!(signature(&compiled.document), signature(&materialized));
    }

    #[test]
    fn hwpx_header_footer_id_and_begin_number_ignore_only_writer_projection() {
        fn signature(doc: &hwp_model::Document) -> SemanticSignature {
            semantic_signature_for(doc, Some(SemanticTarget::Hwpx))
        }

        let mut source = hwp_convert::from_markdown("본문");
        source.header.properties.start_numbers = [0; 6];
        source.sections[0].paragraphs[0]
            .controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: vec![1, 0, 0, 0, 0, 0, 0, 0],
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
        let mut materialized = source.clone();
        materialized.header.properties.start_numbers = [1; 6];
        let hwp_model::Control::Generic(header) = materialized.sections[0].paragraphs[0]
            .controls
            .last_mut()
            .unwrap()
        else {
            panic!("header control")
        };
        header.data[4..8].copy_from_slice(&42_u32.to_le_bytes());
        assert_eq!(signature(&source), signature(&materialized));

        let mut custom_apply_page = materialized.clone();
        let hwp_model::Control::Generic(header) = custom_apply_page.sections[0].paragraphs[0]
            .controls
            .last_mut()
            .unwrap()
        else {
            panic!("header control")
        };
        header.data[0] = 2;
        assert_ne!(signature(&source), signature(&custom_apply_page));

        let mut custom_start = materialized;
        custom_start.header.properties.start_numbers[0] = 2;
        assert_ne!(signature(&source), signature(&custom_start));
    }

    #[test]
    fn hwp_generated_section_and_column_defaults_ignore_only_exact_payloads() {
        fn signature(doc: &hwp_model::Document) -> SemanticSignature {
            semantic_signature_for(doc, Some(SemanticTarget::Hwp))
        }

        let spec = hwp_cli::document_spec::parse_spec(
            r#"{
              "version":"1.0",
              "sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"본문"}]}]}]
            }"#,
            hwp_cli::document_spec::SpecInputFormat::Json,
        )
        .unwrap();
        let source = hwp_cli::document_spec::compile_spec(
            &spec,
            std::path::Path::new("."),
            std::path::Path::new("out.hwp"),
            false,
            false,
            &[],
        )
        .unwrap()
        .document;
        let output =
            std::env::temp_dir().join(format!("hwp-cli-hwp-canonical-{}.hwp", std::process::id()));
        crate::commands::convert::write_hwp_structural(&source, &output).unwrap();
        let materialized = crate::commands::cat::load_document(&output).unwrap();
        let _ = std::fs::remove_file(output);

        assert_eq!(signature(&source), signature(&materialized));

        let mut custom_section = materialized.clone();
        let section = custom_section.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| &mut paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::SectionDef(section) => Some(section),
                _ => None,
            })
            .expect("section definition");
        section.data[0] ^= 1;
        assert_ne!(signature(&source), signature(&custom_section));

        let mut custom_column = materialized;
        let column = custom_column.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| &mut paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::Generic(generic) if generic.ctrl_id == *b"cold" => {
                    Some(generic)
                }
                _ => None,
            })
            .expect("column definition");
        column.data[0] ^= 1;
        assert_ne!(signature(&source), signature(&custom_column));
    }

    #[test]
    fn hwp_generated_picture_projection_keeps_active_semantics() {
        fn signature(doc: &hwp_model::Document) -> SemanticSignature {
            semantic_signature_for(doc, Some(SemanticTarget::Hwp))
        }

        let gif = vec![
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x02, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3b,
        ];
        let temp_dir = std::env::temp_dir().join(format!(
            "hwp-cli-hwp-picture-canonical-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let asset = temp_dir.join("asset.gif");
        std::fs::write(&asset, gif).unwrap();
        let spec = hwp_cli::document_spec::parse_spec(
            r#"{
              "version":"1.0",
              "sections":[{"blocks":[{
                "type":"image","path":"asset.gif","width_mm":20,"height_mm":10,
                "placement":"floating"
              }]}]
            }"#,
            hwp_cli::document_spec::SpecInputFormat::Json,
        )
        .unwrap();
        let mut source = hwp_cli::document_spec::compile_spec(
            &spec,
            &temp_dir,
            std::path::Path::new("out.hwp"),
            false,
            false,
            &[],
        )
        .unwrap()
        .document;
        let source_picture = source.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|paragraph| &mut paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .unwrap();
        source_picture.z_order = 17;
        source_picture.vert_offset = 123;
        source_picture.horz_offset = 456;
        source_picture.description = Some("제목😀\n\n대체 설명".to_string());

        let output = temp_dir.join(format!(
            "hwp-cli-hwp-picture-canonical-{}.hwp",
            std::process::id()
        ));
        crate::commands::convert::write_hwp_structural(&source, &output).unwrap();
        let materialized = crate::commands::cat::load_document(&output).unwrap();
        let _ = std::fs::remove_file(output);
        let _ = std::fs::remove_file(asset);
        let materialized_picture = materialized.sections[0]
            .paragraphs
            .iter()
            .flat_map(|paragraph| &paragraph.controls)
            .find_map(|control| match control {
                hwp_model::Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .unwrap();
        assert!(hwp5::write::is_materialized_generated_picture(
            materialized_picture,
            materialized
                .resolve_bin(&materialized_picture.bin_ref)
                .unwrap()
        ));
        assert_eq!(signature(&source), signature(&materialized));

        let mutate_picture =
            |doc: &mut hwp_model::Document, mutate: &dyn Fn(&mut hwp_model::Picture)| {
                let picture = doc.sections[0]
                    .paragraphs
                    .iter_mut()
                    .flat_map(|paragraph| &mut paragraph.controls)
                    .find_map(|control| match control {
                        hwp_model::Control::Picture(picture) => Some(picture),
                        _ => None,
                    })
                    .unwrap();
                mutate(picture);
            };
        for mutate in [
            (|picture: &mut hwp_model::Picture| picture.width.0 += 1)
                as fn(&mut hwp_model::Picture),
            |picture| picture.vert_offset += 1,
            |picture| picture.horz_offset += 1,
            |picture| picture.z_order += 1,
            |picture| picture.treat_as_char = !picture.treat_as_char,
            |picture| picture.description = Some("다른 설명".to_string()),
        ] {
            let mut changed = materialized.clone();
            mutate_picture(&mut changed, &mutate);
            assert_ne!(signature(&source), signature(&changed));
        }
        let mut changed_media = materialized.clone();
        changed_media.bin_streams[0].data[0] ^= 0xff;
        assert_ne!(signature(&source), signature(&changed_media));

        let mut changed_scaffolding = materialized;
        mutate_picture(&mut changed_scaffolding, &|picture| {
            picture.common_data[0] ^= 1
        });
        assert_ne!(signature(&source), signature(&changed_scaffolding));
    }

    /// #259 review: a seal is measured only when every face matched the requested family
    /// exactly. A substituted, coverage-substituted or missing face, or an incomplete
    /// resolution log, sends `insert_seal` to its constant fallback, so the serialized
    /// offset does not depend on the host's fonts.
    #[test]
    fn seal_measure_requires_exact_face_matches() {
        use hwp_render::{FontResolution, FontResolutionOutcome as O};
        let with = |outcomes: &[O], complete: bool| {
            let mut store = hwp_render::FontStore::new_isolated();
            store.resolutions = outcomes
                .iter()
                .map(|&outcome| FontResolution {
                    requested: "함초롬바탕".into(),
                    requested_bold: false,
                    resolved: None,
                    resolved_sha256: None,
                    resolved_face_index: None,
                    outcome,
                })
                .collect();
            store.resolutions_complete = complete;
            seal_faces_exact(&store)
        };
        assert!(with(&[O::Matched, O::Matched], true));
        assert!(!with(&[O::Matched, O::Substituted], true));
        assert!(!with(&[O::CoverageSubstituted], true));
        assert!(!with(&[O::Missing], true));
        assert!(!with(&[O::Matched], false));
    }

    /// #259 review: with no exact face available (an isolated store and a font directory
    /// that lacks the document's face, so the result is the same on every host) the
    /// measurer returns `None`, and the seal lands exactly where the constant fallback
    /// puts it.
    #[test]
    fn seal_measurer_falls_back_without_the_requested_face() {
        let dir = std::env::temp_dir().join(format!("hwp-seal-no-fonts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty_fonts = dir.join("fonts");
        std::fs::create_dir_all(&empty_fonts).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        let png_path = dir.join("s.png");
        std::fs::write(&png_path, &png).unwrap();

        let source = hwp_convert::from_markdown("결재란 (인) 끝");
        let mut measured = source.clone();
        let mut calls = 0;
        let mut measure =
            seal_measurer_in(&source, hwp_render::FontStore::new_isolated(), &empty_fonts);
        hwp_convert::insert_seal(&mut measured, "(인)", &png_path, None, |p, r| {
            calls += 1;
            let m = measure(p, r);
            assert!(m.is_none(), "no exact face -> no measured metrics");
            m
        })
        .unwrap();
        assert_eq!(calls, 1);

        let mut fallback = source.clone();
        hwp_convert::insert_seal(&mut fallback, "(인)", &png_path, None, |_, _| None).unwrap();
        assert_eq!(measured, fallback, "identical to the constant fallback");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With no exact face the measurer answers `None` and the seal takes the width-class
    /// estimate, whose offsets are pinned here: the same numbers on every host, and for
    /// this D1 text equal to the placement measured with locally held genuine fonts.
    #[test]
    fn seal_fallback_offsets_are_host_independent() {
        let dir =
            std::env::temp_dir().join(format!("hwp-seal-host-independent-{}", std::process::id()));
        let empty_fonts = dir.join("fonts");
        std::fs::create_dir_all(&empty_fonts).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(96u32.to_be_bytes());
        png.extend(96u32.to_be_bytes());
        png.extend([0u8; 8]);
        let png_path = dir.join("s.png");
        std::fs::write(&png_path, &png).unwrap();

        let mut doc = hwp_convert::from_markdown("결재란: (인)");
        let mut measure =
            seal_measurer_in(&doc, hwp_render::FontStore::new_isolated(), &empty_fonts);
        hwp_convert::insert_seal(&mut doc, "(인)", &png_path, Some(18.0), |p, r| {
            let m = measure(p, r);
            assert!(m.is_none(), "no exact face -> no measured metrics");
            m
        })
        .unwrap();
        let pic = doc.sections[0].paragraphs[0]
            .controls
            .iter()
            .find_map(|c| match c {
                hwp_model::Control::Picture(p) => Some(p),
                _ => None,
            })
            .unwrap();
        // anchor_start 3730 + anchor_width 1610 / 2 - seal 5102 / 2; (line 1000 - 5102) / 2.
        assert_eq!((pic.horz_offset, pic.vert_offset), (1984, -2051));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
