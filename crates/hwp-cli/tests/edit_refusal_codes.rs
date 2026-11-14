//! #531: a refused `hwp edit` run still writes edit-report-v1 (1.4), and the refusal carries a
//! stable code, structured values and the human message. One test per code family; the
//! range-tag and preservation families live beside their fixtures in `range_tag_edits.rs` and
//! `edit_surgical.rs`.
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

#[path = "common/temp_path.rs"]
mod temp_path;

const INJECTED: &str = "a; layout_overflow at $.z: b";

fn hwp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ahwp"))
}

struct Fixture {
    dir: temp_path::TempPath,
    base: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = temp_path::TempPath::new("hwp531", label);
        let _ = std::fs::remove_dir_all(&*dir);
        std::fs::create_dir_all(&*dir).unwrap();
        let spec = json!({"version":"1.0","styles":{"body":{"font_size_pt":10}},
        "sections":[{"blocks":[
            {"type":"paragraph","style":"body","runs":[{"type":"text","text":"Body anchor."}]}
        ]}]});
        let spec_path = dir.join("spec.json");
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let base = dir.join("base.hwpx");
        let out = hwp()
            .arg("compose")
            .arg(&spec_path)
            .arg("-o")
            .arg(&base)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self { dir, base }
    }

    /// Runs `edit --ops <ops> --report <file>`, asserts the refusal, and returns the report.
    fn refused_ops(&self, label: &str, ops: Value) -> Value {
        let ops_path = self.dir.join(format!("{label}.json"));
        std::fs::write(&ops_path, serde_json::to_vec(&ops).unwrap()).unwrap();
        self.refused(label, &["--ops", ops_path.to_str().unwrap()])
    }

    fn refused(&self, label: &str, args: &[&str]) -> Value {
        let output = self.dir.join(format!("{label}.hwpx"));
        let report = self.dir.join(format!("{label}.report.json"));
        let run = hwp()
            .arg("edit")
            .arg(&self.base)
            .arg("-o")
            .arg(&output)
            .args(args)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap();
        assert_refused(&run, &output);
        let value = read_report(&report);
        assert!(!run.stderr.is_empty(), "stderr keeps the human sentence");
        value
    }
}

fn assert_refused(run: &Output, output: &Path) {
    assert!(!run.status.success(), "the edit must refuse");
    assert!(!output.exists(), "a refused edit publishes nothing");
}

fn read_report(path: &Path) -> Value {
    let bytes = std::fs::read(path).expect("a refused run writes the --report file (#531)");
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    assert!(
        validator.is_valid(&report),
        "refused report must validate: {report}"
    );
    assert_eq!(report["schema_version"], "1.4");
    report
}

fn insert(position: &str, policy: &str, styles: Value, blocks: Value) -> Value {
    json!([{"op":"insert_blocks","address":{"at":{"section":0,"paragraph":0}},
        "position":position,"missing_style":policy,"styles":styles,"blocks":blocks}])
}

fn paragraph(style: &str) -> Value {
    json!([{"type":"paragraph","style":style,"runs":[{"type":"text","text":"x"}]}])
}

/// The issue's own reproduction: `--add-row 99` on minimal.hwpx used to leave no report.
#[test]
fn legacy_flag_refusal_writes_the_report() {
    let fixture = Fixture::new("legacy");
    let report = fixture.refused("legacy", &["--add-row", "99"]);
    assert_eq!(report["refusal"]["code"], "unclassified");
    // The report carries only the code's fixed, content-free sentence (#348); stderr keeps
    // the full one.
    assert_eq!(
        report["refusal"]["message"],
        hwp_cli::refusal::fixed_message("unclassified"),
        "{report}"
    );
    assert!(!report.to_string().contains("#99"), "{report}");
    assert_eq!(report["ops"], json!([]));
}

#[test]
fn dry_run_prints_the_refused_report() {
    let fixture = Fixture::new("dry");
    let output = fixture.dir.join("dry.hwpx");
    let run = hwp()
        .arg("edit")
        .arg(&fixture.base)
        .arg("-o")
        .arg(&output)
        .args(["--add-table", "없는=>[[\"a\"]]", "--dry-run"])
        .output()
        .unwrap();
    assert_refused(&run, &output);
    let report: Value = serde_json::from_slice(&run.stdout).expect("dry-run prints the report");
    assert_eq!(report["refusal"]["code"], "add_table.anchor_not_found");
    assert_eq!(report["refusal"]["values"]["anchor"], "없는");
    assert_eq!(report["dry_run"], true);
}

#[test]
fn add_table_anchor_miss_names_the_anchor_as_a_value() {
    let fixture = Fixture::new("anchor");
    let anchor = format!("{INJECTED} 없음");
    let report = fixture.refused_ops(
        "anchor",
        json!([{"op":"add_table","anchor":anchor,"rows":[["a"]]}]),
    );
    let op = &report["ops"][0];
    assert_eq!(op["status"], "failed");
    assert_eq!(op["refusal"]["code"], "add_table.anchor_not_found");
    assert_eq!(op["refusal"]["values"]["anchor"], anchor);
    assert_eq!(report["refusal"], op["refusal"]);
}

#[test]
fn keep_together_without_a_field_still_refuses_without_an_unsupported_code() {
    let fixture = Fixture::new("keep-missing");
    let report = fixture.refused_ops(
        "keep-missing",
        json!([{"op":"keep_together","address":{"at":{"section":0,"paragraph":0}}}]),
    );
    assert_eq!(report["refusal"]["code"], "unclassified");
}

#[test]
fn section_owner_insert_and_move_have_their_codes() {
    let fixture = Fixture::new("owner");
    let report = fixture.refused_ops(
        "insert",
        insert("before", "create", json!({}), paragraph("body")),
    );
    assert_eq!(report["ops"][0]["refusal"]["code"], "section_owner_insert");
    let report = fixture.refused_ops(
        "move",
        json!([{"op":"move_para","address":{"at":{"section":0,"paragraph":1}},
            "to":{"address":{"at":{"section":0,"paragraph":0}},"position":"before"}}]),
    );
    assert_eq!(report["ops"][0]["refusal"]["code"], "section_owner_move");
}

#[test]
fn insert_blocks_unknown_style_carries_the_name() {
    let fixture = Fixture::new("unknown");
    let report = fixture.refused_ops(
        "unknown",
        insert("after", "refuse", json!({}), paragraph(INJECTED)),
    );
    let refusal = &report["ops"][0]["refusal"];
    assert_eq!(refusal["code"], "insert_blocks.unknown_style");
    assert_eq!(refusal["values"]["style"], INJECTED);
    assert_eq!(refusal["values"]["missing_style"], "refuse");
}

/// The issue's ambiguity case: a style named `a; layout_overflow at $.z: b` with an invalid
/// definition reads as a layout overflow in the joined prose. Its code is the real one and its
/// path keeps the name as one segment.
#[test]
fn injected_style_name_is_classified_by_its_real_code() {
    let fixture = Fixture::new("injected");
    let report = fixture.refused_ops(
        "injected",
        insert(
            "after",
            "create",
            json!({INJECTED: {"keep_with_next": true}}),
            paragraph(INJECTED),
        ),
    );
    let refusal = &report["ops"][0]["refusal"];
    assert_eq!(
        refusal["code"], "insert_blocks.invalid_definition",
        "{report}"
    );
    let issues = refusal["values"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1, "{report}");
    assert_eq!(issues[0]["code"], "unsupported_native");
    assert_eq!(
        issues[0]["path"],
        json!(["$", "styles", INJECTED, "keep_with_next"])
    );
}

#[test]
fn layout_overflow_carries_the_measured_widths() {
    let fixture = Fixture::new("overflow");
    let report = fixture.refused_ops(
        "overflow",
        insert(
            "after",
            "create",
            json!({}),
            json!([{"type":"table","columns":[{"width_mm":400}],"rows":[{"cells":[{"blocks":[]}]}]}]),
        ),
    );
    let refusal = &report["ops"][0]["refusal"];
    assert_eq!(refusal["code"], "layout_overflow", "{report}");
    assert!(refusal["values"]["container_width_mm"].as_f64().unwrap() > 0.0);
    let issue = &refusal["values"]["issues"][0];
    assert_eq!(issue["code"], "layout_overflow");
    assert_eq!(
        issue["path"],
        json!(["$", "sections", 0, "blocks", 0, "columns"])
    );
    // The column sum that overflowed is in `values`, not only in the prose.
    assert_eq!(issue["measured_width_mm"], 400.0, "{report}");
}

#[test]
fn unapplied_requests_list_the_requests() {
    let fixture = Fixture::new("unapplied");
    let report = fixture.refused_ops(
        "unapplied",
        json!([{"op":"replace","from":"없는 문자열","to":"y"}]),
    );
    assert_eq!(report["refusal"]["code"], "unapplied_requests", "{report}");
    assert_eq!(report["ops"][0]["status"], "failed");
}

/// #348: the report never re-leaks request content, even inside `refusal.message`.
#[test]
fn unapplied_report_does_not_leak_request_text() {
    let fixture = Fixture::new("leak");
    for (label, ops) in [
        (
            "leak-replace",
            json!([{"op":"replace","from":"없는 비밀-from","to":"PRIVATE-TO-7731"}]),
        ),
        (
            "leak-replace-two",
            json!([{"op":"replace","from":"Body","to":"PRIVATE-TO-7731"},
                   {"op":"replace","from":"없는 비밀-from","to":"PRIVATE-TO-7731"}]),
        ),
    ] {
        let report = fixture.refused_ops(label, ops);
        let text = report.to_string();
        assert!(!text.contains("PRIVATE-TO-7731"), "{text}");
        assert!(!text.contains("없는 비밀-from"), "{text}");
    }
}

/// #531: an invalid apply_form_bindings ops file publishes no refused-run report; its receipt
/// is the only artifact a form-binding batch may publish, as on the run() path.
#[test]
fn invalid_form_binding_ops_write_no_report() {
    let fixture = Fixture::new("binding");
    let ops_path = fixture.dir.join("binding.json");
    std::fs::write(
        &ops_path,
        serde_json::to_vec(
            &json!([{"op":"apply_form_bindings"},{"op":"replace","from":"a","to":"b"}]),
        )
        .unwrap(),
    )
    .unwrap();
    let output = fixture.dir.join("binding.hwpx");
    let report = fixture.dir.join("binding.report.json");
    let run = hwp()
        .arg("edit")
        .arg(&fixture.base)
        .arg("-o")
        .arg(&output)
        .arg("--ops")
        .arg(&ops_path)
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert_refused(&run, &output);
    assert!(
        !report.exists(),
        "a form-binding batch writes no separate report"
    );
}

/// #531: a report that cannot be written warns and never masks the refusal itself.
#[test]
fn report_write_failure_keeps_the_refusal() {
    let fixture = Fixture::new("unwritable");
    let output = fixture.dir.join("unwritable.hwpx");
    let report = fixture.dir.join("no-such-dir").join("report.json");
    let run = hwp()
        .arg("edit")
        .arg(&fixture.base)
        .arg("-o")
        .arg(&output)
        .args(["--add-row", "99"])
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert_refused(&run, &output);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("표 #99를 찾을 수 없습니다"), "{stderr}");
}

// --- #571: open-time protection refusals carry `document_protected` and a typed class. ---

/// HWP5 FileHeader attribute bits (DWORD at 36..40), mirrored from the private attr module in
/// crates/hwp5/src/file_header.rs, as `cli_surface.rs` mirrors them.
const BIT_ENCRYPTED: u32 = 1 << 1;
const BIT_DRM: u32 = 1 << 4;
const BIT_HAS_SIGNATURE: u32 = 1 << 7;
const BIT_CERT_ENCRYPTED: u32 = 1 << 8;
const BIT_CERT_DRM: u32 = 1 << 10;

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/samples/report-tables.hwpx")
}

/// The committed sample converted to HWP5 with `bits` ORed into the FileHeader attributes.
fn protected_hwp(dir: &Path, name: &str, bits: u32) -> PathBuf {
    let path = dir.join(format!("{name}.hwp"));
    let run = hwp()
        .arg("convert")
        .arg(sample())
        .arg("-o")
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let mut cfb = cfb::open_rw(&path).unwrap();
    let mut header = Vec::new();
    std::io::Read::read_to_end(&mut cfb.open_stream("/FileHeader").unwrap(), &mut header).unwrap();
    let attrs = u32::from_le_bytes(header[36..40].try_into().unwrap());
    header[36..40].copy_from_slice(&(attrs | bits).to_le_bytes());
    cfb.create_stream("/FileHeader")
        .unwrap()
        .write_all(&header)
        .unwrap();
    path
}

/// The committed sample repacked with an encryption-data manifest, as `cli_surface.rs` does.
fn encrypted_hwpx(dir: &Path) -> PathBuf {
    const MANIFEST: &[u8] = br##"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?><odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><odf:file-entry full-path="Contents/header.xml" media-type="application/xml" size="1"><odf:encryption-data checksum-type="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#sha256-1k" checksum="AAAA"><odf:algorithm algorithm-name="http://www.w3.org/2001/04/xmlenc#aes256-cbc" initialisation-vector="AAAA"/><odf:key-derivation key-derivation-name="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0#pbkdf2" key-size="32" iteration-count="1024" salt="AAAA"/><odf:start-key-generation start-key-generation-name="http://www.w3.org/2000/09/xmldsig#sha256" key-size="32"/></odf:encryption-data></odf:file-entry></odf:manifest>"##;
    let path = dir.join("encrypted.hwpx");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(sample()).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    writer
        .start_file(
            "mimetype",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer
        .write_all(hwpx::package::MIMETYPE.as_bytes())
        .unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        if name == "mimetype" {
            continue;
        }
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
        if name == "META-INF/manifest.xml" {
            data = MANIFEST.to_vec();
        }
        writer
            .start_file(
                &name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(&data).unwrap();
    }
    writer.finish().unwrap();
    path
}

/// Runs `edit <input> -o <out> <args> --report <file>`, asserts the refusal and returns the
/// schema-valid report.
fn refused_open(dir: &Path, input: &Path, label: &str, args: &[&str]) -> Value {
    let output = dir.join(format!("{label}.out.hwpx"));
    let report = dir.join(format!("{label}.report.json"));
    let run = hwp()
        .arg("edit")
        .arg(input)
        .arg("-o")
        .arg(&output)
        .args(args)
        .arg("--report")
        .arg(&report)
        .output()
        .unwrap();
    assert_refused(&run, &output);
    assert!(!run.stderr.is_empty(), "stderr keeps the human sentence");
    read_report(&report)
}

fn assert_protected(report: &Value, protection: &str) {
    let refusal = &report["refusal"];
    assert_eq!(refusal["code"], "document_protected", "{report}");
    assert_eq!(
        refusal["values"],
        json!({"protection": protection}),
        "{report}"
    );
    assert_eq!(
        refusal["message"],
        hwp_cli::refusal::fixed_message("document_protected")
    );
    assert_eq!(report["ops"], json!([]), "{report}");
}

/// Every HWP5 FileHeader protection class the engine tells apart gets its own value, through
/// both entry points (`--replace` flags and `--ops`).
#[test]
fn hwp5_protection_classes_write_document_protected() {
    let dir = temp_path::TempPath::new("hwp571", "hwp5");
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        serde_json::to_vec(&json!([{"op":"replace","from":"a","to":"b"}])).unwrap(),
    )
    .unwrap();
    for (name, bits, protection) in [
        ("pw", BIT_ENCRYPTED, "password"),
        ("cert_enc", BIT_CERT_ENCRYPTED, "certificate_encryption"),
        ("cert_drm", BIT_CERT_DRM, "certificate_drm"),
        ("drm", BIT_DRM, "drm"),
        ("signed", BIT_HAS_SIGNATURE, "signature"),
    ] {
        let input = protected_hwp(&dir, name, bits);
        let flags = refused_open(
            &dir,
            &input,
            &format!("{name}-flags"),
            &["--replace", "a=>b"],
        );
        assert_protected(&flags, protection);
        let typed = refused_open(
            &dir,
            &input,
            &format!("{name}-ops"),
            &["--ops", ops.to_str().unwrap()],
        );
        assert_protected(&typed, protection);
    }
}

/// An encrypted HWPX package is a password refusal on every HWPX path, including the
/// package-preserving `--replace` fast path that never goes through the shared loader.
#[test]
fn encrypted_hwpx_writes_document_protected_password() {
    let dir = temp_path::TempPath::new("hwp571", "hwpx");
    let _ = std::fs::remove_dir_all(&*dir);
    std::fs::create_dir_all(&*dir).unwrap();
    let input = encrypted_hwpx(&dir);
    let replace = refused_open(&dir, &input, "replace", &["--replace", "a=>b"]);
    assert_protected(&replace, "password");
    let other = refused_open(&dir, &input, "row", &["--add-row", "0:0"]);
    assert_protected(&other, "password");
}

/// A refusal that is not a protection one keeps its own code: the classifier reads the typed
/// error, not the message, so a protection sentence in user text selects nothing.
#[test]
fn protection_sentence_in_user_text_does_not_select_the_code() {
    let fixture = Fixture::new("protected-text");
    let anchor =
        "DRM으로 보호된 문서는 지원하지 않습니다. 한글에서 DRM 보안을 해제한 뒤 다시 저장하세요.";
    let report = fixture.refused_ops(
        "protected-text",
        json!([{"op":"add_table","anchor":anchor,"rows":[["a"]]}]),
    );
    assert_eq!(report["refusal"]["code"], "add_table.anchor_not_found");
}

/// A document that is not protected never reports the code.
#[test]
fn unprotected_refusal_is_not_document_protected() {
    let fixture = Fixture::new("not-protected");
    let report = fixture.refused("not-protected", &["--add-row", "99"]);
    assert_ne!(report["refusal"]["code"], "document_protected");
}

/// A schema-valid `document_protected` must name a known class.
#[test]
fn schema_requires_a_known_protection_class() {
    let schema: Value =
        serde_json::from_str(include_str!("../../../schemas/edit-report-v1.schema.json")).unwrap();
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .unwrap();
    let report = |values: Value| {
        json!({"schema_version":"1.4","contract":"hwp-edit-report-v1","output":"o","dry_run":false,
            "ops":[],"applied_count":0,"failed_count":0,
            "refusal":{"code":"document_protected","values":values,"message":"m"}})
    };
    assert!(validator.is_valid(&report(json!({"protection":"drm"}))));
    assert!(!validator.is_valid(&report(json!({"protection":"other"}))));
    assert!(!validator.is_valid(&report(json!({}))));
    for class in hwp_cli::refusal::PROTECTION_CLASSES {
        assert!(
            validator.is_valid(&report(json!({"protection": class}))),
            "{class}"
        );
    }
}
