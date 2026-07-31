//! `hwp mcp` — MCP(Model Context Protocol) stdio 서버.
//!
//! tokio/SDK 없이 serde_json만으로 동기 JSON-RPC 2.0(줄 단위 over stdio)을 구현한다.
//! 에이전트(Claude 등)가 도구 호출로 HWP를 **읽고·렌더해서 보고·편집·변환**하게 한다.
//! stdout은 프로토콜 전용(라이브러리 함수는 stdout 미오염, 로그는 stderr).
//!
//! 도구는 라이브러리 계층을 직접 감싼다(commands/*::run 아님 — 그건 stdout 출력).

use std::io::{BufRead, Read as _, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::cat::load_document;

const PROTOCOL_VERSION: &str = "2024-11-05";
const MAX_REQUEST_LINE_BYTES: usize = 1024 * 1024;
const DEFAULT_READ_BYTES: usize = 256 * 1024;
const MAX_READ_BYTES: usize = 1024 * 1024;

/// 서버 컨텍스트 (렌더/diff 기본 폰트 디렉터리).
pub struct Ctx {
    pub font_dirs: Vec<PathBuf>,
}

/// stdio JSON-RPC 루프. EOF까지 한 줄씩 처리한다.
pub fn run(font_dirs: Vec<PathBuf>) -> anyhow::Result<()> {
    let ctx = Ctx { font_dirs };
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if read_line_bounded(&mut reader, &mut line, MAX_REQUEST_LINE_BYTES)? == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(resp) = handle_request(trimmed, &ctx) {
            out.write_all(resp.as_bytes())?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}

fn read_line_bounded(
    reader: &mut impl BufRead,
    line: &mut String,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut limited = reader.take((max_bytes as u64).saturating_add(1));
    let read = limited.read_line(line)?;
    if line.len() > max_bytes || (read == max_bytes.saturating_add(1) && !line.ends_with('\n')) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("MCP 요청 한 줄이 {max_bytes} bytes 제한을 넘었습니다"),
        ));
    }
    Ok(read)
}

/// 한 줄 JSON-RPC 요청 → 응답 JSON 문자열. 알림(id 없음)이면 None.
pub fn handle_request(line: &str, ctx: &Ctx) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            ));
        }
    };
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let is_notification = id.is_none();

    match method {
        "initialize" => Some(result_response(
            id_or_null(id),
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "hwp-cli", "version": env!("CARGO_PKG_VERSION")},
            }),
        )),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(result_response(id_or_null(id), json!({}))),
        "tools/list" => Some(result_response(
            id_or_null(id),
            json!({ "tools": tool_defs() }),
        )),
        "tools/call" => {
            if is_notification {
                return None;
            }
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Some(result_response(id_or_null(id), call_tool(name, &args, ctx)))
        }
        _ => {
            if is_notification {
                return None;
            }
            Some(error_response(
                id_or_null(id),
                -32601,
                &format!("method not found: {method}"),
            ))
        }
    }
}

fn id_or_null(id: Option<Value>) -> Value {
    id.unwrap_or(Value::Null)
}

fn result_response(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// 도구를 실행해 `tools/call` result를 만든다. 실행 오류는 isError=true content로.
fn call_tool(name: &str, args: &Value, ctx: &Ctx) -> Value {
    let result: Result<Vec<Value>, String> = match name {
        "hwp_info" => tool_info(args),
        "hwp_read" => tool_read(args),
        "hwp_list_fields" => tool_list_fields(args),
        "hwp_list_bookmarks" => tool_list_bookmarks(args),
        "hwp_render" => tool_render(args, ctx),
        "hwp_edit" => tool_edit(args),
        "hwp_convert" => tool_convert(args),
        "hwp_new" => tool_new(args),
        "hwp_compose" => tool_compose(args),
        "hwp_template" => tool_template(args),
        "hwp_diff" => tool_diff(args, ctx),
        "hwp_slots" => tool_slots(args),
        "hwp_fill" => tool_fill(args),
        "hwp_validate" => tool_validate(args),
        "hwp_certify" => tool_certify(args),
        other => Err(format!("알 수 없는 도구: {other}")),
    };
    match result {
        Ok(content) => json!({"content": content, "isError": false}),
        Err(e) => json!({"content": [text_content(&format!("오류: {e}"))], "isError": true}),
    }
}

// ---- content/인자 헬퍼 ----

fn text_content(s: &str) -> Value {
    json!({"type": "text", "text": s})
}

fn image_content(png: &[u8]) -> Value {
    json!({"type": "image", "data": hwp_convert::base64::encode(png), "mimeType": "image/png"})
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("필수 인자 누락: {key}"))
}

fn arg_str_opt<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{key}는 문자열이어야 합니다"))
        })
        .transpose()
}

fn arg_u64(args: &Value, key: &str, default: u64) -> Result<u64, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{key}는 0 이상의 정수여야 합니다")),
    }
}

fn arg_f64(args: &Value, key: &str, default: f64) -> Result<f64, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_f64()
            .ok_or_else(|| format!("{key}는 숫자여야 합니다")),
    }
}

fn arg_bool(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{key}는 boolean이어야 합니다")),
    }
}

fn arg_array<'a>(args: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match args.get(key) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("{key}는 배열이어야 합니다")),
    }
}

fn required_item_str<'a>(item: &'a Value, operation: &str, key: &str) -> Result<&'a str, String> {
    item.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{operation} 항목에 {key} 필요"))
}

fn required_item_u64(item: &Value, operation: &str, key: &str) -> Result<u64, String> {
    item.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{operation} 항목에 {key} 필요"))
}

fn required_item_usize(item: &Value, operation: &str, key: &str) -> Result<usize, String> {
    usize::try_from(required_item_u64(item, operation, key)?)
        .map_err(|_| format!("{operation} 항목의 {key}가 플랫폼 범위를 넘습니다"))
}

fn required_item_u16(item: &Value, operation: &str, key: &str) -> Result<u16, String> {
    u16::try_from(required_item_u64(item, operation, key)?).map_err(|_| {
        format!(
            "{operation} 항목의 {key}는 0..={} 범위여야 합니다",
            u16::MAX
        )
    })
}

fn optional_item_u16(item: &Value, operation: &str, key: &str) -> Result<Option<u16>, String> {
    item.get(key)
        .map(|_| required_item_u16(item, operation, key))
        .transpose()
}

fn optional_item_str<'a>(
    item: &'a Value,
    operation: &str,
    key: &str,
) -> Result<Option<&'a str>, String> {
    item.get(key)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{operation}.{key}는 문자열이어야 합니다"))
        })
        .transpose()
}

fn optional_item_bool(item: &Value, operation: &str, key: &str) -> Result<Option<bool>, String> {
    item.get(key)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("{operation}.{key}는 boolean이어야 합니다"))
        })
        .transpose()
}

fn optional_item_f32(item: &Value, operation: &str, key: &str) -> Result<Option<f32>, String> {
    item.get(key)
        .map(|value| {
            let number = value
                .as_f64()
                .ok_or_else(|| format!("{operation}.{key}는 숫자여야 합니다"))?;
            let number = number as f32;
            if !number.is_finite() {
                return Err(format!("{operation}.{key}는 유한한 f32 범위여야 합니다"));
            }
            Ok(number)
        })
        .transpose()
}

fn font_dirs_for(args: &Value, ctx: &Ctx) -> Result<Vec<PathBuf>, String> {
    let mut dirs = ctx.font_dirs.clone();
    if let Some(d) = arg_str_opt(args, "font_dir")? {
        dirs.push(PathBuf::from(d));
    }
    Ok(dirs)
}

// ---- 도구 핸들러 ----

fn tool_info(args: &Value) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let v = crate::commands::info::info_json(Path::new(path)).map_err(|e| e.to_string())?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

fn tool_read(args: &Value) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let format = arg_str_opt(args, "format")?.unwrap_or("plain");
    let doc = load_document(Path::new(path)).map_err(|e| e.to_string())?;
    let text = match format {
        "plain" => doc.plain_text(),
        "markdown" | "md" => hwp_convert::to_markdown(&doc),
        "json" => hwp_convert::to_json(&doc, true, false).map_err(|e| e.to_string())?,
        other => return Err(format!("알 수 없는 format: {other} (plain|markdown|json)")),
    };
    let offset = usize::try_from(arg_u64(args, "offset", 0)?)
        .map_err(|_| "offset이 플랫폼 범위를 넘습니다".to_string())?;
    let max_bytes = usize::try_from(arg_u64(args, "max_bytes", DEFAULT_READ_BYTES as u64)?)
        .map_err(|_| "max_bytes가 플랫폼 범위를 넘습니다".to_string())?;
    if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
        return Err(format!("max_bytes는 1..={MAX_READ_BYTES} 범위여야 합니다"));
    }
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(format!(
            "offset은 UTF-8 경계인 0..={} byte 범위여야 합니다",
            text.len()
        ));
    }
    let mut end = offset.saturating_add(max_bytes).min(text.len());
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = end < text.len();
    let metadata = json!({
        "offset": offset,
        "returned_bytes": end - offset,
        "total_bytes": text.len(),
        "truncated": truncated,
        "next_offset": truncated.then_some(end),
    });
    Ok(vec![
        text_content(&text[offset..end]),
        text_content(&serde_json::to_string(&metadata).unwrap_or_default()),
    ])
}

fn tool_list_fields(args: &Value) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let doc = load_document(Path::new(path)).map_err(|e| e.to_string())?;
    let fields: Vec<Value> = hwp_convert::list_fields(&doc)
        .iter()
        .map(|f| {
            json!({
                "kind": f.kind, "ctrl_id": f.ctrl_id,
                "name": f.name, "command": f.command, "value": f.value,
            })
        })
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&fields).unwrap_or_default(),
    )])
}

fn tool_list_bookmarks(args: &Value) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let doc = load_document(Path::new(path)).map_err(|e| e.to_string())?;
    let bookmarks: Vec<Value> = hwp_convert::list_bookmarks(&doc)
        .iter()
        .map(|b| json!({ "name": b.name }))
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&bookmarks).unwrap_or_default(),
    )])
}

fn tool_slots(args: &Value) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let doc = load_document(Path::new(path)).map_err(|e| e.to_string())?;
    let items: Vec<Value> = hwp_convert::scan_placeholders(&doc)
        .iter()
        .map(|p| json!({ "name": p.name, "occurrences": p.occurrences }))
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({ "placeholders": items })).unwrap_or_default(),
    )])
}

fn tool_fill(args: &Value) -> Result<Vec<Value>, String> {
    let input = arg_str(args, "input")?;
    let output = arg_str(args, "output")?;
    let values_obj = args
        .get("values")
        .and_then(Value::as_object)
        .ok_or("필수 인자 누락: values (객체 {이름:값})")?;
    let values: std::collections::BTreeMap<String, String> = values_obj
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    let report = crate::commands::fill::execute_values(
        Path::new(input),
        Path::new(output),
        &values,
        arg_bool(args, "allow_partial", false)?,
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&crate::commands::fill::report_json(&report))
            .unwrap_or_default(),
    )])
}

fn tool_validate(args: &Value) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let v = crate::commands::validate::validate_json(Path::new(path));
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

fn tool_certify(args: &Value) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "input" | "policy" | "report"))
    {
        return Err(format!("알 수 없는 hwp_certify 인자: {unknown}"));
    }
    let input = Path::new(arg_str(args, "input")?);
    let policy = Path::new(arg_str(args, "policy")?);
    let report = Path::new(arg_str(args, "report")?);
    let outcome = hwp_cli::certification::execute(input, policy, report)
        .map_err(|error| format!("{error:#}"))?;
    let summary = serde_json::to_string_pretty(&json!({
        "overall": outcome.overall,
        "report": outcome.report_dir,
    }))
    .map_err(|error| error.to_string())?;
    if outcome.overall != hwp_cli::certification::OverallStatus::Passed {
        return Err(summary);
    }
    Ok(vec![text_content(&summary)])
}

fn tool_render(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let path = arg_str(args, "path")?;
    let page = usize::try_from(arg_u64(args, "page", 1)?)
        .map_err(|_| "page가 플랫폼 범위를 넘습니다".to_string())?;
    let dpi = crate::commands::render::validated_dpi(arg_f64(args, "dpi", 120.0)?)
        .map_err(|error| error.to_string())?;
    let doc = load_document(Path::new(path)).map_err(|e| e.to_string())?;
    let out = hwp_render::render_document_pages(
        &doc,
        &hwp_render::RenderOptions {
            dpi,
            font_dirs: font_dirs_for(args, ctx)?,
        },
        Some(&[page]),
    )
    .map_err(|e| e.to_string())?;
    let pixmap = &out.pages[0];
    let png = pixmap
        .encode_png()
        .ok()
        .ok_or_else(|| "PNG 인코딩 실패".to_string())?;
    const MAX_MCP_PNG_BYTES: usize = 16 * 1024 * 1024;
    if png.len() > MAX_MCP_PNG_BYTES {
        return Err(format!(
            "MCP 렌더 PNG가 응답 상한 {MAX_MCP_PNG_BYTES} bytes를 초과합니다: {} bytes",
            png.len()
        ));
    }
    let summary = format!(
        "페이지 {page}/{} 렌더 ({}×{}px, {dpi}dpi). issues={}, info={}, complete={}, sha256={}",
        out.total_pages,
        pixmap.width(),
        pixmap.height(),
        out.report.issue_count,
        out.report.info_count,
        out.report.complete,
        out.report.sha256,
    );
    Ok(vec![text_content(&summary), image_content(&png)])
}

fn tool_edit(args: &Value) -> Result<Vec<Value>, String> {
    let input = arg_str(args, "input")?;
    let output = arg_str(args, "output")?;
    use crate::commands::edit::TypedEditOperation as Op;

    let mut operations = Vec::new();

    for item in arg_array(args, "replace")? {
        operations.push(Op::Replace {
            from: required_item_str(item, "replace", "from")?.to_string(),
            to: required_item_str(item, "replace", "to")?.to_string(),
        });
    }
    for item in arg_array(args, "set_cell")? {
        operations.push(Op::SetCell {
            table: required_item_usize(item, "set_cell", "table")?,
            row: required_item_u16(item, "set_cell", "row")?,
            col: required_item_u16(item, "set_cell", "col")?,
            text: required_item_str(item, "set_cell", "text")?.to_string(),
        });
    }
    for item in arg_array(args, "set_field")? {
        operations.push(Op::SetField {
            name: required_item_str(item, "set_field", "name")?.to_string(),
            value: required_item_str(item, "set_field", "value")?.to_string(),
        });
    }
    for item in arg_array(args, "set_meta")? {
        operations.push(Op::SetMeta {
            key: required_item_str(item, "set_meta", "key")?.to_string(),
            value: required_item_str(item, "set_meta", "value")?.to_string(),
        });
    }
    for item in arg_array(args, "create_field")? {
        operations.push(Op::CreateField {
            anchor: required_item_str(item, "create_field", "anchor")?.to_string(),
            name: required_item_str(item, "create_field", "name")?.to_string(),
            value: optional_item_str(item, "create_field", "value")?
                .unwrap_or("")
                .to_string(),
        });
    }
    for item in arg_array(args, "create_bookmark")? {
        operations.push(Op::CreateBookmark {
            anchor: required_item_str(item, "create_bookmark", "anchor")?.to_string(),
            name: required_item_str(item, "create_bookmark", "name")?.to_string(),
        });
    }
    for item in arg_array(args, "create_hyperlink")? {
        let url = required_item_str(item, "create_hyperlink", "url")?.to_string();
        operations.push(Op::CreateHyperlink {
            anchor: required_item_str(item, "create_hyperlink", "anchor")?.to_string(),
            display: optional_item_str(item, "create_hyperlink", "display")?
                .unwrap_or(&url)
                .to_string(),
            url,
        });
    }
    for item in arg_array(args, "insert_image")? {
        let size_mm = match (
            optional_item_f32(item, "insert_image", "width_mm")?,
            optional_item_f32(item, "insert_image", "height_mm")?,
        ) {
            (Some(width), Some(height)) => Some((width, height)),
            (None, None) => None,
            _ => {
                return Err(
                    "insert_image는 유한한 width_mm와 height_mm를 함께 지정해야 합니다".into(),
                );
            }
        };
        operations.push(Op::InsertImage {
            anchor: required_item_str(item, "insert_image", "anchor")?.to_string(),
            path: PathBuf::from(required_item_str(item, "insert_image", "path")?),
            size_mm,
        });
    }
    for item in arg_array(args, "seal")? {
        let size_mm = optional_item_f32(item, "seal", "size_mm")?;
        operations.push(Op::Seal {
            anchor: required_item_str(item, "seal", "anchor")?.to_string(),
            path: PathBuf::from(required_item_str(item, "seal", "path")?),
            size_mm,
        });
    }
    for item in arg_array(args, "set_format")? {
        let mut format = hwp_convert::CharFormat {
            bold: optional_item_bool(item, "set_format", "bold")?,
            italic: optional_item_bool(item, "set_format", "italic")?,
            underline: optional_item_bool(item, "set_format", "underline")?,
            strike: optional_item_bool(item, "set_format", "strike")?,
            ..Default::default()
        };
        if let Some(value) = optional_item_f32(item, "set_format", "size")? {
            format.size_pt = Some(value);
        }
        if let Some(value) = optional_item_str(item, "set_format", "color")? {
            format.color = Some(
                crate::commands::edit::parse_color(value)
                    .ok_or_else(|| format!("set_format.color를 해석할 수 없습니다: {value:?}"))?,
            );
        }
        operations.push(Op::SetFormat {
            pattern: required_item_str(item, "set_format", "pattern")?.to_string(),
            format,
        });
    }
    for item in arg_array(args, "set_align")? {
        operations.push(Op::SetAlign {
            pattern: required_item_str(item, "set_align", "pattern")?.to_string(),
            align: crate::commands::edit::parse_align(required_item_str(
                item,
                "set_align",
                "align",
            )?)
            .map_err(|error| error.to_string())?,
        });
    }
    for item in arg_array(args, "insert_para")? {
        operations.push(Op::InsertPara {
            anchor: required_item_str(item, "insert_para", "anchor")?.to_string(),
            text: required_item_str(item, "insert_para", "text")?.to_string(),
            before: optional_item_bool(item, "insert_para", "before")?.unwrap_or(false),
        });
    }
    for item in arg_array(args, "delete_para")? {
        operations.push(Op::DeletePara {
            matching: required_item_str(item, "delete_para", "matching")?.to_string(),
        });
    }
    for item in arg_array(args, "add_row")? {
        operations.push(Op::AddRow {
            table: required_item_usize(item, "add_row", "table")?,
        });
    }
    for item in arg_array(args, "add_col")? {
        operations.push(Op::AddCol {
            table: required_item_usize(item, "add_col", "table")?,
            at: optional_item_u16(item, "add_col", "at")?,
        });
    }
    for item in arg_array(args, "delete_row")? {
        operations.push(Op::DeleteRow {
            table: required_item_usize(item, "delete_row", "table")?,
            row: required_item_u16(item, "delete_row", "row")?,
        });
    }
    for item in arg_array(args, "delete_col")? {
        operations.push(Op::DeleteCol {
            table: required_item_usize(item, "delete_col", "table")?,
            col: required_item_u16(item, "delete_col", "col")?,
        });
    }
    for item in arg_array(args, "merge_cells")? {
        operations.push(Op::MergeCells {
            table: required_item_usize(item, "merge_cells", "table")?,
            r1: required_item_u16(item, "merge_cells", "r1")?,
            c1: required_item_u16(item, "merge_cells", "c1")?,
            r2: required_item_u16(item, "merge_cells", "r2")?,
            c2: required_item_u16(item, "merge_cells", "c2")?,
        });
    }
    for item in arg_array(args, "split_cell")? {
        operations.push(Op::SplitCell {
            table: required_item_usize(item, "split_cell", "table")?,
            row: required_item_u16(item, "split_cell", "row")?,
            col: required_item_u16(item, "split_cell", "col")?,
        });
    }

    let plan = crate::commands::edit::EditPlan::from_typed(
        operations,
        true,
        arg_bool(args, "allow_partial", false)?,
    );
    let report = crate::commands::edit::execute(Path::new(input), Path::new(output), &plan)
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": input,
            "output": report.output,
            "applied": report.applied,
            "warnings": report.warnings,
        }))
        .unwrap_or_default(),
    )])
}

fn tool_convert(args: &Value) -> Result<Vec<Value>, String> {
    let input = arg_str(args, "input")?;
    let output = arg_str(args, "output")?;
    let embed_bin = arg_bool(args, "embed_bin", false)?;
    let strict = arg_bool(args, "strict", true)?;
    let report = crate::commands::convert::execute(
        Path::new(input),
        Path::new(output),
        None,
        strict,
        false,
        embed_bin,
        &crate::commands::convert::MdOpts::default(),
        Vec::new(),
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": input,
            "output": output,
            "strict": strict,
            "warnings": report.warnings,
        }))
        .unwrap_or_default(),
    )])
}

fn tool_new(args: &Value) -> Result<Vec<Value>, String> {
    let output = arg_str(args, "output")?;
    let input = match (arg_str_opt(args, "markdown")?, arg_str_opt(args, "json")?) {
        (Some(_), Some(_)) => return Err("markdown과 json은 동시에 지정할 수 없습니다".into()),
        (Some(markdown), None) => crate::commands::new::NewInput::Markdown {
            text: markdown,
            base_dir: None,
        },
        (None, Some(document_json)) => crate::commands::new::NewInput::Json(document_json),
        (None, None) => crate::commands::new::NewInput::Empty,
    };
    let metadata = arg_array(args, "set_meta")?
        .iter()
        .map(|item| {
            Ok(format!(
                "{}={}",
                required_item_str(item, "set_meta", "key")?,
                required_item_str(item, "set_meta", "value")?
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let report = crate::commands::new::execute(Path::new(output), input, &metadata, None)
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "output": report.output,
            "warnings": report.warnings,
        }))
        .unwrap_or_default(),
    )])
}

fn tool_compose(args: &Value) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    const ALLOWED: &[&str] = &[
        "spec",
        "spec_path",
        "format",
        "base_dir",
        "output",
        "dry_run",
        "allow_visual_fallback",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("알 수 없는 hwp_compose 인자: {unknown}"));
    }

    let output = arg_str(args, "output")?;
    let explicit_format = arg_str_opt(args, "format")?
        .map(parse_spec_format)
        .transpose()?;
    let (input, format, base_dir, source_path) =
        match (args.get("spec"), arg_str_opt(args, "spec_path")?) {
            (Some(_), Some(_)) => return Err("spec과 spec_path는 동시에 지정할 수 없습니다".into()),
            (None, None) => return Err("spec 또는 spec_path 중 하나가 필요합니다".into()),
            (Some(spec), None) => {
                let input = match spec {
                    Value::String(text) => text.clone(),
                    Value::Object(_) => serde_json::to_string(spec).map_err(|e| e.to_string())?,
                    _ => {
                        return Err(
                            "spec은 DocumentSpec 객체 또는 JSON/YAML 문자열이어야 합니다".into(),
                        );
                    }
                };
                let format =
                    explicit_format.unwrap_or(hwp_cli::document_spec::SpecInputFormat::Json);
                let base_dir = arg_str_opt(args, "base_dir")?
                    .map_or_else(|| PathBuf::from("."), PathBuf::from);
                (input, format, base_dir, None)
            }
            (None, Some(spec_path)) => {
                if args.get("base_dir").is_some() {
                    return Err("spec_path 사용 시 base_dir는 지정할 수 없습니다".into());
                }
                let path = Path::new(spec_path);
                let input = crate::commands::compose::read_bounded(path)
                    .map_err(|error| format!("{error:#}"))?;
                let format = explicit_format
                    .map(Ok)
                    .unwrap_or_else(|| hwp_cli::document_spec::infer_input_format(path))
                    .map_err(|error| error.to_string())?;
                let base_dir = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                (input, format, base_dir, Some(path.to_path_buf()))
            }
        };
    let report = crate::commands::compose::execute_text_with_source(
        &input,
        format,
        &base_dir,
        Path::new(output),
        arg_bool(args, "dry_run", false)?,
        arg_bool(args, "allow_visual_fallback", false)?,
        source_path.as_deref(),
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&report).unwrap_or_default(),
    )])
}

fn tool_template(args: &Value) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    const ALLOWED: &[&str] = &[
        "template",
        "template_path",
        "template_format",
        "data",
        "data_path",
        "data_format",
        "base_dir",
        "output",
        "dry_run",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("알 수 없는 hwp_template 인자: {unknown}"));
    }
    let output = arg_str(args, "output")?;
    let explicit_template_format = arg_str_opt(args, "template_format")?
        .map(parse_spec_format)
        .transpose()?;
    let explicit_data_format = arg_str_opt(args, "data_format")?
        .map(parse_spec_format)
        .transpose()?;

    let mut source_paths = Vec::new();
    let (template_input, template_format, base_dir) =
        match (args.get("template"), arg_str_opt(args, "template_path")?) {
            (Some(_), Some(_)) => {
                return Err("template과 template_path는 동시에 지정할 수 없습니다".into());
            }
            (None, None) => return Err("template 또는 template_path 중 하나가 필요합니다".into()),
            (Some(template), None) => {
                let input = inline_contract_input(template, "template")?;
                let format = explicit_template_format
                    .unwrap_or(hwp_cli::document_spec::SpecInputFormat::Json);
                let base = arg_str_opt(args, "base_dir")?
                    .map_or_else(|| PathBuf::from("."), PathBuf::from);
                (input, format, base)
            }
            (None, Some(path)) => {
                if args.get("base_dir").is_some() {
                    return Err("template_path 사용 시 base_dir는 지정할 수 없습니다".into());
                }
                let path = PathBuf::from(path);
                let input = crate::commands::template::read_bounded(
                    &path,
                    hwp_cli::template_spec::MAX_TEMPLATE_BYTES,
                    "TemplateSpec",
                )
                .map_err(|error| format!("{error:#}"))?;
                let format = explicit_template_format
                    .map(Ok)
                    .unwrap_or_else(|| hwp_cli::template_spec::infer_input_format(&path))
                    .map_err(|error| error.to_string())?;
                let base = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                source_paths.push(path);
                (input, format, base)
            }
        };
    let (data_input, data_format) = match (args.get("data"), arg_str_opt(args, "data_path")?) {
        (Some(_), Some(_)) => return Err("data와 data_path는 동시에 지정할 수 없습니다".into()),
        (None, None) => return Err("data 또는 data_path 중 하나가 필요합니다".into()),
        (Some(data), None) => (
            inline_contract_input(data, "data")?,
            explicit_data_format.unwrap_or(hwp_cli::document_spec::SpecInputFormat::Json),
        ),
        (None, Some(path)) => {
            let path = PathBuf::from(path);
            let input = crate::commands::template::read_bounded(
                &path,
                hwp_cli::template_spec::MAX_DATA_BYTES,
                "TemplateData",
            )
            .map_err(|error| format!("{error:#}"))?;
            let format = explicit_data_format
                .map(Ok)
                .unwrap_or_else(|| hwp_cli::template_spec::infer_input_format(&path))
                .map_err(|error| error.to_string())?;
            source_paths.push(path);
            (input, format)
        }
    };

    let report = crate::commands::template::execute_text(
        &template_input,
        template_format,
        &data_input,
        data_format,
        &base_dir,
        Path::new(output),
        arg_bool(args, "dry_run", false)?,
        &source_paths,
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &crate::commands::template::serialize_report(&report)
            .map_err(|error| format!("{error:#}"))?,
    )])
}

fn inline_contract_input(value: &Value, label: &str) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Object(_) => serde_json::to_string(value).map_err(|error| error.to_string()),
        _ => Err(format!("{label}은 객체 또는 JSON/YAML 문자열이어야 합니다")),
    }
}

fn parse_spec_format(value: &str) -> Result<hwp_cli::document_spec::SpecInputFormat, String> {
    match value {
        "json" => Ok(hwp_cli::document_spec::SpecInputFormat::Json),
        "yaml" => Ok(hwp_cli::document_spec::SpecInputFormat::Yaml),
        other => Err(format!("알 수 없는 format: {other} (json|yaml)")),
    }
}

fn tool_diff(args: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let input = arg_str(args, "input")?;
    let reference = arg_str(args, "ref")?;
    let page = usize::try_from(arg_u64(args, "page", 1)?)
        .map_err(|_| "page가 플랫폼 범위를 넘습니다".to_string())?;
    let dpi = crate::commands::render::validated_dpi(arg_f64(args, "dpi", 120.0)?)
        .map_err(|error| error.to_string())?;
    let doc = load_document(Path::new(input)).map_err(|e| e.to_string())?;
    let out = hwp_render::render_document_pages(
        &doc,
        &hwp_render::RenderOptions {
            dpi,
            font_dirs: font_dirs_for(args, ctx)?,
        },
        Some(&[page]),
    )
    .map_err(|e| e.to_string())?;
    let refpx = hwp_render::load_png(Path::new(reference)).map_err(|e| e.to_string())?;
    let (rep, _) = hwp_render::compare(&out.pages[0], &refpx, 16)?;
    let v = json!({
        "ink_ratio": rep.ink_ratio,
        "dx": rep.dx,
        "dy": rep.dy,
        "bad_pixel_pct": rep.bad_pixel_pct,
        "mae": rep.mae,
    });
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

// ---- 도구 정의 (tools/list) ----

fn tool_defs() -> Vec<Value> {
    vec![
        json!({
            "name": "hwp_info",
            "description": "HWP/HWPX 파일의 포맷·버전·속성·스트림 목록을 JSON으로 진단(본문 파싱 불필요).",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string", "description": "hwp/hwpx 파일 경로"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_read",
            "description": "본문을 추출한다. format=json이면 전체 IR(구조)을, markdown/plain이면 텍스트를 반환.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "format": {"type": "string", "enum": ["plain", "markdown", "json"], "description": "기본 plain"},
                "offset": {"type": "integer", "minimum": 0, "description": "UTF-8 byte offset, 기본 0"},
                "max_bytes": {"type": "integer", "minimum": 1, "maximum": 1048576, "description": "반환 byte 상한, 기본 262144"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_list_fields",
            "description": "필드/누름틀 목록(이름·종류·값·명령)을 JSON으로. 누름틀(%clk)은 name으로 채울 수 있다.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_list_bookmarks",
            "description": "책갈피(bokm) 목록(이름)을 JSON으로.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_render",
            "description": "지정 페이지를 PNG 이미지로 렌더해 반환(에이전트가 문서를 직접 본다).",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "page": {"type": "integer", "description": "1-기반, 기본 1"},
                "dpi": {"type": "number", "minimum": hwp_render::MIN_DPI, "maximum": hwp_render::MAX_DPI, "description": "기본 120"},
                "font_dir": {"type": "string", "description": "추가 폰트 디렉터리(선택)"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_edit",
            "description": "CLI와 같은 strict·atomic·재읽기 검증 경로로 기존 문서를 편집한다. 기본은 미적용 요청 하나라도 있으면 실패.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"},
                "output": {"type": "string"},
                "replace": {"type": "array", "items": {"type": "object", "properties": {
                    "from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]},
                    "description": "텍스트 치환(모든 일치)"},
                "set_cell": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"},
                    "col": {"type": "integer"}, "text": {"type": "string"}},
                    "required": ["table", "row", "col", "text"]}, "description": "표 셀 설정(0-기반)"},
                "set_field": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["name", "value"]}, "description": "필드/누름틀 채우기(이름으로)"},
                "create_field": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "name": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["anchor", "name"]}, "description": "앵커 텍스트 뒤에 %clk 누름틀 생성(이름·선택 표시값; set_field로 채움)"},
                "create_bookmark": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "name": {"type": "string"}},
                    "required": ["anchor", "name"]}, "description": "앵커 텍스트 뒤에 책갈피(bokm 지점 표식) 생성"},
                "create_hyperlink": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "url": {"type": "string"}, "display": {"type": "string"}},
                    "required": ["anchor", "url"]}, "description": "앵커 텍스트 뒤에 하이퍼링크(%hlk) 생성(display 생략 시 URL 표시)"},
                "insert_image": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "path": {"type": "string"},
                    "width_mm": {"type": "number"}, "height_mm": {"type": "number"}},
                    "required": ["anchor", "path"]}, "description": "앵커 텍스트 뒤에 이미지(png/jpg/bmp/gif) 삽입(width_mm/height_mm 생략 시 원본 크기)"},
                "seal": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "path": {"type": "string"},
                    "size_mm": {"type": "number"}},
                    "required": ["anchor", "path"]}, "description": "앵커 문구 위에 도장 이미지 부유 배치"},
                "set_meta": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "title/author/subject/keywords 메타데이터"},
                "set_format": {"type": "array", "items": {"type": "object", "properties": {
                    "pattern": {"type": "string"}, "bold": {"type": "boolean"},
                    "italic": {"type": "boolean"}, "underline": {"type": "boolean"},
                    "strike": {"type": "boolean"}, "size": {"type": "number", "description": "pt"},
                    "color": {"type": "string", "description": "#RRGGBB 또는 색이름"}},
                    "required": ["pattern"]}, "description": "글자 서식(매칭 텍스트)"},
                "set_align": {"type": "array", "items": {"type": "object", "properties": {
                    "pattern": {"type": "string"},
                    "align": {"type": "string", "enum": ["left", "right", "center", "justify", "distribute", "divide"]}},
                    "required": ["pattern", "align"]}, "description": "문단 정렬(매칭 문단)"},
                "insert_para": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}, "text": {"type": "string"},
                    "before": {"type": "boolean", "description": "true면 앵커 문단 앞(기본 뒤)"}},
                    "required": ["anchor", "text"]}, "description": "문단 삽입(앵커 문단 앞/뒤, 모양 상속)"},
                "delete_para": {"type": "array", "items": {"type": "object", "properties": {
                    "matching": {"type": "string"}},
                    "required": ["matching"]}, "description": "매칭 텍스트가 든 문단 삭제(최소 1문단 유지)"},
                "add_row": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}},
                    "required": ["table"]}, "description": "N번째 표 끝에 빈 행 추가(0-기반, 병합 표는 거부)"},
                "add_col": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "at": {"type": "integer", "minimum": 0, "maximum": 65535}},
                    "required": ["table"]}, "description": "N번째 표의 at 위치(생략 시 끝)에 열 추가(0-기반, 전체 폭 유지, 병합 표도 지원)"},
                "delete_row": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"}},
                    "required": ["table", "row"]}, "description": "N번째 표의 R행 삭제(0-기반, 병합 행은 거부)"},
                "delete_col": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "col": {"type": "integer"}},
                    "required": ["table", "col"]}},
                "merge_cells": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "r1": {"type": "integer"},
                    "c1": {"type": "integer"}, "r2": {"type": "integer"}, "c2": {"type": "integer"}},
                    "required": ["table", "r1", "c1", "r2", "c2"]}},
                "split_cell": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"}, "col": {"type": "integer"}},
                    "required": ["table", "row", "col"]}},
                "allow_partial": {"type": "boolean", "description": "true면 일치한 요청만 게시; 기본 false"}
            }, "required": ["input", "output"]}
        }),
        json!({
            "name": "hwp_convert",
            "description": "포맷 변환. 출력 확장자(.hwp/.hwpx/.json/.md/.html/.pdf/.odt)로 결정. pdf는 텍스트 선택가능 벡터(이미지 포함). embed_bin이면 JSON에 이미지 base64 임베드.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "output": {"type": "string"},
                "embed_bin": {"type": "boolean"},
                "strict": {"type": "boolean", "description": "HWP/HWPX DROP 경고를 실패 처리; MCP 기본 true"}
            }, "required": ["input", "output"]}
        }),
        json!({
            "name": "hwp_new",
            "description": "CLI와 같은 strict·atomic·재읽기 검증 경로로 .hwp/.hwpx 새 문서를 생성.",
            "inputSchema": {"type": "object", "properties": {
                "output": {"type": "string"},
                "markdown": {"type": "string", "description": "markdown 본문(선택)"},
                "json": {"type": "string", "description": "IR JSON 본문(선택)"},
                "set_meta": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}}
            }, "required": ["output"]}
        }),
        json!({
            "name": "hwp_compose",
            "description": "DocumentSpec v1/v2 객체/JSON/YAML을 검증하고 CLI와 같은 deterministic·strict·atomic·재읽기 검증 경로로 .hwp/.hwpx 문서를 합성.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "spec": {
                        "oneOf": [{"type": "object"}, {"type": "string"}],
                        "description": "DocumentSpec v1/v2 객체 또는 JSON/YAML 문자열"
                    },
                    "spec_path": {"type": "string", "description": "DocumentSpec v1/v2 파일 경로; spec과 상호 배타적"},
                    "format": {"type": "string", "enum": ["json", "yaml"], "description": "문자열 spec 또는 확장자 없는 입력의 포맷"},
                    "base_dir": {"type": "string", "description": "inline spec의 상대 asset 기준 디렉터리"},
                    "output": {"type": "string", "description": "출력 .hwp/.hwpx 경로"},
                    "dry_run": {"type": "boolean", "description": "검증·컴파일 보고서만 반환하고 파일을 쓰지 않음"},
                    "allow_visual_fallback": {"type": "boolean", "deprecated": true, "description": "[deprecated] v1 compatibility only; DocumentSpec v2가 true를 받으면 policy_conflict로 거부"}
                },
                "required": ["output"],
                "oneOf": [
                    {"required": ["spec"], "not": {"required": ["spec_path"]}},
                    {"required": ["spec_path"], "not": {"required": ["spec"]}}
                ]
            }
        }),
        json!({
            "name": "hwp_template",
            "description": "TemplateSpec/Data v1의 typed AST를 CLI와 같은 bounded·deterministic·strict 경로로 확장하고 native HWP/HWPX를 생성한다. reference_hwpx는 package-surgical 보존 모드다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "template": {
                        "oneOf": [{"type": "object"}, {"type": "string"}],
                        "description": "TemplateSpec v1 객체 또는 JSON/YAML 문자열"
                    },
                    "template_path": {"type": "string"},
                    "template_format": {"type": "string", "enum": ["json", "yaml"]},
                    "data": {
                        "oneOf": [{"type": "object"}, {"type": "string"}],
                        "description": "TemplateData v1 객체 또는 JSON/YAML 문자열"
                    },
                    "data_path": {"type": "string"},
                    "data_format": {"type": "string", "enum": ["json", "yaml"]},
                    "base_dir": {"type": "string", "description": "inline template의 상대 reference/asset 기준 디렉터리"},
                    "output": {"type": "string", "description": "출력 .hwp/.hwpx 경로"},
                    "dry_run": {"type": "boolean", "description": "실제 확장·writer·검증 후 게시만 생략"}
                },
                "required": ["output"],
                "allOf": [
                    {"oneOf": [
                        {"required": ["template"], "not": {"required": ["template_path"]}},
                        {"required": ["template_path"], "not": {"required": ["template"]}}
                    ]},
                    {"oneOf": [
                        {"required": ["data"], "not": {"required": ["data_path"]}},
                        {"required": ["data_path"], "not": {"required": ["data"]}}
                    ]}
                ]
            }
        }),
        json!({
            "name": "hwp_diff",
            "description": "렌더 결과를 기준 PNG와 비교해 오차(잉크 적용률·위치 오프셋·픽셀 차이율)를 측정.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "ref": {"type": "string", "description": "기준 PNG 경로"},
                "page": {"type": "integer"}, "dpi": {"type": "number", "minimum": hwp_render::MIN_DPI, "maximum": hwp_render::MAX_DPI},
                "font_dir": {"type": "string"}
            }, "required": ["input", "ref"]}
        }),
        json!({
            "name": "hwp_slots",
            "description": "`{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록을 등장 순서로 반환.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_fill",
            "description": "hwpx 템플릿의 `{{name}}`를 값으로 치환(패키지·미리보기 보존). hwpx 입력 전용.",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "output": {"type": "string"},
                "values": {"type": "object", "additionalProperties": {"type": "string"},
                    "description": "{자리표시자이름: 값} 객체"},
                "allow_partial": {"type": "boolean", "description": "미발견 키가 있어도 일치한 값만 게시; 기본 false"}
            }, "required": ["input", "output", "values"]}
        }),
        json!({
            "name": "hwp_validate",
            "description": "구조 검증(mimetype·필수 엔트리·XML 파싱). {valid, errors, warnings} 반환.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_certify",
            "description": "versioned policy로 package/반복 import/native render/선택적 LibreOffice+H2Orestart 독립 import를 인증하고 새 artifact 디렉터리를 원자적으로 게시한다. passed만 성공이다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "input": {"type": "string"},
                    "policy": {"type": "string", "description": "hwp-certification-policy-v1 JSON/YAML 경로"},
                    "report": {"type": "string", "description": "존재하지 않는 artifact 디렉터리 경로"}
                },
                "required": ["input", "policy", "report"]
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx {
            font_dirs: vec![PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fonts"
            ))],
        }
    }

    fn fixture(rel: &str) -> String {
        format!("{}/../../fixtures/{rel}", env!("CARGO_MANIFEST_DIR"))
    }

    /// fixture 바이너리는 저장소에서 제외된다(로컬 전용). 없으면 `true`(스킵).
    fn skip_if_no_fixtures() -> bool {
        if std::path::Path::new(&fixture("hwp5/hello_world.hwp")).exists() {
            return false;
        }
        eprintln!("스킵: fixtures 없음 — fixtures/README.md 참고");
        true
    }

    fn call(line: &str) -> Value {
        let resp = handle_request(line, &ctx()).expect("응답 있어야 함");
        serde_json::from_str(&resp).unwrap()
    }

    #[test]
    fn initialize_응답() {
        let v = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(v["result"]["serverInfo"]["name"].is_string());
    }

    #[test]
    fn 알림은_응답없음() {
        assert!(
            handle_request(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &ctx()
            )
            .is_none()
        );
    }

    #[test]
    fn 미지원_메서드_에러() {
        let v = call(r#"{"jsonrpc":"2.0","id":2,"method":"no_such_method"}"#);
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn tools_list_도구_노출() {
        let v = call(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let tools = v["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in [
            "hwp_info",
            "hwp_read",
            "hwp_render",
            "hwp_edit",
            "hwp_convert",
            "hwp_new",
            "hwp_compose",
            "hwp_template",
            "hwp_diff",
            "hwp_slots",
            "hwp_fill",
            "hwp_validate",
            "hwp_certify",
            "hwp_list_bookmarks",
        ] {
            assert!(names.contains(&expected), "{expected} 누락");
        }
    }

    #[test]
    fn tools_list_exposes_read_bounds_and_add_col_position() {
        let tools = tool_defs();
        let read = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_read")
            .unwrap();
        assert_eq!(
            read["inputSchema"]["properties"]["max_bytes"]["maximum"],
            MAX_READ_BYTES
        );
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_edit")
            .unwrap();
        assert_eq!(
            edit["inputSchema"]["properties"]["add_col"]["items"]["properties"]["at"]["type"],
            "integer"
        );
        for name in ["hwp_render", "hwp_diff"] {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            let dpi = &tool["inputSchema"]["properties"]["dpi"];
            assert_eq!(dpi["minimum"], hwp_render::MIN_DPI);
            assert_eq!(dpi["maximum"], hwp_render::MAX_DPI);
        }
    }

    #[test]
    fn compose_dry_run_accepts_inline_spec_without_writing() {
        let output = std::env::temp_dir().join(format!(
            "hwp-compose-mcp-dry-run-{}-{}.hwpx",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let result = tool_compose(&json!({
            "spec": {
                "version": "1.0",
                "sections": [{
                    "blocks": [{
                        "type": "paragraph",
                        "runs": [{"type": "text", "text": "본문"}]
                    }]
                }]
            },
            "output": output,
            "dry_run": true,
            "allow_visual_fallback": true
        }))
        .expect("compose dry-run");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["native"], true);
        assert!(!output.exists());
    }

    #[test]
    fn compose_v2_rejects_deprecated_global_fallback_policy() {
        let error = tool_compose(&json!({
            "spec": {
                "version": "2.0",
                "document": {
                    "version": "1.0",
                    "sections": [{"blocks": [{
                        "type": "paragraph",
                        "runs": [{"type": "text", "text": "본문"}]
                    }]}]
                },
                "visuals": []
            },
            "output": "out.hwpx",
            "dry_run": true,
            "allow_visual_fallback": true
        }))
        .unwrap_err();
        assert!(error.contains("\"code\": \"policy_conflict\""), "{error}");
        assert!(error.contains("$.policy"), "{error}");
    }

    #[test]
    fn compose_schema_marks_global_fallback_policy_deprecated() {
        let compose = tool_defs()
            .into_iter()
            .find(|tool| tool["name"] == "hwp_compose")
            .unwrap();
        let property = &compose["inputSchema"]["properties"]["allow_visual_fallback"];
        assert_eq!(property["type"], "boolean");
        assert_eq!(property["deprecated"], true);
    }

    #[test]
    fn compose_rejects_unknown_argument() {
        let error = tool_compose(&json!({
            "spec": {"version": "1.0", "sections": []},
            "output": "out.hwpx",
            "unknown": true
        }))
        .unwrap_err();
        assert!(error.contains("unknown"));
    }

    #[test]
    fn template_dry_run_has_cli_report_contract_and_rejects_unknown_argument() {
        let output = std::env::temp_dir().join(format!(
            "hwp-template-mcp-dry-run-{}-{}.hwpx",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_file(&output);
        let result = tool_template(&json!({
            "template": {
                "version": "1.0",
                "variables": {"title": {"type": "string", "required": true}},
                "source": {"mode": "compose", "document": {
                    "version": "1.0",
                    "sections": [{"blocks": [{
                        "type": "paragraph",
                        "runs": [{"type": "text", "text": {
                            "node": "value", "pointer": "/values/title", "as": "text"
                        }}]
                    }]}]
                }}
            },
            "data": {"version": "1.0", "values": {"title": "MCP"}},
            "output": output,
            "dry_run": true
        }))
        .expect("template dry-run");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["schema_version"], "1.0");
        assert_eq!(report["data_schema_version"], "1.0");
        assert_eq!(report["mode"], "compose");
        assert_eq!(report["template_validation"], "passed");
        assert_eq!(report["data_validation"], "passed");
        assert_eq!(report["semantic_validation"], "not_run");
        assert_eq!(report["package_validation"], "not_run");
        assert!(!output.exists());

        let error = tool_template(&json!({
            "template": {"version": "1.0", "variables": {}, "source": {"mode": "compose", "document": {}}},
            "data": {"version": "1.0", "values": {}},
            "output": "out.hwpx",
            "unknown": true
        }))
        .unwrap_err();
        assert!(error.contains("unknown"));
    }

    #[test]
    fn mcp_render_and_diff_reject_non_finite_or_out_of_range_dpi_before_loading() {
        for dpi in [0.0, -1.0, 601.0, 1.0e300] {
            let args = json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "dpi": dpi});
            let render = tool_render(&args, &ctx()).unwrap_err();
            let diff = tool_diff(&args, &ctx()).unwrap_err();
            assert!(render.contains("DPI는 유한한"), "{render}");
            assert!(diff.contains("DPI는 유한한"), "{diff}");
        }
        for args in [
            json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "dpi": "600"}),
            json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "page": -1}),
            json!({"path": "missing.hwpx", "input": "missing.hwpx", "ref": "missing.png", "page": "1"}),
        ] {
            assert!(tool_render(&args, &ctx()).is_err());
            assert!(tool_diff(&args, &ctx()).is_err());
        }
    }

    #[test]
    fn protocol_line_reader_rejects_oversized_request_without_unbounded_allocation() {
        let mut reader = std::io::Cursor::new(vec![b'a'; 17]);
        let mut line = String::new();
        let error = read_line_bounded(&mut reader, &mut line, 16).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(line.len() <= 17);
    }

    #[test]
    fn call_hwp_validate() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"hwp_validate","arguments":{{"path":"{}"}}}}}}"#,
            fixture("hwpx/minimal.hwpx")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"valid\": true"), "유효해야: {text}");
    }

    #[test]
    fn call_hwp_read_json() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"hwp_read","arguments":{{"path":"{}","format":"plain"}}}}}}"#,
            fixture("hwp5/hello_world.hwp")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Hello"), "본문 추출: {text:?}");
    }

    #[test]
    fn call_hwp_render_이미지() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"hwp_render","arguments":{{"path":"{}","page":1,"dpi":96}}}}}}"#,
            fixture("hwp5/hello_world.hwp")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let content = v["result"]["content"].as_array().unwrap();
        let img = content
            .iter()
            .find(|c| c["type"] == "image")
            .expect("이미지 콘텐츠");
        assert_eq!(img["mimeType"], "image/png");
        assert!(
            img["data"].as_str().unwrap().len() > 100,
            "base64 PNG 비어있음"
        );
    }

    #[test]
    fn call_hwp_list_bookmarks() {
        if skip_if_no_fixtures() {
            return;
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{{"name":"hwp_list_bookmarks","arguments":{{"path":"{}"}}}}}}"#,
            fixture("hwp5/bookmark.hwp")
        );
        let v = call(&line);
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("책갈피테스트"), "책갈피 이름: {text}");
    }

    #[test]
    fn call_잘못된_인자_오류() {
        let v = call(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"hwp_read","arguments":{}}}"#,
        );
        assert_eq!(v["result"]["isError"], true);
    }

    fn temp_file(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("hwp-cli-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join(name)
    }

    fn create_hwpx(path: &Path, markdown: &str) {
        crate::commands::new::execute(
            path,
            crate::commands::new::NewInput::Markdown {
                text: markdown,
                base_dir: None,
            },
            &[],
            None,
        )
        .unwrap();
    }

    #[test]
    fn mcp_mutations_share_cli_atomic_and_noop_contracts() {
        let source = temp_file("mutation-source.hwpx");
        let edit_destination = temp_file("mutation-edit.hwpx");
        let fill_destination = temp_file("mutation-fill.hwpx");
        let convert_destination = temp_file("mutation-convert.unsupported");
        create_hwpx(&source, "{{수신}} 본문");

        std::fs::write(&edit_destination, b"EDIT ORIGINAL").unwrap();
        let edit = tool_edit(&json!({
            "input": source,
            "output": edit_destination,
            "replace": [{"from": "없는본문", "to": "값"}]
        }));
        assert!(edit.is_err(), "0건 편집은 MCP도 실패");
        assert_eq!(std::fs::read(&edit_destination).unwrap(), b"EDIT ORIGINAL");

        std::fs::write(&fill_destination, b"FILL ORIGINAL").unwrap();
        let fill = tool_fill(&json!({
            "input": source,
            "output": fill_destination,
            "values": {"없는키": "값"}
        }));
        assert!(fill.is_err(), "0건 fill은 MCP도 실패");
        assert_eq!(std::fs::read(&fill_destination).unwrap(), b"FILL ORIGINAL");

        std::fs::write(&convert_destination, b"CONVERT ORIGINAL").unwrap();
        let convert = tool_convert(&json!({
            "input": source,
            "output": convert_destination
        }));
        assert!(convert.is_err(), "미지원 확장자 변환은 실패");
        assert_eq!(
            std::fs::read(&convert_destination).unwrap(),
            b"CONVERT ORIGINAL"
        );

        for path in [
            &source,
            &edit_destination,
            &fill_destination,
            &convert_destination,
        ] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_new_fails_closed_on_drop_and_preserves_destination() {
        let destination = temp_file("new-drop.hwpx");
        std::fs::write(&destination, b"ORIGINAL").unwrap();

        let mut doc = hwp_convert::from_markdown("본문");
        let paragraph = &mut doc.sections[0].paragraphs[0];
        let control_index = paragraph.controls.len() as u32;
        paragraph
            .controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"zzzz",
                data: Vec::new(),
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
            }));
        let insert_at = paragraph.chars.len().saturating_sub(1);
        paragraph.chars.insert(
            insert_at,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::ctrl_char::OBJECT,
                ctrl_id: *b"zzzz",
                payload: vec![0; 12],
                ctrl_index: Some(control_index),
            },
        );
        let document_json = hwp_convert::to_json(&doc, true, false).unwrap();
        let result = tool_new(&json!({
            "output": destination,
            "json": document_json,
        }));
        assert!(
            result
                .as_ref()
                .is_err_and(|error| error.contains("보존 불가") && error.contains("zzzz")),
            "DROP은 hard failure여야: {result:?}"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"ORIGINAL");
        let _ = std::fs::remove_file(destination);
    }

    #[test]
    fn mcp_partial_edit_returns_machine_readable_warnings() {
        let source = temp_file("partial-source.hwpx");
        let destination = temp_file("partial-destination.hwpx");
        create_hwpx(&source, "있는본문");
        let content = tool_edit(&json!({
            "input": source,
            "output": destination,
            "replace": [
                {"from": "있는본문", "to": "바뀐본문"},
                {"from": "없는본문", "to": "값"}
            ],
            "allow_partial": true
        }))
        .expect("allow_partial MCP edit");
        let report: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["applied"], 1);
        assert!(
            report["warnings"]
                .as_array()
                .is_some_and(|warnings| !warnings.is_empty())
        );
        for path in [&source, &destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_read_is_utf8_bounded_and_pageable() {
        let source = temp_file("bounded-read.hwpx");
        create_hwpx(&source, "가나다라마바사");
        let first = tool_read(&json!({
            "path": source,
            "format": "plain",
            "max_bytes": 7
        }))
        .unwrap();
        assert!(first[0]["text"].as_str().unwrap().len() <= 7);
        let metadata: Value = serde_json::from_str(first[1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["truncated"], true);
        let next = metadata["next_offset"].as_u64().unwrap();
        let second = tool_read(&json!({
            "path": source,
            "format": "plain",
            "offset": next,
            "max_bytes": 7
        }))
        .unwrap();
        assert!(!second[0]["text"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_structured_replace_preserves_cli_delimiters_as_data() {
        let source = temp_file("delimiter-source.hwpx");
        let destination = temp_file("delimiter-destination.hwpx");
        create_hwpx(&source, "A=>B");

        tool_edit(&json!({
            "input": source,
            "output": destination,
            "replace": [{"from": "A=>B", "to": "X=Y=>Z"}]
        }))
        .expect("구조화 치환");

        let edited = load_document(&destination).unwrap();
        assert!(edited.plain_text().contains("X=Y=>Z"));
        assert!(!edited.plain_text().contains("A=>B"));
        for path in [&source, &destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_structured_metadata_and_paragraphs_keep_delimiters() {
        let source = temp_file("typed-structural-source.hwpx");
        let destination = temp_file("typed-structural-destination.hwpx");
        create_hwpx(&source, "Anchor=>Here");

        tool_edit(&json!({
            "input": source,
            "output": destination,
            "set_meta": [{"key": "title", "value": "A=B=>C"}],
            "insert_para": [{
                "anchor": "Anchor=>Here",
                "text": "New=>Text=1"
            }]
        }))
        .expect("구조화 metadata/문단 편집");

        let edited = load_document(&destination).unwrap();
        assert_eq!(edited.metadata.title.as_deref(), Some("A=B=>C"));
        assert!(edited.plain_text().contains("New=>Text=1"));
        for path in [&source, &destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_render_rasterizes_only_selected_page_and_reports_total_pages() {
        let source = temp_file("selected-render.hwpx");
        let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n");
        doc.sections[0].paragraphs[1].header.break_type |= 0x04;
        doc.sections[0].paragraphs[2].header.break_type |= 0x04;
        hwpx::write_document(&doc, &source).unwrap();

        let content = tool_render(&json!({"path": source, "page": 2, "dpi": 36}), &ctx()).unwrap();
        assert!(
            content[0]["text"]
                .as_str()
                .is_some_and(|summary| summary.contains("페이지 2/3 렌더"))
        );
        assert_eq!(content[1]["type"], "image");
        let _ = std::fs::remove_file(source);
    }
}
