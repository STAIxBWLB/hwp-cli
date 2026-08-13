//! PDF 백엔드 스모크 테스트.
//!
//! 폰트 가용성에 의존하지 않는 구조 불변식만 검증한다: 유효한 PDF 헤더/트레일러,
//! 페이지 수, 임베드 CID 폰트 + Identity-H + ToUnicode 존재.
//! 시각 충실도(PDF→PNG vs PNG 백엔드)는 README의 pdftoppm 교차검증으로 확인한다.

use std::path::PathBuf;

use hwp_render::{RenderOptions, count_pages, render_document_pdf};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// fixture 문서는 저장소에 없으므로(로컬 전용 — fixtures/README.md) 없으면 None → 테스트 skip.
fn load_or_skip(rel: &str) -> Option<hwp_model::Document> {
    let p = fixture(rel);
    if !p.exists() {
        eprintln!(
            "스킵: fixture 없음 ({}) — fixtures/README.md 참고",
            p.display()
        );
        return None;
    }
    Some(hwp5::read_document(&p).unwrap().document)
}

/// PDF 바이트 안에서 부분 바이트열 출현 횟수.
fn count(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[test]
fn 유효한_pdf_구조() {
    let Some(doc) = load_or_skip("hwp5/hello_world.hwp") else {
        return;
    };
    let out = render_document_pdf(&doc, &RenderOptions::default(), None).unwrap();
    let pdf = &out.data;

    assert!(pdf.starts_with(b"%PDF-"), "PDF 헤더가 없음");
    assert!(
        pdf.windows(5).any(|w| w == b"%%EOF"),
        "PDF 트레일러(%%EOF)가 없음"
    );
    assert!(pdf.len() > 1000, "PDF가 너무 작음: {} bytes", pdf.len());

    // 임베드 검색 가능 텍스트의 증거.
    assert!(count(pdf, b"/Identity-H") >= 1, "Identity-H 인코딩 없음");
    assert!(
        count(pdf, b"/CIDFontType2") + count(pdf, b"/CIDFontType0") >= 1,
        "CID 폰트 없음"
    );
    assert!(
        count(pdf, b"/ToUnicode") >= 1,
        "ToUnicode CMap 없음(검색 불가)"
    );
    assert!(
        count(pdf, b"/FontFile2") + count(pdf, b"/FontFile3") >= 1,
        "임베드 폰트 프로그램 없음"
    );
}

#[test]
fn 페이지_수_일치() {
    let Some(doc) = load_or_skip("hwp5/annual_report.hwp") else {
        return;
    };
    let opts = RenderOptions::default();
    let total = count_pages(&doc, &opts);
    assert!(total > 1, "멀티페이지 문서여야 함: {total}쪽");

    let out = render_document_pdf(&doc, &opts, None).unwrap();
    // 페이지 트리 1개(/Type /Pages) + 페이지 N개(/Type /Page) = "/Type /Page" N+1회.
    let page_markers = count(&out.data, b"/Type /Page");
    assert_eq!(
        page_markers,
        total + 1,
        "페이지 수 불일치: 마커 {page_markers}, 기대 {}",
        total + 1
    );
}

#[test]
fn 페이지_선택() {
    let Some(doc) = load_or_skip("hwp5/annual_report.hwp") else {
        return;
    };
    let opts = RenderOptions::default();
    let total = count_pages(&doc, &opts);
    assert!(total >= 3);

    let out = render_document_pdf(&doc, &opts, Some(&[1, 2])).unwrap();
    // 선택한 2쪽 + 페이지 트리 1개.
    assert_eq!(count(&out.data, b"/Type /Page"), 3);
}

#[test]
fn 표_문서_렌더() {
    // 표(선·셀 채움·텍스트)를 포함해도 유효한 PDF가 나와야 한다.
    let Some(doc) = load_or_skip("hwp5/work_report.hwp") else {
        return;
    };
    let out = render_document_pdf(&doc, &RenderOptions::default(), None).unwrap();
    assert!(out.data.starts_with(b"%PDF-"));
    assert!(out.data.windows(5).any(|w| w == b"%%EOF"));
}

#[test]
fn 빈_문서_렌더() {
    let Some(doc) = load_or_skip("hwp5/bookmark.hwp") else {
        return;
    };
    let out = render_document_pdf(&doc, &RenderOptions::default(), None).unwrap();
    // 텍스트가 없어도 유효한 1쪽 PDF.
    assert!(out.data.starts_with(b"%PDF-"));
    assert_eq!(count(&out.data, b"/Type /Page"), 2); // 페이지 1 + 트리 1
}

#[test]
fn 문서_수준_속성_한컴_패리티() {
    // docs/design/21 §2 — 한컴 PDF의 문서 수준 6종.
    let Some(doc) = load_or_skip("hwp5/hello_world.hwp") else {
        return;
    };
    let out = render_document_pdf(&doc, &RenderOptions::default(), None).unwrap();
    let pdf = &out.data;
    assert!(pdf.starts_with(b"%PDF-1.4"), "PDF 1.4 헤더");
    assert_eq!(count(pdf, b"/Lang (ko-KR)"), 1, "/Lang");
    assert_eq!(count(pdf, b"/PageLayout /SinglePage"), 1, "/PageLayout");
    assert!(count(pdf, b"/MarkInfo") >= 1, "/MarkInfo");
    assert!(count(pdf, b"/Marked false") >= 1, "/Marked false");
    assert_eq!(count(pdf, b"/OutputIntents"), 1, "/OutputIntents");
    assert_eq!(count(pdf, b"/GTS_PDFA1"), 1, "OutputIntent subtype");
    assert_eq!(count(pdf, b"/DestOutputProfile"), 1, "ICC 프로파일 참조");
    assert!(count(pdf, b"/Type /Metadata") >= 1, "XMP 메타데이터 스트림");
    assert!(count(pdf, b"xpacket") >= 1, "XMP 패킷");
    assert_eq!(count(pdf, b"/Creator"), 1, "/Info Creator");
    assert_eq!(count(pdf, b"/Producer"), 1, "/Info Producer");
    assert_eq!(count(pdf, b"/PDFVersion"), 1, "/Info PDFVersion");
}

#[test]
fn info_메타데이터_매핑() {
    let mut doc = hwp_convert::from_markdown("본문\n");
    // 비어 있으면 Author·날짜 키가 없어야 한다.
    let out = render_document_pdf(&doc, &RenderOptions::default(), None).unwrap();
    assert_eq!(
        count(&out.data, b"/Author"),
        0,
        "author 없음 → /Author 없음"
    );
    assert_eq!(
        count(&out.data, b"/CreationDate"),
        0,
        "create_time 없음 → /CreationDate 없음"
    );

    doc.metadata.author = Some("홍길동".to_string());
    doc.metadata.create_time = Some(133_486_382_450_000_000); // 2024-01-02T03:04:05Z
    doc.metadata.modify_time = Some(11_644_473_600 * 10_000_000); // 1970-01-01T00:00:00Z
    let out = render_document_pdf(&doc, &RenderOptions::default(), None).unwrap();
    let pdf = &out.data;
    // 한글 저자명은 UTF-16BE hex 문자열 (FEFF D64D AE38 B3D9).
    assert_eq!(count(pdf, b"/Author <FEFFD64DAE38B3D9>"), 1, "/Author");
    assert_eq!(
        count(pdf, b"/CreationDate (D:20240102030405Z)"),
        1,
        "/CreationDate"
    );
    assert_eq!(count(pdf, b"/ModDate (D:19700101000000Z)"), 1, "/ModDate");
    // XMP에도 동일 값.
    assert!(
        count(
            pdf,
            b"<xmp:CreateDate>2024-01-02T03:04:05Z</xmp:CreateDate>"
        ) >= 1,
        "XMP CreateDate"
    );
}

#[test]
fn 두_번_렌더_바이트_동일() {
    let Some(doc) = load_or_skip("hwp5/annual_report.hwp") else {
        return;
    };
    let a = render_document_pdf(&doc, &RenderOptions::default(), None)
        .unwrap()
        .data;
    let b = render_document_pdf(&doc, &RenderOptions::default(), None)
        .unwrap()
        .data;
    assert_eq!(a.len(), b.len(), "길이 불일치");
    assert_eq!(a, b, "2회 실행 PDF 바이트가 다름");
}

#[test]
fn srgb_icc_프로파일_고정() {
    use sha2::Digest;
    use std::fmt::Write as _;
    let icc = include_bytes!("../assets/sRGB-IEC61966-2.1.icc");
    assert_eq!(icc.len(), 3144);
    let hex = sha2::Sha256::digest(icc)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            write!(out, "{b:02x}").unwrap();
            out
        });
    assert_eq!(
        hex,
        "2b3aa1645779a9e634744faf9b01e9102b0c9b88fd6deced7934df86b949af7e"
    );
}
