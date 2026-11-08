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
            "hwp" => hwp5::write_document(&doc, &source, &hwp5::WriteOptions::default()).unwrap(),
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
fn mcp(arguments: Value) -> Value {
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
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"hwp_edit","arguments":arguments}}),
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
    json!({"op":"create_style","name":"Body","kind":"paragraph","based_on":0,"size":13})
}
fn apply() -> Value {
    json!({"op":"apply_style","style":"Body","address":{"at":{"section":0,"paragraph":0}}})
}
fn update() -> Value {
    json!({"op":"update_style","style":0,"size":13})
}

fn channel_parity(extension: &'static str, operation: Value) {
    let op = operation["op"].as_str().unwrap();
    let fixture = Fixture::new(op, extension);
    let source = if op == "apply_style" {
        fixture.ops(&fixture.source, "created", json!([create()]))
    } else {
        fixture.source.clone()
    };
    let expected = fixture.ops(&source, "ops-result", json!([operation.clone()]));
    let output = fixture.output("mcp-result");
    let mut item = operation.clone();
    item.as_object_mut().unwrap().remove("op");
    let mut arguments = json!({"input":source,"output":output,"verify":true});
    arguments[op] = json!([item]);
    let response = mcp(arguments);
    assert!(
        response.get("error").is_none() && response["result"]["isError"] != true,
        "{op} MCP must execute the same request as --ops: {response}"
    );
    assert!(output.is_file(), "{op} MCP must publish a native output");
    assert_eq!(
        std::fs::read(expected).unwrap(),
        std::fs::read(&output).unwrap(),
        "{op} channel output must be byte-identical"
    );
    success(hwp().arg("validate").arg(output).output().unwrap());
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
