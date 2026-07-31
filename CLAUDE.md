# CLAUDE.md

HWP 5.0(바이너리)·HWPX(OWPML)를 외부 HWP 라이브러리 없이 **직접 구현**하는 Rust 워크스페이스.
코드 주석·경고 문구는 한국어를 기본으로 한다. 커밋 메시지는 한/영 무관.

## 문서 정책 — 한/영 페어

- 사용자용 문서는 **한국어 정본 + 영문 페어**로 둔다: `NAME.md`(한국어)와 `NAME.en.md`(영문).
- 두 파일 모두 **첫 줄에 언어 링크**를 둔다: `[한국어](NAME.md) · [English](NAME.en.md)`
  (README는 `[한국어](README.md) · [English](README.en.md)`).
- 한쪽만 고치지 않는다. 내용이 바뀌면 **같은 커밋에서 양쪽을 갱신**한다.
- 릴리스 노트는 한/영 병기한다. 정본은 `CHANGELOG.md`(섹션별 한국어 다음 영문)이고, 릴리스 본문은
  이 파일에서 뽑는다(`scripts/release_notes.sh`).
- **예외**: `docs/manual/cli-reference.md`는 clap 정의에서 자동 생성되며 내용이 CLI 자체 도움말
  (한국어)이다. 손번역 페어를 두면 즉시 드리프트하므로 만들지 않는다. 영문이 필요하면 CLI의
  help 문자열부터 이중화해야 한다.
- 내부 작업 문서(`CLAUDE.md`, `TODO.md`의 감사 로그 등)는 페어 대상이 아니다.

## 빌드 · 테스트

```bash
cargo build                    # 디버그 빌드 (bin: hwp)
scripts/check.sh               # 로컬 CI 미러 = CI 3종 게이트 (fmt+clippy+test, PR 전 필수)
HWP_FONT_DIR=$PWD/fonts python3 tools/diagnostic_corpus.py   # 진단 코퍼스 + 자체 검증 하네스
```

- CI 게이트(`.github/workflows/ci.yml`)와 로컬은 **반드시 같은 커맨드**를 쓴다:
  `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo test --workspace`.
  부분 실행(clippy만, test만)은 `scripts/check.sh` 대신 직접 항을 골라 실행한다.
- Rust edition 2024, rust-version 1.93.
- 폰트: 저장소에 동봉 폰트는 **없다**(`/fonts/`는 gitignore — 로컬에 HCR바탕·돋움을 받아두면
  진단 코퍼스·골든 대조가 사용). CI 렌더 글리프는 시스템 폰트(ubuntu는 noto-cjk 설치, macOS 기본
  CJK)에서 오므로, CI에서 도는 테스트는 폰트 의존 단언(글리프·페이지수)을 하면 안 된다.
- `HWP_GOLDEN=1` — 한글 기준 PNG와의 골든 렌더 대조(옵트인). `HWP_CORPUS_DIR` — 대형 야생 corpus 소크 테스트.

## 브랜치 · PR 정책

- 기능추가·수정·문서 등 **모든 작업은 별도 브랜치**에서 한다: `feat/<주제>`·`fix/<주제>`·`docs/<주제>`.
  main 직접 push 금지.
- 정본 저장소는 `STAIxBWLB/hwp-cli`(= origin)다. 브랜치를 origin에 push하고 **origin의
  main을 대상으로 PR**을 연다. (과거의 포크→upstream 구도는 폐지됨.)
- PR로 제출하고, **CI green(ubuntu+macOS 필수)을 확인한 뒤 squash 머지**한다(머지 커밋 제목의
  `(#N)` 관례 유지). Windows 잡은 참고용(비차단 — 잡은 빨갛게 보여도 워크플로는 통과).
- PR 전 로컬 게이트는 `scripts/check.sh` — CI와 동일 3커맨드(fmt → clippy --all-targets
  -D warnings → test). 이걸 통과 못 하면 PR하지 않는다(머지 후 CI 실패의 원천 차단).

## 데이터 정책 (중요)

- `fixtures/hwp5/*.hwp`·`fixtures/hwpx/*.hwpx`는 gitignore(로컬 전용). 없으면 테스트가 skip될 뿐 실패하지 않는다. 출처는 `fixtures/README.md`.
- `fixtures/samples/`는 **예외적으로 커밋한다** — 소유자 자작 문서를 대학명 가명 치환한 테스트 샘플만 둔다(익명화 레시피는 `fixtures/README.md`). 원본은 커밋 금지.
- **정답지 코퍼스(`~/Documents/hwp_samples` 등 정품 한글 파일)는 절대 커밋 금지.**
- **한컴 스펙 문서·파생물(추출 텍스트, 페이지 캡처) 커밋 금지** — `docs/README.md` 참조. 스펙은 섹션 번호로만 인용한다(예: `한글문서파일형식 5.0 §4.2.6`). 로컬 `docs/spec.txt`(gitignore)는 작업 참고용.

## 설계 지식은 docs/design/ 에 있다

- 시작점: [docs/design/00-overview.md](docs/design/00-overview.md) (문서 색인·설계 원칙)
- **필독**: [07-hangul-compat-rules.md](docs/design/07-hangul-compat-rules.md) — 실기로만 확정된 한글 호환 규칙 카탈로그. 이 규칙을 모르고 writer를 고치면 한글에서 파일이 깨진다.
- 포맷 전수 지도: [10-hwp5-structure-map.md](docs/design/10-hwp5-structure-map.md)(레코드/컨트롤 카탈로그), [11-hwpx-structure-map.md](docs/design/11-hwpx-structure-map.md)(OWPML 요소 카탈로그)
- 미구현 기능은 [12-feature-gaps.md](docs/design/12-feature-gaps.md)에서 먼저 확인.

## 불변식 (어기면 안 됨)

1. **hwp-model은 다른 내부 크레이트에 의존하지 않는다** (허브-스포크). `hwp5`↔`hwpx`도 서로 의존하지 않고 IR을 경유한다.
2. **무손실 왕복 게이트**: hwp5→hwp5 identity 재직렬화는 바이트 동일이어야 한다(`crates/hwp5/tests/identity.rs`). 모르는 레코드는 버리지 말고 `OpaqueRecord`로 보존.
3. **정답지 방법론 — 추측 금지**: 포맷 동작은 한글이 저장한 정품 파일 바이트와의 대조로만 확정한다. 최종 판정은 한글(한컴오피스) 실기에서 열리는지 여부다.
4. 새 HWP 관련 외부 크레이트 추가 금지(인프라 크레이트 cfb/zip/quick-xml/tiny-skia 등만 허용).
