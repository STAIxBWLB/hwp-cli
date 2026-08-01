//! `hwp` CLI 통합 테스트 — validate 종료코드 계약 (소비자가 exit code로 판정).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

/// fixture 바이너리는 저장소에서 제외된다(로컬 전용). 없으면 `true`(스킵).
fn skip_if_no_fixtures() -> bool {
    if fixture("hwpx/minimal.hwpx").exists() {
        return false;
    }
    eprintln!("스킵: fixtures 없음 — fixtures/README.md 참고");
    true
}

#[test]
fn validate_valid_hwpx_exit_zero() {
    if skip_if_no_fixtures() {
        return;
    }
    let out = hwp()
        .arg("validate")
        .arg(fixture("hwpx/minimal.hwpx"))
        .output()
        .expect("hwp 실행");
    assert!(
        out.status.success(),
        "유효 hwpx는 exit 0 (stderr: {})",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn validate_corrupt_exit_nonzero_json() {
    let bad = std::env::temp_dir().join("hwp_cli_bad.hwpx");
    std::fs::write(&bad, b"this is not a valid hwp/hwpx file").unwrap();

    let out = hwp()
        .args(["validate", "--json"])
        .arg(&bad)
        .output()
        .expect("hwp 실행");
    assert!(!out.status.success(), "손상 파일은 비-0 종료");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"valid\": false") || stdout.contains("\"valid\":false"),
        "JSON에 valid:false (실제: {stdout})"
    );

    let _ = std::fs::remove_file(&bad);
}

#[test]
fn slots_json_shape() {
    // 합성 템플릿을 만들고 slots --json 구조 확인 (placeholders 배열).
    let document = tmp("hwp_cli_slots.hwpx");
    // hwp new로 {{name}}을 본문에 담은 hwpx 생성.
    let md = tmp("hwp_cli_slots.md");
    std::fs::write(&md, "{{기관명}} 본문 {{제목}}\n").unwrap();
    let mk = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&document)
        .output()
        .expect("hwp new");
    assert!(
        mk.status.success(),
        "hwp new: {}",
        String::from_utf8_lossy(&mk.stderr)
    );

    let out = hwp()
        .args(["slots", "--json"])
        .arg(&document)
        .output()
        .expect("hwp slots");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("placeholders"), "placeholders 키");
    assert!(
        stdout.contains("기관명") && stdout.contains("제목"),
        "자리표시자 이름"
    );

    let _ = std::fs::remove_file(&document);
    let _ = std::fs::remove_file(&md);
}

fn tmp(name: &str) -> PathBuf {
    // PID 포함 — 동시 cargo test 실행 간 산출물 충돌(플레이크) 방지.
    let dir = std::env::temp_dir().join(format!("hwp-cli-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn replace_zip_entry(path: &Path, target: &str, replacement: &[u8]) {
    let rewritten = path.with_extension("rewrite.hwpx");
    let input = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(input).unwrap();
    let output = std::fs::File::create(&rewritten).unwrap();
    let mut writer = zip::ZipWriter::new(output);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let method = if name == "mimetype" {
            zip::CompressionMethod::Stored
        } else {
            entry.compression()
        };
        writer
            .start_file(
                &name,
                zip::write::SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        if name == target {
            writer.write_all(replacement).unwrap();
        } else {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            writer.write_all(&bytes).unwrap();
        }
    }
    writer.finish().unwrap();
    std::fs::rename(rewritten, path).unwrap();
}

#[test]
fn new_rejects_unsupported_output_extension() {
    let unsupported = tmp("hwp_cli_new_unsupported.txt");
    let out = hwp()
        .arg("new")
        .arg("-o")
        .arg(&unsupported)
        .output()
        .expect("hwp new");
    assert!(!out.status.success(), "지원하지 않는 확장자는 실패해야");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("지원하지 않는 출력 확장자")
            && stderr.contains(".hwp")
            && stderr.contains(".hwpx"),
        "지원 확장자를 안내해야: {stderr}"
    );
    assert!(
        !unsupported.exists(),
        "실패한 명령은 출력 파일을 만들면 안 됨"
    );
}

#[test]
fn new_rejects_output_without_extension() {
    let no_extension = tmp("hwp_cli_new_no_extension");
    let out = hwp()
        .arg("new")
        .arg("-o")
        .arg(&no_extension)
        .output()
        .expect("hwp new");
    assert!(!out.status.success(), "확장자가 없는 출력은 실패해야");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("출력 파일에 확장자가 없습니다")
            && stderr.contains(".hwp")
            && stderr.contains(".hwpx"),
        "지원 확장자를 안내해야: {stderr}"
    );
    assert!(
        !no_extension.exists(),
        "실패한 명령은 출력 파일을 만들면 안 됨"
    );
}

#[test]
fn new_failure_preserves_existing_destination() {
    let invalid = tmp("hwp_cli_new_invalid.json");
    let destination = tmp("hwp_cli_new_existing.hwpx");
    std::fs::write(&invalid, b"{not valid json").unwrap();
    std::fs::write(&destination, b"EXISTING DESTINATION").unwrap();

    let result = hwp()
        .args(["new", "--from"])
        .arg(&invalid)
        .arg("-o")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"EXISTING DESTINATION"
    );

    for path in [&invalid, &destination] {
        let _ = std::fs::remove_file(path);
    }
}

fn assert_no_edit_staging_debris(path: &std::path::Path) {
    let parent = path.parent().unwrap();
    let file_name = path.file_name().unwrap().to_string_lossy();
    let prefix = format!(".{file_name}.hwp-output-");
    let leftovers: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(&prefix).then_some(name)
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "edit 임시 작업공간 잔재: {leftovers:?}"
    );
}

#[test]
fn edit_verify_rejects_json_and_markdown_before_write() {
    let md = tmp("hwp_cli_verify_text_source.md");
    let src = tmp("hwp_cli_verify_text_source.hwpx");
    std::fs::write(&md, "초안 본문\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success()
    );

    for extension in ["json", "md", "markdown"] {
        let fresh = tmp(&format!("hwp_cli_verify_fresh.{extension}"));
        let _ = std::fs::remove_file(&fresh);
        let rejected = hwp()
            .arg("edit")
            .arg(&src)
            .arg("-o")
            .arg(&fresh)
            .args(["--replace", "초안=>최종", "--verify"])
            .output()
            .unwrap();
        assert!(
            !rejected.status.success(),
            "{extension} --verify는 거부해야"
        );
        let stderr = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            stderr.contains("--verify는 HWP/HWPX 출력에서만 지원")
                && stderr.contains("JSON/Markdown"),
            "명확한 지원 범위 안내가 필요: {stderr}"
        );
        assert!(!fresh.exists(), "거부된 새 출력이 생기면 안 됨");
        assert_no_edit_staging_debris(&fresh);

        let existing = tmp(&format!("hwp_cli_verify_existing.{extension}"));
        std::fs::write(&existing, b"EXISTING DESTINATION").unwrap();
        let rejected = hwp()
            .arg("edit")
            .arg(&src)
            .arg("-o")
            .arg(&existing)
            .args(["--replace", "초안=>최종", "--verify"])
            .output()
            .unwrap();
        assert!(
            !rejected.status.success(),
            "기존 {extension}도 --verify를 거부해야"
        );
        assert_eq!(
            std::fs::read(&existing).unwrap(),
            b"EXISTING DESTINATION",
            "기존 목적지는 바이트 그대로 보존"
        );
        assert_no_edit_staging_debris(&existing);
        std::fs::remove_file(existing).unwrap();
    }

    for path in [&md, &src] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn edit_hwpx_verify_publishes_only_verified_stage() {
    let md = tmp("hwp_cli_atomic_verify_source.md");
    let src = tmp("hwp_cli_atomic_verify_source.hwpx");
    let destination = tmp("hwp_cli_atomic_verify_destination.hwpx");
    std::fs::write(&md, "초안 본문\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&destination, b"EXISTING DESTINATION").unwrap();

    let edited = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&destination)
        .args(["--replace", "초안=>최종", "--verify"])
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "검증 후 게시 성공: {}",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert!(
        String::from_utf8_lossy(&edited.stderr).contains("검증: 재읽기 OK"),
        "게시 전 재읽기 검증 실행"
    );
    let cat = hwp().arg("cat").arg(&destination).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(
        cat.status.success() && text.contains("최종"),
        "편집 결과: {text}"
    );
    assert_no_edit_staging_debris(&destination);

    for path in [&md, &src, &destination] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn edit_hwpx_in_place_is_safe() {
    let md = tmp("hwp_cli_atomic_in_place.md");
    let document = tmp("hwp_cli_atomic_in_place.hwpx");
    std::fs::write(&md, "초안 제자리 편집\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&document)
            .status()
            .unwrap()
            .success()
    );

    let edited = hwp()
        .arg("edit")
        .arg(&document)
        .arg("-o")
        .arg(&document)
        .args(["--replace", "초안=>최종", "--verify"])
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "제자리 편집 성공: {}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let cat = hwp().arg("cat").arg(&document).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(
        cat.status.success() && text.contains("최종") && !text.contains("초안"),
        "제자리 편집 결과: {text}"
    );
    assert_no_edit_staging_debris(&document);

    for path in [&md, &document] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn edit_zero_match_preserves_destination_and_allow_partial_is_explicit() {
    let md = tmp("hwp_cli_zero_match.md");
    let src = tmp("hwp_cli_zero_match_source.hwpx");
    let destination = tmp("hwp_cli_zero_match_destination.hwpx");
    std::fs::write(&md, "존재하는 본문\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&destination, b"EXISTING DESTINATION").unwrap();

    let rejected = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&destination)
        .args(["--replace", "존재하는=>변경한", "--replace", "없는=>실패"])
        .output()
        .unwrap();
    assert!(
        !rejected.status.success(),
        "부분 일치는 기본적으로 실패해야"
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"EXISTING DESTINATION"
    );

    let allowed = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&destination)
        .args([
            "--replace",
            "존재하는=>변경한",
            "--replace",
            "없는=>실패",
            "--allow-partial",
        ])
        .output()
        .unwrap();
    assert!(
        allowed.status.success(),
        "명시적 부분 적용: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
    let cat = hwp().arg("cat").arg(&destination).output().unwrap();
    assert!(String::from_utf8_lossy(&cat.stdout).contains("변경한 본문"));

    for path in [&md, &src, &destination] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn edit_preview_only_match_is_not_an_applied_request() {
    let md = tmp("hwp_cli_preview_only.md");
    let source = tmp("hwp_cli_preview_only_source.hwpx");
    let destination = tmp("hwp_cli_preview_only_destination.hwpx");
    std::fs::write(&md, "본문에는 대상이 없습니다\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    replace_zip_entry(&source, "Preview/PrvText.txt", "미리보기전용".as_bytes());
    std::fs::write(&destination, b"EXISTING DESTINATION").unwrap();

    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&destination)
        .args(["--replace", "미리보기전용=>바뀐미리보기"])
        .output()
        .unwrap();
    assert!(!result.status.success(), "미리보기만 일치하면 실패해야");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("적용되지 않은 편집 요청"),
        "본문 미일치 진단: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"EXISTING DESTINATION"
    );
    assert_no_edit_staging_debris(&destination);

    for path in [&md, &source, &destination] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn edit_cancelling_replacements_preserve_destination() {
    let md = tmp("hwp_cli_cancel.md");
    let source = tmp("hwp_cli_cancel_source.hwpx");
    let destination = tmp("hwp_cli_cancel_destination.hwpx");
    std::fs::write(&md, "갑 본문\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&destination, b"EXISTING DESTINATION").unwrap();

    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&destination)
        .args(["--replace", "갑=>을", "--replace", "을=>갑"])
        .output()
        .unwrap();
    assert!(!result.status.success(), "최종 의미가 상쇄되면 실패해야");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("최종 결과가 원문과 같아"),
        "상쇄 진단: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"EXISTING DESTINATION"
    );

    for path in [&md, &source, &destination] {
        let _ = std::fs::remove_file(path);
    }
}

/// 실패 진단용: 파일의 cat 출력(본문+stderr)과 info(스트림 크기)를 덤프한다
/// (Windows CI 전용 결함 추적 — write 측 산출물이 비었는지, read 측이 잃는지 구분).
fn dump_file(path: &PathBuf) -> String {
    let cat = hwp().arg("cat").arg(path).output().unwrap();
    let info = hwp().args(["info", "--json"]).arg(path).output().unwrap();
    format!(
        "cat_stdout={:?} cat_stderr={:?} info={}",
        String::from_utf8_lossy(&cat.stdout),
        String::from_utf8_lossy(&cat.stderr),
        String::from_utf8_lossy(&info.stdout)
            .chars()
            .take(400)
            .collect::<String>()
    )
}

#[test]
fn cat_with_header_footer_hidden_flags() {
    // 합성 문서로 cat 텍스트 추출 옵션 플래그가 파싱되고 본문을 출력하는지(스모크).
    let md = tmp("hwp_cli_cat_flags.md");
    std::fs::write(&md, "본문 텍스트입니다\n").unwrap();
    let src = tmp("hwp_cli_cat_flags.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success()
    );
    // plain + 두 플래그.
    let out = hwp()
        .arg("cat")
        .arg(&src)
        .args(["--with-header-footer", "--with-hidden"])
        .output()
        .expect("hwp cat");
    assert!(
        out.status.success(),
        "cat 플래그 실행 성공 (stderr: {})",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("본문 텍스트입니다"),
        "본문 출력"
    );
    // markdown 경로에도 플래그가 유효해야 한다.
    let md_out = hwp()
        .arg("cat")
        .arg(&src)
        .args(["--format", "markdown", "--with-hidden"])
        .output()
        .expect("hwp cat md");
    assert!(md_out.status.success(), "cat markdown 플래그 실행");
    assert!(
        String::from_utf8_lossy(&md_out.stdout).contains("본문 텍스트입니다"),
        "markdown 본문 출력"
    );
    for f in [&md, &src] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn convert_html_has_title_from_metadata() {
    let md = tmp("hwp_cli_html.md");
    std::fs::write(&md, "# 본문 제목\n\n내용\n").unwrap();
    let src = tmp("hwp_cli_html.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .args(["--set-meta", "title=메타 제목"])
            .status()
            .unwrap()
            .success()
    );
    let out = tmp("hwp_cli_html.html");
    assert!(
        hwp()
            .arg("convert")
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .args(["--to", "html"])
            .status()
            .unwrap()
            .success()
    );
    let html = std::fs::read_to_string(&out).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "html 헤더");
    assert!(
        html.contains("<title>메타 제목</title>"),
        "메타데이터 제목이 <title>에: {}",
        &html[..html.len().min(200)]
    );
    for f in [&md, &src, &out] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn convert_pdf_embeds_image_xobject() {
    if skip_if_no_fixtures() {
        return;
    }
    // 이미지 있는 fixture → PDF는 %PDF- 헤더 + Image XObject (폰트 비의존).
    let out = tmp("hwp_cli_img.pdf");
    let status = hwp()
        .arg("convert")
        .arg(fixture("hwp5/annual_report.hwp"))
        .arg("-o")
        .arg(&out)
        .args(["--to", "pdf"])
        .status()
        .unwrap();
    assert!(status.success(), "convert pdf");
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF-"), "PDF 헤더");
    assert!(
        bytes.windows(6).any(|w| w == b"/Image"),
        "Image XObject 임베드"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn new_metadata_then_info_json() {
    let md = tmp("hwp_cli_meta.md");
    std::fs::write(&md, "본문\n").unwrap();
    let src = tmp("hwp_cli_meta.hwp");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .args(["--set-meta", "title=제목X", "--set-meta", "author=지은이Y"])
            .status()
            .unwrap()
            .success()
    );
    let out = hwp().args(["info", "--json"]).arg(&src).output().unwrap();
    let j = String::from_utf8_lossy(&out.stdout);
    assert!(
        j.contains("제목X") && j.contains("지은이Y"),
        "메타데이터: {j}"
    );
    for f in [&md, &src] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn convert_odt_mimetype_first() {
    if skip_if_no_fixtures() {
        return;
    }
    let out = tmp("hwp_cli.odt");
    assert!(
        hwp()
            .arg("convert")
            .arg(fixture("hwpx/minimal.hwpx"))
            .arg("-o")
            .arg(&out)
            .args(["--to", "odt"])
            .status()
            .unwrap()
            .success()
    );
    let bytes = std::fs::read(&out).unwrap();
    // ODF: 첫 엔트리는 STORED mimetype. zip local header(30B) 직후 파일명 "mimetype".
    assert_eq!(&bytes[0..2], b"PK", "zip");
    assert!(
        bytes.windows(8).take(64).any(|w| w == b"mimetype"),
        "mimetype 첫 엔트리"
    );
    assert!(
        bytes
            .windows(39)
            .any(|w| w == b"application/vnd.oasis.opendocument.text"),
        "ODT mimetype 값"
    );
    let _ = std::fs::remove_file(&out);
}

#[test]
fn strict_fails_on_dropped_controls() {
    if skip_if_no_fixtures() {
        return;
    }
    // annual_report의 hwp→hwpx는 이제 무드롭(도형 전부 지원: rect/line/ellipse/arc/polygon).
    // 드롭 발생 경로는 역방향(hwpx→hwp) — hwpx-출신 장식 도형은 hwp5 SHAPE_COMPONENT
    // 정합 역합성을 안 하고 strip으로 드롭한다. --strict면 그 드롭에서 비정상 종료.
    let mid = tmp("hwp_cli_strict.hwpx");
    let fwd = hwp()
        .arg("convert")
        .arg(fixture("hwp5/annual_report.hwp"))
        .arg("-o")
        .arg(&mid)
        .args(["--to", "hwpx"])
        .status()
        .unwrap();
    assert!(fwd.success(), "hwp→hwpx는 무드롭으로 성공");

    let dst = tmp("hwp_cli_strict.hwp");
    std::fs::write(&dst, b"EXISTING DESTINATION").unwrap();
    let strict = hwp()
        .arg("convert")
        .arg(&mid)
        .arg("-o")
        .arg(&dst)
        .arg("--strict")
        .output()
        .unwrap();
    assert!(
        !strict.status.success(),
        "역방향 장식 도형 드롭 시 --strict면 비정상 종료"
    );
    assert!(
        String::from_utf8_lossy(&strict.stderr).contains("strict"),
        "strict 사유 출력"
    );
    assert_eq!(
        std::fs::read(&dst).unwrap(),
        b"EXISTING DESTINATION",
        "strict 실패는 기존 목적지를 바꾸면 안 됨"
    );
    let _ = std::fs::remove_file(&mid);
    let _ = std::fs::remove_file(&dst);
}

#[test]
fn fill_replaces_slots() {
    let md = tmp("hwp_cli_fill.md");
    std::fs::write(&md, "{{수신}} 귀하\n").unwrap();
    let tpl = tmp("hwp_cli_fill_tpl.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&tpl)
            .status()
            .unwrap()
            .success()
    );
    let out = tmp("hwp_cli_fill_out.hwpx");
    let r = hwp()
        .arg("fill")
        .arg(&tpl)
        .arg("-o")
        .arg(&out)
        .args(["--set", "수신=홍길동", "--json"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "fill: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let j = String::from_utf8_lossy(&r.stdout);
    assert!(j.contains("\"replaced\""), "replaced 키: {j}");
    let filled = hwp().arg("cat").arg(&out).output().unwrap();
    assert!(
        String::from_utf8_lossy(&filled.stdout).contains("홍길동"),
        "치환 결과"
    );
    for f in [&md, &tpl, &out] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn fill_parts_splices_part_blocks() {
    // 템플릿: 제목 + {{본문}} 앵커 문단. 부분: md 산문 + HTML 병합 표.
    let tpl_md = tmp("hwp_cli_fillparts_tpl.md");
    std::fs::write(&tpl_md, "# 보고서\n\n{{본문}}\n").unwrap();
    let tpl = tmp("hwp_cli_fillparts_tpl.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&tpl_md)
            .arg("-o")
            .arg(&tpl)
            .status()
            .unwrap()
            .success()
    );
    let part_md = tmp("hwp_cli_fillparts_part.md");
    std::fs::write(
        &part_md,
        "부분 본문입니다.\n\n<table>\n<tr><td colspan=\"2\">가로병합</td></tr>\n\
         <tr><td>a</td><td>b</td></tr>\n</table>\n",
    )
    .unwrap();
    let out = tmp("hwp_cli_fillparts_out.hwpx");
    let r = hwp()
        .arg("fill")
        .arg(&tpl)
        .arg("-o")
        .arg(&out)
        .args(["--set"])
        .arg(format!("본문=@{}", part_md.display()))
        .args(["--json"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "fill parts: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    // 텍스트와 표 구조(colspan)가 이식됐는지 확인.
    let cat = hwp().arg("cat").arg(&out).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(text.contains("부분 본문입니다."), "부분 텍스트: {text}");
    let html = hwp()
        .args(["cat", "--format", "html"])
        .arg(&out)
        .output()
        .unwrap();
    let html = String::from_utf8_lossy(&html.stdout);
    assert!(html.contains("colspan=\"2\""), "부분 표 span: {html}");
    for f in [&tpl_md, &tpl, &part_md, &out] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn fill_parts_missing_anchor_fails() {
    let tpl_md = tmp("hwp_cli_fillparts_miss_tpl.md");
    std::fs::write(&tpl_md, "# 보고서\n\n다른 내용\n").unwrap();
    let tpl = tmp("hwp_cli_fillparts_miss_tpl.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&tpl_md)
            .arg("-o")
            .arg(&tpl)
            .status()
            .unwrap()
            .success()
    );
    let part_md = tmp("hwp_cli_fillparts_miss_part.md");
    std::fs::write(&part_md, "부분\n").unwrap();
    let out = tmp("hwp_cli_fillparts_miss_out.hwpx");
    let r = hwp()
        .arg("fill")
        .arg(&tpl)
        .arg("-o")
        .arg(&out)
        .args(["--set"])
        .arg(format!("본문=@{}", part_md.display()))
        .output()
        .unwrap();
    assert!(!r.status.success(), "앵커 없음은 실패해야 한다");
    assert!(!out.exists(), "실패 시 목적지를 만들지 않는다");
    for f in [&tpl_md, &tpl, &part_md] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn fill_zero_or_partial_match_preserves_destination_by_default() {
    let md = tmp("hwp_cli_fill_strict.md");
    let template = tmp("hwp_cli_fill_strict_template.hwpx");
    let destination = tmp("hwp_cli_fill_strict_destination.hwpx");
    std::fs::write(&md, "{{수신}} 귀하\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&template)
            .status()
            .unwrap()
            .success()
    );

    for sets in [
        vec!["없는키=값"],
        vec!["수신=홍길동", "없는키=값"],
        vec!["수신={{수신}}"],
    ] {
        std::fs::write(&destination, b"EXISTING DESTINATION").unwrap();
        let mut command = hwp();
        command
            .arg("fill")
            .arg(&template)
            .arg("-o")
            .arg(&destination);
        for set in sets {
            command.args(["--set", set]);
        }
        let result = command.output().unwrap();
        assert!(
            !result.status.success(),
            "0건/부분/미해결 치환은 기본 실패: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"EXISTING DESTINATION",
            "검증 실패는 기존 목적지를 보존"
        );
    }

    let partial = hwp()
        .arg("fill")
        .arg(&template)
        .arg("-o")
        .arg(&destination)
        .args([
            "--set",
            "수신=홍길동",
            "--set",
            "없는키=값",
            "--allow-partial",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        partial.status.success(),
        "명시적 부분 치환: {}",
        String::from_utf8_lossy(&partial.stderr)
    );
    let report = String::from_utf8_lossy(&partial.stdout);
    assert!(
        report.contains("\"warnings\"") && report.contains("없는키"),
        "기계 판독 경고: {report}"
    );

    for path in [&md, &template, &destination] {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
#[test]
fn fill_rejects_alias_and_special_destination_without_touching_source() {
    use std::os::unix::fs::symlink;

    let md = tmp("hwp_cli_fill_alias.md");
    let template = tmp("hwp_cli_fill_alias_template.hwpx");
    let alias = tmp("hwp_cli_fill_alias_link.hwpx");
    let special = tmp("hwp_cli_fill_special.hwpx");
    std::fs::write(&md, "{{수신}} 귀하\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&template)
            .status()
            .unwrap()
            .success()
    );
    let before = std::fs::read(&template).unwrap();
    let _ = std::fs::remove_file(&alias);
    symlink(&template, &alias).unwrap();
    let aliased = hwp()
        .arg("fill")
        .arg(&template)
        .arg("-o")
        .arg(&alias)
        .args(["--set", "수신=홍길동"])
        .output()
        .unwrap();
    assert!(!aliased.status.success(), "심볼릭 링크 목적지는 거부");
    assert_eq!(std::fs::read(&template).unwrap(), before);

    let _ = std::fs::remove_dir_all(&special);
    std::fs::create_dir(&special).unwrap();
    let directory = hwp()
        .arg("fill")
        .arg(&template)
        .arg("-o")
        .arg(&special)
        .args(["--set", "수신=홍길동"])
        .output()
        .unwrap();
    assert!(!directory.status.success(), "디렉터리 목적지는 거부");
    assert_eq!(std::fs::read(&template).unwrap(), before);

    let _ = std::fs::remove_file(&alias);
    let _ = std::fs::remove_dir(&special);
    for path in [&md, &template] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn edit_add_row_then_fill() {
    // 양식(2행 표) → 행 3개 추가(pass 1) → 추가 행 셀 채움(pass 2) → hwp5. cat으로 확인.
    // edit 순서상 구조편집(add-row)은 set-cell 뒤에 적용되므로 두 번에 나눠 호출한다.
    let md = tmp("hwp_cli_addrow.md");
    std::fs::write(&md, "| 품목 | 수량 |\n|------|------|\n| | |\n").unwrap();
    let form = tmp("hwp_cli_addrow_form.hwp");
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
    // pass 1: 행 3개 추가
    let rows = tmp("hwp_cli_addrow_rows.hwp");
    let r1 = hwp()
        .arg("edit")
        .arg(&form)
        .arg("-o")
        .arg(&rows)
        .args(["--add-row", "0", "--add-row", "0", "--add-row", "0"])
        .output()
        .unwrap();
    assert!(
        r1.status.success(),
        "edit --add-row: {} | form.hwp: {}",
        String::from_utf8_lossy(&r1.stderr),
        dump_file(&form)
    );
    // pass 2: 추가된 행 셀 채움
    let out = tmp("hwp_cli_addrow_out.hwp");
    let r2 = hwp()
        .arg("edit")
        .arg(&rows)
        .arg("-o")
        .arg(&out)
        .args([
            "--set-cell",
            "0:1:0=노트북",
            "--set-cell",
            "0:3:0=키보드",
            "--verify",
        ])
        .output()
        .unwrap();
    assert!(
        r2.status.success(),
        "edit --set-cell: {}",
        String::from_utf8_lossy(&r2.stderr)
    );
    let cat = hwp().arg("cat").arg(&out).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(
        text.contains("노트북") && text.contains("키보드"),
        "내용: {text}"
    );
    for f in [&md, &form, &rows, &out] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn edit_merge_and_column_ops() {
    // 3열 표 → 셀 병합(상단 3칸)·열 추가·열 삭제를 한 번에 → hwpx. validate + 재읽기로 확인.
    let md = tmp("hwp_cli_merge.md");
    std::fs::write(&md, "| 가 | 나 | 다 |\n|----|----|----|\n| 1 | 2 | 3 |\n").unwrap();
    let form = tmp("hwp_cli_merge_form.hwpx");
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
    let out = tmp("hwp_cli_merge_out.hwpx");
    // 병합(0,0)-(0,2), 열 추가(위치1), 열 삭제(열3). 편집 순서: 병합→분할→추가→삭제.
    let r = hwp()
        .arg("edit")
        .arg(&form)
        .arg("-o")
        .arg(&out)
        .args([
            "--merge-cells",
            "0:0:0:0:2",
            "--add-col",
            "0:1",
            "--delete-col",
            "0:3",
            "--verify",
        ])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "edit 병합/열조작: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let val = hwp().arg("validate").arg(&out).output().unwrap();
    assert!(
        String::from_utf8_lossy(&val.stdout).contains("유효"),
        "validate: {}",
        String::from_utf8_lossy(&val.stdout)
    );
    for f in [&md, &form, &out] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn edit_verify_checks_format_alignment_fields_bookmarks_and_links() {
    let md = tmp("hwp_cli_verify_semantics.md");
    let source = tmp("hwp_cli_verify_semantics_source.hwpx");
    let output = tmp("hwp_cli_verify_semantics_output.hwpx");
    std::fs::write(&md, "제목\n\n참조:\n").unwrap();
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    let edited = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args([
            "--set-format",
            "제목:bold=on,size=16,color=#112233",
            "--set-align",
            "제목=center",
            "--create-field",
            "참조:=>수신=홍길동",
            "--create-bookmark",
            "참조:=>참조점",
            "--create-hyperlink",
            "참조:=>사이트=>https://example.com/path?a=1",
            "--verify",
        ])
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "operation-specific verify: {}",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert!(String::from_utf8_lossy(&edited.stderr).contains("검증: 재읽기 OK"));

    for path in [&md, &source, &output] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn fill_data_tables_grows() {
    // 데이터 구동: --data tables 로 표를 데이터 수만큼 자동 증식 + 채움.
    let md = tmp("hwp_cli_filltab.md");
    std::fs::write(&md, "| 품목 | 수량 |\n|------|------|\n| | |\n").unwrap();
    let form = tmp("hwp_cli_filltab_form.hwp");
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
    let data = tmp("hwp_cli_filltab.json");
    std::fs::write(
        &data,
        r#"{"tables":[{"table":0,"start_row":1,"rows":[["사과","3"],["배","7"],["감","9"]]}]}"#,
    )
    .unwrap();
    let out = tmp("hwp_cli_filltab_out.hwp");
    let r = hwp()
        .arg("fill")
        .arg(&form)
        .arg("-o")
        .arg(&out)
        .arg("--data")
        .arg(&data)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "fill --data tables: {} | form.hwp: {}",
        String::from_utf8_lossy(&r.stderr),
        dump_file(&form)
    );
    let j = String::from_utf8_lossy(&r.stdout);
    assert!(j.contains("\"rows_added\""), "rows_added 키: {j}");
    let cat = hwp().arg("cat").arg(&out).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(
        text.contains("사과") && text.contains("배") && text.contains("감"),
        "데이터 채움: {text}"
    );
    for f in [&md, &form, &data, &out] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn fill_literal_tables_key_not_misrouted() {
    // 최상위 "tables"가 (표 지시 객체가 아닌) 문자열 배열이면 평문 자리표시자 치환으로
    // 라우팅돼야 한다(IR 표 채우기로 오인 → "rows 배열 필요" 오류 금지).
    let md = tmp("hwp_cli_litkey.md");
    std::fs::write(&md, "{{tables}} 목록\n").unwrap();
    let tpl = tmp("hwp_cli_litkey.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&tpl)
            .status()
            .unwrap()
            .success()
    );
    let data = tmp("hwp_cli_litkey.json");
    std::fs::write(&data, r#"{"tables":["사과","배"]}"#).unwrap();
    let out = tmp("hwp_cli_litkey_out.hwpx");
    let r = hwp()
        .arg("fill")
        .arg(&tpl)
        .arg("-o")
        .arg(&out)
        .arg("--data")
        .arg(&data)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "flat tables 키 치환: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let j = String::from_utf8_lossy(&r.stdout);
    assert!(
        j.contains("\"replaced\""),
        "평문 fill 경로(replaced 키): {j}"
    );
    for f in [&md, &tpl, &data, &out] {
        let _ = std::fs::remove_file(f);
    }
}

/// ★글상자 보존 기함 테스트: work_report.hwp의 글상자(gso) 안 텍스트와 %hlk 하이퍼링크가
/// hwp→hwpx 변환에서 살아남는다 — 이전엔 글상자가 통째로 드롭돼 둘 다 소실(⑪의 알려진 한계).
#[test]
fn 변환_글상자_텍스트_필드_보존() {
    if skip_if_no_fixtures() {
        return;
    }
    let src = fixture("hwp5/work_report.hwp");
    if !src.exists() {
        eprintln!("스킵: work_report.hwp 없음");
        return;
    }
    let out = tmp("hwp_cli_textbox.hwpx");
    let r = hwp()
        .arg("convert")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(!stderr.contains("DROP"), "드롭 경고가 없어야: {stderr}");

    // 글상자 안 텍스트 생존.
    let cat = hwp().arg("cat").arg(&out).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(text.contains("나눔글꼴"), "글상자 텍스트 보존: {text}");

    // 글상자 안 %hlk 하이퍼링크 생존.
    let fields = hwp().args(["fields", "--json"]).arg(&out).output().unwrap();
    let j = String::from_utf8_lossy(&fields.stdout);
    assert!(j.contains("%hlk"), "글상자 안 하이퍼링크 보존: {j}");
    assert!(j.contains("설치하기"), "하이퍼링크 표시값 보존: {j}");

    let _ = std::fs::remove_file(&out);
}

/// ★도형 보존: annual_report(디자인 문서, 도형 142개)의 hwp→hwpx 변환에서 장식 도형이
/// 보존된다 — 이전엔 76개가 통째로 드롭. 잔여 드롭(ARC/이미지채움 v1 제외)은 소수만 허용.
#[test]
fn 변환_장식_도형_보존() {
    if skip_if_no_fixtures() {
        return;
    }
    let src = fixture("hwp5/annual_report.hwp");
    if !src.exists() {
        eprintln!("스킵: annual_report.hwp 없음");
        return;
    }
    let out = tmp("hwp_cli_shapes.hwpx");
    let r = hwp()
        .arg("convert")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let stderr = String::from_utf8_lossy(&r.stderr);
    let drops = stderr.matches("DROP").count();
    assert!(
        drops <= 8,
        "도형 드롭이 소수여야(이전 76): {drops}건\n{stderr}"
    );

    // 텍스트(글상자 포함)는 원본과 동일하게 추출돼야 한다.
    let cat_hwp = hwp().arg("cat").arg(&src).output().unwrap();
    let cat_hwpx = hwp().arg("cat").arg(&out).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&cat_hwp.stdout),
        String::from_utf8_lossy(&cat_hwpx.stdout),
        "hwpx 텍스트 추출이 원본 hwp와 동일해야"
    );

    // ★도형 z-order 보존(㉗): 예전엔 전부 zOrder="0"으로 뭉개 한글이 겹친 도형을
    // undefined 순서로 그려 표지가 빈 화면이 됐다. 원본 gso z-order(고유 1~143)를
    // 실값으로 방출하는지 확인 — zOrder 값이 다수 고유해야(전부 0 회귀 방지).
    let xml = std::process::Command::new("unzip")
        .args(["-p"])
        .arg(&out)
        .arg("Contents/section0.xml")
        .output()
        .unwrap()
        .stdout;
    let xml = String::from_utf8_lossy(&xml);
    let zorders: std::collections::HashSet<&str> = xml
        .match_indices("zOrder=\"")
        .map(|(i, _)| {
            let rest = &xml[i + 8..];
            &rest[..rest.find('"').unwrap_or(0)]
        })
        .collect();
    assert!(
        zorders.len() >= 20,
        "도형 zOrder가 다수 고유해야(전부 0 회귀 방지): 고유값 {}종 = {:?}",
        zorders.len(),
        zorders
    );

    let _ = std::fs::remove_file(&out);
}

/// ★완전 왕복 기함: work_report.hwp → hwpx → hwp — 글상자 텍스트·%hlk 하이퍼링크가
/// 양방향 변환을 모두 살아남는다. 역방향(hwpx→hwp)의 gso는 한글 실기 손상 판정으로
/// ㉕에서 안전 저하(글상자 텍스트를 본문으로 보존, 도형 래퍼 생략) — 도형 자체는
/// 왕복에서 유지되지 않으나 텍스트·필드는 보존되고 파일은 유효하다(DROP 없음).
#[test]
fn 변환_완전_왕복_hwp_hwpx_hwp() {
    if skip_if_no_fixtures() {
        return;
    }
    let src = fixture("hwp5/work_report.hwp");
    if !src.exists() {
        eprintln!("스킵: work_report.hwp 없음");
        return;
    }
    let mid = tmp("hwp_cli_rt.hwpx");
    let dst = tmp("hwp_cli_rt.hwp");
    for (i, o) in [(&src, &mid), (&mid.clone(), &dst)] {
        let r = hwp()
            .arg("convert")
            .arg(i)
            .arg("-o")
            .arg(o)
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let stderr = String::from_utf8_lossy(&r.stderr);
        assert!(!stderr.contains("DROP"), "드롭 없어야: {stderr}");
    }

    // 텍스트(글상자 포함) 완전 동일.
    let cat_a = hwp().arg("cat").arg(&src).output().unwrap();
    let cat_b = hwp().arg("cat").arg(&dst).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&cat_a.stdout),
        String::from_utf8_lossy(&cat_b.stdout),
        "왕복 후 텍스트 동일해야"
    );

    // 글상자 안 하이퍼링크 생존.
    let fields = hwp().args(["fields", "--json"]).arg(&dst).output().unwrap();
    let j = String::from_utf8_lossy(&fields.stdout);
    assert!(j.contains("%hlk"), "왕복 후 %hlk 보존: {j}");
    assert!(j.contains("설치하기"), "하이퍼링크 표시값 보존: {j}");

    let _ = std::fs::remove_file(&mid);
    let _ = std::fs::remove_file(&dst);
}

/// markdown 변환 --media-dir: 합성 hwpx에 이미지를 삽입하고 안전한 링크·충돌 거부를 검증한다.
#[test]
fn convert_md_media_dir_figs() {
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hwp_cli_figs_{}_{}", std::process::id(), uniq));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source_md = dir.join("source.md");
    let base = dir.join("base.hwpx");
    let image_doc = dir.join("image.hwpx");
    let image = dir.join("tiny.png");
    std::fs::write(&source_md, "이미지 앵커: 본문\n").unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(32u32.to_be_bytes());
    png.extend(24u32.to_be_bytes());
    png.extend([0u8; 8]);
    std::fs::write(&image, &png).unwrap();

    let new = hwp()
        .args(["new", "--from"])
        .arg(&source_md)
        .arg("-o")
        .arg(&base)
        .output()
        .unwrap();
    assert!(
        new.status.success(),
        "합성 문서 생성: {}",
        String::from_utf8_lossy(&new.stderr)
    );
    let edit = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&image_doc)
        .arg("--insert-image")
        .arg(format!("이미지 앵커:=>{}", image.display()))
        .output()
        .unwrap();
    assert!(
        edit.status.success(),
        "이미지 삽입: {}",
        String::from_utf8_lossy(&edit.stderr)
    );

    let nested = dir.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let out = nested.join("report.md");

    let r = hwp()
        .arg("convert")
        .arg(&image_doc)
        .arg("-o")
        .arg(&out)
        .args(["--media-dir", "my figs"])
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));

    let md = std::fs::read_to_string(&out).unwrap();
    assert!(
        md.contains("![image](<my figs/image1.png>)"),
        "공백 포함 media 경로 링크: {}",
        &md[..md.len().min(400)]
    );
    let figs = nested.join("my figs");
    assert!(figs.is_dir(), "figs 디렉터리가 출력 옆에 생성");
    let extracted = figs.join("image1.png");
    assert_eq!(std::fs::read(&extracted).unwrap(), png, "이미지 바이트");

    std::fs::write(&extracted, b"do not overwrite").unwrap();
    let collision = hwp()
        .arg("convert")
        .arg(&image_doc)
        .arg("-o")
        .arg(&out)
        .args(["--media-dir", "my figs"])
        .output()
        .unwrap();
    assert!(!collision.status.success(), "다른 기존 파일이면 실패");
    assert_eq!(
        std::fs::read(&extracted).unwrap(),
        b"do not overwrite",
        "기존 파일을 덮어쓰지 않음"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// (8) `cat --format markdown --with-segments` 봉투 — 커밋 픽스처(report-tables.hwpx)로
/// skip 없이. (a) markdown 필드가 무옵션 출력과 동일, (b) 세그먼트 정렬·비중첩·end<=문자수,
/// (c) 모든 (section,para)가 IR sections[].paragraphs[] 범위 내, (d) 논-ASCII로 byte != char.
#[test]
fn cat_with_segments_envelope() {
    let fix = fixture("samples/report-tables.hwpx");
    let seg = hwp()
        .arg("cat")
        .arg(&fix)
        .args(["--format", "markdown", "--with-segments"])
        .output()
        .expect("hwp cat --with-segments");
    assert!(
        seg.status.success(),
        "with-segments 실행: {}",
        String::from_utf8_lossy(&seg.stderr)
    );
    // 봉투는 한 줄 컴팩트 JSON.
    assert_eq!(
        seg.stdout.iter().filter(|&&b| b == b'\n').count(),
        1,
        "봉투는 개행 1개(한 줄 + 끝 개행)"
    );
    let env: serde_json::Value = serde_json::from_slice(&seg.stdout).expect("JSON 봉투 파싱");
    let md = env["markdown"].as_str().expect("markdown 필드");

    // (a) markdown == 무옵션 markdown 출력.
    let plain = hwp()
        .arg("cat")
        .arg(&fix)
        .args(["--format", "markdown"])
        .output()
        .expect("hwp cat markdown");
    let plain_md = String::from_utf8(plain.stdout).expect("utf8");
    assert_eq!(md, plain_md, "markdown 필드가 무옵션 출력과 동일해야");

    // (d) 논-ASCII(한국어) → byte 길이 != 문자 수.
    assert_ne!(md.len(), md.chars().count(), "한국어 포함이면 byte != char");

    // (b) 정렬·비중첩·end<=문자수, kind=para.
    let n = md.chars().count();
    let segs = env["segments"].as_array().expect("segments 배열");
    assert!(!segs.is_empty(), "세그먼트가 있어야");
    let mut prev_end = 0usize;
    for s in segs {
        let start = s["start"].as_u64().unwrap() as usize;
        let end = s["end"].as_u64().unwrap() as usize;
        assert_eq!(s["kind"], "para", "kind=para");
        assert!(start < end && end <= n, "범위: [{start},{end}) n={n}");
        assert!(start >= prev_end, "정렬·비중첩: {start} < {prev_end}");
        prev_end = end;
    }

    // (c) 모든 (section,para)가 IR 범위 내.
    let ir_out = hwp()
        .arg("cat")
        .arg(&fix)
        .args(["--format", "json"])
        .output()
        .expect("hwp cat json");
    let ir: serde_json::Value = serde_json::from_slice(&ir_out.stdout).expect("IR JSON");
    let sections = ir["sections"].as_array().expect("sections");
    for s in segs {
        let sec = s["section"].as_u64().unwrap() as usize;
        let par = s["para"].as_u64().unwrap() as usize;
        let paras = sections
            .get(sec)
            .and_then(|x| x["paragraphs"].as_array())
            .unwrap_or_else(|| panic!("section {sec} 범위 밖"));
        assert!(
            par < paras.len(),
            "para {par} in sec {sec} 범위 밖(len {})",
            paras.len()
        );
    }
}

/// (9) 플래그 오용: `--with-segments --format json` 은 비정상 종료 + 에러 메시지.
#[test]
fn with_segments_rejects_non_markdown() {
    let fix = fixture("samples/report-tables.hwpx");
    let out = hwp()
        .arg("cat")
        .arg(&fix)
        .args(["--format", "json", "--with-segments"])
        .output()
        .expect("hwp cat");
    assert!(!out.status.success(), "json + with-segments는 실패해야");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("with-segments") || err.contains("markdown"),
        "에러 메시지에 사유: {err}"
    );
}

/// (10) 표 행 한 줄 불변식(픽스처 전수): 커밋 픽스처(항상) + 존재 시 로컬 hwp5/hwpx 픽스처.
/// 각 markdown 출력에서 줄마다 `<tr` 수 == `</tr>` 수, trim 후 '|' 시작 줄은 '|' 종료.
/// report-tables.hwpx는 `<tr>` 총수 37로 회귀 고정(중첩 표 포함 — 픽스처 갱신 시 함께 갱신).
#[test]
fn table_rows_single_line_all_fixtures() {
    let mut targets = vec![fixture("samples/report-tables.hwpx")];
    // 로컬 전용 픽스처가 있으면 포함(없으면 조용히 건너뜀 — 기존 skip 관례).
    for rel in ["hwp5", "hwpx"] {
        if let Ok(entries) = std::fs::read_dir(fixture(rel)) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                    x.eq_ignore_ascii_case("hwp") || x.eq_ignore_ascii_case("hwpx")
                }) {
                    targets.push(p);
                }
            }
        }
    }

    for path in &targets {
        let out = hwp()
            .arg("cat")
            .arg(path)
            .args(["--format", "markdown"])
            .output()
            .expect("hwp cat markdown");
        // 로컬 픽스처는 파싱 실패(DRM 등)할 수 있으므로 실패 시 건너뛴다.
        // 커밋 픽스처(report-tables)는 아래 회귀 핀이 성공을 강제한다.
        if !out.status.success() {
            continue;
        }
        let md = String::from_utf8_lossy(&out.stdout);
        for (i, line) in md.lines().enumerate() {
            assert_eq!(
                line.matches("<tr").count(),
                line.matches("</tr>").count(),
                "표 행 한 줄 위반 {}:{}: {line:?}",
                path.display(),
                i + 1
            );
            let t = line.trim();
            if t.starts_with('|') {
                assert!(
                    t.ends_with('|'),
                    "파이프 행이 '|'로 끝나야 {}:{}: {line:?}",
                    path.display(),
                    i + 1
                );
            }
        }
    }

    // report-tables.hwpx 회귀 핀: `<tr>` 총수 == 37 (중첩 표 포함 — 픽스처 갱신 시 함께 갱신).
    let rt = hwp()
        .arg("cat")
        .arg(fixture("samples/report-tables.hwpx"))
        .args(["--format", "markdown"])
        .output()
        .expect("hwp cat markdown");
    assert!(rt.status.success(), "report-tables cat 성공");
    let md = String::from_utf8_lossy(&rt.stdout);
    assert_eq!(
        md.matches("<tr").count(),
        37,
        "report-tables `<tr>` 총수 회귀 핀"
    );
}

/// 최소 유효 PNG(시그니처+IHDR)를 만든다 — image_pixel_size가 치수를 읽고
/// writer가 바이트를 그대로 임베드한다(디코딩은 하지 않음).
fn write_min_png(path: &std::path::Path, w: u32, h: u32) {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(w.to_be_bytes());
    png.extend(h.to_be_bytes());
    png.extend([8, 6, 0, 0, 0]); // bit depth/color type 등
    png.extend([0, 0, 0, 0]); // CRC 자리(검증 안 함)
    std::fs::write(path, &png).unwrap();
}

/// ★도장 날인(GM-7): edit --seal 로 앵커 "(인)" 위에 부유 그림을 얹고, hwpx 저장·
/// 재읽기에서 Picture가 살아있으며 validate가 통과한다. hwp5 저장 경로도 왕복 스모크.
#[test]
fn edit_seal_floating_image_roundtrip() {
    let md = tmp("hwp_cli_seal.md");
    std::fs::write(&md, "결재 (인) 란\n").unwrap();
    let src = tmp("hwp_cli_seal_src.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success(),
        "hwp new"
    );
    let png = tmp("hwp_cli_seal.png");
    write_min_png(&png, 100, 50);

    // hwpx 경로: 앵커 위에 도장 부유 배치(18mm).
    let out_hwpx = tmp("hwp_cli_seal_out.hwpx");
    let seal_arg = format!("(인)=>{}@18mm", png.display());
    let ed = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out_hwpx)
        .args(["--seal", &seal_arg, "--verify"])
        .output()
        .expect("hwp edit --seal");
    assert!(
        ed.status.success(),
        "edit --seal 성공 (stderr: {})",
        String::from_utf8_lossy(&ed.stderr)
    );

    // validate 통과(구조 유효).
    assert!(
        hwp()
            .arg("validate")
            .arg(&out_hwpx)
            .status()
            .unwrap()
            .success(),
        "도장 삽입 hwpx는 validate 통과"
    );

    // 재읽기 IR(JSON)에 Picture 컨트롤이 존재.
    let cj = hwp()
        .arg("cat")
        .args(["--format", "json"])
        .arg(&out_hwpx)
        .output()
        .expect("cat json");
    assert!(cj.status.success(), "cat json 성공");
    let j = String::from_utf8_lossy(&cj.stdout);
    assert!(j.contains("Picture"), "재읽기 IR에 Picture 존재: {j:.200}");

    // 앵커 텍스트는 유지되어야 한다.
    let ct = hwp().arg("cat").arg(&out_hwpx).output().expect("cat");
    assert!(
        String::from_utf8_lossy(&ct.stdout).contains("(인)"),
        "앵커 텍스트 유지"
    );

    // hwp5 저장 경로 왕복 스모크(합성 규격 준수 — 재읽기가 성공).
    let out_hwp = tmp("hwp_cli_seal_out.hwp");
    let seal_arg2 = format!("(인)=>{}", png.display()); // 기본 20mm
    let ed5 = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out_hwp)
        .args(["--seal", &seal_arg2])
        .output()
        .expect("hwp edit --seal hwp");
    assert!(
        ed5.status.success(),
        "hwp5 저장 성공 (stderr: {})",
        String::from_utf8_lossy(&ed5.stderr)
    );
    let ct5 = hwp().arg("cat").arg(&out_hwp).output().expect("cat hwp");
    assert!(ct5.status.success(), "hwp5 재읽기 성공");
    assert!(
        String::from_utf8_lossy(&ct5.stdout).contains("(인)"),
        "hwp5 왕복 후 앵커 유지 | out.hwp: {}",
        dump_file(&out_hwp)
    );

    for f in [&md, &src, &png, &out_hwpx, &out_hwp] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn edit_replace_and_seal_applies_both_edits() {
    let md = tmp("hwp_cli_replace_seal.md");
    std::fs::write(&md, "초안 결재 (인) 란\n").unwrap();
    let src = tmp("hwp_cli_replace_seal_src.hwpx");
    assert!(
        hwp()
            .args(["new", "--from"])
            .arg(&md)
            .arg("-o")
            .arg(&src)
            .status()
            .unwrap()
            .success(),
        "hwp new"
    );
    let png = tmp("hwp_cli_replace_seal.png");
    write_min_png(&png, 100, 50);
    let out_hwpx = tmp("hwp_cli_replace_seal_out.hwpx");
    let seal_arg = format!("(인)=>{}", png.display());

    let edited = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out_hwpx)
        .args(["--replace", "초안=>최종", "--seal", &seal_arg])
        .output()
        .expect("hwp edit --replace --seal");
    assert!(
        edited.status.success(),
        "replace+seal 성공 (stderr: {})",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&edited.stderr).contains("치환(패키지 보존)"),
        "seal 요청이 있으면 replace-only 고속 경로를 사용하면 안 됨"
    );

    let text = hwp().arg("cat").arg(&out_hwpx).output().expect("cat");
    assert!(text.status.success(), "결과 본문 재읽기");
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(
        stdout.contains("최종") && !stdout.contains("초안"),
        "텍스트 치환 적용: {stdout}"
    );

    let json = hwp()
        .arg("cat")
        .args(["--format", "json"])
        .arg(&out_hwpx)
        .output()
        .expect("cat json");
    assert!(json.status.success(), "결과 IR 재읽기");
    assert!(
        String::from_utf8_lossy(&json.stdout).contains("Picture"),
        "도장 Picture가 결과에 있어야"
    );

    for f in [&md, &src, &png, &out_hwpx] {
        let _ = std::fs::remove_file(f);
    }
}
