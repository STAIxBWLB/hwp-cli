//! quick-xml 보조 유틸.

use quick_xml::events::BytesStart;

/// 로컬 이름(네임스페이스 접두사 제거) 기준 속성 조회.
/// 속성값을 **엔티티 해석까지 마친** 문자열로 돌려준다. quick-xml은 속성값을 파일에 있던
/// 이스케이프 형태 그대로 주므로, 풀지 않으면 `name="A&amp;B"`가 IR에 `A&amp;B`로 올라가고
/// writer의 `esc()`가 다시 감싸 `A&amp;amp;B`로 이중 이스케이프된다(책갈피·필드 이름,
/// 수식 script 속성에서 실제로 발생). 해석 실패(잘못된 참조)면 원문을 그대로 둔다.
pub fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        let key = a.key.local_name();
        if key.as_ref() == name.as_bytes() {
            let raw = String::from_utf8_lossy(&a.value).into_owned();
            Some(
                quick_xml::escape::unescape(&raw)
                    .map(|s| s.into_owned())
                    .unwrap_or(raw),
            )
        } else {
            None
        }
    })
}

pub fn attr_u32(e: &BytesStart<'_>, name: &str) -> Option<u32> {
    attr(e, name)?.parse().ok()
}

pub fn attr_i32(e: &BytesStart<'_>, name: &str) -> Option<i32> {
    attr(e, name)?.parse().ok()
}

/// 오프셋 등 부호 있는 32비트 속성. hwpx는 음수를 unsigned 2의보수 십진수로
/// 저장(예: -77 = "4294967219")하므로 i64로 파싱 후 i32로 재해석한다.
pub fn attr_offset_i32(e: &BytesStart<'_>, name: &str) -> Option<i32> {
    attr(e, name)?.parse::<i64>().ok().map(|v| v as i32)
}

pub fn attr_u16(e: &BytesStart<'_>, name: &str) -> Option<u16> {
    attr(e, name)?.parse().ok()
}

/// "#RRGGBB" → COLORREF(0x00BBGGRR). "none"/파싱 실패는 0xFFFF_FFFF.
pub fn parse_color(s: &str) -> u32 {
    let hex = s.strip_prefix('#').unwrap_or(s);
    if hex.len() == 6
        && let Ok(rgb) = u32::from_str_radix(hex, 16)
    {
        let r = (rgb >> 16) & 0xFF;
        let g = (rgb >> 8) & 0xFF;
        let b = rgb & 0xFF;
        return (b << 16) | (g << 8) | r;
    }
    0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 색_변환() {
        assert_eq!(parse_color("#FF0000"), 0x0000_00FF); // 빨강 → BGR
        assert_eq!(parse_color("#000000"), 0);
        assert_eq!(parse_color("none"), 0xFFFF_FFFF);
    }

    /// 속성값 엔티티는 읽는 즉시 풀린다 — 안 풀면 writer의 esc()가 다시 감싸
    /// `A&amp;B` → `A&amp;amp;B`로 왕복마다 이중 이스케이프가 쌓인다.
    #[test]
    fn 속성값_엔티티_해석() {
        let el = quick_xml::events::BytesStart::from_content(
            r#"hp:bookmark name="A&amp;B &lt;중&gt; &#48124;" bad="A&nosuch;B""#,
            12,
        );
        assert_eq!(attr(&el, "name").as_deref(), Some("A&B <중> 민")); // &#48124; = U+BBFC
        // 해석 불가한 참조는 원문 유지(정보 손실 없이 그대로 되쓴다).
        assert_eq!(attr(&el, "bad").as_deref(), Some("A&nosuch;B"));
    }
}
