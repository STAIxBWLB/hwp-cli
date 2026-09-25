[한국어](README.ko.md) · [English](README.md)

# 테스트 픽스처

> **이 디렉터리의 `hwp5/*.hwp`·`hwpx/*.hwpx` 문서는 저장소에 동봉하지 않는다**
> (`.gitignore`로 제외 — 로컬 전용). 아래 출처에서 받아 같은 경로에 두면 렌더/PDF 테스트가
> 동작하고, 없으면 해당 테스트는 자동으로 **skip**된다. 이 README와 `golden/README.md`만 커밋한다.
>
> skip된 테스트도 `ok`로 보고되므로 `scripts/check.sh`가 요약 줄에 그 수를 센다
> (`skipped-for-missing-fixtures=N (optional=M)`, 목록은 `target/fixture-skips.log`).
> `HWP_REQUIRE_FIXTURES=1`이면 아래 선택 세트를 뺀 skip이 모두 실패가 된다(#275).
>
> **예외: `samples/`는 커밋한다** — 저장소 소유자 자신의 문서를 익명화한 테스트 샘플로,
> 테스트가 하드 의존한다(skip 없음).

## samples/ (커밋)

- `report-tables.hwpx`: 표 편집(행/열 추가·복합 표 거부)과 패키지 보존 치환의 기능
  테스트 픽스처(표 10개: 톱레벨 병합 3종 + 중첩 단순 6종 + [별표] 단순 7x2).
  - 출처: 저장소 소유자 자작 문서를 소유자가 한글(한컴오피스)에서 직접 공개용으로
    정리·가명화(제주 지역 대학명→한빛대학교·미륵대학교·다온대 등)한 뒤 저장한 파일을
    **바이트 그대로** 커밋한다. 한컴이 저장한 원본 바이트라 줄 배치 캐시(linesegarray)가
    내용과 정합하고, 도구로 재가공하면 한글이 "손상/변조" 경고를 띄울 수 있으므로
    재압축·재작성 없이 그대로 쓴다.
  - `[별표 1] 전문가 등급 기준 <개정 2025.08.14.>`는 공개 규정 성격의 내용이라
    원문 그대로 포함(소유자 승인). content.hpf의 lastsaveby(`yj.lee`)는 리포 커밋
    작성자와 동일한 공개 정보라 유지.
  - 갱신 절차: 소유자가 한글에서 수편집·저장 → 실명 키워드 잔존 검사(0건) →
    `hwp validate` 통과 확인 → 파일 교체 커밋.

## hwp5/

[hahnlee/hwp-rs](https://github.com/hahnlee/hwp-rs) (Apache-2.0)의 통합 테스트
픽스처에서 가져옴:

- `hello_world.hwp`, `bookmark.hwp`, `color_fill.hwp`, `outline.hwp` — 기능별 최소 파일
  (hwp-rs `integration/project/files`)
- `annual_report.hwp`, `work_report.hwp` — 실문서에 가까운 샘플
  (hwp-rs `integration/naver_documents/files`, 원출처 Naver 무료 문서 템플릿)

`annual_report.hwp`에는 템플릿에 포함된 장식용 이미지(BinData JPG/PNG), `work_report.hwp`에는
작은 비트맵(117×17 BMP)이 임베드되어 있다. 본문은 자리표시자("OOOOO", "상세 내용을 입력하세요")
뿐인 빈 템플릿으로 실제 개인정보·조직 식별 정보는 없다. 모든 픽스처는 위 Apache-2.0 저장소에서
재배포된 것을 가져왔다(루트 `NOTICE` 참고).

## hwpx/

- `minimal.hwpx` — hwpx MCP 서버로 생성한 최소 문서 (한/영/숫자 혼합 3문단)

## 현재 보유하지 않은 정답지 세트

`crates/hwp-render/tests/render.rs`는 한글로 저장한 문서 4개를 더 전제로 작성되어 있으나, 저장소
소유자가 현재 보유하지 않고 출처도 여기에 기록되어 있지 않다. 이 세트는 **선택(optional)**이다.
없으면 테스트가 skip되고 그 skip은 `optional=M`에 집계되며, `HWP_REQUIRE_FIXTURES=1`도 실패로
만들지 않는다.

- `hwp5/multicol.hwp`, `hwpx/multicol.hwpx` — 2단 본문. 단 사이 쪽 흐름을 검증한다. 단 넘김을
  쪽 넘김으로 오인하면 안 되며(5쪽이 아니라 3쪽), 1쪽에 좌·우 단이 나란히 그려져야 한다.
- `hwp5/equation.hwp`, `hwpx/equation.hwpx` — 실제 한글 수식 스크립트(다행 `#`·분수·첨자·근호·
  그리스 문자). 두 포맷 모두 조판하는지(hwp5는 eqed 파싱, HWPX는 `hp:equation` 캡처), 잉크량이
  비슷한지 검증한다.

## 대형 corpus

수백 개 이상의 야생 문서 소크 테스트는 커밋하지 않고
`HWP_CORPUS_DIR` 환경변수로 외부 디렉터리를 가리켜 실행한다.
