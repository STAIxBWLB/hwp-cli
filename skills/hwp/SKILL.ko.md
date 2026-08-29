---
name: hwp
description: hwp CLI 또는 MCP stdio 서버로 한컴 HWP 5.0 / HWPX 문서를 읽고, 만들고, 편집하고, 변환하고, 렌더링하고, 검증합니다. .hwp 또는 .hwpx 파일을 다루는 모든 작업 — 텍스트 추출, 검색, 템플릿 채우기, 내용 편집, docx/pdf/html/md/json/odt/txt/csv 변환, 페이지 이미지 렌더링 — 에 사용하세요. 한국 공문서(공문) — 기안문, 보고서, 계획서, 회의록, 공고문, 보도자료 — 작업에도 사용하세요. 마크다운 계약, 템플릿, 표기법은 official-documents 하위 안내서가 다룹니다.
---

[한국어](SKILL.ko.md) · [English](SKILL.md)

# hwp — HWP/HWPX 문서 도구 모음

`hwp`는 한컴 HWP 문서를 위한 단일 바이너리 도구입니다. 바이너리 HWP 5.0 형식과 XML 기반
HWPX 형식을 읽고 쓰며, docx, pdf, html, markdown, json, odt, txt, csv로 납출합니다.
한컴 오피스 설치가 필요 없습니다.

영어판이 원본이며, `SKILL.ko.md`(이 파일)가 완전한 한국어 미러입니다. 두 파일은 항상 함께
납출됩니다.

설치: `brew install staixbwlb/hwp/hwp`, 또는
`curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh`.
`hwp skill export [-o DIR] [--install claude-code|codex|amazon-quick]`으로 이 스킬을
바이너리에서 꺼낼 수 있습니다. Amazon Quick Desktop 프로필은
`--quick-profile ID_OR_ABSOLUTE_PATH`로 선택합니다.

## 명령어 빠른 참조

전체 플래그 참조: `hwp {command} --help` (생성 문서: docs/manual/cli-reference.md).
출력 형식은 `--to`/`--format`이 없으면 출력 확장자로 추론합니다.
아래 사용법에서 `{file}`, `{output}` 같은 단일 중괄호 토큰은 실제 값으로 바꿔야 할
자리 표시자입니다. 이중 중괄호 `{{name}}`만 리터럴 HWPX 템플릿 슬롯 문법입니다
(`fill`, `slots`, 템플릿 도구에서 사용).

- `hwp info {file} [--json]` — 형식, 버전, 속성, 스트림 목록.
- `hwp cat {file} [--format plain|markdown|json|html|csv]` — 텍스트 추출. 유용한 플래그:
  `--preview` (PrvText만, 본문 파싱 없음), `--with-header-footer`, `--with-hidden`
  (숨은 주석), `--with-segments` (마크다운 + 소스 좌표를 한 줄 JSON 봉투로).
  `--format json`은 전체 IR(표, 이미지, 서식)을 납출합니다. 지원 암호 보호 HWP5/HWPX는
  `--password-stdin` 사용을 우선하고, 프로세스 인수 노출을 감수할 때만 `--password {value}`를
  사용하세요.
- `hwp grep {pattern} {file} [--ignore-case]` — 본문, 표 셀, 텍스트 상자를 대상으로 한
  문단 부분 문자열 검색. grep 관례: 일치 항목이 없으면 종료 코드 1.
- `hwp convert {inputs...} -o {output}` — 형식 변환 (`--to hwp|hwpx|md|json|html|pdf|
  odt|txt|csv|docx`). `-`는 stdin 읽기 / stdout 쓰기 (stdout은 텍스트 형식만). 입력이
  여러 개면 `--out-dir {dir}` 필요. `--strict`는 보존 불가능한(불투명) 데이터가 있으면
  발행하지 않고 실패합니다. PDF 출력은 렌더 경로에 위임되며 CJK 폰트가 필요합니다:
  `--font-dir {dir}` (반복 가능; 기본값 `HWP_FONT_DIR` 또는 `fonts/`). 마크다운 납출
  플래그: `--media-dir`, `--with-header-footer`, `--with-hidden`, `--embed-bin` (json).
  보호 입력은 `cat`과 같은 `--password` / `--password-stdin` 쌍을 받습니다.
- `hwp merge {inputs...} -o {output}` — 문서 두 개 이상을 인자 순서대로 하나로 합칩니다.
  입력 하나가 Section 하나가 되고, writer는 출력 확장자(`.hwp`/`.hwpx`)로 정해지며,
  표준 입력 `-`는 받지 않습니다. `--strict`는 보존 불가(opaque) 데이터가 있으면 발행하지
  않고 실패하며, `--loss-report {file.json}`은 손실이 없어도
  `hwp-preservation-report-v1` 원장을 기록합니다. 쪽·각주·개요 번호는 각 입력의
  시작·계속 설정을 그대로 유지하므로 병합한 뒤 다시 확인하세요. `--password` /
  `--password-stdin`은 전체 입력에 한 번만 적용됩니다.
- `hwp split {input} --out-dir {dir}` — 문서 하나를 조각으로 나눕니다. 기본은 Section
  하나당 조각 하나이며 이름은 `{stem}-NNN.{소문자 확장자}`입니다. `--pages "N"|"N-M"`
  (반복 가능)을 주면 쪽 범위로 나누는데, 그 경계는 한컴이 저장한 레이아웃 캐시에서 얻은
  추정값이라 한컴 자체 페이지 나눔과 다를 수 있습니다. `--strict`와 `--loss-report`는
  `merge`와 같습니다.
- `hwp new -o {out.hwpx|out.hwp}` — 문서 생성. `--from {file.md|file.json}`은 마크다운
  또는 JSON IR을 가져옵니다 (생략 시 빈 문서); `--set-meta key=value` (title/author/
  subject/keywords, 반복 가능); `--preset official|report|plan|notice|minutes|press`
  (한국 공문서 프로필, 마크다운 입력 전용); `gian`과 문서화된 호환 별칭은 표준 프로필로
  정규화합니다 — `gian` 별칭은 계속 동작하지만 한 번의 지원 중단 안내를 출력합니다. 변별 여백은 mm 단위 `--margin-top`, `--margin-bottom`, `--margin-left`,
  `--margin-right`로 지정합니다. `--strict`는 마크다운 가져오기가 내용을 유실하면 실패합니다.
- `hwp edit {input} -o {output} [flags...]` — 기존 문서 편집; 이미지, 서식, 미파싱
  레코드는 보존합니다. 문자열 플래그 (모두 반복 가능): `--replace "find=>repl"`,
  `--set-cell "t:r:c=value"` (0-기반), `--set-field "name=value"`, `--set-meta "k=v"`,
  `--create-field "anchor=>name[=value]"`, `--create-bookmark "anchor=>name"`,
  `--create-hyperlink "anchor=>[text=>]URL"`, `--insert-image "anchor=>path[@WxH mm]"`,
  `--seal "anchor=>path[@size mm]"`, `--set-format "find:prop=value,..."`,
  `--set-align "find=left|right|center|justify|distribute"`. 구조 플래그 (모두 반복
  가능): `--insert-para "anchor=>text"`, `--insert-para-before`, `--delete-para "text"`,
  `--add-row "t[:at[:count[:template_row]]]"` / `--add-col "t[:at[:count]]"` (`at` 생략
  또는 `end`는 끝에 추가; 숫자는 해당 행/열 앞에 삽입; 병합 셀 표 지원) /
  `--delete-row "t:r"` / `--delete-col "t:c"` /
  `--merge-cells "t:r1:c1:r2:c2"` / `--split-cell "t:r:c"` / `--add-table "anchor=>[[row],...]"` /
  `--clone-table "src=>anchor[=>blank|keep]"` (표 `src`를 앵커 뒤에 깊은 복사 —
  blank는 구조/스타일만 유지하고 셀을 비우고, keep은 중첩 표/이미지까지 재매핑된 ID로
  복제) /
  `--delete-table "n|anchor"` / `--delete-image "anchor"` / `--delete-field "name"` /
  `--delete-bookmark "name"`, 문단 모양 `--set-para "find=>key:value"`
  (line-spacing, indent, left/right/top/bottom mm)와 페이지 설정 `--set-page "key:value"`
  (width/height/margin-*/orientation). `--verify`는 출력을 다시 읽습니다;
  `--allow-partial`은 전부 아니면 전무 규칙을 완화합니다 (안전 규칙 참조).
- `hwp fill {template.hwpx} -o {output}` — 충실도 보존 `{{name}}` 템플릿 채우기
  (패키지 보존). `--set "name=value"` (반복 가능); `--set "name=@part.md"`는 부분 파일
  (마크다운 + HTML 표 블록)을 앵커 문단에 이어 붙입니다 — 큰 문서를 위한 부분 기반
  조립 (`@@`는 리터럴 `@` 이스케이프); `--data {file.json}` 일괄 채우기
  (`"parts": {...}` 이어 붙이기, `"tables": [...]` 행 채우기); `--json`은 요약 출력;
  `--allow-partial`은 일치한 부분만 발행. 먼저 `hwp slots`로 슬롯을 확인하세요.
- `hwp compose {spec.json|yaml} -o {output}` — DocumentSpec v1/v2로부터의 결정적 조립.
  `--dry-run`은 쓰기 없이 검증만; `--report`는 실행 보고서를 JSON으로 출력.
- `hwp template {template} --data {data} -o {output}` — TemplateSpec/Data v1로부터의
  타입화된 네이티브 HWP/HWPX 생성. `--dry-run`은 발행 없이 확장 + 라이터 + 검증을
  실행; `--report`는 보존/확장 보고서를 JSON으로 출력.
- `hwp render {input} -o {output.png|svg|pdf}` — 페이지 렌더링. `--pages "1"|"1-3"|"all"`,
  `--dpi 36..=600` (기본 96), `--format png|svg|pdf`, `--font-dir {dir}` (반복 가능).
  PNG/SVG는 페이지당 파일 하나; PDF는 단일 다중 페이지 파일. CJK 폰트 필요.
  보호 입력은 `--password` 또는 `--password-stdin`을 받습니다.
- `hwp fields {file} [--json]` / `hwp bookmarks {file} [--json]` / `hwp slots {file} [--json]`
  — 필드 (이름/종류/값), 책갈피 (bokm), `{{name}}` 템플릿 슬롯 나열.
- `hwp validate {file} [--json]` — 구조 검증 (mimetype, 필수 엔트리, XML 파싱);
  유효하면 종료 코드 0.
- `hwp lint {file.md|file.hwp|file.hwpx} [--json] [--strict] [--profile gongmun|report]` —
  공문서 표기·구조 규칙 열 가지를 검사합니다 (`-`는 표준 입력을 markdown으로 읽습니다).
  기본은 조언용이라 항상 종료 코드 0이며, `--strict`가 error 등급 지적을 찾았을 때만
  1로 끝납니다. `--json`은 `hwp-lint-report-v1` 계약
  (`rule_id`/`severity`/`line`/`col`/`message`)을 출력합니다.
- `hwp certify {input} --policy {policy.json|yaml} --report {dir}` — 버전 관리된 정책에
  따라 패키지, 의미, 네이티브 렌더, 독립 임포트를 인증; 보고서 디렉터리를 원자적으로
  발행.
- `hwp diff {input} --ref {hancom.png} [--page N] [--dpi N] [--tolerance N] [-o diff.png]` —
  렌더를 한컴 참조 PNG와 비교 (오프셋, 픽셀 차이).
- `hwp compare {a} {b} [--format text|json]` — 문서 두 개의 문단·구조 차이를 보고하며 두
  입력 모두 수정하지 않습니다. 렌더를 한컴 참조 PNG와 비교하는 `hwp diff`와는 다른
  명령입니다. `--format json`은 `hwp-compare-report-v1` 계약을 출력합니다. 종료 코드는
  diff(1) 관례를 따르므로 아래 안전 규칙의 종료 코드 항목을 확인하세요.

## Official documents (공문서)

`hwp`는 한국 공문서(공문) 작성 표면을 네이티브로 제공합니다. 표준 프로필은 문서 유형마다
하나씩 `official`(기안문), `report`(보고서), `plan`(계획서), `notice`(공고문),
`minutes`(회의록), `press`(보도자료) 여섯 가지입니다. 표준 공문서 프로필은 `gian`,
`gongmun`을 의미상 호환 별칭으로 받습니다. 별칭은 같은 프로필을 선택할 뿐 raw-byte
동일성을 약속하지 않으며, `gian` 별칭은 계속 동작하지만 한 번의 지원 중단 안내를
출력합니다. 한국어 별칭도 사용할 수 있습니다. 각 유형을 마크다운 골격으로
작성하고, `hwp new --from ... --preset official`(또는 맞는 프로필)로 생성한 뒤 `hwp fill`로
채웁니다.

개조식은 프로필이 아니라 문체입니다. 보고서·계획서와 내부결재 문서의 본문에서 사용하는
명사형 종결 방식이므로, 프로필은 문서 유형으로 고르고 문체는 본문에서 적용합니다.
`references/korean-official-format.ko.md` §6 어투를 참고하십시오.

모든 프로필의 기본 여백은 명시적인 변별 override 전 위/아래/왼쪽/오른쪽 20/10/20/20 mm입니다.
본문 기본값은 official 맑은 고딕 12pt/160%, report·plan HCR 바탕 15pt/160%, notice 맑은
고딕 15pt/160%, minutes HCR 바탕 14pt/130%, press HCR 바탕 14pt/160%입니다. report·plan의
머리말/꼬리말 여백은 15 mm, notice·press는 10 mm, official·minutes는 0 mm입니다.
report·plan·notice·press에는 `- N -` 쪽 번호를 넣고 official·minutes에는 넣지 않습니다.

항목 기호는 중첩 리스트 깊이에서 나옵니다. 순서 리스트는 깊이에 따라
`1.` → `가.` → `1)` → `가)` → `(1)` → `(가)` → `①` → `㉮`로 렌더링됩니다. 8단계까지
지원하며 9단계 이상은 파일을 발행하지 않고 오류로 거절합니다. HWPX는 해당 번호 정의를
직접 방출합니다. HWP5는 `하` 이후의 확인된 이어쓰기까지 포함하는 검증된 safe/direct
인코딩 경로를 사용합니다.

□ ○ 사다리는 리터럴입니다: `□ `와 `○ `를 문단 선두 기호로 입력하면 엔진이 들여쓰기를
적용합니다 — ASCII 유사 문자로 대체하지 마세요. `- ` 리스트 불릿은 깊이 1에서 `-`,
그 아래에서 `·`로 렌더링됩니다.

제목 번호는 리터럴입니다: 제목 텍스트에 `Ⅰ. 1.` (Ⅰ = U+2160 전각; ASCII `I.` 금지)을
직접 입력합니다 — 제목에는 자동 번호가 없습니다.

항목이 하나뿐이면 일반 문단입니다: 리스트에 항목이 정확히 하나뿐일 때는 기호 없는
일반 문단으로 작성하세요.

번호 경로에서 기호를 손으로 입력하지 마세요: 평범한 중첩 리스트를 작성하고 엔진이 기호를
부여하게 두세요. 순서 리스트 안에 손으로 입력한 `가.`는 번호가 이중으로 매겨집니다.

문서 유형별 레시피, 슬롯 표, 한국어 별칭 표, 채우기/검증 워크플로우, 한컴 최종 확인은
`official-documents.md` (이 파일 옆에 납출됨)에, 규정 배경은
`references/korean-official-format.md`에 있습니다.

## Editing recipes (편집 레시피)

retired `hwpx` 워크플로우의 정직한 한계를 포함한 이중 언어 네이티브 편집 마이그레이션 대조표는
[references/editing-recipes.ko.md](references/editing-recipes.ko.md)에 있습니다. 분석, anchor 기반
문단 편집, 데이터 기반 표 채우기, label-value 양식, validate-plus-render guard 순서에 사용하십시오.

아래 세 워크플로우는 기록하기 전에 `fixtures/samples/report-tables.hwpx`를 대상으로
그대로 실행해 검증했습니다 (hwp 0.12.1, 2026-08-29).

### 문서 분석 (analyze)

"이 문서에 무엇이 들어 있는가"를 한 번에 보는 패스: 패키지 인벤토리, 원본 좌표가 붙은 텍스트,
그다음 프로그램적 핸들(필드와 템플릿 슬롯) 순서입니다:

```bash
hwp info doc.hwpx --json
hwp cat doc.hwpx --format markdown --with-segments > doc.segments.json
hwp fields doc.hwpx --json
hwp slots doc.hwpx --json
```

`--with-segments`는 한 줄 JSON 봉투 `{"markdown": ..., "segments": [...]}`를 출력하며 각
세그먼트는 `{"kind": "para", "section": N, "para": N, "start": N, "end": N}`이고 start/end는
markdown 문자열 기준 문자 오프셋입니다. 문서에 필드나 `{{slot}}` 자리표시자가 없으면
`fields`와 `slots`는 정당하게 `[]`를 반환합니다 — 비어 있는 것도 답이며 오류가 아닙니다.

### 섹션 단위 편집 (edit-section)

세그먼트 좌표가 문단을 찾아 주고, 실제 편집은 그 범위의 보이는 텍스트를 anchor로
수행합니다 — raw `sec` 인덱스 계약은 없습니다. 검증을 통과할 때까지는 복사본에서
작업하십시오:

```bash
hwp cat doc.hwpx --format markdown --with-segments > doc.segments.json
hwp edit doc.hwpx -o edited.hwpx --insert-para "anchor text=>new paragraph" --verify
hwp edit edited.hwpx -o reverted.hwpx --delete-para "new paragraph" --verify
hwp validate edited.hwpx
```

대상 텍스트가 들어 있는 markdown 범위의 세그먼트에서 `(section, para)` 좌표를 읽고, 그
범위의 평문(markdown 기호 제외)을 anchor로 사용하십시오. `--insert-para "anchor=>text"`는
anchor 문단 뒤에 삽입하고(`--insert-para-before`는 앞에), `--delete-para "text"`는 텍스트로
문단을 삭제합니다. 둘 다 기본적으로 all-or-nothing이며 `--verify`에서 출력을 다시 읽습니다.

### 편집 감시 (guard)

편집 전후 구조 드리프트 검사: 구조 검증과 렌더러 페이지 수 비교입니다.

```bash
hwp validate edited.hwpx
hwp render doc.hwpx -o before.png --report before.render.json
hwp render edited.hwpx -o after.png --report after.render.json
```

두 렌더 보고서의 `total_pages`를 비교하십시오 (보고서에는 `font_coverage`와
`font_resolution_complete`도 들어 있습니다. 렌더링에는 CJK 폰트가 필요하고 페이지 수는
사용 가능한 폰트에 따라 달라집니다). 페이지 수 변화는 검토 신호이지 내용 드리프트의
증거가 아닙니다: IR 왕복만으로도 레이아웃이 리플로우될 수 있습니다 — 픽스처에서
검증한 결과, 편집 없는 `hwp convert --to hwpx` 왕복이 6페이지에서 5페이지로 렌더링된
반면 바이트 보존 `--replace` 고속 경로는 원래 페이지 수를 유지했습니다. 레이아웃을
그대로 유지해야 한다면 `--replace` 전용 편집(패키지 보존 경로)을 우선하고, 페이지 수가
바뀌면 검증을 약화시키지 말고 렌더링된 페이지를 직접 살펴보십시오.

## MCP 서버

`hwp mcp`는 stdio 위에서 동기 JSON-RPC 2.0을 사용합니다 (줄 구분; stdout이 프로토콜을
전달하고 로그는 stderr로). 프로토콜 버전은 `initialize`에서 협상됩니다: 클라이언트의
`protocolVersion`이 `2025-06-18`, `2025-03-26`, `2024-11-05` 중 하나면 그대로 에코하고,
그 외에는 지원하는 최신 버전을 돌려줍니다.

항상 최소 하나의 샌드박스 루트를 지정해 시작하세요:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--root", "/path/to/workspace", "--font-dir", "/path/to/fonts"]
    }
  }
}
```

`--root {dir}` (반복 가능)은 도구가 접근하는 모든 파일 경로 — 입력, 출력, 중첩
이미지/부분 파일 경로, spec `base_dir`, 호출별 `font_dir`, certify 보고서 디렉터리 — 를
지정된 디렉터리로 제한합니다. 루트는 시작 시 정규화되며, 없거나 읽을 수 없는 루트는
즉시 실패합니다. `--root` 없이 실행하면 제한이 해제되고 시작 시 stderr에 한 줄 경고를
출력합니다 — 클라이언트가 허용하면 항상 `--root`를 사용하세요.

Windows의 Amazon Quick Desktop은 로컬 MCP 자식을 Low 강제 무결성(`S-1-16-4096`)으로
시작합니다. Windows `LocalLow` 디렉터리 아래의 전용 하위 폴터와 `C:\Windows\Fonts`를
사용하고, JSON 인수를 분리해 지정하세요:
`["mcp", "--font-dir", "C:\\Windows\\Fonts", "--root",
"C:\\Users\\YOUR_NAME\\AppData\\LocalLow\\hwp-quick-workspace"]`. Quick을 설정하거나
도구를 호출하기 전에 `YOUR_NAME`을 실제 계정 폴터로 바꾸세요. 인수 목록과 MCP 경로는
`%USERPROFILE%`를 확장하지 않습니다. Low 강제 레이블을 상속받도록 루트는
`AppData\LocalLow` 아래에 만드세요. 일반 `C:\TEMP`나 `%LOCALAPPDATA%\Temp`를 쓰기
루트로 쓰지 마세요. 디스커버리는 되더라도 `hwp_new`가 전용 스테이징 디렉터리를 만들
때 `Access is denied (os error 5)`로 실패할 수 있습니다.

Quick의 로컬 폴터 권한에 추가된 폴터는 내장 읽기/검색 도구에서는 쓸 수 있지만 Low
무결성 MCP 자식이 쓸 수 있게 되지는 않습니다. 모든 MCP 입력과 출력은 설정된
`LocalLow` 루트 아래에 두고, Quick의 파일 도구나 탐색기로 산출물을 넣고 빼세요.
반복된 시작 실패로 Quick이 커넥터를 자동 비활성화했다면 다시 활성화하세요.

Quick 설정을 도와줄 때는 JSON 임포트를 선호하고, 인수 안에 셸 인용 문자를 넣지 마세요.
세 계층을 순서대로 검증하세요: 정확한 절대 경로 바이너리가 버전을 반환하는지; Quick이
20개 도구를 보고하고 새로고침 후에도 활성 상태인지; 마지막으로 `hwp_new`와 이어진
`hwp_validate`가 설정된 LocalLow 루트 아래 절대 경로에서 성공하는지 (예:
`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\quick-hwp-smoke.hwpx`).
디스커버리만으로 파일 접근이 증명된다고 주장하지 마세요.
커넥터 수정 후에는 이전에 생성된 도구 접두사를 재사용하지 말고 새로고침하거나 새 채팅을
여세요.
복사-붙여넣기 운영자 및 AI 런북:
`https://github.com/STAIxBWLB/hwp-cli/blob/main/docs/manual/amazon-quick-desktop.md`.

도구 (20):

| 도구 | 필수 인수 | 용도 |
|---|---|---|
| `hwp_info` | `path` | 형식, 버전, 속성, 스트림 진단 |
| `hwp_read` | `path` | 텍스트 추출 (`format`: plain/markdown/json/html/csv; 선택적 호출별 `password`; `with_header_footer`, `with_hidden`, `with_segments`); UTF-8 바이트 페이지네이션 |
| `hwp_grep` | `path`, `pattern` | 문단 부분 문자열 검색; `{matches, count, truncated}`; 0걻도 정상 결과 |
| `hwp_list_fields` | `path` | 필드 나열 |
| `hwp_list_bookmarks` | `path` | 책갈피 (bokm) 나열 |
| `hwp_slots` | `path` | `{{name}}` 자리 표시자 나열 |
| `hwp_render` | `path` | 페이지 렌더링 (선택적 호출별 `password`; `format`: png/svg/pdf, `pages` 범위); 단일 페이지 PNG는 base64 반환, 더 큰 결과는 `output_path`로 파일 기록 |
| `hwp_edit` | `input`, `output` | 타입화된 JSON 연산을 통한 엄격한 원자적 편집 (모든 `hwp edit` 플래그 미러, `add_table`, `clone_table`, `set_para`, `set_page`, `delete_*` 포함) |
| `hwp_convert` | `input`, `output` | 형식 변환 (선택적 호출별 `password`; MCP에서는 `strict` 기본값 true) |
| `hwp_new` | `output` | 마크다운 또는 JSON IR과 메타데이터, 공문서 프로필, `margin_top`/`margin_bottom`/`margin_left`/`margin_right` override로 문서 생성 |
| `hwp_compose` | `output`, `spec`/`spec_path` | CLI와 동일한 경로로 DocumentSpec v1/v2 조립 |
| `hwp_template` | `output`, `template` (+`data`) | TemplateSpec/Data v1의 한정된 확장 |
| `hwp_fill` | `input`, `output`, `values` | hwpx 템플릿의 `{{name}}` 채우기 (패키지 보존) |
| `hwp_validate` | `path` | 구조 검증, `{valid, errors, warnings}` |
| `hwp_lint` | `path` | 마크다운 파일의 공문서 표기법·구조 규칙 검사 (권고성; 경로만, `--root` 샌드박스 적용); hwp-lint-report-v1 findings JSON (`rule_id`/`severity`/`line`/`col`/`message`) 반환 |
| `hwp_certify` | `input`, `policy`, `report` | 인증 실행 후 보고서를 원자적으로 발행 |
| `hwp_diff` | `input`, `ref` | 한 페이지를 렌더링해 참조 PNG와 비교 |
| `hwp_merge` | `inputs`, `output` | 문서 두 개 이상을 합침 (입력 하나당 Section 하나); 보존 손실 원장을 반환 |
| `hwp_split` | `input`, `out_dir` | Section 단위(또는 `pages`)로 조각내고 발행된 조각 경로를 반환 |
| `hwp_compare` | `a`, `b` | 읽기 전용 문단·구조 비교로 `hwp-compare-report-v1`을 반환. 차이가 있는 것은 정상 결과라 `isError`가 되지 않으므로 `identical`을 읽는다 |

## 컨테이너용 HTTP 모드

`hwp serve`는 `hwp mcp`와 같은 프로토콜을 stdio 대신 HTTP로 제공하므로, 같은 20종 도구를
컨테이너에서 사용할 수 있습니다. 인터넷에 직접 노출하는 용도가 아니라, TLS 종단·인증·본문
크기 제한을 이미 수행한 신뢰된 edge 뒤에 두는 것을 전제로 합니다.

```bash
hwp serve --addr 0.0.0.0:8080 --root /work --font-dir /usr/share/fonts/truetype/nanum
```

| 경로 | 동작 |
|---|---|
| `POST /mcp` | JSON-RPC 메시지 하나, 1 MiB 상한. request는 `200 application/json`, notification은 본문 없이 `202` |
| `GET /mcp` | `405` — server가 push하지 않으므로 event stream을 제공하지 않습니다 |
| `GET /healthz` | listener가 bind되면 `200`. container platform이 확인하는 지점입니다 |
| `POST\|GET /files/{name}` | `--files`로만 활성화. 이름은 `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`를 만족해야 하며, 파일당 64 MiB·workspace당 256 MiB |

`--root`는 여기서 필수입니다. 생략하면 경고만 하는 `hwp mcp`와 달리, 원격 배포는 파일 접근이
제한되지 않은 상태로 실행되어서는 안 되기 때문입니다. 들어오는 `Mcp-Session-Id`는 받아들이되
무시합니다. session affinity는 앞단 platform의 책임입니다.

`hwp serve`는 SIGTERM과 SIGINT를 받으면 처리 중인 요청을 끝낸 뒤 종료합니다. 커널은 기본 처리
방식이 그대로인 시그널을 PID 1에 전달하지 않지만, 이 서버는 핸들러를 등록하므로 PID 1로 실행해도
유휴 컨테이너를 그 방식으로 정지시키는 플랫폼이 정상적으로 정지시킬 수 있습니다. 두 번째 시그널은
즉시 종료로 처리합니다. 이 동작이 없는 이전 릴리스를 고정해 쓰는 경우에는 `tini` 같은 init 아래에서,
또는 `docker run --init`으로 실행하세요.

## 안전 규칙 (반드시 준수)

1. **쓰기 후에는 항상 검증하세요.** `new` / `edit` / `fill` / `compose` / `template` /
   `convert`로 hwp/hwpx를 만든 후 (또는 MCP 해당 도구 후) `hwp validate {output}`
   (또는 `hwp_validate`)을 실행해 종료 코드 0을 확인한 뒤 파일을 넘기세요.
2. **변경은 실패-폐쇄적입니다.** 작성 명령 (`new`, `edit`, `fill`)은 `DROP:` 경고 —
   출력에 보존할 수 없는 내용 — 를 하드 실패로 처리하고 발행하지 않습니다. 실패한
   명령은 기존 출력 파일을 그대로 둡니다.
3. **원자적 발행.** 출력은 대상 옆의 전용 스테이징 작업 공간에 기록되고, 검증된 뒤
   교체됩니다. 부분적이거나 중간에 끊긴 출력 상태는 없습니다. 실패 시 이전 파일이
   보존 (롤백 시 복원) 됩니다.
4. **기본은 전부 아니면 전무.** `edit`와 `fill`은 요청된 변경 중 하나라도 대상을 찾지
   못하면 (적용되지 않은 편집 / 치환되지 않은 자리 표시자) 명령 전체를 실패시킵니다.
   `--allow-partial` (또는 MCP `allow_partial` 인수)은 일치한 부분만 발행합니다 —
   의도적으로만 사용하고, 결과를 다시 확인하세요.
5. **MCP에는 `--root`를 선호하세요.** 제한 없이 실행하는 대신, 도구가 정당하게 접근할
   수 있는 모든 디렉터리에 대해 플래그를 반복해 서버 범위를 작업 디렉터리로 한정하세요.
6. **암호는 호출 안에서만 유지하세요.** CLI에서는 `--password`보다 `--password-stdin`을
   우선하고, MCP 암호는 그 인수를 받는 여섯 도구 — `hwp_read`·`hwp_convert`·`hwp_render`·
   `hwp_merge`·`hwp_split`·`hwp_compare` — 중 실제로 필요한 호출에만 넣으세요.
   암호를 리포트·영수증·생성 파일·명령 기록·지속 환경변수에 남기지 마세요.
7. **종료 코드는 명령마다의 관례대로 읽으세요.** 관례가 의도적으로 다르기 때문에
   "0이 아니면 실패"라는 단일 해석은 틀립니다.

   | 명령 | 관례 |
   |---|---|
   | `compare` | diff(1) 관례: 0은 동일, 1은 차이 발견, 2는 실행 자체가 실패 |
   | `lint` | 항상 0; `--strict`일 때만 error 등급 지적에서 1 |
   | `grep` | 일치가 없으면 1 (오류가 아니라 정상 결과) |
   | `validate`, `new --strict`, `convert --strict`, `merge --strict`, `split --strict` | 성공은 0, 실패는 0이 아님 |

   MCP에는 종료 코드가 없습니다. `hwp_compare`는 `identical`을, `hwp_grep`은 `count`를
   돌려주며 두 경우 모두 `isError`는 false입니다.
8. **문서 단위 쓰기 뒤에는 보존 손실 원장을 확인하세요.** `convert`·`merge`·`split`은
   보존하지 못한 항목을 `hwp-preservation-report-v1` 원장에 기록합니다. CLI에서는
   `--loss-report {file.json}`로 기록되고, MCP에서는 모든 `hwp_merge`·`hwp_split` 응답의
   `preservation` 필드로 돌아옵니다. 병합은 첫 입력 이후의 패키지 passthrough를 항상
   버리기 때문에 두 명령 모두 `--strict`가 기본값이 아닌 선택 사항입니다. 손실이 없다고
   가정하지 말고 원장을 확인하세요.
