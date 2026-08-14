//! 패키지 보조 파일 템플릿.
//!
//! 한글에서 저장한 기준 표본(minimal.hwpx)의 구조를 코드로 재현한다 —
//! 바이너리/원문 임베드가 아니라 값을 옮긴 것이므로 라이선스 문제가 없다.

pub const MIMETYPE: &str = "application/hwp+zip";

// appVersion 은 작성 프로그램(hwp-cli) 버전 메타데이터라 크레이트 버전을 따른다
// (major/minor/micro/xmlVersion 등 포맷 호환 상수는 고정 — 절대 변경 금지).
pub const VERSION_XML: &str = concat!(
    r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="1" buildNumber="0" os="1" xmlVersion="1.5" application="hwp-cli" appVersion=""##,
    env!("CARGO_PKG_VERSION"),
    r##""/>"##
);

pub const CONTAINER_XML: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><ocf:container xmlns:ocf="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:hpf="http://www.hancom.co.kr/schema/2011/hpf"><ocf:rootfiles><ocf:rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/><ocf:rootfile full-path="Preview/PrvText.txt" media-type="text/plain"/><ocf:rootfile full-path="META-INF/container.rdf" media-type="application/rdf+xml"/></ocf:rootfiles></ocf:container>"##;

pub const CONTAINER_RDF: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about=""><ns0:hasPart xmlns:ns0="http://www.hancom.co.kr/hwpml/2016/meta/pkg#" rdf:resource="Contents/header.xml"/></rdf:Description><rdf:Description rdf:about="Contents/header.xml"><rdf:type rdf:resource="http://www.hancom.co.kr/hwpml/2016/meta/pkg#HeaderFile"/></rdf:Description><rdf:Description rdf:about=""><ns0:hasPart xmlns:ns0="http://www.hancom.co.kr/hwpml/2016/meta/pkg#" rdf:resource="Contents/section0.xml"/></rdf:Description><rdf:Description rdf:about="Contents/section0.xml"><rdf:type rdf:resource="http://www.hancom.co.kr/hwpml/2016/meta/pkg#SectionFile"/></rdf:Description><rdf:Description rdf:about=""><rdf:type rdf:resource="http://www.hancom.co.kr/hwpml/2016/meta/pkg#Document"/></rdf:Description></rdf:RDF>"##;

pub const MANIFEST_XML: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>"##;

pub const SETTINGS_XML: &str = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/app" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0"><ha:CaretPosition listIDRef="0" paraIDRef="0" pos="0"/></ha:HWPApplicationSetting>"##;

/// OWPML 공통 네임스페이스 선언 (head/content.hpf용 전체 세트).
pub const FULL_XMLNS: &str = r##"xmlns:ha="http://www.hancom.co.kr/hwpml/2011/app" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hp10="http://www.hancom.co.kr/hwpml/2016/paragraph" xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core" xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hhs="http://www.hancom.co.kr/hwpml/2011/history" xmlns:hm="http://www.hancom.co.kr/hwpml/2011/master-page" xmlns:hpf="http://www.hancom.co.kr/schema/2011/hpf" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf/" xmlns:ooxmlchart="http://www.hancom.co.kr/hwpml/2016/ooxmlchart" xmlns:hwpunitchar="http://www.hancom.co.kr/hwpml/2016/HwpUnitChar" xmlns:epub="http://www.idpf.org/2007/ops" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0""##;

/// content.hpf — manifest 항목(섹션 수, 바이너리 항목) + 문서 메타데이터를 끼워 만든다.
/// `extra_items`는 writer가 재생성하지 않는 확장 파트의 원본 매니페스트 항목
/// (id, href, media-type — 예: DocOptions/Layout.xml)으로, bin 항목 뒤에 그대로
/// 등재해 raw-copy된 패키지 엔트리가 고아 파트가 되지 않게 한다.
pub fn content_hpf(
    section_count: usize,
    bin_items: &[(String, String, String)],
    extra_items: &[(String, String, String)],
    meta: &hwp_model::Metadata,
) -> String {
    use std::fmt::Write as _;
    let mut manifest = String::new();
    let mut spine = String::new();
    let _ = write!(
        manifest,
        r##"<opf:item id="header" href="Contents/header.xml" media-type="application/xml"/>"##
    );
    let _ = write!(spine, r##"<opf:itemref idref="header" linear="yes"/>"##);
    for i in 0..section_count {
        let _ = write!(
            manifest,
            r##"<opf:item id="section{i}" href="Contents/section{i}.xml" media-type="application/xml"/>"##
        );
        let _ = write!(spine, r##"<opf:itemref idref="section{i}" linear="yes"/>"##);
    }
    let _ = write!(
        manifest,
        r##"<opf:item id="settings" href="settings.xml" media-type="application/xml"/>"##
    );
    for (id, href, mime) in bin_items {
        let _ = write!(
            manifest,
            r##"<opf:item id="{id}" href="{href}" media-type="{mime}" isEmbeded="1"/>"##
        );
    }
    // 확장 파트 원본 항목 — 값은 원문 속성(이스케이프 상태) 그대로라 esc 없이 방출한다.
    for (id, href, mime) in extra_items {
        let _ = write!(
            manifest,
            r##"<opf:item id="{id}" href="{href}" media-type="{mime}"/>"##
        );
    }
    // 정품 표본(content.hpf)의 metadata 형식·순서를 그대로 재현한다:
    //   title / language(ko) / creator / subject / description / lastsaveby /
    //   CreatedDate / ModifiedDate / date / keyword
    // - title은 <opf:title>...</opf:title>(빈 값이면 <opf:title/>)
    // - 나머지는 모두 <opf:meta name="..." content="text">값</opf:meta> 형식이며,
    //   값이 없으면 정품처럼 빈 요소(<opf:meta name="..." content="text"/>)로 방출.
    let title_el = match meta.title.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => format!("<opf:title>{}</opf:title>", esc(t)),
        None => "<opf:title/>".to_string(),
    };
    // creator: author 값. author가 None이면 앱 이름 "hwp-cli"를 유지(정품은 항상 값 보유,
    // 중복 creator 방출은 제거).
    let creator_val = meta
        .author
        .as_deref()
        .filter(|a| !a.is_empty())
        .unwrap_or("hwp-cli");
    let creator_el = meta_text_el("creator", Some(creator_val));
    let subject_el = meta_text_el("subject", meta.subject.as_deref());
    let description_el = meta_text_el("description", meta.description.as_deref());
    let lastsaveby_el = meta_text_el("lastsaveby", meta.last_saved_by.as_deref());
    // 날짜: create_time/modify_time(FILETIME raw u64)에서 파생.
    // CreatedDate/ModifiedDate는 ISO-8601 UTC, date는 한국어 KST(create 기준).
    let created_iso = meta
        .create_time
        .and_then(hwp_model::filetime_to_iso8601_utc);
    let modified_iso = meta
        .modify_time
        .and_then(hwp_model::filetime_to_iso8601_utc);
    let date_kst = meta.create_time.and_then(hwp_model::filetime_to_korean_kst);
    let created_el = meta_text_el("CreatedDate", created_iso.as_deref());
    let modified_el = meta_text_el("ModifiedDate", modified_iso.as_deref());
    let date_el = meta_text_el("date", date_kst.as_deref());
    let keyword_el = meta_text_el("keyword", meta.keywords.as_deref());
    format!(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><opf:package {FULL_XMLNS} version="" unique-identifier="" id=""><opf:metadata>{title_el}<opf:language>ko</opf:language>{creator_el}{subject_el}{description_el}{lastsaveby_el}{created_el}{modified_el}{date_el}{keyword_el}</opf:metadata><opf:manifest>{manifest}</opf:manifest><opf:spine>{spine}</opf:spine></opf:package>"##
    )
}

/// 정품 `<opf:meta name="{name}" content="text">값</opf:meta>` 형식 한 요소.
/// 값이 비어 있으면 빈 요소(`.../>`)로 방출한다(정품 표본이 빈 요소를 유지).
fn meta_text_el(name: &str, value: Option<&str>) -> String {
    match value.filter(|v| !v.is_empty()) {
        Some(v) => format!(
            "<opf:meta name=\"{name}\" content=\"text\">{}</opf:meta>",
            esc(v)
        ),
        None => format!("<opf:meta name=\"{name}\" content=\"text\"/>"),
    }
}

/// XML 텍스트/속성 이스케이프.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // XML 1.0에서 금지된 C0 제어문자(탭·개행·복귀 제외)는 제거한다 — 이스케이프
            // 해도 무효이고, raw로 방출하면 한글이 파일을 거부한다. 탭/개행은 상위
            // (flush_text)에서 이미 <hp:tab …/>·<hp:lineBreak/> 요소로 변환돼 여기 오지 않는다.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            // 비문자(U+FFFE·U+FFFF)도 XML 1.0 허용 범위 밖이다. 이걸 흘리면 well-formed
            // 하지 않은 패키지가 나와 파서·한글이 파일 전체를 거부한다(C0와 같은 처리).
            '\u{FFFE}' | '\u{FFFF}' => {}
            _ => out.push(c),
        }
    }
    out
}

/// COLORREF → "#rrggbb" (0xFFFFFFFF는 "none").
pub fn color_attr(c: u32) -> String {
    if c == 0xFFFF_FFFF {
        return "none".to_string();
    }
    format!(
        "#{:02X}{:02X}{:02X}",
        c & 0xFF,
        (c >> 8) & 0xFF,
        (c >> 16) & 0xFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::parse_content_meta;

    /// content.hpf에 요약정보 8필드를 정품 형식으로 방출하고 다시 읽어 보존되는지
    /// 왕복 검증. FILETIME은 초 단위 정밀도로 방출/파싱되며, 하위 100ns는 ISO 초
    /// 절사로 소실되므로 여기선 초 경계 값(FT_PER_SEC 배수)을 사용한다.
    #[test]
    fn content_hpf_요약정보_8필드_왕복() {
        // 2025-09-17T04:32:50Z / 2025-09-17T04:33:13Z (모두 초 경계).
        let created = hwp_model::iso8601_utc_to_filetime("2025-09-17T04:32:50Z").unwrap();
        let modified = hwp_model::iso8601_utc_to_filetime("2025-09-17T04:33:13Z").unwrap();
        let meta = hwp_model::Metadata {
            title: Some("제목 A=B=>C & <x>".into()),
            author: Some("지은이".into()),
            subject: Some("주제".into()),
            keywords: Some("키워드".into()),
            description: Some("설명 텍스트".into()),
            last_saved_by: Some("최종 저장자".into()),
            create_time: Some(created),
            modify_time: Some(modified),
        };
        let xml = content_hpf(1, &[], &[], &meta);
        // 정품 형식(요소 텍스트) 방출 확인.
        assert!(xml.contains("<opf:title>제목 A=B=&gt;C &amp; &lt;x&gt;</opf:title>"));
        assert!(xml.contains(r##"<opf:meta name="creator" content="text">지은이</opf:meta>"##));
        assert!(xml.contains(r##"<opf:meta name="subject" content="text">주제</opf:meta>"##));
        assert!(xml.contains(r##"<opf:meta name="keyword" content="text">키워드</opf:meta>"##));
        assert!(xml.contains(r##"name="description" content="text">설명 텍스트</opf:meta>"##));
        assert!(xml.contains(r##"name="lastsaveby" content="text">최종 저장자</opf:meta>"##));
        assert!(xml.contains(
            r##"<opf:meta name="CreatedDate" content="text">2025-09-17T04:32:50Z</opf:meta>"##
        ));
        assert!(xml.contains(
            r##"<opf:meta name="ModifiedDate" content="text">2025-09-17T04:33:13Z</opf:meta>"##
        ));
        // date는 KST(create 기준).
        assert!(xml.contains(
            r##"name="date" content="text">2025년 9월 17일 수요일 오후 1:32:50</opf:meta>"##
        ));
        // 구형 형식(dc:*, keywords 복수형)은 더 이상 방출하지 않는다.
        assert!(!xml.contains("<dc:creator>"));
        assert!(!xml.contains("<dc:subject>"));
        assert!(!xml.contains(r##"name="keywords""##));

        let parsed = parse_content_meta(&xml);
        assert_eq!(parsed.title.as_deref(), Some("제목 A=B=>C & <x>"));
        assert_eq!(parsed.author.as_deref(), Some("지은이"));
        assert_eq!(parsed.subject.as_deref(), Some("주제"));
        assert_eq!(parsed.keywords.as_deref(), Some("키워드"));
        assert_eq!(parsed.description.as_deref(), Some("설명 텍스트"));
        assert_eq!(parsed.last_saved_by.as_deref(), Some("최종 저장자"));
        assert_eq!(parsed.create_time, Some(created));
        assert_eq!(parsed.modify_time, Some(modified));
    }

    /// 확장 필드가 None이면 정품처럼 빈 요소로 방출한다(요소는 유지, 값만 비움).
    /// creator는 author None일 때 앱 이름 "hwp-cli"를 유지한다.
    #[test]
    fn content_hpf_none_필드_빈요소() {
        let xml = content_hpf(1, &[], &[], &hwp_model::Metadata::default());
        // 빈 요소로 존재.
        assert!(xml.contains(r##"<opf:meta name="subject" content="text"/>"##));
        assert!(xml.contains(r##"<opf:meta name="description" content="text"/>"##));
        assert!(xml.contains(r##"<opf:meta name="lastsaveby" content="text"/>"##));
        assert!(xml.contains(r##"<opf:meta name="keyword" content="text"/>"##));
        assert!(xml.contains(r##"<opf:meta name="CreatedDate" content="text"/>"##));
        assert!(xml.contains(r##"<opf:meta name="ModifiedDate" content="text"/>"##));
        assert!(xml.contains(r##"<opf:meta name="date" content="text"/>"##));
        // creator는 hwp-cli.
        assert!(xml.contains(r##"<opf:meta name="creator" content="text">hwp-cli</opf:meta>"##));
        assert!(xml.contains("<opf:title/>"));
        // 빈 요소는 값 없이 파싱되므로 대부분 None(creator만 hwp-cli).
        let parsed = parse_content_meta(&xml);
        assert_eq!(parsed.subject, None);
        assert_eq!(parsed.description, None);
        assert_eq!(parsed.keywords, None);
        assert_eq!(parsed.create_time, None);
        assert_eq!(parsed.author.as_deref(), Some("hwp-cli"));
    }

    /// XML 1.0 허용 범위 밖 문자는 이스케이프가 아니라 제거다 — 흘리면 패키지가
    /// well-formed하지 않아 파서·한글이 파일 전체를 거부한다(수식 스크립트에서 유입 가능).
    #[test]
    fn esc_금지문자_제거() {
        assert_eq!(esc("a<b&c\"d"), "a&lt;b&amp;c&quot;d");
        assert_eq!(esc("a\u{1}b\u{1F}c"), "abc"); // C0 제어문자
        assert_eq!(esc("a\u{FFFE}b\u{FFFF}c"), "abc"); // 비문자
        assert_eq!(esc("탭\t줄\n복귀\r"), "탭\t줄\n복귀\r"); // 허용 3종은 유지
    }

    /// 구형 형식(dc:subject, name="keywords" 복수형+content 속성)도 하위호환 파싱.
    #[test]
    fn parse_구형_형식_하위호환() {
        let xml = r##"<opf:package><opf:metadata><opf:title>T</opf:title><dc:creator>A</dc:creator><dc:subject>S</dc:subject><opf:meta name="keywords" content="K"/><opf:meta name="description" content="text">D</opf:meta></opf:metadata></opf:package>"##;
        let parsed = parse_content_meta(xml);
        assert_eq!(parsed.title.as_deref(), Some("T"));
        assert_eq!(parsed.author.as_deref(), Some("A"));
        assert_eq!(parsed.subject.as_deref(), Some("S"));
        assert_eq!(parsed.keywords.as_deref(), Some("K"));
        assert_eq!(parsed.description.as_deref(), Some("D"));
    }
}
