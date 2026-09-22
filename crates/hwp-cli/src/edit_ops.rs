//! `hwp edit --ops <file.json>` — 타입화된 편집 연산 파일의 검증·변환 계층 (D-10).
//!
//! 규범 스키마는 `schemas/edit-ops-v1.schema.json`이고, 이 모듈은 그 serde 구현
//! ([`OpsEntry`])와 런타임 검증기를 담는다. 스키마는 모양만 검사한다 — 선택자
//! exactly-one, 짝 단위 이미지 크기, count 하한, 줄 간격 상호 배타 같은 의미 제약은
//! [`OpsEntry::into_typed`]가 스키마 통과 뒤에 강제한다. MCP `tool_edit` 경계와 같은
//! 2단 계약이며, 제약 오류는 같은 한국어 문구를 쓴다.

use std::io::Read as _;
use std::path::Path;

use anyhow::Context as _;
use serde::Deserialize;

use crate::commands::edit::TypedEditOperation;
use crate::commands::edit::{mm_to_hwpunit, parse_align, parse_color};
use crate::commands::mcp::checked_cli_read_path;

/// 편집 연산 입력 크기 상한 — MCP `MAX_READ_BYTES` 규약(16 MiB)과 같다.
pub(crate) const MAX_OPS_BYTES: u64 = 16 * 1024 * 1024;
/// 편집 연산 항목 수 상한. 스키마 maxItems와 같은 값 — 파싱 뒤 한 번 더 검사한다.
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

/// 편집 연산 파일(`"-"`는 stdin)을 상한 안에서 UTF-8로 읽는다.
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

/// Bounded UTF-8 read (lint.rs의 read_bounded와 같은 규약): 상한 초과 입력은 한국어
/// 오류로 거부 — 조용한 잘라내기가 아니다. non-UTF-8 입력도 패닉 없이 읽기 오류로
/// 표면화한다.
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

/// 편집 연산 파일을 읽고 스키마 검증 → [`OpsEntry`] 역직렬화 → [`Op`] 변환까지
/// 수행한다. 입력 오류·스키마 위반·의미 제약 위반 모두 한국어 오류로 거부하며,
/// 스키마 위반 메시지는 인스턴스 경로와 jsonschema 오류 문구를 그대로 담는다.
pub(crate) fn load_ops(path: &Path) -> anyhow::Result<Vec<TypedEditOperation>> {
    let raw = read_ops_source(path)?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| anyhow::anyhow!("편집 연산 JSON을 해석할 수 없습니다: {error}"))?;
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/edit-ops-v1.schema.json"))
            .map_err(|error| anyhow::anyhow!("edit-ops-v1 스키마를 해석할 수 없습니다: {error}"))?;
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .map_err(|error| anyhow::anyhow!("edit-ops-v1 스키마를 컴파일할 수 없습니다: {error}"))?;
    if let Some(error) = validator.iter_errors(&value).next() {
        anyhow::bail!(
            "편집 연산이 edit-ops-v1 스키마를 벗어났습니다: {}: {error}",
            error.instance_path
        );
    }
    let entries: Vec<OpsEntry> = serde_json::from_value(value)
        .map_err(|error| anyhow::anyhow!("편집 연산을 해석할 수 없습니다: {error}"))?;
    if entries.len() > MAX_OPS_ITEMS {
        anyhow::bail!("편집 연산이 항목 상한({MAX_OPS_ITEMS}개)을 초과했습니다");
    }
    entries
        .into_iter()
        .map(OpsEntry::into_typed)
        .collect::<Result<Vec<_>, String>>()
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// "10.5mm" → mm 값 (f32). `*_mm` 필드의 단위 문자열 문법.
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

/// "10.5mm" → HWPUNIT (1mm = 7200/25.4). CLI `parse_mm`(edit.rs)와 같은 문법·단위.
pub(crate) fn parse_mm(value: &str) -> anyhow::Result<i32> {
    Ok(mm_to_hwpunit(parse_mm_f32(value)?))
}

/// "160%" → 백분율 값 (f32). `line_spacing_pct` 등 백분율 필드의 문법.
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

/// 크기 단위 문자열 → pt (f32). "12pt"는 그대로, "10mm"는 1in=72pt=25.4mm로 환산.
/// "%"는 절대 pt 기준이 없어 거부한다 (스키마 `unit` 문법과의 차이는 파서 강제).
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

/// set_para/set_cell_para의 평면 문단 속성 원시 값 (스키마 필드 그대로). 두 op가
/// 같은 8개 필드를 공유하며, 단위 문자열 해석은 [`para_props`]가 맡는다.
struct RawParaProps {
    line_spacing_pct: Option<String>,
    line_spacing_pt: Option<String>,
    indent_mm: Option<String>,
    left_mm: Option<String>,
    right_mm: Option<String>,
    top_mm: Option<String>,
    bottom_mm: Option<String>,
    align: Option<String>,
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
    /// 의미 제약(`id`/`at` 중 하나만)과 `id` 문자열 파싱을 여기서 강제한다 — 스키마는
    /// 모양만 검사한다(D-10 계약과 같은 2단 구조).
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

/// `<checksum>.<section>.<index>[.<index>...]` 형태의 segment id를 체크섬·섹션·인덱스
/// 체인으로 나눈다(`segment_id.rs`의 `join()`이 만드는 정확히 그 모양). 32개 초과
/// 인덱스와 정수가 아닌 성분을 한국어 오류로 거부한다(T-07-03).
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

/// 스키마 항목 그대로의 원시 표현 — 단위·색·정렬·스위치는 문자열로 받고
/// [`OpsEntry::into_typed`]에서 해석한다. 알 수 없는 키는 거부한다.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OpsEntry {
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
    SetFormat {
        pattern: Option<String>,
        address: Option<AddressSpec>,
        bold: Option<String>,
        italic: Option<String>,
        underline: Option<String>,
        strike: Option<String>,
        size: Option<String>,
        color: Option<String>,
    },
    SetAlign {
        pattern: Option<String>,
        address: Option<AddressSpec>,
        align: String,
    },
    InsertPara {
        anchor: String,
        text: String,
        before: Option<bool>,
    },
    DeletePara {
        matching: String,
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
}

impl OpsEntry {
    /// 스키마를 통과한 원시 항목을 [`Op`]로 변환한다. 의미 제약(exactly-one 선택자,
    /// 짝 단위 크기, count 하한, 줄 간격 상호 배타)은 여기서 강제하며 오류 문구는
    /// MCP `tool_edit` 경계와 같다.
    pub(crate) fn into_typed(self) -> Result<TypedEditOperation, String> {
        match self {
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
            OpsEntry::SetFormat {
                pattern,
                address,
                bold,
                italic,
                underline,
                strike,
                size,
                color,
            } => {
                if pattern.is_some() == address.is_some() {
                    return Err(if pattern.is_some() {
                        "set_format 항목은 pattern과 address 중 하나만 지정해야 합니다".to_string()
                    } else {
                        "set_format 항목에 pattern 또는 address가 필요합니다".to_string()
                    });
                }
                let format = hwp_convert::CharFormat {
                    bold: parse_switch(bold.as_deref())?,
                    italic: parse_switch(italic.as_deref())?,
                    underline: parse_switch(underline.as_deref())?,
                    strike: parse_switch(strike.as_deref())?,
                    size_pt: match size.as_deref() {
                        Some(value) => Some(string_error(parse_pt(value))?),
                        None => None,
                    },
                    color: match color.as_deref() {
                        Some(value) => Some(parse_color(value).ok_or_else(|| {
                            format!("set_format.color를 해석할 수 없습니다: {value:?}")
                        })?),
                        None => None,
                    },
                };
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
                if pattern.is_some() == address.is_some() {
                    return Err(if pattern.is_some() {
                        "set_align 항목은 pattern과 address 중 하나만 지정해야 합니다".to_string()
                    } else {
                        "set_align 항목에 pattern 또는 address가 필요합니다".to_string()
                    });
                }
                let address = address.map(AddressSpec::into_address).transpose()?;
                Ok(TypedEditOperation::SetAlign {
                    pattern: pattern.unwrap_or_default(),
                    align: parse_align(&align).map_err(|error| error.to_string())?,
                    address,
                })
            }
            OpsEntry::InsertPara {
                anchor,
                text,
                before,
            } => Ok(TypedEditOperation::InsertPara {
                anchor,
                text,
                before: before.unwrap_or(false),
            }),
            OpsEntry::DeletePara { matching } => Ok(TypedEditOperation::DeletePara { matching }),
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
                if pattern.is_some() == address.is_some() {
                    return Err(if pattern.is_some() {
                        "set_para 항목은 pattern과 address 중 하나만 지정해야 합니다".to_string()
                    } else {
                        "set_para 항목에 pattern 또는 address가 필요합니다".to_string()
                    });
                }
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
        }
    }
}

/// 평면 문단 속성 원시 값 → ParaProps. line_spacing_pct/pt 상호 배타는 여기서
/// 강제하며, 오류 문구는 MCP `para_props_item`과 같다.
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

/// "on"/"off" → bool. 스키마 `switch` 문법.
fn parse_switch(value: Option<&str>) -> Result<Option<bool>, String> {
    match value {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(other) => Err(format!("스위치 값은 on 또는 off여야 합니다: {other:?}")),
    }
}

/// `*_mm` 단위 문자열 → HWPUNIT.
fn mm_opt(value: Option<String>) -> Result<Option<i32>, String> {
    string_error(value.as_deref().map(parse_mm).transpose())
}

/// anyhow 오류를 into_typed의 String 오류로 맞춘다 — MCP 경계와 같은 String 계약.
fn string_error<T>(result: anyhow::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ops 경계의 읽기 경로 검사: `..` 구성요소는 한국어 오류로 거부한다.
    /// `checked_cli_read_path`가 pub(crate)이라 통합 테스트는 닿을 수 없어
    /// 크레이트 안에서 검사한다.
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
