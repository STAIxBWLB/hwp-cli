pub mod bookmarks;
pub mod cat;
pub mod certify;
pub mod compose;
pub mod convert;
pub mod corpus;
pub mod diff;
pub mod dump;
pub mod edit;
pub mod fields;
pub mod fill;
pub mod grep;
pub mod info;
pub mod mcp;
pub mod new;
pub(crate) mod output;
pub mod render;
pub mod skill;
pub mod slots;
pub mod template;
pub mod update;
pub mod validate;

/// 구조 문서 writer의 보존 불가 경고를 게시 전에 거부한다.
///
/// `DROP:`은 단순 진단이 아니라 요청한 문서 내용이 산출물에서 사라졌다는 뜻이다.
/// `new`/`edit`처럼 작성 결과가 정본이 되는 명령은 parseable한 파일이라도 게시하지
/// 않는다. 손실 변환이 본래 목적일 수 있는 markdown/pdf 등은 이 함수를 호출하지 않는다.
pub(crate) fn reject_drop_warnings(context: &str, warnings: &[String]) -> anyhow::Result<()> {
    let drops: Vec<&str> = warnings
        .iter()
        .filter(|warning| warning.starts_with("DROP: "))
        .map(String::as_str)
        .collect();
    if drops.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "{context}: 보존 불가 데이터 {}건을 드롭할 수 없어 출력을 게시하지 않습니다\n{}",
        drops.len(),
        drops
            .iter()
            .map(|warning| format!("  - {}", warning.trim_start_matches("DROP: ")))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
