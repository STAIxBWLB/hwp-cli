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

정답지는 **최신 패치를 적용한 Windows용 한컴오피스 2024 한글**이 파일 → PDF로 저장하기로
만든 PDF다. 정품 Producer `Hancom PDF 1.3.0.550` 파일 검사로 문서 수준 동등성 표면 — 6개
catalog/Info 기능 — 을 확정했다:

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
이 필드들은 확인한 구조 계약을 구현한 상태이며, 한컴 값과의 정확한 동일성은 로컬 정답지 실행이 남아 있다.

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

**구현 상태 2026-08-13 (PR 4):** 배치 러너가 있다 — `scripts/pdf-parity.sh run`이 manifest의
모든 케이스를 채점해 커밋 가능한 수치 점수판(`fixtures/pdf-parity/public/scoreboard/`,
스키마 검증, 이름+SHA-256+수치뿐)을 만들고, `selftest`는 기준 없이 하네스를 검증하며,
`hwp diff --format json` / `--ours-png`가 쪽별 래스터 지표 원천이다. manifest, Poppler
버전, 폰트 파일, source/oracle digest가 pin과 일치해야 채점하며, 검증된 산출물 집합만
rollback 보호 방식으로 게시한다. 3~5건의 한글 기준 내기와 첫 커밋 수치는 남은 소유자
액션.

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
re-baseline 반영. 인라인 제어문자가 폭을 갖는다(HYPHEN은 `-` 셰이핑, NB_SPACE는 분리 기회
없이 공백 폭, FW_SPACE는 고정 1em). 정렬은 양쪽/배분/나눔을 구분(모드별 후행 gap·마지막 줄
규칙, 한컴 확인 대상). 자간은 HWPUNIT 정수 도메인 half-up 반올림. 합성 줄간격은 버전 인식
`line_spacing_type`으로 4모드(비율/고정/여백만/최소)를 모두 지원. `MAX_BAD_PIXEL_PCT`를
0.60→0.30으로 조인 것은 re-baseline 약속이며 첫 커밋 스코어보드(PR 4의 소유자 액션)가
검증·조정한다. GG-20/GG-3/GG-4는 정품 한글 저장 파일의 픽셀도 움직이고 GG-18은 합성 경로
전용.

## 4. 게이트

### 4.1 정본 한컴 오라클 게이트 — 향후 공개 코퍼스

이는 향후 공개 한컴 오라클 채점에 적용할 정본 목표다. 현재 structured-corpus 게이트가
아니며, 현재 자체 일관성 검사는 §4.2에 별도로 적는다.

- 페이지 수: 완전 일치
- MediaBox 차이 0.5 pt 이하
- `dx`, `dy` 각각 2 px 이하(150 DPI); `ink_ratio` 0.97–1.03
- `bad_pixel_pct` 5 % 이하, MAE 5 이하
- 기능 ROI 잉크 precision/recall 각각 0.95 이상
- 글꼴 대체, 미지원 누락, fatal 렌더 이슈: **0건**
- `pdffonts`: 모든 글꼴 embedded/subset + Unicode 지원
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
`bad_pixel_pct`, MAE는 현재 structured-corpus 임계값이 아니다. 이 값들은 §4.1의 한컴 오라클
게이트에 적용할 향후/정본 래스터 요구사항이며, 고정 오라클 PDF/PNG 픽스처와 비교 하네스가
준비된 뒤에만 활성화한다.

## 5. 글꼴 게이트 (F1)

한컴은 함초롬바탕/함초롬돋움을 임베드하고, 우리는 fontdb로 해석해 일반 산세리프로 폴백할 수
있다. 대체 한 번이 모든 advance, 모든 줄바꿈 결정, 모든 글리프 형태를 바꾸므로 **대체 글꼴
아래 잰 parity 수치는 무의미하다**.

- `RenderIssueReport::font_coverage()`가 FontMatched / FontSubstituted / FontMissing /
  FontSubsetFallback 횟수를 집계하고, CLI 렌더 리포트가 커버리지 줄을 출력한다.
- `FontCoverage::substitution_free()`가 하드 게이트다: 대체 없이 렌더되지 않은 케이스의
  parity 수치는 발행할 수 없다.
- 커버리지 자체를 확인할 수 없는 경우도 게이트 실패로 처리한다. 두 PDF 중 하나라도
  `pdffonts` 기준 임베드·서브셋·Unicode 지원을 모두 충족하지 못하면 역시 채점하지 않는다.

## 6. 페이지네이션 정본 (F3)

한글은 이미 페이지 경계를 알려준다: `LineSeg.flags` bit0(페이지 첫 줄)과 bit1(단 첫 줄)이
IR에 파싱돼 있다. 렌더러는 이 비트를 1급 페이지/단 경계 신호로 읽고, 합성 lineseg(flags
`0x0006_0000`)에서만 `v_pos` 리셋 휴리스틱으로 폴백한다. 표가 페이지를 걸쳐 잘리는 동안은
페이지 인덱스 비교가 무의미하므로, 페이지네이션 정확성(표 분할 포함, PR 2)은 측정의
**전제조건**이지 소비자가 아니다. **구현 상태 2026-08-13: PR #81**은 `Table.attr` pageBreak 정책에 따라 가능한 행 경계에서 표를 나누고 `row_span` 셀을 보존하며 제목 줄 자동 반복을 지원한다. 한컴오피스 정답지 대조는 남아 있으므로 아직 대등성 인증은 아니다.

## 7. 데이터 정책

- **공개 코퍼스**(`fixtures/pdf-parity/public/`): 소유자 자작·가명화 원본 문서만. 기본 텍스트,
  문단, 목록, 표, 다단, 머리말·꼬리말·쪽번호, 각주·미주, 이미지, 도형, 수식, 복합 보고서를
  HWP/HWPX 쌍으로 커버한다. `.gitignore`는 `fixtures/pdf-parity/` 전체를 기본 무시하고,
  `public/source/` 아래 HWP/HWPX 파일, `public/recipes/` 아래 Markdown/JSON 레시피 파일,
  `public/manifest.json`, `public/scoreboard/` 아래 숫자 JSON/CSV 스코어보드만 다시 허용한다
  (루트의 `scoreboard.json`과 `scoreboard.csv`도 허용). 공개 오라클 PDF/PNG와 모든 비공개
  코퍼스 경로는 계속 무시한다. 매니페스트는 정확한 Hancom 빌드, Windows 버전, PDF 설정,
  글꼴 SHA-256, source/oracle SHA-256, Poppler 버전, 150 DPI를 고정한다.
- **비공개 코퍼스**(`HWP_PDF_PARITY_CORPUS_DIR`): 실제 복합 문서. 보고서는 해시와 집계
  지표만 담고 원문·PDF·절대 경로는 기록하지 않는다.
- **한컴 유래 아티팩트는 커밋하지 않는다** — 내보낸 오라클 PDF/PNG와 비공개 원본 문서는
  로컬에 둔다. 커밋하는 레시피·매니페스트·숫자 스코어보드는 절차, 핀, 해시, 집계 수치만
  담으며 오라클 아티팩트가 아니다. 베이스라인은 파일 → PDF로 저장하기 →
  `pdftoppm -png -r 150`로 수동 생성(3–5건)하고 로컬에 둔다.
- 제3자 실제 문서, 한컴 명세 사본, 비공개 오라클 PDF는 커밋하지 않는다.

## 8. 안티패턴 가드

- Hancom Office/COM을 runtime dependency로 추가하지 않는다.
- PDF를 페이지 이미지로 평면화하지 않는다 — 벡터 텍스트와 ToUnicode를 유지한다.
- PDF 백엔드에 별도 레이아웃 계산을 복제하지 않는다; DisplayList 직렬화 차이만 수정한다.
- 미지원 개체를 조용히 누락하지 않는다; placeholder를 정상 출력으로 인정하지 않는다.
- 일반문서 프로파일 밖의 기능까지 보편적 동등성으로 홍보하지 않는다.

## 9. 완료 조건

- 커밋된 스코어보드가 첫 베이스라인(PR 4)부터 PR 9까지 단조 개선된다.
- 모든 채점 케이스에서 글꼴 대체가 0건이다.
- `MAX_BAD_PIXEL_PCT`는 advance 영향 배치의 단일 리베이스라인 PR(PR 7)에서 정확히 한 번만
  조여지고 placeholder로 남지 않는다.
- `scripts/check.sh`, PDF 단위/통합 테스트, 공개 parity 게이트가 모두 통과한다.
