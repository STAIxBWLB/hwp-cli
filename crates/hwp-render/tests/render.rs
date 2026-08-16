//! 렌더링 스모크 테스트.
//!
//! 픽셀 골든 비교는 폰트 가용성에 좌우되므로(CI 폰트 고정은 M7),
//! 여기서는 구조적 불변식만 검증한다: 페이지 수/크기, 텍스트 영역에
//! 어두운 픽셀 존재, 본문 영역 밖은 흰색.

use std::path::PathBuf;

use hwp_render::{RenderOptions, render_document, render_document_pages};

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
fn selected_page_render_reports_total_without_rasterizing_other_pages() {
    let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n\n셋째 쪽\n");
    doc.sections[0].paragraphs[1].header.break_type |= 0x04;
    doc.sections[0].paragraphs[2].header.break_type |= 0x04;
    let out = render_document_pages(
        &doc,
        &RenderOptions {
            dpi: 36.0,
            font_dirs: Vec::new(),
        },
        Some(&[2]),
    )
    .unwrap();
    assert_eq!(out.total_pages, 3);
    assert_eq!(out.pages.len(), 1);
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
        .issues
        .iter()
        .filter(|issue| issue.code == hwp_render::RenderIssueCode::UnsupportedControlOmitted)
        .collect();
    assert!(
        skipped.iter().all(|issue| issue.count == 1),
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
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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

/// Hancom-saved linesegs declare boundaries with bit0 (first line of a page)
/// and bit1 (first line of a column). Flags must win when they conflict with
/// the v_pos-reset heuristic.
#[test]
fn lineseg_flags가_v_pos_휴리스틱에_우선() {
    let mut doc = hwp_convert::from_markdown("첫 문단\n\n둘째 문단\n\n셋째 문단\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);

    // Keep v_pos monotonic to disable the heuristic and put bit0 only on the
    // second paragraph.
    for (i, p) in doc.sections[0].paragraphs.iter_mut().enumerate() {
        assert_eq!(p.line_segs.len(), 1, "문단당 1줄 가정");
        p.line_segs[0].v_pos = (i as i32) * 2000;
        p.line_segs[0].flags = if i == 1 { 0x0006_0001 } else { 0x0006_0000 };
    }

    let out = render_document(
        &doc,
        &RenderOptions {
            dpi: 36.0,
            font_dirs: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(
        out.pages.len(),
        2,
        "flags bit0(페이지 첫 줄)은 v_pos 증가와 무관하게 페이지를 넘겨야 한다"
    );
}

#[test]
fn authoritative_page_flags_preserve_an_empty_page_without_a_leading_page() {
    let mut doc = hwp_convert::from_markdown("first\n\nblank\n\nthird\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);

    assert_eq!(doc.sections[0].paragraphs.len(), 3);
    doc.sections[0].paragraphs[1].chars = vec![hwp_model::HwpChar::CharCtrl(
        hwp_model::ctrl_char::PARA_BREAK,
    )];
    for paragraph in &mut doc.sections[0].paragraphs {
        assert_eq!(paragraph.line_segs.len(), 1);
        paragraph.line_segs[0].v_pos = 0;
        paragraph.line_segs[0].flags = 0x0006_0001;
    }

    let out = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    assert_eq!(
        out.pages.len(),
        3,
        "the initial bit0 must not add a leading page"
    );
    assert!(!out.pages[0].items.is_empty());
    assert_eq!(
        out.pages[1].items.len(),
        0,
        "the empty flow band must survive"
    );
}

#[test]
fn authoritative_column_flags_preserve_an_empty_first_column() {
    use hwp_model::{ColumnDef, Control, GenericControl, HwpChar};

    let mut doc = hwp_convert::from_markdown("blank\n\nright column\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);

    assert_eq!(doc.sections[0].paragraphs.len(), 2);
    doc.sections[0].paragraphs[0].chars = vec![HwpChar::CharCtrl(hwp_model::ctrl_char::PARA_BREAK)];
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(GenericControl {
            ctrl_id: *b"cold",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: Some(ColumnDef {
                count: 2,
                kind: 0,
                direction: 0,
                same_width: true,
                gap: 1000,
                widths: Vec::new(),
                divider: None,
            }),
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));
    for paragraph in &mut doc.sections[0].paragraphs {
        assert_eq!(paragraph.line_segs.len(), 1);
        paragraph.line_segs[0].v_pos = 0;
        paragraph.line_segs[0].flags = 0x0006_0002;
    }

    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    assert_eq!(list.pages.len(), 1);
    let page = &list.pages[0];
    assert!(page.items.iter().any(|item| matches!(
        item,
        hwp_render::display::Item::Glyphs { x, .. } if *x > page.width_pt / 2.0
    )));
    assert!(!page.items.iter().any(|item| matches!(
        item,
        hwp_render::display::Item::Glyphs { x, .. } if *x < page.width_pt / 2.0
    )));
}

/// HWPX 컨테이너(hp:container)는 container_box 원점에 자식 도형(상대좌표)을
/// 전개해 Item::Path로 렌더하고, unsupported_control_omitted를 기록하지 않는다.
#[test]
fn hwpx_container_children_render_at_container_origin() {
    use hwp_model::{ContainerBox, Control, GenericControl, ShapeGeom, ShapeKind};

    let mut doc = hwp_convert::from_markdown("container host paragraph\n");
    let shape = |kind, x, y, w, h| ShapeGeom {
        kind,
        x,
        y,
        w,
        h,
        points: Vec::new(),
        fill: 0xFFFF_FFFF,
        fill_gradient: None,
        border_color: 0,
        border_width: 40,
        round_ratio: 0,
        border_style: 0,
        arrow_start: 0,
        arrow_end: 0,
        anchored: false,
        description: None,
    };
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(GenericControl {
            ctrl_id: *b"cont",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: vec![
                shape(ShapeKind::Rect, 1000, 2000, 5000, 3000),
                shape(ShapeKind::Ellipse, 3000, 4000, 2000, 1000),
            ],
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: Some(ContainerBox {
                x: 10000,
                y: 20000,
                w: 6000,
                h: 2400,
                anchored: false,
                skipped_objects: 0,
                text_boxes: Vec::new(),
            }),
        }));

    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);

    // 자식 도형 2개가 컨테이너 원점(10000, 20000)만큼 이동해 Path로 나와야 한다.
    // rect 절대좌표 (11000, 22000) HWPUNIT → MoveTo(110.0, 220.0) pt.
    let page = &list.pages[0];
    let rect_path = page.items.iter().find_map(|item| match item {
        hwp_render::display::Item::Path { commands, .. } => commands.first(),
        _ => None,
    });
    assert!(
        matches!(
            rect_path,
            Some(hwp_render::display::PathCmd::MoveTo(px, py))
                if (*px - 110.0).abs() < 0.01 && (*py - 220.0).abs() < 0.01
        ),
        "컨테이너 자식 rect가 원점 기준 절대좌표로 전개돼야 한다: {rect_path:?}"
    );
    let path_count = page
        .items
        .iter()
        .filter(|item| matches!(item, hwp_render::display::Item::Path { .. }))
        .count();
    assert!(path_count >= 2, "자식 도형 2개가 Path로 나와야 한다");
    let report = warns.finish();
    assert!(
        report
            .issues
            .iter()
            .all(|issue| issue.code != hwp_render::RenderIssueCode::UnsupportedControlOmitted),
        "container는 unsupported_control_omitted 대상이 아니다: {:?}",
        report.issues
    );
}

/// 컨테이너에 파싱되지 않은 가시 자식(skipped_objects > 0)이 있으면 typed
/// UnsupportedControlOmitted 손실을 기록한다.
#[test]
fn hwpx_container_skipped_objects_raise_typed_loss() {
    use hwp_model::{ContainerBox, Control, GenericControl};

    let mut doc = hwp_convert::from_markdown("container host paragraph\n");
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(GenericControl {
            ctrl_id: *b"cont",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: Some(ContainerBox {
                x: 10000,
                y: 20000,
                w: 6000,
                h: 2400,
                anchored: false,
                skipped_objects: 2,
                text_boxes: Vec::new(),
            }),
        }));

    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);
    let _list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    let report = warns.finish();
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == hwp_render::RenderIssueCode::UnsupportedControlOmitted),
        "skipped_objects > 0이면 typed loss가 있어야 한다: {:?}",
        report.issues
    );
}

/// 자식 도형 소유 subList는 컨테이너 상자가 아니라 각 도형 상자(text_boxes)에
/// 조판된다 — 두 도형의 텍스트 y 위치가 달라야 한다.
#[test]
fn hwpx_container_child_text_layout_in_owning_boxes() {
    use hwp_model::{ContainerBox, Control, GenericControl, ParagraphList};

    let mut doc = hwp_convert::from_markdown("container host paragraph\n");
    let para_of = |text: &str| {
        hwp_convert::from_markdown(text)
            .sections
            .remove(0)
            .paragraphs
            .remove(0)
    };
    let list_of = |text: &str| ParagraphList {
        header_data: Vec::new(),
        paragraphs: vec![para_of(text)],
    };
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(GenericControl {
            ctrl_id: *b"cont",
            data: Vec::new(),
            paragraph_lists: vec![list_of("첫째 도형 텍스트\n"), list_of("둘째 도형 텍스트\n")],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: Some(ContainerBox {
                x: 10000,
                y: 10000,
                w: 8000,
                h: 6000,
                anchored: false,
                skipped_objects: 0,
                // 첫째 리스트는 (0, 0) 상자, 둘째는 (0, 4000) 상자 소유.
                text_boxes: vec![Some([0, 0, 4000, 2000]), Some([0, 4000, 4000, 2000])],
            }),
        }));

    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    hwp_render::lineseg::synthesize_linesegs(&mut doc, &mut store, &mut warns);
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);

    let y_of = |needle: &str| {
        list.pages[0].items.iter().find_map(|item| match item {
            hwp_render::display::Item::Glyphs { y, run, .. } if run.text.contains(needle) => {
                Some(*y)
            }
            _ => None,
        })
    };
    let (Some(y1), Some(y2)) = (y_of("첫째 도형"), y_of("둘째 도형")) else {
        eprintln!("스킵: 사용 가능한 폰트 없음 - 글리프 미생성");
        return;
    };
    // 컨테이너 원점 y=100pt 기준 첫째 상자 0pt / 둘째 상자 40pt 오프셋.
    assert!(
        (y1 - 100.0).abs() < 1.0,
        "첫째 텍스트는 첫째 상자(y=100pt)에: {y1}"
    );
    assert!(
        (y2 - 140.0).abs() < 1.0,
        "둘째 텍스트는 둘째 상자(y=140pt)에: {y2}"
    );
}

/// 문단 위/아래 간격(spacing_top/bottom)이 합성 줄 배치 v_pos 에 반영되어야 한다.
/// 빠지면 한글이 문단 사이 여백 없이 압축해 그린다(제목 위 여백 사라짐 등).
/// from_markdown 은 제목에 spacing_top=600, spacing_bottom=300 을 준다.
#[test]
fn 문단_간격이_v_pos에_반영() {
    let mut doc = hwp_convert::from_markdown("# 제목\n\n본문 문단.\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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

/// Equation layout emits fraction bars as paths and scripts, radicals, and
/// symbols as glyphs. Fraction bars do not depend on font availability.
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
                caption: None,
                hwpx_raw_xml: None,
                container_box: None,
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
    // Two `over` expressions produce at least two fraction paths plus glyph pixels.
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
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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
    let warn_report = warns.finish();
    let skipped = warn_report
        .issues
        .iter()
        .filter(|issue| issue.code == hwp_render::RenderIssueCode::UnsupportedControlOmitted)
        .count();
    assert_eq!(
        skipped, 0,
        "아직 미지원으로 집계되는 도형이 있음: {warn_report:?}"
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
            rule: hwp_render::display::FillRule::NonZero,
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

/// GG-7: a synthetic display list verifies hatch rendering in all three backends.
#[test]
fn 무늬_채움_백엔드() {
    use hwp_render::display::{DisplayList, Fill, Item, PageList, PathCmd};
    let rect = || {
        vec![
            PathCmd::MoveTo(10.0, 10.0),
            PathCmd::LineTo(90.0, 10.0),
            PathCmd::LineTo(90.0, 90.0),
            PathCmd::LineTo(10.0, 90.0),
            PathCmd::Close,
        ]
    };
    let page = PageList {
        width_pt: 100.0,
        height_pt: 100.0,
        items: vec![Item::Path {
            commands: rect(),
            fill: Some(Fill::Hatch {
                fg: 0x0000_00FF, // Red hatch.
                bg: 0x00FF_FFFF, // White background in COLORREF BGR order.
                style: 1,        // Horizontal hatch.
            }),
            stroke: None,
            rule: hwp_render::display::FillRule::NonZero,
        }],
    };
    let list = DisplayList { pages: vec![page] };

    // SVG defines a pattern and references it by URL.
    let svg = hwp_render::svg::render_svg(&list).remove(0);
    assert!(svg.contains("<pattern"), "SVG 패턴 정의 없음: {svg}");
    assert!(
        svg.contains("url(#hatch0)"),
        "SVG fill url 참조 없음: {svg}"
    );

    // PNG contains both red hatch lines and a white background.
    let pngs = hwp_render::png::render_png(&list, 96.0).unwrap();
    let px = &pngs[0];
    let col = 50; // A column inside the fill region.
    let mut min_r = 255u8;
    let mut max_r = 0u8;
    for y in 15..115 {
        let p = px.pixel(col, y).unwrap();
        min_r = min_r.min(p.red());
        max_r = max_r.max(p.red());
    }
    assert!(max_r > 200, "배경(흰) 존재: max_r={max_r}");
    // Red hatch lines have low green and blue channels.
    let dark_rows = (15..115)
        .filter(|&y| {
            let p = px.pixel(col, y).unwrap();
            p.red() > 150 && p.green() < 150
        })
        .count();
    assert!(dark_rows >= 2, "가로 무늬 선 존재: {dark_rows}행");

    // PDF renders without panic and includes more than a simple rectangle fill.
    let mut issues = hwp_render::RenderIssueAccumulator::new();
    let pdf = hwp_render::pdf::render_pdf(&list, &mut issues).unwrap();
    assert!(pdf.len() > 500, "PDF 생성: {}B", pdf.len());
}

/// GG-15: verifies that flip, rotation, crop, and brightness alter rendered pixels.
#[test]
fn 이미지_변환_보정_백엔드() {
    use hwp_render::display::{DisplayList, Item, PageList};
    use std::sync::Arc;

    // A 4x2 PNG has a red left half, blue right half, and 300x150 HWPUNIT natural size.
    let png_bytes = {
        let mut img = image::RgbaImage::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                img.put_pixel(
                    x,
                    y,
                    if x < 2 {
                        image::Rgba([255, 0, 0, 255])
                    } else {
                        image::Rgba([0, 0, 255, 255])
                    },
                );
            }
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    };

    let render = |img: Item| -> tiny_skia::Pixmap {
        let list = DisplayList {
            pages: vec![PageList {
                width_pt: 120.0,
                height_pt: 70.0,
                items: vec![img],
            }],
        };
        hwp_render::png::render_png(&list, 96.0).unwrap().remove(0)
    };
    let base = |crop, flip, rotation_deg, brightness, contrast| Item::Image {
        x: 10.0,
        y: 10.0,
        w: 100.0,
        h: 50.0,
        data: Arc::new(png_bytes.clone()),
        crop,
        flip,
        rotation_deg,
        brightness,
        contrast,
    };
    // At 96 dpi, 1 pt = 4/3 px, placing the box near x=13..147 and y=13..80.
    let left_px = |px: &tiny_skia::Pixmap| px.pixel(20, 40).unwrap();
    let right_px = |px: &tiny_skia::Pixmap| px.pixel(140, 40).unwrap();

    // Baseline: red on the left and blue on the right.
    let px = render(base(None, 0, 0.0, 0, 0));
    assert!(left_px(&px).red() > 200 && right_px(&px).blue() > 200);

    // Horizontal flip: blue on the left and red on the right.
    let px = render(base(None, 1, 0.0, 0, 0));
    assert!(
        left_px(&px).blue() > 200 && right_px(&px).red() > 200,
        "가로 뒤집기: 좌={:?} 우={:?}",
        left_px(&px),
        right_px(&px)
    );

    // Clockwise 90-degree rotation moves the red left half to the top.
    let px = render(base(None, 0, 90.0, 0, 0));
    let top = px.pixel(80, 16).unwrap();
    let bottom = px.pixel(80, 76).unwrap();
    assert!(
        top.red() > 150 && bottom.blue() > 150,
        "90° 회전: 상={:?} 하={:?}",
        top,
        bottom
    );

    // Cropping to the left half fills the destination with red.
    let px = render(base(Some([0.0, 0.0, 150.0, 150.0]), 0, 0.0, 0, 0));
    assert!(
        right_px(&px).red() > 200,
        "자른 영역이 박스를 채움: {:?}",
        right_px(&px)
    );

    // Brightness +50 raises the green channel of red pixels.
    let px = render(base(None, 0, 0.0, 50, 0));
    let l = left_px(&px);
    assert!(l.red() > 200 && l.green() > 80, "밝기 보정: {l:?}");

    // SVG represents rotation with a matrix and crop with a clipPath.
    let list = DisplayList {
        pages: vec![PageList {
            width_pt: 120.0,
            height_pt: 70.0,
            items: vec![base(Some([0.0, 0.0, 150.0, 150.0]), 1, 30.0, 0, 0)],
        }],
    };
    let svg = hwp_render::svg::render_svg(&list).remove(0);
    assert!(
        svg.contains("transform=\"matrix("),
        "SVG 회전/뒤집기: {svg}"
    );
    assert!(svg.contains("<clipPath"), "SVG 자르기 클립: {svg}");
    // Zero adjustment preserves direct embedding without re-encoding.
    assert!(svg.contains("data:image/png;base64,"), "SVG 임베드: {svg}");

    // SVG brightness uses the PNG re-encoding path.
    let list = DisplayList {
        pages: vec![PageList {
            width_pt: 120.0,
            height_pt: 70.0,
            items: vec![base(None, 0, 0.0, 50, 0)],
        }],
    };
    let svg = hwp_render::svg::render_svg(&list).remove(0);
    assert!(
        svg.contains("data:image/png;base64,"),
        "SVG 재인코드: {svg}"
    );

    // PDF handles rotation, crop, and adjustment together without panic.
    let list = DisplayList {
        pages: vec![PageList {
            width_pt: 120.0,
            height_pt: 70.0,
            items: vec![base(Some([0.0, 0.0, 150.0, 150.0]), 2, 45.0, 10, 10)],
        }],
    };
    let mut issues = hwp_render::RenderIssueAccumulator::new();
    let pdf = hwp_render::pdf::render_pdf(&list, &mut issues).unwrap();
    assert!(pdf.len() > 300, "PDF 생성: {}B", pdf.len());
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
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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
    use hwp_render::display::{Item, PageList, PathCmd};

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
            .flat_map(|it| match it {
                Item::Path {
                    commands,
                    stroke: Some(stroke),
                    ..
                } => match commands.as_slice() {
                    [PathCmd::MoveTo(x1, y1), PathCmd::LineTo(x2, y2)] => {
                        vec![(*x1, *y1, *x2, *y2, stroke.color, stroke.width)]
                    }
                    [
                        PathCmd::MoveTo(x1, y1),
                        PathCmd::LineTo(x2, y2),
                        PathCmd::LineTo(x3, y3),
                        PathCmd::LineTo(x4, y4),
                        PathCmd::Close,
                    ] => vec![
                        (*x1, *y1, *x2, *y2, stroke.color, stroke.width),
                        (*x2, *y2, *x3, *y3, stroke.color, stroke.width),
                        (*x3, *y3, *x4, *y4, stroke.color, stroke.width),
                        (*x4, *y4, *x1, *y1, stroke.color, stroke.width),
                    ],
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            })
            .collect()
    }

    fn layout(doc: &Document) -> hwp_render::display::DisplayList {
        let mut store = hwp_render::FontStore::new();
        let mut warns = hwp_render::RenderIssueAccumulator::new();
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

        // The joined rectangle is prepended so it paints behind text.
        assert!(matches!(page.items.first(), Some(Item::Path { .. })));

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

/// List (bullet) paragraph: the hanging-indent space belongs to the marker and
/// lives inside the text area, so the marker sits at the paragraph's left edge
/// and the first line's text clears it. Hangul places the marker at `left`, never
/// left of the paragraph, which is what the oracle raster shows.
/// Glyph presence depends on font availability (CLAUDE.md CI rule) — skip if none.
#[test]
fn 목록_마커_내어쓰기_배치() {
    use hwp_render::display::Item;
    let doc = hwp_convert::from_markdown("- 항목입니다\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
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
    // ParaShape: margin_left=2000 (10pt), indent=-2000 (10pt hanging). Body left =
    // the from_markdown default page margin 8504HU = 85.04pt, so left = that + 10pt.
    let body_left = 85.04_f32;
    let left = body_left + 10.0;
    let hang = 10.0_f32;
    assert!(
        (marker_x - left).abs() < 0.5,
        "marker belongs at the paragraph left edge ({left}): {marker_x}"
    );
    assert!(
        text_x >= left + hang - 0.5,
        "first-line text must clear the hanging space ({}): {text_x}",
        left + hang
    );
}

#[test]
fn 개요_번호_마커_렌더() {
    use hwp_model::{ParaShape, ParaShapeId, Paragraph};
    let mut doc = hwp_convert::from_markdown("첫째\n\n둘째\n\n셋째\n");
    // Add outline paragraph shapes for levels 1 and 2 and attach them to paragraphs.
    let base = doc.header.para_shapes.len() as u16;
    doc.header.para_shapes.push(ParaShape {
        attr1: (1 << 23) | (1 << 25),
        ..ParaShape::default()
    });
    doc.header.para_shapes.push(ParaShape {
        attr1: (1 << 23) | (2 << 25),
        ..ParaShape::default()
    });
    doc.sections[0].paragraphs[0].para_shape = ParaShapeId(base);
    doc.sections[0].paragraphs[1].para_shape = ParaShapeId(base);
    doc.sections[0].paragraphs[2].para_shape = ParaShapeId(base + 1);
    doc.sections[0].paragraphs.insert(
        0,
        Paragraph {
            para_shape: ParaShapeId(base),
            ..Paragraph::default()
        },
    );

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    let texts = page_texts(&list.pages[0]);
    if texts.is_empty() {
        eprintln!("스킵: 사용 가능한 폰트 없음 — 글리프 미생성");
        return;
    }
    assert!(texts.contains(&"1."), "개요 수준1 마커: {texts:?}");
    assert!(texts.contains(&"2."), "개요 수준1 두 번째 마커: {texts:?}");
    assert!(texts.contains(&"가."), "개요 수준2 마커: {texts:?}");
}

#[test]
fn 글상자_내부_개요_번호_마커_렌더() {
    use hwp_model::{
        Control, GenericControl, ParaShape, ParaShapeId, ParagraphList, ShapeGeom, ShapeKind,
    };

    let mut doc = hwp_convert::from_markdown("anchor\n");
    let outline_shape = doc.header.para_shapes.len() as u16;
    doc.header.para_shapes.push(ParaShape {
        attr1: (1 << 23) | (1 << 25),
        ..ParaShape::default()
    });
    let mut inner = hwp_convert::from_markdown("inside\n")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    inner.para_shape = ParaShapeId(outline_shape);
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(GenericControl {
            ctrl_id: *b"rect",
            data: Vec::new(),
            paragraph_lists: vec![ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![inner],
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: vec![ShapeGeom {
                kind: ShapeKind::Rect,
                x: 0,
                y: 0,
                w: 20_000,
                h: 5_000,
                points: Vec::new(),
                fill: 0xFFFF_FFFF,
                fill_gradient: None,
                border_color: 0xFFFF_FFFF,
                border_width: 0,
                round_ratio: 0,
                border_style: 0,
                arrow_start: 0,
                arrow_end: 0,
                anchored: true,
                description: None,
            }],
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    let texts = page_texts(&list.pages[0]);
    if texts.is_empty() {
        eprintln!("스킵: 사용 가능한 폰트 없음 - 글리프 미생성");
        return;
    }
    assert!(texts.contains(&"1."), "글상자 내부 개요 마커: {texts:?}");
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
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
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
            ..Default::default()
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
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    let warnings = warnings.finish();
    if warnings
        .issues
        .iter()
        .any(|issue| issue.code == hwp_render::RenderIssueCode::PageNumberShapingOmitted)
    {
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
        warnings
            .issues
            .iter()
            .all(|issue| { issue.code != hwp_render::RenderIssueCode::UnsupportedControlOmitted }),
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
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    let warnings = warnings.finish();
    if warnings.issues.iter().any(|issue| {
        matches!(
            issue.code,
            hwp_render::RenderIssueCode::ShapingFailed
                | hwp_render::RenderIssueCode::PageNumberShapingOmitted
        )
    }) {
        eprintln!("스킵: 사용 가능한 폰트 없음 - atno 글리프 미생성");
        return;
    }
    assert_eq!(list.pages.len(), 3);
    assert!(page_texts(&list.pages[0]).contains(&"5"));
    assert!(!page_texts(&list.pages[1]).contains(&"6"));
    assert!(page_texts(&list.pages[2]).contains(&"7"));
    assert!(
        warnings
            .issues
            .iter()
            .all(|issue| { issue.code != hwp_render::RenderIssueCode::UnsupportedControlOmitted }),
        "atno/head가 미지원으로 집계됨: {warnings:?}"
    );
}

// ---------- Table pagination (PDF parity PR 2) ----------

/// Builds a one-column table after `filler` empty paragraphs. Data and header
/// cells use distinct fills so the tests can identify their rectangles.
fn 표_분할_문서(
    attr: u32,
    rows: u16,
    row_h_pt: i32,
    header_rows: u16,
    treat_as_char: bool,
    filler: usize,
) -> hwp_model::Document {
    let mut doc = hwp_convert::from_markdown("앵커");
    // Account for converter-provided fills; BorderFillId is one-based.
    let fill_base = doc.header.border_fills.len() as u16;
    doc.header.border_fills.push(hwp_model::BorderFill {
        bg_color: Some(0x00C8_C8C8),
        ..Default::default()
    }); // fill_base + 1 — 데이터 셀
    doc.header.border_fills.push(hwp_model::BorderFill {
        bg_color: Some(0x0055_5555),
        ..Default::default()
    }); // fill_base + 2 — 제목 셀
    let cell = |row: u16, header: bool| hwp_model::Cell {
        list_attr: if header { 1 << 18 } else { 0 },
        col: 0,
        row,
        col_span: 1,
        row_span: 1,
        width: hwp_model::HwpUnit(5000),
        height: hwp_model::HwpUnit(row_h_pt * 100),
        margins: [0; 4],
        border_fill: hwp_model::BorderFillId(if header { fill_base + 2 } else { fill_base + 1 }),
        header_tail: Vec::new(),
        paragraphs: Vec::new(),
    };
    let table = hwp_model::Table {
        common_data: Vec::new(),
        placement: treat_as_char.then_some(hwp_model::GsoPlacement {
            treat_as_char: true,
            ..Default::default()
        }),
        attr,
        rows,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1; rows as usize],
        border_fill: hwp_model::BorderFillId(fill_base + 1),
        table_tail: Vec::new(),
        cells: (0..rows).map(|r| cell(r, r < header_rows)).collect(),
        caption: None,
        extras: Vec::new(),
    };
    let anchor = &mut doc.sections[0].paragraphs[0];
    anchor.chars.clear(); // 빈 문단 — 표는 본문 상단 + 16pt에 앵커된다
    anchor.controls.push(hwp_model::Control::Table(table));
    for _ in 0..filler {
        let mut p = doc.sections[0].paragraphs[0].clone();
        p.controls.clear();
        doc.sections[0].paragraphs.insert(0, p);
    }
    doc
}

/// Returns the body top and bottom in points, matching `layout.rs`.
fn 본문_기하(doc: &hwp_model::Document) -> (f32, f32) {
    let p = doc.sections[0].section_def().unwrap().page.unwrap();
    let h = p.height.0 as f32 / 100.0;
    let top = (p.margin_top.0 + p.margin_header.0) as f32 / 100.0;
    let bottom = h - (p.margin_bottom.0 + p.margin_footer.0) as f32 / 100.0;
    (top, bottom)
}

/// Returns page fill rectangles as `(y, h, fill)` tuples.
fn 채움_사각형(list: &hwp_render::display::DisplayList, page: usize) -> Vec<(f32, f32, u32)> {
    list.pages[page]
        .items
        .iter()
        .filter_map(|it| match it {
            hwp_render::display::Item::Rect { y, h, fill, .. } => Some((*y, *h, *fill)),
            _ => None,
        })
        .collect()
}

fn 표_레이아웃(
    doc: &hwp_model::Document,
) -> (
    hwp_render::display::DisplayList,
    hwp_render::RenderIssueReport,
) {
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(doc, &mut store, &mut warns);
    (list, warns.finish())
}

/// Regime-A CELL tables use the cached line layout as the only trustworthy
/// internal page boundary.  This fixture keeps all assertions structural so
/// a font substitution cannot change the expected geometry.
#[test]
fn cached_cell_fragments_preserve_text_nested_object_and_continuation_borders() {
    // 650 pt fits on a fresh page, but the preceding paragraphs leave less
    // than the 600 pt cached text run on page 1. There is deliberately no
    // cached page flag/reset: the last fitting cached line boundary must be
    // selected from the actual remaining capacity.
    let mut doc = 표_분할_문서(2, 1, 650, 0, false, 8);
    let mut source = hwp_convert::from_markdown("x")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    let chars: Vec<_> = (0..60)
        .map(|index| hwp_model::HwpChar::Text(char::from(b'a' + (index % 26) as u8)))
        .collect();
    let line_segs: Vec<_> = (0..60)
        .map(|index| hwp_model::LineSeg {
            text_start: index,
            v_pos: index as i32 * 1000,
            line_height: 1000,
            text_height: 900,
            baseline_gap: 800,
            line_spacing: 0,
            col_start: 0,
            seg_width: 50000,
            flags: 0x0006_0000,
        })
        .collect();
    source.chars = chars.clone();
    source.line_segs = line_segs;
    source.controls.clear();

    let nested_fill = doc.header.border_fills.len() as u16 + 1;
    doc.header.border_fills.push(hwp_model::BorderFill {
        bg_color: Some(0x0011_2233),
        ..Default::default()
    });
    let nested = hwp_model::Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: 1,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1],
        border_fill: hwp_model::BorderFillId(nested_fill),
        table_tail: Vec::new(),
        cells: vec![hwp_model::Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: hwp_model::HwpUnit(1800),
            height: hwp_model::HwpUnit(1000),
            margins: [0; 4],
            border_fill: hwp_model::BorderFillId(nested_fill),
            header_tail: Vec::new(),
            paragraphs: Vec::new(),
        }],
        caption: None,
        extras: Vec::new(),
    };
    source.controls.push(hwp_model::Control::Table(nested));

    let outer = doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("outer table");
    outer.cells[0].height = hwp_model::HwpUnit(65000);
    outer.cells[0].paragraphs = vec![source];

    let (list, report) = 표_레이아웃(&doc);
    assert_eq!(
        list.pages.len(),
        2,
        "remaining-space line boundary creates one continuation page"
    );
    for page in &list.pages {
        let rects = page
            .items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            rects
                .iter()
                .all(|(y, h)| *y >= 0.0 && *y + *h <= page.height_pt + 0.6),
            "fragment bounds exceed page: {rects:?}"
        );
    }
    let all_text =
        list.pages
            .iter()
            .flat_map(page_texts)
            .fold(String::new(), |mut text, fragment| {
                text.push_str(fragment);
                text
            });
    if all_text.is_empty() {
        eprintln!("skip text-order assertion: no usable font");
    } else {
        assert_eq!(all_text.matches("abcdefghijklmnopqrstuvwxyz").count(), 2);
        assert!(all_text.starts_with("abcdefghijklmnopqrstuvwxyz"));
    }
    let nested_rects = list
        .pages
        .iter()
        .flat_map(|page| page.items.iter())
        .filter(|item| {
            matches!(
                item,
                hwp_render::display::Item::Rect {
                    fill: 0x0011_2233,
                    ..
                }
            )
        })
        .count();
    assert_eq!(nested_rects, 1, "nested object must be emitted once");
    assert!(
        report
            .info
            .iter()
            .any(|issue| { issue.code == hwp_render::RenderIssueCode::TableSplitAcrossPages })
    );
    assert!(report.issues.iter().all(|issue| {
        issue.code != hwp_render::RenderIssueCode::TableCellFragmentationIncomplete
    }));
}

#[test]
fn cell_split_without_cached_boundary_is_incomplete() {
    let mut doc = 표_분할_문서(2, 1, 900, 0, false, 0);
    let table = doc.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("outer table");
    table.cells[0].height = hwp_model::HwpUnit(90000);
    table.cells[0].paragraphs = vec![
        hwp_convert::from_markdown("uncached")
            .sections
            .remove(0)
            .paragraphs
            .remove(0),
    ];

    let (_list, report) = 표_레이아웃(&doc);
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == hwp_render::RenderIssueCode::TableCellFragmentationIncomplete)
        .expect("required CELL split must report missing cache");
    assert_eq!(issue.severity, hwp_render::RenderIssueSeverity::Incomplete);
    assert_eq!(issue.stage, hwp_render::RenderIssueStage::Layout);
}

/// A cell paragraph whose cached line layout places one character per 10 pt
/// line.  All assertions stay structural so a font substitution cannot
/// change the expected geometry.
fn cached_cell_paragraph(lines: usize) -> hwp_model::Paragraph {
    let mut para = hwp_convert::from_markdown("x")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    para.chars = (0..lines)
        .map(|index| hwp_model::HwpChar::Text(char::from(b'a' + (index % 26) as u8)))
        .collect();
    para.line_segs = (0..lines as u32)
        .map(|index| hwp_model::LineSeg {
            text_start: index,
            v_pos: index as i32 * 1000,
            line_height: 1000,
            text_height: 900,
            baseline_gap: 800,
            line_spacing: 0,
            col_start: 0,
            seg_width: 50000,
            flags: 0x0006_0000,
        })
        .collect();
    para.controls.clear();
    para
}

fn outer_table(doc: &mut hwp_model::Document) -> &mut hwp_model::Table {
    doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("outer table")
}

fn assert_no_fragment_issues(report: &hwp_render::RenderIssueReport) {
    assert!(
        report.issues.iter().all(|issue| {
            !matches!(
                issue.code,
                hwp_render::RenderIssueCode::TableCellFragmentationIncomplete
                    | hwp_render::RenderIssueCode::TableRowTooTallClipped
            )
        }),
        "over-height CELL row must split cleanly: {:?}",
        report.issues
    );
}

/// An over-height CELL row with cached line boundaries must paginate: the
/// text run splits at a cached boundary and the declared row-height remainder
/// becomes page-capacity continuation fragments.
#[test]
fn cached_cell_row_taller_than_body_splits_cleanly() {
    let mut doc = 표_분할_문서(2, 1, 900, 0, false, 0);
    let table = outer_table(&mut doc);
    table.cells[0].height = hwp_model::HwpUnit(90000);
    table.cells[0].paragraphs = vec![cached_cell_paragraph(80)];

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(
        list.pages.len(),
        2,
        "900 pt row over a smaller body paginates"
    );
    for page in &list.pages {
        let rects = page
            .items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            rects
                .iter()
                .all(|(y, h)| *y >= 0.0 && *y + *h <= page.height_pt + 0.6),
            "fragment bounds exceed page: {rects:?}"
        );
    }
    let all_text =
        list.pages
            .iter()
            .flat_map(page_texts)
            .fold(String::new(), |mut text, fragment| {
                text.push_str(fragment);
                text
            });
    if all_text.is_empty() {
        eprintln!("skip text assertion: no usable font");
    } else {
        assert_eq!(
            all_text.chars().count(),
            80,
            "each cached line is emitted exactly once: {all_text:?}"
        );
    }
    assert!(
        report
            .info
            .iter()
            .any(|issue| { issue.code == hwp_render::RenderIssueCode::TableSplitAcrossPages })
    );
}

/// Declared row height beyond the cached content height is preserved as
/// page-capacity blank continuation fragments instead of inflating the last
/// text fragment past the page.
#[test]
fn declared_row_height_remainder_becomes_blank_continuation_fragments() {
    let mut doc = 표_분할_문서(2, 1, 900, 0, false, 0);
    let table = outer_table(&mut doc);
    table.cells[0].height = hwp_model::HwpUnit(90000);
    table.cells[0].paragraphs = vec![cached_cell_paragraph(5)];
    let (_, body_bottom) = 본문_기하(&doc);

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(
        list.pages.len(),
        2,
        "declared remainder continues on page 2"
    );
    let data_rects = |page: usize| -> Vec<(f32, f32)> {
        채움_사각형(&list, page)
            .into_iter()
            .filter(|&(_, _, fill)| fill == 0x00C8_C8C8)
            .map(|(y, h, _)| (y, h))
            .collect()
    };
    let first = data_rects(0);
    let second = data_rects(1);
    assert_eq!(first.len(), 1, "one text fragment on page 1: {first:?}");
    assert_eq!(second.len(), 1, "one blank fragment on page 2: {second:?}");
    assert!(
        (first[0].0 + first[0].1 - body_bottom).abs() <= 0.6,
        "text fragment fills the remaining page capacity: {first:?} bottom={body_bottom}"
    );
    let declared: f32 = first[0].1 + second[0].1;
    assert!(
        (declared - 900.0).abs() <= 0.6,
        "fragments preserve the declared row height: {declared}"
    );
    let second_page_text = page_texts(&list.pages[1]);
    if second_page_text.is_empty() && page_texts(&list.pages[0]).is_empty() {
        eprintln!("skip text assertion: no usable font");
    } else {
        assert!(
            second_page_text.is_empty(),
            "blank continuation replays no text: {second_page_text:?}"
        );
    }
}

/// A v_pos reset without an explicit page flag is a soft boundary: the run
/// after the reset is packed into the remaining page capacity at complete
/// cached line boundaries instead of forcing a fresh page.
#[test]
fn cached_cell_soft_v_pos_reset_packs_into_remaining_capacity() {
    // Three cached 10 pt line runs ('a' x50, 'b' x50, 'c' x20) with a v_pos
    // reset — and no page flag — before the 'b' and 'c' runs.  Declared row
    // height equals the cached content height, so no blank remainder applies.
    let mut doc = 표_분할_문서(2, 1, 1200, 0, false, 0);
    let mut para = hwp_convert::from_markdown("x")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    para.chars = Vec::new();
    para.line_segs = Vec::new();
    for (ch, lines) in [('a', 50usize), ('b', 50), ('c', 20)] {
        for index in 0..lines {
            para.chars.push(hwp_model::HwpChar::Text(ch));
            para.line_segs.push(hwp_model::LineSeg {
                text_start: (para.chars.len() - 1) as u32,
                v_pos: index as i32 * 1000,
                line_height: 1000,
                text_height: 900,
                baseline_gap: 800,
                line_spacing: 0,
                col_start: 0,
                seg_width: 50000,
                flags: 0x0006_0000,
            });
        }
    }
    para.controls.clear();
    let table = outer_table(&mut doc);
    table.cells[0].height = hwp_model::HwpUnit(120000);
    table.cells[0].paragraphs = vec![para];
    let (_, body_bottom) = 본문_기하(&doc);

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(
        list.pages.len(),
        2,
        "soft resets pack 1200 pt of cached runs into two pages"
    );
    for page in &list.pages {
        let rects = page
            .items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            rects
                .iter()
                .all(|(y, h)| *y >= 0.0 && *y + *h <= page.height_pt + 0.6),
            "fragment bounds exceed page: {rects:?}"
        );
    }
    // Page 1 is filled to its capacity: complete lines from the run after the
    // reset moved up instead of the run forcing a new page.  A hard reset
    // would leave page 1 ending after the 500 pt 'a' run.
    let page1_bottom = 채움_사각형(&list, 0)
        .into_iter()
        .filter(|&(_, _, fill)| fill == 0x00C8_C8C8)
        .map(|(y, h, _)| y + h)
        .fold(0.0f32, f32::max);
    assert!(
        page1_bottom > body_bottom - 10.6 && page1_bottom <= body_bottom + 0.6,
        "page 1 filled to capacity by soft-reset packing: bottom={page1_bottom} body={body_bottom}"
    );
    let page_text = |page: usize| -> String {
        page_texts(&list.pages[page])
            .into_iter()
            .fold(String::new(), |mut text, fragment| {
                text.push_str(fragment);
                text
            })
    };
    let all_text: String = (0..list.pages.len()).map(page_text).collect();
    if all_text.is_empty() {
        eprintln!("skip text assertion: no usable font");
    } else {
        let page1 = page_text(0);
        assert!(
            page1.contains('b') && !page1.contains('c'),
            "page 1 packs complete 'b' lines after the reset: {page1:?}"
        );
        assert_eq!(all_text.matches('a').count(), 50);
        assert_eq!(all_text.matches('b').count(), 50);
        assert_eq!(all_text.matches('c').count(), 20);
    }
}

/// Builds a two-column CELL table document so row-span interaction with cell
/// fragmenting can be exercised.  Returns the doc and the data/span fill ids.
fn 이열_셀_표_문서() -> (hwp_model::Document, u16, u16) {
    let mut doc = 표_분할_문서(2, 1, 900, 0, false, 0);
    doc.header.border_fills.push(hwp_model::BorderFill {
        bg_color: Some(0x00AA_AAAA),
        ..Default::default()
    });
    let data_fill = doc.header.border_fills.len() as u16;
    doc.header.border_fills.push(hwp_model::BorderFill {
        bg_color: Some(0x0033_7777),
        ..Default::default()
    });
    let span_fill = doc.header.border_fills.len() as u16;
    (doc, data_fill, span_fill)
}

fn 이열_셀(row: u16, col: u16, row_span: u16, height_pt: i32, fill: u16) -> hwp_model::Cell {
    hwp_model::Cell {
        list_attr: 0,
        col,
        row,
        col_span: 1,
        row_span,
        width: hwp_model::HwpUnit(2500),
        height: hwp_model::HwpUnit(height_pt * 100),
        margins: [0; 4],
        border_fill: hwp_model::BorderFillId(fill),
        header_tail: Vec::new(),
        paragraphs: Vec::new(),
    }
}

/// A row-spanned cell that lives entirely after the fragmented row (and on a
/// single page) must not disable CELL fragmenting.
#[test]
fn row_span_outside_fragmented_row_keeps_cell_fragmenting() {
    let (mut doc, data_fill, span_fill) = 이열_셀_표_문서();
    let mut tall = 이열_셀(1, 0, 1, 900, data_fill);
    tall.paragraphs = vec![cached_cell_paragraph(5)];
    let table = outer_table(&mut doc);
    table.cols = 2;
    table.rows = 4;
    table.row_cell_counts = vec![2, 2, 2, 1];
    table.cells = vec![
        이열_셀(0, 0, 1, 20, data_fill),
        이열_셀(0, 1, 1, 20, data_fill),
        tall,
        이열_셀(1, 1, 1, 900, data_fill),
        이열_셀(2, 0, 2, 20, span_fill),
        이열_셀(2, 1, 1, 20, data_fill),
        이열_셀(3, 1, 1, 20, data_fill),
    ];

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(list.pages.len(), 2, "only the over-height row paginates");
    let span_rects: Vec<(f32, f32)> = 채움_사각형(&list, 1)
        .into_iter()
        .filter(|&(_, _, fill)| fill == 0x0033_7777)
        .map(|(y, h, _)| (y, h))
        .collect();
    assert_eq!(
        span_rects.len(),
        1,
        "span cell emitted once: {span_rects:?}"
    );
    assert!(
        (span_rects[0].1 - 40.0).abs() <= 0.6,
        "span cell covers both spanned rows: {span_rects:?}"
    );
}

/// A row-spanned cell intersecting the internally fragmented row stays
/// fail-closed: the CELL fragment path reports incomplete and yields to the
/// row-boundary planner.
#[test]
fn row_span_intersecting_fragmented_row_remains_fail_closed() {
    let (mut doc, data_fill, span_fill) = 이열_셀_표_문서();
    let mut tall = 이열_셀(1, 1, 1, 900, data_fill);
    tall.paragraphs = vec![cached_cell_paragraph(5)];
    let table = outer_table(&mut doc);
    table.cols = 2;
    table.rows = 3;
    table.row_cell_counts = vec![2, 2, 1];
    table.cells = vec![
        이열_셀(0, 0, 1, 20, data_fill),
        이열_셀(0, 1, 1, 20, data_fill),
        이열_셀(1, 0, 2, 20, span_fill),
        tall,
        이열_셀(2, 1, 1, 20, data_fill),
    ];

    let (_list, report) = 표_레이아웃(&doc);
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == hwp_render::RenderIssueCode::TableCellFragmentationIncomplete)
        .expect("row span crossing a cell fragment must stay fail-closed");
    assert_eq!(issue.severity, hwp_render::RenderIssueSeverity::Incomplete);
    assert_eq!(issue.stage, hwp_render::RenderIssueStage::Layout);
}

/// The declared row-height remainder is a row-level contract: only the plan
/// that owns the row height (largest cached fragment sum) is padded.  A short
/// cell in the same fragmented row must not manufacture its own blank
/// continuation fragments — that would misalign the per-fragment part heights
/// and create an extra nearly blank page.
#[test]
fn declared_remainder_pads_only_the_row_height_owner() {
    let (mut doc, data_fill, owner_fill) = 이열_셀_표_문서();
    // Short left cell: 3 cached lines (30 pt).  Tall right cell: two cached
    // 10 pt runs ('a' x50, soft v_pos reset, 'b' x60) = 1100 pt.  Both row-1
    // cells declare 1200 pt.
    let mut short_para = hwp_convert::from_markdown("x")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    short_para.chars = (0..3).map(|_| hwp_model::HwpChar::Text('L')).collect();
    short_para.line_segs = (0..3u32)
        .map(|index| hwp_model::LineSeg {
            text_start: index,
            v_pos: index as i32 * 1000,
            line_height: 1000,
            text_height: 900,
            baseline_gap: 800,
            line_spacing: 0,
            col_start: 0,
            seg_width: 50000,
            flags: 0x0006_0000,
        })
        .collect();
    short_para.controls.clear();
    let mut tall_para = hwp_convert::from_markdown("x")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    tall_para.chars = Vec::new();
    tall_para.line_segs = Vec::new();
    for (ch, lines) in [('a', 50usize), ('b', 60)] {
        for index in 0..lines {
            tall_para.chars.push(hwp_model::HwpChar::Text(ch));
            tall_para.line_segs.push(hwp_model::LineSeg {
                text_start: (tall_para.chars.len() - 1) as u32,
                v_pos: index as i32 * 1000,
                line_height: 1000,
                text_height: 900,
                baseline_gap: 800,
                line_spacing: 0,
                col_start: 0,
                seg_width: 50000,
                flags: 0x0006_0000,
            });
        }
    }
    tall_para.controls.clear();
    let mut short = 이열_셀(1, 0, 1, 1200, data_fill);
    short.paragraphs = vec![short_para];
    let mut tall = 이열_셀(1, 1, 1, 1200, owner_fill);
    tall.paragraphs = vec![tall_para];
    let table = outer_table(&mut doc);
    table.cols = 2;
    table.rows = 2;
    table.row_cell_counts = vec![2, 2];
    table.cells = vec![
        이열_셀(0, 0, 1, 20, data_fill),
        이열_셀(0, 1, 1, 20, data_fill),
        short,
        tall,
    ];

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(
        list.pages.len(),
        2,
        "short cell must not add an independent blank page"
    );
    // The row-level declared contract still holds: the owner cell's fragment
    // heights sum to the declared 1200 pt.
    let owner_h: f32 = (0..list.pages.len())
        .flat_map(|page| 채움_사각형(&list, page))
        .filter(|&(_, _, fill)| fill == 0x0033_7777)
        .map(|(_, h, _)| h)
        .sum();
    assert!(
        (owner_h - 1200.0).abs() <= 0.6,
        "owner fragments preserve the declared row height: {owner_h}"
    );
    for page in &list.pages {
        let rects = page
            .items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            rects
                .iter()
                .all(|(y, h)| *y >= 0.0 && *y + *h <= page.height_pt + 0.6),
            "fragment bounds exceed page: {rects:?}"
        );
    }
    let page_text = |page: usize| -> String {
        page_texts(&list.pages[page])
            .into_iter()
            .fold(String::new(), |mut text, fragment| {
                text.push_str(fragment);
                text
            })
    };
    let all_text: String = (0..list.pages.len()).map(page_text).collect();
    if all_text.is_empty() {
        eprintln!("skip text assertion: no usable font");
    } else {
        assert!(
            page_text(0).contains('L'),
            "short cell text is drawn in the first fragment"
        );
        assert_eq!(all_text.matches('a').count(), 50);
        assert_eq!(all_text.matches('b').count(), 60);
        assert_eq!(all_text.matches('L').count(), 3);
    }
}

/// A row-spanned block following an over-height fragmented row: the span
/// fits a fresh page but not the current-page remainder.  The row must move
/// to a fresh page instead of tripping the row-span safety bail.
#[test]
fn row_span_after_fragmented_row_moves_to_fresh_page() {
    let (mut doc, data_fill, span_fill) = 이열_셀_표_문서();
    // Row 0 fragments across pages 1-2 (900 pt declared/cached).  Rows 1-2
    // form a 400 pt span block: it fits a fresh page but not the ~385 pt
    // remainder below the second fragment.
    let mut short = 이열_셀(0, 0, 1, 900, data_fill);
    short.paragraphs = vec![cached_cell_paragraph(3)];
    let mut tall = 이열_셀(0, 1, 1, 900, data_fill);
    tall.paragraphs = vec![cached_cell_paragraph(90)];
    let table = outer_table(&mut doc);
    table.cols = 2;
    table.rows = 3;
    table.row_cell_counts = vec![2, 2, 1];
    table.cells = vec![
        short,
        tall,
        이열_셀(1, 0, 2, 200, span_fill),
        이열_셀(1, 1, 1, 200, data_fill),
        이열_셀(2, 1, 1, 200, data_fill),
    ];

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(
        list.pages.len(),
        3,
        "fragmented row uses pages 1-2, span block moves to page 3"
    );
    let span_rects: Vec<(f32, f32)> = 채움_사각형(&list, 2)
        .into_iter()
        .filter(|&(_, _, fill)| fill == 0x0033_7777)
        .map(|(y, h, _)| (y, h))
        .collect();
    assert_eq!(
        span_rects.len(),
        1,
        "span cell emitted once on the fresh page: {span_rects:?}"
    );
    assert!(
        (span_rects[0].1 - 400.0).abs() <= 0.6,
        "span cell covers both spanned rows: {span_rects:?}"
    );
    for page in &list.pages {
        let rects = page
            .items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            rects
                .iter()
                .all(|(y, h)| *y >= 0.0 && *y + *h <= page.height_pt + 0.6),
            "fragment bounds exceed page: {rects:?}"
        );
    }
    let all_text: String = (0..list.pages.len())
        .flat_map(|page| page_texts(&list.pages[page]))
        .fold(String::new(), |mut text, fragment| {
            text.push_str(fragment);
            text
        });
    if all_text.is_empty() {
        eprintln!("skip text assertion: no usable font");
    } else {
        assert_eq!(
            all_text.chars().count(),
            93,
            "each cached line is emitted exactly once"
        );
    }
}

/// A cell whose content fits one page must not force an incomplete render
/// merely because the fragmented row starts on a page-bottom sliver: with no
/// complete cached line fitting the sliver, the first fragment moves to a
/// fresh page where the whole-cell draw is contained.
#[test]
fn cell_needing_boundary_only_for_page_sliver_is_contained_in_first_fragment() {
    let (mut doc, data_fill, _span_fill) = 이열_셀_표_문서();
    // Row 0 (640 pt) leaves a ~1.6 pt sliver on page 1.  Row 1: a short
    // cached cell (3 lines) and a tall cached cell (90 lines = 900 pt).
    let mut short = 이열_셀(1, 0, 1, 900, data_fill);
    short.paragraphs = vec![cached_cell_paragraph(3)];
    let mut tall = 이열_셀(1, 1, 1, 900, data_fill);
    tall.paragraphs = vec![cached_cell_paragraph(90)];
    let table = outer_table(&mut doc);
    table.cols = 2;
    table.rows = 2;
    table.row_cell_counts = vec![2, 2];
    table.cells = vec![
        이열_셀(0, 0, 1, 640, data_fill),
        이열_셀(0, 1, 1, 640, data_fill),
        short,
        tall,
    ];

    let (list, report) = 표_레이아웃(&doc);
    assert_no_fragment_issues(&report);
    assert_eq!(
        list.pages.len(),
        3,
        "row 0 fills page 1; both fragments land on fresh pages"
    );
    for page in &list.pages {
        let rects = page
            .items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Rect { y, h, .. } => Some((*y, *h)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            rects
                .iter()
                .all(|(y, h)| *y >= 0.0 && *y + *h <= page.height_pt + 0.6),
            "fragment bounds exceed page: {rects:?}"
        );
    }
    let page_text = |page: usize| -> String {
        page_texts(&list.pages[page])
            .into_iter()
            .fold(String::new(), |mut text, fragment| {
                text.push_str(fragment);
                text
            })
    };
    let all_text: String = (0..list.pages.len()).map(page_text).collect();
    if all_text.is_empty() {
        eprintln!("skip text assertion: no usable font");
    } else {
        assert_eq!(all_text.chars().count(), 93);
        assert!(
            page_text(1).chars().count() > 63,
            "short cell content travels with the moved first fragment"
        );
    }
}

/// Two independently fragmented rows: the containment height for a
/// boundary-needing cell without a split plan must come only from its own
/// row's plans.  An earlier row's taller first fragment must not vouch for a
/// later row's unsplittable cell.
#[test]
fn uncached_cell_in_second_fragmented_row_stays_fail_closed() {
    let (mut doc, data_fill, _span_fill) = 이열_셀_표_문서();
    // Uncached payload with a font-independent height: a nested table with a
    // 500 pt cell measures ~512 pt regardless of shaping.
    let nested_fill = doc.header.border_fills.len() as u16 + 1;
    doc.header.border_fills.push(hwp_model::BorderFill {
        bg_color: Some(0x0011_2233),
        ..Default::default()
    });
    let nested = hwp_model::Table {
        common_data: Vec::new(),
        placement: None,
        attr: 0,
        rows: 1,
        cols: 1,
        cell_spacing: 0,
        inner_margins: [0; 4],
        row_cell_counts: vec![1],
        border_fill: hwp_model::BorderFillId(nested_fill),
        table_tail: Vec::new(),
        cells: vec![hwp_model::Cell {
            list_attr: 0,
            col: 0,
            row: 0,
            col_span: 1,
            row_span: 1,
            width: hwp_model::HwpUnit(1800),
            height: hwp_model::HwpUnit(50000),
            margins: [0; 4],
            border_fill: hwp_model::BorderFillId(nested_fill),
            header_tail: Vec::new(),
            paragraphs: Vec::new(),
        }],
        caption: None,
        extras: Vec::new(),
    };
    // Row 0 fragments with a ~633 pt first fragment.  Row 1 starts on page 2
    // with ~385 pt remaining; its right cell splits into ~383/+423 pt
    // fragments, while its left cell carries the uncached ~512 pt payload.
    // That payload is contained by neither row 1 fragment (~383 pt) — only
    // row 0's taller first fragment would wrongly vouch for it.
    let mut tall_row0 = 이열_셀(0, 1, 1, 900, data_fill);
    tall_row0.paragraphs = vec![cached_cell_paragraph(90)];
    let mut uncached = 이열_셀(1, 0, 1, 800, data_fill);
    uncached.paragraphs = vec![hwp_model::Paragraph {
        controls: vec![hwp_model::Control::Table(nested)],
        ..hwp_model::Paragraph::default()
    }];
    let mut tall_row1 = 이열_셀(1, 1, 1, 800, data_fill);
    tall_row1.paragraphs = vec![cached_cell_paragraph(80)];
    let table = outer_table(&mut doc);
    table.cols = 2;
    table.rows = 2;
    table.row_cell_counts = vec![2, 2];
    table.cells = vec![
        이열_셀(0, 0, 1, 900, data_fill),
        tall_row0,
        uncached,
        tall_row1,
    ];

    let (_list, report) = 표_레이아웃(&doc);
    let issue = report
        .issues
        .iter()
        .find(|issue| issue.code == hwp_render::RenderIssueCode::TableCellFragmentationIncomplete)
        .expect("unsplittable cell must not borrow an earlier row's fragment height");
    assert_eq!(issue.severity, hwp_render::RenderIssueSeverity::Incomplete);
    assert_eq!(issue.stage, hwp_render::RenderIssueStage::Layout);
}

/// GB-13: a bottom table caption is placed below cell text with its gap.
#[test]
fn 캡션_표_아래_배치() {
    let mut doc = hwp_convert::from_markdown("| 가 |\n| --- |\n| 1 |\n");
    let table = doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("generated table");
    table.caption = Some(hwp_model::Caption {
        side: hwp_model::CaptionSide::Bottom,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 850, // 8.5pt
        width: None,
        last_width: 0,
        paragraphs: vec![hwp_model::Paragraph {
            chars: "캡션텍스트".chars().map(hwp_model::HwpChar::Text).collect(),
            ..hwp_model::Paragraph::default()
        }],
    });

    let (list, _) = 표_레이아웃(&doc);
    let page = &list.pages[0];
    let glyph_y = |needle: &str| {
        page.items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Glyphs { y, run, .. } => {
                    run.text.contains(needle).then_some(*y)
                }
                _ => None,
            })
            .next()
    };
    let cell_y = glyph_y("1").expect("셀 텍스트");
    let caption_y = glyph_y("캡션텍스트").expect("캡션 텍스트 배치");
    assert!(
        caption_y > cell_y + 8.5,
        "캡션은 표 아래 gap(8.5pt) 이상 아래: 셀 {cell_y} vs 캡션 {caption_y}"
    );
}

/// GB-13: a top caption precedes the table and shifts it downward.
#[test]
fn 캡션_표_위_배치() {
    let mut doc = hwp_convert::from_markdown("| 가 |\n| --- |\n| 1 |\n");
    let table = doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("generated table");
    table.caption = Some(hwp_model::Caption {
        side: hwp_model::CaptionSide::Top,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 850,
        width: None,
        last_width: 0,
        paragraphs: vec![hwp_model::Paragraph {
            chars: "캡션텍스트".chars().map(hwp_model::HwpChar::Text).collect(),
            ..hwp_model::Paragraph::default()
        }],
    });

    let (list, _) = 표_레이아웃(&doc);
    let page = &list.pages[0];
    let glyph_y = |needle: &str| {
        page.items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Glyphs { y, run, .. } => {
                    run.text.contains(needle).then_some(*y)
                }
                _ => None,
            })
            .next()
    };
    let cell_y = glyph_y("1").expect("셀 텍스트");
    let caption_y = glyph_y("캡션텍스트").expect("캡션 텍스트 배치");
    assert!(
        caption_y < cell_y,
        "위쪽 캡션은 셀 텍스트보다 위: 캡션 {caption_y} vs 셀 {cell_y}"
    );
}

#[test]
fn generic_shape_caption_is_rendered() {
    let mut doc = hwp_convert::from_markdown("Body\n");
    let para = &mut doc.sections[0].paragraphs[0];
    let control_index = para.controls.len() as u32;
    para.chars.push(hwp_model::HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: *b"gso ",
        payload: Vec::new(),
        ctrl_index: Some(control_index),
    });
    para.controls
        .push(hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"gso ",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: vec![hwp_model::ShapeGeom {
                kind: hwp_model::ShapeKind::Rect,
                x: 0,
                y: 0,
                w: 5000,
                h: 2000,
                points: Vec::new(),
                fill: 0xFFFF_FFFF,
                fill_gradient: None,
                border_color: 0,
                border_width: 10,
                round_ratio: 0,
                border_style: 0,
                arrow_start: 0,
                arrow_end: 0,
                anchored: true,
                description: None,
            }],
            equation: None,
            column_def: None,
            caption: Some(hwp_model::Caption {
                side: hwp_model::CaptionSide::Bottom,
                direction: hwp_model::CaptionDirection::Horizontal,
                gap: 200,
                width: None,
                last_width: 5000,
                paragraphs: vec![hwp_model::Paragraph {
                    chars: "shape-caption"
                        .chars()
                        .map(hwp_model::HwpChar::Text)
                        .collect(),
                    ..hwp_model::Paragraph::default()
                }],
            }),
            hwpx_raw_xml: None,
            container_box: None,
        }));

    let (list, _) = 표_레이아웃(&doc);
    let text = page_texts(&list.pages[0]).concat();
    if text.is_empty() {
        eprintln!("skip: no usable font for shape caption shaping");
        return;
    }
    assert!(text.contains("shape-caption"), "{text:?}");
}

/// TABLE policy splits at row boundaries without silently clipping cells.
#[test]
fn 표_쪽분할_행경계_클립없음() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (_body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - _body_top;
    let rows = (page_h / 100.0) as u16 + 3; // 2쪽 보장
    let doc = 표_분할_문서(1, rows, 100, 0, false, 0);
    let (list, report) = 표_레이아웃(&doc);
    assert!(
        list.pages.len() >= 2,
        "행 경계에서 쪽이 나뉘어야 한다: pages={}",
        list.pages.len()
    );
    let mut data_rects = 0;
    for pi in 0..list.pages.len() {
        let page_rects = 채움_사각형(&list, pi);
        assert!(
            !page_rects.is_empty(),
            "every table page must receive its fragment: page {pi}"
        );
        for &(y, h, fill) in &page_rects {
            assert_eq!(fill, 0x00C8_C8C8);
            data_rects += 1;
            assert!(
                y + h <= body_bottom + 0.6,
                "셀이 본문 하한({body_bottom})을 넘으면 안 된다: y={y} h={h} (page {pi})"
            );
        }
    }
    assert_eq!(data_rects, rows as usize, "행 손실 없이 모두 그려야 한다");
    assert!(
        report
            .info
            .iter()
            .any(|i| i.code == hwp_render::RenderIssueCode::TableSplitAcrossPages),
        "분할이 info로 보고되어야 한다"
    );
}

#[test]
fn split_table_keeps_bottom_caption_on_final_fragment() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let rows = ((body_bottom - body_top) / 100.0) as u16 + 3;
    let mut doc = 표_분할_문서(1, rows, 100, 0, false, 0);
    let table = doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| paragraph.controls.iter_mut())
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .unwrap();
    table.caption = Some(hwp_model::Caption {
        side: hwp_model::CaptionSide::Bottom,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 300,
        width: None,
        last_width: 0,
        paragraphs: vec![hwp_model::Paragraph {
            chars: "split-caption"
                .chars()
                .map(hwp_model::HwpChar::Text)
                .collect(),
            ..hwp_model::Paragraph::default()
        }],
    });

    let (list, _) = 표_레이아웃(&doc);
    let texts: Vec<String> = list
        .pages
        .iter()
        .map(|page| page_texts(page).concat())
        .collect();
    if texts.iter().all(String::is_empty) {
        eprintln!("skip: no usable font for caption shaping");
        return;
    }
    assert!(list.pages.len() >= 2);
    assert!(texts.last().unwrap().contains("split-caption"), "{texts:?}");
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.contains("split-caption"))
            .count(),
        1,
        "{texts:?}"
    );
}

/// A repeated numbered-list header must replay the original marker. Cloning
/// the body list state after the first fragment would render 2., 3., ... on
/// continuation pages.
#[test]
fn 표_반복제목_목록번호는_원본상태로_재생() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let rows = ((body_bottom - body_top) / 100.0) as u16 + 3;
    let mut doc = 표_분할_문서(1 | 4, rows, 100, 1, false, 0);

    let list_doc = hwp_convert::from_markdown("1. 반복 제목");
    let numbered = list_doc.sections[0]
        .paragraphs
        .iter()
        .find(|paragraph| {
            list_doc
                .header
                .para_shapes
                .get(paragraph.para_shape.0 as usize)
                .is_some_and(|shape| shape.head_type() == 2)
        })
        .expect("numbered paragraph")
        .clone();
    let table = doc.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("table");
    table.cells[0].paragraphs = vec![numbered];

    let (list, report) = 표_레이아웃(&doc);
    if report.issues.iter().any(|issue| {
        matches!(
            issue.code,
            hwp_render::RenderIssueCode::ShapingFailed
                | hwp_render::RenderIssueCode::PageNumberShapingOmitted
        )
    }) {
        eprintln!("skip: no usable font for list marker shaping");
        return;
    }
    assert!(list.pages.len() >= 2);
    let marker_signature = |page: &hwp_render::display::PageList| {
        page.items
            .iter()
            .filter_map(|item| match item {
                hwp_render::display::Item::Glyphs { x, run, .. } => Some((
                    *x,
                    run.glyphs.iter().map(|glyph| glyph.id).collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .min_by(|left, right| left.0.partial_cmp(&right.0).unwrap())
            .expect("numbered header marker")
            .1
    };
    let expected = marker_signature(&list.pages[0]);
    assert!(!expected.is_empty());
    for (page_index, page) in list.pages.iter().enumerate().skip(1) {
        assert_eq!(
            marker_signature(page),
            expected,
            "continuation header advanced the list state on page {page_index}"
        );
    }
}

/// Repeat-header tables redraw leading header cells on continuation pages.
#[test]
fn 표_쪽분할_제목줄_반복() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    let rows = (page_h / 100.0) as u16 + 3;
    let doc = 표_분할_문서(1 | 4, rows, 100, 1, false, 0);
    let (list, _report) = 표_레이아웃(&doc);
    assert!(list.pages.len() >= 2);
    let mut header_rects = 0;
    let mut data_rects = 0;
    for pi in 0..list.pages.len() {
        let rects = 채움_사각형(&list, pi);
        let page_headers = rects.iter().filter(|r| r.2 == 0x0055_5555).count();
        assert_eq!(
            page_headers, 1,
            "each table page must contain exactly one header row: page {pi}"
        );
        header_rects += page_headers;
        data_rects += rects.iter().filter(|r| r.2 == 0x00C8_C8C8).count();
        if pi > 0 {
            let top = rects
                .iter()
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
                .expect("이어지는 쪽에도 행이 있어야 한다");
            assert_eq!(top.2, 0x0055_5555, "page {pi} 최상단은 제목 행이어야 한다");
            assert!(
                (top.0 - body_top).abs() < 0.6,
                "제목 행은 본문 상단에: y={}",
                top.0
            );
        }
    }
    assert_eq!(data_rects, rows as usize - 1, "데이터 행은 정확히 한 번씩");
    assert_eq!(
        header_rects,
        list.pages.len(),
        "제목 행은 첫 쪽 1회 + 이어지는 쪽마다 1회"
    );
}

#[test]
fn repeated_header_block_is_never_split_or_duplicated() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    let filler = ((page_h - 150.0) / 16.0).floor() as usize;
    let rows = (page_h / 100.0) as u16 + 4;
    let doc = 표_분할_문서(1 | 4, rows, 100, 2, false, filler);
    let (list, _report) = 표_레이아웃(&doc);

    let mut data_rects = 0usize;
    for page in 0..list.pages.len() {
        let rects = 채움_사각형(&list, page);
        if rects.is_empty() {
            continue;
        }
        assert_eq!(
            rects.iter().filter(|rect| rect.2 == 0x0055_5555).count(),
            2,
            "each table page must contain the complete two-row header: page {page}"
        );
        data_rects += rects.iter().filter(|rect| rect.2 == 0x00C8_C8C8).count();
    }
    assert_eq!(data_rects, rows as usize - 2);
}

#[test]
fn row_spanning_header_repeats_as_one_block() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    let rows = (page_h / 100.0) as u16 + 4;
    let mut doc = 표_분할_문서(1 | 4, rows, 100, 1, false, 0);
    let table = doc.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("table");
    table.cells[0].row_span = 2;
    table.cells.remove(1);
    table.row_cell_counts[1] = 0;

    let (list, _report) = 표_레이아웃(&doc);
    assert!(list.pages.len() >= 2);
    for page in 0..list.pages.len() {
        let header_rects: Vec<_> = 채움_사각형(&list, page)
            .into_iter()
            .filter(|rect| rect.2 == 0x0055_5555)
            .collect();
        assert_eq!(header_rects.len(), 1, "page {page}");
        assert!((header_rects[0].1 - 200.0).abs() < 0.6, "page {page}");
    }
}

/// NONE moves a page-sized table wholesale to the next page.
#[test]
fn 표_page_break_none이면_통째로_다음쪽() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    // Leave less than 300 pt on the first page.
    let filler = ((page_h - 300.0) / 16.0).floor() as usize;
    let doc = 표_분할_문서(0, 3, 100, 0, false, filler);
    let (list, _report) = 표_레이아웃(&doc);
    assert_eq!(list.pages.len(), 2, "통째로 다음 쪽으로 밀려야 한다");
    assert!(
        채움_사각형(&list, 0).is_empty(),
        "첫 쪽에는 표 조각이 없어야 한다"
    );
    let p2 = 채움_사각형(&list, 1);
    assert_eq!(p2.len(), 3);
    let min_y = p2.iter().map(|r| r.0).fold(f32::INFINITY, f32::min);
    assert!(
        (min_y - body_top).abs() < 0.6,
        "표는 새 쪽 본문 상단에서 시작: y={min_y}"
    );
}

#[test]
fn top_caption_moves_with_an_unsplittable_table() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    let filler = ((page_h - 300.0) / 16.0).floor() as usize;
    let mut doc = 표_분할_문서(0, 3, 100, 0, false, filler);
    let table = doc.sections[0]
        .paragraphs
        .iter_mut()
        .flat_map(|paragraph| paragraph.controls.iter_mut())
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .unwrap();
    table.caption = Some(hwp_model::Caption {
        side: hwp_model::CaptionSide::Top,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 300,
        width: None,
        last_width: 0,
        paragraphs: vec![hwp_model::Paragraph {
            chars: "moved-top-caption"
                .chars()
                .map(hwp_model::HwpChar::Text)
                .collect(),
            ..hwp_model::Paragraph::default()
        }],
    });

    let (list, _) = 표_레이아웃(&doc);
    assert_eq!(list.pages.len(), 2);
    assert!(채움_사각형(&list, 0).is_empty());
    let page_text: Vec<String> = list
        .pages
        .iter()
        .map(|page| page_texts(page).concat())
        .collect();
    if page_text.iter().all(String::is_empty) {
        eprintln!("skip: no usable font for caption shaping");
        return;
    }
    assert!(!page_text[0].contains("moved-top-caption"), "{page_text:?}");
    assert!(page_text[1].contains("moved-top-caption"), "{page_text:?}");
    let table_top = 채움_사각형(&list, 1)
        .iter()
        .map(|rect| rect.0)
        .fold(f32::INFINITY, f32::min);
    assert!(
        table_top > body_top,
        "the top caption and gap must precede the table: y={table_top}"
    );
}

/// An inline table is indivisible (GE-8); oversized content is reported.
#[test]
fn 표_글자처럼취급은_분할하지_않음() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    let rows = (page_h / 100.0) as u16 + 3;
    let doc = 표_분할_문서(1, rows, 100, 0, true, 0);
    let (list, report) = 표_레이아웃(&doc);
    assert_eq!(list.pages.len(), 1, "한 글자 표는 쪽을 나누지 않는다");
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == hwp_render::RenderIssueCode::TableRowTooTallClipped),
        "넘침이 보고되어야 한다"
    );
}

/// A row-spanning cell makes every crossed boundary illegal. The complete
/// span must move to one page instead of being truncated at the natural split.
#[test]
fn 표_쪽분할_병합셀은_절단하지_않음() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let page_h = body_bottom - body_top;
    let rows = (page_h / 100.0) as u16 + 3;
    let fit1 = ((page_h - 16.0) / 100.0).floor() as u16; // 첫 쪽에 들어가는 행 수
    let mut doc = 표_분할_문서(1, rows, 100, 0, false, 0);
    let table = doc.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find_map(|c| match c {
            hwp_model::Control::Table(t) => Some(t),
            _ => None,
        })
        .expect("표 컨트롤");
    // Span three rows across the natural split point.
    let cell = table
        .cells
        .iter_mut()
        .find(|c| c.row == fit1 - 1)
        .expect("행 존재");
    cell.row_span = 3;
    let (list, _report) = 표_레이아웃(&doc);
    assert!(list.pages.len() >= 2);
    let span_rect = (0..list.pages.len())
        .flat_map(|page| 채움_사각형(&list, page))
        .find(|(_, h, _)| (*h - 300.0).abs() < 0.6)
        .expect("the complete three-row span must be emitted once");
    assert!((span_rect.1 - 300.0).abs() < 0.6);
    for pi in 0..list.pages.len() {
        for &(y, h, _) in &채움_사각형(&list, pi) {
            assert!(
                y + h <= body_bottom + 0.6,
                "절단된 병합 셀도 본문 안에: y={y} h={h} (page {pi})"
            );
        }
    }
}

#[test]
fn table_fragments_reserve_pending_footnote_space() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let rows = ((body_bottom - body_top) / 100.0) as u16 + 3;
    let mut doc = 표_분할_문서(1, rows, 100, 0, false, 0);

    let note_paragraph = hwp_convert::from_markdown("footnote body")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    let anchor = &mut doc.sections[0].paragraphs[0];
    let note_index = anchor.controls.len() as u32;
    anchor.chars.push(hwp_model::HwpChar::ExtCtrl {
        code: hwp_model::ctrl_char::FOOTNOTE_ENDNOTE,
        ctrl_id: *b"fn  ",
        payload: vec![0; 12],
        ctrl_index: Some(note_index),
    });
    anchor
        .controls
        .push(hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"fn  ",
            data: Vec::new(),
            paragraph_lists: vec![hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![note_paragraph],
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

    let (list, _report) = 표_레이아웃(&doc);
    let separator_y = list.pages[0]
        .items
        .iter()
        .find_map(|item| match item {
            // Footnote separator: horizontal path longer than 100 pt and at most 0.6 pt wide.
            hwp_render::display::Item::Path {
                commands,
                stroke: Some(stroke),
                ..
            } => match commands.as_slice() {
                [
                    hwp_render::display::PathCmd::MoveTo(x1, y1),
                    hwp_render::display::PathCmd::LineTo(x2, y2),
                ] if x2 - x1 > 100.0 && (y2 - y1).abs() < 0.01 && stroke.width <= 0.6 => Some(*y1),
                _ => None,
            },
            _ => None,
        })
        .expect("footnote separator");
    let table_bottom = 채움_사각형(&list, 0)
        .iter()
        .map(|(y, h, _)| y + h)
        .fold(0.0f32, f32::max);
    assert!(
        table_bottom <= separator_y + 0.6,
        "table fragment overlaps footnote reservation: table={table_bottom}, note={separator_y}"
    );
}

#[test]
fn uncovered_declared_rows_do_not_create_blank_pages() {
    let mut doc = 표_분할_문서(1, 1, 18, 0, false, 0);
    let table = doc.sections[0].paragraphs[0]
        .controls
        .iter_mut()
        .find_map(|control| match control {
            hwp_model::Control::Table(table) => Some(table),
            _ => None,
        })
        .expect("table");
    table.rows = u16::MAX;
    table.cells.clear();
    table.row_cell_counts.clear();

    let (list, report) = 표_레이아웃(&doc);
    assert_eq!(list.pages.len(), 1);
    assert!(채움_사각형(&list, 0).is_empty());
    assert!(
        report
            .issues
            .iter()
            .any(|issue| { issue.code == hwp_render::RenderIssueCode::InvalidTableCellOmitted })
    );
}

/// A continuation fragment is real page flow. A following explicit page
/// break must still open a new page instead of being suppressed by the table
/// transition's reset state.
#[test]
fn 표_연속쪽_뒤_명시적_쪽나눔을_보존() {
    let probe = 표_분할_문서(0, 1, 100, 0, false, 0);
    let (body_top, body_bottom) = 본문_기하(&probe);
    let rows = ((body_bottom - body_top) / 100.0) as u16 + 3;
    let mut doc = 표_분할_문서(1, rows, 100, 0, false, 0);

    // Force the anchor through the cached-lineseg path, where the outer
    // fallback branch does not restore flow state after object layout.
    let source = hwp_convert::from_markdown("앵커");
    let anchor = &mut doc.sections[0].paragraphs[0];
    anchor.chars = source.sections[0].paragraphs[0].chars.clone();
    anchor.line_segs = vec![hwp_model::LineSeg {
        text_start: 0,
        v_pos: 0,
        line_height: 2000,
        text_height: 2000,
        baseline_gap: 1600,
        line_spacing: 0,
        col_start: 0,
        seg_width: 50000,
        flags: 0x0006_0000,
    }];
    let mut following = hwp_convert::from_markdown("명시적 다음 쪽")
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    following.header.break_type |= 0x04;
    doc.sections[0].paragraphs.push(following);

    let (list, _report) = 표_레이아웃(&doc);
    assert!(
        list.pages.len() >= 3,
        "the explicit break after a two-page table must create a third page: {}",
        list.pages.len()
    );
}

/// A table attached to a page-spanning paragraph uses the new page's flow y.
#[test]
fn 페이지_걸친_문단의_표는_새쪽_흐름위치에_앵커() {
    let mut doc = 표_분할_문서(1, 1, 50, 0, false, 0); // 1행 50pt 표
    let (body_top, _bb) = 본문_기하(&doc);
    let src = hwp_convert::from_markdown("가나다라");
    let chars = src.sections[0].paragraphs[0].chars.clone();
    let para = &mut doc.sections[0].paragraphs[0];
    para.chars = chars;
    // Mark the second line as the first line of a page.
    let seg = |text_start, flags| hwp_model::LineSeg {
        text_start,
        v_pos: 0,
        line_height: 2000,
        text_height: 2000,
        baseline_gap: 1600,
        line_spacing: 0,
        col_start: 0,
        seg_width: 50000,
        flags,
    };
    para.line_segs = vec![seg(0, 0x0006_0000), seg(2, 0x0006_0001)];
    let (list, _report) = 표_레이아웃(&doc);
    assert_eq!(list.pages.len(), 2, "둘째 줄에서 쪽이 나뉘어야 한다");
    let p2 = 채움_사각형(&list, 1);
    assert_eq!(p2.len(), 1);
    // A 20 pt line puts the table at body_top + 20, not at stale body_top.
    assert!(
        (p2[0].0 - (body_top + 20.0)).abs() < 0.6,
        "표는 새 쪽 흐름 위치에: y={} (기대 {})",
        p2[0].0,
        body_top + 20.0
    );
}

/// GG-17 draws a styled divider at the midpoint of a two-column gap.
#[test]
fn renders_multi_column_divider() {
    use hwp_model::{BorderLine, ColumnDef, Control, GenericControl};
    use hwp_render::display::{Item, PathCmd};

    let mut doc = hwp_convert::from_markdown("본문 한 줄.\n");
    doc.sections[0].paragraphs[0]
        .controls
        .push(Control::Generic(GenericControl {
            ctrl_id: *b"cold",
            data: Vec::new(),
            paragraph_lists: Vec::new(),
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: Some(ColumnDef {
                count: 2,
                kind: 0,
                direction: 0,
                same_width: true,
                gap: 1417, // HWPUNIT ≈ 14.17pt
                widths: Vec::new(),
                divider: Some(BorderLine {
                    line_type: 3, // DOT
                    width: 3,     // 0.2mm
                    color: 0,
                }),
            }),
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        }));

    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    let page = &list.pages[0];

    // Expected x: body left + column width + half the gap.
    let p = doc.sections[0].section_def().unwrap().page.unwrap();
    let body_left = p.margin_left.0 as f32 / 100.0;
    let (body_top, body_bottom) = 본문_기하(&doc);
    let body_width = (p.width.0 - p.margin_left.0 - p.margin_right.0) as f32 / 100.0;
    let gap = 1417.0f32 / 100.0;
    let expect_x = body_left + (body_width - gap) / 2.0 + gap / 2.0;

    let divider = page
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Path {
                commands,
                stroke: Some(s),
                ..
            } => match commands.as_slice() {
                [PathCmd::MoveTo(x1, y1), PathCmd::LineTo(x2, y2)] if (x1 - x2).abs() < 0.01 => {
                    Some((*x1, *y1, *y2, s))
                }
                _ => None,
            },
            _ => None,
        })
        .find(|(x, y1, y2, _)| {
            (x - expect_x).abs() < 0.1
                && (y1 - body_top).abs() < 0.1
                && (y2 - body_bottom).abs() < 0.1
        })
        .expect("expected a vertical column divider");
    assert!(
        !divider.3.dash.is_empty(),
        "DOT divider must use a dash pattern"
    );

    // A default single-column document has no divider.
    let doc = hwp_convert::from_markdown("본문 한 줄.\n");
    let mut store = hwp_render::FontStore::new();
    let mut warns = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warns);
    let has_vertical = list.pages[0].items.iter().any(|it| {
        matches!(
            it,
            Item::Path { commands, .. }
            if matches!(commands.as_slice(),
                [PathCmd::MoveTo(x1, _), PathCmd::LineTo(x2, _)] if (x1 - x2).abs() < 0.01)
        )
    });
    assert!(!has_vertical, "single-column document must have no divider");
}

// ---------- Odd/even furniture + endnote placement (GG-16 / GG-14) ----------

/// Builds a `head` control whose paragraph list holds one text paragraph.
/// `apply` is the head/foot target (0=both, 1=even, 2=odd); `None`
/// leaves the data empty, which the renderer treats as BOTH.
fn 머리말_컨트롤(apply: Option<u32>, text: &str) -> hwp_model::Control {
    let para = hwp_convert::from_markdown(text)
        .sections
        .remove(0)
        .paragraphs
        .remove(0);
    generic_control(
        *b"head",
        apply.map_or_else(Vec::new, |a| a.to_le_bytes().to_vec()),
        vec![hwp_model::ParagraphList {
            header_data: Vec::new(),
            paragraphs: vec![para],
        }],
    )
}

/// GG-16: ODD/EVEN headers render only on matching printed pages.
#[test]
fn 머리말_홀짝_적용대상_선택() {
    let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n");
    doc.sections[0].paragraphs[1].header.break_type |= 0x04;
    doc.sections[0].paragraphs[0]
        .controls
        .push(머리말_컨트롤(Some(2), "홀수머리말"));
    doc.sections[0].paragraphs[0]
        .controls
        .push(머리말_컨트롤(Some(1), "짝수머리말"));

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    assert_eq!(list.pages.len(), 2);
    let p1 = page_texts(&list.pages[0]);
    if p1.is_empty() {
        eprintln!("스킵: 사용 가능한 폰트 없음 - 머리말 글리프 미생성");
        return;
    }
    let p1: String = p1.concat();
    let p2: String = page_texts(&list.pages[1]).concat();
    assert!(p1.contains("홀수머리말"), "1쪽(홀수)에 홀수 머리말: {p1:?}");
    assert!(
        !p1.contains("짝수머리말"),
        "1쪽(홀수)에 짝수 머리말 금지: {p1:?}"
    );
    assert!(p2.contains("짝수머리말"), "2쪽(짝수)에 짝수 머리말: {p2:?}");
    assert!(
        !p2.contains("홀수머리말"),
        "2쪽(짝수)에 홀수 머리말 금지: {p2:?}"
    );
}

/// GG-16: a single header with no apply data defaults to BOTH.
#[test]
fn 머리말_단일_모든페이지_반복() {
    let mut doc = hwp_convert::from_markdown("첫 쪽\n\n둘째 쪽\n");
    doc.sections[0].paragraphs[1].header.break_type |= 0x04;
    doc.sections[0].paragraphs[0]
        .controls
        .push(머리말_컨트롤(None, "공통머리말"));

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    assert_eq!(list.pages.len(), 2);
    let p1 = page_texts(&list.pages[0]);
    if p1.is_empty() {
        eprintln!("스킵: 사용 가능한 폰트 없음 - 머리말 글리프 미생성");
        return;
    }
    let p1: String = p1.concat();
    let p2: String = page_texts(&list.pages[1]).concat();
    assert!(p1.contains("공통머리말"), "1쪽에 공통 머리말: {p1:?}");
    assert!(p2.contains("공통머리말"), "2쪽에 공통 머리말: {p2:?}");
}

#[test]
fn parity_specific_header_does_not_leak_to_the_opposite_page() {
    let mut doc = hwp_convert::from_markdown("First\n\nSecond\n");
    doc.sections[0].paragraphs[1].header.break_type |= 0x04;
    doc.sections[0].paragraphs[0]
        .controls
        .push(머리말_컨트롤(Some(2), "odd-only-header"));

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    let first = page_texts(&list.pages[0]).concat();
    if first.is_empty() {
        eprintln!("skip: no usable font for header shaping");
        return;
    }
    let second = page_texts(&list.pages[1]).concat();
    assert!(first.contains("odd-only-header"), "{first:?}");
    assert!(!second.contains("odd-only-header"), "{second:?}");
}

/// GG-14: endnotes move to the section-closing block while footnotes remain on
/// their anchor page.
#[test]
fn 미주는_구역끝_각주는_앵커페이지() {
    let mut doc = hwp_convert::from_markdown("본문\n\n다음 쪽\n");
    doc.sections[0].paragraphs[1].header.break_type |= 0x04;

    let note = |ctrl_id: &[u8; 4], text: &str| {
        let para = hwp_convert::from_markdown(text)
            .sections
            .remove(0)
            .paragraphs
            .remove(0);
        hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *ctrl_id,
            data: Vec::new(),
            paragraph_lists: vec![hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs: vec![para],
            }],
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: Vec::new(),
            equation: None,
            column_def: None,
            caption: None,
            hwpx_raw_xml: None,
            container_box: None,
        })
    };
    let anchor = &mut doc.sections[0].paragraphs[0];
    let base = anchor.controls.len() as u32;
    for (i, ctrl_id) in [*b"fn  ", *b"en  "].iter().enumerate() {
        anchor.chars.push(hwp_model::HwpChar::ExtCtrl {
            code: hwp_model::ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *ctrl_id,
            payload: vec![0; 12],
            ctrl_index: Some(base + i as u32),
        });
    }
    anchor.controls.push(note(b"fn  ", "각주내용"));
    anchor.controls.push(note(b"en  ", "미주내용"));

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    assert_eq!(list.pages.len(), 2);
    let p1 = page_texts(&list.pages[0]);
    if p1.is_empty() {
        eprintln!("스킵: 사용 가능한 폰트 없음 - 노트 글리프 미생성");
        return;
    }
    let p1: String = p1.concat();
    let p2: String = page_texts(&list.pages[1]).concat();
    assert!(p1.contains("각주내용"), "각주는 앵커 페이지(1쪽)에: {p1:?}");
    assert!(
        !p1.contains("미주내용"),
        "미주는 앵커 페이지(1쪽)에 그리지 않는다: {p1:?}"
    );
    assert!(p2.contains("미주내용"), "미주는 구역 끝(2쪽)에: {p2:?}");
    assert!(!p2.contains("각주내용"), "각주는 2쪽에 없다: {p2:?}");
}

#[test]
fn endnotes_paginate_across_all_required_pages() {
    let mut doc = hwp_convert::from_markdown("Body\n");
    let anchor = &mut doc.sections[0].paragraphs[0];
    for number in 0..80u32 {
        let control_index = anchor.controls.len() as u32;
        anchor.chars.push(hwp_model::HwpChar::ExtCtrl {
            code: hwp_model::ctrl_char::FOOTNOTE_ENDNOTE,
            ctrl_id: *b"en  ",
            payload: vec![0; 12],
            ctrl_index: Some(control_index),
        });
        let para = hwp_model::Paragraph {
            chars: format!("ENDNOTE-{number:03}")
                .chars()
                .map(hwp_model::HwpChar::Text)
                .collect(),
            ..hwp_model::Paragraph::default()
        };
        anchor
            .controls
            .push(hwp_model::Control::Generic(hwp_model::GenericControl {
                ctrl_id: *b"en  ",
                data: Vec::new(),
                paragraph_lists: vec![hwp_model::ParagraphList {
                    header_data: Vec::new(),
                    paragraphs: vec![para],
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
    }

    let mut store = hwp_render::FontStore::new();
    let mut warnings = hwp_render::RenderIssueAccumulator::new();
    let list = hwp_render::layout::layout_document(&doc, &mut store, &mut warnings);
    let page_text: Vec<String> = list
        .pages
        .iter()
        .map(|page| page_texts(page).concat())
        .collect();
    if page_text.iter().all(String::is_empty) {
        eprintln!("skip: no usable font for endnote shaping");
        return;
    }
    assert!(list.pages.len() >= 3, "pages={}", list.pages.len());
    for number in 0..80u32 {
        let needle = format!("ENDNOTE-{number:03}");
        assert_eq!(
            page_text
                .iter()
                .filter(|text| text.contains(&needle))
                .count(),
            1,
            "{needle}: {page_text:?}"
        );
    }
}
