//! `hwp edit` — 기존 문서를 인메모리로 편집해 다시 쓴다.
//!
//! 원본을 IR로 읽어(이미지·opaque 보존) 텍스트 치환·표 셀 설정을 적용한 뒤
//! 출력 포맷으로 저장한다. hwp 출력은 합성 경로(`write_hwp_edited`)를 거쳐
//! 편집으로 낡은 줄 배치·문단 불변식을 다시 세운다.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use hwp_cli::cli::EditArgs;
use hwp_convert::{CharFormat, ImageSize};
use sha2::{Digest, Sha256};

use crate::commands::cat::load_document;

enum EditOperation {
    Replace(Vec<String>),
    SetCell(Vec<String>),
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
    SetPara(Vec<String>),
    SetPage(Vec<String>),
    DeleteImage(Vec<String>),
    DeleteTable(Vec<String>),
    DeleteField(Vec<String>),
    DeleteBookmark(Vec<String>),
}

/// MCP처럼 이미 구조화된 호출자가 CLI mini-language를 거치지 않고 전달하는 편집.
///
/// 문자열 안의 `=>`, `=`, `:`, `@`는 데이터 그대로 유지된다. CLI 전용 문자열
/// 파서는 `EditOperation`에만 남기고, JSON/MCP 경계에서는 이 타입만 사용한다.
pub(crate) enum TypedEditOperation {
    Replace {
        from: String,
        to: String,
    },
    SetCell {
        table: usize,
        row: u16,
        col: u16,
        text: String,
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
    },
    SetAlign {
        pattern: String,
        align: u8,
    },
    InsertPara {
        anchor: String,
        text: String,
        before: bool,
    },
    DeletePara {
        matching: String,
    },
    AddRow {
        table: usize,
    },
    AddCol {
        table: usize,
        at: Option<u16>,
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
    SetPara {
        pattern: String,
        /// Paragraph shape converted up to HWPUNIT/pt×100 units (same units as the CLI `parse_para_props`).
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
                | Self::AddRow { .. }
                | Self::AddCol { .. }
                | Self::DeleteRow { .. }
                | Self::DeleteCol { .. }
                | Self::MergeCells { .. }
                | Self::SplitCell { .. }
                | Self::AddTable { .. }
                | Self::DeleteImage { .. }
                | Self::DeleteTable { .. }
                | Self::DeleteField { .. }
                | Self::DeleteBookmark { .. }
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
            | Self::DeleteImage(_)
            | Self::DeleteTable(_)
            | Self::DeleteField(_)
            | Self::DeleteBookmark(_) => true,
            Self::Replace(_)
            | Self::SetCell(_)
            | Self::CreateField(_)
            | Self::CreateBookmark(_)
            | Self::CreateHyperlink(_)
            | Self::SetField(_)
            | Self::SetMeta(_)
            | Self::SetFormat(_)
            | Self::SetAlign(_)
            | Self::SetPara(_)
            | Self::SetPage(_) => false,
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
}

#[derive(Debug)]
pub struct EditReport {
    pub output: String,
    pub applied: usize,
    pub warnings: Vec<String>,
}

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
            replace,
            set_cell,
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
            set_para,
            set_page,
            delete_image,
            delete_table,
            delete_field,
            delete_bookmark,
            verify,
            allow_partial,
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
        add!(SetPara, set_para);
        add!(SetPage, set_page);
        add!(DeleteImage, delete_image);
        add!(DeleteTable, delete_table);
        add!(DeleteField, delete_field);
        add!(DeleteBookmark, delete_bookmark);

        (
            input,
            output,
            Self {
                operations,
                typed_operations: Vec::new(),
                verify,
                allow_partial,
            },
        )
    }

    pub(crate) fn from_typed(
        operations: Vec<TypedEditOperation>,
        verify: bool,
        allow_partial: bool,
    ) -> Self {
        Self {
            operations: Vec::new(),
            typed_operations: operations,
            verify,
            allow_partial,
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
        if self.operations.is_empty()
            && self
                .typed_operations
                .iter()
                .all(|operation| matches!(operation, TypedEditOperation::Replace { .. }))
        {
            return Ok(Some(
                self.typed_operations
                    .iter()
                    .map(|operation| match operation {
                        TypedEditOperation::Replace { from, to } => (from.clone(), to.clone()),
                        _ => unreachable!("all로 Replace 여부를 확인함"),
                    })
                    .collect(),
            ));
        }
        Ok(None)
    }
}

pub fn run(input: &Path, output: &Path, plan: &EditPlan) -> anyhow::Result<()> {
    let report = execute(input, output, plan)?;
    crate::commands::convert::print_warnings(&report.warnings);
    eprintln!("편집 완료: {} → {}", input.display(), output.display());
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
        let report = crate::commands::output::write_validated(
            output,
            Some(input),
            |staged| patch_replacements_staged(input, staged, &pairs, plan.allow_partial),
            |staged, _| {
                if plan.verify {
                    verify_output(staged, None)?;
                }
                Ok(())
            },
        )?;
        for (entry, n) in &report.counts {
            eprintln!("치환(패키지 보존): {entry} ({n}건)");
        }
        return Ok(EditReport {
            output: output.display().to_string(),
            applied: report.applied_requests,
            warnings: report.warnings,
        });
    }

    let mut doc = load_document(input)?;
    let original_doc = doc.clone();
    let mut edits = 0usize;
    let mut unapplied = Vec::new();
    // 구조 편집(문단/행 추가·삭제·이미지 삽입)은 합성 경로로 써야 한다 — 삽입 문단/행
    // 불변식 + 그림 도형 레코드 합성(빈-extras Picture)이 적용되도록.
    let structural = plan.operations.iter().any(EditOperation::is_structural)
        || plan
            .typed_operations
            .iter()
            .any(TypedEditOperation::is_structural);

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
                    let parts: Vec<&str> = loc.split(':').collect();
                    if parts.len() != 3 {
                        anyhow::bail!("--set-cell 위치는 \"표:행:열\" 형식입니다: {loc:?}");
                    }
                    let ti: usize = parts[0].trim().parse().context("표 인덱스")?;
                    let r: u16 = parts[1].trim().parse().context("행 번호")?;
                    let c: u16 = parts[2].trim().parse().context("열 번호")?;
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
                    hwp_convert::insert_seal(&mut doc, anchor, Path::new(path), size_mm)
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
                        eprintln!("경고: 앵커 {anchor:?}를 찾지 못했습니다");
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
                        eprintln!("경고: 앵커 {anchor:?}를 찾지 못했습니다");
                        unapplied.push(format!("--insert-para {spec:?}"));
                    }
                }
            }
            EditOperation::DeletePara(specs) => {
                for matching in specs {
                    let n = hwp_convert::delete_paragraph(&mut doc, matching);
                    if n == 0 {
                        eprintln!("경고: 삭제 대상 문단 {matching:?}를 찾지 못했습니다");
                        unapplied.push(format!("--delete-para {matching:?}"));
                    } else {
                        eprintln!("문단 삭제: {matching:?} ({n}건)");
                    }
                    edits += n;
                }
            }
            EditOperation::AddRow(specs) => {
                for spec in specs {
                    let ti: usize = spec.trim().parse().with_context(|| {
                        format!("--add-row 형식은 표 인덱스(예: \"0\") 입니다: {spec:?}")
                    })?;
                    hwp_convert::add_rows(&mut doc, ti, None, 1).map_err(|e| anyhow::anyhow!(e))?;
                    eprintln!("표 행 추가: 표{ti}");
                    edits += 1;
                }
            }
            EditOperation::AddCol(specs) => {
                for spec in specs {
                    // "표"(끝에 추가) 또는 "표:위치"(위치에 삽입).
                    if let Some((t, at)) = spec.split_once(':') {
                        let ti: usize = t.trim().parse().context("표 인덱스")?;
                        let at_col: u16 = at.trim().parse().context("열 위치")?;
                        hwp_convert::add_table_column(&mut doc, ti, at_col)
                            .map_err(|e| anyhow::anyhow!(e))?;
                        eprintln!("표 열 추가: 표{ti} 위치{at_col} (전체 폭 유지)");
                    } else {
                        let ti: usize = spec.trim().parse().with_context(|| {
                            format!("--add-col 형식은 \"표\" 또는 \"표:위치\" 입니다: {spec:?}")
                        })?;
                        hwp_convert::add_col(&mut doc, ti).map_err(|e| anyhow::anyhow!(e))?;
                        eprintln!("표 열 추가: 표{ti} 끝 (전체 폭 유지)");
                    }
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
            EditOperation::SetPara(specs) => {
                for spec in specs {
                    let before = doc.clone();
                    let (pattern, kv) = spec.split_once("=>").with_context(|| {
                        format!("--set-para 형식은 \"찾기=>키:값\" 입니다: {spec:?}")
                    })?;
                    let props = parse_para_props(kv)?;
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
        }
    }
    for operation in &plan.typed_operations {
        apply_typed_operation(operation, &mut doc, &mut edits, &mut unapplied)?;
    }

    if !unapplied.is_empty() && !plan.allow_partial {
        anyhow::bail!(
            "적용되지 않은 편집 요청이 있습니다: {} (--allow-partial로 일치한 요청만 적용 가능)",
            unapplied.join(", ")
        );
    }
    if edits == 0 || doc == original_doc {
        anyhow::bail!(
            "적용 가능한 편집이 없어 출력을 게시하지 않습니다 \
             (--replace/--set-cell/--set-field/--set-meta 등 요청 확인)"
        );
    }

    let mut warnings = unapplied
        .iter()
        .map(|request| format!("미적용 편집 요청: {request}"))
        .collect::<Vec<_>>();
    let writer_warnings = crate::commands::output::write_validated(
        output,
        Some(input),
        |staged| write_output(&doc, staged, structural, output_format),
        |staged, writer_warnings| {
            if output_format.supports_verify() {
                crate::commands::reject_drop_warnings("edit", writer_warnings)?;
            }
            if plan.verify {
                verify_output(staged, Some(&doc))?;
            }
            Ok(())
        },
    )?;
    warnings.extend(writer_warnings);
    Ok(EditReport {
        output: output.display().to_string(),
        applied: edits,
        warnings,
    })
}

struct PatchReport {
    counts: BTreeMap<String, usize>,
    applied_requests: usize,
    warnings: Vec<String>,
}

fn patch_replacements_staged(
    input: &Path,
    staged: &Path,
    pairs: &[(String, String)],
    allow_partial: bool,
) -> anyhow::Result<PatchReport> {
    let parent = staged.parent().context("임시 출력 작업공간이 없습니다")?;
    let mut current = input.to_path_buf();
    let mut current_is_temporary = false;
    let mut applied_requests = 0usize;
    let mut totals = BTreeMap::new();
    let mut warnings = Vec::new();

    for (index, (from, to)) in pairs.iter().enumerate() {
        if from.is_empty() || from == to {
            if allow_partial {
                warnings.push(format!("미적용 편집 요청: --replace {from:?}=>{to:?}"));
                continue;
            }
            anyhow::bail!(
                "적용되지 않은 편집 요청이 있습니다: --replace {from:?}=>{to:?} \
                 (--allow-partial로 일치한 요청만 적용 가능)"
            );
        }
        let next = parent.join(format!(".hwp-replace-step-{index}.hwpx"));
        let counts = hwpx::patch::replace_texts(&current, &next, &[(from.clone(), to.clone())])?;
        let matches = counts
            .iter()
            .filter(|(entry, _)| entry.starts_with("Contents/section") && entry.ends_with(".xml"))
            .map(|(_, count)| *count)
            .sum::<usize>();
        if matches == 0 {
            let _ = fs::remove_file(&next);
            if allow_partial {
                warnings.push(format!("미적용 편집 요청: --replace {from:?}=>{to:?}"));
                continue;
            }
            anyhow::bail!(
                "적용되지 않은 편집 요청이 있습니다: --replace {from:?}=>{to:?} \
                 (런 분절 교차 매칭은 미지원, --allow-partial로 일치한 요청만 적용 가능)"
            );
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

    if applied_requests == 0 {
        anyhow::bail!("적용 가능한 편집이 없어 출력을 게시하지 않습니다");
    }
    let original = load_document(input)?;
    let final_doc = load_document(&current)?;
    if semantic_signature(&original) == semantic_signature(&final_doc) {
        anyhow::bail!(
            "순차 치환의 최종 결과가 원문과 같아 출력을 게시하지 않습니다 \
             (상쇄되는 --replace 요청 확인)"
        );
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

fn apply_typed_operation(
    operation: &TypedEditOperation,
    doc: &mut hwp_model::Document,
    edits: &mut usize,
    unapplied: &mut Vec<String>,
) -> anyhow::Result<()> {
    match operation {
        TypedEditOperation::Replace { from, to } => {
            if from.is_empty() || from == to {
                unapplied.push(format!("replace from={from:?} to={to:?}"));
                return Ok(());
            }
            let before = doc.clone();
            let count = hwp_convert::replace_text(doc, from, to, true);
            eprintln!("치환: {from:?} → {to:?} ({count}건)");
            record_effect(
                &before,
                doc,
                format!("replace from={from:?} to={to:?}"),
                edits,
                unapplied,
            );
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
            hwp_convert::insert_seal(doc, anchor, path, *size_mm)
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
        TypedEditOperation::SetFormat { pattern, format } => {
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
        TypedEditOperation::SetAlign { pattern, align } => {
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
        TypedEditOperation::InsertPara {
            anchor,
            text,
            before,
        } => {
            if hwp_convert::insert_paragraph(doc, anchor, text, *before) {
                eprintln!("문단 삽입: {anchor:?}, before={before}, text={text:?}");
                *edits += 1;
            } else {
                unapplied.push(format!("insert_para anchor={anchor:?}"));
            }
        }
        TypedEditOperation::DeletePara { matching } => {
            let count = hwp_convert::delete_paragraph(doc, matching);
            if count == 0 {
                unapplied.push(format!("delete_para matching={matching:?}"));
            } else {
                eprintln!("문단 삭제: {matching:?} ({count}건)");
                *edits += count;
            }
        }
        TypedEditOperation::AddRow { table } => {
            hwp_convert::add_rows(doc, *table, None, 1).map_err(|error| anyhow::anyhow!(error))?;
            eprintln!("표 행 추가: 표{table}");
            *edits += 1;
        }
        TypedEditOperation::AddCol { table, at } => {
            if let Some(at) = at {
                hwp_convert::add_table_column(doc, *table, *at)
                    .map_err(|error| anyhow::anyhow!(error))?;
            } else {
                hwp_convert::add_col(doc, *table).map_err(|error| anyhow::anyhow!(error))?;
            }
            eprintln!("표 열 추가: 표{table}, 위치={at:?}");
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
        TypedEditOperation::SetPara { pattern, props } => {
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
    }
    Ok(())
}

fn write_output(
    doc: &hwp_model::Document,
    output: &Path,
    structural: bool,
    output_format: OutputFormat,
) -> anyhow::Result<Vec<String>> {
    match output_format {
        // 구조 편집은 삽입 문단/행에 불변식을 세우려 합성 경로를 강제한다.
        OutputFormat::Hwp if structural => {
            crate::commands::convert::write_hwp_structural(doc, output)
        }
        OutputFormat::Hwp => crate::commands::convert::write_hwp_edited(doc, output),
        OutputFormat::Hwpx => Ok(hwpx::write_document(doc, output)?),
        OutputFormat::Json => {
            fs::write(output, hwp_convert::to_json(doc, true, true)?)?;
            Ok(Vec::new())
        }
        OutputFormat::Markdown => {
            fs::write(output, hwp_convert::to_markdown(doc))?;
            Ok(Vec::new())
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

/// Parses `--set-para`'s "key:value" into ParaProps.
/// Keys: line-spacing (ratio % or fixed Npt), indent, left, right, top, bottom (mm).
fn parse_para_props(kv: &str) -> anyhow::Result<hwp_convert::ParaProps> {
    let (key, value) = kv
        .split_once(':')
        .with_context(|| format!("--set-para 형식은 \"키:값\" 입니다: {kv:?}"))?;
    let mut props = hwp_convert::ParaProps::default();
    let key = key.trim();
    let value = value.trim();
    match key {
        "line-spacing" => {
            if let Some(pt) = value.strip_suffix("pt") {
                let pt: f32 = pt.parse().context("줄간격 pt 값")?;
                props.line_spacing = Some((1, (pt * 100.0).round() as i32));
            } else {
                let pct: i32 = value.parse().context("줄간격 비율(%) 값")?;
                props.line_spacing = Some((0, pct));
            }
        }
        "indent" => props.indent = Some(parse_mm(value)?),
        "left" => props.margin_left = Some(parse_mm(value)?),
        "right" => props.margin_right = Some(parse_mm(value)?),
        "top" => props.spacing_top = Some(parse_mm(value)?),
        "bottom" => props.spacing_bottom = Some(parse_mm(value)?),
        other => anyhow::bail!(
            "알 수 없는 문단모양 키: {other:?} (line-spacing/indent/left/right/top/bottom)"
        ),
    }
    Ok(props)
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
}
