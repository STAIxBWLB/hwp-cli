//! ahwp: HWP/HWPX document processing CLI.
//!
//! CLI definitions (`Cli`/`Cmd`/value_enum) live in the library target (`hwp_cli::cli`) so
//! generated-document tests can introspect the command tree. This target only parses
//! arguments and dispatches commands.

// Large json! literals like the hwp_edit tool schema in mcp.rs exceed the default recursion limit (128).
use crate::commands;

use clap::{CommandFactory, FromArgMatches};

use hwp_cli::cli::{Cli, Cmd};
use hwp_cli::i18n;

/// Stack for every thread that runs a command or an MCP tool call, including
/// each `hwp serve` connection thread. Deep document nesting recurses in the
/// parsers, so a default 2 MiB thread would abort the process there.
pub(crate) const WORKER_STACK_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn run() -> anyhow::Result<()> {
    use std::io::IsTerminal as _;
    if let Some(program) = std::env::args_os().next()
        && let Some(notice) = compatibility_notice(
            &program,
            std::io::stderr().is_terminal(),
            i18n::Lang::detect(),
        )
    {
        eprintln!("{notice}");
    }
    // Clap's derived command construction/parsing exceeds Windows' default 1 MiB main
    // stack in debug builds (observed in CI). Run on a larger worker stack across
    // platforms and build profiles.
    let worker = std::thread::Builder::new()
        .name("hwp-main".to_string())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(real_main)?;
    match worker.join() {
        Ok(result) => result,
        // Resume the worker panic to preserve its message and behavior.
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn real_main() -> anyhow::Result<()> {
    // Select the language before clap prints help or errors.
    let command = i18n::localize(Cli::command().bin_name("ahwp"), i18n::Lang::detect());
    let cli = match Cli::from_arg_matches(&command.get_matches()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    match cli.cmd {
        Cmd::Info {
            file,
            json,
            body_stats,
        } => commands::info::run(&file, json, body_stats),
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
            segments,
            password,
        } => commands::cat::run(
            &file,
            format,
            preview,
            with_header_footer,
            with_hidden,
            // The switch and the version are two flags at the CLI and one value here:
            // `None` is the envelope switched off.
            with_segments.then_some(segments),
            password,
        ),
        Cmd::Convert {
            inputs,
            output,
            out_dir,
            to,
            strict,
            loss_report,
            preserve_layout,
            embed_bin,
            media_dir,
            with_header_footer,
            with_hidden,
            font_dir,
            password,
        } => commands::convert::run_multi_with_password(
            &inputs,
            output.as_deref(),
            out_dir.as_deref(),
            to,
            strict,
            loss_report.as_deref(),
            preserve_layout,
            embed_bin,
            &commands::convert::MdOpts {
                media_dir: media_dir.as_deref(),
                with_header_footer,
                with_hidden,
            },
            font_dir,
            password,
        ),
        Cmd::Grep {
            pattern,
            file,
            ignore_case,
        } => commands::grep::run(&pattern, &file, ignore_case),
        Cmd::Merge {
            inputs,
            output,
            strict,
            loss_report,
            password,
        } => commands::merge::run(&inputs, &output, strict, loss_report.as_deref(), password),
        Cmd::Split {
            input,
            out_dir,
            pages,
            from,
            to,
            end_exclusive,
            source_sha256,
            range_report,
            strict,
            loss_report,
            password,
        } => commands::split::run(
            &input,
            &out_dir,
            &pages,
            from.map(|from| commands::split::RangeRequest {
                from,
                to,
                end_exclusive,
                source_sha256,
                report: range_report,
            }),
            strict,
            loss_report.as_deref(),
            password,
        ),
        Cmd::Render {
            input,
            output,
            pages,
            dpi,
            format,
            report,
            layout_json,
            font_dir,
            password,
        } => commands::render::run_with_password(
            &input,
            &output,
            &pages,
            dpi,
            format,
            font_dir,
            report.as_deref(),
            layout_json.as_deref(),
            password,
        ),
        Cmd::Diff {
            input,
            r#ref,
            page,
            dpi,
            out,
            font_dir,
            tolerance,
            format,
            ours_png,
        } => commands::diff::run(
            &input,
            &r#ref,
            page,
            dpi,
            out.as_deref(),
            font_dir,
            tolerance,
            format,
            ours_png.as_deref(),
        ),
        // D-13: `compare` follows diff(1) exit-code semantics, which the default
        // `anyhow::Result<()>` → `Termination` mapping cannot express (every other
        // arm's `Err` becomes exit code 1, the exact code reserved here for
        // "differences found"). This arm intercepts the outcome and calls the exit
        // primitive explicitly for all three codes instead of returning through it.
        Cmd::Compare {
            a,
            b,
            format,
            password,
        } => match commands::compare::run(&a, &b, format, password) {
            Ok(commands::compare::CompareOutcome::Identical) => std::process::exit(0),
            Ok(commands::compare::CompareOutcome::Different) => std::process::exit(1),
            Err(error) => {
                // Same `Error: ...` shape every other command's `Err` gets from
                // the `anyhow::Result` → `Termination` mapping; only the exit
                // code (2, not 1) differs here.
                eprintln!("Error: {error:?}");
                std::process::exit(2);
            }
        },
        Cmd::Mcp { font_dir, root } => commands::mcp::run(font_dir, root),
        Cmd::Serve {
            addr,
            root,
            font_dir,
            files,
        } => commands::mcp::serve(addr, root, font_dir, files),
        Cmd::Update {
            check,
            tag,
            force,
            json,
        } => commands::update::run(check, tag.as_deref(), force, json),
        Cmd::New {
            output,
            from,
            template,
            list_templates,
            set_meta,
            preset,
            margin_top,
            margin_bottom,
            margin_left,
            margin_right,
            strict,
            doc_head,
            doc_foot,
            notice_head,
            notice_foot,
            press_head,
            table_placement,
        } => {
            if list_templates {
                commands::skill::print_template_list();
                return Ok(());
            }
            // clap's `required_unless_present = "list_templates"` guarantees `output` is `Some`
            // on every path that reaches here.
            let output = output.expect("clap requires --output unless --list-templates");
            let embedded = template
                .as_deref()
                .map(|name| commands::new::resolve_template(name, from.is_some()))
                .transpose()?;
            // A template names the profile it was written for and the frames it needs; both are
            // defaults an explicit flag overrides.
            let template_defaults = template
                .as_deref()
                .and_then(commands::skill::template_defaults);
            let options = commands::new::NewOptions::from_millimetres(
                preset
                    .map(hwp_cli::cli::PresetArg::canonical)
                    .or(template_defaults.as_ref().map(|d| d.preset)),
                margin_top,
                margin_bottom,
                margin_left,
                margin_right,
                strict,
            )?
            .with_frames(
                &doc_head,
                &doc_foot,
                &notice_head,
                &notice_foot,
                &press_head,
            )?
            .with_template_frames(template_defaults.as_ref());
            let options = commands::new::NewOptions {
                table_placement: table_placement.map(hwp_cli::cli::TablePlacementArg::canonical),
                ..options
            };
            match embedded {
                Some(text) => commands::new::run_embedded(&output, text, &set_meta, &options),
                None => commands::new::run(&output, from.as_deref(), &set_meta, &options),
            }
        }
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
            if args.ops.is_some() {
                // #531: an ops file refused while loading still leaves the requested report.
                let refused = (
                    args.output.clone(),
                    args.report.clone(),
                    args.dry_run,
                    args.ops.clone(),
                );
                let (input, output, plan) = match commands::edit::EditPlan::from_ops(args) {
                    Ok(built) => built,
                    Err(error) => {
                        let (output, report, dry_run, ops) = refused;
                        // apply_form_bindings publishes only its receipt, as in run(); the
                        // write is best-effort so it never masks the refusal.
                        let binding = ops
                            .as_deref()
                            .is_some_and(crate::edit_ops::ops_file_names_form_bindings);
                        if !binding
                            && let Err(write_error) = commands::edit::emit_refused_before_plan(
                                &output, report, dry_run, &error,
                            )
                        {
                            eprintln!("경고: 거부 보고서를 쓰지 못했습니다: {write_error:#}");
                        }
                        return Err(error);
                    }
                };
                commands::edit::run(&input, &output, &plan)
            } else {
                let (input, output, plan) = commands::edit::EditPlan::from_args(args);
                commands::edit::run(&input, &output, &plan)
            }
        }
        Cmd::Fields { file, json } => commands::fields::run(&file, json),
        Cmd::Bookmarks { file, json } => commands::bookmarks::run(&file, json),
        Cmd::Styles { file, format } => commands::styles::run(&file, format),
        Cmd::Slots {
            file,
            json,
            forms,
            occurrences,
        } => commands::slots::run(&file, json, forms, occurrences),
        Cmd::Fill {
            input,
            output,
            set,
            data,
            json,
            allow_partial,
            forms,
        } => commands::fill::run(
            &input,
            &output,
            &set,
            data.as_deref(),
            json,
            allow_partial,
            forms,
        ),
        Cmd::Validate { file, json } => commands::validate::run(&file, json),
        Cmd::Lint {
            file,
            profile,
            json,
            strict,
        } => {
            let profile = match profile {
                hwp_cli::cli::LintProfileArg::Gongmun => hwp_convert::lint::LintProfile::Gongmun,
                hwp_cli::cli::LintProfileArg::Report => hwp_convert::lint::LintProfile::Report,
            };
            commands::lint::run(&file, profile, json, strict)
        }
        Cmd::Certify {
            input,
            policy,
            report,
        } => commands::certify::run(&input, &policy, &report),
        Cmd::Corpus { manifest, report } => commands::corpus::run(&manifest, &report),
        Cmd::Skill { cmd } => match cmd {
            hwp_cli::cli::SkillCmd::Export {
                output,
                install,
                quick_profile,
            } => commands::skill::run(output, install, quick_profile),
        },
    }
}

/// A compatibility invocation is quiet on pipes, including MCP transports.
fn compatibility_notice(
    program: &std::ffi::OsStr,
    terminal: bool,
    language: i18n::Lang,
) -> Option<&'static str> {
    let legacy = std::path::Path::new(program)
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|stem| stem.eq_ignore_ascii_case("hwp"));
    if !terminal || !legacy {
        return None;
    }
    Some(match language {
        i18n::Lang::En => {
            "hwp is deprecated; use ahwp. The compatibility name will be removed in 2.0."
        }
        i18n::Lang::Ko => "hwp 호환 명령은 2.0에서 제거됩니다. ahwp 명령을 사용하세요.",
    })
}

#[cfg(test)]
mod bridge_tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn compatibility_notice_requires_the_legacy_name_and_a_terminal() {
        for name in ["hwp", "hwp.exe", "HWP.EXE"] {
            assert!(compatibility_notice(OsStr::new(name), true, i18n::Lang::En).is_some());
            assert!(compatibility_notice(OsStr::new(name), false, i18n::Lang::En).is_none());
        }
        assert!(compatibility_notice(OsStr::new("ahwp"), true, i18n::Lang::En).is_none());
        assert!(compatibility_notice(OsStr::new("ahwp.exe"), true, i18n::Lang::En).is_none());
    }
}
