[한국어](editing-recipes.ko.md) · [English](editing-recipes.md)

# 네이티브 편집 레시피와 이전 `hwpx` 마이그레이션 대조표

이 레시피는 하나의 번들 `hwp` 스킬에서 사용합니다. 이전 별도 `hwpx` 스킬의 편집 안내를
대체하지만 래퍼, 보조 스크립트, 실행 레시피, 바이너리 템플릿을 추가하지 않습니다. 로컬
체크아웃이 아니라 배포된 `hwp` 바이너리에서 모든 명령을 실행하십시오.

## 범위와 릴리스 경계

첫 Phase 2.5 릴리스 태그는 태그 생성 전에 지원 최소 `hwp` 버전을 채웁니다. 그 전까지 이
원본 레시피 쌍은 명령 계약만 설명합니다. 로컬 빌드, 버전 없는 `PATH` 바이너리, 병합 전 브랜치는
retirement 또는 replacement 근거가 아닙니다.

`templates/` 아래의 마크다운 8개는 번들 템플릿 SSOT로 유지합니다. 이전 바이너리 템플릿 6개는
읽기 전용 retirement 호환성 코퍼스이며, 복사, 보관, 편집하지 않습니다.

## 네이티브 마이그레이션 대조표

### 상태 용어

- **exact**: 네이티브 명령이 이전 동작 그 자체입니다.
- **composed**: 둘 이상의 네이티브 명령으로 유용한 워크플로우를 만듭니다.
- **limited**: 네이티브 명령이 명시한 안전 경로를 지원하지만 이전 의미를 재현하지 않습니다.
- **intentionally retired**: raw package 수술이나 비네이티브 폴백에는 대체 경로가 없습니다.

| 이전 추론 동작 | 네이티브 명령 순서 | 상태 | 알려진 제한 | 실행 가능한 검증 |
|---|---|---|---|---|
| `summary` | `hwp info form.hwpx --json` 다음 `hwp cat form.hwpx --format json > form.json` | composed | 이전 pretty summary 또는 XML section-index map은 없습니다. | `test -s form.json && hwp validate form.hwpx` |
| `unpack` | unpack하지 말고 `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify`를 사용합니다. | intentionally retired | 지원되는 raw-ZIP 추출 워크플로우는 없습니다. | `hwp validate edited.hwpx` |
| `repack` | repack하지 말고 `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify`를 사용합니다. | intentionally retired | 네이티브 writer가 package layout을 맡으며 수동 manifest 변경은 지원하지 않습니다. | `hwp validate edited.hwpx` |
| `edit`의 run-spanning find/replace | `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | limited | 네이티브 replace는 임의 XML text run에 걸쳐 분할된 찾기 문자열의 일치를 보장하지 않습니다. | `hwp validate edited.hwpx && hwp cat edited.hwpx --format plain` |
| `fill`의 run-spanning `{{slot}}` | `hwp slots template.hwpx --json` 다음 `hwp fill template.hwpx -o filled.hwpx --set "name=value" --json` | limited | 슬롯은 서식 run을 넘을 수 있지만 줄바꿈이나 문단 경계를 넘을 수는 없습니다. | `hwp slots template.hwpx --json && hwp validate filled.hwpx` |
| `fill-table` | `hwp fill template.hwpx -o filled.hwpx --data tables.json --json` | exact | `tables.json`은 네이티브 `tables` 데이터 계약을 따르고 기존 표를 대상으로 해야 합니다. | `hwp validate filled.hwpx` |
| `fill-form` | `hwp edit form.hwpx -o filled.hwpx --set-cell-by-label "성명=홍길동" --set-cell-by-label "소속=AI센터" --verify` | limited | 정규화된 각 레이블은 인접 또는 header-row value cell 하나로만 해석되어야 합니다. 모호성이나 중복 대상은 발행을 거부합니다. 재귀 표 범위는 `--label-table N`으로 제한합니다. | `hwp validate filled.hwpx && hwp cat filled.hwpx --format plain` |
| `analyze` | `hwp info form.hwpx --json` 다음 `hwp cat form.hwpx --format json > form.json` | composed | JSON IR은 텍스트, 표, anchor에 유용하지만 이전 raw `sec` child index나 XML style ID를 내보내지 않습니다. | `test -s form.json && hwp validate form.hwpx` |
| `edit-section` | `hwp cat form.hwpx --format json > form.json`, 다음 `hwp edit form.hwpx -o edited.hwpx --delete-para "old paragraph" --insert-para "anchor=>replacement paragraph" --verify` | limited | anchor/문단 동작은 raw `[start:end)` section-index 수술이 아니며 XML 문단 스타일을 인덱스로 복제하지 않습니다. | `hwp validate edited.hwpx && hwp cat edited.hwpx --format json > edited.json` |
| 편집 시 `linesegarray` 정리 | `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | composed | writer가 layout record를 내부 관리하며 사용자 표시 `linesegarray` 스위치나 XML 수준 보고서는 없습니다. | `hwp validate edited.hwpx` |
| sec-index section 편집 | `hwp cat form.hwpx --format json > form.json`, 다음 `hwp edit ... --delete-para ... --insert-para ... --verify` | limited | 네이티브 표면은 안정적인 `sec` child index 계약을 의도적으로 제공하지 않습니다. | `hwp validate edited.hwpx` |
| mimetype-first STORED repack | `hwp edit form.hwpx -o edited.hwpx --replace "old=>new" --verify` | intentionally retired | writer는 유효한 package 구성을 보존하지만 일반 raw-ZIP repack API를 노출하지 않습니다. | `hwp validate edited.hwpx` |
| `guard` / `page_guard` 구조 드리프트 지표 | `hwp validate edited.hwpx`, 다음 `hwp render reference.hwpx -o reference.png --report reference.render.json`, `hwp render edited.hwpx -o edited.png --report edited.render.json` | composed | validation과 render report는 XML 문단/표/텍스트 변화율 임계값을 재현하지 않습니다. layout이 중요하면 report 페이지 수를 비교하고 렌더 결과를 검토하십시오. | `hwp validate edited.hwpx && test -s reference.render.json && test -s edited.render.json` |

## 편집 전 확인

### 편집 가능한 맵 만들기

네이티브 metadata와 IR 출력으로 시작합니다. JSON에서 안정적인 표시 텍스트, 표 내용 또는
field/bookmark 이름을 anchor로 찾고, 이전 XML index를 추론하지 마십시오.

```bash
hwp info form.hwpx --json
hwp cat form.hwpx --format json > form.json
hwp fields form.hwpx --json
hwp bookmarks form.hwpx --json
```

### 확인 검증

```bash
test -s form.json
hwp validate form.hwpx
```

## 네이티브 placeholder와 양식 채우기

### 템플릿 또는 표 채우기

placeholder 작업은 `hwp fill` 전에 `hwp slots`를 사용합니다. 데이터 기반 표 행에는 `--data`를
사용하며, 이것이 이전 `fill-table` 명령의 네이티브 대체입니다.

```bash
hwp slots template.hwpx --json
hwp fill template.hwpx -o filled.hwpx --set "title=2026 Plan" --json
hwp fill table-template.hwpx -o table-filled.hwpx --data tables.json --json
hwp validate filled.hwpx
hwp validate table-filled.hwpx
```

### label-value 양식 채우기

`--set-cell-by-label`은 `label=value`를 받고 인접 `label | value` 및 header/data-row
레이아웃을 찾으며 기본값으로 atomic입니다. 모호하거나 중복된 레이블을 우회하려고
`--allow-partial`을 사용하지 마십시오.

```bash
hwp edit form.hwpx -o form-filled.hwpx \
  --set-cell-by-label "성명=홍길동" \
  --set-cell-by-label "소속=AI센터" \
  --verify
hwp validate form-filled.hwpx
```

## anchor를 통한 내용 편집

### 문단 치환 또는 재구성

지원되는 in-place match에는 `--replace`를 사용합니다. section 형태 변경에는 기존 text anchor와
문단 동작을 사용합니다. validation과 검토를 통과할 때까지 원본은 바꾸지 말고 새 output에
작성합니다.

```bash
hwp edit form.hwpx -o revised.hwpx \
  --replace "2025 Plan=>2026 Plan" \
  --delete-para "old body paragraph" \
  --insert-para "body heading=>replacement paragraph" \
  --verify
hwp validate revised.hwpx
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
hwp validate revised.hwpx
hwp render form.hwpx -o reference.png --report reference.render.json
hwp render revised.hwpx -o revised.png --report revised.render.json
```

### 증거 검토

두 render report 파일을 확인하고 layout이 중요할 때 페이지 이미지를 점검합니다. 페이지 수 변화나
눈에 보이는 overflow는 validation을 약화할 근거가 아니라 내용을 고칠 이유입니다.

## 안전과 비목표

위의 모든 쓰기 명령은 `hwp validate`로 끝납니다. 네이티브 edit와 fill은 atomic publication을
사용하며, 필요한 변경이 실패하면 기존 output을 그대로 둡니다. 이 레시피는 retired wrapper,
Python XML helper, 실행 가능한 recipe 파일, 이전 바이너리 template을 의도적으로 복원하지 않습니다.
