//! HWPX ZIP 자원 제한 및 bounded package I/O 회귀 테스트.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use hwpx::{
    HwpxPackage, NATIVE_PACKAGE_LIMITS, NATIVE_PACKAGE_LIMITS_PROFILE,
    NATIVE_PACKAGE_LIMITS_PROFILE_NAME, PackageLimits,
};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn temp_file(label: &str) -> PathBuf {
    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hwpx_limits_{label}_{}_{}.hwpx",
        std::process::id(),
        id
    ))
}

fn write_zip(path: &Path, entries: &[(&str, &[u8], CompressionMethod)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    for (name, data, method) in entries {
        let options = SimpleFileOptions::default().compression_method(*method);
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn valid_bmp(width: u32, height: u32) -> Vec<u8> {
    let row_bytes = (width * 3).div_ceil(4) * 4;
    let image_bytes = row_bytes * height;
    let file_bytes = 54 + image_bytes;
    let mut data = Vec::with_capacity(file_bytes as usize);
    data.extend_from_slice(b"BM");
    data.extend_from_slice(&file_bytes.to_le_bytes());
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(&54u32.to_le_bytes());
    data.extend_from_slice(&40u32.to_le_bytes());
    data.extend_from_slice(&(width as i32).to_le_bytes());
    data.extend_from_slice(&(height as i32).to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&24u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&image_bytes.to_le_bytes());
    data.extend_from_slice(&2835u32.to_le_bytes());
    data.extend_from_slice(&2835u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    for y in 0..height {
        for x in 0..width {
            data.extend_from_slice(&[
                (x.wrapping_mul(31) ^ y.wrapping_mul(17)) as u8,
                (x.wrapping_mul(7) ^ y.wrapping_mul(29)) as u8,
                (x.wrapping_mul(13) ^ y.wrapping_mul(3)) as u8,
            ]);
        }
        data.resize((54 + (y + 1) * row_bytes) as usize, 0);
    }
    data
}

#[test]
fn native_기본_프로파일의_이름과_정확한_값을_공개() {
    assert_eq!(
        NATIVE_PACKAGE_LIMITS_PROFILE.profile,
        NATIVE_PACKAGE_LIMITS_PROFILE_NAME
    );
    assert_eq!(NATIVE_PACKAGE_LIMITS_PROFILE.limits, NATIVE_PACKAGE_LIMITS);
    assert_eq!(PackageLimits::default(), NATIVE_PACKAGE_LIMITS);
    assert_eq!(NATIVE_PACKAGE_LIMITS.max_entries, 4_096);
    assert_eq!(NATIVE_PACKAGE_LIMITS.max_entry_name_bytes, 65_536);
    assert_eq!(NATIVE_PACKAGE_LIMITS.max_total_name_bytes, 16 * 1024 * 1024);
    assert_eq!(
        NATIVE_PACKAGE_LIMITS.max_entry_uncompressed_bytes,
        512 * 1024 * 1024
    );
    assert_eq!(
        NATIVE_PACKAGE_LIMITS.max_total_uncompressed_bytes,
        2 * 1024 * 1024 * 1024
    );
    assert_eq!(
        NATIVE_PACKAGE_LIMITS.max_xml_uncompressed_bytes,
        64 * 1024 * 1024
    );
    assert_eq!(NATIVE_PACKAGE_LIMITS.max_compression_ratio, 1_000);
}

#[test]
fn 고압축률_bindata는_기본_정책으로_거부() {
    let path = temp_file("ratio");
    let zeros = vec![0u8; 2 * 1024 * 1024];
    write_zip(
        &path,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            ("BinData/bomb.bin", &zeros, CompressionMethod::Deflated),
        ],
    );

    let err = HwpxPackage::open(&path)
        .err()
        .expect("ZIP bomb를 거부해야 함");
    assert!(
        err.to_string().contains("압축률"),
        "한국어 압축률 진단: {err}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn 압축해제_합계와_xml_한도를_각각_검사() {
    let aggregate = temp_file("aggregate");
    let a = [1u8; 80];
    let b = [2u8; 80];
    write_zip(
        &aggregate,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            ("BinData/a.bin", &a, CompressionMethod::Stored),
            ("BinData/b.bin", &b, CompressionMethod::Stored),
        ],
    );
    let aggregate_limits = PackageLimits {
        max_total_uncompressed_bytes: 150,
        ..PackageLimits::default()
    };
    let err = HwpxPackage::open_with_limits(&aggregate, &aggregate_limits)
        .err()
        .expect("합계 제한을 거부해야 함");
    assert!(err.to_string().contains("합계"), "합계 진단: {err}");

    let xml_path = temp_file("xml");
    let xml = vec![b'x'; 2_048];
    write_zip(
        &xml_path,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            ("Contents/section0.xml", &xml, CompressionMethod::Stored),
        ],
    );
    let xml_limits = PackageLimits {
        max_entry_uncompressed_bytes: 4_096,
        max_total_uncompressed_bytes: 8_192,
        max_xml_uncompressed_bytes: 1_024,
        ..PackageLimits::default()
    };
    let err = HwpxPackage::open_with_limits(&xml_path, &xml_limits)
        .err()
        .expect("XML 제한을 거부해야 함");
    assert!(err.to_string().contains("XML 엔트리"), "XML 진단: {err}");

    let _ = std::fs::remove_file(aggregate);
    let _ = std::fs::remove_file(xml_path);
}

#[test]
fn 엔트리_수_한도를_검사() {
    let path = temp_file("count");
    write_zip(
        &path,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            ("a", b"a", CompressionMethod::Stored),
            ("b", b"b", CompressionMethod::Stored),
        ],
    );
    let limits = PackageLimits {
        max_entries: 2,
        ..PackageLimits::default()
    };
    let err = HwpxPackage::open_with_limits(&path, &limits)
        .err()
        .expect("엔트리 수 제한을 거부해야 함");
    assert!(err.to_string().contains("엔트리 수"), "개수 진단: {err}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn 중복_엔트리_이름을_결정적으로_거부() {
    let path = temp_file("duplicate");
    write_raw_stored_zip(
        &path,
        &[
            ("mimetype", b"application/hwp+zip"),
            ("dup.bin", b"first"),
            ("dup.bin", b"second"),
        ],
    );

    let err = HwpxPackage::open(&path)
        .err()
        .expect("중복 이름을 거부해야 함");
    assert!(
        err.to_string()
            .contains("중복 엔트리 이름을 허용하지 않습니다: 'dup.bin'"),
        "중복 진단: {err}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn 엔트리_이름의_개별_합계_바이트_한도를_검사() {
    let individual = temp_file("name_individual");
    write_raw_stored_zip(
        &individual,
        &[("mimetype", b"application/hwp+zip"), ("123456789", b"x")],
    );
    let limits = PackageLimits {
        max_entry_name_bytes: 8,
        ..PackageLimits::default()
    };
    let err = HwpxPackage::open_with_limits(&individual, &limits)
        .err()
        .expect("개별 이름 길이 제한");
    assert!(
        err.to_string().contains("이름 길이"),
        "개별 이름 진단: {err}"
    );

    let aggregate = temp_file("name_aggregate");
    write_raw_stored_zip(
        &aggregate,
        &[("mimetype", b"application/hwp+zip"), ("abc", b"x")],
    );
    let limits = PackageLimits {
        max_entry_name_bytes: 16,
        max_total_name_bytes: 10,
        ..PackageLimits::default()
    };
    let err = HwpxPackage::open_with_limits(&aggregate, &limits)
        .err()
        .expect("이름 합계 제한");
    assert!(
        err.to_string().contains("이름 길이 합계"),
        "이름 합계 진단: {err}"
    );

    let _ = std::fs::remove_file(individual);
    let _ = std::fs::remove_file(aggregate);
}

#[test]
fn 비정상_또는_경로_순회_엔트리_이름을_일관되게_거부() {
    let cases: &[&[u8]] = &[
        b"",
        b"../escape",
        b"/absolute",
        b"C:/windows",
        b"dir\\windows",
        b"dir//empty",
        b"dir/./current",
        b"nul\0name",
        b"invalid-\xff",
    ];
    for (index, invalid_name) in cases.iter().enumerate() {
        let path = temp_file(&format!("invalid_name_{index}"));
        write_raw_stored_zip_bytes(
            &path,
            &[(b"mimetype", b"application/hwp+zip"), (invalid_name, b"x")],
        );
        let err = HwpxPackage::open(&path)
            .err()
            .unwrap_or_else(|| panic!("비정상 이름을 거부해야 함: {invalid_name:?}"));
        assert!(
            err.to_string().contains("엔트리 이름")
                || err.to_string().contains("경로")
                || err.to_string().contains("UTF-8"),
            "이름 진단({invalid_name:?}): {err}"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn eocd_comment는_허용하고_후행_junk와_가짜_eocd는_거부() {
    let valid = temp_file("eocd_comment");
    write_zip_with_comment(&valid, b"valid comment PK\x05\x06 inside");
    HwpxPackage::open(&valid).expect("EOCD signature를 포함한 정상 ZIP comment");

    let trailing = temp_file("eocd_trailing");
    std::fs::copy(&valid, &trailing).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&trailing)
        .unwrap()
        .write_all(b"trailing-junk")
        .unwrap();
    let error = HwpxPackage::open(&trailing)
        .err()
        .expect("EOCD 뒤 junk를 거부해야 함");
    assert!(
        error.to_string().contains("EOCD") || error.to_string().contains("후행"),
        "EOCD 후행 데이터 진단: {error}"
    );

    let fake = temp_file("eocd_fake_trailing");
    std::fs::copy(&valid, &fake).unwrap();
    let mut fake_eocd = Vec::new();
    push_u32(&mut fake_eocd, 0x0605_4b50);
    fake_eocd.extend_from_slice(&[0; 18]);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&fake)
        .unwrap()
        .write_all(&fake_eocd)
        .unwrap();
    let error = HwpxPackage::open(&fake)
        .err()
        .expect("후행 가짜 EOCD로 우회하면 안 됨");
    assert!(
        error.to_string().contains("중앙 디렉터리") || error.to_string().contains("EOCD"),
        "가짜 EOCD 진단: {error}"
    );

    let _ = std::fs::remove_file(valid);
    let _ = std::fs::remove_file(trailing);
    let _ = std::fs::remove_file(fake);
}

#[test]
fn 중앙_디렉터리의_위조된_압축해제_크기를_정규화하지_않고_거부() {
    let src = temp_file("forged_size_src");
    let out = temp_file("forged_size_out");
    write_patch_fixture(
        &src,
        b"<hh:head/>",
        b"<hs:sec><hp:p><hp:run><hp:t>{{name}}</hp:t></hp:run></hp:p></hs:sec>",
    );
    forge_central_uncompressed_size(&src, "BinData/later.bin", 1);

    let mut package = HwpxPackage::open(&src).expect("선언값 preflight 자체는 통과");
    let error = package
        .verify_integrity()
        .expect_err("실제 압축해제 바이트 수가 선언과 달라야 함");
    assert!(
        error.to_string().contains("BinData/later.bin"),
        "위조 크기 엔트리 진단: {error}"
    );

    std::fs::write(&out, b"existing-destination").unwrap();
    let error = hwpx::patch::fill_placeholders(&src, &out, &BTreeMap::new())
        .expect_err("public patch도 위조 크기를 거부해야 함");
    assert!(
        error.to_string().contains("BinData/later.bin"),
        "public patch 위조 크기 진단: {error}"
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"existing-destination",
        "위조된 후반 엔트리는 기존 destination을 바꾸면 안 됨"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn 구조검증은_stored와_deflated_bindata의_손상을_검출() {
    for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
        let path = temp_file(match method {
            CompressionMethod::Stored => "corrupt_stored",
            CompressionMethod::Deflated => "corrupt_deflated",
            _ => unreachable!(),
        });
        write_integrity_fixture(&path, method);
        corrupt_entry_payload(&path, "BinData/payload.bin");

        let err = hwpx::read_structure(&path)
            .err()
            .expect("BinData CRC/압축 손상을 거부해야 함");
        assert!(
            err.to_string().contains("BinData/payload.bin"),
            "손상 엔트리 진단({method:?}): {err}"
        );
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn raw_copy_patch는_손상_입력을_출력_게시_전에_거부() {
    let src = temp_file("corrupt_patch_src");
    let out = temp_file("corrupt_patch_out");
    write_integrity_fixture(&src, CompressionMethod::Deflated);
    corrupt_entry_payload(&src, "BinData/payload.bin");
    std::fs::write(&out, b"existing-output-must-survive").unwrap();

    let err = hwpx::patch::fill_placeholders(&src, &out, &BTreeMap::new())
        .expect_err("raw-copy 전에 전체 입력 무결성을 검사해야 함");
    assert!(
        err.to_string().contains("BinData/payload.bin"),
        "raw-copy 손상 진단: {err}"
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"existing-output-must-survive",
        "손상 입력은 기존 출력 파일을 열거나 truncate하면 안 됨"
    );
    assert_no_patch_temps(&out);

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn public_patch는_transform과_semantic_검증_실패에도_destination을_보존() {
    let invalid_utf8 = temp_file("atomic_invalid_utf8");
    let invalid_semantic = temp_file("atomic_invalid_semantic");
    let out = temp_file("atomic_existing");
    let invalid_section = b"<hs:sec><hp:p><hp:run><hp:t>\xff</hp:t></hp:run></hp:p></hs:sec>";
    write_patch_fixture(&invalid_utf8, b"<hh:head/>", invalid_section);
    write_patch_fixture(
        &invalid_semantic,
        b"<hh:head><broken></hh:head>",
        b"<hs:sec><hp:p><hp:run><hp:t>{{name}}</hp:t></hp:run></hp:p></hs:sec>",
    );

    for source in [&invalid_utf8, &invalid_semantic] {
        std::fs::write(&out, b"stable-existing-output").unwrap();
        let mut values = BTreeMap::new();
        values.insert("name".to_string(), "제주".to_string());
        hwpx::patch::fill_placeholders(source, &out, &values)
            .expect_err("transform/semantic 오류는 public API 전체를 실패시켜야 함");
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"stable-existing-output",
            "실패 종류와 무관하게 기존 destination 보존"
        );
        assert_no_patch_temps(&out);
    }

    let _ = std::fs::remove_file(invalid_utf8);
    let _ = std::fs::remove_file(invalid_semantic);
    let _ = std::fs::remove_file(out);
}

#[test]
fn public_patch는_같은_경로에서_원자적으로_동작() {
    let path = temp_file("atomic_in_place");
    write_patch_fixture(
        &path,
        b"<hh:head/>",
        b"<hs:sec><hp:p><hp:run><hp:t>{{name}}</hp:t></hp:run></hp:p></hs:sec>",
    );
    let mut values = BTreeMap::new();
    values.insert("name".to_string(), "제주".to_string());

    let counts =
        hwpx::patch::fill_placeholders(&path, &path, &values).expect("안전한 in-place patch");
    assert_eq!(counts.get("name"), Some(&1));
    assert!(
        read_zip_entry(&path, "Contents/section0.xml")
            .windows("제주".len())
            .any(|window| window == "제주".as_bytes())
    );
    HwpxPackage::open(&path)
        .unwrap()
        .verify_integrity()
        .expect("게시된 in-place 결과 무결성");
    assert_no_patch_temps(&path);

    let _ = std::fs::remove_file(path);
}

#[cfg(unix)]
#[test]
fn public_patch는_하드링크_alias도_snapshot으로_안전하게_분리() {
    let src = temp_file("atomic_alias_src");
    let alias = temp_file("atomic_alias_out");
    write_patch_fixture(
        &src,
        b"<hh:head/>",
        b"<hs:sec><hp:p><hp:run><hp:t>{{name}}</hp:t></hp:run></hp:p></hs:sec>",
    );
    std::fs::hard_link(&src, &alias).unwrap();
    let original = std::fs::read(&src).unwrap();
    let mut values = BTreeMap::new();
    values.insert("name".to_string(), "제주".to_string());

    hwpx::patch::fill_placeholders(&src, &alias, &values).expect("hardlink alias patch");
    assert_eq!(
        std::fs::read(&src).unwrap(),
        original,
        "source 링크는 원본 inode를 계속 가리켜야 함"
    );
    assert_ne!(
        std::fs::read(&alias).unwrap(),
        original,
        "destination 디렉터리 엔트리만 원자적으로 교체"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(alias);
}

#[test]
fn 정상_대형_bmp는_읽고_구조검증은_bindata를_적재하지_않음() {
    let path = temp_file("large_bmp");
    let bmp = valid_bmp(1_024, 1_024);
    let version = br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#;
    let header = b"<hh:head/>";
    let section = b"<hs:sec><hp:p><hp:run><hp:t>test</hp:t></hp:run></hp:p></hs:sec>";
    write_zip(
        &path,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            ("version.xml", version, CompressionMethod::Deflated),
            ("Contents/header.xml", header, CompressionMethod::Deflated),
            (
                "Contents/section0.xml",
                section,
                CompressionMethod::Deflated,
            ),
            ("BinData/image.bmp", &bmp, CompressionMethod::Deflated),
        ],
    );

    let mut package = HwpxPackage::open(&path).expect("정상 대형 이미지 패키지");
    assert_eq!(
        package.read_entry("BinData/image.bmp").unwrap(),
        bmp,
        "요청한 경우에는 제한 안에서 BinData를 읽음"
    );
    let structure = hwpx::read_structure(&path).expect("BinData 없는 구조 파싱");
    assert!(
        structure.document.bin_streams.is_empty(),
        "구조 검증 경로는 BinData를 적재하지 않음"
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn fill은_대형_bindata를_raw_copy하고_xml만_변환() {
    let src = temp_file("patch_src");
    let out = temp_file("patch_out");
    let bmp = valid_bmp(1_024, 1_024);
    let section = b"<hs:sec><hp:p><hp:run><hp:t>{{name}}</hp:t></hp:run></hp:p></hs:sec>";
    write_zip(
        &src,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            (
                "version.xml",
                br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#,
                CompressionMethod::Deflated,
            ),
            (
                "Contents/header.xml",
                b"<hh:head/>",
                CompressionMethod::Deflated,
            ),
            (
                "Contents/section0.xml",
                section,
                CompressionMethod::Deflated,
            ),
            ("BinData/image.bmp", &bmp, CompressionMethod::Deflated),
        ],
    );

    let before = entry_fingerprint(&src, "BinData/image.bmp");
    let mut values = BTreeMap::new();
    values.insert("name".to_string(), "제주".to_string());
    let limits = PackageLimits {
        max_entry_uncompressed_bytes: 4 * 1024 * 1024,
        max_total_uncompressed_bytes: 5 * 1024 * 1024,
        max_xml_uncompressed_bytes: 1_024,
        ..PackageLimits::default()
    };
    let counts = hwpx::patch::fill_placeholders_with_limits(&src, &out, &values, &limits)
        .expect("bounded fill");
    assert_eq!(counts.get("name"), Some(&1));
    assert_eq!(
        entry_fingerprint(&out, "BinData/image.bmp"),
        before,
        "비대상 BinData의 압축 메타데이터와 CRC 보존"
    );
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let mut xml = String::new();
    zip.by_name("Contents/section0.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains("제주") && !xml.contains("{{name}}"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn writer도_같은_정책을_출력파일_생성_전에_적용() {
    let out = temp_file("writer_limit");
    let limits = PackageLimits {
        max_entries: 1,
        ..PackageLimits::default()
    };
    let err = hwpx::write::write_document_with_limits(
        &hwp_model::Document::default(),
        &out,
        &hwpx::write::HwpxWriteOptions::default(),
        &limits,
    )
    .expect_err("writer 엔트리 제한");
    assert!(err.to_string().contains("엔트리 수"), "writer 진단: {err}");
    assert!(
        !out.exists(),
        "출력 preflight 실패 시 부분 HWPX를 만들면 안 됨"
    );
}

fn entry_fingerprint(path: &Path, name: &str) -> (CompressionMethod, u32, u64, u64) {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let entry = zip.by_name(name).unwrap();
    (
        entry.compression(),
        entry.crc32(),
        entry.compressed_size(),
        entry.size(),
    )
}

fn write_integrity_fixture(path: &Path, bin_method: CompressionMethod) {
    let payload = pseudo_random_bytes(32 * 1024);
    write_zip(
        path,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            (
                "version.xml",
                br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#,
                CompressionMethod::Deflated,
            ),
            (
                "Contents/header.xml",
                b"<hh:head/>",
                CompressionMethod::Deflated,
            ),
            (
                "Contents/section0.xml",
                b"<hs:sec><hp:p><hp:run><hp:t>test</hp:t></hp:run></hp:p></hs:sec>",
                CompressionMethod::Deflated,
            ),
            ("BinData/payload.bin", &payload, bin_method),
        ],
    );
}

fn write_patch_fixture(path: &Path, header: &[u8], section: &[u8]) {
    let later = pseudo_random_bytes(16 * 1024);
    write_zip(
        path,
        &[
            (
                "mimetype",
                b"application/hwp+zip",
                CompressionMethod::Stored,
            ),
            (
                "version.xml",
                br#"<version major="1" minor="4" micro="0" buildNumber="0"/>"#,
                CompressionMethod::Deflated,
            ),
            ("Contents/header.xml", header, CompressionMethod::Deflated),
            (
                "Contents/section0.xml",
                section,
                CompressionMethod::Deflated,
            ),
            ("BinData/later.bin", &later, CompressionMethod::Deflated),
        ],
    );
}

fn write_zip_with_comment(path: &Path, comment: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(
        "mimetype",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(b"application/hwp+zip").unwrap();
    zip.set_raw_comment(comment.to_vec().into_boxed_slice())
        .unwrap();
    zip.finish().unwrap();
}

fn read_zip_entry(path: &Path, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut data = Vec::new();
    zip.by_name(name).unwrap().read_to_end(&mut data).unwrap();
    data
}

fn assert_no_patch_temps(destination: &Path) {
    let parent = destination.parent().unwrap();
    let needle = format!(
        ".{}.hwp-patch-",
        destination.file_name().unwrap().to_string_lossy()
    );
    let leftovers: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&needle))
        .map(|entry| entry.path())
        .collect();
    assert!(leftovers.is_empty(), "patch 임시 파일 잔류: {leftovers:?}");
}

fn forge_central_uncompressed_size(path: &Path, name: &str, size: u32) {
    let mut bytes = std::fs::read(path).unwrap();
    let name_bytes = name.as_bytes();
    let name_at = bytes
        .windows(name_bytes.len())
        .rposition(|window| window == name_bytes)
        .expect("중앙 디렉터리 이름");
    let central_at = name_at.checked_sub(46).expect("중앙 디렉터리 fixed header");
    assert_eq!(&bytes[central_at..central_at + 4], b"PK\x01\x02");
    bytes[central_at + 24..central_at + 28].copy_from_slice(&size.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

fn pseudo_random_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect()
}

fn corrupt_entry_payload(path: &Path, name: &str) {
    let (data_start, compressed_size) = {
        let file = std::fs::File::open(path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let entry = zip.by_name(name).unwrap();
        (entry.data_start().unwrap(), entry.compressed_size())
    };
    assert!(compressed_size > 2, "손상할 압축 payload가 너무 짧음");
    let offset = data_start + compressed_size / 2;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0x5a;
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&byte).unwrap();
}

/// zip 8의 writer는 중복 이름을 선제 거부하므로, 중복 검증 테스트용 최소 STORED ZIP을
/// 직접 만든다. 테스트 데이터만 생성하며 외부·독점 픽스처는 사용하지 않는다.
fn write_raw_stored_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let entries: Vec<(&[u8], &[u8])> = entries
        .iter()
        .map(|(name, data)| (name.as_bytes(), *data))
        .collect();
    write_raw_stored_zip_bytes(path, &entries);
}

fn write_raw_stored_zip_bytes(path: &Path, entries: &[(&[u8], &[u8])]) {
    let mut bytes = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = bytes.len() as u32;
        let crc = crc32(data);
        push_u32(&mut bytes, 0x0403_4b50);
        push_u16(&mut bytes, 20);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u32(&mut bytes, crc);
        push_u32(&mut bytes, data.len() as u32);
        push_u32(&mut bytes, data.len() as u32);
        push_u16(&mut bytes, name.len() as u16);
        push_u16(&mut bytes, 0);
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(data);

        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20);
        push_u16(&mut central, 20);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, crc);
        push_u32(&mut central, data.len() as u32);
        push_u32(&mut central, data.len() as u32);
        push_u16(&mut central, name.len() as u16);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u16(&mut central, 0);
        push_u32(&mut central, 0);
        push_u32(&mut central, offset);
        central.extend_from_slice(name);
    }
    let central_offset = bytes.len() as u32;
    let central_size = central.len() as u32;
    bytes.extend_from_slice(&central);
    push_u32(&mut bytes, 0x0605_4b50);
    push_u16(&mut bytes, 0);
    push_u16(&mut bytes, 0);
    push_u16(&mut bytes, entries.len() as u16);
    push_u16(&mut bytes, entries.len() as u16);
    push_u32(&mut bytes, central_size);
    push_u32(&mut bytes, central_offset);
    push_u16(&mut bytes, 0);
    std::fs::write(path, bytes).unwrap();
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
