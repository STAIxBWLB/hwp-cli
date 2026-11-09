//! Source-bound binding workflow; generated native inputs, no host fonts or private corpus.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
        assert!(
            occurrences
                .iter()
                .all(|item| item["label_occurrences"] == 2 && item["label_ambiguous"] == true),
            "a repeated label is evidence, never an implicit single-target selector"
        );
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
    let output = hwp()
        .args(["slots", "--json", "--forms", "--occurrences"])
        .arg(source)
        .output()
        .unwrap();
    success(&output, "discovery setup");
    let value = serde_json::from_slice(&output.stdout).unwrap();
    schema(
        &value,
        include_str!("../../../schemas/form-discovery-v1.schema.json"),
    );
    value
}

fn schema(value: &Value, definition: &str) {
    let definition: Value = serde_json::from_str(definition).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&definition)
        .unwrap();
    let errors: Vec<_> = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "closed artifact schema: {errors:?}");
}

fn binding_operation(
    workspace: &Workspace,
    source: &std::path::Path,
    value: &str,
) -> serde_json::Value {
    let catalogue = discovery(source);
    let targets: Vec<_> = catalogue["occurrences"].as_array().unwrap().iter()
        .filter(|item| item["kind"] == "placeholder")
        .map(|item| serde_json::json!({"occurrence_id":item["id"],"copies":[0],"max_chars":100,"allow_reflow":true})).collect();
    serde_json::json!({"op":"apply_form_bindings", "plan": {
        "version":1,"source_sha256":catalogue["source_sha256"],"structural_sha256":catalogue["structural_sha256"],
        "bindings":[{"key":"allocation","targets":targets}],"repeats":[],"constraints":[]},
        "values":{"allocation":value}, "receipt":workspace.0.join("receipt.json"), "preview":workspace.0.join("preview.json")})
}

fn edit(
    workspace: &Workspace,
    source: &std::path::Path,
    output: &std::path::Path,
    operation: serde_json::Value,
    dry_run: bool,
) -> Output {
    let ops = workspace.0.join("ops.json");
    std::fs::write(&ops, serde_json::to_vec(&vec![operation]).unwrap()).unwrap();
    let mut command = hwp();
    command
        .arg("edit")
        .arg(source)
        .arg("--ops")
        .arg(ops)
        .arg("-o")
        .arg(output)
        .arg("--verify");
    if dry_run {
        command.arg("--dry-run");
    }
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
        let cat = hwp().arg("cat").arg(&output).output().unwrap();
        success(&cat, "read back");
        let text = String::from_utf8_lossy(&cat.stdout);
        assert_eq!(text.matches("PRIVATE_LITERAL_700").count(), 2);
        assert!(!text.contains("{{ allocation }}"));
        let receipt = std::fs::read_to_string(workspace.0.join("receipt.json")).unwrap();
        assert!(!receipt.contains("PRIVATE_LITERAL_700"));
        assert!(!receipt.contains("allocation"));
        let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        schema(
            &receipt,
            include_str!("../../../schemas/form-binding-receipt-v1.schema.json"),
        );
        assert_eq!(receipt["targets"], 2);
        assert_eq!(receipt["published"], true);
        assert_eq!(receipt["render_font_native"], "unverified");
        let preview = std::fs::read_to_string(workspace.0.join("preview.json")).unwrap();
        schema(
            &serde_json::from_str(&preview).unwrap(),
            include_str!("../../../schemas/form-binding-preview-v1.schema.json"),
        );
        assert!(preview.contains("PRIVATE_LITERAL_700"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE_LITERAL_700"));
    }
}

#[test]
fn stale_plan_refuses_without_replacing_any_artifact() {
    let workspace = Workspace::new("stale");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let mut operation = binding_operation(&workspace, &source, "PRIVATE_STALE");
    operation["plan"]["source_sha256"] = "0".repeat(64).into();
    for file in [
        &output,
        &workspace.0.join("receipt.json"),
        &workspace.0.join("preview.json"),
    ] {
        std::fs::write(file, b"unchanged").unwrap();
    }
    let result = edit(&workspace, &source, &output, operation, false);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("source_sha256"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE_STALE"));
    for file in [
        &output,
        &workspace.0.join("receipt.json"),
        &workspace.0.join("preview.json"),
    ] {
        assert_eq!(std::fs::read(file).unwrap(), b"unchanged");
    }
}

#[test]
fn a_repeated_label_cannot_replace_explicit_reviewed_occurrence_ids() {
    let workspace = Workspace::new("ambiguous-label");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let mut operation = binding_operation(&workspace, &source, "PRIVATE_AMBIGUOUS");
    operation["plan"]["bindings"][0]["targets"] =
        json!([{"label":"allocation","copies":[0],"max_chars":100,"allow_reflow":true}]);
    let result = edit(&workspace, &source, &output, operation, false);
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE_AMBIGUOUS"));
    assert!(!output.exists());
    assert!(!workspace.0.join("receipt.json").exists());
    assert!(!workspace.0.join("preview.json").exists());
}

#[test]
fn binding_preview_validates_native_output_without_publishing_the_document() {
    let workspace = Workspace::new("dry-run");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let result = edit(
        &workspace,
        &source,
        &output,
        binding_operation(&workspace, &source, "700"),
        true,
    );
    success(&result, "binding dry-run");
    assert!(!output.exists());
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.0.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["published"], false);
    assert!(workspace.0.join("preview.json").is_file());
}

#[test]
fn binding_cli_file_stdin_and_mcp_outputs_and_receipts_are_identical() {
    for extension in ["hwp", "hwpx"] {
        let workspace = Workspace::new(&format!("parity-{extension}"));
        let source = workspace.source(extension);
        let catalogue = discovery(&source);
        let response = mcp_tool(
            "hwp_slots",
            json!({"path":source,"forms":true,"occurrences":true}),
        );
        assert!(
            response.get("error").is_none() && response["result"]["isError"] != true,
            "{response}"
        );
        let mcp_catalogue: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(catalogue, mcp_catalogue, "discovery parity");
        let operation = binding_operation(&workspace, &source, "700");
        let file_output = workspace.0.join(format!("file.{extension}"));
        success(
            &edit(&workspace, &source, &file_output, operation.clone(), false),
            "file channel",
        );
        let expected_output = std::fs::read(&file_output).unwrap();
        let expected_receipt = std::fs::read(workspace.0.join("receipt.json")).unwrap();
        let expected_preview = std::fs::read(workspace.0.join("preview.json")).unwrap();
        let stdin_output = workspace.0.join(format!("stdin.{extension}"));
        let mut child = hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&stdin_output)
            .args(["--ops", "-", "--verify"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&vec![operation.clone()]).unwrap())
            .unwrap();
        success(&child.wait_with_output().unwrap(), "stdin channel");
        assert_eq!(expected_output, std::fs::read(stdin_output).unwrap());
        assert_eq!(
            expected_receipt,
            std::fs::read(workspace.0.join("receipt.json")).unwrap()
        );
        let mcp_output = workspace.0.join(format!("mcp.{extension}"));
        let mut item = operation;
        item.as_object_mut().unwrap().remove("op");
        let response = mcp_tool(
            "hwp_edit",
            json!({"input":source,"output":mcp_output,"apply_form_bindings":item}),
        );
        assert!(
            response.get("error").is_none() && response["result"]["isError"] != true,
            "{response}"
        );
        assert_eq!(expected_output, std::fs::read(mcp_output).unwrap());
        assert_eq!(
            expected_receipt,
            std::fs::read(workspace.0.join("receipt.json")).unwrap()
        );
        assert_eq!(
            expected_preview,
            std::fs::read(workspace.0.join("preview.json")).unwrap()
        );
    }
}

#[test]
fn malformed_binding_payload_refuses_without_echoing_private_values() {
    let workspace = Workspace::new("redaction");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let mut operation = binding_operation(&workspace, &source, "PRIVATE_VALIDATION_VALUE");
    operation["plan"]["unexpected_PRIVATE_FIELD"] = json!("PRIVATE_VALIDATION_VALUE");
    let result = edit(&workspace, &source, &output, operation, false);
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("스키마") || error.contains("계약"),
        "{error}"
    );
    assert!(!error.contains("PRIVATE_"), "{error}");
    assert!(!output.exists());
    assert!(!workspace.0.join("receipt.json").exists());
}

#[test]
fn a_missing_ops_array_wrapper_never_echoes_binding_content() {
    let workspace = Workspace::new("wrong-wrapper");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let operation = binding_operation(&workspace, &source, "PRIVATE_WRAPPER_VALUE");
    for payload in [
        operation.clone(),
        json!({"ops":[operation.clone()]}),
        json!([[operation.clone()]]),
        json!(serde_json::to_string(&operation).unwrap()),
    ] {
        let ops = workspace.0.join("wrong.json");
        std::fs::write(&ops, serde_json::to_vec(&payload).unwrap()).unwrap();
        let result = hwp()
            .arg("edit")
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .arg("--ops")
            .arg(&ops)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("PRIVATE_WRAPPER_VALUE"),
            "wrong-wrapper refusals must omit private payloads"
        );
        assert!(!output.exists());
    }
}

#[test]
fn mixed_binding_batches_refuse_before_legacy_semantic_parsing() {
    let workspace = Workspace::new("mixed-binding");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let operation = binding_operation(&workspace, &source, "PRIVATE_VALUE");
    let ops = workspace.0.join("mixed.json");
    std::fs::write(
        &ops,
        serde_json::to_vec(
            &json!([operation, {"op":"set_format","pattern":"PRIVATE_SCOPE","size":"1001pt"}]),
        )
        .unwrap(),
    )
    .unwrap();
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(ops)
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("단독"),
        "the compound boundary precedes unrelated legacy parsing: {error}"
    );
    assert!(!error.contains("PRIVATE_"));
    assert!(!output.exists());
}

#[test]
fn mixed_mcp_binding_requests_never_echo_legacy_values() {
    let workspace = Workspace::new("mixed-mcp");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let mut item = binding_operation(&workspace, &source, "PRIVATE_VALUE");
    item.as_object_mut().unwrap().remove("op");
    let response = mcp_tool(
        "hwp_edit",
        json!({"input":source,"output":output,"apply_form_bindings":item,
        "set_format":[{"pattern":"probe","color":"PRIVATE_COLOR"}]}),
    );
    assert!(response["result"]["isError"] == true || response.get("error").is_some());
    assert!(
        !response.to_string().contains("PRIVATE_"),
        "binding exclusivity must precede unrelated value parsing"
    );
    assert!(!output.exists());
    assert!(!workspace.0.join("receipt.json").exists());
}

#[test]
fn a_component_style_named_values_does_not_turn_a_legacy_batch_into_a_binding() {
    let workspace = Workspace::new("legacy-values-style");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("legacy.hwpx");
    let ops = workspace.0.join("legacy.json");
    std::fs::write(&ops, serde_json::to_vec(&json!([
        {"op":"insert_blocks","address":{"at":{"section":0,"paragraph":1}},"position":"after","missing_style":"create",
         "styles":{"values":{"font_size_pt":10}},"blocks":[{"type":"paragraph","style":"values","runs":[{"type":"text","text":"Legacy component"}]}]},
        {"op":"replace","from":"Application","to":"Reviewed application"}
    ])).unwrap()).unwrap();
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(ops)
        .arg("--verify")
        .output()
        .unwrap();
    success(
        &result,
        "ordinary component keys do not declare the compound operation",
    );
    let result = hwp().arg("cat").arg(output).output().unwrap();
    success(&result, "legacy read-back");
    let text = String::from_utf8_lossy(&result.stdout);
    assert!(text.contains("Legacy component"));
    assert!(text.contains("Reviewed application"));
}

#[test]
fn embedded_binding_plan_schema_matches_the_standalone_contract() {
    fn normalize(value: &Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(key, value)| {
                        let value = if key == "$ref" {
                            value
                                .as_str()
                                .and_then(|reference| {
                                    reference.strip_prefix("#/$defs/formBinding_")
                                })
                                .map(|suffix| Value::String(format!("#/$defs/{suffix}")))
                                .unwrap_or_else(|| normalize(value))
                        } else {
                            normalize(value)
                        };
                        (key.clone(), value)
                    })
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
            _ => value.clone(),
        }
    }
    let standalone: Value = serde_json::from_str(include_str!(
        "../../../schemas/form-binding-plan-v1.schema.json"
    ))
    .unwrap();
    let embedded: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-ops-v1.schema.json")).unwrap();
    for (name, definition) in standalone["$defs"].as_object().unwrap() {
        assert_eq!(
            *definition,
            normalize(&embedded["$defs"][format!("formBinding_{name}")]),
            "{name}"
        );
    }
    let expected: Value = Value::Object(
        standalone
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| !key.starts_with('$'))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    assert_eq!(expected, normalize(&embedded["$defs"]["formBindingPlan"]));
}

#[test]
fn binding_artifact_aliases_and_legacy_report_refuse_before_any_publish() {
    let workspace = Workspace::new("artifact-guards");
    let source = workspace.source("hwpx");
    let output = workspace.0.join("filled.hwpx");
    let mut operation = binding_operation(&workspace, &source, "700");
    operation["receipt"] = json!(output);
    let result = edit(&workspace, &source, &output, operation, false);
    assert!(!result.status.success());
    assert!(!output.exists());
    assert!(!workspace.0.join("preview.json").exists());
    let operation = binding_operation(&workspace, &source, "700");
    let ops = workspace.0.join("ops.json");
    std::fs::write(&ops, serde_json::to_vec(&vec![operation]).unwrap()).unwrap();
    let report = workspace.0.join("legacy-report.json");
    let result = hwp()
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(ops)
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("receipt"));
    assert!(!output.exists());
    assert!(!report.exists());
    assert!(!workspace.0.join("receipt.json").exists());
}

fn immutable_ops_alias_case(field: &str, spelling: u8, extension: &str) {
    let workspace = Workspace::new(&format!("immutable-ops-{field}-{spelling}-{extension}"));
    let source = workspace.source(extension);
    let source_bytes = std::fs::read(&source).unwrap();
    let ops = workspace.0.join("plan.json");
    let alias = match spelling {
        0 => PathBuf::from("plan.json"),
        1 => PathBuf::from("./plan.json"),
        _ => workspace.0.canonicalize().unwrap().join("plan.json"),
    };
    let mut operation = binding_operation(&workspace, &source, "700");
    operation[field] = json!(alias);
    let plan_bytes = serde_json::to_vec(&json!([operation])).unwrap();
    std::fs::write(&ops, &plan_bytes).unwrap();
    let counterpart = workspace.0.join(if field == "receipt" {
        "preview.json"
    } else {
        "receipt.json"
    });
    std::fs::write(&counterpart, b"unchanged artifact").unwrap();
    let output = workspace.0.join(format!("bound.{extension}"));
    let result = hwp()
        .current_dir(&workspace.0)
        .arg("edit")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(["--ops", "plan.json", "--verify"])
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "an immutable ops alias must refuse {field}/{spelling}/{extension}"
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("immutable"));
    assert_eq!(std::fs::read(&ops).unwrap(), plan_bytes);
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    assert_eq!(std::fs::read(counterpart).unwrap(), b"unchanged artifact");
    assert!(!output.exists(), "no native output may be published");
}

macro_rules! immutable_ops_alias_test {
    ($name:ident, $field:literal, $spelling:literal, $extension:literal) => {
        #[test]
        fn $name() {
            immutable_ops_alias_case($field, $spelling, $extension);
        }
    };
}

immutable_ops_alias_test!(immutable_ops_receipt_plain_hwp, "receipt", 0, "hwp");
immutable_ops_alias_test!(immutable_ops_receipt_dot_hwp, "receipt", 1, "hwp");
immutable_ops_alias_test!(immutable_ops_receipt_absolute_hwp, "receipt", 2, "hwp");
immutable_ops_alias_test!(immutable_ops_preview_plain_hwp, "preview", 0, "hwp");
immutable_ops_alias_test!(immutable_ops_preview_dot_hwp, "preview", 1, "hwp");
immutable_ops_alias_test!(immutable_ops_preview_absolute_hwp, "preview", 2, "hwp");
immutable_ops_alias_test!(immutable_ops_receipt_plain_hwpx, "receipt", 0, "hwpx");
immutable_ops_alias_test!(immutable_ops_receipt_dot_hwpx, "receipt", 1, "hwpx");
immutable_ops_alias_test!(immutable_ops_receipt_absolute_hwpx, "receipt", 2, "hwpx");
immutable_ops_alias_test!(immutable_ops_preview_plain_hwpx, "preview", 0, "hwpx");
immutable_ops_alias_test!(immutable_ops_preview_dot_hwpx, "preview", 1, "hwpx");
immutable_ops_alias_test!(immutable_ops_preview_absolute_hwpx, "preview", 2, "hwpx");

fn immutable_source_alias_case(field: &str, spelling: u8, extension: &str) {
    let workspace = Workspace::new(&format!("immutable-{field}-{spelling}-{extension}"));
    let source = workspace.source(extension);
    let original = std::fs::read(&source).unwrap();
    let name = source.file_name().unwrap().to_str().unwrap();
    let absolute = source.canonicalize().unwrap();
    let (input, alias) = match spelling {
        0 => (PathBuf::from(name), absolute),
        1 => (absolute, PathBuf::from(name)),
        _ => (PathBuf::from(name), PathBuf::from(format!("./{name}"))),
    };
    let mut operation = binding_operation(&workspace, &source, "700");
    operation[field] = json!(alias);
    let ops = workspace.0.join("alias.ops.json");
    std::fs::write(&ops, serde_json::to_vec(&json!([operation])).unwrap()).unwrap();
    let counterpart = workspace.0.join(if field == "receipt" {
        "preview.json"
    } else {
        "receipt.json"
    });
    std::fs::write(&counterpart, b"unchanged artifact").unwrap();
    let output = workspace.0.join(format!("bound.{extension}"));
    let result = hwp()
        .current_dir(&workspace.0)
        .arg("edit")
        .arg(input)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(ops)
        .arg("--verify")
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "an immutable source alias must refuse {field}/{spelling}/{extension}"
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        original,
        "source bytes must survive an artifact alias"
    );
    assert_eq!(std::fs::read(counterpart).unwrap(), b"unchanged artifact");
    assert!(
        !output.exists(),
        "no native output may be published on refusal"
    );
}

macro_rules! immutable_source_alias_test {
    ($name:ident, $field:literal, $spelling:literal, $extension:literal) => {
        #[test]
        fn $name() {
            immutable_source_alias_case($field, $spelling, $extension);
        }
    };
}
immutable_source_alias_test!(
    immutable_source_alias_receipt_absolute_hwp,
    "receipt",
    0,
    "hwp"
);
immutable_source_alias_test!(
    immutable_source_alias_receipt_relative_hwp,
    "receipt",
    1,
    "hwp"
);
immutable_source_alias_test!(immutable_source_alias_receipt_dot_hwp, "receipt", 2, "hwp");
immutable_source_alias_test!(
    immutable_source_alias_preview_absolute_hwp,
    "preview",
    0,
    "hwp"
);
immutable_source_alias_test!(
    immutable_source_alias_preview_relative_hwp,
    "preview",
    1,
    "hwp"
);
immutable_source_alias_test!(immutable_source_alias_preview_dot_hwp, "preview", 2, "hwp");
immutable_source_alias_test!(
    immutable_source_alias_receipt_absolute_hwpx,
    "receipt",
    0,
    "hwpx"
);
immutable_source_alias_test!(
    immutable_source_alias_receipt_relative_hwpx,
    "receipt",
    1,
    "hwpx"
);
immutable_source_alias_test!(
    immutable_source_alias_receipt_dot_hwpx,
    "receipt",
    2,
    "hwpx"
);
immutable_source_alias_test!(
    immutable_source_alias_preview_absolute_hwpx,
    "preview",
    0,
    "hwpx"
);
immutable_source_alias_test!(
    immutable_source_alias_preview_relative_hwpx,
    "preview",
    1,
    "hwpx"
);
immutable_source_alias_test!(
    immutable_source_alias_preview_dot_hwpx,
    "preview",
    2,
    "hwpx"
);

#[test]
fn immutable_source_alias_merge_output_cannot_replace_an_input() {
    let workspace = Workspace::new("immutable-merge");
    let source = workspace.source("hwpx");
    let original = std::fs::read(&source).unwrap();
    let second = workspace.0.join("second.hwpx");
    std::fs::copy(&source, &second).unwrap();
    let result = hwp()
        .current_dir(&workspace.0)
        .args(["merge", "source.hwpx", "second.hwpx", "-o"])
        .arg(source.canonicalize().unwrap())
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "merge must refuse a differently spelled immutable input"
    );
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(std::fs::read(second).unwrap(), original);
}

#[test]
fn binding_values_are_literal_text_in_both_native_formats() {
    for extension in ["hwp", "hwpx"] {
        let workspace = Workspace::new(&format!("literal-{extension}"));
        let source = workspace.source(extension);
        let output = workspace.0.join(format!("filled.{extension}"));
        let value = "<table>&|# literal\nnext\t列";
        success(
            &edit(
                &workspace,
                &source,
                &output,
                binding_operation(&workspace, &source, value),
                false,
            ),
            "literal binding",
        );
        let doc = if extension == "hwp" {
            hwp5::read_document(&output).unwrap().document
        } else {
            hwpx::read_document(&output).unwrap().document
        };
        let matches = doc
            .sections
            .iter()
            .flat_map(|section| &section.paragraphs)
            .filter(|para| para.own_text().trim_end() == value)
            .count();
        assert_eq!(
            matches, 2,
            "literal markup and controls must not become document blocks"
        );
    }
}

fn same_key_inline_placeholder_cli_case(extension: &str, inline_first: bool) {
    let workspace = Workspace::new(&format!("same-key-overlap-{extension}-{inline_first}"));
    let markdown = workspace.0.join("fixture.md");
    std::fs::write(&markdown, "# Fixture\n\nName: {{Name}}\n").unwrap();
    let source = workspace.0.join(format!("source.{extension}"));
    success(
        &hwp()
            .args(["new", "--from"])
            .arg(markdown)
            .arg("-o")
            .arg(&source)
            .output()
            .unwrap(),
        "overlap fixture creation",
    );
    let original = std::fs::read(&source).unwrap();
    let catalogue = discovery(&source);
    let items = catalogue["occurrences"].as_array().unwrap();
    let inline = items
        .iter()
        .find(|item| item["kind"] == "inline_value" && item["label"] == "Name")
        .expect("inline fixture claim");
    let slot = items
        .iter()
        .find(|item| item["kind"] == "placeholder" && item["label"] == "Name")
        .expect("slot fixture claim");
    assert!(inline["bindable"] == true && slot["bindable"] == true);
    assert_eq!(inline["end"], slot["end"]);
    assert_eq!(inline["start"], slot["start"]);
    let mut bindings = vec![
        json!({"key":"inline","targets":[{"occurrence_id":inline["id"],"copies":[0],"max_chars":100,"allow_reflow":true}]}),
        json!({"key":"slot","targets":[{"occurrence_id":slot["id"],"copies":[0],"max_chars":100,"allow_reflow":true}]}),
    ];
    if !inline_first {
        bindings.reverse();
    }
    let operation = json!({"op":"apply_form_bindings", "plan":{"version":1,"source_sha256":catalogue["source_sha256"],
        "structural_sha256":catalogue["structural_sha256"],"bindings":bindings,"repeats":[],"constraints":[]},
        "values":{"inline":"B","slot":"A"},"receipt":workspace.0.join("receipt.json"),"preview":workspace.0.join("preview.json")});
    let output = workspace.0.join(format!("bound.{extension}"));
    std::fs::write(&output, b"original output").unwrap();
    std::fs::write(workspace.0.join("receipt.json"), b"original receipt").unwrap();
    std::fs::write(workspace.0.join("preview.json"), b"original preview").unwrap();
    let result = edit(&workspace, &source, &output, operation, false);
    assert!(
        !result.status.success(),
        "overlapping inline and placeholder claims must refuse in either plan order"
    );
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(std::fs::read(output).unwrap(), b"original output");
    assert_eq!(
        std::fs::read(workspace.0.join("receipt.json")).unwrap(),
        b"original receipt"
    );
    assert_eq!(
        std::fs::read(workspace.0.join("preview.json")).unwrap(),
        b"original preview"
    );
}

#[test]
fn same_key_overlap_inline_first_hwp_refuses() {
    same_key_inline_placeholder_cli_case("hwp", true);
}
#[test]
fn same_key_overlap_slot_first_hwp_refuses() {
    same_key_inline_placeholder_cli_case("hwp", false);
}
#[test]
fn same_key_overlap_inline_first_hwpx_refuses() {
    same_key_inline_placeholder_cli_case("hwpx", true);
}
#[test]
fn same_key_overlap_slot_first_hwpx_refuses() {
    same_key_inline_placeholder_cli_case("hwpx", false);
}

fn repeated_plan_report_case(extension: &str, bind: bool) {
    let workspace = Workspace::new(&format!("repeat-report-{extension}-{bind}"));
    let markdown = workspace.0.join("profile.md");
    std::fs::write(
        &markdown,
        "# Profile\n\n| Label | Value |\n| --- | --- |\n| Name | {{name}} |\n\nAfter\n",
    )
    .unwrap();
    let source = workspace.0.join(format!("source.{extension}"));
    success(
        &hwp()
            .args(["new", "--from"])
            .arg(markdown)
            .arg("-o")
            .arg(&source)
            .output()
            .unwrap(),
        "repeat report source",
    );
    let catalogue = discovery(&source);
    let target = catalogue["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "placeholder" && item["label"] == "name")
        .unwrap();
    let doc = if extension == "hwp" {
        hwp5::read_document(&source).unwrap().document
    } else {
        hwpx::read_document(&source).unwrap().document
    };
    let index = doc.sections[0]
        .paragraphs
        .iter()
        .position(|paragraph| paragraph.own_text().trim() == "After")
        .unwrap();
    let route = hwp_convert::SegmentPath {
        section: 0,
        indices: vec![index],
    };
    let anchor = hwp_convert::paragraph_id(&route, &doc.sections[0].paragraphs[index]);
    let bindings = if bind {
        json!([{"key":"name","targets":[{"occurrence_id":target["id"],"copies":[0,1,2],"max_chars":100,"allow_reflow":true}]}])
    } else {
        json!([])
    };
    let values = if bind {
        json!({"name":"updated"})
    } else {
        json!({})
    };
    let output = workspace.0.join(format!("bound.{extension}"));
    let request = json!({"input":source,"output":output,"apply_form_bindings":{
        "plan":{"version":1,"source_sha256":catalogue["source_sha256"],"structural_sha256":catalogue["structural_sha256"],
            "bindings":bindings,"repeats":[{"table_id":target["table_id"],"after_paragraph_id":anchor,
                "copies":3,"profile_ids":["original","copy1","copy2"]}],"constraints":[]},
        "values":values,"receipt":workspace.0.join("receipt.json"),"preview":workspace.0.join("preview.json")}});
    let result = mcp_tool("hwp_edit", request);
    assert_ne!(result["result"]["isError"], true, "{result}");
    let report: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let expected = if bind { 5 } else { 2 };
    assert_eq!(
        report["ops"][0]["pieces_touched"], expected,
        "clones and replacements must both count"
    );
    assert_eq!(report["applied"], expected);
    let envelope = json!({"schema_version":"1.0","contract":"hwp-edit-report-v1",
        "output":report["output"],"dry_run":report["dry_run"],"ops":report["ops"],
        "applied_count":1,"failed_count":0});
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    assert!(validator.is_valid(&envelope), "{envelope}");
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(workspace.0.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["cloned_profiles"], 2);
    assert_eq!(receipt["targets"], if bind { 3 } else { 0 });
}

#[test]
fn clone_only_report_counts_hwp_pieces() {
    repeated_plan_report_case("hwp", false);
}
#[test]
fn clone_only_report_counts_hwpx_pieces() {
    repeated_plan_report_case("hwpx", false);
}
#[test]
fn mixed_clone_report_counts_hwp_pieces() {
    repeated_plan_report_case("hwp", true);
}
#[test]
fn mixed_clone_report_counts_hwpx_pieces() {
    repeated_plan_report_case("hwpx", true);
}

fn mcp_tool(name: &str, arguments: Value) -> Value {
    let mut child = hwp()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut send = |value: Value| {
        writeln!(input, "{value}").unwrap();
        input.flush().unwrap();
    };
    let mut receive = || -> Value {
        let line = match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(line) => line,
            Err(error) => {
                let _ = child.kill();
                panic!("MCP response deadline: {error}");
            }
        };
        serde_json::from_str(&line).unwrap()
    };
    send(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"binding-parity","version":"0"}}}),
    );
    let _ = receive();
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let tools = receive();
    let schema = &tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .expect("called tool must be advertised")["inputSchema"];
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(schema)
        .expect("advertised MCP input schema must compile");
    let errors: Vec<_> = validator
        .iter_errors(&arguments)
        .map(|error| error.to_string())
        .collect();
    send(
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
    );
    let result = receive();
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("MCP failed to stop after EOF");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        errors.is_empty(),
        "MCP {name} request must satisfy its advertised-schema expectation: {errors:?}"
    );
    result
}
