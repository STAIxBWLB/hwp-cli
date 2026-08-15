[한국어](16-certification-v1.ko.md) · [English](16-certification-v1.md)

# Certification v1

`hwp certify INPUT --policy POLICY --report REPORT_DIR`는 버전 고정된 기계 판독 문서 인증 계약을
제공한다. CLI와 MCP(`hwp_certify`)는 같은 구현을 호출한다.

## 신뢰 경계

- 입력, 정책, 폰트, 런타임 실행 파일, 확장은 사용 전에 비공개 불변 스냅샷으로 복사한다.
- HWPX ZIP/XML과 모든 HWP5 CFB 스트림은 의미 파싱 전에 bound를 적용한다. HWP5 제한은 스트림 수·
  이름 바이트, 스트림별 저장·구체화 바이트, 총 구체화 바이트, 레코드 수·깊이를 덮는다. DocInfo,
  BodyText, Scripts, 압축 BinData는 반복 import와 기능 검사에서 하나의 bounded 압축 해제 스냅샷을
  재사용한다.
- DefaultJScript 매크로 검사는 길이 접두 UTF-16LE 블록과 빈 sentinel을 파싱한다. 불투명하거나 형식이
  어긋난 스크립트 데이터는 매크로가 있다·없다는 증거가 아니라 `inspection_incomplete`다.
- 레이아웃, 쪽, 표시 항목, 이미지 디코드, 래스터, 로그, 산출물 작업에 각각 별도 제한이 있다. 독립적인
  쪽 수 계산 pass와 선택 쪽 렌더 pass는 같은 총 쪽 수를 보고해야 한다. 어긋나면 typed 치명 결과이며
  쪽 산출물을 게시하지 않는다.
- typed render issue는 닫힌 code·severity·stage 튜플을 쓴다. 상세는 bounded SHA-256 표본으로만
  표현하며 문서 텍스트와 경로는 남기지 않는다.
- 폰트 해소 진단은 resolver 원천에서 서로 다른 결과 512개로 제한한다. 513번째 결과는 치명 typed
  `font_resolution_budget_exceeded` issue를 내고 해소를 incomplete로 표시하므로, 스키마 크기에 맞춘
  리포트가 조용히 통과할 수 없다.
- 리포트 디렉터리는 미리 존재하면 안 되며, 고정 산출물 트리와 manifest를 감사한 뒤에만 원자적
  no-replace rename으로 게시한다.
- 쪽 PNG는 비공개 렌더 하위 트랜잭션에 쓰고, 선택된 모든 쪽이 통과한 뒤에만 병합한다.

## 네이티브 결과 범위

`scope=native_only`는 패키지, 반복 의미 import, 정책 규칙, 선택된 네이티브 렌더 쪽이 통과했다는
뜻이다. `not_detected` 진단은 알고리즘 범위에 한정된다. 네이티브 성공도, 선택적 오라클도 한컴오피스와의
픽셀 동등성을 주장하지 않는다.

## 독립 import 오라클

선택·필수 오라클은 관리자가 신뢰 환경 pin을 전부 갖추기 전까지 의도적으로 사용 불가다.

- `HWP_CERTIFY_ORACLE_RUNTIME`: Docker 호환 클라이언트 실행 파일
- `HWP_CERTIFY_ORACLE_EXTENSION`: H2Orestart OXT
- `HWP_CERTIFY_ORACLE_IMAGE`: 불변 `repository@sha256:...` 참조
- `HWP_CERTIFY_ORACLE_DOCKER_CLIENT_VERSION`
- `HWP_CERTIFY_ORACLE_DOCKER_SERVER_VERSION`
- `HWP_CERTIFY_ORACLE_IMAGE_ID`: 관측된 `sha256:...` 이미지 ID

정책은 런타임 실행 파일, LibreOffice 실행 파일, 확장, 이미지 digest를 고정한다. 신뢰 환경은 관측된
Docker 클라이언트·서버와 이미지 ID를 추가로 고정한다. 그 pin에 비추어 관측할 수 없는 데몬은
`host_daemon_unattested`가 되며 필수 모드는 통과할 수 없다.

리포트는 관측된 클라이언트·서버 버전과 전체 이미지 참조의 SHA-256만 노출한다. 배포처의 registry·
repository 이름과 신뢰 환경 값은 절대 게시하지 않으며, 내용 주소 기반 이미지 ID만 `sha256:...`로
보인다.

컨테이너는 오프라인·읽기 전용·capability 없음·자원 제한으로 돈다. `/output`은 크기 제한 tmpfs다.
`/output/oracle-result.json`과 `/output/import.pdf`만 비공개 격리 구역으로 복사하고, 그 뒤 컨테이너
제거를 재시도한 다음 not-found 확인 검사로 검증해야 산출물을 게시할 수 있다.

이 저장소는 지원되는 공개 오라클 이미지를 배포하지 않으므로 전체 오라클 프로파일은 기본 제공되지
않는다. LibreOffice 26.2.5와 H2Orestart 0.7.12가 참조 구성 요소 버전이며, 공식 H2Orestart v0.7.12
`H2Orestart.oxt` 릴리스 자산 digest는
`7b5f6f247ed9213776f28a86f3c84d50c94e6d99751c20e2d62bb59e59a76566`다. 정확한 Docker 런타임·이미지와
LibreOffice 실행 파일 해시는 배포처마다 다르며 지어내면 안 된다.

`oracle/primary-artifacts.lock.json`은 공식 LibreOffice 26.2.5 x86_64 DEB 아카이브
(`2f03bfb2...c1bed1e`), 그 서명 URL, H2Orestart 릴리스·태그·라이선스 근거를 기록한다. 이미지 digest와
Dockerfile은 의도적으로 없다. 베이스 이미지와 LibreOffice 런타임 의존성 폐포가 고정돼 있지 않고,
빌드된 이미지·러너 증명도 없다. OXT 아카이브에는 GPL `COPYING` 파일도 빠져 있어 앞으로 재배포하려면
명시적인 라이선스·대응 소스 처리가 필요하다. 이 공백이 메워지기 전까지 필수 오라클 모드는 부분
구현으로 남는다.

## 선택적 증거 검사

문서 정책은 내용 없는(content-free) 선택적 증거 산출물 두 가지를 고정할 수 있다. 각각 정책
파일 기준 상대 경로에서 64 KiB bounded read로 읽고, 닫힌 계약으로 파싱하며, 없거나 유효하지
않으면 닫힌 실패(fail closed)를 반환한다. 섹션이 없으면 리포트는 기존 형태와 정확히 같고,
섹션이 실패하면 `overall=failed`(및 미실행 오라클)가 된다. 리포트 스키마는 이를 전용
`localPassed + evidenceFailed` 분기로 표현한다.

- `document.preservation`: `preservation-report-v1` 산출물(예: `hwp convert --loss-report`
  결과)을 읽는다. 집계 손실 합계(event count의 총합)가 `max_loss_codes`(기본 0) 이하이면
  통과한다. 리포트는 코드별 집계 수만 `checks.preservation`으로 반영한다. 손실 초과 실패는
  `preservation_loss_detected`, 산출물 누락·무효는 `preservation_report_invalid`를 쓴다.
- `document.hancom_open`: 한컴오피스 애플리케이션이 복구·손상 경고 없이 문서를 열었다는
  `hancom-verification-receipt-v1` 산출물을 읽는다. `require_pass`(기본 참)이면 receipt
  결과가 `pass`여야 한다. 리포트는 receipt의 `application`, `verified_at`, `verifier`만
  `checks.hancom_open`으로 반영한다. pass가 아닌 receipt는 `hancom_open_not_attested`,
  누락·무효 receipt는 `hancom_open_receipt_invalid`를 쓰며 아무 필드도 반영하지 않는다.

두 검사 모두 한컴 렌더링 동등성을 주장하지 않는다. 이름 붙인 외부 증거만 입증할 뿐이다.

## 스키마와 소비자

- `schemas/certification-policy-v1.schema.json`
- `schemas/certification-report-v1.schema.json`
- `schemas/certification-oracle-result-v1.schema.json`
- `schemas/preservation-report-v1.schema.json` (선택적 `preservation` 증거 입력)
- `schemas/hancom-verification-receipt-v1.schema.json` (선택적 `hancom_open` 증거 입력)

Maru 같은 소비자는 리포트 스키마를 검증한 뒤, JSON Schema로 표현할 수 없는 런타임 불변식을 직접
확인해야 한다. typed issue 수·해시 재계산, 선택 쪽 진단과 쪽 산출물의 정확한 일치, 산출물 경로의
유일성, 그리고 `oracle/import.pdf`는 통과한 `native_plus_independent_import` 결과에만 존재한다는 점이
그것이다. 개수 제한은 다음과 같다. `report.artifacts`는 최대 257개(쪽 PNG 256개 + 오라클 PDF 1개),
`manifest.files`는 `report.json`이 더해져 최대 258개, 게시된 트리는 `manifest.json`이 더해져 정확히
259개가 상한이다. 내부 publisher의 260 가드는 구현 여유분이며 소비자 기준이 아니다.
