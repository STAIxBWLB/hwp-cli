//! Converts border line types (`BorderLine.line_type`) into strokes.
//!
//! GG-5/GG-6/GG-17/GG-24 replace the old visibility-only handling with
//! dashed and compound strokes. Borders are emitted as `Item::Path` values,
//! and dash lengths use the same width-relative rules as `shape_draw`.

use hwp_model::BorderLine;

use crate::display::{Item, PathCmd, Stroke};

/// Returns `(normal offset in pt, stroke)` entries for one border line.
///
/// `NONE` (0) produces no entries. The width splits and offsets for compound
/// lines (8 through 11) are visual approximations pending Hancom verification.
/// `w` is the total line width in points with a 0.2 pt lower bound.
pub fn border_strokes(line: &BorderLine) -> Vec<(f32, Stroke)> {
    let w = (line.width_mm() * 72.0 / 25.4).max(0.2); // mm to pt
    let u = w.max(0.5); // Same lower bound as shape_draw::dash_pattern.
    let solid = |width: f32| Stroke::solid(line.color, width);
    let dashed = |pattern: &[f32]| Stroke {
        color: line.color,
        width: w,
        dash: pattern.iter().map(|v| v * u).collect(),
    };
    match line.line_type {
        0 => Vec::new(),            // NONE
        1 => vec![(0.0, solid(w))], // SOLID
        // Dashed variants 2 through 6: DASH/DOT/DASH_DOT/DASH_DOT_DOT/LONG_DASH.
        2 => vec![(0.0, dashed(&[3.0, 2.0]))],
        3 => vec![(0.0, dashed(&[1.0, 2.0]))],
        4 => vec![(0.0, dashed(&[3.0, 2.0, 1.0, 2.0]))],
        5 => vec![(0.0, dashed(&[3.0, 2.0, 1.0, 2.0, 1.0, 2.0]))],
        6 => vec![(0.0, dashed(&[6.0, 3.0]))],
        // CIRCLE (7): approximate with dots because backends do not expose round caps.
        7 => vec![(0.0, dashed(&[1.0, 2.0]))],
        // DOUBLE_SLIM (8): two thin strokes.
        8 => vec![(-0.35 * w, solid(0.3 * w)), (0.35 * w, solid(0.3 * w))],
        // SLIM_THICK (9): thin outer stroke and thick inner stroke on rectangles.
        9 => vec![(-0.4 * w, solid(0.2 * w)), (0.25 * w, solid(0.5 * w))],
        // THICK_SLIM (10): the mirror of type 9.
        10 => vec![(-0.25 * w, solid(0.5 * w)), (0.4 * w, solid(0.2 * w))],
        // SLIM_THICK_SLIM (11): thin, thick, thin.
        11 => vec![
            (-0.4 * w, solid(0.2 * w)),
            (0.0, solid(0.3 * w)),
            (0.4 * w, solid(0.2 * w)),
        ],
        // Unknown codes fall back to solid, matching the HWPX reader policy.
        _ => vec![(0.0, solid(w))],
    }
}

/// Emits one open path per stroke for a standalone border segment.
///
/// Positive offsets use the segment's right-hand normal `(dy, -dx) / len`.
/// Rectangle borders must use [`border_rectangle_items`] so each side shares
/// the same inward/outward convention and compound strokes meet at corners.
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

/// Emits border paths for a rectangle with sides ordered left, right, top, bottom.
///
/// Positive offsets always point into the rectangle. When all four enabled
/// sides use the same style, each compound stroke is a single closed path.
/// Mixed or partial borders use intersecting endpoints at adjacent stroke
/// offsets so matching compound strokes still meet without corner gaps.
pub fn border_rectangle_items(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    sides: &[BorderLine; 4],
    enabled: [bool; 4],
) -> Vec<Item> {
    const LEFT: usize = 0;
    const RIGHT: usize = 1;
    const TOP: usize = 2;
    const BOTTOM: usize = 3;

    if x2 - x1 < 1e-3 || y2 - y1 < 1e-3 {
        return Vec::new();
    }

    if enabled.iter().all(|value| *value) && sides.iter().all(|side| *side == sides[0]) {
        return border_strokes(&sides[0])
            .into_iter()
            .map(|(offset, stroke)| Item::Path {
                commands: vec![
                    PathCmd::MoveTo(x1 + offset, y1 + offset),
                    PathCmd::LineTo(x2 - offset, y1 + offset),
                    PathCmd::LineTo(x2 - offset, y2 - offset),
                    PathCmd::LineTo(x1 + offset, y2 - offset),
                    PathCmd::Close,
                ],
                fill: None,
                stroke: Some(stroke),
            })
            .collect();
    }

    let strokes: [Vec<(f32, Stroke)>; 4] = std::array::from_fn(|side| {
        if enabled[side] {
            border_strokes(&sides[side])
        } else {
            Vec::new()
        }
    });
    let adjacent_offset = |side: usize, index: usize| {
        strokes[side]
            .get(index)
            .map(|(offset, _)| *offset)
            .unwrap_or(0.0)
    };
    let mut items = Vec::new();

    for (side, side_strokes) in strokes.iter().enumerate() {
        for (index, (offset, stroke)) in side_strokes.iter().enumerate() {
            let commands = match side {
                LEFT => vec![
                    PathCmd::MoveTo(x1 + offset, y1 + adjacent_offset(TOP, index)),
                    PathCmd::LineTo(x1 + offset, y2 - adjacent_offset(BOTTOM, index)),
                ],
                RIGHT => vec![
                    PathCmd::MoveTo(x2 - offset, y1 + adjacent_offset(TOP, index)),
                    PathCmd::LineTo(x2 - offset, y2 - adjacent_offset(BOTTOM, index)),
                ],
                TOP => vec![
                    PathCmd::MoveTo(x1 + adjacent_offset(LEFT, index), y1 + offset),
                    PathCmd::LineTo(x2 - adjacent_offset(RIGHT, index), y1 + offset),
                ],
                BOTTOM => vec![
                    PathCmd::MoveTo(x1 + adjacent_offset(LEFT, index), y2 - offset),
                    PathCmd::LineTo(x2 - adjacent_offset(RIGHT, index), y2 - offset),
                ],
                _ => unreachable!("rectangle has exactly four sides"),
            };
            items.push(Item::Path {
                commands,
                fill: None,
                stroke: Some(stroke.clone()),
            });
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(line_type: u8) -> BorderLine {
        BorderLine {
            line_type,
            width: 6, // 0.4 mm is approximately 1.13 pt.
            color: 0x00FF_0000,
        }
    }

    fn w() -> f32 {
        0.4 * 72.0 / 25.4
    }

    #[test]
    fn none_has_no_strokes() {
        assert!(border_strokes(&line(0)).is_empty());
        assert!(border_line_items(0.0, 0.0, 10.0, 0.0, &line(0)).is_empty());
    }

    #[test]
    fn solid_has_one_stroke() {
        let s = border_strokes(&line(1));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, 0.0);
        assert!(s[0].1.dash.is_empty());
        assert!((s[0].1.width - w()).abs() < 0.01);
        assert_eq!(s[0].1.color, 0x00FF_0000);
    }

    #[test]
    fn dashed_variants_have_width_relative_patterns() {
        let u = w().max(0.5);
        let dash = |t: u8| border_strokes(&line(t))[0].1.dash.clone();
        assert_eq!(dash(2), vec![3.0 * u, 2.0 * u]); // DASH
        assert_eq!(dash(3), vec![u, 2.0 * u]); // DOT
        assert_eq!(dash(4), vec![3.0 * u, 2.0 * u, u, 2.0 * u]); // DASH_DOT
        assert_eq!(dash(5), vec![3.0 * u, 2.0 * u, u, 2.0 * u, u, 2.0 * u]); // DASH_DOT_DOT
        assert_eq!(dash(6), vec![6.0 * u, 3.0 * u]); // LONG_DASH
        assert_eq!(dash(7), vec![u, 2.0 * u]); // CIRCLE approximates DOT.
    }

    #[test]
    fn compound_variants_have_expected_offsets() {
        let w = w();
        let st = |t: u8| border_strokes(&line(t));
        // DOUBLE_SLIM: two 0.3w strokes at +/-0.35w.
        let s = st(8);
        assert_eq!(s.len(), 2);
        assert!((s[0].0 + 0.35 * w).abs() < 0.01 && (s[1].0 - 0.35 * w).abs() < 0.01);
        assert!((s[0].1.width - 0.3 * w).abs() < 0.01);
        // SLIM_THICK: 0.2w @ -0.4w, 0.5w @ +0.25w
        let s = st(9);
        assert_eq!(s.len(), 2);
        assert!((s[0].1.width - 0.2 * w).abs() < 0.01 && (s[0].0 + 0.4 * w).abs() < 0.01);
        assert!((s[1].1.width - 0.5 * w).abs() < 0.01 && (s[1].0 - 0.25 * w).abs() < 0.01);
        // THICK_SLIM: the mirror of type 9.
        let s = st(10);
        assert_eq!(s.len(), 2);
        assert!((s[0].1.width - 0.5 * w).abs() < 0.01 && (s[0].0 + 0.25 * w).abs() < 0.01);
        assert!((s[1].1.width - 0.2 * w).abs() < 0.01 && (s[1].0 - 0.4 * w).abs() < 0.01);
        // SLIM_THICK_SLIM: 0.2w at +/-0.4w, 0.3w at zero.
        let s = st(11);
        assert_eq!(s.len(), 3);
        assert!((s[0].0 + 0.4 * w).abs() < 0.01);
        assert!((s[1].0).abs() < 0.01 && (s[1].1.width - 0.3 * w).abs() < 0.01);
        assert!((s[2].0 - 0.4 * w).abs() < 0.01);
    }

    #[test]
    fn standalone_segment_offsets_follow_right_hand_normal() {
        // Left-to-right: the first negative offset is below, the second is above.
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
            "expected symmetric horizontal offsets: {ys:?}"
        );
        // Top-to-bottom: the first negative offset is left, the second is right.
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
            "expected symmetric vertical offsets: {xs:?}"
        );
    }

    #[test]
    fn compound_rectangle_strokes_are_closed_and_consistently_inset() {
        let items = border_rectangle_items(10.0, 20.0, 110.0, 80.0, &[line(9); 4], [true; 4]);
        assert_eq!(items.len(), 2);

        let first_points = match &items[0] {
            Item::Path {
                commands,
                stroke: Some(stroke),
                ..
            } => {
                assert_eq!(commands.len(), 5);
                assert!(matches!(commands.last(), Some(PathCmd::Close)));
                assert!((stroke.width - 0.2 * w()).abs() < 0.01);
                commands
            }
            _ => panic!("expected a stroked path"),
        };
        let second_points = match &items[1] {
            Item::Path {
                commands,
                stroke: Some(stroke),
                ..
            } => {
                assert_eq!(commands.len(), 5);
                assert!(matches!(commands.last(), Some(PathCmd::Close)));
                assert!((stroke.width - 0.5 * w()).abs() < 0.01);
                commands
            }
            _ => panic!("expected a stroked path"),
        };

        let PathCmd::MoveTo(outer_x, outer_y) = first_points[0] else {
            panic!("expected MoveTo");
        };
        let PathCmd::MoveTo(inner_x, inner_y) = second_points[0] else {
            panic!("expected MoveTo");
        };
        assert!(outer_x < 10.0 && outer_y < 20.0);
        assert!(inner_x > 10.0 && inner_y > 20.0);
    }

    #[test]
    fn partial_rectangle_uses_inward_normals_on_opposite_sides() {
        let items = border_rectangle_items(
            10.0,
            20.0,
            110.0,
            80.0,
            &[line(9); 4],
            [true, true, false, false],
        );
        assert_eq!(items.len(), 4);

        let x_at = |item: &Item| match item {
            Item::Path { commands, .. } => match commands[0] {
                PathCmd::MoveTo(x, _) => x,
                _ => panic!("expected MoveTo"),
            },
            _ => panic!("expected Path"),
        };
        assert!(x_at(&items[0]) < 10.0);
        assert!(x_at(&items[1]) > 10.0);
        assert!(x_at(&items[2]) > 110.0);
        assert!(x_at(&items[3]) < 110.0);
    }
}
