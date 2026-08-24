//! `hwp new` — 새 문서 생성 (markdown/빈 문서 → hwpx).

use std::path::{Path, PathBuf};

/// Shared new-document inputs after CLI/MCP profile and margin normalization.
#[derive(Clone, Debug, Default)]
pub struct NewOptions {
    pub preset: Option<hwp_convert::OfficialPreset>,
    pub margins: hwp_convert::PageMarginOverrides,
    pub strict: bool,
    /// Document frames (`--doc-head`/`--doc-foot`/...), parsed via [`Self::with_frames`]
    /// (GONG-03, D-01).
    pub frames: hwp_convert::FrameFields,
}

impl NewOptions {
    /// Validate millimetre inputs once for both public entry points, then store writer units.
    pub fn from_millimetres(
        preset: Option<hwp_convert::OfficialPreset>,
        top: Option<f64>,
        bottom: Option<f64>,
        left: Option<f64>,
        right: Option<f64>,
        strict: bool,
    ) -> anyhow::Result<Self> {
        let mm_to_unit = |name: &str, value: Option<f64>| -> anyhow::Result<_> {
            value
                .map(|millimetres| {
                    if !millimetres.is_finite() || !(0.0..=200.0).contains(&millimetres) {
                        anyhow::bail!("{name}은 유한한 0..=200mm 범위여야 합니다");
                    }
                    Ok(hwp_model::HwpUnit(
                        (millimetres * 7200.0 / 25.4).round() as i32
                    ))
                })
                .transpose()
        };
        let margins = hwp_convert::PageMarginOverrides {
            top: mm_to_unit("margin_top_mm", top)?,
            bottom: mm_to_unit("margin_bottom_mm", bottom)?,
            left: mm_to_unit("margin_left_mm", left)?,
            right: mm_to_unit("margin_right_mm", right)?,
        };
        let mut page = hwp_convert::official::page_def(preset);
        margins.apply(&mut page);
        if page.margin_left.0 + page.margin_right.0 >= page.width.0
            || page.margin_top.0 + page.margin_bottom.0 >= page.height.0
        {
            anyhow::bail!("페이지 여백 합계는 A4 본문 영역을 남겨야 합니다");
        }
        Ok(Self {
            preset,
            margins,
            strict,
            frames: hwp_convert::FrameFields::default(),
        })
    }

    /// Parses the five repeatable frame-flag lists (`--doc-head`, `--doc-foot`, `--notice-head`,
    /// `--notice-foot`, `--press-head`) into `frames`, sharing one validator between the CLI and
    /// MCP surfaces (D-01).
    pub fn with_frames(
        mut self,
        doc_head: &[String],
        doc_foot: &[String],
        notice_head: &[String],
        notice_foot: &[String],
        press_head: &[String],
    ) -> anyhow::Result<Self> {
        self.frames = hwp_convert::parse_frame_fields(
            doc_head,
            doc_foot,
            notice_head,
            notice_foot,
            press_head,
        )
        .map_err(|e| anyhow::anyhow!(e))?;
        Ok(self)
    }
}

pub enum NewInput<'a> {
    Markdown {
        text: &'a str,
        base_dir: Option<&'a Path>,
        /// Sandbox roots binding image references inside the markdown (MCP `--root`, #56).
        /// Empty = no check (CLI behavior).
        roots: &'a [PathBuf],
    },
    Json(&'a str),
    Empty,
}

#[derive(Debug)]
pub struct NewReport {
    pub output: String,
    pub warnings: Vec<String>,
    pub preservation: hwp_model::PreservationReport,
}

pub fn run(
    output: &Path,
    from: Option<&Path>,
    set_meta: &[String],
    options: &NewOptions,
) -> anyhow::Result<()> {
    let owned;
    let input = match from {
        Some(src) => {
            owned = std::fs::read_to_string(src)?;
            if src
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                NewInput::Json(&owned)
            } else {
                NewInput::Markdown {
                    text: &owned,
                    base_dir: src.parent(),
                    roots: &[],
                }
            }
        }
        None => NewInput::Empty,
    };
    finish(output, input, set_meta, options)
}

/// Same as [`run`] but sources markdown text from an embedded `--template` skeleton
/// (`commands::skill::template_file`) instead of reading `--from` off disk. `base_dir: None`
/// and empty `roots` because embedded template text carries no filesystem-relative image
/// references (GONG-03, TMPL-01).
pub fn run_embedded(
    output: &Path,
    text: &str,
    set_meta: &[String],
    options: &NewOptions,
) -> anyhow::Result<()> {
    finish(
        output,
        NewInput::Markdown {
            text,
            base_dir: None,
            roots: &[],
        },
        set_meta,
        options,
    )
}

/// Resolves `--template <name>` and enforces D-05: `--template` is refused together with
/// `--from` and with any frame flag (`--doc-head`/`--doc-foot`/`--notice-head`/`--notice-foot`/
/// `--press-head`), because templates already carry their own 두문/결문 (Phase 2.1 D-19) and
/// combining them with frame flags would double the frames. Resolution goes through
/// `commands::skill::template_file`, the same embedded table `--list-templates` reads — never a
/// second embedded copy, never a filesystem path built from `name` (T-02.4-13).
pub fn resolve_template(
    name: &str,
    from_given: bool,
    any_frame_flag: bool,
) -> anyhow::Result<&'static str> {
    if from_given {
        anyhow::bail!(
            "--template과 --from은 함께 쓸 수 없습니다: 둘 다 문서 내용을 지정하는 경로입니다. \
             --template {name} 또는 --from 중 하나만 쓰세요."
        );
    }
    if any_frame_flag {
        anyhow::bail!(
            "--template {name}은(는) 프레임 플래그(--doc-head/--doc-foot/--notice-head/\
             --notice-foot/--press-head)와 함께 쓸 수 없습니다: 템플릿은 두문/결문을 이미 \
             포함하므로 함께 지정하면 프레임이 중복됩니다. 프레임을 직접 구성하려면 --template \
             없이 프레임 플래그만 쓰세요."
        );
    }
    crate::commands::skill::template_file(name)
        .map(|file| file.contents)
        .ok_or_else(|| {
            let accepted: Vec<&str> = crate::commands::skill::template_names()
                .map(|(slug, _)| slug)
                .collect();
            anyhow::anyhow!(
                "알 수 없는 템플릿 이름: {name} (사용 가능: {})",
                accepted.join(", ")
            )
        })
}

fn finish(
    output: &Path,
    input: NewInput<'_>,
    set_meta: &[String],
    options: &NewOptions,
) -> anyhow::Result<()> {
    let report = execute(output, input, set_meta, options)?;
    crate::commands::convert::print_warnings(&report.warnings);
    crate::commands::preservation::print_report(&report.preservation);
    eprintln!("생성 완료: {}", output.display());
    Ok(())
}

pub fn execute(
    output: &Path,
    input: NewInput<'_>,
    set_meta: &[String],
    options: &NewOptions,
) -> anyhow::Result<NewReport> {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let write_hwp = match ext.as_deref() {
        Some("hwp") => true,
        Some("hwpx") => false,
        Some(other) => anyhow::bail!(
            "지원하지 않는 출력 확장자입니다: .{other} (`hwp new`는 .hwp 또는 .hwpx만 지원합니다)"
        ),
        None => anyhow::bail!(
            "출력 파일에 확장자가 없습니다: {} (`hwp new`는 .hwp 또는 .hwpx만 지원합니다)",
            output.display()
        ),
    };
    let preset = options.preset;
    let mut import_warnings = Vec::new();
    let mut doc = match input {
        NewInput::Json(text) => {
            if preset.is_some() || options.margins != hwp_convert::PageMarginOverrides::default() {
                anyhow::bail!(
                    "--preset 및 --margin-*은 markdown 입력 전용입니다 (JSON IR은 헤더 포함)"
                );
            }
            if !options.frames.is_empty() {
                anyhow::bail!(
                    "--doc-head/--doc-foot 등 프레임 플래그는 markdown 입력 전용입니다 (JSON IR은 헤더 포함)"
                );
            }
            hwp_convert::from_json(text).map_err(|e| anyhow::anyhow!("JSON IR 파싱 실패: {e}"))?
        }
        NewInput::Markdown {
            text,
            base_dir,
            roots,
        } => {
            let (doc, warnings) = hwp_convert::from_markdown_report(
                text,
                &hwp_convert::MarkdownImportOptions {
                    base_dir,
                    roots,
                    preset,
                    page_margins: options.margins,
                    frames: Some(&options.frames),
                },
            )
            .map_err(|e| anyhow::anyhow!("markdown 가져오기 실패: {e}"))?;
            import_warnings = warnings;
            doc
        }
        NewInput::Empty => hwp_convert::from_markdown_with(
            "",
            &hwp_convert::MarkdownImportOptions {
                base_dir: None,
                roots: &[],
                preset,
                page_margins: options.margins,
                frames: Some(&options.frames),
            },
        ),
    };

    // Frame/preset compatibility warning (T-02.4-05): advisory only, the document is still
    // written. Never contains "계약 위반" (Pitfall 7), so it cannot trip the --strict filter below.
    import_warnings.extend(hwp_convert::compatibility_warnings(&options.frames, preset));

    // --strict: markdown import가 내용을 드롭했으면(HTML 블록 계약 위반) 실패 처리한다.
    if options.strict {
        let drops: Vec<&str> = import_warnings
            .iter()
            .filter(|w| w.contains("계약 위반"))
            .map(String::as_str)
            .collect();
        if !drops.is_empty() {
            anyhow::bail!(
                "--strict: HTML 블록 계약 위반 {}건 드롭\n{}",
                drops.len(),
                drops.join("\n")
            );
        }
    }

    // The direct HWP5 official-numbering record has evidence for one
    // definition per semantic level only. Reject independently restarted
    // ordered lists before opening the transactional staging path; HWPX
    // remains available for that topology.
    if write_hwp && preset.is_some() {
        hwp5::validate_official_hwp_numbering(&doc).map_err(|error| anyhow::anyhow!(error))?;
    }

    // 메타데이터 지정("키=값")을 덮어쓴다(JSON IR에 있던 값보다 우선).
    for spec in set_meta {
        hwp_convert::apply_meta(&mut doc, spec).map_err(|e| anyhow::anyhow!(e))?;
    }

    let mut writer_report = crate::commands::output::write_validated(
        output,
        None,
        |staged| {
            if write_hwp {
                crate::commands::convert::write_hwp(&doc, staged, false)
            } else {
                Ok(hwpx::write_document_with_report(&doc, staged)?)
            }
        },
        |staged, writer_report| {
            crate::commands::reject_preservation_loss("new", &writer_report.preservation)?;
            crate::commands::cat::load_document(staged)
                .map_err(|e| anyhow::anyhow!("생성 문서 재읽기 실패: {e:#}"))?;
            Ok(())
        },
    )?;
    // markdown import 경고(계약 위반 드롭 등)를 리포트에 실어 CLI/MCP에서 프로그램적으로 보이게 한다.
    writer_report.warnings.extend(import_warnings);
    Ok(NewReport {
        output: output.display().to_string(),
        warnings: writer_report.warnings,
        preservation: writer_report.preservation,
    })
}
