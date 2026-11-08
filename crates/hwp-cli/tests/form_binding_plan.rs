//! Source-bound binding workflow; generated native inputs, no host fonts or private corpus.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Workspace(PathBuf);
impl Workspace {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("hwp-bindings-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn source(&self, extension: &str) -> PathBuf {
        let markdown = self.0.join("source.md");
        std::fs::write(
            &markdown,
            "# Application\n\n{{ allocation }}\n\n{{ allocation }}\n",
        )
        .unwrap();
        let output = self.0.join(format!("source.{extension}"));
        let result = hwp()
            .args(["new", "--from"])
            .arg(markdown)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        success(&result, "fixture creation");
        output
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}
fn success(output: &Output, phase: &str) {
    assert!(
        output.status.success(),
        "{phase}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn discovery_keeps_repeated_occurrences_and_binds_the_source_bytes() {
    for extension in ["hwp", "hwpx"] {
        let workspace = Workspace::new(extension);
        let source = workspace.source(extension);
        let output = hwp()
            .args(["slots", "--json", "--forms", "--occurrences"])
            .arg(&source)
            .output()
            .unwrap();
        success(&output, "occurrence discovery must be available");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["version"], 1);
        assert_eq!(report["source_sha256"].as_str().unwrap().len(), 64);
        let occurrences: Vec<_> = report["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["kind"] == "placeholder" && item["label"] == "allocation")
            .collect();
        assert_eq!(
            occurrences.len(),
            2,
            "individual repeated targets must survive discovery"
        );
        assert_ne!(occurrences[0]["id"], occurrences[1]["id"]);
        assert!(occurrences.iter().all(|item| item["bindable"] == true));
        let again = hwp()
            .args(["slots", "--json", "--forms", "--occurrences"])
            .arg(&source)
            .output()
            .unwrap();
        success(&again, "repeat discovery");
        assert_eq!(output.stdout, again.stdout, "deterministic catalogue");
    }
}

fn discovery(source: &std::path::Path) -> serde_json::Value {
    let output = hwp().args(["slots", "--json", "--forms", "--occurrences"]).arg(source).output().unwrap();
    success(&output, "discovery setup");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn binding_operation(workspace: &Workspace, source: &std::path::Path, value: &str) -> serde_json::Value {
    let catalogue = discovery(source);
    let targets: Vec<_> = catalogue["occurrences"].as_array().unwrap().iter()
        .filter(|item| item["kind"] == "placeholder")
        .map(|item| serde_json::json!({"occurrence_id":item["id"],"copies":[0],"max_chars":100,"allow_reflow":true})).collect();
    serde_json::json!({"op":"apply_form_bindings", "plan": {
        "version":1,"source_sha256":catalogue["source_sha256"],"structural_sha256":catalogue["structural_sha256"],
        "bindings":[{"key":"allocation","targets":targets}],"repeats":[],"constraints":[]},
        "values":{"allocation":value}, "receipt":workspace.0.join("receipt.json"), "preview":workspace.0.join("preview.json")})
}

fn edit(workspace: &Workspace, source: &std::path::Path, output: &std::path::Path, operation: serde_json::Value, dry_run: bool) -> Output {
    let ops = workspace.0.join("ops.json");
    std::fs::write(&ops, serde_json::to_vec(&vec![operation]).unwrap()).unwrap();
    let mut command = hwp();
    command.arg("edit").arg(source).arg("--ops").arg(ops).arg("-o").arg(output).arg("--verify");
    if dry_run { command.arg("--dry-run"); }
    command.output().unwrap()
}

#[test]
fn reviewed_bindings_apply_to_each_occurrence_with_atomic_private_receipt() {
    for extension in ["hwp", "hwpx"] {
        let workspace = Workspace::new(&format!("apply-{extension}"));
        let source = workspace.source(extension);
        let original = std::fs::read(&source).unwrap();
        let output = workspace.0.join(format!("filled.{extension}"));
        let operation = binding_operation(&workspace, &source, "PRIVATE_LITERAL_700");
        let result = edit(&workspace, &source, &output, operation, false);
        success(&result, "reviewed compound binding must apply");
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let cat = hwp().arg("cat").arg(&output).output().unwrap(); success(&cat, "read back");
        let text = String::from_utf8_lossy(&cat.stdout);
        assert_eq!(text.matches("PRIVATE_LITERAL_700").count(), 2);
        assert!(!text.contains("{{ allocation }}"));
        let receipt = std::fs::read_to_string(workspace.0.join("receipt.json")).unwrap();
        assert!(!receipt.contains("PRIVATE_LITERAL_700"));
        assert!(!receipt.contains("allocation"));
        let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(receipt["targets"], 2); assert_eq!(receipt["published"], true);
        assert_eq!(receipt["render_font_native"], "unverified");
        let preview = std::fs::read_to_string(workspace.0.join("preview.json")).unwrap();
        assert!(preview.contains("PRIVATE_LITERAL_700"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE_LITERAL_700"));
    }
}

#[test]
fn stale_plan_refuses_without_replacing_any_artifact() {
    let workspace = Workspace::new("stale"); let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let mut operation = binding_operation(&workspace, &source, "PRIVATE_STALE");
    operation["plan"]["source_sha256"] = "0".repeat(64).into();
    for file in [&output, &workspace.0.join("receipt.json"), &workspace.0.join("preview.json")] { std::fs::write(file, b"unchanged").unwrap(); }
    let result = edit(&workspace, &source, &output, operation, false);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("source_sha256"), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE_STALE"));
    for file in [&output, &workspace.0.join("receipt.json"), &workspace.0.join("preview.json")] { assert_eq!(std::fs::read(file).unwrap(), b"unchanged"); }
}

#[test]
fn binding_preview_validates_native_output_without_publishing_the_document() {
    let workspace = Workspace::new("dry-run"); let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let result = edit(&workspace, &source, &output, binding_operation(&workspace, &source, "700"), true);
    success(&result, "binding dry-run"); assert!(!output.exists());
    let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(workspace.0.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["published"], false); assert!(workspace.0.join("preview.json").is_file());
}
