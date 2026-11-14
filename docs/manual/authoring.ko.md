[한국어](authoring.ko.md) · [English](authoring.md)

# 문서 작성

새 HWP/HWPX 파일을 만드는 방법을 다룬다. DocumentSpec·TemplateSpec 명세에서 합성하기, 공문서 작성하기, JSON IR에서 재생성하기다. 기존 문서를 고치는 것이 아니라 문서를 새로 쓰는 사람과 에이전트를 위한 페이지다.

## 명세로 문서 합성하기

`compose`는 v1 JSON/YAML 명세를 문단·런·스타일·목록·표·수식·필드·머리말/꼬리말·쪽 번호·구역으로
컴파일한다. v2는 그 문서를 감싸고 접근성 설명이 포함된 이미지, 닫힌 SVG→PNG fallback, HWPX 네이티브
사각형 글상자를 더한다. v2 fallback policy는 타겟별로 명시하며 생략하면 native-only로 실패한다.

```sh
# 쓰지 않고 스키마·참조·표 span·자산·네이티브 지원 여부 확인
ahwp compose examples/document-spec-v1/basic.json -o /tmp/basic.hwpx --dry-run --report

# 검증된 파일을 원자적으로 게시
ahwp compose examples/document-spec-v1/comprehensive.yaml -o /tmp/report.hwpx --report
```

`template`은 문자열 보간이나 표현식 실행 없이 typed `value`/`if`/`each` AST만 허용한다. `reference_hwpx`는
기존 패키지의 지정 텍스트/필드만 외과적으로 채우며, 구조 재생성은 명시적 strict gate가 필요하다.

정본 스키마는 [`schemas/`](../../schemas/)에, 계약 문서는 [13-document-spec-v1](../design/13-document-spec-v1.ko.md),
[14-template-spec-v1](../design/14-template-spec-v1.ko.md), [15-document-spec-v2](../design/15-document-spec-v2.ko.md)에 있다.
예제는 [`examples/`](../../examples/).

## 공문서 작성하기

한국 공문서는 표기법과 배치가 정해져 있다. 이 계층은 프롬프트에 맡기지 않고 바이너리에 내장했다.

**프로파일**(`--preset`)은 문서 종류별 여백, 번호 매기기, 문단 모양을 지정한다. `official`(기안문·
공문서), `report`(보고서), `plan`(사업계획서), `notice`(공고문), `minutes`(회의록),
`press`(보도자료) 여섯 가지가 있다. `기안문`, `보고서`, `공고` 같은 한국어 별칭도 같은 여섯 가지로
정규화된다. 중첩 목록은 법정 8단계 항목 부호로 렌더된다.

**템플릿**(`--template`)은 여기서 한 걸음 더 나아간다. 각 템플릿은 자체 프로파일과 함께 네이티브
두문·결문 표를 가져오며, 그 표는 `{{슬롯}}`으로 미리 채워져 있다.

| 슬러그 | 한국어 별칭 | 비고 |
|---|---|---|
| `gian-internal` | 기안문-내부결재 | 내부 결재용 |
| `gian-external` | 기안문-대외시행 | 대외 시행용 |
| `gongmun-basic` | 공문서-기본 | 다수 수신 형식(`{{수신자}}`) |
| `report` | 보고서 | |
| `plan` | 사업계획서 | |
| `minutes` | 회의록 | |
| `notice` | 공고문 | |
| `press` | 보도자료 | |

**문서 틀** 플래그는 그 두문·결문 항목을 직접 채운다. `--doc-head 기관명=…`,
`--doc-foot 발신명의=…`, `--notice-head`, `--notice-foot`, `--press-head`가 여기에 해당한다.
v0.10.0부터 `--preset`과 문서 틀 플래그는 거부되지 않고 템플릿 자체 기본값을 **덮어쓴다**.

**채우기** 생성된 문서 자체가 템플릿이다. `ahwp slots`는 아직 채우지 않은 항목을 나열하고,
`ahwp fill`은 `{{name}}`을 치환하면서 hwpx 패키지의 나머지 부분을 바이트 단위로 보존한다. 두 명령은
슬롯을 같은 방식으로 읽는다. 이름은 중괄호·제어문자를 뺀 임의 텍스트이고(`{{성 명}}`,
`{{사업명(국문)}}`), 중괄호 안 공백(`{{ name }}`)과 요청한 이름의 앞뒤 공백은 제거한다. 값은 한 번에
그대로 삽입하므로, 슬롯처럼 생긴 값이 다시 채워지지 않는다. 일치하지 않는 슬롯이 있으면 오류로
처리하며, `--allow-partial`을 주지 않는 한 아무것도 쓰지 않는다. `--allow-partial`이면 하나도
일치하지 않아도 입력을 그대로 게시하고 건수 0을 보고한다.

**양식 필드** 한국 공문서 양식에는 슬롯이 거의 없다. `ahwp slots --forms`는 레이블 셀(성명, 연락처
등), 본문의 `라벨: 값`, 슬롯을 키·레이블·출처·신뢰도·등장 횟수와 함께 나열한다. `ahwp fill --forms`는
같은 `--data`/`--set` 값으로 이들을 한 번에 채운다. 레이블 셀 옆이나 아래 칸, 본문 라벨 뒤 텍스트,
`학년(  )반` 빈칸, `□동의` 확인란, `(비고:   )` 빈칸이 대상이다. 비어 있지 않은 값 칸과 `라벨:` 뒤
본문 텍스트(쉼표, 세미콜론, 다음 `라벨:`, 줄 끝까지. `(한문:   )`처럼 라벨을 감싼 괄호와 `(서명/인)`
같은 괄호 메모는 남김)는 kordoc처럼 덮어쓰며, 덮어쓸 때마다 보고서에
경고를 남긴다. 누름틀·그림·표가 든 칸은 쓰지 않고, 값은 적힌 그대로 넣는다. 키는 공백·쌍점·괄호를 무시하고 비교하며,
`--json`에 `unmatched` 키가 추가된다. 규칙은 [kordoc](https://github.com/chrisryugj/kordoc)(MIT)에서
이식했다. 이 경로는 IR을 거치며 `.hwpx`만 쓴다. 바뀐 구역만 `ahwp edit`와 같은 방식으로 다시 쓰고,
나머지 구역과 패키지 엔트리는 바이트 단위로 복사한다.

**린트** `ahwp lint`는 `.md`, `.hwp`, `.hwpx` 파일에(또는 `-`로 표준 입력의 markdown에) 10개 규칙을
적용한다. 표기법 규칙 7개(날짜 `2026. 8. 20.`, 시각, 금액, `붙임:`의 쌍점과 번호, 마무리 `끝.`,
문장부호), 장식 기호(`■ ▶ ▲ ◆ ● ※`. 공문서에서는 `□ ○`나 중첩 목록으로 써야 한다)를 잡는 규칙
1개, 그리고 법정 항목 부호와 로마자 제목을 검사하는 구조 규칙 2개로 이루어진다. 오류 등급은 구조
규칙 2개뿐이고 표기법 지적은 모두 경고다. 이 명령은 권고 성격이라 `--strict`를 주지 않으면 항상 0으로
종료한다.

**표 서식** `ahwp edit --style-tables <preset>`은 해당 프로파일의 표 모양을 적용한다. 단일 열 표는
건너뛰며, 두 번 적용해도 바이트가 동일한 결과가 나온다.

## JSON IR로 문서 재생성하기

기존 문서를 편집하지 않고 처음부터 똑같이 신규 생성할 수 있다.

```sh
ahwp convert report.hwpx -o report.json   # hwp/hwpx → JSON IR (--embed-bin: 이미지까지)
ahwp new --from report.json -o regen.hwpx # JSON IR → 신규 문서
```

재생성 검증(`crates/hwp-cli/tests/regen.rs`)은 validate 통과, `ahwp cat` 출력 전문 동일, 표 지도
(개수·행/열·병합·셀 폭) 동일, secPr·tabProperties 슬라이스 바이트 동일을 확인한다. 차이가 남는 것은
설계상 재생성 대상인 줄 배치 캐시(한글이 열 때 재계산), 미리보기 이미지, settings.xml뿐이다.
