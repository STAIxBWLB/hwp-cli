//! Core named-style contracts without native writers, private fixtures or host fonts.
use hwp_convert::format::{CharFormat, ParaProps};
use hwp_convert::segment_id::SegmentPath;
use hwp_convert::styles::StyleKind;
use hwp_convert::styles::edit::{
    CreateStyle, StyleEditError, StyleSelector, UpdateStyle, apply_style_at, create_style,
    update_style,
};
use hwp_model::{
    CharShape, CharShapeId, Control, Document, FaceName, GenericControl, HwpChar, LineSeg,
    ParaShape, ParaShapeId, Paragraph, ParagraphList, Section, Style, StyleId,
};

fn cache() -> Vec<LineSeg> {
    vec![LineSeg {
        text_start: 0,
        v_pos: 10,
        line_height: 1000,
        text_height: 1000,
        baseline_gap: 850,
        line_spacing: 600,
        col_start: 0,
        seg_width: 4000,
        flags: 0,
    }]
}
fn para(text: &str, style: u16, shape: u16, runs: &[(u32, u16)]) -> Paragraph {
    Paragraph {
        chars: text.chars().map(HwpChar::Text).collect(),
        style: StyleId(style),
        para_shape: ParaShapeId(shape),
        char_shape_runs: runs.iter().map(|&(p, id)| (p, CharShapeId(id))).collect(),
        line_segs: cache(),
        ..Paragraph::default()
    }
}
fn fixture() -> Document {
    let mut doc = Document::default();
    doc.header.char_shapes = vec![
        CharShape {
            base_size: 1000,
            ..CharShape::default()
        },
        CharShape {
            base_size: 1000,
            attr: 2,
            ..CharShape::default()
        },
        CharShape {
            base_size: 1400,
            attr: 2,
            ..CharShape::default()
        },
    ];
    doc.header.para_shapes = vec![
        ParaShape::default(),
        ParaShape {
            attr1: 3 << 2,
            ..ParaShape::default()
        },
        ParaShape {
            attr1: 1 << 2,
            ..ParaShape::default()
        },
    ];
    doc.header.styles = vec![
        Style {
            name: "Body".into(),
            english_name: "Body alias".into(),
            attr: 0xa0,
            kind_unknown: false,
            next_style: 1,
            lang_id: 1042,
            tail: vec![7, 9],
            char_shape: CharShapeId(0),
            para_shape: ParaShapeId(0),
        },
        Style {
            name: "Note".into(),
            char_shape: CharShapeId(0),
            para_shape: ParaShapeId(0),
            ..Style::default()
        },
        Style {
            name: "Heading".into(),
            char_shape: CharShapeId(2),
            para_shape: ParaShapeId(2),
            ..Style::default()
        },
    ];
    doc.sections = vec![Section {
        paragraphs: vec![
            para("inherited", 0, 0, &[(0, 0)]),
            para("direct paragraph", 0, 1, &[(0, 0)]),
            para("abCDxy", 0, 0, &[(0, 0), (2, 1), (4, 0)]),
            para("heading-like direct", 0, 0, &[(0, 2)]),
            para("note untouched", 1, 0, &[(0, 0)]),
            para("heading untouched", 2, 2, &[(0, 2)]),
        ],
        ..Section::default()
    }];
    doc
}
fn path(indices: &[usize]) -> SegmentPath {
    SegmentPath {
        section: 0,
        indices: indices.to_vec(),
    }
}
fn selector(id: StyleId) -> StyleSelector {
    StyleSelector::Id(id.0)
}
fn request(name: &str) -> CreateStyle {
    CreateStyle {
        name: name.into(),
        based_on: Some(StyleSelector::Name("Body".into())),
        kind: StyleKind::Paragraph,
        character: CharFormat {
            size_pt: Some(13.0),
            ..CharFormat::default()
        },
        paragraph: ParaProps {
            align: Some(2),
            ..ParaProps::default()
        },
    }
}
fn shapes(doc: &Document, id: StyleId) -> (CharShapeId, ParaShapeId) {
    let s = &doc.header.styles[usize::from(id.0)];
    (s.char_shape, s.para_shape)
}

#[test]
fn create_copies_metadata_and_shapes_without_repointing_shared_users() {
    let mut doc = fixture();
    let before = doc.clone();
    let id = create_style(&mut doc, &request("New body")).unwrap();
    assert_eq!(usize::from(id.0), before.header.styles.len());
    let s = &doc.header.styles[usize::from(id.0)];
    assert_eq!(s.name, "New body");
    assert_eq!(s.english_name, "New body");
    assert_eq!(
        (s.attr, s.next_style, s.lang_id, s.tail.as_slice()),
        (0xa0, 1, 1042, &[7, 9][..])
    );
    assert_eq!(
        doc.header.char_shapes[usize::from(s.char_shape.0)].base_size,
        1300
    );
    assert_eq!(
        doc.header.para_shapes[usize::from(s.para_shape.0)].alignment(),
        2
    );
    assert_eq!(doc.sections, before.sections);
    assert_eq!(
        &doc.header.styles[..before.header.styles.len()],
        before.header.styles.as_slice()
    );
    assert_eq!(
        &doc.header.char_shapes[..before.header.char_shapes.len()],
        before.header.char_shapes.as_slice()
    );
    assert_eq!(
        &doc.header.para_shapes[..before.header.para_shapes.len()],
        before.header.para_shapes.as_slice()
    );
    let mut default = request("Default base");
    default.based_on = None;
    let default_id = create_style(&mut doc, &default).unwrap();
    assert_eq!(shapes(&doc, id), shapes(&doc, default_id));
}
#[test]
fn create_reuses_legacy_formatting_math_and_converts_dimensions_once() {
    let mut doc = fixture();
    let mut op = request("All properties");
    op.character = CharFormat {
        bold: Some(true),
        italic: Some(true),
        underline: Some(true),
        strike: Some(true),
        size_pt: Some(13.0),
        color: Some(0x123456),
        font: Some("New face".into()),
    };
    op.paragraph = ParaProps {
        line_spacing: Some((1, 1234)),
        indent: Some(111),
        margin_left: Some(222),
        margin_right: Some(333),
        spacing_top: Some(444),
        spacing_bottom: Some(555),
        align: Some(2),
    };
    let mut expected = doc.clone();
    assert_eq!(
        hwp_convert::set_char_format(&mut expected, "inherited", &op.character),
        1
    );
    assert_eq!(
        hwp_convert::set_para_props(&mut expected, "inherited", &op.paragraph),
        1
    );
    let id = create_style(&mut doc, &op).unwrap();
    let (cs, ps) = shapes(&doc, id);
    let p = &expected.sections[0].paragraphs[0];
    assert_eq!(
        doc.header.char_shapes[usize::from(cs.0)],
        expected.header.char_shapes[usize::from(p.char_shape_runs[0].1.0)]
    );
    assert_eq!(
        doc.header.para_shapes[usize::from(ps.0)],
        expected.header.para_shapes[usize::from(p.para_shape.0)]
    );
    assert_eq!(doc.header.fonts, expected.header.fonts);
}
#[test]
fn apply_decides_character_and_paragraph_inheritance_independently() {
    let mut doc = fixture();
    let target = create_style(&mut doc, &request("New body")).unwrap();
    let before = doc.clone();
    let (cs, ps) = shapes(&doc, target);
    for i in 0..4 {
        assert!(apply_style_at(&mut doc, &path(&[i]), &selector(target), false).unwrap());
    }
    let p = &doc.sections[0].paragraphs;
    assert_eq!(
        (p[0].para_shape, &p[0].char_shape_runs),
        (ps, &vec![(0, cs)])
    );
    assert_eq!(p[1].para_shape, before.sections[0].paragraphs[1].para_shape);
    assert_eq!(
        p[1].char_shape_runs,
        vec![(0, cs)],
        "direct PS must not block inherited CS"
    );
    assert_eq!(p[2].para_shape, ps);
    assert_eq!(
        p[2].char_shape_runs,
        vec![(0, cs), (2, CharShapeId(1)), (4, cs)]
    );
    assert_eq!(p[3].para_shape, ps);
    assert_eq!(
        p[3].char_shape_runs,
        vec![(0, CharShapeId(2))],
        "direct CS matching another style stays direct"
    );
    assert_eq!(&p[4..], &before.sections[0].paragraphs[4..]);
    for (i, paragraph) in p.iter().enumerate().take(4) {
        assert_eq!(paragraph.style, target);
        assert!(paragraph.line_segs.is_empty());
        assert_eq!(paragraph.chars, before.sections[0].paragraphs[i].chars);
        assert_eq!(
            paragraph.controls,
            before.sections[0].paragraphs[i].controls
        );
    }
    assert_eq!(doc.header, before.header);
}
#[test]
fn clear_direct_collapses_every_run_and_repeated_apply_preserves_caches() {
    let mut doc = fixture();
    let target = create_style(&mut doc, &request("New body")).unwrap();
    let (cs, ps) = shapes(&doc, target);
    for i in [1, 2, 3] {
        assert!(apply_style_at(&mut doc, &path(&[i]), &selector(target), true).unwrap());
        let p = &mut doc.sections[0].paragraphs[i];
        assert_eq!(p.para_shape, ps);
        assert_eq!(p.char_shape_runs, vec![(0, cs)]);
        p.line_segs = cache();
        let before = doc.clone();
        assert!(!apply_style_at(&mut doc, &path(&[i]), &selector(target), true).unwrap());
        assert_eq!(doc, before);
    }
    let before = doc.clone();
    assert!(!apply_style_at(&mut doc, &path(&[1]), &selector(target), false).unwrap());
    assert_eq!(doc, before);
}
fn generic(paragraphs: Vec<Paragraph>) -> GenericControl {
    GenericControl {
        ctrl_id: *b"gso ",
        data: vec![],
        paragraph_lists: vec![ParagraphList {
            header_data: vec![],
            paragraphs,
        }],
        extras: vec![],
        raw_children: vec![],
        gso_shapes: vec![],
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: Some("<captured/>".into()),
        container_box: None,
    }
}
#[test]
fn nested_generic_and_table_paths_invalidate_only_changed_ancestors() {
    let mut doc = fixture();
    let target = create_style(&mut doc, &request("New body")).unwrap();
    let sibling = doc.sections[0].paragraphs[4].clone();
    let leaf = doc.sections[0].paragraphs[1].clone();
    let mut g = generic(vec![sibling.clone()]);
    g.paragraph_lists.push(ParagraphList {
        header_data: vec![],
        paragraphs: vec![sibling.clone(), leaf],
    });
    let mut anchor = para("container", 0, 0, &[(0, 0)]);
    anchor.controls.push(Control::Generic(g));
    doc.sections[0].paragraphs.push(anchor);
    let i = doc.sections[0].paragraphs.len() - 1;
    assert!(apply_style_at(&mut doc, &path(&[i, 0, 2]), &selector(target), true).unwrap());
    let Control::Generic(g) = &doc.sections[0].paragraphs[i].controls[0] else {
        panic!("fixture generic");
    };
    assert!(g.hwpx_raw_xml.is_none());
    assert_eq!(g.paragraph_lists[0].paragraphs[0], sibling);
    assert_eq!(g.paragraph_lists[1].paragraphs[0], sibling);
    assert_eq!(g.paragraph_lists[1].paragraphs[1].style, target);
    assert_eq!(doc.sections[0].paragraphs[i].style, StyleId(0));
    let source = hwp_convert::from_markdown("| cell |\n|---|\n| value |\n");
    let mut t = source
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .find_map(|c| {
            if let Control::Table(t) = c {
                Some(t.clone())
            } else {
                None
            }
        })
        .unwrap();
    t.cells[0].paragraphs = vec![doc.sections[0].paragraphs[0].clone()];
    let mut anchor = para("table", 0, 0, &[(0, 0)]);
    anchor.controls.push(Control::Table(t));
    doc.sections[0].paragraphs.push(anchor);
    let i = doc.sections[0].paragraphs.len() - 1;
    assert!(apply_style_at(&mut doc, &path(&[i, 0, 0, 0]), &selector(target), false).unwrap());
    let Control::Table(t) = &doc.sections[0].paragraphs[i].controls[0] else {
        panic!("fixture table");
    };
    assert_eq!(t.cells[0].paragraphs[0].style, target);
    assert!(t.cells[0].paragraphs[0].line_segs.is_empty());
}
#[test]
fn applying_a_container_paragraph_does_not_restyle_its_descendants() {
    let mut doc = fixture();
    let target = create_style(&mut doc, &request("New body")).unwrap();
    let child = doc.sections[0].paragraphs[0].clone();
    doc.sections[0].paragraphs[0].controls = vec![Control::Generic(generic(vec![child]))];
    let controls = doc.sections[0].paragraphs[0].controls.clone();
    assert!(apply_style_at(&mut doc, &path(&[0]), &selector(target), true).unwrap());
    assert_eq!(doc.sections[0].paragraphs[0].controls, controls);
}
#[test]
fn exact_name_lookup_refuses_ambiguity_but_explicit_ids_remain_valid() {
    let mut doc = fixture();
    assert_eq!(
        StyleSelector::Name("Body".into()).resolve(&doc).unwrap(),
        StyleId(0)
    );
    assert!(
        StyleSelector::Name("Body alias".into())
            .resolve(&doc)
            .is_err()
    );
    assert!(StyleSelector::Name("body".into()).resolve(&doc).is_err());
    assert!(StyleSelector::Id(999).resolve(&doc).is_err());
    doc.header.styles[1].name = "Body".into();
    assert!(matches!(
        StyleSelector::Name("Body".into()).resolve(&doc),
        Err(StyleEditError::AmbiguousName(_))
    ));
    assert_eq!(StyleSelector::Id(1).resolve(&doc).unwrap(), StyleId(1));
}
#[test]
fn invalid_create_and_apply_requests_are_atomic() {
    let baseline = fixture();
    for name in ["", "bad\nname", "Body"] {
        let mut doc = baseline.clone();
        assert!(create_style(&mut doc, &request(name)).is_err());
        assert_eq!(doc, baseline);
    }
    let mut op = request("temporary");
    op.name = "😀".repeat(32768);
    let mut doc = baseline.clone();
    assert!(matches!(
        create_style(&mut doc, &op),
        Err(StyleEditError::InvalidName)
    ));
    assert_eq!(doc, baseline);
    let mut op = request("missing");
    op.based_on = Some(StyleSelector::Id(999));
    assert!(create_style(&mut doc, &op).is_err());
    assert_eq!(doc, baseline);
    for table in ["char", "para"] {
        let mut doc = baseline.clone();
        if table == "char" {
            doc.header.styles[0].char_shape = CharShapeId(999);
        } else {
            doc.header.styles[0].para_shape = ParaShapeId(999);
        }
        let before = doc.clone();
        assert!(matches!(
            create_style(&mut doc, &request("bad reference")),
            Err(StyleEditError::MissingShape { .. })
        ));
        assert_eq!(doc, before);
        assert!(apply_style_at(&mut doc, &path(&[0]), &StyleSelector::Id(0), false).is_err());
        assert_eq!(doc, before);
    }
    for kind in [2, 7] {
        let mut doc = baseline.clone();
        doc.header.styles[1].attr = kind;
        let before = doc.clone();
        assert!(matches!(
            apply_style_at(&mut doc, &path(&[0]), &StyleSelector::Id(1), false),
            Err(StyleEditError::UnsupportedKind(_))
        ));
        assert_eq!(doc, before);
    }
    let mut doc = baseline.clone();
    for invalid in [path(&[]), path(&[999]), path(&[0, 999])] {
        assert!(apply_style_at(&mut doc, &invalid, &StyleSelector::Id(0), false).is_err());
        assert_eq!(doc, baseline);
    }
    assert!(
        apply_style_at(
            &mut doc,
            &path(&[0]),
            &StyleSelector::Name("missing".into()),
            false
        )
        .is_err()
    );
    assert_eq!(doc, baseline);
}
#[test]
fn identifier_capacity_failures_are_atomic_and_equal_shapes_can_be_reused() {
    let max = usize::from(u16::MAX) + 1;
    for table in ["styles", "char_shapes", "para_shapes", "fonts"] {
        let mut doc = fixture();
        let mut op = request("New body");
        match table {
            "styles" => doc.header.styles.resize(max, Style::default()),
            "char_shapes" => {
                let base = doc.header.char_shapes[0].clone();
                doc.header.char_shapes.resize(max, base);
            }
            "para_shapes" => {
                let base = doc.header.para_shapes[0].clone();
                doc.header.para_shapes.resize(max, base);
            }
            "fonts" => {
                doc.header.fonts[6] = vec![
                    FaceName {
                        name: "Existing".into(),
                        ..FaceName::default()
                    };
                    max
                ];
                op.character.font = Some("New face".into());
            }
            _ => unreachable!(),
        }
        let before = doc.clone();
        assert!(
            matches!(
                create_style(&mut doc, &op),
                Err(StyleEditError::Capacity { .. })
            ),
            "capacity must not wrap: {table}"
        );
        assert_eq!(
            doc, before,
            "failure must not leave partial allocations: {table}"
        );
    }
    let mut doc = fixture();
    let cs = doc.header.char_shapes[0].clone();
    doc.header.char_shapes.resize(max, cs);
    let ps = doc.header.para_shapes[0].clone();
    doc.header.para_shapes.resize(max, ps);
    let op = CreateStyle {
        character: CharFormat::default(),
        paragraph: ParaProps::default(),
        ..request("Reuse full tables")
    };
    let id = create_style(&mut doc, &op).unwrap();
    assert_eq!(shapes(&doc, id), (CharShapeId(0), ParaShapeId(0)));
    assert_eq!(
        (doc.header.char_shapes.len(), doc.header.para_shapes.len()),
        (max, max)
    );
}

#[test]
fn utf16_name_boundary_keeps_base_metadata() {
    let mut doc = fixture();
    let mut operation = request("temporary");
    operation.name = format!("{}x", "😀".repeat(32767));
    let id = create_style(&mut doc, &operation).unwrap();
    let style = &doc.header.styles[usize::from(id.0)];
    assert_eq!(style.name.encode_utf16().count(), usize::from(u16::MAX));
    assert_eq!(style.attr, 0xa0);
    assert_eq!((style.next_style, style.lang_id), (1, 1042));
}

#[test]
fn character_style_creation_and_paragraph_application_are_refused_atomically() {
    let mut doc = fixture();
    let mut operation = request("Character accent");
    operation.kind = StyleKind::Character;
    let before = doc.clone();
    assert_eq!(
        create_style(&mut doc, &operation),
        Err(StyleEditError::CharacterStylesUnsupported)
    );
    assert_eq!(doc, before);
    doc.header.styles[1].attr = 1;
    let before = doc.clone();
    assert_eq!(
        apply_style_at(&mut doc, &path(&[0]), &StyleSelector::Id(1), false),
        Err(StyleEditError::CharacterStylesUnsupported)
    );
    assert_eq!(doc, before);
}

#[test]
fn unknown_target_kind_is_refused_without_mutation() {
    for bits in [0, 1, 7] {
        let mut doc = fixture();
        doc.header.styles[1].kind_unknown = true;
        doc.header.styles[1].attr = bits;
        let before = doc.clone();
        assert!(
            apply_style_at(&mut doc, &path(&[0]), &StyleSelector::Id(1), false).is_err(),
            "an unstated kind must not be inferred from attr {bits}"
        );
        assert_eq!(doc, before);
    }
}

#[test]
fn explicit_paragraph_creation_from_unknown_base_states_only_the_new_kind() {
    let mut doc = fixture();
    doc.header.styles[0].kind_unknown = true;
    let before = doc.clone();
    let id = create_style(&mut doc, &request("Explicit paragraph")).unwrap();
    assert!(
        !doc.header.styles[usize::from(id.0)].kind_unknown,
        "an explicit paragraph request states the new style's kind"
    );
    assert_eq!(doc.header.styles[0], before.header.styles[0]);
    assert_eq!(doc.sections, before.sections);
}

fn assert_empty_run_follows_only_inherited_zero(old_shape: u16) {
    let mut doc = fixture();
    doc.header.styles[0].char_shape = CharShapeId(old_shape);
    doc.sections[0].paragraphs[0].char_shape_runs.clear();
    assert!(!doc.sections[0].paragraphs[0].plain_text().is_empty());
    let target = create_style(&mut doc, &request("Empty-run target")).unwrap();
    let (target_shape, _) = shapes(&doc, target);
    assert_ne!(target_shape, CharShapeId(0));
    assert!(apply_style_at(&mut doc, &path(&[0]), &selector(target), false).unwrap());
    let expected = if old_shape == 0 {
        target_shape
    } else {
        CharShapeId(0)
    };
    assert_eq!(
        doc.sections[0].paragraphs[0].char_shape_runs,
        vec![(0, expected)],
        "empty runs must materialize effective shape0 before the inherited/direct decision"
    );
    doc.sections[0].paragraphs[0].line_segs = cache();
    let before = doc.clone();
    assert!(!apply_style_at(&mut doc, &path(&[0]), &selector(target), false).unwrap());
    assert_eq!(
        doc, before,
        "reapplication preserves caches after materializing the effective run"
    );
}

#[test]
fn empty_runs_follow_when_old_style_uses_shape_zero() {
    assert_empty_run_follows_only_inherited_zero(0);
}

#[test]
fn empty_runs_keep_direct_zero_when_old_style_uses_another_shape() {
    assert_empty_run_follows_only_inherited_zero(1);
}

#[test]
fn unknown_style_errors_show_a_name_or_number_without_debug_wrappers() {
    let name = StyleEditError::UnknownStyle(StyleSelector::Name("Missing body".into())).to_string();
    let id = StyleEditError::UnknownStyle(StyleSelector::Id(250)).to_string();
    assert!(
        name.contains("스타일 Missing body을 찾을 수 없습니다"),
        "{name}"
    );
    assert!(id.contains("스타일 #250을 찾을 수 없습니다"), "{id}");
    assert!(!name.contains("Name("));
    assert!(!id.contains("Id("));
}

/// T16-F3: shape zero is only an effective fallback for an absent run record;
/// applying the paragraph's own style must not materialize that record.
#[test]
fn own_style_without_runs_is_an_exact_noop_for_zero_and_nonzero_style_shapes() {
    for old_shape in [CharShapeId(0), CharShapeId(2)] {
        let mut doc = fixture();
        doc.header.styles[0].char_shape = old_shape;
        doc.sections[0].paragraphs[0].char_shape_runs.clear();
        assert!(!doc.sections[0].paragraphs[0].line_segs.is_empty());
        let before = doc.clone();
        let changed = apply_style_at(&mut doc, &path(&[0]), &StyleSelector::Id(0), false)
            .expect("existing own paragraph style is valid");
        assert!(
            !changed,
            "applying its own style must remain a no-op even without run records"
        );
        assert_eq!(
            doc, before,
            "a no-op must retain empty runs, saved caches and header definitions"
        );
    }
}

fn update_request() -> UpdateStyle {
    UpdateStyle {
        style: StyleSelector::Name("Body".into()),
        character: CharFormat {
            size_pt: Some(13.0),
            ..CharFormat::default()
        },
        paragraph: ParaProps {
            align: Some(2),
            ..ParaProps::default()
        },
    }
}

fn valid_font_fixture() -> Document {
    let mut doc = fixture();
    for fonts in &mut doc.header.fonts {
        *fonts = vec![FaceName {
            name: "Fixture Serif".into(),
            ..FaceName::default()
        }];
    }
    doc
}

#[test]
fn update_fresh_shapes_keep_independent_inheritance_and_other_style_users() {
    let mut doc = valid_font_fixture();
    let before = doc.clone();
    assert!(update_style(&mut doc, &update_request()).unwrap());
    let style = &doc.header.styles[0];
    assert_eq!(
        usize::from(style.char_shape.0),
        before.header.char_shapes.len()
    );
    assert_eq!(
        usize::from(style.para_shape.0),
        before.header.para_shapes.len()
    );
    assert_eq!(
        doc.header.char_shapes.len(),
        before.header.char_shapes.len() + 1
    );
    assert_eq!(
        doc.header.para_shapes.len(),
        before.header.para_shapes.len() + 1
    );
    assert_eq!(
        &doc.header.char_shapes[..before.header.char_shapes.len()],
        before.header.char_shapes.as_slice()
    );
    assert_eq!(
        &doc.header.para_shapes[..before.header.para_shapes.len()],
        before.header.para_shapes.as_slice()
    );
    assert_eq!(&doc.header.styles[1..], &before.header.styles[1..]);
    let p = &doc.sections[0].paragraphs;
    assert_eq!(p[0].char_shape_runs, vec![(0, style.char_shape)]);
    assert_eq!(p[1].char_shape_runs, vec![(0, style.char_shape)]);
    assert_eq!(p[1].para_shape, before.sections[0].paragraphs[1].para_shape);
    assert_eq!(
        p[2].char_shape_runs,
        vec![
            (0, style.char_shape),
            (2, CharShapeId(1)),
            (4, style.char_shape)
        ]
    );
    assert_eq!(p[3].char_shape_runs, vec![(0, CharShapeId(2))]);
    for index in [0, 2, 3] {
        assert_eq!(p[index].para_shape, style.para_shape);
    }
    for (index, para) in p.iter().enumerate().take(4) {
        assert!(para.line_segs.is_empty());
        assert_eq!(para.chars, before.sections[0].paragraphs[index].chars);
        assert_eq!(para.controls, before.sections[0].paragraphs[index].controls);
    }
    assert_eq!(&p[4..], &before.sections[0].paragraphs[4..]);
    let mut expected_style = before.header.styles[0].clone();
    expected_style.char_shape = style.char_shape;
    expected_style.para_shape = style.para_shape;
    assert_eq!(
        style, &expected_style,
        "only the changed shape references may alter style metadata"
    );
}

#[test]
fn update_allocates_fresh_colliding_shapes_and_preserves_direct_status_on_repeat() {
    let mut doc = valid_font_fixture();
    let before = doc.clone();
    let first = UpdateStyle {
        character: CharFormat {
            size_pt: Some(14.0),
            bold: Some(true),
            ..CharFormat::default()
        },
        paragraph: ParaProps {
            align: Some(3),
            ..ParaProps::default()
        },
        ..update_request()
    };
    assert!(update_style(&mut doc, &first).unwrap());
    let (cs, ps) = shapes(&doc, StyleId(0));
    assert_eq!(
        doc.header.char_shapes[usize::from(cs.0)],
        before.header.char_shapes[2]
    );
    assert_eq!(
        doc.header.para_shapes[usize::from(ps.0)],
        before.header.para_shapes[1]
    );
    assert_ne!(
        cs,
        CharShapeId(2),
        "equal values must get a fresh CS identity"
    );
    assert_ne!(
        ps,
        ParaShapeId(1),
        "equal values must get a fresh PS identity"
    );
    for para in &mut doc.sections[0].paragraphs {
        para.line_segs = cache();
    }
    let repeat_before = doc.clone();
    assert!(!update_style(&mut doc, &first).unwrap());
    assert_eq!(
        doc, repeat_before,
        "unchanged request cannot allocate or invalidate caches"
    );
    let second = UpdateStyle {
        character: CharFormat {
            size_pt: Some(15.0),
            ..CharFormat::default()
        },
        ..update_request()
    };
    assert!(update_style(&mut doc, &second).unwrap());
    assert_eq!(
        doc.sections[0].paragraphs[3].char_shape_runs,
        before.sections[0].paragraphs[3].char_shape_runs
    );
    assert_eq!(
        doc.sections[0].paragraphs[1].para_shape,
        before.sections[0].paragraphs[1].para_shape
    );
    let inherited = doc.sections[0].paragraphs[1].char_shape_runs[0].1;
    assert_eq!(
        doc.header.char_shapes[usize::from(inherited.0)].base_size,
        1500
    );
    assert_eq!(
        &doc.sections[0].paragraphs[4..],
        &before.sections[0].paragraphs[4..]
    );
    assert_eq!(
        &doc.header.char_shapes[..before.header.char_shapes.len()],
        before.header.char_shapes.as_slice()
    );
}

#[test]
fn update_unchanged_or_empty_properties_never_allocate_or_clear_any_cache() {
    for operation in [
        UpdateStyle {
            character: CharFormat::default(),
            paragraph: ParaProps::default(),
            ..update_request()
        },
        UpdateStyle {
            character: CharFormat {
                size_pt: Some(10.0),
                ..CharFormat::default()
            },
            paragraph: ParaProps {
                align: Some(0),
                ..ParaProps::default()
            },
            ..update_request()
        },
    ] {
        let mut doc = valid_font_fixture();
        let before = doc.clone();
        assert!(!update_style(&mut doc, &operation).unwrap());
        assert_eq!(doc, before);
    }
    let mut doc = valid_font_fixture();
    let before = doc.clone();
    let only_character = UpdateStyle {
        paragraph: ParaProps::default(),
        ..update_request()
    };
    assert!(update_style(&mut doc, &only_character).unwrap());
    assert_eq!(doc.header.para_shapes, before.header.para_shapes);
    assert_eq!(
        doc.header.styles[0].para_shape,
        before.header.styles[0].para_shape
    );
    let only_paragraph = UpdateStyle {
        character: CharFormat::default(),
        ..update_request()
    };
    let char_shapes = doc.header.char_shapes.clone();
    assert!(update_style(&mut doc, &only_paragraph).unwrap());
    assert_eq!(doc.header.char_shapes, char_shapes);
}

fn caption(paragraphs: Vec<Paragraph>) -> hwp_model::Caption {
    hwp_model::Caption {
        side: hwp_model::CaptionSide::Bottom,
        direction: hwp_model::CaptionDirection::Horizontal,
        gap: 23,
        width: None,
        last_width: 1000,
        paragraphs,
    }
}

fn caption_table(cell: Paragraph, cap: Paragraph) -> hwp_model::Table {
    let source = hwp_convert::from_markdown("| cell |\n|---|\n| value |\n");
    let mut table = source
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .find_map(|control| {
            if let Control::Table(table) = control {
                Some(table.clone())
            } else {
                None
            }
        })
        .unwrap();
    table.rows = 1;
    table.cols = 1;
    table.row_cell_counts = vec![1];
    table.cells.truncate(1);
    table.cells[0].paragraphs = vec![cell];
    table.caption = Some(caption(vec![cap]));
    table
}

fn caption_picture(cap: Paragraph) -> hwp_model::Picture {
    // Only caption traversal is exercised; no image file, decoder or writer is involved.
    serde_json::from_value(serde_json::json!({
        "common_data":"", "width":1000, "height":1000, "treat_as_char":true,
        "bin_ref":{"ItemRef":"synthetic-caption-only"}, "extras":[], "caption":caption(vec![cap])
    }))
    .unwrap()
}

fn visit_test_paragraphs<'a>(paras: &'a [Paragraph], out: &mut Vec<&'a Paragraph>) {
    for para in paras {
        out.push(para);
        for control in &para.controls {
            match control {
                Control::Table(table) => {
                    if let Some(cap) = &table.caption {
                        visit_test_paragraphs(&cap.paragraphs, out);
                    }
                    for cell in &table.cells {
                        visit_test_paragraphs(&cell.paragraphs, out);
                    }
                }
                Control::Picture(picture) => {
                    if let Some(cap) = &picture.caption {
                        visit_test_paragraphs(&cap.paragraphs, out);
                    }
                }
                Control::Generic(generic) => {
                    for list in &generic.paragraph_lists {
                        visit_test_paragraphs(&list.paragraphs, out);
                    }
                    if let Some(cap) = &generic.caption {
                        visit_test_paragraphs(&cap.paragraphs, out);
                    }
                }
                Control::SectionDef(_) => {}
            }
        }
    }
}

#[test]
fn update_visits_cells_and_all_caption_kinds_and_invalidates_changed_ancestors() {
    let mut doc = valid_font_fixture();
    let body = |text| para(text, 0, 0, &[(0, 0)]);
    let mut nested = para("nested note ancestor", 1, 0, &[(0, 0)]);
    nested.controls.push(Control::Table(caption_table(
        body("deep cell"),
        body("deep table caption"),
    )));
    let mut object = generic(vec![body("generic list"), nested]);
    object.caption = Some(caption(vec![body("generic caption")]));
    let untouched_object = generic(vec![para("unrelated nested note", 1, 0, &[(0, 0)])]);
    let mut ancestor = para("outer note ancestor", 1, 0, &[(0, 0)]);
    ancestor.controls = vec![
        Control::Table(caption_table(body("table cell"), body("table caption"))),
        Control::Picture(caption_picture(body("picture caption"))),
        Control::Generic(object),
        Control::Generic(untouched_object.clone()),
    ];
    doc.sections[0].paragraphs.push(ancestor);
    let before = doc.clone();
    assert!(update_style(&mut doc, &update_request()).unwrap());
    let style = &doc.header.styles[0];
    let mut paragraphs = Vec::new();
    visit_test_paragraphs(&doc.sections[0].paragraphs, &mut paragraphs);
    for text in [
        "table cell",
        "table caption",
        "picture caption",
        "generic list",
        "generic caption",
        "deep cell",
        "deep table caption",
    ] {
        let target = paragraphs.iter().find(|p| p.plain_text() == text).unwrap();
        assert_eq!(
            target.para_shape, style.para_shape,
            "nested PS missed: {text}"
        );
        assert_eq!(
            target.char_shape_runs,
            vec![(0, style.char_shape)],
            "nested CS missed: {text}"
        );
        assert!(target.line_segs.is_empty(), "nested cache missed: {text}");
    }
    for text in ["outer note ancestor", "nested note ancestor"] {
        let target = paragraphs.iter().find(|p| p.plain_text() == text).unwrap();
        assert_eq!(target.style, StyleId(1));
        assert_eq!(target.para_shape, ParaShapeId(0));
        assert_eq!(target.char_shape_runs, vec![(0, CharShapeId(0))]);
        assert!(
            target.line_segs.is_empty(),
            "changed descendant must invalidate ancestor layout: {text}"
        );
    }
    let outer = doc.sections[0].paragraphs.last().unwrap();
    let Control::Generic(generic) = &outer.controls[2] else {
        panic!("fixture generic");
    };
    assert!(generic.hwpx_raw_xml.is_none());
    assert_eq!(outer.controls[3], Control::Generic(untouched_object));
    assert_eq!(
        &doc.sections[0].paragraphs[4..6],
        &before.sections[0].paragraphs[4..6]
    );
}

#[test]
fn update_font_addition_requires_valid_existing_references_and_is_atomic() {
    let mut doc = valid_font_fixture();
    let before = doc.clone();
    let operation = UpdateStyle {
        character: CharFormat {
            font: Some("Replacement face".into()),
            ..CharFormat::default()
        },
        paragraph: ParaProps::default(),
        ..update_request()
    };
    assert!(update_style(&mut doc, &operation).unwrap());
    let style = &doc.header.styles[0];
    let shape = &doc.header.char_shapes[usize::from(style.char_shape.0)];
    for (slot, id) in shape.face_ids.iter().enumerate() {
        assert_eq!(
            doc.header.fonts[slot][usize::from(*id)].name,
            "Replacement face"
        );
    }
    assert_eq!(
        &doc.header.char_shapes[..before.header.char_shapes.len()],
        before.header.char_shapes.as_slice()
    );
    assert_eq!(
        &doc.sections[0].paragraphs[4..],
        &before.sections[0].paragraphs[4..]
    );
    let repeated = doc.clone();
    assert!(!update_style(&mut doc, &operation).unwrap());
    assert_eq!(doc, repeated);
    let mut broken = valid_font_fixture();
    // Appending face 1 would silently bind this unrelated, previously unresolved reference.
    broken.header.char_shapes[2].face_ids[6] = 1;
    let before = broken.clone();
    assert!(update_style(&mut broken, &operation).is_err());
    assert_eq!(
        broken, before,
        "font append cannot rebind an unrelated dangling reference"
    );
}

#[test]
fn update_refuses_invalid_selectors_kinds_and_base_shapes_without_partial_changes() {
    for style in [
        StyleSelector::Name("missing".into()),
        StyleSelector::Id(999),
    ] {
        let mut doc = valid_font_fixture();
        let before = doc.clone();
        assert!(
            update_style(
                &mut doc,
                &UpdateStyle {
                    style,
                    ..update_request()
                }
            )
            .is_err()
        );
        assert_eq!(doc, before);
    }
    let mut ambiguous = valid_font_fixture();
    ambiguous.header.styles[1].name = "Body".into();
    let before = ambiguous.clone();
    assert!(update_style(&mut ambiguous, &update_request()).is_err());
    assert_eq!(ambiguous, before);
    for kind in [1, 2, 7] {
        let mut doc = valid_font_fixture();
        doc.header.styles[0].attr = kind;
        let before = doc.clone();
        assert!(update_style(&mut doc, &update_request()).is_err());
        assert_eq!(doc, before);
    }
    let mut unknown = valid_font_fixture();
    unknown.header.styles[0].kind_unknown = true;
    let before = unknown.clone();
    assert!(update_style(&mut unknown, &update_request()).is_err());
    assert_eq!(unknown, before);
    for table in ["char", "para"] {
        let mut doc = valid_font_fixture();
        if table == "char" {
            doc.header.styles[0].char_shape = CharShapeId(999);
        } else {
            doc.header.styles[0].para_shape = ParaShapeId(999);
        }
        let before = doc.clone();
        assert!(update_style(&mut doc, &update_request()).is_err());
        assert_eq!(doc, before);
    }
    for size in [f32::NAN, f32::INFINITY] {
        let mut doc = valid_font_fixture();
        let before = doc.clone();
        let operation = UpdateStyle {
            character: CharFormat {
                size_pt: Some(size),
                ..CharFormat::default()
            },
            ..update_request()
        };
        assert!(update_style(&mut doc, &operation).is_err());
        assert_eq!(doc, before);
    }
}

#[test]
fn update_fresh_shape_and_font_capacity_failures_preserve_the_entire_document() {
    let full = usize::from(u16::MAX) + 1;
    for table in ["char", "para", "font", "style"] {
        let mut doc = valid_font_fixture();
        let mut operation = update_request();
        match table {
            "char" => {
                let base = doc.header.char_shapes[0].clone();
                doc.header.char_shapes.resize(full, base);
                doc.header.char_shapes[full - 1].base_size = 1300;
                operation.paragraph = ParaProps::default();
            }
            "para" => {
                let base = doc.header.para_shapes[0].clone();
                doc.header.para_shapes.resize(full, base);
                doc.header.para_shapes[full - 1].attr1 = 2 << 2;
                operation.character = CharFormat::default();
            }
            "font" => {
                doc.header.fonts[6].resize(
                    full,
                    FaceName {
                        name: "Fixture Serif".into(),
                        ..FaceName::default()
                    },
                );
                operation.character.font = Some("Replacement face".into());
            }
            "style" => doc.header.styles.resize(full + 1, Style::default()),
            _ => unreachable!(),
        }
        let before = doc.clone();
        assert!(
            update_style(&mut doc, &operation).is_err(),
            "fresh identity must not wrap or deduplicate on capacity failure: {table}"
        );
        assert!(doc == before, "capacity failure must be atomic: {table}");
    }
}

#[test]
fn update_refuses_affected_raw_backed_users_without_clearing_the_raw_subtree() {
    for where_user in ["list", "caption", "nested-list"] {
        let mut doc = valid_font_fixture();
        let user = para("raw-backed body user", 0, 0, &[(0, 0)]);
        let mut raw = generic(vec![]);
        raw.raw_children = vec![hwp_model::OpaqueRecord {
            tag: 0x7777,
            data: vec![71, 72],
            children: vec![],
        }];
        match where_user {
            "list" => raw.paragraph_lists[0].paragraphs.push(user),
            "caption" => raw.caption = Some(caption(vec![user])),
            "nested-list" => {
                let mut container = para("raw nested note", 1, 0, &[(0, 0)]);
                container
                    .controls
                    .push(Control::Generic(generic(vec![user])));
                raw.paragraph_lists[0].paragraphs.push(container);
            }
            _ => unreachable!(),
        }
        let mut ancestor = para("raw note ancestor", 1, 0, &[(0, 0)]);
        ancestor.controls.push(Control::Generic(raw));
        doc.sections[0].paragraphs.push(ancestor);
        let before = doc.clone();
        let noop = UpdateStyle {
            character: CharFormat::default(),
            paragraph: ParaProps::default(),
            ..update_request()
        };
        assert!(
            !update_style(&mut doc, &noop).unwrap(),
            "unchanged definitions do not affect raw-backed users: {where_user}"
        );
        assert!(
            doc == before,
            "raw-backed no-op changed the document: {where_user}"
        );
        assert!(
            update_style(&mut doc, &update_request()).is_err(),
            "raw-backed inherited user must refuse: {where_user}"
        );
        assert_eq!(
            doc, before,
            "preflight must refuse before editing earlier users or allocating shapes: {where_user}"
        );
    }
    let mut doc = valid_font_fixture();
    let mut raw = generic(vec![para("raw unrelated note", 1, 0, &[(0, 0)])]);
    raw.raw_children = vec![hwp_model::OpaqueRecord {
        tag: 0x7777,
        data: vec![71, 72],
        children: vec![],
    }];
    let mut ancestor = para("raw note ancestor", 1, 0, &[(0, 0)]);
    ancestor.controls.push(Control::Generic(raw.clone()));
    doc.sections[0].paragraphs.push(ancestor);
    assert!(update_style(&mut doc, &update_request()).unwrap());
    assert_eq!(
        doc.sections[0].paragraphs.last().unwrap().controls[0],
        Control::Generic(raw),
        "an unrelated raw-backed user is preserved rather than cleared"
    );
}

#[test]
fn raw_style_refusal_reports_every_model_route_and_a_resolvable_ancestor_without_text() {
    let mut doc = valid_font_fixture();
    let private_text = [
        "NEVER_DISCLOSE_LIST",
        "NEVER_DISCLOSE_CAPTION",
        "NEVER_DISCLOSE_NESTED",
        "NEVER_DISCLOSE_ANCESTOR",
    ];
    let mut raw = generic(vec![para(private_text[0], 0, 0, &[(0, 0)])]);
    raw.caption = Some(caption(vec![para(private_text[1], 0, 0, &[(0, 0)])]));
    let mut nested = para("NEVER_DISCLOSE_NESTED_CONTAINER", 1, 0, &[(0, 0)]);
    nested.controls.push(Control::Generic(generic(vec![para(
        private_text[2],
        0,
        0,
        &[(0, 0)],
    )])));
    raw.paragraph_lists.push(ParagraphList {
        header_data: vec![],
        paragraphs: vec![nested],
    });
    raw.raw_children = vec![hwp_model::OpaqueRecord {
        tag: 0x7777,
        data: vec![71, 72],
        children: vec![],
    }];
    let mut ancestor = para(private_text[3], 1, 0, &[(0, 0)]);
    ancestor.controls.push(Control::Generic(raw));
    let index = doc.sections[0].paragraphs.len();
    let ancestor_path = path(&[index]);
    let ancestor_id = hwp_convert::paragraph_id(&ancestor_path, &ancestor);
    doc.sections[0].paragraphs.push(ancestor);
    let before = doc.clone();
    let error = update_style(&mut doc, &update_request())
        .expect_err("all raw-backed affected users must refuse atomically");
    let message = error.to_string();
    assert!(
        message.contains("문단 3개"),
        "refusal must report all 3 affected typed users, not stop after the first: {message}"
    );
    let prefix = format!("sections[0].paragraphs[{index}].controls[0]");
    for route in [
        format!("{prefix}.paragraph_lists[0].paragraphs[0]"),
        format!("{prefix}.caption.paragraphs[0]"),
        format!(
            "{prefix}.paragraph_lists[1].paragraphs[0].controls[0].paragraph_lists[0].paragraphs[0]"
        ),
    ] {
        assert!(
            message.contains(&route),
            "refusal omitted the human model route {route}: {message}"
        );
    }
    assert!(
        message.contains(&format!("상위 문단 주소: {ancestor_id}")),
        "refusal must label the nearest addressable ancestor ID rather than invent an editable raw/caption ID: {message}"
    );
    let checksum = ancestor_id.split('.').next().unwrap().to_owned();
    let resolved = hwp_convert::resolve(
        &doc,
        &hwp_convert::Address {
            section: 0,
            indices: vec![index],
            checksum: Some(checksum),
            chars: None,
        },
        hwp_convert::Granularity::Paragraph,
    )
    .expect("the diagnostic's ancestor ID must actually resolve");
    assert_eq!(resolved.before_id, ancestor_id);
    for text in private_text
        .into_iter()
        .chain(["NEVER_DISCLOSE_NESTED_CONTAINER"])
    {
        assert!(
            !message.contains(text),
            "diagnostics must not disclose document text"
        );
    }
    assert!(
        doc == before,
        "diagnostic collection cannot mutate users, caches, raw subtrees or header definitions"
    );
}

#[test]
fn raw_style_refusal_counts_all_users_and_bounds_sampled_locations() {
    let mut doc = valid_font_fixture();
    let mut raw = generic(
        (0..10)
            .map(|index| para(&format!("PRIVATE_RAW_USER_{index}"), 0, 0, &[(0, 0)]))
            .collect(),
    );
    raw.raw_children = vec![hwp_model::OpaqueRecord {
        tag: 0x7777,
        data: vec![71, 72],
        children: vec![],
    }];
    let mut ancestor = para("PRIVATE_RAW_ANCESTOR", 1, 0, &[(0, 0)]);
    ancestor.controls.push(Control::Generic(raw));
    let index = doc.sections[0].paragraphs.len();
    let ancestor_id = hwp_convert::paragraph_id(&path(&[index]), &ancestor);
    doc.sections[0].paragraphs.push(ancestor);
    let before = doc.clone();
    let message = update_style(&mut doc, &update_request())
        .expect_err("ten raw users must all be counted before refusal")
        .to_string();
    assert!(
        message.contains("문단 10개"),
        "raw diagnostic must count beyond its sample cap: {message}"
    );
    assert!(
        message.contains("위치 8/10개 표시"),
        "raw diagnostic must declare its bounded sample: {message}"
    );
    let route_prefix =
        format!("sections[0].paragraphs[{index}].controls[0].paragraph_lists[0].paragraphs[");
    assert_eq!(
        message.matches(&route_prefix).count(),
        8,
        "at most eight sampled user routes may be retained in this error"
    );
    assert!(message.contains(&format!("상위 문단 주소: {ancestor_id}")));
    assert!(!message.contains("PRIVATE_RAW_USER_") && !message.contains("PRIVATE_RAW_ANCESTOR"));
    assert!(
        doc == before,
        "bounded diagnostic collection must remain atomic"
    );
}

#[test]
fn update_refuses_captured_container_and_unsupported_xml_users_before_mutation() {
    for container in [true, false] {
        let mut doc = valid_font_fixture();
        let mut object = generic(vec![para("PRIVATE_CAPTURED_STYLE_USER", 0, 0, &[(0, 0)])]);
        object.ctrl_id = if container { *b"cont" } else { *b"zzzz" };
        object.hwpx_raw_xml = Some("<captured-object/>".into());
        if container {
            object.gso_shapes = vec![typed_shape_fixture()];
            object.container_box = Some(hwp_model::ContainerBox {
                x: 0,
                y: 0,
                w: 2000,
                h: 1000,
                anchored: false,
                skipped_objects: 0,
                text_boxes: vec![None],
            });
        }
        let mut ancestor = para("PRIVATE_CAPTURED_ANCESTOR", 1, 0, &[(0, 0)]);
        ancestor.controls.push(Control::Generic(object));
        let index = doc.sections[0].paragraphs.len();
        let ancestor_id = hwp_convert::paragraph_id(&path(&[index]), &ancestor);
        doc.sections[0].paragraphs.push(ancestor);
        let before = doc.clone();
        let message = update_style(&mut doc, &update_request())
            .expect_err(
                "captured XML without a resynthesis path must refuse before changing the style",
            )
            .to_string();
        assert!(
            message.contains("문단 1개"),
            "captured XML user count missing: {message}"
        );
        assert!(
            message.contains(&format!(
                "sections[0].paragraphs[{index}].controls[0].paragraph_lists[0].paragraphs[0]"
            )),
            "captured XML user's model location missing: {message}"
        );
        assert!(message.contains(&ancestor_id));
        assert!(
            message.contains("거부")
                && (message.contains("원본 레코드") || message.contains("XML"))
                && message.contains("재생성"),
            "refusal must name the original-record/XML resynthesis boundary: {message}"
        );
        assert!(
            message.contains("hwp styles") && message.contains("다른 스타일"),
            "refusal needs an actionable style remedy: {message}"
        );
        assert!(
            !message.contains("PRIVATE_CAPTURED_STYLE_USER")
                && !message.contains("PRIVATE_CAPTURED_ANCESTOR")
        );
        assert!(
            doc == before,
            "captured XML refusal cannot change headers, users, caches or original XML"
        );
    }
}

#[test]
fn update_keeps_supported_generic_typed_mutation_paths_available() {
    for ctrl_id in [*b"gso ", *b"head", *b"foot", *b"fn  ", *b"en  "] {
        let mut doc = valid_font_fixture();
        let mut object = generic(vec![para("supported modeled user", 0, 0, &[(0, 0)])]);
        object.ctrl_id = ctrl_id;
        let mut ancestor = para("supported note ancestor", 1, 0, &[(0, 0)]);
        ancestor.controls.push(Control::Generic(object));
        doc.sections[0].paragraphs.push(ancestor);
        assert!(
            update_style(&mut doc, &update_request())
                .expect("known Generic arms retain their typed mutation path")
        );
        let Control::Generic(object) = &doc.sections[0].paragraphs.last().unwrap().controls[0]
        else {
            panic!("fixture generic");
        };
        assert!(object.hwpx_raw_xml.is_none());
        assert_eq!(
            object.paragraph_lists[0].paragraphs[0].char_shape_runs,
            vec![(0, doc.header.styles[0].char_shape)]
        );
    }
}

fn assert_dangling_style_diagnostic(message: &str, owner: &str, table: &str, id: u16) {
    assert!(
        message.contains(owner),
        "dangling reference diagnostic must identify its real owner {owner}: {message}"
    );
    assert!(
        message.contains(table) && message.contains(&format!("참조 {id}")),
        "dangling reference table/index missing: {message}"
    );
    assert!(
        message.contains("추가") && message.contains("의미"),
        "diagnostic must explain that appending definitions can assign meaning to a dangling reference: {message}"
    );
    assert!(
        message.contains("원본 문서") && message.contains("복구") && message.contains("다시"),
        "diagnostic must give a concrete repair/retry step: {message}"
    );
    assert!(!message.contains("PRIVATE_DANGLING_OWNER"));
}

#[test]
fn update_dangling_paragraph_shape_diagnostics_identify_owner_and_append_alias_cause() {
    for character in [false, true] {
        let mut doc = valid_font_fixture();
        doc.sections[0].paragraphs[4].chars = "PRIVATE_DANGLING_OWNER"
            .chars()
            .map(HwpChar::Text)
            .collect();
        let (owner, table, id) = if character {
            let id = u16::try_from(doc.header.char_shapes.len()).unwrap();
            doc.sections[0].paragraphs[4].char_shape_runs = vec![(0, CharShapeId(id))];
            (
                "sections[0].paragraphs[4].char_shape_runs[0]",
                "char_shapes",
                id,
            )
        } else {
            let id = u16::try_from(doc.header.para_shapes.len()).unwrap();
            doc.sections[0].paragraphs[4].para_shape = ParaShapeId(id);
            ("sections[0].paragraphs[4].para_shape", "para_shapes", id)
        };
        let before = doc.clone();
        let message = update_style(&mut doc, &update_request())
            .expect_err("new shapes cannot bind a dangling paragraph reference")
            .to_string();
        assert_dangling_style_diagnostic(&message, owner, table, id);
        assert!(
            doc == before,
            "owner diagnostic preflight must remain atomic"
        );
    }
}

#[test]
fn update_dangling_font_diagnostic_identifies_shape_slot_and_append_alias_cause() {
    let mut doc = valid_font_fixture();
    doc.header.char_shapes[2].face_ids[6] = 1;
    let before = doc.clone();
    let operation = UpdateStyle {
        character: CharFormat {
            font: Some("Replacement face".into()),
            ..CharFormat::default()
        },
        ..update_request()
    };
    let message = update_style(&mut doc, &operation)
        .expect_err("font append cannot bind an unrelated dangling reference")
        .to_string();
    assert_dangling_style_diagnostic(&message, "header.char_shapes[2].face_ids[6]", "fonts[6]", 1);
    assert!(doc == before);
}

#[test]
fn update_dangling_unrelated_style_diagnostic_identifies_definition_owner() {
    for character in [false, true] {
        let mut doc = valid_font_fixture();
        let (owner, table, id) = if character {
            let id = u16::try_from(doc.header.char_shapes.len()).unwrap();
            doc.header.styles[1].char_shape = CharShapeId(id);
            ("header.styles[1].char_shape", "char_shapes", id)
        } else {
            let id = u16::try_from(doc.header.para_shapes.len()).unwrap();
            doc.header.styles[1].para_shape = ParaShapeId(id);
            ("header.styles[1].para_shape", "para_shapes", id)
        };
        let before = doc.clone();
        let message = update_style(&mut doc, &update_request())
            .expect_err("unrelated style references cannot acquire new shape meanings")
            .to_string();
        assert_dangling_style_diagnostic(&message, owner, table, id);
        assert!(doc == before);
    }
}

fn typed_shape_fixture() -> hwp_model::control::ShapeGeom {
    serde_json::from_value(serde_json::json!({
        "kind":"Rect", "x":0, "y":0, "w":2000, "h":1000,
        "fill":4294967295u32, "border_color":4294967295u32, "border_width":0
    }))
    .unwrap()
}

#[test]
fn update_typed_shape_with_captured_xml_keeps_modeled_mutation_path() {
    let mut doc = valid_font_fixture();
    let mut object = generic(vec![para("supported typed shape user", 0, 0, &[(0, 0)])]);
    object.ctrl_id = *b"zzzz";
    object.gso_shapes = vec![typed_shape_fixture()];
    let original_geometry = object.gso_shapes.clone();
    let mut ancestor = para("typed shape note ancestor", 1, 0, &[(0, 0)]);
    ancestor.controls.push(Control::Generic(object));
    doc.sections[0].paragraphs.push(ancestor);
    assert!(
        update_style(&mut doc, &update_request())
            .expect("typed shapes without a container box retain their modeled mutation path")
    );
    let Control::Generic(object) = &doc.sections[0].paragraphs.last().unwrap().controls[0] else {
        panic!("fixture generic");
    };
    assert!(object.hwpx_raw_xml.is_none());
    assert_eq!(object.gso_shapes, original_geometry);
    assert_eq!(
        object.paragraph_lists[0].paragraphs[0].char_shape_runs,
        vec![(0, doc.header.styles[0].char_shape)]
    );
}
