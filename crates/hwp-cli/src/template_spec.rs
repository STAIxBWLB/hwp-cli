//! Versioned, non-executable template contract layered above DocumentSpec v1.
//!
//! Template operators are explicit AST objects. Text interpolation, expression
//! evaluation, includes, and template calls are deliberately absent. Expansion
//! produces an ordinary frozen [`crate::document_spec::DocumentSpec`] or a
//! bounded package-surgical reference fill plan.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::document_spec::{DocumentSpec, SpecInputFormat};

pub const MAX_TEMPLATE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_DATA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_VARIABLES: usize = 1_024;
pub const MAX_VARIABLE_NAME_BYTES: usize = 64;
pub const MAX_REGEX_CHARS: usize = 1_024;
pub const MAX_STRING_CHARS: usize = 2_000_000;
pub const MAX_RICH_BLOCKS: usize = 20_000;
pub const MAX_LIST_ITEMS: usize = 10_000;
pub const MAX_CONTROL_DEPTH: usize = 8;
pub const MAX_EXPANDED_NODES: usize = 250_000;
pub const MAX_EXPANDED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_EACH_ITERATIONS: usize = 100_000;
pub const MAX_REGIONS: usize = 20_000;
pub const MAX_REFERENCE_TARGETS: usize = 1_024;
pub const MAX_REFERENCE_EXPANDED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ISSUES: usize = 100;
pub const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
pub const MAX_REPORT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemplateVersion {
    #[serde(rename = "1.0")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateSpec {
    pub version: TemplateVersion,
    pub variables: BTreeMap<String, VariableSpec>,
    pub source: TemplateSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplateSource {
    Compose {
        document: Value,
    },
    ReferenceHwpx {
        path: PathBuf,
        bindings: Vec<ReferenceBinding>,
    },
    ReferenceRegenerate {
        path: PathBuf,
        strict_unsupported_objects: bool,
        document: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateData {
    pub version: TemplateVersion,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum VariableSpec {
    String {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        regex: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
    },
    Number {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    Bool {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<bool>,
    },
    Date {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<String>,
    },
    Enum {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        values: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },
    RichBlocks {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<Vec<Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
    List {
        #[serde(default)]
        required: bool,
        #[serde(default)]
        secret: bool,
        fields: BTreeMap<String, ItemFieldSpec>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<Vec<BTreeMap<String, Value>>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_items: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_items: Option<usize>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemFieldSpec {
    String {
        #[serde(default)]
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        regex: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
    },
    Number {
        #[serde(default)]
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<f64>,
    },
    Bool {
        #[serde(default)]
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<bool>,
    },
    Date {
        #[serde(default)]
        required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<String>,
    },
    Enum {
        #[serde(default)]
        required: bool,
        values: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceBinding {
    pub region: String,
    pub variable: String,
    pub target: ReferenceTarget,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceTarget {
    Placeholder,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateIssue {
    pub code: String,
    /// RFC 6901 JSON Pointer into TemplateSpec or TemplateData.
    pub pointer: String,
    pub message: String,
}

#[derive(Debug)]
pub struct TemplateError {
    issues: Vec<TemplateIssue>,
    truncated: bool,
}

impl TemplateError {
    pub fn new(mut issues: Vec<TemplateIssue>) -> Self {
        let truncated = issues
            .last()
            .is_some_and(|issue| issue.code == "diagnostics_truncated");
        if truncated {
            issues.pop();
        }
        Self { issues, truncated }
    }

    pub fn single(code: &str, pointer: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(vec![TemplateIssue {
            code: code.to_string(),
            pointer: pointer.into(),
            message: message.into(),
        }])
    }

    pub fn issues(&self) -> &[TemplateIssue] {
        &self.issues
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn total_or_at_least(&self) -> usize {
        if self.truncated {
            MAX_ISSUES + 1
        } else {
            self.issues.len()
        }
    }
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            self.issues
                .iter()
                .map(|issue| format!("{} at {}: {}", issue.code, issue.pointer, issue.message))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

impl std::error::Error for TemplateError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateMode {
    Compose,
    ReferencePackagePreserving,
    ReferenceRegenerate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionKind {
    Conditional,
    Repeated,
    RichBlocks,
    Placeholder,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegionPlan {
    pub id: String,
    pub kind: RegionKind,
    pub template_pointer: String,
    pub input_items: usize,
    pub generated_items: usize,
    pub instances: Vec<RegionInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegionInstance {
    pub path: String,
    pub input_items: usize,
    pub generated_items: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpansionPlan {
    pub input_nodes: usize,
    pub expanded_nodes: usize,
    pub expanded_bytes: usize,
    pub max_control_depth: usize,
    pub each_iterations: usize,
    pub conditions_evaluated: usize,
    pub regions: Vec<RegionPlan>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateReport {
    pub schema_version: TemplateVersion,
    pub data_schema_version: TemplateVersion,
    pub output: String,
    pub dry_run: bool,
    pub deterministic: bool,
    pub mode: TemplateMode,
    pub template_sha256: String,
    pub data_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_sha256: Option<String>,
    pub provided_variables: Vec<String>,
    pub defaulted_variables: Vec<String>,
    pub expansion: ExpansionPlan,
    pub changed_regions: Vec<RegionPlan>,
    pub generated_regions: Vec<RegionPlan>,
    pub unsupported: Vec<String>,
    pub fallback: Vec<String>,
    pub dropped: Vec<String>,
    pub template_validation: ValidationStatus,
    pub data_validation: ValidationStatus,
    pub semantic_validation: ValidationStatus,
    pub package_validation: ValidationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose: Option<crate::document_spec::ComposeReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Passed,
    NotRun,
}

#[derive(Debug, Clone)]
pub struct ExpandedTemplate {
    pub mode: TemplateMode,
    pub output: ExpandedOutput,
    pub plan: ExpansionPlan,
    pub provided_variables: Vec<String>,
    pub defaulted_variables: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ExpandedOutput {
    Compose(DocumentSpec),
    Reference {
        path: PathBuf,
        placeholders: BTreeMap<String, String>,
        fields: BTreeMap<String, String>,
    },
    ReferenceRegenerate {
        path: PathBuf,
        strict_unsupported_objects: bool,
        document: DocumentSpec,
    },
}

#[derive(Debug, Clone)]
struct EffectiveData {
    values: BTreeMap<String, Value>,
    kinds: BTreeMap<String, ValueKind>,
    provided: Vec<String>,
    defaulted: Vec<String>,
}

type ReferenceValues = (BTreeMap<String, String>, BTreeMap<String, String>);

#[derive(Default)]
struct CompiledConstraints {
    regexes: BTreeMap<String, regex::Regex>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Scalar,
    RichBlocks,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayContext {
    Other,
    Blocks,
    Rows,
}

pub fn parse_template(input: &str, format: SpecInputFormat) -> Result<TemplateSpec, TemplateError> {
    parse_bounded(input, format, MAX_TEMPLATE_BYTES, "template")
}

pub fn parse_data(input: &str, format: SpecInputFormat) -> Result<TemplateData, TemplateError> {
    parse_bounded(input, format, MAX_DATA_BYTES, "data")
}

fn parse_bounded<T: for<'de> Deserialize<'de>>(
    input: &str,
    format: SpecInputFormat,
    limit: usize,
    label: &str,
) -> Result<T, TemplateError> {
    if input.len() > limit {
        return Err(TemplateError::single(
            "limit_exceeded",
            "",
            format!("{label} exceeds the {limit} byte limit"),
        ));
    }
    match format {
        SpecInputFormat::Json => serde_json::from_str(input).map_err(|error| {
            TemplateError::single(
                "parse_error",
                "",
                format!(
                    "{label} does not match the closed contract at line {}, column {}; details are redacted",
                    error.line(),
                    error.column()
                ),
            )
        }),
        SpecInputFormat::Yaml => serde_yaml::from_str(input).map_err(|error| {
            let location = error
                .location()
                .map(|location| format!("line {}, column {}", location.line(), location.column()))
                .unwrap_or_else(|| "an unknown location".to_string());
            TemplateError::single(
                "parse_error",
                "",
                format!(
                    "{label} does not match the closed contract at {location}; details are redacted"
                ),
            )
        }),
    }
}

pub fn infer_input_format(path: &Path) -> Result<SpecInputFormat, TemplateError> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("json") => Ok(SpecInputFormat::Json),
        Some("yaml" | "yml") => Ok(SpecInputFormat::Yaml),
        _ => Err(TemplateError::single(
            "unknown_format",
            "",
            "input format must be explicit or use .json, .yaml, or .yml",
        )),
    }
}

pub fn expand_template(
    template: &TemplateSpec,
    data: &TemplateData,
    base_dir: &Path,
) -> Result<ExpandedTemplate, TemplateError> {
    let effective = validate_and_resolve(template, data)?;
    let mut expander = Expander::new(&effective);
    let (mode, output) = match &template.source {
        TemplateSource::Compose { document } => {
            let expanded = expander.expand_document(document)?;
            let document = deserialize_document(expanded)?;
            (TemplateMode::Compose, ExpandedOutput::Compose(document))
        }
        TemplateSource::ReferenceHwpx { path, bindings } => {
            let path = resolve_reference_path(base_dir, path)?;
            let (placeholders, fields) = reference_bindings(bindings, &effective, &mut expander)?;
            (
                TemplateMode::ReferencePackagePreserving,
                ExpandedOutput::Reference {
                    path,
                    placeholders,
                    fields,
                },
            )
        }
        TemplateSource::ReferenceRegenerate {
            path,
            strict_unsupported_objects,
            document,
        } => {
            if !strict_unsupported_objects {
                return Err(TemplateError::single(
                    "strict_gate_required",
                    "/source/strict_unsupported_objects",
                    "reference regeneration requires strict_unsupported_objects=true",
                ));
            }
            let path = resolve_reference_path(base_dir, path)?;
            let expanded = expander.expand_document(document)?;
            let document = deserialize_document(expanded)?;
            (
                TemplateMode::ReferenceRegenerate,
                ExpandedOutput::ReferenceRegenerate {
                    path,
                    strict_unsupported_objects: true,
                    document,
                },
            )
        }
    };
    expander.finish()?;
    Ok(ExpandedTemplate {
        mode,
        output,
        plan: expander.plan,
        provided_variables: effective.provided,
        defaulted_variables: effective.defaulted,
    })
}

fn deserialize_document(value: Value) -> Result<DocumentSpec, TemplateError> {
    serde_json::from_value(value).map_err(|_| {
        TemplateError::single(
            "expanded_document_invalid",
            "/source/document",
            "expanded value is not a valid DocumentSpec v1; details are redacted",
        )
    })
}

fn resolve_reference_path(base_dir: &Path, path: &Path) -> Result<PathBuf, TemplateError> {
    use std::path::Component;

    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(TemplateError::single(
            "invalid_reference",
            "/source/path",
            "reference path must be relative and contain only normal components",
        ));
    }
    let base = std::fs::canonicalize(base_dir).map_err(|_| {
        TemplateError::single(
            "invalid_base_dir",
            "/source/path",
            "template base directory cannot be resolved",
        )
    })?;
    let candidate = base.join(path);
    let metadata = std::fs::symlink_metadata(&candidate).map_err(|_| {
        TemplateError::single(
            "invalid_reference",
            "/source/path",
            "reference package cannot be opened",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(TemplateError::single(
            "invalid_reference",
            "/source/path",
            "reference package must be a regular non-symlink file",
        ));
    }
    let canonical = std::fs::canonicalize(&candidate).map_err(|_| {
        TemplateError::single(
            "invalid_reference",
            "/source/path",
            "reference package cannot be resolved",
        )
    })?;
    if !canonical.starts_with(&base) {
        return Err(TemplateError::single(
            "reference_escape",
            "/source/path",
            "reference package escapes the template base directory",
        ));
    }
    Ok(canonical)
}

fn validate_and_resolve(
    template: &TemplateSpec,
    data: &TemplateData,
) -> Result<EffectiveData, TemplateError> {
    if template.variables.len() > MAX_VARIABLES {
        return Err(TemplateError::single(
            "limit_exceeded",
            "/variables",
            format!("at most {MAX_VARIABLES} variables are allowed"),
        ));
    }
    if data.values.len() > MAX_VARIABLES {
        return Err(TemplateError::single(
            "limit_exceeded",
            "/values",
            format!("at most {MAX_VARIABLES} values are allowed"),
        ));
    }
    let mut issues = Vec::new();
    let mut constraints = CompiledConstraints::default();
    for (name, spec) in &template.variables {
        if issues_saturated(&issues) {
            break;
        }
        validate_name(
            name,
            &format!("/variables/{}", pointer_escape(name)),
            &mut issues,
        );
        validate_variable_declaration(name, spec, &mut constraints, &mut issues);
    }
    for name in data.values.keys() {
        if issues_saturated(&issues) {
            break;
        }
        if !template.variables.contains_key(name) {
            issue(
                &mut issues,
                "unknown_variable",
                format!("/values/{}", pointer_escape(name)),
                "data contains a variable not declared by the template",
            );
        }
    }

    let mut values = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    let mut provided = Vec::new();
    let mut defaulted = Vec::new();
    for (name, spec) in &template.variables {
        if issues_saturated(&issues) {
            break;
        }
        let pointer = format!("/values/{}", pointer_escape(name));
        let candidate = if let Some(value) = data.values.get(name) {
            provided.push(name.clone());
            Some(value.clone())
        } else if let Some(value) = spec.default_value() {
            defaulted.push(name.clone());
            Some(value)
        } else {
            None
        };
        if candidate.is_none() && spec.required() {
            issue(
                &mut issues,
                "required",
                &pointer,
                "required variable is missing",
            );
            continue;
        }
        let Some(candidate) = candidate else {
            continue;
        };
        let candidate = normalize_variable_value(spec, candidate);
        validate_variable_value(name, spec, &candidate, &pointer, &constraints, &mut issues);
        kinds.insert(name.clone(), spec.value_kind());
        values.insert(name.clone(), candidate);
    }

    validate_reference_contract(template, &mut issues);
    if issues.is_empty() {
        Ok(EffectiveData {
            values,
            kinds,
            provided,
            defaulted,
        })
    } else {
        Err(TemplateError::new(issues))
    }
}

impl VariableSpec {
    fn required(&self) -> bool {
        match self {
            Self::String { required, .. }
            | Self::Number { required, .. }
            | Self::Bool { required, .. }
            | Self::Date { required, .. }
            | Self::Enum { required, .. }
            | Self::RichBlocks { required, .. }
            | Self::List { required, .. } => *required,
        }
    }

    pub fn secret(&self) -> bool {
        match self {
            Self::String { secret, .. }
            | Self::Number { secret, .. }
            | Self::Bool { secret, .. }
            | Self::Date { secret, .. }
            | Self::Enum { secret, .. }
            | Self::RichBlocks { secret, .. }
            | Self::List { secret, .. } => *secret,
        }
    }

    fn value_kind(&self) -> ValueKind {
        match self {
            Self::RichBlocks { .. } => ValueKind::RichBlocks,
            Self::List { .. } => ValueKind::List,
            _ => ValueKind::Scalar,
        }
    }

    fn default_value(&self) -> Option<Value> {
        match self {
            Self::String { default, .. }
            | Self::Date { default, .. }
            | Self::Enum { default, .. } => default.clone().map(Value::String),
            Self::Number { default, .. } => default
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            Self::Bool { default, .. } => default.map(Value::Bool),
            Self::RichBlocks { default, .. } => default.clone().map(Value::Array),
            Self::List { default, .. } => default.clone().map(|items| {
                Value::Array(
                    items
                        .into_iter()
                        .map(|item| Value::Object(item.into_iter().collect()))
                        .collect(),
                )
            }),
        }
    }
}

fn declaration_key(variable: &str, field: Option<&str>) -> String {
    field.map_or_else(
        || variable.to_string(),
        |field| format!("{variable}.{field}"),
    )
}

fn validate_variable_declaration(
    name: &str,
    spec: &VariableSpec,
    constraints: &mut CompiledConstraints,
    issues: &mut Vec<TemplateIssue>,
) {
    let pointer = format!("/variables/{}", pointer_escape(name));
    match spec {
        VariableSpec::String {
            default,
            regex,
            min_length,
            max_length,
            ..
        } => {
            validate_length_declaration(*min_length, *max_length, &pointer, issues);
            compile_regex(
                regex.as_deref(),
                declaration_key(name, None),
                &pointer,
                constraints,
                issues,
            );
            if default
                .as_ref()
                .is_some_and(|value| value.chars().count() > MAX_STRING_CHARS)
            {
                issue(
                    issues,
                    "limit_exceeded",
                    &pointer,
                    "string default exceeds the hard character limit",
                );
            }
        }
        VariableSpec::Number {
            default, min, max, ..
        } => validate_number_declaration(*default, *min, *max, &pointer, issues),
        VariableSpec::Bool { .. } => {}
        VariableSpec::Date {
            default, min, max, ..
        } => validate_date_declaration(
            default.as_deref(),
            min.as_deref(),
            max.as_deref(),
            &pointer,
            issues,
        ),
        VariableSpec::Enum {
            values, default, ..
        } => validate_enum_declaration(values, default.as_deref(), &pointer, issues),
        VariableSpec::RichBlocks {
            min_items,
            max_items,
            ..
        } => validate_item_declaration(*min_items, *max_items, MAX_RICH_BLOCKS, &pointer, issues),
        VariableSpec::List {
            fields,
            min_items,
            max_items,
            ..
        } => {
            validate_item_declaration(*min_items, *max_items, MAX_LIST_ITEMS, &pointer, issues);
            if fields.is_empty() || fields.len() > 256 {
                issue(
                    issues,
                    "invalid_constraint",
                    format!("{pointer}/fields"),
                    "list must declare 1..=256 item fields",
                );
            }
            for (field_name, field) in fields {
                if issues_saturated(issues) {
                    break;
                }
                let field_pointer = format!("{pointer}/fields/{}", pointer_escape(field_name));
                validate_name(field_name, &field_pointer, issues);
                validate_item_field_declaration(
                    name,
                    field_name,
                    field,
                    &field_pointer,
                    constraints,
                    issues,
                );
            }
        }
    }
}

fn validate_item_field_declaration(
    variable: &str,
    field_name: &str,
    spec: &ItemFieldSpec,
    pointer: &str,
    constraints: &mut CompiledConstraints,
    issues: &mut Vec<TemplateIssue>,
) {
    match spec {
        ItemFieldSpec::String {
            default,
            regex,
            min_length,
            max_length,
            ..
        } => {
            validate_length_declaration(*min_length, *max_length, pointer, issues);
            compile_regex(
                regex.as_deref(),
                declaration_key(variable, Some(field_name)),
                pointer,
                constraints,
                issues,
            );
            if default
                .as_ref()
                .is_some_and(|value| value.chars().count() > MAX_STRING_CHARS)
            {
                issue(
                    issues,
                    "limit_exceeded",
                    pointer,
                    "string default exceeds the hard character limit",
                );
            }
        }
        ItemFieldSpec::Number {
            default, min, max, ..
        } => validate_number_declaration(*default, *min, *max, pointer, issues),
        ItemFieldSpec::Bool { .. } => {}
        ItemFieldSpec::Date {
            default, min, max, ..
        } => validate_date_declaration(
            default.as_deref(),
            min.as_deref(),
            max.as_deref(),
            pointer,
            issues,
        ),
        ItemFieldSpec::Enum {
            values, default, ..
        } => validate_enum_declaration(values, default.as_deref(), pointer, issues),
    }
}

fn validate_length_declaration(
    min: Option<usize>,
    max: Option<usize>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    if min.is_some_and(|value| value > MAX_STRING_CHARS)
        || max.is_some_and(|value| value > MAX_STRING_CHARS)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            format!("length bounds cannot exceed {MAX_STRING_CHARS}"),
        );
    }
    if min
        .zip(max)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "min_length cannot exceed max_length",
        );
    }
}

fn validate_number_declaration(
    default: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    if default.is_some_and(|value| !value.is_finite()) {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "number default must be finite",
        );
    }
    if min.is_some_and(|value| !value.is_finite())
        || max.is_some_and(|value| !value.is_finite())
        || min
            .zip(max)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "number bounds must be finite and ordered",
        );
    }
}

fn validate_date_declaration(
    default: Option<&str>,
    min: Option<&str>,
    max: Option<&str>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    if default.is_some_and(|value| !is_iso_date(value))
        || min.is_some_and(|value| !is_iso_date(value))
        || max.is_some_and(|value| !is_iso_date(value))
        || min
            .zip(max)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "date default and bounds must be valid ordered YYYY-MM-DD dates",
        );
    }
}

fn validate_enum_declaration(
    values: &[String],
    default: Option<&str>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    let unique = values.iter().collect::<BTreeSet<_>>();
    if values.is_empty() || values.len() > 256 || unique.len() != values.len() {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "enum must declare 1..=256 unique strings",
        );
    }
    if default.is_some_and(|value| !values.iter().any(|allowed| allowed == value)) {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "enum default must be one of the declared values",
        );
    }
}

fn validate_item_declaration(
    min: Option<usize>,
    max: Option<usize>,
    hard_max: usize,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    if min.is_some_and(|value| value > hard_max)
        || max.is_some_and(|value| value > hard_max)
        || min
            .zip(max)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            format!("item bounds must be ordered and cannot exceed {hard_max}"),
        );
    }
}

fn compile_regex(
    pattern: Option<&str>,
    key: String,
    pointer: &str,
    constraints: &mut CompiledConstraints,
    issues: &mut Vec<TemplateIssue>,
) {
    let Some(pattern) = pattern else {
        return;
    };
    if pattern.chars().count() > MAX_REGEX_CHARS {
        issue(
            issues,
            "limit_exceeded",
            pointer,
            format!("regex cannot exceed {MAX_REGEX_CHARS} Unicode scalars"),
        );
        return;
    }
    match RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .nest_limit(64)
        .build()
    {
        Ok(regex) => {
            constraints.regexes.insert(key, regex);
        }
        Err(_) => issue(
            issues,
            "invalid_constraint",
            pointer,
            "declared regex is invalid or exceeds compiler limits",
        ),
    }
}

fn normalize_variable_value(spec: &VariableSpec, value: Value) -> Value {
    let VariableSpec::List { fields, .. } = spec else {
        return value;
    };
    let Value::Array(items) = value else {
        return value;
    };
    Value::Array(
        items
            .into_iter()
            .map(|item| {
                let Value::Object(mut object) = item else {
                    return item;
                };
                for (name, field) in fields {
                    if !object.contains_key(name)
                        && let Some(default) = field.default_value()
                    {
                        object.insert(name.clone(), default);
                    }
                }
                Value::Object(object)
            })
            .collect(),
    )
}

fn validate_variable_value(
    name: &str,
    spec: &VariableSpec,
    value: &Value,
    pointer: &str,
    constraints: &CompiledConstraints,
    issues: &mut Vec<TemplateIssue>,
) {
    match spec {
        VariableSpec::String {
            regex,
            min_length,
            max_length,
            ..
        } => validate_string(
            value,
            regex
                .as_ref()
                .and_then(|_| constraints.regexes.get(&declaration_key(name, None))),
            *min_length,
            *max_length,
            pointer,
            issues,
        ),
        VariableSpec::Number { min, max, .. } => {
            validate_number(value, *min, *max, pointer, issues)
        }
        VariableSpec::Bool { .. } => {
            if !value.is_boolean() {
                type_issue(issues, pointer, "bool");
            }
        }
        VariableSpec::Date { min, max, .. } => {
            validate_date(value, min.as_deref(), max.as_deref(), pointer, issues)
        }
        VariableSpec::Enum { values, .. } => {
            if values.is_empty() || values.len() > 256 {
                issue(
                    issues,
                    "invalid_constraint",
                    pointer,
                    "enum declaration must contain 1..=256 values",
                );
            }
            let unique = values.iter().collect::<BTreeSet<_>>();
            if unique.len() != values.len() {
                issue(
                    issues,
                    "invalid_constraint",
                    pointer,
                    "enum declaration contains duplicate values",
                );
            }
            match value.as_str() {
                Some(text) if values.iter().any(|allowed| allowed == text) => {}
                Some(_) => issue(
                    issues,
                    "enum_mismatch",
                    pointer,
                    "value is not in the declared enum",
                ),
                None => type_issue(issues, pointer, "enum string"),
            }
        }
        VariableSpec::RichBlocks {
            min_items,
            max_items,
            ..
        } => match value.as_array() {
            Some(blocks) => {
                validate_item_bounds(
                    blocks.len(),
                    *min_items,
                    *max_items,
                    MAX_RICH_BLOCKS,
                    pointer,
                    issues,
                );
                for (index, block) in blocks.iter().enumerate() {
                    if issues_saturated(issues) {
                        break;
                    }
                    if contains_image_node(block) {
                        issue(
                            issues,
                            "static_asset_required",
                            format!("{pointer}/{index}"),
                            "data-provided rich_blocks cannot contain image asset paths",
                        );
                    }
                    if serde_json::from_value::<crate::document_spec::BlockSpec>(block.clone())
                        .is_err()
                    {
                        issue(
                            issues,
                            "invalid_rich_block",
                            format!("{pointer}/{index}"),
                            "rich_blocks item must be one native DocumentSpec v1 block",
                        );
                    }
                }
            }
            None => type_issue(issues, pointer, "rich_blocks array"),
        },
        VariableSpec::List {
            fields,
            min_items,
            max_items,
            ..
        } => match value.as_array() {
            Some(items) => {
                validate_item_bounds(
                    items.len(),
                    *min_items,
                    *max_items,
                    MAX_LIST_ITEMS,
                    pointer,
                    issues,
                );
                if fields.is_empty() || fields.len() > 256 {
                    issue(
                        issues,
                        "invalid_constraint",
                        pointer,
                        "list must declare 1..=256 item fields",
                    );
                }
                for field in fields.keys() {
                    if issues_saturated(issues) {
                        break;
                    }
                    validate_name(field, pointer, issues);
                }
                for (index, item) in items.iter().enumerate() {
                    if issues_saturated(issues) {
                        break;
                    }
                    validate_list_item(
                        name,
                        item,
                        fields,
                        &format!("{pointer}/{index}"),
                        constraints,
                        issues,
                    );
                }
            }
            None => type_issue(issues, pointer, "list array"),
        },
    }
}

fn validate_list_item(
    variable_name: &str,
    value: &Value,
    fields: &BTreeMap<String, ItemFieldSpec>,
    pointer: &str,
    constraints: &CompiledConstraints,
    issues: &mut Vec<TemplateIssue>,
) {
    let Some(object) = value.as_object() else {
        return type_issue(issues, pointer, "object");
    };
    for key in object.keys() {
        if issues_saturated(issues) {
            break;
        }
        if !fields.contains_key(key) {
            issue(
                issues,
                "unknown_field",
                format!("{pointer}/{}", pointer_escape(key)),
                "list item contains an undeclared field",
            );
        }
    }
    for (name, spec) in fields {
        if issues_saturated(issues) {
            break;
        }
        let field_pointer = format!("{pointer}/{}", pointer_escape(name));
        let candidate = object.get(name).cloned().or_else(|| spec.default_value());
        if candidate.is_none() && spec.required() {
            issue(
                issues,
                "required",
                &field_pointer,
                "required item field is missing",
            );
        } else if let Some(candidate) = candidate {
            spec.validate(
                &candidate,
                &field_pointer,
                constraints
                    .regexes
                    .get(&declaration_key(variable_name, Some(name))),
                issues,
            );
        }
    }
}

impl ItemFieldSpec {
    fn required(&self) -> bool {
        match self {
            Self::String { required, .. }
            | Self::Number { required, .. }
            | Self::Bool { required, .. }
            | Self::Date { required, .. }
            | Self::Enum { required, .. } => *required,
        }
    }

    fn default_value(&self) -> Option<Value> {
        match self {
            Self::String { default, .. }
            | Self::Date { default, .. }
            | Self::Enum { default, .. } => default.clone().map(Value::String),
            Self::Number { default, .. } => default
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            Self::Bool { default, .. } => default.map(Value::Bool),
        }
    }

    fn validate(
        &self,
        value: &Value,
        pointer: &str,
        compiled_regex: Option<&regex::Regex>,
        issues: &mut Vec<TemplateIssue>,
    ) {
        match self {
            Self::String {
                min_length,
                max_length,
                ..
            } => validate_string(
                value,
                compiled_regex,
                *min_length,
                *max_length,
                pointer,
                issues,
            ),
            Self::Number { min, max, .. } => validate_number(value, *min, *max, pointer, issues),
            Self::Bool { .. } => {
                if !value.is_boolean() {
                    type_issue(issues, pointer, "bool");
                }
            }
            Self::Date { min, max, .. } => {
                validate_date(value, min.as_deref(), max.as_deref(), pointer, issues)
            }
            Self::Enum { values, .. } => match value.as_str() {
                Some(text) if values.iter().any(|allowed| allowed == text) => {}
                Some(_) => issue(
                    issues,
                    "enum_mismatch",
                    pointer,
                    "value is not in the declared enum",
                ),
                None => type_issue(issues, pointer, "enum string"),
            },
        }
    }
}

fn validate_string(
    value: &Value,
    regex: Option<&regex::Regex>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    let Some(text) = value.as_str() else {
        return type_issue(issues, pointer, "string");
    };
    let length = text.chars().count();
    let maximum = max_length.unwrap_or(MAX_STRING_CHARS);
    if min_length.is_some_and(|minimum| length < minimum) || length > maximum {
        issue(
            issues,
            "length_mismatch",
            pointer,
            "string length is outside the declared bounds",
        );
    }
    if regex.is_some_and(|regex| !regex.is_match(text)) {
        issue(
            issues,
            "regex_mismatch",
            pointer,
            "string does not match the declared regex",
        );
    }
}

fn validate_number(
    value: &Value,
    min: Option<f64>,
    max: Option<f64>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    let Some(number) = value.as_f64() else {
        return type_issue(issues, pointer, "number");
    };
    if !number.is_finite() {
        issue(issues, "invalid_number", pointer, "number must be finite");
    }
    if min.is_some_and(|minimum| number < minimum) || max.is_some_and(|maximum| number > maximum) {
        issue(
            issues,
            "range_mismatch",
            pointer,
            "number is outside the declared bounds",
        );
    }
    if min.is_some_and(|minimum| !minimum.is_finite())
        || max.is_some_and(|maximum| !maximum.is_finite())
        || min
            .zip(max)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "number bounds must be finite and ordered",
        );
    }
}

fn validate_date(
    value: &Value,
    min: Option<&str>,
    max: Option<&str>,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    let Some(date) = value.as_str() else {
        return type_issue(issues, pointer, "ISO date string");
    };
    if !is_iso_date(date) {
        issue(
            issues,
            "invalid_date",
            pointer,
            "date must be an exact YYYY-MM-DD calendar date",
        );
        return;
    }
    if min.is_some_and(|minimum| !is_iso_date(minimum))
        || max.is_some_and(|maximum| !is_iso_date(maximum))
        || min
            .zip(max)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "date bounds must be valid ordered YYYY-MM-DD dates",
        );
    } else if min.is_some_and(|minimum| date < minimum) || max.is_some_and(|maximum| date > maximum)
    {
        issue(
            issues,
            "range_mismatch",
            pointer,
            "date is outside the declared bounds",
        );
    }
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return false;
    }
    let year = value[0..4].parse::<u32>().ok();
    let month = value[5..7].parse::<u32>().ok();
    let day = value[8..10].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

fn validate_item_bounds(
    actual: usize,
    min: Option<usize>,
    max: Option<usize>,
    hard_max: usize,
    pointer: &str,
    issues: &mut Vec<TemplateIssue>,
) {
    let maximum = max.unwrap_or(hard_max);
    if min.is_some_and(|minimum| minimum > maximum) {
        issue(
            issues,
            "invalid_constraint",
            pointer,
            "min_items cannot exceed max_items",
        );
    }
    if min.is_some_and(|minimum| actual < minimum) || actual > maximum || actual > hard_max {
        issue(
            issues,
            "item_count_mismatch",
            pointer,
            "item count is outside the declared bounds",
        );
    }
}

fn validate_reference_contract(template: &TemplateSpec, issues: &mut Vec<TemplateIssue>) {
    let bindings = match &template.source {
        TemplateSource::ReferenceHwpx { path, bindings } => {
            if path.extension().and_then(|value| value.to_str()) != Some("hwpx") {
                issue(
                    issues,
                    "invalid_reference",
                    "/source/path",
                    "reference package must use the .hwpx extension",
                );
            }
            bindings
        }
        TemplateSource::ReferenceRegenerate {
            path,
            strict_unsupported_objects,
            ..
        } => {
            if path.extension().and_then(|value| value.to_str()) != Some("hwpx") {
                issue(
                    issues,
                    "invalid_reference",
                    "/source/path",
                    "reference package must use the .hwpx extension",
                );
            }
            if !strict_unsupported_objects {
                issue(
                    issues,
                    "strict_gate_required",
                    "/source/strict_unsupported_objects",
                    "reference regeneration requires strict_unsupported_objects=true",
                );
            }
            return;
        }
        TemplateSource::Compose { .. } => return,
    };
    if bindings.is_empty() || bindings.len() > MAX_REFERENCE_TARGETS {
        issue(
            issues,
            "invalid_binding_count",
            "/source/bindings",
            format!("reference mode requires 1..={MAX_REFERENCE_TARGETS} bindings"),
        );
    }
    let mut targets = BTreeSet::new();
    let mut regions = BTreeSet::new();
    for (index, binding) in bindings.iter().enumerate() {
        let pointer = format!("/source/bindings/{index}");
        validate_name(&binding.region, &format!("{pointer}/region"), issues);
        if !regions.insert(binding.region.as_str()) {
            issue(
                issues,
                "duplicate_region",
                format!("{pointer}/region"),
                "reference region id is already used",
            );
        }
        let Some(variable) = template.variables.get(&binding.variable) else {
            issue(
                issues,
                "unknown_variable",
                format!("{pointer}/variable"),
                "binding references an undeclared variable",
            );
            continue;
        };
        if variable.value_kind() != ValueKind::Scalar {
            issue(
                issues,
                "unsupported_reference_binding",
                format!("{pointer}/variable"),
                "reference text and field bindings require a scalar variable",
            );
        }
        validate_reference_target(&binding.name, &format!("{pointer}/name"), issues);
        let key = match binding.target {
            ReferenceTarget::Placeholder => {
                format!("placeholder:{}", binding.name)
            }
            ReferenceTarget::Field => {
                format!("field:{}", binding.name)
            }
        };
        if !targets.insert(key) {
            issue(
                issues,
                "duplicate_target",
                &pointer,
                "reference target is bound more than once",
            );
        }
    }
}

fn reference_bindings(
    bindings: &[ReferenceBinding],
    data: &EffectiveData,
    expander: &mut Expander<'_>,
) -> Result<ReferenceValues, TemplateError> {
    let mut placeholders = BTreeMap::new();
    let mut fields = BTreeMap::new();
    for (index, binding) in bindings.iter().enumerate() {
        let value = data.values.get(&binding.variable).ok_or_else(|| {
            TemplateError::single(
                "missing_value",
                format!("/source/bindings/{index}/variable"),
                "reference binding variable was not provided or defaulted",
            )
        })?;
        let text = scalar_text(value).ok_or_else(|| {
            TemplateError::single(
                "type_mismatch",
                format!("/source/bindings/{index}/variable"),
                "reference binding value is not scalar",
            )
        })?;
        let escaped_upper_bound = text.len().checked_mul(6).ok_or_else(|| {
            TemplateError::single(
                "limit_exceeded",
                format!("/source/bindings/{index}"),
                "reference replacement size overflow",
            )
        })?;
        expander.add_work_bytes(escaped_upper_bound, &format!("/source/bindings/{index}"))?;
        let kind = match binding.target {
            ReferenceTarget::Placeholder => {
                placeholders.insert(binding.name.clone(), text);
                RegionKind::Placeholder
            }
            ReferenceTarget::Field => {
                fields.insert(binding.name.clone(), text);
                RegionKind::Field
            }
        };
        expander.add_region(RegionPlan {
            id: binding.region.clone(),
            kind,
            template_pointer: format!("/source/bindings/{index}"),
            input_items: 1,
            generated_items: 1,
            instances: Vec::new(),
        })?;
    }
    Ok((placeholders, fields))
}

struct Expander<'a> {
    data: &'a EffectiveData,
    plan: ExpansionPlan,
    region_ids: BTreeSet<String>,
    work_nodes: usize,
    work_bytes: usize,
    instance_stack: Vec<usize>,
}

impl<'a> Expander<'a> {
    fn new(data: &'a EffectiveData) -> Self {
        Self {
            data,
            plan: ExpansionPlan::default(),
            region_ids: BTreeSet::new(),
            work_nodes: 0,
            work_bytes: 0,
            instance_stack: Vec::new(),
        }
    }

    fn expand_document(&mut self, document: &Value) -> Result<Value, TemplateError> {
        self.plan.input_nodes = count_json_nodes(document);
        if self.plan.input_nodes > MAX_EXPANDED_NODES {
            return Err(TemplateError::single(
                "limit_exceeded",
                "/source/document",
                format!("template nodes exceed {MAX_EXPANDED_NODES}"),
            ));
        }
        let expanded = self.expand_value(document, "/source/document", None, 0)?;
        self.plan.expanded_nodes = count_json_nodes(&expanded);
        if self.plan.expanded_nodes > MAX_EXPANDED_NODES {
            return Err(TemplateError::single(
                "limit_exceeded",
                "/source/document",
                format!("expanded nodes exceed {MAX_EXPANDED_NODES}"),
            ));
        }
        self.plan.expanded_bytes = serde_json::to_vec(&expanded)
            .map_err(|_| {
                TemplateError::single(
                    "serialization_error",
                    "/source/document",
                    "expanded document cannot be serialized",
                )
            })?
            .len();
        if self.plan.expanded_bytes > MAX_EXPANDED_BYTES {
            return Err(TemplateError::single(
                "limit_exceeded",
                "/source/document",
                format!("expanded document exceeds {MAX_EXPANDED_BYTES} bytes"),
            ));
        }
        Ok(expanded)
    }

    fn expand_value(
        &mut self,
        value: &Value,
        pointer: &str,
        item: Option<&Map<String, Value>>,
        control_depth: usize,
    ) -> Result<Value, TemplateError> {
        self.bump_work_node(pointer)?;
        match value {
            Value::Object(object) if object.contains_key("node") => {
                let node = object.get("node").and_then(Value::as_str).ok_or_else(|| {
                    TemplateError::single(
                        "invalid_ast",
                        format!("{pointer}/node"),
                        "node discriminator must be a string",
                    )
                })?;
                match node {
                    "value" => self.expand_binding(object, pointer, item),
                    "if" | "each" => Err(TemplateError::single(
                        "unsupported_context",
                        pointer,
                        "if and each nodes are allowed only as block or table-row array items",
                    )),
                    _ => Err(TemplateError::single(
                        "unknown_node",
                        format!("{pointer}/node"),
                        "node must be value, if, or each",
                    )),
                }
            }
            Value::Object(object) => {
                let mut expanded = Map::new();
                for (key, child) in object {
                    let child_pointer = format!("{pointer}/{}", pointer_escape(key));
                    if key == "path" && contains_template_node(child) {
                        return Err(TemplateError::single(
                            "static_asset_required",
                            &child_pointer,
                            "filesystem-bearing DocumentSpec path fields cannot be data-bound",
                        ));
                    }
                    if child
                        .as_object()
                        .and_then(|object| object.get("node"))
                        .and_then(Value::as_str)
                        == Some("value")
                        && !allowed_binding_target(key)
                    {
                        return Err(TemplateError::single(
                            "unsafe_binding_target",
                            &child_pointer,
                            "data values can bind only content or numeric/boolean presentation properties",
                        ));
                    }
                    self.add_work_bytes(key.len().saturating_add(4), &child_pointer)?;
                    let context = array_context(pointer, key);
                    let value = if let Value::Array(items) = child {
                        self.expand_array(items, &child_pointer, item, control_depth, context)?
                    } else {
                        self.expand_value(child, &child_pointer, item, control_depth)?
                    };
                    expanded.insert(key.clone(), value);
                }
                Ok(Value::Object(expanded))
            }
            Value::Array(items) => {
                self.expand_array(items, pointer, item, control_depth, ArrayContext::Other)
            }
            _ => {
                self.add_work_bytes(serialized_len(value), pointer)?;
                Ok(value.clone())
            }
        }
    }

    fn expand_array(
        &mut self,
        items: &[Value],
        pointer: &str,
        item: Option<&Map<String, Value>>,
        control_depth: usize,
        context: ArrayContext,
    ) -> Result<Value, TemplateError> {
        let mut output = Vec::new();
        for (index, value) in items.iter().enumerate() {
            let item_pointer = format!("{pointer}/{index}");
            if let Some(object) = value.as_object()
                && let Some(node) = object.get("node").and_then(Value::as_str)
            {
                self.bump_work_node(&item_pointer)?;
                match node {
                    "if" => {
                        self.require_control_context(context, &item_pointer)?;
                        let generated = self.expand_if(
                            object,
                            &item_pointer,
                            item,
                            control_depth + 1,
                            context,
                        )?;
                        output.extend(generated);
                        continue;
                    }
                    "each" => {
                        self.require_control_context(context, &item_pointer)?;
                        let generated = self.expand_each(
                            object,
                            &item_pointer,
                            item,
                            control_depth + 1,
                            context,
                        )?;
                        output.extend(generated);
                        continue;
                    }
                    "value" if context == ArrayContext::Blocks => {
                        let (value, kind) = self.resolve_binding(object, &item_pointer, item)?;
                        if kind == ValueKind::RichBlocks {
                            let blocks = value.as_array().expect("validated rich_blocks");
                            self.add_work_nodes(count_json_nodes(&value), &item_pointer)?;
                            output.extend(blocks.iter().cloned());
                            self.add_region(RegionPlan {
                                id: required_string(object, "region", &item_pointer)?.to_string(),
                                kind: RegionKind::RichBlocks,
                                template_pointer: item_pointer,
                                input_items: blocks.len(),
                                generated_items: blocks.len(),
                                instances: Vec::new(),
                            })?;
                            continue;
                        }
                    }
                    _ => {}
                }
            }
            output.push(self.expand_value(value, &item_pointer, item, control_depth)?);
        }
        Ok(Value::Array(output))
    }

    fn expand_binding(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        item: Option<&Map<String, Value>>,
    ) -> Result<Value, TemplateError> {
        let (value, _) = self.resolve_binding(object, pointer, item)?;
        Ok(value)
    }

    fn resolve_binding(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        item: Option<&Map<String, Value>>,
    ) -> Result<(Value, ValueKind), TemplateError> {
        reject_unknown(object, &["node", "pointer", "as", "region"], pointer)?;
        let binding = required_string(object, "pointer", pointer)?;
        let (value, kind) = resolve_pointer(binding, self.data, item, pointer)?;
        let output = match object.get("as").and_then(Value::as_str).unwrap_or("native") {
            "native" => {
                self.add_work_bytes(serialized_len(value), pointer)?;
                value.clone()
            }
            "text" => {
                let text = scalar_text(value).ok_or_else(|| {
                    TemplateError::single(
                        "type_mismatch",
                        format!("{pointer}/as"),
                        "as=text requires a scalar value",
                    )
                })?;
                self.add_work_bytes(text.len().saturating_add(2), pointer)?;
                Value::String(text)
            }
            _ => {
                return Err(TemplateError::single(
                    "invalid_ast",
                    format!("{pointer}/as"),
                    "as must be native or text",
                ));
            }
        };
        Ok((output, kind))
    }

    fn expand_if(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        item: Option<&Map<String, Value>>,
        control_depth: usize,
        context: ArrayContext,
    ) -> Result<Vec<Value>, TemplateError> {
        self.check_depth(control_depth, pointer)?;
        reject_unknown(
            object,
            &["node", "condition", "then", "else", "region"],
            pointer,
        )?;
        let condition_pointer = required_string(object, "condition", pointer)?;
        let (condition, _) = resolve_pointer(condition_pointer, self.data, item, pointer)?;
        let condition = condition.as_bool().ok_or_else(|| {
            TemplateError::single(
                "type_mismatch",
                format!("{pointer}/condition"),
                "if condition must resolve to bool",
            )
        })?;
        let then = required_array(object, "then", pointer)?;
        let otherwise = optional_array(object, "else", pointer)?.unwrap_or(&[]);
        let selected = if condition { then } else { otherwise };
        self.plan.conditions_evaluated = self.plan.conditions_evaluated.saturating_add(1);
        let Value::Array(output) = self.expand_array(
            selected,
            &format!("{pointer}/{}", if condition { "then" } else { "else" }),
            item,
            control_depth,
            context,
        )?
        else {
            unreachable!()
        };
        self.add_region(RegionPlan {
            id: required_string(object, "region", pointer)?.to_string(),
            kind: RegionKind::Conditional,
            template_pointer: pointer.to_string(),
            input_items: 1,
            generated_items: output.len(),
            instances: Vec::new(),
        })?;
        Ok(output)
    }

    fn expand_each(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        item: Option<&Map<String, Value>>,
        control_depth: usize,
        context: ArrayContext,
    ) -> Result<Vec<Value>, TemplateError> {
        self.check_depth(control_depth, pointer)?;
        reject_unknown(object, &["node", "items", "body", "region"], pointer)?;
        let items_pointer = required_string(object, "items", pointer)?;
        let (items, kind) = resolve_pointer(items_pointer, self.data, item, pointer)?;
        if !matches!(kind, ValueKind::List | ValueKind::RichBlocks) {
            return Err(TemplateError::single(
                "type_mismatch",
                format!("{pointer}/items"),
                "each items must resolve to list or rich_blocks",
            ));
        }
        let items = items.as_array().ok_or_else(|| {
            TemplateError::single(
                "type_mismatch",
                format!("{pointer}/items"),
                "each items must be an array",
            )
        })?;
        if items.len() > MAX_LIST_ITEMS {
            return Err(TemplateError::single(
                "limit_exceeded",
                format!("{pointer}/items"),
                format!("one each cannot exceed {MAX_LIST_ITEMS} items"),
            ));
        }
        let body = required_array(object, "body", pointer)?;
        let mut output = Vec::new();
        for (index, entry) in items.iter().enumerate() {
            self.plan.each_iterations = self.plan.each_iterations.saturating_add(1);
            if self.plan.each_iterations > MAX_EACH_ITERATIONS {
                return Err(TemplateError::single(
                    "limit_exceeded",
                    pointer,
                    format!("total each iterations exceed {MAX_EACH_ITERATIONS}"),
                ));
            }
            let object = entry.as_object().ok_or_else(|| {
                TemplateError::single(
                    "type_mismatch",
                    format!("{pointer}/items/{index}"),
                    "each list items must be objects",
                )
            })?;
            self.instance_stack.push(index);
            let generated = self.expand_array(
                body,
                &format!("{pointer}/body"),
                Some(object),
                control_depth,
                context,
            );
            self.instance_stack.pop();
            let Value::Array(generated) = generated? else {
                unreachable!()
            };
            output.extend(generated);
        }
        self.add_region(RegionPlan {
            id: required_string(object, "region", pointer)?.to_string(),
            kind: RegionKind::Repeated,
            template_pointer: pointer.to_string(),
            input_items: items.len(),
            generated_items: output.len(),
            instances: Vec::new(),
        })?;
        Ok(output)
    }

    fn require_control_context(
        &self,
        context: ArrayContext,
        pointer: &str,
    ) -> Result<(), TemplateError> {
        if matches!(context, ArrayContext::Blocks | ArrayContext::Rows) {
            Ok(())
        } else {
            Err(TemplateError::single(
                "unsupported_context",
                pointer,
                "if and each can expand only block or table-row arrays",
            ))
        }
    }

    fn check_depth(&mut self, depth: usize, pointer: &str) -> Result<(), TemplateError> {
        self.plan.max_control_depth = self.plan.max_control_depth.max(depth);
        if depth > MAX_CONTROL_DEPTH {
            Err(TemplateError::single(
                "limit_exceeded",
                pointer,
                format!("control depth exceeds {MAX_CONTROL_DEPTH}"),
            ))
        } else {
            Ok(())
        }
    }

    fn bump_work_node(&mut self, pointer: &str) -> Result<(), TemplateError> {
        self.add_work_nodes(1, pointer)
    }

    fn add_work_nodes(&mut self, count: usize, pointer: &str) -> Result<(), TemplateError> {
        self.work_nodes = self.work_nodes.saturating_add(count);
        if self.work_nodes > MAX_EXPANDED_NODES {
            Err(TemplateError::single(
                "limit_exceeded",
                pointer,
                format!("expansion work exceeds {MAX_EXPANDED_NODES} nodes"),
            ))
        } else {
            Ok(())
        }
    }

    fn add_work_bytes(&mut self, count: usize, pointer: &str) -> Result<(), TemplateError> {
        self.work_bytes = self.work_bytes.checked_add(count).ok_or_else(|| {
            TemplateError::single("limit_exceeded", pointer, "expanded byte counter overflow")
        })?;
        if self.work_bytes > MAX_EXPANDED_BYTES {
            Err(TemplateError::single(
                "limit_exceeded",
                pointer,
                format!("expansion exceeds {MAX_EXPANDED_BYTES} bytes"),
            ))
        } else {
            Ok(())
        }
    }

    fn add_region(&mut self, mut region: RegionPlan) -> Result<(), TemplateError> {
        validate_name(&region.id, &region.template_pointer, &mut Vec::new());
        if region.id.is_empty()
            || region.id.len() > MAX_VARIABLE_NAME_BYTES
            || !safe_name(&region.id)
        {
            return Err(TemplateError::single(
                "invalid_region",
                &region.template_pointer,
                "region id must match [A-Za-z][A-Za-z0-9_]{0,63}",
            ));
        }
        let instance = RegionInstance {
            path: if self.instance_stack.is_empty() {
                "/".to_string()
            } else {
                format!(
                    "/{}",
                    self.instance_stack
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join("/")
                )
            },
            input_items: region.input_items,
            generated_items: region.generated_items,
        };
        if !self.region_ids.insert(region.id.clone()) {
            let existing = self
                .plan
                .regions
                .iter_mut()
                .find(|existing| existing.id == region.id)
                .expect("region id set and report remain synchronized");
            if existing.kind != region.kind || existing.template_pointer != region.template_pointer
            {
                return Err(TemplateError::single(
                    "duplicate_region",
                    &region.template_pointer,
                    "region id is used by a different template operator",
                ));
            }
            existing.input_items = existing.input_items.saturating_add(region.input_items);
            existing.generated_items = existing
                .generated_items
                .saturating_add(region.generated_items);
            existing.instances.push(instance);
            return Ok(());
        }
        if self.plan.regions.len() >= MAX_REGIONS {
            return Err(TemplateError::single(
                "limit_exceeded",
                &region.template_pointer,
                format!("regions exceed {MAX_REGIONS}"),
            ));
        }
        region.instances.push(instance);
        self.plan.regions.push(region);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), TemplateError> {
        if self.plan.expanded_bytes == 0 {
            self.plan.expanded_bytes = self.work_bytes;
        }
        if self.plan.expanded_nodes > MAX_EXPANDED_NODES
            || self.plan.expanded_bytes > MAX_EXPANDED_BYTES
        {
            Err(TemplateError::single(
                "limit_exceeded",
                "/source",
                "expanded output exceeds the frozen budget",
            ))
        } else {
            Ok(())
        }
    }
}

fn count_json_nodes(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1usize.saturating_add(
            values
                .iter()
                .map(count_json_nodes)
                .fold(0usize, usize::saturating_add),
        ),
        Value::Object(values) => 1usize.saturating_add(
            values
                .values()
                .map(count_json_nodes)
                .fold(0usize, usize::saturating_add),
        ),
        _ => 1,
    }
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(MAX_EXPANDED_BYTES.saturating_add(1))
}

fn contains_template_node(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("node") || object.values().any(contains_template_node)
        }
        Value::Array(values) => values.iter().any(contains_template_node),
        _ => false,
    }
}

fn contains_image_node(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) == Some("image")
                || object.values().any(contains_image_node)
        }
        Value::Array(values) => values.iter().any(contains_image_node),
        _ => false,
    }
}

fn allowed_binding_target(key: &str) -> bool {
    matches!(
        key,
        "text"
            | "value"
            | "title"
            | "author"
            | "subject"
            | "keywords"
            | "description"
            | "script"
            | "width_mm"
            | "height_mm"
            | "margin_left_mm"
            | "margin_right_mm"
            | "margin_top_mm"
            | "margin_bottom_mm"
            | "margin_header_mm"
            | "margin_footer_mm"
            | "gutter_mm"
            | "font_size_pt"
            | "bold"
            | "italic"
            | "underline"
            | "strike"
            | "color"
            | "background"
            | "spacing_before_pt"
            | "spacing_after_pt"
            | "line_height_percent"
            | "keep_with_next"
            | "start"
    )
}

fn array_context(parent_pointer: &str, key: &str) -> ArrayContext {
    if key == "rows" {
        ArrayContext::Rows
    } else if key == "blocks"
        || matches!(key, "default" | "first" | "odd" | "even")
            && (parent_pointer.ends_with("/header") || parent_pointer.ends_with("/footer"))
    {
        ArrayContext::Blocks
    } else {
        ArrayContext::Other
    }
}

fn resolve_pointer<'a>(
    pointer: &str,
    data: &'a EffectiveData,
    item: Option<&'a Map<String, Value>>,
    diagnostic_pointer: &str,
) -> Result<(&'a Value, ValueKind), TemplateError> {
    let segments = pointer.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["", "values", name] if safe_name(name) => {
            let value = data.values.get(*name).ok_or_else(|| {
                TemplateError::single(
                    "missing_value",
                    diagnostic_pointer,
                    "binding points to a declared value that was not provided or defaulted",
                )
            })?;
            Ok((
                value,
                data.kinds.get(*name).copied().unwrap_or(ValueKind::Scalar),
            ))
        }
        ["", "item", field] if safe_name(field) => {
            let item = item.ok_or_else(|| {
                TemplateError::single(
                    "invalid_pointer_scope",
                    diagnostic_pointer,
                    "/item pointers are valid only inside each body",
                )
            })?;
            let value = item.get(*field).ok_or_else(|| {
                TemplateError::single(
                    "missing_item_field",
                    diagnostic_pointer,
                    "item pointer targets a missing field",
                )
            })?;
            Ok((value, ValueKind::Scalar))
        }
        ["", "item"] => {
            let _ = item.ok_or_else(|| {
                TemplateError::single(
                    "invalid_pointer_scope",
                    diagnostic_pointer,
                    "/item is valid only inside each body",
                )
            })?;
            Err(TemplateError::single(
                "unsupported_pointer",
                diagnostic_pointer,
                "whole-item binding is not supported; bind a declared item field",
            ))
        }
        _ => Err(TemplateError::single(
            "invalid_pointer",
            diagnostic_pointer,
            "pointer must be /values/<name> or /item/<field> with a safe identifier",
        )),
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "true" } else { "false" }.to_string()),
        _ => None,
    }
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    pointer: &str,
) -> Result<&'a str, TemplateError> {
    object.get(key).and_then(Value::as_str).ok_or_else(|| {
        TemplateError::single(
            "invalid_ast",
            format!("{pointer}/{}", pointer_escape(key)),
            "required string property is missing",
        )
    })
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    pointer: &str,
) -> Result<&'a [Value], TemplateError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            TemplateError::single(
                "invalid_ast",
                format!("{pointer}/{}", pointer_escape(key)),
                "required array property is missing",
            )
        })
}

fn optional_array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    pointer: &str,
) -> Result<Option<&'a [Value]>, TemplateError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .map(Some)
            .ok_or_else(|| {
                TemplateError::single(
                    "invalid_ast",
                    format!("{pointer}/{}", pointer_escape(key)),
                    "property must be an array",
                )
            }),
    }
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
    pointer: &str,
) -> Result<(), TemplateError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(TemplateError::single(
            "unknown_property",
            format!("{pointer}/{}", pointer_escape(key)),
            "template AST node contains an unknown property",
        ))
    } else {
        Ok(())
    }
}

fn validate_name(name: &str, pointer: &str, issues: &mut Vec<TemplateIssue>) {
    if name.len() > MAX_VARIABLE_NAME_BYTES || !safe_name(name) {
        issue(
            issues,
            "invalid_name",
            pointer,
            "name must match [A-Za-z][A-Za-z0-9_]{0,63}",
        );
    }
}

fn validate_reference_target(name: &str, pointer: &str, issues: &mut Vec<TemplateIssue>) {
    if name.is_empty()
        || name.chars().count() > 128
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '{' | '}' | '\r' | '\n'))
    {
        issue(
            issues,
            "invalid_target",
            pointer,
            "reference target must contain 1..=128 non-control characters without braces",
        );
    }
}

fn safe_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !matches!(name, "__proto__" | "prototype" | "constructor")
}

fn pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn type_issue(issues: &mut Vec<TemplateIssue>, pointer: &str, expected: &str) {
    issue(
        issues,
        "type_mismatch",
        pointer,
        format!("value must be {expected}; coercion is not performed"),
    );
}

fn issue(
    issues: &mut Vec<TemplateIssue>,
    code: &str,
    pointer: impl Into<String>,
    message: impl Into<String>,
) {
    if issues.len() > MAX_ISSUES {
        return;
    }
    if issues.len() == MAX_ISSUES {
        issues.push(TemplateIssue {
            code: "diagnostics_truncated".to_string(),
            pointer: "".to_string(),
            message: "additional diagnostics were omitted".to_string(),
        });
        return;
    }
    issues.push(TemplateIssue {
        code: code.to_string(),
        pointer: pointer.into(),
        message: message.into(),
    });
}

fn issues_saturated(issues: &[TemplateIssue]) -> bool {
    issues.len() > MAX_ISSUES
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(template: &str, data: &str) -> Result<ExpandedTemplate, TemplateError> {
        let template = parse_template(template, SpecInputFormat::Json)?;
        let data = parse_data(data, SpecInputFormat::Json)?;
        expand_template(&template, &data, Path::new("."))
    }

    #[test]
    fn typed_value_if_and_each_expand_without_interpolation() {
        let template = r#"{
          "version":"1.0",
          "variables":{
            "title":{"type":"string","required":true},
            "show":{"type":"bool","required":true},
            "items":{"type":"list","required":true,"fields":{
              "name":{"type":"string","required":true}
            }}
          },
          "source":{"mode":"compose","document":{
            "version":"1.0","sections":[{"blocks":[
              {"node":"if","condition":"/values/show","region":"summary","then":[
                {"type":"paragraph","runs":[{"type":"text","text":{"node":"value","pointer":"/values/title","as":"text"}}]}
              ]},
              {"node":"each","items":"/values/items","region":"items","body":[
                {"type":"paragraph","runs":[{"type":"text","text":{"node":"value","pointer":"/item/name","as":"text"}}]}
              ]}
            ]}]
          }}
        }"#;
        let data = r#"{"version":"1.0","values":{"title":"Report","show":true,"items":[{"name":"A"},{"name":"B"}]}}"#;
        let expanded = parse(template, data).expect("expand");
        let ExpandedOutput::Compose(document) = expanded.output else {
            panic!("compose output");
        };
        assert_eq!(document.sections[0].blocks.len(), 3);
        assert_eq!(expanded.plan.each_iterations, 2);
        assert_eq!(expanded.plan.regions.len(), 2);
    }

    #[test]
    fn type_coercion_and_expression_like_pointer_are_rejected_without_values_in_error() {
        let template = r#"{
          "version":"1.0","variables":{"count":{"type":"number","required":true,"secret":true}},
          "source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[
            {"type":"paragraph","runs":[{"type":"text","text":{"node":"value","pointer":"/values/count + 1","as":"text"}}]}
          ]}]}}
        }"#;
        let error = parse(
            template,
            r#"{"version":"1.0","values":{"count":"secret-value"}}"#,
        )
        .expect_err("no coercion");
        let serialized = serde_json::to_string(error.issues()).unwrap();
        assert!(serialized.contains("type_mismatch"));
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn control_node_in_runs_is_rejected_by_context() {
        let error = parse(
            r#"{"version":"1.0","variables":{"flag":{"type":"bool","default":true}},"source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"node":"if","condition":"/values/flag","region":"bad","then":[]}]}]}]}}}"#,
            r#"{"version":"1.0","values":{}}"#,
        )
        .expect_err("run control");
        assert_eq!(error.issues()[0].code, "unsupported_context");
    }

    #[test]
    fn reference_regeneration_requires_explicit_strict_gate() {
        let error = parse(
            r#"{"version":"1.0","variables":{},"source":{"mode":"reference_regenerate","path":"template.hwpx","strict_unsupported_objects":false,"document":{"version":"1.0","sections":[{"blocks":[]}]}}}"#,
            r#"{"version":"1.0","values":{}}"#,
        )
        .expect_err("strict gate");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code == "strict_gate_required")
        );
    }

    #[test]
    fn exact_iso_date_and_safe_names_are_enforced() {
        let error = parse(
            r#"{"version":"1.0","variables":{"constructor":{"type":"date","required":true}},"source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[]}]}}}"#,
            r#"{"version":"1.0","values":{"constructor":"2025-02-29"}}"#,
        )
        .expect_err("invalid name and date");
        let codes = error
            .issues()
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<Vec<_>>();
        assert!(codes.contains(&"invalid_name"));
        assert!(codes.contains(&"invalid_date"));
    }

    #[test]
    fn nested_regions_are_aggregated_with_deterministic_instance_paths() {
        let template = r#"{
          "version":"1.0",
          "variables":{"items":{"type":"list","required":true,"fields":{
            "name":{"type":"string","required":true},
            "show":{"type":"bool","default":true}
          }}},
          "source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[
            {"node":"each","items":"/values/items","region":"outer","body":[
              {"node":"if","condition":"/item/show","region":"inner","then":[
                {"type":"paragraph","runs":[{"type":"text","text":{"node":"value","pointer":"/item/name","as":"text"}}]}
              ]}
            ]}
          ]}]}}
        }"#;
        let expanded = parse(
            template,
            r#"{"version":"1.0","values":{"items":[{"name":"A"},{"name":"B"}]}}"#,
        )
        .expect("nested regions");
        let inner = expanded
            .plan
            .regions
            .iter()
            .find(|region| region.id == "inner")
            .unwrap();
        assert_eq!(inner.generated_items, 2);
        assert_eq!(
            inner
                .instances
                .iter()
                .map(|instance| instance.path.as_str())
                .collect::<Vec<_>>(),
            ["/0", "/1"]
        );
        let ExpandedOutput::Compose(document) = expanded.output else {
            panic!("compose");
        };
        assert_eq!(document.sections[0].blocks.len(), 2);
    }

    #[test]
    fn secret_identifier_and_asset_bindings_fail_without_canary_leak() {
        for property in ["style", "path"] {
            let mut paragraph = serde_json::json!({
                "type":"paragraph",
                "runs":[{"type":"text","text":"safe"}]
            });
            paragraph[property] = serde_json::json!({
                "node":"value","pointer":"/values/secret","as":"text"
            });
            let template = serde_json::to_string(&serde_json::json!({
                "version":"1.0",
                "variables":{"secret":{"type":"string","required":true,"secret":true}},
                "source":{"mode":"compose","document":{
                    "version":"1.0","sections":[{"blocks":[paragraph]}]
                }}
            }))
            .unwrap();
            let error = parse(
                &template,
                r#"{"version":"1.0","values":{"secret":"TOPSECRET_CANARY"}}"#,
            )
            .expect_err("unsafe binding target");
            let diagnostic = serde_json::to_string(error.issues()).unwrap();
            assert!(
                diagnostic.contains(if property == "path" {
                    "static_asset_required"
                } else {
                    "unsafe_binding_target"
                }),
                "{property}: {diagnostic}"
            );
            assert!(!diagnostic.contains("TOPSECRET_CANARY"));
        }
    }

    #[test]
    fn data_rich_blocks_cannot_select_image_assets() {
        let error = parse(
            r#"{"version":"1.0","variables":{"content":{"type":"rich_blocks","required":true}},"source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[{"node":"value","pointer":"/values/content","region":"content"}]}]}}}"#,
            r#"{"version":"1.0","values":{"content":[{"type":"image","path":"/tmp/TOPSECRET_FILE","width_mm":20}]}}"#,
        )
        .expect_err("data asset");
        let diagnostic = serde_json::to_string(error.issues()).unwrap();
        assert!(diagnostic.contains("static_asset_required"));
        assert!(!diagnostic.contains("TOPSECRET_FILE"));
    }

    #[test]
    fn diagnostics_are_bounded_for_combinatorial_required_fields() {
        let fields = (0..256)
            .map(|index| {
                (
                    format!("f{index}"),
                    serde_json::json!({"type":"string","required":true}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let template = serde_json::json!({
            "version":"1.0",
            "variables":{"items":{"type":"list","required":true,"fields":fields}},
            "source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[]}]}}
        });
        let data = serde_json::json!({
            "version":"1.0",
            "values":{"items": vec![serde_json::json!({}); 10_000]}
        });
        let error = parse(
            &serde_json::to_string(&template).unwrap(),
            &serde_json::to_string(&data).unwrap(),
        )
        .expect_err("bounded diagnostics");
        assert_eq!(error.issues().len(), MAX_ISSUES);
        assert!(error.truncated());
        assert_eq!(error.total_or_at_least(), MAX_ISSUES + 1);
    }

    #[test]
    fn invalid_unused_declarations_and_nonfinite_yaml_are_rejected() {
        let template = parse_template(
            r#"
version: "1.0"
variables:
  unused:
    type: number
    min: .nan
source:
  mode: compose
  document:
    version: "1.0"
    sections:
      - blocks: []
"#,
            SpecInputFormat::Yaml,
        )
        .expect("yaml model");
        let data = parse_data(r#"{"version":"1.0","values":{}}"#, SpecInputFormat::Json).unwrap();
        let error = expand_template(&template, &data, Path::new("."))
            .expect_err("unused invalid declaration");
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code == "invalid_constraint")
        );
    }

    #[test]
    fn parser_errors_redact_unknown_variant_canary() {
        let error = parse_template(
            r#"{"version":"1.0","variables":{"x":{"type":"TOPSECRET_VARIANT"}},"source":{"mode":"compose","document":{}}}"#,
            SpecInputFormat::Json,
        )
        .expect_err("unknown variant");
        assert!(!error.to_string().contains("TOPSECRET_VARIANT"));
    }

    #[test]
    fn list_item_defaults_are_materialized_for_each_bindings() {
        let expanded = parse(
            r#"{"version":"1.0","variables":{"items":{"type":"list","required":true,"fields":{"label":{"type":"string","default":"defaulted"}}}},"source":{"mode":"compose","document":{"version":"1.0","sections":[{"blocks":[{"node":"each","items":"/values/items","region":"rows","body":[{"type":"paragraph","runs":[{"type":"text","text":{"node":"value","pointer":"/item/label","as":"text"}}]}]}]}]}}}"#,
            r#"{"version":"1.0","values":{"items":[{}]}}"#,
        )
        .expect("defaulted item field");
        let ExpandedOutput::Compose(document) = expanded.output else {
            panic!("compose");
        };
        let crate::document_spec::BlockSpec::Paragraph { runs, .. } =
            &document.sections[0].blocks[0]
        else {
            panic!("paragraph");
        };
        let crate::document_spec::RunSpec::Text { text, .. } = &runs[0] else {
            panic!("text");
        };
        assert_eq!(text, "defaulted");
    }

    #[test]
    fn reference_region_contract_is_positive_and_growth_is_bounded() {
        let base =
            std::env::temp_dir().join(format!("hwp-template-reference-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("reference.hwpx"), b"fixture").unwrap();
        let template = parse_template(
            r#"{"version":"1.0","variables":{"name":{"type":"string","required":true}},"source":{"mode":"reference_hwpx","path":"reference.hwpx","bindings":[{"region":"recipient","variable":"name","target":"field","name":"수신"}]}}"#,
            SpecInputFormat::Json,
        )
        .unwrap();
        let data = parse_data(
            r#"{"version":"1.0","values":{"name":"홍길동"}}"#,
            SpecInputFormat::Json,
        )
        .unwrap();
        let expanded = expand_template(&template, &data, &base).expect("reference plan");
        assert_eq!(expanded.plan.regions[0].id, "recipient");

        let escaped = parse_data(
            &serde_json::to_string(&serde_json::json!({
                "version":"1.0",
                "values":{"name":"&".repeat(MAX_STRING_CHARS)}
            }))
            .unwrap(),
            SpecInputFormat::Json,
        )
        .unwrap();
        let two_targets = parse_template(
            r#"{"version":"1.0","variables":{"name":{"type":"string","required":true}},"source":{"mode":"reference_hwpx","path":"reference.hwpx","bindings":[{"region":"one","variable":"name","target":"field","name":"첫째"},{"region":"two","variable":"name","target":"field","name":"둘째"}]}}"#,
            SpecInputFormat::Json,
        )
        .unwrap();
        let error =
            expand_template(&two_targets, &escaped, &base).expect_err("replacement growth budget");
        assert_eq!(error.issues()[0].code, "limit_exceeded");
        let _ = std::fs::remove_file(base.join("reference.hwpx"));
        let _ = std::fs::remove_dir(base);
    }

    #[test]
    fn reference_parent_escape_is_rejected() {
        let base =
            std::env::temp_dir().join(format!("hwp-template-containment-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let template = parse_template(
            r#"{"version":"1.0","variables":{"x":{"type":"string","default":"x"}},"source":{"mode":"reference_hwpx","path":"../outside.hwpx","bindings":[{"region":"x","variable":"x","target":"field","name":"필드"}]}}"#,
            SpecInputFormat::Json,
        )
        .unwrap();
        let data = parse_data(r#"{"version":"1.0","values":{}}"#, SpecInputFormat::Json).unwrap();
        let error = expand_template(&template, &data, &base).expect_err("parent escape");
        assert_eq!(error.issues()[0].code, "invalid_reference");
        let _ = std::fs::remove_dir(base);
    }

    #[test]
    fn checked_in_examples_schema_ids_properties_and_hashes_are_frozen() {
        let template = parse_template(
            include_str!("../../../examples/template-spec-v1/report-template.yaml"),
            SpecInputFormat::Yaml,
        )
        .expect("checked-in TemplateSpec example");
        let data = parse_data(
            include_str!("../../../examples/template-spec-v1/report-data.json"),
            SpecInputFormat::Json,
        )
        .expect("checked-in TemplateData example");
        let expanded = expand_template(
            &template,
            &data,
            Path::new("../../../examples/template-spec-v1"),
        )
        .expect("checked-in example expansion");
        assert_eq!(expanded.plan.each_iterations, 2);
        assert_eq!(expanded.plan.conditions_evaluated, 1);

        let spec_schema: Value = serde_json::from_slice(include_bytes!(
            "../../../schemas/template-spec-v1.schema.json"
        ))
        .expect("TemplateSpec schema JSON");
        let data_schema: Value = serde_json::from_slice(include_bytes!(
            "../../../schemas/template-data-v1.schema.json"
        ))
        .expect("TemplateData schema JSON");
        let report_schema: Value = serde_json::from_slice(include_bytes!(
            "../../../schemas/template-report-v1.schema.json"
        ))
        .expect("TemplateReport schema JSON");
        assert_eq!(
            spec_schema["$id"],
            "https://hwp-cli.dev/schemas/template-spec-v1.schema.json"
        );
        assert_eq!(
            data_schema["$id"],
            "https://hwp-cli.dev/schemas/template-data-v1.schema.json"
        );
        assert_eq!(
            report_schema["$id"],
            "https://hwp-cli.dev/schemas/template-report-v1.schema.json"
        );
        assert_eq!(
            spec_schema["properties"]["variables"]["maxProperties"],
            MAX_VARIABLES
        );
        assert_eq!(
            spec_schema["$defs"]["stringVariable"]["properties"]["regex"]["maxLength"],
            MAX_REGEX_CHARS
        );
        assert_eq!(
            data_schema["properties"]["values"]["maxProperties"],
            MAX_VARIABLES
        );
        assert_eq!(
            report_schema["$defs"]["expansion"]["properties"]["expanded_bytes"]["maximum"],
            MAX_EXPANDED_BYTES
        );

        let property_names = report_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let expected = [
            "schema_version",
            "data_schema_version",
            "output",
            "dry_run",
            "deterministic",
            "mode",
            "template_sha256",
            "data_sha256",
            "reference_sha256",
            "output_sha256",
            "provided_variables",
            "defaulted_variables",
            "expansion",
            "changed_regions",
            "generated_regions",
            "unsupported",
            "fallback",
            "dropped",
            "template_validation",
            "data_validation",
            "semantic_validation",
            "package_validation",
            "compose",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(property_names, expected);

        for (bytes, expected) in [
            (
                include_bytes!("../../../schemas/template-spec-v1.schema.json").as_slice(),
                "590b9ac7dd2b30d1f8fafc4e087adf3117a831f9e38de39267a102141c549039",
            ),
            (
                include_bytes!("../../../schemas/template-data-v1.schema.json").as_slice(),
                "484bc86d01dcba17122507fad250791f88235be4dd933c12c721ef7b46eea298",
            ),
            (
                include_bytes!("../../../schemas/template-report-v1.schema.json").as_slice(),
                "aa2f011e02a52b29d07a458f84875e512cf1b1c80e6f2edea40ce756d436f705",
            ),
        ] {
            assert_eq!(sha256_hex(bytes), expected);
        }
    }
}
