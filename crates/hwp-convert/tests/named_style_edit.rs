//! Core named-style contracts without native writers, private fixtures or host fonts.
use hwp_convert::format::{CharFormat, ParaProps};
use hwp_convert::segment_id::SegmentPath;
use hwp_convert::styles::StyleKind;
use hwp_convert::styles::edit::{
    CreateStyle, StyleEditError, StyleSelector, apply_style_at, create_style,
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
    for kind in [1, 2, 7] {
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
fn utf16_name_boundary_and_character_kind_keep_base_metadata() {
    let mut doc = fixture();
    let mut operation = request("temporary");
    operation.name = format!("{}x", "😀".repeat(32767));
    operation.kind = StyleKind::Character;
    let id = create_style(&mut doc, &operation).unwrap();
    let style = &doc.header.styles[usize::from(id.0)];
    assert_eq!(style.name.encode_utf16().count(), usize::from(u16::MAX));
    assert_eq!(style.attr, 0xa1);
    assert_eq!((style.next_style, style.lang_id), (1, 1042));
    let before = doc.clone();
    assert!(matches!(
        apply_style_at(&mut doc, &path(&[0]), &selector(id), false),
        Err(StyleEditError::UnsupportedKind(1))
    ));
    assert_eq!(doc, before);
}
