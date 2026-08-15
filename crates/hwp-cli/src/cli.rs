//! `hwp` CLI 정의 — clap derive 기반 명령/플래그 선언.
//!
//! 이 모듈은 lib 타깃으로 노출된다(`hwp_cli::cli`). bin(`main.rs`)이 파싱·디스패치에
//! 쓰고, `tests/cli_reference.rs`가 `clap::CommandFactory`로 명령 트리를 introspect해
//! `docs/manual/cli-reference.md`(영문 정본)와 `cli-reference.ko.md`(한국어)를 자동 생성한다
//! (코드-문서 동기화 게이트).
//!
//! **도움말 텍스트는 영문이 정본이다.** 한국어는 [`crate::i18n`]의 오버레이 표가 정본이며,
//! 런타임에 로케일이나 `--lang`에 따라 덮어쓴다. 여기 doc comment를 고치면 i18n 표도 같이
//! 고쳐야 한다(테스트가 누락을 잡는다).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

fn parse_dpi(value: &str) -> Result<f64, String> {
    let dpi = value
        .parse::<f64>()
        .map_err(|_| format!("DPI가 숫자가 아닙니다: {value}"))?;
    let min = f64::from(hwp_render::MIN_DPI);
    let max = f64::from(hwp_render::MAX_DPI);
    if !dpi.is_finite() || !(min..=max).contains(&dpi) {
        return Err(format!(
            "DPI는 유한한 {min}..={max} 범위여야 합니다: {value}"
        ));
    }
    Ok(dpi)
}

#[derive(Parser)]
#[command(name = "hwp", version, about = "HWP/HWPX document toolkit")]
pub struct Cli {
    /// Help language (default: from locale, otherwise English). Also settable with HWP_LANG
    #[arg(long, value_enum, global = true)]
    pub lang: Option<LangArg>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

/// `--lang` 값. 감지 로직은 [`crate::i18n::Lang`]에 있다.
#[derive(Clone, Copy, ValueEnum)]
pub enum LangArg {
    En,
    Ko,
}

#[derive(Subcommand)]
// Edit 변형이 편집 플래그(Vec<String> 다수)로 커서 다른 변형과 크기차가 크다.
// CLI 명령 enum은 시작 시 한 번만 파싱되므로 크기차는 무의미 — 박싱 대신 허용.
#[allow(clippy::large_enum_variant)]
pub enum Cmd {
    /// Show file information: format, version, properties and stream list
    Info {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },

    /// Extract text
    Cat {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Output format
        #[arg(long, value_enum, default_value = "plain")]
        format: TextFormat,
        /// Print only the PrvText preview, without parsing the body
        #[arg(long)]
        preview: bool,
        /// Also extract header and footer text (default: excluded)
        #[arg(long = "with-header-footer")]
        with_header_footer: bool,
        /// Also extract hidden comment text (default: excluded)
        #[arg(long = "with-hidden")]
        with_hidden: bool,
        /// (markdown only) Emit the markdown together with the source coordinates
        /// (section/paragraph) of each output character range, as a one-line JSON
        /// envelope: {"markdown": ..., "segments": [...]}
        #[arg(long = "with-segments")]
        with_segments: bool,
    },

    /// Search paragraph text (grep semantics; non-zero exit when no match)
    Grep {
        /// Pattern to find (substring match)
        pattern: String,
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Case-insensitive match
        #[arg(long = "ignore-case")]
        ignore_case: bool,
    },

    /// Convert between formats
    Convert {
        /// Input HWP/HWPX files ("-" reads stdin; multiple inputs require --out-dir)
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Output file path ("-" writes stdout for text formats: md/json/html/txt/csv; required with a single input)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Output directory for multiple inputs (file names are "<stem>.<ext>", requires --to)
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Output format (inferred from the extension when omitted)
        #[arg(long, value_enum)]
        to: Option<ConvertFormat>,
        /// Fail when data that cannot be preserved (opaque) is found during conversion
        #[arg(long)]
        strict: bool,
        /// Write the typed preservation ledger (hwp-preservation-report-v1) as JSON to this
        /// path, even when the conversion succeeds without loss (single input only).
        /// The preservation inspection only runs for hwp/hwpx targets — for other output
        /// formats (docx, md, ...) the ledger is always empty
        #[arg(long)]
        loss_report: Option<PathBuf>,
        /// Preserve the line layout cache (unmodified round-trips only; Hancom treats
        /// a layout inconsistent with the content as tampering, so it is dropped by default)
        #[arg(long)]
        preserve_layout: bool,
        /// Embed attached binaries (images) as base64 in JSON output (self-contained JSON)
        #[arg(long)]
        embed_bin: bool,
        /// (md) Image extraction directory, default "<output stem>.media". A relative
        /// path resolves against the output file and links use the path as given (e.g. figs)
        #[arg(long)]
        media_dir: Option<PathBuf>,
        /// (md) Also include header and footer text (default: excluded)
        #[arg(long = "with-header-footer")]
        with_header_footer: bool,
        /// (md) Also include hidden comment text (default: excluded)
        #[arg(long = "with-hidden")]
        with_hidden: bool,
        /// (pdf) Additional font directory (repeatable; defaults to HWP_FONT_DIR or fonts/)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
    },

    /// Render pages
    Render {
        /// Input HWP/HWPX file
        input: PathBuf,
        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
        /// Page range: "1", "1-3", "all"
        #[arg(long, default_value = "all")]
        pages: String,
        /// Resolution in DPI (finite, 36..=600)
        #[arg(long, default_value_t = 96.0, value_parser = parse_dpi)]
        dpi: f64,
        /// Output format (inferred from the extension when omitted)
        #[arg(long, value_enum)]
        format: Option<RenderFormat>,
        /// Write a closed machine-readable render report atomically
        #[arg(long)]
        report: Option<PathBuf>,
        /// Additional font directory (repeatable)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
    },

    /// Create a new document
    New {
        /// Output HWP/HWPX path
        #[arg(short, long)]
        output: PathBuf,
        /// Input markdown or JSON file (empty document when omitted)
        #[arg(long)]
        from: Option<PathBuf>,
        /// Set metadata "key=value" (keys: title|author|subject|keywords; repeatable)
        #[arg(long = "set-meta")]
        set_meta: Vec<String>,
        /// Official-document preset (markdown input only): gian for an approval draft
        /// (Malgun Gothic 11.5pt), report for a report (HCR Batang 15pt). Includes margins,
        /// four-level numbering and page numbers
        #[arg(long, value_enum)]
        preset: Option<PresetArg>,
        /// Fail (non-zero exit) when markdown import drops content, e.g. an HTML block that
        /// violates the import contract. Default: warn and continue (exit 0)
        #[arg(long)]
        strict: bool,
    },

    /// Compose a structured document deterministically from DocumentSpec v1/v2 (JSON/YAML)
    Compose {
        /// DocumentSpec v1/v2 input file (.json, .yaml, .yml)
        spec: PathBuf,
        /// Output HWP/HWPX
        #[arg(short, long)]
        output: PathBuf,
        /// Input format (inferred from the spec extension when omitted)
        #[arg(long, value_enum)]
        format: Option<SpecFormatArg>,
        /// Produce the validation and compilation report without writing the file
        #[arg(long)]
        dry_run: bool,
        /// Print the run report as JSON
        #[arg(long)]
        report: bool,
        /// [deprecated] v1 compatibility only; v2 rejects this policy override
        #[arg(long)]
        allow_visual_fallback: bool,
    },

    /// Generate typed native HWP/HWPX from TemplateSpec/Data v1
    Template {
        /// TemplateSpec v1 input file (.json, .yaml, .yml)
        template: PathBuf,
        /// TemplateData v1 input file (.json, .yaml, .yml)
        #[arg(long)]
        data: PathBuf,
        /// Output HWP/HWPX
        #[arg(short, long)]
        output: PathBuf,
        /// TemplateSpec input format (inferred from the extension when omitted)
        #[arg(long, value_enum)]
        template_format: Option<SpecFormatArg>,
        /// TemplateData input format (inferred from the extension when omitted)
        #[arg(long, value_enum)]
        data_format: Option<SpecFormatArg>,
        /// Run the real expansion, writer and validation paths without publishing the result
        #[arg(long)]
        dry_run: bool,
        /// Print the preservation and expansion report as JSON
        #[arg(long)]
        report: bool,
    },

    /// Compare a render against a Hancom reference PNG (offset and pixel difference)
    Diff {
        /// Input HWP/HWPX file
        input: PathBuf,
        /// Reference PNG exported from Hancom for the same page at the same DPI
        #[arg(long)]
        r#ref: PathBuf,
        /// Page to compare (1-based)
        #[arg(long, default_value_t = 1)]
        page: usize,
        /// Resolution in DPI (finite, 36..=600)
        #[arg(long, default_value_t = 96.0, value_parser = parse_dpi)]
        dpi: f64,
        /// Difference image output path (defaults to <ref>.diff.png)
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Additional font directory (repeatable)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
        /// Per-channel tolerance; differences at or below this count as equal
        #[arg(long, default_value_t = 16)]
        tolerance: u8,
        /// Report output format (json = machine-readable, for the parity batch runner)
        #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
        format: DiffFormat,
        /// Compare this raster (e.g. pdftoppm of our PDF) against --ref instead of
        /// rendering the input document; the input path is only recorded in the report
        #[arg(long)]
        ours_png: Option<PathBuf>,
    },

    /// Edit an existing document (text replacement, table cells); images and formatting preserved
    Edit(EditArgs),

    /// List fields (name, kind, value)
    Fields {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },

    /// List bookmarks (name)
    Bookmarks {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },

    /// List `{{name}}` text placeholders (template slots)
    Slots {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },

    /// Fidelity-preserving template fill (replace `{{name}}` in hwpx, package preserved)
    Fill {
        /// Input HWPX template
        input: PathBuf,
        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
        /// Fill a placeholder, "name=value" (repeatable; replaces `{{name}}`). "name=@part.md" splices a part file (markdown + HTML table blocks, docs/design/18 contract) into the `{{name}}` anchor paragraph instead — part-based composition for large documents. "@@" escapes a literal '@'
        #[arg(long)]
        set: Vec<String>,
        /// JSON object file mapping name to value (bulk fill; "parts": {"name": "path"} splices part files, "tables": [...] fills table rows)
        #[arg(long)]
        data: Option<PathBuf>,
        /// Print the replacement summary as JSON ({output, replaced, counts})
        #[arg(long)]
        json: bool,
        /// Publish the matched values even if some requests found no placeholder (default: fail if any is unreplaced)
        #[arg(long = "allow-partial")]
        allow_partial: bool,
    },

    /// Structural validation (mimetype, required entries, XML parsing); exit code 0 when valid
    Validate {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },

    /// Certify package, semantics, native render and independent import under a versioned policy
    Certify {
        /// HWP/HWPX input to certify
        input: PathBuf,
        /// hwp-certification-policy-v1 JSON/YAML
        #[arg(long)]
        policy: PathBuf,
        /// Atomic artifact directory to create (an existing path is refused)
        #[arg(long)]
        report: PathBuf,
    },

    /// Generate the frozen structured corpus twice, reopen it and certify natively
    Corpus {
        /// hwp-structured-corpus-v1 manifest JSON
        #[arg(long)]
        manifest: PathBuf,
        /// Atomic run report directory to create (an existing path is refused)
        #[arg(long)]
        report: PathBuf,
    },

    /// MCP (Model Context Protocol) stdio server: a tool interface for AI agents
    Mcp {
        /// Default font directory for the render and diff tools (repeatable)
        #[arg(long)]
        font_dir: Vec<PathBuf>,
        /// Restrict all file access to this directory (repeatable). Default: unrestricted
        #[arg(long)]
        root: Vec<PathBuf>,
    },

    /// Self-update: fetch the latest `hwp` from GitHub releases and replace the running binary
    Update {
        /// Report the current and latest versions without replacing
        #[arg(long)]
        check: bool,
        /// Pin a specific release (for example "v0.2.0", to roll back)
        #[arg(long)]
        tag: Option<String>,
        /// Re-download and replace even at the same version (to repair a broken install)
        #[arg(long)]
        force: bool,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },

    /// Manage the bundled agent skill (SKILL.md for AI coding assistants)
    Skill {
        #[command(subcommand)]
        cmd: SkillCmd,
    },

    /// [developer] Dump record and package structure
    Dump {
        /// Target HWP/HWPX file
        file: PathBuf,
        /// Target stream or entry (for example "DocInfo", "BodyText/Section0", "Contents/header.xml")
        #[arg(long)]
        stream: Option<String>,
        /// Print record payloads as hex
        #[arg(long)]
        raw: bool,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },
}

/// `hwp edit` 입력. 편집 실행 전에 `EditPlan`의 타입화된 작업 목록으로 정규화한다.
#[derive(Args)]
pub struct EditArgs {
    /// Input HWP/HWPX file
    pub input: PathBuf,
    /// Output file path
    #[arg(short, long)]
    pub output: PathBuf,
    /// Replace text, "find=>replace" (repeatable; replaces every match)
    #[arg(long)]
    pub replace: Vec<String>,
    /// Set a table cell, "table:row:col=value" (repeatable; 0-based indices)
    #[arg(long = "set-cell")]
    pub set_cell: Vec<String>,
    /// Fill a field, "name=value" (repeatable; list names with hwp fields)
    #[arg(long = "set-field")]
    pub set_field: Vec<String>,
    /// Set metadata, "key=value" (keys: title|author|subject|keywords; repeatable)
    #[arg(long = "set-meta")]
    pub set_meta: Vec<String>,
    /// Create a field, "anchor=>name" or "anchor=>name=value": insert a %clk field after the anchor text (repeatable)
    #[arg(long = "create-field")]
    pub create_field: Vec<String>,
    /// Create a bookmark, "anchor=>name": insert a bokm marker after the anchor text (repeatable)
    #[arg(long = "create-bookmark")]
    pub create_bookmark: Vec<String>,
    /// Create a hyperlink, "anchor=>URL" or "anchor=>text=>URL": insert %hlk after the anchor (repeatable)
    #[arg(long = "create-hyperlink")]
    pub create_hyperlink: Vec<String>,
    /// Insert an image, "anchor=>path" or "anchor=>path@WxH" (mm): insert a picture after the anchor (repeatable)
    #[arg(long = "insert-image")]
    pub insert_image: Vec<String>,
    /// Stamp a seal, "anchor=>path" or "anchor=>path@size" (mm): float the seal over the anchor text (repeatable)
    #[arg(long = "seal")]
    pub seal: Vec<String>,
    /// Character formatting, "find:property=value,..." (for example "Title:bold=on,size=16,color=#FF0000")
    #[arg(long = "set-format")]
    pub set_format: Vec<String>,
    /// Paragraph alignment, "find=alignment" (left/right/center/justify/distribute)
    #[arg(long = "set-align")]
    pub set_align: Vec<String>,
    /// Insert a paragraph, "anchor=>text": after the paragraph containing the anchor (repeatable)
    #[arg(long = "insert-para")]
    pub insert_para: Vec<String>,
    /// Insert a paragraph before, "anchor=>text": before the paragraph containing the anchor (repeatable)
    #[arg(long = "insert-para-before")]
    pub insert_para_before: Vec<String>,
    /// Delete a paragraph, "text": delete the paragraph containing the text (repeatable)
    #[arg(long = "delete-para")]
    pub delete_para: Vec<String>,
    /// Add table rows, "table[:at[:count[:template_row]]]": at omitted or "end" appends, a number inserts before that row; count defaults to 1; template_row donates row height and cell/paragraph/character styling, never text (repeatable, 0-based; merged tables supported)
    #[arg(long = "add-row")]
    pub add_row: Vec<String>,
    /// Add table columns, "table[:at[:count]]": at omitted or "end" appends, a number inserts before that column; count defaults to 1; total width is preserved by shrinking existing columns evenly. Merged tables supported (repeatable, 0-based)
    #[arg(long = "add-col")]
    pub add_col: Vec<String>,
    /// Delete a table row, "table:row" (repeatable, 0-based; a merged row is refused)
    #[arg(long = "delete-row")]
    pub delete_row: Vec<String>,
    /// Delete a table column, "table:col": total width is preserved by redistributing to the remaining columns; merged cells shrink (repeatable, 0-based)
    #[arg(long = "delete-col")]
    pub delete_col: Vec<String>,
    /// Merge cells, "table:r1:c1:r2:c2": merge a rectangular area into its top-left anchor (repeatable, 0-based)
    #[arg(long = "merge-cells")]
    pub merge_cells: Vec<String>,
    /// Split a cell, "table:row:col": break a merged cell back into 1x1 cells (repeatable, 0-based)
    #[arg(long = "split-cell")]
    pub split_cell: Vec<String>,
    /// Insert a table, "anchor=>json": insert a uniform table after the anchor paragraph; json is an array of row arrays (repeatable)
    #[arg(long = "add-table")]
    pub add_table: Vec<String>,
    /// Clone a table, "source_table=>anchor[=>blank|keep]": deep-copy table source_table (0-based, recursive) after the anchor paragraph; blank (default) keeps structure/styles with empty cells, keep also clones supported content (nested tables, images) with remapped ids (repeatable)
    #[arg(long = "clone-table")]
    pub clone_table: Vec<String>,
    /// Paragraph shape properties, "find=>key:value" (keys: line-spacing (% or Npt), indent, left, right, top, bottom (mm); repeatable)
    #[arg(long = "set-para")]
    pub set_para: Vec<String>,
    /// Page setup, "key:value" (keys: width, height, margin-left, margin-right, margin-top, margin-bottom (mm), orientation (portrait|landscape); repeatable)
    #[arg(long = "set-page")]
    pub set_page: Vec<String>,
    /// Delete an image, "anchor": delete the picture in the anchor paragraph (repeatable)
    #[arg(long = "delete-image")]
    pub delete_image: Vec<String>,
    /// Delete a table, "n" (0-based index) or "anchor" (table in the anchor paragraph) (repeatable)
    #[arg(long = "delete-table")]
    pub delete_table: Vec<String>,
    /// Delete a field by name, "name" (repeatable; list names with hwp fields)
    #[arg(long = "delete-field")]
    pub delete_field: Vec<String>,
    /// Delete a bookmark by name, "name" (repeatable; list names with hwp bookmarks)
    #[arg(long = "delete-bookmark")]
    pub delete_bookmark: Vec<String>,
    /// Verify by re-reading after writing
    #[arg(long)]
    pub verify: bool,
    /// Publish the matched edits even if some requests found no target (default: fail if any is unapplied)
    #[arg(long = "allow-partial")]
    pub allow_partial: bool,
}

/// `hwp skill` subcommand.
#[derive(Subcommand)]
pub enum SkillCmd {
    /// Write the embedded SKILL.md into a directory (default ./hwp; the file lands at <DIR>/SKILL.md)
    Export {
        /// Output directory (mutually exclusive with --install)
        #[arg(short, long, conflicts_with = "install")]
        output: Option<PathBuf>,
        /// Install into a known agent skills directory instead
        #[arg(long, value_enum)]
        install: Option<InstallTarget>,
        /// Amazon Quick profile ID or absolute profile directory (Amazon Quick installs only)
        #[arg(
            long,
            requires = "install",
            conflicts_with = "output",
            value_name = "ID_OR_ABSOLUTE_PATH"
        )]
        quick_profile: Option<PathBuf>,
    },
}

/// `--install` target — per-agent skill directory.
#[derive(Clone, Copy, ValueEnum)]
pub enum InstallTarget {
    /// ~/.claude/skills/hwp/
    ClaudeCode,
    /// ~/.codex/skills/hwp/
    Codex,
    /// Active Amazon Quick Desktop profile under ~/.quickwork/
    AmazonQuick,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum TextFormat {
    Plain,
    Markdown,
    Json,
    Html,
    Csv,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ConvertFormat {
    Hwp,
    Hwpx,
    Md,
    Json,
    Html,
    Pdf,
    Odt,
    Txt,
    Csv,
    Docx,
}

/// Official-document preset (`hwp new --preset`).
#[derive(Clone, Copy, ValueEnum)]
pub enum PresetArg {
    Gian,
    Report,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SpecFormatArg {
    Json,
    Yaml,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RenderFormat {
    Png,
    Svg,
    Pdf,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiffFormat {
    Text,
    Json,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_export_parses_amazon_quick_and_rejects_common_conflicts() {
        assert!(
            Cli::try_parse_from(["hwp", "skill", "export"]).is_ok(),
            "the flagless default invocation must keep parsing"
        );
        assert!(
            Cli::try_parse_from(["hwp", "skill", "export", "-o", "out", "--install", "codex"])
                .is_err(),
            "-o and --install must be mutually exclusive"
        );
        assert!(
            Cli::try_parse_from(["hwp", "skill", "export", "--quick-profile", "profile"]).is_err(),
            "--quick-profile must require --install"
        );
        assert!(
            Cli::try_parse_from([
                "hwp",
                "skill",
                "export",
                "-o",
                "out",
                "--install",
                "amazon-quick",
                "--quick-profile",
                "profile",
            ])
            .is_err(),
            "-o must conflict with an Amazon Quick install"
        );

        let parsed = Cli::try_parse_from([
            "hwp",
            "skill",
            "export",
            "--install",
            "amazon-quick",
            "--quick-profile",
            "enterprise-test",
        ])
        .expect("Amazon Quick install target should parse");
        let Cmd::Skill {
            cmd:
                SkillCmd::Export {
                    install: Some(InstallTarget::AmazonQuick),
                    quick_profile: Some(profile),
                    ..
                },
        } = parsed.cmd
        else {
            panic!("unexpected parsed command")
        };
        assert_eq!(profile, PathBuf::from("enterprise-test"));
    }

    #[test]
    fn render_and_diff_reject_non_finite_or_out_of_range_dpi() {
        for command in ["render", "diff"] {
            for dpi in ["0", "-1", "NaN", "inf", "601", "1e300"] {
                let mut args = vec!["hwp", command, "input.hwpx"];
                if command == "render" {
                    args.extend(["--output", "out.png"]);
                } else {
                    args.extend(["--ref", "reference.png"]);
                }
                args.extend(["--dpi", dpi]);
                assert!(
                    Cli::try_parse_from(args).is_err(),
                    "{command} accepted dpi={dpi}"
                );
            }
        }
    }
}
