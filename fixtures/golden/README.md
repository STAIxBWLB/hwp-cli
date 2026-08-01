[한국어](README.md) · [English](README.en.md)

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
