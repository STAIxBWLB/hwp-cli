//! Copy-on-write named-style creation and paragraph-scoped application (#427 T3).
//! Native publication guards and format-specific style limits belong to callers.
use hwp_model::{
    CharShapeId, Control, DocHeader, Document, LANG_COUNT, ParaShapeId, Paragraph, StyleId,
};

use super::StyleKind;
use crate::format::{
    CharFormat, ParaProps, apply_format, apply_para_shape_props, find_or_insert,
    find_or_insert_para, resolve_font_faces, to_ir_units,
};
use crate::segment_id::SegmentPath;

const MAX_ROWS: usize = u16::MAX as usize + 1;

/// An exact primary style name or an IR style-table index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleSelector {
    Name(String),
    Id(u16),
}

impl std::fmt::Display for StyleSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(name) => f.write_str(name),
            Self::Id(id) => write!(f, "#{id}"),
        }
    }
}

/// A named-style request failed without changing the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleEditError {
    InvalidName,
    DuplicateName(String),
    UnknownStyle(StyleSelector),
    AmbiguousName(String),
    MissingShape {
        table: &'static str,
        id: u16,
    },
    Capacity {
        table: &'static str,
    },
    InvalidAddress,
    UnsupportedKind(u8),
    UnknownStyleKind,
    CharacterStylesUnsupported,
    InvalidCharacterProperties,
    UnwritableStyleUser {
        count: usize,
        locations: Vec<String>,
    },
}

impl std::fmt::Display for StyleEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName => write!(
                f,
                "스타일 이름이 비었거나 제어 문자를 포함하거나 UTF-16 길이 제한을 넘습니다. 유효한 이름을 지정해 보세요"
            ),
            Self::DuplicateName(name) => write!(
                f,
                "같은 이름의 스타일 '{name}'이 이미 있습니다. 다른 이름을 지정해 보세요"
            ),
            Self::UnknownStyle(selector) => write!(
                f,
                "스타일 {selector}을 찾을 수 없습니다. 문서의 스타일 이름이나 번호를 확인해 보세요"
            ),
            Self::AmbiguousName(name) => write!(
                f,
                "스타일 이름 '{name}'이 여러 항목과 일치합니다. 스타일 번호를 지정해 보세요"
            ),
            Self::MissingShape { table, id } => write!(
                f,
                "스타일의 {table} 참조 {id}를 찾을 수 없습니다. 원본 문서의 서식 참조를 확인해 보세요"
            ),
            Self::Capacity { table } => write!(
                f,
                "{table} 표가 스타일 편집의 번호 범위를 넘습니다. 문서의 서식 정의 수를 확인해 보세요"
            ),
            Self::InvalidAddress => write!(
                f,
                "스타일을 적용할 문단 주소를 찾을 수 없습니다. 최신 문서에서 문단 주소를 다시 얻어 보세요"
            ),
            Self::CharacterStylesUnsupported => write!(
                f,
                "문자 스타일 편집은 현재 지원하지 않습니다. hwp styles에서 문단 스타일을 선택하세요. 문자 스타일 편집은 #427 후속 작업입니다(HWPX 스타일 종류 보존: PR #442)"
            ),
            Self::UnknownStyleKind => write!(
                f,
                "원본에 스타일 종류가 명시되지 않아 문단 스타일로 적용할 수 없습니다. hwp styles에서 kind가 paragraph인 스타일을 선택하세요"
            ),
            Self::UnsupportedKind(kind) => write!(
                f,
                "종류 {kind}인 스타일은 문단에 적용할 수 없습니다. 문단 스타일을 지정해 보세요"
            ),
            Self::UnwritableStyleUser { count, locations } => {
                write!(
                    f,
                    "원본 레코드 보존 개체 안에서 변경이 필요한 문단 {count}개를 찾았습니다. 위치: {}",
                    locations.join("; ")
                )?;
                if *count > locations.len() {
                    write!(f, " (위치 {}/{count}개 표시)", locations.len())?;
                }
                write!(
                    f,
                    ". hwp styles로 확인한 다른 스타일을 선택하거나 원본 편집기에서 해당 개체를 일반 문단으로 바꾼 뒤 다시 시도하세요"
                )
            }
            Self::InvalidCharacterProperties => write!(
                f,
                "스타일의 글자 크기나 글꼴 이름이 유효하지 않습니다. 유한한 글자 크기와 유효한 글꼴 이름을 지정해 보세요"
            ),
        }
    }
}

impl std::error::Error for StyleEditError {}

impl StyleSelector {
    pub fn resolve(&self, doc: &Document) -> Result<StyleId, StyleEditError> {
        check_rows(doc.header.styles.len(), "styles")?;
        match self {
            Self::Id(id) => doc
                .header
                .styles
                .get(usize::from(*id))
                .map(|_| StyleId(*id))
                .ok_or_else(|| StyleEditError::UnknownStyle(self.clone())),
            Self::Name(name) => {
                let mut matches = doc
                    .header
                    .styles
                    .iter()
                    .enumerate()
                    .filter(|(_, style)| style.name == *name);
                let (index, _) = matches
                    .next()
                    .ok_or_else(|| StyleEditError::UnknownStyle(self.clone()))?;
                if matches.next().is_some() {
                    return Err(StyleEditError::AmbiguousName(name.clone()));
                }
                Ok(StyleId(u16::try_from(index).map_err(|_| {
                    StyleEditError::Capacity { table: "styles" }
                })?))
            }
        }
    }
}

/// Clone the selected base (default: style zero), preserving its metadata and tails.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateStyle {
    pub name: String,
    pub based_on: Option<StyleSelector>,
    pub kind: StyleKind,
    pub character: CharFormat,
    /// Dimensions use the same HWPUNIT input convention as `set_para_props`.
    pub paragraph: ParaProps,
}

fn check_rows(rows: usize, table: &'static str) -> Result<(), StyleEditError> {
    if rows > MAX_ROWS {
        Err(StyleEditError::Capacity { table })
    } else {
        Ok(())
    }
}

fn check_insert<T: PartialEq>(
    rows: &[T],
    value: &T,
    table: &'static str,
) -> Result<(), StyleEditError> {
    check_rows(rows.len(), table)?;
    if rows.len() == MAX_ROWS && !rows.contains(value) {
        return Err(StyleEditError::Capacity { table });
    }
    Ok(())
}

fn check_shapes(doc: &Document, style: &hwp_model::Style) -> Result<(), StyleEditError> {
    check_rows(doc.header.char_shapes.len(), "char_shapes")?;
    check_rows(doc.header.para_shapes.len(), "para_shapes")?;
    if doc
        .header
        .char_shapes
        .get(usize::from(style.char_shape.0))
        .is_none()
    {
        return Err(StyleEditError::MissingShape {
            table: "char_shapes",
            id: style.char_shape.0,
        });
    }
    if doc
        .header
        .para_shapes
        .get(usize::from(style.para_shape.0))
        .is_none()
    {
        return Err(StyleEditError::MissingShape {
            table: "para_shapes",
            id: style.para_shape.0,
        });
    }
    Ok(())
}

fn resolve_style_font(
    header: &mut DocHeader,
    format: &CharFormat,
) -> Result<Option<[u16; LANG_COUNT]>, StyleEditError> {
    if format
        .size_pt
        .is_some_and(|size| !size.is_finite() || size * 100.0 > i32::MAX as f32)
        || format
            .font
            .as_ref()
            .is_some_and(|name| name.is_empty() || name.chars().any(char::is_control))
    {
        return Err(StyleEditError::InvalidCharacterProperties);
    }
    let Some(name) = format.font.as_deref() else {
        return Ok(None);
    };
    for fonts in &header.fonts {
        check_rows(fonts.len(), "fonts")?;
        if fonts.len() == MAX_ROWS && !fonts.iter().any(|face| face.name == name) {
            return Err(StyleEditError::Capacity { table: "fonts" });
        }
    }
    Ok(Some(resolve_font_faces(&mut header.fonts, name)))
}

fn require_paragraph_style(style: &hwp_model::Style) -> Result<(), StyleEditError> {
    match super::style_kind(style) {
        Some(StyleKind::Paragraph) => Ok(()),
        Some(StyleKind::Character) => Err(StyleEditError::CharacterStylesUnsupported),
        None if style.kind_unknown => Err(StyleEditError::UnknownStyleKind),
        None => Err(StyleEditError::UnsupportedKind(style.attr & 0x7)),
    }
}

/// Append one paragraph style, reusing equal shapes or appending changed copies. All fallible
/// work happens on a header candidate, leaving every original reference untouched.
pub fn create_style(doc: &mut Document, request: &CreateStyle) -> Result<StyleId, StyleEditError> {
    if request.kind == StyleKind::Character {
        return Err(StyleEditError::CharacterStylesUnsupported);
    }
    if request.name.is_empty()
        || request.name.chars().any(char::is_control)
        || request.name.encode_utf16().count() > usize::from(u16::MAX)
    {
        return Err(StyleEditError::InvalidName);
    }
    if doc
        .header
        .styles
        .iter()
        .any(|style| style.name == request.name)
    {
        return Err(StyleEditError::DuplicateName(request.name.clone()));
    }
    let id = u16::try_from(doc.header.styles.len())
        .map_err(|_| StyleEditError::Capacity { table: "styles" })?;
    let base_id = request
        .based_on
        .as_ref()
        .unwrap_or(&StyleSelector::Id(0))
        .resolve(doc)?;
    let mut style = doc.header.styles[usize::from(base_id.0)].clone();
    check_shapes(doc, &style)?;
    let mut header = doc.header.clone();
    let resolved_font = resolve_style_font(&mut header, &request.character)?;
    let character = apply_format(
        header.char_shapes[usize::from(style.char_shape.0)].clone(),
        &request.character,
        resolved_font,
    );
    let paragraph = apply_para_shape_props(
        header.para_shapes[usize::from(style.para_shape.0)].clone(),
        &to_ir_units(&request.paragraph),
    );
    check_insert(&header.char_shapes, &character, "char_shapes")?;
    check_insert(&header.para_shapes, &paragraph, "para_shapes")?;
    style.char_shape = find_or_insert(&mut header.char_shapes, character);
    style.para_shape = find_or_insert_para(&mut header.para_shapes, paragraph);
    style.name = request.name.clone();
    style.english_name = request.name.clone();
    style.attr &= !0x7;
    style.kind_unknown = false;
    header.styles.push(style);
    doc.header = header;
    Ok(StyleId(id))
}

/// Apply a paragraph style to exactly one addressed paragraph. Paragraph and run
/// inheritance are independent; only `clear_direct` discards direct formatting.
/// Returns false without cache invalidation when the request changes nothing.
pub fn apply_style_at(
    doc: &mut Document,
    path: &SegmentPath,
    selector: &StyleSelector,
    clear_direct: bool,
) -> Result<bool, StyleEditError> {
    let target_id = selector.resolve(doc)?;
    let target = doc.header.styles[usize::from(target_id.0)].clone();
    require_paragraph_style(&target)?;
    check_shapes(doc, &target)?;
    let (before_style, before_para_shape, before_runs) = {
        let paragraph =
            crate::address::paragraph_at_mut(doc, path).ok_or(StyleEditError::InvalidAddress)?;
        (
            paragraph.style,
            paragraph.para_shape,
            paragraph.char_shape_runs.clone(),
        )
    };
    let old_id = StyleSelector::Id(before_style.0).resolve(doc)?;
    let old = &doc.header.styles[usize::from(old_id.0)];
    check_shapes(doc, old)?;
    if doc
        .header
        .para_shapes
        .get(usize::from(before_para_shape.0))
        .is_none()
    {
        return Err(StyleEditError::MissingShape {
            table: "para_shapes",
            id: before_para_shape.0,
        });
    }
    // hwpx/src/write/section.rs defaults both first_shape and shape_id_at to shape 0
    // when runs are absent. Apply the same inheritance decision to that effective run:
    // shape 0 stays direct when the old style uses another character shape.
    let implicit_run = [(0, CharShapeId(0))];
    let effective_runs = if before_runs.is_empty() {
        implicit_run.as_slice()
    } else {
        before_runs.as_slice()
    };
    for (_, id) in effective_runs {
        if doc.header.char_shapes.get(usize::from(id.0)).is_none() {
            return Err(StyleEditError::MissingShape {
                table: "char_shapes",
                id: id.0,
            });
        }
    }
    let para_shape = if clear_direct || before_para_shape == old.para_shape {
        target.para_shape
    } else {
        before_para_shape
    };
    let runs = if clear_direct {
        vec![(0, target.char_shape)]
    } else {
        effective_runs
            .iter()
            .map(|&(position, id)| {
                (
                    position,
                    if id == old.char_shape {
                        target.char_shape
                    } else {
                        id
                    },
                )
            })
            .collect()
    };
    if before_style == target_id
        && before_para_shape == para_shape
        && effective_runs == runs.as_slice()
    {
        return Ok(false);
    }
    let paragraph =
        crate::address::paragraph_at_mut(doc, path).ok_or(StyleEditError::InvalidAddress)?;
    paragraph.style = target_id;
    paragraph.para_shape = para_shape;
    paragraph.char_shape_runs = runs;
    paragraph.line_segs.clear();
    crate::address::invalidate_ancestors(doc, path);
    Ok(true)
}

/// Change a named paragraph style while keeping every old shape definition intact.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateStyle {
    pub style: StyleSelector,
    pub character: CharFormat,
    /// Dimensions use the HWPUNIT input convention of `set_para_props`.
    pub paragraph: ParaProps,
}

#[derive(Clone, Copy)]
struct RepointStyle {
    style: StyleId,
    old_char: CharShapeId,
    new_char: CharShapeId,
    old_para: ParaShapeId,
    new_para: ParaShapeId,
}

impl RepointStyle {
    fn affects(&self, paragraph: &Paragraph) -> bool {
        paragraph.style == self.style
            && ((self.old_para != self.new_para && paragraph.para_shape == self.old_para)
                || (self.old_char != self.new_char
                    && if paragraph.char_shape_runs.is_empty() {
                        self.old_char == CharShapeId(0)
                    } else {
                        paragraph
                            .char_shape_runs
                            .iter()
                            .any(|(_, id)| *id == self.old_char)
                    }))
    }

    fn apply(&self, paragraph: &mut Paragraph) -> bool {
        if !self.affects(paragraph) {
            return false;
        }
        if self.old_para != self.new_para && paragraph.para_shape == self.old_para {
            paragraph.para_shape = self.new_para;
        }
        if self.old_char != self.new_char {
            if paragraph.char_shape_runs.is_empty() {
                // Same effective shape0 convention as apply_style_at; leave direct empty runs alone.
                if self.old_char == CharShapeId(0) {
                    paragraph.char_shape_runs.push((0, self.new_char));
                }
            } else {
                for (_, id) in &mut paragraph.char_shape_runs {
                    if *id == self.old_char {
                        *id = self.new_char;
                    }
                }
            }
        }
        true
    }
}

fn check_paragraph_shape_refs(
    paragraph: &Paragraph,
    header: &DocHeader,
) -> Result<(), StyleEditError> {
    if usize::from(paragraph.para_shape.0) >= header.para_shapes.len() {
        return Err(StyleEditError::MissingShape {
            table: "para_shapes",
            id: paragraph.para_shape.0,
        });
    }
    for (_, id) in &paragraph.char_shape_runs {
        if usize::from(id.0) >= header.char_shapes.len() {
            return Err(StyleEditError::MissingShape {
                table: "char_shapes",
                id: id.0,
            });
        }
    }
    Ok(())
}

/// A model route plus the closest paragraph that the existing address resolver can reach.
/// Captions and raw-preserved descendants do not receive invented editable segment IDs.
#[derive(Clone)]
struct StyleUserScope {
    route: String,
    path: Option<SegmentPath>,
    paragraph_offset: usize,
    ancestor: Option<SegmentPath>,
    inside_raw: bool,
}

impl StyleUserScope {
    fn section(index: usize) -> Self {
        Self {
            route: format!("sections[{index}]"),
            path: Some(SegmentPath {
                section: index,
                indices: Vec::new(),
            }),
            paragraph_offset: 0,
            ancestor: None,
            inside_raw: false,
        }
    }

    fn paragraph(&self, index: usize) -> Self {
        let mut at = self.clone();
        at.route = format!("{}.paragraphs[{index}]", self.route);
        if let Some(path) = &mut at.path {
            path.indices.push(index + self.paragraph_offset);
            at.ancestor = Some(path.clone());
        }
        at.paragraph_offset = 0;
        at
    }

    fn child(
        &self,
        suffix: String,
        steps: &[usize],
        offset: usize,
        raw: bool,
        addressable: bool,
    ) -> Self {
        let inside_raw = self.inside_raw || raw;
        let path = if inside_raw || !addressable {
            None
        } else {
            self.path.clone().map(|mut path| {
                path.indices.extend_from_slice(steps);
                path
            })
        };
        Self {
            route: format!("{}.{}", self.route, suffix),
            path,
            paragraph_offset: offset,
            ancestor: self.ancestor.clone(),
            inside_raw,
        }
    }
}

#[derive(Default)]
struct RawStyleUsers {
    count: usize,
    locations: Vec<String>,
}

impl RawStyleUsers {
    fn record(&mut self, doc: &Document, scope: &StyleUserScope) {
        self.count += 1;
        if self.locations.len() == 8 {
            return;
        }
        let ancestor = scope.ancestor.as_ref().and_then(|path| {
            crate::address::resolve(
                doc,
                &crate::address::Address {
                    section: path.section,
                    indices: path.indices.clone(),
                    checksum: None,
                    chars: None,
                },
                crate::address::Granularity::Paragraph,
            )
            .ok()
        });
        self.locations.push(match ancestor {
            Some(target) => format!("{} (상위 문단 주소: {})", scope.route, target.before_id),
            None => scope.route.clone(),
        });
    }
}

fn preflight_style_users(
    paragraphs: &[Paragraph],
    rule: RepointStyle,
    doc: &Document,
    scope: &StyleUserScope,
    raw_users: &mut RawStyleUsers,
) -> Result<(), StyleEditError> {
    for (index, paragraph) in paragraphs.iter().enumerate() {
        // New definitions must not give an unrelated dangling reference a new meaning.
        check_paragraph_shape_refs(paragraph, &doc.header)?;
        let at = scope.paragraph(index);
        if at.inside_raw && rule.affects(paragraph) {
            raw_users.record(doc, &at);
        }
        for (control_index, control) in paragraph.controls.iter().enumerate() {
            let control_route = format!("controls[{control_index}]");
            match control {
                Control::Table(table) => {
                    for (cell_index, cell) in table.cells.iter().enumerate() {
                        let child = at.child(
                            format!("{control_route}.cells[{cell_index}]"),
                            &[control_index, cell_index],
                            0,
                            false,
                            true,
                        );
                        preflight_style_users(&cell.paragraphs, rule, doc, &child, raw_users)?;
                    }
                    if let Some(caption) = &table.caption {
                        let child =
                            at.child(format!("{control_route}.caption"), &[], 0, false, false);
                        preflight_style_users(&caption.paragraphs, rule, doc, &child, raw_users)?;
                    }
                }
                Control::Picture(picture) => {
                    if let Some(caption) = &picture.caption {
                        let child =
                            at.child(format!("{control_route}.caption"), &[], 0, false, false);
                        preflight_style_users(&caption.paragraphs, rule, doc, &child, raw_users)?;
                    }
                }
                Control::Generic(generic) => {
                    let raw = !generic.raw_children.is_empty();
                    let mut offset = 0;
                    for (list_index, list) in generic.paragraph_lists.iter().enumerate() {
                        let child = at.child(
                            format!("{control_route}.paragraph_lists[{list_index}]"),
                            &[control_index],
                            offset,
                            raw,
                            true,
                        );
                        preflight_style_users(&list.paragraphs, rule, doc, &child, raw_users)?;
                        offset += list.paragraphs.len();
                    }
                    if let Some(caption) = &generic.caption {
                        let child =
                            at.child(format!("{control_route}.caption"), &[], 0, raw, false);
                        preflight_style_users(&caption.paragraphs, rule, doc, &child, raw_users)?;
                    }
                }
                Control::SectionDef(_) => {}
            }
        }
    }
    Ok(())
}

fn repoint_style_users(paragraphs: &mut [Paragraph], rule: RepointStyle) -> bool {
    let mut changed = false;
    for paragraph in paragraphs {
        let mut here = rule.apply(paragraph);
        for control in &mut paragraph.controls {
            match control {
                Control::Table(table) => {
                    for cell in &mut table.cells {
                        here |= repoint_style_users(&mut cell.paragraphs, rule);
                    }
                    if let Some(caption) = &mut table.caption {
                        here |= repoint_style_users(&mut caption.paragraphs, rule);
                    }
                }
                Control::Picture(picture) => {
                    if let Some(caption) = &mut picture.caption {
                        here |= repoint_style_users(&mut caption.paragraphs, rule);
                    }
                }
                // structure::walk_para_lists uses the same boundary: native emit_control
                // re-emits raw_children and cannot publish edits to their modeled paragraphs.
                Control::Generic(generic) if generic.raw_children.is_empty() => {
                    let mut inner = false;
                    for list in &mut generic.paragraph_lists {
                        inner |= repoint_style_users(&mut list.paragraphs, rule);
                    }
                    if let Some(caption) = &mut generic.caption {
                        inner |= repoint_style_users(&mut caption.paragraphs, rule);
                    }
                    if inner {
                        generic.hwpx_raw_xml = None;
                    }
                    here |= inner;
                }
                Control::Generic(_) | Control::SectionDef(_) => {}
            }
        }
        if here {
            paragraph.line_segs.clear();
        }
        changed |= here;
    }
    changed
}

/// Append fresh changed shapes and repoint only the selected style's inheriting users.
/// All refusal checks precede document mutation; equal repeated requests add no definitions.
pub fn update_style(doc: &mut Document, request: &UpdateStyle) -> Result<bool, StyleEditError> {
    let id = request.style.resolve(doc)?;
    let old = doc.header.styles[usize::from(id.0)].clone();
    require_paragraph_style(&old)?;
    check_shapes(doc, &old)?;
    let mut header = doc.header.clone();
    let font = resolve_style_font(&mut header, &request.character)?;
    let character = apply_format(
        header.char_shapes[usize::from(old.char_shape.0)].clone(),
        &request.character,
        font,
    );
    let paragraph = apply_para_shape_props(
        header.para_shapes[usize::from(old.para_shape.0)].clone(),
        &to_ir_units(&request.paragraph),
    );
    let char_changed = character != doc.header.char_shapes[usize::from(old.char_shape.0)];
    let para_changed = paragraph != doc.header.para_shapes[usize::from(old.para_shape.0)];
    if !char_changed && !para_changed && header.fonts == doc.header.fonts {
        return Ok(false);
    }
    if header.fonts != doc.header.fonts {
        // Appending a face must not fill a previously dangling face index used elsewhere.
        for shape in &doc.header.char_shapes {
            for (slot, face) in shape.face_ids.iter().enumerate() {
                if usize::from(*face) >= doc.header.fonts[slot].len() {
                    return Err(StyleEditError::MissingShape {
                        table: "fonts",
                        id: *face,
                    });
                }
            }
        }
    }
    let new_char = if char_changed {
        let index =
            u16::try_from(header.char_shapes.len()).map_err(|_| StyleEditError::Capacity {
                table: "char_shapes",
            })?;
        // Reusing an equal direct/shared ID would make that direct user inherit the next update.
        header.char_shapes.push(character);
        CharShapeId(index)
    } else {
        old.char_shape
    };
    let new_para = if para_changed {
        let index =
            u16::try_from(header.para_shapes.len()).map_err(|_| StyleEditError::Capacity {
                table: "para_shapes",
            })?;
        header.para_shapes.push(paragraph);
        ParaShapeId(index)
    } else {
        old.para_shape
    };
    let rule = RepointStyle {
        style: id,
        old_char: old.char_shape,
        new_char,
        old_para: old.para_shape,
        new_para,
    };
    for style in &doc.header.styles {
        check_shapes(doc, style)?;
    }
    let mut raw_users = RawStyleUsers::default();
    for (index, section) in doc.sections.iter().enumerate() {
        preflight_style_users(
            &section.paragraphs,
            rule,
            doc,
            &StyleUserScope::section(index),
            &mut raw_users,
        )?;
    }
    if raw_users.count > 0 {
        return Err(StyleEditError::UnwritableStyleUser {
            count: raw_users.count,
            locations: raw_users.locations,
        });
    }
    header.styles[usize::from(id.0)].char_shape = new_char;
    header.styles[usize::from(id.0)].para_shape = new_para;
    doc.header = header;
    for section in &mut doc.sections {
        repoint_style_users(&mut section.paragraphs, rule);
    }
    Ok(true)
}
