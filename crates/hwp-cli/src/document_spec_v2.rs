//! DocumentSpec 2.0 rich-visual extension.
//!
//! Version 1 remains frozen. Version 2 wraps an unchanged v1 document and adds
//! target-aware visual objects. Every fallback is explicit, deterministic, and
//! recorded with stable capability reasons and distinct semantic/media hashes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::document_spec::{
    self, CompiledSpec, ComposeError, DocumentSpec, SpecInputFormat, SpecIssue,
};

const MAX_VISUALS: usize = 1_000;
const MAX_VISUAL_TEXT: usize = 32_768;
const MAX_ASSET_PATH_CHARS: usize = 4_096;
const MAX_ASSET_PATH_BYTES: usize = 4_096;
const MAX_VISUAL_DIMENSION_MM: f32 = 500.0;
const MAX_RASTER_DIMENSION: u32 = 4_096;
const MAX_RASTER_PIXELS: u64 = 16_777_216;
const MAX_TOTAL_RASTER_PIXELS: u64 = 67_108_864;
const MAX_SVG_RENDER_WORK: u64 = 200_000_000;
const MAX_TOTAL_SVG_RENDER_WORK: u64 = 400_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecVersionV2 {
    #[serde(rename = "2.0")]
    V2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSpecV2 {
    pub version: SpecVersionV2,
    /// An unchanged DocumentSpec 1.0 document.
    pub document: DocumentSpec,
    #[serde(default)]
    pub visuals: Vec<VisualSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualSpec {
    pub id: String,
    pub location: VisualLocation,
    #[serde(default)]
    pub policy: VisualPolicyByTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Required accessible alternative. The target document receives a
    /// human-readable description derived from title + alt; the pair remains
    /// separate only in the report and is not claimed to be reversible.
    pub alt: String,
    pub width_mm: f32,
    pub height_mm: f32,
    #[serde(default)]
    pub placement: VisualPlacement,
    pub content: VisualContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualLocation {
    pub section: usize,
    pub paragraph: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualPolicy {
    #[default]
    RequiredNative,
    PreferNative,
    ForceVisualFallback,
}

/// Per-target policy. An omitted map or omitted target fails closed to
/// `required_native`; choosing fallback is therefore always explicit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualPolicyByTarget {
    #[serde(default)]
    pub hwp: VisualPolicy,
    #[serde(default)]
    pub hwpx: VisualPolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualPlacement {
    #[default]
    Inline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum VisualContent {
    Image {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crop: Option<CropSpec>,
        #[serde(default)]
        rotation: Rotation,
    },
    Svg {
        path: PathBuf,
    },
    TextBox {
        text: String,
        #[serde(default = "default_fill")]
        fill: String,
        #[serde(default = "default_border")]
        border: String,
    },
}

fn default_fill() -> String {
    "#F2F4F7".to_string()
}

fn default_border() -> String {
    "#344054".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CropSpec {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    #[serde(rename = "0")]
    Deg0,
    #[serde(rename = "90")]
    Deg90,
    #[serde(rename = "180")]
    Deg180,
    #[serde(rename = "270")]
    Deg270,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualRepresentation {
    Native,
    VisualFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VisualDimensions {
    pub width_mm: f32,
    pub height_mm: f32,
    pub width_px: u32,
    pub height_px: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VisualReport {
    pub id: String,
    pub kind: String,
    pub requested_policy: VisualPolicy,
    pub resolved_representation: VisualRepresentation,
    pub target_format: String,
    pub capability_reason: String,
    pub semantic_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sanitized_svg_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub dimensions: VisualDimensions,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeReportV2 {
    pub schema_version: SpecVersionV2,
    pub base_schema_version: document_spec::SpecVersion,
    pub dry_run: bool,
    pub deterministic: bool,
    pub target_format: String,
    pub sections: usize,
    pub paragraphs: usize,
    pub tables: usize,
    pub images: usize,
    pub equations: usize,
    pub fields: usize,
    pub visuals: Vec<VisualReport>,
    pub warnings: Vec<String>,
}

pub struct CompiledSpecV2 {
    pub document: hwp_model::Document,
    pub report: ComposeReportV2,
}

pub fn parse_spec_v2(input: &str, format: SpecInputFormat) -> Result<DocumentSpecV2, ComposeError> {
    if input.len() > document_spec::MAX_SPEC_BYTES {
        return Err(validation_issue(
            "limit_exceeded",
            "$",
            format!(
                "spec is {} bytes; maximum is {}",
                input.len(),
                document_spec::MAX_SPEC_BYTES
            ),
        ));
    }
    match format {
        SpecInputFormat::Json => serde_json::from_str(input).map_err(|error| ComposeError::Parse {
            format,
            message: error.to_string(),
        }),
        SpecInputFormat::Yaml => serde_yaml::from_str(input).map_err(|error| ComposeError::Parse {
            format,
            message: error.to_string(),
        }),
    }
}

pub fn compile_spec_v2(
    spec: &DocumentSpecV2,
    base_dir: &Path,
    output: &Path,
    dry_run: bool,
) -> Result<CompiledSpecV2, ComposeError> {
    let target = TargetFormat::from_output(output)?;
    let CompiledSpec {
        mut document,
        report: base_report,
    } = document_spec::compile_spec(&spec.document, base_dir, output, dry_run, false)?;
    validate_v2(spec, base_dir, &document)?;

    let mut reports = Vec::with_capacity(spec.visuals.len());
    let mut assets = AssetStore::new(base_dir);
    for (index, visual) in spec.visuals.iter().enumerate() {
        let report = compile_visual(&mut document, visual, index, target, &mut assets)?;
        reports.push(report);
    }
    document.meta.source_format = "document-spec-v2".to_string();
    document.meta.source_version = "2.0".to_string();
    Ok(CompiledSpecV2 {
        document,
        report: ComposeReportV2 {
            schema_version: SpecVersionV2::V2,
            base_schema_version: document_spec::SpecVersion::V1,
            dry_run,
            deterministic: true,
            target_format: target.name().to_string(),
            sections: base_report.sections,
            paragraphs: base_report.paragraphs,
            tables: base_report.tables,
            images: base_report.images
                + reports
                    .iter()
                    .filter(|report| {
                        report.kind == "image"
                            || report.resolved_representation
                                == VisualRepresentation::VisualFallback
                    })
                    .count(),
            equations: base_report.equations,
            fields: base_report.fields,
            visuals: reports,
            warnings: Vec::new(),
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetFormat {
    Hwp,
    Hwpx,
}

impl TargetFormat {
    fn from_output(output: &Path) -> Result<Self, ComposeError> {
        match output
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("hwp") => Ok(Self::Hwp),
            Some("hwpx") => Ok(Self::Hwpx),
            _ => Err(validation_issue(
                "unsupported_output_format",
                "$.output",
                "DocumentSpec 2.0 output must be .hwp or .hwpx",
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Hwp => "hwp",
            Self::Hwpx => "hwpx",
        }
    }

    fn policy(self, policies: VisualPolicyByTarget) -> VisualPolicy {
        match self {
            Self::Hwp => policies.hwp,
            Self::Hwpx => policies.hwpx,
        }
    }
}

struct AssetStore<'a> {
    base_dir: &'a Path,
    by_path: BTreeMap<PathBuf, Rc<[u8]>>,
    unique_hashes: BTreeSet<[u8; 32]>,
    total_bytes: u64,
    svg_render_work: u64,
}

struct SvgAsset {
    source_sha256: String,
    sanitized_sha256: String,
    canonical: String,
}

impl<'a> AssetStore<'a> {
    fn new(base_dir: &'a Path) -> Self {
        Self {
            base_dir,
            by_path: BTreeMap::new(),
            unique_hashes: BTreeSet::new(),
            total_bytes: 0,
            svg_render_work: 0,
        }
    }

    fn read_asset(
        &mut self,
        asset: &Path,
        issue_path: &str,
        max_bytes: u64,
    ) -> Result<Rc<[u8]>, ComposeError> {
        if let Some(bytes) = self.by_path.get(asset) {
            if bytes.len() as u64 > max_bytes {
                return Err(compile_error(issue_path, "asset exceeds the byte budget"));
            }
            return Ok(Rc::clone(bytes));
        }
        let snapshot = crate::asset_snapshot::read_contained(self.base_dir, asset, max_bytes)
            .map_err(|error| {
                compile_error(
                    issue_path,
                    format!("asset_snapshot_{}: {error}", error.code.as_str()),
                )
            })?;
        if self.unique_hashes.insert(snapshot.sha256) {
            self.total_bytes = self.total_bytes.saturating_add(snapshot.data.len() as u64);
            if self.total_bytes > document_spec::MAX_TOTAL_ASSET_BYTES {
                return Err(compile_error(
                    issue_path,
                    "visual assets exceed the aggregate byte budget",
                ));
            }
        }
        let bytes = Rc::<[u8]>::from(snapshot.data);
        self.by_path.insert(asset.to_path_buf(), Rc::clone(&bytes));
        Ok(bytes)
    }

    fn read_valid_raster(
        &mut self,
        asset: &Path,
        issue_path: &str,
    ) -> Result<Rc<[u8]>, ComposeError> {
        let bytes = self.read_asset(asset, issue_path, document_spec::MAX_ASSET_BYTES)?;
        image_mime(&bytes)
            .map_err(|_| compile_error(issue_path, "unsupported raster image type"))?;
        let (width, height) = hwp_convert::image_pixel_size(&bytes)
            .filter(|(width, height)| *width > 0 && *height > 0)
            .ok_or_else(|| compile_error(issue_path, "raster header has no valid dimensions"))?;
        if width > MAX_RASTER_DIMENSION
            || height > MAX_RASTER_DIMENSION
            || u64::from(width) * u64::from(height) > MAX_RASTER_PIXELS
        {
            return Err(compile_error(
                issue_path,
                "raster dimensions exceed the pixel budget",
            ));
        }
        image::load_from_memory(&bytes)
            .map_err(|_| compile_error(issue_path, "raster image decode failed"))?;
        Ok(bytes)
    }

    fn read_sanitized_svg(
        &mut self,
        asset: &Path,
        issue_path: &str,
        output_pixels: u64,
    ) -> Result<SvgAsset, ComposeError> {
        let bytes = self.read_asset(asset, issue_path, hwp_convert::svg::MAX_SVG_BYTES as u64)?;
        let source_sha256 = sha256_hex(&bytes);
        let source = std::str::from_utf8(&bytes)
            .map_err(|_| compile_error(issue_path, "SVG must be UTF-8"))?;
        let sanitized = sanitize_svg_with_stats(source)?;
        if sanitized.canonical.len() > hwp_convert::svg::MAX_SVG_BYTES {
            return Err(compile_error(
                issue_path,
                "canonical SVG exceeds the byte budget",
            ));
        }
        let work = (sanitized.elements as u64)
            .saturating_add(sanitized.geometry_tokens as u64)
            .saturating_mul(output_pixels);
        if work > MAX_SVG_RENDER_WORK {
            return Err(compile_error(
                issue_path,
                "SVG complexity exceeds the per-item render-work budget",
            ));
        }
        self.svg_render_work = self.svg_render_work.saturating_add(work);
        if self.svg_render_work > MAX_TOTAL_SVG_RENDER_WORK {
            return Err(compile_error(
                issue_path,
                "SVG visuals exceed the aggregate render-work budget",
            ));
        }
        Ok(SvgAsset {
            source_sha256,
            sanitized_sha256: sha256_hex(sanitized.canonical.as_bytes()),
            canonical: sanitized.canonical,
        })
    }
}

fn validate_v2(
    spec: &DocumentSpecV2,
    base_dir: &Path,
    document: &hwp_model::Document,
) -> Result<(), ComposeError> {
    let mut issues = Vec::new();
    if spec.visuals.len() > MAX_VISUALS {
        issues.push(issue(
            "limit_exceeded",
            "$.visuals",
            format!("at most {MAX_VISUALS} visuals are allowed"),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut total_raster_pixels = 0_u64;
    for (index, visual) in spec.visuals.iter().enumerate() {
        let path = format!("$.visuals[{index}]");
        if !valid_id(&visual.id) {
            issues.push(issue(
                "invalid_id",
                format!("{path}.id"),
                "id must match [A-Za-z][A-Za-z0-9_-]{0,63}",
            ));
        } else if !ids.insert(&visual.id) {
            issues.push(issue(
                "duplicate_id",
                format!("{path}.id"),
                "visual id must be unique",
            ));
        }
        if visual.alt.trim().is_empty() || visual.alt.chars().count() > MAX_VISUAL_TEXT {
            issues.push(issue(
                "invalid_accessibility",
                format!("{path}.alt"),
                format!("alt is required and cannot exceed {MAX_VISUAL_TEXT} Unicode scalars"),
            ));
        }
        let description = target_description(visual.title.as_deref(), &visual.alt);
        if description.encode_utf16().count() > u16::MAX as usize
            || description.chars().any(is_invalid_authoring_character)
        {
            issues.push(issue(
                "invalid_accessibility",
                format!("{path}.alt"),
                "derived description must be valid XML text and fit 65535 UTF-16 code units",
            ));
        }
        if let Some(title) = &visual.title {
            if title.chars().count() > MAX_VISUAL_TEXT {
                issues.push(issue(
                    "limit_exceeded",
                    format!("{path}.title"),
                    format!("title cannot exceed {MAX_VISUAL_TEXT} Unicode scalars"),
                ));
            }
            if title.chars().any(is_invalid_authoring_character) {
                issues.push(issue(
                    "invalid_accessibility",
                    format!("{path}.title"),
                    "title must be valid XML text without CR",
                ));
            }
        }
        for (name, value) in [
            ("width_mm", visual.width_mm),
            ("height_mm", visual.height_mm),
        ] {
            if !value.is_finite() || !(0.1..=MAX_VISUAL_DIMENSION_MM).contains(&value) {
                issues.push(issue(
                    "invalid_dimension",
                    format!("{path}.{name}"),
                    format!(
                        "dimension must be finite and within 0.1..={MAX_VISUAL_DIMENSION_MM} mm"
                    ),
                ));
            }
        }
        if visual.width_mm.is_finite()
            && visual.height_mm.is_finite()
            && visual.width_mm > 0.0
            && visual.height_mm > 0.0
        {
            let width = (visual.width_mm * 96.0 / 25.4).round().max(1.0) as u64;
            let height = (visual.height_mm * 96.0 / 25.4).round().max(1.0) as u64;
            total_raster_pixels = total_raster_pixels.saturating_add(width.saturating_mul(height));
        }
        match document.sections.get(visual.location.section) {
            None => issues.push(issue(
                "invalid_location",
                format!("{path}.location.section"),
                "section index is outside the compiled document",
            )),
            Some(section) if visual.location.paragraph >= section.paragraphs.len() => {
                issues.push(issue(
                    "invalid_location",
                    format!("{path}.location.paragraph"),
                    "paragraph index is outside the compiled section",
                ));
            }
            _ => {}
        }
        validate_content(&visual.content, base_dir, &path, &mut issues);
    }
    if total_raster_pixels > MAX_TOTAL_RASTER_PIXELS {
        issues.push(issue(
            "limit_exceeded",
            "$.visuals",
            format!(
                "visual raster budget is {total_raster_pixels} pixels; maximum is {MAX_TOTAL_RASTER_PIXELS}"
            ),
        ));
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ComposeError::Validation { issues })
    }
}

fn validate_content(
    content: &VisualContent,
    _base_dir: &Path,
    path: &str,
    issues: &mut Vec<SpecIssue>,
) {
    match content {
        VisualContent::Svg { path: asset } => {
            if !valid_asset_path(asset) {
                issues.push(issue(
                    "invalid_asset_path",
                    format!("{path}.path"),
                    "asset path must contain only relative normal components",
                ));
            }
        }
        VisualContent::Image {
            path: asset, crop, ..
        } => {
            if !valid_asset_path(asset) {
                issues.push(issue(
                    "invalid_asset_path",
                    format!("{path}.path"),
                    "asset path must contain only relative normal components",
                ));
            }
            if let Some(crop) = crop
                && (!crop.x.is_finite()
                    || !crop.y.is_finite()
                    || !crop.width.is_finite()
                    || !crop.height.is_finite()
                    || crop.x < 0.0
                    || crop.y < 0.0
                    || crop.width <= 0.0
                    || crop.height <= 0.0
                    || crop.x + crop.width > 1.0
                    || crop.y + crop.height > 1.0)
            {
                issues.push(issue(
                    "invalid_crop",
                    format!("{path}.crop"),
                    "crop uses normalized coordinates and must fit within 0..=1",
                ));
            }
        }
        VisualContent::TextBox { text, fill, border } => {
            if text.trim().is_empty()
                || text.chars().count() > MAX_VISUAL_TEXT
                || text.encode_utf16().count() > u16::MAX as usize
                || text.chars().any(is_invalid_authoring_character)
            {
                issues.push(issue(
                    "invalid_text",
                    format!("{path}.text"),
                    format!(
                        "text is required, must be XML-safe without CR, and cannot exceed {MAX_VISUAL_TEXT} Unicode scalars or 65535 UTF-16 code units"
                    ),
                ));
            }
            validate_colors(fill, border, path, issues);
        }
    }
}

fn valid_asset_path(path: &Path) -> bool {
    path.to_str().is_some_and(|value| {
        value.chars().count() <= MAX_ASSET_PATH_CHARS
            && value.len() <= MAX_ASSET_PATH_BYTES
            && crate::asset_snapshot::validate_relative_path(path).is_ok()
    })
}

fn validate_colors(fill: &str, border: &str, path: &str, issues: &mut Vec<SpecIssue>) {
    for (name, value) in [("fill", fill), ("border", border)] {
        if parse_color(value).is_none() {
            issues.push(issue(
                "invalid_color",
                format!("{path}.{name}"),
                "color must be #RRGGBB",
            ));
        }
    }
}

fn compile_visual(
    document: &mut hwp_model::Document,
    visual: &VisualSpec,
    index: usize,
    target: TargetFormat,
    assets: &mut AssetStore<'_>,
) -> Result<VisualReport, ComposeError> {
    let path = format!("$.visuals[{index}]");
    let dimensions = dimensions(visual.width_mm, visual.height_mm)?;
    let description = target_description(visual.title.as_deref(), &visual.alt);
    let requested_policy = target.policy(visual.policy);
    let (native, native_reason) = native_capability(&visual.content, target);
    let (fallback, fallback_reason) = fallback_capability(&visual.content);
    let (representation, capability_reason) = match requested_policy {
        VisualPolicy::RequiredNative if native => {
            (VisualRepresentation::Native, native_reason.to_string())
        }
        VisualPolicy::RequiredNative => {
            return Err(compile_error(
                &path,
                format!("native_unavailable: {native_reason}"),
            ));
        }
        VisualPolicy::PreferNative if native => {
            (VisualRepresentation::Native, native_reason.to_string())
        }
        VisualPolicy::PreferNative if fallback => (
            VisualRepresentation::VisualFallback,
            format!("{native_reason}; {fallback_reason}"),
        ),
        VisualPolicy::PreferNative => {
            return Err(compile_error(
                &path,
                format!("visual_fallback_unavailable: {native_reason}; {fallback_reason}"),
            ));
        }
        VisualPolicy::ForceVisualFallback if fallback => (
            VisualRepresentation::VisualFallback,
            format!("forced_by_policy; {fallback_reason}"),
        ),
        VisualPolicy::ForceVisualFallback => {
            return Err(compile_error(
                &path,
                format!("visual_fallback_unavailable: {fallback_reason}"),
            ));
        }
    };

    let mut media_sha256 = None;
    let mut media_type = None;
    let raster = match &visual.content {
        VisualContent::Image { path: asset, .. } => {
            Some(assets.read_valid_raster(asset, &format!("{path}.path"))?)
        }
        _ => None,
    };
    let svg = match &visual.content {
        VisualContent::Svg { path: asset } => Some(assets.read_sanitized_svg(
            asset,
            &format!("{path}.path"),
            u64::from(dimensions.width_px) * u64::from(dimensions.height_px),
        )?),
        _ => None,
    };
    let source_sha256 = raster
        .as_deref()
        .map(sha256_hex)
        .or_else(|| svg.as_ref().map(|asset| asset.source_sha256.clone()));
    let sanitized_svg_sha256 = svg.as_ref().map(|asset| asset.sanitized_sha256.clone());
    let semantic_sha256 = semantic_sha256(visual, source_sha256.as_deref())?;

    if representation == VisualRepresentation::Native {
        match &visual.content {
            VisualContent::Image { .. } => {
                let bytes = raster.expect("image asset loaded");
                media_sha256 = Some(sha256_hex(&bytes));
                media_type = Some(image_mime(&bytes)?.to_string());
                attach_picture(
                    document,
                    visual,
                    bytes.as_ref().to_vec(),
                    &description,
                    dimensions,
                    index,
                )?;
            }
            VisualContent::TextBox { text, fill, border } => {
                attach_text_box(document, visual, text, fill, border, &description)?
            }
            VisualContent::Svg { .. } => unreachable!(),
        }
    } else {
        let media = match &visual.content {
            VisualContent::Svg { .. } => hwp_convert::svg::rasterize_svg_png(
                &svg.expect("SVG asset loaded").canonical,
                dimensions.width_px,
                dimensions.height_px,
                document_spec::MAX_ASSET_BYTES,
            )
            .map_err(|e| compile_error(&path, e))?,
            _ => fallback_png(visual, raster.as_deref(), dimensions, &path)?,
        };
        media_sha256 = Some(sha256_hex(&media));
        media_type = Some(image_mime(&media)?.to_string());
        attach_picture(document, visual, media, &description, dimensions, index)?;
    }

    Ok(VisualReport {
        id: visual.id.clone(),
        kind: visual_kind(&visual.content).to_string(),
        requested_policy,
        resolved_representation: representation,
        target_format: target.name().to_string(),
        capability_reason,
        semantic_sha256,
        source_sha256,
        sanitized_svg_sha256,
        media_sha256,
        media_type,
        dimensions,
    })
}

fn native_capability(content: &VisualContent, target: TargetFormat) -> (bool, &'static str) {
    match content {
        VisualContent::Image { crop, rotation, .. } => {
            if crop.is_none() && *rotation == Rotation::Deg0 {
                (true, "exact_raster_embedding_available")
            } else {
                (false, "crop_or_rotation_requires_raster_fallback")
            }
        }
        VisualContent::TextBox { .. } if target == TargetFormat::Hwpx => {
            (true, "native_hwpx_rectangle_text_box_available")
        }
        VisualContent::TextBox { .. } => (false, "native_text_box_unavailable_for_hwp"),
        VisualContent::Svg { .. } => (false, "svg_requires_deterministic_png_fallback"),
    }
}

fn fallback_capability(content: &VisualContent) -> (bool, &'static str) {
    match content {
        VisualContent::Image { .. } => (true, "deterministic_raster_fallback_available"),
        VisualContent::Svg { .. } => (true, "sanitized_svg_to_png_fallback_available"),
        VisualContent::TextBox { .. } => (
            false,
            "deterministic_font_renderer_unavailable_for_semantic_labels",
        ),
    }
}

fn visual_kind(content: &VisualContent) -> &'static str {
    match content {
        VisualContent::Image { .. } => "image",
        VisualContent::Svg { .. } => "svg",
        VisualContent::TextBox { .. } => "text_box",
    }
}

fn dimensions(width_mm: f32, height_mm: f32) -> Result<VisualDimensions, ComposeError> {
    let width_px = (width_mm * 96.0 / 25.4).round().max(1.0) as u32;
    let height_px = (height_mm * 96.0 / 25.4).round().max(1.0) as u32;
    if width_px > MAX_RASTER_DIMENSION
        || height_px > MAX_RASTER_DIMENSION
        || u64::from(width_px) * u64::from(height_px) > MAX_RASTER_PIXELS
    {
        return Err(validation_issue(
            "limit_exceeded",
            "$.visuals",
            "raster dimensions exceed the deterministic renderer budget",
        ));
    }
    Ok(VisualDimensions {
        width_mm,
        height_mm,
        width_px,
        height_px,
    })
}

fn semantic_sha256(
    visual: &VisualSpec,
    source_sha256: Option<&str>,
) -> Result<String, ComposeError> {
    let canonical = serde_json::to_vec(&(visual, source_sha256))
        .map_err(|_| compile_error("$.visuals", "visual semantics could not be encoded"))?;
    Ok(sha256_hex(&canonical))
}

/// Validates a bounded, closed SVG subset. External references, scripts,
/// event handlers, CSS URLs, DTDs, processing instructions, and text nodes are
/// rejected. The function never dereferences a network or filesystem resource.
/// 구현의 단일 원천은 `hwp_convert::svg`다 (from_html과 공용).
pub fn sanitize_svg(input: &str) -> Result<String, ComposeError> {
    hwp_convert::svg::sanitize_svg(input).map_err(|e| compile_error("$.visuals", e))
}

fn sanitize_svg_with_stats(input: &str) -> Result<hwp_convert::svg::SanitizedSvg, ComposeError> {
    hwp_convert::svg::sanitize_svg_with_stats(input).map_err(|e| compile_error("$.visuals", e))
}

fn fallback_png(
    visual: &VisualSpec,
    raster: Option<&[u8]>,
    dimensions: VisualDimensions,
    path: &str,
) -> Result<Vec<u8>, ComposeError> {
    match &visual.content {
        VisualContent::Image {
            path: asset,
            crop,
            rotation,
        } => {
            let _ = asset;
            transform_image(
                raster.ok_or_else(|| compile_error(path, "raster snapshot is unavailable"))?,
                *crop,
                *rotation,
                dimensions,
                path,
            )
        }
        VisualContent::Svg { .. } => Err(compile_error(
            path,
            "sanitized SVG must use the dedicated rasterizer",
        )),
        VisualContent::TextBox { .. } => Err(compile_error(
            path,
            "text box fallback is unavailable without deterministic font rendering",
        )),
    }
}

fn transform_image(
    bytes: &[u8],
    crop: Option<CropSpec>,
    rotation: Rotation,
    dimensions: VisualDimensions,
    path: &str,
) -> Result<Vec<u8>, ComposeError> {
    let mut image = image::load_from_memory(bytes)
        .map_err(|error| compile_error(path, format!("image decode failed: {error}")))?;
    if u64::from(image.width()) * u64::from(image.height()) > MAX_RASTER_PIXELS {
        return Err(compile_error(
            path,
            "decoded image exceeds the pixel budget",
        ));
    }
    if let Some(crop) = crop {
        let x = (crop.x * image.width() as f32).floor() as u32;
        let y = (crop.y * image.height() as f32).floor() as u32;
        let width = (crop.width * image.width() as f32).round().max(1.0) as u32;
        let height = (crop.height * image.height() as f32).round().max(1.0) as u32;
        image = image.crop_imm(
            x.min(image.width() - 1),
            y.min(image.height() - 1),
            width.min(image.width() - x.min(image.width() - 1)),
            height.min(image.height() - y.min(image.height() - 1)),
        );
    }
    image = match rotation {
        Rotation::Deg0 => image,
        Rotation::Deg90 => image.rotate90(),
        Rotation::Deg180 => image.rotate180(),
        Rotation::Deg270 => image.rotate270(),
    };
    let image = image.resize_exact(
        dimensions.width_px,
        dimensions.height_px,
        image::imageops::FilterType::Triangle,
    );
    hwp_convert::svg::encode_png(image.to_rgba8()).map_err(|e| compile_error("$.visuals", e))
}

fn attach_picture(
    document: &mut hwp_model::Document,
    visual: &VisualSpec,
    bytes: Vec<u8>,
    description: &str,
    dimensions: VisualDimensions,
    index: usize,
) -> Result<(), ComposeError> {
    let hash = sha256_hex(&bytes);
    let extension = image_extension(&bytes)?;
    let name = format!("v2-{:04}-{}.{}", index + 1, &hash[..16], extension);
    if !document
        .bin_streams
        .iter()
        .any(|stream| stream.name == name)
    {
        document.bin_streams.push(hwp_model::BinStream {
            name: name.clone(),
            data: bytes,
        });
    }
    let picture = hwp_model::Picture {
        common_data: Vec::new(),
        width: hwp_model::HwpUnit(mm_to_hwp(dimensions.width_mm)),
        height: hwp_model::HwpUnit(mm_to_hwp(dimensions.height_mm)),
        treat_as_char: visual.placement == VisualPlacement::Inline,
        // Inline objects are ordered by their paragraph control position. HWPX
        // materializes inline `zOrder=0`, so canonical authoring must do the
        // same rather than inventing an unstable stacking order.
        z_order: 0,
        vert_offset: 0,
        horz_offset: 0,
        description: Some(description.to_string()),
        bin_ref: hwp_model::BinRef::ItemRef(name),
        extras: Vec::new(),
    };
    attach_control(
        document,
        visual.location,
        hwp_model::Control::Picture(picture),
    )
}

fn attach_text_box(
    document: &mut hwp_model::Document,
    visual: &VisualSpec,
    text: &str,
    fill: &str,
    border: &str,
    description: &str,
) -> Result<(), ComposeError> {
    let width = mm_to_hwp(visual.width_mm);
    let height = mm_to_hwp(visual.height_mm);
    let shape = hwp_model::ShapeGeom {
        kind: hwp_model::ShapeKind::Rect,
        x: 0,
        y: 0,
        w: width,
        h: height,
        points: Vec::new(),
        fill: color_to_bgr(parse_color(fill).expect("validated")),
        fill_gradient: None,
        border_color: color_to_bgr(parse_color(border).expect("validated")),
        border_width: 20,
        round_ratio: 0,
        border_style: 0,
        arrow_start: 0,
        arrow_end: 0,
        anchored: visual.placement == VisualPlacement::Inline,
        description: Some(description.to_string()),
    };
    let paragraph_lists = vec![hwp_model::ParagraphList {
        header_data: Vec::new(),
        paragraphs: text
            .split('\n')
            .map(|line| hwp_model::Paragraph {
                chars: line.chars().map(hwp_model::HwpChar::Text).collect(),
                char_shape_runs: vec![(0, hwp_model::CharShapeId(0))],
                ..hwp_model::Paragraph::default()
            })
            .collect(),
    }];
    attach_control(
        document,
        visual.location,
        hwp_model::Control::Generic(hwp_model::GenericControl {
            ctrl_id: *b"rect",
            data: Vec::new(),
            paragraph_lists,
            extras: Vec::new(),
            raw_children: Vec::new(),
            gso_shapes: vec![shape],
            equation: None,
            column_def: None,
        }),
    )
}

fn attach_control(
    document: &mut hwp_model::Document,
    location: VisualLocation,
    control: hwp_model::Control,
) -> Result<(), ComposeError> {
    let paragraph = document
        .sections
        .get_mut(location.section)
        .and_then(|section| section.paragraphs.get_mut(location.paragraph))
        .ok_or_else(|| compile_error("$.visuals.location", "compiled location disappeared"))?;
    let ctrl_id = control.ctrl_id();
    let index = paragraph.controls.len() as u32;
    paragraph.chars.push(hwp_model::HwpChar::ExtCtrl {
        code: hwp_model::ctrl_char::OBJECT,
        ctrl_id,
        payload: hwp_convert::field::rev_payload(&ctrl_id),
        ctrl_index: Some(index),
    });
    paragraph.controls.push(control);
    paragraph.header.ctrl_mask = 0;
    paragraph.line_segs.clear();
    Ok(())
}

fn image_mime(bytes: &[u8]) -> Result<&'static str, ComposeError> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Ok("image/png"),
        [0xFF, 0xD8, ..] => Ok("image/jpeg"),
        [b'G', b'I', b'F', ..] => Ok("image/gif"),
        [b'B', b'M', ..] => Ok("image/bmp"),
        _ => Err(compile_error(
            "$.visuals",
            "only PNG, JPEG, GIF, and BMP raster images are allowed",
        )),
    }
}

fn image_extension(bytes: &[u8]) -> Result<&'static str, ComposeError> {
    Ok(match image_mime(bytes)? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => unreachable!(),
    })
}

fn parse_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

fn color_to_bgr([r, g, b]: [u8; 3]) -> u32 {
    u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16)
}

fn mm_to_hwp(value: f32) -> i32 {
    (value * 7200.0 / 25.4).round().max(1.0) as i32
}

fn target_description(title: Option<&str>, alt: &str) -> String {
    match title.map(str::trim).filter(|title| !title.is_empty()) {
        Some(title) if title != alt.trim() => format!("{title}\n\n{}", alt.trim()),
        _ => alt.trim().to_string(),
    }
}

fn is_invalid_authoring_character(character: char) -> bool {
    character == '\r'
        || !matches!(
            character as u32,
            0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
        )
}

fn valid_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphabetic())
        && value.len() <= 64
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn issue(code: &str, path: impl Into<String>, message: impl Into<String>) -> SpecIssue {
    SpecIssue {
        code: code.to_string(),
        path: path.into(),
        message: message.into(),
    }
}

fn validation_issue(
    code: &str,
    path: impl Into<String>,
    message: impl Into<String>,
) -> ComposeError {
    ComposeError::Validation {
        issues: vec![issue(code, path, message)],
    }
}

fn compile_error(path: impl Into<String>, message: impl Into<String>) -> ComposeError {
    ComposeError::Compile {
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_svg_is_rejected_fail_closed() {
        for hostile in [
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="file:///etc/passwd"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect onload="alert(1)"/></svg>"#,
            r#"<!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"><text>font required</text></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:evil="urn:evil"><evil:rect/></svg>"#,
            r#"<evil:svg xmlns:evil="http://www.w3.org/2000/svg"><evil:rect/></evil:svg>"#,
        ] {
            assert!(
                sanitize_svg(hostile).is_err(),
                "accepted hostile SVG: {hostile}"
            );
        }
    }

    #[test]
    fn svg_numeric_and_path_surface_is_bounded_and_canonical() {
        for rejected in [
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 NaN 10"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000001 10"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path d="M0 0L1 1"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 0 10"/>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="-1" height="2"/></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><g transform="scale(1000000)"><rect width="1" height="1"/></g></svg>"#,
        ] {
            assert!(sanitize_svg(rejected).is_err(), "accepted {rejected}");
        }

        let input = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,10,10"><rect y="0" x="0" width="10.0" height="10" fill="#abcdef"/></svg>"##;
        let canonical = sanitize_svg(input).unwrap();
        assert_eq!(
            canonical,
            r##"<svg viewBox="0 0 10 10" xmlns="http://www.w3.org/2000/svg"><rect fill="#ABCDEF" height="10" width="10" x="0" y="0"/></svg>"##
        );
        assert_eq!(sanitize_svg(&canonical).unwrap(), canonical);
    }

    #[test]
    fn sanitized_svg_raster_is_deterministic_and_nonempty() {
        let canonical = sanitize_svg(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4" fill="#2563eb"/></svg>"##,
        )
        .unwrap();
        let dimensions = VisualDimensions {
            width_mm: 20.0,
            height_mm: 20.0,
            width_px: 96,
            height_px: 96,
        };
        let raster = |svg: &str| {
            hwp_convert::svg::rasterize_svg_png(
                svg,
                dimensions.width_px,
                dimensions.height_px,
                1 << 20,
            )
            .unwrap()
        };
        let first = raster(&canonical);
        let second = raster(&canonical);
        assert_eq!(first, second);
        assert_eq!(sha256_hex(&first), sha256_hex(&second));
        let decoded = image::load_from_memory(&first).unwrap().to_rgba8();
        assert!(decoded.pixels().any(|pixel| pixel[3] != 0));

        let empty = sanitize_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="none"/></svg>"#,
        )
        .unwrap();
        assert!(
            hwp_convert::svg::rasterize_svg_png(
                &empty,
                dimensions.width_px,
                dimensions.height_px,
                1 << 20
            )
            .is_err()
        );
    }

    #[test]
    fn svg_fallback_writes_png_and_reopens_for_both_targets() {
        let root = std::env::temp_dir().join(format!(
            "hwp-v2-svg-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,10,10"><rect y="1" x="1" width="8" height="8" fill="#2563eb"/></svg>"##;
        std::fs::write(root.join("visual.svg"), source).unwrap();
        let input = r#"{
          "version":"2.0",
          "document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"anchor"}]}]}]},
          "visuals":[{"id":"vector","location":{"section":0,"paragraph":0},"policy":{"hwp":"force_visual_fallback","hwpx":"force_visual_fallback"},"alt":"blue square","width_mm":30,"height_mm":30,"content":{"type":"svg","path":"visual.svg"}}]
        }"#;
        let spec = parse_spec_v2(input, SpecInputFormat::Json).unwrap();

        for extension in ["hwpx", "hwp"] {
            let output = root.join(format!("vector.{extension}"));
            let compiled = compile_spec_v2(&spec, &root, &output, false).unwrap();
            let visual = &compiled.report.visuals[0];
            assert_eq!(
                visual.resolved_representation,
                VisualRepresentation::VisualFallback
            );
            assert!(visual.source_sha256.is_some());
            assert!(visual.sanitized_svg_sha256.is_some());
            assert!(visual.media_sha256.is_some());
            assert_eq!(visual.media_type.as_deref(), Some("image/png"));
            assert_ne!(visual.source_sha256, visual.sanitized_svg_sha256);
            assert_ne!(visual.sanitized_svg_sha256, visual.media_sha256);
            if extension == "hwpx" {
                hwpx::write_document(&compiled.document, &output).unwrap();
            } else {
                hwp5::write_document(&compiled.document, &output, &hwp5::WriteOptions::default())
                    .unwrap();
            }
            let reopened = if extension == "hwpx" {
                hwpx::read_document(&output).unwrap().document
            } else {
                hwp5::read_document(&output).unwrap().document
            };
            assert!(
                reopened
                    .bin_streams
                    .iter()
                    .any(|stream| stream.data.starts_with(&[0x89, b'P', b'N', b'G']))
            );
            assert!(
                reopened
                    .bin_streams
                    .iter()
                    .all(|stream| !stream.data.starts_with(b"<svg"))
            );
            let rendered = hwp_render::render_document(
                &reopened,
                &hwp_render::RenderOptions {
                    dpi: 72.0,
                    font_dirs: Vec::new(),
                },
            )
            .unwrap();
            assert!(rendered.pages.iter().any(|page| {
                page.pixels().iter().any(|pixel| {
                    pixel.blue() > pixel.red().saturating_add(20)
                        && pixel.blue() > pixel.green().saturating_add(10)
                })
            }));
            let pdf = hwp_render::render_document_pdf(
                &reopened,
                &hwp_render::RenderOptions::default(),
                None,
            )
            .unwrap();
            assert!(pdf.data.starts_with(b"%PDF-") && pdf.data.len() > 1_000);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn target_policy_is_distinct_and_fails_closed_by_default() {
        let default = VisualPolicyByTarget::default();
        assert_eq!(
            TargetFormat::Hwp.policy(default),
            VisualPolicy::RequiredNative
        );
        assert_eq!(
            TargetFormat::Hwpx.policy(default),
            VisualPolicy::RequiredNative
        );
        let split = VisualPolicyByTarget {
            hwp: VisualPolicy::PreferNative,
            hwpx: VisualPolicy::ForceVisualFallback,
        };
        assert_eq!(TargetFormat::Hwp.policy(split), VisualPolicy::PreferNative);
        assert_eq!(
            TargetFormat::Hwpx.policy(split),
            VisualPolicy::ForceVisualFallback
        );
    }

    #[test]
    fn multiline_native_textboxes_reopen_and_report_is_redacted() {
        let input = r##"{
          "version":"2.0",
          "document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"anchor"}]}]}]},
          "visuals":[
            {"id":"box1","location":{"section":0,"paragraph":0},"policy":{"hwpx":"required_native"},"title":"SECRET TITLE","alt":"SECRET ALT","width_mm":40,"height_mm":20,"content":{"type":"text_box","text":"first\nsecond","fill":"#FFFFFF","border":"#000000"}},
            {"id":"box2","location":{"section":0,"paragraph":0},"policy":{"hwpx":"required_native"},"alt":"second box","width_mm":30,"height_mm":15,"content":{"type":"text_box","text":"third","fill":"#FFFFFF","border":"#000000"}}
          ]
        }"##;
        let spec = parse_spec_v2(input, SpecInputFormat::Json).unwrap();
        let root = std::env::temp_dir().join(format!(
            "hwp-v2-textbox-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("test")
                .replace(':', "-")
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("boxes.hwpx");
        let compiled = compile_spec_v2(&spec, &root, &output, false).unwrap();
        let report = serde_json::to_string(&compiled.report).unwrap();
        assert!(!report.contains("SECRET TITLE"));
        assert!(!report.contains("SECRET ALT"));
        assert_eq!(compiled.report.target_format, "hwpx");
        assert_eq!(
            compiled.report.visuals[0].resolved_representation,
            VisualRepresentation::Native
        );
        hwpx::write_document(&compiled.document, &output).unwrap();
        let reopened = hwpx::read_document(&output).unwrap().document;
        let shapes = reopened.sections[0].paragraphs[0]
            .controls
            .iter()
            .filter_map(|control| match control {
                hwp_model::Control::Generic(generic) if generic.ctrl_id == *b"rect" => {
                    Some(generic)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].paragraph_lists.len(), 1);
        assert_eq!(shapes[0].paragraph_lists[0].paragraphs.len(), 2);
        assert_eq!(
            shapes[0].gso_shapes[0].description.as_deref(),
            Some("SECRET TITLE\n\nSECRET ALT")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn carriage_return_is_rejected_before_native_xml_write() {
        let input = r#"{
          "version":"2.0",
          "document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"anchor"}]}]}]},
          "visuals":[{"id":"box","location":{"section":0,"paragraph":0},"policy":{"hwpx":"required_native"},"title":"\r","alt":"safe alt","width_mm":40,"height_mm":20,"content":{"type":"text_box","text":"bad\rtext"}}]
        }"#;
        let spec = parse_spec_v2(input, SpecInputFormat::Json).unwrap();
        let error = match compile_spec_v2(&spec, Path::new("."), Path::new("out.hwpx"), true) {
            Ok(_) => panic!("CR must be rejected"),
            Err(error) => error,
        };
        let ComposeError::Validation { issues } = error else {
            panic!("expected validation error")
        };
        assert!(
            issues.iter().any(
                |issue| issue.code == "invalid_accessibility" && issue.path.ends_with(".title")
            )
        );
        assert!(issues.iter().any(|issue| issue.code == "invalid_text"));
    }

    #[test]
    fn schema_field_bounds_are_followed_by_runtime_path_and_crop_semantic_gates() {
        let overlong = "가".repeat(MAX_ASSET_PATH_BYTES / "가".len() + 1);
        assert!(overlong.chars().count() <= MAX_ASSET_PATH_CHARS);
        assert!(overlong.len() > MAX_ASSET_PATH_BYTES);
        let path_input = format!(
            r#"{{
              "version":"2.0",
              "document":{{"version":"1.0","sections":[{{"blocks":[{{"type":"paragraph","runs":[{{"type":"text","text":"anchor"}}]}}]}}]}},
              "visuals":[{{"id":"image","location":{{"section":0,"paragraph":0}},"alt":"image","width_mm":20,"height_mm":20,"content":{{"type":"image","path":"{overlong}"}}}}]
            }}"#
        );
        let spec = parse_spec_v2(&path_input, SpecInputFormat::Json).unwrap();
        let error = match compile_spec_v2(&spec, Path::new("."), Path::new("out.hwpx"), true) {
            Ok(_) => panic!("over-byte-limit path must fail"),
            Err(error) => error,
        };
        let ComposeError::Validation { issues } = error else {
            panic!("expected validation error")
        };
        assert!(issues.iter().any(|issue| {
            issue.code == "invalid_asset_path" && issue.path == "$.visuals[0].path"
        }));

        // This object satisfies the schema's independent 0..1 field bounds.
        // Cross-property arithmetic is intentionally a typed runtime gate.
        let crop_input = r#"{
          "version":"2.0",
          "document":{"version":"1.0","sections":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"anchor"}]}]}]},
          "visuals":[{"id":"crop","location":{"section":0,"paragraph":0},"alt":"crop","width_mm":20,"height_mm":20,"content":{"type":"image","path":"missing.png","crop":{"x":0.8,"y":0.0,"width":0.3,"height":1.0}}}]
        }"#;
        let spec = parse_spec_v2(crop_input, SpecInputFormat::Json).unwrap();
        let error = match compile_spec_v2(&spec, Path::new("."), Path::new("out.hwpx"), true) {
            Ok(_) => panic!("crop sum must fail at runtime"),
            Err(error) => error,
        };
        let ComposeError::Validation { issues } = error else {
            panic!("expected validation error")
        };
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "invalid_crop" && issue.path == "$.visuals[0].crop")
        );
    }

    #[test]
    fn checked_in_v2_contract_example_and_schema_hashes_are_frozen() {
        let example = include_str!("../../../examples/document-spec-v2/basic.json");
        let spec = parse_spec_v2(example, SpecInputFormat::Json).expect("v2 example parses");
        let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/document-spec-v2");
        for extension in ["hwp", "hwpx"] {
            let output = PathBuf::from(format!("contract.{extension}"));
            let compiled = compile_spec_v2(&spec, &base_dir, &output, true)
                .expect("v2 example compiles for both targets");
            assert_eq!(compiled.report.target_format, extension);
            assert_eq!(compiled.report.visuals.len(), 1);
            assert_eq!(
                compiled.report.visuals[0].resolved_representation,
                VisualRepresentation::VisualFallback
            );
            let report = serde_json::to_string(&compiled.report).unwrap();
            for redacted in [
                "visual.svg",
                "Blue square",
                "A blue square inside a transparent margin.",
            ] {
                assert!(!report.contains(redacted), "report leaked {redacted:?}");
            }
        }

        let text_box = parse_spec_v2(
            include_str!("../../../examples/document-spec-v2/native-text-box.json"),
            SpecInputFormat::Json,
        )
        .expect("native text-box example parses");
        let compiled = compile_spec_v2(
            &text_box,
            &base_dir,
            Path::new("native-text-box.hwpx"),
            true,
        )
        .expect("native text-box example compiles for HWPX");
        assert_eq!(
            compiled.report.visuals[0].resolved_representation,
            VisualRepresentation::Native
        );
        assert!(compiled.report.visuals[0].media_sha256.is_none());

        let spec_schema: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../schemas/document-spec-v2.schema.json"
        ))
        .expect("DocumentSpec v2 schema JSON");
        let report_schema: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../schemas/document-report-v2.schema.json"
        ))
        .expect("DocumentReport v2 schema JSON");
        assert_eq!(
            spec_schema["$id"],
            "https://hwp-cli.dev/schemas/document-spec-v2.schema.json"
        );
        assert_eq!(
            report_schema["$id"],
            "https://hwp-cli.dev/schemas/document-report-v2.schema.json"
        );
        assert_eq!(
            spec_schema["properties"]["visuals"]["maxItems"],
            MAX_VISUALS
        );
        assert_eq!(
            spec_schema["$defs"]["visual"]["properties"]["content"]["oneOf"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        for raw_field in ["path", "title", "alt", "description", "output"] {
            assert!(
                report_schema["properties"].get(raw_field).is_none(),
                "report root exposes {raw_field}"
            );
            assert!(
                report_schema["$defs"]["visualReport"]["properties"]
                    .get(raw_field)
                    .is_none(),
                "visual report exposes {raw_field}"
            );
        }

        for (bytes, expected) in [
            (
                include_bytes!("../../../schemas/document-spec-v1.schema.json").as_slice(),
                "1607cb19c9068306da8c76ba6ebee4ae8e5c6d650490fc0737dadd1a08b9ed1b",
            ),
            (
                include_bytes!("../../../schemas/document-spec-v2.schema.json").as_slice(),
                "d14b6f7bc8a3753a8a2c0e39431ac20ae86be38ceffdf649c804dec3905be746",
            ),
            (
                include_bytes!("../../../schemas/document-report-v2.schema.json").as_slice(),
                "0474ac0a6c3c5cfff4d33bd11259b26169a676a078be5952ab83e2839f54090b",
            ),
        ] {
            assert_eq!(sha256_hex(bytes), expected);
        }
    }
}
