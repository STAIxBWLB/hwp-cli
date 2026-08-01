[한국어](00-overview.ko.md) · [English](00-overview.md)

# hwp-cli 시스템 설계도 (Design Blueprint)

> **목적:** 한글(HWP/HWPX) 문서를 다루는 시스템을 **처음부터(from scratch) 재구축**할 수 있도록,
> 이 프로젝트가 축적한 모든 지식(바이트 포맷·렌더링·변환·한글 호환 규칙·방법론)을 문서로 남긴다.
> 이 문서군(`docs/design/`)은 코드의 "왜"와 "어떻게"를 담은 **설계 기준선**이다.

이 프로젝트는 기존 HWP 라이브러리에 의존하지 않고 **HWP 5.0 바이너리와 HWPX(OWPML)를
직접 구현**한다(파싱·직렬화·변환·렌더링 전부). 문서·주석은 한국어를 기본으로 한다.

---

## 1. 시스템이 하는 일

```
                     ┌─────────────────────────────────────────┐
   .hwp (바이너리) ──▶│                                         │──▶ .hwpx
   .hwpx (OWPML) ────▶│   read → IR(중간표현) → write / render   │──▶ .hwp
   .md (마크다운) ───▶│                                         │──▶ PNG / SVG / PDF
                     └─────────────────────────────────────────┘
                                        │
                          편집(edit/field/bookmark/format/structure)
```

- **읽기**: HWP5(CFB+레코드), HWPX(ZIP+OWPML XML) → 공유 IR
- **쓰기**: IR → HWP5, HWPX (한글에서 정상 열림이 목표)
- **변환**: hwp5 ↔ hwpx ↔ markdown/JSON/HTML
- **렌더링**: IR → PNG/SVG/PDF (최대한 픽셀 정확)
- **편집**: 서식·필드·책갈피·도형·구조 편집 프리미티브 + JSON 왕복

---

## 2. 아키텍처 한눈에

### 2.1 크레이트 (허브-스포크)

```
                hwp-model  (기반 IR — serde만 의존, 다른 내부 크레이트에 의존 안 함)
               /    |    |    \        \
          hwp5   hwpx   hwp-convert   hwp-render
               \    |  ____/  |          /
                hwp-cli  (bin: `hwp`, 위 5개 전부 의존)
```

| 크레이트 | 책임 |
|---|---|
| **hwp-model** | HWP/HWPX 공유 **의미 IR** 타입. 이 안정성이 곧 프로젝트 안정성. |
| **hwp5** | HWP 5.0 바이너리(CFB+레코드) ↔ IR reader/writer |
| **hwpx** | HWPX(OWPML/ZIP+XML) ↔ IR reader/writer + patch |
| **hwp-convert** | IR ↔ markdown/JSON/HTML + 편집 프리미티브 |
| **hwp-render** | IR → PNG/SVG/PDF 렌더러 |
| **hwp-cli** | 서브커맨드 디스패치(info/cat/convert/render/new/edit/…) |

**불변식:** `hwp-model`은 다른 내부 크레이트에 절대 의존하지 않는다. `hwp5`↔`hwpx`는 서로
의존하지 않고 IR을 경유한다. 이 대칭성이 "N 포맷 × M 출력"을 N+M 어댑터로 처리하는 핵심.

### 2.2 IR (중간표현) — L1 의미 계층

`Document → Section → Paragraph → (HwpChar 열 + Control + LineSeg + CharShapeRun)`.
본문은 **UTF-16 코드유닛(WCHAR)** 열이고 0~31은 컨트롤 문자다. 위치 계산의 단일 진실은
`char_kind(code)` 분류(문자형 1폭 / 인라인 8폭 / 확장 8폭). 무손실 왕복은 `OpaqueRecord`로
보존. 상세는 [01-architecture-ir.ko.md](01-architecture-ir.ko.md).

---

## 3. 문서 색인

| # | 문서 | 내용 |
|---|---|---|
| 01 | [architecture-ir](01-architecture-ir.ko.md) | 크레이트 구조, IR 타입 계층, 데이터 흐름, OpaqueRecord |
| 02 | [hwp5-read](02-hwp5-read.ko.md) | CFB, 레코드 헤더 비트 레이아웃, DocInfo·BodyText 파싱, 압축·인코딩 |
| 03 | [hwp5-write](03-hwp5-write.ko.md) | **한글 호환 합성**(CFB V3, EncryptVersion, COMPATIBLE_DOCUMENT, 버전 게이팅) |
| 04 | [hwpx-owpml](04-hwpx-owpml.ko.md) | HWPX ZIP(OPC), hp:/hc: 요소, 도형 기하, 부동/인라인 배치 |
| 05 | [rendering](05-rendering.ko.md) | 레이아웃, lineseg 합성, 도형 드로잉, 폰트 셰이핑, PNG/SVG/PDF |
| 06 | [convert-cli-methodology](06-convert-cli-methodology.ko.md) | 변환 파이프라인, CLI, **정답지 방법론·진단 기법** |
| 07 | [hangul-compat-rules](07-hangul-compat-rules.ko.md) | ★**실기로 확정한 한글 호환성 규칙 전체 카탈로그** |
| 08 | [external-research](08-external-research.ko.md) | OWPML 표준·오픈소스·페이지네이션 동작 외부 근거(deep-research) |
| 09 | [diagnostic-corpus](09-diagnostic-corpus.ko.md) | 기능 격리 진단 코퍼스 + 자체 검증 하네스(현 상태·확장법) |
| 10 | [hwp5-structure-map](10-hwp5-structure-map.ko.md) | **HWP5 전수 지도** — CFB 스트림 트리, 레코드 카탈로그(태그 전수+구현 상태), 컨트롤 문자·ctrl ID |
| 11 | [hwpx-structure-map](11-hwpx-structure-map.ko.md) | **HWPX 전수 지도** — OPC 트리, 네임스페이스, 요소 카탈로그, read/write 대칭성 감사 |
| 12 | [feature-gaps](12-feature-gaps.ko.md) | 기능 격차 카탈로그 + 난이도·의존성 로드맵 (07§F 승계, 10§8·11§5가 근거 데이터) |
| 18 | [html-fragment-contract](18-html-fragment-contract.ko.md) | **HTML fragment 계약** — Maru 부분(part) 작성·조합용 XHTML 부분집합, 표/그림/각주 왕복 규약 |

---

## 4. 핵심 설계 원칙

1. **직접 구현.** 기존 HWP 크레이트 미사용. 인프라 크레이트(cfb/zip/quick-xml/tiny-skia 등)만 허용. 의존성 최소화.
2. **IR 경유 대칭.** 모든 포맷은 IR을 통해서만 만난다(hwp5↔hwpx 직접 의존 없음).
3. **무손실 왕복 우선.** 같은 포맷 왕복(hwp5→hwp5)은 바이트 동일이 게이트. 합성(source≠hwp5)만 재구성.
4. **★정답지(ground-truth) 방법론.** 추측 금지. 한글이 저장한 **정품 파일 바이트**를 정답지로 삼아
   우리 산출물과 유일 차이를 특정해 반영한다. 코퍼스(`~/Documents/hwp_samples`)는 저장소에 절대 커밋 금지.
5. **실기 게이트.** 한글(한컴오피스)에서 실제로 열리고 정상 렌더되는지가 최종 판정. 상세 → [06](06-convert-cli-methodology.ko.md), [07](07-hangul-compat-rules.ko.md).

---

## 5. 현재 상태 (2026-07 기준)

**정상 동작(한글 실기 확인):** HWP5/HWPX 읽기·쓰기·변환·렌더링, 단일/다문단·긴문단·표(단순/긴셀/
1행/다열/빈셀)·본문+표혼합·멀티페이지·복합 보고서, 하이퍼링크·책갈피, 서식·구조 편집, annual 디자인
문서의 도넛·중앙원·숫자·호(arc).

**미해결/조사 중:** annual 5·6쪽 **글상자 드롭 + 페이지 오버플로**. 원인은 구조(도형-문단 배치)가
아니라 **개체 속성 충실도**(vertRelTo/treatAsChar/z-order/textWrap/offset)일 가능성이 유력
(외부 리서치 [08](08-external-research.ko.md) 참조). 그 외 U2(양쪽정렬)·U4(자간)·글상자 렌더 정밀도.
미구현 기능·손실 지점의 전체 카탈로그와 로드맵은 [12-feature-gaps.ko.md](12-feature-gaps.ko.md) 참조.

**★가장 값진 자산:** [07-hangul-compat-rules.ko.md](07-hangul-compat-rules.ko.md) — 정적 분석으로는 잡히지
않고 오직 정품 대조·실기로만 확정된 30여 개 한글 특정 규칙. 이 시스템을 다시 만든다면 이 카탈로그가
가장 큰 시간 절약이 된다.
