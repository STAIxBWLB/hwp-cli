[한국어](02-hwp5-read.ko.md) · [English](02-hwp5-read.md)

# HWP 5.0 바이너리 리더(reader) 재구축 명세

`crates/hwp5/src/`의 읽기 경로를 처음부터 다시 구현할 수 있는 수준으로 기술한다. 대상 파일: `read.rs`, `read/password.rs`, `container.rs`, `file_header.rs`, `codec/{reader,writer,compress}.rs`, `record/{header,scan,tree,tag}.rs`, `doc_info.rs`, `body_text.rs`, `summary.rs`, 그리고 소비단 기하 파서 `crates/hwp-render/src/shape_draw.rs`, 문자 분류표 `crates/hwp-model/src/paragraph.rs`.

핵심 설계 원칙 두 가지가 전 계층을 관통한다. (1) **스캔과 해석의 분리** — `record` 계층은 태그 의미를 전혀 모르고 `(tag, level, data)`만 다루며, 의미 파싱은 `doc_info`/`body_text`가 담당한다. (2) **알려진 prefix + tail 보존** — 모든 레코드 파서는 알려진 앞부분만 구조체로 뜯고 남은 바이트를 `tail: Vec<u8>`로 그대로 보존한다. HWP는 버전이 오르며 필드가 뒤에 추가되는 전방 호환 포맷이라 이 규칙이 무손실 왕복과 미래 버전 내성을 동시에 준다.

---

## 1. 전체 파이프라인

`read_document_with_options(path, options) -> ReadResult { document, warnings }` (`read.rs`)의
순서이며, `read_document(path)`는 기본 옵션을 넘긴다.

1. `Hwp5Container::open(path)` — CFB 열기 + FileHeader 파싱/검증.
2. 보호 게이트를 고정 순서로 검사한다. 암호가 없는 암호화 문서는 거부하고, 암호가 있으면 정품 코퍼스로
   실증한 EncryptVersion 4 프로파일만 bounded 메모리 복호화기로 보낸다. 인증서 암호화·인증서 DRM·DRM·
   서명은 암호 인증 뒤에도 타입드 거부로 유지한다. 배포용문서는 ViewText 경로를 선택한다.
3. 일반 또는 인증된 스트림 소스에서 `/DocInfo` 읽기(압축 해제) → `scan_stream(Tolerant)` →
   `parse_doc_info()` → `DocHeader`.
4. `body_sections()`가 준 `/BodyText/SectionN`마다: 읽기(압축 해제) → `scan_stream` → `parse_section()` → `Section`.
5. `/BinData/*` 스트림: FileHeader 압축 플래그면 raw deflate 시도, 실패 시 원본 폴백 → `BinStream`.
6. `/\x05HwpSummaryInformation`: `parse_summary()` (없거나 손상돼도 기본값으로 진행).
7. `Document { meta, metadata, header, sections, bin_streams }` 조립.

야생 파일 대응을 위해 스캔/파싱은 **Tolerant 모드**로, 실패는 `warnings`로 누적하고 opaque 보존한다(진단 중단 없음). writer 검증·왕복 테스트만 **Strict 모드**를 쓴다.

---

## 2. CFB 컨테이너와 스트림 목록

HWP 5.0 파일은 MS **CFB(Compound File Binary, 구 OLE2 복합 문서)** 컨테이너다. 본 코드는 `cfb` 크레이트(`cfb::CompoundFile<File>`)에 파싱을 위임하고 그 위에 얇은 래퍼(`Hwp5Container`)만 둔다. 재구현 시 CFB 자체(512바이트 섹터, FAT/DIFAT/미니FAT, 디렉터리 엔트리 트리)는 라이브러리를 쓰는 것이 현실적이며, 아래 스트림 경로 규약만 지키면 된다.

`list_streams()`는 `cfb.walk()`로 모든 스트림을 열거해 경로 오름차순 정렬한 `StreamInfo { path, size }` 목록을 준다. 경로는 `/`로 시작하는 절대 경로(`/BodyText/Section0`)다.

| 스트림 경로 | 내용 | 압축 대상 | 파서 |
|---|---|---|---|
| `/FileHeader` | 256B 고정 헤더(시그니처/버전/속성) | 아니오 | `file_header.rs` |
| `/DocInfo` | 문서 정보 레코드 스트림(글꼴/모양/스타일 테이블) | 예 | `doc_info.rs` |
| `/BodyText/Section0..N` | 본문 섹션 레코드 스트림 | 예 | `body_text.rs` |
| `/ViewText/Section0..N` | 배포용 문서 본문 | 예 | `distdoc.rs` → 일반 섹션 파서 |
| `/BinData/*` | 첨부 바이너리(이미지 `BIN0001.png` 등) | 헤더 플래그 따름 | 시도-폴백 |
| `/\x05HwpSummaryInformation` | OLE 속성 집합(제목/지은이/…) | 아니오 | `summary.rs` |
| `/PrvText` | 미리보기 텍스트(UTF-16LE) | 아니오 | — |
| `/PrvImage` | 미리보기 이미지(PNG/BMP) | 아니오 | — |
| `/Scripts/*` | JScriptVersion/DefaultJScript 등 | 예(레코드 스트림 규칙) | — |
| `/DocOptions/*` | `_LinkDoc` 등 문서 옵션 | 아니오 | — |

**본문 섹션 열거** `body_sections()`: `/BodyText/Section`으로 시작하는 스트림을 접미 숫자로 정수 정렬한다(문자열 정렬로는 `Section10`이 `Section2`보다 앞서므로 반드시 `parse::<u32>()` 후 정렬). 섹션 수는 `DocumentProperties.section_count`와 일치해야 하나, 리더는 실제 존재 스트림을 신뢰한다.

**압축 대상 판정** `is_record_stream(path)` (`container.rs:114`): `/DocInfo`, `/BodyText/`, `/ViewText/`, `/Scripts/`로 시작하는 스트림만 FileHeader 압축 플래그의 적용을 받는다. FileHeader/PrvText/PrvImage/BinData/요약정보는 이 판정에서 제외된다(BinData는 read 경로에서 별도 시도-폴백).

**보호 게이트**: `check_body_readable()`은 암호 없는 호환 진입점이다.
`read_document_with_options`는 먼저 `check_body_readable_with_password`를 호출한다. 암호가 공급된
암호화 문서는 실증 프로파일에서만 인증·복호화한 뒤 `check_body_readable_after_password()`로 남은
인증서·DRM·서명 순서를 적용한다. 암호 부재·오류·미지원 프로파일은 CLI가 안정된 공개 거부로
매핑하기 전에 `Hwp5Error::Encrypted`로 정규화한다. 배포용문서는 별도 bounded `distdoc` 경로에서
ViewText 레코드를 복호화하고 동일한 의미 파서에 넘긴다.

---

## 3. FileHeader (256바이트 고정)

`/FileHeader` 스트림은 정확히 256바이트여야 한다(`FILE_HEADER_SIZE`). 다르면 `BadFileHeaderSize`. 리틀엔디언 `ByteReader`로 순차 파싱한다.

| 오프셋 | 크기 | 타입 | 필드 | 비고 |
|---|---|---|---|---|
| 0 | 32 | bytes | 시그니처 | 앞 17바이트 `"HWP Document File"` 일치 필수, 나머지 NUL 패딩 |
| 32 | 4 | u32 LE | 버전 | `0xMMnnPPrr` — `0x05000300` = 5.0.3.0 |
| 36 | 4 | u32 LE | 속성 플래그 | 아래 비트표 |
| 40 | 4 | u32 LE | 라이선스 플래그 | CCL/공공누리 |
| 44 | 4 | u32 LE | EncryptVersion | 암호화 버전 |
| 48 | 1 | u8 | KOGL 국가 코드 | 공공누리 지원 국가 |
| 49 | 207 | bytes | 예약 | 왕복 보존용으로 그대로 유지 |

**버전 인코딩** `HwpVersion::from_u32`: `major=v>>24`, `minor=v>>16 & 0xFF`, `build=v>>8 & 0xFF`, `revision=v & 0xFF`. 표시는 `"5.0.3.0"`.

**속성 플래그 비트**(36 오프셋 DWORD, `file_header.rs::attr`):

| 비트 | 상수 | 의미 |
|---|---|---|
| 0 | COMPRESSED | 압축(레코드 스트림 raw deflate) |
| 1 | ENCRYPTED | 암호화 |
| 2 | DISTRIBUTION | 배포용 문서(ViewText) |
| 3 | HAS_SCRIPT | 스크립트 저장 |
| 4 | DRM | DRM 보안 |
| 5 | HAS_XML_TEMPLATE | XMLTemplate 스토리지 |
| 6 | HAS_HISTORY | 문서 이력 관리 |
| 7 | HAS_SIGNATURE | 전자 서명 |
| 8 | CERT_ENCRYPTED | 공인 인증서 암호화 |
| 9 | SIGNATURE_SPARE | 전자 서명 예비 |
| 10 | CERT_DRM | 공인 인증서 DRM |
| 11 | CCL | CCL 문서 |
| 12 | MOBILE_OPTIMIZED | 모바일 최적화 |
| 13 | PRIVACY_SECURITY | 개인정보 보안 |
| 14 | TRACK_CHANGES | 변경 추적 |
| 15 | KOGL | 공공누리 저작권 |
| 16 | HAS_VIDEO_CONTROL | 비디오 컨트롤 |
| 17 | HAS_TOC_FIELD | 차례 필드 컨트롤 |

읽기 분기에 쓰이는 것은 `COMPRESSED`(bit0), `ENCRYPTED`(bit1), `DISTRIBUTION`(bit2), `DRM`(bit4),
`HAS_SIGNATURE`(bit7), `CERT_ENCRYPTED`(bit8), `CERT_DRM`(bit10)이다. 서명 예비 bit9는 실제 의미를
확립하지 못해 메타데이터로만 유지한다. 모든 이름은 `attribute_names()`로 `hwp info`에도 표시한다.

---

## 4. 압축(raw deflate)과 인코딩(UTF-16LE)

**압축** (`codec/compress.rs`): 압축 대상 레코드 스트림은 **zlib 헤더/Adler32 체크섬이 없는 raw DEFLATE**다(pyhwp의 `wbits=-15`와 동일). 해제는 `flate2::read::DeflateDecoder`(zlib 래퍼가 아닌 순수 deflate)로 `read_to_end`. 실패 시 `Hwp5Error::Decompress { stream, source }`. 재구현 시 zlib 스트림으로 착각해 앞 2바이트를 헤더로 소비하면 안 된다 — 첫 바이트부터 deflate 블록이다. BinData는 개별 압축 여부가 레코드별로 다를 수 있어 read 경로에서 `decompress(...).unwrap_or(raw)`로 시도-폴백한다.

**인코딩**: 모든 텍스트/문자열은 **UTF-16LE**. `ByteReader::read_wchars(n)`이 `n`개의 u16 코드 유닛을 읽는다. HWP 문자열(`read_hwp_string`)은 **`WORD 길이(코드 유닛 수) + UTF-16LE 데이터`**이며 길이에 종단 NUL은 포함하지 않는다(요약정보의 LPWSTR은 별도로 NUL 포함 카운트를 씀 — §11 참조). 디코딩은 `String::from_utf16_lossy`로 손상 내성. 본문 텍스트는 서로게이트 쌍을 직접 처리한다(§8).

---

## 5. 레코드 헤더 비트 레이아웃

압축 해제된 DocInfo/BodyText/Scripts 스트림은 **레코드의 나열**이고, 각 레코드는 4바이트(또는 8바이트) 헤더 + 페이로드다. 헤더는 단일 u32 LE에 세 필드를 비트 패킹한다(`record/header.rs`):

```
u32 LE = tagID(하위 10비트) | level(다음 10비트) | size(상위 12비트)
```

| 비트 범위 | 필드 | 마스크/시프트 | 폭 |
|---|---|---|---|
| 0..10 | tagID | `v & 0x3FF` | 10비트 (0~1023) |
| 10..20 | level | `(v >> 10) & 0x3FF` | 10비트 (트리 깊이) |
| 20..32 | size | `(v >> 20) & 0xFFF` | 12비트 (0~4095, 페이로드 바이트 수) |

**확장 크기**: `size` 비트필드 값이 `0xFFF`(`SIZE_EXTENDED`)이면 **다음 u32 LE가 실제 크기**다. 따라서 헤더는 4바이트 또는 8바이트다. 경계 규칙에 주의: `0xFFF` 자체는 인라인으로 표현할 수 없으므로(그 값이 확장 표식으로 예약됨) **`size >= 0xFFF`인 경우 무조건 확장형**으로 기록·해석한다. 즉 인라인 최대는 `0xFFE`(4094). 디코드 의사코드:

```
v = read_u32()
tag   = v & 0x3FF
level = (v >> 10) & 0x3FF
sf    = (v >> 20) & 0xFFF
size  = if sf == 0xFFF { read_u32() } else { sf }
payload = read_bytes(size)
```

태그는 **항상 원시 u16으로 보존**한다. enum으로 강제 변환하지 않으므로 미지 태그도 정보 손실 없이 통과한다. `tag::tag_name(u16)`은 덤프용 이름 조회일 뿐 파싱 분기와 무관하다.

---

## 6. 스캔과 트리 복원

### 6.1 평면 스캔 (`record/scan.rs`)

`scan_stream(data, mode) -> ScanResult { roots, warnings, record_count }`:

```
while !r.is_empty():
    at = r.pos()
    header = RecordHeader::decode(r)?      # Tolerant: 실패 시 경고+break
    payload = r.read_bytes(header.size)    # Tolerant: 부족 시 남은 전부 보존+경고
    flat.push((header, payload))
(roots, tree_warnings) = RecordNode::build_forest(flat)
```

- **Strict**: 헤더 잘림/페이로드 부족/트리 경고 중 어느 하나라도 있으면 즉시 `Err`. writer 왕복 검증용.
- **Tolerant**: 헤더 잘리면 "스캔 중단" 경고 후 break; 페이로드 부족하면 남은 바이트를 잘린 채 보존하고 경고. read 경로 전용.

### 6.2 트리 복원 (`record/tree.rs`)

레코드의 `level` 필드 = 트리 깊이다. 스택 기반으로 복원한다(`build_forest`):

```
stack: Vec<RecordNode>   # stack[i] = 깊이 i에 열린 노드
for (hdr, data) in flat:
    level = hdr.level
    if level > stack.len():           # 부모 없이 깊어짐(손상)
        경고; level = stack.len()      # 가장 가까운 조상에 연결
    while stack.len() > level:         # 더 깊게 열린 노드들을 닫아 부모/roots에 부착
        attach(stack.pop())
    stack.push(RecordNode{tag, data, children:[]})
스택에 남은 것 모두 attach
```

`RecordNode { tag: u16, data: Vec<u8>, children: Vec<RecordNode> }`. **불변식**: 잘 형성된 파일에서는 `serialize_forest`(깊이로 level 재계산)가 **압축 해제 스트림과 바이트 동일**하게 복원된다 — 이것이 무손실 왕복의 기반이다. level이 비단조로 튀는 손상은 가장 가까운 조상에 붙이고 경고만 남긴다.

---

## 7. DocInfo 파싱 (`doc_info.rs`)

`parse_doc_info(roots) -> (DocHeader, warnings)`. 루트 레코드를 순회하며 두 개만 직접 해석한다: `DOCUMENT_PROPERTIES`(0x10), `ID_MAPPINGS`(0x11). 나머지 루트는 `header.extras`로 opaque 보존.

### 7.1 태그 상수 (`record/tag.rs`)

`HWPTAG_BEGIN = 0x010`. DocInfo 계열은 `BEGIN + n`:

| 태그 | 값 | 이름 | 태그 | 값 | 이름 |
|---|---|---|---|---|---|
| +0 | 0x10 | DOCUMENT_PROPERTIES | +7 | 0x17 | NUMBERING |
| +1 | 0x11 | ID_MAPPINGS | +8 | 0x18 | BULLET |
| +2 | 0x12 | BIN_DATA | +9 | 0x19 | PARA_SHAPE |
| +3 | 0x13 | FACE_NAME | +10 | 0x1A | STYLE |
| +4 | 0x14 | BORDER_FILL | +11 | 0x1B | DOC_DATA |
| +5 | 0x15 | CHAR_SHAPE | +14 | 0x1E | COMPATIBLE_DOCUMENT |
| +6 | 0x16 | TAB_DEF | +15 | 0x1F | LAYOUT_COMPATIBILITY |

### 7.2 DOCUMENT_PROPERTIES (0x10) — 26바이트

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | u16 | section_count (구역 수) |
| 2 | u16×6 | start_numbers (페이지/각주/미주/그림/표/수식 시작 번호) |
| 14 | u32×3 | caret 위치 (list_id, para_id, char_pos) |

### 7.3 ID_MAPPINGS (0x11) — 카운트 배열 + 자식 테이블

페이로드는 **u32 카운트 배열**이다. 배열을 끝까지 읽어 `id_mappings_counts`로 보존한다. 순서(스펙): `[binData, 글꼴×7(언어별), 테두리채움, 글자모양, 탭, 번호, 글머리표, 문단모양, 스타일, (메모모양, 변경추적, 변경추적사용자…)]`. 인덱스 1..8이 언어 슬롯(한글/영어/한자/일어/외국어/기호/사용자)별 글꼴 수 = `font_counts[0..7]`.

**실제 테이블 항목은 ID_MAPPINGS의 자식 레코드**로 나열된다. `parse_id_mapping_child`가 태그별로 분류하며, 순서는 카운트 배열 순서를 따른다. **FACE_NAME의 언어 슬롯 배정**: `font_cursor`를 두고 현재 슬롯의 채워진 글꼴 수가 `font_counts[cursor]`에 도달하면 다음 언어 슬롯으로 넘긴다(글꼴 레코드 자체엔 언어 표시가 없어 카운트로 역산).

각 자식 파서(모두 prefix+tail 규칙, 실패 시 기본값 push + opaque):

**FACE_NAME (0x13)** — 가변:

| 오프셋 | 타입 | 필드 | 조건 |
|---|---|---|---|
| 0 | u8 | attr | bit7=대체글꼴, bit6=PANOSE, bit5=기본글꼴 |
| 1 | HWP str | name | 글꼴 이름 |
| … | u8 + HWP str | alt_kind + alt_name | attr&0x80 |
| … | 10 bytes | panose | attr&0x40 |
| … | HWP str | default_name | attr&0x20 |
| … | tail | 나머지 보존 | |

**CHAR_SHAPE (0x15)** — 68바이트 prefix + tail:

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | u16×7 | face_ids (언어별 글꼴 ID) |
| 14 | u8×7 | ratios (장평) |
| 21 | i8×7 | spacings (자간) |
| 28 | u8×7 | rel_sizes (상대 크기) |
| 35 | i8×7 | offsets (글자 위치) |
| 42 | i32 | base_size (기준 크기, HWPUNIT) |
| 46 | u32 | attr (굵게/기울임/밑줄/외곽선 등 비트) |
| 50 | i8,i8 | shadow_gap (그림자 간격 x,y) |
| 52 | u32 | text_color (COLORREF) |
| 56 | u32 | underline_color |
| 60 | u32 | shade_color |
| 64 | u32 | shadow_color |
| 68 | tail | tail[0..2]=border_fill_id(5.0.2.1+), tail[2..6]=취소선 색(5.0.3.0+) |

주의: raw `attr`의 취소선 비트(18~20)는 DIFFSPEC 의미라 신뢰하지 않는다(`strike: false` 고정, 가짜 취소선 방지).

**PARA_SHAPE (0x19)** — 42바이트 prefix + tail:

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | u32 | attr1 (bit0~1 = 줄간격 종류) |
| 4 | i32 | margin_left |
| 8 | i32 | margin_right |
| 12 | i32 | indent (들여쓰기) |
| 16 | i32 | spacing_top |
| 20 | i32 | spacing_bottom |
| 24 | i32 | line_spacing_old (구버전 줄간격) |
| 28 | u16 | tab_def_id |
| 30 | u16 | numbering_id |
| 32 | u16 | border_fill_id |
| 34 | u16×4 | border_offsets (좌/우/상/하) |
| 42 | tail | tail[0..4]=attr2(5.0.1.7+), tail[4..8]=attr3·tail[8..12]=line_spacing(5.0.2.5+) |

줄간격 값: `tail.len()>=12`이면 `line_spacing = tail[8..12]`(5.0.2.5+), 아니면 `line_spacing_old`. 종류는 `attr1 & 0x3`.

**STYLE (0x1A)**: `name(HWP str) + english_name(HWP str) + attr u8 + next_style u8 + lang_id i16 + para_shape u16 + char_shape u16 + tail`.

**BORDER_FILL (0x14)**: `attr u16` + 4변(좌우상하)×`{line_type u8, width u8, color u32}`(각 6B) + 대각선(6B) + `fill_type u32` + (`fill_type&1`이면 `bg_color u32`) + tail.

**BIN_DATA (0x12)**: `attr u16`; `kind = attr & 0xF`(0=링크, 1=임베딩, 2=스토리지). kind=0이면 `link_abs, link_rel`(HWP str 2개); 아니면 `storage_id u16`, kind=1이면 추가로 `extension`(HWP str). + tail.

**TAB_DEF(0x16)/BULLET(0x18)**: raw 보존. BULLET은 오프셋 8..10의 WCHAR를 글머리 문자로 뽑되 제어문자면 `•` 폴백.

**NUMBERING (0x17)**: raw 보존 + 렌더용 7수준 템플릿 파싱(`parse_numbering_levels`). 각 수준 = `attr u32 + width u16 + dist u16 + charshape_ref u32(정품=0xFFFFFFFF) + template(HWP str)`. `charshape_ref != 0xFFFFFFFF`면 그 수준부터 기본값 폴백. 예: `["^1.","^2.","^3)","^4)","(^5)","(^6)","^7"]`.

`DocHeader`에는 `fonts[7][], char_shapes[], para_shapes[], styles[], border_fills[], bin_data[], numberings[], bullets[]`가 등장 순서대로 채워지며 각 인덱스가 곧 참조 ID다.

---

## 8. BodyText 파싱 (`body_text.rs`)

`parse_section(roots) -> (Section, warnings)`. 섹션 루트는 **PARA_HEADER 트리들의 나열**이다. PARA_HEADER가 아닌 루트는 경고 + `section.extras` opaque 보존.

BodyText 계열 태그(`HWPTAG_BEGIN + n`):

| 태그 | 값 | 이름 | 태그 | 값 | 이름 |
|---|---|---|---|---|---|
| +50 | 0x42 | PARA_HEADER | +60 | 0x4C | SHAPE_COMPONENT |
| +51 | 0x43 | PARA_TEXT | +61 | 0x4D | TABLE |
| +52 | 0x44 | PARA_CHAR_SHAPE | +62 | 0x4E | SHAPE_COMPONENT_LINE |
| +53 | 0x45 | PARA_LINE_SEG | +63 | 0x4F | SHAPE_COMPONENT_RECTANGLE |
| +54 | 0x46 | PARA_RANGE_TAG | +64 | 0x50 | SHAPE_COMPONENT_ELLIPSE |
| +55 | 0x47 | CTRL_HEADER | +65 | 0x51 | SHAPE_COMPONENT_ARC |
| +56 | 0x48 | LIST_HEADER | +66 | 0x52 | SHAPE_COMPONENT_POLYGON |
| +57 | 0x49 | PAGE_DEF | +67 | 0x53 | SHAPE_COMPONENT_CURVE |
| +58 | 0x4A | FOOTNOTE_SHAPE | +69 | 0x55 | SHAPE_COMPONENT_PICTURE |
| +59 | 0x4B | PAGE_BORDER_FILL | +70 | 0x56 | SHAPE_COMPONENT_CONTAINER |

### 8.1 PARA_HEADER (0x42) — 22바이트 prefix + tail

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | u32 | nchars_raw (bit31=플래그, `& 0x7FFFFFFF` = 문단 WCHAR 수) |
| 4 | u32 | ctrl_mask (문단에 등장하는 컨트롤 종류 비트마스크) |
| 8 | u16 | para_shape_id |
| 10 | u8 | style_id |
| 11 | u8 | break_type (단 나누기 비트) |
| 12 | u16 | char_shape_count |
| 14 | u16 | range_tag_count |
| 16 | u16 | line_seg_count |
| 18 | u32 | instance_id |
| 22 | tail | 버전별 꼬리(변경추적 병합 등) 보존 |

`chars_flags = (nchars_raw >> 24) & 0x80`. `nchars = nchars_raw & 0x7FFFFFFF`은 뒤에서 위치 산수 검증에 쓰인다.

PARA_HEADER의 자식으로 PARA_TEXT/PARA_CHAR_SHAPE/PARA_LINE_SEG/CTRL_HEADER가 온다.

### 8.2 PARA_TEXT (0x43) — 컨트롤 문자 분류가 위치 산수의 기준

페이로드를 u16 LE 배열(WCHAR)로 읽는다(홀수 길이면 마지막 바이트 무시 + 경고). 각 유닛 `u`를 다음 규칙으로 소비(`decode_para_text`):

- **`u >= 32`**: 일반 문자. 서로게이트 상위(0xD800..0xDC00)면 다음 유닛과 쌍(`decode_utf16`) → char 1개(2 WCHAR); 짝 없으면 `U+FFFD` + 경고.
- **`u < 32`**: `char_kind(u)`로 분류(§9). 
  - `Char`(1 WCHAR): `HwpChar::CharCtrl(u)`, `i += 1`.
  - `Inline`/`Extended`(8 WCHAR): `[코드 u, 정보 6 WCHAR(12B), 닫는 코드 u]` 구조를 읽는다. 닫는 코드는 여는 코드와 같아야 함(불일치 시 경고). `i += 8`. 정보 6 WCHAR를 `payload: Vec<u8>`로 보존.
    - Inline → `HwpChar::InlineCtrl { code, payload }`.
    - Extended → payload 선두 4바이트가 **역순 ctrl_id**이므로 뒤집어 `ctrl_id`로 보관 → `HwpChar::ExtCtrl { code, ctrl_id, payload, ctrl_index: None }`.

8 WCHAR가 잘리면(`i + 8 > len`) 경고 후 중단.

### 8.3 PARA_CHAR_SHAPE (0x44)

8바이트씩 반복: `pos u32`(문단 내 WCHAR 시작 위치) + `id u32`(CharShapeId, 하위 16비트만 사용). `char_shape_runs: Vec<(u32, CharShapeId)>`.

### 8.4 PARA_LINE_SEG (0x45) — 36바이트/줄

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | u32 | text_start (줄 시작 WCHAR 오프셋) |
| 4 | i32 | v_pos (세로 위치) |
| 8 | i32 | line_height |
| 12 | i32 | text_height |
| 16 | i32 | baseline_gap |
| 20 | i32 | line_spacing |
| 24 | i32 | col_start |
| 28 | i32 | seg_width |
| 32 | u32 | flags (페이지 첫 줄/컬럼 첫 줄/빈 세그 등) |

`remaining >= 36`인 동안 반복. 렌더러가 그대로 신뢰하는 1급 배치 입력.

### 8.5 CTRL_HEADER (0x47) — 확장 컨트롤 본체

페이로드 앞 4바이트가 **역순 저장 ctrl_id**다(예: 스트림 `dces` → 뒤집어 `secd`). 4바이트 미만이면 `????` Generic. 뒤집은 `ctrl_id`로 분기(`parse_control`):

| ctrl_id | 종류 | 파서 |
|---|---|---|
| `secd` | 구역 정의 | `parse_section_def` — 자식 PAGE_DEF 파싱 |
| `tbl ` | 표 | `parse_table` |
| `gso ` | 그리기 개체 | 문단 없고 PICTURE 레코드 있으면 `parse_picture_gso`, 아니면 Generic |
| 그 외 | 일반 | `parse_generic` — 문단 리스트 재귀 수집 + 원본 자식 보존 |

`rest = data[4..]`가 컨트롤별 페이로드. Generic은 `raw_children`에 자식 서브트리를 중첩 그대로 보존(무손실 재직렬화용)하고, 별도로 `paragraph_lists`를 평탄화 수집한다.

**컨트롤 연결** `link_controls`: 문단 텍스트의 `ExtCtrl`들을 등장 순서대로 `controls[]`와 1:1 매칭한다. 각 `ExtCtrl.ctrl_index`를 채우고, 텍스트의 `ctrl_id`와 CTRL_HEADER의 `ctrl_id`가 다르면 경고. 남거나 모자라면 경고. 이 매칭이 어긋나면 위치 계산이 전부 틀어지므로 강력한 검증 지점이다.

**위치 불변식**: PARA_HEADER의 `nchars`와 PARA_TEXT에서 계산한 `wchar_len()`(문자별 `wchar_width`: 일반 1, BMP 밖 2, 컨트롤 8)이 일치해야 한다. 불일치는 컨트롤 분류 오류 신호로 경고.

### 8.6 PAGE_DEF (0x49) — 40바이트

`width, height, margin_left, margin_right, margin_top, margin_bottom, margin_header, margin_footer, gutter`(i32×9, HWPUNIT) + `attr u32`. SectionDef(`secd`)의 자식으로 등장.

### 8.7 TABLE (0x4D) + 셀 (LIST_HEADER 0x48)

표는 `CTRL_HEADER(tbl )` 아래에 **TABLE 레코드 1개 + 셀마다 [LIST_HEADER, PARA_HEADER…]가 형제로** 나열된다. LIST_HEADER가 새 셀을 열고 다음 LIST_HEADER 전까지의 PARA_HEADER가 그 셀 소속.

**TABLE 레코드**:

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | u32 | attr |
| 4 | u16 | rows |
| 6 | u16 | cols |
| 8 | u16 | cell_spacing |
| 10 | u16×4 | inner_margins (좌우상하) |
| 18 | u16×rows | row_cell_counts (행별 셀 개수) |
| 18+2·rows | u16 | border_fill_id |
| … | tail | 보존 |

**셀 LIST_HEADER**(실측 46B prefix + tail):

| 오프셋 | 타입 | 필드 |
|---|---|---|
| 0 | i32 | para_count |
| 4 | u32 | list_attr |
| 8 | u16 | col |
| 10 | u16 | row |
| 12 | u16 | col_span |
| 14 | u16 | row_span |
| 16 | i32 | width (HWPUNIT) |
| 20 | i32 | height |
| 24 | u16×4 | margins |
| 32 | u16 | border_fill_id |
| 34 | tail | 보존 |

### 8.8 그림 개체 gso (`parse_picture_gso`)

개체 공통 속성(`rest`): `attr u32 + v_offset u32 + h_offset u32 + width i32 + height i32`. `treat_as_char = attr & 1`. 자식에서 `SHAPE_COMPONENT_PICTURE`(0x55) 레코드를 찾아 **오프셋 71의 u16 = BinItem ID**를 읽어 `BinRef::Id`로. common_data 전체를 보존해 배치 무손실.

---

## 9. 컨트롤 문자 분류표 (`hwp-model/src/paragraph.rs`)

0~31 코드의 분류가 reader/writer/텍스트 추출의 **단일 진실 공급원**이다. 8 WCHAR 컨트롤을 하나라도 잘못 세면 이후 위치 계산이 전부 어긋난다.

| 분류 | 코드 | WCHAR | 의미 |
|---|---|---|---|
| Char | 0, 10, 13, 24~31 | 1 | 그 자체로 의미(줄바꿈 10, 문단끝 13, 하이픈 24, 묶음빈칸 30, 고정폭빈칸 31 …) |
| Inline | 4~9, 19, 20 | 8 | `[코드, 정보 6 WCHAR, 코드]` 자체 완결(필드끝 4, 탭 9 …) |
| Extended | 1~3, 11~12, 14~18, 21~23 | 8 | 별도 CTRL_HEADER를 가리킴(구역/단 2, 필드시작 3, 개체 11, 각주 17, 자동번호 18 …) |
| Char | 32+ | 1 | 일반 문자 |

`ctrl_mask`(PARA_HEADER 오프셋 4)는 문단에 등장하는 컨트롤 종류를 비트로 요약한 힌트로, 리더는 실제 PARA_TEXT를 순회해 컨트롤을 세므로 파싱 필수 입력은 아니며 보존만 한다.

---

## 10. SHAPE_COMPONENT / SC_* 기하 (`hwp-render/src/shape_draw.rs`)

`body_text.rs`는 그리기 개체 하위 SHAPE_COMPONENT/SC_* 서브트리를 opaque(`raw_children`)로 보존하고, **기하 해석은 렌더 소비단**이 수행한다(IR·라운드트립 라이터 불변). 좌표 변환: local(생성) 공간 점(HWPUNIT) → 렌더 행렬(T·S·R) → `+origin`(HWPUNIT) → `/100` = pt.

**SHAPE_COMPONENT (0x4C)** `parse_style`: 앞 4바이트가 CHID. `d[0..4]==d[4..8]`이면 최상위 개체(CHID 2회) → `base=8`, 아니면 묶음 멤버 → `base=4`. `cnt = u16 @ base+42`(scale/rotation 쌍 수). translation 행렬 = `rd_mat @ base+44`(6×f64=48B, `[a,b,c,d,e,f]` row-major: `x'=a·x+b·y+c, y'=d·x+e·y+f`). 최종 행렬 = `T · (scale_last · rotation_last)`, 마지막 쌍은 `base+44+48+(cnt-1)·96`. 테두리/채우기는 `base+92+cnt·96`부터: `color u32 + width i32 + lattr u32`(`lattr&0x3F`면 선), 이어 fill(`ft u32`: bit0 단색→`color`, bit2 그러데이션→Table28, bit1 이미지→BinItem).

**SC_* 기하 바이트 레이아웃**(`geometry`, 좌표는 i32 HWPUNIT):

| 레코드 | 값 | 레이아웃 |
|---|---|---|
| SC_LINE | 0x4E | 시작(x,y @0) + 끝(x,y @8) |
| SC_RECTANGLE | 0x4F | `곡률% u8 @0` + 4점(@1,@9,@17,@25). 곡률>0이면 둥근 모서리 |
| SC_ELLIPSE | 0x50 | `attr u32 @0` + center(@4) + ax1끝점(@12) + ax2끝점(@20) |
| SC_ARC | 0x51 | `arctype u8 @0` + center(@1) + start(@9) + end(@17) |
| SC_POLYGON | 0x52 | `count u16 @0` + 점 배열(@4, 8B stride) |
| SC_CURVE | 0x53 | `count u16 @0` + 점 배열(@2, 8B stride) — 폴리라인 근사 |
| SC_CONTAINER | 0x56 | 묶음 — 자식으로 재귀(`MAX_DEPTH=16`) |

원호/타원은 KAPPA(0.5522847498) 큐빅 베지에로 근사. **그러데이션(Table 28)**: `type i16, angle i16, cx i16, cy i16, spread i16, num i16(@+10)`; `num>2`면 `i32[num]` 위치 후 `COLORREF[num]` 색(아니면 균등 분포). `type==1`이면 방사형.

---

## 11. 요약 정보 (`summary.rs`)

`/\x05HwpSummaryInformation`은 **MS-OLEPS 속성 집합**. 최선 노력 파싱(어디서 어긋나도 그때까지 값으로 `Metadata` 반환):

- 바이트오더 `u16 @0 == 0xFFFE` 확인.
- `section_count u32 @24`.
- 첫 섹션 오프셋 `u32 @44`(FMTID 16B 건너뜀; 헤더 28 + FMTID 16 = 44).
- 섹션: `size u32 @sec_off`, `prop_count u32 @sec_off+4`, 이어서 `[pid u32, offset u32]` 표(@sec_off+8).
- 값 오프셋은 섹션 시작 기준. `VT_LPWSTR`(타입 31) 값 = `type u32 + count u32(NUL 포함 코드 유닛 수) + UTF-16LE`. NUL에서 종단.

PID 매핑: 0x02=title, 0x03=subject, 0x04=author, 0x05=keywords. 손상 count는 남은 바이트 기준으로 클램프(자원 고갈 방어).

---

## 12. 재구현 체크리스트와 불변식

1. **바이트 커서**: 모든 읽기는 부족 시 panic 없이 `Err(UnexpectedEof)`. 리틀엔디언 고정.
2. **CFB**: 라이브러리 위임 권장. 스트림 경로 규약(§2)만 지키면 됨.
3. **압축**: raw deflate(zlib 헤더 없음). `is_record_stream` 대상에만, 그것도 FileHeader COMPRESSED 비트가 켜졌을 때만.
4. **레코드 헤더**: 10/10/12 비트, `size==0xFFF`(즉 값 `>=0xFFF`)면 8바이트 확장. 태그는 u16 원시 보존.
5. **level = 트리 깊이** 불변식 → 깊이로 재직렬화하면 압축 해제 스트림과 바이트 동일(무손실 왕복의 근간).
6. **prefix + tail** 규칙: 모든 레코드 파서는 알려진 앞부분만 뜯고 나머지는 `tail`로 보존. 미지 태그/레코드는 `OpaqueRecord`로 서브트리째 보존.
7. **ctrl_id 역순 저장**: CTRL_HEADER와 ExtCtrl payload의 앞 4바이트는 뒤집어 해석(`dces`→`secd`).
8. **위치 산수**: PARA_HEADER `nchars == Σ wchar_width`. 컨트롤은 8 WCHAR, BMP 밖 문자는 2, 그 외 1.
9. **관용 모드**: read는 Tolerant(경고 누적 + opaque 보존, 중단 없음), writer 검증만 Strict.
10. **인코딩**: UTF-16LE 일관. HWP 문자열 = WORD 길이(코드 유닛) + 데이터, 종단 NUL 없음(요약정보 LPWSTR만 NUL 포함 카운트).

관련 파일 경로(절대): `/Users/elevn/projects/hwp-cli/crates/hwp5/src/{read,container,file_header,doc_info,body_text,summary,error}.rs`, `.../hwp5/src/codec/{reader,writer,compress}.rs`, `.../hwp5/src/record/{header,scan,tree,tag}.rs`, `/Users/elevn/projects/hwp-cli/crates/hwp-model/src/paragraph.rs`, `/Users/elevn/projects/hwp-cli/crates/hwp-render/src/shape_draw.rs`.
