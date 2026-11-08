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
        std::fs::write(&markdown, "# Application\n\n{{ allocation }}\n\n{{ allocation }}\n").unwrap();
        let output = self.0.join(format!("source.{extension}"));
        let result = hwp().args(["new", "--from"]).arg(markdown).arg("-o").arg(&output).output().unwrap();
        success(&result, "fixture creation");
        output
    }
}
impl Drop for Workspace {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn hwp() -> Command { Command::new(env!("CARGO_BIN_EXE_hwp")) }
fn success(output: &Output, phase: &str) {
    assert!(output.status.success(), "{phase}: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn discovery_keeps_repeated_occurrences_and_binds_the_source_bytes() {
    for extension in ["hwp", "hwpx"] {
        let workspace = Workspace::new(extension);
        let source = workspace.source(extension);
        let output = hwp().args(["slots", "--json", "--forms", "--occurrences"]).arg(&source).output().unwrap();
        success(&output, "occurrence discovery must be available");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["version"], 1);
        assert_eq!(report["source_sha256"].as_str().unwrap().len(), 64);
        let occurrences: Vec<_> = report["occurrences"].as_array().unwrap().iter()
            .filter(|item| item["kind"] == "placeholder" && item["label"] == "allocation").collect();
        assert_eq!(occurrences.len(), 2, "individual repeated targets must survive discovery");
        assert_ne!(occurrences[0]["id"], occurrences[1]["id"]);
        assert!(occurrences.iter().all(|item| item["bindable"] == true));
        let again = hwp().args(["slots", "--json", "--forms", "--occurrences"]).arg(&source).output().unwrap();
        success(&again, "repeat discovery");
        assert_eq!(output.stdout, again.stdout, "deterministic catalogue");
    }
}
