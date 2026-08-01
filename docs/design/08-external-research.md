[한국어](08-external-research.md) · [English](08-external-research.en.md)

# 08. 외부 근거 — OWPML 표준·오픈소스·페이지네이션 동작 (deep-research)

> 2026-07-04, 다중 소스 웹 리서치 + 적대적 검증(98 에이전트)으로 수집·검증. 우리가 실기로
> 관찰한 "도형 몰린 문단 → 글상자 드롭 + 페이지 오버플로"를 **뒷받침 또는 반증**하기 위한 외부 근거.

## 조사 질문

hwp5→hwpx 변환기에서 원본이 "한 페이지 도형 수십 개(예: 35개)를 하나의 문단(`hp:p`)에 앵커"
하는 구조일 때, 한글 렌더링에서 (a) 일부 글상자(`hp:rect`+`drawText`)가 안 그려지고 (b) 페이지가
오버플로해 빈 페이지가 삽입된다. **원본 hwp5는 정상인데 우리 hwpx 변환본만** 이 현상이 발생.

## 검증된 발견 (신뢰도·출처)

| # | 발견 | 신뢰도 | 근거 |
|---|---|---|---|
| 1 | **"한 문단에 도형 수십 개 앵커"는 정상 구조.** 표준 참조 변환기 `hwp2hwpx`도 문단을 **1:1 매핑**해 35개 도형을 한 `hp:p`에 그대로 넣고 **여러 문단으로 재분배하지 않는다**. | 높음 | neolord0/hwplib·hwp2hwpx (`ForSectionXMLFileList.section()`이 문단 1:1, `ForGso.convert()`가 같은 run에 모든 GSO append) |
| 2 | hwp5·HWPX 두 포맷 모두 도형(GSO)은 **문단 하위 앵커 개체**. 다수 개체를 한 문단에 두는 것은 스키마 위반 아님. | 높음 | hwplib(Paragraph.controlList), Hancom tech blog(hp:p→hp:run→hp:t) |
| 3 | (Q1) **문단/런/페이지당 그리기 개체 수 제한이 문서화된 곳 없음.** 개체 분산 강제 규칙도 없음. | 중간(침묵 근거) | hwp2hwpx README, hancom-io/hwpx-owpml-model |
| 4 | (Q2) **페이지네이션은 파일에 저장되지 않고 렌더 타임에 "문단 중심"으로 계산.** "문단을 그리다 영역이 부족하면 그때 페이지가 추가된다." | 높음 | hwplib README(페이지 API 미지원), 유지관리자 이슈 #31 |
| 5 | (Q3) **"도형 많은 문단에서 글상자 드롭" 버그가 문서화된 곳 없음** (변경이력·이슈·공식 모델 저장소 모두). | 중간(침묵 근거) | hwp2hwpx changelog(~2026-06-25)·이슈 7건, hancom 모델 저장소 |
| 6 | HWPX 출력은 **국가표준 KS X 6101 = OWPML** 스키마를 따라야 함. 상세 개체/z-order/렌더 규칙은 오픈소스가 아니라 외부 OWPML **상세 스펙(PDF)**에 있음. | 높음 | Hancom 공식, standard.go.kr, hancom-io 모델 저장소 |
| 7 | (Q5) hwp5=CFB 바이너리(레코드→XML 트리 변환 필요), HWPX=이미 XML(OPC/ZIP). 최소 한 렌더러(rhwp)는 둘을 **단일 통합 IR**로 파싱. | 중간 | pyhwp 문서, rhwp 온보딩(HWP/HWPX→Parser→IR→Paginator→Layout) |

## ★결정적 함의 — 원인 재구성

- **구조는 정상이다.** 표준 변환기조차 하지 않는 "도형 문단 분산"은 정석 해법이 아니다. → 우리가
  계획했던 "도형을 여러 문단에 분산"하는 재설계는 **표준과 어긋나며 근본 해법이 아닐 가능성**이 크다.
- **페이지네이션은 렌더 타임·문단 중심**이다. 빈 페이지는 저장 속성이 아니라 한글의 레이아웃 계산
  차이다. 앵커 문단의 **줄 높이·문단 속성**이 페이지 계산을 좌우한다.
- **가장 유력한 원인(열린 질문):** 변환 매핑이 구조적으로 1:1이어도, 각 개체·문단의 **속성 충실도**
  (`vertRelTo`/`horzRelTo`/`treatAsChar`/`z-order`/`textWrap`/`offset`/`vpos`, 빈 문단 줄 높이 등)가
  원본 hwp5와 출력 hwpx 사이에서 달라져 한글 레이아웃이 바뀌는 것. **속성 단위 diff는 미검증.**

## 한계 (반드시 유의)

- **Q1·Q3 답은 "문서화된 근거 없음"이라는 침묵 근거**(argument from silence). 제한이 없음을 증명하지
  못한다 — 한글 내부(폐쇄 소스) 렌더러의 **미문서화 실용 한계** 가능성은 남는다.
- 이 증상을 직접 설명하려던 4개 가설(① TopAndBottom 개체 공간 예약/vpos 보정 누락, ② treatAsChar
  개체의 LINE_SEG 높이 과다, ③ python-hwpx 하위 요소 누락, ④ shapeObject/connectLine 표현)은
  3표 적대적 검증에서 **전부 반증(0-3/1-2)**. 이 자료만으로는 메커니즘 귀속 불가.
- 출처가 오픈소스 README·표준 등록부에 치우침. 개체 앵커/공간 예약/z-order 규칙이 실제 기술될
  Hancom OWPML **상세 스펙(PDF)은 직접 열람 못 함**. rhwp는 서드파티 재구현이라 한글 실제 동작을 대변 못함.

## 후속 조사 방향 (우선순위)

1. **속성 단위 diff** — 원본 hwp5의 도형/문단 속성 vs 우리 hwpx 출력 속성을 개체별로 대조
   (vertRelTo/horzRelTo/treatAsChar/z-order/textWrap/offset/vpos, 빈 문단 char_shape·줄높이). ← 유력.
2. Hancom OWPML **상세 스펙 PDF**(hancom.com/etc/hwpDownload.do) 확보·열람 — 개체 앵커/공간 예약 규칙.
3. 35개 도형을 유지하되 **속성만 정품과 일치**시켰을 때 한글에서 드롭/오버플로가 해소되는지 실기.
4. 빈 페이지가 도형이 아니라 **앵커 문단의 줄 높이**에서 오는지 — 빈 문단 줄 높이를 정품과 맞춰 검증.

## 참고 출처

- neolord0/hwplib, neolord0/hwp2hwpx (사실상 표준 참조 변환기)
- hancom-io/hwpx-owpml-model (공식 OWPML 모델)
- Hancom tech blog (tech.hancom.com/hwpxformat, python-hwpx-parsing)
- KS X 6101 (국가표준, standard.go.kr) — OWPML 문서 구조
- pyhwp 문서, rhwp (서드파티 통합 렌더러)

---

# 렌더러 완성 조사 (2026-07-05, deep-research 102 에이전트)

> 최우선 목표 "렌더러 완벽 구현"을 위해 미구현·근사 렌더 기능(수식·차트·쪽테두리·세로쓰기·
> 다단·justify/자간·OLE)의 정확한 구현법을 HWP 5.0 스펙·OWPML·오픈소스 기준으로 조사. 25개
> 주장 적대 검증 → 23 확정·2 반증.

## ★결정적 메타 결론
**오픈소스 HWP "렌더러"는 존재하지 않는다.** hwplib·hwpxlib·rust `hwp` 크레이트·hancom-io
owpml-model은 전부 **파서/객체모델**(layout/draw/paint 패키지 없음). → 레이아웃·조판·드로잉
알고리즘은 전부 우리가 직접 구현. 파서 근거만 재사용 가능.

## 기능별 근거 (신뢰도·출처)
| 기능 | 확정 사실 | 근거 |
|---|---|---|
| **수식** | hwp5(HWPTAG_EQEDIT)·hwpx(CEquationType→CScript) 둘 다 **텍스트 스크립트** 저장(글립 아님). 예약문자 `~`공백·`` ` ``¼공백·`{}`그룹·`" "`단어·`#`줄바꿈·`&`열정렬. 키워드 OVER(분수선)/ATOP(선없음)/SQRT/SUP·`^`/SUB·`_`/적분족 INT·OINT·DINT·TINT·ODINT·OTINT/SUM·PROD·UNION·INTER/matrix{`&`열 `#`행}·P/B/D-MATRIX. 함수어(sin·log·lim…) 로만체, 이름 내부 공백→이탤릭. | 한컴 수식 spec rev1.2, equation help, hwplib ControlEquation |
| **차트** | hwp5=ChartObj 이진 트리(VtChart 루트, StoredtypeID 중복 dedup). hwpx=CChartType이 `chartIDRef`만→외부 `chart/chartN.xml`(OOXML DrawingML). 시리즈/축 필드 레이아웃 **미확보(open)**. | 한컴 차트 spec rev1.2, hancom owpml-model |
| **쪽테두리** | BorderFill=4변+대각+채움1, u16 비트필드(bit0 3D·bit1 그림자·bit2-4 slash·…·bit13 중심선), 채움종류 u32(0없음/1색/2이미지/4그러데이션). 위치기준 종이(안쪽)/쪽(바깥), gap 4방향, 최대 25mm(UI 제약). ~~HWPTAG_PAGE_BORDER_FILL 바이트 레이아웃은 반증(미확정)~~ → **해소(2026-07-19, GC-2 조사)**: 당시 "반증"의 근본 원인은 스펙 표135의 자기모순(선언 12B ≠ 필드 합 14B). 정품 236파일·714레코드 전수 **14B**(속성 u32 + 4방향 gap u16×4 + 테두리ID u16, BOTH/EVEN/ODD는 레코드 순서로 구분)로 확정 — 우리 코드는 이미 올바름 | rust hwp border_fill.rs, 5.0 spec 표24, hwplib PageBorderFillProperty, 정품 전수 스윕 |
| **다단** | COLDEF(`cold`, 표138/139): attr **bit0-1 종류(0일반/1배분/2평행)·bit2-9 단수(1-255)·bit10-11 방향(0왼/1오른/2맞쪽)·bit12 동일폭**; gap HWPUNIT16; 비동일폭이면 단별 폭 배열; 구분선(type·굵기·색). **hwplib과 bit단위 일치**. | 5.0 spec, hwplib ControlColumnDefine |
| **세로쓰기** | "방향 플래그일 뿐" 주장 **반증** — 확정 렌더 알고리즘 없음. → **보류**. | (open question) |
| **자간/장평** | CharShape 장평 ratio INT8[7] 50-200%, 자간 spacing 실질 -50~50%. glyph advance = base×장평 + 자간. (우리 shape.rs 이미 적용) | 한컴테크 python-hwp-parsing-2 |

## 미확정(열린 질문) — 구현 시 정답지로 해소
- ~~HWPTAG_PAGE_BORDER_FILL 정확 바이트 구조(반증됨) → 실기 표본 역설계~~ — 해소(2026-07-19, 위 참조).
- 세로쓰기 글립 회전·행 진행 규칙 → 확정 알고리즘 없음.
- justify(양쪽정렬) CJK vs 라틴 여백 분배 정확 규칙·마지막 줄 → 미확보(휴리스틱).
- 차트 시리즈/축 이하 필드 레이아웃, hwpx chartN.xml OOXML 매핑 → 트리/참조까지만 확보.

## 참고 출처 (렌더러)
- 한컴 공식 스펙 PDF: 한글문서파일형식 5.0 rev1.3, 수식 rev1.2, 차트 rev1.2
- 한컴 help: equation(script/explanation/font), page_border, vertical
- neolord0/hwplib·hwpxlib, hancom-io/hwpx-owpml-model, docs.rs `hwp` 크레이트, hahnlee/hwp.js
- 한컴테크 블로그(tech.hancom.com): hwpxformat, python-hwp-parsing (장평·자간)

---

# 생태계 기능 대조 (2026-07-08, deep-research 102 에이전트)

> hwp-cli에 없는, 실사용자가 필요로 하는 기능을 타 구현체·수요 근거로 발굴. 91개 주장 추출 →
> 25개 적대 검증 → **21 확정(전부 3-0)·4 반증**. [12-feature-gaps.md](12-feature-gaps.md) §10 GJ와
> §14 로드맵 재평가(GA-2·GB-1★)의 근거가 이 절이다.

## (a) 타 구현체가 이미 지원 — 구현 선례 존재

| 발견 | 함의(12번 항목) | 근거 |
|---|---|---|
| **배포용 문서 읽기** — pyhwp 0.1b7(2014)~, H2Orestart v0.7.11(2026-04)~. 한컴 공식 스펙 「배포용문서 rev1.2」가 복호화 전체(DISTRIBUTE_DOC_DATA 256B, 난수 배열, SHA1 유도 키, AES-128 ECB)를 공개 — **역설계 불요** | GA-2 난이도 L→**M** 재평가 | pypi pyhwp changelog, H2Orestart #42, 한컴 스펙 PDF 실물 열람 |
| **HWP 3.x 파싱** — rhwp(Rust, 렌더링까지·763페이지 오라클 검증), kordoc v2.7.1(텍스트 추출), LibreOffice hwpfilter(V30SIGNATURE). 공식 스펙 「3.0/HWPML rev1.2」 Part I이 구조 전부 문서화 | GJ-3 착수 가능 | rhwp README·PR#506, kordoc CHANGELOG, LibreOffice docs |
| **HWPML(.hml) 입력** — kordoc 전용 파서 상용화. 같은 스펙 Part II가 XML 요소 레퍼런스 | GJ-2 착수 가능 | kordoc src/hwpml/parser.ts |
| **세로쓰기·수식 행렬** — 동일 언어(Rust) rhwp가 둘 다 구현(글상자 vertical-rl/lr, MATRIX/PMATRIX/BMATRIX/DMATRIX + 수식 테스트 114건) | GC-1·GD-1 참조 구현 존재 | rhwp src/paint/font.rs·renderer/equation/parser.rs |
| **HWPX 네이티브 차트 생성** — kordoc v3.16: 차트는 OLE가 아니라 `Chart/chartN.xml`(OOXML chartSpace) + manifest + `hp:chart chartIDRef`. 20종 차트를 XML 조작만으로 생성 | GB-1 hwpx 경로 L→**M** 재평가 | kordoc CHANGELOG·src/hwpx/chart-gen.ts |
| **양식 개체·글맵시·OLE 객체모델 조작** — hwplib(Java)이 읽기/쓰기 지원(렌더링 아님) | GB-2·GB-4·GB-5 객체모델 선례 | neolord0/hwplib changelog |
| **도장 자동 날인·문서 비교·서식 보존 패치** — kordoc seal("(인)" 앵커에 도장 PNG 부유 배치)·compare_documents·patch_document | GM-7·GM-8 선례 | kordoc README·MCP 도구표 |

## (b) 수요는 확실하나 OSS가 잘 못 하는 것 — 무주공산

| 발견 | 함의 | 근거 |
|---|---|---|
| **HWP→DOCX** — Microsoft가 공식 변환기(HwpConverter + 폴더 일괄 BATCHHWPCONV.exe)를 배포할 정도의 수요. OSS 출력 구현은 사실상 부재(pyhwp=ODT/HTML/txt만, kordoc은 DOCX 입력만) | GJ-1 = 수요 최상 | microsoft.com id=36772·49153 |
| **독립 실행형 문서 병합/분할** — pyhwpx 쿡북의 정식 챕터(33개 병합·100쪽 분할)일 만큼 실무 빈출인데, 현행 해법은 Windows+한글 COM 전용에 클립보드 에러로 취약 | GM-3·GM-4 | wikidocs 8956(pyhwpx 쿡북) |
| **HWPX 배포용 문서** — H2Orestart(#42 오픈)조차 미지원, 어느 구현체도 못 읽음. HWP5용 공식 배포 스펙이 HWPX 변형을 커버하는지 미확인 | GJ-8 (L) | H2Orestart #42 |

## 경쟁 맥락·기타 확정

- **pyhwp는 HWP5 전용·변환 3종(ODT/HTML/txt)·공식 "실험적" 표기·0.1b15(2020) 정체** — hwp-cli의
  변환 스위트가 이 생태계 최광폭. hwpx 지원 요청(#135)은 2013년부터 오픈 → HWPX 수요의 깊이.
- **H2Orestart + LibreOffice headless**가 사실상의 CLI HWP→PDF로 프로덕션 채택(Dangerzone).
  JRE+LO 의존 없는 단일 바이너리가 hwp-cli의 차별점.
- **hwp.js 미해결 이슈 3건은 정합 테스트 소재**: 한글 2018산 5.1.0.1.1 파싱(#59), 첨자(#55),
  단 정의 스펙 14B vs 실파일 16B 불일치(#58 — ★스펙-실파일 불일치의 실증 사례).

## 한계

- 접근성(alt text)·전자서명·PDF/A **규제 수요는 검증 통과 근거 없음**(수요 부재가 아니라 근거 미생존).
  표→CSV 수요도 직접 근거 미생존(kordoc XLS/XLSX는 입력 방향).
- rhwp·kordoc은 2026-03 말 생성된 신생 프로젝트 — 기능은 코드 수준 검증했으나 렌더 정합 품질은
  자체 보고(오라클/aHash) 기반. kordoc HWP3.x는 텍스트 추출 한정.
- 반증 4건 폐기: hwplib 미지원 목록(0-3), hwp-rs read-only 단정(0-3), hwp.js 정체 단정(1-2),
  LibreOffice HWP 2.0/2.1 계보(0-3).

## 참고 출처 (생태계)

- pypi.org/project/pyhwp, pyhwp.readthedocs.io(converters), github.com/mete0r/pyhwp#135
- github.com/ebandal/H2Orestart(#42), extensions.libreoffice.org/27504, freedomofpress/dangerzone
- github.com/edwardkim/rhwp, github.com/chrisryugj/kordoc, github.com/neolord0/hwplib
- github.com/hahnlee/hwp.js(#55·#58·#59), microsoft.com HwpConverter(id=36772·49153)
- store.hancom.com/etc/hwpDownload.do(배포용문서 rev1.2·3.0/HWPML rev1.2), wikidocs.net/book/8956
