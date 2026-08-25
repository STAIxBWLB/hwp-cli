<!-- 자동 생성 문서 — 수동 편집 금지. 재생성: HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference -->

[한국어](cli-reference.ko.md) · [English](cli-reference.md)

# hwp CLI 명령 레퍼런스

이 문서는 `hwp` CLI의 clap 정의에서 자동 생성된다. 직접 편집하지 말고, 명령·플래그가 바뀌면 `HWP_UPDATE_DOCS=1 cargo test -p hwp-cli --test cli_reference`로 재생성하라 — CI 테스트가 코드와 문서의 동기화를 강제한다.

## 명령 색인

- [`hwp info`](#hwp-info)
- [`hwp cat`](#hwp-cat)
- [`hwp grep`](#hwp-grep)
- [`hwp convert`](#hwp-convert)
- [`hwp render`](#hwp-render)
- [`hwp new`](#hwp-new)
- [`hwp compose`](#hwp-compose)
- [`hwp template`](#hwp-template)
- [`hwp diff`](#hwp-diff)
- [`hwp edit`](#hwp-edit)
- [`hwp fields`](#hwp-fields)
- [`hwp bookmarks`](#hwp-bookmarks)
- [`hwp slots`](#hwp-slots)
- [`hwp fill`](#hwp-fill)
- [`hwp validate`](#hwp-validate)
- [`hwp lint`](#hwp-lint)
- [`hwp certify`](#hwp-certify)
- [`hwp corpus`](#hwp-corpus)
- [`hwp mcp`](#hwp-mcp)
- [`hwp update`](#hwp-update)
- [`hwp skill`](#hwp-skill)
- [`hwp skill export`](#hwp-skill-export)
- [`hwp dump`](#hwp-dump)

## `hwp info`

파일 정보 표시: 포맷/버전/속성/스트림 목록

**사용법:** `hwp info [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--json` |  |  | JSON으로 출력 |

## `hwp cat`

텍스트 추출

**사용법:** `hwp cat [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--format` | `plain` \| `markdown` \| `json` \| `html` \| `csv` | `plain` | 출력 포맷 |
| `--preview` |  |  | 본문 파싱 없이 PrvText 미리보기만 출력 |
| `--with-header-footer` |  |  | 머리말/꼬리말 텍스트도 추출에 포함 (기본: 제외) |
| `--with-hidden` |  |  | 숨은 설명 텍스트도 추출에 포함 (기본: 제외) |
| `--with-segments` |  |  | (markdown 전용) markdown과 함께 각 출력 문자 범위의 원본 좌표(섹션/문단)를 한 줄 JSON 봉투로 출력 — {"markdown": ..., "segments": [...]} |

## `hwp grep`

문단 텍스트 검색 (grep 의미 — 일치 없으면 종료 코드 1)

**사용법:** `hwp grep [OPTIONS] <PATTERN> <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<PATTERN>` |  |  | 검색 패턴 (부분 문자열 일치) |
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--ignore-case` |  |  | 대소문자 무시 일치 |

## `hwp convert`

포맷 변환

**사용법:** `hwp convert [OPTIONS] <INPUTS>...`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUTS>` |  |  | 입력 HWP/HWPX 파일들 ("-"는 stdin; 여러 입력은 --out-dir 필요) (반복 가능) |
| `-o, --output` | `<OUTPUT>` |  | 출력 파일 경로 ("-"는 텍스트 포맷(md/json/html/txt/csv)에 한해 stdout; 단일 입력에서 필수) |
| `--out-dir` | `<OUT_DIR>` |  | 여러 입력의 출력 디렉터리 (파일명은 "<스템>.<확장자>", --to 필요) |
| `--to` | `hwp` \| `hwpx` \| `md` \| `json` \| `html` \| `pdf` \| `odt` \| `txt` \| `csv` \| `docx` |  | 출력 포맷 (생략 시 확장자에서 추론) |
| `--strict` |  |  | 변환 중 보존 불가능한(opaque) 데이터 발견 시 실패 처리 |
| `--loss-report` | `<LOSS_REPORT>` |  | typed 보존 ledger(hwp-preservation-report-v1)를 JSON으로 기록 — 무손실 성공 시에도 작성 (단일 입력 전용). 보존 검사는 hwp/hwpx 출력에서만 실행되므로 그 외 포맷(docx, md 등)에서는 항상 빈 ledger |
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
| `<INPUT>` |  |  | 입력 HWP/HWPX 파일 |
| `-o, --output` | `<OUTPUT>` |  | 출력 파일 경로 |
| `--pages` | `<PAGES>` | `all` | 페이지 범위: "1", "1-3", "all" |
| `--dpi` | `<DPI>` | `96` | 해상도 DPI (유한한 36..=600) |
| `--format` | `png` \| `svg` \| `pdf` |  | 출력 포맷 (생략 시 확장자에서 추론) |
| `--report` | `<REPORT>` |  | 기계 판독 렌더 보고서(JSON)를 원자적으로 기록 |
| `--font-dir` | `<FONT_DIR>` |  | 추가 폰트 디렉터리 (반복 가능) |

## `hwp new`

새 문서 생성

**사용법:** `hwp new [OPTIONS]`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `-o, --output` | `<OUTPUT>` |  | 출력 HWP/HWPX 경로 |
| `--from` | `<FROM>` |  | 입력 markdown/JSON 파일 (생략 시 빈 문서) |
| `--template` | `<TEMPLATE>` |  | 내장 문서 템플릿을 영문 슬러그 또는 한국어 별칭으로 사용 (--list-templates 참고). --from과는 함께 쓸 수 없음. 프레임 플래그와는 함께 쓸 수 있으며, 골격이 담지 않는 두문/결문 표를 플래그가 더한다 |
| `--list-templates` |  |  | 내장 문서 템플릿(슬러그·한국어 별칭)을 모두 나열하고 종료; -o 불필요 |
| `--set-meta` | `<SET_META>` |  | 메타데이터 설정 "키=값" (키: title\|author\|subject\|keywords, 반복 가능) |
| `--preset` | `<PRESET>` |  | 공문서 프로필 (markdown 입력 전용): official/report/plan/notice/minutes/press. 기존·한국어 별칭은 하나의 프로필로 정규화 |
| `--margin-top` | `<MARGIN_TOP>` |  | 위쪽 페이지 여백(mm, 0..=200) |
| `--margin-bottom` | `<MARGIN_BOTTOM>` |  | 아래쪽 페이지 여백(mm, 0..=200) |
| `--margin-left` | `<MARGIN_LEFT>` |  | 왼쪽 페이지 여백(mm, 0..=200) |
| `--margin-right` | `<MARGIN_RIGHT>` |  | 오른쪽 페이지 여백(mm, 0..=200) |
| `--strict` |  |  | markdown import가 내용을 드롭하면(HTML 블록 계약 위반) 실패 처리. 기본: 경고 후 진행 (종료 코드 0) |
| `--doc-head` | `<DOC_HEAD>` |  | 공문서 두문 블록 "키=값" (키: 기관명\|수신\|경유, 반복 가능) |
| `--doc-foot` | `<DOC_FOOT>` |  | 공문서 결문 블록 "키=값" (키: 발신명의\|기안자\|검토자\|결재자\|협조자\|시행번호\|시행일자\|접수번호\|접수일자\|주소\|홈페이지\|전화\|팩스\|이메일\|공개구분, 반복 가능) |
| `--notice-head` | `<NOTICE_HEAD>` |  | 공고문 머리 블록 "키=값" (키: 기관명\|공고번호, 반복 가능) |
| `--notice-foot` | `<NOTICE_FOOT>` |  | 공고문 꼬리 블록 "키=값" (키: 공고일자\|발신명의, 반복 가능) |
| `--press-head` | `<PRESS_HEAD>` |  | 보도자료 머리 블록 "키=값" (키: 기관명\|보도시점\|배포일\|담당부서\|담당자\|연락처, 반복 가능) |

## `hwp compose`

DocumentSpec v1/v2(JSON/YAML)에서 구조 문서를 deterministic 합성

**사용법:** `hwp compose [OPTIONS] --output <OUTPUT> <SPEC>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<SPEC>` |  |  | DocumentSpec v1/v2 입력 파일(.json, .yaml, .yml) |
| `-o, --output` | `<OUTPUT>` |  | 출력 HWP/HWPX |
| `--format` | `json` \| `yaml` |  | 입력 포맷 (생략 시 spec 확장자에서 추론) |
| `--dry-run` |  |  | 검증·컴파일 보고서만 생성하고 파일은 쓰지 않음 |
| `--report` |  |  | 실행 보고서를 JSON으로 출력 |
| `--allow-visual-fallback` |  |  | [deprecated] v1 호환 전용 — v2는 이 정책 덮어쓰기를 거부한다 |

## `hwp template`

TemplateSpec/Data v1에서 typed native HWP/HWPX 생성

**사용법:** `hwp template [OPTIONS] --data <DATA> --output <OUTPUT> <TEMPLATE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<TEMPLATE>` |  |  | TemplateSpec v1 입력 파일(.json, .yaml, .yml) |
| `--data` | `<DATA>` |  | TemplateData v1 입력 파일(.json, .yaml, .yml) |
| `-o, --output` | `<OUTPUT>` |  | 출력 HWP/HWPX |
| `--template-format` | `json` \| `yaml` |  | TemplateSpec 입력 포맷 (생략 시 확장자에서 추론) |
| `--data-format` | `json` \| `yaml` |  | TemplateData 입력 포맷 (생략 시 확장자에서 추론) |
| `--dry-run` |  |  | 실제 확장·writer·검증 경로를 실행하되 결과 파일은 게시하지 않음 |
| `--report` |  |  | preservation/expansion 보고서를 JSON으로 출력 |

## `hwp diff`

렌더 결과를 한글 기준 PNG와 비교해 오차 측정 (위치 오프셋·픽셀 차이율)

**사용법:** `hwp diff [OPTIONS] --ref <REF> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  | 입력 HWP/HWPX 파일 |
| `--ref` | `<REF>` |  | 한글에서 같은 페이지를 같은 DPI로 내보낸 기준 PNG |
| `--page` | `<PAGE>` | `1` | 비교할 페이지 (1-기반) |
| `--dpi` | `<DPI>` | `96` | 해상도 DPI (유한한 36..=600) |
| `-o, --out` | `<OUT>` |  | 차이 이미지 출력 경로 (생략 시 <ref>.diff.png) |
| `--font-dir` | `<FONT_DIR>` |  | 추가 폰트 디렉터리 (반복 가능) |
| `--tolerance` | `<TOLERANCE>` | `16` | 채널 차이 허용 오차 (이하면 동일 취급) |
| `--format` | `text` \| `json` | `text` | 리포트 출력 형식 (json = 기계 판독, parity 배치 러너용) |
| `--ours-png` | `<OURS_PNG>` |  | 문서 렌더 대신 이 래스터(우리 PDF의 pdftoppm 결과)를 --ref와 비교; 입력 경로는 리포트 기록용 |

## `hwp edit`

기존 문서 편집 (텍스트 치환·표 셀 설정) — 이미지·서식 보존

**사용법:** `hwp edit [OPTIONS] --output <OUTPUT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  | 입력 HWP/HWPX 파일 |
| `-o, --output` | `<OUTPUT>` |  | 출력 파일 경로 |
| `--replace` | `<REPLACE>` |  | 텍스트 치환 "찾기=>바꾸기" (반복 가능, 모든 일치 치환) |
| `--set-cell` | `<SET_CELL>` |  | 표 셀 설정 "표:행:열=값" (반복 가능, 0-기반 인덱스) |
| `--set-field` | `<SET_FIELD>` |  | 필드/누름틀 채우기 "이름=값" (반복 가능 — hwp fields로 이름 확인) |
| `--set-meta` | `<SET_META>` |  | 메타데이터 설정 "키=값" (키: title\|author\|subject\|keywords, 반복 가능) |
| `--create-field` | `<CREATE_FIELD>` |  | 누름틀 생성 "앵커=>이름" 또는 "앵커=>이름=값" — 앵커 텍스트 뒤에 %clk 필드 삽입 (반복 가능) |
| `--create-bookmark` | `<CREATE_BOOKMARK>` |  | 책갈피 생성 "앵커=>이름" — 앵커 텍스트 뒤에 bokm 지점 표식 삽입 (반복 가능) |
| `--create-hyperlink` | `<CREATE_HYPERLINK>` |  | 하이퍼링크 생성 "앵커=>URL" 또는 "앵커=>표시=>URL" — 앵커 뒤에 %hlk 삽입 (반복 가능) |
| `--insert-image` | `<INSERT_IMAGE>` |  | 이미지 삽입 "앵커=>경로" 또는 "앵커=>경로@너비x높이"(mm) — 앵커 뒤에 그림 삽입 (반복 가능) |
| `--seal` | `<SEAL>` |  | 도장 날인 "앵커=>경로" 또는 "앵커=>경로@크기mm" — 앵커 문구 위에 도장 부유 배치 (반복 가능) |
| `--set-format` | `<SET_FORMAT>` |  | 글자 서식 "찾기:속성=값,..." (예: "제목:bold=on,size=16,color=#FF0000") (반복 가능) |
| `--set-align` | `<SET_ALIGN>` |  | 문단 정렬 "찾기=정렬" (left/right/center/justify/distribute) (반복 가능) |
| `--insert-para` | `<INSERT_PARA>` |  | 문단 삽입 "앵커=>텍스트" — 앵커가 있는 문단 뒤에 새 문단 (반복 가능) |
| `--insert-para-before` | `<INSERT_PARA_BEFORE>` |  | 문단 삽입(앞) "앵커=>텍스트" — 앵커가 있는 문단 앞에 새 문단 (반복 가능) |
| `--delete-para` | `<DELETE_PARA>` |  | 문단 삭제 "텍스트" — 텍스트가 있는 문단 삭제 (반복 가능) |
| `--add-row` | `<ADD_ROW>` |  | 표 행 추가 "표[:위치[:개수[:템플릿행]]]" — 위치 생략·end면 끝, 숫자면 그 행 앞에 삽입 (반복 가능, 0-기반; 병합 표도 지원) |
| `--add-col` | `<ADD_COL>` |  | 표 열 추가 "표[:위치[:개수]]" — 위치 생략·end면 끝, 숫자면 그 열 앞에 삽입. 전체 폭 유지(기존 열 균등 축소). 병합 표도 지원 (반복 가능, 0-기반) |
| `--delete-row` | `<DELETE_ROW>` |  | 표 행 삭제 "표:행" — N번째 표의 R행 (반복 가능, 0-기반; 병합 행은 거부) |
| `--delete-col` | `<DELETE_COL>` |  | 표 열 삭제 "표:열" — N번째 표의 열 삭제. 전체 폭 유지(남은 열에 재분배). 병합 셀은 축소 (반복 가능, 0-기반) |
| `--merge-cells` | `<MERGE_CELLS>` |  | 셀 병합 "표:r1:c1:r2:c2" — 사각 영역을 좌상단 앵커로 병합 (반복 가능, 0-기반) |
| `--split-cell` | `<SPLIT_CELL>` |  | 셀 분할 "표:행:열" — 병합 셀을 1×1로 분해 (반복 가능, 0-기반) |
| `--add-table` | `<ADD_TABLE>` |  | 표 삽입 "앵커=>행JSON" — 앵커 문단 뒤에 균일 표 삽입. 행JSON은 문자열 배열의 배열 (반복 가능) |
| `--clone-table` | `<CLONE_TABLE>` |  | 표 복제 "원본표=>앵커[=>blank\|keep]" — N번째 표(0-기반, 재귀 순서)를 깊은 복제해 앵커 문단 뒤에 삽입. blank(기본)는 구조·서식만 남기고 셀 내용을 비우고, keep은 지원 콘텐츠(중첩 표·그림)까지 복제(id 재부여) (반복 가능) |
| `--set-para` | `<SET_PARA>` |  | 문단 모양 "찾기=>키:값" — 키: line-spacing(% 또는 Npt), indent, left, right, top, bottom (mm) (반복 가능) |
| `--set-page` | `<SET_PAGE>` |  | 페이지 설정 "키:값" — 키: width, height, margin-left, margin-right, margin-top, margin-bottom (mm), orientation (portrait\|landscape) (반복 가능) |
| `--delete-image` | `<DELETE_IMAGE>` |  | 그림 삭제 "앵커" — 앵커 문단의 그림 삭제 (반복 가능) |
| `--delete-table` | `<DELETE_TABLE>` |  | 표 삭제 "n"(0-기반 인덱스) 또는 "앵커"(앵커 문단의 표) (반복 가능) |
| `--delete-field` | `<DELETE_FIELD>` |  | 필드 삭제 "이름" (반복 가능; 이름은 hwp fields로 확인) |
| `--delete-bookmark` | `<DELETE_BOOKMARK>` |  | 책갈피 삭제 "이름" (반복 가능; 이름은 hwp bookmarks로 확인) |
| `--style-tables` | `<STYLE_TABLES>` |  | 공문서 프리셋으로 모든 적용 대상 표 스타일링(헤더 셰이딩·굵게·가운데 정렬, 내용비례 폭) — official\|report\|plan\|notice\|minutes\|press. 1열 표(테두리 블록)는 건너뜀. 두 번 적용해도 바이트 동일 |
| `--verify` |  |  | 쓰기 후 재읽기로 검증 |
| `--allow-partial` |  |  | 일부 요청이 대상을 찾지 못해도 일치한 편집만 게시 (기본: 하나라도 미적용이면 실패) |

## `hwp fields`

필드/누름틀 목록 표시 (이름·종류·값)

**사용법:** `hwp fields [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--json` |  |  | JSON으로 출력 |

## `hwp bookmarks`

책갈피 목록 표시 (이름)

**사용법:** `hwp bookmarks [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--json` |  |  | JSON으로 출력 |

## `hwp slots`

`{{name}}` 텍스트 자리표시자(템플릿 슬롯) 목록 표시

**사용법:** `hwp slots [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--json` |  |  | JSON으로 출력 |

## `hwp fill`

충실도 보존 템플릿 채우기 (hwpx의 `{{name}}` 치환, 패키지 보존)

**사용법:** `hwp fill [OPTIONS] --output <OUTPUT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  | 입력 HWPX 템플릿 |
| `-o, --output` | `<OUTPUT>` |  | 출력 파일 경로 |
| `--set` | `<SET>` |  | 자리표시자 채우기 "이름=값" (반복 가능; `{{이름}}` 치환). "이름=@부분.md"이면 `{{이름}}` 앵커 문단을 부분 파일(md+HTML 표 블록, 계약 docs/design/18)로 교체 — 대규모 문서의 부분별 조합. "@@"는 리터럴 '@' |
| `--data` | `<DATA>` |  | 이름→값 JSON 객체 파일 (일괄 채우기; "parts": {"이름": "경로"} 부분 파일 교체, "tables": [...] 표 행 채우기) |
| `--json` |  |  | 치환 요약을 JSON으로 출력 ({output, replaced, counts}) |
| `--allow-partial` |  |  | 일부 요청이 자리를 찾지 못해도 일치한 값만 게시 (기본: 하나라도 미치환이면 실패) |

## `hwp validate`

구조 검증 (mimetype/필수 엔트리/XML 파싱) — 유효하면 종료코드 0

**사용법:** `hwp validate [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--json` |  |  | JSON으로 출력 |

## `hwp lint`

공문서 표기법·구조 규칙 검사 — 기본은 권고(advisory)이며 항상 종료코드 0. --strict는 오류 심각도 지적이 있을 때만 종료코드 1

**사용법:** `hwp lint [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 검사 대상 .md/.hwp/.hwpx 파일 ("-"는 stdin을 markdown으로 읽음) |
| `--profile` | `gongmun` \| `report` | `gongmun` | 린트 프로필: gongmun(기본) 또는 report — v1에서는 같은 규칙 표 사용 |
| `--json` |  |  | hwp-lint-report-v1 JSON 리포트로 출력 |
| `--strict` |  |  | 오류 심각도 지적이 있으면 종료 코드 1 (기본: 항상 종료 코드 0) |

## `hwp certify`

versioned policy로 package/semantic/native render/independent import 인증

**사용법:** `hwp certify --policy <POLICY> --report <REPORT> <INPUT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<INPUT>` |  |  | 인증할 HWP/HWPX 입력 |
| `--policy` | `<POLICY>` |  | hwp-certification-policy-v1 JSON/YAML |
| `--report` | `<REPORT>` |  | 새로 만들 원자적 artifact 디렉터리(기존 경로 거부) |

## `hwp corpus`

버전 고정 구조 문서 코퍼스를 2회 생성·재개방·native 인증

**사용법:** `hwp corpus --manifest <MANIFEST> --report <REPORT>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `--manifest` | `<MANIFEST>` |  | hwp-structured-corpus-v1 manifest JSON |
| `--report` | `<REPORT>` |  | 새로 만들 원자적 실행 보고서 디렉터리(기존 경로 거부) |

## `hwp mcp`

MCP(Model Context Protocol) stdio 서버 — AI 에이전트용 도구 인터페이스

**사용법:** `hwp mcp [OPTIONS]`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `--font-dir` | `<FONT_DIR>` |  | 렌더/diff 도구의 기본 폰트 디렉터리 (반복 가능) |
| `--root` | `<ROOT>` |  | 모든 파일 접근을 이 디렉터리 아래로 제한 (반복 가능). 기본: 제한 없음 |

## `hwp update`

자체 업데이트 — GitHub 릴리스에서 최신 `hwp`를 받아 실행 중인 바이너리를 교체

**사용법:** `hwp update [OPTIONS]`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `--check` |  |  | 교체 없이 현재/최신 버전만 확인 |
| `--tag` | `<TAG>` |  | 특정 릴리스로 고정 (예: "v0.2.0" — 이전 버전으로 되돌릴 때) |
| `--force` |  |  | 같은 버전이어도 다시 받아 교체 (손상된 설치 복구용) |
| `--json` |  |  | JSON으로 출력 |

## `hwp skill`

번들된 에이전트 스킬(AI 코딩 어시스턴트용 SKILL.md) 관리

**사용법:** `hwp skill <COMMAND>`

_인자·플래그 없음_

## `hwp skill export`

임베드된 스킬 트리(SKILL.md, SKILL.ko.md, 공문서 안내, references/, templates/)를 디렉터리에 기록 (기본 ./hwp)

**사용법:** `hwp skill export [OPTIONS]`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `-o, --output` | `<OUTPUT>` |  | 스킬 트리 출력 디렉터리 (--install과 동시 사용 불가) |
| `--install` | `claude-code` \| `codex` \| `amazon-quick` |  | 대신 알려진 에이전트 스킬 디렉터리에 설치 |
| `--quick-profile` | `<ID_OR_ABSOLUTE_PATH>` |  | Amazon Quick 프로필 ID 또는 절대 프로필 디렉터리 (Amazon Quick 설치 전용) |

## `hwp dump`

[개발자용] 레코드/패키지 구조 덤프

**사용법:** `hwp dump [OPTIONS] <FILE>`

| 인자/플래그 | 값 | 기본값 | 설명 |
|---|---|---|---|
| `<FILE>` |  |  | 대상 HWP/HWPX 파일 |
| `--stream` | `<STREAM>` |  | 대상 스트림/엔트리 (예: "DocInfo", "BodyText/Section0", "Contents/header.xml") |
| `--raw` |  |  | 레코드 페이로드를 hex로 출력 |
| `--json` |  |  | JSON으로 출력 |
