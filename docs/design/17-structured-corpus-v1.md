[한국어](17-structured-corpus-v1.md) · [English](17-structured-corpus-v1.en.md)

# 구조 코퍼스 v1

## 목적

이 코퍼스는 대표적인 한국어 구조 저작에 대한, 저장소에 커밋된 오픈소스 전용 릴리스 게이트다. 성공
신호로서 기존 진단용 Python 스크립트를 대체한다. 외부 HWP/HWPX 표본을 쓰지 않으며 수동 검사를
요구하지 않는다.

spec·정책·manifest는 커밋한다. OFL 폰트와 그 라이선스·metadata는 커밋하지 않는다.
`scripts/fetch-corpus-fonts.sh`가 manifest의 고정 `source_url`에서 받아 gitignore된
`corpus/structured-v1/fonts/`에 두고 manifest SHA-256으로 각각 검증하므로, 대형 바이너리 없이 폰트
identity는 고정된 채로 유지된다. fetcher는 고정 host의 `https`만, bounded 응답만, 코퍼스 내부 상대
목적지만 허용하며, 러너는 그와 무관하게 읽는 모든 바이트를 다시 검증한다.

## 고정 케이스

| ID | 대표 범주 | 소스 계약 | 출력 |
|---|---|---|---|
| `official-letter` | 한국 공문 | DocumentSpec v1 | HWPX, HWP |
| `approval-memo` | 결재 기안문 | DocumentSpec v1 | HWPX, HWP |
| `report` | 보고서 | DocumentSpec v1 | HWPX, HWP |
| `business-plan` | 사업계획서 | DocumentSpec v1 | HWPX, HWP |
| `meeting-minutes` | 회의록 | DocumentSpec v1 | HWPX, HWP |
| `academic-education` | 대학 교육 문서 | DocumentSpec v1 | HWPX, HWP |
| `print-form` | 인쇄 신청서 | TemplateSpec+Data v1 | HWPX, HWP |

이 라벨은 픽스처의 의도를 기술할 뿐, 해당 범주의 실제 문서가 쓰는 모든 기능을 완전히 지원한다는
뜻이 아니다. 기계 판독 summary도 같은 한계를 반복해 기록한다.

## 게이트 절차

각 포맷에 대해 러너는 다음을 수행한다.

1. 고정 manifest·정책·폰트/라이선스/metadata·소스를 bounded 격리 스냅샷으로 읽는다.
2. 고정된 폰트 파일만 써서 실행 A와 실행 B를 생성한다.
3. 같은 프로세스·플랫폼 안에서 바이트 동일성을 요구한다.
4. 두 파일을 다시 열어 구조를 검증한다.
5. 고정된 필수 한국어 텍스트와 bounded 의미 카운트를 둘 다 검사한다. HWPX·HWP 공통 구조 투영은
   `hwp-corpus-common-semantic-v1`로 도메인 분리되며 digest가 같아야 한다.
6. 고정된 native-only 정책으로 둘 다 인증한다.
7. 두 인증 결과의 쪽 수, 선택 쪽 PNG 해시, typed render issue 해시, 해소된 폰트 identity를 비교한다.
8. 닫힌 summary와 내용 주소 기반 산출물 목록을 원자적으로 게시한다.

전역 기대 PNG 해시는 없다. 래스터 해시는 OS·아키텍처에 따라 다를 수 있으므로, 계약은 플랫폼 프로파일을
기록하고 한 실행 안에서의 쌍 결정성만 검사한다.

## 한계값과 프라이버시

- manifest와 summary: 각각 1 MiB
- 케이스: 정확히 7개, 구현 상한 32
- 산출물 파일: `artifacts.json` 이전 최대 255개, 포함 256개
- 디렉터리 128개, 깊이 8, 상대 경로 512 ASCII 바이트
- 산출물 파일 하나 128 MiB, 트리 전체 512 MiB
- 의미 투영: 노드 100,000개, 길이 프레임 투영 필드 64 MiB
- 심볼릭 링크·reparse point·다중 링크 입력/산출물 금지
- summary에 원문 문서 텍스트·자식 출력·절대 입력 경로·예외 메시지 금지

명령은 모든 케이스와 포맷이 통과할 때만 0을 반환한다. 완료됐으나 실패한 실행도 bounded 리포트를
게시하고 0이 아닌 값을 반환한다. manifest·입력 계약 거부는 아무것도 게시하지 않는다.

## 커버리지 한계

v1 픽스처는 고급 그리기·차트, 수식, 이미지, 주석·인용, 목차·색인, 혼합 쪽 구역, 변경 이력·메모,
암호화·서명·매크로, 접근성 태깅, 독립 오피스 스위트 import를 인증하지 않는다. 그것들은 추가 고정
케이스와 정책이 필요하며, 7개 범주 라벨에서 유추해서는 안 된다.

공통 의미 digest는 bounded·스트리밍·타겟 중립 투영이다. 표현 가능한 metadata와 시각, 구역 쪽 정의,
머리말/꼬리말/쪽 번호 컨트롤, 문단 텍스트·컨트롤 순서, 해소된 문단/스타일/글자 모양과 목록 정의,
표 배치와 셀 기하·내용, 모델링된 필드·책갈피·수식, 그림 치수·설명·내용 해시를 덮는다. 줄 배치 캐시,
불투명 레코드, 포맷 고유 컨트롤 원문 페이로드, 통과 XML, 고급 그리기 기하·스타일은 의도적으로
제외한다. HWP5 reader가 모호하지 않게 해석할 수 없는 취소선·밑줄 모양 상세도 제외하며, 공통 밑줄
존재·종류는 계속 포함된다. 이 제외 목록은 `summary.json`에 닫힌 목록으로 반복 기록된다. digest가 같다는
사실을 프로파일 밖의 바이트 단위 IR 동일성으로 해석해서는 안 된다.
