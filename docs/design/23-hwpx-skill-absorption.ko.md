[한국어](23-hwpx-skill-absorption.ko.md) · [English](23-hwpx-skill-absorption.md)

# hwpx 스킬 흡수: 서브커맨드 패리티 매트릭스와 여백 기록

> **상태:** 살아있는 체크리스트. [issue #121](https://github.com/STAIxBWLB/hwp-cli/issues/121)
> (2026-08-20 수정: 두 개가 아닌 하나의 통합 스킬 — D-01)과
> [skills#35](https://github.com/STAIxBWLB/skills/issues/35)(구 스킬 폐기)에서 추적한다.
> Phase 2.2부터 2.5까지는 각 행이 해소될 때마다 이 매트릭스를 **제자리에서** 갱신한다. 이 문서는
> Phase 2.5의 "아무것도 잃지 않았다" 증명의 시드 목록이다. 어떤 상태 셀도 해당 행에 명시된
> 증거 없이 "verified"로 올릴 수 없다.

## 1. 범위

사용자 범위 `hwpx` 스킬(바이너리를 외부에서 구동하던 Python 패키지)은 단일 번들 스킬
`skills/hwp`로 흡수된다. 스킬의 산문, 규정 레퍼런스, 템플릿이 바이너리에 임베드되어
`hwp skill export`로 디렉터리 트리 형태로 풀린다. 이 문서는 조용히 잃거나 조용히 주장해서는
안 되는 두 가지를 기록한다.

1. **패리티** — 기존 `./hwpx` 서브커맨드 전부와 스크립트 수준 엔진 보장 전부를 네이티브
   커맨드, 명명된 갭, 또는 명시적 폐기로 매핑(§2).
2. **여백 기록** — 검증된 공문서 프로필 기본값과 그 규정상 경계(§3).

Phase 지도: **2.1** 스캐폴드 + 이 매트릭스. **2.2** 법정 8단계 번호 매기기, 프리셋 패밀리,
검증된 여백 교정(GONG-01/02). **2.3** `hwp lint`(GONG-03의 표기법 절반). **2.4**
문서 프레임, `--template`, 표 스타일링(GN-4, GN-5, GN-6). **2.5** 편집 패리티 증명 + 구
스킬 폐기(EDIT-01, RET-01).

## 2. 패리티 매트릭스

기존 `./hwpx` 서브커맨드마다 한 행씩 — `scripts/hwpx_cli.py`(1385–1591행)에서
`add_parser` 호출 **28**개를 2026-08-20에 직접 다시 센 결과다. CONTEXT의 이전 "27"은 두
별칭을 하나 빼고 센 것이다 — 여기에 CLI 표면 바깥에서 기존 스크립트가 강제하던 스크립트
수준 보장마다 한 행씩을 더한다. `render-pdf`와 `write-java`는 별칭이며 그렇게 표시한다.

상태 범례: **verified** = 이번 phase에 네이티브 소스와 대조해 다시 측정. **inferred** = 구
스킬 소스를 읽고 매핑했으며 명명된 phase에서 증명 예정. **resolved by absorption** = 스킬이
이제 바이너리 안에 들어가면서 해당 우려 자체가 사라짐.

### 2.1 기존 서브커맨드 (28)

| 기존 서브커맨드 | 네이티브 대응 | Phase | 상태 |
|---|---|---|---|
| read | `hwp cat` (구조는 `--format json`) | 2.1 | verified |
| summary | 직접 대응 없음 (`hwp info` + `hwp cat --format json`) | 2.5 | gap → EDIT-01 레시피 (inferred) |
| to-md | `hwp convert --to md --media-dir` | 2.1 | verified |
| unpack | 없음 | 2.5 | gap → raw-zip 레시피 / EDIT-01 (inferred) |
| repack | 없음 (네이티브 writer가 패키지 레이아웃을 보장) | 2.5 | gap → raw-zip 레시피 / EDIT-01 (inferred) |
| fill | `hwp fill` | 2.1 | verified — 기본 경로 한정. run-spanning 채우기는 §2.2의 갭 행이며 여기서 커버하지 않음 |
| slots | `hwp slots` | 2.1 | verified |
| edit | `hwp edit --replace` (run-spanning 아님) | 2.5 | partial → EDIT-01 증명 항목 (inferred) |
| add-rows | `hwp edit --add-row` | 2.1 | verified |
| add-col | `hwp edit --add-col` | 2.1 | verified |
| fill-table | `hwp fill --data tables.json` | 2.1 | verified (데이터 구동 행 채우기 존재) |
| create | `hwp new --from` | 2.1 | verified |
| styled | `hwp new --preset official|report|plan|notice|minutes|gaejosik|press` | 2.2 | 프로필·번호·레이아웃은 verified, style pass는 계속 없음 |
| beautify | 없음 | 2.4 | gap → `--style-tables` (GONG-03, inferred) |
| validate | `hwp validate` | 2.1 | verified |
| analyze | 없음 | 2.5 | gap → EDIT-01 문서화 레시피 (inferred) |
| guard | 없음 (`hwp render --report`로 페이지 수 확인 가능) | 2.5 | gap → EDIT-01 문서화 레시피 (inferred) |
| edit-section | 직접 대응 없음 | 2.5 | gap → EDIT-01 문서화 레시피 (inferred) |
| fill-form | 직접 대응 없음 | 2.5 | gap → `--set-cell-by-label` (EDIT-01, inferred) |
| to-pdf | `hwp convert --to pdf` / `hwp render` | 2.1 | verified — 기존 soffice 폴백은 **의도적으로 폐기** (네이티브 엔진만) |
| render-pdf | to-pdf와 동일 | 2.1 | verified (`to-pdf --engine hwp`의 **별칭**) |
| to-html | `hwp cat --format html` | 2.1 | verified |
| info | `hwp info` | 2.1 | verified |
| fields | `hwp fields` | 2.1 | verified |
| bookmarks | `hwp bookmarks` | 2.1 | verified |
| render | `hwp render` | 2.1 | verified |
| convert | `hwp convert` | 2.1 | verified |
| write-java | `hwp new --from` | 2.1 | verified (**별칭**, 레거시 이름) |

### 2.2 스크립트 수준 보장 (7)

| 기존 스크립트 보장 | 네이티브 대응 | Phase | 상태 |
|---|---|---|---|
| run-spanning `{{slot}}` 채우기 | 현재 없음 — 네이티브 `hwp fill`은 raw-XML 문자열 치환(`crates/hwpx/src/patch.rs:52-56`)이라 `<hp:t>` run에 걸쳐 나뉜 슬롯은 매칭되지 않음 | 2.5 | gap → EDIT-01 (inferred) — **verified 패리티 아님**. `hwp new --from`으로 만드는 템플릿은 각 슬롯을 하나의 run 안에 유지해야 함 |
| 편집 시 `linesegarray` 지우기 | 엔진 내재: 네이티브 IR 라운드트립은 줄 세그먼트를 다시 쓰고, 바이트 보존 패치 경로는 텍스트를 건드리지 않음 | 2.5 | 엔진 내재, 2.5에서 확인 (inferred) |
| sec 인덱스 섹션 편집 | 없음 | 2.5 | gap → EDIT-01 문서화 레시피 (inferred) |
| mimetype-first STORED repack | 네이티브 writer는 패키지 레이아웃을 준수. raw-zip 경로에는 네이티브 대응 없음 | 2.5 | writer 경로는 해소. raw-zip 레시피 갭 → EDIT-01 (inferred) |
| style_pass 표 규칙 | 없음 | 2.4 | gap → GONG-03 (inferred) — **verified 패리티 아님** |
| page_guard 구조 드리프트 검사 | 없음 | 2.5 | gap → EDIT-01 문서화 레시피 (inferred) — **verified 패리티 아님** |
| 바이너리 탐색 (`$HWP_CLI`, 최고 버전 선택) | 폐기 — 스킬이 자신이 구동하는 바이너리 안에 들어감 | 2.1 | resolved by absorption |

## 3. 여백 기록 (D-14)

**질문:** 공문서 프로필 여백 조합에 규정상 근거가 있는가?

**현재 엔진 동작 (verified):** 모든 표준 프로필은 **위 20 / 아래 10 / 왼쪽 20 / 오른쪽
20 mm**로 시작한다. `official`은 머리말/꼬리말 여백과 쪽 번호가 없고, `report`, `plan`,
`gaejosik`은 머리말/꼬리말 15 mm와 `- N -`, `notice`, `press`는 10 mm와 `- N -`를 쓴다.
`minutes`는 둘 다 없다. 호출자는 네 개의 `hwp new` `--margin-*` 플래그 또는 대응 MCP
`margin_*_mm` 입력으로 한 변만 override할 수 있다.

**근거 (2차 출처):** kordoc의 법규 편집본은 폐기된 위 30 mm 값을 반증한다 —
`gongmunseo-reference.md` §3.2: *"'위 30mm' 같은 수치는 어느 권위 출처에도 없음"*
(refuted로 표기)이며, 2020 행정업무운영 편람의 공식 조합은 **위 20 / 아래 10 / 좌 20 /
우 20 mm**(머리말·꼬리말·제본 0)로 기록한다. 구 `hwpx` 스킬의 레퍼런스는 30/15/20/15를
출처 없이 주장했고, 프리셋 값의 기원이 바로 그것이다.

**판정 (2026-08-22 해소):** 위 30 mm는 근거가 없었고, Phase 2.2는 모든 표준 프로필을
20/10/20/20으로 바꾼 뒤 정품 한컴 HWP/HWPX 14건 관찰에서 프로필 레이아웃을 검증했다.
GN-9는 이 검증 경계에서 해소된다. 이 해소는 프로필 여백과 번호/레이아웃 동작만 닫는다.
`hwp lint`, 문서 프레임, `--template`, 표 스타일링, 편집 패리티는 각 배정 phase에 계속 이연된다.

## 4. 기록된 결정

- **Q1 — 템플릿에는 `.ko.md` 미러가 없다** (D-11): `skills/hwp/templates/*.md` 여덟 파일은
  설계상 본문이 한국어다. EN/KO 미러는 빈 중복이 될 뿐이다. 드리프트·패리티 게이트는
  `templates/`를 미러 요건과 패리티 워크에서 제외한다(02.1-01).
- **Q2 — `claude-web/`은 임베드 테이블과 export에서 제외** (02.1-01):
  `skills/hwp/claude-web/bootstrap.sh`는 스킬 콘텐츠가 아니라 저장소/릴리스 산출물이다.
  드리프트 워크에서도 제외한다.
- **Q3 — 릴리스 웹 번들은 이번 phase에도 SKILL.md만 유지**:
  `hwp-skill-claude-web.zip`은 계속 `SKILL.md`, `bootstrap.sh`, Linux x86_64 `bin/hwp`만
  담는다. 보류 아이디어(기록, 일정 없음): claude.ai 샌드박스 스킬 UI를 다시 확인한 뒤
  트리 형태 웹 번들.

## 5. 출처

- **kordoc** (`~/workspace/references/ai-tools/kordoc`, MIT) — 여기서 사용한 규정 편집본의
  **2차 출처**로 명시한다. 특히 `docs/gongmunseo-reference.md` §3.2(여백 반증). 규칙은
  법규에서 다시 서술하며 복사하지 않는다.
- **jkf87/hwpx-skill** — 구 `hwpx` 스킬의 lint/gonmun 규칙 일부의 규칙 계보를 인정한다. 이
  저장소에는 **라이선스가 없다**(2026-08-20 GitHub API로 확인: `license: null`, LICENSE
  파일 없음). 따라서 이 저장소로 **규칙 텍스트를 복사하지 않으며**, 이 한 줄의 계보 인정이
  재사용의 전부다.
- **구 `hwpx` 스킬** (`STAIxBWLB/skills`, 읽기 전용) — §2.1의 서브커맨드 목록은
  `scripts/hwpx_cli.py`에서 다시 센 것이고, 엔진 매핑은 각 `cmd_*` 본문을 읽어 확인했다
  (2026-08-20 리서치).
- **1차 규정 출처** (조문 번호로 인용, 본문은 옮기지 않음): 행정기관의 업무효율화를 위한 규칙,
  2020 행정업무운영 편람. D-12에 따라 개별 규칙은 법령 조문 + 편람 쪽수 + 신뢰도 태그를
  `skills/hwp/references/korean-official-format.md`에 달아 둔다.
