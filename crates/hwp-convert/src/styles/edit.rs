//! Copy-on-write named-style creation and paragraph-scoped application (#427 T3).
//! Native publication guards and format-specific style limits belong to callers.
use hwp_model::{Document, StyleId};

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

/// A named-style request failed without changing the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleEditError {
    InvalidName,
    DuplicateName(String),
    UnknownStyle(StyleSelector),
    AmbiguousName(String),
    MissingShape { table: &'static str, id: u16 },
    Capacity { table: &'static str },
    InvalidAddress,
    UnsupportedKind(u8),
    CharacterStylesUnsupported,
    InvalidCharacterProperties,
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
                "스타일 {selector:?}을 찾을 수 없습니다. 문서의 스타일 이름이나 번호를 확인해 보세요"
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
            Self::UnsupportedKind(kind) => write!(
                f,
                "종류 {kind}인 스타일은 문단에 적용할 수 없습니다. 문단 스타일을 지정해 보세요"
            ),
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
    if request
        .character
        .size_pt
        .is_some_and(|size| !size.is_finite() || size * 100.0 > i32::MAX as f32)
        || request
            .character
            .font
            .as_ref()
            .is_some_and(|name| name.is_empty() || name.chars().any(char::is_control))
    {
        return Err(StyleEditError::InvalidCharacterProperties);
    }
    let mut header = doc.header.clone();
    let resolved_font = if let Some(name) = request.character.font.as_deref() {
        for fonts in &header.fonts {
            check_rows(fonts.len(), "fonts")?;
            if fonts.len() == MAX_ROWS && !fonts.iter().any(|face| face.name == name) {
                return Err(StyleEditError::Capacity { table: "fonts" });
            }
        }
        Some(resolve_font_faces(&mut header.fonts, name))
    } else {
        None
    };
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
    if target.attr & 0x7 == 1 {
        return Err(StyleEditError::CharacterStylesUnsupported);
    }
    if target.attr & 0x7 != 0 {
        return Err(StyleEditError::UnsupportedKind(target.attr & 0x7));
    }
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
    for (_, id) in &before_runs {
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
        before_runs
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
    if before_style == target_id && before_para_shape == para_shape && before_runs == runs {
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
