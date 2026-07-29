<!-- 자동 생성 문서 — 수동 편집 금지. 재생성: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference -->

# hwp CLI 명령 레퍼런스

이 문서는 `hwp` CLI의 clap 정의에서 자동 생성된다. 직접 편집하지 말고, 명령·플래그가 바뀌면 `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`로 재생성하라 — CI 테스트가 코드와 문서의 동기화를 강제한다.

## 명령 색인

- [`hwp info`](#hwp-info)
- [`hwp cat`](#hwp-cat)
- [`hwp convert`](#hwp-convert)
- [`hwp render`](#hwp-render)
- [`hwp new`](#hwp-new)
- [`hwp diff`](#hwp-diff)
- [`hwp edit`](#hwp-edit)
- [`hwp fields`](#hwp-fields)
- [`hwp bookmarks`](#hwp-bookmarks)
- [`hwp slots`](#hwp-slots)
- [`hwp fill`](#hwp-fill)
- [`hwp validate`](#hwp-validate)
- [`hwp mcp`](#hwp-mcp)
- [`hwp update`](#hwp-update)
- [`hwp dump`](#hwp-dump)

## `hwp info`

파일 정보 표시: 포맷/버전/속성/스트림 목록

**사용법:** `hwp info [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--json` |  |  | JSON으로 출력 |

## `hwp cat`

텍스트 추출

**사용법:** `hwp cat [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--format` | `plain` \| `markdown` \| `json` \| `html` | `plain` |  |
| `--preview` |  |  | 본문 파싱 없이 PrvText 미리보기만 출력 |
| `--with-header-footer` |  |  | 머리말/꼬리말 텍스트도 추출에 포함 (기본: 제외) |
| `--with-hidden` |  |  | 숨은 설명 텍스트도 추출에 포함 (기본: 제외) |
| `--with-segments` |  |  | (markdown 전용) markdown과 함께 각 출력 문자 범위의 원본 좌표(섹션/문단)를 한 줄 JSON 봉투로 출력 — {"markdown": ..., "segments": [...]} |

## `hwp convert`

포맷 변환

**사용법:** `hwp convert [OPTIONS] --output <OUTPUT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  |  |
| `-o, --output` | `<OUTPUT>` |  |  |
| `--to` | `hwp` \| `hwpx` \| `md` \| `json` \| `html` \| `pdf` \| `odt` |  | 출력 포맷 (생략 시 확장자에서 추론) |
| `--strict` |  |  | 변환 중 보존 불가능한(opaque) 데이터 발견 시 실패 처리 |
| `--preserve-layout` |  |  | 줄 배치 캐시 보존 (무수정 왕복 전용 — 한글은 내용과 어긋난 줄 배치를 변조로 판정하므로 기본은 제거) |
| `--embed-bin` |  |  | JSON 출력 시 첨부 바이너리(이미지)를 base64로 임베드 (자급식 JSON) |
| `--media-dir` | `<MEDIA_DIR>` |  | (md) 이미지 추출 디렉터리 — 기본 "<출력스템>.media". 상대경로는 출력 파일 기준으로 해석하고 링크는 입력한 경로 그대로 쓴다 (예: figs) |
| `--with-header-footer` |  |  | (md) 머리말/꼬리말 텍스트도 포함 (기본: 제외) |
| `--with-hidden` |  |  | (md) 숨은 설명 텍스트도 포함 (기본: 제외) |
| `--font-dir` | `<FONT_DIR>` |  | (pdf) 추가 폰트 디렉터리 (반복 가능, 기본: HWP_FONT_DIR 또는 fonts/) |

## `hwp render`

페이지 렌더링

**사용법:** `hwp render [OPTIONS] --output <OUTPUT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  |  |
| `-o, --output` | `<OUTPUT>` |  |  |
| `--pages` | `<PAGES>` | `all` | 페이지 범위: "1", "1-3", "all" |
| `--dpi` | `<DPI>` | `96` |  |
| `--format` | `png` \| `svg` \| `pdf` |  | 출력 포맷 (생략 시 확장자에서 추론) |
| `--font-dir` | `<FONT_DIR>` |  | 추가 폰트 디렉터리 (반복 가능) |

## `hwp new`

새 문서 생성

**사용법:** `hwp new [OPTIONS] --output <OUTPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `-o, --output` | `<OUTPUT>` |  |  |
| `--from` | `<FROM>` |  | 입력 markdown/JSON 파일 (생략 시 빈 문서) |
| `--set-meta` | `<SET_META>` |  | 메타데이터 설정 "키=값" (키: title\|author\|subject\|keywords, 반복 가능) |
| `--preset` | `gian` \| `report` |  | 공문서 프리셋 (markdown 입력 전용): gian=기안문(맑은 고딕 11.5pt), report=보고서(함초롬바탕 15pt). 여백·4단계 번호·쪽번호 포함 |

## `hwp diff`

렌더 결과를 한글 기준 PNG와 비교해 오차 측정 (위치 오프셋·픽셀 차이율)

**사용법:** `hwp diff [OPTIONS] --ref <REF> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  |  |
| `--ref` | `<REF>` |  | 한글에서 같은 페이지를 같은 DPI로 내보낸 기준 PNG |
| `--page` | `<PAGE>` | `1` | 비교할 페이지 (1-기반) |
| `--dpi` | `<DPI>` | `96` |  |
| `-o, --out` | `<OUT>` |  | 차이 이미지 출력 경로 (생략 시 <ref>.diff.png) |
| `--font-dir` | `<FONT_DIR>` |  | 추가 폰트 디렉터리 (반복 가능) |
| `--tolerance` | `<TOLERANCE>` | `16` | 채널 차이 허용 오차 (이하면 동일 취급) |

## `hwp edit`

기존 문서 편집 (텍스트 치환·표 셀 설정) — 이미지·서식 보존

**사용법:** `hwp edit [OPTIONS] --output <OUTPUT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  |  |
| `-o, --output` | `<OUTPUT>` |  |  |
| `--replace` | `<REPLACE>` |  | 텍스트 치환 "찾기=>바꾸기" (반복 가능, 모든 일치 치환) |
| `--set-cell` | `<SET_CELL>` |  | 표 셀 설정 "표:행:열=값" (반복 가능, 0-기반 인덱스) |
| `--set-field` | `<SET_FIELD>` |  | 필드/누름틀 채우기 "이름=값" (반복 가능 — hwp fields로 이름 확인) |
| `--set-meta` | `<SET_META>` |  | 메타데이터 설정 "키=값" (키: title\|author\|subject\|keywords, 반복 가능) |
| `--create-field` | `<CREATE_FIELD>` |  | 누름틀 생성 "앵커=>이름" 또는 "앵커=>이름=값" — 앵커 텍스트 뒤에 %clk 필드 삽입 (반복 가능) |
| `--create-bookmark` | `<CREATE_BOOKMARK>` |  | 책갈피 생성 "앵커=>이름" — 앵커 텍스트 뒤에 bokm 지점 표식 삽입 (반복 가능) |
| `--create-hyperlink` | `<CREATE_HYPERLINK>` |  | 하이퍼링크 생성 "앵커=>URL" 또는 "앵커=>표시=>URL" — 앵커 뒤에 %hlk 삽입 (반복 가능) |
| `--insert-image` | `<INSERT_IMAGE>` |  | 이미지 삽입 "앵커=>경로" 또는 "앵커=>경로@너비x높이"(mm) — 앵커 뒤에 그림 삽입 (반복 가능) |
| `--seal` | `<SEAL>` |  | 도장 날인 "앵커=>경로" 또는 "앵커=>경로@크기mm" — 앵커 문구 위에 도장 부유 배치 (반복 가능) |
| `--set-format` | `<SET_FORMAT>` |  | 글자 서식 "찾기:속성=값,…" (예: "제목:bold=on,size=16,color=#FF0000") (반복 가능) |
| `--set-align` | `<SET_ALIGN>` |  | 문단 정렬 "찾기=정렬" (left/right/center/justify/distribute) (반복 가능) |
| `--insert-para` | `<INSERT_PARA>` |  | 문단 삽입 "앵커=>텍스트" — 앵커가 있는 문단 뒤에 새 문단 (반복 가능) |
| `--insert-para-before` | `<INSERT_PARA_BEFORE>` |  | 문단 삽입(앞) "앵커=>텍스트" — 앵커가 있는 문단 앞에 새 문단 (반복 가능) |
| `--delete-para` | `<DELETE_PARA>` |  | 문단 삭제 "텍스트" — 텍스트가 있는 문단 삭제 (반복 가능) |
| `--add-row` | `<ADD_ROW>` |  | 표 행 추가 "표" — N번째 표 끝에 빈 행 (반복 가능, 0-기반; 병합 셀이 있는 표는 거부) |
| `--add-col` | `<ADD_COL>` |  | 표 열 추가 "표"(끝에) 또는 "표:위치"(삽입) — 전체 폭 유지(기존 열 균등 축소). 병합 셀 표도 지원 (반복 가능, 0-기반) |
| `--delete-row` | `<DELETE_ROW>` |  | 표 행 삭제 "표:행" — N번째 표의 R행 (반복 가능, 0-기반; 병합 행은 거부) |
| `--delete-col` | `<DELETE_COL>` |  | 표 열 삭제 "표:열" — N번째 표의 열 삭제. 전체 폭 유지(남은 열에 재분배). 병합 셀은 축소 (반복 가능, 0-기반) |
| `--merge-cells` | `<MERGE_CELLS>` |  | 셀 병합 "표:r1:c1:r2:c2" — 사각 영역을 좌상단 앵커로 병합 (반복 가능, 0-기반) |
| `--split-cell` | `<SPLIT_CELL>` |  | 셀 분할 "표:행:열" — 병합 셀을 1×1로 분해 (반복 가능, 0-기반) |
| `--verify` |  |  | 쓰기 후 재읽기로 검증 |

## `hwp fields`

필드/누름틀 목록 표시 (이름·종류·값)

**사용법:** `hwp fields [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--json` |  |  | JSON으로 출력 |

## `hwp bookmarks`

책갈피 목록 표시 (이름)

**사용법:** `hwp bookmarks [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--json` |  |  | JSON으로 출력 |

## `hwp slots`

`{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록 표시

**사용법:** `hwp slots [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--json` |  |  | JSON으로 출력 |

## `hwp fill`

충실도 보존 템플릿 채우기 (hwpx의 `{{name}}` 치환, 패키지 보존)

**사용법:** `hwp fill [OPTIONS] --output <OUTPUT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  |  |
| `-o, --output` | `<OUTPUT>` |  |  |
| `--set` | `<SET>` |  | 자리표시자 채우기 "이름=값" (반복 가능; `{{이름}}` 치환) |
| `--data` | `<DATA>` |  | 이름→값 JSON 객체 파일 (일괄 채우기) |
| `--json` |  |  | 치환 요약을 JSON으로 출력 ({output, replaced, counts}) |

## `hwp validate`

구조 검증 (mimetype/필수 엔트리/XML 파싱) — 유효하면 종료코드 0

**사용법:** `hwp validate [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--json` |  |  | JSON으로 출력 |

## `hwp mcp`

MCP(Model Context Protocol) stdio 서버 — AI 에이전트용 도구 인터페이스

**사용법:** `hwp mcp [OPTIONS]`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `--font-dir` | `<FONT_DIR>` |  | 렌더/diff 도구의 기본 폰트 디렉터리 (반복 가능) |

## `hwp update`

자체 업데이트 — GitHub 릴리스에서 최신 `hwp`를 받아 실행 중인 바이너리를 교체

**사용법:** `hwp update [OPTIONS]`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `--check` |  |  | 교체 없이 현재/최신 버전만 확인 |
| `--tag` | `<TAG>` |  | 특정 릴리스로 고정 (예: "v0.2.0" — 이전 버전으로 되돌릴 때) |
| `--force` |  |  | 같은 버전이어도 다시 받아 교체 (손상된 설치 복구용) |
| `--json` |  |  | JSON으로 출력 |

## `hwp dump`

[개발자용] 레코드/패키지 구조 덤프

**사용법:** `hwp dump [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  |  |
| `--stream` | `<STREAM>` |  | 대상 스트림/엔트리 (예: "DocInfo", "BodyText/Section0", "Contents/header.xml") |
| `--raw` |  |  | 레코드 페이로드를 hex로 출력 |
| `--json` |  |  | JSON으로 출력 |
