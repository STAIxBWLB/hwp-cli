[한국어](18-html-fragment-contract.ko.md) · [English](18-html-fragment-contract.md)

# HTML fragment 계약 (v1)

Maru 문서 작성기의 **부분(part) 단위 작성·조합**을 위한 HTML fragment 교환 계약.
본문 산문은 markdown으로, 표·그림 등 비산문 블록은 이 계약의 HTML fragment로 작성한다.
대규모 문서(사업보고서·결과보고서 등)를 부분별로 나누어 작성하고 조합하는 워크플로의
교환 포맷이다.

- **생산자**: `hwp-convert/src/html.rs` (`to_html`, `to_html_fragment`)
- **소비자**: `hwp-convert/src/from_html.rs` (계약 파서), `from_markdown.rs`의 HTML 블록 경로

## 1. 원칙

1. **기계 생성 전용** — 사람이 손으로 쓴 임의 HTML이 아니라, 계약을 아는 생산자가 만든
   출력만 입력으로 받는다.
2. **Well-formed XML (XHTML)** — 빈 태그는 self-closing(`<br/>`, `<img …/>`), 속성은
   큰따옴표, 텍스트는 XML 이스케이프. quick-xml로 파싱 가능해야 한다.
3. **구조 왕복, 스타일 비왕복** — `class`/`style` 속성은 표현 전용이며 import 시 무시한다.
   왕복이 보장되는 것은 문서 구조(표 span·셀 블록·이미지·링크·인라인 마크)뿐이다.
4. **계약 위반은 hard error** — 알 수 없는 태그, malformed XML, span 오버플로를 추측으로
   복구하지 않는다(추측 금지 — 정답지 방법론과 같은 태도).

## 2. 지원 요소

### 2.1 블록

| 요소 | 의미 | 비고 |
|---|---|---|
| `h1`..`h6` | "개요 N" 스타일 문단 | export·import |
| `p` | 본문 문단 | export·import |
| `ul`/`ol`/`li` | 목록 (`li` 중첩 = 수준) | import only (export는 미방출) |
| `table` | 표 — §3 참조 | export·import |
| `figure` + `figcaption` | 그림 + 캡션 문단 | import only (export는 bare `img`) |
| `section.footnotes` | 각주/미주 정의 (§5) | 표현 전용 |

### 2.2 인라인

| 요소 | 의미 |
|---|---|
| `strong`/`em`/`u`/`s` | 굵게/기울임/밑줄/취소선 |
| `sup`/`sub` | 위첨자/아래첨자 (`sup`가 각주 마커 패턴과 충돌하지 않게 §5 규약) |
| `a[href]` | 하이퍼링크 필드 |
| `br/` | 줄나눔 (LINE_BREAK) |
| `img` | 그림 — §4 참조 |

## 3. 표 계약

- 구조: `<table>` → `<tr>` → `<th>`/`<td>`. `thead`/`tbody`는 선택(있어도 되고 없어도 됨).
- **첫 행은 `<th>` 관례** — 표현 전용이다. import는 `th`/`td`를 구별하지 않는다(IR에
  헤더 행 개념이 없다).
- **병합 셀**: origin 셀만 방출하고 `colspan`/`rowspan`을 단다. 병합이 덮는 칸은
  요소를 방출하지 않는다. import는 점유 격자로 역산해 `Cell.col_span`/`row_span`을 복원한다.
- **span 오버플로**(격자 밖으로 나가는 span, 덮인 칸과 겹치는 span)는 import 에러다.
- **셀 내 블록**: 셀 안에는 인라인 내용 외에 중첩 `table`, `img`가 올 수 있다.
  문단 경계는 `<br/>`로 표현한다.

## 4. 그림 계약

- `src` 세 가지 형태:
  1. `data:<mime>;base64,…` — 자기완결 임베드 (export가 쓰는 형태)
  2. 상대 경로 — part 파일 기준 `base_dir`로 해석
  3. `*.svg` — **검증(폐쇄 부분집합) + 결정론적 PNG 래스터화**로 임베드한다
     (`hwp-convert::svg` — DocumentSpec v2의 svg visual과 같은 정책·같은 구현.
     네이티브 표현은 어느 포맷에도 없다). 스크립트·외부 참조·텍스트 노드가 있는
     SVG는 hard error.
- `alt`는 무시하지 않고 IR Picture의 대체 텍스트로 보존한다(export는 `"image"` 고정 — 관례).

## 5. 각주/미주 (표현 전용)

- 본문 마커: `<sup id="fnref-{N}"><a href="#fn-{N}">{N}</a></sup>` (미주는 `e{N}`).
- 정의: `<section class="footnotes">` 안 `<ol>`/`<li id="fn-{N}">`, 끝에
  `<a href="#fnref-{N}">↩</a>` 역링크.
- import는 이 구조를 **평문**으로만 읽는다(각주 의미 재생성은 v1 범위 밖 — `sup` 인라인
  마크와의 모호성을 피하기 위함). `fnref` id를 가진 `sup`는 마커로 인식해 텍스트만 취하고,
  `<section class="footnotes">` 정의 섹션은 통째로 걷어낸다(본문 마커와 정의가 이중으로
  들어가는 것을 막기 위함).

## 6. 비지원 · 에러

- `script`, `iframe`, `form` 등 미열거 태그 → import 에러. 단 `<style>`은 예외적으로
  허용한다 — v2 스타일 왕복(§8)의 `.cs{n}`/`.ps{n}` 규칙만 읽고 나머지는 무시한다.
- 닫힘 불일치·속성 미인용 등 malformed XML → 파서 에러 그대로.
- CSS 클래스 기반 글자/문단 모양 복원은 v2(§8) 규칙에 한한다 — 그 밖의 클래스와
  인라인 `style` 속성은 무시.

## 7. 버전

- v1 (2026-08): 최초 계약. export 측 GH-3/GH-4/GH-5 해소와 from_html 도입으로 성립.
- v2 (2026-08): 스타일 왕복(§8) — `.cs{n}`/`.ps{n}` 클래스로 글자·문단 모양을 실는다.

## 8. 스타일 왕복 (v2)

v1은 구조만 왕복한다. v2는 글자·문단 모양을 CSS 클래스로 실어 타이포그래피까지
왕복한다 — Maru 부분 편집기가 `hwp → html → 편집 → hwp`에서 부분의 모양을 잃지
않게 하기 위함이다.

### 8.1 규칙 위치와 명명

- 규칙: standalone은 `<head>`의 `<style>` 블록, fragment는 **선두 `<style>` 요소**
  (fragment가 자기완결이어야 하므로).
- import는 `<style>`을 허용하되 `.cs{n}`/`.ps{n}` 규칙만 읽고 나머지는 무시한다(§6).
- 명명: `.cs{n}` = 소스 문서의 CharShape id n, `.ps{n}` = ParaShape id n. id는 생산자
  문서 기준이며 소비자는 이름이 아니라 **속성값**으로 복원한다(같은 id 보장 없음).

### 8.2 속성 ↔ 필드 매핑

`.cs{n}` (글자 모양):

| CSS | CharShape 필드 |
|---|---|
| `font-family` | 첫 번째 이름 = face_ids[0]의 글꼴 이름, 나머지는 폴리백(무시) |
| `font-size` | `base_size/100` pt × `rel_sizes[0]` % |
| `color` | `text_color` (COLORREF → #RRGGBB; 0이면 생략) |
| `background-color` | `shade_color` (0xFFFFFFFF가 아닐 때만) |
| `letter-spacing` | `spacings[0]` % (`Nem` = N% of em; 0이면 생략) |

`.ps{n}` (문단 모양):

| CSS | ParaShape 필드 |
|---|---|
| `text-align` | 정렬 (justify/left/right/center; 4·5 배분·나눔은 justify로 근사) |
| `line-height` | 종류 0 비율=단위 없는 배수, 1 고정·3 최소=pt, 2 여백만=`normal`(근사) |
| `margin-left`/`margin-right` | margin_left/right (mm) |
| `text-indent` | indent (mm) |
| `margin-top`/`margin-bottom` | spacing_top/bottom (mm) |

### 8.3 우선 규칙과 한계

- **마크는 태그가 정본** — 굵게·기울임·밑줄·취소선·첨자는 `strong/em/u/s/sup/sub`
  태그로만 복원한다. 클래스는 태그가 못 싣는 나머지(글꼴·크기·색·음영·자간·정렬·
  줄간격·여백)를 싣는다.
- 마크업: 문단은 `<p class="psN">`/`<h{level} class="psN">`, run은
  `<span class="csN">…</span>`이 마크 태그를 감싼다.
- **팔레트 dedup**: 복원한 모양이 기본 팔레트(문단 0~4·글자 0~15)와 같으면 새 id를
  만들지 않고 팔레트 id를 쓴다.
- v2 한계: 표 셀 문단 모양·각주 스타일은 싣지 않는다. 인라인 `style` 속성은 무시
  (`<style>` 블록만 읽는다). border_fill·그림자·양음각·외곽선·장평·오프셋 등 CSS로
  표현할 수 없는 필드는 복원하지 않는다.
