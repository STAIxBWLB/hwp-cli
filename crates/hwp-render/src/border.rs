//! 테두리선 종류(BorderLine.line_type) → 스트로크 변환.
//!
//! GG-5/GG-6/GG-17/GG-24: 렌더러가 선 종류를 is_visible()로만 읽던 것을
//! 점선/이중선 스트로크로 옮긴다. 모든 테두리는 Item::Path(MoveTo/LineTo)로
//! emit된다(Item::Line 폐지). 점선 패턴은 shape_draw::dash_pattern과 같은
//! 굵기 비례 규칙을 쓴다.

use hwp_model::BorderLine;

use crate::display::{Item, PathCmd, Stroke};

/// 테두리선 → (법선 오프셋 pt, 스트로크) 목록. NONE(0)이면 빈 벡터.
///
/// 이중선 계열(8~11)의 굵기 분할·오프셋은 시각 근사값이다 — 정품 한글 대조
/// 라운드(Hancom verification)에서 확정한다. w = 선 전체 굵기(pt, 하한 0.2).
pub fn border_strokes(line: &BorderLine) -> Vec<(f32, Stroke)> {
    let w = (line.width_mm() * 72.0 / 25.4).max(0.2); // mm → pt
    let u = w.max(0.5); // 점선 단위 (shape_draw::dash_pattern과 동일 하한)
    let solid = |width: f32| Stroke::solid(line.color, width);
    let dashed = |pattern: &[f32]| Stroke {
        color: line.color,
        width: w,
        dash: pattern.iter().map(|v| v * u).collect(),
    };
    match line.line_type {
        0 => Vec::new(),            // NONE
        1 => vec![(0.0, solid(w))], // SOLID
        // 2~6 점선 계열: DASH/DOT/DASH_DOT/DASH_DOT_DOT/LONG_DASH.
        2 => vec![(0.0, dashed(&[3.0, 2.0]))],
        3 => vec![(0.0, dashed(&[1.0, 2.0]))],
        4 => vec![(0.0, dashed(&[3.0, 2.0, 1.0, 2.0]))],
        5 => vec![(0.0, dashed(&[3.0, 2.0, 1.0, 2.0, 1.0, 2.0]))],
        6 => vec![(0.0, dashed(&[6.0, 3.0]))],
        // 7 CIRCLE: 백엔드에 원형 캡 제어가 없어 점선으로 근사.
        7 => vec![(0.0, dashed(&[1.0, 2.0]))],
        // 8 DOUBLE_SLIM: 가는 두 줄.
        8 => vec![(-0.35 * w, solid(0.3 * w)), (0.35 * w, solid(0.3 * w))],
        // 9 SLIM_THICK: 위 가는 줄 + 아래 굵은 줄.
        9 => vec![(-0.4 * w, solid(0.2 * w)), (0.25 * w, solid(0.5 * w))],
        // 10 THICK_SLIM: 9의 대칭.
        10 => vec![(-0.25 * w, solid(0.5 * w)), (0.4 * w, solid(0.2 * w))],
        // 11 SLIM_THICK_SLIM: 가는-굵은-가는 세 줄.
        11 => vec![
            (-0.4 * w, solid(0.2 * w)),
            (0.0, solid(0.3 * w)),
            (0.4 * w, solid(0.2 * w)),
        ],
        // 미상 코드는 실선으로 강등(hwpx reader의 기본값 규칙과 동일).
        _ => vec![(0.0, solid(w))],
    }
}

/// 테두리 선분 → Item::Path 목록 (스트로크당 1개).
/// 오프셋은 선분 법선 n = (dy, -dx)/len 방향 — 좌→우 수평변에서 +오프셋은
/// 아래(본문 안쪽)다.
pub fn border_line_items(x1: f32, y1: f32, x2: f32, y2: f32, line: &BorderLine) -> Vec<Item> {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return Vec::new();
    }
    let (nx, ny) = (dy / len, -dx / len);
    border_strokes(line)
        .into_iter()
        .map(|(off, stroke)| Item::Path {
            commands: vec![
                PathCmd::MoveTo(x1 + nx * off, y1 + ny * off),
                PathCmd::LineTo(x2 + nx * off, y2 + ny * off),
            ],
            fill: None,
            stroke: Some(stroke),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(line_type: u8) -> BorderLine {
        BorderLine {
            line_type,
            width: 6, // 0.4mm ≈ 1.13pt
            color: 0x00FF_0000,
        }
    }

    fn w() -> f32 {
        0.4 * 72.0 / 25.4
    }

    #[test]
    fn 코드0은_스트로크_없음() {
        assert!(border_strokes(&line(0)).is_empty());
        assert!(border_line_items(0.0, 0.0, 10.0, 0.0, &line(0)).is_empty());
    }

    #[test]
    fn 실선은_단일_스트로크() {
        let s = border_strokes(&line(1));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, 0.0);
        assert!(s[0].1.dash.is_empty());
        assert!((s[0].1.width - w()).abs() < 0.01);
        assert_eq!(s[0].1.color, 0x00FF_0000);
    }

    #[test]
    fn 점선_계열_패턴() {
        let u = w().max(0.5);
        let dash = |t: u8| border_strokes(&line(t))[0].1.dash.clone();
        assert_eq!(dash(2), vec![3.0 * u, 2.0 * u]); // DASH
        assert_eq!(dash(3), vec![u, 2.0 * u]); // DOT
        assert_eq!(dash(4), vec![3.0 * u, 2.0 * u, u, 2.0 * u]); // DASH_DOT
        assert_eq!(dash(5), vec![3.0 * u, 2.0 * u, u, 2.0 * u, u, 2.0 * u]); // DASH_DOT_DOT
        assert_eq!(dash(6), vec![6.0 * u, 3.0 * u]); // LONG_DASH
        assert_eq!(dash(7), vec![u, 2.0 * u]); // CIRCLE ≈ DOT
    }

    #[test]
    fn 이중선_계열_오프셋() {
        let w = w();
        let st = |t: u8| border_strokes(&line(t));
        // DOUBLE_SLIM: 0.3w 두 줄 @ ±0.35w
        let s = st(8);
        assert_eq!(s.len(), 2);
        assert!((s[0].0 + 0.35 * w).abs() < 0.01 && (s[1].0 - 0.35 * w).abs() < 0.01);
        assert!((s[0].1.width - 0.3 * w).abs() < 0.01);
        // SLIM_THICK: 0.2w @ -0.4w, 0.5w @ +0.25w
        let s = st(9);
        assert_eq!(s.len(), 2);
        assert!((s[0].1.width - 0.2 * w).abs() < 0.01 && (s[0].0 + 0.4 * w).abs() < 0.01);
        assert!((s[1].1.width - 0.5 * w).abs() < 0.01 && (s[1].0 - 0.25 * w).abs() < 0.01);
        // THICK_SLIM: 9의 대칭
        let s = st(10);
        assert_eq!(s.len(), 2);
        assert!((s[0].1.width - 0.5 * w).abs() < 0.01 && (s[0].0 + 0.25 * w).abs() < 0.01);
        assert!((s[1].1.width - 0.2 * w).abs() < 0.01 && (s[1].0 - 0.4 * w).abs() < 0.01);
        // SLIM_THICK_SLIM: 0.2w @ ±0.4w, 0.3w @ 0
        let s = st(11);
        assert_eq!(s.len(), 3);
        assert!((s[0].0 + 0.4 * w).abs() < 0.01);
        assert!((s[1].0).abs() < 0.01 && (s[1].1.width - 0.3 * w).abs() < 0.01);
        assert!((s[2].0 - 0.4 * w).abs() < 0.01);
    }

    #[test]
    fn 오프셋은_법선_방향() {
        // 좌→우 수평변: +오프셋은 아래(y+). 이중선 두 줄은 위/아래 대칭.
        let items = border_line_items(0.0, 10.0, 100.0, 10.0, &line(8));
        assert_eq!(items.len(), 2);
        let ys: Vec<f32> = items
            .iter()
            .map(|it| match it {
                Item::Path { commands, .. } => match commands[0] {
                    PathCmd::MoveTo(_, y) => y,
                    _ => panic!("MoveTo"),
                },
                _ => panic!("Path"),
            })
            .collect();
        assert!(
            ys[0] > 10.0 && ys[1] < 10.0,
            "법선 대칭(+오프셋=아래): {ys:?}"
        );
        // 수직변(위→아래): +오프셋은 오른쪽(x+).
        let items = border_line_items(5.0, 0.0, 5.0, 50.0, &line(8));
        let xs: Vec<f32> = items
            .iter()
            .map(|it| match it {
                Item::Path { commands, .. } => match commands[0] {
                    PathCmd::MoveTo(x, _) => x,
                    _ => panic!("MoveTo"),
                },
                _ => panic!("Path"),
            })
            .collect();
        assert!(
            xs[0] < 5.0 && xs[1] > 5.0,
            "수직변 법선(+오프셋=오른쪽): {xs:?}"
        );
    }
}
