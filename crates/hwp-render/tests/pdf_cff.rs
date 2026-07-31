//! CFF(OTF) 폰트 PDF 임베드 검증 — FontFile3 / CIDFontType0 / Subtype OpenType.
//!
//! 시스템에 CFF(OTTO) OTF 폰트가 있으면 그 폰트를 글꼴로 지정한 문서를 PDF로
//! 렌더해 임베드 구조를 확인한다(폰트 부재 환경은 스킵 — fixtures 패턴과 동일).

use std::path::{Path, PathBuf};

use hwp_render::{RenderOptions, render_document_pdf};

/// 흔한 폰트 디렉터리에서 CFF(OTTO 시그니처) OTF 하나를 찾는다.
fn find_cff_otf() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        format!("{home}/Library/Fonts"),
        "/Library/Fonts".to_string(),
        "/System/Library/Fonts".to_string(),
        "/usr/share/fonts".to_string(),
        "/usr/local/share/fonts".to_string(),
    ];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                == Some("otf".to_string())
                && let Ok(bytes) = std::fs::read(&p)
                && bytes.starts_with(b"OTTO")
            {
                return Some(p);
            }
        }
    }
    None
}

/// 폰트 family 이름(name table ID=1, 유니코드).
fn family_name(data: &[u8]) -> Option<String> {
    let face = rustybuzz::ttf_parser::Face::parse(data, 0).ok()?;
    face.names()
        .into_iter()
        .find(|n| n.name_id == rustybuzz::ttf_parser::name_id::FAMILY && n.is_unicode())
        .and_then(|n| n.to_string())
}

#[test]
fn cff_otf_pdf_임베드_구조() {
    let Some(otf) = find_cff_otf() else {
        eprintln!("스킵: CFF(OTF) 폰트 없음 — 임베드 구조 검증 생략");
        return;
    };
    let data = std::fs::read(&otf).unwrap();
    let Some(family) = family_name(&data) else {
        eprintln!("스킵: '{}' family 이름 파싱 실패", otf.display());
        return;
    };

    // 마크다운으로 최소 문서를 만들고 모든 글꼴 이름을 OTF family로 지정.
    let mut doc = hwp_convert::from_markdown("Hello 안녕하세요 12345");
    for slot in &mut doc.header.fonts {
        for face in slot {
            face.name = family.clone();
            face.alt_name = None;
        }
    }

    let dir = otf.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let out = render_document_pdf(
        &doc,
        &RenderOptions {
            dpi: 96.0,
            font_dirs: vec![dir],
        },
        None,
    )
    .unwrap();

    // 글꼴이 매칭됐는지(폴백이 아닌 일치) — 매칭 실패 시 의미 없는 검증이므로 스킵.
    let matched = out
        .report
        .info
        .iter()
        .any(|issue| issue.code == hwp_render::RenderIssueCode::FontMatched);
    if !matched {
        eprintln!("스킵: '{family}' 매칭 실패(env) — report={:?}", out.report);
        return;
    }

    let pdf = &out.data;
    let has = |needle: &[u8]| pdf.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"FontFile3"), "CFF는 FontFile3로 임베드되어야");
    assert!(has(b"CIDFontType0"), "CFF는 CIDFontType0여야");
    assert!(
        has(b"/Subtype /OpenType") || has(b"OpenType"),
        "FontFile3 Subtype=OpenType"
    );
    assert!(!has(b"FontFile2"), "CFF 경로에 FontFile2가 있으면 안 됨");
    assert!(has(b"Identity-H"), "합성 폰트 Identity-H 인코딩");
    assert!(has(b"ToUnicode"), "검색/복사용 ToUnicode CMap");
    // 서브셋되어 원본(보통 수백 KB~MB)보다 훨씬 작아야 한다.
    // 단, 원본이 소형 특수 폰트(macOS 시스템의 수 KB CFF)면 PDF 구조 오버헤드가
    // 원본보다 커져 성립하지 않으므로, 크기 단언은 의미 있는 크기에서만 수행한다.
    if data.len() > 64 * 1024 {
        assert!(
            pdf.len() < data.len(),
            "PDF({})가 원본 폰트({})보다 작아야(서브셋)",
            pdf.len(),
            data.len()
        );
    }
}
