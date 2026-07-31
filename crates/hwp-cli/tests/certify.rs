use std::fs;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "hwp-certify-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn write_empty_hwpx(path: &Path) {
    let document = hwp_convert::from_markdown("");
    let warnings = hwpx::write_document(&document, path).unwrap();
    assert!(warnings.is_empty(), "fixture writer warnings: {warnings:?}");
}

fn policy(mode: &str) -> serde_json::Value {
    let oracle = if mode == "disabled" {
        serde_json::json!({"mode": "disabled"})
    } else {
        serde_json::json!({
            "mode": mode,
            "configuration": {
                "runtime": {"version": "Docker version 1", "sha256": "0".repeat(64)},
                "libreoffice": {"version": "26.2.5", "executable_sha256": "1".repeat(64)},
                "extension": {"version": "0.7.12", "sha256": "2".repeat(64)},
                "image": {"digest": format!("sha256:{}", "3".repeat(64))}
            }
        })
    };
    serde_json::json!({
        "schema_version": "1.0",
        "document": {"macros": "deny", "external_references": "deny"},
        "render": {
            "pages": [1],
            "page_count": {"exact": 1},
            "allowed_blank_pages": [1],
            "fail_on_outside_page_bounds": false,
            "fail_on_potential_collision": false
        },
        "oracle": oracle
    })
}

fn run(mode: &str) -> (std::process::ExitStatus, PathBuf, PathBuf) {
    let root = temp_dir(mode);
    let input = root.join("input-with-wrong-suffix.bin");
    let policy_path = root.join("policy.json");
    let report = root.join("report");
    write_empty_hwpx(&input);
    fs::write(
        &policy_path,
        serde_json::to_vec_pretty(&policy(mode)).unwrap(),
    )
    .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("certify")
        .arg(&input)
        .arg("--policy")
        .arg(&policy_path)
        .arg("--report")
        .arg(&report)
        .env_remove("HWP_CERTIFY_ORACLE_RUNTIME")
        .env_remove("HWP_CERTIFY_ORACLE_EXTENSION")
        .env_remove("HWP_CERTIFY_ORACLE_IMAGE")
        .status()
        .unwrap();
    (status, root, report)
}

#[test]
fn optional_unavailable_passes_native_only_but_required_is_partial() {
    let (optional_status, optional_root, optional_report) = run("optional");
    assert!(optional_status.success());
    let optional: serde_json::Value =
        serde_json::from_slice(&fs::read(optional_report.join("report.json")).unwrap()).unwrap();
    assert_eq!(optional["overall"], "passed");
    assert_eq!(optional["scope"], "native_only");
    assert_eq!(optional["oracle"]["status"], "oracle_unavailable");
    assert!(optional_report.join("manifest.json").is_file());
    fs::remove_dir_all(optional_root).unwrap();

    let (required_status, required_root, required_report) = run("required");
    assert!(!required_status.success());
    let required: serde_json::Value =
        serde_json::from_slice(&fs::read(required_report.join("report.json")).unwrap()).unwrap();
    assert_eq!(required["overall"], "partial");
    assert_eq!(required["scope"], "native_only");
    assert_eq!(required["oracle"]["status"], "oracle_unavailable");
    fs::remove_dir_all(required_root).unwrap();
}

#[test]
fn existing_report_directory_is_never_replaced() {
    let root = temp_dir("existing");
    let input = root.join("input.hwpx");
    let policy_path = root.join("policy.json");
    let report = root.join("report");
    write_empty_hwpx(&input);
    fs::write(
        &policy_path,
        serde_json::to_vec_pretty(&policy("disabled")).unwrap(),
    )
    .unwrap();
    fs::create_dir(&report).unwrap();
    fs::write(report.join("sentinel"), b"keep").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .args(["certify"])
        .arg(&input)
        .arg("--policy")
        .arg(&policy_path)
        .arg("--report")
        .arg(&report)
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(fs::read(report.join("sentinel")).unwrap(), b"keep");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mcp_certify_executes_the_same_atomic_contract() {
    use std::process::Stdio;

    let root = temp_dir("mcp");
    let input = root.join("input.hwpx");
    let policy_path = root.join("policy.json");
    let report = root.join("report");
    write_empty_hwpx(&input);
    fs::write(
        &policy_path,
        serde_json::to_vec_pretty(&policy("disabled")).unwrap(),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hwp"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "hwp_certify",
            "arguments": {
                "input": input,
                "policy": policy_path,
                "report": report
            }
        }
    });
    writeln!(child.stdin.as_mut().unwrap(), "{request}").unwrap();
    drop(child.stdin.take());
    let mut response = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut response)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["result"]["isError"], false);
    let summary: serde_json::Value =
        serde_json::from_str(value["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(summary["overall"], "passed");
    assert!(report.join("report.json").is_file());
    assert!(report.join("manifest.json").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_native_certification_has_identical_artifacts_and_hashes() {
    let root = temp_dir("deterministic");
    let input = root.join("input.hwpx");
    let policy_path = root.join("policy.json");
    write_empty_hwpx(&input);
    fs::write(
        &policy_path,
        serde_json::to_vec_pretty(&policy("disabled")).unwrap(),
    )
    .unwrap();
    let reports = [root.join("report-a"), root.join("report-b")];
    for report in &reports {
        let output = Command::new(env!("CARGO_BIN_EXE_hwp"))
            .arg("certify")
            .arg(&input)
            .arg("--policy")
            .arg(&policy_path)
            .arg("--report")
            .arg(report)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for relative in ["report.json", "manifest.json", "pages/page-000001.png"] {
        assert_eq!(
            fs::read(reports[0].join(relative)).unwrap(),
            fs::read(reports[1].join(relative)).unwrap(),
            "artifact drift: {relative}"
        );
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(reports[0].join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["artifact_count"], 3);
    assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
    fs::remove_dir_all(root).unwrap();
}
