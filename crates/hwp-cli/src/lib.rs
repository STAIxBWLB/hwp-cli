//! hwp-cli 라이브러리 표면 — CLI 정의만 노출한다.
//!
//! 명령/플래그 선언(`cli`)을 lib으로 올려 두면 `tests/cli_reference.rs`가
//! `clap::CommandFactory`로 명령 트리를 introspect해 문서를 자동 생성할 수 있다.
//! 실제 디스패치·명령 구현(`commands`, `format`)은 bin 전용이라 여기서 노출하지 않는다.

pub mod asset_snapshot;
pub mod certification;
pub mod cli;
pub mod document_spec;
pub mod document_spec_v2;
pub mod template_spec;
