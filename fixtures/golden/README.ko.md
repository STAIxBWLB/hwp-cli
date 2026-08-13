[한국어](README.ko.md) · [English](README.md)

# 골든(기준) 렌더 이미지 — 한글 대조용

`hwp diff`/`golden` 테스트가 우리 렌더를 **한글이 내보낸 기준 이미지**와 비교해
오차를 측정한다. 이 디렉터리에 페이지별 기준 PNG를 둔다(이미지는 gitignore — 레시피만 커밋).

## 기준 이미지 만드는 법 (한글)

1. 대상 문서를 한글에서 연다.
2. **파일 → 인쇄 → PDF로 저장** (또는 **파일 → 다른 이름으로 저장 → PDF**).
3. PDF를 고정 DPI로 PNG화한다. 권장 **150 DPI**(글자 식별 용이):
   ```sh
   # macOS: sips 또는 pdftoppm(brew install poppler)
   pdftoppm -png -r 150 문서.pdf 문서          # 문서-1.png, 문서-2.png ...
   ```
   - 한글의 "그림으로 저장"을 써도 되지만 DPI/배율을 반드시 고정할 것.
4. 파일명을 `<fixture이름>.p<페이지>.ref.png`로 둔다. 예: `work_report.p1.ref.png`.

## 우리 렌더와 비교

같은 DPI로 렌더해 비교한다(치수가 같아야 함):
```sh
HWP_FONT_DIR=$PWD/fonts \
  ./target/release/hwp diff fixtures/hwp5/work_report.hwp \
  --ref fixtures/golden/work_report.p1.ref.png --page 1 --dpi 150 -o /tmp/diff.png
```
출력: `bad_pixel_pct`(픽셀 차이율)·`MAE`·`dx/dy`(위치 오프셋) + 차이 이미지
(빨강=우리만, 파랑=기준만, 회색=일치).

## 폰트 고정

한글과 같은 글자 폭/줄바꿈을 얻으려면 같은 폰트가 필요하다. 함초롬바탕/돋움은
`fonts/`(gitignore)에 두고 `HWP_FONT_DIR`로 가리킨다. annual_report 등은 나눔고딕/명조도
필요할 수 있다(없으면 함초롬으로 대체되어 글리프 모양 오차가 커진다 — 위치 오차와는 분리되어
`dx/dy`로 측정된다).

## 골든 테스트

`HWP_GOLDEN=1 cargo test -p hwp-render golden`로 이 디렉터리의 `*.ref.png`를 자동 대조한다
(이미지가 없으면 통과/스킵). 단계별로 임계를 조여 회귀를 막는다. 폰트 없는 CI에서는
기본적으로 건너뛴다(`tests/render.rs`의 구조 스모크는 상시 실행).

## PDF 동등성 기준 (issue #79)

배치 러너 `scripts/pdf-parity.sh`가 우리 PDF와 한글 기준 PDF를
[docs/design/21-pdf-parity.md](../../docs/design/21-pdf-parity.md) §3의 다섯 지표
(`pdffonts`, `pdfinfo`, 쪽별 `pdftotext -layout`, 같은 `pdftoppm -png -r 150` 래스터의
`dx/dy`·`bad_pixel_pct`/`MAE`)로 채점한다.

케이스별 기준 만드는 순서 (소유자, Windows 한컴오피스 2024):

1. 소스 문서를 직접 작성·비식별화해 `fixtures/pdf-parity/public/source/`에 커밋한다
   (HWP/HWPX만 — 커밋 가능한 유일한 산출물).
2. 한글에서 **파일 → PDF로 저장하기**(기본 설정)로 낸 뒤, 정확한 한글 빌드·Windows
   버전·PDF 설정을 `fixtures/pdf-parity/public/manifest.json`(`pins`)에 적고, 고정 폰트
   (함초롬바탕/돋움, `fonts/`)의 SHA-256도 함께 기록한다. 고정 폰트가 저장소의 `fonts/`
   디렉터리에 없으면 `HWP_FONT_DIR`을 지정한다.
3. 낸 PDF는 로컬에만 둔다 — `$HWP_PDF_PARITY_ORACLE_DIR` 아래(커밋 금지, oracle 트리는
   전부 gitignore).
4. manifest에 케이스를 추가한다:
   `{name, source, source_sha256, oracle, oracle_sha256}`.
5. 실행:

   ```sh
   scripts/pdf-parity.sh run --oracle-dir "$HWP_PDF_PARITY_ORACLE_DIR"
   ```

   점수판(`public/scoreboard/<case>.json`, `scoreboard.json`, `scoreboard.csv`)은 이름·
   SHA-256·수치뿐이며(경로·기준 바이트 없음) 커밋되는 유일한 산출물이다. 렌더 전에 닫힌
   manifest 스키마, Poppler 버전, 고정 폰트 파일, 모든 source/oracle SHA-256을 검증한다.
   글꼴 커버리지를 확인할 수 없거나, 폰트 대체·쪽수 차이·PDF의 임베드/서브셋/Unicode
   계약 위반이 있으면 해당 케이스를 `"scored": false`로 표시한다.

`scripts/pdf-parity.sh selftest`는 하네스 자기 검증(fixture를 자기 PDF와 비교하면 모든
지표가 완벽해야 함)으로 한글 기준 없이 돌릴 수 있다.
