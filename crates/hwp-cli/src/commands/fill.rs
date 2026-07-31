//! `hwp fill` — 템플릿 채우기.
//!
//! 두 경로: (1) **자리표시자 치환**(기본) — `Contents/section*.xml`의 `{{name}}`만
//! 외과 치환하고 나머지 패키지 엔트리(미리보기·compat·BinData)를 바이트 보존(hwpx
//! 입력 전용). (2) **데이터 구동 표 채우기** — `--data`에 `tables` 지시가 있으면 IR로
//! 읽어 표 행을 데이터 수만큼 늘리고(add_rows) 셀을 채운 뒤 다시 쓴다(.hwp/.hwpx 모두).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;

use crate::commands::cat::load_document;

#[derive(Debug)]
pub struct FillReport {
    pub output: String,
    pub mode: &'static str,
    pub replaced: usize,
    pub counts: BTreeMap<String, usize>,
    pub filled: usize,
    pub rows_added: usize,
    pub warnings: Vec<String>,
}

pub fn run(
    input: &Path,
    output: &Path,
    set: &[String],
    data: Option<&Path>,
    json: bool,
    allow_partial: bool,
) -> anyhow::Result<()> {
    let data_value: Option<serde_json::Value> = match data {
        Some(d) => {
            let text = std::fs::read_to_string(d)?;
            Some(
                serde_json::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("--data JSON 파싱 실패 ({}): {e}", d.display()))?,
            )
        }
        None => None,
    };

    let report = execute(input, output, set, data_value.as_ref(), allow_partial)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report_json(&report))?);
    } else if report.mode == "tables" {
        for warning in &report.warnings {
            eprintln!("경고: {warning}");
        }
        eprintln!(
            "[hwp] 표 채움: {}건 (+{}행) -> {}",
            report.filled, report.rows_added, report.output
        );
    } else {
        for warning in &report.warnings {
            eprintln!("경고: {warning}");
        }
        eprintln!("[hwp] {}건 치환 -> {}", report.replaced, report.output);
    }
    Ok(())
}

pub fn execute(
    input: &Path,
    output: &Path,
    set: &[String],
    data_value: Option<&serde_json::Value>,
    allow_partial: bool,
) -> anyhow::Result<FillReport> {
    // 데이터에 `tables`가 (객체 항목의) 비어있지 않은 배열이면 IR 기반 표 채우기로 분기.
    // 객체-배열만 인정해, "tables"라는 이름의 평범한 자리표시자(예: 문자열 배열 값)가
    // 표 채우기로 오인 라우팅돼 실패하지 않게 한다(평문 fill 경로로 떨어뜨림).
    let has_tables = data_value
        .as_ref()
        .and_then(|v| v.get("tables"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|arr| !arr.is_empty() && arr.iter().all(serde_json::Value::is_object));
    if has_tables {
        return fill_tables_ir(
            input,
            output,
            data_value.expect("has_tables로 확인됨"),
            set,
            allow_partial,
        );
    }

    // 기본 경로: {{name}} 자리표시자 바이트 보존 치환(hwpx 전용).
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    if let Some(serde_json::Value::Object(map)) = data_value {
        for (k, v) in map {
            values.insert(k.clone(), value_to_string(v));
        }
    } else if data_value.is_some() {
        anyhow::bail!("--data 최상위는 객체({{...}})여야 합니다");
    }
    for pair in set {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--set 형식은 name=value 여야 합니다: {pair}"))?;
        values.insert(k.to_string(), v.to_string());
    }
    if values.is_empty() {
        anyhow::bail!(
            "치환 값이 없습니다 (--set name=value / --data values.json / --data tables 지시)"
        );
    }

    // 자리표시자 치환은 HWPX(ZIP) 패키지 외과 수술 전용 — .hwp는 모호한 ZIP 오류 대신 명확히 거절.
    // (.hwp 표 채우기는 위 --data tables 경로가 IR로 처리한다.)
    if crate::format::detect(input)? != crate::format::FileFormat::Hwpx {
        anyhow::bail!(
            "{}: 자리표시자 치환(기본 fill)은 HWPX 입력 전용입니다 (.hwp는 --data의 tables 표 채우기만 지원)",
            input.display()
        );
    }

    execute_values(input, output, &values, allow_partial)
}

pub fn execute_values(
    input: &Path,
    output: &Path,
    values: &BTreeMap<String, String>,
    allow_partial: bool,
) -> anyhow::Result<FillReport> {
    if values.is_empty() {
        anyhow::bail!("치환 값이 없습니다");
    }
    if crate::format::detect(input)? != crate::format::FileFormat::Hwpx {
        anyhow::bail!(
            "{}: 자리표시자 치환(기본 fill)은 HWPX 입력 전용입니다",
            input.display()
        );
    }

    let mut report_warnings = Vec::new();
    let counts = crate::commands::output::write_validated(
        output,
        Some(input),
        |staged| {
            hwpx::patch::fill_placeholders(input, staged, values)
                .map_err(|e| anyhow::anyhow!("fill 실패: {e}"))
        },
        |staged, counts| {
            let total: usize = counts.values().sum();
            if total == 0 {
                anyhow::bail!("요청한 자리표시자를 하나도 찾지 못해 출력을 게시하지 않습니다");
            }
            let missing: Vec<&str> = counts
                .iter()
                .filter(|(_, count)| **count == 0)
                .map(|(name, _)| name.as_str())
                .collect();
            if !missing.is_empty() && !allow_partial {
                anyhow::bail!(
                    "요청한 자리표시자를 찾지 못했습니다: {} \
                     (--allow-partial로 일치한 값만 적용 가능)",
                    missing.join(", ")
                );
            }

            ensure_valid_document(staged)?;
            let staged_doc = load_document(staged)?;
            let unresolved: Vec<String> = hwp_convert::scan_placeholders(&staged_doc)
                .into_iter()
                .filter(|slot| values.contains_key(&slot.name))
                .map(|slot| slot.name)
                .collect();
            if !unresolved.is_empty() && !allow_partial {
                anyhow::bail!(
                    "치환 후에도 요청한 자리표시자가 남아 있습니다: {}",
                    unresolved.join(", ")
                );
            }
            Ok(())
        },
    )?;

    let missing: Vec<String> = counts
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(name, _)| name.clone())
        .collect();
    if !missing.is_empty() {
        report_warnings.push(format!("미치환 자리표시자: {}", missing.join(", ")));
    }
    let total = counts.values().sum();
    Ok(FillReport {
        output: output.display().to_string(),
        mode: "placeholders",
        replaced: total,
        counts,
        filled: total,
        rows_added: 0,
        warnings: report_warnings,
    })
}

/// 데이터 구동 표 채우기. `data`는 다음 형태:
/// ```json
/// {
///   "fields": {"부서": "기획팀"},
///   "tables": [
///     {"table": 0, "start_row": 1, "template_row": 1,
///      "rows": [["노트북", "5"], ["모니터", "10"]]}
///   ]
/// }
/// ```
/// `fields`(선택)는 `{{키}}`를 본문 전역 치환한다. 각 표는 `start_row`(기본 1)부터
/// `rows` 길이만큼 행이 차도록 자동으로 늘린 뒤(add_rows) 셀을 채운다.
fn fill_tables_ir(
    input: &Path,
    output: &Path,
    data: &serde_json::Value,
    set: &[String],
    allow_partial: bool,
) -> anyhow::Result<FillReport> {
    let mut doc = load_document(input)?;
    let original = doc.clone();
    let mut filled = 0usize;
    let mut added = 0usize;
    let mut warnings = Vec::new();
    let mut unmatched_fields = Vec::new();

    // 1) fields: {{키}} → 값. 우선순위: 최상위 스칼라(flat 스키마 호환) < data.fields < --set.
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    if let serde_json::Value::Object(top) = data {
        for (k, v) in top {
            if k == "fields" || k == "tables" || v.is_object() || v.is_array() {
                continue; // 예약 키·복합값 제외 — 최상위 스칼라만 흡수.
            }
            fields.insert(k.clone(), value_to_string(v));
        }
    }
    if let Some(serde_json::Value::Object(f)) = data.get("fields") {
        for (k, v) in f {
            fields.insert(k.clone(), value_to_string(v));
        }
    }
    for pair in set {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--set 형식은 name=value 여야 합니다: {pair}"))?;
        fields.insert(k.to_string(), v.to_string());
    }
    for (k, v) in &fields {
        let count = hwp_convert::replace_text(&mut doc, &format!("{{{{{k}}}}}"), v, true);
        if count == 0 {
            unmatched_fields.push(k.clone());
        }
        filled += count;
    }

    // 2) tables: 행 자동 증식 + 셀 채우기
    let tables = data
        .get("tables")
        .and_then(serde_json::Value::as_array)
        .expect("has_tables로 확인됨");
    for (ti, t) in tables.iter().enumerate() {
        let table_index = t
            .get("table")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let start_row = t
            .get("start_row")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1) as u16;
        let template_row = t
            .get("template_row")
            .and_then(serde_json::Value::as_u64)
            .map(|r| r as u16);
        let rows = t
            .get("rows")
            .and_then(serde_json::Value::as_array)
            .with_context(|| format!("tables[{ti}].rows 배열이 필요합니다"))?;

        let (cur_rows, _cols) = hwp_convert::table_dims(&mut doc, table_index)
            .ok_or_else(|| anyhow::anyhow!("표 #{table_index}를 찾을 수 없습니다"))?;
        // start_row가 현재 행 수를 넘으면 그 사이가 빈 행으로 채워진다 — 보통 실수이므로 경고.
        if start_row as usize > cur_rows as usize {
            warnings.push(format!(
                "tables[{ti}] start_row={start_row} > 현재 행 수 {cur_rows}: 사이 {}행을 빈 행으로 추가",
                start_row as usize - cur_rows as usize
            ));
        }
        let need = start_row as usize + rows.len();
        if need > cur_rows as usize {
            let n = need - cur_rows as usize;
            hwp_convert::add_rows(&mut doc, table_index, template_row, n)
                .map_err(|e| anyhow::anyhow!(e))?;
            added += n;
        }
        for (i, row) in rows.iter().enumerate() {
            let r = start_row + i as u16;
            let cells = row
                .as_array()
                .with_context(|| format!("tables[{ti}].rows[{i}]는 셀 값 배열이어야 합니다"))?;
            for (c, val) in cells.iter().enumerate() {
                hwp_convert::set_cell(&mut doc, table_index, r, c as u16, &value_to_string(val))
                    .map_err(|e| anyhow::anyhow!(e))?;
                filled += 1;
            }
        }
    }

    if !unmatched_fields.is_empty() && !allow_partial {
        anyhow::bail!(
            "요청한 자리표시자를 찾지 못했습니다: {} \
             (--allow-partial로 일치한 값만 적용 가능)",
            unmatched_fields.join(", ")
        );
    }
    if doc == original {
        anyhow::bail!("적용 가능한 표/자리표시자 변경이 없어 출력을 게시하지 않습니다");
    }
    if !unmatched_fields.is_empty() {
        warnings.push(format!(
            "미치환 자리표시자: {}",
            unmatched_fields.join(", ")
        ));
    }

    let writer_warnings = crate::commands::output::write_validated(
        output,
        Some(input),
        |staged| write_table_fill(&doc, staged, added > 0),
        |staged, writer_warnings| {
            crate::commands::reject_drop_warnings("fill", writer_warnings)?;
            ensure_valid_document(staged)?;
            crate::commands::edit::verify_document(staged, &doc)?;
            Ok(())
        },
    )?;
    warnings.extend(writer_warnings);

    Ok(FillReport {
        output: output.display().to_string(),
        mode: "tables",
        replaced: filled,
        counts: fields
            .keys()
            .map(|name| {
                (
                    name.clone(),
                    usize::from(!unmatched_fields.iter().any(|missing| missing == name)),
                )
            })
            .collect(),
        filled,
        rows_added: added,
        warnings,
    })
}

fn write_table_fill(
    doc: &hwp_model::Document,
    output: &Path,
    structural: bool,
) -> anyhow::Result<Vec<String>> {
    match output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("hwp") if structural => crate::commands::convert::write_hwp_structural(doc, output),
        Some("hwp") => crate::commands::convert::write_hwp_edited(doc, output),
        Some("hwpx") => Ok(hwpx::write_document(doc, output)?),
        other => anyhow::bail!("fill 출력은 .hwp 또는 .hwpx만 지원합니다 (확장자: {other:?})"),
    }
}

fn ensure_valid_document(path: &Path) -> anyhow::Result<()> {
    let validation = crate::commands::validate::validate_json(path);
    if validation["valid"].as_bool() == Some(true) {
        return Ok(());
    }
    anyhow::bail!(
        "채운 문서 구조 검증 실패: {}",
        serde_json::to_string(&validation)?
    )
}

pub fn report_json(report: &FillReport) -> serde_json::Value {
    if report.mode == "tables" {
        serde_json::json!({
            "output": report.output,
            "mode": report.mode,
            "filled": report.filled,
            "rows_added": report.rows_added,
            "counts": report.counts,
            "warnings": report.warnings,
        })
    } else {
        serde_json::json!({
            "output": report.output,
            "mode": report.mode,
            "replaced": report.replaced,
            "counts": report.counts,
            "warnings": report.warnings,
        })
    }
}

/// JSON 값을 셀/필드 문자열로 — 문자열은 그대로, null은 빈 칸, 수/불리언은 표기.
fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
