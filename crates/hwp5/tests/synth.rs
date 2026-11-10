//! Verify synthesized documents (from md/hwpx, without tails) follow Hangul 5.1.0.1.
//!
//! Regressions for the five defects that caused tamper/security warnings only for synthesized
//! documents in native Hangul checks: version/layout agreement (PARA_SHAPE 58B/PARA_HEADER 24B),
//! nonempty TAB_DEF/NUMBERING (no dangling references), and required secd children.

use std::path::PathBuf;

#[path = "../../hwp-cli/tests/common/temp_path.rs"]
mod temp_path;

fn tmp(name: &str) -> temp_path::TempPath {
    temp_path::TempPath::new("hwp5-synth-tests", name)
}

/// New HWP synthesis must be byte-identical regardless of path or execution time; every CFB
/// directory entry creation/modification FILETIME must equal the canonical CFB epoch.
#[test]
fn 신규_패키지는_시간과_경로에_무관하게_결정적() {
    let doc = hwp_convert::from_markdown("결정적 HWP");
    let root = tmp(&format!("deterministic-{}", std::process::id()));
    let first_dir = root.join("first");
    let second_dir = root.join("second");
    std::fs::create_dir_all(&first_dir).unwrap();
    std::fs::create_dir_all(&second_dir).unwrap();
    let first = first_dir.join("a.hwp");
    let second = second_dir.join("b.hwp");

    hwp5::write_document(&doc, &first, &hwp5::WriteOptions::default()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2_100));
    hwp5::write_document(&doc, &second, &hwp5::WriteOptions::default()).unwrap();

    assert_eq!(
        std::fs::read(&first).unwrap(),
        std::fs::read(&second).unwrap(),
        "동일 문서는 경로와 wall clock에 무관하게 byte-identical해야 함"
    );

    const SECONDS_FROM_CFB_TO_UNIX_EPOCH: u64 = 11_644_473_600;
    let cfb_epoch = std::time::UNIX_EPOCH
        .checked_sub(std::time::Duration::from_secs(
            SECONDS_FROM_CFB_TO_UNIX_EPOCH,
        ))
        .unwrap();
    for path in [&first, &second] {
        let file = std::fs::File::open(path).unwrap();
        let cfb = cfb::CompoundFile::open(file).unwrap();
        for entry in cfb.walk() {
            assert_eq!(
                entry.created(),
                cfb_epoch,
                "{} creation FILETIME",
                entry.path().display()
            );
            assert_eq!(
                entry.modified(),
                cfb_epoch,
                "{} modification FILETIME",
                entry.path().display()
            );
        }
    }

    std::fs::remove_file(first).unwrap();
    std::fs::remove_file(second).unwrap();
}

/// Markdown-to-HWP synthesis must satisfy every condition checked by Hangul integrity validation.
#[test]
fn 합성_문서_한글_규격_충족() {
    let doc = hwp_convert::from_markdown(
        "# 제목\n\n본문 문단입니다.\n\n| A | B |\n| - | - |\n| 1 | 2 |\n",
    );
    let out = tmp("synth.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let reread = hwp5::read_document(&out).unwrap();
    let d = reread.document;

    // 1. TAB_DEF/NUMBERING are nonempty (targets of PARA_SHAPE tab_def_id/numbering_id).
    assert!(!d.header.tab_defs.is_empty(), "TAB_DEF dangling reference");
    assert!(
        !d.header.numberings.is_empty(),
        "NUMBERING dangling reference"
    );

    // 1a. char_shape shade_color != 0: zero draws opaque black shading in every character cell,
    //     producing black bars (native check 14). Genuine files use 0xFFFFFFFF (none).
    for cs in &d.header.char_shapes {
        assert_ne!(cs.shade_color, 0, "char_shape 음영색 0 = 검은 바");
    }

    // 1b. COMPATIBLE_DOCUMENT (0x1E) is required for 5.1.x (genuine 가나다/hello_world samples).
    let mut c0 = hwp5::Hwp5Container::open(&out).unwrap();
    let di0 = c0.read_record_stream("/DocInfo").unwrap();
    let scan = hwp5::record::scan_stream(&di0, hwp5::record::ScanMode::Tolerant).unwrap();
    let compat = scan
        .roots
        .iter()
        .find(|r| r.tag == 0x1E)
        .expect("COMPATIBLE_DOCUMENT");
    let child_tags: Vec<u16> = compat.children.iter().map(|c| c.tag).collect();
    assert!(child_tags.contains(&0x1F), "LAYOUT_COMPATIBILITY 자식");
    assert!(child_tags.contains(&0x20), "TRACKCHANGE 자식");

    // 2. Required secd children: footnote/endnote shapes and all three page border definitions.
    let secd = d.sections[0].section_def().expect("구역 정의");
    let footnotes = secd.extras.iter().filter(|e| e.tag == 0x4A).count();
    let page_borders = secd.extras.iter().filter(|e| e.tag == 0x4B).count();
    assert_eq!(footnotes, 2, "secd 각주/미주 모양");
    assert_eq!(page_borders, 3, "secd 쪽 테두리 3종");
    assert!(secd.page.is_some(), "PAGE_DEF");

    // 3. EncryptVersion=4 (modern Hangul marker).
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    assert!(c.file_header().is_compressed());

    // 4. Record lengths follow 5.1.0.1 (measured directly after decompression).
    let di = c.read_record_stream("/DocInfo").unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();
    assert!(
        record_sizes(&di, 0x19).iter().all(|&s| s == 58),
        "PARA_SHAPE는 58B여야"
    );
    assert!(
        record_sizes(&di, 0x15).iter().all(|&s| s == 74),
        "CHAR_SHAPE는 74B여야"
    );
    assert!(
        record_sizes(&bt, 0x42).iter().all(|&s| s == 24),
        "PARA_HEADER는 24B여야"
    );
}

/// GI-1/GI-2 round trip (c): md (footnotes, ordered/nested lists) -> HWP5 -> read back.
/// Synthesized footnotes and numbered/bulleted paragraphs must survive without violating
/// record-size and reference constraints. Strikethrough is lost because HWP5 does not write
/// the flag, intentionally avoiding DIFFSPEC. The HWPX round trip preserves it.
#[test]
fn 왕복_각주_목록_hwp5_규격() {
    let md = "\
문단에 각주[^1]가 있다. ~~지운 글~~ 도 있다.

1. 첫째
2. 둘째
   - 안쪽 가
3. 셋째

[^1]: 각주 본문이다.
";
    let doc = hwp_convert::from_markdown(md);
    let out = tmp("synth_notes_list.hwp");
    let warnings = hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    // Footnotes and lists must be saved without DROP losses.
    assert!(
        !warnings.iter().any(|w| w.contains("DROP")),
        "DROP 경고: {warnings:?}"
    );

    let reread = hwp5::read_document(&out).unwrap().document;
    let d = &reread;

    // 1) The footnote control (fn) and its body paragraphs survive rereading.
    let has_fn = d.sections[0].paragraphs.iter().any(|p| {
        p.controls.iter().any(|c| matches!(c,
            hwp_model::Control::Generic(g) if g.ctrl_id == *b"fn  " && !g.paragraph_lists.is_empty()))
    });
    assert!(has_fn, "각주 컨트롤 왕복");

    // 2) Numbering/bullet paragraph shapes survive with valid references (defect A10).
    let has_number = d.header.para_shapes.iter().any(|ps| ps.head_type() == 2);
    let has_bullet = d.header.para_shapes.iter().any(|ps| ps.head_type() == 3);
    assert!(has_number, "번호 머리 문단모양 보존");
    assert!(has_bullet, "글머리 머리 문단모양 보존");
    for ps in &d.header.para_shapes {
        match ps.head_type() {
            2 => assert!(
                (ps.numbering_id as usize) < d.header.numberings.len(),
                "번호 정의 dangling: id={} < {}",
                ps.numbering_id,
                d.header.numberings.len()
            ),
            3 => assert!(
                (ps.numbering_id as usize) < d.header.bullets.len(),
                "글머리 정의 dangling: id={} < {}",
                ps.numbering_id,
                d.header.bullets.len()
            ),
            _ => {}
        }
    }

    // 3) Synthesized record sizes include new strike/list shapes and footnote bodies.
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let di = c.read_record_stream("/DocInfo").unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();
    assert!(
        record_sizes(&di, 0x19).iter().all(|&s| s == 58),
        "PARA_SHAPE 58B"
    );
    assert!(
        record_sizes(&di, 0x15).iter().all(|&s| s == 74),
        "CHAR_SHAPE 74B"
    );
    assert!(
        record_sizes(&bt, 0x42).iter().all(|&s| s == 24),
        "PARA_HEADER 24B (각주 본문 문단 포함)"
    );

    // 4) BULLET records match genuine field patterns (all business-plan samples): 25B, [8..12]
    //    has no character-shape ID (0xFFFFFFFF), [12..14] stores the bullet character in IR
    //    bullet_chars order (different by level). Wrong offsets hide markers (native H2 defect 1).
    let bullets = all_records(&di, 0x18);
    assert_eq!(
        bullets.len(),
        doc.header.bullet_chars.len(),
        "BULLET 레코드 수 == IR 글머리 정의 수"
    );
    for (b, want) in bullets.iter().zip(&doc.header.bullet_chars) {
        assert_eq!(b.len(), 25, "BULLET 레코드 25B(정품 실측)");
        assert_eq!(
            &b[8..12],
            &[0xFF, 0xFF, 0xFF, 0xFF],
            "번호 글자모양 id 없음"
        );
        assert_eq!(
            u16::from_le_bytes([b[12], b[13]]),
            *want as u16,
            "글머리표 문자 '{want}'가 오프셋 12에 있어야"
        );
    }

    // 5) Strikethrough: on-disk CharShape attribute bit18 must be set in at least one record.
    //    The attribute follows face_ids(14), ratios(7), spacings(7), relative sizes(7),
    //    offsets(7), and base_size(4). Check (attr >> 18) & 1 directly in the record bytes.
    //    The attribute offset is therefore 46.
    let strike_present = all_records(&di, 0x15).iter().any(|d| {
        d.len() >= 50 && (u32::from_le_bytes([d[46], d[47], d[48], d[49]]) >> 18) & 1 == 1
    });
    assert!(
        strike_present,
        "취소선 CHAR_SHAPE 속성 bit18(취소선 여부)이 기록돼야"
    );
}

/// Numbering/bullet numbering_id format-boundary +/-1 conversions must be exact inverses.
/// The IR is zero-based and HWP5 on disk is one-based: md -> HWP5 (write +1) ->
/// read back (read -1) must preserve numbering_id=0 without loss.
#[test]
fn hwp5_numbering_id_0기반_경계왕복() {
    let doc = hwp_convert::from_markdown("1. 하나\n2. 둘\n\n- 가\n- 나\n");
    // The md-derived IR uses zero-based numbering/bullet references.
    for ps in &doc.header.para_shapes {
        if matches!(ps.head_type(), 2 | 3) {
            assert_eq!(ps.numbering_id, 0, "md 출신 IR numbering_id는 0-기반");
        }
    }
    let out = tmp("numbering_base.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    // On-disk PARA_SHAPE numbering_id must be one-based (head 2/3 paragraphs use 1). Offset 30:
    // attr1(4) + i32*6(24) + tab_def_id(2). Check the writer restored +1 in the bytes.
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let di = c.read_record_stream("/DocInfo").unwrap();
    let raw_ids: Vec<u16> = all_records(&di, 0x19)
        .iter()
        .filter(|d| {
            d.len() >= 32 && ((u32::from_le_bytes([d[0], d[1], d[2], d[3]]) >> 23) & 3) != 0
        })
        .map(|d| u16::from_le_bytes([d[30], d[31]]))
        .collect();
    assert!(
        raw_ids.contains(&1),
        "머리 문단모양 on-disk numbering_id가 1-기반(=1)이어야: {raw_ids:?}"
    );

    // Reading normalizes back to zero-based IDs (read -1 inverts write +1).
    let reread = hwp5::read_document(&out).unwrap().document;
    for ps in &reread.header.para_shapes {
        if matches!(ps.head_type(), 2 | 3) {
            assert_eq!(ps.numbering_id, 0, "재읽기 numbering_id 0-기반 정규화");
        }
    }
}

/// GI-3/GI-4 round trip: md (image/inline code) -> HWP5 -> read preserves Picture/bin/code shapes.
#[test]
fn md_이미지_코드_hwp5_왕복() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("hwp5-md-imgcode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Minimal PNG (16x16 dimension header).
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(16u32.to_be_bytes());
    png.extend(16u32.to_be_bytes());
    png.extend([0u8; 8]);
    let fig = dir.join("f.png");
    std::fs::File::create(&fig)
        .unwrap()
        .write_all(&png)
        .unwrap();

    let mut doc = hwp_convert::from_markdown_with(
        "본문 `let x = 1;` 코드와 이미지.\n\n![alt](f.png)\n",
        &hwp_convert::MarkdownImportOptions {
            base_dir: Some(&dir),
            preset: None,
            ..Default::default()
        },
    );
    for paragraph in &mut doc.sections[0].paragraphs {
        for control in &mut paragraph.controls {
            if let hwp_model::Control::Picture(picture) = control {
                picture.description = Some("제목😀\n\n대체 설명".to_string());
            }
        }
    }
    let out = tmp("md_imgcode.hwp");
    let warnings = hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    assert!(
        !warnings.iter().any(|w| w.contains("DROP")),
        "DROP 경고: {warnings:?}"
    );
    let reread = hwp5::read_document(&out).unwrap().document;

    // The Picture and binary image data survive.
    let has_pic = reread.sections[0].paragraphs.iter().any(|p| {
        p.controls
            .iter()
            .any(|c| matches!(c, hwp_model::Control::Picture(_)))
    });
    assert!(has_pic, "이미지 Picture 왕복");
    assert!(!reread.bin_streams.is_empty(), "bin_streams 왕복");
    let description = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(picture) => picture.description.as_deref(),
            _ => None,
        });
    assert_eq!(description, Some("제목😀\n\n대체 설명"));

    // Inline code: HCR Dotum (face_id=1) character shape and a run referencing it.
    let code_ids: std::collections::HashSet<u16> = reread
        .header
        .char_shapes
        .iter()
        .enumerate()
        .filter(|(_, c)| c.face_ids[0] == 1)
        .map(|(i, _)| i as u16)
        .collect();
    assert!(!code_ids.is_empty(), "코드 글자모양(함초롬돋움) 왕복");
    let has_run = reread.sections[0].paragraphs.iter().any(|p| {
        p.char_shape_runs
            .iter()
            .any(|(_, id)| code_ids.contains(&id.0))
    });
    assert!(has_run, "코드 run 왕복");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Body tabs must round-trip as eight-WCHAR inline controls (code 9) through md -> HWP5.
/// Writing Text('\t') as one WCHAR makes Hangul interpret code 9 as an inline-control start
/// and consume the following seven WCHARs, corrupting the file (section 3.2.3, table 6).
#[test]
fn 본문_탭_hwp5_인라인컨트롤_왕복() {
    use hwp_model::HwpChar;
    let doc = hwp_convert::from_markdown("앞\t뒤\n");
    // IR invariant: tabs use InlineCtrl(9), with no Text('\t').
    let chars = &doc.sections[0].paragraphs[0].chars;
    assert!(
        chars
            .iter()
            .any(|c| matches!(c, HwpChar::InlineCtrl { code: 9, .. })),
        "탭이 InlineCtrl(9)로 적재돼야: {chars:?}"
    );
    assert!(
        !chars.iter().any(|c| matches!(c, HwpChar::Text('\t'))),
        "탭이 Text('\\t')로 남으면 안 됨"
    );

    let out = tmp("synth_tab.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    // PARA_TEXT must contain the 16-byte inline tab control (09 00 + 12*0 + 09 00).
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();
    let pt = first_record(&bt, 0x43).expect("PARA_TEXT");
    let tab16 = [9u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 0];
    assert!(
        pt.windows(16).any(|w| w == tab16),
        "PARA_TEXT에 탭 인라인 컨트롤 16B 없음"
    );

    // Round trip: InlineCtrl(9) and text order remain intact after reading.
    let reread = hwp5::read_document(&out).unwrap().document;
    let rc = &reread.sections[0].paragraphs[0].chars;
    assert!(
        rc.iter()
            .any(|c| matches!(c, HwpChar::InlineCtrl { code: 9, .. })),
        "왕복 후 InlineCtrl(9) 소실: {rc:?}"
    );
    assert!(reread.plain_text().contains("앞\t뒤"), "탭 텍스트 복원");
}

/// Synthesized body structure must match genuine Hangul paragraphs (가나다.hwp 5.1.1.0).
/// Regressions for the five body defects established by genuine-file comparison; together
/// they caused corruption warnings even with security settings lowered.
#[test]
fn 합성_문단_본문_구조_정품_동형() {
    let doc = hwp_convert::from_markdown("가나다\n");
    let out = tmp("synth_para.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();

    // 1. PARA_TEXT ends with paragraph terminator 0x0d=13 (all 188 genuine paragraphs).
    let pt = first_record(&bt, 0x43).expect("PARA_TEXT");
    let last = u16::from_le_bytes([pt[pt.len() - 2], pt[pt.len() - 1]]);
    assert_eq!(last, 13, "PARA_TEXT는 문단끝 0x0d로 종료해야");

    // 2. PARA_HEADER nchars has its top bit set (0x80000000, all genuine single paragraphs).
    let ph = first_record(&bt, 0x42).expect("PARA_HEADER");
    let nchars = u32::from_le_bytes([ph[0], ph[1], ph[2], ph[3]]);
    assert_ne!(nchars & 0x8000_0000, 0, "nchars bit31");

    // 3. The first section paragraph uses break_type=0x03 (offset 11), matching genuine files.
    assert_eq!(ph[11], 0x03, "구역 첫 문단 break_type");

    // 4. PARA_CHAR_SHAPE run count equals char_shape_cnt (offset 12, u16), deduplicated to one.
    let cs = first_record(&bt, 0x44).expect("PARA_CHAR_SHAPE");
    let cnt = u16::from_le_bytes([ph[12], ph[13]]);
    assert_eq!(
        cs.len() / 8,
        cnt as usize,
        "char_shape run 수=char_shape_cnt"
    );
    assert_eq!(cnt, 1, "단일 문단은 단일 char_shape run (중복 없음)");

    // 5. PAGE_BORDER_FILL begins with attribute u32=1 (not leftover hello_world sample garbage).
    let pbf = first_record(&bt, 0x4B).expect("PAGE_BORDER_FILL");
    assert_eq!(
        u32::from_le_bytes([pbf[0], pbf[1], pbf[2], pbf[3]]),
        1,
        "PAGE_BORDER_FILL attribute"
    );
}

/// GFM tables, including empty cells: every cell LIST_HEADER has nparas >= 1.
///
/// Hangul rejects a document as corrupt when a cell lacks PARA_HEADER (nparas=0),
/// as observed in the old M6-md생성.hwp output. At cell close, from_markdown calls
/// flush_paragraph_inner(force=true) and fills missing cells with vec![Paragraph::default()]
/// to ensure nparas >= 1. Cover short rows, empty cells, and header-only tables.
#[test]
fn 표_빈셀_포함_모든_셀_nparas_1이상() {
    // Include empty cells (`| |`), a short row (two cells vs. three header columns), and header-only rows.
    let doc =
        hwp_convert::from_markdown("|  |  |  |\n| --- | --- | --- |\n| a |  |  |\n| b | c |\n");
    let out = tmp("synth_empty_cell.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();

    let list_headers = all_records(&bt, 0x48); // LIST_HEADER
    assert!(!list_headers.is_empty(), "표 셀 LIST_HEADER 가 있어야");
    for (i, lh) in list_headers.iter().enumerate() {
        let nparas = i32::from_le_bytes([lh[0], lh[1], lh[2], lh[3]]);
        assert!(
            nparas >= 1,
            "LIST_HEADER #{i}: nparas={nparas} — 빈 셀에도 문단 1개 필수(한글 손상 방지)"
        );
    }
}

/// Tables extended with add_rows must satisfy the Hangul synthesis constraints.
///
/// Tables extended by form filling (`hwp edit --add-row`, `hwp fill --data tables`) must
/// retain nparas >= 1, paragraph terminators, and nchars bit31 even in new empty cells;
/// row_cell_counts must agree with row/cell counts (the hwp5 extraction assertion).
/// Hangul rejects inconsistent output as corrupt.
#[test]
fn 행_추가_표_합성_규격_충족() {
    let mut doc = hwp_convert::from_markdown("| 품목 | 수량 |\n| --- | --- |\n| | |\n");
    // Add three empty rows and fill some cells (form modification scenario).
    hwp_convert::add_rows(&mut doc, 0, None, 3).unwrap();
    hwp_convert::set_cell(&mut doc, 0, 1, 0, "노트북").unwrap();
    hwp_convert::set_cell(&mut doc, 0, 1, 1, "5").unwrap();
    // Leave row 4 empty to check empty-cell constraints.
    let out = tmp("synth_grown_table.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    // 1) Read-back IR: five rows and consistent row_cell_counts.
    let reread = hwp5::read_document(&out).unwrap().document;
    let table = reread.sections[0]
        .paragraphs
        .iter()
        .flat_map(|p| &p.controls)
        .find_map(|c| match c {
            hwp_model::Control::Table(t) => Some(t),
            _ => None,
        })
        .expect("표 재읽기");
    assert_eq!(table.rows, 5, "행 수 = 원래 2 + 추가 3");
    assert_eq!(
        table.row_cell_counts.len(),
        table.rows as usize,
        "row_cell_counts 길이 == 행 수"
    );
    assert_eq!(
        table
            .row_cell_counts
            .iter()
            .map(|c| *c as usize)
            .sum::<usize>(),
        table.cells.len(),
        "row_cell_counts 합 == 셀 수"
    );

    // 2) Every cell LIST_HEADER has nparas >= 1, including new empty cells.
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();
    let list_headers = all_records(&bt, 0x48);
    assert_eq!(
        list_headers.len(),
        table.cells.len(),
        "셀 수 == LIST_HEADER 수"
    );
    for (i, lh) in list_headers.iter().enumerate() {
        let nparas = i32::from_le_bytes([lh[0], lh[1], lh[2], lh[3]]);
        assert!(
            nparas >= 1,
            "추가 셀 LIST_HEADER #{i}: nparas={nparas} (한글 손상)"
        );
    }

    // 3) Filled cells appear in the body (노트북/5); the new empty row remains empty.
    let text = reread.plain_text();
    assert!(
        text.contains("노트북") && text.contains('5'),
        "채운 셀 반영: {text:?}"
    );
}

/// Empty paragraphs, including empty table cells, must have no PARA_TEXT record.
///
/// Measured genuine work_report/university samples: empty paragraphs have nchars=1,
/// PARA_CHAR_SHAPE and PARA_LINE_SEG, with no PARA_TEXT (the terminator is implicit).
/// Synthesis appends 0x0d to every paragraph; emitting PARA_TEXT=[0x0d] for empty ones
/// causes Hangul to reject the file as corrupt with an empty body. This affected tables
/// with empty cells (title boxes, contents, section headers). emit_paragraph must emit
/// PARA_TEXT only when char_count>1. pyhwp accepted empty PARA_TEXT for 23 rounds;
/// only comparison with genuine bytes revealed this defect.
#[test]
fn 빈_문단은_para_text_없음() {
    // A table with several empty cells (`| |`), filled cells, and body paragraphs.
    let doc = hwp_convert::from_markdown(
        "본문 문단입니다.\n\n|  |  |  |\n| --- | --- | --- |\n| 채움 |  |  |\n|  |  |  |\n",
    );
    let out = tmp("synth_empty_para.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let bt = c.read_record_stream("/BodyText/Section0").unwrap();
    let recs = records_with_level(&bt);

    let mut empty_paras = 0;
    let mut filled_paras = 0;
    for (i, (tag, _lvl, p)) in recs.iter().enumerate() {
        if *tag != 0x42 {
            continue; // PARA_HEADER only.
        }
        let nchars = u32::from_le_bytes([p[0], p[1], p[2], p[3]]) & 0x7FFF_FFFF;
        // Does this paragraph have a PARA_TEXT (0x43) child before the next PARA_HEADER?
        let has_text = recs[i + 1..]
            .iter()
            .take_while(|(t, _, _)| *t != 0x42)
            .any(|(t, _, _)| *t == 0x43);
        if nchars == 1 {
            empty_paras += 1;
            assert!(
                !has_text,
                "빈 문단(nchars=1)은 PARA_TEXT 가 없어야 한다 — 0x0d 만 든 PARA_TEXT 는 한글 손상(빈 셀 표)"
            );
        } else {
            filled_paras += 1;
            assert!(has_text, "채워진 문단(nchars>1)은 PARA_TEXT 가 있어야");
        }
    }
    assert!(
        empty_paras > 0,
        "빈 셀 표는 빈 문단을 만들어야 한다(시험 전제)"
    );
    assert!(filled_paras > 0, "채워진 문단도 있어야 한다(시험 전제)");
}

/// Flatten every stream record into (tag, level, payload).
fn records_with_level(data: &[u8]) -> Vec<(u16, u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let h = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let t = (h & 0x3FF) as u16;
        let lvl = ((h >> 10) & 0x3FF) as u16;
        let mut sz = h >> 20;
        let mut hl = 4;
        if sz == 0xFFF {
            sz = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            hl = 8;
        }
        out.push((t, lvl, data[i + hl..i + hl + sz as usize].to_vec()));
        i += hl + sz as usize;
    }
    out
}

/// Payloads of all stream records with the requested tag.
fn all_records(data: &[u8], tag: u16) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let h = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let t = (h & 0x3FF) as u16;
        let mut sz = h >> 20;
        let mut hl = 4;
        if sz == 0xFFF {
            sz = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            hl = 8;
        }
        if t == tag {
            out.push(data[i + hl..i + hl + sz as usize].to_vec());
        }
        i += hl + sz as usize;
    }
    out
}

/// The first stream record payload with the requested tag.
fn first_record(data: &[u8], tag: u16) -> Option<Vec<u8>> {
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let h = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let t = (h & 0x3FF) as u16;
        let mut sz = h >> 20;
        let mut hl = 4;
        if sz == 0xFFF {
            sz = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            hl = 8;
        }
        if t == tag {
            return Some(data[i + hl..i + hl + sz as usize].to_vec());
        }
        i += hl + sz as usize;
    }
    None
}

/// Payload sizes of stream records with the requested tag.
fn record_sizes(data: &[u8], tag: u16) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= data.len() {
        let h = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let t = (h & 0x3FF) as u16;
        let mut sz = h >> 20;
        let mut hl = 4;
        if sz == 0xFFF {
            sz = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            hl = 8;
        }
        if t == tag {
            out.push(sz);
        }
        i += hl + sz as usize;
    }
    out
}

/// New form fields (%clk) survive an HWP5 binary round trip: reversed ctrl_id in the
/// payload and the CTRL_DATA name BSTR are restored through the actual writer/reader.
/// IR-only assertions cannot verify the payload bytes.
#[test]
fn 누름틀_생성_이진_왕복() {
    let mut doc = hwp_convert::from_markdown("수신: 부서명\n\n참조: 부서명");
    assert!(hwp_convert::create_field(&mut doc, "수신:", "수신처", "").unwrap());
    // Fill the field in the same operation sequence.
    assert_eq!(
        hwp_convert::set_field(&mut doc, "수신처", "기획팀").unwrap(),
        1
    );

    let out = tmp("field.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let reread = hwp5::read_document(&out).unwrap();

    let fields = hwp_convert::list_fields(&reread.document);
    let clk: Vec<_> = fields.iter().filter(|f| f.ctrl_id == "%clk").collect();
    assert_eq!(clk.len(), 1, "누름틀 1개가 왕복돼야: {fields:?}");
    assert_eq!(clk[0].kind, "누름틀");
    assert_eq!(clk[0].name.as_deref(), Some("수신처"));
    assert_eq!(clk[0].value, "기획팀");
}

/// New bookmarks (bokm) survive an HWP5 binary round trip: code-22 ExtCtrl payload
/// (reversed ctrl_id) and bokm CTRL_DATA name BSTR are restored by the writer/reader.
#[test]
fn 책갈피_생성_이진_왕복() {
    let mut doc = hwp_convert::from_markdown("제목 문단\n\n다음 문단");
    assert!(hwp_convert::create_bookmark(&mut doc, "제목", "책갈피테스트").unwrap());

    let out = tmp("bookmark.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let reread = hwp5::read_document(&out).unwrap();

    let bms = hwp_convert::list_bookmarks(&reread.document);
    assert_eq!(bms.len(), 1, "책갈피 1개가 왕복돼야: {bms:?}");
    assert_eq!(bms[0].name, "책갈피테스트");
}

/// New hyperlinks (%hlk) survive an HWP5 binary round trip: field command (URL)
/// bytes are restored through the actual writer/reader.
#[test]
fn 하이퍼링크_생성_이진_왕복() {
    let mut doc = hwp_convert::from_markdown("문서: 참고\n\n본문");
    assert!(
        hwp_convert::create_hyperlink(&mut doc, "문서:", "https://example.com/a", "여기").unwrap()
    );

    let out = tmp("hyperlink.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let reread = hwp5::read_document(&out).unwrap();

    let fields = hwp_convert::list_fields(&reread.document);
    let hlk: Vec<_> = fields.iter().filter(|f| f.ctrl_id == "%hlk").collect();
    assert_eq!(hlk.len(), 1, "하이퍼링크 1개가 왕복돼야: {fields:?}");
    assert_eq!(hlk[0].kind, "하이퍼링크");
    assert_eq!(hlk[0].value, "여기");
    assert_eq!(
        hlk[0].command.as_deref(),
        Some("https\\://example.com/a;1;0;0;")
    );
}

/// HWPX text boxes (structured shapes plus paragraphs) degrade safely: native Hangul
/// rejected synthesized SHAPE_COMPONENT records (check 25), so text is hoisted into
/// the body and the shape wrapper omitted. Text survives; no gso/shape records remain.
#[test]
fn 글상자_hwpx출신_안전저하_텍스트보존() {
    use hwp_model::{CharShapeId, Control, GenericControl, HwpChar, Paragraph, ParagraphList};

    let mut doc = hwp_convert::from_markdown("본문 문단\n\n둘째 문단");
    let boxed = Paragraph {
        chars: "상자속글".chars().map(HwpChar::Text).collect(),
        char_shape_runs: vec![(0, CharShapeId(0))],
        ..Default::default()
    };
    let shape = hwp_model::ShapeGeom {
        kind: hwp_model::ShapeKind::Rect,
        x: 0,
        y: 0,
        w: 4000,
        h: 2000,
        points: Vec::new(),
        fill: 0x00CCEEFF,
        fill_gradient: None,
        border_color: 0x000000FF,
        border_width: 40,
        round_ratio: 0,
        border_style: 0,
        arrow_start: 0,
        arrow_end: 0,
        anchored: true,
        description: None,
    };
    let gso = GenericControl {
        ctrl_id: *b"rect", // The form produced by the HWPX reader.
        data: Vec::new(),
        paragraph_lists: vec![ParagraphList {
            header_data: Vec::new(),
            paragraphs: vec![boxed],
        }],
        extras: Vec::new(),
        raw_children: Vec::new(),
        gso_shapes: vec![shape],
        equation: None,
        column_def: None,
        caption: None,
        hwpx_raw_xml: None,
        container_box: None,
    };
    // Attach an ExtCtrl code-11 anchor and control to the second paragraph.
    let para = &mut doc.sections[0].paragraphs[1];
    let idx = para.controls.len() as u32;
    para.chars.push(HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: *b"rect",
        payload: vec![0u8; 12],
        ctrl_index: Some(idx),
    });
    para.controls.push(Control::Generic(gso));
    para.header.ctrl_mask = 0;

    let out = tmp("gso_degrade.hwp");
    let warnings = hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("본문 텍스트로 저하")),
        "글상자 저하 경고: {warnings:?}"
    );

    let reread = hwp5::read_document(&out).unwrap().document;
    // Text-box text is hoisted into the body and preserved.
    assert!(
        reread.plain_text().contains("상자속글"),
        "글상자 텍스트 보존: {}",
        reread.plain_text()
    );
    // No gso/shape records remain (wrapper omitted).
    let has_gso = reread
        .sections
        .iter()
        .flat_map(|s| &s.paragraphs)
        .flat_map(|p| &p.controls)
        .any(|c| matches!(c, Control::Generic(g) if g.ctrl_id == *b"gso "));
    assert!(!has_gso, "gso 래퍼는 저하로 제거돼야");
    // The corrupting payload-free control drop is replaced by text hoisting.
    assert!(!reread.plain_text().is_empty(), "유효 문서(빈 본문 아님)");
}

/// Style ladder (from_markdown): lists use native BULLET/NUMBERING definitions
/// (B7, 25B, verified in native Hangul); H1-H3 section numbers are literal prefixes.
/// Assert BULLET count equals IR definitions and section-number text round-trips.
#[test]
fn 스타일_사다리_hwp5_정의_왕복() {
    let doc = hwp_convert::from_markdown("# 제목\n\n- 항목\n  - 하위\n\n1. 첫\n2. 둘\n");
    assert!(!doc.header.bullet_chars.is_empty(), "글머리 정의 생성");
    let out = tmp("synth_ladder.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let di = c.read_record_stream("/DocInfo").unwrap();
    let bullets = all_records(&di, 0x18);
    assert_eq!(
        bullets.len(),
        doc.header.bullet_chars.len(),
        "BULLET 레코드 수 == IR 글머리 정의 수"
    );

    // Section numbers round-trip as literal prefixes; list body text has no markers.
    let reread = hwp5::read_document(&out).unwrap().document;
    let text = reread.plain_text();
    assert!(text.contains("1. 제목"), "{text}");
    assert!(text.contains("항목"), "{text}");
    assert!(!text.contains("❍"), "리터럴 마커는 더 쓰지 않는다: {text}");
}

#[test]
fn default_source_free_numbering_stays_on_the_seven_level_hwp5_path() {
    let doc = hwp_convert::from_markdown("1. ordinary item\n");
    let out = tmp("default-source-free-numbering.hwp");

    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let mut container = hwp5::Hwp5Container::open(&out).unwrap();
    let doc_info = container.read_record_stream("/DocInfo").unwrap();
    let numberings = all_records(&doc_info, 0x17);
    assert_eq!(
        numberings.len(),
        1,
        "the default definition remains singular"
    );
    assert_eq!(
        numberings[0].len(),
        226,
        "the default definition remains seven-level"
    );

    let reread = hwp5::read_document(&out).unwrap().document;
    assert_eq!(reread.header.numbering_levels.len(), 1);
    assert_eq!(reread.header.numbering_levels[0].len(), 7);
}

#[test]
fn nonofficial_hwpx_numbering_with_level_eight_uses_the_default_hwp5_path() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    let doc = hwpx::read_document(&fixture).unwrap().document;
    assert!(
        doc.header
            .numbering_levels
            .iter()
            .flatten()
            .any(|level| level.fmt == hwp_model::NumFmt::CircledHangulSyllable),
        "fixture exercises the pre-regression level-eight branch"
    );
    assert!(
        doc.header
            .numbering_levels
            .iter()
            .any(|levels| levels.len() != 8),
        "fixture is not the complete direct eight-level contract"
    );

    let out = tmp("nonofficial-hwpx-default-numbering.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let mut container = hwp5::Hwp5Container::open(&out).unwrap();
    let doc_info = container.read_record_stream("/DocInfo").unwrap();
    let numberings = all_records(&doc_info, 0x17);
    assert!(
        numberings.iter().all(|data| data.len() == 226),
        "non-official HWPX definitions must remain on the default HWP5 serializer"
    );
}

#[test]
fn official_eight_level_hwp5_matches_evidence_contract() {
    let doc = hwp_convert::from_markdown_with(
        "1. level 1\n   1. level 2\n      1. level 3\n         1. level 4\n            1. level 5\n               1. level 6\n                  1. level 7\n                     1. level 8\n",
        &hwp_convert::MarkdownImportOptions {
            preset: Some(hwp_convert::OfficialPreset::Gian),
            ..Default::default()
        },
    );
    let out = tmp("official-eight-level-evidence-contract.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let mut container = hwp5::Hwp5Container::open(&out).unwrap();
    let doc_info = container.read_record_stream("/DocInfo").unwrap();
    let numberings = all_records(&doc_info, 0x17);
    assert_eq!(numberings.len(), 8, "eight 1-based NUMBERING definitions");
    assert!(
        numberings.iter().all(|data| data.len() == 230),
        "the direct contract has ten slots and ten persisted starts"
    );
    for data in &numberings {
        assert_eq!(
            u32::from_le_bytes(data[172..176].try_into().unwrap()),
            0x12c,
            "slot 8 uses the proven circled-Hangul selector"
        );
        assert_eq!(&data[186..190], &[b'^', 0, b'8', 0], "slot 8 template");
        assert!(
            (218..230).step_by(4).all(|offset| u32::from_le_bytes(
                data[offset..offset + 4].try_into().unwrap()
            ) == 1),
            "extension start values"
        );
    }

    let numbered_shapes: Vec<_> = all_records(&doc_info, 0x19)
        .into_iter()
        .filter(|data| ((u32::from_le_bytes(data[0..4].try_into().unwrap()) >> 23) & 0x3) == 2)
        .collect();
    assert_eq!(numbered_shapes.len(), 8);
    for (index, data) in numbered_shapes.iter().enumerate() {
        let attr = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let raw_level = (attr >> 25) & 0x7;
        assert_eq!(raw_level, u32::try_from(index.min(6)).unwrap());
        assert_eq!(
            u16::from_le_bytes(data[30..32].try_into().unwrap()),
            index as u16 + 1
        );
        assert_eq!(
            i32::from_le_bytes(data[4..8].try_into().unwrap()),
            (index as i32 + 1) * 2000
        );
        assert_eq!(i32::from_le_bytes(data[12..16].try_into().unwrap()), -2000);
        assert_eq!(u16::from_le_bytes(data[32..34].try_into().unwrap()), 1);
        assert_eq!(
            u32::from_le_bytes(data[54..58].try_into().unwrap()),
            index as u32,
            "native PARA_SHAPE tail binds the paragraph to its zero-based list level"
        );
    }

    let reread = hwp5::read_document(&out).unwrap().document;
    let level8 = reread.sections[0]
        .paragraphs
        .iter()
        .find(|paragraph| paragraph.plain_text() == "level 8")
        .expect("level 8 paragraph");
    let shape = &reread.header.para_shapes[level8.para_shape.0 as usize];
    assert_eq!(
        shape.head_level(),
        8,
        "reread retains the semantic level-8 link"
    );
    assert_eq!(shape.numbering_id, 7, "disk ref 8 normalizes to IR ref 7");
    assert_eq!(
        u32::from_le_bytes(shape.tail[12..16].try_into().unwrap()),
        7,
        "reread retains the native HWP5 level-8 paragraph binding"
    );
}

#[test]
fn official_hwp5_direct_contract_handles_sentinel_before_numbering() {
    let doc = hwp_convert::from_markdown_with(
        "source first\n\n1. numbered item\n   1. nested item\n\nsource last\n",
        &hwp_convert::MarkdownImportOptions {
            preset: Some(hwp_convert::OfficialPreset::Gian),
            ..Default::default()
        },
    );
    let out = tmp("official-sentinel-before-numbering.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();

    let mut container = hwp5::Hwp5Container::open(&out).unwrap();
    let doc_info = container.read_record_stream("/DocInfo").unwrap();
    let numberings = all_records(&doc_info, 0x17);
    assert_eq!(
        numberings.len(),
        8,
        "official output materializes all levels"
    );
    assert!(
        numberings.iter().all(|data| data.len() == 230),
        "the direct evidence record must be used even with two source definitions"
    );

    let reread = hwp5::read_document(&out).unwrap().document;
    assert_eq!(reread.header.numbering_levels[0].len(), 8);
    assert_eq!(reread.header.numbering_levels[1].len(), 8);
}

#[test]
fn official_hwp5_continuation_14_to_15_matches_evidence() {
    use hwp_model::list::ListState;

    let mut doc = hwp_convert::from_markdown_with(
        "1. level 1\n   1. level 2\n      1. level 3\n         1. level 4\n            1. level 5\n               1. level 6\n                  1. level 7\n                     1. level 8\n",
        &hwp_convert::MarkdownImportOptions {
            preset: Some(hwp_convert::OfficialPreset::Gian),
            ..Default::default()
        },
    );
    let prototypes: Vec<_> = [2, 6, 8]
        .into_iter()
        .map(|level| {
            doc.sections[0]
                .paragraphs
                .iter()
                .find(|paragraph| {
                    doc.header.para_shapes[paragraph.para_shape.0 as usize].head_level() == level
                })
                .unwrap()
                .clone()
        })
        .collect();
    doc.sections[0].paragraphs.clear();
    for prototype in prototypes {
        doc.sections[0]
            .paragraphs
            .extend(std::iter::repeat_n(prototype, 15));
    }

    let out = tmp("official-evidence-continuation.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let reread = hwp5::read_document(&out).unwrap().document;
    let mut state = ListState::default();
    let markers: Vec<_> = reread.sections[0]
        .paragraphs
        .iter()
        .filter_map(|paragraph| state.marker(&reread, paragraph))
        .collect();
    assert_eq!(markers[13], "하.");
    assert_eq!(markers[14], "거.");
    assert_eq!(markers[28], "(하)");
    assert_eq!(markers[29], "(거)");
    assert_eq!(markers[43], "㉻");
    assert_eq!(markers[44], "거", "native HWP supplies the proven circle");
}

#[test]
fn official_hwp5_rejects_unproven_level_or_continuation_before_publication() {
    let markdown = "1. level 1\n   1. level 2\n      1. level 3\n         1. level 4\n            1. level 5\n               1. level 6\n                  1. level 7\n                     1. level 8\n";
    let options = hwp_convert::MarkdownImportOptions {
        preset: Some(hwp_convert::OfficialPreset::Gian),
        ..Default::default()
    };

    let mut unsupported_level = hwp_convert::from_markdown_with(markdown, &options);
    let level8_shape = unsupported_level
        .header
        .para_shapes
        .iter_mut()
        .find(|shape| shape.head_level() == 8)
        .unwrap();
    level8_shape.list_level = Some(9);
    let level_out = tmp("official-unproven-level.hwp");
    let _ = std::fs::remove_file(&level_out);
    let error = hwp5::write_document(
        &unsupported_level,
        &level_out,
        &hwp5::WriteOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        hwp5::Hwp5Error::UnsupportedOfficialNumberingRange(_)
    ));
    assert!(!level_out.exists(), "failure must precede file publication");

    let mut unsupported_count = hwp_convert::from_markdown_with(markdown, &options);
    let prototype = unsupported_count.sections[0]
        .paragraphs
        .iter()
        .find(|paragraph| {
            unsupported_count.header.para_shapes[paragraph.para_shape.0 as usize].head_level() == 8
        })
        .unwrap()
        .clone();
    unsupported_count.sections[0].paragraphs = std::iter::repeat_n(prototype, 16).collect();
    let count_out = tmp("official-unproven-continuation.hwp");
    let _ = std::fs::remove_file(&count_out);
    let error = hwp5::write_document(
        &unsupported_count,
        &count_out,
        &hwp5::WriteOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        hwp5::Hwp5Error::UnsupportedOfficialNumberingRange(_)
    ));
    assert!(!count_out.exists(), "failure must precede file publication");
}

#[test]
fn official_hwp5_rejects_sixteenth_level_two_item_in_writer_visible_subtrees() {
    let markdown = "1. level 1\n   1. level 2\n";
    let options = hwp_convert::MarkdownImportOptions {
        preset: Some(hwp_convert::OfficialPreset::Gian),
        ..Default::default()
    };
    let direct_document = || {
        let doc = hwp_convert::from_markdown_with(markdown, &options);
        let level_two = doc.sections[0]
            .paragraphs
            .iter()
            .find(|paragraph| {
                doc.header.para_shapes[paragraph.para_shape.0 as usize].head_level() == 2
            })
            .unwrap()
            .clone();
        (doc, level_two)
    };

    let (mut table_doc, level_two) = direct_document();
    table_doc.sections[0].paragraphs[0]
        .controls
        .push(hwp_model::Control::Table(hwp_model::Table {
            common_data: Vec::new(),
            placement: None,
            attr: 0,
            rows: 1,
            cols: 1,
            cell_spacing: 0,
            inner_margins: [0; 4],
            row_cell_counts: vec![1],
            border_fill: Default::default(),
            table_tail: Vec::new(),
            caption: None,
            extras: Vec::new(),
            cells: vec![hwp_model::Cell {
                list_attr: 0,
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
                width: hwp_model::HwpUnit(1),
                height: hwp_model::HwpUnit(1),
                margins: [0; 4],
                border_fill: Default::default(),
                header_tail: Vec::new(),
                paragraphs: std::iter::repeat_n(level_two, 15).collect(),
            }],
        }));

    let output = tmp("official-continuation-table.hwp");
    let _ = std::fs::remove_file(&output);
    let error =
        hwp5::write_document(&table_doc, &output, &hwp5::WriteOptions::default()).unwrap_err();
    assert!(matches!(
        error,
        hwp5::Hwp5Error::UnsupportedOfficialNumberingRange(_)
    ));
    assert!(
        !output.exists(),
        "the sixteenth level-two item in a table must not publish an HWP"
    );
}

#[test]
fn official_hwp5_rejects_sixteenth_level_two_item_in_materializable_control_list() {
    let markdown = "1. level 1\n   1. level 2\n";
    let options = hwp_convert::MarkdownImportOptions {
        preset: Some(hwp_convert::OfficialPreset::Gian),
        ..Default::default()
    };
    let mut doc = hwp_convert::from_markdown_with(markdown, &options);
    let level_two = doc.sections[0]
        .paragraphs
        .iter()
        .find(|paragraph| doc.header.para_shapes[paragraph.para_shape.0 as usize].head_level() == 2)
        .unwrap()
        .clone();

    // `fn  ` is intentionally used instead of a synthetic `gso ` wrapper:
    // the HWP5 writer materializes its paragraph list (whereas an empty GSO
    // wrapper is deliberately dropped as opaque/unrepresentable).
    doc.sections[0].paragraphs[0]
        .controls
        .push(hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"fn  ",
            data: Vec::new(),
            paragraph_lists: vec![hwp_model::ParagraphList {
                header_data: Vec::new(),
                paragraphs: std::iter::repeat_n(level_two, 15).collect(),
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

    let output = tmp("official-continuation-control.hwp");
    let _ = std::fs::remove_file(&output);
    let error = hwp5::write_document(&doc, &output, &hwp5::WriteOptions::default()).unwrap_err();
    assert!(matches!(
        error,
        hwp5::Hwp5Error::UnsupportedOfficialNumberingRange(_)
    ));
    assert!(
        !output.exists(),
        "the sixteenth level-two item in a materializable control list must not publish an HWP"
    );
}

/// Stream-separator regression: even if cfb enumerates platform separators (Windows `\`),
/// list_streams/body_sections must return normalized `/` paths. Otherwise the
/// `/BodyText/Section` filter misses every section and Windows reads an empty body
/// (the root cause of three windows-latest missing-table/empty-cat failures).
#[test]
fn 스트림_경로_구분자_정규화() {
    let doc = hwp_convert::from_markdown("본문\n");
    let out = tmp("path_sep.hwp");
    hwp5::write_document(&doc, &out, &hwp5::WriteOptions::default()).unwrap();
    let mut c = hwp5::Hwp5Container::open(&out).unwrap();
    let streams = c.list_streams();
    assert!(!streams.is_empty());
    for s in &streams {
        assert!(s.path.starts_with('/'), "절대 경로: {}", s.path);
        assert!(!s.path.contains('\\'), "백슬래시 금지: {}", s.path);
    }
    let sections = c.body_sections();
    assert_eq!(sections, vec!["/BodyText/Section0".to_string()]);
    // Enumerated paths must be readable directly.
    let body = c.read_record_stream(&sections[0]).unwrap();
    assert!(!body.is_empty(), "본문 스트림 읽기");
}

/// GE-beta7/beta8: XMLTemplate and DocHistory streams pass through a read/write cycle
/// with identical bytes, without interpretation. Inject both storages into synthesized
/// HWP, then check their presence and bytes after read_document/write_document.
#[test]
fn xmltemplate_dochistory_스토리지_왕복() {
    use std::io::{Read as _, Write as _};

    // 1. Save a base document and inject both storages into CFB, mimicking Hangul output.
    let doc = hwp_convert::from_markdown("본문");
    let a = tmp("storage_a.hwp");
    hwp5::write_document(&doc, &a, &hwp5::WriteOptions::default()).unwrap();

    let tmpl_bytes: Vec<u8> = (0u8..=255).collect(); // Arbitrary uncompressed bytes.
    let hist_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02];
    {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&a)
            .unwrap();
        let mut cfb = cfb::CompoundFile::open(file).unwrap();
        cfb.create_storage("/XMLTemplate").unwrap();
        cfb.create_new_stream("/XMLTemplate/Schema")
            .unwrap()
            .write_all(&tmpl_bytes)
            .unwrap();
        cfb.create_storage("/DocHistory").unwrap();
        cfb.create_new_stream("/DocHistory/VersionLog0")
            .unwrap()
            .write_all(&hist_bytes)
            .unwrap();
        cfb.flush().unwrap();
    }

    // 2. read_document preserves both storages verbatim in the IR.
    let reread = hwp5::read_document(&a).unwrap().document;
    assert_eq!(
        reread.hwp5_xml_template,
        vec![("/XMLTemplate/Schema".to_string(), tmpl_bytes.clone())],
        "XMLTemplate 포착"
    );
    assert_eq!(
        reread.hwp5_doc_history,
        vec![("/DocHistory/VersionLog0".to_string(), hist_bytes.clone())],
        "DocHistory 포착"
    );

    // 3. write_document re-emits both storages byte-identically.
    let b = tmp("storage_b.hwp");
    hwp5::write_document(&reread, &b, &hwp5::WriteOptions::default()).unwrap();
    let file = std::fs::File::open(&b).unwrap();
    let mut cfb = cfb::CompoundFile::open(file).unwrap();
    let mut got_tmpl = Vec::new();
    cfb.open_stream("/XMLTemplate/Schema")
        .unwrap()
        .read_to_end(&mut got_tmpl)
        .unwrap();
    assert_eq!(got_tmpl, tmpl_bytes, "XMLTemplate 바이트 동일");
    let mut got_hist = Vec::new();
    cfb.open_stream("/DocHistory/VersionLog0")
        .unwrap()
        .read_to_end(&mut got_hist)
        .unwrap();
    assert_eq!(got_hist, hist_bytes, "DocHistory 바이트 동일");
}
