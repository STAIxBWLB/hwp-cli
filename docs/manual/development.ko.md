[한국어](development.ko.md) · [English](development.md)

# 개발

크레이트 구성과 빌드·테스트 방법을 다룬다. 기여자를 위한 페이지다. PR을 막는 규칙은 [AGENTS.md](../../AGENTS.md)에 있다.

## 워크스페이스 둘러보기

| 크레이트 | 역할 |
|---|---|
| `hwp-model` | 공유 문서 모델(IR). 모든 크레이트가 의존하는 단일 계약, 무손실 보존(opaque/tail), 단위 변환 |
| `hwp5` | HWP 5.0 바이너리 reader/writer (CFB 컨테이너 + 레코드 스트림 + 압축) |
| `hwpx` | HWPX reader/writer (ZIP 패키지 + OWPML XML) |
| `hwp-convert` | IR ↔ markdown / HTML / JSON, 인메모리 편집, 필드 스캔 |
| `hwp-render` | IR → PNG / SVG / PDF / JPEG / WebP 렌더러와 배치 좌표, 줄 배치 합성, 셰이핑, 폰트 서브셋·임베드, 렌더 diff |
| `agent-hwp` | `ahwp` 바이너리 (CLI + MCP 서버) |

이 크레이트들과 MCP 배포 경로를 담은 대화형 아키텍처 지도는
[docs/architecture/](../architecture/)에 있다. `hwp-cli-rendered.html`을 브라우저로 연다.

## 빌드와 테스트

```sh
cargo build --all-targets
scripts/check.sh     # 로컬 CI 미러: fmt + clippy + test + 구조 코퍼스 (PR 전 필수)
```

CI(`.github/workflows/ci.yml`)는 플랫폼 무관 게이트(`cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `scripts/check-structured-corpus.sh`)를
ubuntu `lint` 잡에서 한 번만 실행하고, `cargo test --workspace`는 **ubuntu + macOS + windows**
(모두 필수)에서 실행한다. ubuntu 테스트 잡은 `fonts-nanum`(glyf TTF. CFF `fonts-noto-cjk`는 디버그
빌드 렌더링을 ~100배 느리게 만들었다)를 설치한다. 로컬 미러 `scripts/check.sh`는 같은 게이트를 실행한다.

테스트는 hwp5 바이트 동일 왕복(identity/roundtrip/synth), hwpx 의미 동등 왕복, IR JSON·markdown 왕복,
편집·필드 보정, 렌더 레이아웃·표·diff 메트릭, 구조 코퍼스 결정성을 포함한다. 정품 fixture는 저장소에
없으며 없으면 해당 테스트는 실패가 아니라 skip된다.

한글(한컴오피스) 실기가 필요해 자동 게이트 바깥에 두는 점검표가 두 개 있다. 작성한 파일을 실기에서
검증하는 [docs/hancom-verification-checklist.ko.md](../hancom-verification-checklist.ko.md)와
릴리스 직전 게이트인 [docs/release-readiness.ko.md](../release-readiness.ko.md)다.
