//! `hwp mcp` — MCP(Model Context Protocol) 서버의 transport 독립 코어.
//!
//! tokio/SDK 없이 serde_json만으로 동기 JSON-RPC 2.0을 구현한다.
//! 에이전트(Claude 등)가 도구 호출로 HWP를 **읽고·렌더해서 보고·편집·변환**하게 한다.
//! 이 모듈은 요청 처리·도구 레지스트리·디스패치를 담고, framing은 adapter가 맡는다.
//!
//! 도구는 라이브러리 계층을 직접 감싼다(commands/*::run 아님 — 그건 stdout 출력).

mod authority;
mod http;
mod stdio;

pub use authority::FileAuthority;
pub use http::serve;
pub use stdio::run;

use authority::{checked_read_path, checked_write_path, font_dirs_for};

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::cat::{
    HWP_PASSWORD_REQUIRED_OR_INVALID, LoadDocumentError, LoadOptions, ResolvedPassword,
    load_document, load_document_with_options,
};
use zeroize::Zeroize as _;

/// Supported MCP protocol versions (newest first). Used in initialize negotiation.
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_REQUEST_LINE_BYTES: usize = 1024 * 1024;
const DEFAULT_READ_BYTES: usize = 256 * 1024;
const MAX_READ_BYTES: usize = 1024 * 1024;

/// 인라인 전송 한 건의 복호 후 상한 (doc 22 §3.3).
///
/// base64는 3바이트를 4문자로 부풀리므로 512 KiB는 약 699 KB로 인코딩되고, 1 MiB인
/// `MAX_REQUEST_LINE_BYTES` 안에 JSON-RPC 봉투가 들어갈 여유가 남는다. Tier A의
/// `/files`는 64 MiB까지 받으므로, 이 상한은 사이드밴드가 없는 Tier B의 제약이다.
const MAX_INLINE_CONTENT_BYTES: usize = 512 * 1024;
/// 복호 전에 거절하기 위한 인코딩 길이 상한. `decode`는 출력 전체를 할당하므로
/// 먼저 막지 않으면 요청 하나로 상한을 훨씬 넘는 메모리를 잡을 수 있다.
const MAX_INLINE_CONTENT_B64_CHARS: usize = MAX_INLINE_CONTENT_BYTES.div_ceil(3) * 4;

/// 한 줄 JSON-RPC 요청 → 응답 JSON 문자열. 알림(id 없음)이면 None.
pub fn handle_request(line: &str, ctx: &dyn FileAuthority) -> Option<String> {
    let mut req: Value = match serde_json::from_str(line) {
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
    if is_notification {
        scrub_password_values(&mut req);
        return None;
    }

    match method {
        "initialize" => {
            // Protocol negotiation: echo the client-requested version if supported, otherwise respond with the newest version.
            let requested = req
                .get("params")
                .and_then(|params| params.get("protocolVersion"))
                .and_then(Value::as_str);
            let protocol_version = match requested {
                Some(version) if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) => version,
                _ => SUPPORTED_PROTOCOL_VERSIONS[0],
            };
            Some(result_response(
                id_or_null(id),
                json!({
                    "protocolVersion": protocol_version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "hwp-cli", "version": env!("CARGO_PKG_VERSION")},
                }),
            ))
        }
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(result_response(id_or_null(id), json!({}))),
        "tools/list" => Some(result_response(
            id_or_null(id),
            json!({ "tools": tool_defs() }),
        )),
        "tools/call" => {
            let (name, mut args) = req
                .get_mut("params")
                .and_then(Value::as_object_mut)
                .map(|params| {
                    let name = params
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let args = params.remove("arguments").unwrap_or_else(|| json!({}));
                    (name, args)
                })
                .unwrap_or_else(|| (String::new(), json!({})));
            let result = call_tool(&name, &mut args, ctx);
            scrub_password_values(&mut args);
            scrub_password_values(&mut req);
            Some(result_response(id_or_null(id), result))
        }
        _ => Some(error_response(
            id_or_null(id),
            -32601,
            &format!("method not found: {method}"),
        )),
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

/// Bounded tool failure shape. Password refusals alone add structured content;
/// all pre-existing tool errors retain their text-only behavior.
#[derive(Debug)]
struct McpToolError {
    message: String,
    structured_content: Option<Value>,
}

type McpToolResult = Result<Vec<Value>, McpToolError>;

impl From<String> for McpToolError {
    fn from(message: String) -> Self {
        Self {
            message,
            structured_content: None,
        }
    }
}

impl From<&str> for McpToolError {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

impl McpToolError {
    fn password(path: &Path) -> Self {
        let (format, algorithm_kdf) = match crate::format::detect(path) {
            Ok(crate::format::FileFormat::Hwp5) => ("hwp5", "HWP5-EncryptVersion4"),
            Ok(crate::format::FileFormat::Hwpx) => ("hwpx", "AES256-CBC/PBKDF2-HMAC-SHA1"),
            Err(_) => ("unknown", "unknown"),
        };
        Self {
            message: HWP_PASSWORD_REQUIRED_OR_INVALID.to_owned(),
            structured_content: Some(json!({
                "code": HWP_PASSWORD_REQUIRED_OR_INVALID,
                "format": format,
                "algorithm_kdf": algorithm_kdf,
                "stage": "credential-validation",
            })),
        }
    }
}

fn load_mcp_document(
    path: &Path,
    password: Option<&ResolvedPassword>,
) -> Result<hwp_model::Document, McpToolError> {
    load_document_with_options(path, &LoadOptions { password }).map_err(|error| match error {
        LoadDocumentError::Password(_) => McpToolError::password(path),
        LoadDocumentError::Other(error) => McpToolError::from(error.to_string()),
    })
}

fn map_mcp_convert_error(path: &Path, error: anyhow::Error) -> McpToolError {
    if matches!(
        error.downcast_ref::<LoadDocumentError>(),
        Some(LoadDocumentError::Password(_))
    ) {
        McpToolError::password(path)
    } else {
        McpToolError::from(format!("{error:#}"))
    }
}

fn take_scoped_password(
    args: &mut Value,
    allowed: &[&str],
) -> Result<Option<ResolvedPassword>, McpToolError> {
    let object = args
        .as_object_mut()
        .ok_or_else(|| McpToolError::from("MCP 도구 인자는 객체여야 합니다".to_owned()))?;
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(McpToolError::from(format!("알 수 없는 인자: {unknown}")));
    }
    match object.remove("password") {
        None => Ok(None),
        Some(Value::String(password)) => Ok(Some(ResolvedPassword::from_scoped_string(password))),
        Some(_) => Err(McpToolError::from(
            "password는 문자열이어야 합니다".to_owned(),
        )),
    }
}

fn reject_password_outside_scope(args: &Value) -> Result<(), McpToolError> {
    if args.get("password").is_some() {
        return Err(McpToolError::from("알 수 없는 인자: password".to_owned()));
    }
    Ok(())
}

fn scrub_password_values(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if key == "password" {
                    if let Value::String(secret) = value {
                        secret.zeroize();
                    }
                } else {
                    scrub_password_values(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_password_values(item);
            }
        }
        _ => {}
    }
}

/// 도구를 실행해 `tools/call` result를 만든다. 실행 오류는 isError=true content로.
fn call_tool(name: &str, args: &mut Value, ctx: &dyn FileAuthority) -> Value {
    let result: McpToolResult = match name {
        "hwp_read" => tool_read_scoped(args, ctx),
        "hwp_render" => tool_render_scoped(args, ctx),
        "hwp_convert" => tool_convert_scoped(args, ctx),
        "hwp_merge" => tool_merge_scoped(args, ctx),
        "hwp_split" => tool_split_scoped(args, ctx),
        "hwp_compare" => tool_compare_scoped(args, ctx),
        "hwp_info" => reject_password_outside_scope(args)
            .and_then(|_| tool_info(args, ctx).map_err(Into::into)),
        "hwp_grep" => reject_password_outside_scope(args)
            .and_then(|_| tool_grep(args, ctx).map_err(Into::into)),
        "hwp_list_fields" => reject_password_outside_scope(args)
            .and_then(|_| tool_list_fields(args, ctx).map_err(Into::into)),
        "hwp_list_bookmarks" => reject_password_outside_scope(args)
            .and_then(|_| tool_list_bookmarks(args, ctx).map_err(Into::into)),
        "hwp_edit" => reject_password_outside_scope(args)
            .and_then(|_| tool_edit(args, ctx).map_err(Into::into)),
        "hwp_new" => reject_password_outside_scope(args)
            .and_then(|_| tool_new(args, ctx).map_err(Into::into)),
        "hwp_compose" => reject_password_outside_scope(args)
            .and_then(|_| tool_compose(args, ctx).map_err(Into::into)),
        "hwp_template" => reject_password_outside_scope(args)
            .and_then(|_| tool_template(args, ctx).map_err(Into::into)),
        "hwp_diff" => reject_password_outside_scope(args)
            .and_then(|_| tool_diff(args, ctx).map_err(Into::into)),
        "hwp_slots" => reject_password_outside_scope(args)
            .and_then(|_| tool_slots(args, ctx).map_err(Into::into)),
        "hwp_fill" => reject_password_outside_scope(args)
            .and_then(|_| tool_fill(args, ctx).map_err(Into::into)),
        "hwp_validate" => reject_password_outside_scope(args)
            .and_then(|_| tool_validate(args, ctx).map_err(Into::into)),
        "hwp_lint" => reject_password_outside_scope(args)
            .and_then(|_| tool_lint(args, ctx).map_err(Into::into)),
        "hwp_certify" => reject_password_outside_scope(args)
            .and_then(|_| tool_certify(args, ctx).map_err(Into::into)),
        "hwp_put_file" => reject_password_outside_scope(args)
            .and_then(|_| tool_put_file(args, ctx).map_err(Into::into)),
        "hwp_get_file" => reject_password_outside_scope(args)
            .and_then(|_| tool_get_file(args, ctx).map_err(Into::into)),
        other => Err(McpToolError::from(format!("알 수 없는 도구: {other}"))),
    };
    match result {
        Ok(content) => json!({"content": content, "isError": false}),
        Err(error) => {
            let mut response = json!({
                "content": [text_content(&format!("오류: {}", error.message))],
                "isError": true,
            });
            if let Some(structured_content) = error.structured_content {
                response["structuredContent"] = structured_content;
            }
            response
        }
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

fn arg_f64_opt(args: &Value, key: &str) -> Result<Option<f64>, String> {
    args.get(key)
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| format!("{key}는 숫자여야 합니다"))
        })
        .transpose()
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

fn optional_item_usize(item: &Value, operation: &str, key: &str) -> Result<Option<usize>, String> {
    item.get(key)
        .map(|_| required_item_usize(item, operation, key))
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

/// Builds a [`hwp_convert::ParaProps`] from one typed-edit item. Shared by `set_para` and
/// `set_cell_para`; `operation` names the caller's array in every error message, and the
/// units match the CLI's `parse_para_props` exactly.
fn para_props_item(item: &Value, operation: &str) -> Result<hwp_convert::ParaProps, String> {
    // The CLI line-spacing (% integer | Npt) split into two numeric arguments — mutually exclusive.
    let line_spacing = match (
        optional_item_f32(item, operation, "line_spacing_pct")?,
        optional_item_f32(item, operation, "line_spacing_pt")?,
    ) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{operation}는 line_spacing_pct와 line_spacing_pt를 함께 지정할 수 없습니다"
            ));
        }
        (Some(pct), None) => Some((0, pct as i32)),
        (None, Some(pt)) => Some((1, (pt * 100.0).round() as i32)),
        (None, None) => None,
    };
    let align = match optional_item_str(item, operation, "align")? {
        Some(name) => Some(
            crate::commands::edit::parse_align(name)
                .map_err(|error| format!("{operation}: {error}"))?,
        ),
        None => None,
    };
    Ok(hwp_convert::ParaProps {
        line_spacing,
        indent: optional_item_f32(item, operation, "indent_mm")?
            .map(crate::commands::edit::mm_to_hwpunit),
        margin_left: optional_item_f32(item, operation, "left_mm")?
            .map(crate::commands::edit::mm_to_hwpunit),
        margin_right: optional_item_f32(item, operation, "right_mm")?
            .map(crate::commands::edit::mm_to_hwpunit),
        spacing_top: optional_item_f32(item, operation, "top_mm")?
            .map(crate::commands::edit::mm_to_hwpunit),
        spacing_bottom: optional_item_f32(item, operation, "bottom_mm")?
            .map(crate::commands::edit::mm_to_hwpunit),
        align,
    })
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

// ---- 도구 핸들러 ----

fn tool_info(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let v = crate::commands::info::info_json(&path).map_err(|e| e.to_string())?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

fn tool_read_scoped(args: &mut Value, ctx: &dyn FileAuthority) -> McpToolResult {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let password = take_scoped_password(
        args,
        &[
            "path",
            "format",
            "with_header_footer",
            "with_hidden",
            "with_segments",
            "offset",
            "max_bytes",
            "password",
        ],
    )?;
    let format = arg_str_opt(args, "format")?.unwrap_or("plain");
    let with_header_footer = arg_bool(args, "with_header_footer", false)?;
    let with_hidden = arg_bool(args, "with_hidden", false)?;
    let with_segments = arg_bool(args, "with_segments", false)?;
    // Same contract as cat: with_segments is markdown-only, and the with_* flags apply
    // only to plain/markdown (html/json/csv take no options — they are ignored if given).
    if with_segments && !matches!(format, "markdown" | "md") {
        return Err(format!("with_segments는 format=markdown 전용입니다 (요청: {format})").into());
    }
    let doc = load_mcp_document(&path, password.as_ref())?;
    let text_options = || hwp_model::TextOptions {
        include_header_footer: with_header_footer,
        include_hidden: with_hidden,
    };
    let md_options = || hwp_convert::MarkdownOptions {
        text: text_options(),
        ..Default::default()
    };
    // With with_segments, also collect the character-range segment map alongside the markdown.
    let (text, segments) = match format {
        "plain" => (doc.plain_text_with(&text_options()), None),
        "markdown" | "md" if with_segments => {
            let (markdown, segments) = hwp_convert::to_markdown_with_segments(&doc, &md_options())
                .map_err(|e| e.to_string())?;
            (markdown, Some(segments))
        }
        "markdown" | "md" => (
            hwp_convert::to_markdown_with(&doc, &md_options()).map_err(|e| e.to_string())?,
            None,
        ),
        "json" => (
            hwp_convert::to_json(&doc, true, false).map_err(|e| e.to_string())?,
            None,
        ),
        "html" => (hwp_convert::to_html(&doc), None),
        "csv" => (hwp_convert::to_csv(&doc), None),
        other => {
            return Err(
                format!("알 수 없는 format: {other} (plain|markdown|json|html|csv)").into(),
            );
        }
    };
    let offset = usize::try_from(arg_u64(args, "offset", 0)?)
        .map_err(|_| "offset이 플랫폼 범위를 넘습니다".to_string())?;
    let max_bytes = usize::try_from(arg_u64(args, "max_bytes", DEFAULT_READ_BYTES as u64)?)
        .map_err(|_| "max_bytes가 플랫폼 범위를 넘습니다".to_string())?;
    if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
        return Err(format!("max_bytes는 1..={MAX_READ_BYTES} 범위여야 합니다").into());
    }
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err(format!(
            "offset은 UTF-8 경계인 0..={} byte 범위여야 합니다",
            text.len()
        )
        .into());
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
    let content = match segments {
        // Same shape as cat's single-line JSON envelope. The markdown holds only the
        // returned window, and segments are filtered to those intersecting the window
        // while offsets stay absolute against the full markdown (unicode characters).
        Some(segments) => {
            let char_start = text[..offset].chars().count();
            let char_end = text[..end].chars().count();
            let segments: Vec<Value> = segments
                .iter()
                .filter(|s| s.start < char_end && s.end > char_start)
                .map(|s| {
                    json!({
                        "kind": "para",
                        "section": s.section,
                        "para": s.para,
                        "start": s.start,
                        "end": s.end,
                    })
                })
                .collect();
            let envelope = json!({
                "markdown": &text[offset..end],
                "segments": segments,
            });
            serde_json::to_string(&envelope).map_err(|e| e.to_string())?
        }
        None => text[offset..end].to_string(),
    };
    Ok(vec![
        text_content(&content),
        text_content(&serde_json::to_string(&metadata).unwrap_or_default()),
    ])
}

fn tool_list_fields(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
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

fn tool_list_bookmarks(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let bookmarks: Vec<Value> = hwp_convert::list_bookmarks(&doc)
        .iter()
        .map(|b| json!({ "name": b.name }))
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&bookmarks).unwrap_or_default(),
    )])
}

fn tool_slots(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    let items: Vec<Value> = hwp_convert::scan_placeholders(&doc)
        .iter()
        .map(|p| json!({ "name": p.name, "occurrences": p.occurrences }))
        .collect();
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({ "placeholders": items })).unwrap_or_default(),
    )])
}

fn tool_fill(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
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
    // parts(선택): {앵커이름: 부분 파일 경로} — 부분(md+HTML) 블록을 앵커 문단에 이식.
    let parts_obj = args.get("parts").and_then(Value::as_object);
    let report = if let Some(parts) = parts_obj {
        if parts.is_empty() && values.is_empty() {
            return Err("values와 parts가 모두 비어 있습니다".into());
        }
        let mut set: Vec<String> = values.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for (k, v) in parts {
            let path = v
                .as_str()
                .ok_or("parts 값은 부분 파일 경로 문자열이어야 합니다")?;
            let path = checked_read_path(ctx, path)?;
            set.push(format!("{k}=@{}", path.display()));
        }
        crate::commands::fill::execute(
            &input,
            &output,
            &set,
            None,
            arg_bool(args, "allow_partial", false)?,
            ctx.roots(),
        )
        .map_err(|error| format!("{error:#}"))?
    } else {
        crate::commands::fill::execute_values(
            &input,
            &output,
            &values,
            arg_bool(args, "allow_partial", false)?,
        )
        .map_err(|error| format!("{error:#}"))?
    };
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&crate::commands::fill::report_json(&report))
            .unwrap_or_default(),
    )])
}

fn tool_validate(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let v = crate::commands::validate::validate_json(&path);
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&v).unwrap_or_default(),
    )])
}

/// `hwp_lint` — path-only (D-09): no inline text, no profile argument in v1.
/// The path goes through the same canonicalize + root-containment sandbox as
/// every other read tool (`checked_read_path`), then the shared lint entry —
/// findings carry rule_id/severity/line/col/message only, no source excerpts
/// (T-02.3-04).
fn tool_lint(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let report =
        crate::commands::lint::lint_path_json(&path, hwp_convert::lint::LintProfile::Gongmun);
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&report).unwrap_or_default(),
    )])
}

fn tool_certify(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "input" | "policy" | "report"))
    {
        return Err(format!("알 수 없는 hwp_certify 인자: {unknown}"));
    }
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let policy = checked_read_path(ctx, arg_str(args, "policy")?)?;
    let report = checked_write_path(ctx, arg_str(args, "report")?)?;
    let outcome = hwp_cli::certification::execute(&input, &policy, &report)
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

/// 워크스페이스 파일 하나의 SHA-256 16진 표기.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// base64 문자열의 구조를 검사한다.
///
/// 공유 디코더는 `=`와 공백을 건너뛰고 남는 비트를 버리므로 `"A"`나 `"===="` 같은
/// 입력이 빈 벡터로 조용히 통과한다. 전송 도구에서 그것은 0바이트 파일을 업로드
/// 성공으로 보고한다는 뜻이라 여기서 먼저 막는다.
///
/// 잡아내지 못하는 것도 적어 둔다. 4문자 경계에 딱 맞게 잘린 페이로드는 인코딩만
/// 봐서는 온전한 것과 구분할 수 없다. 그래서 두 도구 모두 영수증에 sha256을 담는다.
fn validate_base64_structure(text: &str) -> Result<(), String> {
    let compact: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    // 빈 입력은 0바이트 파일을 업로드 성공으로 보고하게 만든다. 문서를 옮기는
    // 도구에서 그것은 거의 언제나 호출부의 인코딩이 조용히 실패한 흔적이고,
    // 실제로 라이브 검증 중에 그렇게 물렸다. 다음 도구에서 이해하기 어려운
    // 오류로 드러나느니 여기서 말하는 편이 낫다.
    if compact.is_empty() {
        return Err("content가 비어 있습니다".to_string());
    }
    let padding = compact.iter().rev().take_while(|&&b| b == b'=').count();
    if padding > 2 {
        return Err("잘못된 base64: 패딩이 2자를 넘습니다".to_string());
    }
    let payload = &compact[..compact.len() - padding];
    if payload.contains(&b'=') {
        return Err("잘못된 base64: 패딩이 끝이 아닌 위치에 있습니다".to_string());
    }
    if !compact.len().is_multiple_of(4) {
        return Err(format!(
            "잘못된 base64: 공백을 제외한 길이가 4의 배수가 아닙니다 ({}자)",
            compact.len()
        ));
    }
    // 길이 %4 == 1 은 어떤 바이트열도 만들어낼 수 없는 형태다.
    if payload.len() % 4 == 1 {
        return Err("잘못된 base64: 잘린 페이로드입니다".to_string());
    }
    Ok(())
}

/// 파일 경로를 `file://` URI로 만든다.
///
/// `format!("file://{}", path.display())`는 공백·`#`·비ASCII·윈도우 드라이브 경로에서
/// 유효한 URI가 아니다. MCP 클라이언트가 resource `uri`를 URI로 파싱하면 멀쩡한 blob을
/// 거절하거나 잘못 해석할 수 있다.
fn file_uri(path: &Path) -> String {
    const UNRESERVED_EXTRA: &[u8] = b"-._~/";
    let text = path.display().to_string().replace('\\', "/");
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || UNRESERVED_EXTRA.contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    // 윈도우의 `C:/...`처럼 슬래시로 시작하지 않는 경로는 빈 authority 뒤에 루트가 온다.
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// 확장자로 추정한 MIME 타입. 모르면 octet-stream이다.
fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("hwp") => "application/x-hwp",
        Some("hwpx") => "application/hwp+zip",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("md") => "text/markdown",
        Some("html") => "text/html",
        Some("csv") => "text/csv",
        Some("txt") => "text/plain",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        _ => "application/octet-stream",
    }
}

/// base64 콘텐츠를 워크스페이스 파일로 쓴다.
///
/// Tier B(AgentCore)는 `/mcp` 외의 경로를 제공하지 않으므로 문서가 JSON-RPC 안으로
/// 들어와야 한다. Tier A에도 필요한데, `/files`는 `Mcp-Session-Id`를 실은 별도 HTTP
/// 요청이라 MCP 클라이언트가 호출할 수 없기 때문이다. 이 도구가 생기기 전까지 원격
/// 서비스는 텍스트에서 문서를 **만들 수는** 있어도 기존 문서를 **받을 수는** 없었다.
///
/// 이름 검사는 `checked_write_path`에 맡긴다. `/files` 라우트의 `valid_file_name`을
/// 재사용하지 않는데, 그 정규식은 URL 경로 조각이 canonicalize 검사 없이 root에
/// 이어붙는 라우트에서 **그 자체가 봉쇄 수단**이라 존재한다. 여기서는
/// `checked_write_path`가 더 강한 검사를 하며, ASCII 전용 문자셋을 씌우면 다른 모든
/// 도구가 받는 한글 파일명을 이 도구만 거절하게 된다.
fn tool_put_file(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "name" | "content"))
    {
        return Err(format!("알 수 없는 hwp_put_file 인자: {unknown}"));
    }
    let path = checked_write_path(ctx, arg_str(args, "name")?)?;
    let content = arg_str(args, "content")?;
    if content.len() > MAX_INLINE_CONTENT_B64_CHARS {
        return Err(format!(
            "content가 너무 큽니다: base64 {}자 (상한 {}자, 복호 {} KiB). \
             더 작은 파일로 나누거나 Tier A의 /files 경로를 쓰세요.",
            content.len(),
            MAX_INLINE_CONTENT_B64_CHARS,
            MAX_INLINE_CONTENT_BYTES / 1024
        ));
    }
    validate_base64_structure(content)?;
    let bytes = hwp_convert::base64::decode(content)?;
    // decode는 공백과 패딩을 무시하므로 위 검사만으로는 복호 크기를 보장하지 못한다.
    if bytes.len() > MAX_INLINE_CONTENT_BYTES {
        return Err(format!(
            "content가 너무 큽니다: 복호 {}바이트 (상한 {}바이트)",
            bytes.len(),
            MAX_INLINE_CONTENT_BYTES
        ));
    }
    // 스테이징·fsync·재읽기 검증은 다른 모든 출력 경로와 같은 헬퍼가 처리한다.
    crate::commands::output::write_validated(
        &path,
        None,
        |staged| {
            std::fs::write(staged, &bytes)?;
            Ok(())
        },
        |staged, _| {
            let written = std::fs::read(staged)?;
            if written != bytes {
                anyhow::bail!("업로드 검증 중 바이트 불일치: {}", staged.display());
            }
            Ok(())
        },
    )
    .map_err(|error| format!("{error:#}"))?;

    let summary = serde_json::to_string_pretty(&json!({
        "path": path.display().to_string(),
        "bytes": bytes.len(),
        "sha256": sha256_hex(&bytes),
    }))
    .map_err(|error| error.to_string())?;
    Ok(vec![text_content(&summary)])
}

/// 워크스페이스 파일을 base64로 돌려준다.
///
/// 반환은 두 블록이다. 영수증 역할의 텍스트 블록은 알 수 없는 블록 타입을 버리는
/// 클라이언트에도 무엇이 생겼는지 남기고, embedded resource 블록이 실제 바이트를
/// 나른다. `tool_read_scoped`·`tool_render_scoped`가 이미 쓰는 두 블록 관례와 같다.
///
/// 상한을 넘으면 잘라내지 않고 거절한다. 반쪽 문서는 오류보다 나쁘고, 파일은 그대로
/// 남으므로 더 작은 포맷으로 변환하거나 Tier A의 `/files`로 받으면 된다.
fn tool_get_file(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "arguments는 객체여야 합니다".to_string())?;
    if let Some(unknown) = object.keys().find(|key| key.as_str() != "path") {
        return Err(format!("알 수 없는 hwp_get_file 인자: {unknown}"));
    }
    use std::io::Read as _;
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let file =
        std::fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    // 일반 파일만 받는다. 문자 장치는 metadata가 0바이트라고 보고하므로 크기 검사만
    // 믿으면 /dev/zero 같은 경로에서 읽기가 끝나지 않는다(root 제한이 없는 stdio에서).
    if !metadata.is_file() {
        return Err(format!("일반 파일이 아닙니다: {}", path.display()));
    }
    let too_big = format!(
        "파일이 인라인 상한을 넘습니다 (상한 {}바이트). \
         파일은 워크스페이스에 그대로 있으니 더 작은 포맷으로 변환하거나 \
         Tier A의 /files 경로로 받으세요.",
        MAX_INLINE_CONTENT_BYTES
    );
    if metadata.len() > MAX_INLINE_CONTENT_BYTES as u64 {
        return Err(too_big);
    }
    // 읽기 자체도 묶는다. metadata 이후에 파일이 자라도 상한을 넘길 수 없다.
    let mut bytes = Vec::new();
    file.take(MAX_INLINE_CONTENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > MAX_INLINE_CONTENT_BYTES {
        return Err(too_big);
    }
    let summary = serde_json::to_string_pretty(&json!({
        "path": path.display().to_string(),
        "bytes": bytes.len(),
        "sha256": sha256_hex(&bytes),
    }))
    .map_err(|error| error.to_string())?;
    Ok(vec![
        text_content(&summary),
        json!({
            "type": "resource",
            "resource": {
                "uri": file_uri(&path),
                "mimeType": mime_for(&path),
                "blob": hwp_convert::base64::encode(&bytes),
            }
        }),
    ])
}

/// `hwp merge` over MCP. Every input, the output and the optional loss report
/// are root-checked before any file is touched, and the single password applies
/// to every input — the same single-password-per-batch rule the CLI documents.
fn tool_merge_scoped(args: &mut Value, ctx: &dyn FileAuthority) -> McpToolResult {
    let inputs = arg_array(args, "inputs")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "inputs의 각 항목은 문자열이어야 합니다".to_string())
                .and_then(|raw| checked_read_path(ctx, raw))
        })
        .collect::<Result<Vec<PathBuf>, String>>()?;
    if inputs.len() < 2 {
        return Err("inputs에는 입력 경로가 2개 이상 필요합니다".into());
    }
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    let loss_report = arg_str_opt(args, "loss_report")?
        .map(|raw| checked_write_path(ctx, raw))
        .transpose()?;
    // Unlike hwp_convert, strict defaults to false here — the CLI default. A
    // merge inherently drops the package passthrough of every input after the
    // first, so a fail-closed default would refuse even a two-plain-document
    // merge. The preservation ledger is returned on every call instead, so the
    // caller judges the losses rather than never seeing the tool succeed.
    let strict = arg_bool(args, "strict", false)?;
    let password = take_scoped_password(
        args,
        &["inputs", "output", "loss_report", "strict", "password"],
    )?;
    let report = crate::commands::merge::execute(
        &inputs,
        &output,
        strict,
        loss_report.as_deref(),
        &LoadOptions {
            password: password.as_ref(),
        },
    )
    .map_err(|error| McpToolError::from(format!("{error:#}")))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "inputs": inputs,
            "output": output,
            "strict": strict,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

/// `hwp split` over MCP. `out_dir` is write-checked like any other output path,
/// so the published fragments land under a configured root by construction.
fn tool_split_scoped(args: &mut Value, ctx: &dyn FileAuthority) -> McpToolResult {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let out_dir = checked_write_path(ctx, arg_str(args, "out_dir")?)?;
    let pages = arg_array(args, "pages")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "pages의 각 항목은 문자열이어야 합니다".to_string())
        })
        .collect::<Result<Vec<String>, String>>()?;
    let loss_report = arg_str_opt(args, "loss_report")?
        .map(|raw| checked_write_path(ctx, raw))
        .transpose()?;
    // strict follows the CLI default (false) for the same reason hwp_merge does;
    // the preservation ledger comes back on every call.
    let strict = arg_bool(args, "strict", false)?;
    let password = take_scoped_password(
        args,
        &[
            "input",
            "out_dir",
            "pages",
            "loss_report",
            "strict",
            "password",
        ],
    )?;
    let summary = crate::commands::split::execute(
        &input,
        &out_dir,
        &pages,
        strict,
        loss_report.as_deref(),
        &LoadOptions {
            password: password.as_ref(),
        },
    )
    .map_err(|error| McpToolError::from(format!("{error:#}")))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": input,
            "out_dir": out_dir,
            "strict": strict,
            "fragments": summary.fragments,
            "warnings": summary.report.warnings,
            "preservation": summary.report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

/// `hwp compare` over MCP. Read-only: both inputs are read-checked and neither
/// is written. Differences are a normal result, never `isError` — the CLI's
/// diff(1) exit codes have no MCP equivalent, so the caller reads `identical`
/// instead. This matches how `hwp_grep` reports zero matches.
fn tool_compare_scoped(args: &mut Value, ctx: &dyn FileAuthority) -> McpToolResult {
    let a = checked_read_path(ctx, arg_str(args, "a")?)?;
    let b = checked_read_path(ctx, arg_str(args, "b")?)?;
    let password = take_scoped_password(args, &["a", "b", "password"])?;
    let report = crate::commands::compare::execute(
        &a,
        &b,
        &LoadOptions {
            password: password.as_ref(),
        },
    )
    .map_err(|error| McpToolError::from(format!("{error:#}")))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&report).unwrap_or_default(),
    )])
}

fn tool_render_scoped(args: &mut Value, ctx: &dyn FileAuthority) -> McpToolResult {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    if args.get("page").is_some() && args.get("pages").is_some() {
        return Err("page와 pages는 함께 지정할 수 없습니다".into());
    }
    let format = match arg_str_opt(args, "format")?.unwrap_or("png") {
        "png" => hwp_cli::cli::RenderFormat::Png,
        "svg" => hwp_cli::cli::RenderFormat::Svg,
        "pdf" => hwp_cli::cli::RenderFormat::Pdf,
        other => return Err(format!("알 수 없는 format: {other} (png|svg|pdf)").into()),
    };
    let page = usize::try_from(arg_u64(args, "page", 1)?)
        .map_err(|_| "page가 플랫폼 범위를 넘습니다".to_string())?;
    let dpi = crate::commands::render::validated_dpi(arg_f64(args, "dpi", 120.0)?)
        .map_err(|error| error.to_string())?;
    let output_path = arg_str_opt(args, "output_path")?.map(ToOwned::to_owned);
    let output = output_path
        .as_deref()
        .map(|raw| checked_write_path(ctx, raw))
        .transpose()?;
    let font_dirs = font_dirs_for(args, ctx)?;
    let password = take_scoped_password(
        args,
        &[
            "path",
            "page",
            "pages",
            "format",
            "output_path",
            "dpi",
            "font_dir",
            "password",
        ],
    )?;
    let doc = load_mcp_document(&path, password.as_ref())?;
    let opts = hwp_render::RenderOptions { dpi, font_dirs };
    let pages_spec = arg_str_opt(args, "pages")?;

    // base64 return path: png without output_path. Only a single-page selection is allowed.
    if matches!(format, hwp_cli::cli::RenderFormat::Png) && output_path.is_none() {
        let selected = match pages_spec {
            // Legacy contract: page keeps the same selection semantics as render_document_pages.
            None => vec![page],
            Some(spec) => {
                let total = hwp_render::count_pages(&doc, &opts);
                crate::commands::render::parse_pages(spec, total)
                    .map_err(|error| error.to_string())?
            }
        };
        if selected.len() != 1 {
            return Err(
                "다중 페이지 렌더는 output_path가 필요합니다 (페이지별 파일로 저장)".into(),
            );
        }
        let page = selected[0];
        let out = hwp_render::render_document_pages(&doc, &opts, Some(&[page]))
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
            )
            .into());
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
        return Ok(vec![text_content(&summary), image_content(&png)]);
    }

    // File-publish path: svg/pdf, multiple pages, or an explicit output_path. Goes through
    // the same atomic publish transaction as the CLI render and returns JSON metadata.
    let output = output
        .ok_or("svg/pdf 또는 output_path 없는 다중 페이지 렌더는 output_path 인자가 필요합니다")?;
    // Each path derived from the CLI's per-page filename rule is also sandbox-checked.
    let checked_derived = |base: &Path, page_no: usize, multi: bool| -> Result<PathBuf, String> {
        let derived = crate::commands::render::page_path(base, page_no, multi);
        let raw = derived
            .to_str()
            .ok_or_else(|| format!("출력 경로가 UTF-8이 아닙니다: {}", derived.display()))?;
        checked_write_path(ctx, raw)
    };
    let (files, selected): (Vec<PathBuf>, Vec<usize>) = match format {
        hwp_cli::cli::RenderFormat::Png => {
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = match pages_spec {
                Some(spec) => crate::commands::render::parse_pages(spec, total)
                    .map_err(|error| error.to_string())?,
                None => crate::commands::render::parse_pages(&page.to_string(), total)
                    .map_err(|error| error.to_string())?,
            };
            let result = hwp_render::render_document_pages(&doc, &opts, Some(&selected))
                .map_err(|e| e.to_string())?;
            let multi = selected.len() > 1;
            let mut outputs = Vec::with_capacity(selected.len());
            let mut files = Vec::with_capacity(selected.len());
            for (&page_no, pixmap) in selected.iter().zip(&result.pages) {
                let derived = checked_derived(&output, page_no, multi)?;
                let png = pixmap
                    .encode_png()
                    .map_err(|error| format!("PNG 인코딩 실패 ({}): {error}", derived.display()))?;
                files.push(derived.clone());
                outputs.push((derived, png));
            }
            crate::commands::render::publish_render_set(&outputs, &path)
                .map_err(|error| error.to_string())?;
            (files, selected)
        }
        hwp_cli::cli::RenderFormat::Svg => {
            let result = hwp_render::render_document_svg(&doc, &opts);
            let selected = match pages_spec {
                Some(spec) => crate::commands::render::parse_pages(spec, result.pages.len())
                    .map_err(|error| error.to_string())?,
                None => crate::commands::render::parse_pages(&page.to_string(), result.pages.len())
                    .map_err(|error| error.to_string())?,
            };
            let multi = selected.len() > 1;
            let mut outputs = Vec::with_capacity(selected.len());
            let mut files = Vec::with_capacity(selected.len());
            for &page_no in &selected {
                let derived = checked_derived(&output, page_no, multi)?;
                files.push(derived.clone());
                outputs.push((derived, result.pages[page_no - 1].as_bytes().to_vec()));
            }
            crate::commands::render::publish_render_set(&outputs, &path)
                .map_err(|error| error.to_string())?;
            (files, selected)
        }
        hwp_cli::cli::RenderFormat::Pdf => {
            let total = hwp_render::count_pages(&doc, &opts);
            let selected = match pages_spec {
                Some(spec) => crate::commands::render::parse_pages(spec, total)
                    .map_err(|error| error.to_string())?,
                None => crate::commands::render::parse_pages(&page.to_string(), total)
                    .map_err(|error| error.to_string())?,
            };
            // Unlike PNG/SVG, PDF is a single multi-page file (no per-page split).
            let result = hwp_render::render_document_pdf(&doc, &opts, Some(&selected))
                .map_err(|e| e.to_string())?;
            crate::commands::render::write_render_bytes(&output, &path, &result.data)
                .map_err(|error| error.to_string())?;
            (vec![output.clone()], selected)
        }
    };
    let metadata = json!({
        "files": files,
        "pages": selected,
        "dpi": dpi,
    });
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&metadata).unwrap_or_default(),
    )])
}

/// hwp_grep match return cap — excess matches are cut and marked truncated=true.
/// count is always the full pre-truncation total.
const MAX_GREP_MATCHES: usize = 200;

fn grep_result(matches: Vec<String>) -> Value {
    grep_result_capped(matches, MAX_GREP_MATCHES)
}

fn grep_result_capped(matches: Vec<String>, cap: usize) -> Value {
    let count = matches.len();
    let truncated = count > cap;
    let matches: Vec<String> = matches.into_iter().take(cap).collect();
    json!({
        "matches": matches,
        "count": count,
        "truncated": truncated,
    })
}

fn tool_grep(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let path = checked_read_path(ctx, arg_str(args, "path")?)?;
    let pattern = arg_str(args, "pattern")?;
    let ignore_case = arg_bool(args, "ignore_case", false)?;
    let doc = load_document(&path).map_err(|e| e.to_string())?;
    // Zero matches are a normal result, not an error (unlike the CLI grep exit(1) contract).
    let matches =
        crate::commands::grep::search(&doc, pattern, ignore_case).map_err(|e| e.to_string())?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&grep_result(matches)).unwrap_or_default(),
    )])
}

fn tool_edit(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
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
    for item in arg_array(args, "set_cell_by_label")? {
        operations.push(Op::SetCellByLabel {
            label: required_item_str(item, "set_cell_by_label", "label")?.to_string(),
            text: required_item_str(item, "set_cell_by_label", "text")?.to_string(),
            table: optional_item_usize(item, "set_cell_by_label", "table")?,
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
            path: checked_read_path(ctx, required_item_str(item, "insert_image", "path")?)?,
            size_mm,
        });
    }
    for item in arg_array(args, "seal")? {
        let size_mm = optional_item_f32(item, "seal", "size_mm")?;
        operations.push(Op::Seal {
            anchor: required_item_str(item, "seal", "anchor")?.to_string(),
            path: checked_read_path(ctx, required_item_str(item, "seal", "path")?)?,
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
        let count = optional_item_u16(item, "add_row", "count")?;
        if count == Some(0) {
            return Err("add_row: count는 1 이상이어야 합니다".to_string());
        }
        operations.push(Op::AddRow {
            table: required_item_usize(item, "add_row", "table")?,
            at: optional_item_u16(item, "add_row", "at")?,
            count: count.map(usize::from).unwrap_or(1),
            template_row: optional_item_u16(item, "add_row", "template_row")?,
        });
    }
    for item in arg_array(args, "add_col")? {
        let count = optional_item_u16(item, "add_col", "count")?;
        if count == Some(0) {
            return Err("add_col: count는 1 이상이어야 합니다".to_string());
        }
        operations.push(Op::AddCol {
            table: required_item_usize(item, "add_col", "table")?,
            at: optional_item_u16(item, "add_col", "at")?,
            count: count.unwrap_or(1),
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
    for item in arg_array(args, "add_table")? {
        // The JSON boundary validates only the shape (an array of string arrays) — content
        // problems like empty row data are rejected by the library (add_table), whose error propagates as-is.
        let rows_value = item
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| "add_table 항목에 rows 필요".to_string())?;
        let mut rows = Vec::with_capacity(rows_value.len());
        for row in rows_value {
            let cells = row
                .as_array()
                .ok_or_else(|| "add_table.rows는 문자열 배열의 배열이어야 합니다".to_string())?;
            let mut parsed_row = Vec::with_capacity(cells.len());
            for cell in cells {
                parsed_row.push(
                    cell.as_str()
                        .ok_or_else(|| "add_table.rows의 셀은 문자열이어야 합니다".to_string())?
                        .to_string(),
                );
            }
            rows.push(parsed_row);
        }
        operations.push(Op::AddTable {
            anchor: required_item_str(item, "add_table", "anchor")?.to_string(),
            rows,
        });
    }
    for item in arg_array(args, "clone_table")? {
        let text_mode = match item.get("text_mode").and_then(Value::as_str).map(str::trim) {
            None | Some("") | Some("blank") => hwp_convert::CloneTextMode::Blank,
            Some("keep") => hwp_convert::CloneTextMode::Keep,
            Some(_) => {
                return Err("clone_table: text_mode는 blank|keep 이어야 합니다".to_string());
            }
        };
        operations.push(Op::CloneTable {
            source_table: required_item_usize(item, "clone_table", "source_table")?,
            anchor: required_item_str(item, "clone_table", "anchor")?.to_string(),
            text_mode,
        });
    }
    for item in arg_array(args, "set_para")? {
        operations.push(Op::SetPara {
            pattern: required_item_str(item, "set_para", "pattern")?.to_string(),
            props: para_props_item(item, "set_para")?,
        });
    }
    for item in arg_array(args, "set_cell_para")? {
        operations.push(Op::SetCellPara {
            table: required_item_usize(item, "set_cell_para", "table")?,
            row: required_item_u16(item, "set_cell_para", "row")?,
            col: required_item_u16(item, "set_cell_para", "col")?,
            props: para_props_item(item, "set_cell_para")?,
        });
    }
    // Like the CLI's cumulative --set-page flags, a single object is merged into one PageProps and applied.
    if let Some(item) = args.get("set_page") {
        if !item.is_object() {
            return Err("set_page는 단일 객체여야 합니다".into());
        }
        let props = hwp_convert::PageProps {
            width: optional_item_f32(item, "set_page", "width_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            height: optional_item_f32(item, "set_page", "height_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_left: optional_item_f32(item, "set_page", "margin_left_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_right: optional_item_f32(item, "set_page", "margin_right_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_top: optional_item_f32(item, "set_page", "margin_top_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            margin_bottom: optional_item_f32(item, "set_page", "margin_bottom_mm")?
                .map(crate::commands::edit::mm_to_hwpunit),
            landscape: optional_item_str(item, "set_page", "orientation")?
                .map(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "landscape" | "가로" => Ok(true),
                    "portrait" | "세로" => Ok(false),
                    other => Err(format!(
                        "알 수 없는 용지 방향: {other:?} (portrait/landscape)"
                    )),
                })
                .transpose()?,
        };
        operations.push(Op::SetPage { props });
    }
    for item in arg_array(args, "delete_image")? {
        operations.push(Op::DeleteImage {
            anchor: required_item_str(item, "delete_image", "anchor")?.to_string(),
        });
    }
    for item in arg_array(args, "delete_table")? {
        let index = optional_item_usize(item, "delete_table", "index")?;
        let anchor = optional_item_str(item, "delete_table", "anchor")?.map(str::to_string);
        match (&index, &anchor) {
            (Some(_), Some(_)) => {
                return Err("delete_table 항목은 index와 anchor 중 하나만 지정해야 합니다".into());
            }
            (None, None) => {
                return Err("delete_table 항목에 index 또는 anchor가 필요합니다".into());
            }
            _ => {}
        }
        operations.push(Op::DeleteTable { index, anchor });
    }
    for item in arg_array(args, "delete_field")? {
        operations.push(Op::DeleteField {
            name: required_item_str(item, "delete_field", "name")?.to_string(),
        });
    }
    for item in arg_array(args, "delete_bookmark")? {
        operations.push(Op::DeleteBookmark {
            name: required_item_str(item, "delete_bookmark", "name")?.to_string(),
        });
    }
    // A single preset name, parsed through the same `OfficialPreset::parse` the CLI
    // `--style-tables` flag uses (D-07/D-09). Stays silent on stdout like every other preset
    // path here — stdout is the protocol channel.
    if let Some(preset) = arg_str_opt(args, "style_tables")? {
        operations.push(Op::StyleTables {
            preset: hwp_convert::OfficialPreset::parse(preset)?,
        });
    }

    let plan = crate::commands::edit::EditPlan::from_typed(
        operations,
        true,
        arg_bool(args, "allow_partial", false)?,
    );
    let report = crate::commands::edit::execute(&input, &output, &plan)
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": input,
            "output": report.output,
            "applied": report.applied,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

/// Maps hwp_convert arguments to convert::execute parameters (a test seam verified without rendering).
#[derive(Debug)]
struct ConvertRequest {
    input: PathBuf,
    output: PathBuf,
    to: Option<hwp_cli::cli::ConvertFormat>,
    strict: bool,
    embed_bin: bool,
    media_dir: Option<PathBuf>,
    with_header_footer: bool,
    with_hidden: bool,
    font_dirs: Vec<PathBuf>,
}

fn parse_convert_format(value: &str) -> Result<hwp_cli::cli::ConvertFormat, String> {
    use hwp_cli::cli::ConvertFormat as F;
    match value {
        "hwp" => Ok(F::Hwp),
        "hwpx" => Ok(F::Hwpx),
        "md" | "markdown" => Ok(F::Md),
        "json" => Ok(F::Json),
        "html" => Ok(F::Html),
        "pdf" => Ok(F::Pdf),
        "odt" => Ok(F::Odt),
        "txt" => Ok(F::Txt),
        "csv" => Ok(F::Csv),
        "docx" => Ok(F::Docx),
        other => Err(format!(
            "알 수 없는 to: {other} (hwp|hwpx|md|json|html|pdf|odt|txt|csv|docx)"
        )),
    }
}

fn convert_request(args: &Value, ctx: &dyn FileAuthority) -> Result<ConvertRequest, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    // An explicit to wins over output-extension inference, like the CLI --to.
    let to = arg_str_opt(args, "to")?
        .map(parse_convert_format)
        .transpose()?;
    // media_dir is the markdown image-extraction directory — checked as a write path.
    let media_dir = arg_str_opt(args, "media_dir")?
        .map(|raw| checked_write_path(ctx, raw))
        .transpose()?;
    Ok(ConvertRequest {
        input,
        output,
        to,
        strict: arg_bool(args, "strict", true)?,
        embed_bin: arg_bool(args, "embed_bin", false)?,
        media_dir,
        with_header_footer: arg_bool(args, "with_header_footer", false)?,
        with_hidden: arg_bool(args, "with_hidden", false)?,
        // Passes the merged startup --font-dir + per-call font_dir list through — previously
        // the list was always empty, so fonts set at MCP server startup never applied to PDF conversion.
        font_dirs: font_dirs_for(args, ctx)?,
    })
}

fn tool_convert_scoped(args: &mut Value, ctx: &dyn FileAuthority) -> McpToolResult {
    let request = convert_request(args, ctx)?;
    let password = take_scoped_password(
        args,
        &[
            "input",
            "output",
            "to",
            "media_dir",
            "with_header_footer",
            "with_hidden",
            "font_dir",
            "embed_bin",
            "strict",
            "password",
        ],
    )?;
    let md_options = crate::commands::convert::MdOpts {
        media_dir: request.media_dir.as_deref(),
        with_header_footer: request.with_header_footer,
        with_hidden: request.with_hidden,
    };
    let report = if password.is_some() {
        crate::commands::convert::execute_with_options(
            &request.input,
            &request.output,
            request.to,
            request.strict,
            None,
            false,
            request.embed_bin,
            &md_options,
            request.font_dirs,
            &LoadOptions {
                password: password.as_ref(),
            },
        )
    } else {
        crate::commands::convert::execute(
            &request.input,
            &request.output,
            request.to,
            request.strict,
            None,
            false,
            request.embed_bin,
            &md_options,
            request.font_dirs,
        )
    }
    .map_err(|error| map_mcp_convert_error(&request.input, error))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "input": request.input,
            "output": request.output,
            "strict": request.strict,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

// Unit-test adapter retains the historical borrowed helper. The JSON-RPC path
// exclusively calls the scoped variant and moves `password` out of its owned
// request value before any document load.
#[cfg(test)]
fn tool_read(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let mut args = args.clone();
    tool_read_scoped(&mut args, ctx).map_err(|error| error.message)
}

#[cfg(test)]
fn tool_render(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let mut args = args.clone();
    tool_render_scoped(&mut args, ctx).map_err(|error| error.message)
}

#[cfg(test)]
fn tool_convert(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let mut args = args.clone();
    tool_convert_scoped(&mut args, ctx).map_err(|error| error.message)
}

fn tool_new(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let object = args
        .as_object()
        .ok_or_else(|| "hwp_new 인자는 객체여야 합니다".to_string())?;
    const ALLOWED: &[&str] = &[
        "output",
        "markdown",
        "json",
        "set_meta",
        "preset",
        "margin_top_mm",
        "margin_bottom_mm",
        "margin_left_mm",
        "margin_right_mm",
        "doc_head",
        "doc_foot",
        "notice_head",
        "notice_foot",
        "press_head",
        "template",
    ];
    if let Some(unknown) = object.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(format!("알 수 없는 hwp_new 인자: {unknown}"));
    }
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
    let markdown = arg_str_opt(args, "markdown")?;
    let json_input = arg_str_opt(args, "json")?;
    if markdown.is_some() && json_input.is_some() {
        return Err("markdown과 json은 동시에 지정할 수 없습니다".into());
    }

    // Frame arguments arrive as arrays of {key, value} objects (a structured boundary does not
    // re-encode into the CLI's "k=v" mini-language). Reassemble "k=v" here so the SAME
    // `parse_frame_fields` validator the CLI uses produces identical Korean errors (D-01, D-09).
    let frame_field_specs = |frame: &str| -> Result<Vec<String>, String> {
        arg_array(args, frame)?
            .iter()
            .map(|item| {
                Ok(format!(
                    "{}={}",
                    required_item_str(item, frame, "key")?,
                    required_item_str(item, frame, "value")?
                ))
            })
            .collect()
    };
    let doc_head = frame_field_specs("doc_head")?;
    let doc_foot = frame_field_specs("doc_foot")?;
    let notice_head = frame_field_specs("notice_head")?;
    let notice_foot = frame_field_specs("notice_foot")?;
    let press_head = frame_field_specs("press_head")?;

    // `template` is refused together with `markdown`/`json` (both are the "document content"
    // argument, same as `--template`/`--from` on the CLI). Frame arguments do combine with it:
    // the skeleton carries no native 두문/결문 table, so the frame arguments supply them (the
    // D-05 exclusion was reverted once verification falsified its premise). Resolved through the
    // same fixed in-binary lookup `--list-templates` reads (T-02.4-13) — never a filesystem path
    // built from the name.
    let template = arg_str_opt(args, "template")?;
    let embedded_text = template
        .map(|name| -> Result<&'static str, String> {
            if markdown.is_some() {
                return Err(
                    "template과 markdown은 함께 지정할 수 없습니다: 템플릿은 이미 markdown 본문을 \
                     포함합니다"
                        .into(),
                );
            }
            if json_input.is_some() {
                return Err(
                    "template과 json은 함께 지정할 수 없습니다: 템플릿은 이미 markdown 본문을 \
                     포함합니다"
                        .into(),
                );
            }
            crate::commands::skill::template_file(name)
                .map(|file| file.contents)
                .ok_or_else(|| {
                    let accepted: Vec<&str> = crate::commands::skill::template_names()
                        .map(|(slug, _)| slug)
                        .collect();
                    format!(
                        "알 수 없는 template 이름: {name} (사용 가능: {})",
                        accepted.join(", ")
                    )
                })
        })
        .transpose()?;

    let input = match (embedded_text, markdown, json_input) {
        (Some(text), _, _) => crate::commands::new::NewInput::Markdown {
            text,
            base_dir: None,
            roots: ctx.roots(),
        },
        (None, Some(markdown), None) => crate::commands::new::NewInput::Markdown {
            text: markdown,
            base_dir: None,
            // Bind image references inside the markdown to the sandbox roots (#56).
            roots: ctx.roots(),
        },
        (None, None, Some(document_json)) => crate::commands::new::NewInput::Json(document_json),
        (None, None, None) => crate::commands::new::NewInput::Empty,
        // Ruled out above: markdown and json together return before this match is reached.
        (None, Some(_), Some(_)) => unreachable!("markdown/json exclusivity checked above"),
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
    let preset = arg_str_opt(args, "preset")?
        .map(hwp_convert::OfficialPreset::parse)
        .transpose()?;
    // A template names the profile it was written for and the frames it needs; both are defaults
    // an explicit argument overrides, exactly as on the CLI (D-01: one behavior, two surfaces).
    let template_defaults = template.and_then(crate::commands::skill::template_defaults);
    let options = crate::commands::new::NewOptions::from_millimetres(
        preset.or(template_defaults.as_ref().map(|d| d.preset)),
        arg_f64_opt(args, "margin_top_mm")?,
        arg_f64_opt(args, "margin_bottom_mm")?,
        arg_f64_opt(args, "margin_left_mm")?,
        arg_f64_opt(args, "margin_right_mm")?,
        false,
    )
    .map_err(|error| format!("{error:#}"))?
    .with_frames(
        &doc_head,
        &doc_foot,
        &notice_head,
        &notice_foot,
        &press_head,
    )
    .map_err(|error| format!("{error:#}"))?
    .with_template_frames(template_defaults.as_ref());
    let report = crate::commands::new::execute(&output, input, &metadata, &options)
        .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&json!({
            "output": report.output,
            "warnings": report.warnings,
            "preservation": report.preservation,
        }))
        .unwrap_or_default(),
    )])
}

fn tool_compose(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
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

    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
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
                let base_dir = match arg_str_opt(args, "base_dir")? {
                    Some(raw) => checked_read_path(ctx, raw)?,
                    None => checked_read_path(ctx, ".")?,
                };
                (input, format, base_dir, None)
            }
            (None, Some(spec_path)) => {
                if args.get("base_dir").is_some() {
                    return Err("spec_path 사용 시 base_dir는 지정할 수 없습니다".into());
                }
                let path = checked_read_path(ctx, spec_path)?;
                let input = crate::commands::compose::read_bounded(&path)
                    .map_err(|error| format!("{error:#}"))?;
                let format = explicit_format
                    .map(Ok)
                    .unwrap_or_else(|| hwp_cli::document_spec::infer_input_format(&path))
                    .map_err(|error| error.to_string())?;
                let base_dir = path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
                (input, format, base_dir, Some(path))
            }
        };
    let report = crate::commands::compose::execute_text_with_source(
        &input,
        format,
        &base_dir,
        &output,
        arg_bool(args, "dry_run", false)?,
        arg_bool(args, "allow_visual_fallback", false)?,
        source_path.as_deref(),
        ctx.roots(),
    )
    .map_err(|error| format!("{error:#}"))?;
    Ok(vec![text_content(
        &serde_json::to_string_pretty(&report).unwrap_or_default(),
    )])
}

fn tool_template(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
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
    let output = checked_write_path(ctx, arg_str(args, "output")?)?;
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
                let base = match arg_str_opt(args, "base_dir")? {
                    Some(raw) => checked_read_path(ctx, raw)?,
                    None => checked_read_path(ctx, ".")?,
                };
                (input, format, base)
            }
            (None, Some(path)) => {
                if args.get("base_dir").is_some() {
                    return Err("template_path 사용 시 base_dir는 지정할 수 없습니다".into());
                }
                let path = checked_read_path(ctx, path)?;
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
            let path = checked_read_path(ctx, path)?;
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
        &output,
        arg_bool(args, "dry_run", false)?,
        &source_paths,
        ctx.roots(),
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

fn tool_diff(args: &Value, ctx: &dyn FileAuthority) -> Result<Vec<Value>, String> {
    let input = checked_read_path(ctx, arg_str(args, "input")?)?;
    let reference = checked_read_path(ctx, arg_str(args, "ref")?)?;
    let page = usize::try_from(arg_u64(args, "page", 1)?)
        .map_err(|_| "page가 플랫폼 범위를 넘습니다".to_string())?;
    let dpi = crate::commands::render::validated_dpi(arg_f64(args, "dpi", 120.0)?)
        .map_err(|error| error.to_string())?;
    let doc = load_document(&input).map_err(|e| e.to_string())?;
    let out = hwp_render::render_document_pages(
        &doc,
        &hwp_render::RenderOptions {
            dpi,
            font_dirs: font_dirs_for(args, ctx)?,
        },
        Some(&[page]),
    )
    .map_err(|e| e.to_string())?;
    let refpx = hwp_render::load_png(&reference).map_err(|e| e.to_string())?;
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
            "description": "본문을 추출한다. format=json이면 전체 IR(구조)을, markdown/plain이면 텍스트를, html/csv면 해당 직렬화를 반환. with_header_footer/with_hidden은 plain/markdown에만 적용(cat과 동일). with_segments는 markdown 전용으로 {markdown, segments} JSON 봉투를 반환(오프셋은 전체 기준 절대 문자 위치).",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "path": {"type": "string"},
                "format": {"type": "string", "enum": ["plain", "markdown", "json", "html", "csv"], "description": "기본 plain"},
                "with_header_footer": {"type": "boolean", "description": "머리말/꼬리말 포함(plain/markdown, 기본 false)"},
                "with_hidden": {"type": "boolean", "description": "숨은 설명 포함(plain/markdown, 기본 false)"},
                "with_segments": {"type": "boolean", "description": "markdown 전용. 문단 원본 좌표 세그먼트 맵 포함"},
                "offset": {"type": "integer", "minimum": 0, "description": "UTF-8 byte offset, 기본 0"},
                "max_bytes": {"type": "integer", "minimum": 1, "maximum": 1048576, "description": "반환 byte 상한, 기본 262144"},
                "password": {"type": "string", "description": "이번 hwp_read 호출에만 사용할 문서 암호"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_grep",
            "description": "문단 텍스트 검색(본문·표 셀·글상자 재귀). {matches, count, truncated} 반환 — matches는 최대 200건, count는 전체 매칭 수, 0건 매칭도 정상 결과.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "pattern": {"type": "string", "description": "검색할 부분 문자열"},
                "ignore_case": {"type": "boolean", "description": "대소문자 무시, 기본 false"}
            }, "required": ["path", "pattern"]}
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
            "description": "페이지를 렌더한다. 기본은 단일 페이지 PNG를 base64 이미지로 반환(에이전트가 문서를 직접 본다). format=svg/pdf 또는 다중 페이지 선택(pages)은 output_path가 필요하고, 파일로 저장 뒤 {files, pages, dpi} JSON을 반환한다(16MiB 응답 상한 우회). 페이지별 파일명은 CLI와 같이 <stem>-<N>.<ext>(단일 페이지면 경로 그대로), pdf는 단일 멀티페이지 파일.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "path": {"type": "string"},
                "page": {"type": "integer", "description": "1-기반, 기본 1. pages와 함께 지정 불가"},
                "pages": {"type": "string", "description": "페이지 범위 spec: \"1\", \"1-3\", \"all\". page와 함께 지정 불가"},
                "format": {"type": "string", "enum": ["png", "svg", "pdf"], "description": "기본 png"},
                "output_path": {"type": "string", "description": "출력 파일 경로. svg/pdf·다중 페이지 필수. png 다중 페이지는 페이지별 <stem>-<N>.png"},
                "dpi": {"type": "number", "minimum": hwp_render::MIN_DPI, "maximum": hwp_render::MAX_DPI, "description": "기본 120"},
                "font_dir": {"type": "string", "description": "추가 폰트 디렉터리(선택)"},
                "password": {"type": "string", "description": "이번 hwp_render 호출에만 사용할 문서 암호"}
            }, "required": ["path"]}
        }),
        json!({
            "name": "hwp_edit",
            "description": "CLI와 같은 strict·atomic·재읽기 검증 경로로 기존 문서를 편집한다. 기본은 미적용 요청 하나라도 있으면 실패.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "input": {"type": "string"},
                "output": {"type": "string"},
                "replace": {"type": "array", "items": {"type": "object", "properties": {
                    "from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]},
                    "description": "텍스트 치환(모든 일치)"},
                "set_cell": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"}, "row": {"type": "integer"},
                    "col": {"type": "integer"}, "text": {"type": "string"}},
                    "required": ["table", "row", "col", "text"]}, "description": "표 셀 설정(0-기반)"},
                "set_cell_by_label": {"type": "array", "items": {"type": "object", "additionalProperties": false, "properties": {
                    "label": {"type": "string"}, "text": {"type": "string"},
                    "table": {"type": "integer", "minimum": 0}},
                    "required": ["label", "text"]}, "description": "양식 레이블의 인접 또는 첫 데이터 행 셀 설정(선택 table은 0-기반 재귀 표 범위)"},
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
                    "table": {"type": "integer"},
                    "at": {"type": "integer", "minimum": 0, "maximum": 65535, "description": "삽입 경계(생략 시 끝, 0-기반)"},
                    "count": {"type": "integer", "minimum": 1, "maximum": 65535},
                    "template_row": {"type": "integer", "minimum": 0, "maximum": 65535, "description": "행 높이·셀 서식 기증 행(텍스트는 복제 안 함)"}},
                    "required": ["table"]}, "description": "N번째 표의 at 경계 앞에 빈 행 count개 삽입(생략 시 끝에 1개, 0-기반, 병합 표도 지원)"},
                "add_col": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer"},
                    "at": {"type": "integer", "minimum": 0, "maximum": 65535},
                    "count": {"type": "integer", "minimum": 1, "maximum": 65535}},
                    "required": ["table"]}, "description": "N번째 표의 at 위치(생략 시 끝)에 열 count개 추가(0-기반, 전체 폭 유지, 병합 표도 지원)"},
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
                "add_table": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"},
                    "rows": {"type": "array", "items": {"type": "array", "items": {"type": "string"}}}},
                    "required": ["anchor", "rows"]},
                    "description": "앵커 문단 뒤에 균일 표 삽입(rows는 행(문자열 배열)의 배열)"},
                "clone_table": {"type": "array", "items": {"type": "object", "properties": {
                    "source_table": {"type": "integer", "minimum": 0, "description": "원본 표(0-기반, 재귀 순서)"},
                    "anchor": {"type": "string", "description": "복제본을 삽입할 앵커 문단 텍스트(그 뒤에 삽입)"},
                    "text_mode": {"type": "string", "enum": ["blank", "keep"],
                        "description": "blank(기본)=구조·서식만 복제하고 셀 내용은 빈 문단 1개, keep=지원 콘텐츠(중첩 표·그림)까지 복제(id 재부여, 안전하게 재매핑할 수 없는 개체가 있으면 실패)"}},
                    "required": ["source_table", "anchor"]},
                    "description": "N번째 표를 깊은 복제해 앵커 문단 뒤에 삽입(구조·병합·테두리·채우기·서식 보존)"},
                "set_para": {"type": "array", "items": {"type": "object", "properties": {
                    "pattern": {"type": "string"},
                    "line_spacing_pct": {"type": "number", "description": "줄간격 비율(%)"},
                    "line_spacing_pt": {"type": "number", "description": "고정 줄간격(pt) — pct와 함께 지정 불가"},
                    "indent_mm": {"type": "number"}, "left_mm": {"type": "number"},
                    "right_mm": {"type": "number"}, "top_mm": {"type": "number"},
                    "bottom_mm": {"type": "number"},
                    "align": {"type": "string", "enum": ["left", "right", "center", "justify", "distribute"],
                        "description": "문단 정렬"}},
                    "required": ["pattern"]},
                    "description": "문단모양(매칭 문단): 줄간격(비율% 또는 고정pt)·들여쓰기·여백(mm)·정렬"},
                "set_cell_para": {"type": "array", "items": {"type": "object", "properties": {
                    "table": {"type": "integer", "minimum": 0, "description": "0-기반 표 인덱스(재귀 순서)"},
                    "row": {"type": "integer", "minimum": 0}, "col": {"type": "integer", "minimum": 0},
                    "line_spacing_pct": {"type": "number", "description": "줄간격 비율(%)"},
                    "line_spacing_pt": {"type": "number", "description": "고정 줄간격(pt) — pct와 함께 지정 불가"},
                    "indent_mm": {"type": "number"}, "left_mm": {"type": "number"},
                    "right_mm": {"type": "number"}, "top_mm": {"type": "number"},
                    "bottom_mm": {"type": "number"},
                    "align": {"type": "string", "enum": ["left", "right", "center", "justify", "distribute"],
                        "description": "문단 정렬"}},
                    "required": ["table", "row", "col"]},
                    "description": "셀 문단모양(앵커 없이 그 셀의 모든 문단): 줄간격·들여쓰기·여백(mm)·정렬. 한 번의 실행에서 set_cell 뒤에 적용된다"},
                "set_page": {"type": "object", "properties": {
                    "width_mm": {"type": "number"}, "height_mm": {"type": "number"},
                    "margin_left_mm": {"type": "number"}, "margin_right_mm": {"type": "number"},
                    "margin_top_mm": {"type": "number"}, "margin_bottom_mm": {"type": "number"},
                    "orientation": {"type": "string", "enum": ["portrait", "landscape", "가로", "세로"]}},
                    "description": "페이지 설정(모든 구역 정의에 적용): 용지 크기·여백(mm)·방향"},
                "delete_image": {"type": "array", "items": {"type": "object", "properties": {
                    "anchor": {"type": "string"}}, "required": ["anchor"]},
                    "description": "앵커 문단의 그림 삭제"},
                "delete_table": {"type": "array", "items": {"type": "object", "properties": {
                    "index": {"type": "integer", "description": "0-기반 표 인덱스"},
                    "anchor": {"type": "string", "description": "앵커 텍스트가 든 문단의 표"}}},
                    "description": "표 삭제 — index와 anchor 중 정확히 하나"},
                "delete_field": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}}, "required": ["name"]},
                    "description": "이름으로 필드 삭제(hwp_list_fields로 이름 확인)"},
                "delete_bookmark": {"type": "array", "items": {"type": "object", "properties": {
                    "name": {"type": "string"}}, "required": ["name"]},
                    "description": "이름으로 책갈피 삭제(hwp_list_bookmarks로 이름 확인)"},
                "style_tables": {"type": "string", "description": "official/report/plan/notice/minutes/press 또는 지원 별칭 — 문서의 모든 표에 헤더 행 강조·내용 비례 열너비 스타일 적용(D-07/D-08)"},
                "allow_partial": {"type": "boolean", "description": "true면 일치한 요청만 게시; 기본 false"}
            }, "required": ["input", "output"]}
        }),
        json!({
            "name": "hwp_convert",
            "description": "포맷 변환. 기본은 출력 확장자(.hwp/.hwpx/.json/.md/.html/.pdf/.odt/.txt/.csv/.docx)로 결정하고 to가 있으면 CLI --to처럼 확장자보다 우선한다. pdf는 텍스트 선택가능 벡터(이미지 포함). embed_bin이면 JSON에 이미지 base64 임베드. media_dir/with_header_footer/with_hidden은 markdown 출력 전용.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "input": {"type": "string"}, "output": {"type": "string"},
                "to": {"type": "string", "enum": ["hwp", "hwpx", "md", "json", "html", "pdf", "odt", "txt", "csv", "docx"], "description": "대상 포맷(선택). 지정 시 출력 확장자 추론보다 우선"},
                "media_dir": {"type": "string", "description": "markdown 이미지 추출 디렉터리(선택, 기본 \"<출력스템>.media\")"},
                "with_header_footer": {"type": "boolean", "description": "markdown에 머리말/꼬리말 포함, 기본 false"},
                "with_hidden": {"type": "boolean", "description": "markdown에 숨은 설명 포함, 기본 false"},
                "font_dir": {"type": "string", "description": "추가 폰트 디렉터리(선택) — pdf 렌더에 적용"},
                "embed_bin": {"type": "boolean"},
                "strict": {"type": "boolean", "description": "HWP/HWPX DROP 경고를 실패 처리; MCP 기본 true"},
                "password": {"type": "string", "description": "이번 hwp_convert 호출에만 사용할 문서 암호"}
            }, "required": ["input", "output"]}
        }),
        json!({
            "name": "hwp_new",
            "description": "CLI와 같은 strict·atomic·재읽기 검증 경로로 .hwp/.hwpx 새 문서를 생성.",
            "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
                "output": {"type": "string"},
                "markdown": {"type": "string", "description": "markdown 본문(선택)"},
                "json": {"type": "string", "description": "IR JSON 본문(선택)"},
                "set_meta": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}},
                "preset": {"type": "string", "description": "official/report/plan/notice/minutes/press 또는 지원 별칭"},
                "margin_top_mm": {"type": "number", "minimum": 0, "maximum": 200},
                "margin_bottom_mm": {"type": "number", "minimum": 0, "maximum": 200},
                "margin_left_mm": {"type": "number", "minimum": 0, "maximum": 200},
                "margin_right_mm": {"type": "number", "minimum": 0, "maximum": 200},
                "doc_head": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "두문(기관명|수신|경유); markdown·template과 함께 사용 가능"},
                "doc_foot": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "결문(발신명의|기안자|검토자|결재자|협조자|시행번호|시행일자|접수번호|접수일자|주소|홈페이지|전화|팩스|이메일|공개구분|수신자); 수신자는 두문이 \"수신자 참조\"인 다수 수신 문서의 수신처 목록으로 지정할 때만 결문에 나옴; markdown·template과 함께 사용 가능"},
                "notice_head": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "공고문 머리(기관명|공고번호); markdown·template과 함께 사용 가능"},
                "notice_foot": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "공고문 꼬리(공고일자|발신명의); markdown·template과 함께 사용 가능"},
                "press_head": {"type": "array", "items": {"type": "object", "properties": {
                    "key": {"type": "string"}, "value": {"type": "string"}},
                    "required": ["key", "value"]}, "description": "보도자료 머리(기관명|보도시점|배포일|담당부서|담당자|연락처); markdown·template과 함께 사용 가능"},
                "template": {"type": "string", "description": "내장 문서 템플릿 영문 slug 또는 한국어 별칭(hwp new --list-templates 참고); markdown/json과 상호 배타적이며 프레임 인자와는 함께 사용 가능"}
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
            "description": "템플릿의 `{{name}}`를 채운다. values는 평문 치환(hwpx 패키지 보존), parts는 부분(md+HTML, 계약 docs/design/18) 파일을 앵커 문단에 블록 이식(.hwp/.hwpx).",
            "inputSchema": {"type": "object", "properties": {
                "input": {"type": "string"}, "output": {"type": "string"},
                "values": {"type": "object", "additionalProperties": {"type": "string"},
                    "description": "{자리표시자이름: 값} 객체"},
                "parts": {"type": "object", "additionalProperties": {"type": "string"},
                    "description": "{앵커이름: 부분 파일 경로(md+HTML)} 객체 — 앵커 문단을 부분 블록으로 교체"},
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
            "name": "hwp_lint",
            "description": "공문서 표기법·구조 규칙 검사(markdown). 권고성 — hwp-lint-report-v1 형태의 findings JSON(rule_id·severity·line·col·message)을 반환하며 원문 발췌는 포함하지 않는다.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string", "description": "검사할 markdown 파일 경로(--root 샌드박스 안)"}
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
        json!({
            "name": "hwp_put_file",
            "description": "base64 콘텐츠를 세션 워크스페이스의 파일로 저장한다. 원격 배포에서 기존 문서를 넣는 유일한 경로다(도구 인자는 경로를 받으므로, 먼저 이 도구로 올린 뒤 그 이름을 넘긴다). 복호 512 KiB 상한.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "name": {"type": "string", "description": "워크스페이스 안의 파일명 또는 상대 경로"},
                    "content": {"type": "string", "description": "파일 내용의 base64. 복호 512 KiB 상한"}
                },
                "required": ["name", "content"]
            }
        }),
        json!({
            "name": "hwp_get_file",
            "description": "워크스페이스 파일을 base64로 돌려준다(영수증 JSON + embedded resource 두 블록). 512 KiB 상한이며 초과 시 자르지 않고 거절한다. 중간 산출물은 워크스페이스에 두고 최종 결과물만 받는 것이 좋다 — 반환된 base64는 클라이언트의 메시지 스트림에 그대로 쌓인다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": {"type": "string", "description": "워크스페이스 안의 파일 경로"}
                },
                "required": ["path"]
            }
        }),
        json!({
            "name": "hwp_merge",
            "description": "여러 HWP5/HWPX 문서를 인자 순서대로 하나로 합친다 (입력 하나당 Section 하나). 출력 포맷은 output 확장자(.hwp/.hwpx)로 정한다. 쪽/각주/개요 번호는 각 입력의 시작·계속 설정을 그대로 유지하므로 병합 후 수동 조정이 필요할 수 있다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "inputs": {"type": "array", "items": {"type": "string"}, "minItems": 2, "description": "병합할 입력 경로 2개 이상 (표준 입력 \"-\"는 지원하지 않음)"},
                    "output": {"type": "string", "description": "출력 경로 (.hwp 또는 .hwpx)"},
                    "strict": {"type": "boolean", "description": "보존 불가(opaque) 데이터가 있으면 게시하지 않고 실패; 기본 false (CLI와 동일). 병합은 첫 입력 이후의 패키지 passthrough를 항상 버리므로 strict를 켜면 평범한 병합도 거부된다 — 응답의 preservation 원장으로 손실을 판단하라"},
                    "loss_report": {"type": "string", "description": "hwp-preservation-report-v1 JSON을 기록할 경로(선택)"},
                    "password": {"type": "string", "description": "이번 hwp_merge 호출에만 사용할 문서 암호 — 모든 입력에 동일하게 적용"}
                },
                "required": ["inputs", "output"]
            }
        }),
        json!({
            "name": "hwp_split",
            "description": "문서 하나를 여러 조각으로 나눠 out_dir에 게시한다. 기본은 구역(Section) 단위이며, pages를 주면 쪽 범위로 나눈다. 쪽 경계는 한컴이 저장한 레이아웃 캐시에서 얻은 추정값이라 한컴 자체 페이지 나눔과 다를 수 있다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "input": {"type": "string"},
                    "out_dir": {"type": "string", "description": "조각 출력 디렉터리 (파일명은 \"<입력스템>-NNN.<소문자 확장자>\")"},
                    "pages": {"type": "array", "items": {"type": "string"}, "description": "구역 대신 사용할 쪽 범위 \"N\" 또는 \"N-M\" 목록(1-based, 양끝 포함, 선택)"},
                    "strict": {"type": "boolean", "description": "보존 불가(opaque) 데이터가 있으면 게시하지 않고 실패; 기본 false (CLI와 동일) — 응답의 preservation 원장으로 손실을 판단하라"},
                    "loss_report": {"type": "string", "description": "hwp-preservation-report-v1 JSON을 기록할 경로(선택)"},
                    "password": {"type": "string", "description": "이번 hwp_split 호출에만 사용할 문서 암호"}
                },
                "required": ["input", "out_dir"]
            }
        }),
        json!({
            "name": "hwp_compare",
            "description": "문서 두 개의 문단·구조 차이를 hwp-compare-report-v1 JSON으로 보고한다. 두 입력 모두 수정하지 않는 읽기 전용 작업이다. 차이가 있는 것은 정상 결과이므로 isError가 되지 않으며, 호출자는 identical 필드를 읽는다 (CLI의 diff(1) 종료 코드에 해당하는 MCP 개념은 없다). 렌더 결과를 한컴 참조 PNG와 비교하는 hwp_diff와는 다른 도구다.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "a": {"type": "string", "description": "첫 번째 HWP/HWPX 파일"},
                    "b": {"type": "string", "description": "두 번째 HWP/HWPX 파일"},
                    "password": {"type": "string", "description": "이번 hwp_compare 호출에만 사용할 문서 암호 — 두 입력에 동일하게 적용"}
                },
                "required": ["a", "b"]
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::authority::{LocalFsContext, canonicalize_mcp_path};
    #[cfg(windows)]
    use super::authority::{
        canonical_path_starts_with, sandbox_compatible_mcp_write_path,
        strip_windows_verbatim_prefix,
    };
    use super::stdio::read_line_bounded;
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_use_sandbox_compatible_drive_spelling() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\C:\Temp\document.hwpx")),
            PathBuf::from(r"C:\Temp\document.hwpx")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_use_sandbox_compatible_unc_spelling() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\document.hwpx")),
            PathBuf::from(r"\\server\share\document.hwpx")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_retain_verbatim_spelling_when_semantics_can_change() {
        for raw in [
            r"\\?\C:\root.\document.hwpx",
            r"\\?\C:\root \document.hwpx",
            r"\\?\C:\NUL\document.hwpx",
            r"\\?\C:\CON.txt\document.hwpx",
            r"\\?\C:\COM1\document.hwpx",
            r"\\?\C:\LPT9.log\document.hwpx",
            r"\\?\UNC\server\share.\document.hwpx",
        ] {
            let path = PathBuf::from(raw);
            assert_eq!(strip_windows_verbatim_prefix(path.clone()), path, "{raw}");
        }

        let long = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(260)));
        assert_eq!(strip_windows_verbatim_prefix(long.clone()), long);
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_paths_reserve_ordinary_length_for_atomic_staging() {
        let below_direct_limit = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(243)));
        assert_ne!(
            strip_windows_verbatim_prefix(below_direct_limit.clone()),
            below_direct_limit
        );
        let at_direct_limit = PathBuf::from(format!(r"\\?\C:\{}", "a".repeat(244)));
        assert_eq!(
            strip_windows_verbatim_prefix(at_direct_limit.clone()),
            at_direct_limit
        );

        let below_write_limit = PathBuf::from(format!(r"\\?\C:\{}\x", "a".repeat(140)));
        assert_ne!(
            sandbox_compatible_mcp_write_path(&below_write_limit),
            below_write_limit
        );
        let at_write_limit = PathBuf::from(format!(r"\\?\C:\{}\x", "a".repeat(141)));
        assert_eq!(
            sandbox_compatible_mcp_write_path(&at_write_limit),
            at_write_limit
        );

        let long_filename_output = PathBuf::from(format!(r"\\?\C:\{}\out.hwpx", "a".repeat(180)));
        assert_ne!(
            strip_windows_verbatim_prefix(long_filename_output.clone()),
            long_filename_output
        );
        assert_eq!(
            sandbox_compatible_mcp_write_path(&long_filename_output),
            long_filename_output
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_root_check_accepts_only_semantically_equivalent_spellings() {
        assert!(canonical_path_starts_with(
            Path::new(r"C:\Temp\workspace\document.hwpx"),
            Path::new(r"\\?\C:\Temp\workspace")
        ));
        assert!(!canonical_path_starts_with(
            Path::new(r"C:\root\document.hwpx"),
            Path::new(r"\\?\C:\root.")
        ));
        assert!(canonical_path_starts_with(
            Path::new(r"\\?\C:\root.\document.hwpx"),
            Path::new(r"\\?\C:\root.")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_mcp_roots_keep_canonical_identity_for_downstream_checks() {
        let base =
            std::env::temp_dir().join(format!("hwp-cli-mcp-canonical-root-{}", std::process::id()));
        let root = base.join("root");
        let document = root.join("document.hwpx");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&document, b"test").unwrap();

        let canonical_root = canonicalize_mcp_path(&root).unwrap();
        let canonical_document = std::fs::canonicalize(&document).unwrap();
        assert!(canonical_document.starts_with(&canonical_root));

        let ctx = ctx_with_roots(vec![canonical_root.clone()]);
        assert_eq!(
            checked_read_path(&ctx, document.to_str().unwrap()).unwrap(),
            strip_windows_verbatim_prefix(canonical_document)
        );

        let unsafe_output = canonical_root.join("report.hwpx.");
        assert_eq!(
            checked_write_path(&ctx, unsafe_output.to_str().unwrap()).unwrap(),
            unsafe_output
        );
        let _ = std::fs::remove_dir_all(base);
    }

    fn ctx() -> LocalFsContext {
        LocalFsContext::new(
            vec![PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fonts"
            ))],
            Vec::new(),
        )
    }

    /// Sandbox context allowing only the given root (canonicalized by the caller).
    fn ctx_with_roots(roots: Vec<PathBuf>) -> LocalFsContext {
        LocalFsContext::new(Vec::new(), roots)
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
        assert_eq!(
            v["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
        assert!(v["result"]["serverInfo"]["name"].is_string());
    }

    #[test]
    fn initialize_프로토콜_버전_협상() {
        // A supported version is echoed as-is.
        for version in SUPPORTED_PROTOCOL_VERSIONS {
            let v = call(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{version}"}}}}"#
            ));
            assert_eq!(v["result"]["protocolVersion"], version);
        }
        // An unsupported version gets the newest version in response.
        let v = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(
            v["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
        // A missing protocolVersion parameter also gets the newest version.
        let v = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(
            v["result"]["protocolVersion"],
            SUPPORTED_PROTOCOL_VERSIONS[0]
        );
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
        assert!(
            handle_request(
                r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"hwp_read","arguments":{"path":"ignored.hwp","password":"notification-secret"}}}"#,
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
            "hwp_grep",
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
            "hwp_merge",
            "hwp_split",
            "hwp_compare",
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
        // #77: positioned/counted row+column insertion fields are exposed.
        let add_row = &edit["inputSchema"]["properties"]["add_row"]["items"]["properties"];
        for field in ["at", "count", "template_row"] {
            assert_eq!(add_row[field]["type"], "integer", "add_row.{field}");
        }
        assert_eq!(
            edit["inputSchema"]["properties"]["add_col"]["items"]["properties"]["count"]["minimum"],
            1
        );
        // #78: clone_table exposes source_table/anchor/text_mode.
        let clone_table = &edit["inputSchema"]["properties"]["clone_table"];
        assert_eq!(
            clone_table["items"]["required"],
            json!(["source_table", "anchor"])
        );
        assert_eq!(
            clone_table["items"]["properties"]["text_mode"]["enum"],
            json!(["blank", "keep"])
        );
        for name in ["hwp_render", "hwp_diff"] {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            let dpi = &tool["inputSchema"]["properties"]["dpi"];
            assert_eq!(dpi["minimum"], hwp_render::MIN_DPI);
            assert_eq!(dpi["maximum"], hwp_render::MAX_DPI);
        }
    }

    /// The exact set of tools that may take a per-call `password`. Every one of
    /// them loads a protected input through the password-aware loader; the
    /// document-level trio joined it when `hwp merge`/`split`/`compare` gained
    /// MCP surfaces. Adding a name here is a deliberate widening of the
    /// credential surface, so the closed-schema test below pins it.
    const PASSWORD_SCOPED_TOOLS: [&str; 6] = [
        "hwp_read",
        "hwp_convert",
        "hwp_render",
        "hwp_merge",
        "hwp_split",
        "hwp_compare",
    ];

    #[test]
    fn password_read_schema_is_closed_and_scoped() {
        let tools = tool_defs();
        for name in PASSWORD_SCOPED_TOOLS {
            let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            assert_eq!(
                tool["inputSchema"]["properties"]["password"]["type"],
                "string"
            );
        }
        for tool in &tools {
            let name = tool["name"].as_str().unwrap();
            if !PASSWORD_SCOPED_TOOLS.contains(&name) {
                assert!(
                    tool["inputSchema"]["properties"].get("password").is_none(),
                    "{name} must not accept password"
                );
            }
        }
    }

    /// `hwp_merge` → `hwp_split` → `hwp_compare` over the same documents the CLI
    /// path uses, proving the three MCP tools reach `commands::{merge,split,
    /// compare}::execute` and report their structured results.
    #[test]
    fn mcp_document_workflow_merge_split_compare_round_trip() {
        let a = temp_file("workflow-a.hwpx");
        let b = temp_file("workflow-b.hwpx");
        let merged = temp_file("workflow-merged.hwpx");
        let fragments_dir = temp_file("workflow-fragments");
        create_hwpx(&a, "첫 문서 본문");
        create_hwpx(&b, "둘째 문서 본문");
        std::fs::create_dir_all(&fragments_dir).unwrap();

        let result = tool_merge_scoped(&mut json!({"inputs": [a, b], "output": merged}), &ctx())
            .expect("hwp_merge");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["inputs"].as_array().unwrap().len(), 2);
        assert_eq!(
            report["strict"], false,
            "병합은 패키지 passthrough 손실이 불가피하므로 CLI와 같은 기본값을 쓴다"
        );
        assert!(
            !report["preservation"].is_null(),
            "손실 원장은 strict와 무관하게 항상 돌려준다"
        );
        assert_eq!(
            load_document(&merged).unwrap().sections.len(),
            2,
            "입력 하나당 Section 하나"
        );

        let result = tool_split_scoped(
            &mut json!({"input": merged, "out_dir": fragments_dir}),
            &ctx(),
        )
        .expect("hwp_split");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        let fragments = report["fragments"].as_array().unwrap();
        assert_eq!(fragments.len(), 2, "구역 두 개는 조각 두 개가 된다");
        for fragment in fragments {
            assert!(Path::new(fragment.as_str().unwrap()).exists());
        }

        // Differences are a normal result: isError stays false and the caller
        // reads `identical` instead of a diff(1) exit code.
        let result =
            tool_compare_scoped(&mut json!({"a": a, "b": merged}), &ctx()).expect("hwp_compare");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["contract"], "hwp-compare-report-v1");
        assert_eq!(report["identical"], false);

        let result = tool_compare_scoped(&mut json!({"a": a, "b": a}), &ctx())
            .expect("hwp_compare 자기비교");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["identical"], true);
    }

    /// The document-level tools honour `--root` exactly like every other write
    /// tool: an input or an output outside the configured roots is refused
    /// before any file is touched.
    #[test]
    fn mcp_document_workflow_respects_roots() {
        let root = temp_file("workflow-root");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("inside.hwpx");
        let outside = temp_file("workflow-outside.hwpx");
        create_hwpx(&inside, "루트 안 문서");
        create_hwpx(&outside, "루트 밖 문서");
        let ctx = ctx_with_roots(vec![std::fs::canonicalize(&root).unwrap()]);

        let error = tool_merge_scoped(
            &mut json!({"inputs": [inside, outside], "output": root.join("out.hwpx")}),
            &ctx,
        )
        .expect_err("루트 밖 입력은 거부");
        assert!(!error.message.is_empty());

        let error = tool_compare_scoped(&mut json!({"a": inside, "b": outside}), &ctx)
            .expect_err("루트 밖 비교 대상은 거부");
        assert!(!error.message.is_empty());

        let error = tool_split_scoped(
            &mut json!({"input": inside, "out_dir": temp_file("workflow-outside-dir")}),
            &ctx,
        )
        .expect_err("루트 밖 출력 디렉터리는 거부");
        assert!(!error.message.is_empty());
    }

    /// `hwp merge` needs at least two inputs, and refuses stdin by name because
    /// the loader stages stdin to a single temp path.
    #[test]
    fn mcp_merge_rejects_thin_and_stdin_inputs() {
        let a = temp_file("merge-guard-a.hwpx");
        create_hwpx(&a, "본문");
        let output = temp_file("merge-guard-out.hwpx");

        let error = tool_merge_scoped(&mut json!({"inputs": [a], "output": output}), &ctx())
            .expect_err("입력 하나는 거부");
        assert!(error.message.contains("2개 이상"));

        let error = tool_merge_scoped(&mut json!({"inputs": [a, "-"], "output": output}), &ctx())
            .expect_err("표준 입력은 거부");
        assert!(error.message.contains("표준 입력"));
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
        let result = tool_compose(
            &json!({
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
            }),
            &ctx(),
        )
        .expect("compose dry-run");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["native"], true);
        assert!(!output.exists());
    }

    #[test]
    fn compose_v2_rejects_deprecated_global_fallback_policy() {
        let error = tool_compose(
            &json!({
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
            }),
            &ctx(),
        )
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
        let error = tool_compose(
            &json!({
                "spec": {"version": "1.0", "sections": []},
                "output": "out.hwpx",
                "unknown": true
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("unknown"));
    }

    #[test]
    fn compose_binds_spec_image_assets_to_sandbox_roots() {
        let directory = std::env::temp_dir().join(format!(
            "hwp-mcp-compose-roots-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("assets")).unwrap();
        std::fs::write(
            directory.join("assets/image.gif"),
            b"GIF89a\x02\x00\x01\x00",
        )
        .unwrap();
        let sandbox = ctx_with_roots(vec![std::fs::canonicalize(&directory).unwrap()]);
        let output = directory.join("out.hwpx");

        // A spec referencing assets below the sandbox root composes as usual (verifies roots plumbing).
        let result = tool_compose(
            &json!({
                "spec": {
                    "version": "1.0",
                    "sections": [{
                        "blocks": [{
                            "type": "image",
                            "path": "assets/image.gif",
                            "width_mm": 20,
                            "height_mm": 10,
                            "placement": "inline"
                        }]
                    }]
                },
                "base_dir": directory,
                "output": output,
                "dry_run": true
            }),
            &sandbox,
        )
        .expect("image under sandbox root composes");
        let report: Value = serde_json::from_str(result[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["dry_run"], true);
        assert_eq!(report["images"], 1);

        let _ = std::fs::remove_dir_all(&directory);
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
        let result = tool_template(
            &json!({
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
            }),
            &ctx(),
        )
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
        }), &ctx())
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
                roots: &[],
            },
            &[],
            &crate::commands::new::NewOptions::default(),
        )
        .unwrap();
    }

    #[test]
    fn hwp_new_preset_and_margin_parity() {
        let mcp_output = temp_file("hwp-new-mcp-profile.hwpx");
        let cli_output = temp_file("hwp-new-cli-profile.hwpx");
        for path in [&mcp_output, &cli_output] {
            let _ = std::fs::remove_file(path);
        }
        let mcp = tool_new(
            &json!({
                "output": mcp_output,
                "markdown": "1. item\n",
                "preset": "gongmun",
                "margin_top_mm": 25.0,
                "margin_right_mm": 30.0,
            }),
            &ctx(),
        )
        .expect("MCP hwp_new");
        assert!(!mcp.is_empty());
        let options = crate::commands::new::NewOptions::from_millimetres(
            Some(hwp_convert::OfficialPreset::Official),
            Some(25.0),
            None,
            None,
            Some(30.0),
            false,
        )
        .unwrap();
        crate::commands::new::execute(
            &cli_output,
            crate::commands::new::NewInput::Markdown {
                text: "1. item\n",
                base_dir: None,
                roots: &[],
            },
            &[],
            &options,
        )
        .expect("CLI new path");
        let mcp_document = hwpx::read_document(&mcp_output).unwrap().document;
        let cli_document = hwpx::read_document(&cli_output).unwrap().document;
        assert_eq!(mcp_document.header, cli_document.header);
        assert_eq!(mcp_document.sections, cli_document.sections);

        let schema = tool_defs()
            .into_iter()
            .find(|tool| tool["name"] == "hwp_new")
            .unwrap();
        assert_eq!(schema["inputSchema"]["additionalProperties"], false);
        for key in [
            "output",
            "markdown",
            "json",
            "set_meta",
            "preset",
            "margin_top_mm",
            "margin_bottom_mm",
            "margin_left_mm",
            "margin_right_mm",
            "doc_head",
            "doc_foot",
            "notice_head",
            "notice_foot",
            "press_head",
            "template",
        ] {
            assert!(
                schema["inputSchema"]["properties"].get(key).is_some(),
                "{key}"
            );
        }

        for path in [&mcp_output, &cli_output] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// D-09: the five frame arguments and `style_tables` are published in `tools/list`, so an
    /// agent can discover them without reading source.
    #[test]
    fn hwp_new_and_hwp_edit_schemas_publish_gong03_arguments() {
        let tools = tool_defs();
        let new_schema = tools.iter().find(|tool| tool["name"] == "hwp_new").unwrap();
        for key in [
            "doc_head",
            "doc_foot",
            "notice_head",
            "notice_foot",
            "press_head",
        ] {
            let property = &new_schema["inputSchema"]["properties"][key];
            assert_eq!(property["type"], "array", "{key}");
            assert_eq!(
                property["items"]["properties"]["key"]["type"], "string",
                "{key}"
            );
            assert_eq!(
                property["items"]["properties"]["value"]["type"], "string",
                "{key}"
            );
            assert_eq!(
                property["items"]["required"],
                json!(["key", "value"]),
                "{key}"
            );
        }
        assert_eq!(
            new_schema["inputSchema"]["properties"]["template"]["type"],
            "string"
        );

        let edit_schema = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_edit")
            .unwrap();
        assert_eq!(
            edit_schema["inputSchema"]["properties"]["style_tables"]["type"],
            "string"
        );
    }

    /// Frame arguments over MCP go through the SAME `parse_frame_fields` validator the CLI uses
    /// (`unknown_doc_head_key_fails_closed` in `tests/frames.rs` pins the CLI-side message).
    #[test]
    fn hwp_new_frame_argument_shares_cli_validator_and_error() {
        let output = temp_file("hwp-new-frame-unknown-key.hwpx");
        let _ = std::fs::remove_file(&output);
        let error = tool_new(
            &json!({
                "output": output,
                "markdown": "본문",
                "doc_head": [{"key": "없는키", "value": "x"}],
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(
            error.contains("doc_head에 알 수 없는 키입니다: 없는키=x"),
            "{error}"
        );
        assert!(!output.exists());
    }

    /// Every paragraph's text, table cells included — frame values live inside table cells.
    fn frame_text(paragraphs: &[hwp_model::Paragraph], out: &mut String) {
        for paragraph in paragraphs {
            for ch in &paragraph.chars {
                if let hwp_model::HwpChar::Text(c) = ch {
                    out.push(*c);
                }
            }
            out.push('\n');
            for control in &paragraph.controls {
                if let hwp_model::Control::Table(table) = control {
                    for cell in &table.cells {
                        frame_text(&cell.paragraphs, out);
                    }
                }
            }
        }
    }

    /// A `template` brings its own native frames over MCP as on the CLI, and a frame argument
    /// overrides one key of them instead of adding a second row.
    #[test]
    fn hwp_new_template_frames_are_native_and_arguments_override_them() {
        let output = temp_file("hwp-new-template-with-frames.hwpx");
        let _ = std::fs::remove_file(&output);
        tool_new(
            &json!({
                "output": output,
                "template": "gian-external",
                "doc_head": [{"key": "기관명", "value": "테스트기관"}],
            }),
            &ctx(),
        )
        .expect("MCP hwp_new with a template and a frame argument");
        let document = hwpx::read_document(&output).unwrap().document;
        let tables = document
            .sections
            .iter()
            .flat_map(|section| section.paragraphs.iter())
            .flat_map(|paragraph| paragraph.controls.iter())
            .filter(|control| matches!(control, hwp_model::Control::Table(_)))
            .count();
        assert_eq!(tables, 2, "the template's 두문/결문 must be native tables");
        let mut text = String::new();
        frame_text(&document.sections[0].paragraphs, &mut text);
        assert!(text.contains("테스트기관"), "the argument value is missing");
        assert!(
            !text.contains("{{기관명}}"),
            "the slot default survived alongside the argument — the row is duplicated"
        );
        assert!(
            text.contains("{{수신}}"),
            "a key the caller did not supply must keep its slot default"
        );
        let _ = std::fs::remove_file(&output);
    }

    /// D-05: `template` is also refused together with `markdown`/`json` (the MCP-side equivalent
    /// of the CLI's `--template`/`--from` exclusivity).
    #[test]
    fn hwp_new_template_and_markdown_refused() {
        let output = temp_file("hwp-new-template-markdown-conflict.hwpx");
        let _ = std::fs::remove_file(&output);
        let error = tool_new(
            &json!({
                "output": output,
                "template": "official",
                "markdown": "본문",
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("template과 markdown"), "{error}");
        assert!(!output.exists());
    }

    /// Frame construction over MCP produces the byte-identical document the CLI `--doc-head`/
    /// `--doc-foot` flags produce (D-09 parity, same shape as `hwp_new_preset_and_margin_parity`).
    #[test]
    fn hwp_new_frame_arguments_parity_with_cli() {
        let mcp_output = temp_file("hwp-new-mcp-frames.hwpx");
        let cli_output = temp_file("hwp-new-cli-frames.hwpx");
        for path in [&mcp_output, &cli_output] {
            let _ = std::fs::remove_file(path);
        }
        tool_new(
            &json!({
                "output": mcp_output,
                "markdown": "본문",
                "preset": "official",
                "doc_head": [{"key": "기관명", "value": "테스트기관"}],
                "doc_foot": [{"key": "발신명의", "value": "테스트기관장"}],
            }),
            &ctx(),
        )
        .expect("MCP hwp_new with frames");
        let options = crate::commands::new::NewOptions::from_millimetres(
            Some(hwp_convert::OfficialPreset::Official),
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap()
        .with_frames(
            &["기관명=테스트기관".to_string()],
            &["발신명의=테스트기관장".to_string()],
            &[],
            &[],
            &[],
        )
        .unwrap();
        crate::commands::new::execute(
            &cli_output,
            crate::commands::new::NewInput::Markdown {
                text: "본문",
                base_dir: None,
                roots: &[],
            },
            &[],
            &options,
        )
        .expect("CLI new path with frames");
        let mcp_document = hwpx::read_document(&mcp_output).unwrap().document;
        let cli_document = hwpx::read_document(&cli_output).unwrap().document;
        assert_eq!(mcp_document.header, cli_document.header);
        assert_eq!(mcp_document.sections, cli_document.sections);

        for path in [&mcp_output, &cli_output] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// `hwp_edit`'s `style_tables` creates the same `TypedEditOperation::StyleTables` operation
    /// the CLI `--style-tables` flag routes to (D-09), so it produces the same shading/width
    /// styling `style_table_edit.rs`-shaped tests already pin for the CLI path.
    #[test]
    fn hwp_edit_style_tables_shares_cli_operation() {
        let source = temp_file("style-tables-mcp-source.hwpx");
        let mcp_output = temp_file("style-tables-mcp-out.hwpx");
        // A raw-HTML table (not a GFM markdown table) imports through `from_html`, which never
        // calls `style_table` at import time, so this table starts genuinely unstyled — the
        // same fixture shape `tests/style_tables_idempotence.rs` uses for a real (not
        // already-idempotent) styling target.
        create_hwpx(
            &source,
            "<table>\n<tr><th>이름</th><th>값</th></tr>\n<tr><td>항목1</td><td>1</td></tr>\n</table>\n",
        );
        let _ = std::fs::remove_file(&mcp_output);
        let result = tool_edit(
            &json!({
                "input": source,
                "output": mcp_output,
                "style_tables": "official",
            }),
            &ctx(),
        )
        .expect("MCP hwp_edit style_tables");
        assert!(!result.is_empty());
        let styled = hwpx::read_document(&mcp_output).unwrap().document;
        let before = hwpx::read_document(&source).unwrap().document;
        assert_ne!(
            styled.sections, before.sections,
            "styling must change the tables"
        );

        for path in [&source, &mcp_output] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// `style_tables` rejects an unknown preset name through the same `OfficialPreset::parse`
    /// error the CLI `--style-tables` flag produces.
    #[test]
    fn hwp_edit_style_tables_rejects_unknown_preset() {
        let source = temp_file("style-tables-unknown-preset-source.hwpx");
        create_hwpx(&source, "본문");
        let output = temp_file("style-tables-unknown-preset-out.hwpx");
        let _ = std::fs::remove_file(&output);
        let error = tool_edit(
            &json!({
                "input": source,
                "output": output,
                "style_tables": "no-such-preset",
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(!error.is_empty());
        assert!(!output.exists());

        let _ = std::fs::remove_file(&source);
    }

    #[test]
    fn hwp_new_runtime_rejects_unknown_key() {
        let output = temp_file("hwp-new-runtime-unknown.hwpx");
        let _ = std::fs::remove_file(&output);
        let error = tool_new(&json!({"output": output, "unknown": true}), &ctx()).unwrap_err();
        assert!(error.contains("알 수 없는 hwp_new 인자"), "{error}");
        assert!(!output.exists(), "unknown input must not publish output");
    }

    #[test]
    fn tools_call_hwp_new_rejects_unknown_key() {
        let output = temp_file("hwp-new-dispatch-unknown.hwpx");
        let _ = std::fs::remove_file(&output);
        let request = json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "tools/call",
            "params": {
                "name": "hwp_new",
                "arguments": {"output": output, "unknown": true}
            }
        });
        let response = handle_request(&request.to_string(), &ctx()).unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("알 수 없는 hwp_new 인자")
        );
        assert!(!output.exists(), "unknown input must not publish output");
    }

    #[test]
    fn mcp_mutations_share_cli_atomic_and_noop_contracts() {
        let source = temp_file("mutation-source.hwpx");
        let edit_destination = temp_file("mutation-edit.hwpx");
        let fill_destination = temp_file("mutation-fill.hwpx");
        let convert_destination = temp_file("mutation-convert.unsupported");
        create_hwpx(&source, "{{수신}} 본문");

        std::fs::write(&edit_destination, b"EDIT ORIGINAL").unwrap();
        let edit = tool_edit(
            &json!({
                "input": source,
                "output": edit_destination,
                "replace": [{"from": "없는본문", "to": "값"}]
            }),
            &ctx(),
        );
        assert!(edit.is_err(), "0건 편집은 MCP도 실패");
        assert_eq!(std::fs::read(&edit_destination).unwrap(), b"EDIT ORIGINAL");

        std::fs::write(&fill_destination, b"FILL ORIGINAL").unwrap();
        let fill = tool_fill(
            &json!({
                "input": source,
                "output": fill_destination,
                "values": {"없는키": "값"}
            }),
            &ctx(),
        );
        assert!(fill.is_err(), "0건 fill은 MCP도 실패");
        assert_eq!(std::fs::read(&fill_destination).unwrap(), b"FILL ORIGINAL");

        std::fs::write(&convert_destination, b"CONVERT ORIGINAL").unwrap();
        let convert = tool_convert(
            &json!({
                "input": source,
                "output": convert_destination
            }),
            &ctx(),
        );
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
    fn mcp_fill_parts_splices_part_blocks() {
        let template = temp_file("fill-parts-template.hwpx");
        create_hwpx(&template, "# 보고서\n\n{{본문}}");
        let part = temp_file("fill-parts-part.md");
        std::fs::write(
            &part,
            "부분 본문입니다.\n\n<table>\n<tr><td colspan=\"2\">가로병합</td></tr>\n\
             <tr><td>a</td><td>b</td></tr>\n</table>\n",
        )
        .unwrap();
        let out = temp_file("fill-parts-out.hwpx");
        let result = tool_fill(
            &json!({
                "input": template,
                "output": out,
                "values": {},
                "parts": {"본문": part.display().to_string()}
            }),
            &ctx(),
        );
        let content = result.expect("parts fill 성공");
        let text = content[0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("\"parts\""), "리포트 모드: {text}");
        let doc = crate::commands::cat::load_document(&out).unwrap();
        let plain = doc.plain_text();
        assert!(plain.contains("부분 본문입니다."), "{plain}");
        assert!(plain.contains("가로병합"), "{plain}");
        for path in [&template, &part, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// #56: image references inside `hwp_new` markdown input and `hwp_fill` part files are
    /// bound to the `--root` sandbox — an outside-root reference fails the tool call.
    #[test]
    fn mcp_new_and_fill_bind_markdown_images_to_sandbox_roots() {
        let uniq = format!(
            "{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        );
        let root = std::env::temp_dir().join(format!("hwp-mcp-mdimg-root-{uniq}"));
        let outside = std::env::temp_dir().join(format!("hwp-mcp-mdimg-outside-{uniq}"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // Tiny PNG (magic + IHDR with 8x8) — the file only needs to exist for the sandbox check.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(8u32.to_be_bytes());
        png.extend(8u32.to_be_bytes());
        png.extend([0u8; 8]);
        let outside_png = outside.join("out.png");
        std::fs::write(&outside_png, &png).unwrap();
        // Markdown link destinations treat `\` as an escape — forward slashes (Windows CI).
        let outside_ref = outside_png.display().to_string().replace('\\', "/");
        let sandbox = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);

        // hwp_new: markdown referencing an absolute image outside the roots fails closed.
        let new_out = root.join("new.hwpx");
        let err = tool_new(
            &json!({
                "markdown": format!("본문\n\n![x]({outside_ref})\n"),
                "output": new_out,
            }),
            &sandbox,
        )
        .unwrap_err();
        assert!(err.contains("샌드박스"), "{err}");
        assert!(!new_out.exists(), "실패 시 출력을 게시하지 않음");

        // hwp_fill: a part file (itself inside the roots) referencing an outside image fails.
        let template = root.join("template.hwpx");
        create_hwpx(&template, "{{본문}}");
        let part = root.join("part.md");
        std::fs::write(&part, format!("부분\n\n![x]({outside_ref})\n")).unwrap();
        let fill_out = root.join("fill.hwpx");
        let err = tool_fill(
            &json!({
                "input": template,
                "output": fill_out,
                "values": {},
                "parts": {"본문": part.display().to_string()}
            }),
            &sandbox,
        )
        .unwrap_err();
        assert!(err.contains("샌드박스"), "{err}");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
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
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
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
        let result = tool_new(
            &json!({
                "output": destination,
                "json": document_json,
            }),
            &ctx(),
        );
        assert!(
            result.as_ref().is_err_and(|error| {
                error.contains("보존 불가")
                    && error.contains("opaque_control_unrepresentable")
                    && !error.contains("zzzz")
            }),
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
        let content = tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "replace": [
                    {"from": "있는본문", "to": "바뀐본문"},
                    {"from": "없는본문", "to": "값"}
                ],
                "allow_partial": true
            }),
            &ctx(),
        )
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
    fn mcp_add_row_col_positioned_counted() {
        // #77: MCP add_row/add_col accept at/count/template_row and run through the
        // same conversion primitives as the CLI.
        let source = temp_file("addrowcol-source.hwpx");
        let destination = temp_file("addrowcol-out.hwpx");
        create_hwpx(&source, "| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "add_row": [{"table": 0, "at": 1, "count": 2, "template_row": 0}],
                "add_col": [{"table": 0, "at": 1, "count": 2}]
            }),
            &ctx(),
        )
        .expect("positioned counted MCP edit");
        // Rows 1-2 and cols 1-2 were inserted into the 2x2 table (now 4x4); fill a
        // cell of an inserted row to prove the grid is addressable.
        let fill_destination = temp_file("addrowcol-fill.hwpx");
        tool_edit(
            &json!({
                "input": destination,
                "output": fill_destination,
                "set_cell": [{"table": 0, "row": 2, "col": 3, "text": "MCP신규"}]
            }),
            &ctx(),
        )
        .expect("set_cell into the inserted band");
        let content = tool_read(
            &json!({"path": fill_destination, "format": "plain"}),
            &ctx(),
        )
        .unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("MCP신규"), "inserted cell filled: {text}");
        assert!(text.contains("가"), "original content kept: {text}");
        // count = 0 is rejected.
        let rejected = tool_edit(
            &json!({
                "input": source,
                "output": temp_file("addrowcol-reject.hwpx"),
                "add_row": [{"table": 0, "count": 0}]
            }),
            &ctx(),
        );
        assert!(rejected.is_err(), "count 0 is refused");
        let _ = std::fs::remove_file(temp_file("addrowcol-reject.hwpx"));
        for path in [&source, &destination, &fill_destination] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_clone_table_blank_default_and_keep_mode() {
        // #78: MCP clone_table runs through the same conversion primitive as the CLI.
        let source = temp_file("clonetbl-source.hwpx");
        let blank_out = temp_file("clonetbl-blank.hwpx");
        let keep_out = temp_file("clonetbl-keep.hwpx");
        create_hwpx(&source, "앵커문단\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        // text_mode omitted → blank: structure cloned, cell text stripped.
        tool_edit(
            &json!({
                "input": source,
                "output": blank_out,
                "clone_table": [{"source_table": 0, "anchor": "앵커문단"}]
            }),
            &ctx(),
        )
        .expect("blank clone");
        let content = tool_read(&json!({"path": blank_out, "format": "plain"}), &ctx()).unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert_eq!(
            text.matches('가').count(),
            1,
            "blank clone strips cell text (source only): {text}"
        );
        // keep mode clones cell text as well.
        tool_edit(
            &json!({
                "input": source,
                "output": keep_out,
                "clone_table": [{"source_table": 0, "anchor": "앵커문단", "text_mode": "keep"}]
            }),
            &ctx(),
        )
        .expect("keep clone");
        let content = tool_read(&json!({"path": keep_out, "format": "plain"}), &ctx()).unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert_eq!(
            text.matches('가').count(),
            2,
            "cloned content present: {text}"
        );
        // An invalid text_mode is rejected with a bounded error.
        let rejected = tool_edit(
            &json!({
                "input": source,
                "output": temp_file("clonetbl-reject.hwpx"),
                "clone_table": [{"source_table": 0, "anchor": "앵커문단", "text_mode": "bogus"}]
            }),
            &ctx(),
        );
        assert!(rejected.is_err(), "invalid text_mode is refused");
        let _ = std::fs::remove_file(temp_file("clonetbl-reject.hwpx"));
        for path in [&source, &blank_out, &keep_out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_read_is_utf8_bounded_and_pageable() {
        let source = temp_file("bounded-read.hwpx");
        create_hwpx(&source, "가나다라마바사");
        let first = tool_read(
            &json!({
                "path": source,
                "format": "plain",
                "max_bytes": 7
            }),
            &ctx(),
        )
        .unwrap();
        assert!(first[0]["text"].as_str().unwrap().len() <= 7);
        let metadata: Value = serde_json::from_str(first[1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["truncated"], true);
        let next = metadata["next_offset"].as_u64().unwrap();
        let second = tool_read(
            &json!({
                "path": source,
                "format": "plain",
                "offset": next,
                "max_bytes": 7
            }),
            &ctx(),
        )
        .unwrap();
        assert!(!second[0]["text"].as_str().unwrap().is_empty());
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_structured_replace_preserves_cli_delimiters_as_data() {
        let source = temp_file("delimiter-source.hwpx");
        let destination = temp_file("delimiter-destination.hwpx");
        create_hwpx(&source, "A=>B");

        tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "replace": [{"from": "A=>B", "to": "X=Y=>Z"}]
            }),
            &ctx(),
        )
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

        tool_edit(
            &json!({
                "input": source,
                "output": destination,
                "set_meta": [{"key": "title", "value": "A=B=>C"}],
                "insert_para": [{
                    "anchor": "Anchor=>Here",
                    "text": "New=>Text=1"
                }]
            }),
            &ctx(),
        )
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

    /// Creates (base, root, outside) directories for sandbox tests.
    /// Windows-CI compatible: uses only the real temp dir, and root is canonicalized before entering ctx.
    fn sandbox_dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("hwp-cli-mcp-sandbox-{tag}-{}", std::process::id()));
        let root = base.join("root");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        (base, root, outside)
    }

    #[test]
    fn sandbox_루트없으면_검사없이_통과() {
        let ctx = ctx_with_roots(Vec::new());
        // Nonexistent paths and '..' pass through with the previous behavior.
        let read = checked_read_path(&ctx, "no/such/file.hwpx").unwrap();
        assert_eq!(read, PathBuf::from("no/such/file.hwpx"));
        let write = checked_write_path(&ctx, "../out.hwpx").unwrap();
        assert_eq!(write, PathBuf::from("../out.hwpx"));
    }

    #[test]
    fn sandbox_루트밖_읽기_거부() {
        let (base, root, outside) = sandbox_dirs("read");
        let document = outside.join("doc.hwpx");
        create_hwpx(&document, "본문");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_read(&json!({"path": document}), &ctx).unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_루트밖_쓰기_거부() {
        let (base, root, outside) = sandbox_dirs("write");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let output = outside.join("out.hwpx");
        let error = tool_fill(
            &json!({
                "input": source,
                "output": output,
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        assert!(!output.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_상위경로_쓰기_거부() {
        let (base, root, _outside) = sandbox_dirs("dotdot");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_fill(
            &json!({
                "input": source,
                "output": root.join("sub").join("..").join("escape.hwpx"),
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains(".."), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_심볼링크_덮어쓰기_우회_거부() {
        let (base, root, outside) = sandbox_dirs("symlink");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let victim = outside.join("victim.hwpx");
        std::fs::write(&victim, b"VICTIM").unwrap();
        let link = root.join("link.hwpx");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_fill(
            &json!({
                "input": source,
                "output": link,
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"VICTIM");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_루트안_읽기쓰기_허용() {
        let (base, root, _outside) = sandbox_dirs("inside");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "{{이름}}");
        let output = root.join("out.hwpx");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        tool_fill(
            &json!({
                "input": source,
                "output": output,
                "values": {"이름": "값"}
            }),
            &ctx,
        )
        .expect("루트 안 fill");
        let doc = load_document(&output).unwrap();
        assert!(doc.plain_text().contains("값"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_호출당_font_dir도_검사() {
        let (base, root, outside) = sandbox_dirs("fontdir");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "본문");
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_render(&json!({"path": source, "font_dir": outside}), &ctx).unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sandbox_edit_중첩_이미지경로_거부() {
        let (base, root, outside) = sandbox_dirs("edit-image");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "앵커");
        let image = outside.join("image.png");
        std::fs::write(&image, b"PNG").unwrap();
        let ctx = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = tool_edit(
            &json!({
                "input": source,
                "output": root.join("out.hwpx"),
                "insert_image": [{"anchor": "앵커", "path": image}]
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Counts controls matched across body, table cells, and text boxes recursively (for typed-edit round-trip checks).
    fn count_controls(
        doc: &hwp_model::Document,
        matches: fn(&hwp_model::Control) -> bool,
    ) -> usize {
        fn in_para(para: &hwp_model::Paragraph, matches: fn(&hwp_model::Control) -> bool) -> usize {
            let mut n = 0;
            for control in &para.controls {
                if matches(control) {
                    n += 1;
                }
                match control {
                    hwp_model::Control::Table(table) => {
                        for cell in &table.cells {
                            for p in &cell.paragraphs {
                                n += in_para(p, matches);
                            }
                        }
                    }
                    hwp_model::Control::Generic(g) => {
                        for list in &g.paragraph_lists {
                            for p in &list.paragraphs {
                                n += in_para(p, matches);
                            }
                        }
                    }
                    _ => {}
                }
            }
            n
        }
        doc.sections
            .iter()
            .flat_map(|section| &section.paragraphs)
            .map(|para| in_para(para, matches))
            .sum()
    }

    #[test]
    fn tools_list_exposes_typed_edit_parity_args() {
        let tools = tool_defs();
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_edit")
            .unwrap();
        let properties = &edit["inputSchema"]["properties"];
        for key in [
            "add_table",
            "set_para",
            "set_cell_para",
            "set_page",
            "delete_image",
            "delete_table",
            "delete_field",
            "delete_bookmark",
        ] {
            assert!(
                properties.get(key).is_some(),
                "hwp_edit 스키마에 {key} 누락"
            );
        }
        // set_page is a single object; the rest are arrays.
        assert_eq!(properties["set_page"]["type"], "object");
        assert_eq!(properties["add_table"]["type"], "array");
    }

    #[test]
    fn mcp_set_cell_by_label_uses_shared_preflight_and_closed_item_schema() {
        let tools = tool_defs();
        let edit = tools
            .iter()
            .find(|tool| tool["name"] == "hwp_edit")
            .unwrap();
        let item = &edit["inputSchema"]["properties"]["set_cell_by_label"]["items"];
        assert_eq!(item["additionalProperties"], false);
        assert_eq!(item["properties"]["table"]["minimum"], 0);

        let source = temp_file("set-cell-by-label-mcp-source.hwpx");
        let output = temp_file("set-cell-by-label-mcp-out.hwpx");
        create_hwpx(&source, "| 성명 | |\n|---|---|\n");
        tool_edit(
            &json!({
                "input": source,
                "output": output,
                "set_cell_by_label": [{"label": " 성명：", "text": "MCP홍길동"}]
            }),
            &ctx(),
        )
        .expect("MCP label edit");
        assert!(
            load_document(&output)
                .unwrap()
                .plain_text()
                .contains("MCP홍길동")
        );

        let missing = tool_edit(
            &json!({
                "input": source,
                "output": output,
                "set_cell_by_label": [{"label": null, "text": "MCP홍길동"}]
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(missing.contains("label 필요"), "{missing}");
        for path in [&source, &output] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_add_table_then_delete_table_round_trip() {
        let source = temp_file("typed-add-table-source.hwpx");
        let mid = temp_file("typed-add-table-mid.hwpx");
        let out = temp_file("typed-add-table-out.hwpx");
        create_hwpx(&source, "머리말\n\n끝말");

        tool_edit(
            &json!({
                "input": source,
                "output": mid,
                "add_table": [{"anchor": "머리말", "rows": [["가", "나"], ["1", "2"]]}]
            }),
            &ctx(),
        )
        .expect("add_table 편집");
        let doc = load_document(&mid).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Table(_))),
            1,
            "표가 하나 있어야 한다"
        );
        let plain = doc.plain_text();
        assert!(
            plain.contains('가') && plain.contains('1'),
            "셀 텍스트: {plain}"
        );

        tool_edit(
            &json!({
                "input": mid,
                "output": out,
                "delete_table": [{"index": 0}]
            }),
            &ctx(),
        )
        .expect("delete_table 편집");
        let doc = load_document(&out).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Table(_))),
            0,
            "표가 삭제되어야 한다"
        );
        let plain = doc.plain_text();
        assert!(
            !plain.contains('가') && plain.contains("끝말"),
            "결과: {plain}"
        );
        for path in [&source, &mid, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_set_para_and_set_page_round_trip() {
        let source = temp_file("typed-set-para-source.hwpx");
        let out = temp_file("typed-set-para-out.hwpx");
        create_hwpx(&source, "본문 문단입니다.");

        tool_edit(
            &json!({
                "input": source,
                "output": out,
                "set_para": [{"pattern": "본문", "line_spacing_pct": 130, "indent_mm": 5}],
                "set_page": {"width_mm": 200, "orientation": "landscape"}
            }),
            &ctx(),
        )
        .expect("set_para/set_page 편집");

        let doc = load_document(&out).unwrap();
        let para = doc.sections[0]
            .paragraphs
            .iter()
            .find(|p| p.plain_text().contains("본문"))
            .expect("본문 문단");
        let shape = &doc.header.para_shapes[para.para_shape.0 as usize];
        assert_eq!(shape.line_spacing_type, 0, "비율 줄간격");
        assert_eq!(shape.line_spacing, 130);
        // The IR uses double-HWPUNIT units (hwp5 PARA_SHAPE).
        assert_eq!(shape.indent, 2 * crate::commands::edit::mm_to_hwpunit(5.0));

        let page = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                hwp_model::Control::SectionDef(secd) => secd.page.as_ref(),
                _ => None,
            })
            .expect("구역 정의의 용지 정보");
        assert_eq!(page.width.0, crate::commands::edit::mm_to_hwpunit(200.0));
        assert_eq!(page.attr & 1, 1, "landscape 비트");
        for path in [&source, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// #221: the typed `set_cell_para` operation reaches the same mutation path as the CLI
    /// flag, so a document edited through MCP matches the one edited through the CLI.
    #[test]
    fn mcp_typed_set_cell_para_matches_the_cli_flag() {
        let source = temp_file("typed-set-cell-para-source.hwpx");
        let mcp_out = temp_file("typed-set-cell-para-mcp.hwpx");
        create_hwpx(&source, "본문\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");

        tool_edit(
            &json!({
                "input": source,
                "output": mcp_out,
                "set_cell": [{"table": 0, "row": 1, "col": 0, "text": "첫째\n\n둘째"}],
                "set_cell_para": [{"table": 0, "row": 1, "col": 0,
                                   "line_spacing_pct": 150, "indent_mm": -12, "align": "center"}]
            }),
            &ctx(),
        )
        .expect("set_cell_para 편집");

        // The CLI mini-language spelling of the same request must build the same props,
        // and both surfaces then call the same hwp-convert entry point.
        let cli_props = crate::commands::edit::parse_para_props(
            "line-spacing:150%,indent:-12mm,align:center",
            "--set-cell-para",
        )
        .expect("CLI 속성 파싱");
        let mcp_props = para_props_item(
            &json!({"line_spacing_pct": 150, "indent_mm": -12, "align": "center"}),
            "set_cell_para",
        )
        .expect("MCP 속성 매핑");
        assert_eq!(cli_props, mcp_props, "CLI와 MCP가 만든 ParaProps가 다르다");
        assert_eq!(
            crate::commands::edit::parse_cell_loc("0:1:0", "--set-cell-para").unwrap(),
            (0, 1, 0),
            "CLI 셀 주소 파싱"
        );

        let shape_of = |path: &Path| {
            let doc = load_document(path).unwrap();
            let cell = doc
                .sections
                .iter()
                .flat_map(|s| &s.paragraphs)
                .flat_map(|p| &p.controls)
                .find_map(|c| match c {
                    hwp_model::Control::Table(t) => Some(t),
                    _ => None,
                })
                .expect("표")
                .cells
                .iter()
                .find(|c| c.row == 1 && c.col == 0)
                .expect("셀 (1,0)")
                .clone();
            let shapes: Vec<hwp_model::ParaShape> = cell
                .paragraphs
                .iter()
                .map(|p| doc.header.para_shapes[p.para_shape.0 as usize].clone())
                .collect();
            let texts: Vec<String> = cell.paragraphs.iter().map(|p| p.plain_text()).collect();
            (texts, shapes)
        };

        let (mcp_texts, mcp_shapes) = shape_of(&mcp_out);
        assert_eq!(mcp_texts, vec!["첫째", "둘째"], "블록 순서");
        assert_eq!(mcp_shapes.len(), 2, "블록 2개 → 문단 2개");
        for ps in &mcp_shapes {
            assert_eq!(ps.line_spacing_type, 0);
            assert_eq!(ps.line_spacing, 150);
            assert_eq!(ps.alignment(), 3, "가운데 정렬");
        }
        for path in [&source, &mcp_out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_delete_image_round_trip() {
        let source = temp_file("typed-delete-image-source.hwpx");
        let mid = temp_file("typed-delete-image-mid.hwpx");
        let out = temp_file("typed-delete-image-out.hwpx");
        let png = temp_file("typed-delete-image.png");
        create_hwpx(&source, "사진: 여기");
        // Minimal PNG header (IHDR 96x96) — insert_image only parses the size.
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend([0, 0, 0, 13]);
        bytes.extend(b"IHDR");
        bytes.extend(96u32.to_be_bytes());
        bytes.extend(96u32.to_be_bytes());
        bytes.extend([0u8; 8]);
        std::fs::write(&png, &bytes).unwrap();

        tool_edit(
            &json!({
                "input": source,
                "output": mid,
                "insert_image": [{"anchor": "사진:", "path": png}]
            }),
            &ctx(),
        )
        .expect("insert_image 편집");
        let doc = load_document(&mid).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Picture(_))),
            1,
            "그림이 하나 있어야 한다"
        );

        tool_edit(
            &json!({
                "input": mid,
                "output": out,
                "delete_image": [{"anchor": "사진:"}]
            }),
            &ctx(),
        )
        .expect("delete_image 편집");
        let doc = load_document(&out).unwrap();
        assert_eq!(
            count_controls(&doc, |c| matches!(c, hwp_model::Control::Picture(_))),
            0,
            "그림이 삭제되어야 한다"
        );
        for path in [&source, &mid, &out, &png] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_delete_field_and_bookmark_round_trip() {
        let source = temp_file("typed-delete-field-source.hwpx");
        let mid = temp_file("typed-delete-field-mid.hwpx");
        let out = temp_file("typed-delete-field-out.hwpx");
        create_hwpx(&source, "참조: 본문");

        tool_edit(
            &json!({
                "input": source,
                "output": mid,
                "create_field": [{"anchor": "참조:", "name": "수신", "value": "홍길동"}],
                "create_bookmark": [{"anchor": "참조:", "name": "참조점"}]
            }),
            &ctx(),
        )
        .expect("create_field/create_bookmark 편집");
        let doc = load_document(&mid).unwrap();
        assert!(
            hwp_convert::list_fields(&doc)
                .iter()
                .any(|f| f.name.as_deref() == Some("수신")),
            "필드가 생성되어야 한다"
        );
        assert!(
            hwp_convert::list_bookmarks(&doc)
                .iter()
                .any(|b| b.name == "참조점"),
            "책갈피가 생성되어야 한다"
        );

        let content = tool_edit(
            &json!({
                "input": mid,
                "output": out,
                "delete_field": [{"name": "수신"}],
                "delete_bookmark": [{"name": "참조점"}]
            }),
            &ctx(),
        )
        .expect("delete_field/delete_bookmark 편집");
        let report: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(report["applied"], 2);
        let doc = load_document(&out).unwrap();
        assert!(
            !hwp_convert::list_fields(&doc)
                .iter()
                .any(|f| f.name.as_deref() == Some("수신")),
            "필드가 삭제되어야 한다"
        );
        assert!(
            !hwp_convert::list_bookmarks(&doc)
                .iter()
                .any(|b| b.name == "참조점"),
            "책갈피가 삭제되어야 한다"
        );
        for path in [&source, &mid, &out] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_typed_edit_validation_errors() {
        // Boundary-validation errors fire before any file is read (input/output are dummy paths).
        let dummy = json!({"input": "in.hwpx", "output": "out.hwpx"});

        let mut args = dummy.clone();
        args["delete_table"] = json!([{"index": 0, "anchor": "표"}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("delete_table"), "{error}");

        let mut args = dummy.clone();
        args["delete_table"] = json!([{}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("delete_table"), "{error}");

        let mut args = dummy.clone();
        args["set_para"] =
            json!([{"pattern": "본문", "line_spacing_pct": 130, "line_spacing_pt": 14}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("line_spacing"), "{error}");

        let mut args = dummy.clone();
        args["set_page"] = json!({"orientation": "sideways"});
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("용지 방향"), "{error}");

        let mut args = dummy.clone();
        args["set_page"] = json!([{"width_mm": 200}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("객체"), "{error}");

        let mut args = dummy.clone();
        args["add_table"] = json!([{"anchor": "머리말", "rows": [["a"], [1]]}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("add_table"), "{error}");

        let mut args = dummy.clone();
        args["add_table"] = json!([{"anchor": "머리말", "rows": "x"}]);
        let error = tool_edit(&args, &ctx()).unwrap_err();
        assert!(error.contains("add_table"), "{error}");

        // Empty rows have the right shape but no content — the library rejects them and that error propagates.
        let source = temp_file("typed-add-table-empty-source.hwpx");
        let out = temp_file("typed-add-table-empty-out.hwpx");
        create_hwpx(&source, "머리말");
        let error = tool_edit(
            &json!({
                "input": source,
                "output": out,
                "add_table": [{"anchor": "머리말", "rows": []}]
            }),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("표 행 데이터"), "{error}");
        assert!(!out.exists(), "실패한 편집은 출력을 게시하지 않는다");
        let _ = std::fs::remove_file(&source);
    }

    // ---- read/convert/render parity + hwp_grep (WI-4) ----

    #[test]
    fn mcp_read_html_and_csv_formats() {
        let source = temp_file("read-html-csv.hwpx");
        create_hwpx(
            &source,
            "본문\n\n<table>\n<tr><td>가</td><td>나</td></tr>\n</table>\n",
        );
        let html = tool_read(&json!({"path": source, "format": "html"}), &ctx()).unwrap();
        let html_text = html[0]["text"].as_str().unwrap();
        assert!(html_text.contains('<'), "html 출력: {html_text}");
        let csv = tool_read(&json!({"path": source, "format": "csv"}), &ctx()).unwrap();
        let csv_text = csv[0]["text"].as_str().unwrap();
        assert!(
            csv_text.contains('가') && csv_text.contains('나'),
            "csv 출력: {csv_text}"
        );
        let _ = std::fs::remove_file(source);
    }

    /// Builds an IR containing header/hidden-note controls (saved as .json, load_document
    /// deserializes it verbatim, so it round-trips without writer loss).
    fn create_ir_json_with_hidden_and_header(path: &Path) {
        let mut doc = hwp_convert::from_markdown("본문\n\n숨은메모\n\n머리말텍스트");
        let hidden_para = doc.sections[0].paragraphs.remove(1);
        let header_para = doc.sections[0].paragraphs.remove(1);
        let body = &mut doc.sections[0].paragraphs[0];
        // from_markdown paragraphs already carry controls (section definitions, etc.),
        // so indexes are captured at push time.
        let hidden_index = body.controls.len() as u32;
        body.controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"hcnt",
                data: Vec::new(),
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![hidden_para],
                }],
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
        let header_index = body.controls.len() as u32;
        body.controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"head",
                data: Vec::new(),
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![header_para],
                }],
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
            }));
        body.chars.insert(
            0,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::ctrl_char::HEADER_FOOTER,
                ctrl_id: *b"head",
                payload: vec![0; 12],
                ctrl_index: Some(header_index),
            },
        );
        body.chars.insert(
            0,
            hwp_model::HwpChar::ExtCtrl {
                code: hwp_model::ctrl_char::HIDDEN_COMMENT,
                ctrl_id: *b"hcnt",
                payload: vec![0; 12],
                ctrl_index: Some(hidden_index),
            },
        );
        let document_json = hwp_convert::to_json(&doc, true, false).unwrap();
        std::fs::write(path, document_json).unwrap();
    }

    #[test]
    fn mcp_read_with_hidden_and_header_footer_flags() {
        let source = temp_file("read-with-flags.json");
        create_ir_json_with_hidden_and_header(&source);

        let plain = tool_read(&json!({"path": source, "format": "plain"}), &ctx()).unwrap();
        let text = plain[0]["text"].as_str().unwrap();
        assert!(!text.contains("숨은메모"), "기본은 숨은 설명 제외: {text}");
        assert!(!text.contains("머리말텍스트"), "기본은 머리말 제외: {text}");

        let hidden = tool_read(
            &json!({"path": source, "format": "plain", "with_hidden": true}),
            &ctx(),
        )
        .unwrap();
        let text = hidden[0]["text"].as_str().unwrap();
        assert!(text.contains("숨은메모"), "with_hidden: {text}");
        assert!(!text.contains("머리말텍스트"), "with_hidden만: {text}");

        let header = tool_read(
            &json!({"path": source, "format": "markdown", "with_header_footer": true}),
            &ctx(),
        )
        .unwrap();
        let text = header[0]["text"].as_str().unwrap();
        assert!(text.contains("머리말텍스트"), "with_header_footer: {text}");
        assert!(!text.contains("숨은메모"), "with_header_footer만: {text}");
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_read_with_segments_envelope_and_absolute_offsets() {
        let source = temp_file("read-segments.hwpx");
        create_hwpx(&source, "가나다\n\n마바사");

        let full = tool_read(
            &json!({"path": source, "format": "markdown", "with_segments": true}),
            &ctx(),
        )
        .unwrap();
        let envelope: Value =
            serde_json::from_str(full[0]["text"].as_str().unwrap()).expect("JSON 봉투");
        let markdown = envelope["markdown"].as_str().unwrap();
        assert!(
            markdown.contains("가나다") && markdown.contains("마바사"),
            "{markdown}"
        );
        let full_segments = envelope["segments"].as_array().unwrap();
        assert!(
            full_segments.len() >= 2,
            "문단별 세그먼트: {full_segments:?}"
        );

        // A 2-character window from the middle of the second paragraph ("바") — segment offsets must stay absolute against the full markdown.
        let offset = markdown.find("바").unwrap();
        let window = tool_read(
            &json!({
                "path": source,
                "format": "markdown",
                "with_segments": true,
                "offset": offset,
                "max_bytes": 6
            }),
            &ctx(),
        )
        .unwrap();
        let windowed: Value = serde_json::from_str(window[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(windowed["markdown"].as_str().unwrap(), "바사");
        let window_segments = windowed["segments"].as_array().unwrap();
        assert!(!window_segments.is_empty(), "창과 교차하는 세그먼트");
        assert!(
            window_segments.len() < full_segments.len(),
            "창 밖 세그먼트는 걸러야: {window_segments:?}"
        );
        // Proof of absolute offsets: window-relative remapping would put start near 0,
        // but absolute values point at the character position in the full markdown.
        let char_start = markdown[..offset].chars().count();
        let char_end = char_start + 2;
        for segment in window_segments {
            let start = segment["start"].as_u64().unwrap() as usize;
            let end = segment["end"].as_u64().unwrap() as usize;
            assert!(
                start < char_end && end > char_start,
                "창 [{char_start}, {char_end})과 교차해야: {segment}"
            );
            assert!(
                full_segments.contains(segment),
                "오프셋이 전체 기준 절대값으로 동일해야: {segment}"
            );
        }
        // Pagination metadata is kept as the second content item.
        let metadata: Value = serde_json::from_str(window[1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["truncated"], true);
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_read_with_segments_requires_markdown() {
        let source = temp_file("read-segments-reject.hwpx");
        create_hwpx(&source, "본문");
        for format in ["plain", "json", "html", "csv"] {
            let error = tool_read(
                &json!({"path": source, "format": format, "with_segments": true}),
                &ctx(),
            )
            .unwrap_err();
            assert!(error.contains("markdown"), "{format}: {error}");
        }
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_convert_to_overrides_extension_and_accepts_markdown_options() {
        let source = temp_file("convert-to-source.hwpx");
        let output = temp_file("convert-to-output.txt");
        create_hwpx(&source, "본문");
        // to=json wins over the .txt extension (same precedence as the CLI --to).
        tool_convert(
            &json!({
                "input": source,
                "output": output,
                "to": "json",
                "media_dir": "convert-to-media",
                "with_header_footer": true,
                "with_hidden": true
            }),
            &ctx(),
        )
        .expect("to 명시 변환");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).unwrap()).expect("JSON 출력");
        assert!(written["sections"].is_array(), "IR JSON: {written}");
        for path in [&source, &output] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_convert_request_merges_font_dirs_and_maps_args() {
        // Seam verified without rendering: a pdf-target conversion receives the startup/per-call font directories.
        let sandbox_ctx = LocalFsContext::new(vec![PathBuf::from("/launch-fonts")], Vec::new());
        let request = convert_request(
            &json!({
                "input": "in.hwpx",
                "output": "out.pdf",
                "to": "pdf",
                "font_dir": "/call-fonts",
                "media_dir": "media",
                "with_header_footer": true,
                "with_hidden": true
            }),
            &sandbox_ctx,
        )
        .expect("convert_request");
        assert!(matches!(request.to, Some(hwp_cli::cli::ConvertFormat::Pdf)));
        assert_eq!(
            request.font_dirs,
            vec![PathBuf::from("/launch-fonts"), PathBuf::from("/call-fonts")],
            "PDF 변환은 병합된 폰트 디렉터리를 받아야 한다"
        );
        assert_eq!(request.media_dir, Some(PathBuf::from("media")));
        assert!(request.with_header_footer && request.with_hidden);

        let error = convert_request(
            &json!({"input": "in.hwpx", "output": "out.x", "to": "bogus"}),
            &ctx(),
        )
        .unwrap_err();
        assert!(error.contains("to"), "{error}");
    }

    #[test]
    fn mcp_convert_media_dir_is_sandbox_checked() {
        let (base, root, outside) = sandbox_dirs("convert-media");
        let source = root.join("source.hwpx");
        create_hwpx(&source, "본문");
        let sandbox = ctx_with_roots(vec![canonicalize_mcp_path(&root).unwrap()]);
        let error = convert_request(
            &json!({
                "input": source,
                "output": root.join("out.md"),
                "media_dir": outside.join("media")
            }),
            &sandbox,
        )
        .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Synthetic 3-page document (same break_type trick as the existing mcp_render_rasterizes test).
    fn create_three_page_hwpx(path: &Path) {
        let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n");
        doc.sections[0].paragraphs[1].header.break_type |= 0x04;
        doc.sections[0].paragraphs[2].header.break_type |= 0x04;
        hwpx::write_document(&doc, path).unwrap();
    }

    #[test]
    fn mcp_render_rejects_conflicting_or_missing_page_args() {
        let source = temp_file("render-args.hwpx");
        // The document must have 3 pages for the "1-2" selection to be genuinely multi-page (parse_pages clamps to the page count).
        create_three_page_hwpx(&source);
        // page and pages are mutually exclusive.
        let error =
            tool_render(&json!({"path": source, "page": 1, "pages": "1-2"}), &ctx()).unwrap_err();
        assert!(error.contains("pages"), "{error}");
        // svg/pdf require output_path.
        for format in ["svg", "pdf"] {
            let error =
                tool_render(&json!({"path": source, "format": format}), &ctx()).unwrap_err();
            assert!(error.contains("output_path"), "{format}: {error}");
        }
        // Multi-page png requires output_path too.
        let error =
            tool_render(&json!({"path": source, "pages": "1-2", "dpi": 36}), &ctx()).unwrap_err();
        assert!(error.contains("output_path"), "{error}");
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_render_multi_page_png_writes_numbered_files_and_metadata() {
        let source = temp_file("render-multi-source.hwpx");
        let output = temp_file("render-multi.png");
        create_three_page_hwpx(&source);
        let content = tool_render(
            &json!({"path": source, "pages": "2-3", "dpi": 36, "output_path": output}),
            &ctx(),
        )
        .expect("다중 페이지 png");
        // Unlike the base64 path, this returns only JSON metadata without image content.
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let metadata: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["pages"], json!([2, 3]));
        assert_eq!(metadata["dpi"].as_f64(), Some(36.0));
        let page2 = output.with_file_name("render-multi-2.png");
        let page3 = output.with_file_name("render-multi-3.png");
        let files: Vec<PathBuf> = metadata["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| PathBuf::from(f.as_str().unwrap()))
            .collect();
        assert_eq!(files, vec![page2.clone(), page3.clone()]);
        for path in [&page2, &page3] {
            let bytes = std::fs::read(path).expect("페이지 파일 존재");
            assert!(
                bytes.starts_with(b"\x89PNG"),
                "PNG 매직: {}",
                path.display()
            );
        }
        assert!(!output.exists(), "다중 페이지는 번호 파일만 쓴다");
        for path in [&source, &page2, &page3] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_render_svg_and_pdf_write_files_with_output_path() {
        let source = temp_file("render-vector-source.hwpx");
        create_three_page_hwpx(&source);

        // A single-page svg is written to output_path as-is.
        let svg = temp_file("render-single.svg");
        let content = tool_render(
            &json!({"path": source, "format": "svg", "dpi": 36, "output_path": svg}),
            &ctx(),
        )
        .expect("svg 단일 페이지");
        let metadata: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["pages"], json!([1]));
        assert_eq!(metadata["files"], json!([svg.to_str().unwrap()]));
        let bytes = std::fs::read(&svg).unwrap();
        assert!(
            bytes.windows(4).any(|w| w == b"<svg"),
            "SVG 마크업: {}",
            svg.display()
        );

        // pdf writes the selected pages as a single multi-page file.
        let pdf = temp_file("render-all.pdf");
        let content = tool_render(
            &json!({"path": source, "format": "pdf", "pages": "all", "dpi": 36, "output_path": pdf}),
            &ctx(),
        )
        .expect("pdf 전체 페이지");
        let metadata: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["pages"], json!([1, 2, 3]));
        assert_eq!(metadata["files"], json!([pdf.to_str().unwrap()]));
        let bytes = std::fs::read(&pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF"), "PDF 매직: {}", pdf.display());
        for path in [&source, &svg, &pdf] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn mcp_grep_match_zero_match_and_ignore_case() {
        let source = temp_file("grep-source.hwpx");
        create_hwpx(&source, "사과 바나나\n\n둘째 사과\n\nHello World");

        let content =
            tool_grep(&json!({"path": source, "pattern": "사과"}), &ctx()).expect("grep 매칭");
        let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 2);
        assert_eq!(result["truncated"], false);
        assert_eq!(result["matches"].as_array().unwrap().len(), 2);

        // Zero matches are a normal result, not an error (isError=false).
        let mut zero_args = json!({"path": source, "pattern": "없는문구"});
        let zero = call_tool("hwp_grep", &mut zero_args, &ctx());
        assert_eq!(zero["isError"], false);
        let result: Value =
            serde_json::from_str(zero["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 0);
        assert_eq!(result["matches"].as_array().unwrap().len(), 0);

        // ignore_case ignores letter case.
        let content = tool_grep(
            &json!({"path": source, "pattern": "hello", "ignore_case": true}),
            &ctx(),
        )
        .unwrap();
        let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 1, "ignore_case 매칭: {result}");
        let content = tool_grep(&json!({"path": source, "pattern": "hello"}), &ctx()).unwrap();
        let result: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(result["count"], 0, "대소문자 구분: {result}");
        let _ = std::fs::remove_file(source);
    }

    #[test]
    fn mcp_grep_truncation_cap_logic() {
        let matches: Vec<String> = (0..201).map(|i| format!("m{i}")).collect();
        let capped = grep_result(matches);
        assert_eq!(capped["count"], 201, "count는 절단 전 전체 개수");
        assert_eq!(capped["truncated"], true);
        assert_eq!(
            capped["matches"].as_array().unwrap().len(),
            MAX_GREP_MATCHES
        );
        assert_eq!(MAX_GREP_MATCHES, 200);

        // Counts at or below the cap pass through unchanged.
        let uncapped = grep_result_capped(vec!["a".into(), "b".into()], 2);
        assert_eq!(uncapped["count"], 2);
        assert_eq!(uncapped["truncated"], false);
        let capped = grep_result_capped(vec!["a".into(), "b".into(), "c".into()], 2);
        assert_eq!(capped["count"], 3);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["matches"].as_array().unwrap().len(), 2);
    }

    /// 인라인 전송 왕복. 이 흐름이 R3의 존재 이유다 — 이 도구들이 생기기 전에는
    /// 원격 세션에 기존 문서를 넣을 방법이 프로토콜 안에 없었다.
    #[test]
    fn 인라인_전송_왕복() {
        let root = temp_file("inline-roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ctx_with_roots(vec![root.canonicalize().unwrap()]);

        let source = root.join("source.hwpx");
        create_hwpx(&source, "# 인라인 왕복\n\n본문 한 줄.");
        let original = std::fs::read(&source).unwrap();
        let encoded = hwp_convert::base64::encode(&original);

        let put = tool_put_file(
            &json!({"name": root.join("uploaded.hwpx").display().to_string(), "content": encoded}),
            &ctx,
        )
        .expect("put");
        let receipt: Value = serde_json::from_str(put[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(receipt["bytes"], original.len());

        // 올린 파일이 다른 도구에 그대로 보인다: 경로 인자를 유지한다는 doc 22 §7의 모델.
        let info = tool_info(
            &json!({"path": root.join("uploaded.hwpx").display().to_string()}),
            &ctx,
        );
        assert!(info.is_ok(), "업로드한 문서를 hwp_info가 읽지 못했습니다");

        let got = tool_get_file(
            &json!({"path": root.join("uploaded.hwpx").display().to_string()}),
            &ctx,
        )
        .expect("get");
        assert_eq!(got.len(), 2, "영수증과 resource 두 블록이어야 합니다");
        assert_eq!(got[1]["resource"]["mimeType"], "application/hwp+zip");
        let blob = got[1]["resource"]["blob"].as_str().unwrap();
        assert_eq!(
            hwp_convert::base64::decode(blob).unwrap(),
            original,
            "왕복 후 바이트가 달라졌습니다"
        );
        let got_receipt: Value = serde_json::from_str(got[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(got_receipt["sha256"], receipt["sha256"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 상한 초과는 복호 **전에** 걸러야 한다. 걸러지지 않으면 요청 하나가 상한을
    /// 훨씬 넘는 메모리를 잡으므로, 파일이 생기지 않았다는 것이 그 증거다.
    #[test]
    fn 인라인_업로드_상한() {
        let root = temp_file("inline-cap");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ctx_with_roots(vec![root.canonicalize().unwrap()]);
        let destination = root.join("too-big.bin");

        let oversized = "A".repeat(MAX_INLINE_CONTENT_B64_CHARS + 1);
        let error = tool_put_file(
            &json!({"name": destination.display().to_string(), "content": oversized}),
            &ctx,
        )
        .expect_err("상한을 넘겼는데 통과했습니다");
        assert!(error.contains("너무 큽니다"), "{error}");
        assert!(
            !destination.exists(),
            "복호 전에 거절해야 하므로 파일이 생기면 안 됩니다"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 상한을 넘는 파일은 잘라내지 않고 거절하며, 파일 자체는 남는다.
    #[test]
    fn 인라인_다운로드_상한() {
        let root = temp_file("inline-get-cap");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ctx_with_roots(vec![root.canonicalize().unwrap()]);

        let big = root.join("big.bin");
        std::fs::write(&big, vec![0u8; MAX_INLINE_CONTENT_BYTES + 1]).unwrap();
        let error = tool_get_file(&json!({"path": big.display().to_string()}), &ctx)
            .expect_err("상한을 넘겼는데 통과했습니다");
        assert!(error.contains("인라인 상한"), "{error}");
        assert!(big.exists(), "거절이 파일을 지우면 안 됩니다");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 잘못된 인자와 sandbox 이탈은 다른 도구와 같은 방식으로 거절한다.
    #[test]
    fn 인라인_전송_인자와_sandbox_검사() {
        let root = temp_file("inline-guards");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ctx_with_roots(vec![root.canonicalize().unwrap()]);

        let unknown = tool_put_file(
            &json!({"name": "a.bin", "content": "QQ==", "extra": 1}),
            &ctx,
        )
        .expect_err("알 수 없는 인자를 통과시켰습니다");
        assert!(
            unknown.contains("알 수 없는 hwp_put_file 인자"),
            "{unknown}"
        );

        let bad_base64 = tool_put_file(
            &json!({"name": root.join("a.bin").display().to_string(), "content": "not base64!"}),
            &ctx,
        )
        .expect_err("잘못된 base64를 통과시켰습니다");
        assert!(!bad_base64.is_empty());

        // root 밖으로 나가는 이름은 checked_write_path 가 막는다.
        for escape in ["../escape.bin", "/tmp/escape.bin"] {
            assert!(
                tool_put_file(&json!({"name": escape, "content": "QQ=="}), &ctx).is_err(),
                "sandbox 이탈이 허용되었습니다: {escape}"
            );
        }
        assert!(
            tool_get_file(&json!({"path": "../escape.bin"}), &ctx).is_err(),
            "읽기에서 sandbox 이탈이 허용되었습니다"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 공유 디코더는 `"A"`와 `"===="`를 빈 벡터로 통과시킨다. 전송 도구에서 그것은
    /// 0바이트 파일을 업로드 성공으로 보고한다는 뜻이므로 구조 검사가 먼저 막는다.
    #[test]
    fn 인라인_업로드_잘못된_base64_거절() {
        let root = temp_file("inline-b64");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ctx_with_roots(vec![root.canonicalize().unwrap()]);

        // 디코더 자체는 이것들을 조용히 통과시킨다는 사실을 먼저 고정한다.
        assert_eq!(hwp_convert::base64::decode("A").unwrap(), Vec::<u8>::new());
        assert_eq!(
            hwp_convert::base64::decode("====").unwrap(),
            Vec::<u8>::new()
        );

        for bad in ["", "   ", "A", "====", "QQ", "QUJD="] {
            let destination = root.join(format!("bad-{}.bin", bad.escape_debug()));
            let error = tool_put_file(
                &json!({"name": destination.display().to_string(), "content": bad}),
                &ctx,
            )
            .unwrap_err();
            assert!(
                error.contains("잘못된 base64") || error.contains("비어 있습니다"),
                "{bad}: {error}"
            );
            assert!(!destination.exists(), "{bad}: 파일이 생기면 안 됩니다");
        }

        // 정상 입력은 그대로 통과한다.
        assert!(validate_base64_structure("QUJD").is_ok());
        assert!(validate_base64_structure("QQ==").is_ok());
        assert!(validate_base64_structure("QUJ=").is_ok());
        assert!(validate_base64_structure("QUJD\nRUZH").is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 문자 장치는 metadata가 0바이트라고 보고하므로, 크기 검사만 믿으면 읽기가
    /// 끝나지 않는다. root 제한이 없는 stdio에서 실제로 도달 가능한 경로였다.
    #[test]
    #[cfg(unix)]
    fn 인라인_다운로드_일반_파일만() {
        // roots가 비어 있으면 checked_read_path가 아무 경로나 통과시킨다 — 이것이
        // 이 검사가 필요한 조건 그대로다.
        let ctx = ctx_with_roots(Vec::new());
        let error = tool_get_file(&json!({"path": "/dev/zero"}), &ctx)
            .expect_err("/dev/zero 를 읽으려 시도했습니다");
        assert!(error.contains("일반 파일이 아닙니다"), "{error}");
    }

    /// resource uri는 클라이언트가 URI로 파싱하므로 공백·비ASCII·윈도우 경로에서도
    /// 유효해야 한다.
    #[test]
    fn 인라인_다운로드_uri_인코딩() {
        assert_eq!(
            file_uri(Path::new("/work/보고서 최종.hwpx")),
            "file:///work/%EB%B3%B4%EA%B3%A0%EC%84%9C%20%EC%B5%9C%EC%A2%85.hwpx"
        );
        assert_eq!(
            file_uri(Path::new("/work/a#b.pdf")),
            "file:///work/a%23b.pdf"
        );
        // 슬래시로 시작하지 않는 경로는 빈 authority 뒤에 루트를 세운다.
        let windows = file_uri(Path::new(r"C:\work\out.hwpx"));
        assert!(
            windows == "file:///C%3A/work/out.hwpx",
            "윈도우 경로 URI: {windows}"
        );
    }
}
