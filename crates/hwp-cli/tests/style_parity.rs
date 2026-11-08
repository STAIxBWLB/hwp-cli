//! Cross-channel named-style contracts on small, owner-authored native documents (#427 T5).
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
#[path = "common/temp_path.rs"]
mod temp_path;

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hwp"))
}

fn success(result: std::process::Output) {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

struct Fixture {
    root: temp_path::TempPath,
    source: PathBuf,
    extension: &'static str,
}

impl Fixture {
    fn new(label: &str, extension: &'static str) -> Self {
        let root = temp_path::TempPath::new("style-parity", &format!("{label}-{extension}"));
        std::fs::create_dir_all(&*root).unwrap();
        let source = root.join(format!("base.{extension}"));
        let doc = hwp_convert::from_markdown("Style probe.");
        match extension {
            "hwp" => {
                hwp5::write_document(&doc, &source, &hwp5::WriteOptions::default()).unwrap();
            }
            _ => {
                hwpx::write_document(&doc, &source).unwrap();
            }
        }
        Self {
            root,
            source,
            extension,
        }
    }

    fn output(&self, label: &str) -> PathBuf {
        self.root.join(format!("{label}.{}", self.extension))
    }

    fn ops(&self, input: &Path, label: &str, operations: Value) -> PathBuf {
        let file = self.root.join(format!("{label}.json"));
        let output = self.output(label);
        std::fs::write(&file, serde_json::to_vec(&operations).unwrap()).unwrap();
        success(
            hwp()
                .arg("edit")
                .arg(input)
                .arg("-o")
                .arg(&output)
                .arg("--ops")
                .arg(file)
                .arg("--verify")
                .output()
                .unwrap(),
        );
        output
    }
}

/// A bounded stdio exchange through the actual MCP authority and edit boundary.
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
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"style-parity","version":"0"}}}),
    );
    let _ = receive();
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    send(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
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
    result
}

fn create() -> Value {
    json!({"op":"create_style","name":"Body","kind":"paragraph","based_on":0,"size":"13pt"})
}
fn apply() -> Value {
    json!({"op":"apply_style","style":"Body","address":{"at":{"section":0,"paragraph":0}}})
}
fn update() -> Value {
    json!({"op":"update_style","style":0,"size":"13pt","line_spacing_pct":"190%"})
}

fn channel_parity(extension: &'static str, operation: Value) {
    let op = operation["op"].as_str().unwrap();
    let fixture = Fixture::new(op, extension);
    let source = if op == "apply_style" {
        fixture.ops(&fixture.source, "created", json!([create()]))
    } else {
        fixture.source.clone()
    };
    compare_channels(&fixture, &source, vec![operation]);
}

fn compare_channels(fixture: &Fixture, source: &Path, operations: Vec<Value>) {
    let expected = fixture.ops(source, "ops-result", json!(operations));
    let stdin_output = fixture.output("stdin-result");
    let mut child = hwp()
        .arg("edit")
        .arg(source)
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
        .write_all(&serde_json::to_vec(&operations).unwrap())
        .unwrap();
    success(child.wait_with_output().unwrap());
    let expected_bytes = std::fs::read(&expected).unwrap();
    assert_eq!(
        expected_bytes,
        std::fs::read(stdin_output).unwrap(),
        "file and stdin requests must be byte-identical"
    );
    let output = fixture.output("mcp-result");
    let mut arguments = json!({"input":source,"output":output,"verify":true});
    for operation in &operations {
        let op = operation["op"].as_str().unwrap();
        let mut item = operation.clone();
        item.as_object_mut().unwrap().remove("op");
        if arguments.get(op).is_none() {
            arguments[op] = json!([]);
        }
        arguments[op].as_array_mut().unwrap().push(item);
    }
    let response = mcp_tool("hwp_edit", arguments);
    assert!(
        response.get("error").is_none() && response["result"]["isError"] != true,
        "MCP must execute the same request as --ops: {response}"
    );
    assert!(output.is_file(), "MCP must publish a native output");
    assert_eq!(
        expected_bytes,
        std::fs::read(&output).unwrap(),
        "channel output must be byte-identical"
    );
    success(hwp().arg("validate").arg(&output).output().unwrap());
    let cli = hwp()
        .arg("styles")
        .arg(&output)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "styles CLI must list the edited output"
    );
    let listing: Value = serde_json::from_slice(&cli.stdout).unwrap();
    let response = mcp_tool("hwp_styles", json!({"path":output}));
    assert!(
        response.get("error").is_none() && response["result"]["isError"] != true,
        "hwp_styles must list the edited output: {response}"
    );
    let from_mcp: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        listing, from_mcp,
        "styles CLI and MCP must expose identical attributes and used_by IDs"
    );
}

fn ordered_batch(extension: &'static str) {
    let fixture = Fixture::new("ordered-batch", extension);
    compare_channels(
        &fixture,
        &fixture.source,
        vec![
            create(),
            json!({"op":"update_style","style":"Body","size":"14pt"}),
            apply(),
        ],
    );
}

#[test]
fn ordered_style_batch_matches_all_channels_hwp() {
    ordered_batch("hwp");
}
#[test]
fn ordered_style_batch_matches_all_channels_hwpx() {
    ordered_batch("hwpx");
}

#[test]
fn create_mcp_matches_ops_hwp() {
    channel_parity("hwp", create());
}
#[test]
fn create_mcp_matches_ops_hwpx() {
    channel_parity("hwpx", create());
}
#[test]
fn apply_mcp_matches_ops_hwp() {
    channel_parity("hwp", apply());
}
#[test]
fn apply_mcp_matches_ops_hwpx() {
    channel_parity("hwpx", apply());
}
#[test]
fn update_mcp_matches_ops_hwp() {
    channel_parity("hwp", update());
}
#[test]
fn update_mcp_matches_ops_hwpx() {
    channel_parity("hwpx", update());
}

fn repeated(extension: &'static str, operation: Value) {
    let op = operation["op"].as_str().unwrap();
    let fixture = Fixture::new(&format!("repeat-{op}"), extension);
    let source = if op == "apply_style" {
        fixture.ops(&fixture.source, "created", json!([create()]))
    } else {
        fixture.source.clone()
    };
    let once = fixture.ops(&source, "once", json!([operation.clone()]));
    let twice = fixture.ops(&once, "twice", json!([operation]));
    assert_eq!(
        std::fs::read(once).unwrap(),
        std::fs::read(&twice).unwrap(),
        "repeating {op} must not allocate shapes or rewrite native bytes"
    );
    success(hwp().arg("validate").arg(twice).output().unwrap());
}

fn duplicate_create(extension: &'static str) {
    let fixture = Fixture::new("duplicate-create", extension);
    let once = fixture.ops(&fixture.source, "once", json!([create()]));
    let before = std::fs::read(&once).unwrap();
    let request = fixture.root.join("duplicate.json");
    std::fs::write(&request, serde_json::to_vec(&json!([create()])).unwrap()).unwrap();
    let destination = fixture.output("duplicate-refused");
    std::fs::write(&destination, b"previous output must survive").unwrap();
    let result = hwp()
        .arg("edit")
        .arg(&once)
        .arg("-o")
        .arg(&destination)
        .arg("--ops")
        .arg(request)
        .output()
        .unwrap();
    assert!(!result.status.success(), "duplicate creation must refuse");
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"previous output must survive"
    );
    assert_eq!(std::fs::read(once).unwrap(), before);
}

#[test]
fn identical_apply_is_byte_stable_hwp() {
    repeated("hwp", apply());
}
#[test]
fn identical_apply_is_byte_stable_hwpx() {
    repeated("hwpx", apply());
}
#[test]
fn identical_update_is_byte_stable_hwp() {
    repeated("hwp", update());
}
#[test]
fn identical_update_is_byte_stable_hwpx() {
    repeated("hwpx", update());
}
#[test]
fn duplicate_create_refuses_atomically_hwp() {
    duplicate_create("hwp");
}
#[test]
fn duplicate_create_refuses_atomically_hwpx() {
    duplicate_create("hwpx");
}

#[path = "common/fixture_skip.rs"]
mod fixture_skip;

/// A short text with a pinned host font exercises default certification rules without
/// asserting a page count, glyph positions or any font-dependent metric.
fn certifies(extension: &'static str, operation: Value) {
    use sha2::{Digest, Sha256};
    let op = operation["op"].as_str().unwrap();
    let fixture = Fixture::new(&format!("certify-{op}"), extension);
    let mut fonts = hwp_render::FontStore::new();
    if let Some(dir) = std::env::var_os("HWP_FONT_DIR") {
        fonts.load_dir(Path::new(&dir));
    }
    let Some(font) = fonts.font_covering('A') else {
        fixture_skip::resource_unavailable(
            "ASCII system font for named-style certification",
            "install a system font or set HWP_FONT_DIR, then rerun style_parity",
        );
        return;
    };
    if font.data.len() > hwp_cli::certification::MAX_FONT_FILE_BYTES as usize
        || font.family.is_empty()
    {
        fixture_skip::resource_unavailable(
            "bounded named ASCII font for certification",
            "provide a named font under 32 MiB through HWP_FONT_DIR and rerun style_parity",
        );
        return;
    }
    let pin = fixture.root.join("font.bin");
    std::fs::write(&pin, &*font.data).unwrap();
    let mut isolated = hwp_render::FontStore::new_isolated();
    isolated.load_file(&pin).unwrap();
    if !"Style probe."
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .all(|ch| isolated.font_covering(ch).is_some())
    {
        fixture_skip::resource_unavailable(
            "one pinned font covering the authored ASCII probe",
            "provide an ASCII font via HWP_FONT_DIR and rerun style_parity",
        );
        return;
    }
    let mut doc = if extension == "hwp" {
        hwp5::read_document(&fixture.source).unwrap().document
    } else {
        hwpx::read_document(&fixture.source).unwrap().document
    };
    for face in doc.header.fonts.iter_mut().flatten() {
        *face = hwp_model::FaceName {
            name: font.family.clone(),
            ..Default::default()
        };
    }
    if extension == "hwp" {
        hwp5::write_document(&doc, &fixture.source, &hwp5::WriteOptions::default()).unwrap();
    } else {
        hwpx::write_document(&doc, &fixture.source).unwrap();
    }
    let source = if op == "apply_style" {
        fixture.ops(&fixture.source, "created", json!([create()]))
    } else {
        fixture.source.clone()
    };
    let output = fixture.ops(&source, "certified", json!([operation]));
    success(hwp().arg("validate").arg(&output).output().unwrap());
    // Keep all render checks at their defaults; only the environmental font identity is supplied.
    // Omitted oracle configuration defaults to disabled, so this proves native-only certification.
    let policy = json!({"schema_version":"1.0", "document":{"fonts":{"manifest":[{"path":"font.bin", "sha256":Sha256::digest(&*font.data).iter().map(|byte| format!("{byte:02x}")).collect::<String>()}]}}, "render":{"pages":[1]}, "oracle":{"mode":"disabled"}});
    let policy_path = fixture.root.join("policy.json");
    std::fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let report = fixture.root.join("certification");
    success(
        hwp()
            .arg("certify")
            .arg(output)
            .arg("--policy")
            .arg(policy_path)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap(),
    );
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(report.join("report.json")).unwrap()).unwrap();
    assert_eq!(receipt["overall"], "passed");
    assert_eq!(receipt["scope"], "native_only");
}

#[test]
fn create_passes_default_certification_rules_hwp() {
    certifies("hwp", create());
}
#[test]
fn create_passes_default_certification_rules_hwpx() {
    certifies("hwpx", create());
}
#[test]
fn apply_passes_default_certification_rules_hwp() {
    certifies("hwp", apply());
}
#[test]
fn apply_passes_default_certification_rules_hwpx() {
    certifies("hwpx", apply());
}
#[test]
fn update_passes_default_certification_rules_hwp() {
    certifies("hwp", update());
}
#[test]
fn update_passes_default_certification_rules_hwpx() {
    certifies("hwpx", update());
}

#[test]
fn identical_clear_direct_apply_is_byte_stable_hwp() {
    let mut operation = apply();
    operation["clear_direct"] = json!(true);
    repeated("hwp", operation);
}

#[test]
fn identical_clear_direct_apply_is_byte_stable_hwpx() {
    let mut operation = apply();
    operation["clear_direct"] = json!(true);
    repeated("hwpx", operation);
}
