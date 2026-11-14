[한국어](editing-recipes.ko.md) · [English](editing-recipes.md)

# 네이티브 편집 레시피와 이전 `hwpx` 마이그레이션 대조표

이 문서는 하나의 번들 `hwp` 스킬에 포함된 정본 편집 가이드다. 이전 `hwpx` 안내를 대체하며
래퍼·보조 스크립트·실행 레시피·바이너리 템플릿을 추가하지 않는다. 선택한 레시피를 릴리스와
기능 기록이 뒷받침하는 바이너리로 명령을 실행한다. 아래 원본 양식 참조 실행은 설치된 릴리스가
아닌 출시 전 소스 빌드에서 수행했다. 워크플로는 v1.5.0부터 포함되며 이전 릴리스는 해당 명령을 지원하지 않는다.

## 범위와 릴리스 경계

이전 마이그레이션 대조표의 지원 최소 `hwp` 버전은 **v0.12.1**입니다. 이 릴리스에는 수정된 Phase 2.5
label-form 및 skill-export
계약이 포함됩니다. 로컬 빌드, 버전 없는 `PATH` 바이너리, 병합 전 브랜치는 retirement 또는 replacement
근거가 아닙니다.

`templates/` 아래의 마크다운 8개는 번들 템플릿 SSOT로 유지합니다. 이전 바이너리 템플릿 6개는
읽기 전용 retirement 호환성 코퍼스이며, 복사, 보관, 편집하지 않습니다.

## 원본 양식 워크플로

### 네이티브 원본과 증거 범위 선택

제공된 HWP/HWPX 양식을 완성할 때는 그 네이티브 파일에서 시작한다. Markdown은 재작성할
때만 사용한다. JSON 문단 배열을 잘라내거나 패키지를 풀어 XML을 고치는 방식으로 양식을
추출하지 않는다. 타입이 있는 `split`, 원본에 결속된 바인딩, 네이티브 편집을 사용한다.

아래 전체 워크플로는 `document.original_form`을 포함해 v1.5.0 이상이 필요하다.
[버전별 기능 기록](original-form-capabilities-v1.json)은 기록된 커밋의 **출시 전 소스 빌드**(패키지
버전 1.4.0으로 표시)에서 참조 실행을 수행했고, 출시된 v1.4.0에는 워크플로가 없음을 확인했다.
사용 전에 `ahwp --version`과 해당 바이너리에서 내보낸 기록을 확인한다.

```sh
ahwp --version
ahwp skill export -o ./hwp-guide
```

격리된 작업 디렉터리를 사용한다. 원문이 든 discovery·검토된 계획·정확한 preview·로컬 주소
맵은 비공개로 유지한다. 영수증은 검토된 입력을 대신하지 않는다. 단계별 원본·출력 식별자를
유지한다. 이전 출력의 ID나 해시는 이후 출력의 선택자가 아니다.

### 범위가 한정된 P0 예제 준비

실제 제공된 양식에는 fixture 작성을 생략하고 요청된 글꼴을 유지한다. 공개 참조 실행에는
저장소의 소유자 작성 #416 generator를 사용한다.

```sh
python3 tools/generate_original_form_fixture.py ./target/original-form-workflow
```

참조 변형은 네이티브 원본을 compose하기 전에 생성된 작성 명세의
`document.styles.body.font_family`만 `NanumGothic`으로 바꾼다. 이 작성 입력을
`./target/original-form-workflow/p0-nanum.spec.json`으로 저장하고 `expected.json`, `inside.png`, `outside.png`를
유지한다. 사용권이 있는 `NanumGothic.ttf`와 `NanumGothicBold.ttf`를 `./target/original-form-workflow/fonts/`에 둔다.
정확한 해시는 기능 기록의 인증 정책에 있다. 글꼴은 번들에 포함되지 않는다. 이 fixture
준비는 다른 사람이 제공한 네이티브 양식의 글꼴을 바꾸라는 지시가 아니다.

```sh
cd ./target/original-form-workflow
ahwp compose "$PWD/p0-nanum.spec.json" -o ./source.hwpx
```

기능 파일의 `reference_inputs`에는 완전한 검토 JSON 값이 있다. JSON 편집기나 클라이언트의
파일 작성 기능으로 각 값을 해당 키 이름의 파일에 복사한다. 이것은 데이터 입력이며 별도의
편집 스크립트가 아니다. 기록된 P0 원본과 구현에 결속되어 있으므로 사용 전에 새 해시와
대조한다. 다른 원본에는 네이티브 IR 덤프를 수정하지 말고 새 계획을 작성·검토한다.
완전한 바인딩·인증 계획을 자동으로 도출하는 출시 명령은 없다.

### 네이티브 범위 발견과 추출

```sh
ahwp info ./source.hwpx --json
ahwp slots ./source.hwpx --forms --occurrences --json > ./source-catalog.json
ahwp cat ./source.hwpx --format json --with-segments --segments v2 > ./source-map.json
```

라벨·기존 값·`bindable`·문단/표 ID·본문 좌표를 검토한다. 반복 라벨은 고유 주소가 아니다.
P0 원본에는 이미 프로필 표가 **네 개** 있다. 네 개로 반복하는 과정을 보이려면 신청서 +
개발보고서 + 프로필 1만 선택한다. 이 generator에서 신청서 제목은 본문 문단 4이고 프로필 1
표는 문단 11이다. 실제 맵과 generator manifest에서 좌표를 확인한다. 일반 문서의 상수가 아니다.

`SOURCE_SHA256`을 `source-catalog.json`의 실제 `source_sha256`으로 설정한 뒤 실행한다.

```sh
ahwp split ./source.hwpx --out-dir ./extracted \
  --from heading:FORM_START_APPLICATION --to at:0.11 \
  --source-sha256 "$SOURCE_SHA256" --strict \
  --range-report ./extraction.json --loss-report ./extraction-preservation.json
ahwp validate ./extracted/source-001.hwpx
```

실제 범위 리포트와 원본→출력 맵을 유지한다. 기록된 실행은 `BinData/image2.png`(내부 이미지)를
유지하고 `BinData/image1.png`(앞선 외부 이미지)를 의도적으로 제외했으며, dropped-by-format·
kept-unverified·읽을 수 없는 참조 위치가 없었다. 제외된 원본 자산은 선택한 자산의 손실과
다르다. 동일 형식 HWP 입력도 같은 네이티브 선택 계약을 따른다. 새 주소를 선택하고 자체
리포트를 유지한다.

### 원본에 결속된 네 프로필 계획 검토

추출한 프로필 표 뒤에 고유한 삽입 제목을 준비한 뒤 별도 편집으로 기존 `heading` 이름 있는
스타일을 적용한다. 이 공통 CLI/MCP 순서는 MCP가 제공하지 않는 CLI 인라인
`insert_para.style`/`char` 필드에 의존하지 않는다.

```sh
ahwp cat ./extracted/source-001.hwpx --format json --with-segments --segments v2 > ./extracted-map.json
ahwp edit ./extracted/source-001.hwpx -o ./inserted.hwpx --ops "$PWD/prepare-headings.json" --verify
ahwp styles ./inserted.hwpx --format json
ahwp edit ./inserted.hwpx -o ./prepared.hwpx --ops "$PWD/prepare-style.json" --verify
ahwp slots ./prepared.hwpx --forms --occurrences --json > ./prepared-catalog.json
ahwp cat ./prepared.hwpx --format json --with-segments --segments v2 > ./prepared-map.json
```

P0 맵에서 선택한 표는 원본 문단 11에서 추출 문단 8로 이동한다.
`prepare-headings.json`은 문단 8 뒤에 `FORM_START_PROFILE_2`를 삽입한다.
`prepare-style.json`은 새 문단 9에 `clear_direct: true`로 `heading`을 적용한다.
요청 사이에 새 주소를 얻는다. 변경 대상은 새 제목이며 기존 양식 라벨이나 셀 기하가 아니다.

모든 개별 위치를 `prepared-catalog.json`과 대조한다. 예제 계획에는 바인딩 키 20개, 선택한
복제 대상 41개, 숫자 제약 6개가 있다. 각 대상은 `copies`, `max_chars`, `allow_reflow`를
명시한다. `source_sha256`과 `structural_sha256`은 전체 최초 문서나 첫 추출이 아닌
**준비된** 원본에서 가져온다. 완전한 배열은 기능 파일의
`reference_inputs["binding.ops.json"]`에 있다.

| 키 | 검토한 대상과 값 |
|---|---|
| `course.name`, `course.year` | 신청서·개발보고서·프로필 네 개 모두: `Reviewed Methods`, `2031` |
| `total.hours` | 신청서 시간·개발보고서 총합·프로필별 시간: `90` |
| `profile.name.1`부터 `.4` | 복제 인덱스 0부터 3: `Profile Alpha`, `Profile Beta`, `Profile Gamma`, `Profile Delta` |
| `profile.objective` | 복제본 네 개의 첫 목표 문단: `Reviewed objective.` |
| `profile.continuation` | 복제본 네 개의 두 번째 목표 문단: 선행 공백 세 개와 원문 그대로의 `Nested item: original wording remains unchanged.`; 이후 들여쓰기가 그 공백만 제거 |
| `module.a.lecture`, `.practice`, `.subtotal` | 개발보고서 행 3: `20`, `10`, `30` |
| `module.b.lecture`, `.practice`, `.subtotal` | 개발보고서 행 4: `22`, `8`, `30` |
| `module.c.lecture`, `.practice`, `.subtotal` | 개발보고서 행 5: `18`, `12`, `30` |
| `lecture.total`, `practice.total` | 개발보고서 행 6: `60`, `30` |

`sum` 제약 6개가 모듈별 합계·강의 합계·실습 합계·총합을 확인한다. 합계는 제공하고 검토한
값이며 암묵적 계산이나 표현식 언어가 아니다. 불일치는 발행 전에 거부한다.

반복 항목은 선택한 프로필 표와 고유한 새 제목 문단을 식별하고 `copies: 4`와 서로 다른
`profile_ids` 네 개를 지정한다. 네 개는 원본을 포함하므로 복제본 세 개를 추가한다. 원본 표는
맨 앞에 남고 복제본은 삽입 제목 뒤에 이어지므로 인증 반복 허용을 위한 표 인덱스 2부터 5가
연속된다. 복제본을 바인딩할 때는 복제 인덱스를 명시한다. 라벨 추측으로 사람이나 프로필을
선택하지 않는다.

### Dry-run 검토와 적용

`- draft => reviewed: course` 같은 텍스트 값은 구분자 기반 플래그에 끼워 넣지 않고 `values`의
리터럴 JSON 문자열로 둔다. 바인딩 키는 속성 경로가 아니라 정확한 문자열이다. 숫자 제약의 값에는
여전히 부호 없는 정수 계약을 적용한다.

`binding.ops.json`을 `dry.ops.json`으로 복사하고 `receipt`만 `dry-receipt.json`으로,
`preview`만 `dry-preview.json`으로 바꾼다. 계획과 값은 그대로 둔다. 자산을 포함한 입력과
원자적 리포트 목적지는 상위 디렉터리가 존재하는 절대 경로나 `./` 경로를 사용한다.

```sh
ahwp edit ./prepared.hwpx -o ./dry.hwpx --ops "$PWD/dry.ops.json" --dry-run
```

정확한 로컬 preview·선택한 claim·숫자 값·원본→복제 맵을 검토한다. Dry-run도 비공개 후보를
작성·검증하지만 영수증에 `published: false`를 기록하고 네이티브 출력은 발행하지 않는다.
같은 검토 원본 `prepared.hwpx` 바이트에 적용한다.

```sh
ahwp edit ./prepared.hwpx -o ./bound.hwpx --ops "$PWD/binding.ops.json" --verify
ahwp validate ./bound.hwpx
```

`apply_form_bindings`는 배치의 유일한 연산이다. `--allow-partial`·스타일 연산·`--report`와
함께 사용하지 않고 자체 영수증을 유지한다. `preview`는 명시적이며 정확한 내용을 포함한다.
기본 영수증은 라벨이나 값 대신 해시·횟수·개별 위치 ID를 담는다. 참조 적용은 복제 프로필 3개와
의미 보존 허용 범위 `selected_text_and_declared_clones_only`를 보고한다. 페이지에 맞거나
네이티브 앱에서 열렸다는 뜻은 아니다.

배분값을 다시 수정하려면 같은 준비 원본에 검토 값을 재적용하고 관련 숫자 제약·합계를 모두
갱신한다. 이전 바인딩 출력을 오래된 원본 결속 계획에 넣거나 기존 복제본을 다시 복제하지
않는다. 원본이 달라지면 discovery와 검토를 새로 한다.

### 제목 추가와 쪽 나눔·들여쓰기 적용

새 출력 주소로 두 번째·세 번째 복제본의 호스트 문단 앞에 `FORM_START_PROFILE_3`과 `_4`를
추가한다. 기록된 `bound.hwpx`의 요청 스냅샷에서 본문 문단 11과 12이며 두 번 삽입한 뒤 새
제목은 11과 13이다. 두 번째 요청에서 새 문단에 `heading`을 적용한다. 기존 양식 라벨과
서명 영역은 그대로 유지한다.

```sh
ahwp cat ./bound.hwpx --format json --with-segments --segments v2 > ./bound-map.json
ahwp edit ./bound.hwpx -o ./headings-inserted.hwpx --ops "$PWD/remaining-headings.json" --verify
ahwp edit ./headings-inserted.hwpx -o ./headed.hwpx --ops "$PWD/remaining-style.json" --verify
ahwp slots ./headed.hwpx --forms --occurrences --json > ./headed-catalog.json
ahwp edit ./headed.hwpx -o ./paged.hwpx --ops "$PWD/pagination.ops.json" --verify --report ./pagination-report.json
ahwp slots ./paged.hwpx --forms --occurrences --json > ./paged-catalog.json
ahwp edit ./paged.hwpx -o ./final.hwpx --ops "$PWD/indent-repeat.ops.json" --verify --report ./indent-repeat-report.json
ahwp compare ./paged.hwpx ./final.hwpx --format json > ./indent-compare.json
```

`pagination.ops.json`은 새 프로필 제목(`headed.hwpx`의 문단 9·11·13)에 break-before를
설정하고 연속 문단 네 개의 새 개별 위치 문단 ID에 다음 모양을 적용한다.

```json
{"op":"set_indent_level","address":{"id":"CURRENT_PARAGRAPH_ID"},"level":2,"step":"5mm","hanging":"2mm"}
```

선택자를 현재 목록의 `paragraph_id`로 바꾼다. 완전한 참조 ID는 기능 파일에 있다. `level`은
0부터 7이고 `step`/`hanging`은 단위 문자열이다. 선행 공백 문자만 제거하고 문단 여백·들여쓰기를
사용하므로 내부 단어나 표 격자는 바꾸지 않는다. `set_break_before`는 본문 전용이며 셀 문단과
구역 첫 문단은 거부한다. `keep_together`는 HWP5 플래그 인코딩에 실제 저장 파일의 증거가 없어
현재 거부한다. 글꼴을 몰래 줄이거나 지원하지 않는 프로필 묶음 유지를 주장하지 않는다.

반복 들여쓰기 배열은 `paged-catalog.json`에서 **새로 얻은** 문단 ID를 사용한다. 기록된 두 번째
적용은 네 연산 각각 변경 0개를 보고하고 비교 결과가 동일하며 네이티브 바이트도 일치한다.
텍스트 길이가 바뀐 뒤 오래된 run 주소를 재사용하지 않는다.

### 정확한 전달 산출물 렌더와 인증

```sh
ahwp validate ./final.hwpx
ahwp convert ./final.hwpx -o ./final.hwp --strict --loss-report ./conversion-preservation.json
ahwp validate ./final.hwp
ahwp slots ./final.hwpx --forms --occurrences --json > ./final-hwpx-catalog.json
ahwp slots ./final.hwp --forms --occurrences --json > ./final-hwp-catalog.json
ahwp render ./final.hwpx -o ./hwpx.pdf --font-dir ./fonts --report ./hwpx-render.json --layout-json ./hwpx-layout.json
ahwp render ./final.hwp -o ./hwp.pdf --font-dir ./fonts --report ./hwp-render.json --layout-json ./hwp-layout.json
```

이 참조 실행은 HWPX를 바인딩·편집한 뒤 전달 산출물을 HWP로 변환한다. 동일 형식 HWP 바인딩은
별도로 지원한다. 형식을 바꾸면 다른 형식의 바인딩 계획을 재사용하지 않고 discovery를 새로
수행하고 자체 식별자를 확보한다. 변환 유효성과 빈 보존 원장은 한컴 렌더링 충실도의 증거가 아니다.

`reference_inputs`에서 `expect-hwpx.json`과 `certify-hwpx.json`을 복사한다. 검토된 기대값 계획은
준비 원본 해시·정확한 출력 해시·기대 셀 134개·페이지 1부터 6의 고유한 양식 시작 앵커 6개·표
6개 전체의 행열 수·선택한 세 표 원본에 대한 허용
`repeated_tables: [{"id":"profiles.repeat","table":2,"extra_copies":3}]`를 포함한다.
모두 이 fixture에만 해당하며 다른 환경에도 적용되는 페이지 golden이 아니다.

HWP에는 해당 기대값을 `expect-hwp.json`으로 복사하고 `output_sha256`을 새 discovery에서 얻은
실제 HWP 해시로 설정한다(참조 값은 `reference_input_variants.hwp_certificate`에 있다).
정책은 `certify-hwp.json`으로 복사하고 `document.original_form.plan`을 그에 맞게 바꾼다.
두 정책 모두 정확한 regular·bold 글꼴 파일을 고정하고 대체를 금지하며 모든 페이지를 확인한다.
페이지 바깥 경계·잠재적 충돌 실패 게이트도 활성화 상태로 유지한다.

```sh
ahwp certify ./final.hwpx --policy "$PWD/certify-hwpx.json" --report "$PWD/certificate-hwpx"
ahwp certify ./final.hwp --policy "$PWD/certify-hwp.json" --report "$PWD/certificate-hwp"
```

실행마다 새 리포트 디렉터리를 사용한다. 발행은 원자적이며 기존 리포트 디렉터리는 거부한다.
`overall`, `checks.original_form`, 정책·계획·입력 해시, 글꼴 식별자, 렌더 결과, manifest를 읽는다.
기대값 계획의 원본 해시는 선언·반영되는 값이며 제공 원본을 별도로 불러와 비교하는 검사가
아니다. 실제 연결 관계를 입증하도록 추출·제목 편집·바인딩·최종 discovery 기록을 유지한다.

인증은 기대 셀 값과 표 행열 수를 확인하며 완전한 원본→출력 너비·테두리·스타일·병합 투영을
검사하지 않는다. 바인딩의 의미 보존 허용 범위와 네이티브 재읽기 검증은 별도 증거다.
`render-report.complete`는 진단 수집이 완전했다는 뜻이며 페이지가 시각적으로 정확하다는
뜻이 아니다. 글꼴이 고정되지 않으면 원본 양식 결과의 제한을 명시하며 인증은 성공할 수 없다.

기록된 CLI 실행은 형식별 셀 134개·양식 시작점 6개·페이지 6개로 선택한 두 정책을 통과했다.
그 정책 아래에서 페이지 바깥 경계나 잠재적 충돌은 발견되지 않았다. HWP에는 기대 텍스트
커버리지를 통과하면서 `table_cell_content_overflow` 경고 하나가 남는다. 검토할 수 있게 경고를
유지한다. 이는 한정된 네이티브 백엔드 PDF 생성이며, 모든 네이티브 표의 PDF 지원·환경 간
픽셀 동일성·한컴 동등성의 증거가 아니다.

### 대응하는 MCP 호출 사용

격리된 작업공간에서 stdio 서버를 시작하거나 설정된 MCP 호스트가 시작하게 한다.

```sh
ahwp mcp --root "$PWD" --font-dir "$PWD/fonts"
```

같은 검토 데이터와 단계 경계를 사용한다.

| CLI 단계 | MCP 도구와 인자 |
|---|---|
| Discovery | `hwp_slots`: `path`, `forms: true`, `occurrences: true` |
| 본문 좌표 확인 | `hwp_read`: `path`, `format: "markdown"`, `with_segments: true` |
| 네이티브 범위 | `hwp_split`: `input`, `out_dir`, `from`, `to`, `source_sha256`, `range_report`, `loss_report`, `strict` |
| 새 제목 삽입 | `hwp_edit`: `input`, `output`, `address`, `text`, `before`를 담은 `insert_para` 배열 |
| 삽입 제목 스타일 | 새 주소·`style: "heading"`·`clear_direct: true`를 담은 `apply_style` 배열로 별도 `hwp_edit` |
| Dry-run / 바인딩·반복 | `plan`, `values`, `receipt`, 선택적 `preview`를 담은 단독 `hwp_edit.apply_form_bindings` **객체**; 검토 호출에는 최상위 `dry_run: true` |
| 쪽 나눔 / 들여쓰기 | `set_break_before`·`set_indent_level` 배열을 담은 `hwp_edit`; 먼저 개별 위치 문단 ID 갱신 |
| 비교 / 검증 | `a`/`b`를 받는 `hwp_compare`; `path`를 받는 `hwp_validate` |
| 출력 변환 | `input`, `output`, `strict: true`를 받는 `hwp_convert` |
| PDF 생성 | `path`, `format: "pdf"`, `output_path`, `pages: "all"`, `font_dir`를 받는 `hwp_render` |
| 인증 | `input`, `policy`, `report`를 받는 `hwp_certify` |

바인딩에는 `reference_inputs["binding.ops.json"]`의 객체 하나를 가져와 `op`만 제거하고 새 로컬
영수증·preview 경로를 선택해 `apply_form_bindings` 아래에 둔다. 배열·`verify`·`base_dir`·
`report`·`allow_partial`·스타일 연산을 함께 보내지 않는다. 네이티브 검증은 이 연산에 내재한다.
Dry-run은 네이티브 출력을 발행하지 않는다.

MCP `insert_para`는 CLI 인라인 `style`/`char`를 제공하지 않는다. 위의 별도 스타일 요청을 사용한다.
MCP `hwp_read`는 기존 markdown 좌표만 제공하며 `segments: "v2"`나 JSON과 segments의 조합은
제공하지 않는다. 중첩 편집에는 새 개별 위치의 `paragraph_id`나 스타일의 `used_by`를 사용하고,
새 본문 제목에는 새 본문 좌표를 확인한다. MCP `hwp_render`에는 `report`나 `layout_json` 인자가
없다. 필요한 경우 그 두 산출물은 CLI로 내보낸다. MCP 인증도 자체 정책·렌더 증거를 발행한다.
CLI 플래그를 흉내 내려 도구 인자를 만들어내지 않는다.

참조 MCP 실행은 이 인터페이스로 실제 stdio 도구 호출 22회를 수행했고 모두 통과했다.
최종 HWPX·HWP 바이트가 CLI 산출물과 일치했으며 MCP 인증 두 호출도 통과했다.
이는 fixture에 한정된 관찰이며 보편적인 바이트 동등성 보장이 아니다.

### JSON IR·내장 리소스와 워크플로 구분

확인용 JSON은 일반 텍스트 작성 형식이 아니다. `Paragraph.chars`는 타입이 있는 `HwpChar`
배열이다. 예를 들어 `{"chars":[{"Text":"가"},{"Text":"나"},{"CharCtrl":13}]}`이며 문자열
`"가나"`가 아니다. 네이티브 컨트롤은 자체 너비와 payload를 갖는다. 문자열 인덱싱이나 raw 배열
슬라이싱을 대체 편집 엔진으로 사용하지 않는다.

`ahwp cat --format json`(CLI v2 segments 옆의 문서 포함)은 확인용 내보내기이며 자체 완결
바이너리 리소스 패키지가 아니다. JSON으로 완전한 왕복이 필요하면 다음을 사용한다.

```sh
ahwp convert ./final.hwpx --to json --embed-bin -o ./final.self-contained.json
ahwp new --from ./final.self-contained.json -o ./json-roundtrip.hwpx
ahwp validate ./json-roundtrip.hwpx
ahwp compare ./final.hwpx ./json-roundtrip.hwpx --format json
```

MCP 내보내기는 `input`, `output`, `to: "json"`, `embed_bin: true`를 담은 `hwp_convert`를 사용한다.
재입력은 `output`과 전체 IR JSON **문자열**을 담은 `json`으로 `hwp_new`를 호출한다. 파일명을
`json`에 넣지 않는다. `ahwp convert`는 이 JSON을 네이티브 입력으로 받지 않는다. 기록된 JSON
재입력은 표 6개와 이미지 제어를 유지했고 semantic-identical로 비교됐다.
이 왕복 주장은 해당 fixture의 의미·리소스 확인에 한정하며, 모든 네이티브 형식이나 opaque
레코드의 바이트 동일성을 주장하지 않는다.
Payload 바이트가 없는 내보내기는 이미지 왕복에 불완전하다. 자산 누락 거부는 보호 장치이며
임의의 네이티브 이미지가 손실됐다는 증거가 아니다. 양식 완성에는 네이티브 편집을 우선한다.

### 사람이 수행한 별도 한컴 증거 확보

네이티브 호환성을 주장하기 전에 사람이 정확한 전달 HWP/HWPX 파일을 한컴 오피스에서 열고
글꼴·채운 셀과 이어진 셀 전체·서명·페이지 시작점·유지한 이미지를 확인한 뒤 PDF를 내보낸다.
제품·빌드와 산출물·내보내기 해시를 해당 검증 영수증에 기록한다. CLI로 렌더한 PDF나 통과한
네이티브 전용 인증서는 그 영수증이 아니다. 주장하는 증거 종류에 맞는 공개된 한컴 영수증·
PDF-parity 계약을 따른다. 자동 렌더링으로 사람이 수행한 `pass`를 채우지 않는다.

일치하는 `hancom-verification-receipt-v1`이 있으면 인증 정책에 `receipt`, `require_pass: true`,
`require_artifact_sha256: true`를 담은 `document.hancom_open`을 추가하고 정확한 산출물을 새
리포트 디렉터리로 인증할 수 있다. 기록된 참조 실행에는 한컴 열기·내보내기 증거가 **없다**.
릴리스된 ahwp 2.0.0으로 별도 재실행한 결과에는 에이전트가 수행한(사람이 수행하지 않은) 영수증이
하나 있다. macOS의 한컴오피스 한글 12.30.0이 그 `final.hwpx`/`final.hwp`를 열고 PDF로 내보냈다
(능력 기록의 `hancom_open_export_receipts`). 이 영수증은 해당 2.0.0 해시에만 적용되며 certify,
픽셀 동일성, HWP 넘침 경고는 여전히 미검증이다.

## 네이티브 마이그레이션 대조표

### 상태 용어

- **exact**: 네이티브 명령이 이전 동작 그 자체입니다.
- **composed**: 둘 이상의 네이티브 명령으로 유용한 워크플로우를 만듭니다.
- **limited**: 네이티브 명령이 명시한 안전 경로를 지원하지만 이전 의미를 재현하지 않습니다.
- **intentionally retired**: raw package 수술이나 비네이티브 폴백에는 대체 경로가 없습니다.

| 이전 추론 동작 | 네이티브 명령 순서 | 상태 | 알려진 제한 | 실행 가능한 검증 |
|---|---|---|---|---|
| `summary` | `ahwp info form.hwpx --json` 다음 `ahwp cat form.hwpx --format json > form.json` | composed | 이전 pretty summary 또는 XML section-index map은 없습니다. | `test -s form.json && ahwp validate form.hwpx` |
| `unpack` | unpack하지 말고 `ahwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify`를 사용합니다. | intentionally retired | 지원되는 raw-ZIP 추출 워크플로우는 없습니다. | `ahwp validate edited.hwpx` |
| `repack` | repack하지 말고 `ahwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify`를 사용합니다. | intentionally retired | 네이티브 writer가 package layout을 맡으며 수동 manifest 변경은 지원하지 않습니다. | `ahwp validate edited.hwpx` |
| `edit`의 run-spanning find/replace | `ahwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | limited | 네이티브 replace는 임의 XML text run에 걸쳐 분할된 찾기 문자열의 일치를 보장하지 않습니다. | `ahwp validate edited.hwpx && ahwp cat edited.hwpx --format plain` |
| `fill`의 run-spanning `{{slot}}` | `ahwp slots template.hwpx --json` 다음 `ahwp fill template.hwpx -o filled.hwpx --set "name=value" --json` | limited | 슬롯은 서식 run을 넘을 수 있지만 줄바꿈이나 문단 경계를 넘을 수는 없습니다. | `ahwp slots template.hwpx --json && ahwp validate filled.hwpx` |
| `fill-table` | `ahwp fill template.hwpx -o filled.hwpx --data tables.json --json` | exact | `tables.json`은 네이티브 `tables` 데이터 계약을 따르고 기존 표를 대상으로 해야 합니다. | `ahwp validate filled.hwpx` |
| `fill-form` | `ahwp edit form.hwpx -o filled.hwpx --set-cell-by-label "성명=홍길동" --set-cell-by-label "소속=AI센터" --verify` | limited | 정규화된 각 레이블은 인접 또는 header-row value cell 하나로만 해석되어야 합니다. 모호성이나 중복 대상은 발행을 거부합니다. 재귀 표 범위는 `--label-table N`으로 제한합니다. | `ahwp validate filled.hwpx && ahwp cat filled.hwpx --format plain` |
| `analyze` | `ahwp info form.hwpx --json` 다음 `ahwp cat form.hwpx --format json > form.json` | composed | JSON IR은 텍스트, 표, anchor에 유용하지만 이전 raw `sec` child index나 XML style ID를 내보내지 않습니다. | `test -s form.json && ahwp validate form.hwpx` |
| `edit-section` | `ahwp cat form.hwpx --format json > form.json`, 다음 `ahwp edit form.hwpx -o edited.hwpx --delete-para "old paragraph" --insert-para "anchor=>replacement paragraph" --verify` | limited | anchor/문단 동작은 raw `[start:end)` section-index 수술이 아니며 XML 문단 스타일을 인덱스로 복제하지 않습니다. | `ahwp validate edited.hwpx && ahwp cat edited.hwpx --format json > edited.json` |
| 편집 시 `linesegarray` 정리 | `ahwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | composed | writer가 layout record를 내부 관리하며 사용자 표시 `linesegarray` 스위치나 XML 수준 보고서는 없습니다. | `ahwp validate edited.hwpx` |
| sec-index section 편집 | `ahwp cat form.hwpx --format json > form.json`, 다음 `ahwp edit ... --delete-para ... --insert-para ... --verify` | limited | 네이티브 표면은 안정적인 `sec` child index 계약을 의도적으로 제공하지 않습니다. | `ahwp validate edited.hwpx` |
| mimetype-first STORED repack | `ahwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | intentionally retired | writer는 유효한 package 구성을 보존하지만 일반 raw-ZIP repack API를 노출하지 않습니다. | `ahwp validate edited.hwpx` |
| `guard` / `page_guard` 구조 드리프트 지표 | `ahwp validate edited.hwpx`, 다음 `ahwp render reference.hwpx -o reference.png --report reference.render.json`, `ahwp render edited.hwpx -o edited.png --report edited.render.json` | composed | validation과 render report는 XML 문단/표/텍스트 변화율 임계값을 재현하지 않습니다. layout이 중요하면 report 페이지 수를 비교하고 렌더 결과를 검토하십시오. | `ahwp validate edited.hwpx && test -s reference.render.json && test -s edited.render.json` |

## 편집 전 확인

### 편집 가능한 맵 만들기

네이티브 metadata와 IR 출력으로 시작합니다. JSON에서 안정적인 표시 텍스트, 표 내용 또는
field/bookmark 이름을 anchor로 찾고, 이전 XML index를 추론하지 마십시오.

```bash
ahwp info form.hwpx --json
ahwp cat form.hwpx --format json > form.json
ahwp fields form.hwpx --json
ahwp bookmarks form.hwpx --json
```

### 확인 검증

```bash
test -s form.json
ahwp validate form.hwpx
```

## 네이티브 placeholder와 양식 채우기

### 템플릿 또는 표 채우기

placeholder 작업은 `ahwp fill` 전에 `ahwp slots`를 사용합니다. 데이터 기반 표 행에는 `--data`를
사용하며, 이것이 이전 `fill-table` 명령의 네이티브 대체입니다.

```bash
ahwp slots template.hwpx --json
ahwp fill template.hwpx -o filled.hwpx --set "title=2026 Plan" --json
ahwp fill table-template.hwpx -o table-filled.hwpx --data tables.json --json
ahwp validate filled.hwpx
ahwp validate table-filled.hwpx
```

### label-value 양식 채우기

`--set-cell-by-label`은 `label=value`를 받고 인접 `label | value` 및 header/data-row
레이아웃을 찾으며 기본값으로 atomic입니다. 모호하거나 중복된 레이블을 우회하려고
`--allow-partial`을 사용하지 마십시오.

```bash
ahwp edit form.hwpx -o form-filled.hwpx \
  --set-cell-by-label "성명=홍길동" \
  --set-cell-by-label "소속=AI센터" \
  --verify
ahwp validate form-filled.hwpx
```

## anchor를 통한 내용 편집

### 문단 치환 또는 재구성

지원되는 in-place match에는 `--replace`를 사용합니다. section 형태 변경에는 기존 text anchor와
문단 동작을 사용합니다. validation과 검토를 통과할 때까지 원본은 바꾸지 말고 새 output에
작성합니다.

```bash
ahwp edit form.hwpx -o revised.hwpx \
  --replace "2025 Plan=>2026 Plan" \
  --delete-para "old body paragraph" \
  --insert-para "body heading=>replacement paragraph" \
  --verify
ahwp validate revised.hwpx
```

### 경계 이해하기

네이티브 raw `sec` index editor, XML 문단 스타일 복제 동작, unpack/repack 경로, Python helper는
없습니다. 의도한 편집이 그 의미에 의존하면 이 레시피를 exact migration으로 취급하지 말고
원본 문서를 그대로 보존하십시오.

## 완성 출력 지키기

### 검증하고 렌더하기

validation은 package 구조를 확인합니다. render report는 페이지 수와 renderer 진단을 제공하지만
이전 structural XML-drift 임계값의 대체는 아닙니다.

```bash
ahwp validate revised.hwpx
ahwp render form.hwpx -o reference.png --report reference.render.json
ahwp render revised.hwpx -o revised.png --report revised.render.json
```

### 증거 검토

두 render report 파일을 확인하고 layout이 중요할 때 페이지 이미지를 점검합니다. 페이지 수 변화나
눈에 보이는 overflow는 validation을 약화할 근거가 아니라 내용을 고칠 이유입니다.

## 안전과 비목표

위의 모든 쓰기 명령은 `ahwp validate`로 끝납니다. 네이티브 edit와 fill은 atomic publication을
사용하며, 필요한 변경이 실패하면 기존 output을 그대로 둡니다. 이 레시피는 retired wrapper,
Python XML helper, 실행 가능한 recipe 파일, 이전 바이너리 template을 의도적으로 복원하지 않습니다.
