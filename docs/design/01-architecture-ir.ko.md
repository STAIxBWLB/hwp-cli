[한국어](01-architecture-ir.ko.md) · [English](01-architecture-ir.md)

# hwp-cli 워크스페이스 아키텍처 & 중간표현(IR) 완전 재구축 문서

대상 저장소: `/Users/elevn/projects/hwp-cli` (Cargo 워크스페이스, `resolver = "3"`, `edition = "2024"`, `rust-version = "1.93"`). 이 문서 하나로 워크스페이스 골격, 크레이트 경계, IR 타입 계층, 3계층 설계 근거, 무손실 보존 메커니즘, 데이터 흐름을 처음부터 다시 구현할 수 있게 하는 것이 목표다.

---

## 1. 크레이트 의존 그래프와 책임

루트 `Cargo.toml`은 `members = ["crates/*"]`로 6개 내부 크레이트를 묶고, 모든 외부 의존성 버전을 `[workspace.dependencies]`에 고정한다. 각 크레이트는 `version.workspace = true` 등으로 상속만 받는다.

### 1.1 의존 방향 (비순환 DAG)

```
                    hwp-model  (기반 IR — serde만 의존)
                   /    |    |    \        \
                  /     |    |     \        \
             hwp5   hwpx   hwp-convert   hwp-render
               \      | \       |            /
                \     |  \______ | __________/
                 \    |         \|/
                  \   +----------+ (hwpx → hwp-convert 재사용)
                   \  |          |
                    hwp-cli  (bin: `hwp`, 위 5개 전부 의존)
```

핵심 불변식: **`hwp-model`은 다른 내부 크레이트에 절대 의존하지 않는다.** 모든 크레이트가 여기에 의존하므로 이 API 안정성이 곧 프로젝트 안정성이다(`lib.rs` 주석 명시). 순환은 없다 — 유일하게 비자명한 간선은 `hwpx → hwp-convert`인데, hwpx writer가 필드 이름/명령 인코딩(CTRL_DATA)·OWPML 타입 매핑을 hwp-convert에서 재사용하기 때문이며, `hwp-convert`는 `hwp-model`에만 의존하므로 순환이 생기지 않는다.

### 1.2 크레이트별 책임/의존성 표

| 크레이트 | 산출물 | 책임 (한 문장) | 내부 의존 | 주요 외부 의존 |
|---|---|---|---|---|
| **hwp-model** | lib | HWP/HWPX 공유 **의미 IR(L1)** 타입 정의 + 텍스트 추출·단위 변환 | 없음 | `serde` (그것만) |
| **hwp5** | lib | HWP 5.0 바이너리(CFB+레코드) ↔ IR **reader/writer** | hwp-model | `cfb`, `flate2`, `thiserror` |
| **hwpx** | lib | HWPX(OWPML/ZIP+XML) ↔ IR **reader/writer** + `patch`(충실도 보존 치환) | hwp-model, hwp-convert | `zip`, `quick-xml`, `thiserror` |
| **hwp-convert** | lib | IR ↔ markdown/JSON/HTML/ODT 변환 + 편집 프리미티브(edit/field/bookmark/gso/image/structure) | hwp-model | `serde_json`, `pulldown-cmark`, `zip` |
| **hwp-render** | lib | IR → PNG/SVG/PDF 페이지 렌더러 + 픽셀 diff | hwp-model | `tiny-skia`, `rustybuzz`, `fontdb`, `image`, `pdf-writer`, `subsetter`, `flate2` |
| **hwp-cli** | bin `hwp` | 서브커맨드 디스패치(info/cat/convert/render/new/edit/fields/…/mcp/dump) | 위 5개 전부 | `anyhow`, `clap`, `serde_json` |

의존성 최소화가 설계 규범이다: `hwp-model`은 serde 하나만, 나머지 크레이트도 포맷/기능에 딱 필요한 크레이트만 끌어온다. `hwp5`↔`hwpx`는 서로 의존하지 않는다(양쪽 다 IR을 경유). 이 대칭성이 "N개 포맷 × M개 출력"을 N+M 어댑터로 처리하는 허브-스포크 구조의 핵심이다.

### 1.3 hwp5 크레이트 내부 모듈 계층

`hwp5`는 아래에서 위로 계층화되며, **"스캔과 해석의 분리"**가 규범이다(`record/mod.rs`).

- `container` — MS CFB 래핑, 스트림 열거/읽기 (`Hwp5Container::open`, `read_record_stream`, `body_sections`).
- `file_header` — 256바이트 고정 `FileHeader`(시그니처/버전/압축 플래그) 파싱·직렬화.
- `codec` — 바이트 커서(`ByteReader`/`ByteWriter`)와 raw deflate 압축(`compress`/`decompress`).
- `record` — **의미를 전혀 해석하지 않는** 레이어. `header`(4바이트 헤더 코덱), `tag`(태그 상수/이름 조회, 항상 원시 u16 보존), `scan`(평면 스트림 스캔, `ScanMode::Tolerant`), `tree`(레벨 기반 forest 복원 `RecordNode::build_forest`).
- `doc_info` / `body_text` — 위 `RecordNode` 트리를 의미 파싱해 IR(`DocHeader`/`Section`)로 승격.
- `read` / `write` — 최상위 `read_document(path) -> ReadResult`, `write_document(doc, path, opts)`.

### 1.4 hwpx 크레이트 내부 모듈

- `package` — ZIP 컨테이너(`HwpxPackage`), `mimetype`=`application/hwp+zip`이 첫 엔트리·무압축.
- `read/{mod,header,section,xml}` — OWPML XML을 quick-xml로 파싱해 IR로. **IR 의미를 hwp5와 일치**시킨다: `hp:secPr`/`hp:ctrl(colPr)`/`hp:tbl`을 hwp5처럼 확장 컨트롤 문자(8 WCHAR)+`Control`로 표현.
- `write/{mod,header,section,templates}` — IR → OWPML. `mimetype` stored + 나머지 deflate.
- `patch` — 패키지를 재직렬화하지 않고 XML 텍스트만 외과적으로 치환(템플릿 슬롯 `{{name}}` 채우기, 최대 충실도).

---

## 2. IR 타입 계층 전체 (L1, `hwp-model`)

모듈 구성: `document`, `header`, `paragraph`, `control`, `text`, `units`, `ids`, `opaque`. 모든 IR 타입은 `Serialize + Deserialize`(JSON 왕복 가능). 재직렬화 안정성을 위해 옵션/렌더 전용 필드는 `#[serde(default, skip_serializing_if = ...)]`을 광범위하게 사용한다.

### 2.1 최상위: `Document`

```rust
pub struct Document {
    pub meta: DocMeta,          // 출처(포맷/버전)
    pub metadata: Metadata,     // 제목/지은이/주제/키워드
    pub header: DocHeader,      // ID 참조 테이블 일체 (DocInfo/header.xml)
    pub sections: Vec<Section>, // 본문 구역들
    pub bin_streams: Vec<BinStream>, // 첨부 바이너리(이미지 등)
}
```

- `DocMeta { source_format: String("hwp5"|"hwpx"), source_version: String }` — writer가 재합성 여부를 결정하는 게이트로 사용(§5).
- `Metadata { title/author/subject/keywords: Option<String> }` — hwp5 `\x05HwpSummaryInformation` / hwpx `Contents/content.hpf`(OPF dc:*)에 대응. 전부 Option+default라 JSON 왕복 호환 유지. `is_empty()` 헬퍼.
- `BinStream { name: String, #[serde(skip)] data: Vec<u8> }` — 바이트는 기본 JSON 직렬화에서 제외(L2 비대 방지). 키는 원본 컨테이너 항목 이름(hwp5 `"BIN0001.png"`, hwpx `"BinData/image1.png"`).
- `Document::resolve_bin(&BinRef) -> Option<&[u8]>` — `BinRef::Id(n)`(1-기반)는 `header.bin_data[n-1]`의 `storage_id`+`extension`으로 `BIN{id:04X}.{ext}` 이름을 합성해 매칭; `BinRef::ItemRef(s)`는 이름/접미/스템 휴리스틱 매칭.

### 2.2 `Section`

```rust
pub struct Section {
    pub paragraphs: Vec<Paragraph>,
    pub extras: Vec<OpaqueRecord>, // 문단 아닌 최상위 레코드(정상 파일에선 빈)
}
```

- `Section::section_def() -> Option<&SectionDef>` — 보통 첫 문단의 첫 컨트롤에서 구역 정의를 찾는다. 구역 속성(용지/여백)은 별도 필드가 아니라 문단 안 확장 컨트롤(`secd`)로 표현된다 — hwp5/hwpx 공통 표현.

### 2.3 문단·문자 모델 (`paragraph`)

HWP 본문은 **UTF-16 코드 유닛(WCHAR)** 열이며 0~31은 컨트롤 문자다. 위치 계산의 단일 진실 공급원은 `char_kind(code)` 분류표다.

**컨트롤 문자 분류표 (§4.2.4 스펙, 32개 코드 전수 커버 — 테스트로 강제):**

| 종류 `CharKind` | WCHAR 폭 | 코드 | 의미 |
|---|---|---|---|
| `Char` | 1 | 0, 10, 13, 24–31, ≥32 | 문자형(그 자체로 의미). 줄바꿈 10, 문단끝 13, 하이픈 24, 묶음빈칸 30, 고정폭빈칸 31 |
| `Inline` | 8 | 4–9, 19, 20 | `[코드, 정보 6 WCHAR, 코드]` 인라인 컨트롤. 탭 9, 필드끝 4 |
| `Extended` | 8 | 1–3, 11, 12, 14–18, 21–23 | 별도 CTRL_HEADER 레코드를 가리키는 확장 컨트롤. 개체/표 11, 머리·꼬리말 16, 각주 17, 책갈피 22 |

`ctrl_char` 모듈에 잘 알려진 코드 상수(`LINE_BREAK=10`, `PARA_BREAK=13`, `OBJECT=11`, `HEADER_FOOTER=16`, `FOOTNOTE_ENDNOTE=17` …)가 있다.

```rust
pub enum HwpChar {
    Text(char),                                   // 일반 문자(서로게이트 쌍=char 하나)
    CharCtrl(u16),                                // 1 WCHAR 문자형 컨트롤
    InlineCtrl { code: u16, payload: Vec<u8> },   // 8 WCHAR, payload=정보 6 WCHAR(12바이트)
    ExtCtrl {                                      // 8 WCHAR 확장 컨트롤
        code: u16,
        ctrl_id: [u8; 4],       // 정방향 id(예 b"secd"); 스트림엔 역순 저장
        payload: Vec<u8>,       // 정보 6 WCHAR 원본 12바이트(선두 4=역순 ctrl_id)
        ctrl_index: Option<u32>,// Paragraph::controls의 인덱스(매칭 실패 시 None)
    },
}
```

- `HwpChar::wchar_width()` — `Text`는 `len_utf16()`, 컨트롤은 1 또는 8. **위치 계산의 기준.** 확장/인라인 컨트롤을 잘못 세면 이후 모든 오프셋이 어긋난다.

```rust
pub struct Paragraph {
    pub para_shape: ParaShapeId,
    pub style: StyleId,
    pub chars: Vec<HwpChar>,
    pub char_shape_runs: Vec<(u32, CharShapeId)>, // (WCHAR 시작위치, 문자모양) = PARA_CHAR_SHAPE
    pub line_segs: Vec<LineSeg>,                   // PARA_LINE_SEG (비면 렌더러 폴백)
    pub controls: Vec<Control>,                    // 확장 컨트롤이 가리키는 실체들
    pub header: ParaHeaderInfo,
    pub extras: Vec<OpaqueRecord>,
}
```

`Paragraph::wchar_len()` = Σ`wchar_width` (PARA_HEADER nchars 대조용).

**`LineSeg` (PARA_LINE_SEG 한 줄, 36바이트) — 한글이 저장한 줄 배치. 렌더러가 그대로 신뢰하는 1급 입력:**

| 필드 | 타입 | 의미 |
|---|---|---|
| `text_start` | u32 | 줄 시작 텍스트 위치(문단 내 WCHAR 오프셋) |
| `v_pos` | i32 | 줄 세로 위치 |
| `line_height` | i32 | 줄 높이 |
| `text_height` | i32 | 텍스트 부분 높이 |
| `baseline_gap` | i32 | 줄 세로위치→베이스라인 거리 |
| `line_spacing` | i32 | 줄간격 |
| `col_start` | i32 | 컬럼 내 시작 위치 |
| `seg_width` | i32 | 세그먼트 폭 |
| `flags` | u32 | 페이지/컬럼 첫 줄, 빈 세그먼트 등 |

**`ParaHeaderInfo`** = `{ chars_flags: u8, ctrl_mask: u32, break_type: u8, instance_id: u32, tail: Vec<u8> }` — nchars 최상위 비트/단 나누기 종류/버전별 꼬리(변경추적 병합 등) 왕복 보존.

### 2.4 컨트롤 모델 (`control`)

```rust
pub enum Control {
    SectionDef(SectionDef), // "secd" 구역 정의
    Table(Table),           // "tbl " 표
    Picture(Picture),       // "gso "(그림)/hp:pic 이미지
    Generic(GenericControl),// 그 외 — 원본 보존 + 문단 리스트 수집
}
```

M1 시점에 표(`tbl `)와 구역정의(`secd`)만 의미 파싱하고 나머지는 `Generic`으로 보존한다. `Control::ctrl_id() -> [u8;4]`가 정방향 4바이트 id를 돌려준다.

**`SectionDef`** = `{ data: Vec<u8>(CTRL_HEADER 페이로드, 미해석), page: Option<PageDef>, extras: Vec<OpaqueRecord> }`.

**`PageDef` (PAGE_DEF, 40바이트) — 용지 정의:**

| 필드 | 의미 |
|---|---|
| `width, height` | 용지 크기(HwpUnit) |
| `margin_{left,right,top,bottom,header,footer}` | 여백 6종 |
| `gutter` | 제본 여백 |
| `attr: u32` | bit0 방향(가로), bit1~2 제책 방법 |

**`Table`:**

| 필드 | 타입 | 의미 |
|---|---|---|
| `common_data` | Vec<u8> | CTRL_HEADER 개체 공통 속성 원본(hwp5 출신은 채워짐) |
| `placement` | Option<GsoPlacement> | hwpx 출신 배치 정보(hwp5 출신은 None) |
| `attr` | u32 | 표 속성 |
| `rows, cols` | u16 | 행·열 수 |
| `cell_spacing` | u16 | 셀 간격 |
| `inner_margins` | [u16;4] | 안쪽 여백 왼/오/위/아래 |
| `row_cell_counts` | Vec<u16> | 행별 셀 개수(실측: 스펙의 "Row Size"는 셀 수) |
| `border_fill` | BorderFillId | 표 테두리/배경 |
| `table_tail` | Vec<u8> | TABLE 레코드 나머지 |
| `cells` | Vec<Cell> | 셀 목록(LIST_HEADER 등장순=행 우선) |
| `extras` | Vec<OpaqueRecord> | 미해석 자식 |

**`Cell`** = `{ list_attr: u32, col/row/col_span/row_span: u16, width/height: HwpUnit, margins: [u16;4], border_fill: BorderFillId, header_tail: Vec<u8>, paragraphs: Vec<Paragraph> }`. 셀 안에 다시 `Paragraph`가 재귀로 들어간다.

**`Picture`** = `{ common_data: Vec<u8>, width/height: HwpUnit, treat_as_char: bool(글자처럼/floating), z_order: u32, vert_offset/horz_offset: i32, bin_ref: BinRef, extras: Vec<OpaqueRecord> }`.

**`BinRef`** = `Id(BinDataId)`(hwp5, 1-기반) | `ItemRef(String)`(hwpx manifest `binaryItemIDRef`).

**`GsoPlacement`** — hwpx `<hp:pos>/<hp:sz>/<hp:outMargin>/zOrder`에서 읽어 hwp5 CTRL_HEADER 40바이트 공통 속성으로 합성하기 위한 배치 정보. 필드: `treat_as_char, affect_line_spacing, flow_with_text, hold_anchor: bool`, `vert_rel_to/horz_rel_to/vert_align/horz_align: u8`, `vert_offset/horz_offset/z_order/width/height: i32`, `out_margins: [u16;4]`. 핵심 메서드 `synth_attr() -> u32`가 비트를 합성한다:

```
0x082a_0000
 | treat_as_char        // bit0
 | affect_line_spacing<<2
 | (vert_rel_to&3)<<3    // bits3-4
 | (vert_align&7)<<5     // bits5-7
 | (horz_rel_to&3)<<8    // bits8-9
 | (horz_align&7)<<10    // bits10-12
 | flow_with_text<<13    // bit13
```

상위 16비트 `0x082a`는 관측 상수(widthRelTo/heightRelTo=ABSOLUTE 등). 이 정보를 잃으면 writer가 인라인 표를 떠 있는(floating) 개체로 덮어써 본문 흐름에서 빠지므로 회귀 테스트로 정품값(`0x082a2311` 등)을 강제한다.

**`GenericControl` (미해석 컨트롤의 무손실 컨테이너):**

| 필드 | 의미 |
|---|---|
| `ctrl_id: [u8;4]` | 정방향 id(b"gso ", b"head" 등) |
| `data: Vec<u8>` | CTRL_HEADER 페이로드 |
| `paragraph_lists: Vec<ParagraphList>` | LIST_HEADER 단위 문단 리스트 — **텍스트 추출 전용** 재귀 수집 |
| `extras: Vec<OpaqueRecord>` | 미해석 자식 |
| `raw_children: Vec<OpaqueRecord>` | hwp5 원본 CTRL_HEADER 서브트리(중첩 포함) — **무손실 재직렬화용**. 존재하면 emit 시 이 트리를 그대로 방출, paragraph_lists/extras는 추출 전용(gso 등 중첩 평탄화 방지) |
| `gso_shapes: Vec<ShapeGeom>` | hwpx 그리기 개체 기하/스타일 — **렌더 전용**(hwpx reader가 채움) |
| `equation: Option<Equation>` | 수식 — **렌더 전용** |

`ParagraphList` = `{ header_data: Vec<u8>, paragraphs: Vec<Paragraph> }`.

**렌더 전용 그리기 개체 타입:**

- `ShapeKind` = `Rect | Ellipse | Line | Polygon | Curve | Arc`.
- `ShapeGeom { kind, x/y/w/h: i32(경계상자 HWPUNIT), points: Vec<(i32,i32)>, fill: u32(COLORREF), fill_gradient: Option<GradientSpec>, border_color: u32, border_width: i32, round_ratio: u8, border_style: u8(0실선~5긴파선), arrow_start/arrow_end: u8, anchored: bool }`.
- `GradientSpec { radial: bool, angle_deg: f32, stops: Vec<(f32 위치0..1, u32 COLORREF)> }`.
- `Equation { script: String, width/height: i32, inline: bool, x/y: i32 }` — 렌더러가 상자+스크립트 텍스트로 근사.

### 2.5 헤더(참조 테이블) 모델 (`header`)

`LANG_COUNT = 7` (한글/영문/한자/일어/외국어/기호/사용자).

```rust
pub struct DocHeader {
    pub properties: DocumentProperties,
    pub fonts: [Vec<FaceName>; LANG_COUNT], // 언어 슬롯별 글꼴
    pub bin_data: Vec<BinDataItem>,
    pub border_fills: Vec<BorderFill>,      // 참조는 1-기반 관례
    pub char_shapes: Vec<CharShape>,
    pub tab_defs: Vec<RawEntry>,
    pub numberings: Vec<RawEntry>,
    pub bullets: Vec<RawEntry>,
    pub bullet_chars: Vec<char>,            // 렌더 전용, bullets와 병렬
    pub numbering_levels: Vec<Vec<NumLevel>>,// 렌더 전용, numberings와 병렬
    pub para_shapes: Vec<ParaShape>,
    pub styles: Vec<Style>,
    pub id_mappings_counts: Vec<u32>,       // ID_MAPPINGS 원본 카운트 배열(버전별 길이 보존)
    pub id_extras: Vec<OpaqueRecord>,       // ID_MAPPINGS 미해석 자식
    pub extras: Vec<OpaqueRecord>,          // DocInfo 최상위 미해석(DOC_DATA, 호환설정)
}
```

- **`DocumentProperties`** (DOCUMENT_PROPERTIES, 26바이트) = `{ section_count: u16, start_numbers: [u16;6](쪽/각주/미주/그림/표/수식), caret: (u32,u32,u32) }`.
- **`FaceName`** = `{ attr: u8, name: String, alt_kind: Option<u8>, alt_name: Option<String>, panose: Option<[u8;10]>(attr bit6), default_name: Option<String>(attr bit5), type_info: Option<String>(OWPML 왕복), tail: Vec<u8> }`.
- **`CharShape`** — 문자 모양. 언어 슬롯별 배열(`face_ids: [u16;7]`, `ratios/rel_sizes: [u8;7]`, `spacings/offsets: [i8;7]`), `base_size: i32`(10pt=1000), `attr: u32`(효과 비트), `strike: bool`(의미 플래그), 색상 4종(`text_color/underline_color/shade_color/shadow_color: u32` COLORREF 0x00BBGGRR), `shadow_gap: (i8,i8)`, `border_fill_id: u16`, `tail: Vec<u8>`. 접근자: `is_bold`(bit1), `is_italic`(bit0), `underline_kind`(bit2~3), `has_outline`(8~10), `has_shadow`(11~12), `is_emboss`(13)/`is_engrave`(14), `is_superscript`(15)/`is_subscript`(16), `char_offset(lang)`. **취소선 주의:** raw 비트18~20은 DIFFSPEC(스펙 이견)이라 신뢰하지 않고 별도 `strike` 플래그로만 판단 — HWP5 reader는 항상 false, HWPX reader만 visible `<hp:strikeout>`일 때 true. `attr`는 보존(바이트 왕복 무영향).
- **`ParaShape`** — 문단 모양. `attr1: u32`, `indent`, `margin_left/right`, `spacing_top/bottom`, `line_spacing_old`, `tab_def_id/numbering_id/border_fill_id: u16`, `border_offsets: [i16;4]`, `line_spacing_type: u8`(0비율/1고정/2여백/3최소), `line_spacing: i32`, `tail`. 접근자: `alignment()`(attr1 bit2~4: 0양쪽~5나눔), `head_type()`(bit23~24: 0없음/1개요/2번호/3불릿), `head_level()`(bit25~27: 1~7).
- **`NumLevel`** = `{ start: u32, fmt: NumFmt, template: String("^N"=N수준 번호자리) }`. `NumFmt` = `Digit | HangulSyllable | HangulJamo | CircledDigit | LatinUpper | LatinLower | RomanUpper | RomanLower`.
- **`Style`** = `{ name, english_name: String, attr/next_style: u8, lang_id: i16, para_shape: ParaShapeId, char_shape: CharShapeId, tail }`.
- **`BinDataItem`** = `{ attr: u16, link_abs/link_rel: Option<String>, storage_id: Option<u16>, extension: Option<String>, tail }`. `kind()` = attr&0xF (0링크/1임베딩/2스토리지).
- **`BorderLine`** = `{ line_type: u8(0없음/1실선/…), width: u8(굵기표 인덱스), color: u32 }`. `width_mm()`가 16단계 mm 테이블(0.1~5.0) 조회, `is_visible()`.
- **`BorderFill`** = `{ attr: u16, sides: [BorderLine;4](왼/오/위/아래), diagonal: BorderLine, fill_type: u32, bg_color: Option<u32>, tail }`. `visible_bg()`가 0xFFFFFFFF(없음) 제외.
- **`RawEntry`** = `{ data: Vec<u8>, children: Vec<OpaqueRecord> }` — 의미 파싱 전 ID 테이블 항목의 원시 보존형(tab_defs/numberings/bullets).

### 2.6 ID newtype (`ids`) — 타입 안전 인덱스

`id_type!` 매크로로 생성되는 `#[serde(transparent)]` newtype: `CharShapeId(u16)`, `ParaShapeId(u16)`, `StyleId(u16)`, `BorderFillId(u16)`(**1-기반 관례**), `FaceNameId(u16)`, `BinDataId(u16)`. 종류가 다른 ID를 섞는 실수를 컴파일 타임에 방지한다.

### 2.7 단위 (`units`)

`HwpUnit(pub i32)` = 1/7200 인치(`#[serde(transparent)]`). `PER_PT=100`, `PER_INCH=7200`. **1pt=정확히 100 HWPUNIT**이라 pt 변환 무손실. 변환: `to_pt()`, `to_mm()`, `to_px(dpi)`. 레이아웃 계산은 이 정수 단위로 수행한다.

### 2.8 텍스트 추출 (`text`)

`TextOptions { include_header_footer: bool, include_hidden: bool }`. `Document::plain_text[_with]`가 섹션→문단 순회. 컨트롤 포함 정책은 **확장 컨트롤의 문자 코드 기준**(ctrl_id보다 안정): 표/개체(11)·각주(17) 포함, 머리·꼬리말(16)·숨은설명(15) 제외 기본. 표는 셀 사이 탭·행 사이 개행(hwp5txt 유사), `Generic`은 paragraph_lists를 순회.

---

## 3. 왜 IR이 3계층인가 (L0 / L1 / L2)

`hwp-model/lib.rs`가 명시하는 설계:

- **L0 (포맷별 무손실 표현)** — 각 포맷의 바이트에 가장 가까운 표현. hwp5의 `RecordNode { tag: u16, data: Vec<u8>, children }` forest(레벨 기반 트리), hwpx의 XML 텍스트/`HwpxPackage` 엔트리. **의미를 해석하지 않는다** — `record` 모듈은 `(tag, level, data)`만 다루고 태그는 항상 원시 u16으로 보존(enum 강제 변환 없음 → 새 태그가 와도 안 깨짐).
- **L1 (의미 IR)** — 이 `hwp-model` 크레이트. 포맷 중립 의미 모델. `Document/Section/Paragraph/Control/DocHeader/...`.
- **L2 (파생 표현)** — markdown/JSON/HTML/ODT(`hwp-convert`), PNG/SVG/PDF(`hwp-render`). 손실을 허용하는 출력.

**3계층인 이유:**

1. **HWP 5.0과 HWPX(OWPML)는 의미론적으로 거의 동형**이다. 따라서 L1은 "두 포맷의 최소공통분모"가 아니라 **HWP 의미 모델 그 자체**를 충실히 옮긴다 — hwpx reader조차 `secPr/colPr/tbl`을 hwp5식 확장 컨트롤(8 WCHAR)로 정규화해, 위치 산수와 텍스트 추출이 **양 포맷 공용 코드**를 타게 만든다. 이것이 "공통 조상 추상화" 대신 "한 포맷의 모델을 정본으로 채택"한 결정의 핵심이다.
2. **L0을 L1과 분리**하지 않으면 무손실 왕복이 불가능하다. 파싱(스캔)이 실패하거나 미지원 레코드를 만나도 L0 형태(`OpaqueRecord`/`RawEntry`/`raw_children`/`tail`)로 운반하다가 같은 포맷 재저장 시 바이트 그대로 방출한다. "스캔과 해석의 분리"가 이걸 가능케 한다.
3. **L1을 L2와 분리**하면 N포맷×M출력이 N+M 어댑터로 준다(허브-스포크). 모든 변환·편집·렌더는 L1만 소비하므로, 새 입력 포맷은 reader 하나, 새 출력은 converter/renderer 하나만 추가하면 된다.

---

## 4. `OpaqueRecord`와 무손실 왕복 설계

### 4.1 핵심 타입

```rust
pub struct OpaqueRecord {
    pub tag: u16,
    pub data: Vec<u8>,          // hex 문자열로 직렬화(스냅샷 가독성)
    pub children: Vec<OpaqueRecord>, // 서브트리 통째 보존
}
```

무손실 전략: **모르는 레코드는 버리지 않고 원시 형태(서브트리 통째)로 운반한다.** 같은 포맷 재저장 시 그대로 방출하고, 교차 포맷 변환 시에는 드롭하되 경고(`DROP:`)로 집계한다(§5.3의 `--strict`가 이걸로 실패 판정).

### 4.2 hex_bytes serde 모듈

`OpaqueRecord::data`를 비롯한 모든 raw 바이트 필드(`Picture::common_data`, `SectionDef::data`, `*::tail`, `HwpChar::*::payload` 등)는 `#[serde(with = "hex_bytes")]`로 **hex 문자열** 직렬화한다. 목적은 JSON/insta 스냅샷 가독성. deserialize는 길이 짝수 검증 후 2자씩 `u8::from_str_radix(_,16)`. 이 덕에 L2 JSON(§5.4)도 바이너리를 잃지 않고 왕복한다.

### 4.3 "알려진 prefix + tail" 규칙

무손실의 두 번째 축은 부분 파싱이다. 각 레코드 타입은 **의미를 아는 앞부분(prefix)만 구조화하고, 버전별로 붙는 뒷부분은 `tail: Vec<u8>`로 통째 보존**한다(`CharShape`/`ParaShape`/`Style`/`FaceName`/`BorderFill`/`BinDataItem`/`ParaHeaderInfo` 전부). writer는 파서와 **거울 대칭**으로 "prefix 재방출 + tail 그대로 append"한다. 그래서 hwp5에서 읽은 단순 컨트롤 문서는 압축 해제 스트림 기준 **바이트 동일 왕복**이 성립한다.

계층별 무손실 장치를 정리하면:

| 위치 | 장치 | 보존 대상 |
|---|---|---|
| 레코드 헤더 | `RecordHeader{tag,level,size}` 코덱 | 태그 원시 u16, 확장 크기(0xFFF 표식) |
| 트리 구조 | `RecordNode::build_forest` | level==깊이 → 트리→깊이 재계산으로 왕복 |
| 미지원 레코드 | `OpaqueRecord` | tag+data(hex)+children 서브트리 |
| 알려진 레코드 꼬리 | `*.tail` | 버전별 추가 필드 |
| 확장 컨트롤 문자 | `HwpChar::ExtCtrl.payload` | 정보 6 WCHAR 12바이트(역순 ctrl_id 포함) |
| hwp5 중첩 개체 | `GenericControl::raw_children` | CTRL_HEADER 서브트리 전체(평탄화 방지) |
| ID_MAPPINGS 카운트 | `id_mappings_counts` | 버전별 배열 길이(쓰기 시 유도값과 대조) |
| hwpx 글꼴 | `FaceName::type_info` | OWPML typeInfo 요소 원문 |

`RecordNode::build_forest`는 손상 관용: level이 부모 없이 튀면 가장 가까운 조상에 붙이고 경고 누적(`ScanMode::Tolerant`). ID_MAPPINGS 카운트는 테이블 길이에서 유도(수동 동기화 금지)하되 원본 버전별 추가 카운트가 있으면 꼬리만 보존.

### 4.4 렌더 전용 vs 왕복 필드의 분리

무손실을 깨지 않기 위해, 렌더링에만 쓰는 파생 필드(`bullet_chars`, `numbering_levels`, `gso_shapes`, `equation`, `strike`, `NumLevel::template`)는 전부 `#[serde(default, skip_serializing_if=...)]`로 선언해 **바이너리에 쓰지 않고** JSON에서도 비어 있으면 생략한다. 예: `CharShape::strike`는 의미 플래그일 뿐 `attr` 원본 비트는 그대로 보존되어 바이트 왕복에 영향이 없다.

---

## 5. 데이터 흐름

### 5.1 포맷 감지 (허브 입구)

`hwp-cli/format.rs`가 **확장자가 아니라 매직 바이트**로 판별: CFB `D0CF11E0A1B11AE1`→`Hwp5`, ZIP `PK`(504B)→`Hwpx`. `commands::cat::load_document(path)`가 디스패치의 정본:

- 확장자 `.json` → `hwp_convert::from_json` (L2 JSON을 IR로 역직렬화)
- CFB → `hwp5::read_document`
- ZIP → `hwpx::read_document`

세 경로 모두 `Document`(L1)를 돌려주고 경고를 stderr로 흘린다. 이후 모든 명령은 L1만 소비한다.

### 5.2 read → IR (L0 → L1)

**hwp5 (`read::read_document`):**
1. `Hwp5Container::open` → CFB 열고 `check_body_readable`.
2. `/DocInfo` 스트림 읽어(raw deflate 해제) `scan_stream(_, Tolerant)` → `RecordNode` forest(L0) → `parse_doc_info` → `DocHeader`.
3. `body_sections()`의 각 `/BodyText/SectionN` → 같은 스캔 → `parse_section` → `Section`.
4. `/BinData/*` 스트림 → (압축 플래그면 해제 시도-폴백) → `Vec<BinStream>`.
5. `\x05HwpSummaryInformation` → `parse_summary` → `Metadata`(최선 노력).
6. `Document{ meta:{source_format:"hwp5", source_version:버전}, metadata, header, sections, bin_streams }`.

**hwpx (`read::read_document`):**
1. `HwpxPackage::open`.
2. `Contents/header.xml` → `header::parse_header` → `DocHeader`.
3. `section_entries()`(`Contents/sectionN.xml`) → `section::parse_section` → `Section`. `properties.section_count` 갱신.
4. `BinData/*` → `BinStream`.
5. `version.xml`의 major.minor.micro.buildNumber → source_version. `Contents/content.hpf`(OPF) → `Metadata`.

두 reader의 산출 IR은 **의미가 정렬**되어 있어(hwpx도 secd/colPr/tbl을 확장 컨트롤로 표현) 이후 로직이 포맷을 구분하지 않는다.

### 5.3 IR → write (L1 → L0/파일)

`commands::convert::run`이 출력 확장자(또는 `--to`)로 분기(`write_by_ext`도 공용):

| 대상 | 경로 |
|---|---|
| `.hwp` | `write_hwp[_edited/_structural]` → `hwp5::write_document` |
| `.hwpx` | `hwpx::write::write_document_with(preserve_linesegs)` |
| `.md` | `hwp_convert::to_markdown` |
| `.html` | `hwp_convert::to_html` |
| `.odt` | `hwp_convert::to_odt` |
| `.json` | `hwp_convert::to_json(pretty, embed_bin)` |
| `.pdf` | render 경로 위임 → `hwp_render::render_document_pdf` |

**writer의 재합성 게이트(핵심 불변식)** — `hwp5::write_document`는 `doc.meta.source_format`으로 처리를 나눈다:
- **hwp5 출신·무수정** → "알려진 prefix + tail" 거울 대칭으로 바이트 동일 방출.
- **hwpx/md 출신 또는 편집됨(`edited`)** → 문단 불변식을 다시 세운다: 문단끝 `0x0d` 문자·nchars bit31(마지막 문단)·빈 문단 PARA_TEXT 생략·줄 배치 방출. `synthesize_pictures`(hwpx/md 이미지를 hwp5 SHAPE_COMPONENT 레코드로 합성), `degrade_hwpx_gso`(gso_shapes를 hwp5 gso 레코드로 역합성)를 태워야 `strip_unwritable_pictures`에 드롭되지 않는다.

**줄 배치(lineseg) 처리 — 한글 "변조" 경고 회피:** 한글은 줄 배치 캐시가 내용과 어긋나면 보안 경고를 띄운다. 그래서 기본은 줄 배치를 **제거**하고 한글이 열 때 재계산하게 한다(`preserve_linesegs=false` 기본). `write_hwp_impl`의 분기:
- hwp5 무수정/`--preserve-layout` → 원본 줄 배치 그대로.
- hwpx 출신/편집된 hwp5(줄 배치 있음) → `clear_linesegs`(표 셀·머리말 등 중첩 포함 재귀 제거) 후 한글 재계산.
- markdown 등 줄 배치 없는 출처 → `hwp_render::lineseg::synthesize_linesegs`로 폰트 셰이핑 계산 후 IR에 채움(함초롬 폰트 필요).
- 편집된 hwp5 무수정(`write_hwp_edited`) → **외과적**: 편집된 문단만 줄 배치를 비워(count=0) 그 문단만 재계산, 미편집 문단은 원본 보존(전부 비우면 표 셀 빈 문단이 부풀어 빈 칸 발생 — 실측).

hwp 저장 시 1쪽을 48dpi로 렌더해 PrvImage로 동봉한다. `--strict`는 `DROP:` 경고가 있으면 비정상 종료(구조 보존 대상 hwp/hwpx에만 의미).

### 5.4 IR ↔ JSON (L1 ↔ L2, 무손실 왕복)

`hwp_convert::to_json(doc, pretty, embed_bin)`: `embed_bin=false`면 그냥 serde(이미지 바이트 제외 — `BinStream::data`가 `#[serde(skip)]`). `embed_bin=true`면 `bin_streams[].data_b64`에 base64로 실어 **자급식 JSON**. `from_json`이 `data_b64`를 분리·디코드해 이미지까지 복원. 테스트가 "이미지 외 IR 전체 동일"과 "embed면 완전 동일"을 강제한다.

### 5.5 IR → render (L1 → 픽셀/벡터)

`hwp-render/lib.rs` 파이프라인: **IR → Layout(`layout::layout_document`, LineSegLayouter) → `display::DisplayList` → 백엔드(png tiny-skia / svg / pdf)**. 세 백엔드가 같은 DisplayList를 소비한다.

- `build_display_list(doc, opts)`: `FontStore` 생성 → `opts.font_dirs` 로드 → `layout_document(doc, &mut store, &mut warnings)` → 폰트 해석 리포트 병합.
- `render_document` → `png::render_png(list, dpi)` → `RenderOutput{ pages: Vec<Pixmap>, report }`.
- `render_document_svg` → `svg::render_svg` → `SvgOutput{ pages: Vec<String>, report }`.
- `render_document_pdf(doc, opts, pages)` → 페이지 선택(1-기반) → `pdf::render_pdf`(폰트 임베드+검색 가능 텍스트) → `PdfOutput{ data, report }`.
- `RenderOptions{ dpi: f32(기본 96), font_dirs }`. 폰트 미지정 시 `resolve_font_dirs`가 `HWP_FONT_DIR`(없으면 `fonts/`)로 함초롬 글꼴 로드 — 안 하면 시스템 폰트로 대체돼 충실도 급락.
- `layout`이 `line_segs`가 있으면 한글이 저장한 줄 배치를 1급 입력으로 신뢰하고, 비어 있으면 폴백 경로로 셰이핑. `diff::compare`가 한글 기준 PNG와 픽셀/오프셋 오차를 측정(`hwp diff`).

### 5.6 전체 파이프라인 요약

```
파일(hwp5/hwpx/json)
   │  detect(매직바이트) → load_document
   ▼
[L0] RecordNode forest / OWPML XML / JSON
   │  parse_doc_info·parse_section / parse_header·parse_section / from_json
   ▼
[L1] Document ──────────────┬───────────────┬──────────────┐
   │ hwp5::write_document   │ hwpx::write   │ hwp_convert   │ hwp_render
   ▼ (재합성 게이트)         ▼               ▼ to_md/html/    ▼ layout→DisplayList
 .hwp                     .hwpx            odt/json         → png/svg/pdf
```

편집 흐름(`hwp edit`)은 L1을 로드 → `hwp_convert`의 편집 프리미티브(`replace_text`, `set_cell`, `set_field`, `create_bookmark/hyperlink`, `insert_image`, `insert_paragraph`, `add/delete_table_row`, `set_char_format`, `set_para_align`, `apply_meta`)로 L1을 변형 → `write_hwp_edited`(외과적)/`write_hwp_structural`(합성 강제)로 저장. hwpx 템플릿 채우기(`hwp fill`)는 L1을 우회해 `hwpx::patch`로 XML 텍스트만 치환(최대 충실도).

---

## 6. 재구축 시 반드시 지킬 불변식 요약

1. **`hwp-model`은 serde 외 어떤 내부 크레이트에도 의존하지 않는다** — DAG 비순환의 뿌리.
2. **`char_kind`가 WCHAR 폭 분류의 단일 진실 공급원** — 확장/인라인=8, 나머지=1. 이걸 어기면 모든 위치 오프셋이 깨진다.
3. **레코드 태그는 원시 u16으로 보존**(enum 강제 변환 금지), 트리는 level==깊이로 왕복.
4. **"알려진 prefix + tail" 대칭** — reader/writer가 거울. 모르는 건 `OpaqueRecord`/`RawEntry`/`raw_children`으로 통째 보존.
5. **렌더 전용 필드는 `skip_serializing_if`로 바이너리/JSON 왕복에서 격리** — 무손실 불변 유지.
6. **줄 배치는 편집·교차포맷 시 제거하고 한글이 재계산** — 정합 안 하면 "변조" 경고. hwp5 무수정만 원본 보존.
7. **writer 재합성 게이트는 `source_format`으로** — hwp5 출신 무수정은 바이트 동일, 그 외/편집은 문단 불변식·개체 레코드 합성.
8. **BorderFillId는 1-기반**, `BinRef::Id`도 1-기반(`bin_data[id-1]`), `HwpUnit`=1/7200인치·1pt=100단위(무손실).
