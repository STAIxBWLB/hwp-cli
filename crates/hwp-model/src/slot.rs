//! The `{{ name }}` text-slot grammar.
//!
//! `hwp slots` scans the IR and `hwp fill` rewrites raw section XML, so the two read a slot from
//! different text. Both parse it here so they cannot drift apart again (#362). A slot is `{{`,
//! a name, `}}`. The name is any run of characters other than `{`, `}` and control characters,
//! with surrounding whitespace trimmed, and must not be empty once trimmed. This is the name
//! rule TemplateSpec reference bindings already accept. The token is the whole `{{ ... }}` span.

use std::collections::BTreeMap;
use std::ops::Range;

/// One slot token found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotToken<'a> {
    /// Byte range of the whole `{{ ... }}` token in the scanned text.
    pub range: Range<usize>,
    /// The name, without the surrounding whitespace.
    pub name: &'a str,
}

/// A character a slot name may hold.
pub fn is_slot_name_char(c: char) -> bool {
    !matches!(c, '{' | '}') && !c.is_control()
}

/// Every slot token in `text`, left to right and non-overlapping.
///
/// A `{{` that does not open a well-formed token is skipped one byte at a time, so a stray
/// `{{` before a real slot (`{{ {{이름}}`) does not hide it.
pub fn slot_tokens(text: &str) -> Vec<SlotToken<'_>> {
    let mut tokens = Vec::new();
    let mut from = 0usize;
    while let Some(hit) = text[from..].find("{{") {
        let open = from + hit;
        let body = open + 2;
        let close = text[body..]
            .find(|c: char| !is_slot_name_char(c))
            .map_or(text.len(), |len| body + len);
        let name = text[body..close].trim();
        if !name.is_empty() && text[close..].starts_with("}}") {
            tokens.push(SlotToken {
                range: open..close + 2,
                name,
            });
            from = close + 2;
        } else {
            from = open + 1;
        }
    }
    tokens
}

/// One requested slot: every key that names it, as the caller spelled them (each is counted
/// for the tokens the slot fills), and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRequest<'a, V> {
    pub keys: Vec<&'a str>,
    pub value: &'a V,
}

/// Index requested slot values by slot name, each key trimmed the way a name is. A key that
/// trims to nothing names no slot and is left out. Keys that name one slot are accepted when
/// their values are equal and refused when they differ, since only one value could win.
pub fn slot_lookup<V: PartialEq>(
    values: &BTreeMap<String, V>,
) -> Result<BTreeMap<&str, SlotRequest<'_, V>>, String> {
    let mut lookup: BTreeMap<&str, SlotRequest<'_, V>> = BTreeMap::new();
    for (key, value) in values {
        let name = key.trim();
        if name.is_empty() {
            continue;
        }
        match lookup.get_mut(name) {
            Some(request) if request.value == value => request.keys.push(key),
            Some(request) => {
                return Err(format!(
                    "two requested names are the same slot once trimmed, with different values: \
                     {:?}, {key:?}",
                    request.keys[0]
                ));
            }
            None => {
                lookup.insert(
                    name,
                    SlotRequest {
                        keys: vec![key.as_str()],
                        value,
                    },
                );
            }
        }
    }
    Ok(lookup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<&str> {
        slot_tokens(text).into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn padded_and_unpadded_spellings_share_one_name() {
        let text = "{{제목}} {{ 제목 }} {{\u{3000}제목  }}";
        let tokens = slot_tokens(text);
        assert_eq!(names(text), ["제목", "제목", "제목"]);
        assert_eq!(&text[tokens[1].range.clone()], "{{ 제목 }}");
    }

    /// Any name TemplateSpec accepts is a slot: spaces, parentheses, `·`, `:`, `/` included.
    #[test]
    fn a_name_is_any_text_without_braces_or_controls() {
        assert_eq!(
            names("{{성 명}} {{사업명(국문)}} {{가·나}} {{기간: 시작}} {{a/b}} {{a.b-c_d1}}"),
            [
                "성 명",
                "사업명(국문)",
                "가·나",
                "기간: 시작",
                "a/b",
                "a.b-c_d1"
            ]
        );
        assert!(names("{{}} {{  }} {{a}").is_empty());
        assert!(names("{{a\nb}} {{a\tb}} {{a\u{7f}}}").is_empty());
    }

    #[test]
    fn a_stray_open_brace_does_not_hide_the_next_slot() {
        assert_eq!(names("{{ {{이름}}"), ["이름"]);
        assert_eq!(names("{{{a}}}"), ["a"]);
        let text = "x{{{a}}}";
        assert_eq!(&text[slot_tokens(text)[0].range.clone()], "{{a}}");
    }

    #[test]
    fn lookup_trims_keys_and_refuses_only_conflicting_values() {
        let values = BTreeMap::from([(" 제목 ".to_string(), 1), ("  ".to_string(), 2)]);
        let lookup = slot_lookup(&values).unwrap();
        assert_eq!(lookup.len(), 1);
        assert_eq!(lookup["제목"].keys, [" 제목 "]);
        let same = BTreeMap::from([(" 제목".to_string(), 1), ("제목".to_string(), 1)]);
        assert_eq!(slot_lookup(&same).unwrap()["제목"].keys, [" 제목", "제목"]);
        let differ = BTreeMap::from([(" 제목".to_string(), 1), ("제목".to_string(), 2)]);
        assert!(slot_lookup(&differ).is_err());
    }
}
