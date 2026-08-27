//! 표 편집 통합 테스트 — 커밋된 익명화 픽스처(fixtures/samples/report-tables.hwpx)에
//! 하드 의존한다(스킵 없음). 표 지도(재귀 깊이 우선 인덱스):
//!   #0 5x4(병합2, 깨끗한 행 0)  #1 9x6(병합6, 깨끗한 행 3~8)  #2 11x10(병합30, 깨끗한 행 없음)
//!   #3~#8 중첩 2x1 단순표(표#2 셀 안)  #9 7x2 단순표([별표 1] 전문가 등급 기준, 병합 없음)

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use hwp_model::{Control, HwpChar, Table};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn fixture() -> PathBuf {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    assert!(p.exists(), "커밋된 픽스처가 없습니다: {}", p.display());
    p
}

fn tmp(name: &str) -> PathBuf {
    // PID 포함 — 같은 머신에서 cargo test가 동시에 돌면(다른 세션·CI 병렬) 고정 경로가
    // 서로 산출물을 덮어써 플레이크가 난다(실측).
    let dir = std::env::temp_dir().join(format!("hwp-cli-table-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Generate a compact form table for label-fill tests without depending on private fixtures.
fn label_form(name: &str, markdown: &str) -> PathBuf {
    let source = tmp(&format!("{name}.md"));
    std::fs::write(&source, markdown).unwrap();
    let form = tmp(&format!("{name}.hwpx"));
    let result = hwp()
        .args(["new", "--from"])
        .arg(&source)
        .arg("-o")
        .arg(&form)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "form generation: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    form
}

/// 픽스처를 임시 사본으로 복사해 편집한다(원본 불변).
fn copy_fixture(name: &str) -> PathBuf {
    let dst = tmp(name);
    std::fs::copy(fixture(), &dst).unwrap();
    dst
}

fn cat(path: &PathBuf) -> String {
    let out = hwp().arg("cat").arg(path).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn read_zip_entry(path: &PathBuf, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut buf = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
    buf
}

#[derive(Clone, Copy)]
enum GeneratedLabelTable {
    Adjacent,
    Header,
    Nested,
}

fn first_table(doc: &hwp_model::Document) -> Table {
    doc.sections
        .iter()
        .flat_map(|section| &section.paragraphs)
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            Control::Table(table) => Some(table.clone()),
            _ => None,
        })
        .expect("generated markdown has a table")
}

fn table_has_text(table: &Table, text: &str) -> bool {
    table.cells.iter().any(|cell| {
        cell.paragraphs
            .iter()
            .any(|paragraph| paragraph.plain_text() == text)
    })
}

/// Build a generated HWPX form with a nested table instead of relying on a
/// committed fixture. Each layout permutation moves the nested table relative
/// to the adjacent and header/data form layouts while retaining three ambiguous
/// `중복` candidates and one unique `유일` target.
fn generated_label_ordering_form(name: &str, layout: &[GeneratedLabelTable]) -> PathBuf {
    let mut markdown = String::from("generated label ordering fixture\n\n");
    for table in layout {
        match table {
            GeneratedLabelTable::Adjacent => {
                markdown.push_str("| 중복 | |\n|---|---|\n\n");
            }
            GeneratedLabelTable::Header => {
                markdown.push_str("| 중복 |\n|---|\n| |\n\n");
            }
            GeneratedLabelTable::Nested => {
                markdown.push_str("| 바깥 | NEST-HOST |\n|---|---|\n\n");
            }
        }
    }
    markdown.push_str("| 유일 | |\n|---|---|\n");

    let mut doc = hwp_convert::from_markdown(&markdown);
    let inner = first_table(&hwp_convert::from_markdown("| 중복 |\n|---|\n| |\n"));
    let host = doc
        .sections
        .iter_mut()
        .flat_map(|section| &mut section.paragraphs)
        .flat_map(|paragraph| &mut paragraph.controls)
        .find_map(|control| match control {
            Control::Table(table) if table_has_text(table, "NEST-HOST") => Some(table),
            _ => None,
        })
        .expect("generated fixture has a nested-table host");
    let host_cell = host
        .cells
        .iter_mut()
        .find(|cell| {
            cell.paragraphs
                .iter()
                .any(|paragraph| paragraph.plain_text() == "NEST-HOST")
        })
        .expect("generated fixture has a host cell");
    let host_paragraph = host_cell
        .paragraphs
        .first_mut()
        .expect("generated fixture host cell has a paragraph");
    let control_index = host_paragraph.controls.len() as u32;
    host_paragraph.controls.push(Control::Table(inner));
    host_paragraph.chars.push(HwpChar::ExtCtrl {
        code: 11,
        ctrl_id: *b"tbl ",
        payload: vec![0; 12],
        ctrl_index: Some(control_index),
    });

    let path = tmp(&format!("{name}.hwpx"));
    hwpx::write_document(&doc, &path).expect("write generated label fixture");
    path
}

fn label_edit(input: &Path, output: &Path, requests: &[&str]) -> std::process::Output {
    let mut command = hwp();
    command.arg("edit").arg(input).arg("-o").arg(output);
    for request in requests {
        command.args(["--set-cell-by-label", request]);
    }
    command.output().unwrap()
}

fn candidate_coordinates(stderr: &str) -> Vec<(usize, u16, u16)> {
    stderr
        .split('표')
        .skip(1)
        .filter_map(|candidate| {
            let (table, coordinate) = candidate.split_once(" (")?;
            let (coordinate, _) = coordinate.split_once(')')?;
            let (row, column) = coordinate.split_once(',')?;
            Some((table.parse().ok()?, row.parse().ok()?, column.parse().ok()?))
        })
        .collect()
}

fn request_permutations() -> [[&'static str; 3]; 6] {
    [
        ["중복=one", " 중복：=two", "중복=three"],
        ["중복=one", "중복=three", " 중복：=two"],
        [" 중복：=two", "중복=one", "중복=three"],
        [" 중복：=two", "중복=three", "중복=one"],
        ["중복=three", "중복=one", " 중복：=two"],
        ["중복=three", " 중복：=two", "중복=one"],
    ]
}

fn unique_request_permutations() -> [[&'static str; 3]; 6] {
    [
        ["유일=one", " 유일：=two", "유일=three"],
        ["유일=one", "유일=three", " 유일：=two"],
        [" 유일：=two", "유일=one", "유일=three"],
        [" 유일：=two", "유일=three", "유일=one"],
        ["유일=three", "유일=one", " 유일：=two"],
        ["유일=three", " 유일：=two", "유일=one"],
    ]
}

#[test]
fn generated_label_candidate_ordering_and_duplicate_requests_are_atomic() {
    let layouts = [
        [
            GeneratedLabelTable::Nested,
            GeneratedLabelTable::Adjacent,
            GeneratedLabelTable::Header,
        ],
        [
            GeneratedLabelTable::Header,
            GeneratedLabelTable::Nested,
            GeneratedLabelTable::Adjacent,
        ],
        [
            GeneratedLabelTable::Adjacent,
            GeneratedLabelTable::Header,
            GeneratedLabelTable::Nested,
        ],
    ];

    for (fixture_index, layout) in layouts.iter().enumerate() {
        let source =
            generated_label_ordering_form(&format!("label_ordering_{fixture_index}"), layout);
        let source_before = std::fs::read(&source).unwrap();

        let mut baseline_diagnostic = None;
        for (permutation_index, requests) in request_permutations().iter().enumerate() {
            let output = tmp(&format!(
                "label_ordering_{fixture_index}_ambiguous_{permutation_index}.hwpx"
            ));
            let result = label_edit(&source, &output, requests);
            assert!(
                !result.status.success(),
                "ambiguous generated fixture {fixture_index}, permutation {permutation_index} must fail"
            );
            assert!(
                !output.exists(),
                "ambiguous generated fixture {fixture_index}, permutation {permutation_index} must not publish output"
            );
            assert_eq!(
                std::fs::read(&source).unwrap(),
                source_before,
                "ambiguous preflight must leave every source byte unchanged"
            );

            let stderr = String::from_utf8(result.stderr).unwrap();
            assert!(stderr.contains("양식 레이블 대상이 모호합니다"), "{stderr}");
            let coordinates = candidate_coordinates(&stderr);
            assert_eq!(coordinates.len(), 3, "{stderr}");
            assert!(
                coordinates.windows(2).all(|pair| pair[0] < pair[1]),
                "candidate diagnostics must be sorted by table, row, column: {stderr}"
            );
            if let Some(baseline) = &baseline_diagnostic {
                assert_eq!(
                    &stderr, baseline,
                    "candidate diagnostic must be byte-identical across request permutations"
                );
            } else {
                baseline_diagnostic = Some(stderr);
            }
        }

        let mut baseline_duplicate = None;
        for (permutation_index, requests) in unique_request_permutations().iter().enumerate() {
            let output = tmp(&format!(
                "label_ordering_{fixture_index}_duplicate_{permutation_index}.hwpx"
            ));
            let sentinel =
                format!("duplicate-sentinel-{fixture_index}-{permutation_index}").into_bytes();
            std::fs::write(&output, &sentinel).unwrap();
            let result = label_edit(&source, &output, requests);
            assert!(
                !result.status.success(),
                "duplicate target fixture {fixture_index}, permutation {permutation_index} must fail"
            );
            assert_eq!(
                std::fs::read(&output).unwrap(),
                sentinel,
                "duplicate preflight must leave destination bytes unchanged"
            );
            assert_eq!(
                std::fs::read(&source).unwrap(),
                source_before,
                "duplicate preflight must leave all non-target source bytes unchanged"
            );

            let stderr = String::from_utf8(result.stderr).unwrap();
            assert!(stderr.contains("양식 레이블 대상이 중복됩니다"), "{stderr}");
            if let Some(baseline) = &baseline_duplicate {
                assert_eq!(
                    &stderr, baseline,
                    "duplicate diagnostic must be byte-identical across request permutations"
                );
            } else {
                baseline_duplicate = Some(stderr);
            }
        }
    }
}

#[test]
fn set_cell_by_label_fills_unique_adjacent_cell_and_validates() {
    let source = label_form(
        "label_adjacent",
        "| 성명： | |\n|---|---|\n| 소울별 | 이름 |\n",
    );
    let output = tmp("label_adjacent_out.hwpx");
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(["--set-cell-by-label", "  성명：=홍길동", "--verify"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "label fill: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(cat(&output).contains("홍길동"), "filled value is visible");
    assert!(
        hwp()
            .arg("validate")
            .arg(&output)
            .status()
            .unwrap()
            .success(),
        "label fill output validates"
    );
}

#[test]
fn set_cell_by_label_prefers_a_nonempty_adjacent_form_value_over_a_following_data_row() {
    let source = label_form(
        "label_adjacent_precedence",
        "| 성명 | {{성명}} |\n|---|---|\n| below-target-must-remain | data |\n",
    );
    let output = tmp("label_adjacent_precedence_out.hwpx");
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(["--set-cell-by-label", "성명=홍길동", "--verify"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "adjacent form precedence: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = cat(&output);
    assert!(text.contains("홍길동"), "adjacent value is changed");
    assert!(
        text.contains("below-target-must-remain"),
        "a following data-row cell must not be changed"
    );
}

#[test]
fn set_cell_by_label_fills_first_data_row_below_a_header_label() {
    let source = label_form("label_header", "| 성명 |\n|---|\n| |\n");
    let output = tmp("label_header_out.hwpx");
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(["--set-cell-by-label", "성명=홍길동"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "header fill: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(cat(&output).contains("홍길동"), "filled value is visible");
}

#[test]
fn set_cell_by_label_fills_below_a_multicolumn_header_without_ambiguity() {
    let source = label_form(
        "label_multicolumn_header",
        "| 성명 | 소속 |\n|---|---|\n| | |\n",
    );
    let output = tmp("label_multicolumn_header_out.hwpx");
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(["--set-cell-by-label", "성명=홍길동", "--verify"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "multi-column header fill: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(cat(&output).contains("홍길동"), "filled value is visible");
}

#[test]
fn set_cell_by_label_uses_exact_normalized_matching_only() {
    let source = label_form("label_exact", "| 성명: | |\n|---|---|\n");
    for (case, request) in [
        ("different_label", "성명A=값이"),
        ("substring", "성=김"),
        ("nfd", "가=김"),
    ] {
        let output = tmp(&format!("label_exact_{case}.hwpx"));
        let result = hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .args(["--set-cell-by-label", request])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{case} must not match");
        assert!(!output.exists(), "{case} must not publish an output");
    }
}

#[test]
fn set_cell_by_label_rejects_empty_or_missing_without_changing_files() {
    let source = label_form("label_atomic", "| 성명 | |\n|---|---|\n");
    let source_before = std::fs::read(&source).unwrap();
    for (case, request) in [("empty", " ：=홍길동"), ("missing", "주소=제주")] {
        let output = tmp(&format!("label_atomic_{case}.hwpx"));
        let existing = format!("sentinel-{case}").into_bytes();
        std::fs::write(&output, &existing).unwrap();
        let result = hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .args(["--set-cell-by-label", request])
            .output()
            .unwrap();
        assert!(!result.status.success(), "{case} must fail");
        assert_eq!(
            std::fs::read(&source).unwrap(),
            source_before,
            "source unchanged"
        );
        assert_eq!(
            std::fs::read(&output).unwrap(),
            existing,
            "destination unchanged"
        );
    }
}

#[test]
fn set_cell_by_label_requires_scope_for_ambiguity_and_rejects_duplicates() {
    let source = label_form(
        "label_ambiguous",
        "| 성명 | |\n|---|---|\n\n| 성명 | |\n|---|---|\n",
    );
    for (case, args) in [
        (
            "ambiguous",
            vec!["--set-cell-by-label", "성명=홍길동", "--allow-partial"],
        ),
        (
            "duplicate",
            vec![
                "--set-cell-by-label",
                "성명=홍길동",
                "--set-cell-by-label",
                " 성명：=김철수",
            ],
        ),
    ] {
        let output = tmp(&format!("label_{case}_out.hwpx"));
        let sentinel = format!("sentinel-{case}").into_bytes();
        std::fs::write(&output, &sentinel).unwrap();
        let result = hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .args(args)
            .output()
            .unwrap();
        assert!(!result.status.success(), "{case} must fail");
        assert_eq!(std::fs::read(output).unwrap(), sentinel, "{case} is atomic");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            !stderr.contains("홍길동"),
            "{case} must not disclose values"
        );
    }
}

#[test]
fn set_cell_by_label_partial_and_table_scope_are_deterministic() {
    let source = label_form(
        "label_partial_scope",
        "| 성명 | |\n|---|---|\n\n| 성명 | |\n|---|---|\n",
    );
    let default_output = tmp("label_partial_default.hwpx");
    let sentinel = b"default-sentinel";
    std::fs::write(&default_output, sentinel).unwrap();
    let default_result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&default_output)
        .args([
            "--set-cell-by-label",
            "성명=홍길동",
            "--set-cell-by-label",
            "주소=제주",
        ])
        .output()
        .unwrap();
    assert!(!default_result.status.success());
    assert_eq!(std::fs::read(&default_output).unwrap(), sentinel);

    let scoped_output = tmp("label_scope_out.hwpx");
    let scoped_result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&scoped_output)
        .args([
            "--set-cell-by-label",
            "성명=홍길동",
            "--set-cell-by-label",
            "주소=제주",
            "--label-table",
            "1",
            "--allow-partial",
        ])
        .output()
        .unwrap();
    assert!(
        scoped_result.status.success(),
        "scoped partial: {}",
        String::from_utf8_lossy(&scoped_result.stderr)
    );
    assert!(cat(&scoped_output).contains("홍길동"));
}

/// 픽스처 자체가 유효해야 한다(익명화 후에도 한컴 규격 충족).
#[test]
fn fixture_is_valid() {
    let out = hwp().arg("validate").arg(fixture()).output().unwrap();
    assert!(
        out.status.success(),
        "validate: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 표#0(병합2, 깨끗한 행 존재): 행 추가 성공 → 새 행에 값 채우기까지.
/// (edit는 add-row를 set-cell 뒤에 적용하므로 두 호출로 나눈다 — 기존 CLI 의미.)
#[test]
fn tbl0_add_row_then_fill() {
    let src = copy_fixture("tbl0_row.hwpx");
    let out = tmp("tbl0_row_out.hwpx");
    // pass 1: 행 추가
    let r1 = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-row", "0"])
        .output()
        .unwrap();
    assert!(
        r1.status.success(),
        "add-row 성공해야: {}",
        String::from_utf8_lossy(&r1.stderr)
    );
    // pass 2: 새 행(인덱스 5) 채우기
    let out2 = tmp("tbl0_row_out2.hwpx");
    let r2 = hwp()
        .arg("edit")
        .arg(&out)
        .arg("-o")
        .arg(&out2)
        .args(["--set-cell", "0:5:0=신규행값"])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "set-cell: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    assert!(cat(&out2).contains("신규행값"), "새 행 값 확인");
}

/// 표#0(5x4, 병합2): 열 추가는 이제 **지원**(GK-2 통합 — 병합 표도 열 조작 가능).
#[test]
fn tbl0_add_col_supported() {
    let src = copy_fixture("tbl0_col.hwpx");
    let out = tmp("tbl0_col_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-col", "0"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "병합 표 열 추가 지원: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    // 구조 유효(한글 규격).
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "열 추가 후 validate 통과"
    );
}

/// 표#2(11x10, 병합 30): 행 추가는 깨끗한 행이 없어 거부, 열 추가는 병합 표도 지원.
#[test]
fn tbl2_add_row_refused_col_supported() {
    let src = copy_fixture("tbl2.hwpx");
    let out = tmp("tbl2_out.hwpx");
    // 행 추가: 병합 없는 템플릿 행이 없어 거부(add_rows 규칙, 변경 없음).
    let r_row = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-row", "2"])
        .output()
        .unwrap();
    assert!(!r_row.status.success(), "add-row는 깨끗한 행 없어 거부돼야");
    // 열 추가: 병합 표도 지원(전체 폭 유지, 열 정렬 보존) + validate.
    let out2 = tmp("tbl2_col_out.hwpx");
    let r_col = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out2)
        .args(["--add-col", "2"])
        .output()
        .unwrap();
    assert!(
        r_col.status.success(),
        "병합 표 열 추가 지원: {}",
        String::from_utf8_lossy(&r_col.stderr)
    );
    assert!(
        hwp()
            .arg("validate")
            .arg(&out2)
            .output()
            .unwrap()
            .status
            .success(),
        "병합 표 열 추가 후 validate 통과"
    );
}

/// 중첩 표(재귀 인덱스 3~8): set-cell/add-row가 재귀 로케이터로 걸린다.
#[test]
fn nested_table_recursive_indexing() {
    let src = copy_fixture("nested.hwpx");
    let out = tmp("nested_out.hwpx");
    // 표#3(2x1 단순): 값 교체 + 행 추가.
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-cell", "3:0:0=중첩교체", "--add-row", "3"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "중첩 표 편집 성공해야: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    // 새 행(인덱스 2) 채우기 — 재귀 인덱스가 set-cell과 일치해야 한다.
    let out2 = tmp("nested_out2.hwpx");
    let r2 = hwp()
        .arg("edit")
        .arg(&out)
        .arg("-o")
        .arg(&out2)
        .args(["--set-cell", "3:2:0=중첩신규"])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "set-cell: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    let text = cat(&out2);
    assert!(text.contains("중첩교체"), "set-cell 재귀 인덱싱");
    assert!(
        text.contains("중첩신규"),
        "add-row 후 새 행 채우기(재귀 인덱싱)"
    );
}

/// replace 고속 경로: 미수정 엔트리(header.xml)는 입력과 바이트 동일해야 한다
/// (IR 재작성 경로였다면 재합성되어 달라진다).
#[test]
fn replace_fast_path_preserves_package() {
    let src = fixture();
    let out = tmp("replace_fast.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--replace", "한빛대학교=>검증대학교", "--verify"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "replace: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(
        String::from_utf8_lossy(&r.stderr).contains("패키지 보존"),
        "고속 경로 사용 확인: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    // header.xml은 바이트 동일, 본문은 치환됨.
    assert_eq!(
        read_zip_entry(&src, "Contents/header.xml"),
        read_zip_entry(&out, "Contents/header.xml"),
        "header.xml 바이트 보존"
    );
    let section = String::from_utf8(read_zip_entry(&out, "Contents/section0.xml")).unwrap();
    assert!(section.contains("검증대학교"), "본문 치환");
    assert!(!section.contains("한빛대학교"), "원 이름 제거");
}

/// add-col 성공 경로: 합성 단순 표에서 열 추가 → 새 셀 채우기 (.hwpx/.hwp 양쪽).
#[test]
fn add_col_success_synthetic() {
    let md = tmp("addcol.md");
    std::fs::write(&md, "| 가 | 나 |\n|----|----|\n| 1 | 2 |\n").unwrap();
    for ext in ["hwpx", "hwp"] {
        let form = tmp(&format!("addcol_form.{ext}"));
        assert!(
            hwp()
                .args(["new", "--from"])
                .arg(&md)
                .arg("-o")
                .arg(&form)
                .status()
                .unwrap()
                .success()
        );
        let out = tmp(&format!("addcol_out.{ext}"));
        let r = hwp()
            .arg("edit")
            .arg(&form)
            .arg("-o")
            .arg(&out)
            .args(["--add-col", "0"])
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "{ext} add-col: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        // 새 열(인덱스 2) 채우기.
        let out2 = tmp(&format!("addcol_out2.{ext}"));
        let r2 = hwp()
            .arg("edit")
            .arg(&out)
            .arg("-o")
            .arg(&out2)
            .args(["--set-cell", "0:0:2=열3", "--verify"])
            .output()
            .unwrap();
        assert!(
            r2.status.success(),
            "{ext} set-cell: {}",
            String::from_utf8_lossy(&r2.stderr)
        );
        assert!(cat(&out2).contains("열3"), "{ext} 새 열 값 확인");
    }
}

/// 표#9([별표 1] 7x2 단순표): 행 추가 성공 → 새 행 채우기.
#[test]
fn tbl9_add_row_then_fill() {
    let src = copy_fixture("tbl9_row.hwpx");
    let out = tmp("tbl9_row_out.hwpx");
    let r1 = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-row", "9"])
        .output()
        .unwrap();
    assert!(
        r1.status.success(),
        "단순 표 add-row 성공해야: {}",
        String::from_utf8_lossy(&r1.stderr)
    );
    let out2 = tmp("tbl9_row_out2.hwpx");
    let r2 = hwp()
        .arg("edit")
        .arg(&out)
        .arg("-o")
        .arg(&out2)
        .args(["--set-cell", "9:7:0=7급", "--set-cell", "9:7:1=신규 요건"])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "set-cell: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    let text = cat(&out2);
    assert!(
        text.contains("7급") && text.contains("신규 요건"),
        "새 행 값 확인"
    );
}

/// 표#9: 열 추가 성공 + 전체 표 폭이 정확히 보존(행별 총폭 동일).
#[test]
fn tbl9_add_col_width_preserved() {
    let src = copy_fixture("tbl9_col.hwpx");
    let out = tmp("tbl9_col_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-col", "9"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "단순 표 add-col 성공해야: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    // IR JSON으로 행별 총폭 비교 (입력 vs 출력).
    fn row_sums(path: &PathBuf, nth: usize) -> Vec<i64> {
        let out = hwp()
            .arg("cat")
            .arg(path)
            .args(["--format", "json"])
            .output()
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let mut tables = Vec::new();
        collect_tables(&j["sections"][0]["paragraphs"], &mut tables);
        let t = &tables[nth];
        let rows = t["rows"].as_u64().unwrap() as i64;
        (0..rows)
            .map(|r| {
                t["cells"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|c| c["row"].as_i64() == Some(r))
                    .map(|c| c["width"].as_i64().unwrap())
                    .sum()
            })
            .collect()
    }
    fn collect_tables<'a>(paras: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
        for p in paras.as_array().unwrap() {
            for c in p["controls"].as_array().unwrap() {
                if let Some(t) = c.get("Table") {
                    out.push(t);
                    for cell in t["cells"].as_array().unwrap() {
                        collect_tables(&cell["paragraphs"], out);
                    }
                } else if let Some(g) = c.get("Generic") {
                    for l in g["paragraph_lists"].as_array().unwrap() {
                        collect_tables(&l["paragraphs"], out);
                    }
                }
            }
        }
    }

    let before = row_sums(&src, 9);
    let after = row_sums(&out, 9);
    assert_eq!(before.len(), after.len(), "행 수 유지");
    assert_eq!(before, after, "행별 총폭 정확 보존");

    // 새 열(인덱스 2) 채우기.
    let out2 = tmp("tbl9_col_out2.hwpx");
    let r2 = hwp()
        .arg("edit")
        .arg(&out)
        .arg("-o")
        .arg(&out2)
        .args(["--set-cell", "9:0:2=비고", "--verify"])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "set-cell: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    assert!(cat(&out2).contains("비고"), "새 열 값 확인");
}

// ── #77: positioned, counted row/column insertion ────────────────────────────

/// Read table `nth` (recursive depth-first index) as JSON via `cat --format json`.
fn table_json(path: &PathBuf, nth: usize) -> serde_json::Value {
    fn collect<'a>(paras: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
        for p in paras.as_array().unwrap() {
            for c in p["controls"].as_array().unwrap() {
                if let Some(t) = c.get("Table") {
                    out.push(t);
                    for cell in t["cells"].as_array().unwrap() {
                        collect(&cell["paragraphs"], out);
                    }
                } else if let Some(g) = c.get("Generic") {
                    for l in g["paragraph_lists"].as_array().unwrap() {
                        collect(&l["paragraphs"], out);
                    }
                }
            }
        }
    }
    let out = hwp()
        .arg("cat")
        .arg(path)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut tables = Vec::new();
    collect(&j["sections"][0]["paragraphs"], &mut tables);
    tables[nth].clone()
}

/// Table #2 (11x10, 30 merges, no clean row): append stays refused, but positioned
/// insertion projects styles from the nearest row and succeeds.
#[test]
fn tbl2_positioned_add_row_supported() {
    let src = copy_fixture("tbl2_pos.hwpx");
    let out = tmp("tbl2_pos_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-row", "2:5:2:4", "--verify"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "positioned add-row on merged table: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let t = table_json(&out, 2);
    assert_eq!(t["rows"].as_u64().unwrap(), 13, "11 + 2 inserted");
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "positioned insert keeps the document valid"
    );
}

/// Table #0 (5x4, 2 merges): counted positioned row + column insertion in one pass.
#[test]
fn tbl0_positioned_counted_row_and_col() {
    let src = copy_fixture("tbl0_pos.hwpx");
    let out = tmp("tbl0_pos_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-row", "0:2:2:0", "--add-col", "0:1:2"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "positioned counted edits: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let t = table_json(&out, 0);
    assert_eq!(t["rows"].as_u64().unwrap(), 7, "5 + 2 rows");
    assert_eq!(t["cols"].as_u64().unwrap(), 6, "4 + 2 cols");
    // Fill a cell in the inserted band (row 2, col 1).
    let out2 = tmp("tbl0_pos_out2.hwpx");
    let r2 = hwp()
        .arg("edit")
        .arg(&out)
        .arg("-o")
        .arg(&out2)
        .args(["--set-cell", "0:2:1=삽입셀", "--verify"])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "set-cell into inserted band: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    assert!(cat(&out2).contains("삽입셀"), "inserted cell filled");
}

/// Malformed or out-of-bounds specs fail with a nonzero exit and publish nothing.
#[test]
fn add_row_col_spec_errors() {
    for spec in ["0:1:2:3:4", "0:abc", "0:1:0", "0:99"] {
        let src = copy_fixture("spec_err.hwpx");
        let out = tmp("spec_err_out.hwpx");
        let r = hwp()
            .arg("edit")
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .args(["--add-row", spec])
            .output()
            .unwrap();
        assert!(!r.status.success(), "--add-row {spec:?} must fail");
        assert!(!out.exists(), "--add-row {spec:?} must not publish");
    }
    for spec in ["0:1:2:3", "0:xyz", "0:1:0", "0:99"] {
        let src = copy_fixture("spec_err_col.hwpx");
        let out = tmp("spec_err_col_out.hwpx");
        let r = hwp()
            .arg("edit")
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .args(["--add-col", spec])
            .output()
            .unwrap();
        assert!(!r.status.success(), "--add-col {spec:?} must fail");
        assert!(!out.exists(), "--add-col {spec:?} must not publish");
    }
}

/// `end` is an explicit append: `--add-row "9:end:2"` behaves like the legacy form.
#[test]
fn add_row_end_keyword_appends() {
    let src = copy_fixture("tbl9_end.hwpx");
    let out = tmp("tbl9_end_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--add-row", "9:end:2"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "end append: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let t = table_json(&out, 9);
    assert_eq!(t["rows"].as_u64().unwrap(), 9, "7 + 2 appended");
}

/// Positioned insertion on synthetic HWP and HWPX documents, then fill + verify.
#[test]
fn add_row_positioned_synthetic_both_formats() {
    let md = tmp("addrow_pos.md");
    std::fs::write(&md, "| 가 | 나 |\n|----|----|\n| 1 | 2 |\n").unwrap();
    for ext in ["hwpx", "hwp"] {
        let form = tmp(&format!("addrow_pos_form.{ext}"));
        assert!(
            hwp()
                .args(["new", "--from"])
                .arg(&md)
                .arg("-o")
                .arg(&form)
                .status()
                .unwrap()
                .success()
        );
        let out = tmp(&format!("addrow_pos_out.{ext}"));
        let r = hwp()
            .arg("edit")
            .arg(&form)
            .arg("-o")
            .arg(&out)
            .args(["--add-row", "0:1:2:0", "--verify"])
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "{ext} positioned add-row: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        let out2 = tmp(&format!("addrow_pos_out2.{ext}"));
        let r2 = hwp()
            .arg("edit")
            .arg(&out)
            .arg("-o")
            .arg(&out2)
            .args(["--set-cell", "0:1:0=삽입행", "--set-cell", "0:2:1=삽입행2"])
            .output()
            .unwrap();
        assert!(
            r2.status.success(),
            "{ext} set-cell: {}",
            String::from_utf8_lossy(&r2.stderr)
        );
        let text = cat(&out2);
        assert!(text.contains("삽입행"), "{ext} inserted row 1 filled");
        assert!(text.contains("삽입행2"), "{ext} inserted row 2 filled");
        assert!(text.contains('가'), "{ext} original content kept");
    }
}

// ── #78: deep table cloning (blank / keep) ───────────────────────────────────

/// All tables (recursive depth-first index) as JSON via `cat --format json`.
fn all_tables_json(path: &PathBuf) -> Vec<serde_json::Value> {
    fn collect<'a>(paras: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
        for p in paras.as_array().unwrap() {
            for c in p["controls"].as_array().unwrap() {
                if let Some(t) = c.get("Table") {
                    out.push(t);
                    for cell in t["cells"].as_array().unwrap() {
                        collect(&cell["paragraphs"], out);
                    }
                } else if let Some(g) = c.get("Generic") {
                    for l in g["paragraph_lists"].as_array().unwrap() {
                        collect(&l["paragraphs"], out);
                    }
                }
            }
        }
    }
    let out = hwp()
        .arg("cat")
        .arg(path)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut tables = Vec::new();
    collect(&j["sections"][0]["paragraphs"], &mut tables);
    tables.into_iter().cloned().collect()
}

/// Concatenated text of every cell paragraph of a table JSON node.
fn table_text(t: &serde_json::Value) -> String {
    let mut s = String::new();
    for cell in t["cells"].as_array().unwrap() {
        for p in cell["paragraphs"].as_array().unwrap() {
            for ch in p["chars"].as_array().unwrap() {
                if let Some(c) = ch.get("Text").and_then(|v| v.as_str()) {
                    s.push_str(c);
                }
            }
        }
    }
    s
}

/// The fixture anchor "한빛대학교" sits in a top-level paragraph before every
/// table, so a clone inserted after it becomes the new table #0.
#[test]
fn clone_table_blank_after_anchor() {
    let src = copy_fixture("clone_blank.hwpx");
    let out = tmp("clone_blank_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--clone-table", "9=>한빛대학교", "--verify"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "blank clone: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let tables = all_tables_json(&out);
    assert_eq!(tables.len(), 11, "10 source tables + 1 clone");
    let clone = &tables[0];
    assert_eq!(clone["rows"].as_u64().unwrap(), 7);
    assert_eq!(clone["cols"].as_u64().unwrap(), 2);
    assert!(
        table_text(clone).is_empty(),
        "blank clone carries no source text"
    );
    // The source table (shifted to #10) is untouched.
    assert!(!table_text(&tables[10]).is_empty(), "source table kept");
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "blank clone keeps the document valid"
    );
}

/// Keep mode clones the merged 11x10 table #2 (with its 6 nested tables) —
/// geometry, merge topology, and content survive; instance ids are remapped
/// above the source maximum.
#[test]
fn clone_table_keep_preserves_content_and_geometry() {
    let src = copy_fixture("clone_keep.hwpx");
    let out = tmp("clone_keep_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--clone-table", "2=>한빛대학교=>keep", "--verify"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "keep clone: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let tables = all_tables_json(&out);
    assert_eq!(tables.len(), 17, "10 + clone with 6 nested tables");
    let (clone, source) = (&tables[0], &tables[9]);
    assert_eq!(clone["rows"].as_u64().unwrap(), 11);
    assert_eq!(clone["cols"].as_u64().unwrap(), 10);
    assert_eq!(clone["row_cell_counts"], source["row_cell_counts"]);
    assert_eq!(
        table_text(clone),
        table_text(source),
        "keep clone preserves content"
    );
    // (Instance-id remapping is HWP-only — the HWPX writer assigns element ids
    // itself; see clone_table_keep_hwp_instance_ids_remapped.)
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "keep clone keeps the document valid"
    );
}

/// HWP outputs carry paragraph instance ids; a keep clone must remap every id
/// above the document maximum, leaving the whole document collision-free.
#[test]
fn clone_table_keep_hwp_instance_ids_remapped() {
    let src = copy_fixture("clone_keep_hwp.hwpx");
    let hwp_src = tmp("clone_keep_src.hwp");
    let c = hwp()
        .arg("convert")
        .arg(&src)
        .arg("-o")
        .arg(&hwp_src)
        .output()
        .unwrap();
    assert!(
        c.status.success(),
        "fixture converts to hwp: {}",
        String::from_utf8_lossy(&c.stderr)
    );
    let out = tmp("clone_keep_hwp_out.hwp");
    let r = hwp()
        .arg("edit")
        .arg(&hwp_src)
        .arg("-o")
        .arg(&out)
        .args(["--clone-table", "2=>한빛대학교=>keep", "--verify"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "hwp keep clone: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    // Every paragraph instance id in the output is non-zero and unique.
    let cat = hwp()
        .arg("cat")
        .arg(&out)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let j: serde_json::Value = serde_json::from_slice(&cat.stdout).unwrap();
    let mut ids = Vec::new();
    fn walk_paras(paras: &serde_json::Value, ids: &mut Vec<u64>) {
        for p in paras.as_array().unwrap() {
            ids.push(p["header"]["instance_id"].as_u64().unwrap());
            for c in p["controls"].as_array().unwrap() {
                if let Some(t) = c.get("Table") {
                    for cell in t["cells"].as_array().unwrap() {
                        walk_paras(&cell["paragraphs"], ids);
                    }
                } else if let Some(g) = c.get("Generic") {
                    for l in g["paragraph_lists"].as_array().unwrap() {
                        walk_paras(&l["paragraphs"], ids);
                    }
                }
            }
        }
    }
    for section in j["sections"].as_array().unwrap() {
        walk_paras(&section["paragraphs"], &mut ids);
    }
    assert!(ids.iter().all(|&id| id != 0), "all ids assigned");
    let mut dedup = ids.clone();
    dedup.sort_unstable();
    dedup.dedup();
    assert_eq!(dedup.len(), ids.len(), "no instance id collisions");
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "hwp keep clone keeps the document valid"
    );
}

/// Two sequential clones keep working and keep ids rising.
#[test]
fn clone_table_multiple_sequential() {
    let src = copy_fixture("clone_seq.hwpx");
    let out1 = tmp("clone_seq_out1.hwpx");
    let r1 = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out1)
        .args(["--clone-table", "9=>한빛대학교", "--verify"])
        .output()
        .unwrap();
    assert!(
        r1.status.success(),
        "first clone: {}",
        String::from_utf8_lossy(&r1.stderr)
    );
    let out2 = tmp("clone_seq_out2.hwpx");
    let r2 = hwp()
        .arg("edit")
        .arg(&out1)
        .arg("-o")
        .arg(&out2)
        .args(["--clone-table", "0=>한빛대학교", "--verify"])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "second clone (of the first clone): {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    let tables = all_tables_json(&out2);
    assert_eq!(tables.len(), 12, "10 + 2 sequential clones");
    assert_eq!(tables[0]["rows"].as_u64().unwrap(), 7);
    assert_eq!(tables[1]["rows"].as_u64().unwrap(), 7);
}

/// Bad indices, missing anchors, and bad mode tokens fail and publish nothing.
#[test]
fn clone_table_spec_errors() {
    for spec in [
        "99=>한빛대학교",
        "0=>없는앵커문장",
        "0=>한빛대학교=>bogus",
        "x=>한빛대학교",
    ] {
        let src = copy_fixture("clone_err.hwpx");
        let out = tmp("clone_err_out.hwpx");
        let r = hwp()
            .arg("edit")
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .args(["--clone-table", spec])
            .output()
            .unwrap();
        assert!(!r.status.success(), "--clone-table {spec:?} must fail");
        assert!(!out.exists(), "--clone-table {spec:?} must not publish");
    }
}

/// Cloning on synthetic HWP and HWPX documents, both modes, then fill + verify.
#[test]
fn clone_table_synthetic_both_formats() {
    let md = tmp("clone_syn.md");
    std::fs::write(&md, "앵커\n\n| 가 | 나 |\n|----|----|\n| 1 | 2 |\n").unwrap();
    for ext in ["hwpx", "hwp"] {
        let form = tmp(&format!("clone_syn_form.{ext}"));
        assert!(
            hwp()
                .args(["new", "--from"])
                .arg(&md)
                .arg("-o")
                .arg(&form)
                .status()
                .unwrap()
                .success()
        );
        for mode in ["blank", "keep"] {
            let out = tmp(&format!("clone_syn_{mode}.{ext}"));
            let r = hwp()
                .arg("edit")
                .arg(&form)
                .arg("-o")
                .arg(&out)
                .args(["--clone-table", &format!("0=>앵커=>{mode}"), "--verify"])
                .output()
                .unwrap();
            assert!(
                r.status.success(),
                "{ext} {mode} clone: {}",
                String::from_utf8_lossy(&r.stderr)
            );
            assert!(
                hwp()
                    .arg("validate")
                    .arg(&out)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "{ext} {mode} clone stays valid"
            );
            // The clone (new table #0) is addressable by set-cell.
            let out2 = tmp(&format!("clone_syn_{mode}_fill.{ext}"));
            let r2 = hwp()
                .arg("edit")
                .arg(&out)
                .arg("-o")
                .arg(&out2)
                .args(["--set-cell", "0:0:0=복제셀"])
                .output()
                .unwrap();
            assert!(
                r2.status.success(),
                "{ext} {mode} set-cell into clone: {}",
                String::from_utf8_lossy(&r2.stderr)
            );
            let text = cat(&out2);
            assert!(text.contains("복제셀"), "{ext} {mode} clone cell filled");
            assert!(text.contains('1'), "{ext} {mode} source table kept");
        }
    }
}
