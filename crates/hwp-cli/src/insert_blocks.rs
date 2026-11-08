//! Text-block insertion using the existing DocumentSpec compiler against target header tables.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use hwp_convert::SegmentPath;
use hwp_convert::styles::{StyleKind, list_named_styles};
use hwp_model::{BorderFillId, Control, Document, StyleId};
use serde::Deserialize;

use crate::document_spec::{
    BlockCompiler, BlockDefaults, BlockSpec, BreakKind, DocumentSpec, ListSpec, MetadataSpec,
    PageSpec, RunSpec, SectionSpec, SpecVersion, StyleSpec, validate_spec,
};

#[derive(Debug, Clone, Copy, Deserialize, Default)]
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

fn unsupported(kind: &str) -> String {
    format!(
        "insert_blocks는 현재 paragraph/text/line_break 및 page/column break만 지원합니다 ({kind}). 표/이미지/필드/수식은 #428 T3 후속 작업이며, 지금은 지원하는 텍스트 블록으로 나누어 요청하세요"
    )
}

fn referenced_styles(request: &InsertBlocks) -> Result<BTreeSet<String>, String> {
    if request.blocks.is_empty() || request.blocks.len() > crate::document_spec::MAX_BLOCKS {
        return Err("insert_blocks의 blocks 개수가 제한을 벗어났습니다. 1..=20000개 블록으로 나누어 요청하세요".into());
    }
    let mut names = BTreeSet::new();
    let mut lists = BTreeSet::new();
    for block in &request.blocks {
        match block {
            BlockSpec::Paragraph {
                style, list, runs, ..
            } => {
                names.extend(style.iter().cloned());
                if let Some(list) = list {
                    lists.insert(list.name.clone());
                }
                for run in runs {
                    match run {
                        RunSpec::Text { style, .. } => names.extend(style.iter().cloned()),
                        RunSpec::LineBreak => {}
                        RunSpec::Image { .. } => return Err(unsupported("image run")),
                        RunSpec::Equation { .. } => return Err(unsupported("equation run")),
                        RunSpec::Field { .. } => return Err(unsupported("field run")),
                    }
                }
            }
            BlockSpec::Break {
                kind: BreakKind::Page | BreakKind::Column,
            } => {}
            BlockSpec::Break {
                kind: BreakKind::Section,
            } => return Err(unsupported("section break")),
            BlockSpec::Table { .. } => return Err(unsupported("table")),
            BlockSpec::Image { .. } => return Err(unsupported("image")),
            BlockSpec::Equation { .. } => return Err(unsupported("equation")),
            BlockSpec::Field { .. } => return Err(unsupported("field")),
        }
    }
    for name in lists {
        let list = request.lists.get(&name).ok_or_else(|| format!(
            "목록 '{name}'의 lists 정의가 없습니다. lists에 DocumentSpec 목록 정의를 추가한 뒤 다시 시도하세요"
        ))?;
        for level in &list.levels {
            names.extend(level.style.iter().cloned());
        }
    }
    Ok(names)
}

/// Limit this phase to the body and table cells. Opaque Generic objects need a writer
/// regeneration proof, not merely a readable address into their extracted model lists.
fn check_target(doc: &Document, target: &SegmentPath) -> Result<(), String> {
    let invalid = || {
        "insert_blocks 삽입 위치가 본문 또는 표 셀 문단이 아닙니다. 최신 문서에서 본문/표 셀 문단 주소를 다시 선택하세요".to_owned()
    };
    let section = doc.sections.get(target.section).ok_or_else(invalid)?;
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
        paragraph = table
            .cells
            .get(*cell)
            .and_then(|cell| cell.paragraphs.get(*para))
            .ok_or_else(invalid)?;
        rest = tail;
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

/// Compile on a candidate and publish its header/content only after every check succeeds.
/// `max_style` is 255 for HWP output and 65535 for HWPX output.
pub fn apply(
    doc: &mut Document,
    target: &SegmentPath,
    request: &InsertBlocks,
    max_style: u16,
) -> Result<usize, String> {
    check_target(doc, target)?;
    if request.styles.len() > 256 || request.lists.len() > 256 {
        return Err(
            "insert_blocks의 styles/lists 정의가 상한 256개를 넘습니다. 정의를 나누어 요청하세요"
                .into(),
        );
    }
    let names = referenced_styles(request)?;
    let styles = list_named_styles(doc, &[]).map_err(|error| error.to_string())?;
    let default = doc.header.styles.first().ok_or_else(|| "대상 문서의 기본 스타일 0이 없습니다. hwp styles로 유효한 문서인지 확인한 뒤 다시 시도하세요".to_owned())?;
    if styles.first().and_then(|style| style.kind) != Some(StyleKind::Paragraph) {
        return Err("기본 스타일 0의 종류가 paragraph가 아닙니다. 문단 기본 스타일이 있는 원본 문서를 선택하세요".into());
    }
    let defaults = BlockDefaults {
        style: StyleId(0),
        para_shape: default.para_shape,
        char_shape: default.char_shape,
        border_fill: BorderFillId(0),
    };
    let mut compiler = BlockCompiler::over_document(doc.clone(), defaults, Path::new("."), &[]);
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
    let lists: BTreeMap<_, _> = request
        .blocks
        .iter()
        .filter_map(|block| match block {
            BlockSpec::Paragraph {
                list: Some(list), ..
            } => Some(list.name.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| {
            let definition = request.lists[&name].clone();
            (name, definition)
        })
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
    validate_spec(&validation, Path::new(".")).map_err(|error| format!("insert_blocks 블록/스타일/목록 정의가 유효하지 않습니다: {error}. DocumentSpec v1 정의를 수정한 뒤 다시 시도하세요"))?;
    compiler
        .compile_lists(&lists)
        .map_err(|error| format!("목록 컴파일 실패: {error}. lists 정의와 원본 번호/글머리표 표 크기를 확인한 뒤 다시 시도하세요"))?;
    let mut paragraphs = Vec::new();
    for (index, block) in request.blocks.iter().enumerate() {
        paragraphs.extend(compiler.compile_block(block, &format!("$.blocks[{index}]")).map_err(|error| format!("insert_blocks 컴파일 실패: {error}. DocumentSpec v1 정의를 확인한 뒤 다시 시도하세요"))?);
    }
    let count = paragraphs.len();
    let mut candidate = compiler.into_document();
    hwp_convert::structure::insert_paragraphs_at(
        &mut candidate,
        target,
        request.before(),
        paragraphs,
    )?;
    *doc = candidate;
    Ok(count)
}
