[한국어](05-rendering.ko.md) · [English](05-rendering.md)

# hwp-render 렌더링 엔진 재구축 사양서

크레이트 루트: `/Users/elevn/projects/hwp-cli/crates/hwp-render`. 대상 파일: `src/lib.rs`, `layout.rs`, `lineseg.rs`, `shape.rs`, `shape_draw.rs`, `gso.rs`, `tab.rs`, `fonts.rs`, `display.rs`, `png.rs`, `svg.rs`, `pdf.rs`. 입력은 `hwp_model::Document` IR, 출력은 PNG(tiny-skia 픽스맵)·SVG 문자열·단일 멀티페이지 PDF.

---

## 0. 좌표계와 단위 규약 (모든 계층 공통 불변식)

- **HWPUNIT**: HWP 내부 정수 단위. `1 pt = 100 HWPUNIT`. 어디서든 pt 환산은 `hu / 100.0`. `HwpUnit::to_pt()`도 동일.
- **PARA_SHAPE 여백류**(margin_left/right, indent, spacing_top/bottom)만 예외적으로 **2×HWPUNIT**로 IR에 저장된다. pt 환산은 `/200.0` (`para_geometry`). 근거: hwpx reader가 OWPML `left=1500`을 hwp5 `ml=3000`으로 저장하기 때문.
- **COLORREF**: `0x00BBGGRR`. 추출식 `r=c&0xFF, g=(c>>8)&0xFF, b=(c>>16)&0xFF`. 관례상 `0xFFFFFFFF`=“없음/따름”이며 래스터에서는 검정으로 폴백한다.
- **페이지 좌표계**(DisplayList): 원점 좌상단, y 아래 방향, 단위 pt(f32). PDF 백엔드만 방출 시 `y' = page_h - y`로 뒤집는다(PDF는 좌하단·y위).
- **폰트 글리프**: y-up. 래스터/PDF에서 뒤집어 배치.

파이프라인 전역:
```
Document(IR) ─layout::layout_document─▶ display::DisplayList ─┬─ png::render_png (tiny-skia)
                                                             ├─ svg::render_svg (문자열)
                                                             └─ pdf::render_pdf (Type0/CID)
```
`lib.rs`의 `build_display_list`가 `FontStore::new()` + `--font-dir` 로드 후 `layout_document`를 호출하고, 폰트 해석 리포트를 warnings에 병합한다. `RenderOptions{ dpi: f32(기본 96), font_dirs }`.

---

## 1. 레이아웃 파이프라인 (`layout.rs`, `layout_document`)

### 1.1 페이지 기하 산출

섹션마다 `section.section_def().page` (없으면 `default_page()` = A4 59528×84186 HWPUNIT). 가로(landscape)는 `page_def.attr & 1`이면 폭↔높이 스왑.

```
w, h            = paper_w_hu/100, paper_h_hu/100
body_left       = margin_left / 100
body_top        = (margin_top + margin_header) / 100
body_width      = (paper_w - margin_left - margin_right) / 100
body_bottom     = h - (margin_bottom + margin_footer) / 100
```

상태 변수: `prev_v_pos=-1`(페이지 리셋 감지), `content_bottom=body_top`(흐름 커서), `paras_on_page=0`, `page_notes: Vec<&Note>`, `list_state: ListState`.

### 1.2 페이지 나누기 규칙 (세 가지 트리거)

1. **본문 넘침**: `content_bottom > body_bottom && paras_on_page>0` → 각주 렌더 + 페이지 가구(`PageNumberState::finish`: 머리/꼬리말·쪽번호) + 페이지 push, 상태 리셋.
2. **명시 쪽 나누기**: `para.header.break_type & 0x04 != 0 && paras_on_page>0`.
3. **캐시 v_pos 리셋**: 저장된 lineseg를 그릴 때 `seg.v_pos < prev_v_pos && !page.items.is_empty()` → 정품 멀티페이지는 페이지마다 v_pos가 0으로 리셋되므로 감소를 페이지 경계로 본다.

### 1.3 문단 처리 분기

문단별로 `footnote::para_marks/para_notes`(각주 마커/노트 수집), `tab::tab_stops`, `para_geometry`, `hyperlink_ranges`, `list_state.marker_for_render`를 준비한 뒤:

- **`line_segs`가 비었을 때(폴백)**: 빈 문단이면 `content_bottom += 16.0`. 아니면 `shape_range_notes`로 전체 셰이핑 → `place_wrapped`로 `body_width` 기준 그리디 줄바꿈. `baseline_y = content_bottom + spacing_top + max_size*1.2`, `content_bottom = last_y + max_size*0.4 + spacing_bottom`. 정렬(가운데/오른쪽)은 한 줄에 들어갈 때만 보정.
- **`line_segs`가 있을 때(캐시 존중)**: 각 seg마다 `[text_start, next.text_start)` 구간을 `shape_range_notes`로 셰이핑. 핵심 좌표:
  ```
  stored_baseline = body_top + (seg.v_pos + seg.baseline_gap) / 100
  baseline_y      = max(stored_baseline, content_bottom + baseline_gap_pt)   // 흐름 커서 보정
  x               = body_left + seg.col_start/100 + align_shift
  line_advance    = max(seg.line_height + seg.line_spacing, seg.line_height) / 100
  content_bottom  = last_y + max(line_height_pt - baseline_gap_pt, 0)
  ```
  `wrap_width`는 문단에 seg가 1개일 때만 `seg_width_pt`(불완전 캐시로 보고 재줄바꿈), 여러 개면 `f32::INFINITY`(캐시 신뢰). 첫 seg(i==0)에서 목록 마커를 그린다.

문단 뒤 `layout_para_objects`로 표/이미지/글상자/도형/수식을 배치.

### 1.4 정렬 (`align_line` / `justify_line`)

`align = (para_shape.attr1 >> 2) & 0x7` (0 양쪽, 1 왼쪽, 2 오른쪽, 3 가운데, 4 배분, 5 나눔).
- 오른쪽: `shift = max(seg_width - natural, 0)`.
- 가운데: `shift = max((seg_width - natural)/2, 0)`.
- 양쪽(0, 마지막 줄 제외): `justify_line`이 잉여폭 `slack = clamp(seg_width - natural, 0, natural)`을 글리프 advance에 분배. **공백이 있으면 공백 글리프에만, 없으면 마지막 보이는 글리프 전 gap에 균등**. 후행 공백엔 분배 안 함(텍스트가 오른쪽 끝에 닿게). Rustybuzz cluster source로 합자·결합문자를 포함한 글리프와 Unicode sequence를 매핑.
- 배분(4): 양쪽 정렬용 안전 cap 없이 전체 잉여폭을 **마지막 글리프 뒤 gap까지 포함해** 균등 분배하고 마지막 줄에도 적용. 글리프 하나인 줄도 후행 gap으로 전체 폭 채움.
- 나눔(5): 전체 잉여폭을 **마지막 gap을 제외한** gap에 균등 분배하고 마지막 줄에도 적용. (4/5의 마지막 줄 의미론은 한컴 확인 대상.)

### 1.5 표 배치 (`layout_table`)

1. **그리드 기하**: `col_span==1` 셀에서 열 폭, `row_span==1` 셀에서 행 높이 확정.
2. `derive_col_widths`: 병합 셀에서 미지 열 유도(작은 병합부터) → 평균 폴백 → `table_true_width`(행별 셀폭 합의 최대)로 스케일. 표 실제폭 > 가용폭이면 가용폭에 맞춰 비율 유지 축소.
3. **측정 패스**: 셀마다 스크래치 `PageList`에 `layout_box_paragraphs`로 그려 실측 내용 높이를 재고 `row_h[r] = max(row_h[r], content_h + mt + mb)`. `row_span>1`은 스팬 합 부족분을 마지막 스팬 행에 가산.
4. 누적 오프셋 `col_x = prefix_sums(col_w, x)`, `row_prefix = prefix_sums(row_h, 0)` (조각마다 베이스 y를 더하므로 같은 prefix를 모든 쪽 조각이 공유).
5. 셀마다: **배경 Rect** → **내용**(여백 + 세로정렬 `voff`: `(list_attr>>5)&0x3` 0 위/1 가운데 avail*0.5/2 아래 avail) → **4변 테두리 Line**(`width_mm()*72/25.4`) → **대각선**(`diagonal_dirs(attr)`: slash bits2~4, backslash bits5~7).
6. **쪽 분할**(본문 흐름에서만, `TableSplitCtx` 경유; 셀/글상자 안 중첩 표는 나누지 않음): 표가 `body_bottom`을 넘으면 `Table::page_break_policy()`(`attr` bits 0-1)로 결정. NONE이면 통째로 다음 쪽으로(04 §6.1 불변식), 한 쪽보다 클 때만 행 분할로 폴백. TABLE/CELL은 행 경계에서 나눔(셀 내부 분할은 미지원, 한 쪽보다 큰 행은 넘치며 `TableRowTooTallClipped` 보고). `treat_as_char` 표는 "한 글자"라 나누지 않음(GE-8). 분할마다 표준 쪽 마감 순서(노트 → 가구 → `push_page_checked` → 흐름 상태 리셋)를 거치므로 다음 문단의 한글 경계 플래그가 흡수돼 이중 분할되지 않는다. `repeat_header()`면 선두의 "모든 셀이 `Cell::is_header()`(`list_attr` bit18)"인 행들을 이어지는 쪽 `body_top`에 다시 그린다. `row_span`이 가로지르는 후보 경계는 제외하므로 병합 셀은 절단되지 않고 하나의 분할 불가 행 묶음으로 이동한다. 이 묶음이 한 쪽보다 크면 `TableRowTooTallClipped`로 보고한다.

`cell_margins`: 셀 지정 → 표 `inner_margins` → 기본 `DEFAULT_CELL_MARGINS=[510,510,141,141]` HWPUNIT. 반환은 /100 pt.

### 1.6 블록 개체 (`layout_para_objects`)

`para.controls`를 순회하며 `object_y`(앵커 상단)부터 세로로 이어 배치:
- **Table**: `layout_table`로 배치(쪽 분할 가능 — §1.5 6번). 페이지를 건너간 문단의 앵커는 새 쪽 흐름 위치로 재앵커한다: 캐시 lineseg 쪽 경계마다 `para_top`을 지워, 마지막 쪽에 첫 쪽의 stale y로 그리던 버그를 고쳤다.
- **Picture**: `doc.resolve_bin(bin_ref)` → `Item::Image`.
- **글상자(gso, paragraph_lists 있음)**: `gso::parse_gso_box`로 위치. `treat_as_char()`면 흐름 위치(inline), 아니면 `(horz_offset, vert_offset)` 페이지 절대(floating). 프레임은 `shape_draw::draw_gso_shapes`가 텍스트 뒤에 먼저 그림. 내부 문단은 `split_columns`(v_pos 감소=단 나누기)로 단 분할, `continuation_columns`(같은 폭·높이·세로오프셋의 더 오른쪽 떠있는 gso)로 연결 글상자 위치를 찾아 흐른다.
- **순수 도형(gso, paragraph_lists 없음, `has_shape`)**: `draw_gso_shapes`.
- **hwpx 구조화 도형(`gso_shapes` 있음)**: anchored면 흐름 위치로 clone 조정 후 `draw_ir_shapes`.
- **수식(`equation` 있음)**: 회색 점선 상자 + `prettify_equation`(그리스/연산자 토큰→유니코드) 텍스트.

### 1.7 머리/꼬리말·각주 (`Furniture`, `render_page_notes`)

섹션에서 처음 나온 `b"head"`/`b"foot"` gso를 모든 페이지에 반복. 각주는 `footnote::collect_notes`로 섹션 전체 번호 매김 후, 페이지별 앵커 노트를 스크래치에 y=0부터 쌓아 총높이 측정 → 블록 하단을 `body_bottom`에 닿게 위로 올리고 구분선(`body_width*0.34` 폭) + `translate_item`으로 합친다.

### 1.8 다단(multi-column) — `cold`/`ColumnDef`

**IR:** `cold` CTRL_HEADER → `ColumnDef{count,kind,direction,same_width,gap,widths,divider}`. hwp5는 페이로드에서 파싱(실측 `08 10 dc 08 …` = attr `0x1008`[bit2-9 단수·bit12 동일폭] + gap 2268), hwpx는 `<hp:colPr>` 속성에서. hwplib COLDEF와 bit단위 일치.

**핵심(실측 정답지):** 한글이 저장한 line_seg는 **`col_start`(horzpos)=0**(단 상대)이고 **`seg_width`(horzsize)=단 폭**이다. 단의 x-위치는 저장되지 않으므로 **밴드 인덱스로 계산**한다. `v_pos` 리셋(줄 배치가 상단으로 되돌아감)은 **단 넘김과 페이지 넘김을 겸한다**: 리셋마다 밴드 인덱스 `col_band`를 증가시키고, `col_band % count == 0`이면 페이지 넘김, 아니면 같은 페이지 내 단 넘김(x만 다음 단으로 이동, 커서만 상단으로). 줄 x = `body_left + (col_band % count)·(col_width+gap) + col_start`. `col_width = (body_width - gap·(count-1))/count`. v1은 일반(normal) 순차 흐름·등폭; 배분(높이 밸런싱)·구분선·합성(md) 다단은 후속. 검증: 정답지 `multicol.hwp/.hwpx`(2단) → 3쪽·좌우 단 나란히(테스트 `다단_2단_렌더`).

### 1.9 수식 조판 (`equation.rs`, mini-TeX)

한글 수식은 글립이 아니라 **텍스트 스크립트**(EQN 호환)를 저장한다(hwp5 EQEDIT 레코드 0x58: attr(4)+len(2)+WCHAR[len]; hwpx `<hp:equation>`의 script). `equation.rs`가 스크립트를 토크나이즈→math 트리→box model로 조판해 글리프 런 + 분수선·근호선(`Item::Line`)을 **baseline 상대 좌표**로 방출한다.

**문법(한컴 수식 spec rev1.2):** `over`/`atop`(분수), `sqrt`(근호), `^`/`_`(첨자), `{ }`(그룹), `#`(줄바꿈=행 세로 스택), `&`(열정렬 v1=공백), `~`·`` ` ``(공백), 그리스·연산자·함수어 매핑. AST = `Row/Stack/Sym/Frac/Script/Sqrt/Space`. **스택(다행)**은 각 행의 실제 높이(분수 행은 더 큼)로 세로 배치. **크기 2-pass:** 기준 12pt로 시험 조판→실제 높이를 재 `eq.height`에 맞춰 스케일(다행 수식 과대 방지)→상단 정렬. 검증: 정답지 `equation.hwp/.hwpx`(다행 ΑΒΓ·∑·분수·근호·첨자) → 한글과 구조 일치(테스트 `수식_정답지_렌더`). 후속: 변수 이탤릭/함수 로만 분화, 행렬, 큰연산자 극한.

### 1.10 쪽번호 (`PageNumberState`, `page_number.rs`)

문서 쪽번호는 `DocHeader.properties.start_numbers[0]`(0이면 1)에서 시작하며, 모든 페이지 마감 경로가 `PageNumberState::finish`를 거친 뒤 1씩 증가한다. 문단의 쪽번호 제어는 쪽 나누기 판정 후 적용하므로 새 쪽 첫 문단의 제어가 이전 쪽을 오염시키지 않는다.

- `pgnp`: `props` 하위 8비트 번호 서식, bit8~11 위치를 해석한다. 위치 1~6은 위/아래의 왼쪽·가운데·오른쪽, 7~10은 홀짝 쪽에 따라 바뀌는 바깥쪽/안쪽이다. 위치 0은 표시 없음. 사용자·접두·접미·양옆 문자를 보존해 `sideChar='-'`는 `- 1 -`로 그린다.
- `atno`: 종류가 PAGE(0)인 컨트롤만 `shape_range_page`가 현재 논리 쪽번호로 치환한다. 같은 머리말/꼬리말 문단을 페이지마다 다시 셰이핑하므로 저장된 `number` 값이 아니라 각 쪽의 실제 번호가 표시된다. PAGE 외 자동번호는 다른 번호 렌더러의 책임으로 남긴다.
- `nwno`: PAGE 종류이면 해당 문단이 속한 쪽의 논리 번호를 즉시 재시작한다.
- `pghd`: bit5가 설정된 쪽에서 위치형 쪽번호와 PAGE `atno`를 모두 숨긴다. 다음 쪽 마감 뒤 숨김 상태는 리셋한다.

번호 서식은 IR이 표현하는 DIGIT·CIRCLED_DIGIT·ROMAN 대/소문자·LATIN 대/소문자·HANGUL_SYLLABLE·HANGUL_JAMO를 렌더한다. 그 밖의 형식은 번호 자체가 사라지지 않도록 십진수로 대체하고 한 번만 경고한다. 이는 렌더 동작이며, HWPX 변환에서 `pgnp formatType`을 DIGIT로 고정하는 GE-4는 별도 갭으로 유지한다.

---

## 2. lineseg 합성 (`lineseg.rs`, `synthesize_linesegs`)

md/hwpx 출신 합성 문서는 PARA_LINE_SEG 캐시가 없어 한글이 “손상”으로 판정한다. 여기서 정품과 동일 폰트(함초롬바탕)로 셰이핑해 줄바꿈을 재현하고 lineseg를 생성한다.

### 2.1 상수와 전제

- `TAB_INTERVAL_PT = 40.0` (layout.rs와 동일해야 함).
- `TABLE_BLOCK_PADDING = 566` HWPUNIT (=2.0mm). 정품 실측: `표 advance − Σ행높이 = 566` 상수.
- 폴백 페이지 본문높이 75686, 폴백 body_width 42520.

### 2.2 섹션 순회와 페이지 리셋

`doc.clone()` 스냅샷(snap)으로 불변 참조, doc는 가변 갱신. 섹션마다:
```
body_width = pg.width - margin_left - margin_right
content_h  = max(pg.height - margin_top - margin_bottom, 1)
v_pos      = 0   // 페이지 상대 누적
```
문단마다 `spacing_top`(첫 문단 제외)을 v_pos에 더하고, `fill_nested`로 셀/글상자 내부 줄배치를 먼저 채운 뒤, 표 총높이 계산. **표가 잔여 공간에 안 들어가면**(`v_pos + table_total > content_h`) `v_pos=0`으로 다음 페이지. 표 앵커 문단은 진입 v_pos에 놓고, 표 있는 문단은 `v_pos = anchor_v + table_total`로 커서를 덮어쓴다(겹침 방지). 마지막에 `spacing_bottom` 가산.

### 2.3 문단 한 개 줄배치 (`compute_linesegs`)

줄 메트릭을 첫 글자모양 `base_size`(기본 1000)에서 유도:
```
base          = char_shapes[first_run].base_size (>0)
ls_type       = para_shape.line_spacing_type()   // 0 비율%, 1 고정, 2 여백만, 3 최소
ls_val        = para_shape.line_spacing(버전 인식, 없으면 line_spacing_old 폴백)
line_advance  = { 고정:               max(ls_val, 0) / 2       // 정확값, 0·겹침 허용
                  여백만:             base + max(ls_val, 0) / 2
                  최소:               max(max(ls_val, 0)/2, base) // 자연 높이는 당분간 base 근사
                  비율(ls_val>0):     base * ls_val / 100
                  미지정:             base * 160 / 100 }      // 정품 기본 160%
line_spacing  = max(line_advance - base, 0)
baseline_gap  = base * 85 / 100
seg_width     = max(body_width, 1)
```

셰이핑(`shape_range`)으로 글리프 x_advance를 pt로 재고 `limit_pt = seg_width/100` 기준 그리디 줄바꿈. NB_SPACE 양옆은 줄바꿈 금지. 줄 시작점은 glyph index가 아니라 shaping-cluster source의 UTF-16 길이로 계산해 보조평면 문자·합자·결합문자의 WCHAR offset 보존. 각 줄 방출은 클로저 `place`:
```
if v_pos>0 && v_pos + base > content_h { v_pos = 0 }   // 페이지 리셋(셀은 content_h=MAX라 무발동)
segs.push(LineSeg{ text_start, v_pos, line_height:base, text_height:base,
                   baseline_gap, line_spacing, col_start:0, seg_width, flags:0x0006_0000 })
v_pos += line_advance
```
탭은 `tab::next_tab(tabs, acc, 40)` — 다음 명시 탭 스톱, 없으면 `floor(acc/40)*40 + 40`으로 간다. 빈 문단도 줄 1개를 갖는다. `flags=0x0006_0000`는 정품 본문 줄의 표준 플래그값.

### 2.4 표 높이 (`table_height`, `fill_nested`, `para_line_block`)

`fill_nested`가 먼저 셀/글상자 내부 문단의 줄배치를 채운다(셀 폭 = `cell.width - margins[0] - margins[1]`, 셀마다 v_pos 리셋, `content_h=i32::MAX`로 페이지 분할 없음. 글상자는 폭 `gso_shapes[0].w - 566`).

표 높이 공식(정품 실측 도출):
```
줄블록(문단) = 마지막 줄.v_pos + 마지막 줄.line_height       // para_line_block
rowH        = margins[2](상) + Σ문단 줄블록 + margins[3](하)
표 높이     = Σ_행 max(row_span==1 셀의 rowH) + 566
```
`row_span!=1` 셀은 행 높이 계산에서 제외, 전부 병합인 행은 폴백 `141+1000+141`.

---

## 3. 텍스트 셰이핑 (`shape.rs`)

### 3.1 조각 분할 → 셰이핑 (`shape_range_notes`)

1. **조각 수집**: `para.chars`를 순회하며 (문자모양 ID, 언어 슬롯) 경계로 텍스트를 `Piece`로 모은다. `shape_id_at(para,pos)`는 `char_shape_runs`를 역순 탐색해 `start<=pos`인 마지막 ID. `lang_slot_of(c)`: 한글 0, 라틴 0x0000–0x024F 1, CJK한자 2, 가나 3, 그 외 5. 탭(`ctrl_char::TAB`)은 `InlineItem::Tab`, 각주 앵커는 `note_mark_run`(윗첨자 번호), PAGE `atno`는 `shape_range_page`가 현재 논리 쪽번호 런으로 치환한다.
2. **조각별 셰이핑**(`shape_piece`): `face_id = cs.face_ids[lang]`, `store.resolve(doc,lang,face_id)`로 주 글꼴. 요청 글꼴이 heavy 계열(`is_heavy_name`: 견고딕/헤드라인/Bold 등)이면 `bold` 강제(faux-bold). **글자별 커버리지 폴백**: 주 글꼴이 글리프(.notdef 아님)를 가지면 primary, 아니면 `store.font_covering(c)`. 글꼴 경계마다 별도 `ShapedRun`으로 분할하며 `start_wchar`는 `len_utf16()`(BMP 1, 그 외 2)로 진행.

### 3.2 글꼴 하나로 셰이핑 (`shape_with_font`)

```
base      = cs.base_size (>0 else 1000)
rel       = cs.rel_sizes[lang] (기본 100)
full_size = (base/100) * (rel/100)                       // pt
size_pt   = sup||sub ? full_size*0.65 : full_size
scale     = size_pt / upem
y_raise   = full_size * cs.char_offset(lang)/100
            + (sup ? full_size*0.34 : 0) + (sub ? -full_size*0.16 : 0)
spacing_pt= hwpunit_round(size_hu * cs.spacings[lang] / 100) / 100   // 자간, HWPUNIT half-up
x_scale   = cs.ratios[lang] / 100                        // 장평
```
rustybuzz `shape(&face, &[], buffer)` 후 글리프마다:
```
x_advance = gpos.x_advance * scale * x_scale + spacing_pt
x_offset  = gpos.x_offset  * scale * x_scale
y_offset  = gpos.y_offset  * scale + y_raise
```

### 3.3 문자모양 비트 (`hwp_model::CharShape`, `attr: u32`)

| 효과 | 비트 | 메서드 |
|---|---|---|
| italic | bit0 | `is_italic` |
| bold | bit1 | `is_bold` |
| underline | bits2–3 (1 아래, 3 위) | `underline_kind`/`has_underline`(==1) |
| outline | bits8–10 (≠0) | `has_outline` |
| shadow | bits11–12 (≠0) | `has_shadow` |
| emboss(양각) | bit13 | `is_emboss` |
| engrave(음각) | bit14 | `is_engrave` |
| superscript | bit15 | `is_superscript` |
| subscript | bit16 | `is_subscript` |
| **strike(취소선)** | **비트 안 씀** — 별도 `strike: bool` | `has_strike` |

취소선 비트(18~20)는 DIFFSPEC라 불신뢰. HWP5 reader는 항상 false, HWPX만 visible `<hp:strikeout>`에서 켠다. `shade_color`는 배경 하이라이트(0xFFFFFFFF=없음), `underline_color`/`shadow_color`도 COLORREF. `ShapedRun`이 색·bold·italic·underline·strike·underline_color·shade_color·shadow(Option)·outline·emboss·engrave를 실어 나른다.

### 3.4 하이퍼링크·목록·각주 마커

`hyperlink_ranges`: `%hlk` FIELD_START(ExtCtrl code3) ~ FIELD_END(InlineCtrl code4) 사이 WCHAR 범위. `apply_link_style`이 그 범위의 Run에 밑줄 + `LINK_BLUE=0x00CC0000`. `shape_plain`은 합성 텍스트(수식/마커)용 — 단일 Run 통짜 셰이핑(폴백 분할 안 함, `shade_color=0xFFFFFFFF`로 검은박스 트랩 회피).

목록 마커는 `ListState::marker_for_render`(`hwp-model/src/list.rs`)에서 만든다: 번호(head_type 2)·불릿(3)은 기존과 동일하고, 개요(head_type 1) 문단은 전용 카운터 계열로 기본 수준별 고정 마커(`1.` / `가.` / `1)` / `가)` / `(1)` / `(가)` / `①`)를 단다. 빈 문단은 카운터를 소비하지 않고, 글상자마다 별도 카운터 범위를 쓴다. 개요 `numbering_id`는 아직 비정규화 원시값이므로 사용자 정의 개요, 재시작 규칙, 확인된 한글 14자 이후 순번은 GG-12 정답지 작업으로 남는다. 텍스트 변환(markdown 등)은 `marker()`를 써서 개요를 헤딩 구조로만 둔다.

---

## 4. 도형 드로잉 (`shape_draw.rs`, `gso.rs`)

### 4.1 두 경로: hwp5 직접(raw) vs ShapeGeom(IR)

- **hwp5 직접 경로** (`draw_gso_shapes` → `walk` → `draw_component` → `geometry`): gso 컨트롤의 `raw_children`(OpaqueRecord)를 **렌더 시점에** 파싱. IR·라이터를 안 건드리는 소비단 전용. 좌표 변환 `local 점 → 렌더행렬(T·S·R) → +origin → /100 = pt`.
- **ShapeGeom(IR) 경로** (`draw_ir_shapes` → `ir_shape_path`): hwpx 구조화 도형. 좌표는 이미 페이지 절대 HWPUNIT라 행렬 없이 `(x+px)/100`.

`has_shape(recs)`가 SHAPE_COMPONENT(0x4C) 자식에 기하(SC_LINE 0x4E, SC_RECTANGLE 0x4F, SC_ELLIPSE 0x50, SC_ARC 0x51, SC_POLYGON 0x52, SC_CURVE 0x53)가 있는지 재귀 판정(SC_CONTAINER 0x56 통과). `MAX_DEPTH=16`.

### 4.2 SHAPE_COMPONENT 바이트 레이아웃 (`parse_style`)

```
base = (d[0..4]==d[4..8]) ? 8 : 4          // top-level은 CHID 2회, 묶음 멤버 1회
cnt  = u16 @ base+42                        // scale/rotation 쌍 개수
t    = mat @ base+44 (48바이트, translation)
pair = base+44+48+(cnt-1)*96               // 마지막 scale/rotation 쌍
m    = t.mul( mat@pair.mul(mat@pair+48) )  // T·S·R
bo   = base+92+cnt*96                       // 테두리 오프셋
  color = u32@bo, width = i32@bo+4, lattr = u32@bo+8   // lattr&0x3F≠0 → stroke
fo   = bo+13                                // 채우기(Table 28)
  ft = u32@fo:  ft&1 → 단색(u32@fo+4)  |  ft&4 → gradient  |  ft&2 → 이미지
```
`Mat`은 3×2 [a,b,c,d,e,f], `x' = a·x+b·y+c, y' = d·x+e·y+f`. `rd_mat`은 f64 6개(48바이트). `mul`은 표준 어파인 합성.

### 4.3 기하 레코드 (`geometry`, local HWPUNIT)

- **SC_LINE**: start=p(0), end=p(8). 같으면 None.
- **SC_RECTANGLE**: byte 곡률% + 4×(x,y)@p(1),p(9),p(17),p(25). 곡률>0 & radius>1 → `rounded_quad_path`.
- **SC_POLYGON**: u16 n@0, 점 @4+i*8. 닫힌 경로.
- **SC_ELLIPSE**: u32 attr + center@p(4) + ax1끝점@p(12) + ax2끝점@p(20) → `ellipse_path(cx,cy, ax1-c, ax2-c)`.
- **SC_ARC**: byte arctype + center@p(1) + start@p(9) + end@p(17) → `arc_path`.
- **SC_CURVE**: u16 n@0, 점 @2+i*8. 폴리라인 근사.

### 4.4 타원 — KAPPA 4-베지에 (`ellipse_path`)

`KAPPA = 0.5522847498 = 4/3·tan(45°/2)`. 중심 C와 켤레 두 축 벡터 a1, a2. 앵커는 C±a1, C±a2. 각 90° 호를 큐빅으로:
```
MoveTo P0 = C+a1
Cubic( C + a1 + k·a2,  C + a2 + k·a1,  P1=C+a2 )
Cubic( C + a2 − k·a1,  C − a1 + k·a2,  P2=C−a1 )
Cubic( C − a1 − k·a2,  C − a2 − k·a1,  P3=C−a2 )
Cubic( C − a2 + k·a1,  C + a1 − k·a2,  P0 )  Close
```
**켤레축 어파인 불변**: 제어점을 축 벡터의 선형결합으로만 정의하므로, 원래 원에 임의 어파인(비수직 켤레축 포함)을 걸어도 베지에가 정확히 변환된다. 회전·전단된 타원도 정확.

### 4.5 원호 (`arc_path`)

중심 C, 시작/끝 점. `r=|start−C|`, `t0=atan2(s.y,s.x)`, `sweep = atan2(e)−t0`를 짧은 쪽 `[−π,π]`로 정규화. `segs = ceil(|sweep|/(π/2))`, `dphi=sweep/segs`, `alpha = 4/3·tan(dphi/4)`. 각 세그먼트 제어점은 접선 `T'(θ)=r(−sinθ, cosθ)` 기준 `P ± alpha·T'`:
```
c1 = (C + r·(cosθ, sinθ)) + alpha·r·(−sinθ,  cosθ)
c2 = (C + r·(cosθ₁,sinθ₁)) − alpha·r·(−sinθ₁, cosθ₁)
```

### 4.6 ir_shape_path (ShapeGeom)

- **Arc, points≥3**: `points[0]=center, [1]=ax1, [2]=ax2`(켤레축). 1/4 타원호를 **단일 큐빅**으로. points 없으면 타원 폴백.
- **Ellipse / Arc(폴백)**: center=(x0+w/2, y0+h/2), 축 (w/2,0),(0,h/2) → `ellipse_path`.
- **Rect**: `radius = (round_ratio/100)·min(w,h)/2`, >0.1이면 `rounded_quad_path`, 아니면 직사각형.
- **Line**: points≥2면 폴리라인, 아니면 (x0,y0)→(x0+w,y0+h).
- **Polygon/Curve**: 닫힌 폴리곤.

`rounded_quad_path`: 4점 각 모서리를 인접변 절반으로 캡한 반경으로 진입/이탈점 계산, 90° 호를 KAPPA 큐빅으로. 화살촉 `arrowheads`/`arrow_triangle`은 끝점 방향 이등변 삼각형(`size=max(width*4,5)`). 점선 `dash_pattern(style,width)`: 1 파선, 2 점선, 3 일점쇄선, 4 이점쇄선, 5 긴파선(굵기 비례).

### 4.7 그러데이션·이미지 채움

`parse_gradient`(Table 28): type(i16) angle(i16) ... num(i16), num>2면 INT32[num] 위치+정규화, 이어 COLORREF[num]. `radial = (gtype==1)`. `parse_image_fill`은 끝부분 4바이트 정렬에서 유효 BinData ID를 역탐색해 `resolve_bin`.

### 4.8 gso 공통 속성 (`gso.rs`, `parse_gso_box`)

CTRL_HEADER 페이로드(ctrl_id 이후) 20바이트: `attr(4) vert_offset(4) horz_offset(4) width(4) height(4)` (모두 i32 LE, HWPUNIT). hwp5 `parse_picture_gso`와 **동일 레이아웃**. `attr` bit0=treat_as_char(인라인), bits3–4=vert_rel_to(0 PAPER/1 PAGE/2 PARA), bits8–9=horz_rel_to(0 PAPER/1 PAGE/2 COLUMN/3 PARA).

---

## 5. 폰트 (`fonts.rs`, `FontStore`)

### 5.1 로딩과 HWP_FONT_DIR

`FontStore::new()`는 `fontdb::Database`에 `load_system_fonts()`. `load_dir`으로 추가 디렉터리. CLI(`commands/convert.rs`, `render.rs`)는 `--font-dir` 미지정 시 `HWP_FONT_DIR` 환경변수(없으면 프로젝트 `fonts/`)를 기본 로드 → 번들 **함초롬바탕/함초롬돋움**. 골든 테스트도 `HWP_FONT_DIR`를 읽는다.

### 5.2 해석 체인 (`resolve`)

`(lang_slot, face_id)` → `doc.header.fonts[lang][face_id]`의 이름·대체이름. 후보 순서:
1. 요청 이름 → 2. 대체(alt) 이름 → 3. **계열 폴백**(`classify`로 고딕/명조 추정): 고딕이면 `GOTHIC_FALLBACKS`(함초롬돋움/Apple SD Gothic/나눔고딕…), 명조면 `SERIF_FALLBACKS`(함초롬바탕/AppleMyungjo/나눔명조…), 불명이면 `FALLBACKS`(함초롬바탕 우선) → 4. 최후: 시스템 SansSerif → 5. 실패.

`classify`: 한국어 키워드(돋움/고딕/굴림 vs 바탕/명조/궁서) + 라틴(gothic/dotum vs batang/myeongjo/serif). **조용한 대체 금지** — 모든 결과를 `report`에 (`글꼴 일치`/`글꼴 대체: A → B`). 결과는 `resolved: HashMap<요청이름, Option<Arc<LoadedFont>>>`에 캐시.

`font_covering(c)`: 특정 글자 두부(□) 방지용 커버리지 폴백(함초롬→Noto CJK→나눔…), 문자별 캐시(`\u{1}cover:` 키). `LoadedFont{ data: Arc<Vec<u8>>, index: u32, family }` — `fontdb::Source`(File/Binary/SharedFile)에서 바이트 로드, ID별 `loaded` 캐시.

---

## 6. 백엔드

### 6.1 DisplayList (`display.rs`) — 레이아웃↔백엔드 계약

```
PageList{ width_pt, height_pt, items: Vec<Item> }
Item = Glyphs{x,y,run}                       // 베이스라인 원점
     | Rect{x,y,w,h,fill:COLORREF}
     | Line{x1,y1,x2,y2,color,width}
     | Image{x,y,w,h,data:Arc<Vec<u8>>}       // 인코딩 원본
     | Path{commands:Vec<PathCmd>, fill:Option<Fill>, stroke:Option<Stroke>}
PathCmd = MoveTo|LineTo|CubicTo|Close
Fill = Solid(COLORREF) | Gradient(Gradient{radial, angle_deg, stops:[(0..1, COLORREF)]})
Stroke{ color, width, dash:Vec<f32> }
```
`Gradient::color_at(t)`는 stop 사이 선형보간. `path_bbox(cmds)`가 그러데이션 배치용 경계상자.

### 6.2 PNG (`png.rs`, tiny-skia)

`px_scale = dpi/72`, 픽스맵 `ceil(pt·px_scale)`, 흰 배경. 글리프는 `ttf_parser::Face::outline_glyph` + `OutlinePath`(OutlineBuilder)로 tiny-skia Path 추출. 변환:
```
t = scale(glyph_scale·x_scale, −glyph_scale)   // y-up 뒤집기 + 장평
  ∘ (italic ? skew(−0.2126, 0) : I)            // ITALIC_SKEW
  ∘ translate(pen_x + x_offset + dx, y − y_offset + dy)
  ∘ scale(px_scale, px_scale)
```
`glyph_scale = size_pt/upem`. **합성 굵게** = fill + `stroke(size_pt·0.045/glyph_scale)` (BOLD_STROKE 4.5%). **외곽선** = stroke만(`0.025`). **shade** = 글리프 뒤 Rect. **shadow** = 오프셋 0.06em 복사. **양각/음각** = 흰 하이라이트 오프셋(양각 좌상 −0.05, 음각 우하 +0.05). 이미지는 `image::load_from_memory` → premultiplied RGBA, 디코드 실패 시 자홍 placeholder. 그러데이션은 `gradient_shader`(Linear/Radial, bbox 기준, `px_scale` transform).

### 6.3 SVG (`svg.rs`)

뷰어 폰트 의존 제거를 위해 글리프를 윤곽선 `<path>`로. `(font_ptr, glyph_id)→d` 캐시. 변환은 `matrix(a 0 skew_c dd e f)` (`a=s·x_scale, dd=−s, s=size_pt/upem`). bold=fill+stroke(0.045·upem), outline=stroke만(0.025·upem). 이미지는 `sniff_mime` + 자체 `base64` data URI. 그러데이션은 `<linearGradient>/<radialGradient>` userSpaceOnUse. `hex_color`는 COLORREF→`#rrggbb`(BGR 스왑).

### 6.4 PDF (`pdf.rs`) + CFF

**폰트 임베드**: 고유 폰트별 `FontInfo` 수집 → 사용 글리프 `GlyphRemapper::remap` + `orig_to_unicode`(원문 우선, 부분 런은 `reverse_cmap` 보완). `subsetter::subset`로 서브셋(실패 시 전체 임베드 CID=GID). 아웃라인 종류별:
- **glyf(트루타입)**: `CIDFontType2` + `FontFile2`(Length1).
- **CFF(OTF, `CFF ` 테이블)**: `CIDFontType0` + `FontFile3`(Subtype=OpenType). `face.tables().cff.is_some()`로 분기.

둘 다 **Type0(composite) + Identity-H + ToUnicode CMap**. 객체: Type0 → CIDFont(W 폭배열, `glyph_hor_advance·1000/upem`) → FontDescriptor(SYMBOLIC, bbox·ascent·descent·cap_height ×`1000/upem`) → FontFile(FlateDecode) → ToUnicode(`UnicodeCmap`). 서브셋은 BaseFont에 6글자 태그(`subset_tag`) 접두사.

**콘텐츠**: y 뒤집기 `h−y`. 글리프는 `write_glyph_run` — `begin_text`, `set_font(size_pt)`, `set_horizontal_scaling(x_scale·100)`(Tz 장평), 렌더모드(bold=FillStroke `size·0.045`, outline=Stroke `0.025`, 기본 Fill), 각 글리프 `set_text_matrix([1,0,shear,1, pen_x+x_offset, page_h−(y−y_offset)−dy])` 후 `show(out_gid.to_be_bytes())`. `out_gid` = 서브셋이면 재매핑 GID, 아니면 원본. advance는 png/svg와 동일 `pen_x += x_advance`로 픽셀 일치.

**경로/이미지**: `pdf_emit_path`(CubicTo도 y 뒤집기). 그러데이션은 경로 클립 후 `pdf_gradient_bands`(48띠 선형 / 동심원 방사, `pdf_circle` KAPPA 0.552285). 점선 상태는 항목 뒤 `set_dash_pattern([])`로 실선 복원. 이미지: JPEG(gray/RGB)는 `DctDecode` 원본, 그 외는 디코드 후 RGB(+알파 SMask) FlateDecode. `jpeg_info`가 SOF 마커에서 (w,h,comps) 파싱.

---

## 7. 재구축 시 지켜야 할 핵심 불변식

1. **셰이핑 advance 단일 출처**: lineseg 줄바꿈(`compute_linesegs`)·layout 배치(`place_wrapped`)·세 백엔드가 **동일한** `glyph.x_advance` 누적을 써야 픽셀이 일치한다. 탭은 어디서나 `tab::next_tab`(명시 탭 스톱 우선, 없으면 40pt 기본 간격 폴백).
2. **캐시 v_pos 존중 vs 흐름 커서**: 저장된 lineseg는 `baseline = body_top + (v_pos+baseline_gap)/100`을 신뢰하되, `max(stored, content_bottom+gap)`으로만 아래로 민다(위로 끌어올리지 않음 — 키 큰 글상자 드리프트 방지). 셀 내부(`layout_box_para_iter`)는 흐름 하한을 흐름배치 콘텐츠에만 적용.
3. **페이지 상대 v_pos**: 합성·렌더 모두 페이지마다 v_pos를 0으로 리셋. 섹션 단조 누적하면 한글이 “손상” 판정.
4. **표 상수 566**: 표 총높이에 반드시 `TABLE_BLOCK_PADDING` 1회 가산.
5. **켤레축 어파인 불변**: 타원/원호 제어점은 축 벡터 선형결합으로만 — 회전·전단 타원 정확도의 근거.
6. **조용한 대체 금지**: 폰트 대체·이미지 디코드 실패·미지원 컨트롤은 모두 report/warning 또는 자홍 placeholder로 가시화.
7. **COLORREF 0xFFFFFFFF**: shade는 “없음”, 래스터 텍스트 색은 검정 폴백 — 문맥별로 다르게 해석.
8. **PARA_SHAPE 여백만 /200**, 그 외 모든 HWPUNIT는 /100.
