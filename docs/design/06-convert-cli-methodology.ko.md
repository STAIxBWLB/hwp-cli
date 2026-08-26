[한국어](06-convert-cli-methodology.ko.md) · [English](06-convert-cli-methodology.md)

# hwp-cli 변환 서브시스템 재구축 가이드

이 문서는 `crates/hwp-convert`(IR 편집·변환 로직), `crates/hwp-cli`(사용자 인터페이스), 그리고 정답지(ground-truth)·진단 방법론을 **처음부터 다시 구현**할 수 있는 수준으로 기술한다. 파일 경로는 모두 `/Users/elevn/projects/hwp-cli/` 기준이다.

---

## 1. 전체 아키텍처와 변환 방향

### 1.1 크레이트 계층과 IR 경유 변환

모든 변환은 단일 공용 문서 모델(IR) `hwp_model::Document{ meta, header, metadata, sections, bin_streams }`을 **허브**로 삼아 이뤄진다. 포맷 간 직접 변환은 없고, 항상 `읽기 → IR → 쓰기`다.

| 크레이트 | 역할 |
|---|---|
| `hwp-model` | 공유 IR 계약. `Document`, `Paragraph{chars, char_shape_runs, controls, line_segs, header}`, `Control`(Table/Picture/SectionDef/Generic), `HwpChar`(Text/CharCtrl/InlineCtrl/ExtCtrl), `ShapeGeom`, opaque 무손실 보존 |
| `hwp5` | HWP 5.0 바이너리(CFB+레코드) read/write |
| `hwpx` | HWPX(ZIP+OWPML XML) read/write |
| `hwp-convert` | IR ↔ markdown/JSON, 인메모리 편집 프리미티브, 필드/책갈피 스캔 (본 문서 핵심) |
| `hwp-render` | IR → PNG/SVG/PDF, 줄 배치(lineseg) 합성, 렌더 diff |

`hwp-convert/src/lib.rs`가 재수출하는 공개 API: `replace_text`, `set_cell`, `add_rows`, `table_dims`, `apply_meta`(edit), `create_field/create_hyperlink/list_fields/set_field/scan_placeholders`(field), `create_bookmark/list_bookmarks/make_bokm_ctrl_data`(bookmark), `set_char_format/set_para_align`(format), `insert_paragraph/delete_paragraph/add_table_row/delete_table_row`(structure), `from_markdown/default_header`, `to_markdown/to_html/to_odt`, `insert_image`, `to_json/from_json`.

### 1.2 변환 방향 매트릭스

| 변환 | 경로 | 충실도 |
|---|---|---|
| hwp5 → hwp5 (무수정) | `write_hwp(preserve_layout)` | **바이트 동일 왕복** (identity 게이트) |
| hwp5 → hwpx | reader→IR→hwpx writer | 의미 동등 (도형·필드·표 보존) |
| hwpx → hwp5 | IR→`write_hwp_edited`(합성) | 의미 동등, gso는 안전 저하 |
| md → hwp5/hwpx | `from_markdown`→writer(합성) | 신규 문서 |
| html → hwp5/hwpx | `from_html`→writer(합성) | 신규 문서. [18](18-html-fragment-contract.ko.md)의 XHTML 부분집합을 받는다 |
| json(IR) → hwp/hwpx | `from_json`→writer | 편집 왕복 |
| hwp/hwpx → md/html/json/odt | IR→직렬화 | 손실 변환(단방향) |
| hwp/hwpx → docx | IR→`docx.rs`(OOXML 패키지) | 손실 변환(단방향). 출력 전용이며 docx 입력은 없다 |
| hwp/hwpx → csv/txt | IR→`csv.rs` 또는 텍스트 추출 | 표 또는 평문만 |
| hwp/hwpx → pdf/png/svg | render 경로 위임 | 렌더 출력 |

**핵심 분기**: `doc.meta.source_format`이 `"hwp5"`이고 미편집이면 원본 줄 배치를 보존해 바이트 동일 왕복. 그 외(md/hwpx 출신 또는 편집됨)는 **합성 경로**(`edited=true`)로 줄 배치를 비우고 한글이 재계산하게 한다.

### 1.3 CLI 서브커맨드 (`crates/hwp-cli/src/cli.rs`, clap derive)

커맨드 표면은 **산문이 아니라 자동 생성 문서**다. `crates/hwp-cli/tests/cli_reference.rs`가
[manual/cli-reference](../manual/cli-reference.ko.md)와 그 영문 짝을 clap 트리에서 직접 렌더하고
어긋나면 빌드를 실패시키므로, 서브커맨드와 플래그의 정본 목록은 그쪽에 있으며 여기에 옮겨 적지
않는다. 같은 테스트가 `crates/hwp-cli/src/i18n.rs`의 한국어 도움말 오버레이도 누락·잔존 항목
기준으로 검사한다.

이 문서가 담아야 할 것은 자동 생성 레퍼런스가 말해 줄 수 없는 부분이다.

`format.rs`의 `detect(path)`는 **확장자가 아니라 매직 바이트**로 판별한다: CFB(`D0 CF 11 E0 A1 B1 1A E1`)→Hwp5, ZIP(`50 4B`)→Hwpx. `.json` 입력은 IR 직렬화본으로 간주(`load_document`, `cat.rs`).

---

## 2. gso.rs — SHAPE_COMPONENT → ShapeGeom

`hwp-convert/src/gso.rs`는 hwp5의 도형 서브트리(`OpaqueRecord` 트리)를 구조화된 `ShapeGeom` 목록으로 변환한다. hwpx writer가 hwp→hwpx에서 장식 도형·글상자 테두리를 보존할 때 소비한다. **바이트 레이아웃은 `hwp-render/src/shape_draw.rs`(렌더 파서)와 동일 실측**이며 역의존 불가라 사본으로 둔다 — 오프셋 수정 시 양쪽을 함께 봐야 한다.

### 2.1 레코드 태그 상수와 트리 순회

```
SHAPE_COMPONENT = 0x4C   SC_LINE = 0x4E   SC_RECTANGLE = 0x4F
SC_ELLIPSE = 0x50   SC_ARC = 0x51   SC_POLYGON = 0x52
SC_CURVE = 0x53   SC_CONTAINER = 0x56   MAX_DEPTH = 8
```

진입점 `shapes_from_raw(raw: &[OpaqueRecord]) -> Vec<ShapeGeom>` → `walk`(SHAPE_COMPONENT는 `component`, CONTAINER는 재귀) → `component`(각 자식 도형 레코드마다 `geometry`, 중첩 SHAPE_COMPONENT/CONTAINER 재귀). 좌표는 렌더 행렬 적용 후 HWPUNIT, gso 박스 원점 기준.

### 2.2 3×2 어파인 행렬 `Mat`

```
struct Mat { a,b,c,d,e,f: f64 }   // x' = a·x + b·y + c,  y' = d·x + e·y + f
apply(x,y) = (a·x+b·y+c, d·x+e·y+f)
mul(o)     = 표준 3×2 합성 (self ∘ o)
```

읽기 헬퍼: `rd_u16/rd_i32/rd_u32`(LE), `rd_f64`(8B LE), `rd_mat(d,o)`는 `o, o+8, …, o+40`의 6개 f64 = **48바이트**를 읽어 Mat 구성.

### 2.3 `parse_style` — 행렬 T·S·R, 테두리, 채움

SHAPE_COMPONENT data 레이아웃과 파싱 규약:

| 단계 | 오프셋 | 내용 |
|---|---|---|
| CHID | `base` 결정 | `d[0..4]==d[4..8]` → base=8(최상위, CHID×2), 아니면 base=4(멤버 CHID×1) |
| 속성 | `base`~ | 도형 공통 속성 |
| translation | `base+44` | Mat `t` (48B) |
| scale/rot 카운트 | `base+42` | `cnt = rd_u16` |
| scale·rotation 쌍 | `pair = base+44+48+(cnt-1)*96` | `m = t · (rd_mat(pair) · rd_mat(pair+48))` — 마지막 쌍만 사용 |
| 테두리선 | `bo = base+92+cnt*96` | color(u32)@bo, width(i32)@bo+4, lattr(u32)@bo+8 |
| 채움 | `fo = bo+13` | ft(u32)@fo |

- 테두리: `lt = lattr & 0x3F`. `lt != 0`이면 `border_color=color`, `border_width=width.max(1)`, `border_style=hwp5_line_style(lt)`. `lt==0`이면 무테두리(color 0xFFFFFFFF, width 0).
- 채움: `ft & 0x1` → solid `fill = rd_u32(fo+4)`; `ft & 0x4` → `parse_gradient(fo+4)`; bit1(이미지 채움)은 **v1 제외**(bin 참조 필요).
- 데이터가 `pair+96`보다 짧으면 `m = t`(스케일/회전 없음).

`hwp5_line_style(lt)`: `2→1(DASH)`, `3→2(DOT)`, `4→3(DASH_DOT)`, `5→4(DASH_DOT_DOT)`, `6→5(LONG_DASH)`, 그 외→0(SOLID).

### 2.4 `parse_gradient` (spec Table 28)

`fo`부터: `type(i16) angle(i16) cx cy spread num(i16@fo+10)`. `num`은 2~16만 허용. `num>2`면 `INT32[num]` 위치 배열(최댓값으로 정규화해 0~1 clamp) 뒤에 `COLORREF[num]`, `num==2`면 등간격. `radial = (gtype==1)`. stops를 위치순 정렬해 `GradientSpec{radial, angle_deg, stops}` 반환.

### 2.5 `geometry` — 로컬 점에 행렬 적용

`p(o)` = `(rd_i32(o), rd_i32(o+4))`. 태그별 로컬 점:

| 태그 | 레이아웃 | ShapeKind | round_ratio |
|---|---|---|---|
| SC_LINE | p(0)=시작, p(8)=끝 (제로 길이면 None) | Line | 0 |
| SC_RECTANGLE | `d[0]`=곡률%(min 100) + 4모서리 p(1),p(9),p(17),p(25) | Rect | 곡률 |
| SC_POLYGON | n=rd_u16(0) (2~4096), pts p(4+i·8) | Polygon | 0 |
| SC_ELLIPSE | attr(u32) + center p(4) + ax1끝 p(12) + ax2끝 p(20) → bbox 근사 | Ellipse | 0 |
| SC_CURVE | n=rd_u16(0), pts p(2+i·8) (세그먼트 타입 무시, 폴리라인 근사) | Curve | 0 |
| SC_ARC | `d[0]`=kind + center p(1) + ax1끝 p(9) + ax2끝 p(17), **3점 보존** | Arc | 0 |

타원 bbox: `rx = |a1−c|`, `ry = |a2−c|` → `(c.x−rx, c.y−ry)..(c.x+rx, c.y+ry)`.

모든 로컬 점에 `s.m.apply()`를 적용한 뒤 bbox(minx/miny/maxx/maxy) 계산. `points`는 **bbox 원점 상대**(x−minx, y−miny)로 정규화(Line/Polygon/Curve/Arc만; Rect/Ellipse는 sz로 왕복). 최종 `ShapeGeom{ kind, x=minx, y=miny, w, h, points, fill, fill_gradient, border_color, border_width, round_ratio, border_style, arrow_start=0, arrow_end=0, anchored=false }`. 배치(anchored)는 gso 40B 헤더가 결정 — writer가 pos로 방출.

### 2.6 Arc 등방화 (핵심 함정 해결)

행렬(회전+비균등 스케일)은 center/ax1/ax2 두 축을 **비수직(전단)**으로 만든다. 그러나 한글 OWPML `<hp:arc>`는 **수직 두 축만** 수용하며 비수직이면 pinwheel(풍차)로 깨진다. 해결:

```
v1 = ax1 − c,  v2 = ax2 − c
r  = (|v1| + |v2|) / 2
a1 = atan2(v1),  a2 = atan2(v2)
d  = normalize(a2 − a1) → [−π, π]          // v1→v2 sweep, 짧은 쪽
bis = a1 + d/2,  q = sign(d) · π/4          // 이등분선 ±45°
ax1' = c + r·(cos(bis−q), sin(bis−q))
ax2' = c + r·(cos(bis+q), sin(bis+q))
```

두 축을 각의 이등분선 기준 90° 벌려 **원형 1/4호로 근사**(회전·방향 보존, 미세 타원율만 손실). 등방화 후 bbox·points 재계산.

### 2.7 hwpx writer 소비 (`hwpx/src/write/section.rs`)

`write_gso`가 `shapes_from_raw`를 호출. 텍스트가 있으면(글상자) rect 하나+첫 도형 스타일+drawText로 방출, 없으면 도형별로 `write_shape_element`. **그룹 도형(도넛 등) z 충돌 방지**: `zorder * Z_SCALE(64) + index`로 고유화(상대 순서 보존). `curSz`는 Ellipse/Arc만 (0,0)(미리사이즈 없음 표식), 나머지는 (w,h). `fillBrush`는 **채움이 있을 때만** 방출(무채움 0xFFFFFFFF를 불투명 흰색으로 내면 뒤 내용을 덮어 미렌더). reader(`collect_shape`)는 Arc를 `center/ax1/ax2` 3점으로 왕복한다.

---

## 3. 필드·책갈피·서식·구조 편집

모든 편집 프리미티브의 공통 규약: **IR만 바꾸고 불변식 재수립은 writer가 담당**. 편집한 문단은 `line_segs.clear()`(낡은 줄 배치 제거)하고 `header.ctrl_mask=0`(writer가 chars에서 재계산). hwp5 출력은 반드시 합성 경로(`WriteOptions.edited=true`)를 거쳐야 한글이 수용한다.

### 3.1 필드 (`field.rs`) — %hlk 하이퍼링크·FIELD_START/END

HWP 필드 = `FIELD_START`(ExtCtrl, **code 3**, ctrl_id) … 표시 텍스트 … `FIELD_END`(InlineCtrl, **code 4**). 컨트롤 ID: `%clk`(누름틀), `%fmu`(계산식), `%hlk`(하이퍼링크), `%mmg/%dte/%ddt/%xrf/%bmk/%pat/%smr/%usr/%unk`. `is_field_ctrl_id`가 이 집합을 판별. `owpml_field_type`/`field_ctrl_id_from_owpml`가 OWPML type(CLICK_HERE/FORMULA/HYPERLINK/…)과 양방향 매핑.

**읽기**: `list_fields`가 본문·표 셀·글상자를 재귀 순회. 각 FIELD_START의 `ctrl_index`로 컨트롤을 찾아 `field_meta`(이름·명령)를, `field_value`(START 다음~FIELD_END 전 텍스트)를 수집.

**바이트 레이아웃**:
- 필드 command data: `속성(4) 기타(1) len(2=WCHAR수) WCHAR[len] id(4) trailing(4)`. `parse_command`가 역파싱.
- CTRL_DATA(누름틀 이름) Parameter Set: `setid(2) count(2 i16)` + 항목들. `first_bstr`가 첫 BSTR 추출 — 항목은 `id(2) type(2)`, PIT_BSTR(1)은 `UINT32 len + WCHAR[len]`. 타입 크기: 0(null)=0, 2/6=1, 3/7=2, 4/5/8/9=4, 미지→안전 중단.

**생성 (정품 실측 필수)**:
- `rev_payload(ctrl_id)`: 12B, 선두 4B = 역순 ctrl_id (리더가 역순 파싱).
- `field_end_payload`: 12B, 선두 **3B = 역순 ctrl_id(`%` 제외)**, p[3]=0. 예 `%hlk` END = `6b 6c 68 00`. **END payload가 전부 0이면 한글이 START↔END 짝을 못 지어 필드 미완성 → 클릭 이동 불가**(4차 정답지 확정).
- `make_field_command_data`: `%hlk`=(attr `0x0000a800`, etc 0), `%fmu`=(0, `0x08`), 기타=(0,0). **id는 반드시 비영** — id=0이면 한글이 %hlk를 평문 취급. `field_instance_id`가 command의 FNV-1a 32bit 해시(0→1)로 결정론적 비영 id 부여.
- `hlk_command(url)`: `\ ; :`만 백슬래시 이스케이프 후 `;1;0;0;` 접미. (예: `http\://…;1;0;0;`)
- 하이퍼링크 표시 텍스트는 `hyperlink_char_shape`로 **파랑(0x00FF0000)+밑줄** 글자모양을 헤더에 확보해 적용 — 미적용 시 한글이 링크로 인식/표시하지 않음(실기 확인). `apply_run_style`이 표시 구간 `[iw+8, iw+8+표시폭)`에 적용하고 뒤에서 원 모양 복원.

`create_field/create_hyperlink`는 `find_match`로 앵커를 찾아 컨트롤+필드 chars 삽입, `adjust_runs`로 run 보정, `relink_ctrl_index`로 ExtCtrl↔controls 등장순서 재연결.

### 3.2 책갈피 (`bookmark.rs`)

책갈피는 필드가 아니라 `bokm` 컨트롤 — `list_fields`가 못 잡는다. 문자는 `ExtCtrl{ code: 22(BOOKMARK), ctrl_id: b"bokm" }` **단일 지점 표식**(START/END 쌍 없음), 컨트롤은 `Generic{ ctrl_id: bokm, data: [], raw_children: [CTRL_DATA(이름)] }`.

CTRL_DATA 레이아웃(정품과 **바이트 동일** 필수): `setid=0x021b(2) count=1(2) id=0x40000000(4) type=1 BSTR(2) len(2) WCHAR[len]`. `make_bokm_ctrl_data` 상수 프리픽스 = `[0x1b,0x02, 0x01,0x00, 0x00,0x00,0x00,0x40, 0x01,0x00]` + len + WCHAR. `decode_bokm_name`은 오프셋 12부터 이름 읽음. 정품 `bookmark.hwp`의 "책갈피테스트" 24B와 `assert_eq!` 대조.

### 3.3 서식 편집 (`format.rs`)

`CharFormat{ bold/italic/underline/strike: Option<bool>, size_pt, color }`. `set_char_format`이 매칭 범위마다 `restyle_range` — 범위 내 각 run의 기존 모양을 base로 요청 비트만 토글(부분 서식 보존). `apply_format` 비트: bold `1<<1`, italic `1<<0`, underline 비트 2~3(1<<2=글자 아래), strike 비트 18~20(1<<18)+strike 플래그, `base_size = pt×100`(min 100), text_color(COLORREF 0x00BBGGRR).

`set_para_align`: para_shape의 `attr1` 비트 2~4에 정렬 값(0 양쪽/1 왼쪽/2 오른쪽/3 가운데/4 배분/5 나눔). `find_or_insert`/`find_or_insert_para`가 헤더 테이블에 중복 없이 append — **writer가 ID_MAPPINGS 카운트를 `.len()`에서 유도**하므로 append는 안전. `normalize_runs`가 run 불변식(정렬·동일 위치 제거·인접 동일 id 병합·첫 run pos=0) 재수립.

### 3.4 구조 편집 (`structure.rs`)

- `insert_paragraph(anchor, text, before)`: 앵커 문단의 (para_shape, style, 첫 char_shape) 상속. `make_paragraph`는 chars 끝에 `PARA_BREAK(0x0d)` 추가, `char_shape_runs=[(0,cs)]`.
- `delete_paragraph`: **SectionDef 문단 보존 + 섹션당 최소 1문단 유지**(빔 방지).
- `add_table_row`: 마지막 행 셀 구조 복제(내용 비움), `rows++`, `row_cell_counts.push(cnt)`.
- `delete_table_row`: `cells.retain`, row>row 재번호, `rows--`, `row_cell_counts.remove`. 마지막 행은 삭제 불가.

### 3.5 인메모리 편집 프리미티브 (`edit.rs`)

- `replace_text(from,to,all)`: **연속된 Text 문자 안에서만** 매칭(컨트롤 경계에서 끊김 → 서식/구조 보존). `to`가 `from`을 포함하면(예 "한라대학교"→"제주한라대학교") 삽입 텍스트에서 재매칭돼 **무한 루프** — `start = char_idx + to_chars`로 방지.
- `find_match`: `chars`에서 연속 Text 세그먼트를 이어붙여 `from` 검색, `(chars 인덱스, WCHAR 오프셋)` 반환.
- `adjust_runs(runs, p, lo, ln)`: 치환 위치 p·옛 길이 lo·새 길이 ln에 맞춰 run 경계 이동(구간 내부 경계 제거, 이후 경계는 `delta=ln−lo`만큼 평행 이동).
- `set_cell`: 셀 첫 문단을 서식 템플릿으로, 내용만 교체.
- `add_rows`: **u16 범위 검사**(초과 시 절단 손상 대신 거부), `clean_template_row`(전 열 채우고 병합 없는 마지막 행) 복제. **복제 문단은 고유 비영 `instance_id`(표 내 max+1)와 nchars bit31(`chars_flags |= 0x80`, 마지막 문단 표식) 부여** — hwp5 편집 경로는 writer가 재부여하지 않으므로 여기서 세우지 않으면 개체 링크가 깨진다.
- `blank_para_like`: 템플릿의 para_shape/style/첫 char_shape/header 보존, `chars_flags |= 0x80`.

### 3.6 이미지 삽입 (`image.rs`)

`insert_image(anchor, path, size)`: `image_pixel_size`(PNG IHDR@16, GIF LSD@6, BMP@18, JPEG SOF 마커 — 무의존 헤더 파싱)로 픽셀 크기 읽고, `display_size`로 표시 크기 계산(Natural은 본문 폭 초과 시 비례 축소, `px·7200/96` HWPUNIT; Mm은 `mm·283.46457`). 앵커 뒤에 `ExtCtrl{ code:11, ctrl_id: b"gso " }` + `Control::Picture{ bin_ref: ItemRef(name), extras: [] }` 삽입 + `BinStream` 추가. **빈 extras** — writer가 hwp5 도형 레코드(SHAPE_COMPONENT+그림)를 합성.

---

## 4. markdown → hwpx (`from_markdown.rs`)

`from_markdown(md)`는 `pulldown_cmark`(ENABLE_TABLES) 이벤트를 `Builder`로 받아 IR을 만든다. 헤딩→"개요 N" 스타일, 굵게/기울임→글자 모양, GFM 표→Table, 목록→"• " 접두, 줄바꿈→CharCtrl(10).

### 4.1 `default_header` — 한글 빈 문서 준하는 최소 구성

- **글꼴**: 모든 LANG 슬롯에 "함초롬바탕"(`attr=0x01` TTF, `default_name="HCR Batang"`). `emit_face_name`이 0x20 비트(글꼴 대체용 기본 이름) 자동 OR.
- **char_shapes 10슬롯**: 0 본문/1 굵게/2 기울임/3 굵게+기울임/4~9 H1~H6(비율 1.8/1.5/1.3/1.2/1.1/1.1). **`shade_color=0xFFFFFFFF`(없음)** — 기본값 0이면 한글이 불투명 검정 음영으로 해석해 글자마다 검은 막대(14차 실기 "검은 바"). `shadow_gap=(10,10)`, `shadow_color=0x00C0C0C0`.
- **para_shapes**: `attr1=0x180`(bit7 한글 줄나눔 + bit8 줄 격자), `line_spacing_old=160`, `border_fill_id=2` — 정상 표본 실측. 0 기본/1 제목(왼쪽+간격)/2 본문(아래 간격).
- **tab_defs**: 한글 기본 좌/중/우 자동 탭 3개(각 8B: 속성 u32 0/1/2 + count 0 + 예약). **비우면 dangling reference로 손상 판정**.
- **border_fills**: 1·2=무테두리, 3=실선 0.12mm.

### 4.2 `inject_section_controls` — 구역/단 정의 주입

첫 문단 앞에 `SectionDef`(PageDef A4: width 59528, height 84186, 여백 등)와 `cold`(단 정의) 컨트롤을 삽입. 기존 참조 시프트: `ctrl_index += 2`, `char_shape_runs pos += 16`, `line_segs.text_start += 16`. chars 앞에 `secd`/`cold` ExtCtrl(code 2, 역순 ctrl_id payload) 삽입. **`header.break_type = 0x03`**(bit0 구역나눔 + bit1 다단나눔) — 한글이 구역 첫 문단에 항상 쓰는 값, 0이면 헤더-컨트롤 정합 깨져 손상. (hwp5 왕복 경로는 이 함수를 거치지 않아 바이트동일 게이트 무영향.)

### 4.3 `table_paragraph` — GFM 표

`tbl ` ExtCtrl(code 11) 앵커 + `Control::Table`. 셀 여백 `[510,510,141,141]`, border_fill 3, col_w=BODY_WIDTH(42520)/cols. **빈 셀도 문단 1개+char_shape run 1개 필수**(nparas≥1) — nparas=0 셀은 한글이 손상 처리하고 pyhwp도 크래시. 짧은 행 누락 칸은 빈 문단으로 채움.

markdown→hwpx 전체 흐름: `new`/`convert` 커맨드가 `from_markdown`으로 IR 생성 → `hwpx::write_document`(합성 경로, `preserve_linesegs:false`)로 방출. IR→markdown 역변환은 `markdown.rs`의 `to_markdown`(개요 스타일→헤딩, char_shape run→`**`/`*`, 표→GFM).

---

## 5. 쓰기 경로 분기 (`commands/convert.rs`)

`write_hwp_impl(doc, output, preserve_layout, edited)`의 3분기:

1. **`!synthesize || preserve_layout`** (hwp5 무수정): 원본 줄 배치 그대로 → 바이트 동일.
2. **`has_source_linesegs`** (hwpx 출신/편집된 hwp5): `clear_linesegs`로 모든 문단(표 셀·머리말 재귀) 줄 배치 제거 → 한글이 문단/글자 모양 기준 재계산.
3. **줄 배치 없는 출처**(markdown): `hwp_render::lineseg::synthesize_linesegs`로 폰트 셰이핑해 합성 — **함초롬 글꼴 필요**(`HWP_FONT_DIR` 또는 `fonts/`).

`write_hwp_edited`: source가 hwp5면 원본 줄 배치 보존(편집 문단만 count=0), 아니면 합성. `write_hwp_structural`: 삽입 문단/행 불변식(0x0d·마지막 비트·카운트) 적용 위해 **모든 출처에 합성 강제**. `--strict`는 `bail_on_strict`가 `DROP:` 접두 경고를 세어 비정상 종료.

`fill` 커맨드는 두 경로: (1) hwpx `{{name}}` **바이트 보존 치환**(`hwpx::patch::fill_placeholders`, 미리보기·BinData 보존), (2) `--data`에 `tables` 객체 배열이 있으면 IR 표 채우기(`add_rows`+`set_cell`). "tables"가 문자열 배열이면 평문 fill로 라우팅(오인 방지, `has_tables`는 **객체-배열만** 인정).

---

## 6. 정답지(ground-truth) 방법론

### 6.1 코퍼스 커밋 금지

`.gitignore`가 제외하는 것: `/corpus`(HWP_CORPUS_DIR 외부 디렉터리), `/fonts/`, `/fixtures/golden/*.png`, `/fixtures/hwp5/`·`/fixtures/hwpx/`(테스트 문서), `/docs/*.pdf|*.hwp|*.png|spec.txt`(한컴 저작물). **레시피(README)만 커밋**. `fixtures/hwp5/*.hwp`는 hahnlee/hwp-rs(Apache-2.0)에서 받아 배치, 없으면 관련 테스트 자동 skip(`skip_if_no_fixtures`가 `fixtures/hwpx/minimal.hwpx` 존재로 판별).

### 6.2 정품 바이트 대조

정답지 = **원본.hwp ↔ 한글 export hwpx 쌍**. 우리 생성 바이트가 정품 한글과 **완전히 같아야** 한글이 수용한다. 코드에 정품 실측 hex를 상수로 박아 단위 테스트로 대조:
- `gso.rs`: `LINE_SC`(장식선 SHAPE_COMPONENT 252B, CHID `$lin`×2, scale 496.08/0.04, border width=32) + `LINE_GEOM`(SC_LINE (0,0)→(100,100))이 한글 export `<hp:line>` curSz 49608×4·width=32와 정확 대응.
- `bookmark.rs`: `FIXTURE_CTRL_DATA` 24B `assert_eq!(make_bokm_ctrl_data("책갈피테스트"), …)`.
- `field.rs`: `make_field_command_data(b"%hlk", …)` attr `0x0000a800`·len 37(정품 work_report %hlk와 동일)·id≠0, END payload `6b 6c 68`.

태그 값은 테스트에서 **리터럴(0x4c/0x4e)**로 박아 상수 오류를 잡는다.

### 6.3 합성 vs 왕복 분리

두 테스트 등급을 명확히 나눈다:
- **바이트 동일 왕복**(identity 게이트): hwp5 무수정 read→write. 전체 fixture가 원본과 바이트 동일해야 통과. `hwp5/tests/identity.rs`, `roundtrip.rs`.
- **의미 동등**(합성): md/hwpx 출신·편집본은 텍스트·구조 보존만 검증(레이아웃 재계산 허용). `cli.rs`의 왕복 기함 테스트가 `cat` 텍스트 동일 + `DROP` 경고 없음 + 필드 보존을 확인.

`cli.rs` 대표 기함 테스트: `변환_글상자_텍스트_필드_보존`(work_report 글상자 "나눔글꼴"·%hlk "설치하기" 생존), `변환_장식_도형_보존`(annual_report 도형 드롭 ≤8·zOrder 고유값 ≥20종), `변환_완전_왕복_hwp_hwpx_hwp`(양방향 텍스트·필드 보존).

---

## 7. 진단 기법

### 7.1 정답지 문서에 우리 요소 주입

`tools/gen_verification_set.sh`가 `~/Documents/hwp-verification/`에 11종 파일 생성 후 사용자가 **한컴오피스 한글에서 직접 열어** 수용 여부 확인. 방식: base.md(앵커 "제목"·"여기에" 포함)로 base.hwp 생성 → `edit --create-bookmark`/`--create-hyperlink`로 **우리 요소를 주입** → hwp·hwpx 양쪽 방출. 각 파일은 자체 재읽기 게이트(`check`: `cat`이 경고 없이 내용 출력)를 통과한 것만 넘긴다 — **깨진 파일을 사용자에게 넘기지 않기 위함**. A 시리즈(실무 문서 전체 파이프라인), B 시리즈(기능별 최소 파일 — 실패 시 원인 격리).

### 7.2 배치 진단

한글은 겹친 도형을 z-order로 그린다. 예전엔 전부 `zOrder="0"`으로 뭉개 undefined 순서 → 표지 빈 화면. 진단: `변환_장식_도형_보존` 테스트가 section0.xml의 `zOrder="` 값 고유 집합을 세어 **≥20종**을 요구(전부 0 회귀 방지). 그룹 도형(도넛)은 `Z_SCALE=64` 배로 늘리고 인덱스 더해 고유화. `textWrap=IN_FRONT_OF_TEXT`(정품 실측) — TOP_AND_BOTTOM이면 다수 도형 배치 실패로 빈 화면.

### 7.3 3중 대조

세 관점을 교차: (1) **우리 리더**(cat 텍스트/fields JSON), (2) **우리 렌더**(PNG/PDF), (3) **한글 export**(기준 PNG/hwpx). `hwp diff <in> --ref <한글 PNG> --dpi <n>`이 우리 렌더를 한글 기준 PNG와 픽셀·프로파일 비교: `ink_ratio`(잉크 적용률=완전성), `dx/dy`(위치 오프셋), `bad_pixel_pct`(픽셀 차이율=글리프/AA), `MAE`. 폰트를 한글과 동일하게(`HWP_FONT_DIR`) 고정해야 같은 줄바꿈. `golden` 테스트(`HWP_GOLDEN=1`)가 `*.ref.png` 자동 대조. `validate` 커맨드는 exit code 계약(mimetype·필수 엔트리·XML 파싱, `valid:false`면 exit 1).

---

## 8. 실무 함정 (실기 검증으로 확정)

| 증상 | 원인 | 해결 |
|---|---|---|
| 열자마자 **검은 막대** | char_shape `shade_color=0` → 한글 불투명 검정 음영 | `shade_color=0xFFFFFFFF`(없음 표식) |
| 하이퍼링크 **클릭 이동 불가** | FIELD_END payload 전부 0 → START↔END 짝 못 지음 | END payload 선두 3B=역순 ctrl_id(`%` 제외) |
| %hlk **평문 취급**(파랑만) | field command id=0 | FNV-1a 해시로 비영 id, attr `0x0000a800` |
| 하이퍼링크 **미인식** | 표시 텍스트 글자모양 없음 | 파랑(0x00FF0000)+밑줄 char_shape 적용 |
| 표 셀 **손상 팝업** | nparas=0 셀(빈 `\| \|`) | 빈 셀도 문단 1개+char_shape run 1개 |
| 삽입 필드 **깨짐** | ctrl_index↔controls 등장순서 불일치 | `relink_ctrl_index` 호출 |
| 편집 후 **변조 판정** | 낡은 줄 배치가 내용과 어긋남 | 편집 문단 `line_segs.clear()` + 합성 경로 |
| 행 복제 후 **개체 링크 깨짐** | 복제 문단 instance_id 중복 | 표 내 max+1 고유 부여 |
| 표 레코드 **깨짐** | 행 수 u16 절단 → cells/counts 어긋남 | 남은 용량 초과 시 거부 |
| 치환 **무한 루프** | `to`가 `from` 포함 시 재매칭 | `start = char_idx + to_chars` |
| 호(arc) **풍차(pinwheel)** | 행렬이 두 축을 비수직화 | 이등분선 ±45° 등방화 |
| 가이드 도형이 **불투명 흰 원반** | 무채움(0xFFFFFFFF)을 흰색 fillBrush로 방출 | 채움 있을 때만 fillBrush |
| 도넛/그룹 도형 **미렌더** | 같은 gso 다중 도형 z 충돌 | `z*Z_SCALE+index` 고유화 |
| dangling reference 손상 | tab_defs 비움 | 기본 탭 3개 |
| markdown 문단 손상 | 구역 첫 문단 break_type=0 | `break_type=0x03` |

**hwpx→hwp 역방향 gso 안전 저하**(㉕): 한글 실기가 역합성 왕복 gso를 손상 판정 → 글상자를 텍스트만 본문으로 보존(도형 래퍼 생략). 도형 자체는 왕복 미유지되나 텍스트·필드는 보존되고 파일은 유효. `--strict`면 역방향 장식 도형 drop에서 비정상 종료.

---

## 9. 재구축 체크리스트

1. `hwp_model` IR(Document/Paragraph/HwpChar/Control/ShapeGeom) 계약 확정.
2. `format.rs::detect`(매직 바이트) → `load_document`(hwp5/hwpx/json).
3. `gso.rs`: 태그 상수 → `walk`/`component`/`geometry` → `parse_style`(행렬 T·S·R, 오프셋 표) → arc 등방화 → 정품 LINE_SC 252B 테스트.
4. `field.rs`/`bookmark.rs`: FIELD_START(3)/END(4)/BOOKMARK(22), rev_payload/end_payload, command/CTRL_DATA 바이트, 정품 대조 테스트.
5. `format.rs`/`structure.rs`/`edit.rs`: run 비트·정렬·표 행·치환·adjust_runs·instance_id·nchars bit31.
6. `from_markdown.rs`: default_header(shade_color·attr1·tab_defs) + inject_section_controls(break_type 0x03) + table_paragraph(nparas≥1).
7. 쓰기 분기(`write_hwp_impl` 3경로) + 합성/무수정 분리.
8. CLI(clap) + `cli.rs` 통합 테스트(exit code·기함 왕복) + 매뉴얼을 생성하고 한국어 도움말 오버레이를 고정하는 `cli_reference.rs` 드리프트 게이트.
9. 정답지 게이트(gitignore·정품 hex 상수·identity vs 합성) + 진단(주입·zOrder·diff 3중 대조).
