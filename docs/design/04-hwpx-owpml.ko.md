[한국어](04-hwpx-owpml.ko.md) · [English](04-hwpx-owpml.md)

## HWPX/OWPML 읽기·쓰기 서브시스템 (crates/hwpx)

hwp-cli의 HWPX 계층은 **OPC(ZIP) 컨테이너 ↔ OWPML XML ↔ IR(`hwp_model`)** 3단 변환기다. IR은 hwp5(바이너리 HWP)와 **완전히 동일한 의미**를 갖도록 설계되어, 텍스트 추출·위치 산수·렌더링 코드가 두 포맷에서 같은 경로를 탄다. 이 문서는 `crates/hwpx/src/read/section.rs`와 `crates/hwpx/src/write/section.rs`를 중심으로, 처음부터 재구현 가능한 수준으로 규약·바이트 레이아웃·불변식을 정리한다.

핵심 파일:
- `crates/hwpx/src/package.rs` — ZIP/OPC 컨테이너 접근·mimetype 검증
- `crates/hwpx/src/read/{mod,section,header,xml}.rs` — HWPX → IR
- `crates/hwpx/src/write/{mod,section,header,templates}.rs` — IR → HWPX
- `crates/hwp-model/src/{control,paragraph}.rs` — IR 타입 정의

---

## 1. HWPX ZIP(OPC) 컨테이너 구조

HWPX는 OPC(Open Packaging Conventions) 규약의 ZIP 아카이브다. 항목·순서·압축 방식이 한글(HWP)과의 호환에 직접 영향을 준다.

### 1.1 항목 목록과 쓰기 순서

`write/mod.rs::write_document_with`가 만드는 항목 순서(왼쪽이 먼저):

| # | 경로 | 압축 | 역할 | 소스 |
|---|------|------|------|------|
| 1 | `mimetype` | **Stored(무압축)** | 컨테이너 타입 매직. 반드시 **첫 엔트리 + 무압축** | `templates::MIMETYPE` = `application/hwp+zip` |
| 2 | `version.xml` | Deflate | 포맷 버전(`hv:HCFVersion`) | `templates::VERSION_XML` |
| 3 | `META-INF/container.rdf` | Deflate | RDF 패키지 관계 | `templates::CONTAINER_RDF` |
| 4 | `META-INF/container.xml` | Deflate | OCF rootfiles(진입점) | `templates::CONTAINER_XML` |
| 5 | `META-INF/manifest.xml` | Deflate | ODF manifest(빈 셸) | `templates::MANIFEST_XML` |
| 6 | `Contents/content.hpf` | Deflate | OPF 패키지: manifest+spine+메타데이터 | `templates::content_hpf()` |
| 7 | `Contents/header.xml` | Deflate | 글꼴/문자모양/문단모양/테두리채움/스타일 테이블 | `write/header.rs` |
| 8.. | `Contents/section0.xml`, `section1.xml`, … | Deflate | 본문(문단·표·도형) | `write/section.rs` |
| .. | `BinData/image1.png`, … | Deflate | 임베드 이미지 | `BinCollector` |
| .. | `Preview/PrvText.txt` | Deflate | 미리보기 텍스트(선두 ~1000자) | `doc.plain_text()` |
| .. | `settings.xml` | Deflate | 캐럿 위치 등 앱 설정 | `templates::SETTINGS_XML` |

**불변식(재구현 시 위반 금지):**
- `mimetype`은 ZIP의 **첫 로컬 헤더**여야 하고 `CompressionMethod::Stored`여야 한다. 파일 앞부분에 무압축 매직이 오도록 하는 OPC 규약. 위반 시 한글이 손상 파일로 판단.
- `version.xml`의 `major="5" minor="1" micro="1" buildNumber="0" xmlVersion="1.5"`는 **고정 상수**(포맷 호환). `application`/`appVersion`만 작성 프로그램(hwp-cli, `CARGO_PKG_VERSION`) 값.

### 1.2 읽기 경로 (`package.rs`, `read/mod.rs`)

`HwpxPackage::open(path)`:
1. `zip::ZipArchive`로 연다.
2. `mimetype` 엔트리를 읽어 `application/hwp+zip`인지 검증 — 불일치 시 `HwpxError::BadMimetype`.

`read_document(path)`:
1. `Contents/header.xml` → `header::parse_header` (글꼴·모양 테이블 → `DocHeader`).
2. `section_entries()` — `Contents/section*.xml`을 이름의 숫자 접미사로 **수치 정렬**(`section0 < section1 < … < section10`) 후 각각 `section::parse_section`.
3. `BinData/`로 시작하는 모든 엔트리 → `BinStream { name, data }`.
4. `version.xml`의 `major.minor.micro.buildNumber` → `DocMeta::source_version`.
5. `Contents/content.hpf`(OPF) → `parse_content_meta`로 제목/지은이/주제/키워드 추출(최선 노력 — 없어도 진행).

`section_entries` 정렬 키:
```
n.trim_start_matches("Contents/section").trim_end_matches(".xml").parse::<u32>()
```
파싱 실패 항목은 `u32::MAX`로 밀려 뒤로 정렬.

### 1.3 content.hpf (OPF) 구조

`templates::content_hpf(section_count, bin_items, meta)`가 합성. `<opf:package>` 안에:
- `<opf:metadata>`: `<opf:title>`, `<opf:language>ko`, `<dc:creator>`, `<dc:subject>`, `<opf:meta name="keywords" content="…"/>`, 그리고 앱 표식 `<opf:meta name="creator" content="text">hwp-cli</opf:meta>`.
- `<opf:manifest>`: `header` + `section{i}` + `settings` + 바이너리 항목(`isEmbeded="1"`).
- `<opf:spine>`: `header`·`section{i}`를 `<opf:itemref linear="yes"/>`로.

바이너리 항목 id/href/mime는 `BinCollector`가 채운다. `read/mod.rs::parse_content_meta`는 이 파일의 `title`/`creator`/`subject` local-name과 `meta[name=keywords]`만 읽는다(`name="creator"`인 앱 표식은 local-name이 `meta`라 무시).

---

## 2. 네임스페이스와 주요 요소

### 2.1 네임스페이스

| 접두사 | URI | 용도 |
|--------|-----|------|
| `hs` | `http://www.hancom.co.kr/hwpml/2011/section` | 섹션 루트 `hs:sec` |
| `hp` | `http://www.hancom.co.kr/hwpml/2011/paragraph` | 문단·런·컨트롤·표·도형 (`p`, `run`, `t`, `ctrl`, `tbl`, `rect`, `pic`, `pos`, `sz`, `drawText` …) |
| `hc` | `http://www.hancom.co.kr/hwpml/2011/core` | 코어 기하/스타일 (`img`, `fillBrush`, `winBrush`, `gradation`, `color`, `pt0…`, `center`, `ax1`, `transMatrix` …) |
| `hh` | `…/2011/head` | header.xml 루트 |
| `ha` | `…/2011/app` | settings.xml |
| `hpf`,`dc`,`opf` | (OPF/DC) | content.hpf 메타데이터 |

섹션 루트 방출(write):
```xml
<hs:sec xmlns:hs="…/section" xmlns:hp="…/paragraph" xmlns:hc="…/core">…</hs:sec>
```

**파서 규약:** `read/xml.rs::attr`와 모든 매칭은 `local_name()`(접두사 제거) 기준이다. 따라서 `hp:p`든 `p`든 로컬 이름 `p`로 매칭된다 — 네임스페이스 접두사에 의존하지 않음.

### 2.2 요소 → IR 매핑 (parse_paragraph)

`hp:p` 하나를 `parse_paragraph`가 소비하며, 자식 요소를 다음처럼 IR로 변환한다:

| OWPML 요소 | IR 표현 | 확장/인라인 코드 | 비고 |
|-----------|---------|-----------------|------|
| `hp:p` | `Paragraph` | — | `paraPrIDRef`→`para_shape`, `styleIDRef`→`style`, `pageBreak="1"`→`break_type\|=0x04`, `columnBreak="1"`→`break_type\|=0x08` |
| `hp:run` | `char_shape_runs.push((wchar_pos, id))` | — | `charPrIDRef`. 같은 위치면 마지막 run으로 덮어씀(빈 `<hp:t/>` 대응) |
| `hp:t` | `HwpChar::Text(c)` 열 | — | `parse_text`. `wchar_pos += c.len_utf16()`. `GeneralRef`(`&amp;` 등) 해석 |
| `hp:tab` | `HwpChar::InlineCtrl{code:9, payload:[0;12]}` | Inline 9 | `wchar_pos += 8` |
| `hp:lineBreak` | `HwpChar::CharCtrl(10)` | Char 10 | `wchar_pos += 1` |
| `hp:secPr` | `ExtCtrl(2,"secd")` + `Control::SectionDef` | Ext 2 | `parse_sec_pr` |
| `hp:ctrl` | (자식별 분기, §5) | — | `parse_ctrl` |
| `hp:tbl` | `ExtCtrl(11,"tbl ")` + `Control::Table` | Ext 11 | `parse_table` |
| `hp:equation` | `ExtCtrl(11,"eqed")` + `Generic{equation}` | Ext 11 | `parse_equation` |
| `hp:pic` | `ExtCtrl(11,"gso ")` + `Control::Picture` | Ext 11 | `zOrder`는 **시작 태그** 속성(자식 pos 아님) |
| `hp:rect/ellipse/line/polygon/curve/arc` | `ExtCtrl(11,ctrl_id)` + `Generic{gso_shapes}` | Ext 11 | `collect_shape` |
| `hp:linesegarray` | `para.line_segs` | — | `parse_linesegs` |
| 기타 개체 | `ExtCtrl(11,ctrl_id)` + `Generic{paragraph_lists}` | Ext 11 | `collect_sub_lists`(글상자 텍스트) |

### 2.3 확장 컨트롤 문자 삽입 (`push_ext_ctrl`)

확장 개체(secd/tbl/gso/…)는 문단 문자열 안에 **8 WCHAR 확장 컨트롤 문자**로 나타난다. `push_ext_ctrl(para, wchar_pos, code, ctrl_id)`:
- `HwpChar::ExtCtrl { code, ctrl_id, payload, ctrl_index }`를 `para.chars`에 push.
- `payload`(12바이트): 선두 4바이트 = **역순 ctrl_id**(hwp5 저장 형식과 동일), 나머지 0.
- `ctrl_index = Some(para.controls.len())` — 뒤이어 push할 `Control`을 가리킴.
- `wchar_pos += 8`.

`HwpChar::wchar_width()`: `Text`=`len_utf16`(1 또는 서로게이트 2), `CharCtrl`=1, `InlineCtrl`/`ExtCtrl`=8. **위치 산수의 단일 기준** — 8 WCHAR을 잘못 세면 이후 모든 `char_shape_runs`/`line_segs` 정렬이 어긋난다.

컨트롤 문자 분류표(`hwp_model::paragraph::char_kind`, hwp5 §4.2.4):
- **Char(1 WCHAR):** 0, 10, 13, 24~31
- **Inline(8 WCHAR):** 4~9, 19, 20
- **Extended(8 WCHAR):** 1~3, 11, 12, 14~18, 21~23

---

## 3. 도형 기하 (collect_shape ↔ write_shape_element)

### 3.1 읽기: `collect_shape`

`shape_kind(name)`이 요소 이름 → `ShapeKind`: `rect`→Rect, `ellipse`→Ellipse, `line`→Line, `polygon`→Polygon, `curve`→Curve, `arc`→Arc.

`collect_shape`가 서브트리를 소비하며 `ShapeGeom`을 채운다:

| 자식 요소 | 읽는 속성 | ShapeGeom 필드 |
|-----------|-----------|----------------|
| `hp:pos` | `horzOffset`→x, `vertOffset`→y, `treatAsChar="1"`→anchored | x, y, anchored |
| `hp:sz` | `width`→w, `height`→h | w, h |
| `hp:lineShape` | `color`→border_color(parse_color), `width`→border_width, `style`→border_style, `headStyle`→arrow_start, `tailStyle`→arrow_end | 테두리 |
| `hc:winBrush` | `faceColor`→fill(parse_color) | fill |
| `hc:gradation` | `parse_gradation` | fill_gradient |
| `hc:pt0…ptN` | Polygon/Curve일 때만 `x`,`y` | points |
| `hc:center`/`ax1`/`ax2` | Arc일 때만 `x`,`y`(등장 순서) | points(3점) |
| `hp:subList` | 문단 재귀 | paragraph_lists(도형 내 텍스트) |

**핵심 불변식 — pt 취급이 도형 종류에 따라 다르다:**
- **Rect/Ellipse/Arc의 `pt0~3`은 무시**한다. 이들은 bbox 4모서리(정품 형식)이며, 크기는 `hp:sz`로 왕복하므로 pt를 다시 읽으면 도형에 헛점이 붙는다.
- **Polygon/Curve만 `pt*`를 기하 점**으로 취한다.
- **Arc는 `center`/`ax1`/`ax2` 3점**(bbox 기준 중심+켤레 두 축)을 `points`로 운반. 렌더러가 이 3점으로 호를 그린다.
- Rect의 둥근 모서리: 시작 태그 `ratio`(0~100%) → `round_ratio`.

방출 조건: `w != 0 || h != 0 || !points.is_empty()` (가로/세로 선은 한 축이 0일 수 있어 OR 조건).

### 3.2 색 변환 규약

`read/xml.rs::parse_color("#RRGGBB")` → COLORREF `0x00BBGGRR`(R↔B 스왑). `"none"`/실패 → `0xFFFF_FFFF`.
역방향 `write/section.rs::color_hex(c)` → `"#RRGGBB"`, `templates::color_attr`는 `0xFFFF_FFFF`를 `"none"`으로.

### 3.3 쓰기: `write_shape_element`

hwpx-출신(`write_ir_shapes`)·hwp5-출신(`write_gso`) 도형 모두 이 함수를 거친다. 방출 순서(정품 실측 순서 준수):

1. **여는 태그** `<hp:{el} id zOrder numberingType="PICTURE" textWrap="IN_FRONT_OF_TEXT" textFlow="BOTH_SIDES" lock dropcapstyle href groupLevel instid>` + 종류별 추가 속성:
   - Rect: `ratio="{round_ratio}"`
   - Ellipse: `intervalDirty="0" hasArcPr="0" arcType="NORMAL"`
   - Arc: `type="NORMAL"`
2. **`write_obj_scaffold`** — `hp:offset(0,0)`, `hp:orgSz(w,h)`, `hp:curSz(cur_w,cur_h)`, `hp:flip`, `hp:rotationInfo(centerX=w/2,centerY=h/2)`, `hp:renderingInfo`(trans/sca/rotMatrix 항등행렬).
   - **curSz 규약:** Ellipse/Arc는 `(0,0)`(미리사이즈 없음 표식), 그 외는 `(w,h)`.
3. **`hp:lineShape`** — `border_width<=0`이면 `style="NONE" width="0"`, 아니면 color/width/style(`line_style_name`)/headStyle/tailStyle(`arrow_name`).
4. **`hc:fillBrush`** — **채움이 있을 때만** 방출:
   - `fill_gradient` Some → `hc:gradation type angle colorNum` + `hc:color` 자식들.
   - `fill != 0xFFFF_FFFF` → `hc:winBrush faceColor`.
   - **무채움(`0xFFFF_FFFF`)은 fillBrush 자체를 생략** — 불투명 흰색으로 내보내면 투명 가이드 도형이 뒤 내용을 덮는다(도넛/링 다이어그램 미렌더 버그의 원인).
5. **`hp:shadow type="NONE"`** — 정품 실측 필수 요소.
6. **`hp:drawText`**(텍스트 있을 때) — §3.5.
7. **기하 좌표점**(drawText 뒤, 정품 순서):

| 종류 | 방출 요소 |
|------|-----------|
| Line | `hc:startPt`, `hc:endPt` (points 없으면 `(0,0)`~`(w,h)`) |
| Polygon/Curve | `hc:pt0 … hc:ptN` (points 순회) |
| Rect | `hc:pt0(0,0) pt1(w,0) pt2(w,h) pt3(0,h)` (bbox 4모서리) |
| Ellipse | `hc:center(w/2,h/2) ax1(w,h/2) ax2(w/2,0) start1/end1/start2/end2(0,0)` |
| Arc | `hc:center/ax1/ax2` (points 3점 사용, 없으면 bbox 근사 `center(0,0) ax1(0,h) ax2(w,0)`) |

8. **`hp:sz width height widthRelTo="ABSOLUTE" heightRelTo="ABSOLUTE"`** + **pos_xml** + **`hp:outMargin`** + 닫는 태그.

**불변식:** Rect/Ellipse의 pt/center 요소가 없으면 한글이 도형 외곽을 몰라 렌더하지 않는다(빈 화면). 즉 reader가 pt를 버려도(§3.1) writer는 반드시 재합성해야 한다 — 이것이 왕복 규약의 핵심(§6).

### 3.4 그러데이션 (`parse_gradation` ↔ 방출)

읽기: `type`(LINEAR→선형, 그 외 RADIAL/CIRCLE/… → radial 근사), `angle`, `hc:color value` 자식들을 균등 위치 stop으로. 색 2개 미만이면 `None`.
쓰기: `hc:gradation type="{LINEAR|RADIAL}" angle colorNum` + 각 stop `hc:color value`.

### 3.5 도형 내 텍스트 (`drawText`)

`write_draw_text`: `<hp:drawText lastWidth="{width}" name="" editable="0"><hp:subList vertAlign="CENTER">문단들</hp:subList><hp:textMargin left/right/top/bottom="283"/></hp:drawText>`. 모든 `paragraph_lists`를 하나의 subList로 병합(다단 글상자 v1 근사). 도형 텍스트 문단은 `preserve_linesegs`와 무관하게 **항상 linesegarray를 방출**(정품 실측 — 한글은 글상자 문단에 줄배치를 항상 담음).

---

## 4. 부동/인라인 배치 (`hp:pos`)

`hp:pos`는 개체가 글자처럼 흐르는지(인라인) 떠 있는지(부동)와 기준·오프셋을 정한다.

### 4.1 속성 ↔ 코드 매핑

| 속성 | 값 → 코드 | 읽기 함수 |
|------|-----------|-----------|
| `treatAsChar` | `"1"`→인라인(anchored) | (직접) |
| `vertRelTo` | PAPER=0, PAGE=1, PARA=2 | `vert_rel_to_code` |
| `horzRelTo` | PAPER=0, PAGE=1, COLUMN=2, PARA=3 | `horz_rel_to_code` |
| `vertAlign`/`horzAlign` | TOP/LEFT=0, CENTER=1, BOTTOM/RIGHT=2 | `align_code` |
| `vertOffset`/`horzOffset` | i32(HWPUNIT) | `attr_offset_i32` |
| `affectLSpacing`,`flowWithText`,`holdAnchorAndSO` | `"1"`→bool | (직접) |

**음수 오프셋 규약:** hwpx는 음수를 **unsigned 2의보수 십진수**로 저장(예: `-77` → `"4294967219"`). `attr_offset_i32`는 `i64`로 파싱 후 `as i32` 재해석 — `i32` 직접 파싱은 범위 초과로 실패하므로 필수.

### 4.2 GsoPlacement ↔ hwp5 공통 속성 attr

hwpx 표/도형은 `<hp:pos>`/`<hp:sz>`/`<hp:outMargin>`/`zOrder`를 읽어 `GsoPlacement`에 담고, hwp5 CTRL_HEADER 공통 속성 `attr(u32)`로 합성한다(`synth_attr`). 이를 안 읽으면 writer가 부동 상수로 덮어써 인라인 표가 본문 흐름에서 빠진다.

`GsoPlacement::synth_attr` 비트 레이아웃(상위 16비트는 관측 상수 `0x082a`):

| 비트 | 필드 |
|------|------|
| bit0 | `treat_as_char` |
| bit2 | `affect_line_spacing` |
| bits3-4 | `vert_rel_to` |
| bits5-7 | `vert_align` |
| bits8-9 | `horz_rel_to` |
| bits10-12 | `horz_align` |
| bit13 | `flow_with_text` |

정품 실측 예: 인라인 표 `treatAsChar=1, vertRelTo=PARA(2), horzRelTo=PARA(3), flowWithText=1` → `0x082a_2311`.

### 4.3 hwp5-출신 gso의 pos 역합성 (`gso_pos_xml`)

`parse_gso_header(data)`(20B+): `attr@0(u32)`, `voff@4`, `hoff@8`, `w@12`, `h@16`, `zorder@20`(길이<24면 0). `gso_pos_xml(attr, voff, hoff)`가 비트를 역추출:
- `treat=attr&1`, `vrel=(attr>>3)&3`, `valign=(attr>>5)&7`, `hrel=(attr>>8)&3`, `halign=(attr>>10)&7`.
- **부동(treat=0)** → `flowWithText=0 allowOverlap=1`, **인라인(treat=1)** → `flowWithText=1 allowOverlap=0`(정품 실측 — 부동인데 flow=1이면 한글이 다수 도형 배치 실패→빈 화면).

### 4.4 hwpx-출신 도형의 pos (`write_ir_shapes`)

`ShapeGeom`엔 relTo가 없어 근사: `anchored`면 인라인 관례 `(treat=1, vertRelTo=PARA, horzRelTo=COLUMN)`, 아니면 절대좌표 `(treat=0, PAPER, PAPER)`. 오프셋은 `s.y`/`s.x`. z-order 부재라 도형 순서(`i`)로 증가 부여.

---

## 5. hp:ctrl 자식 컨트롤 (parse_ctrl ↔ writer arms)

`hp:ctrl` 안의 컨트롤은 hwp5 ctrl_id + 컨트롤 문자 코드로 매핑되고, 페이로드를 여기서 합성한다. writer는 빈 페이로드 GenericControl을 드롭한다.

| OWPML | ctrl_id | 코드 | 페이로드 | 빌더/역 |
|-------|---------|------|----------|---------|
| `colPr` | `cold` | 2 | 없음 | — |
| `header`/`footer` | `head`/`foot` | 16 | 8B: `apply(u32)`+`id(u32)` | `head_foot_data` |
| `footNote`/`endNote` | `fn  `/`en  ` | 17 | 없음 | — |
| `autoNum` | `atno` | 18 | 12B: `0,4,0`(u32×3) | `build_atno` |
| `pageNum` | `pgnp` | 21 | 12B: `props(u32)`+6B0+`sideChar(u16)` | `build_pgnp`↔`page_num_pos_name` |
| `pageHiding` | `pghd` | 21 | 4B 비트맵 | `build_pghd` |
| `newNum` | `nwno` | 21 | 6B: `0(u32)`+`num(u16)` | `build_nwno` |
| `fieldBegin` | (type→id) | 3(Ext) | CTRL_DATA(0x0057) | §5.2 |
| `fieldEnd` | — | 4(Inline) | 매칭 start 역순 ctrl_id 3B | §5.2 |
| `bookmark` | `bokm` | 22(Ext) | 이름 CTRL_DATA(0x0057) | `bookmark` 모듈 |

### 5.1 페이로드 바이트 레이아웃 (실측 확정)

- **head_foot_data(8B):** `apply(u32 LE)` + `id(u32 LE)`. apply: BOTH=0, EVEN=1, ODD=2. 실측 `<hp:header id="2" applyPageType="BOTH">` → `00000000 02000000`.
- **build_pgnp(12B):** `props(u32) = format | (position<<8)` + `6B 0` + `sideChar(u16)`. position: NONE=0, TOP_LEFT=1…BOTTOM_RIGHT=6, OUTSIDE_TOP=7, OUTSIDE_BOTTOM=8, INSIDE_TOP=9, INSIDE_BOTTOM=10. format은 DIGIT=0만 매핑. 실측 `pos=BOTTOM_CENTER, sideChar='-'` → `00 05 00 00 00 00 00 00 00 00 2d 00`.
- **build_pghd(4B):** 비트맵 `bit0 hideHeader, 1 hideFooter, 2 hideMasterPage, 3 hideBorder, 4 hideFill, 5 hidePageNum`. 실측 표지 `0x21`, 목차 `0x20`.
- **build_atno(12B):** `0, 4, 0`(u32×3, 실측 표준).
- **build_nwno(6B):** `0(u32, 종류=PAGE)` + `num(u16)`. 실측 `num=1` → `00000000 0100`.

### 5.2 필드 왕복 (fieldBegin/fieldEnd)

읽기(`parse_ctrl`):
- `fieldBegin`: `type`→ctrl_id(`field_ctrl_id_from_owpml`), `name`→이름 CTRL_DATA(레코드 태그 `0x0057`, `make_field_ctrl_data`), 자식 `hp:parameters>stringParam[name=Command]`→`make_field_command_data`(비영 id 포함 — 한글이 `%hlk`를 하이퍼링크로 인식하는 데 필수). `ExtCtrl(3, ctrl_id)`.
- `fieldEnd`: `matching_field_start`가 `para.chars`를 뒤→앞 LIFO 스캔(중첩 필드 대응)해 매칭 FIELD_START(코드3)의 ctrl_id를 찾고, `field_end_payload`로 역순 3B(`%` 제외) 페이로드 생성. `InlineCtrl(4)`. 전부 0이면 한글이 필드 짝을 못 지어 하이퍼링크 클릭 불가.

쓰기(`write_paragraph`):
- Generic이 필드 ctrl_id면 `<hp:fieldBegin id type name editable dirty zorder fieldid metaTag>` + (Command 있으면) `<hp:parameters><hp:stringParam name="Command">…`. `current_field_id`에 id 저장.
- `InlineCtrl(4)` 만나면 `<hp:fieldEnd beginIDRef="{fid}" fieldid="{fid}"/>`로 닫음.

---

## 6. 표·셀 왕복

### 6.1 표 (`parse_table` ↔ `write_table`)

읽기 속성: `pageBreak`(NONE=0/TABLE=1/CELL=2 → `attr` bits0-1), `repeatHeader="1"`→`attr` bit2, `noAdjust="1"`→`attr` bit3, `rowCnt`/`colCnt`/`cellSpacing`/`borderFillIDRef`. 자식: `hp:tc`→셀, `hp:inMargin`→`inner_margins[left,right,top,bottom]`, `hp:pos`/`hp:sz`/`hp:outMargin`→`GsoPlacement`. 루프 후 `row_cell_counts`를 셀의 row로 재구성.

**불변식:** `attr`의 pageBreak를 0으로 두면 표가 "나누지 않음"이 되어 잔여 공간에 안 들어가는 표가 통째로 다음 쪽으로 밀린다(목차 박스 분리 버그).

쓰기: `col_w`/`row_h`를 셀 width/height 최댓값으로 추정→`total_w`/`total_h`. `hp:tbl`(고정 속성 `pageBreak="CELL" repeatHeader="1"`) + `hp:sz` + `hp:pos`(인라인 `treatAsChar="1" vertRelTo="PARA"`) + `hp:inMargin`. 셀을 row별 그룹화(`BTreeMap<u16,Vec<&Cell>>`) → `hp:tr` > `hp:tc`.

### 6.2 셀 (`parse_cell`)

`hp:tc`: `header="1"`→`list_attr` bit18(제목 셀, 반복 대상), `borderFillIDRef`. 자식: `cellAddr`(colAddr/rowAddr→col/row), `cellSpan`(colSpan/rowSpan), `cellSz`(width/height), `cellMargin`(left/right/top/bottom→margins), `subList vertAlign`(TOP=0/CENTER=1/BOTTOM=2 → `list_attr` bits5-6), `p`→문단.

**불변식:** subList vertAlign을 안 읽으면 0(TOP)이 되어 셀 내용이 위로 몰리고, 셀 높이가 내용보다 크면 빈 아래 영역이 다음 쪽으로 분리(빈 페이지). 정품 셀은 CENTER(`0x20`).

---

## 7. write_paragraph 런 상태 기계와 왕복 규약

### 7.1 런 상태 기계

`write_paragraph`는 `<hp:p id paraPrIDRef styleIDRef pageBreak columnBreak merged="0">`를 열고, `para.chars`를 순회하며 **문자 모양 경계**에서 `<hp:run charPrIDRef>`을 전환한다(`open_run!` 매크로 — shape 변경 시 텍스트 flush + run 닫기/열기). `shape_id_at(para, wchar_pos)`가 위치별 유효 char_shape를 준다.

문자 처리:
- `Text(c)`→`text_buf`, flush는 `<hp:t xml:space="preserve">`로.
- `CharCtrl`: 10→`<hp:lineBreak/>`, 24→`'-'`, 30→NBSP(` `), 31→space.
- `InlineCtrl`: 9→`<hp:tab/>`, 4→fieldEnd.
- `ExtCtrl`: `ctrl_index`로 `para.controls`를 찾아 종류별 방출(SectionDef, cold, head/foot, Table, Picture, 필드, bokm, pgnp/pghd/nwno/atno, gso_shapes, gso, 그 외는 `DROP` 경고).

첫 문단에 SectionDef가 없으면 `inject_secpr`로 `write_default_sec_pr`(기본 A4)+`write_col_ctrl` 주입. 빈 문단은 `<hp:run charPrIDRef><hp:t/></hp:run>` 하나를 보장.

### 7.2 도형 run 분할 (SHAPE_RUN_LIMIT)

한글은 **run당 앞쪽 ~21개 도형만 렌더하고 나머지를 버린다**(실기 확정). `SHAPE_RUN_LIMIT=12`, `shape_break!` 매크로가 도형 방출 전 run_shapes가 한계면 같은 char_shape로 run을 새로 연다. `count_shape_tags`가 방출된 XML에서 `<hp:rect `·`<hp:ellipse `·`<hp:line `(뒤 공백으로 `lineShape`/`lineseg`/`lineBreak`와 구분)·`arc`·`polygon`·`curve`·`pic`·`connectLine `를 센다.

### 7.3 z-order 고유화 (Z_SCALE)

그룹 도형(한 gso 다중 도형, 예: 도넛=회색+흰 구멍)이 gso z-order를 공유하면 z 충돌로 한글이 하나만 그린다. `Z_SCALE=64`로 `zorder*Z_SCALE + i`를 부여해 상대 순서를 보존하며 고유화.

### 7.4 linesegarray 보존

`preserve_linesegs`(기본 false)일 때만 본문 문단의 `<hp:linesegarray>`를 방출. 줄배치가 내용과 불일치하면 한글이 "변조" 보안 경고를 띄우므로 기본 제거(한글이 재계산). **무수정 왕복에만 true.** 단, 도형 내 텍스트는 §3.5대로 항상 보존.

### 7.5 왕복(round-trip) 규약 요약

| 항목 | reader가 버리는 것 | writer가 재합성하는 것 | 근거 |
|------|-------------------|----------------------|------|
| Rect/Ellipse/Arc pt | pt0~3(bbox 모서리) | sz에서 pt0~3/center/ax 재계산 | pt 중복이면 헛점 |
| 도형 크기 | — | `hp:sz`(w,h) | 유일 크기 소스 |
| curSz | — | Ellipse/Arc=(0,0), 그 외=(w,h) | 정품 실측 |
| 무채움 fill(0xFFFFFFFF) | — | fillBrush **생략** | 투명 유지 |
| linesegarray(본문) | 유지하나 | 기본 방출 안 함 | 변조 경고 회피 |
| 필드 짝 | — | beginIDRef/fieldEnd LIFO 연결 | 하이퍼링크 동작 |
| gso 공통 attr | `<hp:pos>` 개별 속성 | attr(u32) 비트 합성 | 인라인/부동 보존 |
| z-order | hwpx ShapeGeom엔 없음 | 순서 인덱스 or gso*Z_SCALE+i | 겹침 순서 |

**미지원 컨트롤**은 `DROP:` 경고로 집계하고 드롭한다(글상자 일부·해석 실패 gso 등). 경고는 `Vec<String>`로 상위(`read_document`/`write_document`)까지 전파.

---

## 8. 재구현 체크리스트 (요약)

1. **컨테이너:** mimetype(stored, 첫 엔트리) → version.xml → META-INF/* → content.hpf → header.xml → section*.xml → BinData/* → Preview → settings. 읽을 땐 mimetype 검증 + section 수치 정렬.
2. **위치 산수:** 모든 개체는 8 WCHAR 확장 컨트롤 문자로 문단에 삽입(`push_ext_ctrl`), `wchar_width`로 정확히 계수. char_shape_runs/line_segs는 이 좌표계에 정렬.
3. **도형:** collect_shape는 pos/sz/lineShape/winBrush/gradation + (Polygon/Curve의 pt, Arc의 center·ax). Rect/Ellipse/Arc의 pt는 무시하되 writer가 재합성. fillBrush는 채움 있을 때만. curSz는 Ellipse/Arc=(0,0).
4. **배치:** treatAsChar/vertRelTo/horzRelTo/vertAlign/horzAlign/offset ↔ GsoPlacement 비트(0x082a 상수 + 저위 비트). 음수 오프셋은 u32 2의보수 파싱.
5. **컨트롤 페이로드:** head_foot(8B)/pgnp(12B)/pghd(4B)/atno(12B)/nwno(6B) LE 레이아웃 정확히. 필드는 CTRL_DATA(0x0057) + LIFO 짝 연결.
6. **경고 전파:** 드롭·미해석은 `warnings`로 수집해 무손실 여부를 진단 가능하게.
