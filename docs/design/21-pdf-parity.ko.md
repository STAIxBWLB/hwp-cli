[한국어](21-pdf-parity.ko.md) · [English](21-pdf-parity.md)

# PDF 동등성 계약 (한컴오피스 2024)

> **상태:** 활성 계약. [issue #79](https://github.com/STAIxBWLB/hwp-cli/issues/79)에서 추적한다.
> 이 문서는 parity 하네스가 읽는 지속 계약이다 — 지표 집합, 임계값, 데이터 정책, 비목표를
> 정의한다. 작업 순서(PR 1–9)는 이슈 본문이 정하고, 이 문서는 "동등"의 뜻을 정의한다.

목표: `hwp convert --to pdf`와 `hwp render --format pdf`가 일반 문서(공문, 보고서, 표, 양식,
이미지, 도형, 수식)에 대해 한국 사무실 사용자가 한컴오피스 2024 한글의 **파일 → PDF로
저장하기** 출력과 상호 교환 가능하다고 받아들이는 PDF를 낸다 — 시각적으로 충실하고 텍스트
선택·검색·복사가 가능해야 한다.

## 1. 범위

**Regime A — 한글이 저장한 문서 → PDF.** HWP 파일은 자체 줄 배치를 PARA_LINE_SEG에 캐시하고
렌더러는 그 캐시를 재생한다(Regime A). 줄 위치는 한글 자신의 것이고, 차이는 잉크와 한정된
구조적 간극뿐이다. 이 계약은 Regime A만 다룬다.

**Regime B — hwp-cli가 생성한 문서 → PDF**(합성 lineseg, 금칙처리, Latin 단어 유지,
widow/orphan, 어울림 텍스트 감싸기)는 별개 이슈로 여기서는 제외한다.

### 1.1 비목표

[release-readiness](../release-readiness.ko.md)와 issue #79에서 인용:

- 릴리스는 스모크 픽스처가 모든 실제 문서 형태를 커버한다거나, 한컴 픽셀 동등성을
  제공한다거나, 플랫폼 간 동일 래스터 바이트를 증명한다고 주장하지 않는다.
- 책갈피(`/Outlines`)와 클릭 가능한 링크(`/Annots`): 한컴 PDF는 둘 다 내지 않으므로 동등성을
  **넘어서는** 기능이다.
- PDF/A-2b와 tagged PDF: 제외. tagged PDF는 의도적으로 도메인 없는 DisplayList 때문에
  구조적으로 막혀 있다.
- 릴리스 카피에 "한컴 픽셀 동등성" 주장 금지. README의 "No Hancom parity claim" 문구는
  일반문서 프로파일 전체가 §4의 모든 게이트를 통과한 후에만 제한적인 "validated Hancom
  Office 2024 PDF profile"로 바꿀 수 있다.
- 차트, OLE, 동영상, 글맵시, 메모, 마스터 페이지, 세로쓰기, 고급 수식 구조는 명시적 누락으로
  보고하며 조용히 누락하지 않는다.

## 2. 오라클

현재 공개 게이트의 정답지는 의도적으로 범위를 제한한 하나의 오라클이다. 소유자가 작성하고
익명화한 한 쪽 원본 문서를 Mac 한컴 HWP로 저장한 PDF다. HWP 12.30.0(빌드 6446)을 macOS
26.6.1(빌드 25G76)에서 사용했으며, 기본 파일 → PDF로 저장하기 경로를 적용했다. 생성 PDF의
Producer는 `macOS 버전 26.6.1(빌드 25G76) Quartz PDFContext`이고, A4, 정확히 1쪽이다. 이
출처 정보는 해당 픽스처에만 적용되는 기준이며 Windows용 한컴, 모든 한컴 빌드 또는 보편적인
플랫폼 간 동등성을 주장하지 않는다.

커밋된 원본/오라클과 Linux 후보 렌더에 사용하는 정확한 OFL 글꼴 바이트를 다음과 같이 고정한다.
글꼴 파일은 `scripts/fetch-pdf-parity-fonts.sh`로 받아 두며 커밋하지 않는다:

| 아티팩트 | SHA-256 |
|---|---|
| `public-safety-rfp-p1.hwp` | `8c4e62fb8166828eaddd2d0d304732acd88484ba8526692545b316985e0c0aba` |
| `public-safety-rfp-p1.hwpx` | `a5b6bb59bc4492f81deeada58cd4b6c0a13579d06afe39e0aaec2687a3eaaf5c` |
| `public-safety-rfp-p1.pdf` | `8fe8a4a4f3f6640248a1efde26421c7134374b4203363acca1a5967b2c0602e7` |
| `Noto Sans CJK KR 2.004 Regular` | `6bcb2a0703aa137e874fc2dffa85f6c21ba9a67fa329e81b8c801663af7e992a` |
| `Noto Sans CJK KR 2.004 Bold` | `26d0c6748500a0444844280b308f5b62c7ae92ac6c6ac88148e502dd211eb52a` |

매니페스트는 공개 프로파일을 `hancom-hwp12-macos-12.30.0-build-6446-quartz`로, 기대하는
Poppler 바이너리를 `pdfinfo version 24.02.0`으로 기록한다. Quartz Producer 문자열은 출처
정보일 뿐이며, 이 픽스처에서는 PDF 메타데이터를 동등성 지표로 사용하지 않는다.

렌더러의 문서 수준 출력 계약은 다음 6개 catalog/Info 기능을 유지한다. 한 쪽 Quartz 오라클이
동일한 메타데이터를 포함한다는 주장은 아니다:

- `/Lang (ko-KR)`, `/PageLayout /SinglePage`, XMP `/Metadata`,
  `/OutputIntents` (GTS_PDFA1 + sRGB IEC61966-2.1, 임베디드 ICC), `/MarkInfo <</Marked false>>`
- `/Info`는 Author, Creator, Producer, CreationDate, ModDate, PDFVersion뿐
- 글꼴: Type0 + CIDFontType2 + FontFile2 + Identity-H + ToUnicode (hwp-render가 이미 구현한
  방식)
- 없음: `/Outlines`, `/Annots`, `/PageLabels`, `/AcroForm`, `/Encrypt`, `/Shading`,
  `/Pattern`, `/ExtGState`, `/SMask` (한컴도 그라데이션을 평면화 — 우리의 밴드 근사는
  결함이 아니다)
- PDF 1.4; tagged PDF 아님

**상태 (2026-08-13, PR 3):** 6개 catalog/Info 기능을 모두 낸다. `/Lang (ko-KR)`,
`/PageLayout /SinglePage`, `/MarkInfo <</Marked false>>`, 최소 XMP `/Metadata` 패킷
(dc/pdf/xmp), `/OutputIntents`(GTS_PDFA1 + 임베디드 ICC), 그리고 `/Info`는 6키 한정 —
Author는 문서에 있을 때만, Creator/Producer는 `hwp-cli <버전>`, CreationDate/ModDate는 문서
FILETIME 메타데이터 변환(현재 시각 사용 금지 — 2회 실행 바이트 동일 게이트 유지), `PDFVersion`
쌍. 헤더 버전은 PDF 1.4. 임베디드 프로파일은 ICC Registry의 `sRGB2014` v2 프로파일이며
`crates/hwp-render/assets/sRGB2014.icc.hex`로 커밋한다. 출처와 재배포 조건은 인접한
`LICENSE-sRGB2014.txt`에 기록하고, 디코딩한 3,024바이트 프로파일의 SHA-256
`384b832de3412066743b52a75ee906b6fb9fb8d9e09e936fc2c43223815c6e0a`를 테스트로 고정한다.
이 필드들은 확인한 구조 계약을 구현한 상태이며, 공개 한 쪽 오라클 게이트는 CI에서 실행한다.
더 넓은 한컴 코퍼스는 후속 작업 범위다.

## 3. 다섯 지표 집합 (우선순위 순)

양쪽 모두 벡터 텍스트이므로 픽셀 차이 지표는 글꼴 대체와 안티앨리어싱에 지배된다 — 엔진
아티팩트이지 충실도가 아니다. 결정적 지표는 픽셀이 필요 없다:

| # | 지표 | 도구 | 잡아내는 것 |
|---|---|---|---|
| 1 | 임베디드 글꼴 목록 | `pdffonts` | 글꼴 대체 (§5) |
| 2 | 페이지 수 차이 | `pdfinfo` | 표 분할, 넘침, 장식 회귀 |
| 3 | 페이지별 추출 텍스트 일치 | `pdftotext -layout`, 정규화 | 누락 콘텐츠, 잘못된 페이지네이션, 깨진 ToUnicode |
| 4 | 잉크 bbox 차이 (`dx`, `dy`) | 150 DPI에서 한 번 래스터 | 체계적 여백/베이스라인 오프셋 |
| 5 | `bad_pixel_pct` / `MAE` | `hwp diff` | 타이브레이커 전용 |

지표 4–5의 래스터화는 Hancom PDF와 자체 PDF 모두 같은 고정 Poppler(`pdftoppm -png -r 150`)를
쓴다.

**구현 상태 2026-08-14 (PR 4):** 배치 러너가 있다 — `scripts/pdf-parity.sh run`이 manifest의
모든 케이스를 채점해 커밋 가능한 수치 점수판(`fixtures/pdf-parity/public/scoreboard/`,
스키마 검증, 이름+SHA-256+수치뿐)을 만들고, `selftest`는 기준 없이 하네스를 검증하며,
`hwp diff --format json` / `--ours-png`가 쪽별 래스터 지표 원천이다. manifest, Poppler
버전, 폰트 파일, source/oracle digest가 pin과 일치해야 채점하며, 검증된 산출물 집합만
rollback 보호 방식으로 게시한다. 공개 코퍼스에는 소유자가 작성하고 익명화한 한 쪽 오라클이
있으며, 고정 `ubuntu-24.04` CI 작업이 OFL 글꼴을 받고 `hwp`를 빌드한 뒤 HWP/HWPX 두 입력을
대조한다. 한 건의 회귀 게이트이며 보편적인 동등성 인증은 아니다.

**구현 상태 2026-08-13 (PR 5):** 테두리 충실도 배치 반영(GG-5, GG-6, GG-17, GG-21, GG-24).
셀·문단·쪽·대각선 테두리가 `hwp-render/src/border.rs`를 통해 `BorderLine.line_type`을
반영한다 — 점선 계열은 `Stroke.dash`, 이중선 계열은 오프셋 병렬 선 — 하고 `Item::Line`은
`Item::Path`로 대체·삭제됐다. HWPX 단 구분선(`hp:colLine`)은 왕복·렌더되며 hwp5 coldef
구분선 파싱은 보류(바이트 오프셋 미확정). hwp5 raw 경로 도형에 점선 패턴·화살촉을 적용
(종전 hwpx 전용). 탭 전진은 `items_width`·`place_wrapped`·`compute_linesegs`가
`tab::next_tab`으로 통일됐다. 이중선 굵기 분할은 근사값으로 한컴 검증 라운드 확인 대상.

**구현 상태 2026-08-13 (PR 6):** 문자 장식 배치 반영(GG-8, GG-9, GG-10, GG-11, GG-22).
강조점(attr bits 21~24, 13종 전부)이 글리프별로 렌더되고 hwpx `symMark`가 왕복한다. 밑줄
모양·'글자 위' 밑줄은 0-기반 장식 표(점선 계열, 이중·가중 오프셋, cubic 물결)를
`border.rs::decor_strokes`로 적용하고, 취소선 모양(bits 26~29)도 같은 표를 쓰며 hwpx에서
왕복한다. 글자 그림자는 세 백엔드 모두 실제 `shadow_gap` 백분율을 쓰고, 글자 단위
테두리/배경(`CharShape.border_fill_id`)은 런 단위로 배경을 글리프보다 먼저 내어 렌더한다.
장식 메트릭(y 오프셋, 물결 상수, 강조점 크기, 글자 상자 범위, (0,0) 그림자 폭백)은 한컴
검증 라운드 확인용 플레이스홀더.

**구현 상태 2026-08-14 (PR 7):** advance 영향 배치(GG-20, GG-3, GG-4, GG-18)와 단일
re-baseline 반영. 인라인 제어문자가 폭을 갖는다(HYPHEN은 `-` 셰이핑, NB_SPACE는 no-break
source를 보존하면서 일반 공백 폭 사용, FW_SPACE는 고정 1em). 정렬은 양쪽/배분/나눔을 구분
(모드별 후행 gap·마지막 줄 규칙, 한컴 확인 대상). 자간은 HWPUNIT 정수 도메인 half-up 반올림. 합성 줄간격은 버전 인식
`line_spacing_type`으로 4모드(비율/고정/여백만/최소)를 모두 지원. `MAX_BAD_PIXEL_PCT`를
0.60→0.30으로 조인 것은 re-baseline 약속이며 첫 커밋 스코어보드(PR 4의 소유자 액션)가
검증·조정한다. GG-20/GG-3/GG-4는 정품 한글 저장 파일의 픽셀도 움직이고 GG-18은 합성 경로
전용.

**구현 상태 2026-08-14 (PR 8):** 이미지·채우기 배치 반영(GG-15, GG-7, GG-23). `Item::Image`가
계약 변경 — 자르기·반전·회전·밝기·대비 — 을 hwp5 그림 레코드와 hwpx `hp:pic` 속성에서
파싱해 세 백엔드가 반영한다(png Transform+선행 자르기, pdf 행렬+clip, svg transform+
clipPath. 픽셀 효과가 없으면 pdf JPEG 고속 경로·svg 제로카피 임베드 유지). 그림 효과(표
108~116)는 파싱 후 `picture_effects_unsupported` 경고로 보고하며 렌더하지 않는다.
셀·문단·글자 배경이 무늬·그러데이션 채우기를 지원(`Fill::Hatch` 신설: png 선분, svg
`<pattern>`, pdf 평탄 선). 변환된 타원은 축 벡터 `ellipse_arc_path`로 호/부채꼴/현을
렌더한다. 한컴 라운드 확인 대상 근사: 밝기/대비 곡선, 무늬 간격/굵기, 호 종류·sweep 매핑,
회전 부호 관례. hwp5 반전 비트는 미확정.

**구현 상태 2026-08-14 (PR 9):** 로드맵 마지막 배치 반영(GB-13, GG-14, GG-16). 캡션 파싱이
전 구간 연결됐다 — Table/Picture/GenericControl에 `Caption` IR(side·direction·gap·width·
last_width·paragraphs)을 두고, hwp5는 pyhwp의 `TableCaption`/`GShapeObjectCaption` 모델대로
캡션 LIST_HEADER를 판별(TABLE 레코드 이전의 LIST_HEADER가 캡션, gso 직속 LIST_HEADER
자식도 동일)해 재합성하며, hwpx `<hp:caption>`은 side/fullSz/width/gap/lastWidth
속성(direction은 `subList@textDirection`)과 함께 왕복한다. 표·그림·일반 도형·미지원 GSO
캡션은 읽기 순서를 보존하고 속성 쪽과 gap에 따라 렌더된다. 미주는 앵커 페이지를 떠난다.
레이아웃이 페이지 노트를 `NoteKind`로 분리해 예약을 각주 전용으로 유지하고, 누적한 미주를
마지막 쪽 각주와 겹치지 않는 구역 끝 블록으로 여러 쪽에 걸쳐 배치한다. 홀짝 furniture는
모든 head/foot 컨트롤의 적용쪽 값(data bits 0-1: BOTH/EVEN/ODD)을 보존하고, 출력 쪽번호와
정확히 일치하는 항목 다음 BOTH만 폴백으로 선택한다. 적용쪽 데이터가 없으면 BOTH로
해석하므로 일반 단일 머리말 구역은 유지되며, 홀수/짝수 전용 항목은 반대쪽에 나타나지 않는다.
한컴 라운드 근사 항목: 캡션 listflags 상위 비트, 스펙 표
71/72 길이 불일치(표 72 준거), 첫쪽 전용 furniture(소스 포맷 표현 부재).

## 4. 게이트

### 4.1 정본 한컴 오라클 게이트 — 현재 공개 한 건 프로파일

이는 커밋된 한 쪽 공개 프로파일에 적용하는 정본 게이트다. CI는 고정
`ubuntu-24.04`에서 배포판 `poppler-utils`를 설치하고, 매니페스트의
`pdfinfo version 24.02.0`과 실제 바이너리를 대조하며, 정확한 OFL 글꼴을 받고 `hwp`를 빌드한
뒤 공개 HWP/HWPX 입력 두 개를 커밋된 오라클과 비교한다. 보편적 또는 Windows 동등성 주장이
아니며, §4.2의 structured-corpus 게이트와 별개다.

- 페이지 수: 완전 일치
- MediaBox 차이 0.5 pt 이하
- `dx`, `dy` 각각 2 px 이하(150 DPI); `ink_ratio` 0.97–1.03
- `bad_pixel_pct` 5 % 이하, MAE 5 이하
- 기능 ROI 잉크 precision/recall 각각 0.95 이상. 한 잉크 픽셀은 150 DPI에서 매니페스트로
  고정한 10 px 정사각 반경(4.8 pt) 안에 상대 잉크가 있을 때 일치로 계산함. 이는 벡터 힌팅과
  누적 글리프 advance 반올림만 흡수하며, 쪽 단위 `dx`/`dy`, 래스터 오차, 텍스트 순서,
  잉크 비율 게이트는 독립 적용하므로 이동·누락·추가 기능을 허용하지 않음.
- 글꼴 대체, 미지원 누락, fatal 렌더 이슈: **0건**
- `pdffonts`: 후보 PDF의 모든 글꼴 embedded/subset + Unicode 지원. Mac producer가 쪽번호
  글꼴을 Unicode cmap 없이 embedded하므로 oracle 글꼴 행은 진단 정보로만 유지함.
- `pdftotext -layout` 정규화 결과가 기대 가시 텍스트·순서와 일치
- 동일 입력·글꼴로 두 번 생성한 PDF가 byte-identical

### 4.2 현재 structured-corpus 자체 일관성 게이트

structured-corpus run(`scripts/check-structured-corpus.sh`)은 고정 Noto Sans KR 아래에서 다음
백엔드 자체 일관성을 현재 강제한다:

- PDF 페이지 수가 PNG 백엔드 페이지 수와 일치
- 표시된 PDF 글리프 전체가 ToUnicode 매핑을 거쳐 완전한 기대 논리 텍스트로 왕복하는지 확인
  (required-text 부분 문자열만 확인하는 검사가 아님)
- 동일 입력·글꼴로 PDF를 두 번 렌더링한 결과가 byte-identical

현재는 PDF와 PNG 출력을 서로 raster 비교하지 않는다. 따라서 `dx`, `dy`, `ink_ratio`,
`bad_pixel_pct`, MAE는 structured-corpus 임계값이 아니며, §4.1의 고정 공개 한컴 오라클
게이트에서 적용한다.

### 4.3 매니페스트 선언 게이트 제외 (v2)

v2 매니페스트는 선택 항목 `gate_exclusions` 배열을 받는다: 8개 v2 게이트(`page_count`,
`media_box`, `text`, `fonts`, `render_issues`, `raster`, `roi`, `determinism`)에서 고른
중복 없는 이름, 최대 8개. 로컬 비공개 프로파일이 특정 게이트를 측정은 하되 차단하지
않는(non-blocking) 항목으로 선언할 때 쓴다 — 예: 승인된 글꼴 face를 측정 호스트에서 아직
쓸 수 없을 때 `fonts`. 알 수 없는 게이트 이름, 중복, 배열이 아닌 값은 어떤 케이스도
측정하기 전에 실행을 실패시킨다.

제외는 적격 판정만 완화하며 측정은 그대로다:

- 모든 게이트를 여전히 측정·보고하며 `passed_gates`/`failed_gates`는 변하지 않는다.
- 케이스는 `blocking_failed_gates`(실패 게이트에서 제외 집합을 뺀 것)가 없을 때 적격이다.
  스코어보드 적격 판정과 러너의 종료 코드도 같은 규칙을 따른다.
- 각 스코어카드와 스코어보드는 `excluded_gates`(정렬)와 `blocking_failed_gates`를 필수
  필드로 에코하므로, 제외에 기댄 통과를 완전 통과로 오인할 수 없다.
- `excluded_gates`가 비어 있으면 스키마가 기존 strict 규칙(적격 ⟺ 실패 게이트 없음 +
  8개 게이트 모두 통과)을 그대로 강제하므로, 제외를 선언하지 않는 공개 CI 프로파일은
  이전과 동일하게 검증된다.

**상태 (2026-08-16, epic #90 PR 8b, 진행 중):** 계약과 러너 구현 완료. 공개 매니페스트는
제외를 선언하지 않는다. 비공개 프로파일이 사용한 제외는 릴리스 노트에 명시하고 사유를
밝혀야 한다([release-readiness](../release-readiness.ko.md) 참조).

### 4.4 비공개 프로파일 잔여 격차 (2026-08-15 실행)

머지된 main(PR 7a/7b 이후)에서 새로 실행한 비공개 복합 케이스는 `page_count`(13 == 13),
`media_box`, `render_issues`, `determinism`을 통과하고 `gate_exclusions: ["fonts"]`를
선언한다. 차단 중인 잔여 게이트는 `text`, `raster`, `roi`이며, 측정 형태는 다음과 같다
(content-free 집계만 기재):

- `text`: 정확히 일치하는 쪽은 1/13이지만, 쪽별 문자 멀티셋은 전체의 99.17%가 일치한다
  (17,093자 중 141자 차이). 한글 음절 차이는 전혀 없고, 차이는 소수의 고정된 기호/ASCII
  문자와 추출 순서 차이(쪽별 시퀀스 유사도 0.63–0.99)뿐이다. 즉 격차는 내용 손실이 아니라
  순서와 일부 기호 매핑이다.
- `raster`: 불량 픽셀 비율 0.145–0.234(임계 0.05)와 MAE 19–28(임계 5)이 13쪽 전반에
  고르게 분포하고 11쪽의 잉크 비율은 ≈ 1.0이다. 내용 누락은 아니다. 두 쪽은 잉크 비율이
  벗어나며(1.42 / 0.86) 해당 쪽의 레이아웃 수준 이동을 시사한다. (8건의 글꼴 대체가 모든
  글리프 형태를 바꾼다는 이전 해석은 §4.5에서 반증됐다.)
- `roi`: 4개 ROI 중 3개 통과. 2쪽 다이어그램 영역만 실패(정밀도 0.849, 재현율 0.887,
  임계 0.95)하며 같은 쪽의 잉크 비율 이탈과 상관한다 — 전역 회귀가 아니라 다이어그램
  쪽의 구조적 레이아웃 차이다.

### 4.5 잔여 격차의 근본 원인 (2026-08-16 진단)

쪽별 오버레이 실행(`hwp diff`로 후보 vs 오라클 래스터 비교, 산출물은 비공개 전용)과
오라클이 임베드한 글꼴 서브셋의 바이트 수준 대조로 §4.4의 가설을 실측 원인으로 교체했다.

- **대체는 결함이 아니라 오라클과 동일한 선택이다.** 오라클이 임베드한 face 중 `ArialMT`를
  제외한 전부가 고정 글꼴 디렉터리에 `fontRevision`·`unitsPerEm`·hhea 메트릭까지 동일하게
  존재한다. 문서의 지배적 본문 face는 `substFont`가 선언돼 있고 오라클 호스트에도 그 face가
  없었으며, 한컴도 같은 선언을 따랐다 — 즉 우리 대체는 오라클이 임베드한 것과 같은 face로
  해석된다. 따라서 `fonts` 게이트의 `substitution_free` 기준은 이 케이스에서 원리적으로
  달성 불가이며, 바뀌어야 하는 것은 렌더가 아니라 기준이다. 없는 원본 글꼴을 설치하면
  오히려 오라클에서 멀어진다.
- **첫 줄 들여쓰기를 텍스트 영역 밖에 적용했다(수정 완료).** 정품 `line_seg`의 `horzpos`는
  문단 좌여백만 담고 첫 줄 들여쓰기는 담지 않는다 — 내어쓰기 문단도 마찬가지다(좌여백이
  있는 문단과 없는 문단 모두에서 확인). 렌더러는 내어쓰기 구간을 줄상자 왼쪽 바깥으로
  보고 있었고, 그 결과 문서 본문 대부분인 목록 문단이 한컴 위치보다 내어쓰기 폭(14–18pt)
  만큼 왼쪽에, 마커는 다시 마커 폭만큼 더 왼쪽에 그려졌다. 내어쓰기 구간은 텍스트 영역
  **안쪽**의 마커 자리다: 마커는 줄상자 왼쪽, 첫 줄 글자는 그 다음, 이후 줄은 내어쓰기
  폭만큼 들여쓴다. 수정 후 모든 쪽의 최좌측 잉크가 오라클과 1px 이내로 맞는다(이전 최대
  29px 차이).
- **이후 수정됨:** 줄 안 advance 이탈(07-hangul-compat-rules B9 — 자간은 글자 자신의
  advance에 곱하고, `useFontSpace` 비트가 꺼지면 빈칸은 고정 1/2 em, `useKerning`이 꺼지면
  커닝 끔), 쪽번호 꼬리말 누락(쪽 컨트롤은 중첩 문단 리스트에도 있고, 번호는 머리말/꼬리말
  밴드에 놓인다), 표 격자(저장된 행 높이는 한글의 레이아웃이므로 우리 측정으로 늘리지 않고,
  조각은 실제 쪽 교차에서만 닫는다).

### 4.6 epic #90 종료 시점 상태 (2026-08-16 실행)

머지된 main에서의 최종 비공개 실행은 `page_count`(13 == 13)·`media_box`·`render_issues`·
`determinism`을 통과한다. `text`·`raster`·`roi`는 `gate_exclusions`에 선언해 측정·보고는
그대로 하되 eligibility를 막지 않으며, `fonts`는 §4.5의 이유로 계속 제외한다. 종료 시점
측정 형태(content-free):

| 게이트 | 측정값 |
|---|---|
| `text` | 13쪽 중 1쪽 완전 일치. 차이는 문자가 아니라 페이지네이션 — 4쪽의 유일한 차이는 쪽 경계를 넘는 줄 하나 |
| `raster` | `bad_pixel_pct` 0.1418–0.2332, MAE 18.5–27.9, 잉크 비율 0.854–1.435, dx/dy 최대 절댓값 40px |
| `roi` | 4개 중 3개 통과. 2쪽 다이어그램 영역만 실패(정밀도 0.848, 재현율 0.892) |

지표는 정렬 없이 계산하므로, 격자가 맞은 뒤에도 남은 쪽별 오프셋이 쪽 전체 수치를 0.05/5
임계에서 멀리 떨어뜨린다. 후속 이슈로 넘기는 잔여 항목: CELL 조각 패킹이 쓸 수 있는 쪽
공간을 약 50pt 남기고(우리 2쪽 조각은 1515px에서 닫는데 오라클은 1620px), 그 때문에 이후
조각들이 밀린다. 벡터 이미지 텍스트가 두 쪽에서 페이지 원점에 놓이고, 이미지
글머리표(`useImage`)가 선언된 문자로 폴백하며, 2쪽 다이어그램이 잉크 비율과 실패하는 ROI를
동시에 만든다.

## 5. 글꼴 게이트 (F1)

한컴은 함초롬바탕/함초롬돋움을 임베드하고, 우리는 fontdb로 해석해 일반 산세리프로 폴백할 수
있다. 대체 한 번이 모든 advance, 모든 줄바꿈 결정, 모든 글리프 형태를 바꾸므로 **대체 글꼴
아래 잰 parity 수치는 무의미하다**.

- `RenderIssueReport::font_coverage()`가 FontMatched / FontSubstituted / FontMissing /
  FontSubsetFallback 횟수를 집계하고, CLI 렌더 리포트는 해시화한 요청/해결 글꼴 identity,
  요청 weight 상태, face index, 해석 완결성도 함께 발행한다.
- `FontCoverage::substitution_free()`는 하드 게이트의 일부다: 대체 없이 렌더되지 않은 케이스의
  parity 수치는 발행할 수 없다. 해석 완결성도 충족해야 하고, 해결된 모든 font-byte 해시는
  manifest에 고정된 집합에 속해야 한다. 직접 요청한 패밀리(선택된 face의 검증된 보조 별칭 포함)만
  matched로 인정한다.
- 커버리지나 identity 증거를 확인할 수 없는 경우도 게이트 실패로 처리한다. 두 PDF 중 하나라도
  `pdffonts` 기준 임베드·서브셋·Unicode 지원을 모두 충족하지 못하면 역시 채점하지 않는다.

## 6. 페이지네이션 정본 (F3)

한글은 이미 페이지 경계를 알려준다: `LineSeg.flags` bit0(페이지 첫 줄)과 bit1(단 첫 줄)이
IR에 파싱돼 있다. 렌더러는 이 비트를 1급 페이지/단 경계 신호로 읽는다. 명시 플래그가 없는
`v_pos` 리셋은 soft boundary로만 보고 CELL fragment의 남은 용량에 채울 수 있으며, 합성 lineseg(flags
`0x0006_0000`)에서는 unflagged reset을 soft boundary로 폴백한다. 표가 페이지를 걸쳐 잘리는 동안은
페이지 인덱스 비교가 무의미하므로, 페이지네이션 정확성(표 분할 포함, PR 2)은 측정의
**전제조건**이지 소비자가 아니다. **구현 상태 2026-08-13: PR #81**은 `Table.attr` pageBreak 정책에 따라 가능한 행 경계에서 표를 나누고 `row_span` 셀을 보존하며 제목 줄 자동 반복을 지원한다. 공개 한 쪽 프로파일은 이제 CI에서 대조하며, 더 넓은 한컴 코퍼스는 남아 있으므로 보편적인 대등성 인증은 아니다.
**PR #89 후속 보완:** `CELL` 정책은 한 행도 기존 cached line 경계에서 이어 그린다. 명시적인
page-reset flag가 없는 경우에도 현재 쪽의 남은 공간에 들어가는 마지막 cached line 다음에서
분할한다. 각 조각의 내용은 한 번만 출력하고 이어지는 테두리를 유지하며, cached content를
넘는 선언 행 높이는 페이지 용량에 맞춘 빈 continuation fragment로 보존한다. 하나로 옮길 수
있는 row-span은 새 페이지에서 통째로 유지한다. 안전한 cache 경계가 없거나, row-span이 셀
단편화가 일어난 행과 교차하거나, 새 페이지에도 전체 span이 들어가지 않으면 조용히 자르지
않고 typed `table_cell_fragmentation_incomplete`로 보고한다. 공개 한 쪽 프로파일은 CI에서
대조하며, 더 넓은 한컴 코퍼스는 남아 있으므로 보편적인 대등성 인증은 아니다.

## 7. 데이터 정책

- **공개 코퍼스**(`fixtures/pdf-parity/public/`): 소유자 자작·가명화 원본 문서만. 기본 텍스트,
  문단, 목록, 표, 다단, 머리말·꼬리말·쪽번호, 각주·미주, 이미지, 도형, 수식, 복합 보고서를
  HWP/HWPX 쌍으로 커버한다. `.gitignore`는 `fixtures/pdf-parity/` 전체를 기본 무시하고,
  `public/source/` 아래 HWP/HWPX 파일, `public/recipes/` 아래 Markdown/JSON 레시피 파일,
  `public/manifest.json`, `public/scoreboard/` 아래 숫자 JSON/CSV 스코어보드(루트의
  `scoreboard.json`과 `scoreboard.csv`도 허용), 정확히
  `public/oracle/public-safety-rfp-p1.pdf`만 다시 허용한다. 그 밖의 공개 오라클 PDF/PNG와
  모든 비공개 코퍼스 경로는 계속 무시한다. 매니페스트는 Mac HWP 출처, A4/한 쪽 프로파일,
  source/oracle SHA-256, 정확한 OFL 글꼴 SHA-256, Poppler 버전, 150 DPI를 고정한다.
- **비공개 코퍼스**(`HWP_PDF_PARITY_CORPUS_DIR`): 실제 복합 문서. 보고서는 해시와 집계
  지표만 담고 원문·PDF·절대 경로는 기록하지 않는다.
- **공개 오라클 예외:** 위의 단일 PDF만 소유자가 작성하고 익명화한 Mac 한컴 HWP 12.30.0
  빌드 6446 캡처로 커밋한다. macOS 26.6.1 빌드 25G76에서 Quartz PDFContext와 기본 PDF
  저장 설정으로 만든 A4 한 쪽 픽스처다. 범위가 고정된 회귀 픽스처이며 보편적 또는 Windows
  동등성 주장이 아니다. source/oracle/글꼴 해시와 재현 절차는
  `fixtures/pdf-parity/public/recipes/public-safety-rfp-p1.md`에 둔다.
- 비공개 한컴 아티팩트, 제3자 실제 문서, 제3자 한컴 산출물, 한컴 명세 사본 및 그 밖의 모든
  오라클 PDF/PNG는 계속 금지한다. 커밋하는 레시피·매니페스트·숫자 스코어보드는 이 한 개의
  명시적 공개 오라클을 제외하면 절차, 출처, 핀, 해시, 집계 수치만 담는다.

## 8. 안티패턴 가드

- Hancom Office/COM을 runtime dependency로 추가하지 않는다.
- PDF를 페이지 이미지로 평면화하지 않는다 — 벡터 텍스트와 ToUnicode를 유지한다.
- PDF 백엔드에 별도 레이아웃 계산을 복제하지 않는다; DisplayList 직렬화 차이만 수정한다.
- 미지원 개체를 조용히 누락하지 않는다; placeholder를 정상 출력으로 인정하지 않는다.
- 일반문서 프로파일 밖의 기능까지 보편적 동등성으로 홍보하지 않는다.

## 9. 완료 조건

- 동일 공개 source/oracle/font/Poppler pin에서 PR 4부터 PR 9까지 커밋된 스코어보드가 단조
  비악화할 것. 픽스처가 해당 배치를 사용하지 않으면 동일 수치를 허용하되 회귀는 허용하지 않음.
- `MAX_BAD_PIXEL_PCT`가 PR 7에서 0.60에서 0.30으로 한 번 조정됐고 최초 placeholder로 남아
  있지 않을 것.
- 고정 Ubuntu 공개 게이트가 커밋된 한 쪽 오라클과 HWP/HWPX 두 케이스를 모두 통과한다.
- 모든 채점 케이스에서 글꼴 대체가 0건이며, source/oracle/글꼴 해시가 매니페스트와 레시피에서
  변경되지 않는다.
- 커밋하는 스코어보드는 고정 게이트 통과 뒤에만 생성하며, 스키마를 만족하고 경로를 포함하지
  않는다. 더 넓은 코퍼스는 명시적으로 후속 범위다.
- 해당 환경에서 `scripts/check.sh`, PDF 단위/통합 테스트 및 공개 parity 게이트가 모두 통과한다.
