[한국어](18-html-fragment-contract.md) · [English](18-html-fragment-contract.en.md)

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
  3. `*.svg` — DocumentSpec v2의 svg visual 정책 경로로 넘긴다(hwpx 네이티브,
     hwp5는 기존 폴리백 정책). v1에서 재사용이 불가하면 조용한 래스터화 대신 명시 에러.
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

- `script`, `style`, `iframe`, `form` 등 미열거 태그 → import 에러.
- 닫힘 불일치·속성 미인용 등 malformed XML → 파서 에러 그대로.
- CSS 클래스 기반 글자/문단 모양 복원 → v1 범위 밖(무시).

## 7. 버전

- v1 (2026-08): 최초 계약. export 측 GH-3/GH-4/GH-5 해소와 from_html 도입으로 성립.
