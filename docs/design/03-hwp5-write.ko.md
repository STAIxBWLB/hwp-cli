[한국어](03-hwp5-write.ko.md) · [English](03-hwp5-write.md)

# HWP5 바이너리 Writer 및 한글 호환 합성 — 재구축 명세

이 문서는 `hwp-cli`에서 IR(`hwp_model::Document`)을 한컴오피스 한글이 손상/변조 판정 없이 열 수 있는 HWP 5.0 바이너리로 직렬화하는 서브시스템 전체를 처음부터 재구현할 수 있도록 기술한다. 핵심 구현은 `/Users/elevn/projects/hwp-cli/crates/hwp5/src/write.rs`(2049줄) 이며, 불변식은 `tests/roundtrip.rs`·`tests/identity.rs`·`tests/synth.rs`가 고정한다. 여기 나오는 모든 상수·바이트 오프셋·길이는 **정품 한글 저장 파일(hello_world 5.1.0.1, 가나다 5.1.1.0, work_report 5.0.2.4, halla 5.1.1.0, annual_report)과 실기(한글 프로그램) 게이트로 확정한 실측 정답지**이며, 스펙 문서만으로는 유도할 수 없다(pyhwp 등 관대한 파서는 통과시켜도 한글 본체는 거부하는 값들이다).

---

## 0. 설계 원리: 거울 대칭 "prefix + tail"

모든 레코드는 **`{알려진 prefix를 구조체 필드로} + {남은 바이트를 tail로 그대로}`** 규칙으로 파싱/방출한다. HWP는 버전이 오르며 레코드 꼬리에 필드를 추가하는 전방 호환 포맷이므로:

- 파서(`doc_info.rs`/`body_text.rs`)는 `r.take_rest().to_vec()`로 미해석 꼬리를 `tail`에 보존한다.
- Writer(`write.rs`)의 각 `emit_*`는 **파서와 바이트 단위로 거울 대칭**이다. `tail`이 비어 있지 않으면(=hwp5 원본 왕복) 그대로 덧붙이고, 비어 있으면(=hwpx/md 합성) 선언 버전 규격의 기본 꼬리를 채운다.

이 대칭 덕에 단순 컨트롤만 있는 hwp5 문서는 **압축 해제 스트림 기준 바이트 동일**로 왕복한다(`identity.rs`·`roundtrip.rs`의 최종 증명). gso 등 평탄화되는 구조는 의미 수준 동등을 보장한다.

레코드 트리 ↔ 평면 스트림 변환은 `record/tree.rs`가 담당한다. **level = 트리 깊이**로 재계산해 직렬화하므로, 파싱→트리→재직렬화가 바이트 동일이다(`RecordNode::serialize_forest`).

---

## 1. 컨테이너: CFB V3 (512B 섹터) 필수

HWP 5.0 = MS CFB(Compound File Binary) 컨테이너. **반드시 버전 3(512바이트 섹터)로 만들어야 한다.** 기본값인 V4(4096B 섹터)로 쓰면 한글이 "손상된 파일"로 거부한다(실기 게이트 실측).

```rust
let mut cfb = cfb::CompoundFile::create_with_version(cfb::Version::V3, file)?;
```

스트림 기록 순서와 내용(`write_document`, write.rs:161~229):

| CFB 경로 | 내용 | 압축 |
|---|---|---|
| `/FileHeader` | 256B 고정 헤더 | 없음 |
| `/DocInfo` | DocInfo 레코드 포레스트 | raw deflate |
| `/BodyText/Section{i}` | 섹션별 본문 레코드 포레스트 | raw deflate |
| `/BinData/BIN{id:04X}.{ext}` | BIN_DATA 테이블이 참조하는 이미지만 | raw deflate |
| `/DocOptions/_LinkDoc` | `[0u8; 524]` | 없음 |
| `/Scripts/JScriptVersion` | 13B 표본 원시 바이트 | (레코드 스트림 취급이나 표본은 비압축 바이트 그대로 기록) |
| `/Scripts/DefaultJScript` | 16B 표본 원시 바이트 | 〃 |
| `/\u{5}HwpSummaryInformation` | OLE 속성 집합(§9) | 없음 |
| `/PrvText` | 미리보기 UTF-16LE (~1000자) | 없음 |
| `/PrvImage` | PNG (opts.prv_image 있을 때만) | 없음 |

**압축**은 zlib 헤더 없는 **raw deflate**(pyhwp의 `wbits=-15`, `flate2::DeflateEncoder`). 어떤 스트림이 압축 대상인지는 `container::is_record_stream`이 판정(`/DocInfo`, `/BodyText/`, `/ViewText/`, `/Scripts/` 접두).

`BinData`는 `header.bin_data`의 `storage_id`가 있는 항목만 `BIN{id:04X}.{ext}` 이름으로 동봉한다. 테이블이 참조하지 않는 스트림은 드롭하고 경고를 남긴다(hwp5 명명 규칙 준수).

---

## 2. FileHeader — EncryptVersion=4 필수

레이아웃(256B 고정, `file_header.rs`):

| 오프셋 | 크기 | 필드 | 합성 시 값 |
|---|---|---|---|
| 0 | 32 | 시그니처 `"HWP Document File"` + NUL 패딩 | 고정 |
| 32 | 4 | 버전 DWORD `0xMMnnPPrr` | `source_version` 파싱값 |
| 36 | 4 | 속성 플래그 | `0x1` (bit0=압축) |
| 40 | 4 | 라이선스(CCL/공공누리) | `0` |
| 44 | 4 | **EncryptVersion** | **`4`** |
| 48 | 1 | 공공누리 지원 국가 | `0` |
| 49 | 207 | 예약 | `0` |

**EncryptVersion=4가 결정적이다.** 비암호 문서라도 현대 한글(2010+, 글 7.0+)은 EncryptVersion=4를 무조건 쓴다. fixtures/hwp5 표본 6개 전부 속성 플래그의 암호화 비트(bit1)는 0인데 encver=4다. **0을 쓰면 한글이 손상/변조로 거부한다.** 속성 플래그 bit0(압축)은 항상 켜며, `is_compressed()`가 참이어야 함을 `roundtrip.rs`/`synth.rs`가 단정한다.

버전 인코딩: `major<<24 | minor<<16 | build<<8 | revision`. `parse_version`은 `source_version`(예 `"5.1.0.1"`)을 파싱하되 실패 시 기본값 **5.1.0.1**(`HwpVersion{5,1,0,1}`)로 폴백한다. 이 기본값이 아래 모든 버전 게이트의 기준선이다.

---

## 3. 합성 경로(source≠hwp5) vs 무수정 왕복(identity) 분리

이 서브시스템의 가장 중요한 아키텍처 결정. **두 경로는 절대 섞이면 안 된다.**

### 게이트 플래그 계산 (write.rs:53~57)

```
synth_pictures = meta.source_format != "hwp5" || has_synthesizable_picture(doc)
synth_gso      = meta.source_format != "hwp5" || has_synthesizable_gso(doc)
synthesize     = synth_pictures || synth_gso || opts.edited
```

- `has_synthesizable_picture`: `extras`가 빈 `Picture`(=hwp5 도형 레코드 미보유)가 있으면 참. **편집으로 새로 삽입된 그림**은 출처가 hwp5여도 합성해야 strip에 드롭되지 않는다.
- `has_synthesizable_gso`: `ctrl_id != "gso "` 인데 `gso_shapes`가 있는(hwpx reader가 만든 구조화 도형) 컨트롤이 있으면 참.
- `opts.edited`: hwp5 원본을 읽었으나 내용을 바꿔 다시 쓰는 경우. 그림 재합성(`synthesize_pictures`)은 **하지 않으나**(이미 도형 레코드 보유) 문단 불변식·줄 배치는 다시 세워야 한다.

### synthesize=true (합성 경로)일 때 하는 일

1. `synthesize_pictures`(§7 그림 합성), `degrade_hwpx_gso`(§8 글상자 저하), `strip_unwritable_pictures`(합성 불가 컨트롤 드롭).
2. 리스트별 마지막 문단 플래그 `set_last_para_flag`(§6).
3. `ensure_para_shape_defaults`(ParaShape 누락 기준값 보정: `line_spacing_old` 0→160, `border_fill_id` 0→2).
4. 각 섹션 첫 문단 `break_type |= 0x03`.
5. `emit_paragraph` 내부에서 문단끝 0x0d·char_shape dedup·빈 문단 PARA_TEXT 생략(§6).
6. `assign_instance_ids`: PARA_HEADER instance_id가 0이면 유니크 non-zero 부여(0x10000001부터). **한글은 instance_id=0을 비정상으로 보고 손상 판정**(표본은 전부 non-zero).
7. `emit_doc_info`에서 COMPATIBLE_DOCUMENT 주입(§5), TAB_DEF/NUMBERING 기본값 주입(§4·안전망), DOCUMENT_PROPERTIES 시작번호 `max(1)`.

### synthesize=false (identity 경로)일 때

- 문서를 **복제조차 하지 않는다**(`needs_normalize(doc)`도 false면). 원본 IR을 그대로 방출.
- 모든 `tail`·`instance_id`(0 포함)·`chars_flags`를 원본 그대로 보존 → 바이트 동일.
- COMPATIBLE_DOCUMENT는 `header.extras`로 이미 보존되므로 재주입하지 않는다.
- `preserve_linesegs=true`일 때만 PARA_LINE_SEG를 방출(바이트 동일 게이트). `false`면 한글이 재계산하도록 seg_count=0으로 생략(내용 수정 시 줄 배치 캐시 불일치 → 한글 "변조" 경고 방지).

**직교성 주의:** `synthesize`(불변식 보정)와 `preserve_linesegs`(줄 배치 방출)는 별개 축이다. 줄 배치 방출 조건은 `emit_lineseg = synthesize || preserve_linesegs`.

---

## 4. 레코드 길이 버전 게이팅

한글은 선언 버전과 레코드 실제 길이가 어긋나면 손상/변조로 거부한다. 합성 문단(`tail` 빈 경우)에만 버전별 규격 길이를 채우고, 원본 왕복은 `tail`로 정확히 보존한다.

### PARA_SHAPE (0x19): 54B → 58B

5.1.0.1 규격은 **58B**. prefix 42B 뒤 `tail`이 비면 다음 16B를 채운다(`emit_para_shape`, write.rs:1436):

| 오프셋 | 크기 | 필드 |
|---|---|---|
| 0 | 4 | attr1 |
| 4~28 | 24 | margin_left, margin_right, indent, spacing_top, spacing_bottom, line_spacing_old (각 i32) |
| 28 | 2 | tab_def_id |
| 30 | 2 | numbering_id |
| 32 | 2 | border_fill_id |
| 34 | 8 | border_offsets[4] (u16) |
| 42 | 4 | 속성2 = 0 |
| 46 | 4 | 속성3 = 0 |
| 50 | 4 | 줄간격 = `line_spacing>0 ? line_spacing : 160` |
| 54 | 4 | **후행 4B = 0** |

**후행 4B(오프셋 54~58)를 누락하면 54B가 되어 한글이 무결성 위반 경고를 띄운다.** `synth.rs`가 `record_sizes(&di, 0x19).all(== 58)`을 단정. CHAR_SHAPE도 동일 게이트로 **74B**여야 함을 단정.

### PARA_HEADER (0x42): 22B → 24B

prefix 22B. 선언 버전이 **5.0.3.2 이상**이면 '변경추적 병합 문단여부' UINT16(=0)을 붙여 **24B**(스펙 표 58). 게이트: `add_tracking_tail = source_version >= 0x05_00_03_02` (write.rs:113). pre-5.0.3.2(work_report 5.0.2.4)는 22B가 정답이므로 게이트 false. `synth.rs`가 `record_sizes(&bt, 0x42).all(== 24)` 단정(합성은 5.1.0.1 선언).

### ID_MAPPINGS (0x11): 15/16/18개 카운트

카운트 배열 길이는 선언 버전과 정합해야 한다(스펙 표 15·16). **테이블 실제 길이에서 유도**하며 수동 동기화하지 않는다(write.rs:1166~1204):

| 인덱스 | 카운트 | 도입 버전 |
|---|---|---|
| 0 | bin_data | 기저(15개) |
| 1~7 | fonts[언어 7슬롯] | |
| 8 | border_fills | |
| 9 | char_shapes | |
| 10 | tab_defs | |
| 11 | numberings | |
| 12 | bullets | |
| 13 | para_shapes | |
| 14 | styles | |
| 15 | 메모 모양 | 5.0.2.1+ (16개) |
| 16 | 변경추적 | 5.0.3.2+ (18개) |
| 17 | 변경추적 사용자 | 5.0.3.2+ |

```
version_target = if ver >= 0x05_00_03_02 { 18 }
                 else if ver >= 0x05_00_02_01 { 16 } else { 15 }
target = max(원본 카운트 길이, version_target, 파생 카운트 길이)
```

**무조건 18로 패딩하면 5.0.2.x(16개) 문서를 부풀려 버전-레이아웃이 어긋나 손상 판정된다**(work_report 실증). 카운트 emit 루프와 자식 레코드 emit 루프가 **동일 Vec**(`tab_defs_owned`/`numberings_owned`)를 참조하므로 카운트와 실제 항목 수는 항상 정합한다(불변식).

### 기타 tail 게이트

- **BORDER_FILL**: tail 비면 무늬색(u32=0)+무늬종류(u32=0xFFFFFFFF)+추가속성크기(u32=0)+투명도(u8=0)로 채운다(fill_type&1일 때만 색/투명도).
- **CHAR_SHAPE**: tail 비면 border_fill_id(u16, `max(2)`)+취소선색(u32=0). **shade_color는 0이면 안 된다**(0xFFFFFFFF='없음'). 0이면 한글이 글자 칸마다 불투명 검정 음영을 그려 '검은 바'가 된다(`synth.rs` 단정).
- **STYLE**: tail 비면 잠금 u16=0.
- **LIST_HEADER(셀)**: tail 비면 텍스트폭(i32=셀 폭)+예약 8B → 46B.
- **TABLE**: table_tail 비면 영역속성크기 u16=0(5.0.1.0+).

---

## 5. COMPATIBLE_DOCUMENT 서브트리 (5.1.x 필수)

정품 5.1.x(가나다 5.1.1.0, hello_world 5.1.0.1)는 모두 이 서브트리를 가진다. **누락 시 한글이 손상/변조로 거부.** 구버전(work_report 5.0.2.4)은 면제. 주입 조건: `source_format != "hwp5"` **그리고** `header.extras`에 이미 없을 때만(write.rs:1264). hwp5 원본 왕복은 `extras`로 보존되므로 재주입하지 않는다.

트리 구조(태그값은 `HWPTAG_BEGIN=0x10` 기준):

```
COMPATIBLE_DOCUMENT (0x1E)          data = [0u8; 4]  (대상 프로그램 0)
├─ LAYOUT_COMPATIBILITY (0x1F)      data = [0u8; 20]
└─ TRACKCHANGE (0x20)               data = [0u8; 1032], 단 data[0]=0x38
```

`TRACKCHANGE`는 **1032B**이고 선두 바이트만 0x38(표본 실측), 나머지 0. `synth.rs`가 DocInfo에서 0x1E를 찾아 자식으로 0x1F·0x20이 있는지 단정한다.

---

## 6. 문단 규칙 (합성 경로 불변식)

정품 한글 문단(가나다 188문단 전수 대조)과 동형이 되도록 `emit_paragraph`(write.rs:1507)가 강제하는 규칙. 이 5대 결함이 합쳐져 "보안 낮춤에도 손상" 경고를 냈던 근본 원인이다.

### (a) 모든 문단은 문단끝 0x0d로 종료

`synthesize`면 마지막 char가 `CharCtrl(13)`이 아닐 때 push한다. `synth.rs`가 PARA_TEXT 마지막 u16 == 13을 단정.

### (b) 빈 문단은 PARA_TEXT 레코드 생략, nchars=1

정품 실측: 빈 문단/빈 셀은 `nchars=1`(암묵적 문단끝) + PARA_CHAR_SHAPE + PARA_LINE_SEG를 갖되 **PARA_TEXT 레코드가 없다.**

```rust
let char_count = if para.chars.is_empty() { 1 } else { para.wchar_len() };
// ...
if char_count > 1 { /* PARA_TEXT 방출 */ }
```

합성 경로는 모든 문단에 0x0d를 붙이므로 빈 문단이 `chars=[0x0d]`(char_count=1)가 되는데, 이를 `PARA_TEXT=[0x0d]`로 방출하면 **한글이 "파일 손상 + 본문 비어있음"으로 거부한다**(빈 셀 표=제목 박스·목차·구역 헤더 전부 손상의 원인). pyhwp는 빈 PARA_TEXT를 관대하게 통과시켜 23라운드 미검출 — 정품 바이트 대조로만 잡힘. `synth.rs`의 `빈_문단은_para_text_없음` 회귀 방지.

`nchars`는 하위 31비트 = 글자 수, `wchar_len()`은 `Text→len_utf16`, `CharCtrl→1`, `Inline/ExtCtrl→8`(6 WCHAR payload + 앞뒤 코드)의 합.

### (c) nchars bit31 = 리스트의 마지막 문단만

`nchars`의 최상위 비트(0x80000000)는 '리스트(섹션/표 셀/글상자)의 마지막 문단' 표식이다. `set_last_para_flag`가 각 리스트의 **마지막 문단만** `chars_flags |= 0x80`, 나머지는 clear(재귀로 표 셀·글상자 내부까지). PARA_HEADER emit 시 `nchars = char_count | (chars_flags << 24)`.

**모든 문단에 세팅하면 한글이 첫 문단을 마지막으로 보고 뒤 문단을 무시한다**(다문단 "둘째부터 안 보임"). `synth.rs`가 단일 문단은 bit31 세팅을 단정.

### (d) PARA_CHAR_SHAPE 연속 동일 id run 병합(dedup)

```rust
p.char_shape_runs.dedup_by(|(_, b), (_, a)| a == b);
```

중복 run은 손상 판정. `synth.rs`가 단일 문단은 run 수=1(char_shape_cnt=1)을 단정.

### (e) 구역 첫 문단 break_type=0x03

각 섹션 첫 문단 `break_type |= 0x03`(구역/단 나눔). `synth.rs`가 PARA_HEADER 오프셋 11 == 0x03 단정.

### (f) ctrl_mask는 확장/인라인 컨트롤만

`ctrl_mask`는 원본값이 있으면 보존, 없으면 `chars`의 `InlineCtrl`/`ExtCtrl` code로부터 `1<<code`를 OR로 계산한다. **문자형 컨트롤(문단끝 13, 줄나눔 10 등)은 포함하지 않는다** — 켜면 한글이 "ctrl_mask에 있다는 컨트롤이 실제로 없다"고 손상 판정.

### PARA_HEADER 22B prefix 레이아웃

| 오프셋 | 크기 | 필드 |
|---|---|---|
| 0 | 4 | nchars (bit31=마지막문단, 하위31=글자수) |
| 4 | 4 | ctrl_mask |
| 8 | 2 | para_shape id |
| 10 | 1 | style id |
| 11 | 1 | break_type |
| 12 | 2 | char_shape run 수 |
| 14 | 2 | range_tag 수 (PARA_RANGE_TAG 개수) |
| 16 | 2 | line_seg 수 (`emit_lineseg`면 실제, 아니면 0) |
| 18 | 4 | instance_id |
| 22 | (2) | 변경추적 병합여부 (5.0.3.2+ tail) |

자식 레코드 순서: PARA_TEXT(char_count>1일 때만) → PARA_CHAR_SHAPE → PARA_LINE_SEG(방출 시) → extras → 컨트롤들(CTRL_HEADER).

PARA_TEXT 인코딩(`emit_para_text`): `Text`→UTF-16LE, `CharCtrl`→u16, `Inline/ExtCtrl`→`[code(u16), payload 12B(부족시 0패딩), code(u16)]`(총 8 WCHAR).

---

## 7. 그림 합성 (hwpx/md 출신 이미지 → hwp5 도형 레코드)

hwpx의 `<hp:pic>`은 `extras`가 빈 IR `Picture`로 읽힌다. hwp5는 그림을 `gso CTRL_HEADER → SHAPE_COMPONENT → SHAPE_COMPONENT_PICTURE` 트리 + BIN_DATA 항목 + BinData 스트림으로 저장한다. `synthesize_pictures`(write.rs:590)가 정품 work_report의 레코드를 템플릿으로 크기·BinItem ID만 패치한다.

### 흐름

1. `bin_streams`의 각 이미지에서 픽셀 크기 추출(`image_pixel_size`: PNG IHDR / JPEG SOFn / GIF LSD / BMP BITMAPINFOHEADER).
2. `storage_id` 부여(기존 max+1부터, 공유 이미지는 재사용), `BinDataItem{attr:1(임베딩), storage_id, extension}` 추가, 스트림 이름을 `BinData/BIN{s:04X}.{ext}`로 rename.
3. **gso 개체 공통 속성 40B**를 배치에서 합성:

| 오프셋 | 크기 | 필드 |
|---|---|---|
| 0 | 4 | attr — 인라인(글자처럼)=`0x042a6001`, 떠있음=`0x040a6000` (한라대 실측) |
| 4 | 4 | 세로 오프셋 (vert_offset — treat_as_char여도 보존) |
| 8 | 4 | 가로 오프셋 |
| 12 | 4 | 폭 |
| 16 | 4 | 높이 |
| 20 | 4 | z_order |
| 24 | 8 | 바깥 여백(왼/위, 오른/아래) |
| 32 | 4 | instance_id (유니크, 0x30000000부터) |
| 36 | 4 | 쪽 나눔 방지 |
| 40 | 2 | **desc_len = 0** (개체 설명 BSTR, CommonControl≥5.0.0.5 필수 — 빈 설명도 길이 u16=0 필수) |

4. **SHAPE_COMPONENT(0x4C) 196B** 템플릿(`SHAPE_COMPONENT_TEMPLATE`): chid `"$pic"`(역순 `"cip$"`), 폭@20/28, 높이@24/32 패치(단위행렬이라 초기=최종).
5. **SHAPE_COMPONENT_PICTURE(0x55) 91B**(`build_picture_extras`) — 정품 5.1.x 레이아웃:

| 오프셋 | 크기 | 내용 |
|---|---|---|
| 0 | 12 | 테두리 |
| 12 | 32 | 표시 사각형 4꼭지점 (0,0)(w,0)(w,h)(0,h) |
| 44 | 16 | 자르기 = (0, 0, **자연폭, 자연높이**) |
| 60 | 8 | 안쪽 여백 |
| 68 | 3 | 밝기/명암/효과 |
| 71 | 2 | **BinItem ID** |
| 73 | 1 | 테두리 투명도 |
| 74 | 4 | instance_id (gso와 다르게 `^0x00100000` 파생) |
| 78 | 4 | picture_effect flags = 0 |
| 82 | 8 | picture_effect 자연폭/자연높이 |
| 90 | 1 | reserved |

**자르기(clip)는 표시 크기가 아니라 원본 자연 크기(픽셀×7200/96, 96DPI)여야 한다.** 표시 크기를 쓰면 원본 좌상단 일부(예 8196/150000≈5%)만 잘려 한글에서 그림이 거의 안 보인다. `build_picture_extras`가 채운 `extras`로 §3의 `strip_unwritable_pictures`가 드롭하지 않게 된다.

합성 후 `bin_ref = BinRef::Id(storage_id)`. `emit_picture`(write.rs:1941)는 `common_data`(없으면 최소 40B 글자처럼취급)를 쓰고 `extras`를 자식으로 방출.

머리말/꼬리말(head/foot)의 빈 LIST_HEADER는 `fill_head_foot_list_header`가 구역 PageDef 치수(textWidth=본문폭, textHeight=머리말/꼬리말 여백)로 채운다(`HEADER_LIST_HEADER_TEMPLATE` 34B, paraCount만 실제 문단 수로 패치). 안 채우면 strip이 머리말째 드롭한다.

---

## 8. gso raw_children 무손실 + hwpx-출신 도형 안전 저하

### hwp5 원본 gso: raw_children 무손실

hwp5 출신 GenericControl은 원본 자식 서브트리를 `raw_children`(OpaqueRecord 트리)으로 통째 보존한다(`parse_generic`). Writer(`emit_control`, write.rs:1720)는 `raw_children`가 있으면 **그대로 중첩 방출하고 즉시 return** — `paragraph_lists`/`extras`(텍스트 추출 전용)로 평탄화하지 않는다. 이것이 표/그림/도형/책갈피 포함 전체 fixture의 바이트 동일 왕복을 성립시킨다.

`raw_children`가 없을 때만 `paragraph_lists`(LIST_HEADER + 문단들) + `extras`를 조합해 CTRL_HEADER를 만든다. ctrl_id는 **역순 저장**(`reversed`, `b"secd"→"dces"`). `cold`(단 정의)는 data 비면 `DEFAULT_COLD_DATA` 12B로 대체.

### hwpx-출신 구조화 도형: 안전 저하

hwpx reader가 만든 도형(`ctrl_id != "gso "` + `gso_shapes` 보유)은 hwp5 SHAPE_COMPONENT가 없다. 정품 템플릿 역합성(㉒)은 한글 실기에서 손상 판정났다(라인 252B 템플릿을 사각형에 사용 → 정품 239B와 13B 어긋남). 자가검증 불가로 **안전 저하**로 전환(`degrade_hwpx_gso`, write.rs:467):

- **글상자(텍스트 보유)**: 그 문단들을 host 문단 뒤 본문으로 hoist(텍스트 보존), 도형 래퍼는 생략.
- **순수 장식(텍스트 없음)**: 손대지 않아 `strip_unwritable_pictures`가 드롭(유효 보장).

hoist 시 제거된 컨트롤의 ExtCtrl 문자를 `chars.retain`으로 삭제하고 남은 `ctrl_index`를 재매핑한다(strip과 동일 로직). `synth.rs`의 `글상자_hwpx출신_안전저하_텍스트보존`이 회귀 방지.

---

## 9. 보조 스트림

한글 저장 파일에 항상 존재하며 부재 시 손상 판정 위험(write.rs:207~228).

- **`/DocOptions/_LinkDoc`**: `[0u8; 524]`.
- **`/Scripts/JScriptVersion`**: 표본 원시 13B `63 64 80 00 00 F7 DF 88 A9 08 00 00 00`.
- **`/Scripts/DefaultJScript`**: 표본 원시 16B `63 60 40 05 FF 81 00 00 6E BB 6E D1 14 00 00 00`.
- **`/\u{5}HwpSummaryInformation`**(`hwp_summary_information`, write.rs:977): OLE 속성 집합. FMTID `9FA2B660-1061-11D4-B4C6-006097C09D8C`, 14개 속성(PID 0x02제목/0x03주제/0x04지은이/0x05키워드/0x09프로그램="hwp-cli"/FILETIME×3/I4×2/**PID 0=Dictionary**). PID 0을 VT_NULL로 쓰면 pyhwp 등이 count=1 사전으로 읽다 EOF로 거부 — 반드시 항목 1개 사전(id=0, 빈 이름 1B)으로 13B 기록. `summary.rs` 파서와 대칭.
- **`/PrvText`**: `plain_text()` 앞 1000자 UTF-16LE.

`hwp_string` 헬퍼: `u16 길이(UTF-16 유닛 수) + UTF-16LE 바이트`.

---

## 10. DocInfo/BodyText 최상위 조립 순서

### emit_doc_info (write.rs:1109) 루트 순서

1. **안전망**: `tab_defs` 비면 3개 기본(`[0..]`,`[1..]`,`[2..]` 8B), `numberings` 비면 `DEFAULT_NUMBERING_DATA` 226B 주입. **모든 PARA_SHAPE가 tab_def_id=0·numbering_id=0을 참조하므로 테이블이 비면 dangling reference가 되어 한글이 손상 거부**(halla 실증). `synth.rs`가 non-empty 단정.
2. `DOCUMENT_PROPERTIES`(0x10): section_count `max(1)`, 시작번호 6개 각 `max(1)`(쪽번호 0은 비정상), caret 3×u32.
3. `ID_MAPPINGS`(0x11): §4의 카운트 배열 + 자식 테이블(bin_data → fonts 7슬롯 → border_fills → char_shapes → tab_defs → numberings → bullets → para_shapes → styles → id_extras).
4. `COMPATIBLE_DOCUMENT`(§5, 합성+미보유 시).
5. `header.extras`(hwp5 원본의 COMPATIBLE 등).

### emit_section (write.rs:1483)

섹션 문단들을 `emit_paragraph`로 → `section.extras` 추가. 각 섹션 직렬화 후 `assign_instance_ids`(합성 시).

### emit_section_def / secd 필수 자식 (write.rs:1760)

ctrl_id `"dces"`(=secd 역순) + data(없으면 `DEFAULT_SECD_DATA` 43B). 자식:
- `PAGE_DEF`(0x49) 40B: width, height, margin×6, gutter(각 i32), attr(u32).
- **`def.extras`가 비면(합성)**: `FOOTNOTE_SHAPE`(0x4A) 28B ×2(각주+미주 표본) + `PAGE_BORDER_FILL`(0x4B) 14B ×3(BOTH/EVEN/ODD).

**PAGE_BORDER_FILL 3종 전부 첫 u32=1**(properties), 여백 4×u16=1417(0x0589), border_fill_id u16=1. hello_world 표본의 BOTH/EVEN 값(0x0978f9c1 등)은 미초기화 garbage라 채택하지 않는다. `synth.rs`가 secd 아래 0x4A ×2, 0x4B ×3, 첫 u32=1을 단정. PAGE_DEF만 있고 각주모양·쪽테두리가 없으면 한글이 손상 거부.

### emit_table (write.rs:1816)

CTRL_HEADER data = `" lbt"`(=tbl 역순) + 개체 공통 속성(`common_data` 보존 / hwpx placement 합성 / md 셀크기 계산). 자식: `TABLE`(0x4D) 레코드(attr u32, rows/cols/cell_spacing u16, inner_margins 4×u16, **row_cell_counts rows×u16**, border_fill u16, tail) → 셀마다 `LIST_HEADER`(0x48, `emit_cell_header`, para_count=문단수 `≥1`) + 문단들.

**빈 셀도 nparas≥1 필수**(문단 없으면 한글 손상). `from_markdown`이 셀 종료 시 `flush_paragraph_inner(force=true)`·누락 칸 `Paragraph::default()` 충전으로 보장. `row_cell_counts` 길이=행수, 합=셀수 정합(`synth.rs` 행추가 표 단정).

---

## 11. 재직렬화·불변식 체크리스트 (테스트로 고정)

`/Users/elevn/projects/hwp-cli/crates/hwp5/tests/`:

- **identity.rs** `레코드_스트림_바이트_동일_재직렬화`: 전체 fixture(hello_world/bookmark/color_fill/outline/work_report/annual_report)의 `/DocInfo`+본문 섹션을 strict 스캔→트리→`serialize_forest`가 **원본 바이트 동일**(레코드 계층 무손실 1차 증명).
- **roundtrip.rs** `전체_fixture_바이트_동일_왕복`: `preserve_linesegs:true`로 IR 경유 재저장 시 압축 해제 스트림 바이트 동일 + `is_compressed()`. `전체_fixture_의미_왕복`: 텍스트·char_shape 수·섹션 수·lineseg 수 보존.
- **synth.rs** `합성_문서_한글_규격_충족`: TAB_DEF/NUMBERING non-empty, shade_color≠0, COMPATIBLE(0x1E)의 0x1F·0x20 자식, secd 각주×2·쪽테두리×3, EncryptVersion=4, PARA_SHAPE=58B/CHAR_SHAPE=74B/PARA_HEADER=24B. `합성_문단_본문_구조_정품_동형`: PARA_TEXT 끝 0x0d, nchars bit31, break_type=0x03, char_shape run=1(dedup), PAGE_BORDER_FILL attr=1. `빈_문단은_para_text_없음`, `행_추가_표_합성_규격_충족`, `누름틀/책갈피/하이퍼링크_생성_이진_왕복`, `글상자_hwpx출신_안전저하_텍스트보존`.

### 레코드 헤더 코덱 (record/header.rs)

`u32 LE = tag(비트0~9) | level(비트10~19)<<10 | size(비트20~31)<<20`. **size 필드가 0xFFF이면 후속 u32가 실제 크기**(0xFFE는 인라인 4B, 0xFFF부터 확장 8B). `serialize_forest`는 level을 트리 깊이로 재계산(`serialize_into(depth)`).

### 컨트롤 문자 분류표 (hwp_model::char_kind — 단일 진실 공급원)

| code | 종류 | WCHAR |
|---|---|---|
| 0,10,13,24~31 | Char(문자형) | 1 |
| 4~9,19,20 | Inline(인라인) | 8 |
| 1~3,11,12,14~18,21~23 | Extended(확장, controls 참조) | 8 |
| 32+ | 일반 문자 | len_utf16 |

이 분류표가 파서·writer·텍스트 추출·위치 산수 모두의 기준이며, `wchar_len() == nchars` 불변식으로 분류 오류가 즉시 드러난다.

---

## 12. 재구현 시 핵심 함정 요약

1. CFB는 반드시 V3. V4는 즉시 손상.
2. EncryptVersion=4 하드코딩(비암호라도).
3. `synthesize` ≠ `preserve_linesegs`(직교). identity 경로는 원본을 복제조차 하지 말 것(tail·instance_id 0·chars_flags 보존이 바이트 동일의 전제).
4. 버전 게이트는 **선언 버전 기준**으로 PARA_SHAPE(58/54)·PARA_HEADER(24/22)·ID_MAPPINGS(18/16/15)를 분기. 최신 규격으로 일괄 패딩 금지.
5. 빈 문단은 nchars=1 + PARA_TEXT 생략(0x0d만 든 PARA_TEXT는 손상).
6. nchars bit31은 리스트 마지막 문단에만.
7. char_shape run dedup, shade_color≠0.
8. 그림 자르기는 자연 크기(표시 크기 아님).
9. dangling reference 방지: TAB_DEF/NUMBERING/PAGE_BORDER_FILL/각주모양 기본값 안전망.
10. hwp5 원본 gso는 raw_children로 무손실; hwpx 도형은 안전 저하(재합성은 손상 재발).

이 모든 값은 스펙이 아니라 정품 파일 실측 + 한글 실기 게이트로 확정한 정답지다.

---

## 13. 원본 보존형 네이티브 HWP 되쓰기

네이티브 HWP 편집은 입력 컨테이너를 새로 조립하지 않는다. `rewrite_document_with_report`는
불변 source 경로, 그 snapshot에서 읽은 IR, 편집 IR을 함께 받는다. 쓰기 전에 source를 다시 읽어
원본 IR과 다르면 snapshot mismatch로 거부하고, 원본과 편집본의 차이로 mutation plan을 만든다.

mutation plan이 소유하는 대상은 아래뿐이다.

- 메타데이터 변경: `\u0005HwpSummaryInformation`
- header 또는 BinData 관계 변경: `DocInfo`
- section 변경: 해당 `BodyText/SectionN`과 미리보기
- binary 변경·삭제: 해당 `BinData/*`

무수정은 `fs::copy`이므로 CFB 전체가 바이트 동일하다. 편집 시에도 source CFB를 먼저 복사한 뒤
선택한 stream만 교체한다. `FileHeader`, `MemoExtended`, `Scripts`, `DocOptions`, XMLTemplate,
DocHistory, 미지 stream/storage, 미변경 binary, 패치 대상과 무관한 CFB directory entry는 source가 계속 소유한다.

BodyText도 외과적으로 materialize한다. 문단은 고유한 non-zero `instance_id`, 완전 동일성, 동일
길이 위치 fallback 순으로 대응한다. 미변경 문단은 원본 record subtree를 그대로 사용한다. 변경
문단 안에서도 미변경 typed control은 source subtree를 이식하고, 표 셀 편집은 변경 셀 문단만
재귀적으로 교체한다. `CTRL_DATA`, `PAGE_DEF`, `PAGE_BORDER_FILL`처럼 의미 IR에서 분리해 표현해도
실제 record level과 순서가 중요한 구조를 지키기 위함이다. `ParaHeaderInfo::hwp5_child_order`도
opaque 문단 자식과 `CTRL_HEADER` 사이의 원본 순서를 보존한다.

변경·삽입 문단은 renderer로 새 `PARA_LINE_SEG`를 계산한다. 계산 후 표 셀과 generic control 내부를
포함한 의미상 미변경 문단은 불변 snapshot 값으로 복원한다. 이 방식으로 stale layout cache와
문서 전체 layout churn을 함께 피한다.

CLI의 `convert` 무수정, `edit`, IR `fill` 경로는 쓰기 전에 private immutable input snapshot을
만든다. 결과 공개는 기존처럼 atomic하며 typed preservation gate를 통과해야 한다. 회귀 테스트는
무수정 exact copy, metadata-only targeting, opaque stream 보존, 미변경 typed control subtree 보존,
image 관계 materialization, 같은 포맷 CLI 바이트 동일을 고정한다.
