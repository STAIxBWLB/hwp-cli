//! `hwp new` — 새 문서 생성 (markdown/빈 문서 → hwpx).

use std::path::Path;

use hwp_cli::cli::PresetArg;

pub fn run(
    output: &Path,
    from: Option<&Path>,
    set_meta: &[String],
    preset: Option<PresetArg>,
) -> anyhow::Result<()> {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let preset = preset.map(|p| match p {
        PresetArg::Gian => hwp_convert::OfficialPreset::Gian,
        PresetArg::Report => hwp_convert::OfficialPreset::Report,
    });
    let mut doc = match from {
        Some(src) => {
            let text = std::fs::read_to_string(src)?;
            if src
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                if preset.is_some() {
                    anyhow::bail!("--preset은 markdown 입력 전용입니다 (JSON IR은 헤더 포함)");
                }
                // JSON IR(편집 왕복) 입력 — 헤더가 JSON에 포함됨
                hwp_convert::from_json(&text)
                    .map_err(|e| anyhow::anyhow!("JSON IR 파싱 실패 ({}): {e}", src.display()))?
            } else {
                // md 파일의 디렉터리를 상대 경로 이미지의 기준으로 넘긴다.
                hwp_convert::from_markdown_with(
                    &text,
                    &hwp_convert::MarkdownImportOptions {
                        base_dir: src.parent(),
                        preset,
                    },
                )
            }
        }
        None => hwp_convert::from_markdown_with(
            "",
            &hwp_convert::MarkdownImportOptions {
                base_dir: None,
                preset,
            },
        ),
    };

    // 메타데이터 지정("키=값")을 덮어쓴다(JSON IR에 있던 값보다 우선).
    for spec in set_meta {
        hwp_convert::apply_meta(&mut doc, spec).map_err(|e| anyhow::anyhow!(e))?;
    }

    let warnings = if ext.as_deref() == Some("hwp") {
        crate::commands::convert::write_hwp(&doc, output, false)?
    } else {
        hwpx::write_document(&doc, output)?
    };
    crate::commands::convert::print_warnings(&warnings);
    eprintln!("생성 완료: {}", output.display());
    Ok(())
}
