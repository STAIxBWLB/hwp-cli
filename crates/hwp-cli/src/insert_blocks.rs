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

fn hwpunit_to_mm(units: i32) -> f32 {
    (f64::from(units) * 25.4 / f64::from(hwp_model::HwpUnit::PER_INCH)) as f32
}

/// Limit this phase to the body and table cells, and return the width in mm of the text column
/// or table cell the blocks will land in. Opaque Generic objects need a writer regeneration
/// proof, not merely a readable address into their extracted model lists.
fn check_target(doc: &Document, target: &SegmentPath) -> Result<f32, String> {
    let invalid = || {
        "insert_blocks 삽입 위치가 본문 또는 표 셀 문단이 아닙니다. 최신 문서에서 본문/표 셀 문단 주소를 다시 선택하세요".to_owned()
    };
    let section = doc.sections.get(target.section).ok_or_else(invalid)?;
    let mut width =
        section
            .section_def()
            .and_then(|def| def.page)
            .map_or(DEFAULT_CONTAINER_MM, |page| {
                hwpunit_to_mm(
                    page.width.0 - page.margin_left.0 - page.margin_right.0 - page.gutter.0,
                )
            });
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
        width =
            hwpunit_to_mm(cell.width.0 - i32::from(cell.margins[0]) - i32::from(cell.margins[1]));
        paragraph = cell.paragraphs.get(*para).ok_or_else(invalid)?;
        rest = tail;
    }
    if !width.is_finite() || width <= 0.0 {
        return Err(
            "삽입 위치의 본문 폭을 알 수 없습니다. 표 셀 폭과 여백이 유효한 문서인지 확인하세요"
                .into(),
        );
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
            let used = request.blocks.iter().any(|block| {
                matches!(block,
                    BlockSpec::Paragraph { list: Some(reference), .. }
                        if reference.name == *name && usize::from(reference.level) == index
                )
            });
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
    let container_mm = check_target(doc, target)?;
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
    let mut shell = Document {
        header: doc.header.clone(),
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
    hwp_convert::document_merge::renumber_inserted_objects(doc, &mut paragraphs)?;
    // The only fallible step that touches the target: it validates before it splices, so a
    // refusal leaves `doc` exactly as it was.
    hwp_convert::structure::insert_paragraphs_at(doc, target, request.before(), paragraphs)?;
    doc.header = compiled.header;
    for stream in compiled.bin_streams {
        if !doc.bin_streams.iter().any(|have| have.name == stream.name) {
            doc.bin_streams.push(stream);
        }
    }
    Ok(count)
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
        apply(&mut doc, &at, &request("assets/logo.png"), false, &ctx).unwrap();

        let picture = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("inserted picture");
        assert_eq!(
            doc.resolve_bin(&picture.bin_ref),
            Some(PNG),
            "the picture must resolve to the inserted PNG, not the stream the manifest maps {name} to"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
