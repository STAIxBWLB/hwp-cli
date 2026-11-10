//! Generates the CLI command reference and gates it against drift (both languages).
//!
//! Introspects the `Cli` command tree via `clap::CommandFactory` and deterministically renders
//! `docs/manual/cli-reference.md` (English, canonical) and `cli-reference.ko.md` (Korean). The
//! test fails when the committed docs and the regenerated ones diverge, which forces a doc
//! update whenever the CLI definition (flags, help text) changes.
//!
//! Bless: `HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference`.

use clap::builder::StyledStr;
use clap::{Arg, ArgAction, Command, CommandFactory};
use hwp_cli::cli::Cli;
use hwp_cli::i18n::{self, Lang};

/// Committed doc path (relative to the crate).
fn doc_path(lang: Lang) -> std::path::PathBuf {
    let name = match lang {
        Lang::Ko => "cli-reference.ko.md",
        Lang::En => "cli-reference.md",
    };
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/manual")
        .join(name)
}

const HEADER_COMMENT_KO: &str = "<!-- 자동 생성 문서 — 수동 편집 금지. 재생성: HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference -->";
const HEADER_COMMENT_EN: &str = "<!-- Generated document. Do not edit by hand. Regenerate with: HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference -->";

/// Strip outer whitespace from a plain, ANSI-free styled string.
fn plain(s: &StyledStr) -> String {
    s.to_string().trim().to_string()
}

/// Escape table cells: collapse whitespace and escape vertical bars.
fn cell(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.replace('|', "\\|")
}

/// GitHub anchor: `ahwp skill export` → `ahwp-skill-export` (spaces become hyphens).
fn anchor(name: &str) -> String {
    format!("ahwp-{}", name.replace(' ', "-"))
}

/// Whether an action takes no value, such as a boolean flag.
fn is_flag(action: &ArgAction) -> bool {
    matches!(
        action,
        ArgAction::SetTrue
            | ArgAction::SetFalse
            | ArgAction::Count
            | ArgAction::Help
            | ArgAction::HelpShort
            | ArgAction::HelpLong
            | ArgAction::Version
    )
}

/// Omit automatic help/version actions and hidden arguments from generated docs.
fn skip_arg(arg: &Arg) -> bool {
    arg.is_hide_set()
        || matches!(
            arg.get_action(),
            ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version
        )
        || matches!(arg.get_id().as_str(), "help" | "version")
}

/// Render the value placeholder, using the uppercase ID when value_name is absent.
fn value_placeholder(arg: &Arg) -> String {
    let name = arg
        .get_value_names()
        .and_then(|ns| ns.first())
        .map(|s| s.to_string())
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());
    format!("`<{name}>`")
}

/// Visible enum values in declaration order; empty for non-enum arguments.
fn possible_values(arg: &Arg) -> Vec<String> {
    arg.get_possible_values()
        .iter()
        .filter(|pv| !pv.is_hide_set())
        .map(|pv| pv.get_name().to_string())
        .collect()
}

/// Normalize render_usage() to the full canonical command path.
/// A detached subcommand can omit its parent program name.
/// Reassemble the portion after its first name under the canonical command path.
fn usage_line(sub: &Command, path: &str) -> String {
    let raw = sub.clone().render_usage().to_string();
    let after_label = raw
        .trim()
        .strip_prefix("Usage:")
        .map(str::trim)
        .unwrap_or_else(|| raw.trim());
    // Take the body after the bare name (last path token) and reassemble it with the
    // full path — nested subcommands (`export`) normalize to `ahwp skill export …` too.
    let bare = path.rsplit(' ').next().unwrap_or(path);
    let body = match after_label.split_once(bare) {
        Some((_, rest)) => rest.trim_start(),
        None => "",
    };
    if body.is_empty() {
        format!("ahwp {path}")
    } else {
        format!("ahwp {path} {body}")
    }
}

/// Argument/flag table rows for a subcommand, in declaration order.
fn arg_rows(sub: &Command, lang: Lang) -> Vec<String> {
    let mut rows = Vec::new();
    for arg in sub.get_arguments() {
        if skip_arg(arg) {
            continue;
        }
        // Column 1: argument or flag name.
        let name_col = if arg.is_positional() {
            value_placeholder(arg)
        } else {
            match (arg.get_short(), arg.get_long()) {
                (Some(s), Some(l)) => format!("`-{s}, --{l}`"),
                (None, Some(l)) => format!("`--{l}`"),
                (Some(s), None) => format!("`-{s}`"),
                (None, None) => format!("`{}`", arg.get_id().as_str()),
            }
        };

        // Column 2: enum values or placeholder; empty for boolean flags.
        let value_col = if is_flag(arg.get_action()) {
            String::new()
        } else {
            let pvs = possible_values(arg);
            if !pvs.is_empty() {
                pvs.iter()
                    .map(|v| format!("`{v}`"))
                    .collect::<Vec<_>>()
                    .join(" \\| ")
            } else if arg.is_positional() {
                // The first column already contains the placeholder.
                String::new()
            } else {
                value_placeholder(arg)
            }
        };

        // Column 3: default value.
        let default_col = arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", ");
        let default_col = if default_col.is_empty() {
            String::new()
        } else {
            format!("`{default_col}`")
        };

        // Column 4: help text and repeatability marker.
        // Avoid duplicating an explicit repeatability marker already in the help.
        // Append the standard marker only to unmarked Append actions.
        let mut help = arg.get_help().map(plain).unwrap_or_default();
        let marker = match lang {
            Lang::Ko => "반복 가능",
            Lang::En => "repeatable",
        };
        if matches!(arg.get_action(), ArgAction::Append) && !help.contains(marker) {
            if help.is_empty() {
                help = format!("({marker})");
            } else {
                help.push_str(&format!(" ({marker})"));
            }
        }
        let help_col = cell(&help);

        rows.push(format!(
            "| {name_col} | {value_col} | {default_col} | {help_col} |"
        ));
    }
    rows
}

/// Generate the complete Markdown reference from clap definitions.
fn generate(lang: Lang) -> String {
    let root = i18n::localize(Cli::command(), lang);
    // Flatten exposed subcommands (excluding hidden ones) in declaration order — nested
    // subcommands (`export` under `skill`) also appear exactly once, with the full path ("skill export").
    let mut subs: Vec<(String, &Command)> = Vec::new();
    fn flatten<'a>(cmd: &'a Command, path: String, subs: &mut Vec<(String, &'a Command)>) {
        subs.push((path.clone(), cmd));
        for sub in cmd.get_subcommands().filter(|c| !c.is_hide_set()) {
            flatten(sub, format!("{path} {}", sub.get_name()), subs);
        }
    }
    for sub in root.get_subcommands().filter(|c| !c.is_hide_set()) {
        flatten(sub, sub.get_name().to_string(), &mut subs);
    }

    let mut out = String::new();
    match lang {
        Lang::Ko => {
            out.push_str(HEADER_COMMENT_KO);
            out.push_str(
                "\n\n[한국어](cli-reference.ko.md) · [English](cli-reference.md)\n\n\
                 # ahwp CLI 명령 레퍼런스\n\n",
            );
            out.push_str(
                "이 문서는 `ahwp` CLI의 clap 정의에서 자동 생성된다. 직접 편집하지 말고, 명령·플래그가 \
                 바뀌면 `HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference`로 재생성하라 — \
                 CI 테스트가 코드와 문서의 동기화를 강제한다.\n\n",
            );
            out.push_str("## 명령 색인\n\n");
        }
        Lang::En => {
            out.push_str(HEADER_COMMENT_EN);
            out.push_str(
                "\n\n[한국어](cli-reference.ko.md) · [English](cli-reference.md)\n\n\
                 # ahwp CLI command reference\n\n",
            );
            out.push_str(
                "This document is generated from the clap definitions of the `ahwp` CLI. Do not edit \
                 it by hand: when a command or flag changes, regenerate it with \
                 `HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference`. A CI test enforces \
                 that it stays in sync with the code.\n\n",
            );
            out.push_str("## Command index\n\n");
        }
    }
    for (name, _) in &subs {
        out.push_str(&format!("- [`ahwp {name}`](#{})\n", anchor(name)));
    }
    out.push('\n');

    // Per-command sections.
    for (name, sub) in &subs {
        out.push_str(&format!("## `ahwp {name}`\n\n"));

        // Prefer long_about over about.
        let about = sub
            .get_long_about()
            .or_else(|| sub.get_about())
            .map(plain)
            .unwrap_or_default();
        if !about.is_empty() {
            out.push_str(&about);
            out.push_str("\n\n");
        }

        // Usage.
        let usage_label = match lang {
            Lang::Ko => "사용법",
            Lang::En => "Usage",
        };
        out.push_str(&format!(
            "**{usage_label}:** `{}`\n\n",
            usage_line(sub, name)
        ));

        // Argument/flag table.
        let rows = arg_rows(sub, lang);
        if rows.is_empty() {
            out.push_str(match lang {
                Lang::Ko => "_인자·플래그 없음_\n\n",
                Lang::En => "_No arguments or flags_\n\n",
            });
        } else {
            out.push_str(match lang {
                Lang::Ko => "| 인자/플래그 | 값 | 기본값 | 설명 |\n",
                Lang::En => "| Argument/flag | Value | Default | Description |\n",
            });
            out.push_str("|---|---|---|---|\n");
            for r in rows {
                out.push_str(&r);
                out.push('\n');
            }
            out.push('\n');
        }
    }

    // Normalize to one final newline.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

fn check_language(lang: Lang) {
    let generated = generate(lang);
    let path = doc_path(lang);

    // Bless mode writes the generated files.
    if std::env::var_os("HWP_UPDATE_DOCS").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("docs/manual 디렉터리 생성");
        }
        std::fs::write(&path, &generated).expect("cli-reference 쓰기");
        eprintln!("cli-reference 재생성 완료: {}", path.display());
        return;
    }

    // Verification compares committed content, normalizing CRLF for Windows.
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cli-reference를 읽을 수 없음({e}) — \
             `HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference`로 최초 생성하라: {}",
            path.display()
        )
    });
    let committed = committed.replace("\r\n", "\n");

    assert_eq!(
        committed, generated,
        "\nCLI 정의가 문서와 어긋남 — \
         `HWP_UPDATE_DOCS=1 cargo test -p agent-hwp --test cli_reference`로 재생성한 뒤 \
         diff를 확인해 커밋하라."
    );
}

#[test]
fn cli_reference_up_to_date() {
    check_language(Lang::Ko);
    check_language(Lang::En);
}

/// Gate missing Korean overlay entries.
///
/// Missing entries silently leave English help, so require every new command and flag
/// to have a corresponding localization entry.
#[test]
fn korean_overlay_covers_every_command_and_argument() {
    fn ids(command: &Command, path: &str, missing: &mut Vec<String>) {
        let has = |arg: &str| {
            hwp_cli::i18n::KO
                .iter()
                .any(|(cmd, id, _)| *cmd == path && *id == arg)
        };
        if !has("") {
            missing.push(format!("about: {path:?}"));
        }
        for arg in command.get_arguments() {
            let id = arg.get_id().as_str();
            if matches!(id, "help" | "version") {
                continue;
            }
            if !has(id) {
                missing.push(format!("{path:?} / {id}"));
            }
        }
        for sub in command.get_subcommands() {
            ids(sub, sub.get_name(), missing);
        }
    }

    let mut missing = Vec::new();
    ids(&Cli::command(), "", &mut missing);
    assert!(
        missing.is_empty(),
        "i18n 한국어 표에 없는 항목 — crates/hwp-cli/src/i18n.rs 의 KO 에 추가하라:\n{}",
        missing.join("\n")
    );
}

/// Reject localization entries for commands or arguments that no longer exist.
#[test]
fn korean_overlay_has_no_stale_entries() {
    fn exists(command: &Command, path: &str, arg: &str) -> bool {
        // Nested subcommands are looked up by bare name regardless of depth (same keying as translate).
        fn find<'a>(cmd: &'a Command, name: &str) -> Option<&'a Command> {
            cmd.get_subcommands()
                .find(|c| c.get_name() == name)
                .or_else(|| cmd.get_subcommands().find_map(|c| find(c, name)))
        }
        let target = if path.is_empty() {
            Some(command)
        } else {
            find(command, path)
        };
        match target {
            Some(cmd) => arg.is_empty() || cmd.get_arguments().any(|a| a.get_id().as_str() == arg),
            None => false,
        }
    }

    let root = Cli::command();
    let stale: Vec<_> = hwp_cli::i18n::KO
        .iter()
        .filter(|(cmd, id, _)| !exists(&root, cmd, id))
        .map(|(cmd, id, _)| format!("{cmd:?} / {id}"))
        .collect();
    assert!(
        stale.is_empty(),
        "i18n 표의 죽은 항목:\n{}",
        stale.join("\n")
    );
}
