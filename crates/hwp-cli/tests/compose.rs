//! DocumentSpec v1 CLI 계약: parse/validation, dry-run, deterministic atomic publish.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hwp-cli-compose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// 폰트 가용성은 CI 러너마다 다르므로(`AGENTS.md`: CI 테스트는 폰트 의존 단언 금지)
/// font_resolution 경고를 뺀 나머지가 비어 있는지만 본다.
fn assert_no_warnings_besides_fonts(report: &serde_json::Value) {
    let rest: Vec<_> = report["warnings"]
        .as_array()
        .expect("report warnings array")
        .iter()
        .filter(|warning| {
            !warning
                .as_str()
                .is_some_and(|text| text.starts_with("font_resolution/"))
        })
        .collect();
    assert!(rest.is_empty(), "unexpected warnings: {rest:?}");
}

fn assert_no_staging_debris(path: &Path) {
    let parent = path.parent().unwrap();
    let file_name = path.file_name().unwrap().to_string_lossy();
    let prefix = format!(".{file_name}.hwp-output-");
    let leftovers = std::fs::read_dir(parent)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "staging debris: {leftovers:?}");
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").unwrap();
            output
        })
}

fn assert_same_bytes(first: &Path, second: &Path) {
    let first_bytes = std::fs::read(first).unwrap();
    let second_bytes = std::fs::read(second).unwrap();
    if first_bytes != second_bytes {
        let first_diff = first_bytes
            .iter()
            .zip(&second_bytes)
            .position(|(left, right)| left != right);
        panic!(
            "outputs differ: first_sha256={}, second_sha256={}, first_diff={first_diff:?}",
            sha256_hex(&first_bytes),
            sha256_hex(&second_bytes)
        );
    }
}

fn write_image_spec(spec: &Path, asset: &str) {
    let value = serde_json::json!({
        "version": "1.0",
        "sections": [{
            "blocks": [{
                "type": "image",
                "path": asset,
                "width_mm": 20
            }]
        }]
    });
    std::fs::write(spec, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn assert_asset_rejected_without_leak(spec: &Path, output: &Path, secrets: &[&str]) {
    let _ = std::fs::remove_file(output);
    let result = hwp()
        .arg("compose")
        .arg(spec)
        .arg("-o")
        .arg(output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("$.sections[0].blocks[0].path"),
        "stderr lacks JSON pointer: {stderr}"
    );
    for secret in secrets {
        assert!(
            !stderr.contains(secret),
            "stderr leaked {secret:?}: {stderr}"
        );
    }
    assert!(!output.exists());
    assert_no_staging_debris(output);
}

#[test]
fn dry_run_yaml_returns_plan_without_output() {
    let spec = repo("examples/document-spec-v1/comprehensive.yaml");
    let output = temp("dry-run.hwpx");
    let _ = std::fs::remove_file(&output);

    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .args(["--dry-run", "--report"])
        .output()
        .expect("hwp compose");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["schema_version"], "1.0");
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["native"], true);
    assert_eq!(report["deterministic"], true);
    assert_eq!(report["visual_fallback_used"], serde_json::json!([]));
    report["output"] = serde_json::Value::String("report.hwpx".to_string());
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "golden/document-spec-v1-comprehensive-report.json"
    ))
    .unwrap();
    assert_eq!(report, golden);
    assert!(!output.exists());
    assert_no_staging_debris(&output);
}

#[test]
fn hwpx_output_is_deterministic_and_reopens() {
    let spec = repo("examples/document-spec-v1/basic.json");
    let first = temp("deterministic-a.hwpx");
    let second = temp("deterministic-b.hwpx");
    for (index, output) in [&first, &second].into_iter().enumerate() {
        let _ = std::fs::remove_file(output);
        let result = hwp()
            .arg("compose")
            .arg(&spec)
            .arg("-o")
            .arg(output)
            .arg("--report")
            .output()
            .expect("hwp compose");
        assert!(
            result.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["deterministic"], true);
        assert_no_warnings_besides_fonts(&report);
        assert_no_staging_debris(output);
        if index == 0 {
            // ZIP의 DOS timestamp 정밀도(2초)를 넘겨도 바이트가 같아야 한다.
            std::thread::sleep(std::time::Duration::from_millis(2_100));
        }
    }
    assert_same_bytes(&first, &second);

    let validate = hwp().arg("validate").arg(&first).output().unwrap();
    assert!(
        validate.status.success(),
        "validate: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    let cat = hwp().arg("cat").arg(&first).output().unwrap();
    let text = String::from_utf8_lossy(&cat.stdout);
    assert!(cat.status.success() && text.contains("구조 문서"));

    for output in [&first, &second] {
        let _ = std::fs::remove_file(output);
    }
}

#[test]
fn hwp_output_is_deterministic_and_reopens() {
    let spec = repo("examples/document-spec-v1/basic.json");
    let first = temp("deterministic-a.hwp");
    let second = temp("deterministic-b.hwp");
    for (index, output) in [&first, &second].into_iter().enumerate() {
        let _ = std::fs::remove_file(output);
        let result = hwp()
            .arg("compose")
            .arg(&spec)
            .arg("-o")
            .arg(output)
            .arg("--report")
            .output()
            .expect("hwp compose");
        assert!(
            result.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["deterministic"], true);
        assert_no_warnings_besides_fonts(&report);
        assert_no_staging_debris(output);
        if index == 0 {
            std::thread::sleep(std::time::Duration::from_millis(2_100));
        }
    }
    assert_same_bytes(&first, &second);

    let validate = hwp().arg("validate").arg(&first).output().unwrap();
    assert!(validate.status.success());
    let cat = hwp().arg("cat").arg(&first).output().unwrap();
    assert!(cat.status.success() && String::from_utf8_lossy(&cat.stdout).contains("구조 문서"));

    for output in [&first, &second] {
        let _ = std::fs::remove_file(output);
    }
}

#[test]
fn comprehensive_hwpx_publishes_structures_and_fields() {
    let spec = repo("examples/document-spec-v1/comprehensive.yaml");
    let output = temp("comprehensive.hwpx");
    let _ = std::fs::remove_file(&output);

    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .arg("--report")
        .output()
        .expect("hwp compose");
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["tables"], 1);
    assert_eq!(report["equations"], 1);
    assert_eq!(report["fields"], 2);
    assert_no_warnings_besides_fonts(&report);

    let document = hwpx::read_document(&output).unwrap().document;
    assert_eq!(document.sections.len(), 1);
    assert_eq!(hwp_convert::list_fields(&document).len(), 2);
    let text = hwp()
        .arg("cat")
        .arg(&output)
        .args(["--with-header-footer"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    for expected in [
        "기본 머리말",
        "자동 생성 문서",
        "Ⅰ. 구조화 문서",
        "병합",
        "두 번째 페이지",
    ] {
        assert!(text.contains(expected), "missing {expected:?}: {text}");
    }
    assert_no_staging_debris(&output);
    let _ = std::fs::remove_file(output);
}

#[test]
fn image_asset_is_embedded_once_with_natural_height() {
    let spec = temp("image.json");
    let asset = temp("pixel.gif");
    let output = temp("image.hwpx");
    // 2x1 transparent GIF89a.
    std::fs::write(
        &asset,
        [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x02, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3b,
        ],
    )
    .unwrap();
    std::fs::write(
        &spec,
        r#"{
          "version": "1.0",
          "sections": [{"blocks": [{
            "type": "image",
            "path": "pixel.gif",
            "width_mm": 20
          }]}]
        }"#,
    )
    .unwrap();
    let _ = std::fs::remove_file(&output);

    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .arg("--report")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["images"], 1);
    let document = hwpx::read_document(&output).unwrap().document;
    assert_eq!(document.bin_streams.len(), 1);
    let picture = document.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("picture");
    assert!((picture.width.0 - 5_669).abs() <= 1);
    assert!((picture.height.0 - 2_835).abs() <= 1);
    assert_no_staging_debris(&output);

    for path in [&spec, &asset, &output] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn hwp_image_asset_publishes_reopens_and_is_deterministic() {
    let spec = temp("image-hwp.json");
    let asset = temp("pixel-hwp.gif");
    let first = temp("image-first.hwp");
    let second = temp("image-second.hwp");
    let gif = [
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x02, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00,
        0x00, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
    ];
    std::fs::write(&asset, gif).unwrap();
    std::fs::write(
        &spec,
        r#"{
          "version": "1.0",
          "sections": [{"blocks": [{
            "type": "image",
            "path": "pixel-hwp.gif",
            "width_mm": 20
          }]}]
        }"#,
    )
    .unwrap();

    for output in [&first, &second] {
        let _ = std::fs::remove_file(output);
        let result = hwp()
            .arg("compose")
            .arg(&spec)
            .arg("-o")
            .arg(output)
            .arg("--report")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_no_staging_debris(output);
    }
    assert_same_bytes(&first, &second);

    let document = hwp5::read_document(&first).unwrap().document;
    let picture = document.sections[0]
        .paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.controls)
        .find_map(|control| match control {
            hwp_model::Control::Picture(picture) => Some(picture),
            _ => None,
        })
        .expect("picture");
    assert!((picture.width.0 - 5_669).abs() <= 1);
    assert!((picture.height.0 - 2_835).abs() <= 1);
    assert_eq!(picture.description, None);
    assert_eq!(document.resolve_bin(&picture.bin_ref), Some(gif.as_slice()));

    for path in [&spec, &asset, &first, &second] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn asset_paths_cannot_use_absolute_or_parent_authority() {
    let root = temp("asset-authority");
    let _ = std::fs::remove_dir_all(&root);
    let specs = root.join("specs");
    std::fs::create_dir_all(&specs).unwrap();
    let canary = root.join("outside-canary.gif");
    std::fs::write(&canary, b"SECRET-ASSET-CANARY-CONTENT").unwrap();

    let absolute_spec = specs.join("absolute.json");
    let absolute_output = specs.join("absolute.hwpx");
    let absolute = canary.display().to_string();
    write_image_spec(&absolute_spec, &absolute);
    assert_asset_rejected_without_leak(
        &absolute_spec,
        &absolute_output,
        &[&absolute, "SECRET-ASSET-CANARY-CONTENT"],
    );

    let parent_spec = specs.join("parent.json");
    let parent_output = specs.join("parent.hwpx");
    write_image_spec(&parent_spec, "../outside-canary.gif");
    assert_asset_rejected_without_leak(
        &parent_spec,
        &parent_output,
        &[&root.display().to_string(), "SECRET-ASSET-CANARY-CONTENT"],
    );

    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn asset_symlink_components_and_hardlinks_are_rejected_without_leaks() {
    use std::os::unix::fs::symlink;

    let root = temp("asset-links");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let canary = root.join("SECRET-CANARY.gif");
    std::fs::write(&canary, b"SECRET-LINK-CONTENT").unwrap();

    symlink(&canary, root.join("linked.gif")).unwrap();
    let symlink_spec = root.join("symlink.json");
    let symlink_output = root.join("symlink.hwpx");
    write_image_spec(&symlink_spec, "linked.gif");
    assert_asset_rejected_without_leak(
        &symlink_spec,
        &symlink_output,
        &[&root.display().to_string(), "SECRET-LINK-CONTENT"],
    );
    std::fs::remove_file(root.join("linked.gif")).unwrap();

    let nested = root.join("nested");
    symlink(&root, &nested).unwrap();
    let nested_spec = root.join("nested-symlink.json");
    let nested_output = root.join("nested-symlink.hwpx");
    write_image_spec(&nested_spec, "nested/SECRET-CANARY.gif");
    assert_asset_rejected_without_leak(
        &nested_spec,
        &nested_output,
        &[&root.display().to_string(), "SECRET-LINK-CONTENT"],
    );
    std::fs::remove_file(nested).unwrap();

    std::fs::hard_link(&canary, root.join("hard.gif")).unwrap();
    let hard_spec = root.join("hardlink.json");
    let hard_output = root.join("hardlink.hwpx");
    write_image_spec(&hard_spec, "hard.gif");
    assert_asset_rejected_without_leak(
        &hard_spec,
        &hard_output,
        &[&root.display().to_string(), "SECRET-LINK-CONTENT"],
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn document_spec_v2_svg_fallback_cli_is_deterministic_and_png_only() {
    let root = temp("v2-svg-cli");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("visual.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" fill="#2563EB"/></svg>"##,
    )
    .unwrap();
    std::fs::write(
        root.join("second.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#DC2626"/></svg>"##,
    )
    .unwrap();
    let spec = root.join("document.json");
    std::fs::write(
        &spec,
        r#"{
          "version":"2.0",
          "document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"anchor"}]}]}]},
          "visuals":[
            {"id":"vector","location":{"section":0,"paragraph":0},"policy":{"hwp":"force_visual_fallback","hwpx":"force_visual_fallback"},"alt":"blue square","width_mm":30,"height_mm":30,"content":{"type":"svg","path":"visual.svg"}},
            {"id":"second","location":{"section":0,"paragraph":0},"policy":{"hwp":"force_visual_fallback","hwpx":"force_visual_fallback"},"title":"Red circle","alt":"second visual","width_mm":20,"height_mm":10,"content":{"type":"svg","path":"second.svg"}}
          ]
        }"#,
    )
    .unwrap();

    for extension in ["hwpx", "hwp"] {
        let first = root.join(format!("first.{extension}"));
        let second = root.join(format!("second.{extension}"));
        for output in [&first, &second] {
            let result = hwp()
                .arg("compose")
                .arg(&spec)
                .arg("-o")
                .arg(output)
                .arg("--report")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "stderr: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["schema_version"], "2.0");
            assert_eq!(report["target_format"], extension);
            assert_eq!(report["visuals"].as_array().unwrap().len(), 2);
            for (index, id) in ["vector", "second"].into_iter().enumerate() {
                assert_eq!(report["visuals"][index]["id"], id);
                assert_eq!(
                    report["visuals"][index]["requested_policy"],
                    "force_visual_fallback"
                );
                assert_eq!(
                    report["visuals"][index]["resolved_representation"],
                    "visual_fallback"
                );
                assert_eq!(report["visuals"][index]["target_format"], extension);
                assert_eq!(report["visuals"][index]["media_type"], "image/png");
            }
            assert_eq!(report["visuals"][0]["dimensions"]["width_mm"], 30.0);
            assert_eq!(report["visuals"][1]["dimensions"]["width_mm"], 20.0);
            assert_ne!(
                report["visuals"][0]["media_sha256"],
                report["visuals"][1]["media_sha256"]
            );
            let report_text = String::from_utf8_lossy(&result.stdout);
            assert!(!report_text.contains("visual.svg"));
            assert!(!report_text.contains("second.svg"));
            assert!(!report_text.contains("Red circle"));
            assert!(!report_text.contains("second visual"));
            assert_no_staging_debris(output);
        }
        assert_same_bytes(&first, &second);
        let document = if extension == "hwpx" {
            hwpx::read_document(&first).unwrap().document
        } else {
            hwp5::read_document(&first).unwrap().document
        };
        assert_eq!(document.bin_streams.len(), 2);
        assert!(document.bin_streams.iter().all(|stream| {
            stream.data.starts_with(&[0x89, b'P', b'N', b'G']) && !stream.data.starts_with(b"<svg")
        }));
        let pictures = document.sections[0].paragraphs[0]
            .controls
            .iter()
            .filter_map(|control| match control {
                hwp_model::Control::Picture(picture) => Some(picture),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[0].description.as_deref(), Some("blue square"));
        assert_eq!(
            pictures[1].description.as_deref(),
            Some("Red circle\n\nsecond visual")
        );
        assert_eq!((pictures[0].width.0, pictures[0].height.0), (8_504, 8_504));
        assert_eq!((pictures[1].width.0, pictures[1].height.0), (5_669, 2_835));
        assert!(pictures.iter().all(|picture| picture.treat_as_char));
        assert!(pictures.iter().all(|picture| picture.z_order == 0));
        assert_ne!(
            document.resolve_bin(&pictures[0].bin_ref),
            document.resolve_bin(&pictures[1].bin_ref)
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_closed_contract_preserves_existing_destination() {
    let spec = temp("invalid-unknown.json");
    let output = temp("invalid-existing.hwpx");
    std::fs::write(
        &spec,
        r#"{
          "version": "1.0",
          "sections": [{
            "blocks": [{
              "type": "paragraph",
              "runs": [{"type": "text", "text": "x", "font_weight": 700}]
            }]
          }]
        }"#,
    )
    .unwrap();
    std::fs::write(&output, b"EXISTING DESTINATION").unwrap();

    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .arg("--allow-visual-fallback")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("font_weight"),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"EXISTING DESTINATION");
    assert_no_staging_debris(&output);

    for path in [&spec, &output] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn unsupported_native_request_is_typed_and_does_not_publish() {
    let spec = temp("unsupported-native.json");
    let output = temp("unsupported-native.hwpx");
    std::fs::write(
        &spec,
        r#"{
          "version": "1.0",
          "styles": {"body": {"keep_with_next": true}},
          "sections": [{
            "blocks": [{
              "type": "paragraph",
              "style": "body",
              "runs": [{"type": "text", "text": "x"}]
            }]
          }]
        }"#,
    )
    .unwrap();
    let _ = std::fs::remove_file(&output);

    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("\"code\": \"unsupported_native\"")
            && stderr.contains("$.styles.body.keep_with_next"),
        "stderr: {stderr}"
    );
    assert!(!output.exists());
    assert_no_staging_debris(&output);

    let _ = std::fs::remove_file(&spec);
}

#[test]
fn document_spec_v2_rejects_deprecated_global_fallback_policy() {
    let spec = temp("v2-policy-conflict.json");
    let output = temp("v2-policy-conflict.hwpx");
    std::fs::write(
        &spec,
        r#"{
          "version":"2.0",
          "document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"anchor"}]}]}]},
          "visuals":[]
        }"#,
    )
    .unwrap();
    let _ = std::fs::remove_file(&output);

    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .arg("--allow-visual-fallback")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("\"code\": \"policy_conflict\"") && stderr.contains("$.policy"),
        "stderr: {stderr}"
    );
    assert!(!output.exists());
    assert_no_staging_debris(&output);

    let _ = std::fs::remove_file(spec);
}

/// Reads the raw paragraph-head level of every numbered paragraph whose text starts with
/// `prefix`, straight from the written HWP5 file: attr1 bits 25..=27 and the PARA_SHAPE
/// tail's final u32 (07-hangul-compat-rules K1, #472). The raw values are what Hancom uses
/// to pick the NUMBERING slot, so `head_level()` (which `list_level` can override) is not used.
fn hwp_numbered_raw_levels(path: &Path) -> Vec<(String, u32, Option<u32>, String)> {
    let doc = hwp5::read::read_document(path).unwrap().document;
    let mut out = Vec::new();
    for paragraph in &doc.sections[0].paragraphs {
        let shape = &doc.header.para_shapes[paragraph.para_shape.0 as usize];
        if shape.head_type() != 2 {
            continue;
        }
        let text: String = paragraph
            .chars
            .iter()
            .filter_map(|c| match c {
                hwp_model::HwpChar::Text(ch) => Some(*ch),
                _ => None,
            })
            .collect();
        let raw = (shape.attr1 >> 25) & 0x7;
        let tail = (shape.tail.len() >= 16)
            .then(|| u32::from_le_bytes(shape.tail[12..16].try_into().unwrap()));
        let template = doc.header.numbering_levels[usize::from(shape.numbering_id)][raw as usize]
            .template
            .clone();
        out.push((text, raw, tail, template));
    }
    out
}

const ORDERED_472_SPEC: &str = r#"{"version":"1.0","styles":{"body":{"font_size_pt":10}},
"lists":{"o":{"kind":"ordered","levels":[{"marker":"{n}.","start":1},{"marker":"{n}.","start":1},{"marker":"{n})","start":1}]}},
"sections":[{"blocks":[
{"type":"paragraph","style":"body","list":{"name":"o","level":0},"runs":[{"type":"text","text":"top"}]},
{"type":"paragraph","style":"body","list":{"name":"o","level":2},"runs":[{"type":"text","text":"deep"}]}]}]}"#;

fn assert_472_levels(path: &Path, label: &str) {
    let levels = hwp_numbered_raw_levels(path);
    let find = |text: &str| {
        levels
            .iter()
            .find(|(t, ..)| t == text)
            .unwrap_or_else(|| panic!("{label}: numbered paragraph {text} missing: {levels:?}"))
            .clone()
    };
    for (text, level, template) in [("top", 0, "^1."), ("deep", 2, "^3)")] {
        let (_, raw, tail, drawn) = find(text);
        assert_eq!(
            raw, level,
            "{label}: {text} attr1 level bits must be zero-based"
        );
        assert_eq!(
            tail,
            Some(level),
            "{label}: {text} tail level must repeat the bits"
        );
        assert_eq!(
            drawn, template,
            "{label}: {text} draws its own level's slot"
        );
    }
}

/// #472: `compose` to `.hwp` must store an ordered list's levels zero-based. Before the fix
/// the run refused at semantic verification, or level 0 drew the second slot ("가.").
#[test]
fn compose_ordered_list_to_hwp_stores_zero_based_levels() {
    let spec = temp("ordered-472.json");
    std::fs::write(&spec, ORDERED_472_SPEC).unwrap();
    let output = temp("ordered-472.hwp");
    let _ = std::fs::remove_file(&output);
    let run = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "compose ordered list to HWP: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_472_levels(&output, "compose .hwp");
}

/// #472: `hwp convert` HWPX → HWP is the same writer path; the levels must come out
/// zero-based too.
#[test]
fn convert_hwpx_ordered_list_to_hwp_stores_zero_based_levels() {
    let spec = temp("ordered-472-convert.json");
    std::fs::write(&spec, ORDERED_472_SPEC).unwrap();
    let hwpx = temp("ordered-472.hwpx");
    let hwp_out = temp("ordered-472-converted.hwp");
    let _ = std::fs::remove_file(&hwp_out);
    let run = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&hwpx)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let run = hwp()
        .arg("convert")
        .arg(&hwpx)
        .arg("-o")
        .arg(&hwp_out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_472_levels(&hwp_out, "convert hwpx->hwp");
}

/// Semantic level and drawn marker of each numbered paragraph, after rereading with hwp-cli's
/// own reader. A one-shallow reread (the reader taking zero-based bits as one-based) would
/// show level 1 for "deep" and draw the previous level's template.
fn reread_levels_and_markers(path: &Path) -> Vec<(String, u8, String)> {
    let doc = hwp5::read::read_document(path).unwrap().document;
    let mut state = hwp_model::list::ListState::default();
    let mut out = Vec::new();
    for paragraph in &doc.sections[0].paragraphs {
        let shape = &doc.header.para_shapes[paragraph.para_shape.0 as usize];
        let marker = state.marker(&doc, paragraph);
        if shape.head_type() != 2 {
            continue;
        }
        let text: String = paragraph
            .chars
            .iter()
            .filter_map(|c| match c {
                hwp_model::HwpChar::Text(ch) => Some(*ch),
                _ => None,
            })
            .collect();
        out.push((text, shape.head_level(), marker.unwrap_or_default()));
    }
    out
}

fn assert_round_trip_levels(path: &Path, label: &str) {
    let rows = reread_levels_and_markers(path);
    let expected = [
        ("top".to_string(), 1u8, "1.".to_string()),
        ("deep".to_string(), 3u8, "1)".to_string()),
    ];
    for (text, level, marker) in expected {
        let row = rows
            .iter()
            .find(|(t, ..)| *t == text)
            .unwrap_or_else(|| panic!("{label}: {text} missing: {rows:?}"));
        assert_eq!(row.1, level, "{label}: {text} rereads at its own level");
        assert_eq!(
            row.2, marker,
            "{label}: {text} draws its own level's marker"
        );
    }
}

/// #472 T28-F2: what the HWP writer stores zero-based, hwp-cli's own reader must read back at
/// the same semantic level, for both `compose` and HWPX→HWP `convert`.
#[test]
fn ordered_list_hwp_round_trip_keeps_level_and_marker() {
    let spec = temp("ordered-472-rt.json");
    std::fs::write(&spec, ORDERED_472_SPEC).unwrap();
    let hwp_direct = temp("ordered-472-rt.hwp");
    let hwpx = temp("ordered-472-rt.hwpx");
    let hwp_converted = temp("ordered-472-rt-converted.hwp");
    for path in [&hwp_direct, &hwp_converted] {
        let _ = std::fs::remove_file(path);
    }
    for out in [&hwp_direct, &hwpx] {
        let run = hwp()
            .arg("compose")
            .arg(&spec)
            .arg("-o")
            .arg(out)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    let run = hwp()
        .arg("convert")
        .arg(&hwpx)
        .arg("-o")
        .arg(&hwp_converted)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_round_trip_levels(&hwp_direct, "compose .hwp");
    assert_round_trip_levels(&hwp_converted, "convert hwpx->hwp");
}

/// #472 T28-F2 audit: `indent_para` on an HWP-read numbered paragraph goes through
/// `ParaShape::set_head_level`, which writes a one-based level into attr1. The written HWP
/// must still store it zero-based and reread at the new semantic level.
#[test]
fn indent_para_on_hwp_numbered_item_keeps_zero_based_levels() {
    let spec = temp("ordered-472-indent.json");
    std::fs::write(&spec, ORDERED_472_SPEC).unwrap();
    let source = temp("ordered-472-indent-src.hwp");
    let output = temp("ordered-472-indent-out.hwp");
    let ops = temp("ordered-472-indent-ops.json");
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(&output);
    let run = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    // The "top" item is paragraph 0 of section 0 (the spec has no other body paragraph before it).
    let doc = hwp5::read::read_document(&source).unwrap().document;
    let top = doc.sections[0]
        .paragraphs
        .iter()
        .position(|p| {
            p.chars
                .iter()
                .filter_map(|c| match c {
                    hwp_model::HwpChar::Text(ch) => Some(*ch),
                    _ => None,
                })
                .collect::<String>()
                == "top"
        })
        .unwrap();
    std::fs::write(
        &ops,
        serde_json::to_vec(&serde_json::json!([
            {"op":"indent_para","address":{"at":{"section":0,"paragraph":top}}}
        ]))
        .unwrap(),
    )
    .unwrap();
    let run = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let levels = hwp_numbered_raw_levels(&output);
    let (_, raw, tail, _) = levels.iter().find(|(t, ..)| t == "top").unwrap().clone();
    assert_eq!(
        raw, 1,
        "indented level 1 item stores bits zero-based: {levels:?}"
    );
    assert_eq!(tail, Some(1), "tail repeats the new level: {levels:?}");
    let reread = reread_levels_and_markers(&output);
    let row = reread.iter().find(|(t, ..)| t == "top").unwrap();
    assert_eq!(
        row.1, 2,
        "indented item rereads at semantic level 2: {reread:?}"
    );
}
