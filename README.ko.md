[한국어](README.ko.md) · [English](README.md)

# hwp-cli

> 한컴오피스나 COM 자동화 없이 HWP 5.0 / HWPX 문서를 읽고·변환하고·렌더하고·쓰고·AI로 편집하는 클린룸 Rust 툴킷. Linux / macOS / CI에서 그대로 동작한다.

[![CI](https://github.com/STAIxBWLB/hwp-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/STAIxBWLB/hwp-cli/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#라이선스)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)

한글 문서(`.hwp` HWP 5.0 바이너리, `.hwpx` OWPML/KS X 6101)를 **외부 HWP 라이브러리 없이** 다루는
Rust 워크스페이스다. CFB 컨테이너, HWP 레코드 스트림, OWPML XML, 페이지 레이아웃, 글리프 셰이핑까지
전부 스펙과 정품 파일 실측을 근거로 직접 구현했다. 한컴오피스나 Windows COM 자동화에 의존하지 않으므로
Linux/macOS 서버와 CI에서 그대로 돈다.

## 주요 기능

- **읽기·텍스트 추출** hwp/hwpx에서 plain / markdown / HTML / JSON(전체 IR)로. 표·이미지·머리말/꼬리말·
  미해석 레코드까지 보존하며 파싱한다.
- **포맷 변환** hwp ↔ hwpx, hwp/hwpx ↔ markdown, hwp/hwpx ↔ JSON(IR). 공용 문서 모델(IR)을 경유한
  양방향 변환.
- **렌더링** hwp/hwpx → PNG / SVG / PDF. 파일에 저장된 줄 배치(PARA_LINE_SEG)를 우선 쓰고, 없으면
  자체 줄바꿈으로 보정한다. PDF는 폰트를 서브셋·임베드한 단일 멀티페이지 문서라 텍스트 선택·검색·복사가
  된다(ToUnicode CMap).
- **문서 쓰기(hwp 바이너리 포함)** hwpx 패키지 쓰기와 HWP 5.0 바이너리(CFB) 쓰기를 모두 구현했다.
  hwp 출신·무수정 문서는 압축 해제 스트림 기준 바이트 동일 왕복까지 보장한다.
- **구조 문서 합성** DocumentSpec v1/v2, TemplateSpec/Data v1 명세에서 결정론적으로 HWP/HWPX를 만든다
  (`compose`, `template`). 문자열 보간이나 표현식 실행 없이 typed AST만 허용한다.
- **인증·코퍼스 게이트** `certify`는 package·반복 import·bounded native render를 인증해 원자적으로
  리포트를 게시하고, `corpus`는 고정 코퍼스로 2회 생성 결과의 바이트·의미·렌더 해시 일치를 강제한다.
- **AI 편집** IR을 JSON으로 내보내 고치고 되쓰는 read → edit → rewrite 왕복. 텍스트 치환, 표 셀 설정,
  누름틀/필드 채우기를 이미지·서식·미해석 레코드를 보존한 채 적용한다.
- **MCP 서버** 의존성 없는(serde_json만) stdio MCP 서버로 Amazon Quick Desktop을 포함한
  데스크톱 클라이언트에 16개 도구를 노출한다.

## 구현 상태

| 영역 | 상태 |
|---|---|
| hwp/hwpx 읽기, 텍스트·markdown·JSON 추출 | 구현 완료 |
| hwpx 쓰기, HWP 5.0 바이너리 쓰기 | 구현 완료 |
| PNG/SVG/PDF 렌더 (표·그림·글상자·도형·각주/미주·쪽번호·글자 효과) | 구현 완료 |
| 구조 편집 (문단·표 행/열·셀 병합/분할·필드·이미지·도장) | 구현 완료 (병합 표 포함) |
| DocumentSpec v1/v2, TemplateSpec v1 합성 | 구현 완료 |
| 인증(certify) · 구조 코퍼스 게이트(corpus) | 구현 완료 |
| MCP 서버 (16 도구) | 구현 완료 |
| HTML 변환 | 동작하나 markdown 대비 충실도 낮음 (로드맵) |
| 수식 | 상자+스크립트 근사 렌더 |
| 차트 · OLE | 미지원 |
| 암호화/배포용(DRM) 문서 | 읽기 거부 |

### 한계

- **무손실 왕복의 범위** hwp 출신·무수정 문서만 바이트 동일 왕복을 보장한다. 편집했거나
  hwpx/markdown 출신인 문서는 writer 합성 경로를 거쳐 **의미 동등**(텍스트·구조 보존)으로 되쓴다.
  hwpx 쓰기는 항상 의미 동등(템플릿 기반 재생성)이다. JSON 이미지까지 포함한 완전 무손실 왕복은
  `--embed-bin` 경로 전용이다.
- **포맷 간 의미 변환** 표·그림·구역·머리말/꼬리말·글상자·필드·각주/미주는 의미 파싱·렌더되지만,
  도형·수식·차트·OLE는 렌더는 되어도 hwp↔hwpx 레코드 합성은 아직 안 된다(같은 포맷 안에서는 원형 보존).
- **필드** 기존 이름의 값을 채우거나 앵커 뒤에 새 누름틀을 만드는 것까지이며, 임의 필드 종류 생성은 없다.
- **렌더 해시** 코퍼스가 기록하는 렌더 해시는 그 OS/아키텍처의 관측값이지, 플랫폼 간 픽셀 동일성 주장이 아니다.
- **한컴 동등성 주장 없음** 최종 판정은 언제나 한글(한컴오피스) 실기에서 열리는지 여부다.

## 로드맵

상세 항목은 [TODO.ko.md](TODO.ko.md)(영문: [TODO.md](TODO.md)), 미구현 기능 카탈로그는
[docs/design/12-feature-gaps.ko.md](docs/design/12-feature-gaps.ko.md)에 있다.

1. **스펙 재문서화 (진행 중)** HWP 5.0 스펙 PDF를 검수 가능한 Markdown으로 재구성한다. rev1.3 본문은
   재구성이 끝났고(§1~§4.4), OWPML/KS X 6101·수식·차트·배포용 문서 스펙은 확보가 필요하다.
2. **스펙 기준 전면 재검토 (진행 중)** 재구성 스펙을 기준으로 파서·writer·IR의 비트 단위 값을 전수
   대조한다. 기능 커버리지 감사에서 신규 갭 17건을 등재했고, 콘텐츠 소실급 4건이 수정 1순위다.
3. **HTML 변환 고도화** markdown에서 이미 해소한 각주 마커·병합 셀 colspan/rowspan·셀 내 블록을
   HTML 경로에도 반영하고, 글자·문단 모양의 CSS 매핑을 보강한다.
4. **Windows** v0.5.0부터 컴파일·게시 경로가 안정화됐다. CI에서 ubuntu·macOS·Windows 모두
   필수 게이트다.
5. **배포용(DRM) 문서** 스펙 확보를 전제로 착수 후보다.

## 설치

### 한 줄 스크립트 (macOS / Linux)

Rust 툴체인 없이 릴리스 바이너리를 설치한다. 설치 후에는 `hwp update`로 자체 갱신된다.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

기본 위치는 `~/.local/bin`이다(PATH에 없으면 안내를 출력한다). 위치·버전은 인자나 환경변수로 바꾼다:

```sh
curl -fsSL .../install.sh | sh -s -- --dir /usr/local/bin --tag v0.5.0
HWP_INSTALL_DIR=~/bin sh scripts/install.sh
```

아카이브는 `.sha256` 자산과 대조한 뒤에만 설치한다.

### Homebrew (macOS / Linux)

저장소 자체가 tap이다(별도 `homebrew-*` 저장소 없음).

```sh
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/hwp-cli
brew install hwp
hwp --version
```

업그레이드는 `brew update && brew upgrade hwp`. 지원 플랫폼은 macOS(Apple Silicon·Intel)와 Linux x86_64다.

### 사전 빌드 바이너리

각 [릴리스](https://github.com/STAIxBWLB/hwp-cli/releases)에 플랫폼별 아카이브와 `.sha256`이 붙는다.

| 플랫폼 | 아카이브 |
|---|---|
| Linux x86_64 | `hwp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `hwp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `hwp-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

압축을 풀어 `hwp`를 PATH에 둔다(검증: `shasum -a 256 -c hwp-*.sha256`).

### 서버리스·컨테이너 (Vercel, AWS Lambda, Docker)

Linux 아카이브는 **glibc 2.17** 기준으로 크로스 빌드한다. 그래서 Amazon Linux 2·2023, Debian 8+,
RHEL/CentOS 7+ 및 그 이후 배포판에서 그대로 돌아간다. 빌드 러너보다 glibc가 낮은 Vercel Node
런타임과 AWS Lambda도 포함된다.

바이너리를 커밋하지 말고 빌드 단계에서 받는다. 그러면 갱신할 것은 고정한 태그 하나뿐이다.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh \
  | sh -s -- --tag v0.8.5 --dir ./bin
```

플랫폼이 배포 번들을 수집하기 전에 바이너리가 있어야 하므로(Vercel `includeFiles`, Next.js
`outputFileTracingIncludes`, Docker 레이어) 요청 시점이 아니라 빌드 커맨드나 `prebuild`
스크립트에서 실행한다. 폰트는 동봉되지 않으니 렌더 계열 명령에는 폰트를 함께 올리고
`--font-dir`(또는 `HWP_FONT_DIR`)로 지정한다.

### 소스 빌드

```sh
git clone git@github.com:STAIxBWLB/hwp-cli.git && cd hwp-cli
cargo build --release
cargo install --path crates/hwp-cli   # `hwp` 바이너리 설치
```

Rust edition 2024, `rust-version = 1.93` 이상이 필요하다.

### 폰트

**저장소에 폰트를 동봉하지 않는다**(`fonts/`는 gitignore). 텍스트 추출·변환은 폰트 없이 동작하고,
렌더·PDF·hwp 바이너리 쓰기(미리보기 이미지)에만 CJK 글리프가 필요하다.

| 사용처 | 폰트 지정 |
|---|---|
| `render` / `diff` / `mcp` | `--font-dir <dir>`(반복 지정 가능) |
| `convert` / 테스트 | 환경변수 `HWP_FONT_DIR`(미설정 시 프로젝트 `fonts/`) |

구조 코퍼스 게이트는 시스템 폰트를 쓰지 않고 해시로 고정된 OFL 폰트만 쓴다. 폰트 바이트는 커밋하지
않으며 `scripts/fetch-corpus-fonts.sh`가 manifest의 고정 URL에서 받아 SHA-256으로 검증한다.

### 업데이트

```sh
hwp update            # 최신 릴리스로 자체 교체 (체크섬 대조 후 원자적 교체)
hwp update --check    # 교체 없이 현재/최신 버전만 확인
hwp update --tag v0.4.0   # 특정 버전으로 되돌리기
```

설치 방식을 스스로 판별한다. 한 줄 스크립트·릴리스 아카이브·`cargo install`로 설치했으면 실행 중인
바이너리를 제자리 교체하고(sha256 대조 → 같은 디렉터리 임시 파일 → rename, 실패 시 원본 복원),
Homebrew(Cellar) 설치본은 `brew upgrade hwp`에 위임한다(brew는 버전 고정이 안 되므로 `--tag`는 거부).

## 빠른 시작

```sh
# 진단: 포맷/버전/속성/스트림
hwp info report.hwp

# 본문 추출
hwp cat report.hwp                       # plain text
hwp cat report.hwp --format markdown     # markdown
hwp cat report.hwp --format json         # 전체 IR(JSON)

# 변환 (출력 확장자로 포맷 추론)
hwp convert report.hwp   -o report.hwpx  # hwp → hwpx (표·이미지·머리말 보존)
hwp convert report.hwpx  -o report.hwp   # hwpx → hwp 바이너리
hwp convert report.hwp   -o report.md    # 이미지는 report.media/에 추출
hwp convert report.hwp   -o doc.json --embed-bin   # 이미지까지 임베드한 자급식 JSON

# 렌더링
hwp render report.hwp -o page.png --dpi 150
hwp render report.hwp -o report.pdf --font-dir ./fonts   # 단일 멀티페이지 PDF(검색 가능)

# 새 문서 생성
hwp new -o out.hwpx --from notes.md
hwp new -o out.hwp  --from doc.json

# 구조 문서 합성
hwp compose spec.yaml -o report.hwpx --report
hwp template report-template.yaml --data report-data.json -o report.hwpx

# 편집 (이미지·서식·미해석 레코드 보존)
hwp fields form.hwp                        # 채울 수 있는 필드/누름틀 이름 확인
hwp edit form.hwp -o filled.hwp \
    --replace "초안=>최종" \
    --set-cell "0:1:2=12,300원" \
    --set-field "수신처=홍길동" --verify

# 구조 편집 (문단 삽입/삭제·표 행/열·셀 병합/분할)
hwp edit report.hwp -o out.hwp \
    --insert-para "개요=>추가 설명 문단입니다." \
    --add-row "0" --add-col "1" --merge-cells "0:1:1:2:2" --verify

# 렌더 충실도 비교 (한글 기준 PNG와 잉크/오프셋/픽셀 오차)
hwp diff report.hwp --ref hancom_p1.png --page 1 --dpi 150 --font-dir ./fonts

# MCP stdio 서버
hwp mcp --font-dir ./fonts
```

## 명령 레퍼런스

전체 자동 생성 레퍼런스는 [docs/manual/cli-reference.ko.md](docs/manual/cli-reference.ko.md)(영문:
[cli-reference.md](docs/manual/cli-reference.md))에 있다. clap 정의에서 두 언어 모두 생성되며
코드와의 동기화를 CI 테스트가 강제한다. 아래는 요약이다.

도움말 표시 언어는 로케일을 따르고(한국어 로케일이면 한국어, 그 외 영문), `--lang en|ko`나
`HWP_LANG`으로 바꿀 수 있다.

| 명령 | 설명 |
|---|---|
| `info <file>` | 포맷/버전/속성/스트림 진단 |
| `cat <file>` | 본문 추출(plain/markdown/json). `--with-segments`로 추출 근거 좌표 동반 |
| `convert <input> -o <output>` | 포맷 변환. 출력이 `.pdf`면 렌더 경로로 위임. `--strict`는 보존 불가 데이터 발견 시 게시하지 않고 실패 |
| `render <input> -o <output>` | 페이지를 PNG/SVG(페이지별 파일)·PDF(단일 멀티페이지)로 렌더 |
| `new -o <output>` | markdown/JSON IR에서 새 문서 생성 |
| `compose <spec> -o <output>` | DocumentSpec v1/v2를 결정론적으로 합성. `--dry-run`으로 쓰지 않고 검증 |
| `template <template> --data <data> -o <output>` | TemplateSpec/Data v1의 typed AST를 bounded expansion |
| `edit <input> -o <output>` | 텍스트·서식·구조 편집(문단, 표 행/열, 셀 병합/분할, 필드, 이미지, 도장) |
| `fields` / `bookmarks` / `slots` `<file>` | 필드·누름틀 / 책갈피 / `{{name}}` 슬롯 목록 |
| `fill <input> -o <output>` | 충실도 보존 템플릿 채우기(hwpx 패키지 보존) |
| `validate <file>` | 구조 검증(mimetype·필수 엔트리·XML 파싱) |
| `certify <input> --policy <file> --report <dir>` | 인증 리포트를 원자적으로 게시. [Certification v1](docs/design/16-certification-v1.ko.md) |
| `corpus --manifest <file> --report <dir>` | 고정 구조 코퍼스 게이트. [코퍼스 계약](docs/design/17-structured-corpus-v1.ko.md) |
| `diff <input> --ref <png>` | 렌더 결과를 한글 기준 PNG와 비교(잉크·오프셋·픽셀 오차·MAE) |
| `mcp` | MCP stdio 서버 실행 |
| `skill export` | Amazon Quick 프로필 자동 탐색을 포함한 번들 에이전트 스킬 내보내기/설치 |
| `update` | 자체 업데이트(brew 설치본은 `brew upgrade`에 위임) |
| `dump <file>` | [개발자용] 레코드/패키지 구조 덤프 |

출력 포맷은 대부분 출력 파일 확장자에서 추론된다. `convert`/`render`는 `--to`/`--format`으로 명시할 수도 있다.

## Markdown 내보내기

hwp/hwpx를 GFM으로 변환한다. `hwp cat --format markdown`은 stdout 전용이라 이미지를 추출하지 않는다.

```sh
hwp convert report.hwp -o report.md                    # 이미지는 report.media/에 추출
hwp convert report.hwp -o report.md --media-dir figs   # figs/에 추출하고 figs/... 로 링크
hwp convert report.hwp -o full.md --with-header-footer --with-hidden
```

| HWP 요소 | markdown |
|---|---|
| `개요 N` 스타일 문단 | `#` × N 헤딩 |
| 굵게 / 기울임 / 취소선 / 밑줄 / 첨자 | `**굵게**` / `*기울임*` / `~~취소선~~` / `<u>` / `<sup>`·`<sub>` |
| 하이퍼링크(%hlk) | `[표시텍스트](URL)` |
| 이미지 | `![image](<media>/imageN.png)` (확장자는 매직 바이트로 판별) |
| 표(병합 없음) | GFM 파이프 표 |
| 표(병합 셀·중첩 표·블록 수식) | HTML `<table>`(colspan/rowspan) |
| 글머리표 / 번호 문단 | `- ` / `N. ` 목록(번호 형식은 numbering 정의에서 합성) |
| 각주 / 미주 | `[^N]` / `[^eN]` GFM 풋노트 |
| 수식(eqed) | `$스크립트$` / `$$스크립트$$` (HWP 수식 스크립트 원문, LaTeX 아님) |
| 머리말/꼬리말·숨은 설명 | 기본 제외, `--with-header-footer` / `--with-hidden`으로 포함 |

**표 직렬화 보장**: HTML 표의 각 `<tr>...</tr>`과 GFM 파이프 표의 각 행은 항상 한 줄로 직렬화된다
(셀 안 중첩 표도 그 줄에 인라인으로 얹힌다). 소비자가 행 단위로 안정적으로 인용·파싱할 수 있도록
테스트로 고정돼 있다.

한계: 헤딩 인식은 `개요 N` 스타일뿐이고, 떠 있는 개체의 위치·z-order는 반영하지 않으며, 역방향
(md → hwp)은 표·굵게 등 기본 구문만 복원된다.

### `--with-segments` (추출 근거 좌표)

`hwp cat <file> --format markdown --with-segments`는 markdown과 함께 각 출력 문자 범위가 어느 원본
문단에서 왔는지를 한 줄 JSON 봉투로 낸다.

```json
{"markdown": "...", "segments": [
  {"kind": "para", "section": 0, "para": 12, "start": 345, "end": 512}
]}
```

- **오프셋은 유니코드 스칼라(문자) 단위** Python `str` 인덱싱과 같다. `markdown[start:end]`로 그대로 슬라이스된다.
- **좌표는 IR 인덱스** `section`/`para`는 `--format json`의 `sections[]`/`paragraphs[]` 인덱스라 재디코드에도 안정적이다.
- **정렬·비중첩** 세그먼트는 `start` 오름차순이며 겹치지 않는다. 어느 문단에도 귀속되지 않는 출력은 간극으로 남는다.
- 표가 만든 줄은 표를 담은 문단 인덱스를 상속하고, 각주/미주 정의는 참조 문단에 귀속된다.
- `markdown` 필드는 `--with-segments` 없이 실행한 출력과 바이트 단위로 동일하다.

## 구조 문서 합성

`compose`는 v1 JSON/YAML 명세를 문단·런·스타일·목록·표·수식·필드·머리말/꼬리말·쪽 번호·구역으로
컴파일한다. v2는 그 문서를 감싸고 접근성 설명이 포함된 이미지, 닫힌 SVG→PNG fallback, HWPX 네이티브
사각형 글상자를 더한다. v2 fallback policy는 타겟별로 명시하며 생략하면 native-only로 실패한다.

```sh
# 쓰지 않고 스키마·참조·표 span·자산·네이티브 지원 여부 확인
hwp compose examples/document-spec-v1/basic.json -o /tmp/basic.hwpx --dry-run --report

# 검증된 파일을 원자적으로 게시
hwp compose examples/document-spec-v1/comprehensive.yaml -o /tmp/report.hwpx --report
```

`template`은 문자열 보간이나 표현식 실행 없이 typed `value`/`if`/`each` AST만 허용한다. `reference_hwpx`는
기존 패키지의 지정 텍스트/필드만 외과적으로 채우며, 구조 재생성은 명시적 strict gate가 필요하다.

정본 스키마는 [`schemas/`](schemas/)에, 계약 문서는 [13-document-spec-v1](docs/design/13-document-spec-v1.ko.md),
[14-template-spec-v1](docs/design/14-template-spec-v1.ko.md), [15-document-spec-v2](docs/design/15-document-spec-v2.ko.md)에 있다.
예제는 [`examples/`](examples/).

## 인증과 구조 코퍼스

`certify`는 package 검증, 반복 import, bounded native render, 선택적 독립 import를 고정 정책으로
인증하고 새 디렉터리를 원자적으로 게시한다. 정책은 폰트 identity 고정, 폰트 대체 금지, macro/external
reference 금지, bounds/collision/unresolved-field 실패를 강제한다.

`corpus`는 자체 작성 한국어 7종 문서를 HWPX/HWP로 각각 2회 생성하고, 두 실행의 문서 바이트·의미 통계·
페이지 PNG 해시·render issue 해시·폰트 identity가 모두 같아야 통과시킨다.

```sh
bash scripts/fetch-corpus-fonts.sh    # 고정 OFL 폰트를 받아 SHA-256 검증 (최초 1회)
hwp corpus --manifest corpus/structured-v1/manifest.json --report /new/path/corpus-report
```

`scripts/check-structured-corpus.sh`(CI 게이트)가 이 fetch를 먼저 실행하므로 별도 준비는 필요 없다.
계약 상세는 [17-structured-corpus-v1](docs/design/17-structured-corpus-v1.ko.md), 인증 계약은
[16-certification-v1](docs/design/16-certification-v1.ko.md).

## JSON IR로 문서 재생성

기존 문서를 편집하지 않고 처음부터 똑같이 신규 생성할 수 있다.

```sh
hwp convert report.hwpx -o report.json   # hwp/hwpx → JSON IR (--embed-bin: 이미지까지)
hwp new --from report.json -o regen.hwpx # JSON IR → 신규 문서
```

재생성 검증(`crates/hwp-cli/tests/regen.rs`)은 validate 통과, `hwp cat` 출력 전문 동일, 표 지도
(개수·행/열·병합·셀 폭) 동일, secPr·tabProperties 슬라이스 바이트 동일을 확인한다. 차이가 남는 것은
설계상 재생성 대상인 줄 배치 캐시(한글이 열 때 재계산), 미리보기 이미지, settings.xml뿐이다.

## MCP 서버 (AI 에이전트 연동)

`hwp mcp`는 tokio나 SDK 없이 `serde_json`만으로 동기 JSON-RPC 2.0(stdio, 줄 단위)을 구현한 MCP
서버다. 프로토콜 버전은 `initialize`에서 협상한다: 클라이언트의 `protocolVersion`이
`2025-06-18`·`2025-03-26`·`2024-11-05` 중 하나면 그대로 돌려주고, 아니면 최신 지원 버전으로
응답한다. stdout은 프로토콜 전용이고 로그는 stderr로 나간다.

클라이언트별 설정(Claude Code/Desktop, Codex CLI/cloud, Kiro, Kimi, claude.ai 스킬 업로드,
Amazon Quick Desktop)과 번들 에이전트 스킬(`hwp skill export`):
[docs/manual/ai-integrations.ko.md](docs/manual/ai-integrations.ko.md).
Windows 복사·실행 설정, 생성·검증 acceptance test, 재사용 가능한 에이전트 지침, 증상별 복구는 전용
[Amazon Quick Desktop 가이드](docs/manual/amazon-quick-desktop.ko.md)에 정리했다.

Amazon Quick Desktop은 로컬 stdio 서버를 실행해 16개 도구를 모두 노출할 수 있다. publish-safe
스킬은 활성 프로필에 다음과 같이 설치한다.

```sh
hwp skill export --install amazon-quick
```

Windows에서는 `%USERPROFILE%\AppData\LocalLow` 아래에 전용 교환 root(예: `hwp-quick-workspace`)를
만들고 그 절대 경로를 MCP `--root`로 전달한다(Quick 인자는 환경 변수를 확장하지 않는다). Quick은
로컬 MCP 자식을 Low mandatory integrity로 시작하므로 `C:\TEMP`에서 도구 탐색은 통과해도 첫 쓰기가
거부될 수 있다. Quick의 로컬 폴더 권한은 이 쓰기 무결성을 바꾸지 않으므로 전용 가이드의
생성·import JSON·복구 절차를 따른다.

Amazon Quick Web은 로컬 stdio 프로세스를 시작할 수 없다. 인증된 Streamable HTTP, tenant 격리,
artifact 전송 요구사항은 후속 작업용 [Remote MCP transport](docs/design/20-remote-mcp.ko.md)에
정리했으며 현재 릴리스에는 HTTP runtime이 없다.

### 노출 도구 (16종)

| 도구 | 필수 인자 | 기능 |
|---|---|---|
| `hwp_info` | `path` | 포맷/버전/속성/스트림 진단 |
| `hwp_read` | `path` | 본문 추출(`plain`/`markdown`/`json`/`html`/`csv`, 머리말·꼬리말/숨은 설명/세그먼트 옵션). UTF-8 byte 페이지네이션(기본 256 KiB, 최대 1 MiB) |
| `hwp_grep` | `path`, `pattern` | 문단 텍스트 검색. `{matches, count, truncated}` 반환 — 0건이어도 정상 결과 |
| `hwp_list_fields` | `path` | 필드/누름틀 목록 |
| `hwp_list_bookmarks` | `path` | 책갈피(bokm) 목록 |
| `hwp_slots` | `path` | `{{name}}` 자리표시자 목록 |
| `hwp_render` | `path` | 페이지를 PNG로 렌더(base64, 응답 최대 16 MiB)하거나 `output_path`로 PNG/SVG/PDF 파일 작성(dpi 36..=600) |
| `hwp_edit` | `input`, `output` | typed JSON 작업으로 strict atomic 편집 |
| `hwp_convert` | `input`, `output` | 포맷 변환(MCP의 `strict` 기본값은 true) |
| `hwp_new` | `output` | markdown/JSON IR과 metadata에서 새 문서 생성 |
| `hwp_compose` | `output`, `spec`/`spec_path` | DocumentSpec v1/v2를 CLI와 같은 경로로 합성 |
| `hwp_template` | `output`, `template`(+`data`) | TemplateSpec/Data v1을 bounded expansion |
| `hwp_certify` | `input`, `policy`, `report` | 인증 실행 및 원자적 리포트 게시 |
| `hwp_fill` | `input`, `output`, `values` | hwpx 템플릿 `{{name}}` 치환(패키지 보존) |
| `hwp_diff` | `input`, `ref` | 지정 페이지를 렌더해 기준 PNG와 비교 |
| `hwp_validate` | `path` | 구조 검증 `{valid, errors, warnings}` |

### 클라이언트 설정 예

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--font-dir", "<repo>/fonts", "--root", "<repo>"]
    }
  }
}
```

`--root`(반복 가능)는 도구가 다루는 모든 파일 경로 — 입력·출력, 중첩 이미지/부분 경로, spec
`base_dir`, 호출당 `font_dir` — 를 지정 디렉터리 아래로 제한한다. 이 루트는 compose/template spec
내부 참조에도 적용되어, spec이 참조하는 이미지·비주얼 에셋과 `reference_hwpx` 패키지도 허용 루트
아래여야 한다. `--root` 없이 실행하면 종전처럼
제한 없이 동작하며, 기동 시 stderr에 한 줄 경고를 출력한다.

### read → edit → rewrite 왕복

1. **읽기** `hwp_read`(`format=json`)가 문서 전체를 IR로 내보낸다. 표·이미지 참조·서식·미해석 레코드까지 담긴다.
2. **편집** `hwp_edit`로 치환·셀 설정·필드 채우기를 적용한다. IR만 바꾸므로 이미지·서식·opaque 레코드가
   보존되고, 편집된 문단의 줄 배치만 무효화되어 writer가 재합성한다.
3. **확인** `hwp_render`가 결과 페이지를 PNG로 돌려주어 에이전트가 변경을 눈으로 검증한다. `hwp_diff`로
   한글 기준 렌더와 정량 비교할 수 있다.

편집된 hwp는 writer 합성 경로에서 한글 문단 불변식(줄 배치·문단끝 `0x0d`·nchars 등)을 다시 세우므로
한글에서 정상 문서로 열린다.

## 워크스페이스 구성

| 크레이트 | 역할 |
|---|---|
| `hwp-model` | 공유 문서 모델(IR). 모든 크레이트가 의존하는 단일 계약, 무손실 보존(opaque/tail), 단위 변환 |
| `hwp5` | HWP 5.0 바이너리 reader/writer (CFB 컨테이너 + 레코드 스트림 + 압축) |
| `hwpx` | HWPX reader/writer (ZIP 패키지 + OWPML XML) |
| `hwp-convert` | IR ↔ markdown / HTML / JSON, 인메모리 편집, 필드 스캔 |
| `hwp-render` | IR → PNG / SVG / PDF 렌더러, 줄 배치 합성, 셰이핑, 폰트 서브셋·임베드, 렌더 diff |
| `hwp-cli` | `hwp` 바이너리 (CLI + MCP 서버) |

## 개발과 테스트

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
`fixtures/README.md`와 루트 `NOTICE` 참고.

## 라이선스

[MIT](LICENSE-MIT) 또는 [Apache-2.0](LICENSE-APACHE) 듀얼 라이선스다. 별도 명시가 없는 한, 이 저장소에
기여한 코드는 위 두 라이선스로 동일하게 배포되는 것에 동의하는 것으로 간주한다.
