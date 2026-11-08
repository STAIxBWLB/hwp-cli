//! Exact, source-bound occurrences. Suggestions never select a target on the caller's behalf.

mod apply;
pub mod contract;
#[cfg(test)]
mod tests;
pub use apply::{
    BindingChange, BindingPreview, PreparedBindings, ProfileCopy, apply_prepared_bindings,
    prepare_binding_plan,
};

use hwp_model::{Control, Document, HwpChar, Paragraph};
use serde::{Deserialize, Serialize};

use crate::segment_id::{SegmentPath, paragraph_id, table_id};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormOccurrence {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub paragraph_id: Option<String>,
    pub section: usize,
    pub indices: Vec<usize>,
    pub model_route: String,
    pub table_id: Option<String>,
    pub table_index: Option<usize>,
    pub row: Option<u16>,
    pub col: Option<u16>,
    /// Half-open `Paragraph::chars` indices, not UTF-8 bytes or WCHAR offsets.
    pub start: usize,
    pub end: usize,
    pub old_text: String,
    pub prior_sha256: String,
    pub bindable: bool,
    pub cause: Option<String>,
    pub next_step: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormDiscovery {
    pub version: u32,
    pub source_sha256: String,
    /// Full parsed-IR identity supplements, and never substitutes for, the original byte hash.
    pub structural_sha256: String,
    pub occurrences: Vec<FormOccurrence>,
}

pub fn digest(bytes: &[u8]) -> String {
    crate::document_split::source_sha256(bytes)
}

pub fn structural_fingerprint(doc: &Document) -> Result<String, String> {
    serde_json::to_vec(doc).map(|bytes| digest(&bytes)).map_err(|_| {
        "문서 구조를 식별할 수 없습니다. 문서를 validate로 확인한 뒤 discovery를 다시 실행하세요".into()
    })
}

/// Inventory exact text claims, including unsupported caption/raw/rich occurrences.
/// Cell paragraphs are explicit review targets, not inferred form-label ownership. Repeated
/// labels and overlapping slot/cell claims are retained; the plan must choose nonoverlapping ones.
pub fn scan_form_occurrences(doc: &Document, source_sha256: &str) -> Result<FormDiscovery, String> {
    let mut scan = Scan {
        source: source_sha256,
        out: Vec::new(),
        next_table: 0,
    };
    for (section, body) in doc.sections.iter().enumerate() {
        for (index, para) in body.paragraphs.iter().enumerate() {
            let path = SegmentPath {
                section,
                indices: vec![index],
            };
            scan.paragraph(para, &path, &format!("s{section}/p{index}"), None, None, 0)?;
        }
    }
    Ok(FormDiscovery {
        version: 1,
        source_sha256: source_sha256.into(),
        structural_sha256: structural_fingerprint(doc)?,
        occurrences: scan.out,
    })
}

#[derive(Clone)]
struct CellContext {
    table: String,
    index: usize,
    row: u16,
    col: u16,
    label: String,
}

struct Scan<'a> {
    source: &'a str,
    out: Vec<FormOccurrence>,
    next_table: usize,
}

impl Scan<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        para: &Paragraph,
        path: &SegmentPath,
        route: &str,
        cell: Option<&CellContext>,
        unsupported: Option<&str>,
        kind: &str,
        label: &str,
        start: usize,
        end: usize,
        old_text: String,
    ) -> Result<(), String> {
        if self.out.len() >= 20_000 {
            return Err("양식 위치가 20000개를 초과했습니다. 범위를 split으로 줄인 뒤 discovery를 다시 실행하세요".into());
        }
        // Opaque paragraph records can contain source text positions. A precise splice cannot
        // promise to relocate them, so they remain visible but unbindable rather than guessed.
        let rich = !para.controls.is_empty()
            || !para.extras.is_empty()
            || para.chars.iter().enumerate().any(|(index, ch)| {
                !(matches!(ch, HwpChar::Text(_))
                    || index + 1 == para.chars.len() && matches!(ch, HwpChar::CharCtrl(13)))
            })
            || para
                .char_shape_runs
                .iter()
                .map(|(_, shape)| shape)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1;
        let cause = unsupported.or(if kind == "native_field" {
            Some("native_field")
        } else if rich {
            Some("rich_target")
        } else {
            None
        });
        let prior_sha256 = digest(old_text.as_bytes());
        let identity =
            serde_json::to_vec(&(self.source, route, kind, start, end, &prior_sha256))
                .map_err(|_| "양식 위치 식별 실패. discovery를 다시 실행하세요".to_string())?;
        self.out.push(FormOccurrence {
            id: format!("occ-{}", digest(&identity)),
            kind: kind.into(),
            label: label.into(),
            paragraph_id: if unsupported == Some("caption_target") {
                None
            } else {
                Some(paragraph_id(path, para))
            },
            section: path.section,
            indices: path.indices.clone(),
            model_route: route.into(),
            table_id: cell.map(|c| c.table.clone()),
            table_index: cell
                .filter(|_| unsupported != Some("caption_target"))
                .map(|c| c.index),
            row: cell.map(|c| c.row),
            col: cell.map(|c| c.col),
            start,
            end,
            old_text,
            prior_sha256,
            bindable: cause.is_none(),
            cause: cause.map(str::to_owned),
            next_step: cause.map(|_| {
                if kind == "native_field" {
                    "hwp fields".into()
                } else {
                    "지원되는 일반 텍스트 위치를 선택하거나 원본 편집기에서 해당 대상을 수정하세요"
                        .into()
                }
            }),
        });
        Ok(())
    }

    fn paragraph(
        &mut self,
        para: &Paragraph,
        path: &SegmentPath,
        route: &str,
        cell: Option<&CellContext>,
        unsupported: Option<&str>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 32 || path.indices.len() > 32 {
            return Err("양식 주소/중첩이 32단계를 초과했습니다. split으로 범위를 줄이세요".into());
        }
        if cell.is_some() || unsupported.is_some() {
            let end = para.chars.len()
                - usize::from(matches!(para.chars.last(), Some(HwpChar::CharCtrl(13))));
            let text = para.chars[..end]
                .iter()
                .filter_map(|ch| {
                    if let HwpChar::Text(ch) = ch {
                        Some(*ch)
                    } else {
                        None
                    }
                })
                .collect();
            self.push(
                para,
                path,
                route,
                cell,
                unsupported,
                "cell_text",
                cell.map_or("", |c| c.label.as_str()),
                0,
                end,
                text,
            )?;
        }
        for (offset, segment) in crate::field::text_segments(para) {
            for token in hwp_model::slot_tokens(&segment) {
                let start = offset + segment[..token.range.start].chars().count();
                let end = offset + segment[..token.range.end].chars().count();
                self.push(
                    para,
                    path,
                    route,
                    cell,
                    unsupported,
                    "placeholder",
                    token.name.trim(),
                    start,
                    end,
                    segment[token.range].into(),
                )?;
            }
            if cell.is_none() {
                for inline in super::inline_labels(&segment) {
                    if super::defers_to_a_different_key(
                        &segment[inline.label.clone()],
                        &segment[inline.value.clone()],
                    ) {
                        continue;
                    }
                    let start = offset + segment[..inline.value.start].chars().count();
                    let end = offset + segment[..inline.value.end].chars().count();
                    self.push(
                        para,
                        path,
                        route,
                        cell,
                        unsupported,
                        "inline_value",
                        &segment[inline.label],
                        start,
                        end,
                        segment[inline.value].into(),
                    )?;
                }
            }
        }
        for (index, ch) in para.chars.iter().enumerate() {
            if let HwpChar::ExtCtrl {
                code: 3,
                ctrl_index,
                ..
            } = ch
            {
                let name = ctrl_index
                    .and_then(|index| para.controls.get(index as usize))
                    .and_then(|control| crate::field::field_meta(control).0)
                    .unwrap_or_default();
                self.push(
                    para,
                    path,
                    route,
                    cell,
                    unsupported,
                    "native_field",
                    &name,
                    index,
                    index + 1,
                    String::new(),
                )?;
            }
        }
        for (control_index, control) in para.controls.iter().enumerate() {
            let mut nested = path.clone();
            nested.indices.push(control_index);
            let control_route = format!("{route}/c{control_index}");
            match control {
                Control::Table(table) => {
                    let index = self.next_table;
                    if unsupported != Some("caption_target") {
                        self.next_table += 1;
                    }
                    let id = table_id(&nested, table);
                    for (cell_index, child) in table.cells.iter().enumerate() {
                        let label = table
                            .cells
                            .iter()
                            .filter(|candidate| {
                                (candidate.row == child.row
                                    && candidate.col.saturating_add(candidate.col_span)
                                        == child.col)
                                    || (candidate.col == child.col
                                        && candidate.row.saturating_add(candidate.row_span)
                                            == child.row)
                            })
                            .map(super::cell_text)
                            .collect::<Vec<_>>()
                            .join(" | ");
                        let context = CellContext {
                            table: id.clone(),
                            index,
                            row: child.row,
                            col: child.col,
                            label,
                        };
                        for (paragraph_index, child) in child.paragraphs.iter().enumerate() {
                            let mut child_path = nested.clone();
                            child_path.indices.extend([cell_index, paragraph_index]);
                            self.paragraph(
                                child,
                                &child_path,
                                &format!("{control_route}/cell{cell_index}/p{paragraph_index}"),
                                Some(&context),
                                unsupported,
                                depth + 1,
                            )?;
                        }
                    }
                    if let Some(caption) = &table.caption {
                        self.caption(caption, &nested, &control_route, depth)?;
                    }
                }
                Control::Generic(generic) => {
                    let unsupported = unsupported.or(
                        if !generic.raw_children.is_empty() || generic.hwpx_raw_xml.is_some() {
                            Some("raw_preserved_target")
                        } else {
                            None
                        },
                    );
                    let mut seq = 0;
                    for (list_index, list) in generic.paragraph_lists.iter().enumerate() {
                        for (paragraph_index, child) in list.paragraphs.iter().enumerate() {
                            let mut child_path = nested.clone();
                            child_path.indices.push(seq);
                            seq += 1;
                            self.paragraph(
                                child,
                                &child_path,
                                &format!("{control_route}/list{list_index}/p{paragraph_index}"),
                                cell,
                                unsupported,
                                depth + 1,
                            )?;
                        }
                    }
                    if let Some(caption) = &generic.caption {
                        self.caption(caption, &nested, &control_route, depth)?;
                    }
                }
                Control::Picture(picture) => {
                    if let Some(caption) = &picture.caption {
                        self.caption(caption, &nested, &control_route, depth)?;
                    }
                }
                Control::SectionDef(_) => {}
            }
        }
        Ok(())
    }

    fn caption(
        &mut self,
        caption: &hwp_model::Caption,
        path: &SegmentPath,
        route: &str,
        depth: usize,
    ) -> Result<(), String> {
        for (index, para) in caption.paragraphs.iter().enumerate() {
            self.paragraph(
                para,
                path,
                &format!("{route}/caption/p{index}"),
                None,
                Some("caption_target"),
                depth + 1,
            )?;
        }
        Ok(())
    }
}
