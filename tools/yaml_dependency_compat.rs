//! Manual, authored serde_yaml -> serde_norway comparison; never a root dependency.
//! Invoked only by check_yaml_dependency_compat.py in an isolated Cargo workspace.
use std::error::Error;
use std::fs;
use std::path::Path;

use hwp_cli::certification::CertificationPolicy;
use hwp_cli::document_spec::{DocumentSpec, SpecInputFormat, compile_spec};
use hwp_cli::document_spec_v2::{DocumentSpecV2, compile_spec_v2};
use hwp_cli::template_spec::{
    ExpandedOutput, TemplateData, TemplateSpec, expand_template, sha256_hex,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

// These inputs are owner-authored synthetic cases. Multiline content is metadata,
// since DocumentSpec intentionally refuses line controls inside text runs.
const V1: &str = r#"version: "1.0"
metadata:
  title: &label "한글 YAML 비교"
  author: "hwp-cli"
  description: |
    First line
    두 번째 줄
styles:
  body:
    font_family: "Requested family only"
    bold: false
    font_size_pt: 10.5
sections:
  - blocks:
      - type: paragraph
        style: body
        runs:
          - {type: text, text: *label}
          - {type: text, text: "true"}
          - {type: text, text: "null"}
          - {type: text, text: "00123"}
"#;
const V2: &str = r#"version: "2.0"
document:
  version: "1.0"
  metadata: {title: "v2 한글"}
  sections:
    - blocks:
        - type: paragraph
          runs: [{type: text, text: "false / null / 007"}]
visuals: []
tables: {placement: inline}
"#;
const TEMPLATE: &str = r#"version: "1.0"
variables:
  title: {type: string, required: true, min_length: 1}
source:
  mode: compose
  document:
    version: "1.0"
    sections:
      - blocks:
          - type: paragraph
            runs:
              - type: text
                text: {node: value, pointer: /values/title, as: text}
"#;
const DATA: &str = r#"version: "1.0"
values: {title: "한글 true null 00123"}
"#;
const POLICY: &str = r#"schema_version: "1.0"
document:
  fonts: {manifest: [], forbid_substitution: false}
  macros: deny
  external_references: deny
render:
  dpi: 96
  pages: [1]
  page_count: {min: 1, max: 2}
  allowed_blank_pages: []
  fail_on_outside_page_bounds: true
  fail_on_potential_collision: false
  fail_on_unresolved_fields: true
oracle: {mode: disabled}
"#;

#[derive(Default)]
struct Proof {
    comparisons: Vec<Value>,
    parse_cases: usize,
    refusal_cases: usize,
    output_files: usize,
}

impl Proof {
    fn bytes(&mut self, label: &str, old: &[u8], new: &[u8]) -> Result<()> {
        if old != new {
            return Err(format!("{label}: byte mismatch (legacy {}, candidate {}); inspect this authored case before accepting the dependency change", sha256_hex(old), sha256_hex(new)).into());
        }
        self.comparisons
            .push(json!({"case": label, "bytes": old.len(), "sha256": sha256_hex(old)}));
        Ok(())
    }

    fn parse<T: DeserializeOwned + Serialize>(
        &mut self,
        label: &str,
        yaml: &str,
    ) -> Result<(T, T)> {
        let old: T = yaml_legacy::from_str(yaml)
            .map_err(|e| format!("{label}: legacy parse failed: {e}"))?;
        let new: T = yaml_candidate::from_str(yaml)
            .map_err(|e| format!("{label}: candidate parse failed: {e}"))?;
        let old_json = serde_json::to_vec(&old)?;
        let new_json = serde_json::to_vec(&new)?;
        self.bytes(&format!("{label}/model-json"), &old_json, &new_json)?;
        let old_yaml = yaml_legacy::to_string(&old)?;
        let new_yaml = yaml_candidate::to_string(&new)?;
        self.bytes(
            &format!("{label}/serialized-yaml"),
            old_yaml.as_bytes(),
            new_yaml.as_bytes(),
        )?;
        let old_via_new: T = yaml_candidate::from_str(&old_yaml)?;
        let new_via_old: T = yaml_legacy::from_str(&new_yaml)?;
        self.bytes(
            &format!("{label}/candidate-reparse"),
            &old_json,
            &serde_json::to_vec(&old_via_new)?,
        )?;
        self.bytes(
            &format!("{label}/legacy-reparse"),
            &new_json,
            &serde_json::to_vec(&new_via_old)?,
        )?;
        self.parse_cases += 1;
        Ok((old, new))
    }

    fn document(
        &mut self,
        label: &str,
        old: &hwp_model::Document,
        new: &hwp_model::Document,
        directory: &Path,
        extension: &str,
    ) -> Result<()> {
        let old_path = directory.join(format!("{label}-legacy.{extension}"));
        let new_path = directory.join(format!("{label}-candidate.{extension}"));
        for (document, output) in [(old, &old_path), (new, &new_path)] {
            let warnings = match extension {
                "hwp" => hwp5::write_document(document, output, &hwp5::WriteOptions::default())?,
                "hwpx" => hwpx::write_document(document, output)?,
                _ => return Err("unknown proof output format".into()),
            };
            if !warnings.is_empty() {
                return Err(format!("{label}/{extension}: writer emitted {} warnings; inspect before claiming byte compatibility", warnings.len()).into());
            }
        }
        self.bytes(
            &format!("{label}/{extension}"),
            &fs::read(old_path)?,
            &fs::read(new_path)?,
        )?;
        self.output_files += 2;
        Ok(())
    }
}

fn main() {
    if let Err(error) = execute() {
        eprintln!(
            "Authored YAML compatibility proof failed: {error}\nNext step: inspect the named authored case and migration; no compatibility receipt was issued."
        );
        std::process::exit(1);
    }
}

fn execute() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .ok_or("missing output directory; invoke the Python launcher")?;
    let directory = Path::new(&output);
    fs::create_dir_all(directory)?;
    let mut proof = Proof::default();
    let (old_v1, new_v1) = proof.parse::<DocumentSpec>("document-v1", V1)?;
    let (old_v2, new_v2) = proof.parse::<DocumentSpecV2>("document-v2", V2)?;
    let (old_template, new_template) = proof.parse::<TemplateSpec>("template", TEMPLATE)?;
    let (old_data, new_data) = proof.parse::<TemplateData>("template-data", DATA)?;
    let _ = proof.parse::<CertificationPolicy>("certification-policy", POLICY)?;
    // Public parsers retain their validation and redacted diagnostics; the harness
    // does not replace them in production or execute a certification policy.
    proof.bytes(
        "public-v1-parser",
        &serde_json::to_vec(&new_v1)?,
        &serde_json::to_vec(&hwp_cli::document_spec::parse_spec(
            V1,
            SpecInputFormat::Yaml,
        )?)?,
    )?;
    proof.bytes(
        "public-v2-parser",
        &serde_json::to_vec(&new_v2)?,
        &serde_json::to_vec(&hwp_cli::document_spec_v2::parse_spec_v2(
            V2,
            SpecInputFormat::Yaml,
        )?)?,
    )?;
    proof.bytes(
        "public-template-parser",
        &serde_json::to_vec(&new_template)?,
        &serde_json::to_vec(&hwp_cli::template_spec::parse_template(
            TEMPLATE,
            SpecInputFormat::Yaml,
        )?)?,
    )?;
    proof.bytes(
        "public-data-parser",
        &serde_json::to_vec(&new_data)?,
        &serde_json::to_vec(&hwp_cli::template_spec::parse_data(
            DATA,
            SpecInputFormat::Yaml,
        )?)?,
    )?;
    for yaml in [
        "version: \"1.0\"\nsections: []\nunknown: rejected\n",
        "version: \"not-supported\"\nsections: []\n",
        "version: [unterminated\n",
    ] {
        if yaml_legacy::from_str::<DocumentSpec>(yaml).is_ok()
            || yaml_candidate::from_str::<DocumentSpec>(yaml).is_ok()
        {
            return Err("DocumentSpec refusal changed; inspect invalid authored input before accepting migration".into());
        }
        if hwp_cli::document_spec::parse_spec(yaml, SpecInputFormat::Yaml).is_ok() {
            return Err("public DocumentSpec parser accepted a refused authored input".into());
        }
        proof.refusal_cases += 1;
    }
    let sensitive = "version: \"1.0\"\nvariables: {}\nsource: {mode: unsupported, private: NEVER_DISCLOSE_THIS_VALUE}\n";
    if yaml_legacy::from_str::<TemplateSpec>(sensitive).is_ok()
        || yaml_candidate::from_str::<TemplateSpec>(sensitive).is_ok()
    {
        return Err("template refusal changed for invalid source mode".into());
    }
    let redacted = hwp_cli::template_spec::parse_template(sensitive, SpecInputFormat::Yaml)
        .expect_err("invalid template source must remain refused");
    if redacted.to_string().contains("NEVER_DISCLOSE_THIS_VALUE") {
        return Err("public template parse diagnostic disclosed authored sentinel value".into());
    }
    proof.refusal_cases += 1;
    // This option is intentionally outside the v1 native compiler contract. Keep its
    // rejection as a negative case rather than asking a positive fixture to emit it.
    let unsupported = V2.replace(
        "type: paragraph",
        "type: paragraph\n          keep_with_next: true",
    );
    let old_unsupported: DocumentSpecV2 = yaml_legacy::from_str(&unsupported)?;
    let new_unsupported: DocumentSpecV2 = yaml_candidate::from_str(&unsupported)?;
    for extension in ["hwp", "hwpx"] {
        let target = directory.join(format!("refused.{extension}"));
        let before = compile_spec_v2(&old_unsupported, directory, &target, false, &[])
            .err()
            .ok_or("legacy keep_with_next unexpectedly compiled; inspect the refusal contract")?;
        let after = compile_spec_v2(&new_unsupported, directory, &target, false, &[])
            .err()
            .ok_or(
                "candidate keep_with_next unexpectedly compiled; inspect the refusal contract",
            )?;
        proof.bytes(
            &format!("unsupported-keep-with-next/{extension}"),
            before.to_string().as_bytes(),
            after.to_string().as_bytes(),
        )?;
    }
    proof.refusal_cases += 1;
    let old_expanded = expand_template(&old_template, &old_data, directory, &[])?;
    let new_expanded = expand_template(&new_template, &new_data, directory, &[])?;
    let (ExpandedOutput::Compose(old_composed), ExpandedOutput::Compose(new_composed)) =
        (old_expanded.output, new_expanded.output)
    else {
        return Err("authored compose template unexpectedly selected a reference path".into());
    };
    for extension in ["hwp", "hwpx"] {
        let target = directory.join(format!("target.{extension}"));
        let old = compile_spec(&old_v1, directory, &target, false, false, &[])?;
        let new = compile_spec(&new_v1, directory, &target, false, false, &[])?;
        proof.document(
            "document-v1",
            &old.document,
            &new.document,
            directory,
            extension,
        )?;
        let old = compile_spec_v2(&old_v2, directory, &target, false, &[])?;
        let new = compile_spec_v2(&new_v2, directory, &target, false, &[])?;
        proof.document(
            "document-v2",
            &old.document,
            &new.document,
            directory,
            extension,
        )?;
        let old = compile_spec(&old_composed, directory, &target, false, false, &[])?;
        let new = compile_spec(&new_composed, directory, &target, false, false, &[])?;
        proof.document(
            "template-compose",
            &old.document,
            &new.document,
            directory,
            extension,
        )?;
    }
    let comparisons_sha256 = sha256_hex(&serde_json::to_vec(&proof.comparisons)?);
    println!(
        "{}",
        serde_json::to_string(&json!({
            "schema": "hwp-yaml-authored-byte-proof-v1", "parse_cases": proof.parse_cases,
            "refusal_cases": proof.refusal_cases, "output_files": proof.output_files,
            "comparisons": proof.comparisons, "comparisons_sha256": comparisons_sha256,
            "limitations": ["bounded authored cases only", "no private fixtures", "no font shaping/rendering", "no certification execution", "no Hancom open/export evidence", "no reference-template package path"]
        }))?
    );
    Ok(())
}
