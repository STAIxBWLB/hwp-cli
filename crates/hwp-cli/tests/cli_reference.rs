//! Generates the CLI command reference and gates it against drift (both languages).
//!
//! Introspects the `Cli` command tree via `clap::CommandFactory` and deterministically renders
//! `docs/manual/cli-reference.md` (English, canonical) and `cli-reference.ko.md` (Korean). The
//! test fails when the committed docs and the regenerated ones diverge, which forces a doc
//! update whenever the CLI definition (flags, help text) changes.
//!
//! Bless: `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`.

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

const HEADER_COMMENT_KO: &str = "<!-- 자동 생성 문서 — 수동 편집 금지. 재생성: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference -->";
const HEADER_COMMENT_EN: &str = "<!-- Generated document. Do not edit by hand. Regenerate with: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference -->";

/// StyledStr → 순수 텍스트(ANSI 없음), 앞뒤 공백 제거.
fn plain(s: &StyledStr) -> String {
    s.to_string().trim().to_string()
}

/// 표 셀 안전 이스케이프: 개행→공백(연속 공백 축약), `|`→`\|`.
fn cell(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.replace('|', "\\|")
}

/// GitHub anchor: `hwp skill export` → `hwp-skill-export` (spaces become hyphens).
fn anchor(name: &str) -> String {
    format!("hwp-{}", name.replace(' ', "-"))
}

/// 값을 갖지 않는 액션(불리언 플래그 등)인지.
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

/// 문서 생성에서 제외할 인자(clap 자동 추가 help/version, 숨김 인자).
fn skip_arg(arg: &Arg) -> bool {
    arg.is_hide_set()
        || matches!(
            arg.get_action(),
            ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version
        )
        || matches!(arg.get_id().as_str(), "help" | "version")
}

/// 인자의 값 이름 placeholder(예: `<OUTPUT>`). value_name 없으면 id를 대문자화.
fn value_placeholder(arg: &Arg) -> String {
    let name = arg
        .get_value_names()
        .and_then(|ns| ns.first())
        .map(|s| s.to_string())
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());
    format!("`<{name}>`")
}

/// value_enum의 노출 가능한 값 목록(선언 순서). enum이 아니면 빈 Vec.
fn possible_values(arg: &Arg) -> Vec<String> {
    arg.get_possible_values()
        .iter()
        .filter(|pv| !pv.is_hide_set())
        .map(|pv| pv.get_name().to_string())
        .collect()
}

/// `render_usage()`를 `hwp <name> …` 형태로 정규화한다.
/// (서브커맨드를 부모에서 꺼내면 프로그램명이 없어 `Usage: <name> …`로 나올 수 있다 —
/// 첫 `<name>` 토큰 뒤 본문만 취해 `hwp <name> <본문>`으로 재조립한다.)
fn usage_line(sub: &Command, path: &str) -> String {
    let raw = sub.clone().render_usage().to_string();
    let after_label = raw
        .trim()
        .strip_prefix("Usage:")
        .map(str::trim)
        .unwrap_or_else(|| raw.trim());
    // Take the body after the bare name (last path token) and reassemble it with the
    // full path — nested subcommands (`export`) normalize to `hwp skill export …` too.
    let bare = path.rsplit(' ').next().unwrap_or(path);
    let body = match after_label.split_once(bare) {
        Some((_, rest)) => rest.trim_start(),
        None => "",
    };
    if body.is_empty() {
        format!("hwp {path}")
    } else {
        format!("hwp {path} {body}")
    }
}

/// 한 서브커맨드의 인자/플래그 표 행들. 선언 순서 유지.
fn arg_rows(sub: &Command, lang: Lang) -> Vec<String> {
    let mut rows = Vec::new();
    for arg in sub.get_arguments() {
        if skip_arg(arg) {
            continue;
        }
        // 1열: 인자/플래그 이름.
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

        // 2열: 값 (enum 값 목록 또는 placeholder; 불리언 플래그는 빈칸).
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
                // placeholder는 이미 1열에 있으므로 중복 표기하지 않는다.
                String::new()
            } else {
                value_placeholder(arg)
            }
        };

        // 3열: 기본값.
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

        // 4열: 설명 (help) + 반복 가능 표기.
        // help가 이미 "반복 가능"을 담고 있으면(doc comment 관례) 중복을 피한다 —
        // Append인데 표기가 없는 플래그에만 표준 마커를 덧붙여 일관성을 맞춘다.
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

/// clap 정의에서 마크다운 레퍼런스 전문을 생성한다.
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
                 # hwp CLI 명령 레퍼런스\n\n",
            );
            out.push_str(
                "이 문서는 `hwp` CLI의 clap 정의에서 자동 생성된다. 직접 편집하지 말고, 명령·플래그가 \
                 바뀌면 `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`로 재생성하라 — \
                 CI 테스트가 코드와 문서의 동기화를 강제한다.\n\n",
            );
            out.push_str("## 명령 색인\n\n");
        }
        Lang::En => {
            out.push_str(HEADER_COMMENT_EN);
            out.push_str(
                "\n\n[한국어](cli-reference.ko.md) · [English](cli-reference.md)\n\n\
                 # hwp CLI command reference\n\n",
            );
            out.push_str(
                "This document is generated from the clap definitions of the `hwp` CLI. Do not edit \
                 it by hand: when a command or flag changes, regenerate it with \
                 `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`. A CI test enforces \
                 that it stays in sync with the code.\n\n",
            );
            out.push_str("## Command index\n\n");
        }
    }
    for (name, _) in &subs {
        out.push_str(&format!("- [`hwp {name}`](#{})\n", anchor(name)));
    }
    out.push('\n');

    // 명령별 섹션.
    for (name, sub) in &subs {
        out.push_str(&format!("## `hwp {name}`\n\n"));

        // about / long_about (long_about 우선).
        let about = sub
            .get_long_about()
            .or_else(|| sub.get_about())
            .map(plain)
            .unwrap_or_default();
        if !about.is_empty() {
            out.push_str(&about);
            out.push_str("\n\n");
        }

        // 사용법.
        let usage_label = match lang {
            Lang::Ko => "사용법",
            Lang::En => "Usage",
        };
        out.push_str(&format!(
            "**{usage_label}:** `{}`\n\n",
            usage_line(sub, name)
        ));

        // 인자/플래그 표.
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

    // 파일 끝 개행 1개로 정규화.
    while out.ends_with('\n') {
        out.pop();
    }
    out.push('\n');
    out
}

fn check_language(lang: Lang) {
    let generated = generate(lang);
    let path = doc_path(lang);

    // bless 모드: 파일을 새로 쓰고 통과.
    if std::env::var_os("HWP_UPDATE_DOCS").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("docs/manual 디렉터리 생성");
        }
        std::fs::write(&path, &generated).expect("cli-reference 쓰기");
        eprintln!("cli-reference 재생성 완료: {}", path.display());
        return;
    }

    // 검증 모드: 커밋본과 비교(Windows CI 대비 CRLF 정규화).
    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cli-reference를 읽을 수 없음({e}) — \
             `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`로 최초 생성하라: {}",
            path.display()
        )
    });
    let committed = committed.replace("\r\n", "\n");

    assert_eq!(
        committed, generated,
        "\nCLI 정의가 문서와 어긋남 — \
         `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`로 재생성한 뒤 \
         diff를 확인해 커밋하라."
    );
}

#[test]
fn cli_reference_up_to_date() {
    check_language(Lang::Ko);
    check_language(Lang::En);
}

/// 한국어 오버레이 누락 게이트.
///
/// 표에 항목이 없으면 그 도움말만 조용히 영문으로 남는다. 새 명령·플래그를 추가하고
/// i18n 표를 잊는 드리프트를 여기서 막는다.
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

/// 표에 죽은 항목(더 이상 존재하지 않는 명령·인자)이 남지 않게 한다.
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
