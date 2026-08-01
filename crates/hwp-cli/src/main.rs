//! hwp — HWP/HWPX 문서 처리 CLI.
//!
//! CLI 정의(`Cli`/`Cmd`/value_enum)는 lib 타깃(`hwp_cli::cli`)에 있다 — 문서 자동
//! 생성 테스트가 명령 트리를 introspect할 수 있게 하기 위함. 여기서는 파싱과
//! 서브커맨드 디스패치만 담당한다.

mod commands;
mod format;

use clap::{CommandFactory, FromArgMatches};

use hwp_cli::cli::{Cli, Cmd};
use hwp_cli::i18n;

fn main() -> anyhow::Result<()> {
    // 도움말 언어는 clap이 help/오류를 출력하기 전에 정해야 한다.
    let command = i18n::localize(Cli::command(), i18n::Lang::detect());
    let cli = match Cli::from_arg_matches(&command.get_matches()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    match cli.cmd {
        Cmd::Info { file, json } => commands::info::run(&file, json),
        Cmd::Dump {
            file,
            stream,
            raw,
            json,
        } => commands::dump::run(&file, stream.as_deref(), raw, json),
        Cmd::Cat {
            file,
            format,
            preview,
            with_header_footer,
            with_hidden,
            with_segments,
        } => commands::cat::run(
            &file,
            format,
            preview,
            with_header_footer,
            with_hidden,
            with_segments,
        ),
        Cmd::Convert {
            input,
            output,
            to,
            strict,
            preserve_layout,
            embed_bin,
            media_dir,
            with_header_footer,
            with_hidden,
            font_dir,
        } => commands::convert::run(
            &input,
            &output,
            to,
            strict,
            preserve_layout,
            embed_bin,
            &commands::convert::MdOpts {
                media_dir: media_dir.as_deref(),
                with_header_footer,
                with_hidden,
            },
            font_dir,
        ),
        Cmd::Render {
            input,
            output,
            pages,
            dpi,
            format,
            font_dir,
        } => commands::render::run(&input, &output, &pages, dpi, format, font_dir),
        Cmd::Diff {
            input,
            r#ref,
            page,
            dpi,
            out,
            font_dir,
            tolerance,
        } => commands::diff::run(
            &input,
            &r#ref,
            page,
            dpi,
            out.as_deref(),
            font_dir,
            tolerance,
        ),
        Cmd::Mcp { font_dir } => commands::mcp::run(font_dir),
        Cmd::Update {
            check,
            tag,
            force,
            json,
        } => commands::update::run(check, tag.as_deref(), force, json),
        Cmd::New {
            output,
            from,
            set_meta,
            preset,
        } => commands::new::run(&output, from.as_deref(), &set_meta, preset),
        Cmd::Compose {
            spec,
            output,
            format,
            dry_run,
            report,
            allow_visual_fallback,
        } => commands::compose::run(
            &spec,
            &output,
            format,
            dry_run,
            report,
            allow_visual_fallback,
        ),
        Cmd::Template {
            template,
            data,
            output,
            template_format,
            data_format,
            dry_run,
            report,
        } => commands::template::run(
            &template,
            &data,
            &output,
            template_format,
            data_format,
            dry_run,
            report,
        ),
        Cmd::Edit(args) => {
            let (input, output, plan) = commands::edit::EditPlan::from_args(args);
            commands::edit::run(&input, &output, &plan)
        }
        Cmd::Fields { file, json } => commands::fields::run(&file, json),
        Cmd::Bookmarks { file, json } => commands::bookmarks::run(&file, json),
        Cmd::Slots { file, json } => commands::slots::run(&file, json),
        Cmd::Fill {
            input,
            output,
            set,
            data,
            json,
            allow_partial,
        } => commands::fill::run(&input, &output, &set, data.as_deref(), json, allow_partial),
        Cmd::Validate { file, json } => commands::validate::run(&file, json),
        Cmd::Certify {
            input,
            policy,
            report,
        } => commands::certify::run(&input, &policy, &report),
        Cmd::Corpus { manifest, report } => commands::corpus::run(&manifest, &report),
    }
}
