//! DocumentSpec v1 block insertion (paragraphs, lists, breaks, tables, images, fields and
//! equations) using the existing compiler against a header-only copy of the target, then the
//! merge remapper's identity rules before the splice.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use hwp_convert::SegmentPath;
use hwp_convert::styles::{StyleKind, list_named_styles};
use hwp_model::{BorderFill, BorderFillId, BorderLine, Control, Document, StyleId};
use serde::Deserialize;

use crate::document_spec::{
    BlockCompiler, BlockDefaults, BlockSpec, BreakKind, DocumentSpec, ListKind, ListSpec,
    MAX_BLOCKS, MetadataSpec, PageSpec, RunSpec, SectionSpec, SpecVersion, StyleSpec,
    validate_spec_in_width,
};

#[derive(Debug, Clone, Copy, Deserialize, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingStylePolicy {
    #[default]
    Create,
    Refuse,
}
impl MissingStylePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Refuse => "refuse",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Before,
    After,
}

/// DocumentSpec v1 content only; the address is resolved by the edit batch preflight.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsertBlocks {
    pub position: Position,
    #[serde(default)]
    pub missing_style: MissingStylePolicy,
    #[serde(default)]
    pub styles: BTreeMap<String, StyleSpec>,
    #[serde(default)]
    pub lists: BTreeMap<String, ListSpec>,
    pub blocks: Vec<BlockSpec>,
}
impl InsertBlocks {
    pub fn before(&self) -> bool {
        matches!(self.position, Position::Before)
    }
}

/// What the blocks (and, through table cells, the blocks nested in them) reference.
#[derive(Default)]
struct Referenced {
    styles: BTreeSet<String>,
    lists: BTreeSet<String>,
    tables: bool,
    images: bool,
    equations: bool,
}

fn scan_blocks(blocks: &[BlockSpec], found: &mut Referenced) -> Result<(), String> {
    for block in blocks {
        match block {
            BlockSpec::Paragraph {
                style, list, runs, ..
            } => {
                found.styles.extend(style.iter().cloned());
                if let Some(list) = list {
                    found.lists.insert(list.name.clone());
                }
                for run in runs {
                    match run {
                        RunSpec::Text { style, .. } | RunSpec::Field { style, .. } => {
                            found.styles.extend(style.iter().cloned());
                        }
                        RunSpec::Equation { .. } => found.equations = true,
                        RunSpec::Image { .. } => found.images = true,
                        RunSpec::LineBreak => {}
                    }
                }
            }
            BlockSpec::Field { style, .. } => found.styles.extend(style.iter().cloned()),
            BlockSpec::Equation { .. } => found.equations = true,
            BlockSpec::Image { .. } => found.images = true,
            BlockSpec::Break {
                kind: BreakKind::Page | BreakKind::Column,
            } => {}
            BlockSpec::Break {
                kind: BreakKind::Section,
            } => {
                return Err("insert_blocks는 section break 블록을 지원하지 않습니다. 구역 나누기는 insert_blocks 밖에서 처리하고, 이 요청에서는 section break 블록을 제거하세요".into());
            }
            BlockSpec::Table { rows, .. } => {
                found.tables = true;
                for row in rows {
                    for cell in &row.cells {
                        scan_blocks(&cell.blocks, found)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether any block, at any table-cell depth, references `name` at `level`.
fn uses_list_level(blocks: &[BlockSpec], name: &str, level: usize) -> bool {
    blocks.iter().any(|block| match block {
        BlockSpec::Paragraph {
            list: Some(reference),
            ..
        } => reference.name == name && usize::from(reference.level) == level,
        BlockSpec::Table { rows, .. } => rows
            .iter()
            .flat_map(|row| &row.cells)
            .any(|cell| uses_list_level(&cell.blocks, name, level)),
        _ => false,
    })
}

fn referenced(request: &InsertBlocks) -> Result<Referenced, String> {
    if request.blocks.is_empty() || request.blocks.len() > MAX_BLOCKS {
        return Err(format!(
            "insert_blocks의 blocks 개수가 제한을 벗어났습니다. 1..={MAX_BLOCKS}개 블록으로 나누어 요청하세요"
        ));
    }
    let mut found = Referenced::default();
    scan_blocks(&request.blocks, &mut found)?;
    for name in &found.lists {
        let list = request.lists.get(name).ok_or_else(|| format!(
            "목록 '{name}'의 lists 정의가 없습니다. lists에 DocumentSpec 목록 정의를 추가한 뒤 다시 시도하세요"
        ))?;
        for level in &list.levels {
            found.styles.extend(level.style.iter().cloned());
        }
    }
    Ok(found)
}

/// Where `base_dir`-relative image assets are read from and which roots contain them: the same
/// containment [`crate::asset_snapshot::read_contained_with_roots`] applies to `compose`.
#[derive(Debug, Clone, Copy)]
pub struct AssetContext<'a> {
    /// Directory that relative asset paths resolve against. `None` refuses image blocks.
    pub base_dir: Option<&'a Path>,
    /// MCP sandbox roots (canonical). Empty means no extra root check, as for the CLI.
    pub roots: &'a [PathBuf],
}

/// Writable width of an A4 portrait page with 20 mm margins, for a section without a page
/// definition (PageSpec default).
const DEFAULT_CONTAINER_MM: f32 = 170.0;

fn hwpunit_to_mm(units: i64) -> f32 {
    (units as f64 * 25.4 / f64::from(hwp_model::HwpUnit::PER_INCH)) as f32
}

/// Width in HWPUNIT of one text column of `section` at its top-level paragraph `at`, given the
/// full body width: the column definition in effect there, which is the last one at or before
/// that paragraph (a section may change its layout partway). A `before` insertion lands ahead
/// of the target paragraph and of any column control it carries
/// (`hwp_convert::structure::insert_paragraphs_at` splices at the paragraph's own index), so it
/// flows under the definition preceding the target: the target's own is excluded. The column
/// width is split the way `hwp-render` splits it (equal columns share the body width less the
/// gaps). Columns of unequal width list their own widths; which of them a block lands in is not
/// known here, so the narrowest is the safe measure.
///
/// Every number here comes from the document, so the arithmetic is widened to i64 and a
/// negative gap, which has no meaning, yields `None`. A width of zero or less (gaps wider than
/// the body) is refused by the caller's positivity check.
fn column_width_hwp(
    section: &hwp_model::Section,
    at: usize,
    before: bool,
    body: i64,
) -> Option<i64> {
    let def = section
        .paragraphs
        .iter()
        .take(if before { at } else { at.saturating_add(1) })
        .flat_map(|paragraph| &paragraph.controls)
        .filter_map(|control| match control {
            Control::Generic(generic) => generic.column_def.as_ref(),
            _ => None,
        })
        .next_back();
    let Some(def) = def else { return Some(body) };
    let count = i64::from(def.count.max(1));
    if count == 1 {
        return Some(body);
    }
    if !def.same_width
        && let Some(narrowest) = def.widths.iter().copied().filter(|w| *w > 0).min()
    {
        return Some(i64::from(narrowest));
    }
    if def.gap < 0 {
        return None;
    }
    // `count` is a u16 and `gap` an i32, so the product stays below 2^48 and cannot overflow an
    // i64. Gaps wider than the body give a width of zero or less, which `check_target` refuses.
    Some((body - i64::from(def.gap) * (count - 1)) / count)
}

/// Limit this phase to the body and table cells, and return the width in mm of the text column
/// or table cell the blocks will land in. Opaque Generic objects need a writer regeneration
/// proof, not merely a readable address into their extracted model lists.
fn check_target(doc: &Document, target: &SegmentPath, before: bool) -> Result<f32, String> {
    let invalid = || {
        "insert_blocks 삽입 위치가 본문 또는 표 셀 문단이 아닙니다. 최신 문서에서 본문/표 셀 문단 주소를 다시 선택하세요".to_owned()
    };
    let section = doc.sections.get(target.section).ok_or_else(invalid)?;
    let first = *target.indices.first().ok_or_else(invalid)?;
    let unknown = || {
        "삽입 위치의 본문 폭을 알 수 없습니다. 쪽 여백, 단 간격, 표 셀 폭과 여백이 유효한 문서인지 확인하세요"
            .to_owned()
    };
    // Page, column and cell numbers all come from the document and the HWP5 reader accepts any
    // i32 for them, so the sums below run in i64 and an impossible geometry is refused as an
    // unknown width instead of panicking (overflow-checked builds) or wrapping (release).
    let mut width = match section.section_def().and_then(|def| def.page) {
        None => DEFAULT_CONTAINER_MM,
        Some(page) => {
            // PAGE_DEF attr bit 0 rotates the paper, so the horizontal extent is `height`.
            // `hwp-render` reads it the same way (layout.rs, "Landscape (PAGE_DEF attr bit0)").
            let paper = if page.attr & 1 != 0 {
                page.height.0
            } else {
                page.width.0
            };
            if [page.margin_left.0, page.margin_right.0, page.gutter.0]
                .iter()
                .any(|v| *v < 0)
            {
                return Err(unknown());
            }
            let body = i64::from(paper)
                - i64::from(page.margin_left.0)
                - i64::from(page.margin_right.0)
                - i64::from(page.gutter.0);
            let column = column_width_hwp(section, first, before, body).ok_or_else(unknown)?;
            hwpunit_to_mm(column)
        }
    };
    let mut paragraph = section
        .paragraphs
        .get(*target.indices.first().ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let mut rest = &target.indices[1..];
    while !rest.is_empty() {
        let [control, cell, para, tail @ ..] = rest else {
            return Err(invalid());
        };
        let Some(Control::Table(table)) = paragraph.controls.get(*control) else {
            return Err(invalid());
        };
        let cell = table.cells.get(*cell).ok_or_else(invalid)?;
        // The cell's own margins are often the 0xffff "inherit" sentinel or all zero
        // ("omitted"); `Table::cell_margins` resolves both exactly as the renderer does.
        let margins = table.cell_margins(cell);
        width =
            hwpunit_to_mm(i64::from(cell.width.0) - i64::from(margins[0]) - i64::from(margins[1]));
        paragraph = cell.paragraphs.get(*para).ok_or_else(invalid)?;
        rest = tail;
    }
    if !width.is_finite() || width <= 0.0 {
        return Err(unknown());
    }
    Ok(width)
}

/// 1-based id of a solid black four-sided border fill, appended to the target header when it
/// has none. Compose's template keeps this look at fixed id 3, which another document's table
/// does not share, so an inserted table asks the target for the fill instead of assuming it.
fn table_border_fill(header: &mut hwp_model::DocHeader) -> Result<BorderFillId, String> {
    let solid = BorderLine {
        line_type: 1,
        width: 1,
        color: 0,
    };
    let wanted = BorderFill {
        sides: [solid; 4],
        diagonal: BorderLine {
            line_type: 1,
            width: 0,
            color: 0,
        },
        ..BorderFill::default()
    };
    let same = |fill: &BorderFill| {
        fill.attr == wanted.attr
            && fill.sides == wanted.sides
            && fill.diagonal == wanted.diagonal
            && fill.fill_type == 0
            && fill.bg_color.is_none()
            && fill.hatch.is_none()
            && fill.gradient.is_none()
    };
    let index = match header.border_fills.iter().position(same) {
        Some(index) => index,
        None => {
            header.border_fills.push(wanted);
            header.border_fills.len() - 1
        }
    };
    u16::try_from(index + 1)
        .map(BorderFillId)
        .map_err(|_| "대상 문서의 테두리/배경 표가 가득 차 표 테두리를 추가할 수 없습니다. 표 개수를 줄이거나 다른 문서에 삽입하세요".to_owned())
}

/// Non-raw HWP NUMBERING definitions become the writer's synthesized record.
/// Compare its actual templates/starts and the format selectors the reader does not retain;
/// do not project away a list setting that the requested output cannot represent.
fn check_hwp_lists(
    original: &Document,
    request: &InsertBlocks,
    lists: &BTreeMap<String, ListSpec>,
) -> Result<(), String> {
    // Match materialize_evidenced_official_numbering's ORIGINAL-document gate.
    // An appended seven-level definition disables that all-definitions contract,
    // making the writer replace existing official definitions with defaults too.
    if original.meta.source_format != "hwp5"
        && original.header.numberings.is_empty()
        && hwp5::numbering::is_official_eight_level_contract(&original.header.numbering_levels)
        && let Some((name, _)) = lists
            .iter()
            .find(|(_, list)| list.kind == ListKind::Ordered)
    {
        return Err(format!(
            "목록 '{name}'의 새 번호 정의를 추가하면 기존 8수준 official 번호의 HWP 직렬화 계약이 깨져 기존 번호 인코딩이 변경됩니다. 기존 번호를 보존하려면 HWPX 출력으로 다시 시도하세요"
        ));
    }
    let defaults = hwp5::write::synthesized_numbering_levels();
    for (name, list) in lists {
        if list.kind != ListKind::Ordered {
            continue;
        }
        for (index, level) in list.levels.iter().enumerate() {
            let Some(default) = defaults.get(index) else {
                return Err(format!(
                    "목록 '{name}'의 level {index}는 현재 HWP 출력기의 기본 번호 범위를 벗어납니다. HWPX 출력으로 다시 시도하세요"
                ));
            };
            let token = format!("^{}", index + 1);
            if level.start != default.start
                || level.marker.replace("{n}", &token) != default.template
            {
                let marker = default.template.replace(&token, "{n}");
                return Err(format!(
                    "목록 '{name}'의 level {index} marker/start를 현재 HWP 출력기가 보존할 수 없습니다. HWPX 출력으로 다시 시도하거나 기본 marker '{marker}', start {}을 사용하세요",
                    default.start
                ));
            }
            let used = uses_list_level(&request.blocks, name, index);
            if used
                && hwp5::write::DEFAULT_NUMBERING_FORMATS.get(index)
                    != Some(&hwp_model::NumFmt::Digit)
            {
                return Err(format!(
                    "목록 '{name}'의 level {index} 번호 형식(Digit)을 현재 HWP 출력기가 해당 수준의 기본 형식으로 바꿉니다. 번호 형식을 보존하려면 HWPX 출력으로 다시 시도하세요"
                ));
            }
        }
    }
    Ok(())
}

struct StyleResolver<'a, 'b> {
    compiler: &'a mut BlockCompiler<'b>,
    request: &'a InsertBlocks,
    target: &'a [hwp_convert::styles::NamedStyle],
    resolved: BTreeMap<String, StyleId>,
    validation: BTreeMap<String, StyleSpec>,
    active: BTreeSet<String>,
    max_style: u16,
}
impl StyleResolver<'_, '_> {
    fn resolve(&mut self, name: &str) -> Result<StyleId, String> {
        if let Some(id) = self.resolved.get(name) {
            return Ok(*id);
        }
        if !self.active.insert(name.to_owned()) {
            return Err(format!(
                "스타일 '{name}'의 based_on 순환 참조로 삽입을 거부했습니다. styles의 상속 순환을 제거한 뒤 다시 시도하세요"
            ));
        }
        let matches: Vec<_> = self
            .target
            .iter()
            .filter(|entry| entry.name == name)
            .collect();
        let id = if let [entry] = matches.as_slice() {
            if entry.kind != Some(StyleKind::Paragraph) {
                return Err(format!(
                    "스타일 '{name}'의 종류가 paragraph가 아니거나 알려져 있지 않습니다. hwp styles에서 문단 스타일을 선택하세요"
                ));
            }
            self.compiler
                .register_target_style(name, StyleId(entry.id))
                .map_err(|error| format!("스타일 '{name}'의 정의를 해석할 수 없습니다: {error}. styles와 원본 문서의 서식 참조/표 크기를 확인한 뒤 다시 시도하세요"))?;
            self.validation
                .insert(name.to_owned(), StyleSpec::default());
            StyleId(entry.id)
        } else if !matches.is_empty() {
            return Err(format!(
                "스타일 '{name}'이 대상 문서에 여러 개 있어 삽입을 거부했습니다. 원본 문서의 중복 이름을 정리한 뒤 hwp styles에서 확인하세요"
            ));
        } else {
            let definition = self.request.styles.get(name).filter(|_| matches!(self.request.missing_style, MissingStylePolicy::Create)).ok_or_else(|| format!(
                "스타일 '{name}'을 대상 문서에서 찾을 수 없습니다. styles 정의를 제공하고 missing_style=create로 다시 시도하거나 hwp styles에서 기존 스타일을 선택하세요"
            ))?.clone();
            let base = match &definition.based_on {
                Some(parent) => self.resolve(parent)?,
                None => StyleId(0),
            };
            let id = self
                .compiler
                .compile_style_over(name, &definition, base)
                .map_err(|error| format!("스타일 '{name}'의 정의를 해석할 수 없습니다: {error}. styles와 원본 문서의 서식 참조/표 크기를 확인한 뒤 다시 시도하세요"))?;
            self.validation.insert(name.to_owned(), definition);
            id
        };
        if id.0 > self.max_style {
            return Err(format!(
                "스타일 '{name}'의 번호 {}를 HWP의 1바이트 범위에 저장할 수 없습니다. HWPX 출력으로 다시 시도하세요",
                id.0
            ));
        }
        self.active.remove(name);
        self.resolved.insert(name.to_owned(), id);
        Ok(id)
    }
}

/// Compile the blocks against a header-only copy of the target and publish them only after
/// every check succeeds. HWP output is bounded by the existing writer's style IDs, numbering
/// records and (for equations) its lack of an equation record.
///
/// The compiler works on `Document { header, ..Default }`, not on a clone of the whole target:
/// it only appends to header tables and to `bin_streams`, so cloning the target's sections and
/// binary payloads for every op was pure cost. New binaries are grafted by name afterwards; an
/// asset the target already holds under the same name and bytes is shared, a same name with
/// different bytes is refused.
pub fn apply(
    doc: &mut Document,
    target: &SegmentPath,
    request: &InsertBlocks,
    hwp_output: bool,
    assets: &AssetContext,
) -> Result<usize, String> {
    let container_mm = check_target(doc, target, request.before())?;
    if request.styles.len() > 256 || request.lists.len() > 256 {
        return Err(
            "insert_blocks의 styles/lists 정의가 상한 256개를 넘습니다. 정의를 나누어 요청하세요"
                .into(),
        );
    }
    let max_style = if hwp_output {
        u16::from(u8::MAX)
    } else {
        u16::MAX
    };
    let found = referenced(request)?;
    if hwp_output && found.equations {
        return Err("HWP 출력은 수식 블록을 저장하지 못합니다 (수식 개체가 제거됩니다). 수식이 필요하면 HWPX 출력으로 다시 시도하거나 수식 블록을 제거하세요".into());
    }
    let base_dir = match (found.images, assets.base_dir) {
        (true, None) => {
            return Err("이 호출 경로에는 이미지 자산 기준 디렉터리가 없어 이미지 블록을 삽입할 수 없습니다. --ops 파일 옆에 이미지를 두고 CLI로 실행하세요".into());
        }
        (_, base) => base.unwrap_or(Path::new(".")),
    };
    let names = found.styles;
    let styles = list_named_styles(doc, &[]).map_err(|error| error.to_string())?;
    let default = doc.header.styles.first().ok_or_else(|| "대상 문서의 기본 스타일 0이 없습니다. hwp styles로 유효한 문서인지 확인한 뒤 다시 시도하세요".to_owned())?;
    if styles.first().and_then(|style| style.kind) != Some(StyleKind::Paragraph) {
        return Err("기본 스타일 0의 종류가 paragraph가 아닙니다. 문단 기본 스타일이 있는 원본 문서를 선택하세요".into());
    }
    // The shell carries the target's source format: the compiler encodes list levels in
    // the convention of the IR it extends (zero-based raw bits for an HWP5-read target, #472).
    let mut shell = Document {
        header: doc.header.clone(),
        meta: doc.meta.clone(),
        ..Document::default()
    };
    let border_fill = if found.tables {
        table_border_fill(&mut shell.header)?
    } else {
        BorderFillId(0)
    };
    let defaults = BlockDefaults {
        style: StyleId(0),
        para_shape: default.para_shape,
        char_shape: default.char_shape,
        border_fill,
    };
    let mut compiler = BlockCompiler::over_document(shell, defaults, base_dir, assets.roots);
    compiler
        .register_target_style("", StyleId(0))
        .map_err(|error| format!("기본 스타일 0의 서식 참조가 유효하지 않습니다: {error}. 원본 문서의 기본 스타일 참조를 복구한 뒤 다시 시도하세요"))?;
    let mut resolver = StyleResolver {
        compiler: &mut compiler,
        request,
        target: &styles,
        resolved: BTreeMap::new(),
        validation: BTreeMap::new(),
        active: BTreeSet::new(),
        max_style,
    };
    for name in names {
        resolver.resolve(&name)?;
    }
    let lists: BTreeMap<_, _> = found
        .lists
        .iter()
        .map(|name| (name.clone(), request.lists[name].clone()))
        .collect();
    let validation = DocumentSpec {
        version: SpecVersion::V1,
        metadata: MetadataSpec::default(),
        page: PageSpec::default(),
        styles: resolver.validation,
        lists: lists.clone(),
        sections: vec![SectionSpec {
            id: None,
            page: None,
            header: None,
            footer: None,
            page_number: None,
            blocks: request.blocks.clone(),
        }],
    };
    validate_spec_in_width(&validation, base_dir, Some(container_mm)).map_err(|error| format!("insert_blocks 블록/스타일/목록 정의가 유효하지 않습니다: {error}. DocumentSpec v1 정의를 수정한 뒤 다시 시도하세요 (삽입 위치의 폭은 {container_mm:.1}mm입니다)"))?;
    if hwp_output {
        check_hwp_lists(doc, request, &lists)?;
    }
    compiler
        .compile_lists(&lists)
        .map_err(|error| format!("목록 컴파일 실패: {error}. lists 정의와 원본 번호/글머리표 표 크기를 확인한 뒤 다시 시도하세요"))?;
    let mut paragraphs = Vec::new();
    for (index, block) in request.blocks.iter().enumerate() {
        paragraphs.extend(compiler.compile_block(block, &format!("$.blocks[{index}]")).map_err(|error| format!("insert_blocks 컴파일 실패: {error}. DocumentSpec v1 정의를 확인한 뒤 다시 시도하세요"))?);
    }
    let count = paragraphs.len();
    let compiled = compiler.into_document();
    for stream in &compiled.bin_streams {
        if let Some(existing) = doc.bin_streams.iter().find(|have| have.name == stream.name)
            && existing.data != stream.data
        {
            return Err(format!(
                "대상 문서에 같은 이름의 다른 바이너리 '{}'가 이미 있습니다. 이미지 파일을 바꾸거나 다른 이름의 문서에 삽입하세요",
                stream.name
            ));
        }
    }
    let streams = plan_streams(doc, &compiled.bin_streams);
    if !streams.renames.is_empty() {
        for paragraph in &mut paragraphs {
            rename_item_refs(paragraph, &streams.renames);
        }
    }
    hwp_convert::document_merge::renumber_inserted_objects(doc, &mut paragraphs)?;
    // The only fallible step that touches the target: it validates before it splices, so a
    // refusal leaves `doc` exactly as it was.
    hwp_convert::structure::insert_paragraphs_at(doc, target, request.before(), paragraphs)?;
    doc.header = compiled.header;
    for mut stream in compiled.bin_streams {
        if streams.shared.contains(&stream.name) {
            continue;
        }
        if let Some(new_name) = streams.renames.get(&stream.name) {
            stream.name.clone_from(new_name);
        }
        if !doc.bin_streams.iter().any(|have| have.name == stream.name) {
            doc.bin_streams.push(stream);
        }
    }
    Ok(count)
}

/// What to do with each compiled image stream before it joins the target.
struct StreamPlan {
    /// Streams whose name the target's resolver would send somewhere else, with the unused
    /// name each takes instead.
    renames: BTreeMap<String, String>,
    /// Streams the target already holds, byte for byte, where the resolver finds them: the
    /// picture's reference already resolves to the same bytes, so no second copy is stored.
    shared: BTreeSet<String>,
}

/// Decide, for every compiled stream, how a picture naming it will resolve in the target.
/// `Document::bin_stream_index` is manifest-first and also matches by `/<name>` suffix and by
/// file stem, so an `ItemRef` can land on a stream other than the one just compiled:
///
/// - the HWP5 writer's own lookup is more permissive than the model's (ASCII case-insensitive
///   file name, then substring), so a name any of them would take counts as a collision;
/// - it resolves to a stream with the same bytes: share it, do not append a duplicate;
/// - it resolves to other bytes, or the package manifest lists the name for another file, or
///   lists it for a file the document does not carry: rename the compiled stream to a name
///   the resolver finds nowhere in the target, so the appended stream is the only match.
fn plan_streams(doc: &Document, streams: &[hwp_model::BinStream]) -> StreamPlan {
    let manifest_lists = |name: &str| {
        doc.hwpx_bin_manifest
            .iter()
            .any(|(id, href)| id == name || href == name)
    };
    // Every stream any writer could take for `name`: the model resolver's pick plus what the
    // HWP5 writer's lookup (`hwp5::write::bin_stream_index`) accepts, which is the most
    // permissive one: the file name ignoring ASCII case, then any name containing it. A
    // collision with any of them would capture the picture in that format's output.
    let candidates = |name: &str| -> BTreeSet<usize> {
        let mut found: BTreeSet<usize> = doc
            .bin_stream_index(&hwp_model::BinRef::ItemRef(name.to_owned()))
            .into_iter()
            .collect();
        for (index, have) in doc.bin_streams.iter().enumerate() {
            let trailing = have.name.rsplit('/').next().unwrap_or(&have.name);
            if trailing.eq_ignore_ascii_case(name) || have.name.contains(name) {
                found.insert(index);
            }
        }
        found
    };
    let unresolved = |name: &str, plan: &StreamPlan| {
        candidates(name).is_empty()
            && !manifest_lists(name)
            && !streams.iter().any(|have| have.name == name)
            && !plan.renames.values().any(|taken| taken == name)
    };
    let mut plan = StreamPlan {
        renames: BTreeMap::new(),
        shared: BTreeSet::new(),
    };
    for stream in streams {
        let hits = candidates(&stream.name);
        let model = doc.bin_stream_index(&hwp_model::BinRef::ItemRef(stream.name.clone()));
        if hits.is_empty() {
            let listed_for_another_file = doc
                .hwpx_bin_manifest
                .iter()
                .any(|(id, href)| *id == stream.name && *href != stream.name);
            if !listed_for_another_file {
                continue;
            }
        } else if model.is_some() && hits.iter().all(|i| doc.bin_streams[*i].data == stream.data) {
            // Every writer lands on a byte-identical stream, and the model resolver finds it
            // too (the HWPX output keeps the reference): store nothing twice.
            plan.shared.insert(stream.name.clone());
            continue;
        }
        let (stem, extension) = match stream.name.rsplit_once('.') {
            Some((stem, extension)) => (stem.to_owned(), format!(".{extension}")),
            None => (stream.name.clone(), String::new()),
        };
        let mut n = 2u32;
        let new_name = loop {
            let candidate = format!("{stem}-{n}{extension}");
            if unresolved(&candidate, &plan) {
                break candidate;
            }
            n += 1;
        };
        plan.renames.insert(stream.name.clone(), new_name);
    }
    plan
}

fn rename_item_refs(paragraph: &mut hwp_model::Paragraph, renames: &BTreeMap<String, String>) {
    for control in &mut paragraph.controls {
        match control {
            Control::Picture(picture) => {
                if let hwp_model::BinRef::ItemRef(name) = &mut picture.bin_ref
                    && let Some(new_name) = renames.get(name)
                {
                    name.clone_from(new_name);
                }
                if let Some(caption) = &mut picture.caption {
                    for p in &mut caption.paragraphs {
                        rename_item_refs(p, renames);
                    }
                }
            }
            Control::Table(table) => {
                if let Some(caption) = &mut table.caption {
                    for p in &mut caption.paragraphs {
                        rename_item_refs(p, renames);
                    }
                }
                for cell in &mut table.cells {
                    for p in &mut cell.paragraphs {
                        rename_item_refs(p, renames);
                    }
                }
            }
            Control::Generic(generic) => {
                for list in &mut generic.paragraph_lists {
                    for p in &mut list.paragraphs {
                        rename_item_refs(p, renames);
                    }
                }
                if let Some(caption) = &mut generic.caption {
                    for p in &mut caption.paragraphs {
                        rename_item_refs(p, renames);
                    }
                }
            }
            Control::SectionDef(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0d, b'I', b'D',
        b'A', b'T', 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff,
        0x56, 0xc7, 0x2f, 0x0d, 0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82,
    ];

    fn request(path: &str) -> InsertBlocks {
        serde_json::from_value(serde_json::json!({
            "position": "after",
            "blocks": [{"type": "image", "path": path, "width_mm": 10}]
        }))
        .expect("insert_blocks request")
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hwp-insert-blocks-unit-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/logo.png"), PNG).unwrap();
        dir
    }

    fn target() -> (Document, SegmentPath) {
        let doc = hwp_convert::from_markdown("anchor\n");
        (
            doc,
            SegmentPath {
                section: 0,
                indices: vec![0],
            },
        )
    }

    #[test]
    fn image_assets_resolve_below_the_base_dir_and_are_bound_to_the_sandbox_roots() {
        let dir = scratch("roots");
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let other = scratch("roots-other");
        let other_root = std::fs::canonicalize(&other).unwrap();

        // Inside the allowed root: inserted, and the asset is grafted into the target.
        let (mut doc, at) = target();
        let streams = doc.bin_streams.len();
        let roots = [canonical.clone()];
        let inside = AssetContext {
            base_dir: Some(&canonical),
            roots: &roots,
        };
        apply(&mut doc, &at, &request("assets/logo.png"), false, &inside)
            .expect("asset inside the sandbox root");
        assert_eq!(doc.bin_streams.len(), streams + 1);

        // Same base directory, but the sandbox allows another root only: refused, target intact.
        let (mut doc, at) = target();
        let before = doc.clone();
        let roots = [other_root];
        let outside = AssetContext {
            base_dir: Some(&canonical),
            roots: &roots,
        };
        let error = apply(&mut doc, &at, &request("assets/logo.png"), false, &outside)
            .expect_err("asset outside the sandbox roots");
        assert!(error.contains("outside_roots"), "{error}");
        assert_eq!(doc, before, "a refused insertion must not touch the target");

        // No asset directory (stdin ops, MCP before T4): image blocks are refused by name.
        let (mut doc, at) = target();
        let none = AssetContext {
            base_dir: None,
            roots: &[],
        };
        let error = apply(&mut doc, &at, &request("assets/logo.png"), false, &none)
            .expect_err("no asset base directory");
        assert!(error.contains("이미지"), "{error}");
        assert_eq!(doc, before);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_same_named_asset_with_different_bytes_in_the_target_is_refused() {
        let dir = scratch("clash");
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let ctx = AssetContext {
            base_dir: Some(&canonical),
            roots: &[],
        };
        let (mut first, at) = target();
        apply(&mut first, &at, &request("assets/logo.png"), false, &ctx).unwrap();
        let name = first.bin_streams.last().unwrap().name.clone();

        // A target already holding that stream name with other bytes cannot share it.
        let (mut doc, at) = target();
        doc.bin_streams.push(hwp_model::BinStream {
            name,
            data: b"different".to_vec(),
        });
        let before = doc.clone();
        let error = apply(&mut doc, &at, &request("assets/logo.png"), false, &ctx)
            .expect_err("same name, different bytes");
        assert!(error.contains("같은 이름"), "{error}");
        assert_eq!(doc, before);

        // Same name and the same bytes: shared, not duplicated.
        let (mut doc, at) = target();
        doc.bin_streams
            .push(first.bin_streams.last().unwrap().clone());
        let streams = doc.bin_streams.len();
        apply(&mut doc, &at, &request("assets/logo.png"), false, &ctx).unwrap();
        assert_eq!(doc.bin_streams.len(), streams);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #446 made `Document::bin_stream_index` manifest-first: an id the package manifest lists
    /// resolves to its href or to nothing, whatever a stream of the same name holds. An inserted
    /// picture names its stream directly, so a target manifest already listing that name for
    /// another href would send the picture to the wrong bytes (or none).
    #[test]
    fn inserted_picture_resolves_to_its_own_stream_when_the_target_manifest_lists_the_same_name() {
        let dir = scratch("manifest");
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let ctx = AssetContext {
            base_dir: Some(&canonical),
            roots: &[],
        };
        let (mut probe, at) = target();
        apply(&mut probe, &at, &request("assets/logo.png"), false, &ctx).unwrap();
        let name = probe.bin_streams.last().unwrap().name.clone();

        let (mut doc, at) = target();
        doc.bin_streams.push(hwp_model::BinStream {
            name: "BinData/other.png".into(),
            data: b"someone else's bytes".to_vec(),
        });
        doc.hwpx_bin_manifest = vec![(name.clone(), "BinData/other.png".into())];
        // One picture at the top level and one inside a table cell: the rename reaches both.
        let nested: InsertBlocks = serde_json::from_value(serde_json::json!({
            "position": "after",
            "blocks": [
                {"type": "image", "path": "assets/logo.png", "width_mm": 10},
                {"type": "table", "columns": [{"width_mm": 30}], "rows": [{"cells": [
                    {"blocks": [{"type": "image", "path": "assets/logo.png", "width_mm": 10}]}
                ]}]}
            ]
        }))
        .unwrap();
        apply(&mut doc, &at, &nested, false, &ctx).unwrap();

        let mut pictures = Vec::new();
        for paragraph in &doc.sections[0].paragraphs {
            for control in &paragraph.controls {
                match control {
                    Control::Picture(p) => pictures.push(p),
                    Control::Table(t) => pictures.extend(
                        t.cells
                            .iter()
                            .flat_map(|c| &c.paragraphs)
                            .flat_map(|p| &p.controls)
                            .filter_map(|c| match c {
                                Control::Picture(p) => Some(p),
                                _ => None,
                            }),
                    ),
                    _ => {}
                }
            }
        }
        assert_eq!(pictures.len(), 2, "top-level and in-cell picture");
        for picture in pictures {
            assert_eq!(
                doc.resolve_bin(&picture.bin_ref),
                Some(PNG),
                "the picture must resolve to the inserted PNG, not the stream the manifest maps {name} to"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn set_page(doc: &mut Document, page: hwp_model::PageDef) {
        for control in doc.sections[0]
            .paragraphs
            .iter_mut()
            .flat_map(|p| &mut p.controls)
        {
            if let Control::SectionDef(def) = control {
                def.page = Some(page);
            }
        }
    }

    fn a4_page(width: i32, height: i32, attr: u32) -> hwp_model::PageDef {
        hwp_model::PageDef {
            width: hwp_model::HwpUnit(width),
            height: hwp_model::HwpUnit(height),
            margin_left: hwp_model::HwpUnit(5669),
            margin_right: hwp_model::HwpUnit(5669),
            margin_top: hwp_model::HwpUnit(5669),
            margin_bottom: hwp_model::HwpUnit(5669),
            margin_header: hwp_model::HwpUnit(2835),
            margin_footer: hwp_model::HwpUnit(2835),
            gutter: hwp_model::HwpUnit(0),
            attr,
        }
    }

    /// `hwp-render` reads PAGE_DEF attr bit 0 as "rotate the paper": the horizontal extent is
    /// then `height`, not `width`. Genuine landscape files store the portrait width and height
    /// with the bit set, so a landscape form is 297 mm wide and its body 257 mm.
    #[test]
    fn insertion_width_follows_the_renderers_page_orientation() {
        let at = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let width_with = |page: hwp_model::PageDef| {
            let mut doc = hwp_convert::from_markdown("anchor\n");
            set_page(&mut doc, page);
            check_target(&doc, &at, false).expect("body paragraph target")
        };
        let portrait = width_with(a4_page(59528, 84189, 0));
        assert!(
            (portrait - 170.0).abs() < 1.0,
            "portrait body {portrait} mm"
        );
        // Genuine landscape: portrait dimensions, rotate bit set. The renderer's paper is 297 mm.
        // A binding gutter comes off the body in either orientation: 10 mm here.
        let mut bound = a4_page(59528, 84189, 1);
        bound.gutter = hwp_model::HwpUnit(2835);
        let bound = width_with(bound);
        assert!(
            (bound - 247.0).abs() < 1.0,
            "a 10 mm gutter leaves 257 - 10 = 247 mm, got {bound} mm"
        );
        let landscape = width_with(a4_page(59528, 84189, 1));
        assert!(
            (landscape - 257.0).abs() < 1.0,
            "a landscape body is 297 - 2 * 20 = 257 mm, got {landscape} mm"
        );
    }

    fn column_control(count: u16) -> Control {
        Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"cold",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: Some(hwp_model::ColumnDef {
                count,
                kind: 0,
                direction: 0,
                same_width: true,
                gap: 2835,
                widths: Vec::new(),
                divider: None,
            }),
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    }

    /// A section can change its column layout partway: the layout that applies to a block is
    /// the last column definition at or before the paragraph it lands next to.
    #[test]
    fn insertion_width_uses_the_column_definition_in_effect_at_the_target() {
        let mut doc = hwp_convert::from_markdown("first\n\nsecond\n\nthird\n");
        assert!(doc.sections[0].paragraphs.len() >= 3);
        doc.sections[0].paragraphs[1]
            .controls
            .push(column_control(2));
        let width_at = |doc: &Document, paragraph: usize| {
            check_target(
                doc,
                &SegmentPath {
                    section: 0,
                    indices: vec![paragraph],
                },
                false,
            )
            .unwrap()
        };
        let before = width_at(&doc, 0);
        let at_change = width_at(&doc, 1);
        let after = width_at(&doc, 2);
        assert!(before > 150.0, "before the change: one column, {before} mm");
        assert!(
            at_change < 90.0,
            "at the change: two columns, {at_change} mm"
        );
        assert!(after < 90.0, "after the change: two columns, {after} mm");
        // A later single-column definition ends the two-column run.
        doc.sections[0].paragraphs[2]
            .controls
            .push(column_control(1));
        let back = width_at(&doc, 2);
        assert!(back > 150.0, "back to one column, {back} mm");
    }

    fn picture_name(dir: &Path) -> String {
        let canonical = std::fs::canonicalize(dir).unwrap();
        let ctx = AssetContext {
            base_dir: Some(&canonical),
            roots: &[],
        };
        let (mut probe, at) = target();
        apply(&mut probe, &at, &request("assets/logo.png"), false, &ctx).unwrap();
        probe.bin_streams.last().unwrap().name.clone()
    }

    /// The resolver also matches a stream whose path ends in `/<name>`. A target already
    /// carrying the same image under a folder must not get a second copy.
    #[test]
    fn an_identical_stream_the_resolver_already_finds_by_suffix_is_shared() {
        let dir = scratch("suffix-same");
        let name = picture_name(&dir);
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let ctx = AssetContext {
            base_dir: Some(&canonical),
            roots: &[],
        };
        let (mut doc, at) = target();
        doc.bin_streams.push(hwp_model::BinStream {
            name: format!("BinData/{name}"),
            data: PNG.to_vec(),
        });
        apply(&mut doc, &at, &request("assets/logo.png"), false, &ctx).unwrap();
        assert_eq!(
            doc.bin_streams.len(),
            1,
            "the image is already stored as BinData/{name}: {:?}",
            doc.bin_streams.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let picture = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(doc.resolve_bin(&picture.bin_ref), Some(PNG));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same suffix match with different bytes would send the new picture to the old file.
    #[test]
    fn a_different_stream_the_resolver_would_find_by_suffix_forces_a_fresh_name() {
        let dir = scratch("suffix-other");
        let name = picture_name(&dir);
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let ctx = AssetContext {
            base_dir: Some(&canonical),
            roots: &[],
        };
        let (mut doc, at) = target();
        doc.bin_streams.push(hwp_model::BinStream {
            name: format!("BinData/{name}"),
            data: b"another image entirely".to_vec(),
        });
        apply(&mut doc, &at, &request("assets/logo.png"), false, &ctx).unwrap();
        let picture = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            doc.resolve_bin(&picture.bin_ref),
            Some(PNG),
            "the picture resolved to the older stream that shares its file name"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `position: before` puts the blocks ahead of the target paragraph, and with them ahead of
    /// any column control that paragraph carries, so they flow under the PREVIOUS layout.
    /// `after` lands under the target's own.
    #[test]
    fn a_before_insertion_is_measured_against_the_layout_preceding_the_target() {
        let width = |doc: &Document, paragraph: usize, before: bool| {
            check_target(
                doc,
                &SegmentPath {
                    section: 0,
                    indices: vec![paragraph],
                },
                before,
            )
            .unwrap()
        };
        // One column, then a change to two columns carried by paragraph 1.
        let mut to_two = hwp_convert::from_markdown("first\n\nsecond\n\nthird\n");
        to_two.sections[0].paragraphs[1]
            .controls
            .push(column_control(2));
        assert!(
            width(&to_two, 1, true) > 150.0,
            "before the change: the one-column layout"
        );
        assert!(
            width(&to_two, 1, false) < 90.0,
            "after the change paragraph: two columns"
        );
        // Two columns, then a change back to one carried by paragraph 2.
        let mut to_one = hwp_convert::from_markdown("first\n\nsecond\n\nthird\n");
        to_one.sections[0].paragraphs[1]
            .controls
            .push(column_control(2));
        to_one.sections[0].paragraphs[2]
            .controls
            .push(column_control(1));
        assert!(
            width(&to_one, 2, true) < 90.0,
            "before the change back: still two columns"
        );
        assert!(width(&to_one, 2, false) > 150.0, "after it: one column");
        // The first paragraph has no preceding layout of its own, so `before` falls back to the
        // section default rather than reading a definition that follows the block.
        assert!(width(&to_two, 0, true) > 150.0);
    }

    fn insert_table_request(position: &str, width_mm: u32) -> InsertBlocks {
        serde_json::from_value(serde_json::json!({
            "position": position,
            "blocks": [{"type": "table", "columns": [{"width_mm": width_mm}],
                "rows": [{"cells": [{"blocks": [{"type": "paragraph", "runs": [
                    {"type": "text", "text": "x"}]}]}]}]}]
        }))
        .expect("request")
    }

    /// The same check through `apply`, so the position the caller passes is the one measured.
    #[test]
    fn apply_passes_the_position_to_the_width_check() {
        let make = || {
            let mut doc = hwp_convert::from_markdown("first\n\nsecond\n\nthird\n");
            doc.sections[0].paragraphs[1]
                .controls
                .push(column_control(2));
            doc
        };
        let at = SegmentPath {
            section: 0,
            indices: vec![1],
        };
        let none = AssetContext {
            base_dir: None,
            roots: &[],
        };
        // Before the 2-column change a 120 mm table sits in the 170 mm one-column layout.
        let mut doc = make();
        apply(
            &mut doc,
            &at,
            &insert_table_request("before", 120),
            false,
            &none,
        )
        .expect("fits the preceding one-column layout");
        // After the paragraph that starts two columns it does not.
        let mut doc = make();
        let before = doc.clone();
        let error = apply(
            &mut doc,
            &at,
            &insert_table_request("after", 120),
            false,
            &none,
        )
        .expect_err("120 mm exceeds an 80 mm column");
        assert!(error.contains("layout_overflow"), "{error}");
        assert_eq!(doc, before);
    }

    /// A document whose first table has one cell of the given width and margins, and whose table
    /// states the given inner margins; the cell's first paragraph is `[0, control, 0, 0]`.
    fn doc_with_cell(
        width: i32,
        cell_margins: [u16; 4],
        inner: [u16; 4],
    ) -> (Document, SegmentPath) {
        let mut doc = hwp_convert::from_markdown("| a |\n|---|\n| b |\n");
        let (section_index, paragraph_index, control_index) = doc
            .sections
            .iter()
            .enumerate()
            .find_map(|(si, section)| {
                section.paragraphs.iter().enumerate().find_map(|(pi, p)| {
                    p.controls
                        .iter()
                        .position(|c| matches!(c, Control::Table(_)))
                        .map(|ci| (si, pi, ci))
                })
            })
            .expect("a table");
        let Control::Table(table) =
            &mut doc.sections[section_index].paragraphs[paragraph_index].controls[control_index]
        else {
            unreachable!()
        };
        table.inner_margins = inner;
        let cell = &mut table.cells[0];
        cell.width = hwp_model::HwpUnit(width);
        cell.margins = cell_margins;
        (
            doc,
            SegmentPath {
                section: section_index,
                indices: vec![paragraph_index, control_index, 0, 0],
            },
        )
    }

    /// 0xffff in a cell margin means "use the table's"; subtracting it raw gives a width of about
    /// -422 mm and refuses every insertion, text included.
    #[test]
    fn an_inherited_cell_margin_is_resolved_not_subtracted_raw() {
        let max = u16::MAX;
        // A 10000-unit (about 35.3 mm) cell inheriting 700/800 side margins from its table.
        let (doc, at) = doc_with_cell(10000, [max, max, max, max], [700, 800, 900, 1000]);
        let width = check_target(&doc, &at, false).expect("an inherited margin is a valid cell");
        let expected = hwpunit_to_mm(10000 - 700 - 800);
        assert!(
            (width - expected).abs() < 0.01,
            "{width} mm, expected {expected} mm"
        );
        // One side inherited, the other the cell's own.
        let (doc, at) = doc_with_cell(10000, [max, 300, 1, 1], [700, 800, 900, 1000]);
        let width = check_target(&doc, &at, false).unwrap();
        let expected = hwpunit_to_mm(10000 - 700 - 300);
        assert!(
            (width - expected).abs() < 0.01,
            "{width} mm, expected {expected} mm"
        );
        // A table that states nothing inherits Hangul's default.
        let (doc, at) = doc_with_cell(10000, [max, max, max, max], [0; 4]);
        let width = check_target(&doc, &at, false).unwrap();
        let expected = hwpunit_to_mm(10000 - 510 - 510);
        assert!(
            (width - expected).abs() < 0.01,
            "{width} mm, expected {expected} mm"
        );
    }

    /// An all-zero margin array is the IR's "omitted" marker: the table's (or the default)
    /// applies, so the width is smaller than the cell, not equal to it.
    #[test]
    fn omitted_cell_margins_take_the_table_and_default_margins() {
        let (doc, at) = doc_with_cell(10000, [0; 4], [0; 4]);
        let width = check_target(&doc, &at, false).unwrap();
        let expected = hwpunit_to_mm(10000 - 510 - 510);
        assert!(
            (width - expected).abs() < 0.01,
            "{width} mm, expected {expected} mm"
        );
        let (doc, at) = doc_with_cell(10000, [0; 4], [600, 650, 1, 1]);
        let width = check_target(&doc, &at, false).unwrap();
        let expected = hwpunit_to_mm(10000 - 600 - 650);
        assert!(
            (width - expected).abs() < 0.01,
            "{width} mm, expected {expected} mm"
        );
    }

    const UNKNOWN_WIDTH: &str = "본문 폭을 알 수 없습니다";

    /// Document-supplied geometry is untrusted: the HWP5 reader accepts any i32 for a cell or
    /// page width, so the width computation must refuse values it cannot subtract safely
    /// instead of panicking (overflow-checked builds) or wrapping to a huge width (release).
    #[test]
    fn hostile_cell_geometry_is_refused_not_overflowed() {
        for width in [i32::MIN, i32::MIN + 1, -1, 0, 1000] {
            let (doc, at) = doc_with_cell(width, [0; 4], [0; 4]);
            let error = check_target(&doc, &at, false)
                .expect_err("a cell narrower than its margins has no usable width");
            assert!(error.contains(UNKNOWN_WIDTH), "{width}: {error}");
        }
        // The widest cell is fine and does not overflow either.
        let (doc, at) = doc_with_cell(i32::MAX, [0; 4], [0; 4]);
        assert!(check_target(&doc, &at, false).unwrap() > 1.0);
    }

    #[test]
    fn hostile_page_geometry_is_refused_not_overflowed() {
        let at = SegmentPath {
            section: 0,
            indices: vec![0],
        };
        let width_of = |page: hwp_model::PageDef| {
            let mut doc = hwp_convert::from_markdown("anchor\n");
            set_page(&mut doc, page);
            check_target(&doc, &at, false)
        };
        let page = |width: i32, height: i32, left: i32, right: i32, gutter: i32, attr: u32| {
            let mut page = a4_page(width, height, attr);
            page.margin_left = hwp_model::HwpUnit(left);
            page.margin_right = hwp_model::HwpUnit(right);
            page.gutter = hwp_model::HwpUnit(gutter);
            page
        };
        for (label, hostile) in [
            ("paper at i32::MIN", page(i32::MIN, 0, 0, 0, 0, 0)),
            ("rotated paper at i32::MIN", page(0, i32::MIN, 0, 0, 0, 1)),
            (
                "margin pair overflowing",
                page(59528, 84189, i32::MAX, i32::MAX, 0, 0),
            ),
            ("negative margin", page(59528, 84189, -1, 5669, 0, 0)),
            ("negative gutter", page(59528, 84189, 5669, 5669, -1, 0)),
            (
                "negative margin at i32::MIN",
                page(59528, 84189, i32::MIN, 5669, 0, 0),
            ),
            (
                "margins wider than the paper",
                page(59528, 84189, 40000, 40000, 0, 0),
            ),
        ] {
            let error = width_of(hostile).expect_err(label);
            assert!(error.contains(UNKNOWN_WIDTH), "{label}: {error}");
        }
    }

    /// A column gap is read from a signed field; a negative one has no meaning and must not
    /// widen the columns or reach the multiplication unchecked.
    #[test]
    fn hostile_column_geometry_is_refused_not_overflowed() {
        let column = |count: u16, gap: i32, same_width: bool, widths: Vec<i32>| {
            let mut doc = hwp_convert::from_markdown("first\n\nsecond\n");
            doc.sections[0].paragraphs[1]
                .controls
                .push(Control::Generic(hwp_model::GenericControl {
                    ctrl_id: *b"cold",
                    data: Vec::new(),
                    paragraph_lists: Vec::new(),
                    extras: Vec::new(),
                    raw_children: Vec::new(),
                    gso_shapes: Vec::new(),
                    equation: None,
                    column_def: Some(hwp_model::ColumnDef {
                        count,
                        kind: 0,
                        direction: 0,
                        same_width,
                        gap,
                        widths,
                        divider: None,
                    }),
                    caption: None,
                    hwpx_raw_xml: None,
                    container_box: None,
                }));
            check_target(
                &doc,
                &SegmentPath {
                    section: 0,
                    indices: vec![1],
                },
                false,
            )
        };
        for (label, result) in [
            ("negative gap", column(2, -2835, true, Vec::new())),
            ("gap at i32::MIN", column(2, i32::MIN, true, Vec::new())),
            ("gap at i32::MAX", column(65535, i32::MAX, true, Vec::new())),
            (
                "gaps wider than the body",
                column(255, 100_000, true, Vec::new()),
            ),
        ] {
            let error = result.expect_err(label);
            assert!(error.contains(UNKNOWN_WIDTH), "{label}: {error}");
        }
        // Valid and degenerate layouts keep working: a stated width list, a zero count that
        // means one column, and a single column with a nonsensical gap that is never used.
        assert!(column(2, 2835, false, vec![i32::MIN, 20000, 30000]).unwrap() > 50.0);
        assert!(column(0, i32::MIN, true, Vec::new()).unwrap() > 150.0);
        assert!(column(1, -5, true, Vec::new()).unwrap() > 150.0);
    }

    /// The HWP5 writer finds a stream by file name ignoring ASCII case and by substring. A
    /// target holding the inserted image's name in other letters, with other bytes, would
    /// capture the picture there, so the stream must be renamed, and the written .hwp must
    /// resolve the picture to its own bytes.
    #[test]
    fn a_case_only_name_collision_with_other_bytes_does_not_capture_the_picture_in_hwp() {
        let dir = scratch("case-collision");
        let name = picture_name(&dir);
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let ctx = AssetContext {
            base_dir: Some(&canonical),
            roots: &[],
        };
        let (mut doc, at) = target();
        doc.bin_streams.push(hwp_model::BinStream {
            name: format!("BinData/{}", name.to_ascii_uppercase()),
            data: b"another image entirely".to_vec(),
        });
        apply(&mut doc, &at, &request("assets/logo.png"), true, &ctx).unwrap();
        let output = dir.join("out.hwp");
        hwp5::write_document(&doc, &output, &hwp5::write::WriteOptions::default()).unwrap();
        let reread = hwp5::read_document(&output).unwrap().document;
        let picture = reread.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("the picture survives the HWP write");
        assert_eq!(
            reread.resolve_bin(&picture.bin_ref),
            Some(PNG),
            "the written .hwp resolved the picture to the stream that differs only in case"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
