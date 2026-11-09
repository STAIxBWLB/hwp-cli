[한국어](26-original-form-certification.ko.md) · [English](26-original-form-certification.md)

# 원본 양식 인증

`hwp certify`는 전달된 HWP/HWPX 산출물을 제공받은 네이티브 양식의 검토된 기대 계획과 대조할 수
있다(#419, #411의 일부). 원본, 계획, 출력, 글꼴, 렌더 식별자를 한 리포트에 묶고, 모든 거부는 문서
텍스트 없이 원인과 다음 조치를 알린다.

## 입력

인증 정책에 선택적 섹션 하나가 추가된다.

```json
"document": {
  "fonts": {"manifest": [{"path": "fonts/NanumGothic.ttf", "sha256": "..."}]},
  "original_form": {"plan": "plan.json", "address_map": false}
}
```

`plan`은 정책 파일 옆의 `original-form-certification-plan-v1` 파일이다. 기대 텍스트를 담으므로
로컬에 둔다. certify는 한 번 읽어 해시하고, 리포트 디렉터리·거부 메시지·로그로 복사하지 않는다.
계획에 선언할 수 있는 항목은 다음과 같다.

| 필드 | 의미 |
|---|---|
| `source_sha256` | 제공받은 원본 양식의 식별자. 리포트에 그대로 반영한다 |
| `output_sha256` | 있으면 인증 대상 산출물의 해시가 정확히 같아야 한다 |
| `table_count` | 원본 양식의 표 개수. `--set-cell`과 같은 재귀 순서다 |
| `form_starts` | 필수 양식 시작 앵커. 각 양식을 여는 최상위 본문 문단의 정확한 텍스트와 `page_start`(기본 참), 선택적 `page` |
| `tables` | 안정된 격자 투영. 표 번호별 행·열 수 |
| `expected_cells` | `table`/`row`/`col`별 기대 값. 공백 묶음은 정규화하고 보이는 문자는 모두 같아야 한다 |
| `allowances.repeated_tables` | 전달 양식이 반복하는 프로필 표. `extra_copies`개 복제가 그 표 바로 뒤에 오고 격자를 유지한다. `table_count`도 그만큼 늘어난다 |
| `allow_year_change`(셀별) | 독립된 네 자리 연도(1900~2099)는 달라도 된다 |

계획 항목마다 안정된 ID(`[A-Za-z0-9._-]`, 64자 이하)가 있다. 발견 사항은 이 ID를 쓰며, 리포트는
ID가 가리키는 텍스트를 반영하지 않는다.

## 검사 항목과 근거

| 검사 | 근거 | 발견 코드 |
|---|---|---|
| 출력 식별자 | 인증 입력의 SHA-256 | `output_identity_mismatch` |
| 표 개수와 격자 | 파싱한 문서 | `table_count_changed`, `table_missing`, `repeated_table_missing`, `table_grid_changed` |
| 기대 셀 값 | 파싱한 문서 | `expected_cell_missing`, `expected_cell_value_missing`, `expected_cell_value_mismatch` |
| 기대 셀 그려짐 | 인증 레이아웃의 세그먼트 행 | `expected_cell_not_rendered` |
| 양식 시작의 존재와 유일성 | 파싱한 문서 | `form_start_missing`, `form_start_ambiguous` |
| 양식 시작 배치 | 인증 레이아웃의 세그먼트 행 | `form_start_not_rendered`, `form_start_on_wrong_page`, `form_start_not_at_page_start` |
| 잘린 행, 불완전한 셀 분할 | 같은 레이아웃의 typed 렌더 이슈 | `table_row_clipped`, `cell_fragmentation_incomplete` |
| 허용되지 않은 글꼴 대체 | 정책이 `fonts.forbid_substitution`을 켤 때 렌더의 글꼴 해석 | `disallowed_font_substitution` |

배치와 맞춤은 레이아웃 자신의 typed 이슈와 세그먼트 행에서 읽는다
(`hwp_render::layout_segments_isolated_bounded`: 인증 렌더와 같은 격리 글꼴 집합과 레이아웃 예산).
텍스트 길이로 추정하지 않는다. 반복되는 표 머리행이 빠진 본문 행을 가리지 않는다. 기대 셀은 하나씩
검사하고, 잘림은 머리행과 무관하게 레이아웃이 보고한다.

발견 사항마다 `next_step` 코드도 있다. 예: `fill_the_expected_cell`,
`restore_the_page_break_before_the_form`, `pin_the_requested_fonts_and_rerender`.

## 제한된 결과

쪽 배치는 줄 나눔에, 줄 나눔은 글꼴에 달려 있다. 그래서 배치·잘림·분할은 글꼴 환경이 고정됐을
때만 발견 사항이 된다. 글꼴 manifest가 있고, 대체를 금지하며, 요청한 글꼴이 모두 일치해야 한다.
그렇지 않으면 검사는 `limited: true`와 `missing_evidence`의 `pinned_font_environment`를 보고하고
인증은 `passed`가 되지 않는다. 통과했지만 제한된 검사는 전체 결과를 `partial`로 만든다. 그 밖의
누락 증거 코드는 `render_layout_unavailable`, `segment_map_truncated`,
`cell_render_location_unavailable`(글상자 안의 표), `parsed_document`이다. 정책이 허용한 대체
(`forbid_substitution: false`)는 발견 사항이 아니다. 환경이 고정되지 않았을 뿐이므로 결과는 제한된다.

쓸 수 없는 계획은 내용 없는 원인 분류 하나로 거부한다. `original_form_plan_unreadable`(경로·파일 없음·
크기), `original_form_plan_not_json`(계획의 닫힌 JSON 형태가 아님), `original_form_plan_contract`(계약·
버전), `original_form_plan_bounds`(개수·번호·쪽·텍스트 길이·해시, 또는 검사 항목 없음),
`original_form_plan_ids`(잘못되거나 중복된 ID, 같은 주소의 셀 둘)이다.

## 분리해 두는 것

`checks.original_form`은 기존 검사 옆에 놓이며 그 결과를 바꾸지 않는다. 구조 유효성(`package`),
의미 보존(`repeat_import_consistency`, `preservation`), 렌더 품질(`render`), 실기 열기 증명
(`hancom_open`)은 각자의 결과를 유지한다. `render-report.complete`는 진단 완전성이라는 뜻을 유지하며,
여기서 시각적 정확성으로 읽지 않는다. 이 검사의 어떤 결과도 한컴 렌더링 동등성을 뜻하지 않는다.

## 개인정보와 로컬 주소 맵

리포트·거부 메시지·로그는 모든 모드에서 안정된 ID, 원인 코드, 다음 조치 코드만 담는다.
`address_map: true`이면 리포트 디렉터리에 `original-form/address-map.json`
(`original-form-address-map-v1`)도 생긴다. 발견 사항 ID마다 구역/문단, 표/행/열, 쪽을 적는다.
텍스트는 없지만 내용의 위치를 알려 주므로 계획과 함께 로컬에 둔다. `manifest.json`에는 나열되지만
`report.artifacts`에는 속하지 않는다. 게시 manifest의 258개 상한을 함께 쓰므로, 이미 상한에 이른
리포트(256쪽과 오라클 PDF)에서는 생략된다. `address_map`은 요청 여부, `address_map_published`는 파일이
실제로 있는지를 나타낸다. MCP의 `hwp_certify`도 같은 디렉터리를 쓰며 맵을
응답에 직접 싣지 않는다.

## 스키마

- `schemas/certification-policy-v1.schema.json` (`document.original_form`)
- `schemas/certification-report-v1.schema.json` (`checks.original_form`)
- `schemas/original-form-certification-plan-v1.schema.json`
- `schemas/original-form-address-map-v1.schema.json`

## 근거와 한계

검증 범위는 직접 작성한 P0 양식 집합이다(`tools/generate_original_form_fixture.py`, #416).
`crates/hwp-cli/tests/original_form_certify.rs`는 올바르게 채운 양식을 인증하고, 잘린 행·빠진 셀
값·옮겨진 양식 시작·대체된 글꼴을 각각 고유 원인으로 실패시킨다. 글꼴을 고정하지 않은 실행과
정책이 허용한 대체는 제한된 결과로 누락 증거를 밝힌다. 글꼴 고정 사례에는 호스트의 단일 face TrueType 글꼴 하나가
필요하며(Linux CI는 fonts-nanum, macOS는 AppleGothic), 그 밖의 환경에서는 누락 자원으로 집계된다.
이는 로컬 렌더러 결과이며 한컴 출력이 아니다.
