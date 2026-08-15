//! 같은 포맷 hwpx→hwpx `hwp edit`의 패키지 외과 수술 경로 통합 테스트 (epic #90 PR 3).
//!
//! 편집된 콘텐츠 엔트리(section/header/content.hpf)만 재직렬화되고 나머지 엔트리
//! (BinData·META-INF·미리보기·settings 등)는 입력과 바이트 동일해야 한다.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

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
    let dir = std::env::temp_dir().join(format!("hwp-cli-edit-surgical-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn copy_fixture(name: &str) -> PathBuf {
    let dst = tmp(name);
    std::fs::copy(fixture(), &dst).unwrap();
    dst
}

fn read_zip_entry(path: &Path, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut buf = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut buf).unwrap();
    buf
}

fn zip_entry_names(path: &Path) -> Vec<String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().to_string())
        .collect()
}

/// 외과 수술 계약: 비대상 엔트리가 입력과 바이트 동일해야 한다.
fn assert_entries_identical(src: &Path, out: &Path, names: &[&str]) {
    for name in names {
        assert_eq!(
            read_zip_entry(src, name),
            read_zip_entry(out, name),
            "{name} 바이트 보존"
        );
    }
}

/// hp:container(미해석 개체) + BinData + 미리보기 + META-INF 원본 바이트를 담은
/// 합성 HWPX. 컨테이너 보존은 커밋된 표 픽스처(컨테이너 없음)로는 검증할 수 없어
/// 직접 만든다.
fn build_container_fixture(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();
    zip.start_file("META-INF/container.rdf", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><rdf:RDF>SURGICAL-CLI-MARKER</rdf:RDF>"#)
        .unwrap();
    zip.start_file("Contents/content.hpf", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>컨테이너 문서</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/><opf:item id="layout" href="DocOptions/Layout.xml" media-type="application/xml"/><opf:item id="image1" href="BinData/image1.png" media-type="image/png" isEmbeded="1"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>앵커 문단</hp:t><hp:container id="77" zOrder="3"><hp:sz width="1000" height="500"/><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>컨테이너 텍스트</hp:t></hp:run></hp:p></hp:subList></hp:container><hp:pic id="5" zOrder="0"><hp:sz width="100" height="100"/><hp:pos treatAsChar="1" vertOffset="0" horzOffset="0"/><hp:img binaryItemIDRef="image1" bright="0" contrast="0"/></hp:pic></hp:run></hp:p></hs:sec>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("BinData/image1.png", deflated).unwrap();
    zip.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 1, 1, 1])
        .unwrap();
    zip.start_file("DocOptions/Layout.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><layout>DISTINCTIVE-DOCOPTIONS-MARKER</layout>"#)
        .unwrap();
    zip.start_file("Preview/PrvImage.png", deflated).unwrap();
    zip.write_all(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 9, 9, 9, 9])
        .unwrap();
    zip.start_file("settings.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><setting>CLI-SETTINGS-MARKER</setting>"#)
        .unwrap();
    zip.finish().unwrap();
}

fn tiny_png(path: &Path) {
    // 32x24 IHDR만 담은 최소 PNG (cli.rs의 insert-image 테스트와 같은 형태).
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(32u32.to_be_bytes());
    png.extend(24u32.to_be_bytes());
    png.extend([0u8; 8]);
    std::fs::write(path, &png).unwrap();
}

/// hp:container(미해석 개체)의 subList 안에 1×1 표를 담은 합성 HWPX.
/// 표 op이 컨테이너 안 표를 바꾸면 원문 XML이 낡으므로 fail-closed로 거부돼야 한다.
fn build_container_table_fixture(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.start_file("version.xml", deflated).unwrap();
    zip.write_all(br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#)
        .unwrap();
    zip.start_file("Contents/content.hpf", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0"?><opf:package xmlns:opf="http://www.idpf.org/2007/opf/"><opf:metadata><opf:title>컨테이너 표 문서</opf:title></opf:metadata><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="settings" href="settings.xml" media-type="application/xml"/></opf:manifest><opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("Contents/header.xml", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0" encoding="UTF-8"?><hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head"/>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("Contents/section0.xml", deflated).unwrap();
    zip.write_all(
        r##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>앵커 문단</hp:t><hp:container id="78" zOrder="3"><hp:sz width="2000" height="1000"/><hp:subList><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:tbl id="9" zOrder="0" pageBreak="NONE" repeatHeader="0" rowCnt="1" colCnt="1" cellSpacing="0" borderFillIDRef="0" noAdjust="0"><hp:sz width="1000" height="500"/><hp:pos treatAsChar="1" affectLSpacing="0" flowWithText="0" holdAnchorAndSO="0" vertRelTo="PARA" horzRelTo="PARA" vertAlign="TOP" horzAlign="LEFT" vertOffset="0" horzOffset="0"/><hp:outMargin left="0" right="0" top="0" bottom="0"/><hp:inMargin left="0" right="0" top="0" bottom="0"/><hp:tr><hp:tc name="" header="0" hasMargin="0" protect="0" editable="0" dirty="0" borderFillIDRef="0"><hp:subList id="" textDirection="HORIZONTAL" lineWrap="BREAK" vertAlign="CENTER" linkListIDRef="0" linkListNextIDRef="0" textWidth="0" textHeight="0" hasTextRef="0" hasNumRef="0"><hp:p id="0" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="0"><hp:t>컨테이너 셀</hp:t></hp:run></hp:p></hp:subList><hp:cellAddr colAddr="0" rowAddr="0"/><hp:cellSpan colSpan="1" rowSpan="1"/><hp:cellSz width="1000" height="500"/><hp:cellMargin left="0" right="0" top="0" bottom="0"/></hp:tc></hp:tr></hp:tbl></hp:run></hp:p></hp:subList></hp:container></hp:run></hp:p></hs:sec>"##
            .as_bytes(),
    )
    .unwrap();
    zip.start_file("settings.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><setting>CLI-SETTINGS-MARKER</setting>"#)
        .unwrap();
    zip.finish().unwrap();
}

/// 표 op(set-cell): 본문만 재직렬화되고 header/content.hpf/미리보기/META-INF는
/// 입력과 바이트 동일해야 한다(전체 재작성이었다면 header.xml이 재합성된다).
#[test]
fn table_op_preserves_non_target_entries() {
    let src = copy_fixture("surgical_tbl.hwpx");
    let out = tmp("surgical_tbl_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-cell", "9:0:0=외과수술셀"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "set-cell: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    assert_entries_identical(
        &src,
        &out,
        &[
            "Contents/header.xml",
            "Contents/content.hpf",
            "Preview/PrvImage.png",
            "Preview/PrvText.txt",
            "META-INF/container.rdf",
            "META-INF/container.xml",
            "META-INF/manifest.xml",
            "settings.xml",
            "version.xml",
        ],
    );
    let section = String::from_utf8(read_zip_entry(&out, "Contents/section0.xml")).unwrap();
    assert!(section.contains("외과수술셀"), "셀 편집 반영");
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "validate 통과"
    );
}

/// 메타데이터 op(set-meta): content.hpf만 재생성되고 header/미리보기/META-INF는
/// 바이트 동일. BinData가 없는 문서라 엔트리 수도 같아야 한다.
#[test]
fn meta_op_regenerates_only_content_hpf() {
    let src = copy_fixture("surgical_meta.hwpx");
    let out = tmp("surgical_meta_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-meta", "title=외과수술 제목"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "set-meta: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    assert_entries_identical(
        &src,
        &out,
        &[
            "Contents/header.xml",
            "Preview/PrvImage.png",
            "META-INF/container.rdf",
            "settings.xml",
        ],
    );
    assert_eq!(
        zip_entry_names(&src).len(),
        zip_entry_names(&out).len(),
        "엔트리 수 불변"
    );
    let hpf = String::from_utf8(read_zip_entry(&out, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains("<opf:title>외과수술 제목</opf:title>"),
        "메타데이터 반영: {hpf}"
    );
}

/// 그림 삽입(insert-image): 새 BinData 엔트리가 추가되고 content.hpf 매니페스트가
/// 갱신되며, 기존 엔트리는 바이트 보존. 제자리(in-place) 편집도 성공해야 한다.
#[test]
fn image_insert_appends_bindata() {
    let md = tmp("surgical_img.md");
    std::fs::write(&md, "이미지 앵커: 본문\n").unwrap();
    let base = tmp("surgical_img_base.hwpx");
    let new = hwp()
        .args(["new", "--from"])
        .arg(&md)
        .arg("-o")
        .arg(&base)
        .output()
        .unwrap();
    assert!(
        new.status.success(),
        "합성 문서 생성: {}",
        String::from_utf8_lossy(&new.stderr)
    );
    let image = tmp("surgical_img.png");
    tiny_png(&image);

    let out = tmp("surgical_img_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&base)
        .arg("-o")
        .arg(&out)
        .arg("--insert-image")
        .arg(format!("이미지 앵커:=>{}", image.display()))
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "insert-image: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    let base_names = zip_entry_names(&base);
    let out_names = zip_entry_names(&out);
    assert!(
        !base_names.iter().any(|n| n.starts_with("BinData/")),
        "원본은 BinData 없음"
    );
    assert!(
        out_names.iter().any(|n| n == "BinData/image1.png"),
        "새 BinData 엔트리 추가: {out_names:?}"
    );
    assert_eq!(
        read_zip_entry(&out, "BinData/image1.png"),
        std::fs::read(&image).unwrap(),
        "삽입 이미지 바이트"
    );
    // 신규 항목이 매니페스트에 등록됐다.
    let hpf = String::from_utf8(read_zip_entry(&out, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains(r#"id="image1" href="BinData/image1.png""#),
        "매니페스트 갱신: {hpf}"
    );
    // 비대상 엔트리는 바이트 보존.
    assert_entries_identical(&base, &out, &["Contents/header.xml", "settings.xml"]);
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "validate 통과"
    );

    // 제자리 편집(입력=출력 경로)도 snapshot 덕분에 안전하게 성공해야 한다.
    let inplace = tmp("surgical_img_inplace.hwpx");
    std::fs::copy(&base, &inplace).unwrap();
    let r = hwp()
        .arg("edit")
        .arg(&inplace)
        .arg("-o")
        .arg(&inplace)
        .arg("--insert-image")
        .arg(format!("이미지 앵커:=>{}", image.display()))
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "in-place insert-image: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(
        zip_entry_names(&inplace)
            .iter()
            .any(|n| n == "BinData/image1.png"),
        "in-place 결과에도 새 BinData 존재"
    );
}

/// --replace와 표 op를 섞으면 고속 경로가 아니라 IR 외과 수술 경로를 타야 하고,
/// 그래도 비대상 엔트리는 바이트 보존이어야 한다.
#[test]
fn mixed_replace_and_table_op_stays_surgical() {
    let src = copy_fixture("surgical_mixed.hwpx");
    let out = tmp("surgical_mixed_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args([
            "--replace",
            "한빛대학교=>검증대학교",
            "--set-cell",
            "9:0:0=혼합편집",
        ])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "mixed edit: {}",
        String::from_utf8_lossy(&r.stderr)
    );
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        !stderr.contains("패키지 보존"),
        "혼합 편집은 replace 고속 경로가 아니어야: {stderr}"
    );

    assert_entries_identical(
        &src,
        &out,
        &[
            "Contents/header.xml",
            "Preview/PrvImage.png",
            "META-INF/container.rdf",
            "settings.xml",
        ],
    );
    let section = String::from_utf8(read_zip_entry(&out, "Contents/section0.xml")).unwrap();
    assert!(section.contains("검증대학교"), "치환 반영");
    assert!(section.contains("혼합편집"), "셀 편집 반영");
}

/// 미해석 개체(hp:container)를 담은 문서의 IR 외과 수술: 컨테이너 원문 XML과
/// 기존 그림 참조가 유지되고 불투명 엔트리는 바이트 보존.
#[test]
fn container_and_picture_survive_surgical_edit() {
    let src = tmp("surgical_container.hwpx");
    build_container_fixture(&src);
    let out = tmp("surgical_container_out.hwpx");

    // IR의 컨테이너 개수를 세는 도우미 (cat JSON에서 ctrl_id "cont" = [99,111,110,116]).
    // 모든 Generic을 세면 안 된다 — 이 합성 픽스처는 secPr가 없어 writer가 기본
    // 구역 정의를 주입하고, 그 colPr가 Generic(cold)으로 다시 읽힌다(전체 재작성
    // 경로도 동일한 기존 동작).
    fn collect_generic_ids(paras: &serde_json::Value, out: &mut Vec<Vec<u8>>) {
        for p in paras.as_array().unwrap() {
            for c in p["controls"].as_array().unwrap() {
                if let Some(g) = c.get("Generic") {
                    let id: Vec<u8> = g["ctrl_id"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_u64().unwrap() as u8)
                        .collect();
                    out.push(id);
                    for l in g["paragraph_lists"].as_array().unwrap() {
                        collect_generic_ids(&l["paragraphs"], out);
                    }
                } else if let Some(t) = c.get("Table") {
                    for cell in t["cells"].as_array().unwrap() {
                        collect_generic_ids(&cell["paragraphs"], out);
                    }
                }
            }
        }
    }
    let container_count = |path: &Path| {
        let cat = hwp()
            .arg("cat")
            .arg(path)
            .args(["--format", "json"])
            .output()
            .unwrap();
        let j: serde_json::Value = serde_json::from_slice(&cat.stdout).unwrap();
        let mut ids = Vec::new();
        for section in j["sections"].as_array().unwrap() {
            collect_generic_ids(&section["paragraphs"], &mut ids);
        }
        ids.iter().filter(|id| id.as_slice() == b"cont").count()
    };
    let before = container_count(&src);
    assert!(before >= 1, "픽스처에 컨테이너 존재");

    // set-meta는 본문과 무관하지만, 외과 수술 경로에서 본문은 항상 재직렬화된다 —
    // 이 때 컨테이너 원문(hwpx_raw_xml)이 유지돼야 한다.
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-meta", "title=컨테이너 편집 후"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "set-meta: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    assert_eq!(container_count(&out), before, "컨테이너 수 불변");
    let section = String::from_utf8(read_zip_entry(&out, "Contents/section0.xml")).unwrap();
    assert!(
        section.contains(r#"<hp:container id="77" zOrder="3">"#),
        "컨테이너 원문 유지: {section}"
    );
    assert!(
        section.contains(r#"binaryItemIDRef="image1""#),
        "기존 그림 원본 id 유지: {section}"
    );
    assert_entries_identical(
        &src,
        &out,
        &[
            "BinData/image1.png",
            "Preview/PrvImage.png",
            "META-INF/container.rdf",
            "settings.xml",
            "Contents/header.xml",
        ],
    );
}

/// 컨테이너(hp:container) 안의 표를 겨냥한 표 op은 컨테이너 원문 XML을 낡게 만든다.
/// writer는 컨테이너를 재방출할 emitter가 없으므로 OpaqueControlUnrepresentable을
/// 기록하고, fail-closed 보존 검사가 편집을 거부해야 한다(조용한 성공 = 편집 유실이
/// 진짜 버그). 거부되면 출력은 게시되지 않고 입력도 그대로여야 한다.
#[test]
fn table_op_inside_opaque_container_fails_closed() {
    let src = tmp("surgical_container_tbl.hwpx");
    build_container_table_fixture(&src);
    let src_bytes = std::fs::read(&src).unwrap();
    let out = tmp("surgical_container_tbl_out.hwpx");

    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-cell", "0:0:0=변경시도"])
        .output()
        .unwrap();
    assert!(
        !r.status.success(),
        "컨테이너 안 표 편집은 거부돼야 한다: {}",
        String::from_utf8_lossy(&r.stdout)
    );
    let stderr = String::from_utf8_lossy(&r.stderr);
    assert!(
        stderr.contains("보존 불가") && stderr.contains("opaque_control_unrepresentable"),
        "fail-closed 보존 오류여야 한다: {stderr}"
    );
    assert!(!out.exists(), "거부된 편집은 출력을 게시하지 않는다");
    assert_eq!(
        std::fs::read(&src).unwrap(),
        src_bytes,
        "입력 파일은 그대로여야 한다"
    );
}

/// 확장 파트(DocOptions/Layout.xml)가 원본 OPF 매니페스트에 등재된 문서의 set-meta:
/// content.hpf 재생성 후에도 확장 파트 항목이 매니페스트에 남아야 한다(엔트리는
/// raw-copy되는데 등재가 사라지면 고아 파트가 된다).
#[test]
fn meta_edit_keeps_extension_manifest_items() {
    let src = tmp("surgical_ext_item.hwpx");
    build_container_fixture(&src);
    let out = tmp("surgical_ext_item_out.hwpx");

    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-meta", "title=확장 파트 편집"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "set-meta: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    let hpf = String::from_utf8(read_zip_entry(&out, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains(
            r#"<opf:item id="layout" href="DocOptions/Layout.xml" media-type="application/xml"/>"#
        ),
        "재생성 매니페스트에 확장 파트 항목 유지: {hpf}"
    );
    assert_entries_identical(&src, &out, &["DocOptions/Layout.xml"]);
}

/// 표 복제 op(clone-table, #78): section0.xml만 재직렬화되고 header/content.hpf/
/// 미리보기/META-INF/settings는 입력과 바이트 동일해야 한다(keep 모드는 BinData를
/// 새로 추가하지 않고 기존 참조를 재사용한다).
#[test]
fn clone_table_preserves_non_target_entries() {
    let src = copy_fixture("surgical_clone.hwpx");
    let out = tmp("surgical_clone_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--clone-table", "2=>한빛대학교=>keep"])
        .output()
        .unwrap();
    assert!(
        r.status.success(),
        "clone-table: {}",
        String::from_utf8_lossy(&r.stderr)
    );

    assert_entries_identical(
        &src,
        &out,
        &[
            "Contents/header.xml",
            "Contents/content.hpf",
            "Preview/PrvImage.png",
            "Preview/PrvText.txt",
            "META-INF/container.rdf",
            "META-INF/container.xml",
            "META-INF/manifest.xml",
            "settings.xml",
            "version.xml",
        ],
    );
    // Entry set is unchanged — keep mode reuses BinData references.
    assert_eq!(
        zip_entry_names(&src),
        zip_entry_names(&out),
        "엔트리 집합 동일(BinData 재사용)"
    );
    // The cloned table landed in the section entry (clone = 1 + 6 nested tables).
    let count_tbls = |path: &Path| {
        String::from_utf8(read_zip_entry(path, "Contents/section0.xml"))
            .unwrap()
            .matches("<hp:tbl")
            .count()
    };
    assert_eq!(
        count_tbls(&out),
        count_tbls(&src) + 7,
        "clone adds 7 tables (1 + 6 nested)"
    );
    assert!(
        hwp()
            .arg("validate")
            .arg(&out)
            .output()
            .unwrap()
            .status
            .success(),
        "validate 통과"
    );
}
