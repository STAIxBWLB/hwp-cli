[한국어](CHANGELOG.md) · [English](CHANGELOG.md#english)

# 변경 이력 / Changelog

릴리스 노트는 한국어와 영문을 병기한다. 이 파일이 정본이며, GitHub Release 본문은
`scripts/release_notes.sh <version>`이 여기서 해당 버전 절을 그대로 뽑아 쓴다.

버전은 워크스페이스 `Cargo.toml`의 `[workspace.package] version`이 단일 기준이다.

Release notes are written in Korean and English together. This file is the source of truth, and
`scripts/release_notes.sh <version>` extracts the section for a version verbatim as the GitHub
Release body.

---

## [0.5.0]

### 한국어

**추가**

- 구조 문서 저작 경로. DocumentSpec v1/v2와 TemplateSpec/Data v1 명세에서 HWP/HWPX를 결정론적으로
  합성하는 `compose`·`template` 명령을 추가했다. 문자열 보간이나 표현식 실행 없이 typed AST만 허용한다.
- 네이티브 전용 인증 `certify`. 폰트 identity 고정, 폰트 대체·macro·external reference 금지,
  전체 페이지 렌더, bounds/collision/unresolved-field 실패를 강제하고 리포트를 원자적으로 게시한다.
- 구조 코퍼스 게이트 `corpus`. 자체 작성 한국어 7종 문서를 HWPX/HWP로 2회씩 생성해 문서 바이트·의미
  통계·페이지 PNG 해시·render issue 해시·폰트 identity가 모두 일치할 때만 통과시킨다.
- 공개 JSON Schema(`schemas/`)와 예제(`examples/`), 설계 문서 13~17.
- MCP 도구 3종 추가(`hwp_compose`·`hwp_template`·`hwp_certify`), 총 15종.
- CLI 도움말 다국어. 기본은 영문이고 로케일이 한국어면 한국어로 표시하며, `--lang <en|ko>`나
  `HWP_LANG`으로 명시 지정할 수 있다. CLI 레퍼런스도 두 언어로 자동 생성한다.

**수정**

- Windows 빌드 복구. nightly 전용 `windows_by_handle` API를 쓰던 파일 신원·링크 수 조회를 핸들 기반
  (`GetFileInformationByHandle`)으로 바꿨다. 신원을 못 읽으면 어떤 값과도 같지 않게 해 TOCTOU 재검사가
  fail-closed로 동작한다.
- Windows에서 상속 DACL 계산 시 임퍼소네이션 토큰을 명시적으로 넘긴다. 토큰을 NULL로 두면
  `ERROR_NO_TOKEN`이 나 모든 게시가 실패했다.
- Windows의 `FlushFileBuffers`가 쓰기 권한을 요구하므로, staged 파일을 쓰기로 열어 fsync한다.
  읽기 전용 핸들이면 hwpx 외과 치환 게시 전체가 `ERROR_ACCESS_DENIED`로 실패했다.
- 해시가 고정된 `examples/` 입력에 `eol=lf`를 지정해 Windows 체크아웃에서 골든 리포트가 어긋나지 않게 했다.

**변경**

- 코퍼스 폰트를 커밋하지 않고 fetch한다. `scripts/fetch-corpus-fonts.sh`가 manifest의 고정 URL에서
  받아 SHA-256으로 검증한다. `https` + 고정 host만 허용하고, 목적지는 코퍼스 내부 상대경로만 허용한다.
- 사용자용 문서를 한국어 정본 + 영문 페어(`NAME.md` / `NAME.en.md`)로 정리했다.
- 릴리스 노트를 `CHANGELOG.md` 기반 한/영 병기로 바꿨다. 해당 버전 절이 없으면 릴리스가 실패한다.

### English

**Added**

- A structured authoring path: `compose` and `template` deterministically generate HWP/HWPX from
  DocumentSpec v1/v2 and TemplateSpec/Data v1. Only a typed AST is allowed, with no string
  interpolation and no expression evaluation.
- Native-only certification (`certify`): pins font identity, forbids font substitution, macros and
  external references, renders every page, fails on bounds, collision and unresolved fields, and
  publishes the report atomically.
- A structured corpus gate (`corpus`): generates seven self-authored Korean documents as HWPX and HWP
  twice each and passes only when document bytes, semantic statistics, page PNG hashes, render-issue
  hashes and font identities all agree.
- Published JSON Schemas (`schemas/`), examples (`examples/`) and design documents 13 to 17.
- Three new MCP tools (`hwp_compose`, `hwp_template`, `hwp_certify`), for 15 in total.
- Localized CLI help: English by default, Korean under a Korean locale, and overridable with
  `--lang <en|ko>` or `HWP_LANG`. The CLI reference is generated in both languages.

**Fixed**

- Restored the Windows build. File identity and link counts no longer use the nightly-only
  `windows_by_handle` API; they are read from a handle via `GetFileInformationByHandle`. An identity
  that cannot be read never compares equal, so the TOCTOU recheck fails closed.
- Pass an explicit impersonation token when computing inherited DACLs on Windows. With a NULL token
  the call returned `ERROR_NO_TOKEN` and every publish failed.
- Open the staged file for write before fsync, because `FlushFileBuffers` requires write access on
  Windows. With a read-only handle every surgical hwpx publish failed with `ERROR_ACCESS_DENIED`.
- Pin `eol=lf` for the hash-pinned inputs under `examples/` so golden reports still match on a Windows
  checkout.

**Changed**

- Corpus fonts are fetched rather than committed. `scripts/fetch-corpus-fonts.sh` downloads them from
  the manifest's pinned URL and verifies each against its SHA-256, accepting only `https` from the
  pinned host and only relative in-corpus destinations.
- User-facing documentation is now a Korean original plus an English pair (`NAME.md` / `NAME.en.md`).
- Release notes are now bilingual and driven by `CHANGELOG.md`; a release fails without a section for
  that version.

---

## [0.4.1]

### 한국어

- HWP·HWPX 출력 모두에 쪽 번호를 렌더한다 (#30).
- 한국 공문서 프리셋(기안문·보고서)을 markdown 가져오기에 추가했다 (#28).
- 개조식 문단의 □·○ 간격과 목록 마커(-··) 가시성을 고쳤다 (#27).
- 실기 검증 산출물 디렉터리 이름을 `hwp-verification`으로 정리했다 (#29).

### English

- Page numbers are rendered in both HWP and HWPX output (#30).
- Added Korean official-document presets (gian, report) to the markdown import path (#28).
- Fixed □/○ paragraph spacing and list marker (-, ·) visibility in gaejosik documents (#27).
- Renamed the Hancom verification output directory to `hwp-verification` (#29).

---

## [0.4.0]

### 한국어

- `hwp update` 자체 업데이트와 Homebrew 없는 한 줄 설치 스크립트를 추가했다 (#25).

### English

- Added `hwp update` self-updating and a one-line installation script that does not need Homebrew (#25).

---

## [0.3.0]

### 한국어

- Homebrew 설치를 지원한다. 저장소 자체가 tap이며 릴리스 시 formula를 자동 갱신한다 (#24).
- hwpx 수식 방출과 `hp:script` 엔티티 해석을 추가했다 (#23).
- `convert`에 `--font-dir`를 추가해 PDF 변환의 외부 폰트를 CLI로 제어한다 (#22).
- CLI 레퍼런스를 clap 정의에서 자동 생성하고 드리프트 게이트를 추가했다 (#20).

### English

- Homebrew installation is supported: the repository is its own tap and the formula is updated
  automatically on release (#24).
- Added hwpx equation emission and `hp:script` entity resolution (#23).
- Added `--font-dir` to `convert` so PDF conversion can select external fonts from the CLI (#22).
- The CLI reference is generated from the clap definitions, with a drift gate (#20).
