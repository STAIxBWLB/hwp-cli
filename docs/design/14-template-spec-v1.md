[한국어](14-template-spec-v1.md) · [English](14-template-spec-v1.en.md)

# TemplateSpec · TemplateData v1

상태: 고정된 v1 계약. 규범 스키마는 `schemas/template-spec-v1.schema.json`,
`schemas/template-data-v1.schema.json`, `schemas/template-report-v1.schema.json`이다.
DocumentSpec v1은 고정된 채이며 유일한 네이티브 재생성 대상이다.

고정 SHA-256:

| 스키마 | SHA-256 |
|---|---|
| TemplateSpec v1 | `590b9ac7dd2b30d1f8fafc4e087adf3117a831f9e38de39267a102141c549039` |
| TemplateData v1 | `484bc86d01dcba17122507fad250791f88235be4dd933c12c721ef7b46eea298` |
| TemplateReport v1 | `aa2f011e02a52b29d07a458f84875e512cf1b1c80e6f2edea40ce756d436f705` |

## 목표

- 문자열 구분자 치환 없이 typed 데이터를 네이티브 HWP/HWPX로 렌더한다.
- 확장을 결정론적·bounded·비실행으로 유지하고 JSON Pointer로 진단 가능하게 한다.
- 기존 텍스트 자리표시자나 필드만 바뀔 때는 패키지 외과적(reference HWPX) 채우기를 우선한다.
- 재생성은 명시적으로만 하고, 참조 문서에 미지원 객체가 있으면 fail closed한다.

## 최상위 계약

TemplateSpec:

```json
{
  "version": "1.0",
  "variables": {},
  "source": { "mode": "compose", "document": {} }
}
```

TemplateData:

```json
{
  "version": "1.0",
  "values": {}
}
```

모르는 속성은 오류다. JSON과 YAML은 같은 serde 모델로 매핑된다. 값은 절대 강제 변환되지 않는다.
따옴표 친 숫자는 문자열이고, `yes` 같은 YAML 표기는 이 계약에서 불리언이 되지 않는다.

## Typed 변수

스칼라 타입은 `string`·`number`·`bool`·`date`·`enum`이다. `rich_blocks`는 네이티브 DocumentSpec v1
블록 객체를 담는다. `list`는 닫힌 스칼라 필드 스키마를 선언하며 bounded 행·블록 반복의 입력 타입이다.

모든 타입이 `required`·`default`·`secret`을 지원한다. 적용 가능한 제약은 다음과 같다.

- string: `regex`, `min_length`, `max_length`
- number: 유한한 `min`, `max`
- date: 정확한 그레고리력 `YYYY-MM-DD`와 `min`, `max`
- enum: 유일한 문자열 1~256개
- rich_blocks·list: `min_items`, `max_items`

Rust의 선형 시간 정규식 엔진을 패턴 1,024 유니코드 스칼라 제한, bounded 중첩, bounded 오토마타 크기로
컴파일한다. 진단은 JSON Pointer와 규칙 이름을 밝히되 거부된 값은 절대 포함하지 않는다.

이름은 `[A-Za-z][A-Za-z0-9_]{0,63}`을 따른다. `__proto__`·`prototype`·`constructor`는 거부하는데,
다른 런타임이 프로토타입이나 경로 키 모호성 없이 이 계약을 구현할 수 있게 하기 위해서다.

## 명시적 AST

compose·재생성 모드에는 `${...}`·`{{...}}`·표현식 언어·코드 실행·동적 속성 조회·include·매크로·
템플릿 호출이 **없다**.

값 바인딩:

```json
{ "node": "value", "pointer": "/values/title", "as": "text" }
```

허용되는 포인터는 `/values/<선언된-이름>`과, `each` 안에서의 `/item/<선언된-필드>`뿐이다.
`as: native`는 JSON 스칼라 타입을 보존한다. `as: text`는 문자열·숫자·불리언·enum·date를 결정론적으로
포맷한다. rich block은 블록 배열에만 끼워 넣을 수 있고 유일한 `region` id를 가져야 한다.

조건 영역:

```json
{
  "node": "if",
  "condition": "/values/show_summary",
  "region": "summary",
  "then": [],
  "else": []
}
```

반복 영역:

```json
{
  "node": "each",
  "items": "/values/items",
  "region": "item_rows",
  "body": []
}
```

`if`와 `each`는 DocumentSpec 블록 배열, 머리말/꼬리말 블록 배열, 표 셀 블록 배열, 표 `rows`의
항목으로만 올 수 있다. 스타일·열·run·임의 객체 속성·metadata 컬렉션은 바꿀 수 없다. 중첩 제어는
bounded이며 언어에 재귀가 없다.

## 고정 예산

| 자원 | 제한 |
|---|---:|
| 템플릿 입력 | 4 MiB |
| 데이터 입력 | 8 MiB |
| 변수 | 1,024 |
| 정규식 원문 | 1,024 유니코드 스칼라 |
| 문자열 하나 | 2,000,000 유니코드 스칼라 |
| rich block | 20,000 |
| list 하나 / each 하나 | 10,000 항목 |
| each 반복 총합 | 100,000 |
| 제어 깊이 | 8 |
| 확장 노드 | 250,000 |
| 확장 JSON | 16 MiB |
| region | 20,000 |
| 오류 봉투 | 64 KiB |
| 성공 리포트 | 64 MiB |

DocumentSpec 자체의 구역·블록·run·셀·텍스트·자산 예산은 확장 뒤에 적용한다.

## Source 모드

### `compose`

확장된 AST는 고정 DocumentSpec v1으로 역직렬화돼야 한다. 기존 compose 컴파일러와 원자적 의미 검증
경로로 넘어간다. writer DROP 경고는 치명적이다.

### `reference_hwpx`

바인딩은 기존 자리표시자 이름이나 HWPX 필드 이름을 대상으로 한다. 값은 스칼라여야 한다. 구현은
staging 전에 입력 패키지 전체를 검증하고, 선택된 section XML만 바꾸고, 건드리지 않은 ZIP local
record·압축 페이로드·central directory metadata를 raw 복사하며(필요한 새 local header 오프셋만
바뀐다), staged 패키지와 의미 결과를 검증한 뒤 원자적으로 게시한다. 중복 대상, 없는 대상, 모호한
중복 필드, 텍스트가 아닌 필드 영역, 패키지 예산을 넘는 출력 증가, 해소되지 않은 요청 대상은 목적지를
바꾸지 않고 실패한다.

자리표시자 매칭은 네임스페이스를 인식한다. 규범 HWPX 문단 네임스페이스의 local name `t` 아래 문자
데이터만 대상이다. 속성·컨트롤 metadata·주석·CDATA·비텍스트 노드·외부 네임스페이스에 있는 요청
자리표시자는 fail closed한다. 필드 채우기는 텍스트와 줄바꿈만 담은 모호하지 않은 필드 하나만 받는다.

참조 문서는 한 번 연 핸들에서 비공개 스냅샷으로 복사한다. SHA-256, strict 게이트, 패키지 패치,
의미 검증이 모두 같은 바이트를 쓴다. 명령 시작 시점의 목적지 스냅샷 하나가 최종 원자적 게시까지
권위를 가지며, 목적지 경합은 새 덮어쓰기 기준이 되는 대신 거부된다.

이것은 패키지 외과적 보존이지 파일 전체 바이트 동일성이 아니다. 리포트는 바뀐 영역을 나열하되 데이터
값은 절대 담지 않는다.

보존 경계는 정확하지만 좁다.

- 자리표시자는 규범 HWPX 텍스트 노드 하나에 온전히 들어 있어야 한다. run·텍스트 노드로 쪼개진
  자리표시자는 미해소로 처리돼 요청 대상 검사에서 실패한다.
- 요청된 필드는 정확히 한 번 나타나야 하고 `hp:t` 텍스트와 `hp:lineBreak`만 담아야 한다.
- 바뀐 section은 재직렬화·재압축되며 패키지 전체 오프셋과 EOCD는 필연적으로 바뀐다.
- `changed_regions`는 검증된 논리 바인딩과 인스턴스 수 총합을 기록할 뿐, 바이트 수준 XML diff나
  건드린 모든 ZIP record 목록이 아니다.
- 건드리지 않은 엔트리의 local record·압축 페이로드·central metadata는 보존되지만, 결과 패키지
  전체가 바이트 동일하다고 주장하지 않는다.

### `reference_regenerate`

구조적 `if`·`each` 요청은 패키지 외과 모드를 조용히 벗어날 수 없다. 명시적 `reference_regenerate`
source 모드와 리터럴 `strict_unsupported_objects: true`를 요구한다. compose 전에 참조 문서를 읽어
소실될 미지원·불투명 콘텐츠가 있는지 검사하고, 있으면 `unsupported_reference_object`로 실패한다.
성공 출력은 보존이 아니라 재생성으로 보고된다.

재생성은 편집을 참조 패키지에 병합하지 않는다. 참조는 명시적 strict 호환 게이트일 뿐이며, 출력 문서는
`source.document`의 확장된 고정 DocumentSpec만으로 전부 컴파일된다. 거기 다시 기술되지 않은 내용·
레이아웃·metadata·패키지 산출물은 승계되지 않는다. 이 게이트는 현재 reader와 writer 경고가 드러내는
미지원·불투명 콘텐츠를 거부할 수 있으나, 앞으로의 임의 HWPX 확장이 모델링돼 있다는 증명은 아니다.

## CLI · MCP 계약

CLI:

```text
hwp template TEMPLATE --data DATA -o OUTPUT
  [--template-format json|yaml] [--data-format json|yaml]
  [--dry-run] [--report]
```

파일 경로와 참조 자산은 TemplateSpec 디렉터리 기준으로 해소한다. `--dry-run`은 파싱·검증·확장,
해당되면 참조 검사, DocumentSpec 컴파일러 실행까지 하되 게시하지 않는다. compose와 참조 재생성
dry-run은 출력 의미·패키지 검증을 `not_run`으로 보고한다. 패키지 보존 참조 dry-run은 비공개 패키지를
실제로 만들어 검증하므로 두 상태 모두 `passed`이며 목적지는 건드리지 않는다.

MCP 도구 `hwp_template`은 `template`/`template_path` 중 정확히 하나, `data`/`data_path` 중 정확히
하나, 같은 선택적 포맷 둘, 인라인 입력용 `base_dir`, `output`, `dry_run`을 받고 모르는 인자는 받지
않는다. MCP와 CLI는 같은 실행기와 오류 봉투를 쓴다.

```json
{
  "error": "template_spec",
  "issues": [
    { "code": "type_mismatch", "pointer": "/values/count", "message": "..." }
  ]
}
```

네이티브 stderr와 MCP 오류는 이 값 없는 봉투를 담는다. 성공 출력은 같은 리포트 객체다.

## 보존 리포트

기계 판독 리포트는 다음을 담는다.

- 모드, dry-run 여부, 결정성 플래그
- 템플릿·데이터·참조·출력 바이트의 SHA-256
- 제공·기본값 적용된 변수 **이름**(값은 절대 아님)
- bounded 확장 카운트와 변경·생성된 region id
- unsupported·fallback·dropped 배열
- 템플릿/데이터, 의미, 패키지 검증 상태
- 재생성 시 중첩된 compose 리포트

v1에서 성공하려면 `fallback`과 `dropped`가 비어 있어야 한다. 해시는 정확한 입력·출력 바이트를 기술할
뿐 의미·패키지 검증을 대신하지 않는다.

반복·조건 영역은 논리적 집계다. `input_items`와 `generated_items`는 중첩된 모든 발생을 합산하고,
`instances`는 안정적인 경로(`/`, `/0`, `/0/1`, ...)와 인스턴스별 카운트를 기록한다. 이렇게 해서 논리
region id 중복 없이 결정론적 감사 상세를 유지한다.
