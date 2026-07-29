//! 렌더링 스모크 테스트.
//!
//! 픽셀 골든 비교는 폰트 가용성에 좌우되므로(CI 폰트 고정은 M7),
//! 여기서는 구조적 불변식만 검증한다: 페이지 수/크기, 텍스트 영역에
//! 어두운 픽셀 존재, 본문 영역 밖은 흰색.

use std::path::PathBuf;

use hwp_render::{RenderOptions, render_document};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// fixture 문서는 저장소에 없으므로(로컬 전용 — fixtures/README.md) 없으면 건너뛴다.
fn fixture_or_skip(rel: &str) -> Option<PathBuf> {
    let p = fixture(rel);
    if !p.exists() {
        eprintln!(
            "스킵: fixture 없음 ({}) — fixtures/README.md 참고",
            p.display()
        );
        return None;
    }
    Some(p)
}

/// 어두운 픽셀(텍스트) 수를 센다.
fn dark_pixels(pixmap: &tiny_skia::Pixmap) -> usize {
    pixmap
        .pixels()
        .iter()
        .filter(|p| p.red() < 128 && p.green() < 128 && p.blue() < 128)
        .count()
}

#[test]
fn hello_world_렌더() {
    let Some(path) = fixture_or_skip("hwp5/hello_world.hwp") else {
        return;
    };
    let doc = hwp5::read_document(&path).unwrap().document;
    let out = render_document(
        &doc,
        &RenderOptions {
            dpi: 96.0,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(out.pages.len(), 1);
    let page = &out.pages[0];
    // A4 @96dpi: 59528/7200*96 ≈ 793.7 → 794
    assert_eq!(page.width(), 794);
    assert_eq!(page.height(), 1123);

    // "Hello World!" 텍스트가 그려졌는지 (시스템에 폰트가 하나라도 있으면)
    let dark = dark_pixels(page);
    assert!(dark > 100, "텍스트 픽셀이 너무 적음: {dark}");

    // 본문 영역 밖(여백)은 흰색이어야 한다 — 좌상단 모서리
    let corner = page.pixel(5, 5).unwrap();
    assert_eq!(
        (corner.red(), corner.green(), corner.blue()),
        (255, 255, 255)
    );
}

#[test]
fn hwpx_폴백_렌더() {
    // minimal.hwpx의 문단 대부분은 lineseg가 없다 — 폴백 경로 검증
    let Some(path) = fixture_or_skip("hwpx/minimal.hwpx") else {
        return;
    };
    let doc = hwpx::read_document(&path).unwrap().document;
    let out = render_document(&doc, &RenderOptions::default()).unwrap();
    assert_eq!(out.pages.len(), 1);
    assert!(
        dark_pixels(&out.pages[0]) > 500,
        "세 문단이 모두 그려져야 한다"
    );
}

#[test]
fn 다단_2단_렌더() {
    // multicol.hwp/.hwpx = 한글 2단 본문(정답지). 단 넘김을 페이지 넘김으로 오인하던 버그를
    // 고쳐 5쪽이 아니라 3쪽(2단×2쪽 + 잔여 1쪽)이 되고, 1쪽에 좌·우 단이 나란히 그려져야 한다.
    for rel in ["hwp5/multicol.hwp", "hwpx/multicol.hwpx"] {
        let Some(path) = fixture_or_skip(rel) else {
            continue;
        };
        let doc = if rel.ends_with(".hwp") {
            hwp5::read_document(&path).unwrap().document
        } else {
            hwpx::read_document(&path).unwrap().document
        };
        let out = render_document(
            &doc,
            &RenderOptions {
                dpi: 96.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            out.pages.len(),
            3,
            "{rel}: 2단이면 3쪽(단 넘김≠페이지 넘김)"
        );
        // 1쪽 좌·우 절반 모두에 내용(어두운 픽셀)이 있어야 한다 = 두 단 나란히.
        let p = &out.pages[0];
        let (w, hh) = (p.width(), p.height());
        let dark_in = |x0: u32, x1: u32| {
            let mut n = 0usize;
            for y in 0..hh {
                for x in x0..x1 {
                    if p.pixel(x, y).unwrap().red() < 128 {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(dark_in(0, w / 2) > 500, "{rel}: 좌 단 내용 부족");
        assert!(dark_in(w / 2, w) > 500, "{rel}: 우 단 내용 부족");
    }
}

#[test]
fn 표_렌더() {
    let Some(path) = fixture_or_skip("hwp5/work_report.hwp") else {
        return;
    };
    let doc = hwp5::read_document(&path).unwrap().document;
    let out = render_document(&doc, &RenderOptions::default()).unwrap();
    assert_eq!(out.pages.len(), 1);
    let page = &out.pages[0];

    // 표 테두리 + 셀 텍스트로 어두운 픽셀이 충분해야 한다
    assert!(
        dark_pixels(page) > 5_000,
        "표 선·텍스트: {}",
        dark_pixels(page)
    );

    // 표·머리말·꼬리말은 더 이상 미지원으로 집계되지 않는다 (글상자 1개만 남음)
    let skipped: Vec<_> = out
        .report
        .iter()
        .filter(|w| w.contains("미지원 컨트롤"))
        .collect();
    assert!(
        skipped.iter().all(|w| w.contains("1개")),
        "표/머리말이 미지원으로 집계됨: {skipped:?}"
    );
}

/// 멀티페이지 문서의 합성 줄 배치는 페이지마다 v_pos 가 0 으로 리셋(페이지 상대)
/// 되어야 한다. 리셋 없이 섹션 단조 누적하면 v_pos 가 페이지 본문 높이를 한참
/// 초과해(정품은 페이지 상대) 한글이 '손상'으로 판정한다(커밋 29014b0).
/// 폰트 없이도(문단당 1줄) 페이지 분할 로직만 검증한다.
#[test]
fn 멀티페이지_lineseg_페이지_상대_v_pos() {
    let md: String = (1..=120)
        .map(|i| format!("{i}번째 문단입니다. 페이지를 넘기기 위한 본문.\n\n"))
        .collect();
    let mut doc = hwp_convert::from_markdown(&md);

    let page = doc.sections[0].section_def().unwrap().page.unwrap();
    let content_h = page.height.0 - page.margin_top.0 - page.margin_bottom.0;

    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);

    let vs: Vec<i32> = doc.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| p.line_segs.iter().map(|s| s.v_pos))
        .collect();

    assert!(
        vs.len() >= 120,
        "문단마다 줄 배치가 합성되어야: {}",
        vs.len()
    );
    let maxv = *vs.iter().max().unwrap();
    assert!(
        maxv <= content_h,
        "모든 v_pos 는 페이지 본문 높이({content_h}) 이내여야 한다(페이지 상대) — 최댓값 {maxv}"
    );
    let resets = vs.windows(2).filter(|w| w[1] < w[0]).count();
    assert!(
        resets >= 1,
        "한 페이지를 넘기는 문서는 v_pos 리셋이 있어야 한다 — 리셋 {resets}회"
    );
}

/// 문단 위/아래 간격(spacing_top/bottom)이 합성 줄 배치 v_pos 에 반영되어야 한다.
/// 빠지면 한글이 문단 사이 여백 없이 압축해 그린다(제목 위 여백 사라짐 등).
/// from_markdown 은 제목에 spacing_top=600, spacing_bottom=300 을 준다.
#[test]
fn 문단_간격이_v_pos에_반영() {
    let mut doc = hwp_convert::from_markdown("# 제목\n\n본문 문단.\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);

    let paras = &doc.sections[0].paragraphs;
    let h = &paras[0].line_segs[0]; // 제목 (한 줄)
    let b = &paras[1].line_segs[0]; // 본문 (한 줄)
    // 본문 첫 줄 v_pos = 제목 줄 v_pos + 제목 line_advance + 제목 아래간격(300).
    let heading_advance = h.line_height + h.line_spacing;
    assert_eq!(
        b.v_pos - h.v_pos,
        heading_advance + 300,
        "본문 v_pos 는 제목 advance + 제목 아래간격(300) 만큼 떨어져야"
    );
}

#[test]
fn 빈_문서_렌더() {
    let Some(path) = fixture_or_skip("hwp5/bookmark.hwp") else {
        return;
    };
    let doc = hwp5::read_document(&path).unwrap().document;
    let out = render_document(&doc, &RenderOptions::default()).unwrap();
    assert_eq!(out.pages.len(), 1);
    assert_eq!(dark_pixels(&out.pages[0]), 0, "빈 문서는 흰 페이지");
}

/// 수식 조판(equation.rs): 스크립트를 실제 math로 배치한다. 분수(over)는 분수선(Item::Line)을,
/// 첨자(^/_)·근호(sqrt)·기호는 글리프를 만든다. 폰트 유무와 무관하게 분수선은 그려져야 한다.
#[test]
fn 수식_조판_렌더() {
    use hwp_model::{Control, Equation, GenericControl};
    let mut doc = hwp_convert::from_markdown("수식:\n");
    let scripts = [
        "a over b",
        "x^2 + y_i",
        "sqrt {a+b}",
        "E=mc^2",
        "alpha + beta over 2",
    ];
    for (i, sc) in scripts.iter().enumerate() {
        doc.sections[0]
            .paragraphs
            .first_mut()
            .unwrap()
            .controls
            .push(Control::Generic(GenericControl {
                ctrl_id: *b"eqed",
                data: vec![],
                paragraph_lists: vec![],
                extras: vec![],
                raw_children: vec![],
                gso_shapes: vec![],
                equation: Some(Equation {
                    script: sc.to_string(),
                    width: 12000,
                    height: 3500,
                    inline: false,
                    x: 8000,
                    y: 6000 + i as i32 * 5000,
                    ..Equation::default()
                }),
                column_def: None,
            }));
    }
    let out = render_document(
        &doc,
        &RenderOptions {
            dpi: 120.0,
            ..Default::default()
        },
    )
    .unwrap();
    // 분수 2개(over) → 분수선 Item::Line ≥ 2, 그리고 글리프 픽셀.
    if std::env::var_os("HWP_EQ_PNG").is_some() {
        out.pages[0].save_png("/tmp/eq_test.png").ok();
    }
    assert!(
        dark_pixels(&out.pages[0]) > 200,
        "수식 글리프가 그려져야: {}",
        dark_pixels(&out.pages[0])
    );
}

/// 정답지 수식 문서(equation.hwp/.hwpx): 실제 한글 수식 스크립트(다행 `#`·분수·첨자·근호·
/// 그리스)를 두 포맷 모두 조판해 그려야 한다. hwp5는 eqed 파싱, hwpx는 hp:equation 캡처.
/// 스크립트가 같으므로 두 렌더의 잉크량이 비슷해야 한다(조판 일관성).
#[test]
fn 수식_정답지_렌더() {
    let (Some(hp), Some(hx)) = (
        fixture_or_skip("hwp5/equation.hwp"),
        fixture_or_skip("hwpx/equation.hwpx"),
    ) else {
        return;
    };
    let d5 = hwp5::read_document(&hp).unwrap().document;
    let dx = hwpx::read_document(&hx).unwrap().document;
    let opt = RenderOptions {
        dpi: 120.0,
        ..Default::default()
    };
    let (o5, ox) = (
        render_document(&d5, &opt).unwrap(),
        render_document(&dx, &opt).unwrap(),
    );
    let (p5, px) = (dark_pixels(&o5.pages[0]), dark_pixels(&ox.pages[0]));
    assert!(p5 > 300, "hwp5 수식이 조판돼야(eqed 파싱): {p5}");
    assert!(px > 300, "hwpx 수식이 조판돼야: {px}");
    // 같은 스크립트 → 두 포맷 잉크량이 2배 이내로 비슷해야 한다.
    let ratio = p5.max(px) as f32 / p5.min(px).max(1) as f32;
    assert!(
        ratio < 2.0,
        "hwp5({p5})/hwpx({px}) 조판 불일치: 비 {ratio:.1}"
    );
}

/// 연결 다단 글상자: annual_report "At a Glance"(5쪽)는 월 텍스트가 왼쪽→오른쪽 단으로
/// 흐른다. (1) 글자 베이스라인이 페이지 하단을 넘지 않아야 하고(흐름 드리프트/잘림 회귀
/// 방지), (2) 오른쪽 단(x≈300pt)에 본문이 배치돼야 한다(다단 흐름). 폰트 무관 — 배치는
/// 캐시 v_pos·글상자 위치가 좌우한다.
#[test]
fn 글상자_연결_다단_배치() {
    let Some(path) = fixture_or_skip("hwp5/annual_report.hwp") else {
        return;
    };
    let doc = hwp5::read_document(&path).unwrap().document;
    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    assert!(
        list.pages.len() >= 5,
        "annual_report 는 5쪽 이상: {}",
        list.pages.len()
    );

    let page = &list.pages[4]; // 5쪽 (0-기반)
    let glyphs: Vec<(f32, f32)> = page
        .items
        .iter()
        .filter_map(|it| match it {
            hwp_render::display::Item::Glyphs { x, y, .. } => Some((*x, *y)),
            _ => None,
        })
        .collect();
    assert!(!glyphs.is_empty(), "5쪽에 글자가 있어야 한다");

    // (1) 세로 넘침 없음
    let max_y = glyphs.iter().map(|(_, y)| *y).fold(0.0_f32, f32::max);
    assert!(
        max_y <= page.height_pt,
        "5쪽 글자 베이스라인({max_y:.1}pt)이 페이지 하단({:.1}pt)을 넘음 — 글상자 드리프트",
        page.height_pt
    );

    // (2) 오른쪽 단 배치 (연결 다단 글상자가 둘째 단을 우측으로 흘림)
    let right_col = glyphs
        .iter()
        .any(|(x, y)| (280.0..330.0).contains(x) && (200.0..800.0).contains(y));
    assert!(
        right_col,
        "오른쪽 단(x≈300pt)에 본문이 없음 — 다단 글상자 미배치"
    );
}

/// 그리기 개체(도형) 렌더: annual_report의 선/사각형/타원/호/다각형이 Item::Path로
/// 생성되고, 미지원 컨트롤로 생략되지 않아야 한다. 파이(링) 페이지엔 곡선(CubicTo)
/// 경로(타원/호)가 있어야 한다. 폰트 무관 — 배치는 도형 기하·행렬이 좌우.
#[test]
fn 도형_렌더_경로_생성() {
    use hwp_render::display::{Item, PathCmd};
    let Some(path) = fixture_or_skip("hwp5/annual_report.hwp") else {
        return;
    };
    let doc = hwp5::read_document(&path).unwrap().document;
    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);

    let paths = list
        .pages
        .iter()
        .flat_map(|p| &p.items)
        .filter(|i| matches!(i, Item::Path { .. }))
        .count();
    // 보이지 않는 글상자 프레임은 제외되므로 가시 도형(선 43·타원·호·다각형 등)만 ~80개.
    assert!(
        paths > 50,
        "도형 경로가 너무 적음: {paths} (선·사각형·타원 등 미렌더)"
    );

    // 파이(링) 페이지: 타원/호 유래 곡선(CubicTo) 경로 존재.
    let has_curve = list.pages.iter().flat_map(|p| &p.items).any(|i| {
        matches!(i, Item::Path { commands, .. }
            if commands.iter().any(|c| matches!(c, PathCmd::CubicTo(..))))
    });
    assert!(has_curve, "타원/호 유래 곡선 경로가 없음 (파이/원 미렌더)");

    // 도형이 더 이상 "미지원 컨트롤"로 집계되지 않아야 한다.
    let skipped = warns.iter().filter(|w| w.contains("미지원 컨트롤")).count();
    assert_eq!(
        skipped, 0,
        "아직 미지원으로 집계되는 도형이 있음: {warns:?}"
    );
}

/// 그러데이션 채움이 백엔드에서 실제 그러데이션으로 렌더되는지(단색 근사가 아니라).
/// 도형 fixture가 없어 합성 DisplayList로 검증한다.
#[test]
fn 그러데이션_채움_백엔드() {
    use hwp_render::display::{DisplayList, Fill, Gradient, Item, PageList, PathCmd};
    let page = PageList {
        width_pt: 100.0,
        height_pt: 100.0,
        items: vec![Item::Path {
            commands: vec![
                PathCmd::MoveTo(10.0, 10.0),
                PathCmd::LineTo(90.0, 10.0),
                PathCmd::LineTo(90.0, 90.0),
                PathCmd::LineTo(10.0, 90.0),
                PathCmd::Close,
            ],
            fill: Some(Fill::Gradient(Gradient {
                radial: false,
                angle_deg: 0.0,                                      // 가로
                stops: vec![(0.0, 0x0000_00FF), (1.0, 0x00FF_0000)], // 빨강→파랑
            })),
            stroke: None,
        }],
    };
    let list = DisplayList { pages: vec![page] };

    // SVG: <linearGradient> 정의 + url 참조
    let svg = hwp_render::svg::render_svg(&list).remove(0);
    assert!(svg.contains("<linearGradient"), "SVG 그러데이션 정의 없음");
    assert!(svg.contains("url(#grad0)"), "SVG fill url 참조 없음");

    // PNG: 좌(빨강)와 우(파랑)가 달라야 한다(실제 그러데이션).
    let pngs = hwp_render::png::render_png(&list, 96.0).unwrap();
    let px = &pngs[0];
    let mid = px.height() / 2;
    let left = px.pixel(20, mid).unwrap();
    let right = px.pixel(px.width() - 20, mid).unwrap();
    assert!(
        left.red() > right.red() && left.blue() < right.blue(),
        "좌측은 빨강, 우측은 파랑이어야 — 좌({},{}) 우({},{})",
        left.red(),
        left.blue(),
        right.red(),
        right.blue()
    );
}

/// GC-8 내어쓰기(음수 first-line indent): 첫 줄이 나머지 줄보다 왼쪽에 놓여야 한다.
/// 폴백(캐시 없는) 문단 경로를 탄다 — 합성 문서(line_segs 없음)라 layout이 그리디
/// 줄바꿈한다. 픽셀 골든이 아니라 DisplayList의 글리프 x를 줄별로 비교한다.
#[test]
fn 내어쓰기_첫줄이_왼쪽() {
    use hwp_render::display::Item;
    // 여러 줄로 넘치도록 충분히 긴 한 문단.
    let mut doc = hwp_convert::from_markdown(&"가".repeat(400));
    // 이 문단의 문단모양에 좌여백(60pt) + 내어쓰기(-40pt) 설정. IR 여백류는 2×HWPUNIT.
    let psid = doc.sections[0].paragraphs[0].para_shape.0 as usize;
    doc.header.para_shapes[psid].margin_left = 12000; // /200 = 60pt
    doc.header.para_shapes[psid].indent = -8000; // /200 = -40pt (내어쓰기)

    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);

    // (y=베이스라인, x) 글리프 목록.
    let glyphs: Vec<(f32, f32)> = list.pages[0]
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Glyphs { x, y, .. } => Some((*y, *x)),
            _ => None,
        })
        .collect();
    assert!(glyphs.len() >= 2, "여러 줄로 줄바꿈돼야: {}", glyphs.len());

    let min_y = glyphs.iter().map(|(y, _)| *y).fold(f32::INFINITY, f32::min);
    // 첫 줄(min_y)의 최소 x.
    let first_x = glyphs
        .iter()
        .filter(|(y, _)| (*y - min_y).abs() < 0.5)
        .map(|(_, x)| *x)
        .fold(f32::INFINITY, f32::min);
    // 더 아래 줄(둘째 줄 이후)의 최소 x.
    let rest_x = glyphs
        .iter()
        .filter(|(y, _)| *y > min_y + 0.5)
        .map(|(_, x)| *x)
        .fold(f32::INFINITY, f32::min);
    assert!(rest_x.is_finite(), "둘째 줄이 있어야 한다(줄바꿈 발생)");
    assert!(
        first_x < rest_x - 1.0,
        "내어쓰기: 첫 줄 x({first_x:.1})이 나머지 줄 x({rest_x:.1})보다 왼쪽이어야"
    );
}

/// GC-9 페이지 걸친 문단 배경: 배경 border_fill을 가진 긴 문단이 페이지를 넘기면
/// 각 페이지에 배경 조각(Rect)이 그려져야 한다(통째 생략 금지). 합성 line_segs로
/// 멀티페이지를 만들고, 두 페이지 모두에 그 채움색 Rect가 있는지 DisplayList로 확인한다.
#[test]
fn 페이지_걸친_문단배경_조각() {
    use hwp_model::BorderFill;
    use hwp_render::display::{Item, PageList};

    // 여러 페이지를 넘길 만큼 아주 긴 한 문단.
    let mut doc = hwp_convert::from_markdown(&"가".repeat(4000));

    // 가시 배경 border_fill 추가 → 첫 문단 문단모양이 참조(id는 1-based).
    let fill_color = 0x00FF_EEDDu32;
    doc.header.border_fills.push(BorderFill {
        bg_color: Some(fill_color),
        fill_type: 1,
        ..BorderFill::default()
    });
    let bf_id = doc.header.border_fills.len() as u16;
    let psid = doc.sections[0].paragraphs[0].para_shape.0 as usize;
    doc.header.para_shapes[psid].border_fill_id = bf_id;

    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);

    assert!(
        list.pages.len() >= 2,
        "문단이 페이지를 걸쳐야 한다: {}쪽",
        list.pages.len()
    );
    let has_bg = |p: &PageList| {
        p.items
            .iter()
            .any(|it| matches!(it, Item::Rect { fill, .. } if *fill == fill_color))
    };
    assert!(
        has_bg(&list.pages[0]),
        "1쪽에 배경 조각(Rect)이 있어야 한다"
    );
    assert!(
        has_bg(&list.pages[1]),
        "2쪽에도 배경 조각(Rect)이 있어야 한다 — 페이지 걸친 배경 통째 생략 금지"
    );
}

/// 쪽 테두리(PAGE_BORDER_FILL BOTH) 렌더: 정답지 BF#7(4변 실선 0.4mm 검정)을 종이
/// 기준 gap 1417(≈5mm)로 주입하면 용지 가장자리에서 gap만큼 안쪽에 4변 Line이 그려지고
/// (색·굵기·위치 반영), 텍스트 뒤(맨 앞 삽입)에 놓여야 한다. id=1(무테두리)·PAGE_BORDER
/// 미존재는 무출력(기본 문서 불변).
#[test]
fn 쪽_테두리_렌더() {
    use hwp_model::{BorderFill, BorderLine, Control, Document};
    use hwp_render::display::{Item, PageList};

    // PAGE_BORDER_FILL 14바이트 합성: attr u32 + gap u16×4(왼/오/위/아래) + 테두리ID u16.
    fn raw(attr: u32, gap: [u16; 4], id: u16) -> Vec<u8> {
        let mut v = Vec::with_capacity(14);
        v.extend_from_slice(&attr.to_le_bytes());
        for g in gap {
            v.extend_from_slice(&g.to_le_bytes());
        }
        v.extend_from_slice(&id.to_le_bytes());
        v
    }

    // 첫 SectionDef의 page_border_fills_raw에 BOTH 레코드를 주입한다(순서=BOTH/EVEN/ODD).
    fn inject(doc: &mut Document, data: Vec<u8>) {
        for para in &mut doc.sections[0].paragraphs {
            for c in &mut para.controls {
                if let Control::SectionDef(sd) = c {
                    sd.page_border_fills_raw.push(data);
                    return;
                }
            }
        }
        panic!("SectionDef 없음 — from_markdown 구조 변경?");
    }

    fn lines(page: &PageList) -> Vec<(f32, f32, f32, f32, u32, f32)> {
        page.items
            .iter()
            .filter_map(|it| match it {
                Item::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    color,
                    width,
                } => Some((*x1, *y1, *x2, *y2, *color, *width)),
                _ => None,
            })
            .collect()
    }

    fn layout(doc: &Document) -> hwp_render::display::DisplayList {
        let mut store = hwp_render::FontStore::new();
        let mut warns = Vec::new();
        hwp_render::layout::layout_document(doc, &mut store, &mut warns)
    }

    // A4 종이(from_markdown 기본): 595.28 × 841.86 pt.
    const PAPER_W: f32 = 595.28;
    const PAPER_H: f32 = 841.86;
    const GAP_PT: f32 = 14.17; // 1417 HWPUNIT ≈ 5mm
    // 0.4mm(굵기 인덱스 6) → pt.
    let expect_w = 0.4 * 72.0 / 25.4;

    // border_fills: id 7(index 6) = 4변 실선 0.4mm 검정. index 0(id 1)=무테두리(기본).
    let real_border = BorderFill {
        sides: [BorderLine {
            line_type: 1,
            width: 6,
            color: 0,
        }; 4],
        ..BorderFill::default()
    };

    // (A) BOTH=id7 실테두리 → 4변 Line, 종이 가장자리에서 gap만큼 안쪽.
    {
        let mut doc = hwp_convert::from_markdown("본문 한 줄.\n");
        while doc.header.border_fills.len() < 6 {
            doc.header.border_fills.push(BorderFill::default());
        }
        doc.header.border_fills.push(real_border.clone());
        inject(&mut doc, raw(1, [1417; 4], 7)); // attr bit0=1(종이 기준)

        let list = layout(&doc);
        let page = &list.pages[0];
        let ls = lines(page);
        assert_eq!(ls.len(), 4, "4변(전 변 실선)이 그려져야: {}", ls.len());

        // 맨 앞 4개가 테두리(텍스트 뒤에 그림).
        for i in 0..4 {
            assert!(
                matches!(page.items[i], Item::Line { .. }),
                "쪽 테두리는 페이지 맨 앞(뒤에 그림)에 삽입돼야 한다"
            );
        }

        // 색·굵기.
        for &(_, _, _, _, color, width) in &ls {
            assert_eq!(color, 0, "테두리 색은 검정(0)");
            assert!((width - expect_w).abs() < 0.01, "굵기 {width} ≠ {expect_w}");
        }

        // 위치: 사각형 경계가 종이 가장자리에서 gap만큼 안쪽(gap 반영).
        let minx = ls.iter().map(|l| l.0.min(l.2)).fold(f32::MAX, f32::min);
        let maxx = ls.iter().map(|l| l.0.max(l.2)).fold(f32::MIN, f32::max);
        let miny = ls.iter().map(|l| l.1.min(l.3)).fold(f32::MAX, f32::min);
        let maxy = ls.iter().map(|l| l.1.max(l.3)).fold(f32::MIN, f32::max);
        assert!((minx - GAP_PT).abs() < 0.1, "좌변 안쪽 gap: {minx}");
        assert!((miny - GAP_PT).abs() < 0.1, "상변 안쪽 gap: {miny}");
        assert!(
            (PAPER_W - maxx - GAP_PT).abs() < 0.1,
            "우변 안쪽 gap: {}",
            PAPER_W - maxx
        );
        assert!(
            (PAPER_H - maxy - GAP_PT).abs() < 0.1,
            "하변 안쪽 gap: {}",
            PAPER_H - maxy
        );
        // 4변 각각 축 정렬(수직/수평).
        for &(x1, y1, x2, y2, ..) in &ls {
            let axis = (x1 - x2).abs() < 0.01 || (y1 - y2).abs() < 0.01;
            assert!(axis, "변은 축 정렬이어야: ({x1},{y1})-({x2},{y2})");
        }
    }

    // (B) id=1(전 변 무테두리) 주입 → 무출력.
    {
        let mut doc = hwp_convert::from_markdown("본문.\n");
        inject(&mut doc, raw(1, [1417; 4], 1));
        let list = layout(&doc);
        assert!(
            lines(&list.pages[0]).is_empty(),
            "id=1(무테두리)은 쪽 테두리를 그리지 않아야 한다"
        );
    }

    // (C) PAGE_BORDER_FILL 미존재(기본 문서) → 무출력(기존 렌더 불변).
    {
        let doc = hwp_convert::from_markdown("본문.\n");
        let list = layout(&doc);
        assert!(
            lines(&list.pages[0]).is_empty(),
            "PAGE_BORDER_FILL 없는 기본 문서는 쪽 테두리 무출력"
        );
    }
}

/// 목록(불릿) 문단: 마커는 내어쓰기 구간에, 첫 줄 텍스트는 좌여백(left)에 온다.
/// 과거엔 첫 줄 텍스트가 left+indent(마커 자리)까지 당겨져 마커가 글자 밑에 깔렸다.
/// 글리프 유무는 폰트 가용성에 좌우되므로(CLAUDE.md CI 규칙) 글리프가 없으면 스킵.
#[test]
fn 목록_마커_내어쓰기_배치() {
    use hwp_render::display::Item;
    let doc = hwp_convert::from_markdown("- 항목입니다\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = Vec::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    let glyphs: Vec<(f32, &str)> = list.pages[0]
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Glyphs { x, run, .. } => Some((*x, run.text.as_str())),
            _ => None,
        })
        .collect();
    if glyphs.is_empty() {
        eprintln!("스킵: 사용 가능한 폰트 없음 — 글리프 미생성");
        return;
    }
    let marker_x = glyphs
        .iter()
        .find(|(_, t)| t.contains('-'))
        .map(|(x, _)| *x)
        .expect("불릿 마커(-) 글리프가 있어야");
    let text_x = glyphs
        .iter()
        .find(|(_, t)| t.contains('항'))
        .map(|(x, _)| *x)
        .expect("본문 첫 글자 글리프가 있어야");
    // 문단모양: margin_left=2000(10pt), indent=-2000. 본문 좌변 = 페이지 여백
    // 8504HU(from_markdown 기본) = 85.04pt, left = 그 + 10pt.
    let body_left = 85.04_f32;
    let left = body_left + 10.0;
    assert!(
        (text_x - left).abs() < 0.5,
        "첫 줄 텍스트는 left({left})에 와야: {text_x}"
    );
    assert!(
        marker_x < text_x && marker_x >= body_left - 0.5,
        "마커는 내어쓰기 구간([{body_left}, {text_x}))에 와야: {marker_x}"
    );
}

fn generic_control(
    ctrl_id: [u8; 4],
    data: Vec<u8>,
    paragraph_lists: Vec<hwp_model::ParagraphList>,
) -> hwp_model::Control {
    hwp_model::Control::Generic(hwp_model::GenericControl {
        ctrl_id,
        data,
        paragraph_lists,
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: Vec::new(),
        equation: None,
        column_def: None,
    })
}

fn page_texts(page: &hwp_render::display::PageList) -> Vec<&str> {
    page.items
        .iter()
        .filter_map(|item| match item {
            hwp_render::display::Item::Glyphs { run, .. } => Some(run.text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn 쪽번호_시작_재시작_숨김() {
    let mut doc = hwp_convert::from_markdown_with(
        "첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n",
        &hwp_convert::MarkdownImportOptions {
            base_dir: None,
            preset: Some(hwp_convert::OfficialPreset::Gian),
        },
    );
    doc.header.properties.start_numbers[0] = 3;
    let paras = &mut doc.sections[0].paragraphs;
    assert!(paras.len() >= 3);
    paras[1].header.break_type |= 0x04;
    let mut nwno = vec![0u8; 6];
    nwno[4..6].copy_from_slice(&10u16.to_le_bytes());
    paras[1]
        .controls
        .push(generic_control(*b"nwno", nwno, Vec::new()));
    paras[2].header.break_type |= 0x04;
    paras[2].controls.push(generic_control(
        *b"pghd",
        (1u32 << 5).to_le_bytes().to_vec(),
        Vec::new(),
    ));

    let mut store = hwp_render::FontStore::new();
    let mut warnings = Vec::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    if warnings.iter().any(|w| w.contains("쪽번호 셰이핑 실패")) {
        eprintln!("스킵: 사용 가능한 폰트 없음 - 쪽번호 글리프 미생성");
        return;
    }
    assert_eq!(list.pages.len(), 3);
    assert!(page_texts(&list.pages[0]).contains(&"- 3 -"));
    assert!(page_texts(&list.pages[1]).contains(&"- 10 -"));
    assert!(
        !page_texts(&list.pages[2])
            .iter()
            .any(|t| t.starts_with("- "))
    );
    assert!(
        warnings.iter().all(|w| !w.contains("렌더 미지원 컨트롤")),
        "쪽번호 제어가 미지원으로 집계됨: {warnings:?}"
    );
}

#[test]
fn 머리말_atno는_페이지마다_현재_쪽번호로_치환() {
    let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n");
    doc.header.properties.start_numbers[0] = 5;
    doc.sections[0].paragraphs[1].header.break_type |= 0x04;
    doc.sections[0].paragraphs[1].controls.push(generic_control(
        *b"pghd",
        (1u32 << 5).to_le_bytes().to_vec(),
        Vec::new(),
    ));
    doc.sections[0].paragraphs[2].header.break_type |= 0x04;

    let atno = generic_control(*b"atno", vec![0u8; 12], Vec::new());
    let header_para = hwp_model::Paragraph {
        chars: vec![hwp_model::HwpChar::ExtCtrl {
            code: 18,
            ctrl_id: *b"atno",
            payload: vec![0u8; 12],
            ctrl_index: Some(0),
        }],
        char_shape_runs: vec![(0, hwp_model::CharShapeId(0))],
        controls: vec![atno],
        ..hwp_model::Paragraph::default()
    };
    let header = generic_control(
        *b"head",
        vec![0u8; 8],
        vec![hwp_model::ParagraphList {
            header_data: Vec::new(),
            paragraphs: vec![header_para],
        }],
    );
    doc.sections[0].paragraphs[0].controls.push(header);

    let mut store = hwp_render::FontStore::new();
    let mut warnings = Vec::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    if warnings.iter().any(|w| w.contains("셰이핑 실패")) {
        eprintln!("스킵: 사용 가능한 폰트 없음 - atno 글리프 미생성");
        return;
    }
    assert_eq!(list.pages.len(), 3);
    assert!(page_texts(&list.pages[0]).contains(&"5"));
    assert!(!page_texts(&list.pages[1]).contains(&"6"));
    assert!(page_texts(&list.pages[2]).contains(&"7"));
    assert!(
        warnings.iter().all(|w| !w.contains("렌더 미지원 컨트롤")),
        "atno/head가 미지원으로 집계됨: {warnings:?}"
    );
}
