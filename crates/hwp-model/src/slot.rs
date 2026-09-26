//! The `{{ name }}` text-slot grammar.
//!
//! `hwp slots` scans the IR and `hwp fill` rewrites raw section XML, so the two read a slot from
//! different text. Both parse it here so they cannot drift apart again (#362): `{{`, optional
//! whitespace, a name, optional whitespace, `}}`. The name is one or more alphanumerics, `.`, `-`
//! or `_`, and the token is the whole `{{ ... }}` span, padding included.

use std::ops::Range;

/// One slot token found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotToken<'a> {
    /// Byte range of the whole `{{ ... }}` token in the scanned text.
    pub range: Range<usize>,
    /// The name, without the padding.
    pub name: &'a str,
}

/// A character a slot name may hold.
pub fn is_slot_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '-' | '_')
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
        let rest = &text[body..];
        let name_start = body + (rest.len() - rest.trim_start().len());
        let name_len = text[name_start..]
            .find(|c: char| !is_slot_name_char(c))
            .unwrap_or(text.len() - name_start);
        let after_name = &text[name_start + name_len..];
        let close = name_start + name_len + (after_name.len() - after_name.trim_start().len());
        if name_len > 0 && text[close..].starts_with("}}") {
            tokens.push(SlotToken {
                range: open..close + 2,
                name: &text[name_start..name_start + name_len],
            });
            from = close + 2;
        } else {
            from = open + 1;
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<&str> {
        slot_tokens(text).into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn padded_and_unpadded_spellings_share_one_name() {
        let text = "{{제목}} {{ 제목 }} {{\u{3000}제목\t}}";
        let tokens = slot_tokens(text);
        assert_eq!(names(text), ["제목", "제목", "제목"]);
        assert_eq!(&text[tokens[1].range.clone()], "{{ 제목 }}");
    }

    #[test]
    fn a_name_with_inner_space_or_punctuation_is_not_a_slot() {
        assert!(names("{{a b}} {{a,b}} {{}} {{  }} {{a}").is_empty());
        assert_eq!(names("{{a.b-c_d1}}"), ["a.b-c_d1"]);
    }

    #[test]
    fn a_stray_open_brace_does_not_hide_the_next_slot() {
        assert_eq!(names("{{ {{이름}}"), ["이름"]);
        assert_eq!(names("{{{a}}}"), ["a"]);
        let text = "x{{{a}}}";
        assert_eq!(&text[slot_tokens(text)[0].range.clone()], "{{a}}");
    }
}
