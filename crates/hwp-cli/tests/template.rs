//! TemplateSpec/Data v1 CLI, determinism, preservation and redaction contract.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn repo(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("hwp-cli-template-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
    let first = std::fs::read(first).unwrap();
    let second = std::fs::read(second).unwrap();
    assert_eq!(
        first,
        second,
        "deterministic hashes differ: {} != {}",
        sha256_hex(&first),
        sha256_hex(&second)
    );
}

#[test]
fn dry_run_report_matches_golden_and_does_not_publish() {
    let dir = test_dir("dry-run");
    let output = dir.join("report.hwpx");
    let result = hwp()
        .arg("template")
        .arg(repo("examples/template-spec-v1/report-template.yaml"))
        .arg("--data")
        .arg(repo("examples/template-spec-v1/report-data.json"))
        .arg("-o")
        .arg(&output)
        .args(["--dry-run", "--report"])
        .output()
        .unwrap();
    assert_success(&result);

    let mut report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    report["output"] = serde_json::json!("report.hwpx");
    report["compose"]["output"] = serde_json::json!("report.hwpx");
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("golden/template-spec-v1-report.json")).unwrap();
    assert_eq!(report, golden);
    assert!(!output.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn compose_outputs_are_byte_deterministic_and_validated() {
    let dir = test_dir("deterministic");
    let first = dir.join("first.hwpx");
    let second = dir.join("second.hwpx");
    for output in [&first, &second] {
        let result = hwp()
            .arg("template")
            .arg(repo("examples/template-spec-v1/report-template.yaml"))
            .arg("--data")
            .arg(repo("examples/template-spec-v1/report-data.json"))
            .arg("-o")
            .arg(output)
            .arg("--report")
            .output()
            .unwrap();
        assert_success(&result);
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["semantic_validation"], "passed");
        assert_eq!(report["package_validation"], "passed");
        assert_eq!(
            report["output_sha256"],
            sha256_hex(&std::fs::read(output).unwrap())
        );
        std::thread::sleep(std::time::Duration::from_millis(2_100));
    }
    assert_same_bytes(&first, &second);
    let validation = hwp().arg("validate").arg(&first).output().unwrap();
    assert_success(&validation);
    std::fs::remove_dir_all(dir).unwrap();
}

fn build_reference_fixture(dir: &Path) -> PathBuf {
    let spec = dir.join("reference-document.json");
    let reference = dir.join("reference.hwpx");
    std::fs::write(
        &spec,
        r#"{
          "version":"1.0",
          "sections":[{"blocks":[{"type":"paragraph","runs":[
            {"type":"text","text":"{{기관명}} 운영 보고"}
          ]}]}]
        }"#,
    )
    .unwrap();
    let result = hwp()
        .arg("compose")
        .arg(&spec)
        .arg("-o")
        .arg(&reference)
        .output()
        .unwrap();
    assert_success(&result);
    reference
}

fn write_reference_contract(dir: &Path, target: &str) -> (PathBuf, PathBuf) {
    let template = dir.join(format!("reference-{target}.json"));
    let data = dir.join("reference-data.json");
    std::fs::write(
        &template,
        format!(
            r#"{{
              "version":"1.0",
              "variables":{{"institution":{{"type":"string","required":true}}}},
              "source":{{
                "mode":"reference_hwpx",
                "path":"reference.hwpx",
                "bindings":[{{
                  "region":"institution_region",
                  "variable":"institution",
                  "target":"placeholder",
                  "name":"{target}"
                }}]
              }}
            }}"#
        ),
    )
    .unwrap();
    std::fs::write(
        &data,
        r#"{"version":"1.0","values":{"institution":"제주한라대학교"}}"#,
    )
    .unwrap();
    (template, data)
}

#[test]
fn reference_dry_run_and_publish_share_strict_preservation_contract() {
    let dir = test_dir("reference");
    build_reference_fixture(&dir);
    let (template, data) = write_reference_contract(&dir, "기관명");
    let output = dir.join("filled.hwpx");
    std::fs::write(&output, b"KEEP").unwrap();

    let dry = hwp()
        .arg("template")
        .arg(&template)
        .arg("--data")
        .arg(&data)
        .arg("-o")
        .arg(&output)
        .args(["--dry-run", "--report"])
        .output()
        .unwrap();
    assert_success(&dry);
    let dry_report: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(dry_report["mode"], "reference_package_preserving");
    assert_eq!(dry_report["semantic_validation"], "passed");
    assert_eq!(dry_report["package_validation"], "passed");
    assert_eq!(std::fs::read(&output).unwrap(), b"KEEP");

    let actual = hwp()
        .arg("template")
        .arg(&template)
        .arg("--data")
        .arg(&data)
        .arg("-o")
        .arg(&output)
        .arg("--report")
        .output()
        .unwrap();
    assert_success(&actual);
    let report: serde_json::Value = serde_json::from_slice(&actual.stdout).unwrap();
    assert_eq!(report["changed_regions"][0]["id"], "institution_region");
    assert_eq!(report["generated_regions"], serde_json::json!([]));
    let cat = hwp().arg("cat").arg(&output).output().unwrap();
    assert_success(&cat);
    assert!(String::from_utf8_lossy(&cat.stdout).contains("제주한라대학교 운영 보고"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn missing_reference_target_and_secret_downstream_error_do_not_publish_or_leak() {
    let dir = test_dir("fail-closed");
    build_reference_fixture(&dir);
    let (template, data) = write_reference_contract(&dir, "없는대상");
    let output = dir.join("missing-target.hwpx");
    std::fs::write(&output, b"KEEP").unwrap();
    let missing = hwp()
        .arg("template")
        .arg(&template)
        .arg("--data")
        .arg(&data)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert_eq!(std::fs::read(&output).unwrap(), b"KEEP");

    let secret_template = dir.join("secret-template.json");
    let secret_data = dir.join("secret-data.json");
    std::fs::write(
        &secret_template,
        r#"{
          "version":"1.0",
            "variables":{"blocks":{"type":"rich_blocks","required":true,"secret":true}},
            "source":{"mode":"compose","document":{
              "version":"1.0",
              "sections":[{"blocks":[{"node":"value","pointer":"/values/blocks","region":"secret_blocks"}]}]
            }}
        }"#,
    )
    .unwrap();
    std::fs::write(
        &secret_data,
        r#"{"version":"1.0","values":{"blocks":[{"type":"paragraph","style":"TOPSECRET_CANARY","runs":[]}]}}"#,
    )
    .unwrap();
    let secret_output = dir.join("secret.hwpx");
    let secret = hwp()
        .arg("template")
        .arg(&secret_template)
        .arg("--data")
        .arg(&secret_data)
        .arg("-o")
        .arg(&secret_output)
        .output()
        .unwrap();
    assert!(!secret.status.success());
    let stderr = String::from_utf8_lossy(&secret.stderr);
    assert!(stderr.contains("compose_rejected"), "{stderr}");
    assert!(!stderr.contains("TOPSECRET_CANARY"), "{stderr}");
    assert!(stderr.len() <= hwp_cli::template_spec::MAX_DIAGNOSTIC_BYTES);
    assert!(!secret_output.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn strict_reference_regeneration_is_explicit_and_validated() {
    let dir = test_dir("regenerate");
    build_reference_fixture(&dir);
    let template = dir.join("regenerate.json");
    let data = dir.join("regenerate-data.json");
    let output = dir.join("regenerated.hwpx");
    std::fs::write(
        &template,
        r#"{
          "version":"1.0",
          "variables":{"title":{"type":"string","required":true}},
          "source":{
            "mode":"reference_regenerate",
            "path":"reference.hwpx",
            "strict_unsupported_objects":true,
            "document":{
              "version":"1.0",
              "sections":[{"blocks":[{"type":"paragraph","runs":[{
                "type":"text",
                "text":{"node":"value","pointer":"/values/title","as":"text"}
              }]}]}]
            }
          }
        }"#,
    )
    .unwrap();
    std::fs::write(
        &data,
        r#"{"version":"1.0","values":{"title":"재생성 결과"}}"#,
    )
    .unwrap();

    let result = hwp()
        .arg("template")
        .arg(&template)
        .arg("--data")
        .arg(&data)
        .arg("-o")
        .arg(&output)
        .arg("--report")
        .output()
        .unwrap();
    assert_success(&result);
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["mode"], "reference_regenerate");
    assert_eq!(report["semantic_validation"], "passed");
    assert_eq!(report["package_validation"], "passed");
    assert_eq!(report["compose"]["output"], output.display().to_string());
    assert!(
        !report["compose"]["output"]
            .as_str()
            .unwrap()
            .contains(".hwp-output-")
    );
    let cat = hwp().arg("cat").arg(&output).output().unwrap();
    assert_success(&cat);
    assert!(String::from_utf8_lossy(&cat.stdout).contains("재생성 결과"));
    std::fs::remove_dir_all(dir).unwrap();
}
