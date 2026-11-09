//! hwp edit --ops <file.json>: validation and conversion for typed operation files (D-10).
//!
//! The normative schema is schemas/edit-ops-v1.schema.json. This module contains its
//! serde representation (OpsEntry) and runtime validator. The schema checks shape only;
//! OpsEntry::into_typed enforces semantic constraints afterward: exactly one selector,
//! paired image dimensions, minimum counts and mutually exclusive line-spacing fields.
//! This matches the MCP tool_edit boundary's two-stage contract and Korean error wording.

use std::io::Read as _;
use std::path::Path;

use anyhow::Context as _;
use serde::Deserialize;

use crate::commands::edit::TypedEditOperation;
use crate::commands::edit::{mm_to_hwpunit, parse_align, parse_color};
use crate::commands::mcp::checked_cli_read_path;

/// Operation-file byte limit, matching MCP MAX_READ_BYTES (16 MiB).
pub(crate) const MAX_OPS_BYTES: u64 = 16 * 1024 * 1024;
/// Operation-count limit, matching schema maxItems and checked again after parsing.
pub(crate) const MAX_OPS_ITEMS: usize = 10_000;

/// Upper bound for `*_mm` fields (WR-02). The schema's `unit` pattern
/// (`^\d+(\.\d+)?(mm|pt|%)$`) forbids a sign but does not cap the digit count, and an
/// f32 overflow silently yields `inf` (no panic: `mm_to_hwpunit` then saturates to
/// `i32::MAX`), so the parser checks finiteness and range itself. 5000mm (5m) is far
/// beyond any real document dimension — page, margin or image size.
const MM_MAX: f32 = 5000.0;
/// Upper bound for `*_pt` fields (font size, fixed line spacing). A negative value is
/// meaningless for both.
const PT_MAX: f32 = 1000.0;
/// Upper bound for `*_pct` fields (line-spacing ratio). A negative ratio is meaningless.
const PCT_MAX: f32 = 1000.0;

/// Read an operation file, or stdin for -, as bounded UTF-8.
fn read_ops_source(path: &Path) -> anyhow::Result<String> {
    if path.as_os_str() == "-" {
        return read_bounded(std::io::stdin(), MAX_OPS_BYTES);
    }
    let file = std::fs::File::open(path).map_err(|error| {
        anyhow::anyhow!(
            "편집 연산 파일을 열 수 없습니다: {} ({error})",
            path.display()
        )
    })?;
    read_bounded(std::io::BufReader::new(file), MAX_OPS_BYTES)
}

/// Bounded UTF-8 read, matching lint.rs read_bounded: oversized input is refused with
/// a Korean error rather than silently truncated. Non-UTF-8 input becomes a read error
/// without panicking.
fn read_bounded(reader: impl std::io::Read, cap: u64) -> anyhow::Result<String> {
    let mut buf = String::new();
    reader
        .take(cap + 1)
        .read_to_string(&mut buf)
        .map_err(|error| anyhow::anyhow!("편집 연산 입력을 읽을 수 없습니다: {error}"))?;
    if buf.len() as u64 > cap {
        anyhow::bail!("입력이 크기 제한({cap}바이트)을 초과했습니다");
    }
    Ok(buf)
}

/// Read operation input, validate its schema, deserialize OpsEntry, then convert into Op.
/// Refuse input, schema and semantic-constraint failures with Korean errors. Schema
/// Malformed envelopes and binding-bearing failures omit payloads; other legacy violations
/// retain jsonschema wording. Redaction is required because
/// values, keys and unknown property names may contain private document content.
pub(crate) fn load_ops(path: &Path) -> anyhow::Result<Vec<TypedEditOperation>> {
    let raw = read_ops_source(path)?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| anyhow::anyhow!("편집 연산 JSON을 해석할 수 없습니다: {error}"))?;
    parse_ops_value(value)
}

/// Validate every JSON channel against the same closed vocabulary before normalization.
pub(crate) fn parse_ops_value(value: serde_json::Value) -> anyhow::Result<Vec<TypedEditOperation>> {
    let entries = value.as_array().ok_or_else(|| anyhow::anyhow!(
        "편집 연산이 edit-ops-v1 스키마를 벗어났습니다: 최상위 값은 배열이어야 합니다. 연산 객체를 [ ... ]로 감싸세요 (입력 내용 생략)"))?;
    if entries.iter().any(|entry| !entry.is_object()) {
        anyhow::bail!(
            "편집 연산이 edit-ops-v1 스키마를 벗어났습니다: 모든 배열 항목은 객체여야 합니다. 중첩 배열/문자열 포장을 제거하고 op 속성을 가진 객체를 지정하세요 (입력 내용 생략)"
        );
    }
    fn contains_binding(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                object.contains_key("plan")
                    || object.contains_key("values")
                    || object.get("op").and_then(serde_json::Value::as_str)
                        == Some("apply_form_bindings")
                    || object.values().any(contains_binding)
            }
            serde_json::Value::Array(items) => items.iter().any(contains_binding),
            serde_json::Value::String(text) => text.contains("apply_form_bindings"),
            _ => false,
        }
    }
    let binding_input = contains_binding(&value);
    if entries.iter().any(|entry| {
        entry.get("op").and_then(serde_json::Value::as_str) == Some("apply_form_bindings")
    }) && entries.len() != 1
    {
        anyhow::bail!(
            "apply_form_bindings는 단독 compound 연산이어야 합니다. 다른 연산을 별도 요청으로 분리하세요 (입력 내용 생략)"
        );
    }
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/edit-ops-v1.schema.json"))
            .map_err(|error| anyhow::anyhow!("edit-ops-v1 스키마를 해석할 수 없습니다: {error}"))?;
    let document_spec: serde_json::Value = serde_json::from_str(include_str!(
        "../../../schemas/document-spec-v1.schema.json"
    )).map_err(|error| anyhow::anyhow!("내장 document-spec-v1 스키마를 해석할 수 없습니다: {error}. 정상 빌드의 hwp를 설치하거나 해당 스키마를 수정해 다시 빌드하세요"))?;
    let document_spec = jsonschema::Resource::from_contents(document_spec)
        .map_err(|error| anyhow::anyhow!("내장 document-spec-v1 스키마 리소스를 만들 수 없습니다: {error}. 스키마의 $schema/$id를 확인하고 다시 빌드하세요"))?;
    let validator = jsonschema::options()
        .with_resource(
            "https://hwp-cli.staix.net/schemas/document-spec-v1.schema.json",
            document_spec,
        )
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .map_err(|error| anyhow::anyhow!("edit-ops-v1 스키마를 컴파일할 수 없습니다: {error}"))?;
    if let Some(error) = validator.iter_errors(&value).next() {
        if binding_input {
            anyhow::bail!(
                "편집 연산이 edit-ops-v1 스키마를 벗어났습니다: 닫힌 연산 계약과 다릅니다. 필수 속성과 자료형을 스키마와 비교해 수정하세요 (입력 내용 생략)"
            );
        }
        anyhow::bail!(
            "편집 연산이 edit-ops-v1 스키마를 벗어났습니다: {}: {error}",
            error.instance_path
        );
    }
    let entries: Vec<OpsEntry> = serde_json::from_value(value).map_err(|error| {
        if binding_input { anyhow::anyhow!("양식 바인딩 JSON을 해석할 수 없습니다. 닫힌 스키마의 필수 속성과 자료형을 확인하세요 (입력 내용 생략)") }
        else { anyhow::anyhow!("편집 연산을 해석할 수 없습니다: {error}") }
    })?;
    if entries.len() > MAX_OPS_ITEMS {
        anyhow::bail!("편집 연산이 항목 상한({MAX_OPS_ITEMS}개)을 초과했습니다");
    }
    entries
        .into_iter()
        .map(OpsEntry::into_typed)
        .collect::<Result<Vec<_>, String>>()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// Tag one typed MCP payload. The transport chooses its operation, not a nested `op`.
pub(crate) fn named_style_item(
    mut value: serde_json::Value,
    operation: &str,
) -> anyhow::Result<serde_json::Value> {
    let object = value.as_object_mut().ok_or_else(|| anyhow::anyhow!(
        "{operation} 항목은 JSON 객체여야 합니다. edit-ops-v1의 해당 연산에서 op를 뺀 속성 객체를 사용하세요"
    ))?;
    if object.contains_key("op") {
        anyhow::bail!(
            "{operation} 항목에 op를 넣을 수 없습니다. 연산은 바깥 필드가 지정하므로 내부 op를 제거하세요"
        );
    }
    object.insert("op".into(), serde_json::Value::String(operation.into()));
    Ok(value)
}

/// Publish an embedded edit-op vocabulary without external schema refs. Named-style callers
/// use the array wrapper; the compound binding caller takes its inlined item schema.
pub(crate) fn named_style_schema(operation: &str) -> serde_json::Value {
    use serde_json::Value;
    fn expand(value: &Value, root: &Value, depth: usize) -> Value {
        assert!(depth < 32, "embedded style schema reference cycle");
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    let pointer = reference
                        .strip_prefix('#')
                        .expect("style schema refs must be local");
                    let mut expanded = expand(
                        root.pointer(pointer)
                            .expect("embedded style schema reference exists"),
                        root,
                        depth + 1,
                    );
                    for (key, value) in object {
                        if key != "$ref" {
                            expanded[key] = expand(value, root, depth + 1);
                        }
                    }
                    return expanded;
                }
                let mut result = serde_json::Map::new();
                for (key, value) in object {
                    if key == "properties" {
                        let mut properties =
                            value.as_object().expect("schema properties object").clone();
                        properties.remove("op");
                        result.insert(
                            key.clone(),
                            expand(&Value::Object(properties), root, depth + 1),
                        );
                    } else if key == "required" {
                        result.insert(
                            key.clone(),
                            Value::Array(
                                value
                                    .as_array()
                                    .expect("schema required array")
                                    .iter()
                                    .filter(|item| item.as_str() != Some("op"))
                                    .cloned()
                                    .collect(),
                            ),
                        );
                    } else {
                        result.insert(key.clone(), expand(value, root, depth + 1));
                    }
                }
                Value::Object(result)
            }
            Value::Array(array) => Value::Array(
                array
                    .iter()
                    .map(|value| expand(value, root, depth + 1))
                    .collect(),
            ),
            _ => value.clone(),
        }
    }
    let root: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-ops-v1.schema.json"))
            .expect("embedded ops schema JSON");
    let schema = root["$defs"]["editOp"]["oneOf"]
        .as_array()
        .expect("ops schema alternatives")
        .iter()
        .find(|schema| {
            schema
                .pointer("/properties/op/const")
                .and_then(Value::as_str)
                == Some(operation)
                || schema
                    .get("allOf")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item.pointer("/properties/op/const").and_then(Value::as_str)
                                == Some(operation)
                        })
                    })
        })
        .expect("named-style schema exists");
    serde_json::json!({"type":"array", "maxItems":MAX_OPS_ITEMS, "items":expand(schema, &root, 0)})
}

/// Parse 10.5mm into an f32 millimeter value, the unit-string grammar for *_mm fields.
fn parse_mm_f32(value: &str) -> anyhow::Result<f32> {
    let mm: f32 = value
        .trim()
        .strip_suffix("mm")
        .ok_or_else(|| anyhow::anyhow!("mm 값은 mm로 끝나야 합니다: {value:?}"))?
        .parse()
        .with_context(|| format!("mm 값이 숫자가 아닙니다: {value:?}"))?;
    if !mm.is_finite() || !(0.0..=MM_MAX).contains(&mm) {
        anyhow::bail!("mm 값은 유한한 0..={MM_MAX} 범위여야 합니다: {value:?}");
    }
    Ok(mm)
}

/// Parse 10.5mm into HWPUNIT (1 mm = 7200/25.4), matching CLI parse_mm in edit.rs.
pub(crate) fn parse_mm(value: &str) -> anyhow::Result<i32> {
    Ok(mm_to_hwpunit(parse_mm_f32(value)?))
}

/// Parse 160% into an f32 percentage, used by line_spacing_pct and similar fields.
pub(crate) fn parse_pct(value: &str) -> anyhow::Result<f32> {
    let pct: f32 = value
        .trim()
        .strip_suffix('%')
        .ok_or_else(|| anyhow::anyhow!("백분율 값은 %로 끝나야 합니다: {value:?}"))?
        .parse()
        .with_context(|| format!("% 값이 숫자가 아닙니다: {value:?}"))?;
    if !pct.is_finite() || !(0.0..=PCT_MAX).contains(&pct) {
        anyhow::bail!("백분율 값은 유한한 0..={PCT_MAX} 범위여야 합니다: {value:?}");
    }
    Ok(pct)
}

/// Convert a size string into points: 12pt stays unchanged; 10mm uses 1 in = 72 pt = 25.4 mm.
/// Refuse percent because no absolute point reference exists; the parser narrows schema unit syntax.
pub(crate) fn parse_pt(value: &str) -> anyhow::Result<f32> {
    let trimmed = value.trim();
    let pt = if let Some(pt) = trimmed.strip_suffix("pt") {
        pt.parse::<f32>()
            .with_context(|| format!("pt 값이 숫자가 아닙니다: {value:?}"))?
    } else if let Some(mm) = trimmed.strip_suffix("mm") {
        let mm: f32 = mm
            .parse()
            .with_context(|| format!("mm 값이 숫자가 아닙니다: {value:?}"))?;
        mm * 72.0 / 25.4
    } else {
        anyhow::bail!(
            "크기 값은 pt 또는 mm 단위여야 합니다: {value:?} (%는 절대 pt 기준이 없습니다)"
        )
    };
    if !pt.is_finite() || !(0.0..=PT_MAX).contains(&pt) {
        anyhow::bail!("pt 값은 유한한 0..={PT_MAX} 범위여야 합니다: {value:?}");
    }
    Ok(pt)
}

/// Raw flat paragraph properties for set_para/set_cell_para, also deserialized as the
/// nested insert_para.style object. Fields match schema paraPropsFields (D-08), with
/// one spelling per concept. para_props interprets unit strings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawParaProps {
    line_spacing_pct: Option<String>,
    line_spacing_pt: Option<String>,
    indent_mm: Option<String>,
    left_mm: Option<String>,
    right_mm: Option<String>,
    top_mm: Option<String>,
    bottom_mm: Option<String>,
    align: Option<String>,
}

/// Raw flat character properties for set_format, also deserialized as the nested
/// insert_para.char object with the same schema charPropsFields set (D-08).
/// Do not use serde(flatten): operation files must remain a flat array of flat tagged
/// entries (06-01). Keep individual set_format fields; char_format shares conversion
/// logic across the two use sites.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCharProps {
    bold: Option<String>,
    italic: Option<String>,
    underline: Option<String>,
    strike: Option<String>,
    size: Option<String>,
    color: Option<String>,
    font: Option<String>,
}

/// A primary style name or its explicit header-table index; strings are never coerced to ids.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum RawStyleSelector {
    Name(String),
    Id(u16),
}
impl RawStyleSelector {
    fn into_selector(self) -> hwp_convert::styles::edit::StyleSelector {
        match self {
            Self::Name(name) => hwp_convert::styles::edit::StyleSelector::Name(name),
            Self::Id(id) => hwp_convert::styles::edit::StyleSelector::Id(id),
        }
    }
}

fn named_style_kind(value: &str) -> Result<hwp_convert::styles::StyleKind, String> {
    match value {
        "paragraph" => Ok(hwp_convert::styles::StyleKind::Paragraph),
        "character" => Ok(hwp_convert::styles::StyleKind::Character),
        _ => Err(
            "create_style.kind는 paragraph 또는 character여야 합니다. 지원되는 종류를 지정하세요"
                .into(),
        ),
    }
}

/// Convert RawCharProps into CharFormat. Both flat set_format fields and nested
/// insert_para.char go through this function and share the same error wording.
fn char_format(raw: RawCharProps) -> Result<hwp_convert::CharFormat, String> {
    Ok(hwp_convert::CharFormat {
        bold: parse_switch(raw.bold.as_deref())?,
        italic: parse_switch(raw.italic.as_deref())?,
        underline: parse_switch(raw.underline.as_deref())?,
        strike: parse_switch(raw.strike.as_deref())?,
        size_pt: match raw.size.as_deref() {
            Some(value) => Some(string_error(parse_pt(value))?),
            None => None,
        },
        color: match raw.color.as_deref() {
            Some(value) => Some(
                parse_color(value)
                    .ok_or_else(|| format!("char.color를 해석할 수 없습니다: {value:?}"))?,
            ),
            None => None,
        },
        // Schema minLength/pattern already reject an empty or control-character name (T-07-17);
        // this is the parser's own belt-and-suspenders check, matching the house discipline of
        // every other field here re-validating past the schema's shape-only guarantee.
        font: match raw.font.as_deref() {
            Some("") => return Err("char.font는 비어 있을 수 없습니다".to_string()),
            Some(value) => Some(value.to_string()),
            None => None,
        },
    })
}

/// Maximum index-chain depth an `id` address's path may carry (T-07-03), mirroring
/// `hwp_convert::address::MAX_ADDRESS_INDICES`. Documents nest far shallower; the resolver
/// iterates rather than recurses over the chain, so this bounds parse-time work here.
const MAX_ADDRESS_INDICES: usize = hwp_convert::address::MAX_ADDRESS_INDICES;

/// `$defs.addressPath` — a raw positional address naming a top-level paragraph (D-12/D-05).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddressPathSpec {
    section: usize,
    paragraph: usize,
    run: Option<usize>,
}

/// `$defs.address` — mirrors the schema's `id` XOR `at` plus optional `chars` shape; the
/// exactly-one-of check and the `id`-string parse both happen in [`AddressSpec::into_address`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddressSpec {
    id: Option<String>,
    at: Option<AddressPathSpec>,
    chars: Option<[u32; 2]>,
}

impl AddressSpec {
    /// Enforce exactly one of id/at and parse id here. The schema checks shape only,
    /// following the same two-stage structure as the D-10 contract.
    fn into_address(self) -> Result<hwp_convert::address::Address, String> {
        match (self.id, self.at) {
            (Some(_), Some(_)) => {
                Err("address 항목은 id와 at 중 하나만 지정해야 합니다".to_string())
            }
            (None, None) => Err("address 항목에 id 또는 at가 필요합니다".to_string()),
            (Some(id), None) => {
                let (checksum, section, indices) = parse_segment_id(&id)?;
                Ok(hwp_convert::address::Address {
                    section,
                    indices,
                    checksum: Some(checksum),
                    chars: self.chars.map(|[s, e]| (s, e)),
                })
            }
            (None, Some(at)) => {
                let mut indices = vec![at.paragraph];
                if let Some(run) = at.run {
                    indices.push(run);
                }
                Ok(hwp_convert::address::Address {
                    section: at.section,
                    indices,
                    checksum: None,
                    chars: self.chars.map(|[s, e]| (s, e)),
                })
            }
        }
    }
}

/// Split checksum.section.index[.index...] into checksum, section and index chain,
/// matching segment_id.rs join(). Refuse more than 32 indices or noninteger
/// components with Korean errors (T-07-03).
fn parse_segment_id(id: &str) -> Result<(String, usize, Vec<usize>), String> {
    let mut parts = id.split('.');
    let checksum = parts
        .next()
        .filter(|s| {
            s.len() == 16
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
        .ok_or_else(|| format!("id의 체크섬 형식이 올바르지 않습니다: {id:?}"))?
        .to_string();
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return Err(format!("id에 위치 경로가 없습니다: {id:?}"));
    }
    if rest.len() > MAX_ADDRESS_INDICES + 1 {
        return Err(format!(
            "id의 위치 경로가 상한({MAX_ADDRESS_INDICES}개)을 초과했습니다: {id:?}"
        ));
    }
    let numbers = rest
        .iter()
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| format!("id의 위치 값이 정수가 아닙니다: {id:?}"))
        })
        .collect::<Result<Vec<usize>, String>>()?;
    let section = numbers[0];
    let indices = numbers[1..].to_vec();
    if indices.is_empty() {
        return Err(format!("id에 문단 경로가 없습니다: {id:?}"));
    }
    Ok((checksum, section, indices))
}

/// `move_para.to` — a reference address plus `before`/`after` positioning (D-17 planner
/// decision 1). `position`'s before/after validity is checked in [`OpsEntry::into_typed`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MoveToSpec {
    address: AddressSpec,
    position: String,
}

/// Raw schema-entry representation: units/colors/alignment/switches arrive as strings
/// and OpsEntry::into_typed interprets them. Unknown keys are refused.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OpsEntry {
    ApplyFormBindings {
        #[serde(flatten)]
        request: crate::commands::edit::form_bindings::Request,
    },
    Replace {
        /// Always required — unlike `pattern` on `set_para`/`set_align`, `from` is never purely
        /// a selector, it is always the matched substring (Task 3's WCHAR-drift fixture depends
        /// on this: an addressed replace still narrows a substring, not the whole paragraph).
        from: String,
        /// Narrows the from/to match from the whole document (default) to inside one paragraph
        /// (D-12). Additive, not an alternative to `from`.
        address: Option<AddressSpec>,
        to: String,
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
        value: Option<String>,
    },
    CreateBookmark {
        anchor: String,
        name: String,
    },
    CreateHyperlink {
        anchor: String,
        display: Option<String>,
        url: String,
    },
    InsertImage {
        anchor: String,
        path: String,
        width_mm: Option<String>,
        height_mm: Option<String>,
    },
    Seal {
        anchor: String,
        path: String,
        size_mm: Option<String>,
    },
    SetField {
        name: String,
        value: String,
    },
    SetMeta {
        key: String,
        value: String,
    },
    CreateStyle {
        name: String,
        based_on: Option<RawStyleSelector>,
        kind: String,
        bold: Option<String>,
        italic: Option<String>,
        underline: Option<String>,
        strike: Option<String>,
        size: Option<String>,
        color: Option<String>,
        font: Option<String>,
        line_spacing_pct: Option<String>,
        line_spacing_pt: Option<String>,
        indent_mm: Option<String>,
        left_mm: Option<String>,
        right_mm: Option<String>,
        top_mm: Option<String>,
        bottom_mm: Option<String>,
        align: Option<String>,
    },
    UpdateStyle {
        style: RawStyleSelector,
        bold: Option<String>,
        italic: Option<String>,
        underline: Option<String>,
        strike: Option<String>,
        size: Option<String>,
        color: Option<String>,
        font: Option<String>,
        line_spacing_pct: Option<String>,
        line_spacing_pt: Option<String>,
        indent_mm: Option<String>,
        left_mm: Option<String>,
        right_mm: Option<String>,
        top_mm: Option<String>,
        bottom_mm: Option<String>,
        align: Option<String>,
    },
    ApplyStyle {
        address: AddressSpec,
        style: RawStyleSelector,
        clear_direct: Option<bool>,
    },
    SetFormat {
        pattern: Option<String>,
        address: Option<AddressSpec>,
        bold: Option<String>,
        italic: Option<String>,
        underline: Option<String>,
        strike: Option<String>,
        size: Option<String>,
        color: Option<String>,
        font: Option<String>,
    },
    SetAlign {
        pattern: Option<String>,
        address: Option<AddressSpec>,
        align: String,
    },
    InsertBlocks {
        address: AddressSpec,
        #[serde(flatten)]
        request: hwp_cli::insert_blocks::InsertBlocks,
    },
    InsertPara {
        /// Exactly one of anchor or address is required — the same D-12 selector shape
        /// SetPara/SetAlign already use.
        anchor: Option<String>,
        address: Option<AddressSpec>,
        text: String,
        before: Option<bool>,
        /// Inline paragraph-shape properties for the new paragraph itself (D-08).
        style: Option<RawParaProps>,
        /// Inline char-format properties for the new paragraph's own run (D-08).
        char: Option<RawCharProps>,
    },
    DeletePara {
        matching: Option<String>,
        address: Option<AddressSpec>,
    },
    MovePara {
        address: AddressSpec,
        to: MoveToSpec,
    },
    IndentPara {
        address: AddressSpec,
    },
    OutdentPara {
        address: AddressSpec,
    },
    AddRow {
        table: usize,
        at: Option<u16>,
        count: Option<usize>,
        template_row: Option<u16>,
    },
    AddCol {
        table: usize,
        at: Option<u16>,
        count: Option<u16>,
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
        text_mode: Option<String>,
    },
    SetPara {
        pattern: Option<String>,
        address: Option<AddressSpec>,
        line_spacing_pct: Option<String>,
        line_spacing_pt: Option<String>,
        indent_mm: Option<String>,
        left_mm: Option<String>,
        right_mm: Option<String>,
        top_mm: Option<String>,
        bottom_mm: Option<String>,
        align: Option<String>,
    },
    SetCellPara {
        table: usize,
        row: u16,
        col: u16,
        line_spacing_pct: Option<String>,
        line_spacing_pt: Option<String>,
        indent_mm: Option<String>,
        left_mm: Option<String>,
        right_mm: Option<String>,
        top_mm: Option<String>,
        bottom_mm: Option<String>,
        align: Option<String>,
    },
    SetPage {
        width_mm: Option<String>,
        height_mm: Option<String>,
        margin_left_mm: Option<String>,
        margin_right_mm: Option<String>,
        margin_top_mm: Option<String>,
        margin_bottom_mm: Option<String>,
        orientation: Option<String>,
    },
    DeleteImage {
        anchor: String,
    },
    DeleteTable {
        index: Option<usize>,
        anchor: Option<String>,
    },
    DeleteField {
        name: String,
    },
    DeleteBookmark {
        name: String,
    },
    StyleTables {
        preset: String,
    },
    /// #296 table placement: placement is inline or floating; table is a zero-based recursive
    /// table index, omitted to select all tables.
    SetTablePlacement {
        placement: String,
        table: Option<usize>,
    },
}

impl OpsEntry {
    /// Convert a schema-valid raw entry into Op. Enforce semantic constraints here:
    /// exactly one selector, paired dimensions, minimum counts and exclusive line spacing.
    /// Error wording matches the MCP tool_edit boundary.
    pub(crate) fn into_typed(self) -> Result<TypedEditOperation, String> {
        match self {
            OpsEntry::ApplyFormBindings { request } => {
                request.plan.validate_values(&request.values)?;
                Ok(TypedEditOperation::ApplyFormBindings { request })
            }
            OpsEntry::Replace { from, address, to } => {
                let address = address.map(AddressSpec::into_address).transpose()?;
                Ok(TypedEditOperation::Replace { from, to, address })
            }
            OpsEntry::SetCell {
                table,
                row,
                col,
                text,
            } => Ok(TypedEditOperation::SetCell {
                table,
                row,
                col,
                text,
            }),
            OpsEntry::SetCellByLabel { label, text, table } => {
                Ok(TypedEditOperation::SetCellByLabel { label, text, table })
            }
            OpsEntry::CreateField {
                anchor,
                name,
                value,
            } => Ok(TypedEditOperation::CreateField {
                anchor,
                name,
                value: value.unwrap_or_default(),
            }),
            OpsEntry::CreateBookmark { anchor, name } => {
                Ok(TypedEditOperation::CreateBookmark { anchor, name })
            }
            OpsEntry::CreateHyperlink {
                anchor,
                display,
                url,
            } => Ok(TypedEditOperation::CreateHyperlink {
                anchor,
                display: display.unwrap_or_else(|| url.clone()),
                url,
            }),
            OpsEntry::InsertImage {
                anchor,
                path,
                width_mm,
                height_mm,
            } => {
                let size_mm = match (width_mm.as_deref(), height_mm.as_deref()) {
                    (Some(width), Some(height)) => {
                        let width = string_error(parse_mm_f32(width))?;
                        let height = string_error(parse_mm_f32(height))?;
                        Some((width, height))
                    }
                    (None, None) => None,
                    _ => {
                        return Err(
                            "insert_image는 유한한 width_mm와 height_mm를 함께 지정해야 합니다"
                                .into(),
                        );
                    }
                };
                Ok(TypedEditOperation::InsertImage {
                    anchor,
                    path: checked_cli_read_path(&path)?,
                    size_mm,
                })
            }
            OpsEntry::Seal {
                anchor,
                path,
                size_mm,
            } => Ok(TypedEditOperation::Seal {
                anchor,
                path: checked_cli_read_path(&path)?,
                size_mm: string_error(size_mm.as_deref().map(parse_mm_f32).transpose())?,
            }),
            OpsEntry::SetField { name, value } => Ok(TypedEditOperation::SetField { name, value }),
            OpsEntry::SetMeta { key, value } => Ok(TypedEditOperation::SetMeta { key, value }),
            OpsEntry::CreateStyle {
                name,
                based_on,
                kind,
                bold,
                italic,
                underline,
                strike,
                size,
                color,
                font,
                line_spacing_pct,
                line_spacing_pt,
                indent_mm,
                left_mm,
                right_mm,
                top_mm,
                bottom_mm,
                align,
            } => Ok(TypedEditOperation::CreateStyle {
                request: hwp_convert::styles::edit::CreateStyle {
                    name,
                    based_on: based_on.map(RawStyleSelector::into_selector),
                    kind: named_style_kind(&kind)?,
                    character: char_format(RawCharProps {
                        bold,
                        italic,
                        underline,
                        strike,
                        size,
                        color,
                        font,
                    })?,
                    paragraph: para_props(
                        "create_style",
                        RawParaProps {
                            line_spacing_pct,
                            line_spacing_pt,
                            indent_mm,
                            left_mm,
                            right_mm,
                            top_mm,
                            bottom_mm,
                            align,
                        },
                    )?,
                },
            }),
            OpsEntry::UpdateStyle {
                style,
                bold,
                italic,
                underline,
                strike,
                size,
                color,
                font,
                line_spacing_pct,
                line_spacing_pt,
                indent_mm,
                left_mm,
                right_mm,
                top_mm,
                bottom_mm,
                align,
            } => Ok(TypedEditOperation::UpdateStyle {
                request: hwp_convert::styles::edit::UpdateStyle {
                    style: style.into_selector(),
                    character: char_format(RawCharProps {
                        bold,
                        italic,
                        underline,
                        strike,
                        size,
                        color,
                        font,
                    })?,
                    paragraph: para_props(
                        "update_style",
                        RawParaProps {
                            line_spacing_pct,
                            line_spacing_pt,
                            indent_mm,
                            left_mm,
                            right_mm,
                            top_mm,
                            bottom_mm,
                            align,
                        },
                    )?,
                },
            }),
            OpsEntry::ApplyStyle {
                address,
                style,
                clear_direct,
            } => Ok(TypedEditOperation::ApplyStyle {
                address: address.into_address()?,
                style: style.into_selector(),
                clear_direct: clear_direct.unwrap_or(false),
            }),
            OpsEntry::SetFormat {
                pattern,
                address,
                bold,
                italic,
                underline,
                strike,
                size,
                color,
                font,
            } => {
                exactly_one_selector(
                    "set_format",
                    "pattern",
                    pattern.is_some(),
                    address.is_some(),
                )?;
                let format = char_format(RawCharProps {
                    bold,
                    italic,
                    underline,
                    strike,
                    size,
                    color,
                    font,
                })?;
                let address = address.map(AddressSpec::into_address).transpose()?;
                Ok(TypedEditOperation::SetFormat {
                    pattern: pattern.unwrap_or_default(),
                    format,
                    address,
                })
            }
            OpsEntry::SetAlign {
                pattern,
                address,
                align,
            } => {
                exactly_one_selector("set_align", "pattern", pattern.is_some(), address.is_some())?;
                let address = address.map(AddressSpec::into_address).transpose()?;
                Ok(TypedEditOperation::SetAlign {
                    pattern: pattern.unwrap_or_default(),
                    align: parse_align(&align).map_err(|error| error.to_string())?,
                    address,
                })
            }
            OpsEntry::InsertBlocks { address, request } => {
                let address = address.into_address()?;
                if address.chars.is_some() {
                    return Err("insert_blocks는 문단 주소만 지원합니다. chars 범위를 제거한 뒤 다시 시도하세요".into());
                }
                Ok(TypedEditOperation::InsertBlocks { address, request })
            }
            OpsEntry::InsertPara {
                anchor,
                address,
                text,
                before,
                style,
                char,
            } => {
                exactly_one_selector("insert_para", "anchor", anchor.is_some(), address.is_some())?;
                let address = address.map(AddressSpec::into_address).transpose()?;
                let style = style
                    .map(|raw| para_props("insert_para", raw))
                    .transpose()?;
                let char = char.map(char_format).transpose()?;
                Ok(TypedEditOperation::InsertPara {
                    anchor: anchor.unwrap_or_default(),
                    text,
                    before: before.unwrap_or(false),
                    address,
                    style,
                    char,
                })
            }
            OpsEntry::DeletePara { matching, address } => {
                exactly_one_selector(
                    "delete_para",
                    "matching",
                    matching.is_some(),
                    address.is_some(),
                )?;
                let address = address.map(AddressSpec::into_address).transpose()?;
                Ok(TypedEditOperation::DeletePara {
                    matching: matching.unwrap_or_default(),
                    address,
                })
            }
            OpsEntry::MovePara { address, to } => {
                let address = address.into_address()?;
                let to_address = to.address.into_address()?;
                let before = match to.position.as_str() {
                    "before" => true,
                    "after" => false,
                    other => {
                        return Err(format!(
                            "move_para.to.position은 before 또는 after여야 합니다: {other:?}"
                        ));
                    }
                };
                Ok(TypedEditOperation::MoveParagraph {
                    address,
                    to_address,
                    before,
                })
            }
            OpsEntry::IndentPara { address } => {
                let address = address.into_address()?;
                Ok(TypedEditOperation::IndentPara { address })
            }
            OpsEntry::OutdentPara { address } => {
                let address = address.into_address()?;
                Ok(TypedEditOperation::OutdentPara { address })
            }
            OpsEntry::AddRow {
                table,
                at,
                count,
                template_row,
            } => {
                let count = count.unwrap_or(1);
                if count == 0 {
                    return Err("add_row: count는 1 이상이어야 합니다".to_string());
                }
                Ok(TypedEditOperation::AddRow {
                    table,
                    at,
                    count,
                    template_row,
                })
            }
            OpsEntry::AddCol { table, at, count } => {
                let count = count.unwrap_or(1);
                if count == 0 {
                    return Err("add_col: count는 1 이상이어야 합니다".to_string());
                }
                Ok(TypedEditOperation::AddCol { table, at, count })
            }
            OpsEntry::DeleteRow { table, row } => Ok(TypedEditOperation::DeleteRow { table, row }),
            OpsEntry::DeleteCol { table, col } => Ok(TypedEditOperation::DeleteCol { table, col }),
            OpsEntry::MergeCells {
                table,
                r1,
                c1,
                r2,
                c2,
            } => Ok(TypedEditOperation::MergeCells {
                table,
                r1,
                c1,
                r2,
                c2,
            }),
            OpsEntry::SplitCell { table, row, col } => {
                Ok(TypedEditOperation::SplitCell { table, row, col })
            }
            OpsEntry::AddTable { anchor, rows } => {
                Ok(TypedEditOperation::AddTable { anchor, rows })
            }
            OpsEntry::CloneTable {
                source_table,
                anchor,
                text_mode,
            } => {
                let text_mode = match text_mode.as_deref().map(str::trim) {
                    None | Some("") | Some("blank") => hwp_convert::CloneTextMode::Blank,
                    Some("keep") => hwp_convert::CloneTextMode::Keep,
                    Some(_) => {
                        return Err("clone_table: text_mode는 blank|keep 이어야 합니다".to_string());
                    }
                };
                Ok(TypedEditOperation::CloneTable {
                    source_table,
                    anchor,
                    text_mode,
                })
            }
            OpsEntry::SetPara {
                pattern,
                address,
                line_spacing_pct,
                line_spacing_pt,
                indent_mm,
                left_mm,
                right_mm,
                top_mm,
                bottom_mm,
                align,
            } => {
                exactly_one_selector("set_para", "pattern", pattern.is_some(), address.is_some())?;
                let address = address.map(AddressSpec::into_address).transpose()?;
                Ok(TypedEditOperation::SetPara {
                    pattern: pattern.unwrap_or_default(),
                    props: para_props(
                        "set_para",
                        RawParaProps {
                            line_spacing_pct,
                            line_spacing_pt,
                            indent_mm,
                            left_mm,
                            right_mm,
                            top_mm,
                            bottom_mm,
                            align,
                        },
                    )?,
                    address,
                })
            }
            OpsEntry::SetCellPara {
                table,
                row,
                col,
                line_spacing_pct,
                line_spacing_pt,
                indent_mm,
                left_mm,
                right_mm,
                top_mm,
                bottom_mm,
                align,
            } => Ok(TypedEditOperation::SetCellPara {
                table,
                row,
                col,
                props: para_props(
                    "set_cell_para",
                    RawParaProps {
                        line_spacing_pct,
                        line_spacing_pt,
                        indent_mm,
                        left_mm,
                        right_mm,
                        top_mm,
                        bottom_mm,
                        align,
                    },
                )?,
            }),
            OpsEntry::SetPage {
                width_mm,
                height_mm,
                margin_left_mm,
                margin_right_mm,
                margin_top_mm,
                margin_bottom_mm,
                orientation,
            } => {
                let props = hwp_convert::PageProps {
                    width: mm_opt(width_mm)?,
                    height: mm_opt(height_mm)?,
                    margin_left: mm_opt(margin_left_mm)?,
                    margin_right: mm_opt(margin_right_mm)?,
                    margin_top: mm_opt(margin_top_mm)?,
                    margin_bottom: mm_opt(margin_bottom_mm)?,
                    // No lowercasing here: the schema's `orientation` enum is
                    // case-sensitive (WR-01), so this arm only ever sees the
                    // exact-cased values already listed below.
                    landscape: match orientation.as_deref() {
                        Some(value) => Some(match value.trim() {
                            "landscape" | "가로" => true,
                            "portrait" | "세로" => false,
                            other => {
                                return Err(format!(
                                    "알 수 없는 용지 방향: {other:?} (portrait/landscape)"
                                ));
                            }
                        }),
                        None => None,
                    },
                };
                Ok(TypedEditOperation::SetPage { props })
            }
            OpsEntry::DeleteImage { anchor } => Ok(TypedEditOperation::DeleteImage { anchor }),
            OpsEntry::DeleteTable { index, anchor } => match (index.as_ref(), anchor.as_ref()) {
                (Some(_), Some(_)) => {
                    Err("delete_table 항목은 index와 anchor 중 하나만 지정해야 합니다".to_string())
                }
                (None, None) => {
                    Err("delete_table 항목에 index 또는 anchor가 필요합니다".to_string())
                }
                _ => Ok(TypedEditOperation::DeleteTable { index, anchor }),
            },
            OpsEntry::DeleteField { name } => Ok(TypedEditOperation::DeleteField { name }),
            OpsEntry::DeleteBookmark { name } => Ok(TypedEditOperation::DeleteBookmark { name }),
            OpsEntry::StyleTables { preset } => Ok(TypedEditOperation::StyleTables {
                preset: hwp_convert::OfficialPreset::parse(&preset)?,
            }),
            OpsEntry::SetTablePlacement { placement, table } => {
                let placement = match placement.as_str() {
                    "inline" => hwp_convert::TablePlacement::Inline,
                    "floating" => hwp_convert::TablePlacement::Floating,
                    other => {
                        return Err(format!(
                            "set_table_placement의 placement는 inline|floating 중 하나여야 합니다: {other}"
                        ));
                    }
                };
                Ok(TypedEditOperation::SetTablePlacement { placement, table })
            }
        }
    }
}

/// The exactly-one rule between a legacy text selector (`key`: `pattern`, `anchor` or
/// `matching`) and `address`. Shared by `into_typed` and the MCP `tool_edit` arms (#350), so both
/// channels refuse the same items with the same text.
pub(crate) fn exactly_one_selector(
    op: &str,
    key: &str,
    has_selector: bool,
    has_address: bool,
) -> Result<(), String> {
    if has_selector != has_address {
        return Ok(());
    }
    let particle = if key == "anchor" { "와" } else { "과" };
    Err(if has_selector {
        format!("{op} 항목은 {key}{particle} address 중 하나만 지정해야 합니다")
    } else {
        format!("{op} 항목에 {key} 또는 address가 필요합니다")
    })
}

/// Convert raw flat paragraph properties into ParaProps. Enforce exclusive
/// line_spacing_pct/pt here, sharing error wording with MCP para_props_item.
fn para_props(op: &str, raw: RawParaProps) -> Result<hwp_convert::ParaProps, String> {
    let RawParaProps {
        line_spacing_pct,
        line_spacing_pt,
        indent_mm,
        left_mm,
        right_mm,
        top_mm,
        bottom_mm,
        align,
    } = raw;
    let line_spacing = match (line_spacing_pct.as_deref(), line_spacing_pt.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{op}는 line_spacing_pct와 line_spacing_pt를 함께 지정할 수 없습니다"
            ));
        }
        (Some(pct), None) => Some((0, (string_error(parse_pct(pct))?) as i32)),
        (None, Some(pt)) => Some((1, ((string_error(parse_pt(pt))?) * 100.0).round() as i32)),
        (None, None) => None,
    };
    Ok(hwp_convert::ParaProps {
        line_spacing,
        indent: mm_opt(indent_mm)?,
        margin_left: mm_opt(left_mm)?,
        margin_right: mm_opt(right_mm)?,
        spacing_top: mm_opt(top_mm)?,
        spacing_bottom: mm_opt(bottom_mm)?,
        align: match align.as_deref() {
            Some(name) => Some(parse_align(name).map_err(|error| format!("{op}: {error}"))?),
            None => None,
        },
    })
}

/// Convert on/off into bool, matching schema switch syntax.
fn parse_switch(value: Option<&str>) -> Result<Option<bool>, String> {
    match value {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(other) => Err(format!("스위치 값은 on 또는 off여야 합니다: {other:?}")),
    }
}

/// Convert *_mm unit strings into HWPUNIT.
fn mm_opt(value: Option<String>) -> Result<Option<i32>, String> {
    string_error(value.as_deref().map(parse_mm).transpose())
}

/// Convert anyhow errors to into_typed String errors, matching the MCP boundary contract.
fn string_error<T>(result: anyhow::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_style_transport_schemas_remain_closed_and_match_ops() {
        use serde_json::json;
        for (operation, item) in [
            (
                "create_style",
                json!({"name":"Body", "kind":"paragraph", "size":"13pt"}),
            ),
            ("update_style", json!({"style":0, "size":"13pt"})),
            (
                "apply_style",
                json!({"style":0, "address":{"at":{"section":0,"paragraph":0}}}),
            ),
        ] {
            let schema = named_style_schema(operation);
            let validator = jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .build(&schema)
                .unwrap();
            assert!(
                validator.is_valid(&json!([item.clone()])),
                "{operation} advertised schema must accept canonical payload"
            );
            assert!(
                parse_ops_value(json!([named_style_item(item.clone(), operation).unwrap()]))
                    .is_ok()
            );
            let mut extra = item.clone();
            extra["surprise"] = json!(true);
            assert!(
                !validator.is_valid(&json!([extra.clone()])),
                "{operation} schema must reject unknown fields"
            );
            assert!(parse_ops_value(json!([named_style_item(extra, operation).unwrap()])).is_err());
            let mut spoofed = item;
            spoofed["op"] = json!("replace");
            assert!(!validator.is_valid(&json!([spoofed.clone()])));
            assert!(named_style_item(spoofed, operation).is_err());
        }
        let schema = named_style_schema("apply_style");
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(&schema)
            .unwrap();
        for address in [
            json!({"at":{"section":0,"paragraph":0,"run":0}}),
            json!({"at":{"section":0,"paragraph":0}, "chars":[0,1]}),
            json!({"at":{"section":-1,"paragraph":0}}),
        ] {
            assert!(!validator.is_valid(&json!([{"style":0,"address":address}])));
        }
    }

    /// Read-path checks at the ops boundary: refuse parent-directory components with Korean errors.
    /// Integration tests cannot reach pub(crate) checked_cli_read_path, so test it
    /// inside the crate.
    #[test]
    fn checked_cli_read_path_rejects_parent_dir() {
        let error =
            checked_cli_read_path("../outside/ops.json").expect_err("`..` must be rejected");
        assert!(
            error.contains("'..'를 포함한 입력 경로는 거부합니다"),
            "거부 오류 문구가 일치하지 않는다: {error}"
        );
        assert!(
            error.contains("../outside/ops.json"),
            "오류에 경로가 있어야 한다: {error}"
        );
    }
}
