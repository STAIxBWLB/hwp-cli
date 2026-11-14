[한국어](README.ko.md) · [English](README.md)

# agent-hwp

> 한컴오피스나 COM 자동화 없이 HWP 5.0 / HWPX 문서를 읽고·변환하고·렌더하고·쓰고·AI로 편집하는 클린룸 Rust 툴킷. Linux / macOS / CI에서 그대로 동작한다.

[![CI](https://github.com/STAIxBWLB/agent-hwp/actions/workflows/ci.yml/badge.svg)](https://github.com/STAIxBWLB/agent-hwp/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#라이선스)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)

한글 문서(`.hwp` HWP 5.0 바이너리, `.hwpx` OWPML/KS X 6101)를 **외부 HWP 라이브러리 없이** 다루는
Rust 워크스페이스다. CFB 컨테이너, HWP 레코드 스트림, OWPML XML, 페이지 레이아웃, 글리프 셰이핑까지
전부 스펙과 정품 파일 실측을 근거로 직접 구현했다. 한컴오피스나 Windows COM 자동화에 의존하지 않으므로
Linux/macOS 서버와 CI에서 그대로 돈다.

## 되는 것과 안 되는 것

`ahwp`는 `.hwp`와 `.hwpx`를 읽고·변환하고·렌더하고·쓰며, 이미지·서식·미해석 레코드를 보존한 채
제자리 편집하고, typed 명세와 공문서 템플릿에서 새 문서를 합성하며, 같은 작업을 MCP로 AI 에이전트에
제공한다. 차트·OLE 개체는 지원하지 않고, 수식은 근사 렌더하며, 바이트 동일 왕복은 hwp 출신·무수정
문서에만 보장한다.

- **한컴 동등성 주장 없음.** 최종 판정은 언제나 한글(한컴오피스) 실기에서 열리는지 여부다.

전체 기능 목록, 상태 표, 한계, 로드맵은 [ahwp의 기능과 한계](docs/manual/capabilities.ko.md)에 있다.
미구현 기능은 [12-feature-gaps](docs/design/12-feature-gaps.ko.md)에 정리돼 있다.

## 설치

명령 이름은 `ahwp`다. 2.0에서 1.x의 `hwp` 호환 이름을 제거했으므로 스크립트와 MCP 클라이언트 설정의 `hwp`를 `ahwp`로 바꾼다.

```sh
# 한 줄 스크립트 (macOS / Linux). ~/.local/bin에 설치하고 .sha256으로 검증
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh

# Homebrew (macOS / Linux). 저장소 자체가 tap
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/agent-hwp
brew install ahwp

# 소스 빌드 (Rust edition 2024, rust-version 1.93 이상)
git clone https://github.com/STAIxBWLB/agent-hwp.git && cd agent-hwp
cargo install --path crates/hwp-cli

ahwp update            # 최신 릴리스로 자체 갱신 (brew 설치본은 brew에 위임)
```

빌드 단계에서는 버전과 설치 위치를 고정한다:

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh \
  | sh -s -- --dir /usr/local/bin --tag v2.1.0
```

**폰트는 동봉하지 않는다.** 텍스트 추출과 변환은 폰트 없이 동작하고, 렌더와 PDF 출력에는
`--font-dir <dir>`이나 `HWP_FONT_DIR`로 CJK 폰트를 지정한다. 플랫폼별 사전 빌드 바이너리,
서버리스·컨테이너 빌드(glibc 2.17 기준), 폰트 상세, 되돌리기는 [설치와 업데이트](docs/manual/install.ko.md)에 있다.

## 빠른 시작

```sh
ahwp info report.hwp                                  # 포맷·버전·속성
ahwp cat report.hwp --format markdown                 # 본문을 markdown으로 추출
ahwp grep "예산" report.hwp                           # 검색 (일치 없으면 1로 종료)
ahwp convert report.hwp -o report.hwpx                # hwp → hwpx
ahwp render report.hwp -o report.pdf --font-dir ./fonts   # 검색 가능한 멀티페이지 PDF
ahwp new -o out.hwpx --from notes.md                  # markdown에서 새 문서
ahwp new -o gian.hwpx --template gian-internal        # 두문·결문 포함 기안문
ahwp fill gian.hwpx -o final.hwpx --set "제목=예산 집행 계획"
ahwp edit form.hwp -o filled.hwp --replace "초안=>최종" --verify
ahwp mcp --root . --font-dir ./fonts                  # AI 클라이언트용 MCP stdio 서버
```

더 많은 예와 전체 명령 요약은 [명령 한눈에 보기](docs/manual/commands.ko.md)에 있다.

## 다음에 읽을 곳

| 하려는 일 | 읽을 곳 |
|---|---|
| 전체 기능, 상태 표, 한계, 로드맵 확인 | [ahwp의 기능과 한계](docs/manual/capabilities.ko.md) |
| 다른 플랫폼·컨테이너·서버리스 빌드에 설치, 폰트 준비 | [설치와 업데이트](docs/manual/install.ko.md) |
| 명령이나 플래그 찾기 | [명령 한눈에 보기](docs/manual/commands.ko.md) |
| 문서 변환, 텍스트·markdown·추출 근거 세그먼트 추출 | [Markdown 내보내기](docs/manual/markdown-export.ko.md) |
| 문서 편집·채우기, 양식 필드 바인딩(워크플로와 규약) | [에이전트 규약](docs/manual/agent-conventions.ko.md) |
| 명세로 새 문서 작성, 공문 작성, JSON에서 재생성 | [문서 작성](docs/manual/authoring.ko.md) |
| 작성한 파일 인증, 구조 코퍼스 실행 | [인증과 구조 코퍼스](docs/manual/certification.ko.md) |
| AI 에이전트에서 MCP로 `ahwp` 사용 | [MCP 서버](docs/manual/mcp-server.ko.md) |
| 특정 AI 클라이언트 설정(Amazon Quick 포함), 에이전트 스킬 설치 | [AI 클라이언트 연동](docs/manual/ai-integrations.ko.md) |
| MCP 서버 원격 배포 | [원격 MCP 배포](docs/design/22-remote-mcp-deployment.ko.md) |
| 그 밖의 페이지 찾기 | [문서 지도](docs/README.ko.md) |
| 기여, 소스 빌드 | [개발](docs/manual/development.ko.md) |

## 버전 정책

agent-hwp는 1.0.0부터 [유의적 버전](https://semver.org/lang/ko/)을 따른다. 다음 중 하나라도
호환되지 않게 바뀌면 주 버전을 올린다.

- `ahwp` CLI의 명령, 플래그, 종료 코드
- stdio와 `ahwp serve`로 노출하는 MCP 도구의 이름과 인자
- [`schemas/`](schemas)에 게시한 JSON 스키마. 스키마마다 파일 이름에 자체 버전이 있어
  (`edit-ops-v1`, `segment-envelope-v2`) 호환되지 않는 변경은 새 파일로 나온다.

사람이 읽는 메시지와 `--help` 문구, 렌더링 픽셀, 써낸 파일의 정확한 바이트는 대상이 아니다.

## 기여

버그 리포트와 PR을 환영한다.

- 모든 작업은 `feat/`·`fix/`·`docs/` 브랜치에서 하고 PR로 제출한다. main 직접 push는 하지 않는다.
- PR 전 `scripts/check.sh`를 통과시킨다(CI와 같은 게이트).
- 새 포맷 기능은 가능하면 왕복/골든 테스트를 함께 추가한다.
- 사용자용 문서는 영문 정본(`NAME.md`)과 한국어 페어(`NAME.ko.md`)를 같은 커밋에서 함께 갱신한다.
- 커밋 메시지·PR 본문·릴리스 노트는 영문으로만 쓴다.
- 스펙 참고 자료는 한컴 공식 [한글 문서 파일 형식 5.0](https://store.hancom.com/etc/hwpDownload.do)을
  본다(저장소에 동봉하지 않는다).

## 고지

본 제품은 한글과컴퓨터의 한글 문서 파일(`.hwp`) 공개 문서를 참고하여 개발하였습니다.

> This product was developed with reference to Hancom's HWP document file format open
> specification, [한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0](https://store.hancom.com/etc/hwpDownload.do)
> (© (주)한글과컴퓨터).

한컴 공개 문서의 저작권은 (주)한글과컴퓨터에 있다. 한컴 공개 문서 라이선스는 자유로운 열람·복사·배포를
허용하되 **수정되지 않은 원본/복사본**으로 제한하므로, 이 저장소는 스펙 문서(및 추출본·페이지 캡처 등
파생물)를 동봉하지 않고 공식 배포처 링크만 제공한다([docs/README.ko.md](docs/README.ko.md) 참고).

테스트 픽스처 일부는 [hahnlee/hwp-rs](https://github.com/hahnlee/hwp-rs)(Apache-2.0)에서 가져왔다.
`fixtures/README.md`와 루트 `NOTICE` 참고. `ahwp slots --forms`·`ahwp fill --forms`의 양식 필드 규칙은
[chrisryugj/kordoc](https://github.com/chrisryugj/kordoc)(MIT)에서 이식했다. 루트 `NOTICE` 참고.

## 라이선스

[MIT](LICENSE-MIT) 또는 [Apache-2.0](LICENSE-APACHE) 듀얼 라이선스다. 별도 명시가 없는 한, 이 저장소에
기여한 코드는 위 두 라이선스로 동일하게 배포되는 것에 동의하는 것으로 간주한다.
