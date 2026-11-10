//! Reviewed orchestration over existing clone/splice primitives; no native writer logic.

use std::collections::{BTreeMap, BTreeSet};

use hwp_model::{Control, Document, HwpChar};
use serde::{Deserialize, Serialize};

use super::contract::{BindingPlan, Values};
use super::{FormOccurrence, digest, scan_form_occurrences, structural_fingerprint};
use crate::segment_id::{SegmentPath, paragraph_id, table_id};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCopy {
    pub source_table_id: String,
    pub copy: usize,
    pub profile_id: String,
    pub table_id: String,
    pub section: usize,
    pub indices: Vec<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingChange {
    pub occurrence_id: String,
    pub copy: usize,
    pub profile_id: Option<String>,
    pub paragraph_id: String,
    pub section: usize,
    pub indices: Vec<usize>,
    pub row: Option<u16>,
    pub col: Option<u16>,
    pub start: usize,
    pub end: usize,
    pub old_text: String,
    pub new_text: String,
    pub max_chars: usize,
    pub allow_reflow: bool,
}

/// Explicit local preview contains content. Receipts must project only its identities/counts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPreview {
    pub version: u32,
    pub source_sha256: String,
    pub plan_sha256: String,
    pub values_sha256: String,
    pub copies: Vec<ProfileCopy>,
    pub changes: Vec<BindingChange>,
    /// Filled by the native executor for the explicitly requested local artifact only.
    /// Empty core preparation does not attest absence of font/render issues.
    pub native_warnings: Vec<String>,
}

pub struct PreparedBindings {
    before: String,
    document: Document,
    pub preview: BindingPreview,
}

impl PreparedBindings {
    pub fn document(&self) -> &Document {
        &self.document
    }
}

fn refuse(cause: &str) -> String {
    format!(
        "양식 바인딩을 거부했습니다 ({cause}). 원본 discovery와 계획의 해당 위치/허용 범위를 다시 확인하고 재검토하세요"
    )
}

/// JSON deliberately omits binary payloads and the preview image. Include their identities
/// in the private prepare/apply guard so a caller cannot lose intervening in-memory changes.
fn live_document_fingerprint(doc: &Document) -> Result<String, String> {
    let binaries: Vec<_> = doc
        .bin_streams
        .iter()
        .map(|stream| (&stream.name, digest(&stream.data)))
        .collect();
    let preview = doc.hwpx_preview_image.as_ref().map(|bytes| digest(bytes));
    let bytes = serde_json::to_vec(&(structural_fingerprint(doc)?, binaries, preview))
        .map_err(|_| refuse("prepare/apply 문서 상태를 식별할 수 없음"))?;
    Ok(digest(&bytes))
}

pub(super) fn claims_overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    // An insertion owns its point. Replacing a half-open range must not overwrite it,
    // including at the range's start; its excluded end remains an adjacent position.
    match (a.0 == a.1, b.0 == b.1) {
        (true, true) => a.0 == b.0,
        (true, false) => (b.0..b.1).contains(&a.0),
        (false, true) => (a.0..a.1).contains(&b.0),
        (false, false) => a.0 < b.1 && b.0 < a.1,
    }
}

fn mapped(path: &SegmentPath, insertions: &[(usize, usize)]) -> SegmentPath {
    let mut path = path.clone();
    for &(section, at) in insertions {
        if path.section == section && path.indices[0] >= at {
            path.indices[0] += 1;
        }
    }
    path
}

fn path_of(occurrence: &FormOccurrence) -> SegmentPath {
    SegmentPath {
        section: occurrence.section,
        indices: occurrence.indices.clone(),
    }
}

struct CopyRoute {
    source: SegmentPath,
    destination: SegmentPath,
    repeat: usize,
    copy: usize,
}

/// Preflight all constraints and selectors on the original input, then prepare a private
/// candidate through clone_table and splice_edits. A refusal never mutates the supplied doc.
pub fn prepare_binding_plan(
    doc: &Document,
    actual_source_sha256: &str,
    plan: &BindingPlan,
    values: &Values,
) -> Result<PreparedBindings, String> {
    plan.validate_values(values)?;
    if actual_source_sha256 != plan.source_sha256 {
        return Err(refuse(
            "source_sha256 불일치; 수정된 원본은 새 discovery 필요",
        ));
    }
    let before = structural_fingerprint(doc)?;
    if before != plan.structural_sha256 {
        return Err(refuse("원본 구조 fingerprint 불일치"));
    }
    let discovery = scan_form_occurrences(doc, actual_source_sha256)?;
    let occurrences: BTreeMap<_, _> = discovery
        .occurrences
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    let mut selected = Vec::new();
    let mut claims = BTreeMap::<(usize, Vec<usize>, usize), Vec<(usize, usize)>>::new();
    for (binding_index, binding) in plan.bindings.iter().enumerate() {
        for (target_index, target) in binding.targets.iter().enumerate() {
            let target_refuse = |cause: &str| {
                refuse(&format!(
                    "binding[{binding_index}].targets[{target_index}]: {cause}"
                ))
            };
            let occurrence = occurrences
                .get(target.occurrence_id.as_str())
                .ok_or_else(|| target_refuse("occurrence_id가 원본 목록에 없음"))?;
            if !occurrence.bindable {
                return Err(target_refuse(
                    "선택한 occurrence는 native/rich/raw/caption 대상으로 지원하지 않음",
                ));
            }
            let text = &values[&binding.key];
            if !target.allow_reflow
                && (text.contains(['\n', '\t'])
                    || text.encode_utf16().count() > occurrence.old_text.encode_utf16().count())
            {
                return Err(target_refuse(
                    "명시적 reflow 허용 없이 원래 텍스트 범위를 초과함",
                ));
            }
            for &copy in &target.copies {
                if selected.len() >= 20_000 {
                    return Err(target_refuse(
                        "적용 위치 합계가 20000개를 초과함; 계획을 분리해야 함",
                    ));
                }
                let ranges = claims
                    .entry((occurrence.section, occurrence.indices.clone(), copy))
                    .or_default();
                if ranges
                    .iter()
                    .any(|&existing| claims_overlap(existing, (occurrence.start, occurrence.end)))
                {
                    return Err(target_refuse("선택한 occurrence의 텍스트 범위가 중복/겹침"));
                }
                ranges.push((occurrence.start, occurrence.end));
                selected.push((*occurrence, target, text, copy));
            }
        }
    }
    let mut candidate = doc.clone();
    let mut insertions = Vec::new();
    let mut routes = Vec::<CopyRoute>::new();
    let mut repeated = BTreeSet::new();
    for (repeat_index, repeat) in plan.repeats.iter().enumerate() {
        let repeat_refuse = |cause: &str| refuse(&format!("repeats[{repeat_index}]: {cause}"));
        if !repeated.insert(repeat.table_id.as_str()) {
            return Err(repeat_refuse("같은 표의 repeat 선언이 중복됨"));
        }
        // V1 repeats whole top-level profile tables. Nested/caption/raw tables are not
        // promoted to body tables, because doing so would silently change their context.
        let mut source = None;
        let mut anchor = None;
        for (section_index, section) in doc.sections.iter().enumerate() {
            for (para_index, para) in section.paragraphs.iter().enumerate() {
                let path = SegmentPath {
                    section: section_index,
                    indices: vec![para_index],
                };
                if paragraph_id(&path, para) == repeat.after_paragraph_id {
                    anchor = Some(path.clone());
                }
                for (control_index, control) in para.controls.iter().enumerate() {
                    if let Control::Table(table) = control {
                        let mut path = path.clone();
                        path.indices.push(control_index);
                        if table_id(&path, table) == repeat.table_id {
                            source = Some(path);
                        }
                    }
                }
            }
        }
        let source = source
            .ok_or_else(|| repeat_refuse("repeat 표가 최상위 본문 표가 아니거나 원본 id가 없음"))?;
        let anchor =
            anchor.ok_or_else(|| repeat_refuse("repeat 삽입 앵커의 원본 문단 id가 없음"))?;
        let anchor_para = &doc.sections[anchor.section].paragraphs[anchor.indices[0]];
        let anchor_text: String = anchor_para
            .chars
            .iter()
            .filter_map(|ch| {
                if let HwpChar::Text(ch) = ch {
                    Some(*ch)
                } else {
                    None
                }
            })
            .collect();
        if anchor_text.is_empty()
            || doc
                .sections
                .iter()
                .flat_map(|s| &s.paragraphs)
                .filter(|p| crate::edit::find_match(&p.chars, &anchor_text, 0).is_some())
                .count()
                != 1
        {
            return Err(repeat_refuse(
                "repeat 앵커 텍스트가 비어 있거나 여러 본문 문단과 일치함; 고유 본문 앵커 필요",
            ));
        }
        for copy in (1..repeat.copies).rev() {
            let current_source = mapped(&source, &insertions);
            let current = scan_form_occurrences(&candidate, actual_source_sha256)?;
            let table_index = current
                .occurrences
                .iter()
                .find(|item| {
                    item.section == current_source.section
                        && item.indices.starts_with(&current_source.indices)
                        && item.indices.len() == current_source.indices.len() + 2
                })
                .and_then(|item| item.table_index)
                .ok_or_else(|| repeat_refuse("복제할 표의 현재 주소를 확인할 수 없음"))?;
            let current_anchor = mapped(&anchor, &insertions);
            let insertion = (current_anchor.section, current_anchor.indices[0] + 1);
            crate::edit::clone_table(
                &mut candidate,
                table_index,
                &anchor_text,
                crate::edit::CloneTextMode::Keep,
            )
            .map_err(|_| {
                repeat_refuse("기존 clone_table preflight가 보존할 수 없는 표/개체를 발견함")
            })?;
            for route in &mut routes {
                if route.destination.section == insertion.0
                    && route.destination.indices[0] >= insertion.1
                {
                    route.destination.indices[0] += 1;
                }
            }
            routes.push(CopyRoute {
                source: source.clone(),
                destination: SegmentPath {
                    section: insertion.0,
                    indices: vec![insertion.1, 0],
                },
                repeat: repeat_index,
                copy,
            });
            insertions.push(insertion);
        }
        routes.push(CopyRoute {
            source: source.clone(),
            destination: mapped(&source, &insertions),
            repeat: repeat_index,
            copy: 0,
        });
    }
    // Original routes must reflect insertions by later repeat groups as well.
    for route in &mut routes {
        if route.copy == 0 {
            route.destination = mapped(&route.source, &insertions);
        }
    }
    let recatalogue = scan_form_occurrences(&candidate, actual_source_sha256)?;
    let mut copies = Vec::new();
    for route in &routes {
        let occurrence = recatalogue
            .occurrences
            .iter()
            .find(|item| {
                item.section == route.destination.section
                    && item.indices.starts_with(&route.destination.indices)
                    && item.indices.len() == route.destination.indices.len() + 2
            })
            .ok_or_else(|| refuse("복제 후 표의 occurrence map이 없음"))?;
        let repeat = &plan.repeats[route.repeat];
        copies.push(ProfileCopy {
            source_table_id: repeat.table_id.clone(),
            copy: route.copy,
            profile_id: repeat.profile_ids[route.copy].clone(),
            table_id: occurrence
                .table_id
                .clone()
                .ok_or_else(|| refuse("복제 후 table id가 없음"))?,
            section: route.destination.section,
            indices: route.destination.indices.clone(),
        });
    }
    copies.sort_by(|a, b| (&a.source_table_id, a.copy).cmp(&(&b.source_table_id, b.copy)));
    let mut changes = Vec::new();
    let mut edits = BTreeMap::<(usize, Vec<usize>), Vec<_>>::new();
    for (occurrence, target, text, copy) in selected {
        let route = routes.iter().find(|route| {
            occurrence.section == route.source.section
                && occurrence.indices.starts_with(&route.source.indices)
                && route.copy == copy
        });
        let (path, profile_id) = if let Some(route) = route {
            let mut path = route.destination.clone();
            path.indices
                .extend_from_slice(&occurrence.indices[route.source.indices.len()..]);
            (
                path,
                Some(plan.repeats[route.repeat].profile_ids[copy].clone()),
            )
        } else if copy == 0 {
            (mapped(&path_of(occurrence), &insertions), None)
        } else {
            return Err(refuse("선택한 copy에 대응하는 repeat/profile 선언이 없음"));
        };
        let paragraph = crate::address::paragraph_at_mut(&mut candidate, &path)
            .ok_or_else(|| refuse("복제 후 문단 주소가 없음"))?;
        let old_text: String = paragraph
            .chars
            .get(occurrence.start..occurrence.end)
            .ok_or_else(|| refuse("텍스트 범위가 오래됨"))?
            .iter()
            .filter_map(|ch| {
                if let HwpChar::Text(ch) = ch {
                    Some(*ch)
                } else {
                    None
                }
            })
            .collect();
        if digest(old_text.as_bytes()) != occurrence.prior_sha256 {
            return Err(refuse("복제 후 원래 값 digest 불일치"));
        }
        changes.push(BindingChange {
            occurrence_id: occurrence.id.clone(),
            copy,
            profile_id,
            paragraph_id: paragraph_id(&path, paragraph),
            section: path.section,
            indices: path.indices.clone(),
            row: occurrence.row,
            col: occurrence.col,
            start: occurrence.start,
            end: occurrence.end,
            old_text,
            new_text: text.clone(),
            max_chars: target.max_chars,
            allow_reflow: target.allow_reflow,
        });
        edits
            .entry((path.section, path.indices))
            .or_default()
            .push((occurrence.start..occurrence.end, text.clone()));
    }
    for ((section, indices), mut edits) in edits {
        edits.sort_by_key(|(range, _)| range.start);
        let path = SegmentPath { section, indices };
        let paragraph = crate::address::paragraph_at_mut(&mut candidate, &path)
            .ok_or_else(|| refuse("적용할 문단 주소가 없음"))?;
        crate::edit::splice_edits(paragraph, edits)?;
        crate::address::invalidate_ancestors(&mut candidate, &path);
    }
    let plan_bytes = serde_json::to_vec(plan).map_err(|_| refuse("계획 직렬화 실패"))?;
    let value_bytes = serde_json::to_vec(values).map_err(|_| refuse("값 직렬화 실패"))?;
    Ok(PreparedBindings {
        before: live_document_fingerprint(doc)?,
        document: candidate,
        preview: BindingPreview {
            version: 1,
            source_sha256: actual_source_sha256.into(),
            plan_sha256: digest(&plan_bytes),
            values_sha256: digest(&value_bytes),
            copies,
            changes,
            native_warnings: Vec::new(),
        },
    })
}

pub fn apply_prepared_bindings(
    doc: &mut Document,
    prepared: PreparedBindings,
) -> Result<BindingPreview, String> {
    if live_document_fingerprint(doc)? != prepared.before {
        return Err(refuse("prepare 이후 문서가 바뀜"));
    }
    *doc = prepared.document;
    Ok(prepared.preview)
}
