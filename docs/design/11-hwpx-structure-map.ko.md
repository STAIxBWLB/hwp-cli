[한국어](11-hwpx-structure-map.ko.md) · [English](11-hwpx-structure-map.md)

# HWPX 구조 지도 (요소 전수 카탈로그 + read/write 대칭성 감사)

이 문서는 HWPX(OWPML) 패키지가 담는 **모든 파트·네임스페이스·XML 요소를 표로 조회**하고,
우리 코드가 그것을 **어떻게 읽고(read) 어떻게 쓰는지(write)**를 요소 단위로 감사한다. 목적은
두 가지다. (1) 재구현자가 "이 요소는 코드 어디서 어떻게 처리되는가"를 한 번에 찾게 하는 색인,
(2) read가 해석하는 것 / read가 버리는 것 / write가 방출하는 것의 **대칭성 갭을 드러내는 감사표**
(→ 무손실 왕복이 깨지는 지점 목록).

## 04·12 문서와의 역할 분담

- [04-hwpx-owpml.ko.md](04-hwpx-owpml.ko.md) — **서브시스템 설계**. 왜 이렇게 파싱하는가, 도형 기하
  변환 규약, 배치 비트 레이아웃, 컨트롤 페이로드 바이트 레이아웃, 왕복 규약의 *근거*. 즉 "어떻게
  동작하는가"를 서사로 설명한다.
- 11(이 문서) — **전수 카탈로그 + 대칭성 감사표**. 요소를 빠짐없이 나열하고 read/write 처리 상태를
  표로 대조한다. 04가 대표 사례로 설명한 규약을 11은 *모든 요소에 대해* 조회 가능하게 편다.
- 12-feature-gaps.md(작성 예정) — 이 문서 §5 표 J의 "미구현"·"정보 소실" 행을 입력으로 받아
  **기능 격차 우선순위·복원 계획**을 다룬다.

## 실측 원칙 (저작권 고지)

OWPML은 KS X 6101(한글 문서 파일 형식) 표준이라 요소·속성 이름 자체를 나열하는 것은 문제되지
않는다. 다만 이 문서는 한컴 스키마 문서를 전재하지 않고 **우리 코드가 실제로 다루는 요소만
실측 기재**한다 — 즉 `crates/hwpx/src/**`의 match 분기와 방출 문자열에 등장하는 것만 표에 오른다.
코드가 건드리지 않는 OWPML 요소(예: `hp:chart` 내부 스키마)는 이름만 언급하고 세부를 옮기지
않는다. 스펙 인용 규약은 [docs/README.ko.md](../README.ko.md)를 따른다.

---

## 1. OPC 패키지 트리 (표 E)

HWPX는 OPC(Open Packaging Conventions) ZIP 아카이브다. 아래는 한글이 저장한 **정품 표본을 언집한
실측 트리**(파일 크기 바이트)로, 문서에 스냅숏을 자체 포함한다(저장소 `scratchpad_hwpx/` 실물에
의존하지 않는다).

```
정품 표본(언집) 트리
├── mimetype                     19 B   application/hwp+zip  (STORED·첫 엔트리)
├── version.xml                 310 B   hv:HCFVersion
├── settings.xml                279 B   ha:HWPApplicationSetting (캐럿 위치)
├── META-INF/
│   ├── container.xml           475 B   ocf:container rootfiles (진입점)
│   ├── container.rdf           867 B   rdf:RDF 패키지 관계
│   └── manifest.xml            134 B   odf:manifest (빈 셸)
├── Contents/
│   ├── content.hpf           1,860 B   opf:package manifest+spine+메타
│   ├── header.xml           42,625 B   글꼴/문자·문단모양/테두리채움/스타일
│   └── section0.xml          3,340 B   본문(문단·표·도형)
├── Preview/
│   ├── PrvText.txt               2 B   미리보기 텍스트(선두 ~1000자)
│   └── PrvImage.png          4,485 B   미리보기 썸네일(PNG)
└── BinData/                    (이 표본엔 없음 — 임베드 이미지 있을 때만)
    └── imageN.{png,jpg,gif,bmp}       개체가 참조하는 원본 바이너리
```

각 파트의 read/write 경로 대응:

| 파트 | 역할 | read 경로 (파일:줄) | write 경로 (파일:줄 / 소스) | 왕복 상태 |
|------|------|--------------------|----------------------------|-----------|
| `mimetype` | 컨테이너 매직(첫 엔트리·STORED) | `package.rs:34` 검증만 | `write/mod.rs:72` `MIMETYPE` 상수 | 상수 |
| `version.xml` | 포맷 버전 | `version_info` 의미 파싱 + **원문 pass-through**(`Document.hwpx_version_xml`) | 원문 재방출(없으면 `VERSION_XML` 상수) | ✅ 원문 보존(2026-07-15, GE-β5) |
| `settings.xml` | 앱 설정(캐럿) | **원문 pass-through**(`Document.hwpx_settings_xml`) | 원문 재방출(없으면 `SETTINGS_XML` 상수) | ✅ 원문 보존(2026-07-15, GE-β5) |
| `META-INF/container.xml` | OCF rootfiles(진입점) | **없음** | `write/mod.rs:87` `CONTAINER_XML` 상수 | 상수 |
| `META-INF/container.rdf` | RDF 패키지 관계 | **없음** | `write/mod.rs:83` `CONTAINER_RDF` 상수 | 상수(header+section0만 기술) |
| `META-INF/manifest.xml` | ODF manifest(빈 셸) | **없음** | `write/mod.rs:92` `MANIFEST_XML` 상수 | 상수 |
| `Contents/content.hpf` | OPF 패키지·문서 메타 | `read/mod.rs:90` `parse_content_meta` (title/creator/subject/keywords) | `templates.rs:28` `content_hpf()` | 메타 4종만 왕복, spine/manifest 재합성 |
| `Contents/header.xml` | 글꼴·모양·스타일 테이블 | `read/header.rs:99` `parse_header` | `write/header.rs:44` `write_header` | §3.2/§4 참조 |
| `Contents/section{i}.xml` | 본문(문단·표·도형) | `read/section.rs:52` `parse_section` | `write/section.rs:50` `write_section` | §3.1/§4 참조 |
| `BinData/imageN.*` | 임베드 이미지 원본 | `read/mod.rs:47` → `BinStream` | `write/mod.rs:110` ← `BinCollector`(`section.rs:24`) | 바이트 보존(중복 제거) |
| `Preview/PrvText.txt` | 미리보기 텍스트 | **없음** | `write/mod.rs:113` `doc.plain_text()` 선두 1000자 | 본문에서 재생성 |
| `Preview/PrvImage.png` | 미리보기 썸네일 | **없음** | **없음(IR 경로 손실)** | `patch.rs`만 raw 복사로 보존 |

**쓰기 엔트리 순서**(`write/mod.rs:72‑114`, 왼쪽이 먼저): `mimetype`(STORED) → `version.xml` →
`META-INF/container.rdf` → `container.xml` → `manifest.xml` → `Contents/content.hpf` → `header.xml` →
`section{0..}.xml` → `BinData/*` → `Preview/PrvText.txt` → `settings.xml`. `mimetype`은 반드시 첫
로컬 헤더 + 무압축이어야 한다(OPC 규약; 위반 시 한글이 손상 파일로 판단).

**읽기 진입**(`read/mod.rs:24` `read_document`): header → section(수치 정렬 `section0<…<section10`,
`package.rs:105`) → BinData → version → content.hpf 순. `settings.xml`·`Preview/*`·`META-INF/*`는
읽기 경로가 없어 IR로 들어오지 않는다 — IR 왕복 시 write가 상수/재생성으로 채운다.

---

## 2. 네임스페이스 표 (표 F)

파서는 모든 요소를 `local_name()`(접두사 제거) 기준으로 매칭하므로(§3.3) 접두사는 *방출* 쪽에서만
의미가 있다. 방출 접두사는 두 곳에서 나온다: 본문 섹션 루트(`write/section.rs:59`)의 3종, 그리고
`FULL_XMLNS`(`templates.rs:25`, header/content.hpf가 선언하는 전체 15종). 패키지 보조 파일은
각자 다른 계열을 쓴다.

| 접두사 | URI | 용도 | 등장 파일 | 방출 요소 有無 |
|--------|-----|------|-----------|:---:|
| `ha` | `…/hwpml/2011/app` | 앱 설정 루트 | settings.xml, (FULL_XMLNS) | 有(settings) |
| `hp` | `…/hwpml/2011/paragraph` | 문단·런·컨트롤·표·도형 | section*.xml, header 선언 | 有 |
| `hp10` | `…/hwpml/2016/paragraph` | 2016 확장 문단 | (FULL_XMLNS) | 선언만 |
| `hs` | `…/hwpml/2011/section` | 섹션 루트 `hs:sec` | section*.xml | 有 |
| `hc` | `…/hwpml/2011/core` | 코어 기하·색·행렬 | section*.xml, header.xml | 有 |
| `hh` | `…/hwpml/2011/head` | header 루트 `hh:head` | header.xml | 有 |
| `hhs` | `…/hwpml/2011/history` | 편집 이력 | (FULL_XMLNS) | 선언만 |
| `hm` | `…/hwpml/2011/master-page` | 바탕쪽 | (FULL_XMLNS) | 선언만 |
| `hpf` | `…/schema/2011/hpf` | hpf 패키지 스키마 | content.hpf, container.xml | 선언만 |
| `dc` | `http://purl.org/dc/elements/1.1/` | 더블린코어 메타 | content.hpf | 有(creator/subject) |
| `opf` | `http://www.idpf.org/2007/opf/` | OPF 패키지 | content.hpf | 有 |
| `ooxmlchart` | `…/hwpml/2016/ooxmlchart` | 차트 | (FULL_XMLNS) | 선언만 |
| `hwpunitchar` | `…/hwpml/2016/HwpUnitChar` | 단위 문자 | (FULL_XMLNS) | 선언만 |
| `epub` | `http://www.idpf.org/2007/ops` | EPUB 상호운용 | (FULL_XMLNS) | 선언만 |
| `config` | `urn:oasis:…:config:1.0` | ODF 설정 | settings.xml, (FULL_XMLNS) | 有(settings) |

패키지 계열(보조 파일 전용, `templates.rs`):

| 접두사 | URI | 등장 파일 |
|--------|-----|-----------|
| `hv` | `…/hwpml/2011/version` | version.xml (루트 `hv:HCFVersion`) |
| `ocf` | `urn:oasis:…:container` | container.xml (루트 `ocf:container`) |
| `rdf` | `http://www.w3.org/1999/02/22-rdf-syntax-ns#` | container.rdf |
| `ns0`(pkg#) | `…/hwpml/2016/meta/pkg#` | container.rdf (`hasPart`, `HeaderFile`/`SectionFile`/`Document` 타입) |
| `odf` | `urn:oasis:…:manifest:1.0` | manifest.xml (루트 `odf:manifest`) |

**감사 포인트:** `FULL_XMLNS`는 15종을 선언하지만 실제 요소를 방출하는 것은 `hp/hs/hc/hh/dc/opf`
(+settings의 `ha/config`)뿐이다. `hp10/hhs/hm/hpf/ooxmlchart/hwpunitchar/epub`은 **선언만 하고
방출 요소가 없다** — 즉 2016 확장·이력·바탕쪽·차트를 우리가 생성하지 않는다는 뜻(→ §5 미구현).

---

## 3. read 요소 카탈로그

### 3.1 section.xml (표 G)

처리 상태 4단계:
- **의미파싱** — 속성을 IR 필드로 온전히 해석.
- **부분파싱** — 일부 속성만 해석(나머지 무시). 셀 옆에 무엇을 빠뜨렸는지 표기.
- **fallback 보존** — 미지원 요소를 `GenericControl`로 감싸 텍스트(subList)만 재귀 보존.
- **skip** — 서브트리를 소비하고 버림(정보 소실).

근거는 `read/section.rs`의 실제 줄번호. 아래 표는 각 파서 함수의 **모든 요소 분기**를 담는다
(match 분기 전수 대조 결과는 문서 말미 참조).

#### parse_section / parse_paragraph (`hp:p` 및 그 자식)

| 요소(로컬명) | 부모 | 읽는 속성 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|---|
| `p` | `hs:sec`/`tc`/`subList` | paraPrIDRef, styleIDRef, pageBreak, columnBreak | `Paragraph`(para_shape/style/break_type) | 의미파싱 | :59, :78‑88 |
| `run` | `p` | charPrIDRef | `char_shape_runs`(같은 pos 덮어씀) | 의미파싱 | :99 |
| `t` | `p` | (텍스트/엔티티/lineBreak) | `HwpChar::Text` 열 | 의미파싱 | :117 → `parse_text` :243 |
| `tab` | `p` | — | `InlineCtrl{9}` (8 WCHAR) | 의미파싱 | :122 |
| `lineBreak` | `p` | — | `CharCtrl(10)` | 의미파싱 | :132 |
| `secPr` | `p` | (자식 pagePr/margin) | `ExtCtrl(2,secd)`+`SectionDef` | 부분파싱(grid/note/border 무시) | :136 → `parse_sec_pr` :312 |
| `ctrl` | `p` | (자식별 분기) | — | 재귀 | :149 → `parse_ctrl` :504 |
| `tbl` | `p` | rowCnt/colCnt/cellSpacing/pageBreak/repeatHeader/noAdjust/borderFillIDRef/zOrder | `ExtCtrl(11,tbl )`+`Table` | 의미파싱 | :154 → `parse_table` :691 |
| `equation` | `p` | script + sz/pos | `ExtCtrl(11,eqed)`+`Generic{equation}` | 부분파싱(스크립트 원문만) | :159 → `parse_equation` :1130 |
| `linesegarray` | `p` | lineseg* | `para.line_segs` | 의미파싱 | :173 → `parse_linesegs` :1255 |
| `pic` | `p` | zOrder(시작태그) + 자식 | `ExtCtrl(11,gso )`+`Picture` | 의미파싱 | :178 → `parse_picture` :889 |
| `rect`/`ellipse`/`line`/`polygon`/`curve`/`arc` | `p` | (도형 기하) | `ExtCtrl(11,ctrl_id)`+`Generic{gso_shapes}` | 의미파싱 | :191 → `collect_shape` :995 (`shape_kind` :981) |
| *그 외 개체*(container/textart 등) | `p` | subList만 | `ExtCtrl(11,ctrl_id)`+`Generic{paragraph_lists}` | fallback 보존 | :191 → `collect_sub_lists` :933 |

#### parse_text (`hp:t` 내부)

| 이벤트 | 읽는 것 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|
| `Text` | 문자열(UTF-16 계수) | `HwpChar::Text` | 의미파싱 | :251 |
| `GeneralRef` | `&amp; &lt; &gt; &quot; &apos;`·수치 참조 | `HwpChar::Text` | 의미파싱 | :262 |
| `lineBreak` | — | `CharCtrl(10)` | 의미파싱 | :281 |

#### parse_sec_pr (`hp:secPr` 자식)

| 요소 | 읽는 속성 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|
| `pagePr` | width, height, landscape | `PageDef`(크기·attr bit0) | 부분파싱 | :335 |
| `margin` | left/right/top/bottom/header/footer/gutter | `PageDef` 여백 | 의미파싱 | :344 |
| *그 외*(grid/startNum/visibility/footNotePr/endNotePr/pageBorderFill/lineNumberShape) | — | — | **skip(무시)** | :353 `_ => {}` |

#### parse_ctrl (`hp:ctrl` 자식)

| 요소 | ctrl_id/코드 | 읽는 속성 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|---|
| `fieldBegin` | (type→id) / 3 | type, name, 자식 Command | `Generic`+CTRL_DATA(0x0057) | 의미파싱 | :516 |
| `fieldEnd` | — / 4 | (LIFO 매칭) | `InlineCtrl(4)` 역순 ctrl_id | 의미파싱 | :550 |
| `bookmark` | `bokm` / 22 | name | `Generic`+이름 CTRL_DATA | 의미파싱 | :562 |
| `colPr` | `cold` / 2 | type, layout, colCount, sameSz, sameGap | `Generic{column_def}` | 부분파싱(colSz/colLine 미수집) | :586 → `parse_col_pr` :377 |
| `header` | `head` / 16 | applyPageType, id + subList | `Generic`+8B 페이로드 | 의미파싱 | :587 → `head_foot_data` :399 |
| `footer` | `foot` / 16 | applyPageType, id + subList | `Generic`+8B 페이로드 | 의미파싱 | :588 |
| `footNote` | `fn  ` / 17 | (subList) | `Generic` | 부분파싱(페이로드 없음) | :589 |
| `endNote` | `en  ` / 17 | (subList) | `Generic` | 부분파싱 | :590 |
| `autoNum` | `atno` / 18 | — | `Generic`+상수 12B | 부분파싱(표준값) | :593 → `build_atno` :465 |
| `pageNum` | `pgnp` / 21 | pos, sideChar | `Generic`+12B | 부분파싱(format=DIGIT만) | :594 → `build_pgnp` :415 |
| `pageHiding` | `pghd` / 21 | hideHeader/Footer/MasterPage/Border/Fill/PageNum | `Generic`+4B 비트맵 | 의미파싱 | :595 → `build_pghd` :446 |
| `newNum` | `nwno` / 21 | num | `Generic`+6B | 부분파싱(종류=PAGE 고정) | :596 → `build_nwno` :475 |
| *그 외 ctrl 자식* | (id) / 21 | subList만 | `Generic` | fallback 보존 | :597 `other` |
| `stringParam[name=Command]` | — | 텍스트 | 필드 command | 의미파싱 | `read_field_command` :643 |

#### parse_table (`hp:tbl` 자식) / parse_cell (`hp:tc` 자식)

| 요소 | 부모 | 읽는 속성 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|---|
| `tc` | `tbl` | header, borderFillIDRef | `Cell` | 재귀 | :738 → `parse_cell` :802 |
| `tr` | `tbl` | — | (컨테이너; row는 cellAddr로 복원) | **skip(무시)** | :742 |
| `inMargin` | `tbl` | left/right/top/bottom | `Table.inner_margins` | 의미파싱 | :750 |
| `pos` | `tbl` | treatAsChar/relTo/align/offset/flow 등 | `GsoPlacement` | 의미파싱 | :760 |
| `sz` | `tbl` | width, height | `GsoPlacement` | 의미파싱 | :772 |
| `outMargin` | `tbl` | left/right/top/bottom | `GsoPlacement.out_margins` | 의미파싱 | :776 |
| *그 외 tbl 자식* | `tbl` | — | — | **skip(subtree 소비)** | :743 `_ => skip_subtree` |
| `cellAddr` | `tc` | colAddr, rowAddr | `Cell.col/row` | 의미파싱 | :829 |
| `cellSpan` | `tc` | colSpan, rowSpan | `Cell.col_span/row_span` | 의미파싱 | :833 |
| `cellSz` | `tc` | width, height | `Cell.width/height` | 의미파싱 | :837 |
| `cellMargin` | `tc` | left/right/top/bottom | `Cell.margins` | 의미파싱 | :841 |
| `subList` | `tc` | vertAlign | `Cell.list_attr` bits5‑6 | 부분파싱(vertAlign만) | :849 |
| `p` | `tc` | (문단) | `Cell.paragraphs` | 재귀 | :861 |
| *그 외 tc 자식* | `tc` | — | — | **skip(무시)** | :864 `_ => {}` |

#### parse_picture (`hp:pic` 자식) / collect_shape (도형 자식) / parse_equation / parse_gradation / parse_linesegs

| 요소 | 부모 | 읽는 속성 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|---|
| `sz` | `pic` | width, height | `Picture.width/height` | 의미파싱 | :897 |
| `pos` | `pic` | treatAsChar, vertOffset, horzOffset | `Picture`(treat/offset) | 부분파싱(relTo 미수집) | :901 |
| `img` | `pic` | binaryItemIDRef | `Picture.bin_ref` | 의미파싱 | :909 |
| *그 외 pic 자식*(imgRect/imgClip/imgDim/renderingInfo/img 효과) | `pic` | — | — | **skip(무시)** | :916 `_ => {}` |
| `pos` | 도형 | horzOffset, vertOffset, treatAsChar | `ShapeGeom.x/y/anchored` | 의미파싱 | :1014 |
| `sz` | 도형 | width, height | `ShapeGeom.w/h` | 의미파싱 | :1021 |
| `lineShape` | 도형 | color, width, style, headStyle, tailStyle | 테두리 필드 | 의미파싱 | :1025 |
| `winBrush` | 도형(fillBrush) | faceColor | `ShapeGeom.fill` | 의미파싱 | :1040 |
| `pt0…ptN` | Polygon/Curve | x, y | `ShapeGeom.points` | 의미파싱 | :1047 |
| `center`/`ax1`/`ax2` | Arc | x, y | `ShapeGeom.points`(3점) | 의미파싱 | :1054 |
| `gradation` | 도형(fillBrush) | type, angle, color* | `fill_gradient` | 부분파싱(각도 근사) | :1085 → `parse_gradation` :1217 |
| `subList` | 도형 | (문단) | `paragraph_lists` | 재귀 | :1068 |
| *그 외 도형 자식*(shadow/outMargin/renderingInfo·Rect/Ellipse/Arc의 pt) | 도형 | — | — | **skip(무시)** | :1059 `_ => {}` |
| `script` | equation | 텍스트 | `Equation.script` | 의미파싱 | :1145 |
| `sz` | equation | width, height | `Equation.width/height` | 의미파싱 | :1147 |
| `pos` | equation | treatAsChar, offset | `Equation.inline/x/y` | 의미파싱 | :1150 |
| `color` | gradation | value | stops | 의미파싱 | :1228 |
| `lineseg` | linesegarray | textpos/vertpos/vertsize/textheight/baseline/spacing/horzpos/horzsize/flags | `LineSeg` | 의미파싱 | :1258 |

### 3.2 header.xml (표 H)

`parse_header`는 단일 스트리밍 루프에서 컨텍스트 변수(`current_char`/`current_para`/
`current_border`/`current_numbering`)에 누적한다. 근거는 `read/header.rs` 줄번호.

| 요소(로컬명) | 부모 컨텍스트 | 읽는 속성 | IR 대상 | 상태 | 근거 |
|---|---|---|---|---|---|
| `fontface` | refList | lang | `current_lang` 슬롯(7언어) | 의미파싱 | :125 (`lang_slot` :58) |
| `font` | fontface | face | `fonts[slot]` `FaceName` | 의미파싱 | :132 |
| `typeInfo` | font | (모든 속성 원문) | `FaceName.type_info` | fallback 보존(속성 문자열) | :440 |
| `charPr` | charProperties | height, textColor, shadeColor, useFontSpace, useKerning, borderFillIDRef | `CharShape` | 의미파싱 | :140 |
| `fontRef` | charPr | hangul…user | `CharShape.face_ids` | 의미파싱 | :166 |
| `ratio` | charPr | 언어별 | `CharShape.ratios` | 의미파싱 | :178 |
| `spacing` | charPr | 언어별 | `CharShape.spacings` | 의미파싱 | :178 |
| `relSz` | charPr | 언어별 | `CharShape.rel_sizes` | 의미파싱 | :178 |
| `offset` | charPr | 언어별 | `CharShape.offsets` | 의미파싱 | :178 |
| `bold` | charPr | — | attr bit1 | 의미파싱 | :194 |
| `italic` | charPr | — | attr bit0 | 의미파싱 | :199 |
| `underline` | charPr | type, shape, color | attr bits2‑3, underline_shape, underline_color | 의미파싱 | :204 |
| `strikeout` | charPr | shape | attr bit18, strike | 부분파싱(NONE·3D는 비취소선) | :218 |
| `supscript` | charPr | — | attr bit15 | 의미파싱(write 대칭 — 2026-07-15) | :234 |
| `subscript` | charPr | — | attr bit16 | 의미파싱(write 대칭) | :239 |
| `shadow` | charPr | type, color, offsetX, offsetY | attr bit11, shadow_color/gap | 의미파싱(write 대칭) | :245 |
| `outline` | charPr | type | attr bit8 | 부분파싱(유무만 — write 대칭 SOLID/NONE) | :259 |
| `emboss` | charPr | — | attr bit13 | 의미파싱(write 대칭) | :266 |
| `engrave` | charPr | — | attr bit14 | 의미파싱(write 대칭) | :271 |
| `paraPr` | paraProperties | snapToGrid, condense, fontLineHeight, tabPrIDRef | `ParaShape.attr1/tab_def_id` | 의미파싱 | :276 |
| `align` | paraPr | horizontal | attr1 bits2‑4 | 의미파싱 | :301 (`alignment_code` :87) |
| `heading` | paraPr | type, level, idRef | attr1 bits23‑27, numbering_id | 의미파싱 | :309 |
| `intent`/`left`/`right`/`prev`/`next` | paraPr>margin | value(×2 단위) | `ParaShape` 여백 | 의미파싱 | :356 |
| `margin`(End) | paraPr | — | `para_margin_done`(첫 분기만 취함) | 제어 | :515 |
| `lineSpacing` | paraPr | type, value | line_spacing_type/line_spacing(_old) | 의미파싱 | :375 |
| `breakSetting` | paraPr | breakLatinWord, breakNonLatinWord, widowOrphan, keepWithNext, keepLines, pageBreakBefore | attr1 여러 비트 | 의미파싱 | :404 |
| `border` | paraPr | borderFillIDRef | `ParaShape.border_fill_id` | 의미파싱 | :432 |
| `numbering` | numberings | — | `current_numbering` | 의미파싱 | :326 |
| `paraHead` | numbering | level, start, numFormat + 텍스트 | `NumLevel`(fmt/template) | 의미파싱 | :333 (`num_fmt` :72) |
| `bullet` | (refList) | char | `bullet_chars` | 의미파싱 | :350 |
| `borderFill` | borderFills | — | `current_border` | 의미파싱 | :454 |
| `slash`/`backSlash` | borderFill | type | attr bit2/bit5 | 부분파싱(유무만) | :464 |
| `leftBorder`/`rightBorder`/`topBorder`/`bottomBorder` | borderFill | type, width, color | `BorderFill.sides` | 의미파싱 | :475 (`parse_border_line` :49) |
| `diagonal` | borderFill | type, width, color | `BorderFill.diagonal` | 의미파싱 | :486 |
| `winBrush` | borderFill(fillBrush) | faceColor | `BorderFill.bg_color`, fill_type bit0 | 의미파싱 | :491 |
| `style` | styles | name, engName, paraPrIDRef, charPrIDRef, nextStyleIDRef, langID | `Style` | 의미파싱 | :499 |
| *그 외*(beginNum/compatibleDocument/docOption/linkinfo/autoSpacing…) | — | — | — | **skip(무시)** | :510 `_ => {}` |

### 3.3 로컬명 매칭 정책과 그 함의

**정책:** 모든 매칭은 `e.local_name()`(네임스페이스 접두사 제거) 기준이다(`read/xml.rs:6` `attr`,
그리고 위 두 파서의 모든 `match e.local_name().as_ref()`). 따라서 `hp:p`든 `p`든 로컬명 `p`로
매칭된다 — 접두사 종류·재정의에 무관.

**함의 1 (강건성):** 접두사가 문서마다 달라도(정품이 `hp:`를 쓰지 않고 다른 접두사로 재선언해도)
파서가 깨지지 않는다.

**함의 2 (충돌 위험):** 접두사가 다른데 로컬명이 같은 요소는 구분되지 않는다. 실제로 `hc:winBrush`
(도형 채움)와 `hh:winBrush`(테두리채움 배경)가 로컬명 `winBrush`로 같지만, 서로 다른 파서
컨텍스트(collect_shape vs parse_header)에 있어 충돌하지 않는다. `sz`/`pos`도 tbl/pic/도형/equation
각 컨텍스트에서 지역적으로만 해석된다.

**미매칭 요소의 두 갈래:**
- **fallback 보존** — `parse_paragraph`의 `_ =>`(`section.rs:191`)와 `parse_ctrl`의 `other`
  (`:597`)는 미지원 요소를 `GenericControl`(원래 로컬명 4바이트를 ctrl_id로)로 감싸고, 자식
  `subList` 문단을 재귀 수집한다. **텍스트는 살아남지만** 개체 고유 속성(차트 데이터, OLE 등)은
  버려진다. write에서 gso_shapes도 paragraph_lists도 없으면 DROP(§4·§5).
- **skip(정보 소실)** — 관심 없는 요소를 `_ => {}`(이벤트 무시) 또는 `skip_subtree`(서브트리
  통째 소비)로 버린다. 표 G/H에서 **skip**으로 표시한 행 전부가 여기 해당한다. IR에 흔적을
  남기지 않으므로 왕복 시 write가 상수로 재합성하거나 그냥 사라진다.

---

## 4. write 방출 카탈로그 (표 I)

`write/section.rs`는 **89종**, `write/header.rs`는 **51종** 유니크 요소를 방출한다(접두사 포함
grep 실측). 개별 나열 대신 계열로 묶는다. read가 만들지 못하는(상수) 요소는 *상수*로 표기.

### 4.1 write/section.rs (89종)

| 계열 | 방출 요소 | 소스 함수 |
|---|---|---|
| 섹션 루트 | `hs:sec` | `write_section` :59 |
| 문단·런·텍스트 | `hp:p`, `hp:run`, `hp:t`, `hp:tab`, `hp:lineBreak` | `write_paragraph` :116, `flush_text` :410 |
| 구역 정의(상수 다수) | `hp:secPr`, `hp:grid`, `hp:startNum`, `hp:visibility`, `hp:lineNumberShape`, `hp:pagePr`, `hp:margin`, `hp:footNotePr`, `hp:endNotePr`, `hp:autoNumFormat`, `hp:noteLine`, `hp:noteSpacing`, `hp:numbering`, `hp:placement`, `hp:pageBorderFill` | `write_default_sec_pr` :450 |
| 다단 | `hp:colPr`(+`hp:ctrl` 래퍼) | `write_col_ctrl` :473 |
| 머리말/꼬리말 | `hp:header`/`hp:footer`(로컬명 방출), `hp:subList` | `write_header_footer` :500 |
| 페이지 컨트롤 | `hp:pageNum`, `hp:pageHiding`, `hp:newNum`, `hp:autoNum` | `write_paragraph` arms :292‑344 |
| 필드·책갈피 | `hp:fieldBegin`, `hp:fieldEnd`, `hp:parameters`, `hp:stringParam`, `hp:bookmark` | :256‑291 |
| 표 | `hp:tbl`, `hp:tr`, `hp:tc`, `hp:cellAddr`, `hp:cellSpan`, `hp:cellSz`, `hp:cellMargin`, `hp:inMargin`, `hp:outMargin`, `hp:sz`, `hp:pos` | `write_table` :972 |
| 도형 공통 스캐폴드(상수) | `hp:offset`, `hp:orgSz`, `hp:curSz`, `hp:flip`, `hp:rotationInfo`, `hp:renderingInfo`, `hc:transMatrix`, `hc:scaMatrix`, `hc:rotMatrix` | `write_obj_scaffold` :609 |
| 도형 요소 | `hp:rect`, `hp:ellipse`, `hp:line`, `hp:polygon`, `hp:curve`, `hp:arc`, `hp:connectLine`(계수용) | `write_shape_element` :692 |
| 도형 스타일·채움 | `hp:lineShape`, `hc:fillBrush`, `hc:winBrush`, `hc:gradation`, `hc:color`, `hp:shadow` | :741‑781 |
| 도형 기하점 | `hc:startPt`, `hc:endPt`, `hc:pt`/`hc:pt0..3`, `hc:center`, `hc:ax1`, `hc:ax2`, `hc:start1`, `hc:end1`, `hc:start2`, `hc:end2` | :787‑839 |
| 글상자 텍스트 | `hp:drawText`, `hp:subList`, `hp:textMargin` | `write_draw_text` :622 |
| 그림 | `hp:pic`, `hc:img`, `hp:imgRect`, `hp:imgClip`, `hp:imgDim` | `write_picture` :1060 |
| 줄배치(옵션) | `hp:linesegarray`, `hp:lineseg` | :381 |

### 4.2 write/header.rs (51종)

| 계열 | 방출 요소 | 소스 함수 |
|---|---|---|
| 루트·구조 | `hh:head`, `hh:beginNum`, `hh:refList`, `hh:compatibleDocument`, `hh:layoutCompatibility`, `hh:docOption`, `hh:linkinfo` | `write_header` :44 |
| 글꼴 | `hh:fontfaces`, `hh:fontface`, `hh:font`, `hh:typeInfo` | `write_fontfaces` :76 |
| 테두리채움 | `hh:borderFills`, `hh:borderFill`, `hh:slash`, `hh:backSlash`, `hh:leftBorder`/`rightBorder`/`topBorder`/`bottomBorder`, `hh:diagonal`, `hc:fillBrush`, `hc:winBrush` | `write_border_fills` :130 |
| 문자모양 | `hh:charProperties`, `hh:charPr`, `hh:fontRef`, `hh:ratio`, `hh:spacing`, `hh:relSz`, `hh:offset`, `hh:italic`, `hh:bold`, `hh:underline`, `hh:strikeout`, `hh:outline`, `hh:shadow`, `hh:emboss`, `hh:engrave`, `hh:supscript`, `hh:subscript` (2026-07-15부터 전부 IR 기반) | `write_char_properties` :184 |
| 탭 | `hh:tabProperties`, `hh:tabPr` | `write_tab_properties` :263 |
| 번호·글머리 | `hh:numberings`, `hh:numbering`, `hh:paraHead`, `hh:bullets`, `hh:bullet`(PR #8 신설 — 이전엔 글머리 정의가 쓰기에서 소실) | `write_numberings`, `write_bullets` |
| 문단모양 | `hh:paraProperties`, `hh:paraPr`, `hh:align`, `hh:heading`, `hh:breakSetting`, `hh:autoSpacing`, `hh:margin`, `hc:intent`/`left`/`right`/`prev`/`next`, `hh:lineSpacing`, `hh:border` | `write_para_properties` :291 |
| 스타일 | `hh:styles`, `hh:style` | `write_styles` :346 |

**감사 포인트:** `write_default_sec_pr`의 상당 요소는 IR과 무관한 **고정 상수 템플릿**이다
(각주/미주/페이지테두리 상수). 즉 이 요소들은 "유효한 문서"를 위한 채움이지 왕복 보존이 아니다.
`write_numberings`는 2026-07-15부터 `numbering_levels`가 있으면 IR 기반으로 방출하고, 없을
때(hwp5 경로)만 기존 `^{level}.` 상수로 채운다.

---

## 5. read↔write 대칭성 매트릭스 (표 J)

무손실 왕복이 깨지는 지점을 세 부류로 감사한다. 이 표가 12번 갭 문서의 입력이다.

### (a) write만 방출·read 미해석 — 왕복 시 write가 재합성

read가 버리므로(§3의 skip) IR엔 없다. write가 sz/좌표 등에서 재계산하거나 상수로 채운다.

| 요소 | write 방출 | read | 왕복 영향 | 근거 |
|---|---|---|---|---|
| `hp:offset`, `hp:orgSz`, `hp:flip`, `hp:rotationInfo` | 상수(0,0)/(w,h)/각도0 | skip | 회전·플립 미보존(항상 0) | `write/section.rs:609` |
| `hp:renderingInfo`+`hc:transMatrix`/`scaMatrix`/`rotMatrix` | 항등행렬 상수 | skip | 변환행렬 항등 재생성 | :612 |
| `hp:curSz` | Ellipse/Arc=(0,0), 그 외=(w,h) | skip | 정품 실측값 재합성 | :736 |
| `hc:pt0~3`(Rect), `hc:center`/`ax1`/`ax2`/`start*`/`end*`(Ellipse) | sz에서 bbox 재계산 | **무시**(Rect/Ellipse의 pt) | sz로 재합성(중복 방지) | :805, :813 |
| `hp:shadow type="NONE"` | 상수 | skip | 도형 그림자 상수 | :781 |
| `hp:imgRect`, `hp:imgClip`, `hp:imgDim` | bbox 상수 | skip(parse_picture) | 이미지 크롭·치수 재합성 | :1079 |
| `hp:drawText`>`hp:textMargin` | 여백 283 상수 | 텍스트(subList)만 수집 | 글상자 여백 상수화 | :622 |
| lineShape `headfill`/`tailfill`/`headSz`/`tailSz`/`endCap`/`outlineStyle`/`alpha` | 상수 | color/width/style/head/tail만 | 화살표 크기·꼬리 상수 | :748 |
| `hp:pageBorderFill`, `hp:footNotePr`, `hp:endNotePr`, `hp:grid`, `hp:startNum`, `hp:visibility`, `hp:lineNumberShape` | 원문 있으면 원문 재방출, 없으면 구역 상수 | **원문 XML pass-through**(`secpr_raw_children`) | ✅ **해소(2026-07-15, GC-5)** — hwpx 왕복에서 원문·순서 보존(의미 파싱은 아님) | `parse_sec_pr` ↔ `write_default_sec_pr` |
| `hh:beginNum`, `hh:compatibleDocument`, `hh:docOption`, `hh:autoSpacing` | 상수 | skip(header) | 호환·문서옵션 상수 | `write/header.rs:51,71` |

### (b) read만 해석·write는 상수/근사/미방출 — 읽었으나 hwpx로 되돌리지 못함

IR엔 값이 있으나(hwp5로는 나감) hwpx write가 상수/근사로 눌러 **hwpx→hwpx 왕복에서 손실**된다.

| 요소/속성 | read 해석 | write | 왕복 영향(hwpx→hwpx) | 근거 |
|---|---|---|---|---|
| charPr `shadow` | attr bit11 + color/offset | ✅ IR 기반(`DROP`+color/offset) | **해소(2026-07-15)** | 읽기 `header.rs:245` ↔ `write_char_properties` |
| charPr `outline` | attr bit8 | ✅ 유무 기반 `SOLID`/`NONE` | **해소(2026-07-15)** | `:259` ↔ 동상 |
| charPr `emboss`/`engrave` | attr bit13/14 | ✅ 켜진 것만 방출 | **해소(2026-07-15)** | `:266,:271` ↔ 동상 |
| charPr `supscript`/`subscript` | attr bit15/16 | ✅ 켜진 것만 방출 | **해소(2026-07-15)** | `:234,:239` ↔ 동상 |
| `hh:underline shape` | type/**shape**/color 해석(IR `underline_shape` 신설) | ✅ `underline_shape` 기반(0=SOLID) | **해소(2026-07-15)** | `:204` ↔ 동상 |
| colPr `colSz`/`colLine`(단별폭·구분선) | 미수집(등폭 가정) | 값 방출하나 단별폭 없음 | 불균등 단·구분선 손실 | `parse_col_pr :377` ↔ `write_col_ctrl :473` |
| `hc:gradation angle`·중심·step | angle만(라디안 근사) | angle round + centerX/Y/step 상수 | 그러데이션 중심·단계 근사 | `parse_gradation :1217` ↔ `:764` |
| `hp:pagePr landscape` | attr bit0 | default_sec_pr에서 재방출 | 보존(단 secPr 다른 상수와 함께) | `:340` ↔ `:453` |
| numbering `paraHead` 형식 | template/start/numFormat 수집 | ✅ `numbering_levels` 기반(없으면 기존 상수) | **해소(2026-07-15)** — 다중 번호정의 itemCnt 뭉개짐도 함께 수정 | `:333` ↔ `write_numberings` |
| tab `tabPr`(위치·채움) | `tabPr/tabItem` 의미 파싱 → IR `TabDef` | ✅ `tab_stops` 기반 방출(없으면 기존 상수) | **해소(2026-07-15, GC-4)** | `read/header.rs` ↔ `write_tab_properties` |
| paraPr `heading`(문단↔번호 연결) | attr1 bits23‑27 + numbering_id | ✅ OUTLINE/NUMBER/BULLET 역방출 | **해소(2026-07-15 2차)** — [12](12-feature-gaps.ko.md) GE-α8. PR #8(07-18)이 OUTLINE idRef +1 밀림 실기 버그 추가 수정(정품 idRef=0) + 정의 id 비연속 관용화 | `:309` ↔ `write_para_properties` |

### (c) 양쪽 없음(미구현) — read·write 모두 의미 처리 없음

| 개체/요소 | 현재 처리 | 근거 |
|---|---|---|
| `hp:chart`(ooxmlchart) | read fallback(텍스트 없음)→write DROP | 네임스페이스 선언만(§2) |
| OLE 개체(`hp:ole` 등) | read fallback→write DROP | `collect_sub_lists` :933 → DROP `write/section.rs:364` |
| `hp:video`/미디어 | read fallback→write DROP | 동일 |
| `hp:container`(그룹 개체) | read fallback(subList 텍스트만)→write DROP | :191 → :364 |
| `hp:textart` | read fallback(텍스트만)→write DROP | 동일 |
| `hp:formObject`(양식 개체) | read fallback→write DROP | 동일 |
| `hp:compose`/`hp:dutmal`(겹침·덧말) | read fallback→write DROP | 동일 |
| 찾아보기 표식(idxm)·홀짝 조정(pgct)·숨은설명(tcmt) — hwp5 유입 Generic | read fallback(4B ctrl_id 합성, `read/section.rs:704-710`)→write DROP | 2026-07-19 감사 보완 — tcmt는 문단 리스트째 드롭이라 콘텐츠 손실([12](12-feature-gaps.ko.md) GF-5), pgct는 hwpx 대응 요소명 자체 미확인 |
| 바탕쪽(`hm:` master-page) | read·write 모두 없음 | 네임스페이스 선언만 |
| 편집 이력(`hhs:`) | read·write 모두 없음 | 네임스페이스 선언만 |

**주의(범주 (c)의 뉘앙스):** `container`/`textart`/`formObject`/`compose`는 read가 **완전 무시가
아니라** `collect_sub_lists`로 자식 `subList` 문단을 `GenericControl.paragraph_lists`에 담는다 —
즉 *텍스트는 IR에 남는다*. 그러나 write에서 이 Generic은 gso_shapes도 없고 알려진 ctrl_id도
아니므로 최종 `Control::Generic(g) => warnings.push("DROP…")`(`write/section.rs:364`)로 드롭되며,
담아둔 텍스트까지 함께 사라진다. `chart`/`ole`/`video`는 텍스트조차 없어 완전 소실이다.

---

## 6. OWPML 열거값 ↔ hwp5 코드 변환표

코드에 실재하는 변환 함수들의 매핑. read 함수(OWPML 문자열→코드)와 write 함수(코드→OWPML
문자열)가 쌍을 이룬다. 근거는 `read/*.rs`·`write/*.rs`의 해당 함수.

### 배치·기준 (section)

| 축 | OWPML 값 → 코드 | read | write(역) |
|---|---|---|---|
| `vertRelTo` | PAPER=0, PAGE=1, PARA=2 | `vert_rel_to_code` :663 | `vert_rel_to_name` :565 |
| `horzRelTo` | PAPER=0, PAGE=1, COLUMN=2, PARA=3 | `horz_rel_to_code` :672 | `horz_rel_to_name` :572 |
| `vertAlign` | TOP=0, CENTER=1, BOTTOM=2 | `align_code` :682 | `vert_align_name` :580 |
| `horzAlign` | LEFT=0, CENTER=1, RIGHT=2 | `align_code` :682 | `horz_align_name` :587 |

### 선·테두리

| 축 | OWPML 값 → 코드 | read | write(역) |
|---|---|---|---|
| 도형 선종류(lineShape `style`) | SOLID=0, DASH=1, DOT=2, DASH_DOT=3, DASH_DOT_DOT=4, LONG_DASH=5 | `line_style_code` :1195 | `line_style_name` :594 |
| 화살촉(head/tailStyle) | NORMAL/NONE/""=0, 그 외=1 | `arrow_code` :1207 | `arrow_name` :604 |
| 테두리선(borderFill `type`) | NONE=0, SOLID=1, DASH=2, DOT=3, DASH_DOT=4, DASH_DOT_DOT=5, LONG_DASH=6, CIRCLE=7, DOUBLE_SLIM=8, SLIM_THICK=9, THICK_SLIM=10, SLIM_THICK_SLIM=11 | `line_type_code` `header.rs:17` | `line_type_name` `write/header.rs:16` |
| 테두리 굵기 | 16단계 mm 테이블(0.1…5.0mm)의 최근접 인덱스 | `width_index` `header.rs:36` | `width_mm_attr` `write/header.rs:34` |

### 문자·문단 (header)

| 축 | OWPML 값 → 코드 | read | write(역) |
|---|---|---|---|
| 정렬(align `horizontal`) | JUSTIFY=0, LEFT=1, RIGHT=2, CENTER=3, DISTRIBUTE=4, DISTRIBUTE_SPACE=5 | `alignment_code` :87 | `write_para_properties` :298 |
| 줄간격 종류(lineSpacing `type`) | PERCENT=0, FIXED=1, BETWEEN_LINES=2, AT_LEAST=3 | :380 | :307 |
| 밑줄(underline `type`) | NONE=0, BOTTOM=1, TOP=3 | :207 | :239 |
| 문단머리(heading `type`) | NONE=0, OUTLINE=1, NUMBER=2, BULLET=3 | :311 | (heading 상수 방출) |
| 번호형식(`numFormat`) | DIGIT / HANGUL_SYLLABLE / HANGUL_JAMO / CIRCLED_DIGIT / LATIN_UPPER·LOWER / ROMAN_UPPER·LOWER | `num_fmt` :72 | (상수) |
| 언어 슬롯(fontface `lang`) | HANGUL=0, LATIN=1, HANJA=2, JAPANESE=3, OTHER=4, SYMBOL=5, USER=6 | `lang_slot` :58 | `LANG_NAMES` `write/header.rs:12` |

### 컨트롤 페이로드 (section, `hp:ctrl`)

| 축 | OWPML 값 → 코드 | read | write(역) |
|---|---|---|---|
| 다단 종류(colPr `type`) | NEWSPAPER=0, BALANCED=1, PARALLEL=2 | `parse_col_pr` :377 | `write_col_ctrl` :473 |
| 다단 방향(colPr `layout`) | LEFT=0, RIGHT=1, MIRROR=2 | `parse_col_pr` :383 | `write_col_ctrl` :482 |
| 머리말/꼬리말 적용(`applyPageType`) | BOTH=0, EVEN=1, ODD=2 | `head_foot_data` :400 | `write_header_footer`(BOTH 상수) :516 |
| 쪽번호 위치(pageNum `pos`) | NONE=0, TOP_LEFT=1 … BOTTOM_RIGHT=6, OUTSIDE_TOP=7, OUTSIDE_BOTTOM=8, INSIDE_TOP=9, INSIDE_BOTTOM=10 | `build_pgnp` :415 | `page_num_pos_name` :654 |
| 표 쪽나눔(tbl `pageBreak`) | NONE=0, TABLE=1, CELL=2 | `parse_table` :700 | (write는 CELL 고정) :1002 |
| 그러데이션(gradation `type`) | LINEAR=선형, 그 외=방사 근사 | `parse_gradation` :1223 | `write_shape_element`(LINEAR/RADIAL) :765 |
| 색(`#RRGGBB` ↔ COLORREF) | `#RRGGBB` → `0x00BBGGRR`(R↔B 스왑) | `parse_color` `xml.rs:36` | `color_hex` `section.rs:555` / `color_attr` `templates.rs:101` |

---

## 부록: match 분기 전수 대조 결과

문서 작성 시 `read/section.rs`·`read/header.rs`의 **모든** 요소 매칭 분기를 §3 표와 1:1 대조했다.

- **read/section.rs** — 요소 처리 분기(로컬명 기준)를 파서별로 전수 확인: `parse_paragraph`
  11분기(+fallback 1) · `parse_text` 3 · `parse_sec_pr` 2(+skip) · `parse_ctrl` 12(+other) ·
  `parse_table` 6(+skip_subtree) · `parse_cell` 5(+skip) · `parse_picture` 3(+skip) ·
  `collect_shape` 8(+skip) · `parse_equation` 3 · `parse_gradation` 1 · `parse_linesegs` 1 ·
  `read_field_command` 1. 표 G에 누락 없음(엔티티 상수 `amp/lt/gt/quot/apos`는 텍스트 해석
  세부라 §3.1 parse_text 행에 통합).
- **read/header.rs** — Start/Empty 디스패치 35요소분기 + End 디스패치 7(fontface/margin/charPr/
  borderFill/paraPr/numbering/paraHead) + Text(paraHead 템플릿) 전수 확인. 표 H에 누락 없음.

### 12번 갭 문서 입력 요약

**미구현(양쪽 없음):** chart, ole, video, container, textart, formObject, compose/dutmal, 바탕쪽
(hm master-page), 편집 이력(hhs). — container/textart/formObject/compose는 read가 텍스트만 fallback
보존하나 write DROP으로 소실.

**skip(정보 소실) — 서브트리 소비/무시로 IR에 흔적 없음:**
- section: `hp:secPr`의 grid/startNum/visibility/footNotePr/endNotePr/pageBorderFill/lineNumberShape
  (`:353`); `hp:tbl`의 미지 자식(`:743` skip_subtree)·`hp:tr`(`:742`); `hp:tc` 미지 자식(`:864`);
  `hp:pic`의 imgRect/imgClip/imgDim/renderingInfo/img효과(`:916`); 도형의 shadow/outMargin/
  renderingInfo 및 Rect/Ellipse/Arc의 pt(`:1059`).
- header: beginNum/compatibleDocument/docOption/linkinfo/autoSpacing 등 미매칭 요소(`:510`).

**read만 해석·write 손실(범주 (b)):** charPr의 shadow·outline·emboss·engrave·supscript·subscript,
underline shape, colPr 단별폭·구분선, gradation 중심·step, numbering 번호형식, tab 정의.
