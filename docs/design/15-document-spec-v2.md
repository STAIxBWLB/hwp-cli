[한국어](15-document-spec-v2.md) · [English](15-document-spec-v2.en.md)

# DocumentSpec v2

## 상태와 정본

`schemas/document-spec-v2.schema.json`이 규범적 입력 계약, `schemas/document-report-v2.schema.json`이
규범적 compose 리포트 계약이다. `hwp_cli::document_spec_v2`가 둘의 닫힌 Rust/serde 구현이다.
DocumentSpec v1은 그대로이며 v2의 `document` 속성 아래에 중첩된다.

v2는 writer와 쓰기 후 검증기가 **증명할 수 있는** 시각 연산만 의도적으로 고정한다.

- PNG·JPEG·GIF·BMP의 정확한 임베드
- fallback이 명시적으로 허용됐을 때의 결정론적 crop·회전·리사이즈 후 PNG 변환
- bounded·닫힌 SVG 기하 부분집합을 결정론적 PNG로 래스터화
- HWPX의 네이티브 인라인 사각형 글상자

차트, 다이어그램, 임의 도형, 부유 배치, SVG 텍스트, 그리고 모든 암묵적 시각 fallback은 이 버전
밖이다. 결정론적 폰트·렌더링 계약이나 네이티브 writer 계약이 생긴 뒤의 이후 스키마 버전이 필요하다.

## 타겟 정책

모든 시각 객체는 `policy.hwp`와 `policy.hwpx` 값을 독립적으로 가진다.

- `required_native`(기본): 타겟이 요청된 표현을 네이티브로 보존할 수 없으면 실패
- `prefer_native`: 가능하면 네이티브, 아니면 증명된 시각 fallback만 사용
- `force_visual_fallback`: 증명된 fallback 경로를 요구

생략된 타겟은 `required_native`가 기본이다. deprecated된 CLI/MCP `allow_visual_fallback` 플래그는
DocumentSpec v1 호환 입력으로만 남아 있다. v2와 함께 주면 typed `policy_conflict`이며, 타겟별 v2
정책을 절대 덮어쓰지 않는다.

## 접근성과 배치

`alt`는 필수다. 임베드 객체 설명은 서로 다른 비어 있지 않은 title이 있으면 `title + "\n\n" + alt`로,
아니면 trim된 `alt`로 유도한다. 결과는 XML 안전해야 하고, 캐리지 리턴을 포함하지 않아야 하며,
UTF-16 코드 단위 65,535 안에 들어가야 한다. 글상자 내용도 같은 문자 게이트를 적용한다.

v2는 `inline` 배치만 지원한다. 한 문단 위치의 여러 객체는 문단 컨트롤 순서를 통해 배열 순서를
유지하며, 인라인 z-order는 HWP·HWPX 모두에서 규범적으로 0이다.

## 격리된 자산과 SVG fallback

자산 경로는 상대 normal component만 포함한다. 컴파일러는 링크를 따라가지 않고 spec 디렉터리 아래에서
열며, 하드링크를 거부하고, bounded 불변 스냅샷을 한 번 읽어 검증·해시·변환·임베드에 **같은 바이트**를
쓴다. 경로는 유니코드 스칼라 4,096, UTF-8 바이트 4,096으로 제한한다. JSON Schema `maxLength`는 스칼라
경계를 표현하고, 런타임 격리 게이트가 두 경계를 모두 강제한다.

SVG 부분집합은 `svg`·`g`·`rect`·`ellipse`·`circle`·`line`·`polyline`·`polygon`과 요소별 수치·색
속성만 허용한다. DTD·처리 명령·스크립트·스타일·텍스트·path·transform·접두사·외부 참조·리소스 요소·
미지원 속성은 거부한다. 정규화·소독된 SVG는 리소스 조회를 끈 채 파싱해 선언된 출력 픽셀 크기로
렌더한다. 원본 SVG는 절대 임베드하지 않는다. 리포트는 원본·소독본·의미·최종 PNG 해시를 구분한다.

항목별·총합 바이트, 픽셀, 요소, 중첩, 점, 렌더 작업량 예산을 게시 전에 검사한다. 비어 있거나 완전히
투명한 SVG 렌더는 실패한다.

crop 필드는 JSON Schema가 각각 0..1로 제한한다. draft 2020-12는 형제 속성 간 산술을 표현할 수 없으므로
런타임 의미 게이트가 `x + width <= 1`, `y + height <= 1`을 추가로 요구하며, 위반은 typed `invalid_crop`
오류다.

## 실행과 의미 검증

CLI와 MCP는 최상위 `version`으로 분기하고 같은 컴파일·게시 경로를 공유한다. dry-run이 아닌 합성은
출력을 원자적으로 staging하고, writer `DROP:` 경고를 거부하며, staged HWP/HWPX를 다시 열어 전체 규범
문서 투영을 비교한다. 투영은 writer가 생성한 정확한 뼈대와 캐시만 정규화한다. PNG 바이트와 해시
동일성, 치수, 유도된 객체 설명, 인라인 배치·순서, 그 밖의 모든 활성 의미는 비교 대상으로 남는다.

리포트에는 원본 경로·title·대체 텍스트·설명·출력 경로가 담기지 않는다. 안정적인 정책·표현 사유와
의미·원본·소독기·미디어 해시만 기록한다.
