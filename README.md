# hwp-cli

> A clean-room Rust toolkit to read, convert, render, write and AI-edit HWP 5.0 / HWPX documents with **no Hancom or COM dependency** — runs on Linux / macOS / CI.

[![CI](https://github.com/STAIxBWLB/hwp-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/STAIxBWLB/hwp-cli/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#라이선스)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg)](Cargo.toml)

한글 문서(`.hwp` HWP 5.0 바이너리, `.hwpx` OWPML/KS X 6101)를 **외부 HWP 라이브러리 없이**
읽고·추출하고·변환하고·렌더링하고·쓰고·AI로 편집하는 Rust 워크스페이스. CFB 컨테이너, HWP 레코드
스트림, OWPML XML, 페이지 레이아웃, 글리프 셰이핑까지 전부 스펙 기반으로 직접 구현한 클린룸
엔진이다. 한컴오피스나 Windows COM 자동화에 의존하지 않으므로 Linux/macOS 서버와 CI에서
그대로 동작한다.

## 주요 기능

- **읽기·텍스트 추출** — hwp/hwpx → plain / markdown / JSON(전체 IR). 표·이미지·머리말/꼬리말·
  미해석 레코드까지 보존하며 파싱한다.
- **포맷 변환** — hwp ↔ hwpx, hwp/hwpx ↔ markdown, hwp/hwpx ↔ JSON(IR). 공용 문서 모델(IR)을
  경유한 양방향 변환.
- **렌더링** — hwp/hwpx → **PNG / SVG / PDF**. 파일에 저장된 줄 배치(PARA_LINE_SEG)를 우선 사용하고,
  없으면 자체 줄바꿈으로 보정한다. 표·이미지·머리말/꼬리말·밑줄/취소선·글상자·**그리기 개체(선·사각형·
  타원·호·다각형)**·**연결 다단 글상자**·**쪽번호**를 그리고, **양쪽 정렬**을 포함한 정렬을 적용한다.
  PDF는 폰트를 서브셋·임베드한 단일 멀티페이지 문서로, **텍스트 선택·검색·복사가 가능**하다(ToUnicode CMap).
- **문서 쓰기 (hwp 바이너리 포함)** — hwpx 패키지 쓰기와 **HWP 5.0 바이너리(CFB) 쓰기**를 모두 구현.
  hwp 출신·무수정 문서는 표·이미지·도형·책갈피를 포함해도 압축 해제 스트림 기준 **바이트 동일 왕복**까지
  보장한다(전체 fixture 게이트).
- **AI 편집** — IR을 JSON으로 내보내 고치고 되쓰는 read→edit→rewrite 왕복. 텍스트 치환, 표 셀 설정,
  누름틀/필드 채우기를 이미지·서식·미해석 레코드를 보존한 채 인메모리로 적용한다.
- **MCP 서버** — 의존성 없는(serde_json만) stdio MCP 서버로 14개 도구를 노출. 에이전트가 문서를
  읽고·렌더해서 직접 보고·편집·변환한다.

## 현재 범위와 한계

- **구현 완료**: hwp/hwpx 읽기, hwpx 쓰기, **hwp 바이너리 쓰기**(convert hwpx→hwp, new→hwp,
  edit→hwp 포함), markdown/JSON 왕복, PNG/SVG/PDF 렌더링(표·그림·글상자·도형·각주/미주·쪽번호·글자 효과),
  렌더 충실도 diff, 누름틀/필드 채우기, MCP 서버.
- **PDF 출력** — 폰트 임베드(서브셋, CIDFontType2(glyf)·CIDFontType0(CFF/OTF), Identity-H) + ToUnicode로
  텍스트 검색이 가능한 단일 멀티페이지 PDF. glyf는 FontFile2, CFF(OTF)는 FontFile3(OpenType)로 정식
  임베드한다. 그리기 개체(선·사각형·**둥근 사각형**·타원·호·다각형)는 **hwp·hwpx 모두** 그리고, 채움은
  **단색·그러데이션(선형/방사형)·이미지**, 선은 **점선·화살표**, 표는 **대각선 테두리**까지 렌더한다.
  글자 효과는 **음영(형광펜)·위/아래 첨자·그림자**를, 본문에는 **각주/미주**(하단 영역 + 번호 참조)를 그린다.
  양쪽 정렬은 공백 우선 분배. 수식은 상자+스크립트로 근사 렌더하며, 차트·OLE만 아직 미지원이다.
- **무손실 왕복의 범위**: hwp 출신·무수정 문서는 표·이미지·도형·책갈피를 포함해도 압축 해제 스트림
  기준 바이트 동일 왕복까지 보장한다(전체 fixture 게이트). 편집했거나 hwpx/markdown 출신인 문서는
  writer의 합성 경로를 거쳐 **의미 동등**(텍스트·구조 보존)으로 되쓴다. hwpx 쓰기는 항상 의미 동등
  (템플릿 기반 재생성)이다. JSON 이미지까지 포함한 완전 무손실 왕복은 `--embed-bin`(base64 임베드)
  경로 전용이다.
- **미지원 입력**: 암호화/배포용(DRM) HWP 5.0 문서는 읽기를 거부한다.
- **의미 모델 한계**: 표·그림·구역·머리말/꼬리말·글상자·필드·각주/미주는 의미 파싱·렌더되지만, 도형·
  수식·차트·OLE는 렌더는 되어도(도형은 hwp·hwpx 모두) **포맷 간 의미 변환**(hwp↔hwpx 도형 레코드 합성)은
  아직 안 된다 — 같은 포맷 안에서는 원형 보존(round-trip)된다. 누름틀/필드는 기존 이름의 값만 채울 수
  있고 신규 필드 생성은 없다.

## 설치와 빌드

### 사전 요구사항

- **Rust** edition 2024, `rust-version = 1.93` 이상(워크스페이스 `Cargo.toml` 기준).
- **CJK 폰트** — 렌더링과 hwp 바이너리 쓰기(미리보기 텍스트/이미지)에 한글 글리프가 필요하다.
  레포에 함초롬 폰트(`HCRBatang`, `HCRDotum`, 각 Bold 포함 4종)가 `fonts/`에 동봉되어 있다.

#### 폰트 지정 방법

| 사용처 | 폰트 지정 |
|---|---|
| `render` / `diff` / `mcp` | `--font-dir <dir>` 플래그(반복 지정 가능) |
| `convert` / 테스트 | 환경변수 `HWP_FONT_DIR`(미설정 시 프로젝트 `fonts/` 자동 사용) |

### 설치 (한 줄 스크립트 — macOS / Linux)

Homebrew 없이 릴리스 바이너리를 바로 설치한다. Rust 툴체인이 필요 없고, 설치 후에는 `hwp update`로 자체 갱신된다.

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

기본 설치 위치는 `~/.local/bin`이다(PATH에 없으면 추가 안내를 출력한다). 위치·버전은 인자나 환경변수로 바꾼다:

```sh
curl -fsSL .../install.sh | sh -s -- --dir /usr/local/bin --tag v0.3.0
HWP_INSTALL_DIR=~/bin sh scripts/install.sh
```

아카이브는 `.sha256` 자산과 대조한 뒤에만 설치한다. Windows는 아래 [사전 빌드 바이너리](#다운로드-사전-빌드-바이너리)를 쓴다.

### 설치 (Homebrew — macOS / Linux)

저장소 자체가 tap이다(별도 `homebrew-*` 저장소 없음). 릴리스 바이너리를 받으므로 Rust 툴체인이 필요 없다.

```sh
brew tap staixbwlb/hwp https://github.com/STAIxBWLB/hwp-cli
brew install hwp
hwp --version
```

업그레이드는 `brew update && brew upgrade hwp`. 지원 플랫폼은 macOS(Apple Silicon·Intel)와 Linux x86_64이며,
Windows는 아래 [사전 빌드 바이너리](#다운로드-사전-빌드-바이너리)를 쓴다.

### 빌드 / 설치 (소스)

```sh
git clone git@github.com:STAIxBWLB/hwp-cli.git && cd hwp-cli
cargo build --release
cargo install --path crates/hwp-cli   # `hwp` 바이너리 설치
```

> 이후 예시에서 `<repo>`는 위에서 클론한 디렉터리의 절대 경로를 가리킨다.

### 다운로드 (사전 빌드 바이너리)

각 [릴리스](https://github.com/STAIxBWLB/hwp-cli/releases)에 플랫폼별 `hwp` 아카이브와 `.sha256` 체크섬이 첨부된다:

| 플랫폼 | 아카이브 |
|---|---|
| Linux x86_64 | `hwp-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `hwp-vX.Y.Z-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `hwp-vX.Y.Z-x86_64-apple-darwin.tar.gz` |
| Windows x86_64 | `hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip` |

압축을 풀어 `hwp`를 PATH에 두면 된다(체크섬 검증: `shasum -a 256 -c hwp-*.sha256`).
렌더/PDF엔 CJK 폰트가 필요하고(위 폰트 지정 참고), 텍스트 추출·변환은 폰트 없이 동작한다.

### 업데이트

```sh
hwp update            # 최신 릴리스로 자체 교체 (체크섬 대조 후 원자적 교체)
hwp update --check    # 교체 없이 현재/최신 버전만 확인
hwp update --tag v0.2.0   # 특정 버전으로 되돌리기
```

설치 방식을 스스로 판별한다:

| 설치 방식 | `hwp update` 동작 |
|---|---|
| 한 줄 스크립트·릴리스 아카이브·`cargo install` | 릴리스 아카이브를 받아 **실행 중인 바이너리를 제자리 교체**(sha256 대조 → 같은 디렉터리에 임시 파일 → rename, 실패 시 원본 복원) |
| Homebrew(Cellar) | `brew upgrade hwp`에 위임 — Cellar를 직접 덮어써 brew 상태가 어긋나는 것을 막는다. brew는 버전 고정이 안 되므로 `--tag`는 거부한다 |

내려받기는 `curl`, 압축 해제는 `tar`에 위임한다(macOS·Linux·Windows 10 1803+ 기본 탑재 — 새 런타임 의존성 없음).
`hwp update`는 0.4.0부터 있으므로 그 이전 버전은 위 설치 스크립트로 한 번만 갱신하면 된다.

### 릴리스 (메인테이너)

버전은 워크스페이스 `Cargo.toml`의 `[workspace.package] version`이 단일 기준(SSOT)이다.

```sh
scripts/release.sh 0.2.0                        # 버전 bump + 커밋 + 태그 (푸시는 수동)
git push origin main && git push origin v0.2.0  # 태그 푸시가 릴리스를 트리거
```

`vX.Y.Z` 태그를 푸시하면 `release.yml`이 ① 테스트(fmt/clippy/test) 통과 ② 태그↔`Cargo.toml` 버전 일치를 확인한 **뒤에만** 4개 플랫폼 바이너리를 빌드해 GitHub Release로 게시한다. 태그가 커밋된 버전과 다르면 릴리스가 차단된다.

자산 업로드가 끝나면 `update-formula` 잡이 `Formula/hwp.rb`의 version·sha256을 릴리스 체크섬으로 갱신해 main에 커밋한다(brew tap 자동 추종). 잡이 실패했거나 수동 복구가 필요하면 `scripts/update_formula.sh X.Y.Z`를 로컬에서 돌리면 된다 — formula를 손으로 고치지 말 것.

## 빠른 시작 (Quickstart)

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
hwp convert report.hwp   -o report.md    # hwp → markdown (이미지는 report.media/에 추출, 링크는 [텍스트](URL))
hwp convert report.hwp   -o report.md --media-dir figs   # 이미지를 figs/에 추출하고 figs/… 로 링크
hwp convert report.hwp   -o report.md --with-header-footer   # 머리말/꼬리말도 포함 (--with-hidden: 숨은 설명)
hwp convert report.hwp   -o doc.json --embed-bin   # 이미지까지 임베드한 자급식 JSON

# 렌더링 (번들 함초롬 글꼴 자동 로드 — --font-dir 생략 시 HWP_FONT_DIR 또는 ./fonts)
hwp render report.hwp -o page.png --dpi 150
hwp render report.hwp -o page.svg --pages 1-3 --font-dir ./fonts
hwp render report.hwp -o report.pdf --font-dir ./fonts   # 단일 멀티페이지 PDF(검색 가능)
hwp convert report.hwp -o report.pdf   # convert로도 동일(.pdf는 렌더 출력) — 폰트는 시스템 글꼴 사용

# 새 문서 생성
hwp new -o out.hwpx --from notes.md
hwp new -o out.hwp  --from doc.json

# 편집 (이미지·서식·미해석 레코드 보존)
hwp fields form.hwp                        # 채울 수 있는 필드/누름틀 이름 확인
hwp edit form.hwp -o filled.hwp \
    --replace "초안=>최종" \
    --set-cell "0:1:2=12,300원" \
    --set-field "수신처=홍길동" --verify

# 구조 편집 (문단 삽입/삭제·표 행/열 추가, 행 삭제 — 모양 상속; 병합 표는 거부)
hwp edit report.hwp -o out.hwp \
    --insert-para "개요=>추가 설명 문단입니다." \
    --insert-para-before "결론=>결론 직전에 삽입." \
    --delete-para "임시 메모" \
    --add-row "0" --add-col "1" --delete-row "1:3" --verify

# 누름틀 생성 + 채우기 (앵커 텍스트 뒤에 %clk 필드 삽입 후 이름으로 채움)
# 출력이 .hwp든 .hwpx든 필드가 보존된다(hwpx는 OWPML fieldBegin/fieldEnd로 왕복).
hwp edit form.hwp -o out.hwpx \
    --create-field "수신:=>수신처" --set-field "수신처=홍길동" --verify

# 이미지 삽입 (앵커 뒤에 그림 — 자연 크기 또는 "@너비x높이"mm)
hwp edit report.hwp -o out.hwp \
    --insert-image "그림:=>./logo.png" \
    --insert-image "표지:=>./cover.jpg@120x80" --verify

# 렌더 충실도 비교 (한글 기준 PNG와 잉크/오프셋/픽셀 오차)
hwp diff report.hwp --ref hancom_p1.png --page 1 --dpi 150 --font-dir ./fonts

# MCP stdio 서버
hwp mcp --font-dir ./fonts

# 자체 업데이트 (brew 설치본은 brew upgrade에 위임)
hwp update --check
hwp update
```

## 명령 레퍼런스

전체 자동 생성 레퍼런스는 [docs/manual/cli-reference.md](docs/manual/cli-reference.md) — clap 정의에서 생성되며 코드와의 동기화를 CI 테스트가 강제한다. 아래는 요약이다.

| 명령 | 인자 / 플래그 | 설명 |
|---|---|---|
| `info <file>` | `--json` | 포맷/버전/속성/스트림 진단 |
| `cat <file>` | `--format plain\|markdown\|json` (기본 `plain`), `--preview`, `--with-header-footer`, `--with-hidden`, `--with-segments`(md 전용) | 본문 추출. `--preview`는 본문 파싱 없이 PrvText만 출력. `--with-*`는 머리말/꼬리말·숨은 설명 포함(기본 제외). `--with-segments`는 markdown과 함께 각 출력 문자 범위의 원본 좌표를 한 줄 JSON 봉투로 낸다(아래 [`--with-segments`](#--with-segments-추출-근거-좌표) 참고) |
| `convert <input> -o <output>` | `--to hwp\|hwpx\|md\|json`(생략 시 확장자 추론), `--strict`, `--preserve-layout`, `--embed-bin`, `--media-dir <dir>`(md), `--with-header-footer`(md), `--with-hidden`(md) | 포맷 변환. 출력이 `.pdf`이면 렌더 경로로 위임(시스템 글꼴 사용 — 정밀 글꼴은 `render --font-dir` 권장). `--preserve-layout`는 무수정 왕복 전용 줄 배치 보존. `--embed-bin`은 JSON에 이미지 base64 임베드. md 출력은 `--media-dir figs`처럼 이미지 추출 디렉터리 지정 가능(기본 `<스템>.media`, 상대경로는 출력 파일 기준·링크는 Markdown에 안전하게 직렬화). `--strict`는 HWP/HWPX 변환에서 보존 불가(`DROP:`) 데이터를 발견하면 destination을 게시하지 않고 실패 |
| `render <input> -o <output>` | `--pages "1"\|"1-3"\|"all"`(기본 `all`), `--dpi <f64>`(기본 96, 유한한 36..=600), `--format png\|svg\|pdf`(생략 시 확장자 추론), `--font-dir <dir>`(반복) | 페이지를 PNG/SVG(페이지별 파일)·PDF(단일 멀티페이지)로 렌더. PNG/SVG 다중 페이지는 전체 파일 집합을 검증한 뒤 한 게시 단위로 교체하며, 후반 실패 시 기존 집합을 복원. 번호 목록은 형식 템플릿(`^1.`→"1.", `(^5)`→"(5)", `제^1조`→"제1조")을 적용 |
| `new -o <output>` | `--from <md\|json>`(생략 시 빈 문서) | markdown/JSON IR에서 새 문서 생성. markdown 목록은 진짜 번호(NUMBER)/글머리(BULLET) 머리 문단으로 들여오고(중첩=수준), H1~H3 제목엔 절 번호(`1.`/`1-1.`/`1-1-1.`, 숫자 시작 제목은 생략)를 접두한다 |
| `compose <spec> -o <output>` | `--format json\|yaml`(생략 시 확장자 추론), `--dry-run`, `--report`, `--allow-visual-fallback`(deprecated, v1 호환 전용) | DocumentSpec v1/v2 구조 문서를 HWP/HWPX로 deterministic 합성. v2 visual policy는 spec의 타겟별 설정이 SSOT이며 deprecated flag를 같이 주면 `policy_conflict`로 거부 |
| `template <template> --data <data> -o <output>` | `--template-format json\|yaml`, `--data-format json\|yaml`, `--dry-run`, `--report` | TemplateSpec/Data v1의 typed AST를 bounded expansion해 native HWP/HWPX 생성. `reference_hwpx`는 기존 패키지의 지정 텍스트/필드만 외과적으로 채우며, 구조 재생성은 명시적 strict gate 필요 |
| `edit <input> -o <output>` | `--replace "찾기=>바꾸기"`, `--set-cell "표:행:열=값"`(0-기반), `--set-field "이름=값"`, `--create-field "앵커=>이름"`(또는 `"앵커=>이름=값"`, %clk 누름틀 생성), `--insert-image "앵커=>경로"`(또는 `"앵커=>경로@너비x높이"`mm, png/jpg/bmp/gif 삽입), `--seal "앵커=>경로"`(또는 `"앵커=>경로@크기mm"`, 도장 이미지를 앵커 문구 위에 글 앞 부유 배치 — 기본 20mm), `--set-format "찾기:bold=on,size=16,color=#RRGGBB"`, `--set-align "찾기=left\|right\|center\|justify\|distribute\|divide"`, `--insert-para "앵커=>텍스트"`(앵커 문단 뒤), `--insert-para-before "앵커=>텍스트"`(앞), `--delete-para "텍스트"`, `--add-row "표"`, `--add-col "표"`(또는 `"표:위치"`), `--delete-row "표:행"`, `--delete-col "표:열"`, `--merge-cells "표:r1:c1:r2:c2"`(사각 영역 병합), `--split-cell "표:행:열"`(병합 해제), `--verify` (모두 반복 가능) | 기존 문서 편집. 텍스트·서식·구조(문단/표 행·열·셀 병합/분할) 편집. 삽입 문단·행은 앵커/템플릿 모양을 상속하고 합성 경로로 저장(불변식 적용). 표 인덱스는 **재귀 깊이 우선**(중첩 표 포함, --set-cell과 동일). 열 추가·삭제·셀 병합/분할은 **병합 셀 표도 지원**(정품 1,816개 실측 규칙 — 피병합 셀 생략·행 우선·병합 cellSz). `--add-col`은 전체 표 폭을 유지하며 기존 열을 균등 축소(잔차는 행 마지막 기존 셀); 위치 생략 시 표 끝에 추가. `--add-row`는 병합 없는 템플릿 행이 필요(없으면 거부). `--replace`만 있고 hwpx→hwpx이면 **패키지 보존 고속 경로**(미리보기·호환 블록 바이트 유지, 런 분절 교차 매칭 미지원). `--verify`는 쓰기 후 재읽기로 검증 |
| `fields <file>` | `--json` | 필드/누름틀 목록(이름·종류·값·명령) |
| `bookmarks <file>` | `--json` | 책갈피(bokm) 목록(이름) |
| `slots <file>` | `--json` | `{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록 |
| `fill <input> -o <output>` | `--set "이름=값"`(반복), `--data <json>`, `--json` | 충실도 보존 템플릿 채우기 — hwpx의 `{{name}}` 치환(패키지 보존). `--data`는 이름→값 JSON 객체 파일로 일괄 채움 |
| `validate <file>` | `--json` | 구조 검증(mimetype·필수 엔트리·XML 파싱) — 유효 시 종료코드 0 |
| `certify <input> --policy <file> --report <dir>` | versioned JSON/YAML policy | package·반복 import·bounded native render와 선택적 독립 import를 인증해 새 디렉터리를 원자적으로 게시. 상세 계약은 [Certification v1](docs/design/16-certification-v1.md) 참고 |
| `corpus --manifest <file> --report <dir>` | frozen structured corpus v1 | 자체 작성 한국어 7종 문서를 HWPX/HWP로 각각 2회 생성하고 두 출력 모두 재개방·구조검증·native 인증. 명시된 OFL 폰트만 사용하며 결과/렌더 hash를 bounded 원자적 report로 게시. [코퍼스 계약](docs/design/17-structured-corpus-v1.md) 참고 |
| `diff <input> --ref <png>` | `--page <n>`(기본 1), `--dpi <f64>`(기본 96, 유한한 36..=600), `-o/--out <png>`, `--font-dir <dir>`(반복), `--tolerance <u8>`(기본 16) | 렌더 결과를 한글 기준 PNG와 비교(잉크 적용률·dx/dy 오프셋·픽셀 차이율·MAE) |
| `mcp` | `--font-dir <dir>`(반복) | MCP stdio 서버 실행 |
| `update` | `--check`, `--tag <tag>`, `--force`, `--json` | 자체 업데이트 — 릴리스 아카이브를 받아(체크섬 대조) 실행 중인 바이너리 교체. brew 설치본은 `brew upgrade`에 위임 |
| `dump <file>` | `--stream <name>`, `--raw`, `--json` | [개발자용] 레코드/패키지 구조 덤프 |

> 출력 포맷은 대부분 출력 파일의 확장자(`.hwp` / `.hwpx` / `.md` / `.json` / `.png` / `.svg`)에서
> 추론된다. `convert`/`render`는 `--to`/`--format`으로 명시할 수도 있다.

## Markdown 내보내기 (`convert --to md`)

hwp/hwpx를 GFM(GitHub-Flavored Markdown)으로 변환한다. 같은 변환기를 쓰는
`hwp cat <file> --format markdown`은 stdout 출력 전용이라 이미지를 추출하지 않는다(빈 참조 `![image]()`).

```sh
hwp convert report.hwp -o report.md                    # 이미지는 report.media/에 추출
hwp convert report.hwp -o report.md --media-dir figs   # figs/에 추출하고 figs/image1.png 식으로 링크
hwp convert report.hwp -o full.md --with-header-footer --with-hidden
```

`--media-dir`은 상대경로면 **출력 파일 기준**으로 해석하고(`-o docs/report.md --media-dir figs` →
`docs/figs/`에 추출), 링크는 경로 구분자를 `/`로 통일하고 공백·괄호를 Markdown에 안전하게
직렬화한다. 같은 이름의 파일이 이미 있으면 바이트가 같을 때만 재사용하며, 내용이 다르면 기존
파일을 덮어쓰지 않고 변환을 실패시킨다.

HWP 요소 → markdown 매핑:

| HWP 요소 | markdown |
|---|---|
| `개요 N` 스타일 문단 | `#` × N 헤딩 |
| 굵게 / 기울임 | `**굵게**` / `*기울임*` |
| 취소선 / 밑줄 / 위·아래첨자 | `~~취소선~~` / `<u>밑줄</u>` / `<sup>위</sup>`·`<sub>아래</sub>` |
| 하이퍼링크(%hlk) | `[표시텍스트](URL)` |
| 이미지 | `![image](<media>/imageN.png)` — 바이트는 추출, 확장자는 매직 바이트로 판별 |
| 표(병합 없음) | GFM 파이프 표 (첫 행 헤더) |
| 표(병합 셀·중첩 표·블록 수식 있음) | HTML `<table>` (colspan/rowspan, 셀 블록을 등장 순서대로 보존) |
| 글머리표 / 번호 문단 | `- ` / `N. ` 목록 (부모 마커의 본문 시작 열에 맞춘 들여쓰기, 번호 형식은 numbering 정의에서 합성 — `가.`·`제1조` 등 숫자 외 형식은 `- 가. `처럼 리터럴 마커로 보존) |
| 각주 / 미주 | 일반 본문은 `[^N]` / `[^eN]` GFM 풋노트, HTML 표 안은 대응하는 HTML 링크·정의 |
| 수식(eqed) | 인라인 `$스크립트$`, 블록 `$$스크립트$$` — **HWP 수식 스크립트 원문**(LaTeX 아님) |
| 머리말/꼬리말·숨은 설명 | 기본 제외, `--with-header-footer` / `--with-hidden`으로 포함 |

**표 직렬화 보장**: HTML 표의 각 `<tr>…</tr>` 행과 GFM 파이프 표의 각 행은 항상 한 줄로
직렬화된다(행 = 연속된 한 줄 — 셀 안 중첩 표도 그 줄에 인라인으로 얹힌다). 소비자가 행 단위로
안정적으로 인용·파싱할 수 있도록 테스트로 고정돼 있다.

한계: 헤딩 인식은 `개요 N` 스타일뿐이고(사용자 정의 제목 스타일은 본문 취급), 떠 있는(floating)
개체의 위치·z-order는 반영하지 않으며, 역방향(md → hwp)은 표·굵게 등 기본 구문만 복원된다.

### `--with-segments` (추출 근거 좌표)

`hwp cat <file> --format markdown --with-segments`는 markdown과 함께 **각 출력 문자 범위가
어느 원본 문단에서 왔는지**를 한 줄 컴팩트 JSON 봉투로 낸다. 외부 소비자가 추출 근거를
"verbatim 인용 + 원본 좌표(섹션/문단)"로 기록할 때 쓴다.

```json
{"markdown": "...", "segments": [
  {"kind": "para", "section": 0, "para": 12, "start": 345, "end": 512},
  {"kind": "para", "section": 0, "para": 13, "start": 514, "end": 590}
]}
```

계약 사항:

- **오프셋은 유니코드 스칼라(문자) 단위** — Python `str` 인덱싱과 동일하다(바이트 아님).
  `markdown[start:end]`로 그대로 슬라이스하면 그 문단의 출력 조각을 얻는다.
- **좌표는 IR 인덱스** — `section`/`para`는 `--format json`으로 얻는 `sections[]`/`paragraphs[]`
  (최상위 문단) 인덱스라 재디코드에도 안정적이다.
- **정렬·비중첩** — 세그먼트는 `start` 오름차순이며 서로 겹치지 않는다.
- **간극 허용** — 어느 문단에도 귀속되지 않는 출력(빈 줄, 구역 구분 주석 등)은 세그먼트 사이
  간극으로 남는다.
- **표가 만든 줄은 표를 담은 문단 인덱스를 상속**한다(표 블록 전체가 그 문단 범위 안).
- **각주/미주 정의는 참조 문단에 귀속**된다(문서 끝의 정의 줄도 참조한 문단 좌표를 갖는다).
- **머리말/꼬리말은 기본 제외** — `--with-header-footer`(및 `--with-hidden`)를 함께 주면 포함되고,
  그 범위도 세그먼트로 잡힌다.
- `markdown` 필드는 `--with-segments` 없이 실행한 markdown 출력과 **바이트 단위로 동일**하다.
- `kind`는 현재 항상 `"para"`(미래 확장용 필드).

`--with-segments`는 markdown 전용이라 다른 `--format`이나 `--preview`와 함께 쓰면 에러가 난다.

## DocumentSpec v1/v2 구조 문서 합성

`compose`는 v1 JSON/YAML 명세를 문단·런·스타일·목록·표·수식·필드·머리말/꼬리말·쪽 번호·구역으로 컴파일한다. v2는 그 문서를 감싸고 접근성 설명이 포함된 이미지, 닫힌 SVG→PNG fallback, HWPX 네이티브 사각형 글상자를 더한다. v2 fallback policy는 `policy.hwp`/`policy.hwpx`에서 타겟별로 명시하며 생략 시 native-only로 실패한다.

```bash
# 쓰지 않고 닫힌 스키마, 참조, 표 span, 자산, 네이티브 지원 여부 확인
hwp compose examples/document-spec-v1/basic.json \
  -o /tmp/basic.hwpx --dry-run --report

# 검증된 파일을 원자적으로 게시
hwp compose examples/document-spec-v1/comprehensive.yaml \
  -o /tmp/report.hwpx --report

# v2 닫힌 SVG를 deterministic PNG로 합성
hwp compose examples/document-spec-v2/basic.json \
  -o /tmp/visual.hwpx --report
```

v1 SSOT는 [`document-spec-v1.schema.json`](schemas/document-spec-v1.schema.json)과 [`13-document-spec-v1.md`](docs/design/13-document-spec-v1.md), v2 SSOT는 [`document-spec-v2.schema.json`](schemas/document-spec-v2.schema.json), [`document-report-v2.schema.json`](schemas/document-report-v2.schema.json), [`15-document-spec-v2.md`](docs/design/15-document-spec-v2.md)다. 자산 경로는 명세 파일 디렉터리 기준의 relative normal component만 허용한다.

`template`은 문자열 보간이나 표현식 실행 없이 typed `value`/`if`/`each` AST만 허용한다.
예제는 [`examples/template-spec-v1/`](examples/template-spec-v1/), normative schema는
[`template-spec-v1.schema.json`](schemas/template-spec-v1.schema.json),
[`template-data-v1.schema.json`](schemas/template-data-v1.schema.json),
[`template-report-v1.schema.json`](schemas/template-report-v1.schema.json)에 있다. 실행·보존 계약과
고정 제한은 [`docs/design/14-template-spec-v1.md`](docs/design/14-template-spec-v1.md)를 따른다.

## JSON IR로 문서 재생성 (regen)

기존 문서를 편집하지 않고 **처음부터 똑같이 신규 생성**할 수 있다 — JSON IR이 무손실
왕복을 보장하고, hwpx read/write 경계의 손실(secPr·tabPr·각주/미주)은 원문 보존으로 해소됐다.

```sh
hwp convert report.hwpx -o report.json   # hwp/hwpx → JSON IR (--embed-bin: 이미지까지 임베드)
hwp new --from report.json -o regen.hwpx # JSON IR → 신규 문서
```

재생성 검증(자동, `crates/hwp-cli/tests/regen.rs`): validate 통과, `hwp cat` 출력 전문 동일,
표 지도(개수·행/열·병합·셀 폭) 동일, secPr·tabProperties 슬라이스 바이트 동일.
차이가 남는 것은 설계상 재생성 대상인 줄 배치 캐시(linesegarray — 한글이 열 때 재계산),
미리보기 이미지, settings.xml뿐이다. markdown 경로(`hwp new --from md`)는 병합 표·폰트·
글자 색 같은 표현력 밖 요소를 재현하지 않는다(RISE 스타일 veneer까지).

## MCP 서버 (AI 에이전트 연동)

`hwp mcp`는 tokio나 SDK 없이 `serde_json`만으로 동기 JSON-RPC 2.0(stdio, 줄 단위)을 구현한 **MCP
서버**다(프로토콜 버전 `2024-11-05`). stdout은 프로토콜 전용이고 로그는 stderr로 나간다.
Windows/한컴이 필요한 COM 자동화와 달리 크로스플랫폼 오픈 엔진으로 동작한다.

### 노출 도구 (14종)

| 도구 | 필수 인자 | 기능 |
|---|---|---|
| `hwp_info` | `path` | 포맷/버전/속성/스트림 진단(JSON) |
| `hwp_read` | `path` (`format`, `offset`, `max_bytes`) | 본문 추출. `json`이면 전체 IR 구조. UTF-8 byte 단위 페이지네이션이며 기본 256 KiB, 호출당 최대 1 MiB |
| `hwp_list_fields` | `path` | 필드/누름틀 목록(이름·종류·값·명령) |
| `hwp_list_bookmarks` | `path` | 책갈피(bokm) 목록(이름) |
| `hwp_render` | `path` (`page`, `dpi`, `font_dir`) | 지정 페이지만 PNG로 렌더해 반환. `dpi`는 유한한 36..=600, 응답 PNG는 최대 16 MiB |
| `hwp_edit` | `input`, `output` (편집 배열, `allow_partial`) | typed JSON 작업으로 치환, 셀·필드·metadata, 책갈피·링크·이미지·도장, 서식·정렬·문단, 표 행/열/병합을 strict atomic 편집. `add_col[].at`은 0-기반 삽입 위치 |
| `hwp_convert` | `input`, `output` (`embed_bin`, `strict`) | 포맷 변환(확장자로 결정). MCP의 `strict` 기본값은 true |
| `hwp_new` | `output` (`markdown` 또는 `json`, `set_meta[]`) | markdown/JSON IR과 metadata에서 새 문서 생성 |
| `hwp_compose` | `output`, `spec` 또는 `spec_path` (`format`, `base_dir`, `dry_run`, `allow_visual_fallback`) | DocumentSpec v1/v2 객체/JSON/YAML을 CLI와 같은 deterministic·strict·atomic 경로로 HWP/HWPX 합성. `allow_visual_fallback`은 deprecated v1 호환 전용이며 v2에서는 `policy_conflict` |
| `hwp_template` | `output`, `template` 또는 `template_path`, `data` 또는 `data_path` (`template_format`, `data_format`, `base_dir`, `dry_run`) | TemplateSpec/Data v1을 CLI와 같은 bounded·deterministic·strict 경로로 확장. reference fill은 package-surgical 보존 및 단일 source/destination snapshot 사용 |
| `hwp_diff` | `input`, `ref` (`page`, `dpi`, `font_dir`) | 지정 페이지만 렌더해 기준 PNG와 비교. `dpi`는 유한한 36..=600 |
| `hwp_slots` | `path` | `{{name}}` 자리표시자(템플릿 슬롯) 목록을 등장 순서로 |
| `hwp_fill` | `input`, `output`, `values` (`allow_partial`) | hwpx 템플릿의 `{{name}}` 치환(패키지·미리보기 보존, hwpx 전용). 기본은 미발견 키 하나라도 있으면 실패 |
| `hwp_validate` | `path` | 구조 검증(mimetype·필수 엔트리·XML 파싱) — `{valid, errors, warnings}` |

### 클라이언트 설정 예

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--font-dir", "<repo>/fonts"]
    }
  }
}
```

### AI read → edit → rewrite 왕복

1. **읽기** — `hwp_read`(`format=json`)가 문서 전체를 IR(JSON)로 내보낸다. 텍스트뿐 아니라 표·이미지
   참조·서식·미해석 레코드까지 구조적으로 담긴다.
2. **편집** — 에이전트는 `hwp_edit`로 텍스트 치환·표 셀 설정·누름틀 채우기를 적용한다. IR만 바꾸므로
   이미지·서식·opaque 레코드가 보존되고, 편집된 문단의 줄 배치만 무효화되어 writer가 재합성한다.
   `hwp_new`/`hwp_convert`로 JSON IR을 그대로 문서로 되쓸 수도 있다.
3. **확인** — `hwp_render`가 결과 페이지를 PNG로 돌려주어 에이전트가 변경을 **눈으로 검증**한다.
   `hwp_diff`로 한글 기준 렌더와 정량 비교할 수 있다.

편집된 hwp는 writer의 합성 경로를 거쳐 한글 문단 불변식(줄 배치·문단끝 `0x0d`·nchars 등)을 다시
세우므로 한글에서 정상 문서로 열린다.

## 워크스페이스 구성

| 크레이트 | 역할 |
|---|---|
| `hwp-model` | 공유 문서 모델(IR) — 모든 크레이트가 의존하는 단일 계약. `Document{meta,header,sections,bin_streams}`, 무손실 보존(opaque/tail), 단위 변환 |
| `hwp5` | HWP 5.0 바이너리 reader/writer (CFB 컨테이너 + 레코드 스트림 + 압축) |
| `hwpx` | HWPX reader/writer (ZIP 패키지 + OWPML XML) |
| `hwp-convert` | IR ↔ markdown / JSON, 인메모리 편집(치환·셀·필드), 필드 스캔 |
| `hwp-render` | IR → PNG / SVG / PDF 렌더러, 줄 배치 합성, 텍스트 셰이핑, 폰트 서브셋·임베드, 렌더 diff |
| `hwp-cli` | `hwp` 바이너리 (CLI + MCP 서버) |

## 개발과 테스트

```sh
cargo build --all-targets
scripts/check.sh                        # 로컬 CI 미러: fmt + clippy + test (PR 전 필수)
```

테스트는 hwp5의 바이트 동일 왕복(identity/roundtrip/synth), hwpx 의미 동등 왕복, IR JSON·markdown
왕복, 편집/필드 보정, 렌더 레이아웃·표·diff 메트릭 등을 포함한다.

HWP 5.0 포맷 스펙은 한컴 공식 [한글 문서 파일 형식 5.0](https://store.hancom.com/etc/hwpDownload.do)
문서를 참고한다 — 저작권상 저장소에 동봉하지 않고 공식 배포처 링크만 둔다(`docs/README.md` 참고).

**CI** (`.github/workflows/ci.yml`, GitHub Actions): `fonts-noto-cjk` 설치(ubuntu) 후
`cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
`cargo test --workspace`를 **ubuntu + macOS**(필수)와 **windows**(참고용, 비차단)에서 실행한다.
로컬 미러 `scripts/check.sh`는 **문자 그대로 같은 3커맨드**를 실행한다(PR 전 필수).

## 기여

버그 리포트와 PR을 환영한다. 이슈는 GitHub Issues에, 변경은 PR로 제출한다.

- **브랜치·PR 정책**: 기능추가·수정·문서 등 모든 작업은 `feat/`·`fix/`·`docs/` 브랜치에서
  하고, PR로 제출한다. main 직접 push는 하지 않는다. CI green을 확인한 뒤 squash
  머지한다(머지 커밋 제목의 `(#N)` 관례 유지). 포크에서 작업하는 경우 PR 대상은
  이 저장소(upstream)의 main이다.
- PR 전 로컬 게이트는 `scripts/check.sh` 한 방으로 통과시킨다(CI와 동일 3커맨드:
  `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` →
  `cargo test --workspace`).
- 새 포맷 기능은 가능하면 왕복/골든 테스트를 함께 추가한다.
- 스펙 참고 자료는 한컴 공식 [한글 문서 파일 형식 5.0](https://store.hancom.com/etc/hwpDownload.do)
  문서를 본다(저장소에 동봉하지 않는다).

## 고지 (Acknowledgments)

본 제품은 한글과컴퓨터의 한글 문서 파일(`.hwp`) 공개 문서를 참고하여 개발하였습니다.

> This product was developed with reference to Hancom's HWP document file format open
> specification — [한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0](https://store.hancom.com/etc/hwpDownload.do)
> (© (주)한글과컴퓨터).

한컴 공개 문서의 저작권은 (주)한글과컴퓨터에 있다. 한컴 공개 문서 라이선스는 자유로운 열람·복사·
배포를 허용하되 **수정되지 않은 원본/복사본**으로 제한하므로, 이 저장소는 스펙 문서(및 그 추출본·
페이지 캡처 등 파생물)를 **동봉하지 않고** 공식 배포처 링크만 제공한다(`docs/README.md` 참고).

테스트 픽스처 일부는 [hahnlee/hwp-rs](https://github.com/hahnlee/hwp-rs)(Apache-2.0)에서 가져왔다 —
`fixtures/README.md`와 루트 `NOTICE` 참고.

## 라이선스

이 프로젝트는 듀얼 라이선스다 — [MIT](LICENSE-MIT) 또는 [Apache-2.0](LICENSE-APACHE) 중 하나를
선택할 수 있다(워크스페이스 `Cargo.toml`에 `MIT OR Apache-2.0`으로 선언). 별도 명시가 없는 한, 이
저장소에 기여한 코드는 위 두 라이선스로 동일하게 배포되는 것에 동의하는 것으로 간주한다.
