//! Markdown → IR.
//!
//! 매핑: 헤딩 → "개요 N" 스타일, 굵게/기울임/취소선 → 문자 모양 변형,
//! GFM 표 → Table 컨트롤, 순서·글머리 목록 → 머리(NUMBER/BULLET) 문단,
//! 각주/미주(`[^N]`/`[^eN]`) → fn/en 컨트롤, 줄바꿈 → CharCtrl(10).
//!
//! 내보내기(markdown.rs)와의 대칭이 왕복 폐쇄의 기준이다:
//! - 취소선: 내보내기가 `CharShape.strike`를 읽으므로 strike=true 전용 문자모양을 만든다.
//! - 각주/미주: 내보내기가 `FOOTNOTE_ENDNOTE` ExtCtrl + `fn `/`en ` GenericControl의
//!   `paragraph_lists`를 읽으므로 그 구조를 그대로 합성한다.
//! - 목록: 내보내기가 `ParaShape.head_type()/head_level()/numbering_id`와
//!   `numbering_levels`/`bullet_chars`로 마커를 그리므로 그 정의를 만든다.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hwp_model::{
    BinRef, BinStream, BorderFill, BorderFillId, BorderLine, Cell, CharShape, CharShapeId, Control,
    DocMeta, Document, FaceName, GenericControl, HwpChar, HwpUnit, LANG_COUNT, NumFmt, NumLevel,
    ParaShape, ParaShapeId, Paragraph, ParagraphList, Picture, Section, Style, StyleId, Table,
    ctrl_char,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// default_header가 만드는 기본 문단 모양 개수(인덱스 0~4). 목록용 문단 모양은
/// 이 뒤(5~)에 붙는다. from_html의 목록 문단모양도 같은 기준을 쓴다.
pub(crate) const BASE_PARA_SHAPES: u16 = 5;

/// markdown 들여오기 옵션.
#[derive(Default)]
pub struct MarkdownImportOptions<'a> {
    /// 상대 경로 이미지(`![](fig.png)`)를 해석할 기준 디렉터리(md 파일의 위치).
    /// `None`이면 상대 경로 이미지는 경고 후 alt 텍스트만 보존한다(절대 경로는 그대로 시도).
    pub base_dir: Option<&'a Path>,
    /// 공문서 프리셋 — 지정 시 용지 여백·글꼴·번호 체계·쪽번호를 규정에 맞춘다.
    /// `None`이면 기존 기본값(변경 없음).
    pub preset: Option<OfficialPreset>,
}

/// 한국 공문서 작성 규정(「행정 효율과 협업 촉진에 관한 규정」) 프리셋.
/// 공통: A4 여백 위30/아래15/좌20/우15mm, 줄간격 160%, 순서 목록 4단계 번호
/// (1. → 가. → 1) → 가)), 쪽번호 하단 중앙(pgnp, 정품 실측 sideChar '-').
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OfficialPreset {
    /// 기안문·공문: 맑은 고딕 11.5pt (전자결재 시스템 표준).
    Gian,
    /// 보고서·사업계획서: 함초롬바탕 15pt (전통 관행).
    Report,
}

/// 문자 모양 ID 배치 (default_header와 일치해야 함). from_html도 같은 팔레트를 쓴다.
pub(crate) mod shapes {
    pub const NORMAL: u16 = 0;
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 2;
    pub const BOLD_ITALIC: u16 = 3;
    /// H1~H6 → 4~9
    pub const HEADING_BASE: u16 = 4;
    /// 하이퍼링크 표시 텍스트(파랑 + 밑줄)
    pub const HYPERLINK: u16 = 10;
    /// 취소선 조합(본문/굵게/기울임/굵게+기울임 + strike) → 11~14
    pub const STRIKE: u16 = 11;
    pub const BOLD_STRIKE: u16 = 12;
    pub const ITALIC_STRIKE: u16 = 13;
    pub const BOLD_ITALIC_STRIKE: u16 = 14;
    /// 인라인 코드(함초롬돋움 + 연회색 음영) → 15
    pub const CODE: u16 = 15;
}

/// default_header 글꼴 테이블의 함초롬돋움 인덱스(인라인 코드용). 함초롬바탕=0.
const FONT_DOTUM: u16 = 1;

/// 테두리/배경 ID 배치: 1·2 = 무테두리(기본/참조용), 3 = 실선 0.12mm.
pub(crate) const TABLE_BORDER_FILL: u16 = 3;

/// 셀 LIST_HEADER 속성의 세로 정렬 = 가운데(bits5-6=1 → 0x20). 정품 한글 표 셀은
/// 거의 전수(work_report·color_fill·코퍼스 실측)가 이 비트를 세운다. 0(위)으로 두면
/// hwp5 writer가 그대로 방출해 셀 내용이 상단에 붙는다(위 여백<아래 여백). hwpx writer는
/// vertAlign="CENTER"를 상수로 방출하므로 hwpx 산출물에는 영향 없다.
pub(crate) const CELL_VALIGN_CENTER: u32 = 0x20;

/// 본문 영역 폭 (A4 기본 여백 기준, HWPUNIT). from_html의 표 폭 계산도 같은 기준.
pub(crate) const BODY_WIDTH: i32 = 42520;

/// `hwp new`용 기본 문서 헤더 — 한글 빈 문서에 준하는 최소 구성.
pub fn default_header() -> hwp_model::DocHeader {
    // 본문 함초롬바탕 10pt(1000 HWPUNIT). 헤딩 크기 = 본문 × 비율(1800/1500/1300/1200/1100/1100).
    let body = 1000;
    let h = |factor: i32| (body * factor) / 100;
    let mut header = hwp_model::DocHeader::default();
    for slot in 0..LANG_COUNT {
        header.fonts[slot] = vec![
            FaceName {
                name: "함초롬바탕".to_string(),
                // 한글 무결성 검사는 글꼴 대체를 위해 기본 글꼴 이름(attr bit5, 0x20)을 기대한다.
                // 정상 표본 hello_world.hwp 의 '함초롬바탕'은 default_name="HCR Batang", attr=0x21.
                // attr 하위 0x01 = 글꼴 유형 TTF(표 20). emit_face_name 이 0x20 비트를 자동 OR 한다.
                attr: 0x01,
                default_name: Some("HCR Batang".to_string()),
                ..FaceName::default()
            },
            // 인덱스 1 = 함초롬돋움(고딕/산세리프) — 인라인 코드용. 번들 fonts/의 HCRDotum으로
            // 렌더러도 실제 글리프를 그린다. 두 writer(hwp5 emit_face_name 루프·hwpx
            // write_fontfaces)가 슬롯별 fonts 전체를 방출하고 ID_MAPPINGS 카운트도 len으로 유도돼
            // 정합한다.
            FaceName {
                name: "함초롬돋움".to_string(),
                attr: 0x01,
                default_name: Some("HCR Dotum".to_string()),
                ..FaceName::default()
            },
        ];
    }

    let base = CharShape {
        base_size: 1000,
        ratios: [100; LANG_COUNT],
        rel_sizes: [100; LANG_COUNT],
        // 음영 색(shade_color)은 0xFFFFFFFF = '없음' 표식이어야 한다. 기본값 0은
        // 한글이 '불투명 검정 음영(글자 배경 하이라이트)'으로 해석해, 글자 칸마다
        // 검은 막대를 그리고 (검정) 글자가 그 위에서 안 보이게 된다 — 14차 실기의
        // '검은 바' 원인. 정상 표본(가나다.hwp 5.1.1.0, hello_world.hwp 5.1.0.1)은
        // 모두 shade_color=0xFFFFFFFF, shadow_gap=(10,10), shadow_color≈0xC0C0C0.
        // (face_id=0은 무해 — hello_world도 char_shape[0].face_ids=0이고 정상 렌더.)
        shade_color: 0xFFFF_FFFF,
        shadow_color: 0x00C0_C0C0,
        shadow_gap: (10, 10),
        ..CharShape::default()
    };
    let cs = |size: i32, bold: bool, italic: bool| CharShape {
        base_size: size,
        attr: u32::from(bold) << 1 | u32::from(italic),
        ..base.clone()
    };
    header.char_shapes = vec![
        cs(body, false, false),  // 0 본문
        cs(body, true, false),   // 1 굵게
        cs(body, false, true),   // 2 기울임
        cs(body, true, true),    // 3 굵게+기울임
        cs(h(180), true, false), // 4 H1
        cs(h(150), true, false), // 5 H2
        cs(h(130), true, false), // 6 H3
        cs(h(120), true, false), // 7 H4
        cs(h(110), true, false), // 8 H5
        cs(h(110), true, false), // 9 H6
        // 10 하이퍼링크: 파랑(COLORREF 0x00BBGGRR=RGB(0,0,255)) + 밑줄 종류 1.
        // field.rs::hyperlink_char_shape와 동일 규칙 — 한글이 링크로 인식/표시하려면 필요.
        CharShape {
            base_size: body,
            text_color: 0x00FF_0000,
            underline_color: 0x00FF_0000,
            attr: 1 << 2,
            ..base.clone()
        },
    ];
    // 11~14 취소선 조합. 내보내기(markdown.rs)는 CharShape.strike(명시 플래그)로
    // 취소선을 감지하므로, `~~`가 왕복하려면 strike=true 전용 문자모양이 필요하다.
    // hwp5는 strike를 바이트로 쓰지 않아(무영향), hwpx는 <hh:strikeout SOLID>로 방출.
    let cs_strike = |bold: bool, italic: bool| CharShape {
        base_size: body,
        attr: u32::from(bold) << 1 | u32::from(italic),
        strike: true,
        ..base.clone()
    };
    header.char_shapes.push(cs_strike(false, false)); // 11 취소선
    header.char_shapes.push(cs_strike(true, false)); // 12 굵게+취소선
    header.char_shapes.push(cs_strike(false, true)); // 13 기울임+취소선
    header.char_shapes.push(cs_strike(true, true)); // 14 굵게+기울임+취소선
    // 15 인라인 코드: 함초롬돋움(face_id=1) + 연회색 음영(0xF0F0F0). 한글은 shade_color를
    // 글자 배경 하이라이트로 그려 코드 스팬에 회색 배경을 준다(0xFFFFFFFF='없음'과 대비).
    header.char_shapes.push(CharShape {
        base_size: body,
        face_ids: [FONT_DOTUM; LANG_COUNT],
        shade_color: 0x00F0_F0F0,
        ..base.clone()
    });

    // 탭 정의 — 한글 기본 좌/중/우 자동 탭 3개. 정상 표본(hello_world 등
    // 5.1.0.1)은 전부 이 3개를 가지며, 모든 PARA_SHAPE가 tab_def_id=0 을
    // 참조한다. 비우면 dangling reference가 되어 한글이 '손상/변조'로 거부.
    // 각 8바이트: 속성 u32(0/1/2) + count i16=0 + 예약 u16 (spec 표36, count=0→8B).
    header.tab_defs = vec![
        hwp_model::RawEntry {
            data: vec![0, 0, 0, 0, 0, 0, 0, 0],
            children: Vec::new(),
        },
        hwp_model::RawEntry {
            data: vec![1, 0, 0, 0, 0, 0, 0, 0],
            children: Vec::new(),
        },
        hwp_model::RawEntry {
            data: vec![2, 0, 0, 0, 0, 0, 0, 0],
            children: Vec::new(),
        },
    ];
    // HWPX writer/렌더러가 사용하는 의미 사본. raw TAB_DEF와 같은 순서로 좌/중/우
    // 자동 탭 속성을 채워 교차 저장에서도 사용자 의미가 기본값으로 소실되지 않게 한다.
    header.tab_stops = vec![
        hwp_model::TabDef {
            attr: 0,
            items: Vec::new(),
        },
        hwp_model::TabDef {
            attr: 1,
            items: Vec::new(),
        },
        hwp_model::TabDef {
            attr: 2,
            items: Vec::new(),
        },
    ];

    // 0 기본·표 셀(양쪽, 간격 없음), 1 제목(왼쪽 + 위/아래 간격), 2 본문(양쪽 + 아래 간격).
    //
    // 본문 문단은 아래 간격(spacing_bottom)을 줘서 md 생성물이 실제 문서처럼
    // 문단 사이가 떨어져 보이게 한다. 표 셀은 0(간격 없음)을 써서 셀이 불필요하게
    // 커지지 않게 한다 — flush_paragraph_inner가 self.table 유무로 둘을 가른다.
    //
    // 정상 표본(가나다.hwp 5.1.1.0, hello_world.hwp 5.1.0.1)의 PARA_SHAPE[0]은
    // attr1=0x180(bit7 한글 줄나눔=글자 + bit8 줄 격자 사용), line_spacing_old=160,
    // border_fill_id=2 다. 이는 본문 줄 배치를 한글이 재계산할 때의 기준값으로,
    // 0(우리 기존값)이면 줄 격자·줄나눔 기준이 정상 표본과 어긋난다. 검은 바의
    // 직접 원인은 char_shape 음영색이지만, 한글이 줄 배치를 다시 잡을 때 안전하도록
    // 정상 표본 바이트에 맞춘다. (BodyText의 PARA_LINE_SEG 캐시는 합성기가 채운다.)
    let base_para = ParaShape {
        attr1: 0x180,
        line_spacing_old: 160,
        border_fill_id: 2,
        line_spacing: 160,
        ..ParaShape::default()
    };
    header.para_shapes = vec![
        base_para.clone(),
        ParaShape {
            attr1: 0x180 | (1 << 2), // 정상 attr1 + 왼쪽 정렬
            spacing_top: 600,
            spacing_bottom: 300,
            ..base_para.clone()
        },
        ParaShape {
            spacing_bottom: 600, // 본문 문단 아래 간격
            ..base_para.clone()
        },
        // 3 인용문: 왼쪽 들여쓰기 + 좌측 막대(border_fill 1-based id 4).
        ParaShape {
            attr1: 0x180 | (1 << 2),
            margin_left: 3000,
            border_fill_id: 4,
            spacing_top: 300,
            spacing_bottom: 300,
            ..base_para.clone()
        },
        // 4 코드블록: 좌우 들여쓰기 + 회색 배경(border_fill 1-based id 5).
        ParaShape {
            attr1: 0x180 | (1 << 2),
            margin_left: 2500,
            margin_right: 2500,
            border_fill_id: 5,
            spacing_top: 300,
            spacing_bottom: 300,
            ..base_para
        },
    ];

    header.styles = vec![Style {
        name: "바탕글".to_string(),
        english_name: "Normal".to_string(),
        ..Style::default()
    }];
    for n in 1..=6u16 {
        header.styles.push(Style {
            name: format!("개요 {n}"),
            english_name: format!("Outline {n}"),
            para_shape: ParaShapeId(1),
            char_shape: CharShapeId(shapes::HEADING_BASE + n - 1),
            ..Style::default()
        });
    }

    let none = BorderFill {
        diagonal: BorderLine {
            line_type: 1,
            width: 0,
            color: 0,
        },
        ..BorderFill::default()
    };
    let solid_line = BorderLine {
        line_type: 1,
        width: 1,
        color: 0,
    }; // 실선 0.12mm 검정
    header.border_fills = vec![
        none.clone(),
        none,
        BorderFill {
            sides: [solid_line; 4],
            diagonal: BorderLine {
                line_type: 1,
                width: 0,
                color: 0,
            },
            ..BorderFill::default()
        },
        // 3 (1-based id 4) 인용문: 좌측 회색 막대(1.5mm), 나머지 변 없음. 한글이 hwpx 문단
        // 테두리를 hwp5보다 얇게 그려서, 1.0mm→1.5mm로 올려 hwpx에서도 또렷하게 보이게 함.
        BorderFill {
            sides: [
                BorderLine {
                    line_type: 1,
                    width: 11,
                    color: 0x0080_8080,
                },
                BorderLine::default(),
                BorderLine::default(),
                BorderLine::default(),
            ],
            ..BorderFill::default()
        },
        // 4 (1-based id 5) 코드블록: 연회색 배경 + 얇은 회색 테두리.
        BorderFill {
            sides: [BorderLine {
                line_type: 1,
                width: 0,
                color: 0x00C0_C0C0,
            }; 4],
            fill_type: 1,
            bg_color: Some(0x00F0_F0F0),
            ..BorderFill::default()
        },
    ];
    header
}

/// 공문서 프리셋을 헤더에 적용한다 — 글꼴·크기와 순서 목록 4단계 번호 체계.
/// (여백·쪽번호는 inject_section_controls가 담당.)
fn apply_official_preset(header: &mut hwp_model::DocHeader, preset: OfficialPreset) {
    // 본문·제목 크기(HWPUNIT/100pt): 기안문 11.5pt·제목 15pt / 보고서 15pt·제목 18pt.
    let (body, headings) = match preset {
        OfficialPreset::Gian => (1150, [1500, 1400, 1300, 1200, 1150, 1150]),
        OfficialPreset::Report => (1500, [1800, 1700, 1600, 1550, 1500, 1500]),
    };
    if preset == OfficialPreset::Gian {
        // 슬롯 0(본문 글꼴)만 맑은 고딕으로 교체 — 1(함초롬돋움, 인라인 코드)은 유지.
        for slot in 0..LANG_COUNT {
            header.fonts[slot][0] = FaceName {
                name: "맑은 고딕".to_string(),
                attr: 0x01,
                default_name: Some("Malgun Gothic".to_string()),
                ..FaceName::default()
            };
        }
    }
    // 크기 재배치: 4~9 = H1~H6, 나머지(본문·강조·링크·취소선·코드)는 본문 크기.
    for (i, cs) in header.char_shapes.iter_mut().enumerate() {
        cs.base_size = match i.checked_sub(shapes::HEADING_BASE as usize) {
            Some(h) if h < headings.len() => headings[h],
            _ => body,
        };
    }
    // 순서 목록 4단계 번호(규정 §5): 1. → 가. → 1) → 가), 5수준부터 반복.
    for levels in &mut header.numbering_levels {
        for (i, nl) in levels.iter_mut().enumerate() {
            let (fmt, suffix) = match i % 4 {
                0 => (NumFmt::Digit, "."),
                1 => (NumFmt::HangulSyllable, "."),
                2 => (NumFmt::Digit, ")"),
                _ => (NumFmt::HangulSyllable, ")"),
            };
            nl.fmt = fmt;
            nl.template = format!("^{}{suffix}", i + 1);
        }
    }
}

/// markdown 텍스트를 문서로 변환한다(기존 시그니처 — 상대 경로 이미지는 경고 후 alt 보존).
pub fn from_markdown(md: &str) -> Document {
    from_markdown_with(md, &MarkdownImportOptions::default())
}

/// 옵션을 받는 변형. `base_dir` 지정 시 상대 경로 이미지(`![](fig.png)`)를 임베드한다.
/// 원격 URL·없는 파일·미지원 포맷은 경고(stderr) 후 alt 텍스트만 본문에 보존한다.
pub fn from_markdown_with(md: &str, opts: &MarkdownImportOptions) -> Document {
    from_markdown_inner(md, opts, true)
}

/// 부분(part) 조각용 변형 — 구역/단 정의(secd/cold) 주입을 건드리지 않는다.
/// 조합 대상 문서 중간에 이식될 블록이므로, 구역 정의가 끼면 문서가 둘로 갈라진다.
/// `hwp fill --set name=@part.md`(템플릿+부분 채우기)이 쓴다.
pub fn from_markdown_blocks(md: &str, opts: &MarkdownImportOptions) -> Document {
    from_markdown_inner(md, opts, false)
}

fn from_markdown_inner(md: &str, opts: &MarkdownImportOptions, inject: bool) -> Document {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    // 취소선(`~~`)·각주(`[^N]`)를 파싱한다. 작업목록(TASKLISTS)은 대응 IR 의미가 없어 제외.
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    // 각주 참조 시점에 정의 본문이 필요하므로 이벤트를 한 번에 모아 두 번 훑는다.
    let events: Vec<Event> = Parser::new_ext(md, options).collect();

    // 1) 각주/미주 정의 본문을 미리 렌더한다(참조에서 재사용).
    let note_bodies = collect_note_bodies(&events);

    // 2) 본문 처리.
    let mut b = Builder {
        note_bodies,
        base_dir: opts.base_dir.map(Path::to_path_buf),
        ..Builder::default()
    };
    for event in &events {
        b.event(event.clone());
    }
    b.flush_html(); // 버퍼에 남은 블록 HTML 마감
    b.flush_paragraph();

    // 이미지 실패 등 경고는 stderr로 남긴다(문서 생성 자체는 성공한다).
    for w in &b.warnings {
        eprintln!("경고: {w}");
    }

    if b.paragraphs.is_empty() {
        // 빈 문서도 문단 하나로 닫는다. 문단끝 문자는 writer가 보장한다.
        b.paragraphs.push(Paragraph::default());
    }
    if inject {
        // 첫 문단에 구역/단 정의 주입 — hwp5/한글 호환의 전제 조건
        inject_section_controls(&mut b.paragraphs[0], opts.preset);
    }

    // 목록에서 만든 문단 모양·번호/글머리 정의를 헤더에 합친다.
    let mut header = default_header();
    header.char_shapes.extend(b.extra_char_shapes);
    header.para_shapes.extend(b.extra_para_shapes);
    for slot in &mut header.fonts {
        slot.extend(b.extra_fonts.iter().cloned());
    }
    header.numbering_levels = b.numbering_levels;
    header.bullet_chars = b.bullet_chars;
    if let Some(preset) = opts.preset {
        apply_official_preset(&mut header, preset);
    }

    Document {
        meta: DocMeta {
            source_format: "markdown".to_string(),
            source_version: String::new(),
        },
        metadata: Default::default(),
        header,
        sections: vec![Section {
            paragraphs: b.paragraphs,
            extras: Vec::new(),
        }],
        bin_streams: b.bin_streams,
        hwpx_settings_xml: None,
        hwpx_version_xml: None,
        hwp5_xml_template: Vec::new(),
        hwp5_doc_history: Vec::new(),
    }
}

/// 각주/미주 라벨이 미주(`eN`)인지 — 내보내기가 미주를 `[^eN]`으로 쓰는 규약과 대칭.
fn is_endnote_label(label: &str) -> bool {
    label
        .strip_prefix('e')
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// 각주/미주 정의(`[^N]: 본문`) 블록을 라벨→본문 문단으로 미리 렌더한다.
/// 참조(`[^N]`)가 정의보다 먼저 등장할 수 있어 선수집이 필요하다.
fn collect_note_bodies(events: &[Event]) -> HashMap<String, Vec<Paragraph>> {
    let mut map = HashMap::new();
    let mut i = 0;
    while i < events.len() {
        let Event::Start(Tag::FootnoteDefinition(label)) = &events[i] else {
            i += 1;
            continue;
        };
        // 대응하는 End까지의 내부 이벤트를 추린다(정의는 중첩되지 않지만 깊이로 방어).
        let mut depth = 1usize;
        let start = i + 1;
        let mut j = start;
        while j < events.len() {
            match &events[j] {
                Event::Start(Tag::FootnoteDefinition(_)) => depth += 1,
                Event::End(TagEnd::FootnoteDefinition) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let mut sub = Builder::default();
        for ev in &events[start..j] {
            sub.event(ev.clone());
        }
        sub.flush_html();
        sub.flush_paragraph();
        let mut body = sub.paragraphs;
        if body.is_empty() {
            body.push(note_body_para());
        }
        // 각주 본문은 목록 문단모양(합쳐지지 않는 서브빌더 산출)을 참조할 수 없으므로
        // 기본 본문 모양으로 되돌린다(각주 안 목록은 v1 미지원 — 텍스트만 보존).
        for p in &mut body {
            if p.para_shape.0 >= BASE_PARA_SHAPES {
                p.para_shape = ParaShapeId(2);
            }
        }
        map.insert(label.to_string(), body);
        i = j + 1;
    }
    map
}

/// 빈 각주/미주 본문 문단(문자 모양 run 1개 필수 불변식 충족).
fn note_body_para() -> Paragraph {
    Paragraph {
        char_shape_runs: vec![(0, CharShapeId(0))],
        ..Paragraph::default()
    }
}

#[derive(Default)]
struct Builder {
    paragraphs: Vec<Paragraph>,
    // 현재 문단 상태
    chars: Vec<HwpChar>,
    runs: Vec<(u32, CharShapeId)>,
    controls: Vec<Control>, // 현재 문단의 확장 컨트롤(하이퍼링크 등)
    wchar_pos: u32,
    style: u16,
    bold: bool,
    italic: bool,
    strike: bool,              // 취소선 구간(`~~`)
    underline: bool,           // 밑줄 구간(인라인 HTML `<u>`)
    sup: bool,                 // 위첨자 구간(인라인 HTML `<sup>`)
    sub: bool,                 // 아래첨자 구간(인라인 HTML `<sub>`)
    in_code: bool,             // 인라인 코드 구간(`code` — 함초롬돋움+음영)
    in_link: bool,             // 하이퍼링크 표시 텍스트 구간(파랑+밑줄)
    link_end: Option<HwpChar>, // 링크 종료 시 방출할 FIELD_END 문자
    in_blockquote: u32,        // 인용문 중첩 깊이(>0이면 인용 문단)
    in_codeblock: bool,        // 코드블록 구간(회색 배경 문단)
    heading: Option<u16>,      // 1..=6
    // H1~H3 절 번호 사다리(1. / 1-1. / 1-1-1.) — 보고서 표준. 제목이 이미 번호로
    // 시작하면(1. / Ⅰ. / 가.) 접두를 생략한다(이중 번호 방지).
    h_counters: [u32; 3],
    pending_heading_num: Option<String>,
    section_para_shape: Option<u16>, // H2/H3 들여쓰기(left 2000) 문단모양 인덱스
    // 개조식 기호(□·○)로 시작하는 본문 문단의 내어쓰기 문단모양(기호별 1회 할당).
    symbol_para_shapes: HashMap<char, u16>,
    // 표 수집 상태
    table: Option<TableBuilder>,
    // 목록 상태 — 수준별 프레임 스택(중첩), 항목 문단에 머리 문단모양을 부여.
    list_stack: Vec<ListFrame>,
    // 목록에서 만든 문단 모양(헤더 인덱스 BASE_PARA_SHAPES~)·번호/글머리 정의(0~).
    extra_para_shapes: Vec<ParaShape>,
    numbering_levels: Vec<Vec<NumLevel>>,
    bullet_chars: Vec<char>,
    // 각주/미주: 선수집한 정의 본문(라벨→문단) + 정의 블록 건너뛰기 깊이.
    note_bodies: HashMap<String, Vec<Paragraph>>,
    skip_note_def: u32,
    // 이미지: 상대 경로 기준 디렉터리 + 임베드한 바이너리 + 경고 + alt 억제 상태.
    base_dir: Option<PathBuf>,
    bin_streams: Vec<BinStream>,
    warnings: Vec<String>,
    // HTML 블록(표·그림 — 계약 docs/design/18): 연속 Html 이벤트 버퍼 +
    // 밑줄·첨자 등 팔레트 외 문자모양(팔레트 뒤에 1회 할당) + 할당 캐시.
    html_buf: String,
    extra_char_shapes: Vec<CharShape>,
    html_shape_cache: HashMap<(bool, bool, bool, bool, bool, bool), u16>,
    // HTML 블록의 `<style>` 규칙이 복원한 추가 글꼴 (계약 v2 — 슬롯 전체에 연장).
    extra_fonts: Vec<FaceName>,
    in_image_suppress: bool, // 이미지 임베드 성공 시 alt 텍스트를 억제
}

/// 목록 한 수준(프레임). `Start(List)`마다 하나 생기고 항목이 이 머리 문단모양을 쓴다.
struct ListFrame {
    /// 이 목록 항목 문단이 참조할 문단 모양 인덱스.
    para_shape_id: u16,
    /// 지금 이 수준의 항목이 열려 있는지(문단 flush 시 머리 부여 여부).
    item_open: bool,
}

#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<Paragraph>>,
    current_row: Vec<Paragraph>,
    in_head: bool,
}

impl Builder {
    fn current_shape(&self) -> u16 {
        // 인라인 코드는 함초롬돋움+음영으로 다른 서식을 지배한다(가장 우선).
        if self.in_code {
            return shapes::CODE;
        }
        if self.in_link {
            return shapes::HYPERLINK;
        }
        if let Some(level) = self.heading {
            return shapes::HEADING_BASE + level - 1;
        }
        match (self.bold, self.italic, self.strike) {
            (false, false, false) => shapes::NORMAL,
            (true, false, false) => shapes::BOLD,
            (false, true, false) => shapes::ITALIC,
            (true, true, false) => shapes::BOLD_ITALIC,
            (false, false, true) => shapes::STRIKE,
            (true, false, true) => shapes::BOLD_STRIKE,
            (false, true, true) => shapes::ITALIC_STRIKE,
            (true, true, true) => shapes::BOLD_ITALIC_STRIKE,
        }
    }

    /// 현재 상태의 문자 모양 ID. 밑줄·첨자가 끼면 팔레트 조합으로 부족하므로
    /// 팔레트 뒤(extra_char_shapes)에 1회 할당한다(from_html과 같은 규칙).
    fn shape_id(&mut self) -> u16 {
        if !self.underline && !self.sup && !self.sub {
            return self.current_shape();
        }
        if self.in_code {
            return shapes::CODE;
        }
        if self.in_link {
            return shapes::HYPERLINK;
        }
        if let Some(level) = self.heading {
            return shapes::HEADING_BASE + level - 1;
        }
        let key = (
            self.bold,
            self.italic,
            self.underline,
            self.strike,
            self.sup,
            self.sub,
        );
        if let Some(&id) = self.html_shape_cache.get(&key) {
            return id;
        }
        let id = crate::from_html::PALETTE_LEN + self.extra_char_shapes.len() as u16;
        let normal = default_header().char_shapes[shapes::NORMAL as usize].clone();
        let mut attr = u32::from(self.bold) << 1 | u32::from(self.italic);
        if self.underline {
            attr |= 1 << 2; // 밑줄 종류 1(글자 아래)
        }
        if self.sup {
            attr |= 1 << 15;
        }
        if self.sub {
            attr |= 1 << 16;
        }
        self.extra_char_shapes.push(CharShape {
            attr,
            strike: self.strike,
            ..normal
        });
        self.html_shape_cache.insert(key, id);
        id
    }

    fn push_text(&mut self, text: &str) {
        let shape = CharShapeId(self.shape_id());
        if self.runs.last().map(|(_, s)| *s) != Some(shape) {
            self.runs.push((self.wchar_pos, shape));
        }
        for c in text.chars() {
            match c {
                // 탭: HWP는 코드 9를 8 WCHAR 인라인 컨트롤로 저장한다(§3.2.3 표 6).
                // Text('\t')(1 WCHAR)로 적재하면 hwp5 PARA_TEXT/hwpx <hp:t>가 모두
                // 깨지므로 IR 불변식대로 InlineCtrl로 분리 적재한다.
                '\t' => {
                    self.wchar_pos += 8;
                    self.chars.push(HwpChar::InlineCtrl {
                        code: hwp_model::ctrl_char::TAB,
                        payload: vec![0; 12],
                    });
                }
                // 그 외 C0 제어문자(0x00~0x1F)는 문서를 깨뜨릴 수 있어 드롭한다. markdown의
                // 줄바꿈은 SoftBreak/HardBreak 이벤트로 따로 처리되므로 여기 Text에는
                // 정상 텍스트만 남는다(코드블록의 개행도 push_code_text가 CharCtrl로 분리).
                c if (c as u32) < 0x20 => {}
                c => {
                    self.wchar_pos += c.len_utf16() as u32;
                    self.chars.push(HwpChar::Text(c));
                }
            }
        }
    }

    /// 코드블록 텍스트: 줄 경계 `\n` → CharCtrl(10)(줄바꿈)으로 보존한다. 후행 개행 1개는
    /// 코드 상자 끝의 빈 줄을 피하려 제거(fenced 블록은 보통 `\n`으로 끝남).
    fn push_code_text(&mut self, text: &str) {
        let text = text.strip_suffix('\n').unwrap_or(text);
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.chars.push(HwpChar::CharCtrl(10));
                self.wchar_pos += 1;
            }
            if !line.is_empty() {
                self.push_text(line);
            }
        }
    }

    fn flush_paragraph(&mut self) {
        self.flush_paragraph_inner(false);
    }

    /// 문단을 닫는다. `force`면 내용이 없어도 빈 문단을 만든다.
    ///
    /// 표 셀은 반드시 문단을 1개 이상 가져야 한다(LIST_HEADER nparas≥1).
    /// 빈 markdown 셀(`| |`)을 그냥 흘리면 셀에 PARA_HEADER가 하나도 안 붙어
    /// nparas=0 셀이 되고, 한글이 이를 '손상'으로 거부한다. 셀 종료 시 force=true로
    /// 호출해 빈 셀도 빈 문단을 갖게 한다.
    fn flush_paragraph_inner(&mut self, force: bool) {
        if self.chars.is_empty() && self.runs.is_empty() && !force {
            return;
        }
        // 문단끝 문자(0x0d)·nchars bit31·char_shape run 병합 등 한글 문단 불변식은
        // hwp5 writer(emit_paragraph)가 합성 경로 전체(md+hwpx)에 일원 적용한다.
        // 단, 모든 문단은 PARA_CHAR_SHAPE를 1개 이상 가져야 한다(정품 전수:
        // PARA_HEADER 수 == PARA_CHAR_SHAPE 수, 빈 셀 문단도 (0,id) run 1개 보유).
        // writer는 char_shape_runs가 비면 PARA_CHAR_SHAPE를 아예 방출하지 않으므로,
        // 빈 문단(force로 만든 빈 셀 등)은 여기서 (0, 본문모양) run 1개를 채운다.
        // 누락 시 한글이 '손상'으로 거부하고 pyhwp 파서도 크래시한다.
        let mut runs = std::mem::take(&mut self.runs);
        if runs.is_empty() {
            runs.push((0, CharShapeId(self.shape_id())));
        }
        // 목록 항목이 열려 있으면 머리(NUMBER/BULLET) 문단모양을 우선한다.
        // 그 외: 코드블록→4(회색 배경), 인용→3(들여쓰기+막대), 제목→1,
        // 표 셀→0(간격 없음), 개조식 기호 본문(□·○)→내어쓰기 모양, 본문→2.
        let symbol = leading_symbol(&self.chars);
        let para_shape = if let Some(id) = self.active_list_para_shape() {
            id
        } else if self.in_codeblock {
            4
        } else if self.in_blockquote > 0 {
            3
        } else if let Some(h) = self.heading {
            // H2/H3는 들여쓰기(SECTION) 문단모양, 그 외 제목은 기본 제목 모양.
            if (2..=3).contains(&h) {
                self.section_para_shape.unwrap_or(1)
            } else {
                1
            }
        } else if self.table.is_some() {
            0
        } else if let Some(sym) = symbol {
            self.symbol_para_shape(sym)
        } else {
            2
        };
        let mut para = Paragraph {
            para_shape: ParaShapeId(para_shape),
            style: StyleId(self.style),
            chars: std::mem::take(&mut self.chars),
            char_shape_runs: runs,
            controls: std::mem::take(&mut self.controls),
            ..Paragraph::default()
        };
        // FIELD_START(하이퍼링크 등) ExtCtrl ↔ controls 등장순서 연결.
        crate::field::relink_ctrl_index(&mut para);
        self.wchar_pos = 0;
        match &mut self.table {
            Some(tb) => tb.current_row.push(para),
            None => self.paragraphs.push(para),
        }
    }

    /// 지금 열려 있는 목록 항목의 머리 문단모양(없으면 None).
    fn active_list_para_shape(&self) -> Option<u16> {
        self.list_stack
            .last()
            .filter(|f| f.item_open)
            .map(|f| f.para_shape_id)
    }

    /// 목록 진입 — 상위 항목 문단을 닫고 이 수준의 머리 문단모양·정의를 만든다.
    fn start_list(&mut self, start: Option<u64>) {
        // 상위 항목의 문단(예: 중첩 앞 "second")을 먼저 닫는다.
        self.flush_paragraph();
        let level = (self.list_stack.len() as u16 + 1).min(7);
        let para_shape_id = match start {
            // 순서 목록: 번호 정의(내보내기가 numbering_levels로 마커 그림) + NUMBER 머리.
            Some(s) => {
                let def_id = self.numbering_levels.len() as u16;
                let mut levels = vec![NumLevel::default(); 7];
                // 이 목록 수준의 시작 번호를 보존한다(내보내기가 start를 반영).
                levels[(level as usize - 1).min(6)].start = s.max(1) as u32;
                self.numbering_levels.push(levels);
                self.push_list_para_shape(2, level, def_id)
            }
            // 글머리표 목록: 불릿 문자 + BULLET 머리. 글머리 문자는 개조식 사다리
            // (□ → ○ → - → ·)의 아래 두 칸을 쓴다 — 이 도구의 대상이 한국 공문서라
            // 1수준 `-`, 2수준 이하 `·`가 기본이다(`•`는 더 쓰지 않는다).
            None => {
                let def_id = self.bullet_chars.len() as u16;
                self.bullet_chars.push(if level >= 2 { '·' } else { '-' });
                self.push_list_para_shape(3, level, def_id)
            }
        };
        self.list_stack.push(ListFrame {
            para_shape_id,
            item_open: false,
        });
    }

    fn end_list(&mut self) {
        self.flush_paragraph();
        self.list_stack.pop();
    }

    /// 인라인 HTML 태그 — 계약 마크(`u`/`s`/`sup`/`sub`/`strong`/`em`) 토글과 `<br/>`.
    /// 그 외 태그(임의 `<a>`·`<span>` 등)는 경고만 남기고 무시한다.
    fn inline_html_tag(&mut self, h: &str) {
        let tag = h.trim().trim_end_matches('/').to_ascii_lowercase();
        let tag = tag.trim_end_matches('>');
        match tag {
            "<u" => self.underline = true,
            "</u" => self.underline = false,
            "<s" => self.strike = true,
            "</s" => self.strike = false,
            "<sup" => self.sup = true,
            "</sup" => self.sup = false,
            "<sub" => self.sub = true,
            "</sub" => self.sub = false,
            "<strong" => self.bold = true,
            "</strong" => self.bold = false,
            "<em" => self.italic = true,
            "</em" => self.italic = false,
            "<br" => {
                self.chars.push(HwpChar::CharCtrl(10));
                self.wchar_pos += 1;
            }
            _ => self
                .warnings
                .push(format!("인라인 HTML 태그 무시: {}", h.trim())),
        }
    }

    /// 버퍼에 모인 블록 HTML을 파싱해 문단에 병합한다 (계약 docs/design/18).
    /// 파싱 실패(계약 위반)는 문서 생성을 중단하지 않고 경고로 남긴다 — markdown
    /// 변환 자체는 성공시키는 기존 정책(이미지 실패 경고와 동일).
    fn flush_html(&mut self) {
        let html = std::mem::take(&mut self.html_buf);
        if html.trim().is_empty() {
            return;
        }
        self.flush_paragraph();
        let opts = crate::from_html::HtmlImportOptions {
            base_dir: self.base_dir.as_deref(),
            bin_seed: self.bin_streams.len(),
        };
        match crate::from_html::parse_fragment(&html, &opts) {
            Ok(blocks) => self.merge_html_blocks(blocks),
            Err(e) => self
                .warnings
                .push(format!("HTML 블록을 무시합니다(계약 위반): {e}")),
        }
    }

    /// from_html 산출물을 현재 문서에 병합한다. 헤더 컬렉션(문단모양·번호/글머리 정의·
    /// 추가 문자모양)의 인덱스가 각자 0부터 시작하므로 오프셋만큼 시프트해 붙인다.
    fn merge_html_blocks(&mut self, blocks: crate::from_html::HtmlBlocks) {
        let ps_off = self.extra_para_shapes.len() as u16;
        let num_off = self.numbering_levels.len() as u16;
        let bul_off = self.bullet_chars.len() as u16;
        let cs_off = self.extra_char_shapes.len() as u16;
        for mut ps in blocks.extra_para_shapes {
            match (ps.attr1 >> 23) & 0x3 {
                2 => ps.numbering_id += num_off, // 번호 정의 참조
                3 => ps.numbering_id += bul_off, // 글머리 정의 참조
                _ => {}
            }
            self.extra_para_shapes.push(ps);
        }
        self.numbering_levels.extend(blocks.numbering_levels);
        self.bullet_chars.extend(blocks.bullet_chars);
        self.extra_char_shapes.extend(blocks.extra_char_shapes);
        self.extra_fonts.extend(blocks.extra_fonts);
        self.bin_streams.extend(blocks.bin_streams);
        self.warnings.extend(blocks.warnings);
        for mut para in blocks.paragraphs {
            remap_para_ids(&mut para, ps_off, cs_off);
            self.paragraphs.push(para);
        }
    }

    /// 목록 항목용 문단 모양을 만들어 인덱스를 돌려준다.
    /// head_type: 2=번호, 3=글머리표. level 1~7 → 머리 수준(내보내기가 중첩 감지에 사용).
    fn push_list_para_shape(&mut self, head_type: u32, level: u16, def_id: u16) -> u16 {
        let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        // 수준당 들여쓰기(HWPUNIT) — 한글에서 중첩이 눈에 띄게. 내보내기의 중첩 감지는
        // head_level 기준이라 왕복 폐쇄에는 무영향(여백은 실기 표시용).
        let step = 2000i32;
        self.extra_para_shapes.push(ParaShape {
            // 정상 본문 문단모양(0x180: 한글 줄나눔+줄격자) + 왼쪽 정렬 + 머리 종류/수준.
            attr1: 0x180 | (1 << 2) | (head_type << 23) | (u32::from(level) << 25),
            margin_left: i32::from(level) * step,
            indent: -step, // 내어쓰기: 마커와 본문 정렬
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            numbering_id: def_id,
            ..ParaShape::default()
        });
        idx
    }

    /// 개조식 기호 문단(`□ `·`○ `)용 내어쓰기 문단모양을 만들어(기호별 1회) 인덱스를 준다.
    /// push_list_para_shape와 같은 여백 사다리를 쓰되 머리(BULLET) 비트는 세우지 않는다 —
    /// 기호가 이미 본문 텍스트에 있으므로 한글이 마커를 겹쳐 그리면 안 된다.
    fn symbol_para_shape(&mut self, sym: char) -> u16 {
        if let Some(&id) = self.symbol_para_shapes.get(&sym) {
            return id;
        }
        let step = 2000i32;
        // □=1단, ○=2단. 내어쓰기(-step)로 접힌 줄이 기호 뒤 본문에 맞춰 정렬된다.
        let depth = if sym == '□' { 1 } else { 2 };
        let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
        self.extra_para_shapes.push(ParaShape {
            attr1: 0x180 | (1 << 2), // 정상 본문 + 왼쪽 정렬(머리 종류/수준 없음)
            margin_left: depth * step,
            indent: -step,
            // □ 블록 경계가 눈에 보이게 문단 간격을 준다 — □ 위 600은 제목(1)의
            // spacing_top, 공통 아래 300은 제목의 spacing_bottom 관례와 맞춘 값.
            spacing_top: if depth == 1 { 600 } else { 0 },
            spacing_bottom: 300,
            line_spacing_old: 160,
            line_spacing: 160,
            border_fill_id: 2,
            ..ParaShape::default()
        });
        self.symbol_para_shapes.insert(sym, idx);
        idx
    }

    /// 각주/미주 참조를 현재 문단에 심는다 — FOOTNOTE_ENDNOTE ExtCtrl(앵커) +
    /// fn/en GenericControl(본문 문단 리스트). 내보내기가 이 구조를 읽어 `[^N]`을 낸다.
    fn push_footnote(&mut self, label: &str) {
        let ctrl_id = if is_endnote_label(label) {
            *b"en  "
        } else {
            *b"fn  "
        };
        let body = self
            .note_bodies
            .get(label)
            .cloned()
            .unwrap_or_else(|| vec![note_body_para()]);
        // 앵커: ExtCtrl(code 17). payload 12B 앞 4B = 역순 ctrl_id(다른 앵커와 동일 규약).
        let mut payload = vec![0u8; 12];
        let mut rev = ctrl_id;
        rev.reverse();
        payload[..4].copy_from_slice(&rev);
        let idx = self.controls.len() as u32;
        self.chars.push(HwpChar::ExtCtrl {
            code: ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id,
            payload,
            ctrl_index: Some(idx), // flush의 relink_ctrl_index가 최종 재배치
        });
        self.wchar_pos += 8;
        self.controls.push(Control::Generic(GenericControl {
            ctrl_id,
            data: Vec::new(),
            paragraph_lists: vec![ParagraphList {
                header_data: Vec::new(),
                paragraphs: body,
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
        }));
    }

    /// 이미지 참조를 현재 문단에 임베드한다 — 로컬 파일이면 BinStream + 인라인 Picture(글자처럼,
    /// 자연 크기)로 삽입하고 alt를 억제, 실패면(원격/없음/미지원) 경고 후 alt 텍스트를 남긴다.
    fn start_image(&mut self, dest_url: &str) {
        match self.load_image(dest_url) {
            Ok((data, name, w, h)) => {
                let idx = self.controls.len() as u32;
                self.controls.push(Control::Picture(Picture {
                    common_data: Vec::new(),
                    width: HwpUnit(w.max(1)),
                    height: HwpUnit(h.max(1)),
                    treat_as_char: true, // 인라인(글자처럼) 배치 — writer가 도형 레코드 합성
                    z_order: 0,
                    vert_offset: 0,
                    horz_offset: 0,
                    description: None,
                    bin_ref: BinRef::ItemRef(name.clone()),
                    extras: Vec::new(),
                }));
                // gso 앵커 문자(code 11) — insert_image와 동일 규약. relink가 ctrl_index 재배치.
                self.chars.push(HwpChar::ExtCtrl {
                    code: 11,
                    ctrl_id: *b"gso ",
                    payload: crate::field::rev_payload(b"gso "),
                    ctrl_index: Some(idx),
                });
                self.wchar_pos += 8;
                self.bin_streams.push(BinStream { name, data });
                self.in_image_suppress = true;
            }
            Err(warn) => {
                self.warnings.push(warn);
                self.in_image_suppress = false; // alt 텍스트를 폴백으로 보존
            }
        }
    }

    /// 이미지 경로를 해석·판독한다. 성공 시 (바이트, bin 이름, 표시폭, 표시높이).
    /// 로컬 경로(절대 + base_dir 기준 상대)만 허용 — 원격 URL은 네트워크 의존 금지.
    fn load_image(&self, dest_url: &str) -> Result<(Vec<u8>, String, i32, i32), String> {
        let lower = dest_url.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return Err(format!(
                "원격 이미지 URL은 지원하지 않습니다(alt 보존): {dest_url}"
            ));
        }
        // file: 스킴 접두는 벗겨서 로컬 경로로 다룬다.
        let raw = dest_url.strip_prefix("file://").unwrap_or(dest_url);
        let path = Path::new(raw);
        let resolved: PathBuf = if path.is_absolute() {
            path.to_path_buf()
        } else {
            match &self.base_dir {
                Some(dir) => dir.join(path),
                None => {
                    return Err(format!(
                        "상대 경로 이미지의 기준 디렉터리를 알 수 없습니다(alt 보존): {dest_url}"
                    ));
                }
            }
        };
        let data = std::fs::read(&resolved)
            .map_err(|e| format!("이미지 읽기 실패 {}: {e} (alt 보존)", resolved.display()))?;
        if data.is_empty() {
            return Err(format!("빈 이미지 파일(alt 보존): {}", resolved.display()));
        }
        // 매직 바이트로 포맷 판별 — 미지(.bin)면 미지원으로 처리(alt 보존).
        let (ext, _) = crate::image::image_kind(&data);
        if ext == "bin" {
            return Err(format!(
                "지원하지 않는 이미지 형식(alt 보존): {}",
                resolved.display()
            ));
        }
        let (w, h) =
            crate::image::display_size(&data, &crate::image::ImageSize::Natural, BODY_WIDTH);
        let name = format!("md_image{}.{ext}", self.bin_streams.len() + 1);
        Ok((data, name, w, h))
    }

    fn event(&mut self, event: Event<'_>) {
        // 각주/미주 정의 블록은 collect_note_bodies가 선수집했으므로 본문에서 건너뛴다
        // (깊이만 추적). skip 중에는 다른 이벤트를 무시한다.
        if let Event::Start(Tag::FootnoteDefinition(_)) = &event {
            self.skip_note_def += 1;
            return;
        }
        if let Event::End(TagEnd::FootnoteDefinition) = &event {
            self.skip_note_def = self.skip_note_def.saturating_sub(1);
            return;
        }
        if self.skip_note_def > 0 {
            return;
        }
        // 블록 HTML 버퍼는 비(非)Html 이벤트에서 닫는다(연속 Html 이벤트 = 하나의 블록).
        if !matches!(event, Event::Html(_)) {
            self.flush_html();
        }
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                self.flush_paragraph();
                let n = heading_level(level);
                self.heading = Some(n);
                self.style = n; // 개요 N 스타일
                // H1~H3 절 번호 계산: 해당 수준 +1, 하위 0 리셋, 0인 상위는 1로 승격.
                self.pending_heading_num = if n <= 3 {
                    let i = (n - 1) as usize;
                    self.h_counters[i] += 1;
                    for c in &mut self.h_counters[i + 1..] {
                        *c = 0;
                    }
                    for c in &mut self.h_counters[..i] {
                        if *c == 0 {
                            *c = 1;
                        }
                    }
                    let nums: Vec<String> =
                        self.h_counters[..=i].iter().map(u32::to_string).collect();
                    Some(format!("{}. ", nums.join("-")))
                } else {
                    None
                };
                // H2/H3 들여쓰기 문단모양(1회 할당).
                if (2..=3).contains(&n) && self.section_para_shape.is_none() {
                    let idx = BASE_PARA_SHAPES + self.extra_para_shapes.len() as u16;
                    self.extra_para_shapes.push(ParaShape {
                        attr1: 0x180 | (1 << 2),
                        margin_left: 2000,
                        line_spacing_old: 160,
                        line_spacing: 160,
                        border_fill_id: 2,
                        ..ParaShape::default()
                    });
                    self.section_para_shape = Some(idx);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                self.flush_paragraph();
                self.heading = None;
                self.style = 0;
                self.pending_heading_num = None;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => self.flush_paragraph(),
            Event::Start(Tag::Strong) => self.bold = true,
            Event::End(TagEnd::Strong) => self.bold = false,
            Event::Start(Tag::Emphasis) => self.italic = true,
            Event::End(TagEnd::Emphasis) => self.italic = false,
            Event::Start(Tag::Strikethrough) => self.strike = true,
            Event::End(TagEnd::Strikethrough) => self.strike = false,
            Event::Text(t) => {
                // 절 번호 접두: 제목 첫 텍스트 앞에 삽입(이미 번호가 있는 제목은 생략).
                if self.heading.is_some()
                    && let Some(num) = self.pending_heading_num.take()
                    && !starts_with_literal_number(&t)
                {
                    self.push_text(&num);
                }
                if self.in_image_suppress {
                    // 이미지 임베드 성공 → alt 텍스트 억제(그림이 대체한다).
                } else if self.in_codeblock {
                    self.push_code_text(&t); // 코드블록 텍스트의 \n → 줄바꿈
                } else {
                    self.push_text(&t);
                }
            }
            // ── 인라인 코드(`code`) → 함초롬돋움+음영 글자모양 run ──
            Event::Code(t) => {
                self.in_code = true;
                self.push_text(&t);
                self.in_code = false;
            }
            // ── 이미지(`![alt](경로)`) → 인라인 Picture + BinStream (로컬 경로만) ──
            Event::Start(Tag::Image { dest_url, .. }) => self.start_image(&dest_url),
            Event::End(TagEnd::Image) => self.in_image_suppress = false,
            // ── 각주/미주 참조(`[^N]`/`[^eN]`) → FOOTNOTE_ENDNOTE ExtCtrl + fn/en 컨트롤 ──
            Event::FootnoteReference(label) => self.push_footnote(&label),
            // ── 하이퍼링크: [텍스트](url) → %hlk 필드(FIELD_START + 파랑밑줄 텍스트 + FIELD_END) ──
            Event::Start(Tag::Link { dest_url, .. }) => {
                let (start, _end, control) = crate::field::hyperlink_field_parts(&dest_url);
                self.chars.push(start);
                self.wchar_pos += 8; // FIELD_START ExtCtrl = 8 WCHAR
                self.controls.push(control);
                self.in_link = true; // 이후 표시 텍스트는 HYPERLINK 글자모양
                self.link_end = Some(_end);
            }
            Event::End(TagEnd::Link) => {
                if let Some(end) = self.link_end.take() {
                    self.chars.push(end);
                    self.wchar_pos += 8; // FIELD_END InlineCtrl = 8 WCHAR
                }
                self.in_link = false;
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => {
                self.chars.push(HwpChar::CharCtrl(10));
                self.wchar_pos += 1;
            }
            // ── 인용문(> ) → 들여쓰기+좌측 막대 문단(para_shape 3) ──
            Event::Start(Tag::BlockQuote(_)) => {
                self.flush_paragraph();
                self.in_blockquote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                self.flush_paragraph();
                self.in_blockquote = self.in_blockquote.saturating_sub(1);
            }
            // ── 코드블록(```) → 회색 배경 문단(para_shape 4), 줄바꿈 보존 ──
            Event::Start(Tag::CodeBlock(_)) => {
                self.flush_paragraph();
                self.in_codeblock = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                self.flush_paragraph();
                self.in_codeblock = false;
            }
            // ── 순서/글머리 목록 → 머리(NUMBER/BULLET) 문단, 중첩은 수준으로 ──
            Event::Start(Tag::List(start)) => self.start_list(start),
            Event::End(TagEnd::List(_)) => self.end_list(),
            Event::Start(Tag::Item) => {
                if let Some(f) = self.list_stack.last_mut() {
                    f.item_open = true;
                }
            }
            Event::End(TagEnd::Item) => {
                self.flush_paragraph();
                if let Some(f) = self.list_stack.last_mut() {
                    f.item_open = false;
                }
            }
            // ── GFM 표 ──
            Event::Start(Tag::Table(_)) => {
                self.flush_paragraph();
                self.table = Some(TableBuilder::default());
            }
            Event::Start(Tag::TableHead) => {
                if let Some(tb) = &mut self.table {
                    tb.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(tb) = &mut self.table {
                    let row = std::mem::take(&mut tb.current_row);
                    tb.rows.push(row);
                    tb.in_head = false;
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(tb) = &mut self.table {
                    let row = std::mem::take(&mut tb.current_row);
                    tb.rows.push(row);
                }
            }
            Event::Start(Tag::TableCell) => {
                if self.table.as_ref().is_some_and(|tb| tb.in_head) {
                    self.bold = true;
                }
            }
            Event::End(TagEnd::TableCell) => {
                // 빈 셀도 문단 1개를 반드시 만든다(nparas≥1 보장 + 열 수 정합).
                self.flush_paragraph_inner(true);
                self.bold = false;
            }
            Event::End(TagEnd::Table) => {
                if let Some(tb) = self.table.take() {
                    self.paragraphs.push(table_paragraph(tb));
                }
            }
            // ── 블록 HTML(표·그림 — 계약 docs/design/18): 연속 이벤트를 버퍼에 모은다 ──
            Event::Html(h) => self.html_buf.push_str(&h),
            // ── 인라인 HTML(`<u>`·`<sup>`·`<sub>`·`<s>`·`<br/>`) → 마크 토글·줄바꿈 ──
            Event::InlineHtml(h) => self.inline_html_tag(&h),
            _ => {}
        }
    }
}

/// 문단이 개조식 기호 `□ `/`○ `로 시작하면 그 기호를 준다. markdown은 줄 앞 공백을
/// 지워 사다리가 평평해지므로, 이 문단만 여백으로 단을 복원한다(정확히 이 두 접두만).
/// HTML 블록 병합 시프트 — 문단모양 id(≥BASE_PARA_SHAPES)와 추가 문자모양 id
/// (≥PALETTE_LEN)를 재귀적으로 옮긴다(중첩 표 셀·Generic 문단 리스트 포함).
fn remap_para_ids(para: &mut Paragraph, ps_off: u16, cs_off: u16) {
    if para.para_shape.0 >= BASE_PARA_SHAPES {
        para.para_shape.0 += ps_off;
    }
    for (_, id) in &mut para.char_shape_runs {
        if id.0 >= crate::from_html::PALETTE_LEN {
            id.0 += cs_off;
        }
    }
    for control in &mut para.controls {
        match control {
            Control::Table(t) => {
                for cell in &mut t.cells {
                    for p in &mut cell.paragraphs {
                        remap_para_ids(p, ps_off, cs_off);
                    }
                }
            }
            Control::Generic(g) => {
                for list in &mut g.paragraph_lists {
                    for p in &mut list.paragraphs {
                        remap_para_ids(p, ps_off, cs_off);
                    }
                }
            }
            _ => {}
        }
    }
}

fn leading_symbol(chars: &[HwpChar]) -> Option<char> {
    match (chars.first(), chars.get(1)) {
        (Some(HwpChar::Text(sym @ ('□' | '○'))), Some(HwpChar::Text(' '))) => Some(*sym),
        _ => None,
    }
}

/// 제목이 이미 번호를 달고 있는지 — 자동 절번호 접두를 생략할 조건(이중 번호 방지).
/// 아라비아 숫자(`1.`), 전각 로마 숫자(`Ⅰ.`·`ⅰ.`), 한글 항목 기호(`가.`·`나)`).
/// 번호 없는 낱말 제목(`사업 개요`)은 걸리지 않아 자동 번호를 그대로 받는다.
fn starts_with_literal_number(t: &str) -> bool {
    let mut chars = t.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_digit()
        || ('\u{2160}'..='\u{217F}').contains(&first)
        || (('\u{AC00}'..='\u{D7A3}').contains(&first) && matches!(chars.next(), Some('.' | ')')))
}

fn heading_level(level: HeadingLevel) -> u16 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// 첫 문단 앞에 secd/cold(프리셋 시 + pgnp) 확장 컨트롤을 삽입한다
/// (컨트롤당 8 WCHAR 시프트 포함).
pub(crate) fn inject_section_controls(para: &mut Paragraph, preset: Option<OfficialPreset>) {
    use hwp_model::{Control, GenericControl, HwpUnit, PageDef, SectionDef};
    if para
        .controls
        .iter()
        .any(|c| matches!(c, Control::SectionDef(_)))
    {
        return;
    }
    // 삽입할 확장 컨트롤 수: secd + cold (+ 프리셋 쪽번호 pgnp).
    let n_ctrl = if preset.is_some() { 3 } else { 2 };
    // 기존 참조들 시프트
    for ch in &mut para.chars {
        if let HwpChar::ExtCtrl {
            ctrl_index: Some(i),
            ..
        } = ch
        {
            *i += n_ctrl;
        }
    }
    for (pos, _) in &mut para.char_shape_runs {
        *pos += n_ctrl * 8;
    }
    for seg in &mut para.line_segs {
        seg.text_start += n_ctrl * 8;
    }
    let first_shape = para
        .char_shape_runs
        .first()
        .map_or(CharShapeId(0), |(_, id)| *id);
    if para.char_shape_runs.first().map(|(p, _)| *p) != Some(0) {
        para.char_shape_runs.insert(0, (0, first_shape));
    }
    // 연속 동일 id run 병합(secd/cold 삽입으로 생기는 [(0,0),(16,0)] 중복 등)은
    // writer가 합성 경로 전체에 적용한다.

    // 여백: 기본은 한글 새 문서(좌우 30·위 20·아래 15mm), 공문서 프리셋은
    // 작성 규정(위 30·아래 15·좌 20·우 15mm). 머리말/꼬리말 15mm는 공통.
    let (ml, mr, mt, mb) = if preset.is_some() {
        (5668, 4252, 8504, 4252)
    } else {
        (8504, 8504, 5668, 4252)
    };
    let page = PageDef {
        width: HwpUnit(59528),
        height: HwpUnit(84186),
        margin_left: HwpUnit(ml),
        margin_right: HwpUnit(mr),
        margin_top: HwpUnit(mt),
        margin_bottom: HwpUnit(mb),
        margin_header: HwpUnit(4252),
        margin_footer: HwpUnit(4252),
        gutter: HwpUnit(0),
        attr: 0,
    };
    para.controls.insert(
        0,
        Control::SectionDef(SectionDef {
            data: Vec::new(),
            page: Some(page),
            extras: Vec::new(),
            secpr_raw_children: Vec::new(),
            footnote_shape_raw: None,
            endnote_shape_raw: None,
            page_border_fills_raw: Vec::new(),
        }),
    );
    para.controls.insert(
        1,
        Control::Generic(GenericControl {
            ctrl_id: *b"cold",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
        }),
    );
    let ext = |code: u16, ctrl_id: [u8; 4], idx: u32| {
        let mut payload = vec![0u8; 12];
        let mut rev = ctrl_id;
        rev.reverse();
        payload[..4].copy_from_slice(&rev);
        HwpChar::ExtCtrl {
            code,
            ctrl_id,
            payload,
            ctrl_index: Some(idx),
        }
    };
    para.chars.insert(0, ext(2, *b"secd", 0));
    para.chars.insert(1, ext(2, *b"cold", 1));
    if preset.is_some() {
        // 쪽번호 하단 중앙(pgnp, 규정 §10). 12B: props(u32: 서식 DIGIT=0 |
        // 위치 BOTTOM_CENTER=5 <<8) + 예약 6B + sideChar WCHAR — 정품 실측
        // (hwpx read build_pgnp와 동일 레이아웃)은 sideChar '-'("- 1 -" 표기).
        let mut data = vec![0u8; 12];
        data[..4].copy_from_slice(&(5u32 << 8).to_le_bytes());
        data[10..12].copy_from_slice(&(u16::from(b'-')).to_le_bytes());
        para.controls.insert(
            2,
            Control::Generic(GenericControl {
                ctrl_id: *b"pgnp",
                data,
                paragraph_lists: Vec::new(),
                extras: Vec::new(),
                raw_children: Vec::new(),
                gso_shapes: Vec::new(),
                equation: None,
                column_def: None,
            }),
        );
        para.chars.insert(2, ext(21, *b"pgnp", 2));
    }
    // 구역 첫 문단의 break_type — 한글이 직접 저장한 단일 문단 표본 전수
    // (가나다·hello_world·outline·bookmark)가 모두 0x03(bit0 구역나눔 +
    // bit1 다단나눔)이다. secd/cold ExtCtrl를 품은 '구역 첫 문단'에 한글이
    // 항상 쓰는 값으로, 0x00이면 헤더-컨트롤 정합이 깨져 손상 판정된다.
    // (hwp5 왕복 경로는 body_text.rs에서 원본 break_type를 보존하며 이
    // 함수를 거치지 않으므로 바이트동일 게이트에 영향 없음.)
    para.header.break_type = 0x03;
}

/// 수집한 표를 앵커 문단(확장 컨트롤 1개)으로 만든다.
fn table_paragraph(tb: TableBuilder) -> Paragraph {
    let rows = tb.rows.len().max(1);
    let cols = tb.rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let col_w = BODY_WIDTH / cols as i32;
    let row_h = 1700i32; // 10pt 텍스트 + 셀 위아래 여백

    let mut cells = Vec::new();
    for (r, row) in tb.rows.iter().enumerate() {
        for c in 0..cols {
            cells.push(Cell {
                list_attr: CELL_VALIGN_CENTER,
                col: c as u16,
                row: r as u16,
                col_span: 1,
                row_span: 1,
                width: HwpUnit(col_w),
                height: HwpUnit(row_h),
                margins: [510, 510, 141, 141],
                border_fill: BorderFillId(TABLE_BORDER_FILL),
                header_tail: Vec::new(),
                // 셀은 문단 1개 이상 필수(nparas≥1). 짧은 행에서 누락된 칸은
                // 빈 문단으로 채운다 — nparas=0 셀은 한글이 손상 처리한다. 채움
                // 문단도 PARA_CHAR_SHAPE run 1개를 가져야 한다(정품 전수 불변식,
                // writer는 char_shape_runs가 비면 레코드를 방출하지 않음).
                paragraphs: row.get(c).cloned().map_or_else(
                    || {
                        vec![Paragraph {
                            char_shape_runs: vec![(0, CharShapeId(0))],
                            ..Paragraph::default()
                        }]
                    },
                    |p| vec![p],
                ),
            });
        }
    }
    let table = Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: rows as u16,
        cols: cols as u16,
        cell_spacing: 0,
        inner_margins: [510, 510, 141, 141],
        row_cell_counts: vec![cols as u16; rows],
        border_fill: BorderFillId(TABLE_BORDER_FILL),
        table_tail: Vec::new(),
        cells,
        extras: Vec::new(),
    };

    let mut payload = vec![0u8; 12];
    payload[..4].copy_from_slice(b" lbt"); // 역순 ctrl_id
    Paragraph {
        chars: vec![
            HwpChar::ExtCtrl {
                code: 11,
                ctrl_id: *b"tbl ",
                payload,
                ctrl_index: Some(0),
            },
            HwpChar::CharCtrl(13),
        ],
        char_shape_runs: vec![(0, CharShapeId(0))],
        controls: vec![Control::Table(table)],
        ..Paragraph::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_header_크기() {
        let h = default_header();
        assert_eq!(h.char_shapes[0].base_size, 1000); // 본문 10pt
        assert_eq!(h.char_shapes[4].base_size, 1800); // H1 = 본문 × 1.8
    }

    #[test]
    fn 표_셀_세로정렬_가운데() {
        // 정품 한글 표 셀 기본 = 세로 정렬 가운데(list_attr bits5-6=1=0x20). 0(위)이면
        // hwp5 writer가 그대로 방출해 셀 내용이 상단에 붙는다(위 여백<아래 여백).
        use hwp_model::Control;
        let doc = from_markdown("| 가 | 나 |\n|----|----|\n| 1 | 2 |\n");
        let table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음");
        assert!(!table.cells.is_empty());
        for c in &table.cells {
            assert_eq!(
                (c.list_attr >> 5) & 3,
                1,
                "셀 세로정렬 가운데(0x20): {:#x}",
                c.list_attr
            );
        }
    }

    /// GI-1/GI-2 왕복 (a): md → IR → md 에서 각주·취소선·순서목록(start)·중첩이 보존.
    #[test]
    fn 왕복_각주_취소선_순서목록_중첩() {
        let md = "\
문단에 각주[^1]가 있다.

~~지운 글~~ 과 보통 글.

1. 첫째
2. 둘째
   - 안쪽 가
   - 안쪽 나
3. 셋째

[^1]: 각주 본문이다.
";
        let doc = from_markdown(md);
        let out = crate::markdown::to_markdown(&doc);

        // 각주: 본문 마커 + 문서 끝 정의.
        assert!(out.contains("[^1]"), "각주 마커: {out}");
        assert!(out.contains("[^1]: 각주 본문이다."), "각주 정의: {out}");
        // 취소선.
        assert!(out.contains("~~지운 글~~"), "취소선: {out}");
        // 순서 목록(1./2./3.).
        assert!(out.contains("1. 첫째"), "순서1: {out}");
        assert!(out.contains("2. 둘째"), "순서2: {out}");
        assert!(out.contains("3. 셋째"), "순서3: {out}");
        // 중첩 글머리 목록(들여쓰기된 `-`).
        assert!(out.contains("- 안쪽 가"), "중첩 불릿: {out}");
        let idx = out.find("안쪽 가").unwrap();
        let line_start = out[..idx].rfind('\n').map_or(0, |p| p + 1);
        assert!(
            out[line_start..idx].starts_with(' '),
            "중첩은 들여쓰기: {out}"
        );
    }

    /// 순서 목록 start 보존: `3.`으로 시작하면 왕복 후에도 `3.`.
    #[test]
    fn 왕복_순서목록_start_보존() {
        let doc = from_markdown("3. 셋\n4. 넷\n");
        let out = crate::markdown::to_markdown(&doc);
        assert!(out.contains("3. 셋"), "start=3 보존: {out}");
        assert!(out.contains("4. 넷"), "다음 번호: {out}");
    }

    /// 미주(`[^eN]`)도 대칭 왕복.
    #[test]
    fn 왕복_미주() {
        let doc = from_markdown("본문[^e1] 끝.\n\n[^e1]: 미주 본문.\n");
        let out = crate::markdown::to_markdown(&doc);
        assert!(out.contains("[^e1]"), "미주 마커: {out}");
        assert!(out.contains("[^e1]: 미주 본문."), "미주 정의: {out}");
    }

    /// 각주 컨트롤이 fn GenericControl + FOOTNOTE_ENDNOTE 앵커로 합성되는지(구조 단언).
    #[test]
    fn 각주_컨트롤_구조() {
        let doc = from_markdown("가[^1]나\n\n[^1]: 각주.\n");
        let para = &doc.sections[0].paragraphs[0];
        let has_anchor = para.chars.iter().any(|c| {
            matches!(c, HwpChar::ExtCtrl { code, ctrl_id, .. }
                if *code == hwp_model::ctrl_char::FOOTNOTE_ENDNOTE && ctrl_id == b"fn  ")
        });
        assert!(has_anchor, "각주 앵커 존재");
        let has_ctrl = para.controls.iter().any(|c| {
            matches!(c,
            Control::Generic(g) if g.ctrl_id == *b"fn  " && !g.paragraph_lists.is_empty())
        });
        assert!(has_ctrl, "각주 컨트롤+본문 존재");
    }

    /// 테스트용 최소 PNG(치수 헤더만) 파일을 쓰고 경로를 돌려준다.
    fn write_png(dir: &std::path::Path, name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend(w.to_be_bytes());
        png.extend(h.to_be_bytes());
        png.extend([0u8; 8]);
        let p = dir.join(name);
        std::fs::write(&p, &png).unwrap();
        p
    }

    /// GI-3: 로컬 이미지 `![alt](fig.png)` → 인라인 Picture + BinStream(자연 크기).
    #[test]
    fn 이미지_로컬_임베드() {
        let dir = std::env::temp_dir().join("hwp-md-img-embed");
        std::fs::create_dir_all(&dir).unwrap();
        write_png(&dir, "fig.png", 96, 48);
        let doc = from_markdown_with(
            "본문\n\n![대체텍스트](fig.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&dir),
                preset: None,
            },
        );
        assert_eq!(doc.bin_streams.len(), 1, "BinStream 1개 임베드");
        let pic = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Picture(p) => Some(p),
                _ => None,
            })
            .expect("Picture 존재");
        assert!(pic.treat_as_char, "인라인(글자처럼) 배치");
        assert!(pic.extras.is_empty(), "writer 합성용 빈 extras");
        assert!(doc.resolve_bin(&pic.bin_ref).is_some(), "bin_ref 해석");
        assert_eq!(pic.width.0, 96 * 7200 / 96, "자연 크기(96px→7200)");
        // 성공한 이미지의 alt 텍스트는 억제된다.
        assert!(!doc.plain_text().contains("대체텍스트"), "alt 억제");
    }

    /// GI-3: 없는 파일·원격 URL·상대경로(기준 없음)는 경고 후 alt 텍스트를 보존한다.
    #[test]
    fn 이미지_실패는_alt_보존() {
        let dir = std::env::temp_dir().join("hwp-md-img-fail");
        std::fs::create_dir_all(&dir).unwrap();
        // 없는 파일.
        let d1 = from_markdown_with(
            "![없음alt](nope.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&dir),
                preset: None,
            },
        );
        assert!(d1.bin_streams.is_empty(), "임베드 없음");
        assert!(d1.plain_text().contains("없음alt"), "alt 보존");
        // 원격 URL(네트워크 금지).
        let d2 = from_markdown("![원격alt](https://example.com/a.png)\n");
        assert!(d2.bin_streams.is_empty());
        assert!(d2.plain_text().contains("원격alt"), "원격은 alt 보존");
        // 상대경로 + 기준 디렉터리 없음.
        let d3 = from_markdown("![상대alt](fig.png)\n");
        assert!(d3.bin_streams.is_empty());
        assert!(d3.plain_text().contains("상대alt"), "기준없음은 alt 보존");
    }

    /// GI-3 왕복: md(이미지)→IR→#8 exporter(media_dir) 재수출 시 이미지 데이터 보존.
    #[test]
    fn 이미지_왕복_exporter_데이터보존() {
        let dir = std::env::temp_dir().join("hwp-md-img-rt");
        std::fs::create_dir_all(&dir).unwrap();
        let png_path = write_png(&dir, "rt.png", 32, 32);
        let orig = std::fs::read(&png_path).unwrap();
        let doc = from_markdown_with(
            "![x](rt.png)\n",
            &MarkdownImportOptions {
                base_dir: Some(&dir),
                preset: None,
            },
        );
        let media = dir.join("out_media");
        let _ = std::fs::remove_dir_all(&media);
        let md = crate::markdown::to_markdown_with(
            &doc,
            &crate::markdown::MarkdownOptions {
                media_dir: Some(&media),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(md.contains("!["), "이미지 참조 재수출: {md}");
        let extracted = std::fs::read(media.join("image1.png")).expect("추출 이미지");
        assert_eq!(extracted, orig, "추출 이미지 바이트 == 원본(무손실)");
        let _ = std::fs::remove_dir_all(&media);
    }

    /// GI-4: 인라인 코드 `code` → 함초롬돋움(face_id=1) + 연회색 음영 글자모양 run.
    #[test]
    fn 인라인_코드_글자모양() {
        let doc = from_markdown("이건 `let x = 1;` 코드다.\n");
        let code_id = shapes::CODE;
        let cs = &doc.header.char_shapes[code_id as usize];
        assert_eq!(cs.face_ids[0], FONT_DOTUM, "함초롬돋움 face_id");
        assert_eq!(cs.shade_color, 0x00F0_F0F0, "연회색 음영");
        // 코드 텍스트가 CODE 글자모양 run으로 적재됐는지.
        let para = &doc.sections[0].paragraphs[0];
        let has_code_run = para.char_shape_runs.iter().any(|(_, id)| id.0 == code_id);
        assert!(has_code_run, "CODE run 존재: {:?}", para.char_shape_runs);
        // 함초롬돋움 글꼴이 테이블에 있다.
        assert_eq!(doc.header.fonts[0][FONT_DOTUM as usize].name, "함초롬돋움");
    }

    /// push_text: 탭은 InlineCtrl(9)로, 그 외 C0 제어문자는 드롭, 일반 문자는 Text로.
    #[test]
    fn push_text_탭_인라인컨트롤_제어문자_드롭() {
        let mut b = Builder::default();
        b.push_text("A\tB\u{0001}C");
        let kinds: Vec<_> = b.chars.to_vec();
        assert_eq!(kinds.len(), 4, "A, 탭, B, C (0x01 드롭): {kinds:?}");
        assert!(matches!(kinds[0], HwpChar::Text('A')));
        assert!(matches!(kinds[1], HwpChar::InlineCtrl { code: 9, .. }));
        assert!(matches!(kinds[2], HwpChar::Text('B')));
        assert!(matches!(kinds[3], HwpChar::Text('C')));
        // wchar_pos = 1 + 8 + 1 + 1 (0x01은 소비 안 함).
        assert_eq!(b.wchar_pos, 11);
    }

    /// H1~H3 절 번호 사다리: 카운터 증가·하위 리셋·상위 0 승격 + H2/H3 들여쓰기 모양.
    #[test]
    fn 헤딩_절번호_카운터() {
        let doc = from_markdown("# 서론\n## 배경\n## 목적\n# 본론\n### 세부\n");
        let text = doc.plain_text();
        for want in [
            "1. 서론",
            "1-1. 배경",
            "1-2. 목적",
            "2. 본론",
            "2-1-1. 세부",
        ] {
            assert!(text.contains(want), "{want} 없음: {text}");
        }
        // H2/H3 문단은 SECTION 문단모양(margin_left 2000)을 쓴다.
        let ps_of = |needle: &str| {
            let p = doc.sections[0]
                .paragraphs
                .iter()
                .find(|p| {
                    p.chars.iter().any(|c| matches!(c, HwpChar::Text(_)))
                        && plain_of(p).contains(needle)
                })
                .unwrap();
            p.para_shape.0
        };
        fn plain_of(p: &Paragraph) -> String {
            p.chars
                .iter()
                .filter_map(|c| match c {
                    HwpChar::Text(ch) => Some(*ch),
                    _ => None,
                })
                .collect()
        }
        let section_ps = ps_of("1-1. 배경");
        assert!(section_ps >= BASE_PARA_SHAPES, "SECTION은 확장 모양");
        assert_eq!(ps_of("2-1-1. 세부"), section_ps, "H2/H3 동일 모양");
        assert_eq!(
            doc.header.para_shapes[section_ps as usize].margin_left,
            2000
        );
        assert_eq!(ps_of("1. 서론"), 1, "H1은 기본 제목 모양");
    }

    /// 숫자로 시작하는 제목은 절 번호 접두를 생략한다(이중 번호 방지).
    #[test]
    fn 헤딩_이중번호_방지() {
        let doc = from_markdown("# 1. 서론\n");
        let text = doc.plain_text();
        assert!(text.contains("1. 서론"), "{text}");
        assert!(!text.contains("1. 1. 서론"), "이중 번호 금지: {text}");
    }

    /// 개조식 리터럴 번호(전각 로마 숫자·한글 항목 기호)도 자동 절번호를 막는다.
    /// 단 번호 없는 낱말 제목은 그대로 자동 번호를 받는다(가드 과확장 방지).
    #[test]
    fn 헤딩_리터럴_번호_이중번호_방지() {
        let doc = from_markdown("# Ⅰ. 사업 개요\n## 가. 배경\n## 나) 세부\n");
        let text = doc.plain_text();
        assert!(text.contains("Ⅰ. 사업 개요"), "{text}");
        assert!(!text.contains("1. Ⅰ."), "전각 로마 숫자 이중 번호: {text}");
        assert!(!text.contains("1-1. 가."), "한글 `가.` 이중 번호: {text}");
        assert!(!text.contains("1-2. 나)"), "한글 `나)` 이중 번호: {text}");

        let plain = from_markdown("## 사업 개요\n").plain_text();
        assert!(
            plain.contains("1-1. 사업 개요"),
            "낱말 제목은 자동 번호: {plain}"
        );
    }

    /// 글머리 문자 사다리: 1수준 `-`, 2수준 이하 `·`(개조식 표준, `•` 폐기).
    #[test]
    fn 글머리_사다리_수준별_문자() {
        let doc = from_markdown("- 상위\n  - 하위\n");
        assert_eq!(doc.header.bullet_chars, vec!['-', '·']);
    }

    /// `□ `/`○ ` 본문 문단은 내어쓰기 문단모양(여백 사다리)을 받고, 머리(BULLET)
    /// 비트는 세우지 않는다(기호가 이미 텍스트에 있음). 그 외 문단은 그대로 본문(2).
    #[test]
    fn 개조식_기호_문단_내어쓰기() {
        let doc = from_markdown("□ 현황\n\n○ 세부\n\n□ 계획\n\n일반 문단\n");
        let ps_of = |needle: &str| {
            doc.sections[0]
                .paragraphs
                .iter()
                .find(|p| {
                    p.chars
                        .iter()
                        .filter_map(|c| match c {
                            HwpChar::Text(ch) => Some(*ch),
                            _ => None,
                        })
                        .collect::<String>()
                        .contains(needle)
                })
                .unwrap_or_else(|| panic!("{needle} 문단 없음"))
                .para_shape
                .0
        };
        let square = ps_of("□ 현황");
        let circle = ps_of("○ 세부");
        assert_eq!(ps_of("□ 계획"), square, "같은 기호는 문단모양 재사용");
        assert_eq!(ps_of("일반 문단"), 2, "기호 없는 본문은 기본 본문 모양");

        let shape = |id: u16| &doc.header.para_shapes[id as usize];
        assert_eq!(
            (shape(square).margin_left, shape(square).indent),
            (2000, -2000)
        );
        assert_eq!(
            (shape(circle).margin_left, shape(circle).indent),
            (4000, -2000)
        );
        assert_eq!(shape(square).head_type(), 0, "머리(BULLET) 비트 없음");
        assert_eq!(shape(circle).head_type(), 0, "머리(BULLET) 비트 없음");
        // 문단 간격: □ 블록 경계가 보이도록 위 600, 공통 아래 300 (○는 위 0).
        assert_eq!(
            (shape(square).spacing_top, shape(square).spacing_bottom),
            (600, 300)
        );
        assert_eq!(
            (shape(circle).spacing_top, shape(circle).spacing_bottom),
            (0, 300)
        );
    }

    /// 표 셀·제목·목록 항목은 기호 문단모양의 영향을 받지 않는다.
    #[test]
    fn 개조식_기호_예외_경로() {
        use hwp_model::Control;
        let doc = from_markdown("# □ 제목\n\n- □ 항목\n\n| □ 머리 |\n|----|\n| □ 셀 |\n");
        let heading = &doc.sections[0].paragraphs[0];
        assert_eq!(heading.para_shape.0, 1, "제목은 제목 문단모양");
        let table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("표 없음");
        for cell in &table.cells {
            assert_eq!(cell.paragraphs[0].para_shape.0, 0, "표 셀은 셀 문단모양");
        }
        let item = doc.sections[0]
            .paragraphs
            .iter()
            .find(|p| p.para_shape.0 >= BASE_PARA_SHAPES)
            .expect("목록 항목 없음");
        assert_eq!(
            doc.header.para_shapes[item.para_shape.0 as usize].head_type(),
            3,
            "목록 항목은 BULLET 머리 유지"
        );
    }

    /// 공문서 프리셋: 규정 여백·글꼴/크기·4단계 번호·쪽번호(pgnp)가 적용되고,
    /// 프리셋 없는 기본 경로는 기존 값 그대로여야 한다.
    #[test]
    fn 공문서_프리셋() {
        use hwp_model::{Control, NumFmt};
        let md = "# 제목\n\n1. 하나\n   1. 둘\n\n본문\n";
        let opts = |p| MarkdownImportOptions {
            base_dir: None,
            preset: p,
        };
        let page_of = |doc: &Document| {
            doc.sections[0].paragraphs[0]
                .controls
                .iter()
                .find_map(|c| match c {
                    Control::SectionDef(sd) => sd.page,
                    _ => None,
                })
                .expect("PageDef 있어야")
        };

        // 기본(프리셋 없음): 기존 여백·크기·번호 형식 유지.
        let plain = from_markdown_with(md, &opts(None));
        let p = page_of(&plain);
        assert_eq!(
            (
                p.margin_left.0,
                p.margin_top.0,
                p.margin_right.0,
                p.margin_bottom.0
            ),
            (8504, 5668, 8504, 4252)
        );
        assert_eq!(plain.header.char_shapes[0].base_size, 1000);
        assert!(plain.header.numbering_levels[0][1].template.is_empty());
        assert!(
            !plain.sections[0].paragraphs[0]
                .controls
                .iter()
                .any(|c| matches!(c, Control::Generic(g) if g.ctrl_id == *b"pgnp"))
        );

        // report: 여백 위30/아래15/좌20/우15mm, 함초롬바탕 15pt, H1 18pt.
        let report = from_markdown_with(md, &opts(Some(OfficialPreset::Report)));
        let p = page_of(&report);
        assert_eq!(
            (
                p.margin_left.0,
                p.margin_top.0,
                p.margin_right.0,
                p.margin_bottom.0
            ),
            (5668, 8504, 4252, 4252)
        );
        assert_eq!(report.header.fonts[0][0].name, "함초롬바탕");
        assert_eq!(report.header.char_shapes[0].base_size, 1500);
        assert_eq!(report.header.char_shapes[4].base_size, 1800); // H1
        assert_eq!(report.header.char_shapes[15].base_size, 1500); // 인라인 코드도 본문 크기

        // gian: 맑은 고딕 11.5pt, H1 15pt.
        let gian = from_markdown_with(md, &opts(Some(OfficialPreset::Gian)));
        assert_eq!(gian.header.fonts[0][0].name, "맑은 고딕");
        assert_eq!(
            gian.header.fonts[0][1].name, "함초롬돋움",
            "인라인 코드 글꼴 유지"
        );
        assert_eq!(gian.header.char_shapes[0].base_size, 1150);
        assert_eq!(gian.header.char_shapes[4].base_size, 1500);

        // 4단계 번호 사다리: ^1. / ^2.(가나다) / ^3) / ^4)(가나다), 5수준부터 반복.
        let levels = &report.header.numbering_levels[0];
        let fmt_tpl: Vec<(NumFmt, &str)> = levels
            .iter()
            .map(|l| (l.fmt, l.template.as_str()))
            .collect();
        assert_eq!(fmt_tpl[0], (NumFmt::Digit, "^1."));
        assert_eq!(fmt_tpl[1], (NumFmt::HangulSyllable, "^2."));
        assert_eq!(fmt_tpl[2], (NumFmt::Digit, "^3)"));
        assert_eq!(fmt_tpl[3], (NumFmt::HangulSyllable, "^4)"));
        assert_eq!(fmt_tpl[4], (NumFmt::Digit, "^5."));

        // 쪽번호: pgnp(하단 중앙 + sideChar '-') 컨트롤과 ExtCtrl 앵커.
        let first = &report.sections[0].paragraphs[0];
        let pgnp = first
            .controls
            .iter()
            .find_map(|c| match c {
                Control::Generic(g) if g.ctrl_id == *b"pgnp" => Some(g),
                _ => None,
            })
            .expect("pgnp 있어야");
        let props = u32::from_le_bytes(pgnp.data[..4].try_into().unwrap());
        assert_eq!((props >> 8) & 0xFF, 5, "BOTTOM_CENTER");
        assert_eq!(
            u16::from_le_bytes(pgnp.data[10..12].try_into().unwrap()),
            u16::from(b'-')
        );
        assert!(
            first
                .chars
                .iter()
                .any(|ch| matches!(ch, HwpChar::ExtCtrl { ctrl_id, .. } if ctrl_id == b"pgnp"))
        );
    }

    // ── md + HTML 혼합(계약 docs/design/18) ──

    #[test]
    fn html_표_블록_혼합() {
        let doc = from_markdown(
            "본문입니다.\n\n<table>\n<tr><td colspan=\"2\">가로병합</td></tr>\n\
             <tr><td>a</td><td>b</td></tr>\n</table>\n",
        );
        let table = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.controls)
            .find_map(|c| match c {
                Control::Table(t) => Some(t),
                _ => None,
            })
            .expect("HTML 표 컨트롤");
        assert_eq!((table.rows, table.cols), (2, 2));
        assert_eq!(table.cells.len(), 3, "앵커 셀만: {}", table.cells.len());
        assert_eq!(table.cells[0].col_span, 2);
        assert_eq!(table.row_cell_counts, vec![1, 2]);
    }

    #[test]
    fn 인라인_html_마크_왕복() {
        // md에 섞인 <u>/<sup>/<sub>가 IR을 거쳐 md 재수출까지 보존(GH-8 대칭).
        let doc = from_markdown("이건 <u>밑줄</u>이고 x<sup>2</sup>입니다\n");
        let md = crate::markdown::to_markdown(&doc);
        assert!(md.contains("<u>밑줄</u>"), "밑줄 왕복: {md}");
        assert!(md.contains("<sup>2</sup>"), "위첨자 왕복: {md}");
    }

    #[test]
    fn 계약_위반_html은_경고로_무시() {
        // 미지원 태그 블록은 문서 생성을 깨지 않고 무시한다(기존 이미지 경고 정책과 동일).
        let doc = from_markdown("본문\n\n<div><p>무시됨</p></div>\n");
        let text: String = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.chars)
            .filter_map(|c| match c {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(text.contains("본문"), "{text}");
        assert!(!text.contains("무시됨"), "{text}");
    }

    #[test]
    fn html_목록_병합_오프셋() {
        // md 목록 뒤 HTML ol이 오면 번호 정의가 둘 다 살아 있어야 한다(오프셋 병합).
        let doc = from_markdown("1. md 항목\n\n<ol><li>html 항목</li></ol>\n");
        assert_eq!(
            doc.header.numbering_levels.len(),
            2,
            "md+html 번호 정의: {}",
            doc.header.numbering_levels.len()
        );
        let text: String = doc.sections[0]
            .paragraphs
            .iter()
            .flat_map(|p| &p.chars)
            .filter_map(|c| match c {
                HwpChar::Text(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert!(
            text.contains("md 항목") && text.contains("html 항목"),
            "{text}"
        );
    }
}
