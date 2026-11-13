//! Integration tests for same-format HWPX hwp edit surgical rewriting (epic #90 PR 3).
//!
//! Only edited content entries (section/header/content.hpf) are reserialized; all other
//! entries, including BinData/META-INF/previews/settings, must remain byte-identical.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

fn fixture() -> PathBuf {
    let p =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx");
    assert!(p.exists(), "커밋된 픽스처가 없습니다: {}", p.display());
    p
}

fn tmp(name: &str) -> PathBuf {
    // Include the PID: concurrent cargo test sessions on the same machine otherwise
    // overwrite artifacts at a fixed path and cause observed flakes.
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
        .map(|i| zip.by_index(i).unwrap().name().unwrap().into_owned())
        .collect()
}

/// Surgical contract: non-target entries remain byte-identical to the input.
fn assert_entries_identical(src: &Path, out: &Path, names: &[&str]) {
    for name in names {
        assert_eq!(
            read_zip_entry(src, name),
            read_zip_entry(out, name),
            "{name} 바이트 보존"
        );
    }
}

/// Synthetic HWPX containing captured hp:container XML, BinData, preview and original META-INF.
/// The committed table fixture lacks containers, so it cannot establish container
/// preservation; build a dedicated fixture here.
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
    // Minimal PNG containing only a 32x24 IHDR, matching cli.rs insert-image tests.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0, 0, 0, 13]);
    png.extend(b"IHDR");
    png.extend(32u32.to_be_bytes());
    png.extend(24u32.to_be_bytes());
    png.extend([0u8; 8]);
    std::fs::write(path, &png).unwrap();
}

/// Synthetic HWPX with a 1x1 table in an unparsed hp:container subList.
/// Editing that table invalidates captured XML and must fail closed.
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

/// Table set-cell: reserialize only the body; header/content.hpf/previews/META-INF remain
/// byte-identical, unlike a full rewrite that would synthesize header.xml again.
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

/// #220: set-cell values containing blank lines create multiple paragraphs but remain nonstructural;
/// the same entry set as the single-block case above must remain byte-identical.
#[test]
fn multi_paragraph_table_op_preserves_non_target_entries() {
    let src = copy_fixture("surgical_tbl_multi.hwpx");
    let out = tmp("surgical_tbl_multi_out.hwpx");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-cell", "9:0:0=첫째 블록\n\n둘째 블록"])
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
    assert!(section.contains("첫째 블록"), "첫 블록 반영");
    assert!(section.contains("둘째 블록"), "둘째 블록 반영");
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

/// Metadata set-meta regenerates only content.hpf; header/previews/META-INF remain
/// byte-identical. With no BinData, the number of entries also stays unchanged.
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

/// insert-image adds a BinData entry and updates the content.hpf manifest while preserving
/// existing entry bytes. In-place editing must also succeed.
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
    // The new item is registered in the manifest.
    let hpf = String::from_utf8(read_zip_entry(&out, "Contents/content.hpf")).unwrap();
    assert!(
        hpf.contains(r#"id="image1" href="BinData/image1.png""#),
        "매니페스트 갱신: {hpf}"
    );
    // Preserve non-target entry bytes.
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

    // Snapshotting must also make in-place editing (identical input/output path) safe.
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

/// Combining --replace with a table operation must use surgical IR rewriting instead of the fast path,
/// while still preserving non-target entry bytes.
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

/// Surgical rewriting with an unparsed hp:container retains captured XML and existing
/// picture references, and preserves opaque entry bytes.
#[test]
fn container_and_picture_survive_surgical_edit() {
    let src = tmp("surgical_container.hwpx");
    build_container_fixture(&src);
    let out = tmp("surgical_container_out.hwpx");

    // Count IR containers: cat JSON represents ctrl_id cont as [99,111,110,116].
    // Do not count every Generic: this synthetic fixture lacks secPr, so the writer injects
    // a default section definition whose colPr rereads as Generic(cold). The full rewrite
    // path has the same existing behavior.
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

    // Although set-meta does not edit the body, the surgical path always reserializes it;
    // captured container XML (hwpx_raw_xml) must survive this step.
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

/// A table operation inside hp:container makes its captured XML stale. The writer
/// cannot re-emit that container and records OpaqueControlUnrepresentable; the
/// fail-closed preservation check must refuse editing, rather than silently lose it.
/// Refusal must leave both the unpublished output and input untouched.
#[test]
fn table_op_inside_opaque_container_fails_closed() {
    let src = tmp("surgical_container_tbl.hwpx");
    build_container_table_fixture(&src);
    let src_bytes = std::fs::read(&src).unwrap();
    let out = tmp("surgical_container_tbl_out.hwpx");

    let report = tmp("surgical_container_tbl_report.json");
    let r = hwp()
        .arg("edit")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--set-cell", "0:0:0=변경시도", "--report"])
        .arg(&report)
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
    // #531: the refused run still writes edit-report-v1 with `preservation_loss` and the
    // event codes as values.
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("refused run writes the report"))
            .unwrap();
    assert_eq!(report["refusal"]["code"], "preservation_loss", "{report}");
    let events = report["refusal"]["values"]["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["code"] == "opaque_control_unrepresentable"),
        "{report}"
    );
}

/// set-meta with an extension part (DocOptions/Layout.xml) in the original OPF manifest:
/// after content.hpf regeneration the extension item must remain listed, avoiding an
/// orphaned part whose bytes were raw-copied but whose manifest item disappeared.
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

/// clone-table (#78) reserializes only section0.xml; header/content.hpf/previews/META-INF/
/// settings remain byte-identical. Keep mode adds no new BinData and reuses existing
/// references.
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
