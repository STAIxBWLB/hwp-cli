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

상단 합과 하단 합은 lineseg 합성이 페이지 나누기에 쓰는 `lineseg::body_box`에서 온다. 그래서 그려지는 본문과 합성 캐시가 본문 끝 위치에서 어긋나지 않는다([#423](https://github.com/STAIxBWLB/hwp-cli/issues/423)).

상태 변수: `prev_v_pos=-1`(페이지 리셋 감지), `content_bottom=body_top`(흐름 커서), `paras_on_page=0`, `page_notes: Vec<&Note>`, `list_state: ListState`.

### 1.2 페이지 나누기 규칙 (네 가지 트리거)

1. **본문 넘침**: `content_bottom > body_bottom && paras_on_page>0` → 각주 렌더 + 페이지 가구(`PageNumberState::finish`: 머리/꼬리말·쪽번호) + 페이지 push, 상태 리셋.
2. **명시 쪽 나누기**: `para.header.break_type & 0x04 != 0 && paras_on_page>0`.
3. **캐시 v_pos 리셋**: 저장된 lineseg를 그릴 때 `seg.v_pos < prev_v_pos && !page.items.is_empty()` → 정품 멀티페이지는 페이지마다 v_pos가 0으로 리셋되므로 감소를 페이지 경계로 본다.
4. **폴백 줄이 본문 하한을 넘을 때**: 캐시 없는 밴드에서 `baseline + max_size*0.4`가 `body_bottom`을 넘는 줄은 새 페이지를 열고 문단의 나머지를 데려간다. 페이지의 첫 줄은 트리거하지 않는다(빈 페이지가 생기고 이미 그린 목록 마커가 고아가 된다). lineseg가 아예 없는 우리 생성 문서의 문단 중간 나누기로, 이것이 없으면 그런 문단은 페이지 하단 아래로 통째로 그려지고 *다음* 문단에서만 쪽이 넘어갔다([#282](https://github.com/STAIxBWLB/hwp-cli/issues/282)).

### 1.3 문단 처리 분기

문단별로 `footnote::para_marks/para_notes`(각주 마커/노트 수집), `tab::tab_stops`, `para_geometry`, `hyperlink_ranges`, `list_state.marker_for_render`를 준비한 뒤:

- **`line_segs`가 비었을 때(폴백)**: 빈 문단이면 `content_bottom`을 자신의 space-before와 합성이 쓰는 줄 하나(`lineseg::paragraph_metrics`의 advance)만큼 전진시킨다 — 흐름이 기록한 앵커를 흐름 자신이 소비한다는 뜻이라, 기본 문단의 16pt 전진은 그대로다([#422](https://github.com/STAIxBWLB/hwp-cli/issues/422)). space-after와 raw/반값 간격 단위는 이 전진에 넣지 않는다(#415). 아니면 `shape_range_notes`로 전체 셰이핑 → `place_wrapped`로 `body_width` 기준 그리디 줄바꿈. `baseline_y = content_bottom + spacing_top + max_size*1.2`, `content_bottom = last_y + max_size*0.4 + spacing_bottom`. 정렬(가운데/오른쪽)은 한 줄에 들어갈 때만 보정. 줄바꿈은 스크래치 페이지에서 수행하며 줄마다 `(item 인덱스, baseline)`을 함께 모으고, 위 트리거 4에 따라 줄 단위로 실제 페이지에 옮긴다. 모든 나누기는 `push_page_checked`를 거쳐 세그먼트 기록기의 페이지 서수가 유도 가능하게 유지된다. 본문 상자보다 큰 줄은 어떤 나누기로도 담을 수 없다 — 그대로 그리고 편차만 `paragraph_line_content_overflow`로 보고한다(표 셀의 `table_cell_content_overflow`와 같은 계약).
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
3. **측정 패스**: 셀마다 스크래치 `PageList`에 `layout_box_paragraphs`로 그려 실측 내용 높이를 잰다. 이 측정은 **문서가 크기를 정하지 않은 행**을 채운다. 줄 배치 캐시가 있는 셀의 행은 한글이 계산해 둔 최종 높이이므로 그대로 두는데, 늘리면 아래 모든 행과 다음 쪽들의 조각 격자까지 밀리기 때문이다(실측: 개체를 담은 셀 하나가 행을 190pt 늘려 한 쪽의 조각을 통째로 이동). 실측이 저장 높이를 넘으면 내용은 그대로 그리고 편차만 typed 경고 `table_cell_content_overflow`로 보고한다. 단 저장 높이는 하한일 뿐이다: 셀 자신의 캐시는 한글이 줄을 그린 자리라, 행은 적어도 캐시 글줄 높이(조각별 합; 조각은 쪽 시작 플래그, `v_pos` 역행, 전진하지 않는 문단 시작에서 나뉜다 — 한글이 다음 쪽에 이어 그린 내용, #233)만큼은 크다. 텍스트 구간이 빈 캐시 레코드는 높이를 더하지 않지만, 빈 문단 자신의 줄은 예외다: 글자는 없어도 줄 하나는 차지하므로, 마지막이거나 유일한 빈 문단의 줄 하단이 행에 계속 기여하고 셀이 여백만 남도록 접히지 않는다([#422](https://github.com/STAIxBWLB/hwp-cli/issues/422)). 한글 hwpx 내보내기는 더 큰 캐시 아래 선언 최소 높이를 유지하고, 편집으로 재합성된 셀은 편집 전 높이를 유지한다(#245); 두 경우 모두 행을 캐시 전체 높이까지 늘리고, `layout_box_para_iter`가 이어지는 조각을 앞 줄 아래(`run_shift`, 한 문단 안) 또는 앞 문단 아래(`para_shift`)에 그려 셀 전체가 한 행 안에 머문다. 이 하한을 넘는 실측(중첩 개체, 우리 줄바꿈)은 종전대로 행을 늘리지 않고 보고만 한다. 캐시가 없는 셀(우리가 만든 문서 등)은 저장 높이도 레이아웃 산출물이 아니므로 종전대로 실측으로 행을 늘린다. `row_span>1`은 스팬 합 부족분을 마지막 스팬 행에 가산 — 역시 캐시가 없거나 캐시 하한이 저장 높이를 넘을 때만.
4. 누적 오프셋 `col_x = prefix_sums(col_w, x)`, `row_prefix = prefix_sums(row_h, 0)` (조각마다 베이스 y를 더하므로 같은 prefix를 모든 쪽 조각이 공유).
5. 셀마다: **배경 Rect** → **내용**(여백 + 세로정렬 `voff`: `(list_attr>>5)&0x3` 0 위/1 가운데 avail*0.5/2 아래 avail) → **4변 테두리 Line**(`width_mm()*72/25.4`) → **대각선**(`diagonal_dirs(attr)`: slash bits2~4, backslash bits5~7).
6. **쪽 분할**(본문 흐름에서만, `TableSplitCtx` 경유; 셀/글상자 안 중첩 표는 나누지 않음): 표가 `body_bottom`을 넘으면 `Table::page_break_policy()`(`attr` bits 0-1)로 결정. NONE이면 통째로 다음 쪽으로(04 §6.1 불변식), 한 쪽보다 클 때만 행 분할로 폴백. TABLE/CELL은 행 경계에서 나눔. 셀 내부 분할은 TABLE 정책만 미지원이고, CELL은 한 쪽보다 큰 단일 행을 기존 cached line 경계에서 이어 그림(soft `v_pos` 채움, 행 단위 빈 continuation fragment, span 인지 채움, 쪽 하단 sliver 봉인 — 계약은 21-pdf-parity §6). 안전하지 않은 cache나 단편화된 행과 교차하는 row-span은 `table_cell_fragmentation_incomplete`로 보고하고, 새 쪽에도 안 들어가는 묶음은 넘치며 `TableRowTooTallClipped` 보고. `treat_as_char` 표는 "한 글자"라 나누지 않음(GE-8). 분할마다 표준 쪽 마감 순서(노트 → 가구 → `push_page_checked` → 흐름 상태 리셋)를 거치므로 다음 문단의 한글 경계 플래그가 흡수돼 이중 분할되지 않는다. `repeat_header()`면 선두의 "모든 셀이 `Cell::is_header()`(`list_attr` bit18)"인 행들을 이어지는 쪽 `body_top`에 다시 그린다. `row_span`이 가로지르는 후보 경계는 제외하므로 병합 셀은 절단되지 않고 하나의 분할 불가 행 묶음으로 이동한다. 이 묶음이 한 쪽보다 크면 `TableRowTooTallClipped`로 보고한다. 방출되는 모든 조각은 셀의 4변 테두리를 그대로 갖는다: 한글은 각 쪽의 조각을 자기 가로선으로 닫아 분할된 행이 경계 양쪽에서 닫힌 상자로 보인다(오라클 실측 — 우리가 긋지 않던 자리에 쪽 하단과 연속 상단 모두 전폭 가로선이 있다). 세로정렬과 대각선은 여전히 셀 전체의 속성이라 조각이 셀 전체일 때만, 또는 첫 조각에만 적용한다.

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

한글은 이 컨트롤들을 캐럿 위치에 넣으므로, 정품 문서는 이를 본문 흐름이 아니라 중첩 문단 리스트(글상자·도형) 안에 담는 일이 흔하다. 그 리스트는 앵커 문단에 묶여 같은 쪽에 놓이므로, 문단 자신의 컨트롤과 함께 중첩 리스트를 깊이 제한(8) 안에서 훑는다. 표 셀은 의도적으로 제외한다: 표는 쪽을 걸쳐 나뉘므로 뒤쪽 행의 컨트롤은 뒤쪽 쪽에 속하는데, 앵커 시점에 적용하면 `nwno` 재시작이나 `pghd` 해제가 엉뚱한 쪽에서 일어난다. 셀 컨트롤을 제대로 반영하려면 조각 배치 시점에 훑어야 하며, 아직 그런 문서는 관측되지 않았다.

위치형 쪽번호는 `Furniture::render`가 쓰는 것과 같은 머리말/꼬리말 밴드에 그린다: 머리말 밴드는 `margin_top`에서 시작하고, 꼬리말 밴드는 `page_height - margin_bottom`에서 끝난다. 밴드 안에서는 바깥 가장자리에 붙인다 — 꼬리말은 글자 아래끝이 아래 여백선에 놓이며 한컴 오라클과 0.3pt 이내로 일치하고, 머리말은 같은 규칙을 대칭 적용한다(머리말은 아직 오라클 근거가 없다). ascent·descent는 face 메트릭이 아니라 런 크기로 근사한다.

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
content_h  = max(pg.height - margin_top - margin_bottom - margin_header - margin_footer, 1)
v_pos      = 0   // 페이지 상대 누적
```
`content_h`는 `lineseg::body_box`의 높이이며, `layout_document_pages`가 빼는 네 여백과 같다. 따라서 머리말·꼬리말 밴드는 두 경로 모두에서 줄 공간이 아니다([#423](https://github.com/STAIxBWLB/hwp-cli/issues/423)).

문단마다 `spacing_top`(첫 문단 제외)을 v_pos에 더하고, `fill_nested`로 셀/글상자 내부 줄배치를 먼저 채운 뒤, 표 총높이 계산. **표가 잔여 공간에 안 들어가면**(`v_pos + table_total > content_h`) `v_pos=0`으로 다음 페이지. 표 앵커 문단은 진입 v_pos에 놓고, 표 있는 문단은 `v_pos = anchor_v + table_total`로 커서를 덮어쓴다(겹침 방지). 마지막에 `spacing_bottom` 가산.

### 2.2.1 본문 흐름 표가 있는 섹션의 사전 앵커

본문 흐름 표(`placement.treat_as_char == false`)를 가진 섹션은 위 커서를 쓸 수 없다. 렌더러가 그 표의 행/셀 조각을 나눠 배치하므로 문단 위치도 같은 레이아웃 패스에서 나와야 한다. `layout::synthesized_body_anchors`가 본문 캐시를 모두 비운 복사본에 레이아웃을 한 번 돌려 문단마다 확정된 첫 줄 상단(`BodyAnchor::v_pos`)과 그 줄이 페이지를 열었는지(`starts_page`)를 기록한다. `synthesize_linesegs`는 그런 섹션의 각 문단을 앵커에서 시작하고, 앵커가 페이지를 연 경우 `LineSeg.flags` bit0을 세운다(한글문서파일형식 5.0 §4.3.4). HWP5 원본은 기존 커서 경로를 그대로 쓰므로 정품 캐시와 HWP5 동일성은 건드리지 않는다. 앵커는 그 문단의 *첫 줄*만 담으므로, 여러 줄 문단의 이후 줄은 `compute_linesegs` 단독으로 페이지를 나누며 그 자체 적합 판정이 결정한다(§2.3). 앵커를 만든 패스와는 독립적이다([#423](https://github.com/STAIxBWLB/hwp-cli/issues/423)).

사전 패스 결과는 문단마다 `Option<BodyAnchor>` 슬롯을 갖는 `BodyAnchors`와 `completed` 플래그다(#426). 슬롯이 `None`이면 그 문단을 방문하기 전에 패스가 멈춘 것이므로, 채워지지 않은 항목을 정상적인 페이지 상단 앵커(`v_pos = 0`, `starts_page = false`)로 읽을 수 없다. `completed`는 페이지나 표시 항목 예산이 페이지 전환을 거부하면 거짓이 된다. 모든 앵커를 이미 기록한 뒤에 멈춘 경우도 포함하며, 다음 페이지가 필요한 본문 흐름 표가 정확히 그 경우다.

정책: 미완 사전 패스는 앵커를 게시하지 않는다. 앵커가 완전하지 않은 본문 흐름 표 섹션은 위 기본 커서 경로로 되돌아가고 섹션마다 `anchor_prepass_incomplete`(`section=<n>`, 심각도 `incomplete`, 단계 `layout`)를 한 번 보고하며, 사전 패스 자체의 예산 거부는 `layout_budget_exceeded`로 전달된다. 거부(typed refusal) 대신 폴백을 택한 이유는 기본 커서가 그 섹션들의 기존 배치이기 때문이다. 가정상의 중단은 렌더 실패가 아니라 위치 열화로 끝난다. 사전 패스는 전용 누산기를 쓰므로 호출자의 페이지/표시 항목 예산을 소비하지 않고 문서의 다른 경고를 두 번 보고하지 않는다. 전달하는 것은 앵커를 무효화하는 진단뿐이다.

### 2.2.2 편집된 HWP5 문서는 남긴 앞부분에서 이어 합성한다

HWP5 원본에 `ahwp edit`를 걸면 문서 전체를 다시 합성한 뒤, 편집이 건드리지 않은 문단을 저장된 레코드 그대로 되돌린다(`convert::restore_unchanged_native_paragraphs`). 그래서 새 PARA_LINE_SEG 레코드는 편집이 옮긴 부분에만 생긴다. 그 이음매 양쪽 모두 페이지 기준 `v_pos`를 담고 있고, 섹션을 0에서 다시 시작하는 합성은 첫 편집 문단을 앞 문단의 저장 캐시 *위*에 놓는다. `layout_document_pages`는 그 되돌아간 걸음을 페이지 경계(`seg.v_pos < prev_v_pos`)로 읽어 편집 내용을 다음 페이지에 그린다([#462](https://github.com/STAIxBWLB/hwp-cli/issues/462)).

수정은 새 기하 규칙 없이 한 규칙만 더한다. 작성기가 되돌릴 앞부분을 측정해 섹션마다 `lineseg::BodySeed` 하나를 넘긴다(`synthesize_linesegs_seeded`는 `lineseg::BodyFlow { seeds, preserve_nested }`를 받는다). 시드는 섹션, 편집이 흐름을 옮긴 첫 본문 문단, 그리고 그 문단이 시작할 커서를 담는다. 커서는 남긴 앞부분 마지막 문단을 `lineseg::saved_body_cursor`로 측정하며, 그것은 위 §2.2 순회가 쓰는 단일 규칙 `lineseg::body_cursor`를 그대로 쓴다. 표를 앵커한 문단이면 진입 위치에 표 높이를 더하고(표 뒤에서 글은 줄 커서로 이어지지 않는다), 아니면 줄 합성기가 도달한 커서를 쓰며, 여기에 그 문단의 space-after를 더한다.

순회는 그 줄 커서를 넘겨받아 쓰고, 커밋된 줄에서 다시 읽어내지 않는다. 둘은 같은 수가 아니다. 커밋된 줄은 `line_spacing = max(advance - base, 0)`으로 직렬화되므로 `v_pos + line_height + line_spacing`은 `max(base, advance)`로 읽힌다. 고정 줄간격이 기준 줄보다 작거나 0이면 커서를 다시 유도하는 순간 뒤따르는 모든 문단이 움직이고, 시드 없는 합성(HWPX·markdown → HWP)의 출력이 바뀐다. `saved_body_cursor`가 저장 레코드에서 그 값을 재구성하는 것은 저장 캐시에 다른 필드가 없기 때문이며, 그 경로는 시드로만 쓰이고 시드 없는 순회에는 쓰이지 않는다([#509](https://github.com/STAIxBWLB/hwp-cli/pull/509)).

되돌리기는 합성 앞뒤로 두 번 돈다. 앞에서는 편집이 건드리지 않은 모든 문단이 저장 레코드를 되찾으므로, 출력이 유지할 중첩 캐시가 제자리에 있고 순회는 그 캐시를 측정한다(`BodyFlow::preserve_nested`는 캐시가 이미 있는 중첩 목록을 다시 합성하지 않고 그대로 쓰며, 유지된 목록 문단도 자기 블록만큼 목록 커서를 전진시킨다). 편집된 문단은 그 문서에서 캐시가 없으므로, 남아 있는 캐시는 작성기가 되돌린 것이다. 그것을 측정해야 유지된 표의 높이가 아래 문단을 배치할 때 쓴 높이와 같아진다. 합성 뒤에는 본문 캐시만 다시 되돌린다. 이음매 앞부분은 저장 `v_pos`를 되찾고, 그 문단부터는 합성기가 만든 본문 캐시를 그대로 쓴다. 이음매 뒤 문단은 순회가 보는 문서에서 캐시가 없고(`SuffixCache::Drop`), 합성된 캐시는 그 뒤에 그대로 남는다(`SuffixCache::Keep`). 편집이 건드리지 않은 표 셀과 일반 목록의 문단은 이음매 양쪽에서 되돌리며, `instance_id`, 완전 일치, 그리고 목록 길이가 같을 때만 인덱스 순으로 대응시킨다. 캡션은 편집이 건드리지 않은 문단 전체가 함께 올 때만 되돌아간다. 편집이 바꾼 컨트롤 안의 캡션은 여기서 다시 쓰지 않는다.

중첩 목록은 스스로 판단하며 답이 셋이다(`lineseg::ListCaches`). "캐시를 유지한다"에 서로 다른 질문 셋이 접혀 있기 때문이다. 어느 문단이 자기 캐시를 유지하는가, 하위 표 셀은 무엇을 하는가, 유지한 캐시가 목록 커서를 전진시키는가. `Recompute`는 목록의 모든 문단을 다시 합성하고, `KeepInPlace`는 문단 자신의 캐시는 두되 커서는 건드리지 않으며, `KeepMeasured`는 캐시된 앞부분을 유지하고 유지한 블록만큼 커서를 전진시킨 뒤 첫 캐시 없는 문단부터 다시 합성한다. 편집이 없는 문서는 맨 위에서 `Recompute`를 쓰고, 그것이 캡션과 일반 목록에는 `KeepInPlace`, 표 셀에는 `Recompute`로 대응된다. 이것이 기존 동작이므로 캡션, 일반 목록, 그 안에 중첩된 표는 예전과 똑같이 합성된다([#509](https://github.com/STAIxBWLB/hwp-cli/pull/509)). `KeepMeasured`를 쓰는 것은 편집된 문서의 본문뿐이며, 그 경로 위의 캡션과 셀까지 유지된다.

한 목록 안에서 이음매는 문단별이 아니라 위치 기준이다. 목록의 저장 `v_pos`는 누적값이라, 그 목록의 첫 *움직인* 문단 앞부분만 아직 목록을 설명한다. `first_moved_paragraph`는 대응이 자기 인덱스에 없거나, 내용이 다르거나, 대응이 아예 없을 때 그 문단을 움직인 것으로 본다. 위에서 문단을 넣거나 지워 생긴 밀림도 포함되며, 이것이 유지된 형제가 편집이 바꾼 블록 아래에서 잰 `v_pos`를 그대로 들고 있지 못하게 한다. "내용"은 그 문단의 줄을 결정하는 필드들이고, 줄 캐시는 지금 계산하는 답이며 PARA_TEXT의 "목록의 마지막 문단" 비트는 문단이 아니라 목록을 따라가므로, 형제를 넣으면 아무도 건드리지 않은 문단에서 그 비트가 뒤집힌다.

문단 내용이 같아도 그 문단의 로컬 부모 context가 같아야 캐시를 복원할 수 있다. 셀 문단은 셀 폭, 네 방향 여백, `list_attr`, 미해석 `header_tail`을 비교한다. Generic 문단 목록은 첫 `gso_shapes` 항목의 폭(항목 존재 여부 포함)과 목록의 미해석 `header_data`를 비교한다. 하나라도 다르면 해당 목록의 이음매를 첫 문단으로 둔다. 합성 전에는 그 목록 문단 자신의 저장 줄 캐시를 버리고, 합성 후에는 새 줄 캐시를 유지한다. 따라서 `split_cell`이나 폭만 바꾼 `style_tables` 편집이 텍스트와 문단 서식이 같다는 이유로 이전 상자에서 잰 캐시를 되살리지 않는다. 셀의 행·열 좌표 이동이나 테두리만 바뀐 경우에는 셀 내부 줄 캐시를 무효화하지 않는다. 유효한 하위 컨트롤은 저장 레코드를 계속 유지하므로, 바깥 셀 context가 바뀌어도 그대로인 중첩 표의 큰 셀 캐시를 다시 합성할 필요는 없다. 이는 로컬 유효성 비교이며 미해석 목록 헤더의 새 해석이나 네이티브 기하 규칙 추가가 아니다. 이 회귀 검사는 한글에서 열리는 동작을 입증하지 않는다.

유지된 표 앵커는 표를 자기 줄 블록 안에 담으므로 `saved_body_cursor`는 남긴 마지막 줄의 하단을 하한으로 쓴다. 앵커 줄의 높이가 셀 높이 합보다 클 수 있고(공개 픽스처에서 4412 HU 표가 5100 HU 줄 아래에 있다), 그 뒤 문단은 출력이 유지하는 줄 안에서 다시 시작하면 안 된다.

지켜야 할 결과. 편집이 없는 문서는 이 경로에 들어오지 않으므로 HWP5 동일성은 바이트 그대로이고, 시드 없는 합성(HWPX·markdown → HWP)은 이전과 같은 커서 규칙을 쓴다. 섹션 첫 문단을 고친 경우 시드가 필요 없고(어차피 0에서 시작한다), 저장 캐시가 없는 앞부분 문단은 추측 대신 시드를 만들지 않는다. 시드는 본문 상자(`0..=content_h`) 안에 들어올 때만 적용하고 그 주변 산술은 포화한다. 저장 커서는 믿을 수 없는 바이트에서 오므로, 본문 밖 커서는 넘침이 아니라 페이지 시작으로 열화해야 한다. 시드 문단에 명시적 쪽 나눔이 있으면 그쪽이 여전히 우선한다. 시드는 합성기 자신의 간격 단위를 이어 쓰므로, [#415](https://github.com/STAIxBWLB/hwp-cli/issues/415)의 단위 보정 전까지는 이음매가 한글 저장 간격과 문단 간격의 절반만큼 어긋날 수 있다. 본문 흐름 표가 있는 섹션은 커서 대신 사전 앵커를 읽으므로(§2.2.1) 시드가 효력이 없다.

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

페이지 리셋은 줄 자체의 블록 `line_height`를 예약한다. 정품 저장 캐시는 기준선을 `baseline_gap`에 두고 descent가 `line_height`에서 끝나므로, 그것이 한글이 그 줄에 기록한 블록이다. `layout.rs` 폴백 플래너는 다른 블록을 예약한다. 흐름 커서 위 `1.2 x size`에 기준선을 두고 `0.4 x size` descent를 `body_bottom`과 비교하므로, 두 패스는 페이지의 마지막 줄을 다르게 옮길 수 있다. 로컬 정품 표본 중 본문 하단에서 문단을 끊는 것은 없다(로컬 코퍼스의 문단 중간 페이지 리셋은 모두 하단보다 훨씬 위에서 일어난다). 따라서 어느 비율도 저장 바이트로 확립되지 않았고, 캐시는 자기 저장 필드가 말하는 규칙을 유지하며 남은 불일치는 한글 주장이 아닌 알려진 격차로 남는다([#423](https://github.com/STAIxBWLB/hwp-cli/issues/423)).

셰이핑(`shape_range`)으로 글리프 x_advance를 pt로 재고 `seg_width/100`에서 렌더러가 줄상자 안에서 줄 들여쓰기(`layout::line_indents`)를 뺀 폭 기준 그리디 줄바꿈: 들여쓰기(양수)는 첫 줄, 내어쓰기는 둘째 줄부터 좁힌다. 이를 빼지 않으면 합성한 줄이 렌더 시 문단 우변을 넘는다. 목록 마커 폭은 셰이핑이 필요해 반영하지 않으므로, 마커가 내어쓰기 폭보다 넓은 문단의 첫 줄은 그만큼 길 수 있다. NB_SPACE 양옆은 줄바꿈 금지. 줄 시작점은 glyph index가 아니라 shaping-cluster source의 UTF-16 길이로 계산해 보조평면 문자·합자·결합문자의 WCHAR offset 보존. 각 줄 방출은 클로저 `place`:
```
if v_pos>0 && v_pos + base > content_h { v_pos = 0 }   // 페이지 리셋(셀은 content_h=MAX라 무발동)
segs.push(LineSeg{ text_start, v_pos, line_height:base, text_height:base,
                   baseline_gap, line_spacing, col_start:0, seg_width, flags:0x0006_0000 })
v_pos += line_advance
```
탭은 `tab::next_tab(tabs, acc, 40)` — 다음 명시 탭 스톱, 없으면 `floor(acc/40)*40 + 40`으로 간다. 빈 문단도 줄 1개를 갖는다. `flags=0x0006_0000`는 정품 본문 줄의 표준 플래그값.

### 2.4 표 높이 (`table_height`, `fill_nested`, `para_line_block`)

`fill_nested`가 먼저 셀/글상자 내부 문단의 줄배치를 채운다(셀 폭 = `cell.width - margins[0] - margins[1]`, 셀마다 v_pos 리셋, `content_h=i32::MAX`로 페이지 분할 없음. 글상자는 폭 `gso_shapes[0].w - 566`). 캐시가 전혀 없는 목록은 이웃 문단과 같은 임시 합성으로 그리지만, 빈 문단만은 합성하지 않는다: 그래서 `marker_for_render`에 닿지 않아 보이지 않는 목록 번호를 소비하지 않고, 대신 줄 하나는 그대로 소유한다. 그 줄이 흐름 커서를 `paragraph_metrics`만큼 전진시키고, 그 줄 하단이 행이 재는 값이 된다(셀의 마지막 문단일 때도 마찬가지다)([#422](https://github.com/STAIxBWLB/hwp-cli/issues/422)).

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
