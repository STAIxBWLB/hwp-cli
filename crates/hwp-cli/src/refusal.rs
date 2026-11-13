//! Machine-readable refusal object for `edit-report-v1` 1.1 and the MCP `hwp_edit` error result
//! (#531). A refusal carries a stable snake_case `code`, the structured inputs behind it in
//! `values` (user-typed strings live here, never spliced into a code or a path string), and the
//! human sentence in `message`, which is exactly what stderr prints.
//!
//! Classification never parses free prose that may contain user input. A code is assigned only
//! where the refusing call site is known (the op kind, the function that refused), or by exact
//! equality with a fixed refusal constant. Anything else is `unclassified`.

use serde::Serialize;
use serde_json::{Map, Value};

/// Every code `edit-report-v1` 1.1 may carry in `refusal.code`. Keep this list, the schema
/// enum and the manual tables in step; `refusal_codes_match_schema` pins the first two.
pub const REFUSAL_CODES: &[&str] = &[
    RANGE_TAG_SHIFT,
    KEEP_TOGETHER_UNSUPPORTED,
    SECTION_OWNER_INSERT,
    SECTION_OWNER_MOVE,
    ADD_TABLE_ANCHOR_NOT_FOUND,
    INSERT_BLOCKS_UNKNOWN_STYLE,
    INSERT_BLOCKS_INVALID_DEFINITION,
    INSERT_BLOCKS_STYLE_NOT_PARAGRAPH,
    INSERT_BLOCKS_DUPLICATE_STYLE,
    INSERT_BLOCKS_STYLE_CYCLE,
    INSERT_BLOCKS_STYLE_ID_OVERFLOW,
    LAYOUT_OVERFLOW,
    INSERT_BLOCKS_INVALID_SPEC,
    PRESERVATION_LOSS,
    UNAPPLIED_REQUESTS,
    NO_APPLICABLE_EDITS,
    UNCLASSIFIED,
];

pub const RANGE_TAG_SHIFT: &str = "range_tag_shift";
pub const KEEP_TOGETHER_UNSUPPORTED: &str = "keep_together_unsupported";
pub const SECTION_OWNER_INSERT: &str = "section_owner_insert";
pub const SECTION_OWNER_MOVE: &str = "section_owner_move";
pub const ADD_TABLE_ANCHOR_NOT_FOUND: &str = "add_table.anchor_not_found";
pub const INSERT_BLOCKS_UNKNOWN_STYLE: &str = "insert_blocks.unknown_style";
pub const INSERT_BLOCKS_INVALID_DEFINITION: &str = "insert_blocks.invalid_definition";
pub const INSERT_BLOCKS_STYLE_NOT_PARAGRAPH: &str = "insert_blocks.style_not_paragraph";
pub const INSERT_BLOCKS_DUPLICATE_STYLE: &str = "insert_blocks.duplicate_style";
pub const INSERT_BLOCKS_STYLE_CYCLE: &str = "insert_blocks.style_cycle";
pub const INSERT_BLOCKS_STYLE_ID_OVERFLOW: &str = "insert_blocks.style_id_overflow";
pub const LAYOUT_OVERFLOW: &str = "layout_overflow";
pub const INSERT_BLOCKS_INVALID_SPEC: &str = "insert_blocks.invalid_spec";
pub const PRESERVATION_LOSS: &str = "preservation_loss";
pub const UNAPPLIED_REQUESTS: &str = "unapplied_requests";
pub const NO_APPLICABLE_EDITS: &str = "no_applicable_edits";
pub const UNCLASSIFIED: &str = "unclassified";

/// The fixed, content-free sentence `refusal.message` carries for each code (#348, #531).
/// It never interpolates request text, patterns, anchors, URLs, names or error chains; the
/// structured inputs live in `values`, and the full prose goes to stderr only.
pub fn fixed_message(code: &str) -> &'static str {
    match code {
        RANGE_TAG_SHIFT => {
            "편집이 해석하지 않은 범위 태그(PARA_RANGE_TAG)를 옮기거나 지우게 되어 거부했습니다"
        }
        KEEP_TOGETHER_UNSUPPORTED => "keep_together는 아직 지원하지 않습니다",
        SECTION_OWNER_INSERT => "구역 설정 문단 앞에 삽입하게 되어 거부했습니다",
        SECTION_OWNER_MOVE => "구역 설정 문단 앞으로 이동하게 되어 거부했습니다",
        ADD_TABLE_ANCHOR_NOT_FOUND => "add_table 앵커를 담은 최상위 문단이 없습니다",
        INSERT_BLOCKS_UNKNOWN_STYLE => "insert_blocks가 쓰는 스타일이 대상 문서에 없습니다",
        INSERT_BLOCKS_INVALID_DEFINITION => {
            "insert_blocks 스타일 또는 목록 정의가 올바르지 않습니다"
        }
        INSERT_BLOCKS_STYLE_NOT_PARAGRAPH => "insert_blocks 대상 스타일이 문단 스타일이 아닙니다",
        INSERT_BLOCKS_DUPLICATE_STYLE => "대상 문서에 같은 이름의 스타일이 둘 이상 있습니다",
        INSERT_BLOCKS_STYLE_CYCLE => "스타일의 based_on이 순환합니다",
        INSERT_BLOCKS_STYLE_ID_OVERFLOW => "스타일 번호가 HWP의 1바이트 범위를 넘습니다",
        LAYOUT_OVERFLOW => "삽입할 블록의 너비가 삽입 위치의 너비를 넘습니다",
        INSERT_BLOCKS_INVALID_SPEC => "insert_blocks 블록 정의가 올바르지 않습니다",
        PRESERVATION_LOSS => "보존할 수 없는 데이터를 지우거나 바꾸게 되어 거부했습니다",
        UNAPPLIED_REQUESTS => "적용되지 않은 편집 요청이 있습니다",
        NO_APPLICABLE_EDITS => "문서를 바꾼 편집 요청이 없습니다",
        _ => "편집이 거부되었습니다",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Refusal {
    pub code: &'static str,
    pub values: Map<String, Value>,
    /// [`fixed_message`] for `code`: what edit-report-v1 and MCP `structuredContent` carry.
    pub message: &'static str,
    /// The full human sentence for stderr and the MCP error text. Never serialized, since it
    /// may quote request content (#348).
    #[serde(skip)]
    pub prose: String,
}

impl Refusal {
    pub fn new(code: &'static str, prose: impl Into<String>) -> Self {
        Self {
            code,
            values: Map::new(),
            message: fixed_message(code),
            prose: prose.into(),
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.values.insert(key.to_owned(), value.into());
        self
    }

    pub fn unclassified(message: impl Into<String>) -> Self {
        Self::new(UNCLASSIFIED, message)
    }

    /// Classifies a `hwp-convert` String error by exact equality with its fixed refusal
    /// constants. Equality (not `contains`) keeps user text that merely quotes a constant
    /// from borrowing its code.
    pub fn from_convert_message(message: impl Into<String>) -> Self {
        let message = message.into();
        let code = if message == hwp_convert::RANGE_TAG_SHIFT_REFUSAL {
            RANGE_TAG_SHIFT
        } else if message == hwp_convert::SECTION_OWNER_INSERT_REFUSAL {
            SECTION_OWNER_INSERT
        } else if message == hwp_convert::SECTION_OWNER_MOVE_REFUSAL {
            SECTION_OWNER_MOVE
        } else {
            UNCLASSIFIED
        };
        Self::new(code, message)
    }

    /// Finds a [`Refusal`] anywhere in an error chain (context layers included).
    pub fn find_in(error: &anyhow::Error) -> Option<Self> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Refusal>())
            .cloned()
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.prose)
    }
}

impl std::error::Error for Refusal {}

/// A String error from a step that has no fixed refusal class.
impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Self::unclassified(message)
    }
}

impl From<&str> for Refusal {
    fn from(message: &str) -> Self {
        Self::unclassified(message)
    }
}

/// Splits a DocumentSpec issue path such as `$.styles.<name>.based_on` or
/// `$.sections[0].blocks[2].width_mm` into segments. Style and list names may contain `.`, `;`
/// or anything else, so a name segment is matched against the known names (longest first)
/// rather than split on dots. Array indices become their own numeric segments.
pub fn split_spec_path(path: &str, names: &[&str]) -> Vec<Value> {
    let mut segments = Vec::new();
    let Some(mut rest) = path.strip_prefix('$') else {
        return vec![Value::String(path.to_owned())];
    };
    segments.push(Value::String("$".to_owned()));
    let mut previous: Option<String> = None;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let Some(end) = after.find(']') else {
                segments.push(Value::String(rest.to_owned()));
                break;
            };
            let inner = &after[..end];
            segments.push(
                inner
                    .parse::<u64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(inner.to_owned())),
            );
            rest = &after[end + 1..];
            continue;
        }
        let Some(after) = rest.strip_prefix('.') else {
            segments.push(Value::String(rest.to_owned()));
            break;
        };
        let keyed = matches!(previous.as_deref(), Some("styles" | "lists"));
        let name = keyed
            .then(|| {
                let mut candidates: Vec<&&str> = names
                    .iter()
                    .filter(|name| {
                        after.starts_with(**name)
                            && matches!(after[name.len()..].chars().next(), None | Some('.' | '['))
                    })
                    .collect();
                candidates.sort_by_key(|name| std::cmp::Reverse(name.len()));
                candidates.first().map(|name| **name)
            })
            .flatten();
        let segment = match name {
            Some(name) => name,
            None => {
                let end = after.find(['.', '[']).unwrap_or(after.len());
                &after[..end]
            }
        };
        segments.push(Value::String(segment.to_owned()));
        previous = Some(segment.to_owned());
        rest = &after[segment.len()..];
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_names_with_separators_stay_one_segment() {
        let name = "a; layout_overflow at $.z: b";
        let path = format!("$.styles.{name}.based_on");
        assert_eq!(
            split_spec_path(&path, &[name, "a"]),
            vec![
                Value::from("$"),
                Value::from("styles"),
                Value::from(name),
                Value::from("based_on")
            ]
        );
        assert_eq!(
            split_spec_path("$.sections[0].blocks[2].width_mm", &[]),
            vec![
                Value::from("$"),
                Value::from("sections"),
                Value::from(0),
                Value::from("blocks"),
                Value::from(2),
                Value::from("width_mm")
            ]
        );
    }

    #[test]
    fn convert_constants_classify_only_by_equality() {
        assert_eq!(
            Refusal::from_convert_message(hwp_convert::RANGE_TAG_SHIFT_REFUSAL).code,
            RANGE_TAG_SHIFT
        );
        let quoted = format!(
            "앵커를 찾을 수 없습니다: {}",
            hwp_convert::RANGE_TAG_SHIFT_REFUSAL
        );
        assert_eq!(Refusal::from_convert_message(quoted).code, UNCLASSIFIED);
    }

    #[test]
    fn serialized_message_is_the_fixed_sentence() {
        let refusal = Refusal::new(UNAPPLIED_REQUESTS, "--replace \"secret\"=>\"x\"");
        let json = serde_json::to_string(&refusal).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert_eq!(refusal.to_string(), "--replace \"secret\"=>\"x\"");
        for code in REFUSAL_CODES {
            assert!(!fixed_message(code).is_empty());
        }
    }

    #[test]
    fn refusal_codes_match_schema() {
        let schema: Value =
            serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json"))
                .unwrap();
        let listed: Vec<&str> = schema["$defs"]["refusal"]["properties"]["code"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_str().unwrap())
            .collect();
        assert_eq!(listed, REFUSAL_CODES);
    }
}
