//! `hwp fill` — 템플릿 채우기.
//!
//! 세 경로: (1) **자리표시자 치환**(기본) — `Contents/section*.xml`의 `{{name}}`만
//! 외과 치환하고 나머지 패키지 엔트리(미리보기·compat·BinData)를 바이트 보존(hwpx
//! 입력 전용). (2) **데이터 구동 표 채우기** — `--data`에 `tables` 지시가 있으면 IR로
//! 읽어 표 행을 데이터 수만큼 늘리고(add_rows) 셀을 채운 뒤 다시 쓴다(.hwp/.hwpx 모두).
//! (3) **부분(part) 채우기** — `--set name=@part.md` 또는 `--data`의 `parts` 맵이 있으면
//! `{{name}}`만 담긴 앵커 문단을 부분 파일(md+HTML 혼합, 계약 docs/design/18)의 블록으로
//! 교체한다(IR 경로, .hwp/.hwpx 모두 — Maru 부분별 작성·조합 워크플로).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub preservation: hwp_model::PreservationReport,
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
            let mut value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("--data JSON 파싱 실패 ({}): {e}", d.display()))?;
            // parts의 상대 경로는 --data 파일 위치 기준으로 해석한다.
            if let Some(dir) = d.parent()
                && let Some(serde_json::Value::Object(parts)) = value.get_mut("parts")
            {
                for p in parts.values_mut() {
                    if let Some(s) = p.as_str() {
                        let path = Path::new(s);
                        if path.is_relative() {
                            *p = serde_json::Value::String(dir.join(path).display().to_string());
                        }
                    }
                }
            }
            Some(value)
        }
        None => None,
    };

    let report = execute(input, output, set, data_value.as_ref(), allow_partial, &[])?;
    crate::commands::preservation::print_report(&report.preservation);

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
    roots: &[PathBuf],
) -> anyhow::Result<FillReport> {
    // 데이터에 `tables`가 (객체 항목의) 비어있지 않은 배열이면 IR 기반 표 채우기로 분기.
    // 객체-배열만 인정해, "tables"라는 이름의 평범한 자리표시자(예: 문자열 배열 값)가
    // 표 채우기로 오인 라우팅돼 실패하지 않게 한다(평문 fill 경로로 떨어뜨림).
    let has_tables = data_value
        .as_ref()
        .and_then(|v| v.get("tables"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|arr| !arr.is_empty() && arr.iter().all(serde_json::Value::is_object));

    // 부분(part) 채우기 수집: --set name=@path (`@@`는 리터럴 '@') + --data의 parts 맵.
    let mut part_paths: BTreeMap<String, PathBuf> = BTreeMap::new();
    if let Some(serde_json::Value::Object(map)) = data_value
        && let Some(serde_json::Value::Object(parts)) = map.get("parts")
    {
        for (k, v) in parts {
            let Some(p) = v.as_str() else {
                anyhow::bail!("--data parts.{k}는 파일 경로 문자열이어야 합니다");
            };
            part_paths.insert(k.clone(), PathBuf::from(p));
        }
    }
    let mut plain_set: Vec<String> = Vec::new();
    for pair in set {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--set 형식은 name=value 여야 합니다: {pair}"))?;
        match v.strip_prefix('@') {
            Some(path) if !path.starts_with('@') => {
                part_paths.insert(k.to_string(), PathBuf::from(path));
            }
            Some(literal) => plain_set.push(format!("{k}=@{literal}")), // '@@' → 리터럴
            None => plain_set.push(pair.clone()),
        }
    }
    if !part_paths.is_empty() {
        if has_tables {
            anyhow::bail!("parts(부분 채우기)와 tables(표 채우기)는 한 번에 쓸 수 없습니다 (v1)");
        }
        return fill_parts_ir(
            input,
            output,
            data_value,
            &plain_set,
            &part_paths,
            allow_partial,
            roots,
        );
    }

    if has_tables {
        return fill_tables_ir(
            input,
            output,
            data_value.expect("has_tables로 확인됨"),
            &plain_set,
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

    hwp_model::slot_lookup(values).map_err(|e| anyhow::anyhow!("fill 실패: {e}"))?;
    let mut report_warnings = Vec::new();
    let counts = crate::commands::output::write_validated(
        output,
        Some(input),
        |staged| {
            hwpx::patch::fill_placeholders(input, staged, values)
                .map_err(|e| anyhow::anyhow!("fill 실패: {e}"))
        },
        |staged, counts| {
            // --allow-partial covers a zero total too: the input is published unchanged and
            // the report shows every count at 0 (#362).
            let total: usize = counts.values().sum();
            if total == 0 && !allow_partial {
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
            let unresolved = leftover_slots(&load_document(staged)?, values, counts)?;
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
    let total = replaced_total(values, &counts);
    Ok(FillReport {
        output: output.display().to_string(),
        mode: "placeholders",
        replaced: total,
        counts,
        filled: total,
        rows_added: 0,
        warnings: report_warnings,
        preservation: hwp_model::PreservationReport::new(),
    })
}

/// Requested keys whose slot the filled document still shows (read through the IR, as
/// `hwp slots` does) more often than the inserted values spell it themselves. Values are
/// literal, so `a={{b}}` legitimately leaves one `{{b}}` per `a` it filled; anything beyond that
/// is an original token the fill missed, wherever else a count was credited.
fn leftover_slots(
    doc: &hwp_model::Document,
    values: &BTreeMap<String, String>,
    counts: &BTreeMap<String, usize>,
) -> anyhow::Result<Vec<String>> {
    let lookup = hwp_model::slot_lookup(values).map_err(|e| anyhow::anyhow!("fill 실패: {e}"))?;
    let mut spelled: BTreeMap<&str, usize> = BTreeMap::new();
    for request in lookup.values() {
        let filled = counts.get(request.keys[0]).copied().unwrap_or(0);
        for token in hwp_model::slot_tokens(request.value) {
            *spelled.entry(token.name).or_default() += filled;
        }
    }
    Ok(hwp_convert::scan_placeholders(doc)
        .into_iter()
        .filter_map(|slot| {
            let request = lookup.get(slot.name.as_str())?;
            let allowed = spelled.get(slot.name.as_str()).copied().unwrap_or(0);
            (slot.occurrences > allowed).then(|| request.keys[0].to_string())
        })
        .collect())
}

/// Tokens replaced, each counted once although every key naming its slot reports it.
fn replaced_total(values: &BTreeMap<String, String>, counts: &BTreeMap<String, usize>) -> usize {
    match hwp_model::slot_lookup(values) {
        Ok(lookup) => lookup
            .values()
            .map(|request| counts.get(request.keys[0]).copied().unwrap_or(0))
            .sum(),
        Err(_) => counts.values().sum(),
    }
}

/// `--allow-partial` with nothing to change on an IR path: publish the input unchanged, as the
/// placeholder path does. When the output is the input's own format, the published bytes are a
/// private, size-bound snapshot of the input, checked to still read as `original`, the document
/// the fill examined. The other HWP format (`.hwp`/`.hwpx`) makes the fill a plain conversion,
/// so it goes through `hwp convert` (the fill writer's re-read check does not hold across
/// formats); any other extension is refused, as the writer refuses it.
fn publish_unchanged(
    input: &Path,
    output: &Path,
    original: &hwp_model::Document,
) -> anyhow::Result<hwp_model::WriteReport> {
    let same_format = match crate::format::detect(input)? {
        crate::format::FileFormat::Hwpx => "hwpx",
        crate::format::FileFormat::Hwp5 => "hwp",
    };
    let output_ext = output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    // The same output contract as the writer path: a zero-match fill is no way into `convert`.
    if !matches!(output_ext.as_deref(), Some("hwp" | "hwpx")) {
        anyhow::bail!(
            "fill 출력은 .hwp 또는 .hwpx만 지원합니다 (확장자: {:?})",
            output_ext.as_deref()
        );
    }
    if output_ext.as_deref() != Some(same_format) {
        let report = crate::commands::convert::execute(
            input,
            output,
            None,
            false,
            None,
            false,
            false,
            &crate::commands::convert::MdOpts::default(),
            Vec::new(),
        )?;
        let mut written = hwp_model::WriteReport::new();
        written.warnings = report.warnings;
        written.preservation = report.preservation;
        return Ok(written);
    }
    crate::commands::output::write_with_private_input_snapshot(
        output,
        input,
        hwp_cli::certification::MAX_INPUT_BYTES,
        crate::commands::output::SnapshotOutputMode::Publish,
        |snapshot, staged, _| {
            if load_document(snapshot)? != *original {
                anyhow::bail!("입력 파일이 fill 도중 바뀌어 게시하지 않습니다");
            }
            std::fs::write(staged, std::fs::read(snapshot)?)?;
            Ok(())
        },
        |staged, _| ensure_valid_document(staged),
    )?;
    Ok(hwp_model::WriteReport::new())
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
    // One literal pass (#362): a value that spells a slot is not filled again.
    let field_counts = hwp_convert::replace_slots(&mut doc, &fields)
        .map_err(|e| anyhow::anyhow!("fill 실패: {e}"))?;
    for (k, count) in &field_counts {
        if *count == 0 {
            unmatched_fields.push(k.clone());
        }
    }
    filled += replaced_total(&fields, &field_counts);

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
    if doc == original && !allow_partial {
        anyhow::bail!("적용 가능한 표/자리표시자 변경이 없어 출력을 게시하지 않습니다");
    }
    if !unmatched_fields.is_empty() {
        warnings.push(format!(
            "미치환 자리표시자: {}",
            unmatched_fields.join(", ")
        ));
    }

    let writer_report = if doc == original {
        publish_unchanged(input, output, &original)?
    } else {
        write_ir_fill(input, output, &original, &doc, added > 0)?
    };
    warnings.extend(writer_report.warnings);

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
        preservation: writer_report.preservation,
    })
}

/// 부분(part) 채우기 — `{{name}}`만 담긴 앵커 문단을 부분 파일(md+HTML 혼합,
/// 계약 docs/design/18)의 블록으로 교체한다. 대규모 문서의 부분별 작성·조합 워크플로.
/// 템플릿과 부분 모두 hwp-cli 생성 문서(기본 팔레트 계열)여야 한다(merge::part_paragraphs).
fn fill_parts_ir(
    input: &Path,
    output: &Path,
    data: Option<&serde_json::Value>,
    set: &[String],
    part_paths: &BTreeMap<String, PathBuf>,
    allow_partial: bool,
    roots: &[PathBuf],
) -> anyhow::Result<FillReport> {
    let mut doc = load_document(input)?;
    let original = doc.clone();
    let mut filled = 0usize;
    let mut warnings = Vec::new();
    let mut unmatched = Vec::new();

    // 1) fields: 평문 자리표시자 치환 (fill_tables_ir와 동일 규칙).
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    if let Some(serde_json::Value::Object(top)) = data {
        for (k, v) in top {
            if k == "fields" || k == "parts" || k == "tables" || v.is_object() || v.is_array() {
                continue; // 예약 키·복합값 제외 — 최상위 스칼라만 흡수.
            }
            fields.insert(k.clone(), value_to_string(v));
        }
        if let Some(serde_json::Value::Object(f)) = top.get("fields") {
            for (k, v) in f {
                fields.insert(k.clone(), value_to_string(v));
            }
        }
    }
    for pair in set {
        let (k, v) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--set 형식은 name=value 여야 합니다: {pair}"))?;
        fields.insert(k.to_string(), v.to_string());
    }
    // Two part names for one anchor would splice the same paragraph twice.
    let mut anchor_names = std::collections::BTreeSet::new();
    if let Some(name) = part_paths
        .keys()
        .find(|name| !anchor_names.insert(name.trim()))
    {
        anyhow::bail!("fill 실패: 부분 앵커 이름이 겹칩니다 (앞뒤 공백 무시): {name:?}");
    }
    // 2) Anchor paragraphs, found on the unfilled document so that a field value spelling
    //    `{{name}}` is never taken for one (#362): values are literal.
    let mut anchors: Vec<(usize, usize, &String)> = Vec::new(); // (section, paragraph, part)
    for (section_index, section) in doc.sections.iter().enumerate() {
        for (para_index, para) in section.paragraphs.iter().enumerate() {
            let text = paragraph_text(para);
            let text = text.trim();
            let tokens = hwp_model::slot_tokens(text);
            for name in part_paths.keys() {
                let name_str = name.trim();
                if matches!(tokens.as_slice(), [token]
                    if token.name == name_str && token.range == (0..text.len()))
                {
                    anchors.push((section_index, para_index, name));
                } else if tokens.iter().any(|token| token.name == name_str) {
                    // 앵커 문단은 자리표시자만으로 구성돼야 한다 — 문장 중간의
                    // {{name}}은 블록 교체가 성립하지 않으므로 필드 치환으로 안내.
                    let anchor = format!("{{{{{name}}}}}");
                    if !allow_partial {
                        anyhow::bail!(
                            "부분 앵커 문단은 자리표시자만 담겨 있어야 합니다: {anchor} \
                             (문장 중간의 {anchor}는 fields 경로 사용)"
                        );
                    }
                    warnings.push(format!(
                        "부분 앵커가 문장 중간에 있어 건드리지 않습니다: {anchor}"
                    ));
                }
            }
        }
    }

    // 3) fields, in one literal pass. Paragraph counts do not change, so the anchors stay put.
    let field_counts = hwp_convert::replace_slots(&mut doc, &fields)
        .map_err(|e| anyhow::anyhow!("fill 실패: {e}"))?;
    for (k, count) in &field_counts {
        if *count == 0 {
            unmatched.push(k.clone());
        }
    }
    filled += replaced_total(&fields, &field_counts);

    // 4) parts: 앵커 문단 → 부분 블록 교체. A part with no anchor is not imported.
    let mut counts = BTreeMap::new();
    let mut blocks_by_name: BTreeMap<&String, Vec<hwp_model::Paragraph>> = BTreeMap::new();
    for (name, path) in part_paths {
        let hits = anchors.iter().filter(|(.., part)| *part == name).count();
        if hits == 0 {
            if !allow_partial {
                anyhow::bail!("부분 앵커를 찾지 못했습니다: {{{{{name}}}}}");
            }
            unmatched.push(name.clone());
            counts.insert(name.clone(), 0);
            continue;
        }
        let md = std::fs::read_to_string(path)
            .with_context(|| format!("부분 파일 읽기 실패: {}", path.display()))?;
        // `roots` binds image references inside the part file (MCP `--root`, #56): an
        // outside-root reference is a hard error here, not an alt-text warning. Empty roots
        // (CLI) keep the previous behavior.
        let (part_direct, part_warnings) = hwp_convert::from_markdown_blocks_report(
            &md,
            &hwp_convert::MarkdownImportOptions {
                base_dir: path.parent(),
                roots,
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("부분 markdown 가져오기 실패 ({}): {e}", path.display()))?;
        // Keep the previous stderr visibility (from_markdown_blocks printed import warnings).
        for w in &part_warnings {
            eprintln!("경고: {w}");
        }
        // hwpx 왕복으로 writer 정규형에 맞춘다 — 템플릿은 파일에서 읽은 정규형이라,
        // 비정규 부수 필드(음영·attr1·글꼴 속성 등)가 그대로 이식되면 쓰기→재읽기
        // 의미 불변식 검증(verify_document)이 깨진다.
        let part = roundtrip_hwpx(&part_direct)
            .with_context(|| format!("부분 문서 정규화(hwpx 왕복) 실패: {}", path.display()))?;
        let blocks = hwp_convert::merge::part_paragraphs(&mut doc, &part)
            .map_err(|e| anyhow::anyhow!("부분 이식 실패 ({}): {e}", path.display()))?;
        blocks_by_name.insert(name, blocks);
        counts.insert(name.clone(), hits);
        filled += hits;
    }
    // Back to front, so the earlier anchors keep their paragraph indices.
    for (section_index, para_index, name) in anchors.iter().rev() {
        let blocks = &blocks_by_name[name];
        doc.sections[*section_index]
            .paragraphs
            .splice(*para_index..=*para_index, blocks.iter().cloned());
    }

    if !unmatched.is_empty() && !allow_partial {
        anyhow::bail!(
            "요청한 자리표시자를 찾지 못했습니다: {} \
             (--allow-partial로 일치한 값만 적용 가능)",
            unmatched.join(", ")
        );
    }
    if doc == original && !allow_partial {
        anyhow::bail!("적용 가능한 부분/자리표시자 변경이 없어 출력을 게시하지 않습니다");
    }
    if !unmatched.is_empty() {
        warnings.push(format!("미치환 자리표시자: {}", unmatched.join(", ")));
    }

    let writer_report = if doc == original {
        publish_unchanged(input, output, &original)?
    } else {
        write_ir_fill(input, output, &original, &doc, true)?
    };
    warnings.extend(writer_report.warnings);

    Ok(FillReport {
        output: output.display().to_string(),
        mode: "parts",
        replaced: filled,
        counts,
        filled,
        rows_added: 0,
        warnings,
        preservation: writer_report.preservation,
    })
}

/// 문단의 표시 텍스트(문자만) — 앵커 판별용 단순 추출.
fn paragraph_text(para: &hwp_model::Paragraph) -> String {
    para.chars
        .iter()
        .filter_map(|c| match c {
            hwp_model::HwpChar::Text(c) => Some(*c),
            _ => None,
        })
        .collect()
}

/// 부분 문서를 hwpx로 썼다가 다시 읽어 writer 정규형으로 만든다 (임시 파일 사용).
/// 왕복이 첫 문단에 secd/cold를 재합성하므로 벗겨낸다 — 부분은 구역 정의를 가지면 안 된다.
fn roundtrip_hwpx(doc: &hwp_model::Document) -> anyhow::Result<hwp_model::Document> {
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp =
        std::env::temp_dir().join(format!("hwp_fill_part_{}_{uniq}.hwpx", std::process::id()));
    let result = (|| {
        hwpx::write_document(doc, &tmp).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut doc = load_document(&tmp)?;
        for section in &mut doc.sections {
            for para in &mut section.paragraphs {
                strip_section_controls(para);
            }
        }
        Ok(doc)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// hwpx 왕복이 합성한 구역/단 정의(secd/cold)를 문단에서 벗긴다.
/// 앵커 문자 제거 → WCHAR 위치 보정 → controls 재연결 순으로 처리한다.
fn strip_section_controls(para: &mut hwp_model::Paragraph) {
    use hwp_model::{Control, HwpChar};
    let has = para.controls.iter().any(|c| match c {
        Control::SectionDef(_) => true,
        Control::Generic(g) => g.ctrl_id == *b"cold",
        _ => false,
    });
    if !has {
        return;
    }
    // 1) 앵커 문자 제거 — (원래 WCHAR 위치, 폭) 기록.
    let mut removed: Vec<(u32, u32)> = Vec::new();
    let mut orig_pos = 0u32;
    let mut kept = Vec::with_capacity(para.chars.len());
    for ch in std::mem::take(&mut para.chars) {
        let strip = matches!(&ch, HwpChar::ExtCtrl { ctrl_id, .. } if ctrl_id == b"secd" || ctrl_id == b"cold");
        let width = ch.wchar_width();
        if strip {
            removed.push((orig_pos, width));
        } else {
            kept.push(ch);
        }
        orig_pos += width;
    }
    para.chars = kept;
    // 2) 컨트롤 제거.
    para.controls.retain(|c| match c {
        Control::SectionDef(_) => false,
        Control::Generic(g) => g.ctrl_id != *b"cold",
        _ => true,
    });
    // 3) char_shape_runs 위치 보정 (제거된 문자 폭만큼 앞으로).
    for (pos, _) in &mut para.char_shape_runs {
        let shift: u32 = removed
            .iter()
            .filter(|(start, width)| start + width <= *pos)
            .map(|(_, width)| width)
            .sum();
        *pos -= shift;
    }
    para.char_shape_runs.dedup();
    // 4) ExtCtrl ↔ controls 등장순서 재연결.
    hwp_convert::field::relink_ctrl_index(para);
}

fn write_table_fill(
    source: Option<(&Path, &hwp_model::Document)>,
    doc: &hwp_model::Document,
    output: &Path,
    structural: bool,
) -> anyhow::Result<hwp_model::WriteReport> {
    match output
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("hwp")
            if source.is_some_and(|(_, original)| original.meta.source_format == "hwp5") =>
        {
            let (source, original) = source.expect("guarded source");
            crate::commands::convert::write_hwp_preserving_source(
                source,
                original,
                doc,
                output,
                !structural,
                structural,
            )
        }
        Some("hwp") if structural => crate::commands::convert::write_hwp_structural(doc, output),
        Some("hwp") => crate::commands::convert::write_hwp_edited(doc, output),
        Some("hwpx") => Ok(hwpx::write_document_with_report(doc, output)?),
        other => anyhow::bail!("fill 출력은 .hwp 또는 .hwpx만 지원합니다 (확장자: {other:?})"),
    }
}

fn write_ir_fill(
    input: &Path,
    output: &Path,
    original: &hwp_model::Document,
    edited: &hwp_model::Document,
    structural: bool,
) -> anyhow::Result<hwp_model::WriteReport> {
    let write_staged = |source: &Path, staged: &Path| {
        let mut report = write_table_fill(Some((source, original)), edited, staged, structural)?;
        report
            .preservation
            .extend(crate::commands::preservation::inspect_same_format_container(source, staged)?);
        Ok(report)
    };
    let verify_staged = |staged: &Path, writer_report: &hwp_model::WriteReport| {
        crate::commands::reject_preservation_loss("fill", &writer_report.preservation)?;
        ensure_valid_document(staged)?;
        crate::commands::edit::verify_document(staged, edited)?;
        Ok(())
    };
    let hwp_source_and_output = original.meta.source_format == "hwp5"
        && output
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("hwp"));
    if hwp_source_and_output {
        let (_, report) = crate::commands::output::write_with_private_input_snapshot(
            output,
            input,
            hwp_cli::certification::MAX_INPUT_BYTES,
            crate::commands::output::SnapshotOutputMode::Publish,
            |snapshot, staged, _| write_staged(snapshot, staged),
            verify_staged,
        )?;
        Ok(report)
    } else {
        crate::commands::output::write_validated(
            output,
            Some(input),
            |staged| write_staged(input, staged),
            verify_staged,
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// A slot the filled document still shows is a leftover only beyond what inserted values
    /// spell: here `a={{b}}` was inserted once, so one `{{b}}` is allowed and a second is not,
    /// whatever count the raw pass credited to `b` (#363 review).
    #[test]
    fn leftover_slots_allows_only_what_values_spell() {
        let values = BTreeMap::from([
            ("a".to_string(), "{{b}}".to_string()),
            ("b".to_string(), "B".to_string()),
        ]);
        let doc = hwp_convert::from_markdown("{{b}} 그리고 {{b}}\n");
        let leftover = leftover_slots(&doc, &values, &map(&[("a", 1), ("b", 1)])).unwrap();
        assert_eq!(leftover, ["b"]);
        let leftover = leftover_slots(&doc, &values, &map(&[("a", 2), ("b", 1)])).unwrap();
        assert!(leftover.is_empty(), "two insertions of a spell two b");
    }
}
