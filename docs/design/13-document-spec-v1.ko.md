[한국어](13-document-spec-v1.ko.md) · [English](13-document-spec-v1.md)

# DocumentSpec v1

## 상태와 정본

`schemas/document-spec-v1.schema.json`이 네이티브 구조 저작의 규범적 wire 계약이고,
`hwp_cli::document_spec`이 그 버전 고정 Rust/serde 구현이다. 어느 한쪽을 바꾸면 다른 쪽과 예제,
계약 테스트를 **같은 커밋에서** 함께 바꿔야 한다.

`schemas/edit-ops-v1.schema.json`을 컴파일하는 소비자는
`schemas/document-spec-v1.schema.json`도 읽고 `$id`인
`https://hwp-cli.staix.net/schemas/document-spec-v1.schema.json`으로 등록해야 한다.
EditOps는 이 리소스의 블록·스타일·목록 정의를 참조한다. `insert_blocks`가 없는 배치도
스키마 컴파일 시 참조 해석이 필요할 수 있으므로 두 스키마를 같은 번들에서 함께 읽는다.
CLI는 두 리소스를 내장·등록하므로 검증에 네트워크 요청이 필요하지 않다. 삽입은 대상 문서의
헤더만 복사한 문서에 대해 블록을 컴파일하므로 연산마다 구역과 바이너리 스트림을 복제하지 않는다.
새 이미지 스트림은 컴파일 뒤 이름으로 이식한다. 같은 스트림은 공유하고, 같은 이름에 바이트가
다른 스트림은 거부한다.

보고와 전송. 편집 보고서는 삽입된 세그먼트를 나열한다. 삽입된 최상위 문단 안에서
`ahwp cat --segments v2`가 발행하는 세그먼트마다 생성 쌍(`before`는 null) 하나를, 해당 연산 직후의
문서에서 읽어 담는다. 따라서 id는 새로 실행한 `cat`의 값과 같고 런 id는 `set_format`이 바로 받는다.
`edit-report-v1`은 `changed`를 10,000개로 제한하는데 유효한 요청도 그보다 많이 발행할 수 있다
(100 x 50 표 하나가 15,002개). 그래서 보고서는 문서 순서로 앞 10,000쌍만 담고 실제 개수를
선택 필드 `changed_total`에 적는다. 이 필드가 없으면 목록이 완전하다. 요청 하나가 블록 20,000개를
가질 수 있으므로 더 거친 보고서로도 상한을 보장할 수 없다.
`hwp_edit`는 이 연산을 `insert_blocks` 배열로 받는다(항목은 `op` 태그를 뺀 edit-ops 객체이며
ops 채널의 항목 타입이 그대로 역직렬화하므로 CLI와 MCP가 파서 하나를 공유한다). 이미지 경로는
`base_dir` 인수 기준으로 해석되며 이 인수도 샌드박스 읽기 경로 검사를 받고, `--root` 루트가 모든
자산 읽기를 묶는다. `base_dir`가 없으면 서버 작업 디렉터리에서 읽지 않고 이미지 블록을 거부한다.

계약은 의도적으로 닫혀 있다.

- 루트 `version`은 정확히 `"1.0"`이다.
- 모든 객체는 모르는 속성을 거부한다.
- 모든 union은 `type`으로 명시 태깅한다.
- 잘못된 참조, 미지원 값, 유한하지 않은 치수, 접근 불가 자산은 전부 typed 오류다. 경고나 조용한
  대체가 아니다.
- JSON과 YAML은 같은 데이터 모델로 매핑된다.
- 객체 map은 키 순서가 결정적이며, 컴파일이 해시 순회 순서에 의존하지 않는다.

## 저작 모델

```text
DocumentSpec
├── metadata
├── page
├── styles: {이름 -> StyleSpec}
├── lists: {이름 -> ListSpec}
└── sections[]
    ├── page override
    ├── header/footer: default, first, odd, even 블록 배열
    ├── page_number
    └── blocks[]
        ├── paragraph -> runs[]
        ├── table -> columns[] + rows[].cells[].blocks[]
        ├── image
        ├── equation
        ├── field
        └── break: page | column | section
```

문단 run은 `text`·`field`·`equation`·`image`·`line_break`다. 텍스트 run은 이름 있는 스타일과 명시적
run 서식을 함께 쓸 수 있으며 명시 값이 이긴다. 문단은 이름 있는 목록과 0부터 시작하는 수준을 참조할
수 있다. 표 셀은 사각형 `col_span`·`row_span`을 지원하고, 덮인 셀은 이후 행·셀에서 생략해야 한다.

순서 있는 목록은 Digit 형식을 요청한다. HWP compose는 기존 NUMBERING 기본 레코드로 보존할
수 없는 marker/start를 거부한다. 지정한 수준마다 기본 marker와 시작값 1이 일치해야 하고,
실제로 참조하는 수준은 그 레코드의 Digit 슬롯(0부터 세어 0·2·4)이어야 한다. 네이티브 기본
번호 단계는 `1. 가. 1) 가) (1) (가) ①`이며, 일곱 수준 모두가 일반 숫자는 아니다. HWP
`insert_blocks`와 같은 검사를 공유하고 compose의 표 셀·머리말·꼬리말 블록도 검사한다.
HWP 기본 레코드로 보존할 수 없는 사용자 번호에는 HWPX를 사용한다.

변환은 계속 허용한다. 원시 NUMBERING 레코드가 없는 모델 정의가 기본 단계로 바뀌면 CLI
stderr와 MCP `ConvertReport.warnings`에 `numbering_default_substitution`을 기록한다.
경고에는 정의·수준 ID와 바뀌는 template/format/start 분류를 넣고, 비공개 marker 텍스트는
내보내지 않는다. 원시 NUMBERING 레코드가 있는 정의나 실제 기본값과 일치하는 모델 수준에는
이 경고가 없다. 기본 레코드의 7개 슬롯을 넘는 수준은 번호 문단이 해당 정의·수준을 실제로
참조할 때만 경고한다. 사용하지 않는 HWPX·Markdown 확장 수준에는 경고하지 않으며, 7개
슬롯 안의 차이는 사용 여부와 관계없이 계속 경고한다. 변환 성공이 요청한 번호 marker의
보존을 뜻하지는 않는다. compose의 HWP
검증은 일반 기본값 투영 전에 사용된 수준을 별도로 비교하므로, 작성 단계의 검사가 누락돼도
바뀐 template/start나 알려진 기본 format selector가 조용히 통과하지 않는다.

모든 물리 치수는 십진 밀리미터(`*_mm`), 글꼴·문단 간격은 포인트(`*_pt`), 줄 높이는 백분율, 색은
`#RRGGBB`, 목록 수준은 사용처에서 0부터, 표 열은 왼쪽에서 오른쪽 순으로 선언한다. 문자열 제한은
UTF-8 바이트가 아니라 유니코드 스칼라 수를 세며 JSON Schema `maxLength`와 일치한다. 구역 `id`는 진단용
논리 키로 유일해야 하며, HWP/HWPX에 대응 필드가 없으므로 문서에 직렬화되지 않는다. 이미지 자산은
PNG·JPEG·GIF·BMP만 허용한다. `height_mm`을 생략하면 컴파일러가 픽셀 치수를 읽어 고유 종횡비를
유지하며, 잘못됐거나 크기가 0인 헤더는 실패한다.

## 네이티브 우선 실행 계약

`ahwp compose SPEC -o OUTPUT`은 CLI와 MCP가 같은 파이프라인을 따른다.

1. JSON/YAML을 bound 안에서 파싱한다.
2. 닫힌 v1 계약과 모든 상호 참조를 검증한다.
3. 스타일과 자산을 spec 파일 기준으로 해소한다.
4. 결정론적 `hwp_model::Document`로 컴파일한다.
5. dry-run이면 쓰지 않고 계획·리포트를 반환한다.
6. 아니면 공용 원자적 publisher로 staging한다.
7. writer의 `DROP:` 경고는 전부 거부한다.
8. staged HWP/HWPX를 다시 열어 의미 서명을 비교한다.
9. 검증에 성공한 뒤에만 게시한다.

`deterministic=true`는 같은 spec·자산 바이트·대상 포맷에 대해 출력 경로나 실행 시각이 달라도
바이트 재현이 된다는 뜻이다. 리포트의 `output` 문자열은 호출자가 요청한 경로를 반영할 뿐 문서
바이트의 일부가 아니다.

네이티브 컴파일이 기본이며 리포트는 항상 `native=true`다. 네이티브로 지원되지 않는 요청은 안정적인
issue 코드·경로와 함께 실패한다. Visual fallback은 암묵적 복구가 아니라 정책이다. 호출자가
`--allow-visual-fallback`(또는 MCP 불리언)을 주지 않으면 비활성이고, 실제 fallback은 전부
`visual_fallback_used`에 나열해야 한다. v1에는 자동으로 fallback하는 요청이 없으므로, 켠다고 해서
미지원 네이티브 기능이 성공으로 바뀌지 않는다.

v1의 네이티브 교집합은 첫 쪽 전용 머리말/꼬리말, 십진이 아닌 쪽 번호, 좌우가 다른 쪽 번호 장식 문자,
`keep_with_next`, 비어 있지 않은 이미지 `alt`에 대해 의도적으로 fail-closed다. 마지막 항목은 앞으로의
호환을 위해 스키마에 남아 있으나 절대 조용히 버려지지 않는다. 지원하려면 양쪽 writer가 구현하는
Picture description 모델이 있어야 한다.

## 제한

구현은 spec 4 MiB, 구역 64, 블록 20,000, run 100,000, 표 격자 슬롯 100,000, 중첩 블록 16단계,
텍스트 스칼라 2,000,000, 자산 하나당 64 MiB, 고유 자산 총합 128 MiB를 적용한다. 이름은 유니코드
스칼라 128, 짧은 문자열 4,096, 설명 32,768, 수식 스크립트 100,000까지다. 쪽·치수 범위는 할당이나
게시 전에 검사한다. 위반은 전부 typed 검증 issue로 반환한다.

## 호환성

v1 reader가 거부할 필드를 추가하는 변경은 새 명시 스키마 버전을 요구한다. 기존 v1 의미는 절대 바뀌지
않는다. 이후 버전은 별도 Rust variant와 스키마 파일이며, `"1.0"`을 관대한 파싱으로 재사용하지 않는다.
