//! `hwp cat` — 텍스트 추출.
//!
//! 본문 파싱 기반 추출(plain/markdown/json)과 `--preview`(PrvText)를
//! 지원한다. 미리보기는 컨테이너 계층만 사용하므로 본문 파싱이 실패하는
//! 파일의 폴백으로도 쓰인다.

use std::fmt;
use std::io::BufRead as _;
use std::path::Path;

use hwp_model::Document;
use zeroize::Zeroizing;

use crate::format::{FileFormat, detect};
use hwp_cli::cli::{PasswordArgs, TextFormat};

/// Stable public refusal code for absent or invalid password credentials.
pub const HWP_PASSWORD_REQUIRED_OR_INVALID: &str = "HWP_PASSWORD_REQUIRED_OR_INVALID";

/// A password whose backing string is zeroed when it leaves the command scope.
/// It deliberately exposes only an exact borrowed view to the format readers.
pub struct ResolvedPassword(Zeroizing<String>);

impl ResolvedPassword {
    fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Per-load input shared by the CLI surfaces as they gain password support.
#[derive(Clone, Copy, Default)]
pub struct LoadOptions<'a> {
    pub password: Option<&'a ResolvedPassword>,
}

/// Refusal metadata is intentionally bounded to public format/profile stage
/// information. Its displayed form never includes credential or parser data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordRefusal {
    format: FileFormat,
    algorithm: &'static str,
    stage: &'static str,
}

impl PasswordRefusal {
    fn hwp5() -> Self {
        Self {
            format: FileFormat::Hwp5,
            algorithm: "HWP5-EncryptVersion4",
            stage: "credential-validation",
        }
    }

    fn hwpx() -> Self {
        Self {
            format: FileFormat::Hwpx,
            algorithm: "AES256-CBC/PBKDF2-HMAC-SHA1",
            stage: "credential-validation",
        }
    }
}

impl fmt::Display for PasswordRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = (self.format, self.algorithm, self.stage);
        write!(
            formatter,
            "{HWP_PASSWORD_REQUIRED_OR_INVALID}: 암호화된 문서는 지원하지 않습니다. 한글에서 암호를 해제한 뒤 다시 저장하세요."
        )
    }
}

impl std::error::Error for PasswordRefusal {}

/// Shared document-loader failure surface. Password failures are deliberately
/// separated so callers can preserve one public absent/wrong credential code.
#[derive(Debug)]
pub enum LoadDocumentError {
    Password(PasswordRefusal),
    Other(anyhow::Error),
}

impl fmt::Display for LoadDocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password(error) => error.fmt(formatter),
            Self::Other(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LoadDocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Password(error) => Some(error),
            Self::Other(error) => Some(error.root_cause()),
        }
    }
}

impl From<anyhow::Error> for LoadDocumentError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

/// Resolves command-local password input before any document read. Only a
/// final LF or CRLF is removed; all other bytes stay untouched.
pub fn resolve_password_args(
    args: PasswordArgs,
    file: &Path,
) -> anyhow::Result<Option<ResolvedPassword>> {
    if args.password.is_some() && args.password_stdin {
        anyhow::bail!("--password와 --password-stdin은 함께 사용할 수 없습니다");
    }
    if args.password_stdin && file == Path::new("-") {
        anyhow::bail!("문서 입력과 --password-stdin은 모두 표준 입력을 사용할 수 없습니다");
    }
    if let Some(password) = args.password {
        return Ok(Some(ResolvedPassword::new(password)));
    }
    if !args.password_stdin {
        return Ok(None);
    }

    let mut line = Zeroizing::new(String::new());
    std::io::stdin().lock().read_line(&mut line)?;
    let line_len = line.len();
    if line.ends_with("\r\n") {
        line.truncate(line_len - 2);
    } else if line.ends_with('\n') {
        line.truncate(line_len - 1);
    }
    Ok(Some(ResolvedPassword(line)))
}

/// 포맷을 감지해 IR로 읽는다 (cat/convert/render 공용).
///
/// `.json` 입력은 IR 직렬화본으로 보고 역직렬화한다(편집 왕복 경로) — 그 외는
/// 매직 바이트로 hwp5/hwpx를 판별한다.
pub fn load_document(path: &Path) -> anyhow::Result<Document> {
    load_document_with_options(path, &LoadOptions::default()).map_err(anyhow::Error::new)
}

/// Reads a document through the shared options-aware loader.
pub fn load_document_with_options(
    path: &Path,
    options: &LoadOptions<'_>,
) -> Result<Document, LoadDocumentError> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        let text = std::fs::read_to_string(path).map_err(anyhow::Error::new)?;
        return hwp_convert::from_json(&text)
            .map_err(|e| anyhow::anyhow!("JSON IR 파싱 실패 ({}): {e}", path.display()))
            .map_err(Into::into);
    }
    match detect(path)? {
        FileFormat::Hwp5 => {
            let result = hwp5::read_document_with_options(
                path,
                &hwp5::ReadOptions {
                    password: options.password.map(ResolvedPassword::as_str),
                },
            )
            .map_err(|error| match error {
                hwp5::Hwp5Error::Encrypted => LoadDocumentError::Password(PasswordRefusal::hwp5()),
                other => LoadDocumentError::Other(anyhow::Error::new(other)),
            })?;
            for w in &result.warnings {
                eprintln!("경고: {w}");
            }
            if result.unwrapped_distribution {
                // The hwp5 writer synthesizes a fixed attribute DWORD (0x1,
                // compression only) for every IR-built output, so the
                // distribution bit never survives a cross-format write. This is
                // pre-existing writer behaviour GATE-01 made newly reachable,
                // not a regression. Measured on a genuine dist-*.hwp
                // (2026-08-20, plan 02-04 Task 0): full synthesis strips the
                // bit; `convert --to hwp` is a pure byte copy that keeps it;
                // the source-preserving edit path fails closed on a
                // /ViewText-only document instead of writing anything. Hence
                // the caveat names conversion only, and lives folded into this
                // ONE line (D-10a) at load - the single choke point every
                // command shares - because the write paths are many.
                eprintln!(
                    "정보: 배포용 문서를 해제했습니다 \
                     (다른 형식으로 변환해 저장하면 배포용 보호는 유지되지 않습니다)"
                );
            }
            Ok(result.document)
        }
        FileFormat::Hwpx => {
            let result = hwpx::read_document_with_options(
                path,
                &hwpx::ReadOptions {
                    password: options.password.map(ResolvedPassword::as_str),
                },
            )
            .map_err(|error| match error {
                hwpx::HwpxError::Encrypted => LoadDocumentError::Password(PasswordRefusal::hwpx()),
                other => LoadDocumentError::Other(anyhow::Error::new(other)),
            })?;
            for w in &result.warnings {
                eprintln!("경고: {w}");
            }
            Ok(result.document)
        }
    }
}

/// 본문 텍스트 추출.
///
/// `preview`면 본문 파싱 없이 PrvText 미리보기만 출력한다. `with_header_footer`/`with_hidden`은
/// 머리말·꼬리말/숨은 설명 포함 여부(기본 제외) — plain·markdown 경로에 일관되게 적용된다
/// (html/json은 옵션 미대상). `with_segments`는 markdown 전용으로, markdown과 함께 각 출력
/// 문자 범위의 원본 좌표를 한 줄 JSON 봉투로 낸다.
pub fn run(
    path: &Path,
    format: TextFormat,
    preview: bool,
    with_header_footer: bool,
    with_hidden: bool,
    with_segments: bool,
    password_args: PasswordArgs,
) -> anyhow::Result<()> {
    if with_segments {
        if preview {
            anyhow::bail!(
                "--with-segments는 --format markdown 전용입니다 (--preview와 함께 쓸 수 없습니다)"
            );
        }
        if !matches!(format, TextFormat::Markdown) {
            anyhow::bail!("--with-segments는 --format markdown 전용입니다");
        }
    }
    let password = resolve_password_args(password_args, path)?;
    if preview {
        return self::preview(path);
    }

    let doc = load_document_with_options(
        path,
        &LoadOptions {
            password: password.as_ref(),
        },
    )
    .map_err(anyhow::Error::new)?;
    let opts = hwp_model::TextOptions {
        include_header_footer: with_header_footer,
        include_hidden: with_hidden,
    };
    let md_opts = || hwp_convert::MarkdownOptions {
        text: hwp_model::TextOptions {
            include_header_footer: with_header_footer,
            include_hidden: with_hidden,
        },
        ..Default::default()
    };
    match format {
        TextFormat::Plain => print!("{}", doc.plain_text_with(&opts)),
        TextFormat::Markdown if with_segments => {
            let (markdown, segments) = hwp_convert::to_markdown_with_segments(&doc, &md_opts())?;
            // 한 줄 컴팩트 JSON 봉투 + 개행. kind는 현재 항상 "para"(미래 확장용).
            let segments: Vec<serde_json::Value> = segments
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "kind": "para",
                        "section": s.section,
                        "para": s.para,
                        "start": s.start,
                        "end": s.end,
                    })
                })
                .collect();
            let envelope = serde_json::json!({
                "markdown": markdown,
                "segments": segments,
            });
            println!("{}", serde_json::to_string(&envelope)?);
        }
        TextFormat::Markdown => print!("{}", hwp_convert::to_markdown_with(&doc, &md_opts())?),
        TextFormat::Html => print!("{}", hwp_convert::to_html(&doc)),
        TextFormat::Json => println!("{}", hwp_convert::to_json(&doc, true, false)?),
        TextFormat::Csv => print!("{}", hwp_convert::to_csv(&doc)),
    }
    Ok(())
}

pub fn preview(path: &Path) -> anyhow::Result<()> {
    let text = match detect(path)? {
        FileFormat::Hwp5 => {
            let mut container = hwp5::Hwp5Container::open(path)?;
            let raw = container.read_stream_raw("/PrvText")?;
            decode_utf16le(&raw)
        }
        FileFormat::Hwpx => {
            let mut pkg = hwpx::HwpxPackage::open(path)?;
            let raw = pkg.read_entry("Preview/PrvText.txt")?;
            // HWPX 미리보기는 보통 UTF-8이지만 UTF-16LE인 경우도 방어
            if raw.iter().take(64).any(|&b| b == 0) {
                decode_utf16le(&raw)
            } else {
                String::from_utf8_lossy(&raw).into_owned()
            }
        }
    };
    println!("{text}");
    Ok(())
}

fn decode_utf16le(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    // 후행 NUL 제거 후 손실 허용 디코드
    let end = units.iter().rposition(|&u| u != 0).map_or(0, |i| i + 1);
    String::from_utf16_lossy(&units[..end])
}
