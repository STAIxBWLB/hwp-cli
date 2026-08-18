[한국어](10-hwp5-structure-map.ko.md) · [English](10-hwp5-structure-map.md)

# HWP 5.0 포맷 전수 지도 (Structure Map)

HWP 5.0 파일이 담는 **모든 스트림·레코드 태그·컨트롤 문자·확장 컨트롤 ID**를 한 곳에서
조회하고, 각각을 hwp-cli가 어떤 충실도로 다루는지 확인하는 **단일 진실(single source of
truth)** 문서다. "무엇이 존재하며 우리가 그것을 어떻게 처리하는가"의 카탈로그다.

## 다른 문서와의 역할 분담

| 문서 | 다루는 것 | 이 문서와의 관계 |
|---|---|---|
| [02-hwp5-read](02-hwp5-read.ko.md) | **파싱 방법** — 비트 레이아웃, 커서, 트리 복원 알고리즘 | 여기서 "상세는 [02] §n"로 링크. 이 문서는 *지도*, 02는 *구현 명세* |
| [03-hwp5-write](03-hwp5-write.ko.md) | **합성/쓰기** — CFB V3, 버전 게이팅, 한글 호환 조립 | 스트림 트리(§1)의 쓰기 열이 03의 요약 |
| [12-feature-gaps](12-feature-gaps.ko.md) | **미구현·손실 목록** — Opaque/raw보존 레코드가 실제로 무엇을 잃는가 | 이 문서 표 A/B의 `Opaque`·`raw보존` 행이 12의 입력 |
| [19-hwp5-spec-supplement](19-hwp5-spec-supplement.ko.md) | **명세 보완 색인** — 정오표·버전 매트릭스·적합성 규칙 | 이 문서의 ★ 항목이 19 §1 정오표의 입력 |

이 문서의 상태 분류(§3~§4)가 확정 사실이고, 12번은 그 사실이 실기에서 어떤 결함으로 드러나는지를
평가한다. 상태 라벨을 바꿔야 하면 **여기부터** 고친다.

### 저작권 고지

스펙 표·문구는 전재하지 않는다. 태그 이름(`HWPTAG_*`)·값·스펙 § 번호는 사실 인용이며,
"페이로드 요약" 열은 스펙 설명 복사가 아니라 **코드가 실제로 읽는 레이아웃**을 자체 문구로
기술한 것이다. 스펙 문구와 코드 실측이 어긋나는 지점은 ★로 표시한다. 스펙 원본은 동봉하지 않는다
([docs/README.ko.md](../README.ko.md)).

### 상태 4분류 (표 A~D 공통)

| 라벨 | 뜻 |
|---|---|
| **의미파싱** | 레코드 필드를 IR의 이름 있는 구조체로 완전 해석. 렌더·편집이 값으로 동작. |
| **부분파싱+raw** | 알려진 prefix만 구조체로 뜯고 tail 보존. 또는 일부 필드만 해석(렌더 힌트)하고 나머지 raw. |
| **raw보존** | 의미 해석 없이 바이트 통째 보존하되, 소비단(렌더러 등)이 별도로 파싱하거나 무손실 재방출에 쓰는 1급 슬롯(`raw_children`, `common_data`, `RawEntry` 등)이 있다. |
| **Opaque** | `OpaqueRecord`로 `extras`/`id_extras`에 서브트리째 보존. 어떤 소비단도 해석하지 않음 — 순수 무손실 왕복만. |

`상수 미정의(Opaque 경유 보존)`는 `tag.rs`에 상수가 없는 스펙 레코드다. 태그를 u16 원시로
통과시키므로 스캔·트리 복원에는 문제없고, 결국 Opaque로 보존된다.

---

## 1. CFB 스토리지/스트림 트리

HWP 5.0 파일은 MS **CFB(Compound File Binary)** 컨테이너다. 아래가 전체 트리와 읽기·쓰기 지원
매트릭스다. "압축"은 FileHeader COMPRESSED 비트(bit0)가 켜졌을 때 **레코드 스트림에만** 적용되는
raw DEFLATE(zlib 헤더/Adler32 없음)를 뜻한다.

```
/ (루트 스토리지)
├── FileHeader                     256B 고정, 비압축
├── DocInfo                        문서 정보 레코드 스트림 (압축)
├── BodyText/                      스토리지
│   ├── Section0                   본문 구역 0 레코드 스트림 (압축)
│   ├── Section1 … SectionN
├── ViewText/                      배포용 본문 (배포용문서 — cat/convert/render가
│   └── Section0 …              읽기 시 투명하게 복호화, GATE-01)
├── BinData/                       스토리지
│   ├── BIN0001.png                첨부 바이너리 (헤더 플래그 따름)
│   └── …
├── \x05HwpSummaryInformation      OLE 속성 집합 (비압축)
├── PrvText                        미리보기 텍스트 UTF-16LE (비압축)
├── PrvImage                       미리보기 이미지 PNG/BMP (비압축)
├── DocOptions/                    스토리지
│   ├── _LinkDoc                   문서 연결 옵션
│   └── (DrmLicense·DrmRootSect·CertDrmHeader·CertDrmInfo·
│        DigitalSignature·PublicKeyInfo — 옵션 스트림, 미해석·미방출)
├── Scripts/                       스토리지
│   ├── JScriptVersion             (레코드 스트림 규칙 — 압축 대상)
│   └── DefaultJScript
├── XMLTemplate/                   XML 템플릿 스토리지 (미지원 — 통과)
├── DocHistory/                    문서 이력 관리 (미지원 — 통과)
└── Bibliography/                  참고문헌 XML (미지원 — 통과)
```

| 스트림/스토리지 | 스펙 § | 읽기 | 쓰기(합성) | 압축 | 근거 코드 |
|---|---|---|---|---|---|
| `/FileHeader` | 3.2.1 | 의미파싱 | 방출 | 아니오 | `file_header.rs:91`, `container.rs:35` |
| `/DocInfo` | 3.2.2 | 의미파싱 | 방출 | 예 | `doc_info.rs:18`, `write.rs:170` |
| `/BodyText/SectionN` | 3.2.3 | 의미파싱 | 방출 | 예 | `body_text.rs:23`, `write.rs:173` |
| `/ViewText/SectionN` | 3.2.3 | 의미파싱, 복호화 경유 (`cat`/`convert`/`render`); `hwp dump`·인증 bounded-read 경로는 여전히 거부 | — | 예(복호화 후) | `distdoc.rs`, `read.rs` (GATE-01) |
| `/BinData/*` | 3.2.5 | 시도-폴백 | 참조분만 동봉 | 헤더 플래그 | `read.rs`, `write.rs:190` |
| `/\x05HwpSummaryInformation` | 3.2.4 | 부분파싱+raw | 합성 | 아니오 | `summary.rs`, `write.rs:223` |
| `/PrvText` | 3.2.6 | 미해석 | 본문 발췌 생성 | 아니오 | `write.rs:225` |
| `/PrvImage` | 3.2.7 | 미해석 | 옵션 제공 시 | 아니오 | `write.rs:226` |
| `/DocOptions/_LinkDoc` | 3.2.8 | 미해석 | 상수 방출 | 아니오 | `write.rs:209` |
| `/DocOptions/{Drm*,CertDrm*,DigitalSignature,PublicKeyInfo}` | 3.2.8 | 미해석 | **미방출** | 아니오 | (분기 없음 — [12](12-feature-gaps.ko.md) GE-β3) |
| `/Scripts/*` | 3.2.9 | 미해석 | 표본 상수 방출 | 예(규칙상) | `write.rs:213`, `container.rs:114` |
| `/XMLTemplate/*` | 3.2.10 | 미지원(통과) | 없음 | — | — |
| `/DocHistory/*` | 3.2.11 | 미지원(통과) | 없음 | — | — |
| `/Bibliography/*` | 3.2.12 | 미지원(통과) | 없음 | — | (분기 없음 — [12](12-feature-gaps.ko.md) GB-12) |

**압축 판정** `is_record_stream(path)` (`container.rs:114`): `/DocInfo`, `/BodyText/`, `/ViewText/`,
`/Scripts/`로 시작하는 스트림만 압축 대상이다. FileHeader·PrvText·PrvImage·BinData·요약정보는
제외. BinData는 read 경로에서 `decompress(...).unwrap_or(raw)`로 개별 시도-폴백한다.

**★쓰기 비대칭**: writer는 DocInfo·BodyText·BinData를 raw deflate로 압축하지만, `/Scripts/*`는
한글 빈 문서에서 뜬 **표본 바이트(이미 압축된 형태)를 그대로** 방출하고(`write.rs:213`),
`_LinkDoc`은 524B 0으로, 보조 스트림은 부재 시 한글이 손상으로 판정할 수 있어 항상 만든다
(`write.rs:207`). CFB는 반드시 **버전 3(512B 섹터)** — V4(4096B)를 한글이 손상 파일로 본다
(`write.rs:167`, 실기 게이트).

**본문 구역 열거** `body_sections()` (`container.rs:62`): `/BodyText/Section` 접미 숫자를
정수 정렬한다(`Section10`이 `Section2` 뒤에 오도록 `parse::<u32>()` 필수).

**ViewText 배포 레코드** (`/ViewText/SectionN`, GATE-01): 각 스트림의 원본(압축 해제 전)
바이트는 `DISTRIBUTE_DOC_DATA` 레코드(태그 `0x1C`, level 0, 고정 256바이트 페이로드)로
시작하며, 이후 이어지는 바이트를 복호화할 AES-128 키를 난독화해 담고 있다. 이 레코드는
`/DocInfo` 레코드가 **아니다** — 스펙 표는 다른 DocInfo 항목들과 함께 묶지만(§3, 표 A),
정품 파일은 결코 그곳에 두지 않는다. 진품 11개 배포용문서 코퍼스 표본 전체를 직접 조사해
확립한 사실: `crates/hwp5/tests/distdoc_corpus.rs`의
`distribute_doc_data_lives_at_the_head_of_view_text_not_doc_info` 테스트가 각 코퍼스 파일의
`/ViewText/SectionN` 스트림 선행 레코드를 디코드하고, `/DocInfo`에는 그 태그를 가진 레코드가
없음을 별도로 확인한다. 디코드와 복호화는 `doc_info.rs`가 아닌 `distdoc.rs`에 있다. 이 레코드는
읽기 중 소비되고 버려질 뿐 재방출되지 않으므로 위 4가지 상태 라벨 어디에도 속하지 않는다.

---

## 2. 레코드 헤더 구조 (요약)

압축 해제된 DocInfo/BodyText/Scripts 스트림은 레코드의 나열이며, 각 레코드는 4바이트(또는 8바이트)
헤더 + 페이로드다. 단일 u32 LE에 세 필드를 비트 패킹한다.

```
u32 LE = tagID(하위 10비트) | level(다음 10비트) | size(상위 12비트)
size 비트필드 == 0xFFF 이면 다음 u32 LE가 실제 크기 (헤더 8바이트)
```

- **tagID**: 10비트(0~1023). 항상 원시 u16 보존 — enum 강제 변환 안 함(미지 태그 무손실 통과).
- **level**: 10비트, 트리 깊이. 깊이로 재직렬화하면 압축 해제 스트림과 바이트 동일(무손실 왕복 근간).
- **size**: 12비트(0~4094 인라인). `0xFFF`는 확장 표식으로 예약되어 인라인 불가.

스펙 §4.1(데이터 레코드 구조). 비트 마스크·시프트, Tolerant/Strict 스캔, 스택 기반 트리 복원의
**상세는 [02] §5(레코드 헤더 비트 레이아웃)·§6(스캔과 트리 복원)**. 근거: `record/header.rs`,
`record/scan.rs`, `record/tree.rs`.

---

## 3. DocInfo 레코드 카탈로그 (표 A)

`/DocInfo` 스트림에 오는 레코드 전수다. 스펙 표 13(§4.2)이 권위 목록이며, 아래 21행이 그것과 1:1
대응한다. `parse_doc_info`(`doc_info.rs:18`)는 루트에서 **DOCUMENT_PROPERTIES·ID_MAPPINGS만** 직접
해석하고, 실제 테이블 항목은 ID_MAPPINGS의 자식으로 `parse_id_mapping_child`(`doc_info.rs:64`)가
분류한다. 그 밖의 루트는 `header.extras`로, 미지 ID_MAPPINGS 자식은 `header.id_extras`로 Opaque 보존.

> ⚠️ **스펙 그룹핑 ≠ 코드 주석 그룹핑.** 스펙 표 13은 `MEMO_SHAPE`(BEGIN+76)·`FORBIDDEN_CHAR`(+78)·
> `TRACK_CHANGE`(+80)·`TRACK_CHANGE_AUTHOR`(+81)를 **DocInfo 레코드**로 분류한다(태그 값이 본문
> 수치 대역인데도). 그러나 `tag.rs`는 이들을 `// ── 본문(BodyText) 레코드` 주석 아래 둔다
> (`tag.rs:53~58`) — 태그 값이 `BEGIN+50` 이상이라 숫자 순서로 배치했기 때문이다. **정본은 스펙 표
> 13**: 이 넷은 의미상 DocInfo이므로 표 A에 넣었다. 주석의 위치를 근거로 "본문 레코드"라 오해하지 말 것.

| 태그 ID(hex) | 이름(HWPTAG_*) | 값(BEGIN+n) | 스펙 § | 페이로드 요약 | hwp-cli 상태 | 근거 코드 |
|---|---|---|---|---|---|---|
| 0x10 | DOCUMENT_PROPERTIES | +0 | 4.2.1 | 구역 수 u16 + 시작번호 u16×6 + 캐럿(list/para/char u32×3) | 의미파싱 | `doc_info.rs:152` |
| 0x11 | ID_MAPPINGS | +1 | 4.2.2 | u32 카운트 배열(binData·글꼴×7·…) + 자식이 실제 테이블 | 의미파싱 | `doc_info.rs:34` |
| 0x12 | BIN_DATA | +2 | 4.2.3 | attr u16(kind=하위4비트) + 링크경로 or storage_id/확장자 + tail | 부분파싱+raw | `doc_info.rs:357` |
| 0x13 | FACE_NAME | +3 | 4.2.4 | attr u8 + 이름 + [대체글꼴·PANOSE 10B·기본글꼴] + tail | 부분파싱+raw | `doc_info.rs:167` |
| 0x14 | BORDER_FILL | +4 | 4.2.5 | attr u16 + 4변(종류/굵기/색 각 6B) + 대각선 6B + 채우기 u32 + [배경색] + tail | 부분파싱+raw | `doc_info.rs:325` |
| 0x15 | CHAR_SHAPE | +5 | 4.2.6 | 68B prefix(글꼴ID×7·장평·자간·상대크기·글자위치·기준크기·attr·색×4) + tail | 부분파싱+raw | `doc_info.rs:206` |
| 0x16 | TAB_DEF | +6 | 4.2.7 | raw 보존 + 속성 u32·탭항목(위치 i32/종류 u8/채움 u8) 의미 파싱(`tab_stops`, 2026-07-15 GC-4) | 부분파싱+raw | `doc_info.rs:112`, `parse_tab_def` |
| 0x17 | NUMBERING | +7 | 4.2.8 | raw 보존 + 렌더용 7수준 형식 템플릿(`^1.` 등) 파싱 | 부분파싱+raw | `doc_info.rs:113`, `:401` |
| 0x18 | BULLET | +8 | 4.2.9 | raw 보존; 오프셋 8의 WCHAR만 글머리 글리프로 추출(제어문자면 `•`) | raw보존 | `doc_info.rs:120` |
| 0x19 | PARA_SHAPE | +9 | 4.2.10 | 42B prefix(attr1·여백·들여쓰기·간격·탭/번호/테두리 ID·오프셋×4) + tail(줄간격) | 부분파싱+raw | `doc_info.rs:260` |
| 0x1A | STYLE | +10 | 4.2.11 | 이름·영문명 + attr u8 + 다음스타일 u8 + 언어 i16 + 문단/글자모양 u16 + tail | 부분파싱+raw | `doc_info.rs:302` |
| 0x1B | DOC_DATA | +11 | 4.2.12 | 문서 임의 데이터 — 루트, 미해석 | Opaque | `doc_info.rs:57` |
| 0x1D | *(RESERVED)* | +13 | 4.2 표13 | 예약 — `tag.rs` 상수 없음 | 상수 미정의(Opaque 경유 보존) | `tag.rs`(미정의) |
| 0x1E | COMPATIBLE_DOCUMENT | +14 | 4.2.14 | 호환 문서 — 루트, 미해석(writer는 별도 합성) | Opaque | `doc_info.rs:57` |
| 0x1F | LAYOUT_COMPATIBILITY | +15 | 4.2.15 | 레이아웃 호환성 — 루트, 미해석 | Opaque | `doc_info.rs:57` |
| 0x20 | TRACKCHANGE | +16 | 4.2 표13 | 변경 추적 정보 — 루트, 미해석 | Opaque | `doc_info.rs:57` |
| 0x5C | MEMO_SHAPE | +76 | 4.2 표13 | 메모 모양 — ID_MAPPINGS 자식, 미해석 | Opaque | `doc_info.rs:148` |
| 0x5E | FORBIDDEN_CHAR | +78 | 4.2 표13 | 금칙처리 문자 — 미해석 | Opaque | `doc_info.rs:57`/`:148` |
| 0x60 | TRACK_CHANGE | +80 | 4.2 표13 | 변경 추적 내용/모양 — ID_MAPPINGS 자식, 미해석 | Opaque | `doc_info.rs:148` |
| 0x61 | TRACK_CHANGE_AUTHOR | +81 | 4.2 표13 | 변경 추적 작성자 — ID_MAPPINGS 자식, 미해석 | Opaque | `doc_info.rs:148` |

> ⚠️ **0x1C DISTRIBUTE_DOC_DATA는 `/DocInfo` 레코드가 아니다.** 스펙 §4.2.13은 위 DocInfo
> 항목들과 함께 묶지만, 정품 파일은 이를 각 `/ViewText/SectionN` 스트림의 첫 레코드로 둔다.
> `/DocInfo`에는 두지 않는다. §1의 "ViewText 배포 레코드" 항목 참고.

### 3.1 ID_MAPPINGS 카운트 배열과 언어 슬롯 배정

ID_MAPPINGS(0x11) 페이로드는 **u32 카운트 배열**이고, 실제 테이블 항목은 자식 레코드로 나열된다.
카운트 순서(스펙): `[binData, 글꼴×7(언어별), 테두리채움, 글자모양, 탭, 번호, 글머리표, 문단모양,
스타일, (메모모양, 변경추적, 변경추적사용자…)]`. 인덱스 1..8이 언어별 글꼴 수다
(`font_counts[0..7]`, `doc_info.rs:42`).

**FACE_NAME 언어 슬롯 배정**(`doc_info.rs:79`): 글꼴 레코드 자체엔 언어 표시가 없다. `font_cursor`를
두고 현재 슬롯의 채워진 글꼴 수가 `font_counts[cursor]`에 도달하면 다음 언어 슬롯(한글/영어/한자/
일어/외국어/기호/사용자)으로 넘긴다 — **카운트로 역산**한다.

★ NUMBERING의 렌더 템플릿 파싱(`parse_numbering_levels`, `doc_info.rs:401`)은 각 수준의 글자모양
참조가 `0xFFFFFFFF`(정품 "없음")인지로 구조 정합을 검증하고, 아니면 그 수준부터 기본값 폴백한다.
스펙에 기대지 않고 정품 바이트로 역설계한 렌더 전용 경로다.

---

## 4. 본문(BodyText) 레코드 카탈로그 (표 B)

`/BodyText/SectionN` 스트림에 오는 레코드다. 섹션 루트는 **PARA_HEADER 트리들의 나열**이며,
`parse_section`(`body_text.rs:23`)이 PARA_HEADER 아닌 루트를 경고+Opaque 보존한다. 29개 태그 상수가
여기 온다(표 A로 옮긴 MEMO_SHAPE·FORBIDDEN_CHAR·TRACK_CHANGE·TRACK_CHANGE_AUTHOR 제외).

| 태그 ID(hex) | 이름(HWPTAG_*) | 값(BEGIN+n) | 스펙 § | 페이로드 요약 | hwp-cli 상태 | 근거 코드 |
|---|---|---|---|---|---|---|
| 0x42 | PARA_HEADER | +50 | 4.3.1 | 22B prefix(nchars u32·ctrl_mask·문단/글자모양·break·카운트×3·instance) + tail | 부분파싱+raw | `body_text.rs:92` |
| 0x43 | PARA_TEXT | +51 | 4.3.2 | WCHAR 배열 → 일반문자·서로게이트·컨트롤 분해(§5) | 의미파싱 | `body_text.rs:114` |
| 0x44 | PARA_CHAR_SHAPE | +52 | 4.3.3 | (pos u32, charShapeId u32) 반복 8B | 의미파싱 | `body_text.rs:60` |
| 0x45 | PARA_LINE_SEG | +53 | 4.3.4 | 36B/줄(text_start·v_pos·높이×2·baseline·간격·col·폭·flags) | 의미파싱 | `body_text.rs:196` |
| 0x46 | PARA_RANGE_TAG | +54 | 4.3.5 | 영역 태그(변경추적 등) — 미해석 | Opaque | `body_text.rs:73` |
| 0x47 | CTRL_HEADER | +55 | 4.3.6 | 역순 ctrl_id 4B + 나머지 페이로드 → ctrl_id별 분기(표 D) | 부분파싱+raw | `body_text.rs:253` |
| 0x48 | LIST_HEADER | +56 | 4.3.7 | 표 셀=34B prefix(문단수·속성·행/열/span·크기·여백·테두리)+tail; 일반=header_data raw | 부분파싱+raw | `body_text.rs:452`, `:585` |
| 0x49 | PAGE_DEF | +57 | 4.3.10.1.1 | 40B(용지 W/H·여백 6종·제본여백·attr) | 의미파싱 | `body_text.rs:364` |
| 0x4A | FOOTNOTE_SHAPE | +58 | 4.3.10.1.2 | 각주/미주 모양 — secd 자식, 미해석 | Opaque | `body_text.rs:357` |
| 0x4B | PAGE_BORDER_FILL | +59 | 4.3.10.1.3 | 쪽 테두리/배경 — secd 자식, 미해석 | Opaque | `body_text.rs:357` |
| 0x4C | SHAPE_COMPONENT | +60 | 4.3.9.2.1 | 개체 요소(CHID·변환행렬·테두리/채움) — 렌더가 파싱, IR은 raw | raw보존 | `body_text.rs:608`; 렌더 `shape_draw.rs` |
| 0x4D | TABLE | +61 | 4.3.9.1 | attr·행·열·간격·안여백×4 + ★행별 셀 개수 u16×rows + 테두리ID + tail | 부분파싱+raw | `body_text.rs:436` |
| 0x4E | SHAPE_COMPONENT_LINE | +62 | 4.3.9.2.2 | 선: 시작·끝점 i32 — 렌더가 파싱 | raw보존 | `shape_draw.rs`(렌더) |
| 0x4F | SHAPE_COMPONENT_RECTANGLE | +63 | 4.3.9.2.3 | 사각형: 곡률 u8 + 4점 — 렌더가 파싱 | raw보존 | `shape_draw.rs`(렌더) |
| 0x50 | SHAPE_COMPONENT_ELLIPSE | +64 | 4.3.9.2.4 | 타원: attr + 중심·두 축 끝점 — 렌더가 파싱 | raw보존 | `shape_draw.rs`(렌더) |
| 0x51 | SHAPE_COMPONENT_ARC | +65 | 4.3.9.2.6 | 호: arctype + 중심·시작·끝 — 렌더가 파싱 | raw보존 | `shape_draw.rs`(렌더) |
| 0x52 | SHAPE_COMPONENT_POLYGON | +66 | 4.3.9.2.5 | 다각형: 점 수 + 점 배열 — 렌더가 파싱 | raw보존 | `shape_draw.rs`(렌더) |
| 0x53 | SHAPE_COMPONENT_CURVE | +67 | 4.3.9.2.7 | 곡선: 점 수 + 점 배열(폴리라인 근사) — 렌더가 파싱 | raw보존 | `shape_draw.rs`(렌더) |
| 0x54 | SHAPE_COMPONENT_OLE | +68 | 4.3.9.5 | OLE 개체 — 미해석(렌더 미지원) | Opaque | `body_text.rs:617` |
| 0x55 | SHAPE_COMPONENT_PICTURE | +69 | 4.3.9.4 | 그림: 오프셋 71의 u16 = BinItem ID만 추출, 나머지 raw 보존 | 부분파싱+raw | `body_text.rs:318` |
| 0x56 | SHAPE_COMPONENT_CONTAINER | +70 | 4.3.9.7 | 묶음 개체 — 렌더가 자식 재귀, IR은 raw | raw보존 | `shape_draw.rs`(렌더) |
| 0x57 | CTRL_DATA | +71 | 4.3.8 | 컨트롤 임의 데이터(필드 이름 Parameter Set 등) — 온디맨드 BSTR만 읽음 | raw보존 | `field.rs:189` |
| 0x58 | EQEDIT | +72 | 4.3.9.3 | 수식: attr(4)+len(2)+WCHAR[len] 스크립트를 렌더용 파싱, raw 보존 | 부분파싱+raw | `body_text.rs:536` |
| 0x5A | SHAPE_COMPONENT_TEXTART | +74 | 4.3.9(글맵시) | 글맵시 — 미해석 | Opaque | `body_text.rs:617` |
| 0x5B | FORM_OBJECT | +75 | 4.3.9(양식) | 양식 개체 — 미해석 | Opaque | `body_text.rs:617` |
| 0x5D | MEMO_LIST | +77 | 4.3(메모) | 메모 리스트 — 미해석 | Opaque | `body_text.rs:617` |
| 0x5F | CHART_DATA | +79 | 4.3.9.6 | 차트 개체 — 미해석 | Opaque | `body_text.rs:617` |
| 0x62 | VIDEO_DATA | +82 | 4.3.9.8 | 동영상 개체 — 미해석 | Opaque | `body_text.rs:617` |
| 0x73 | SHAPE_COMPONENT_UNKNOWN | +99 | *(없음)* | 미지 개체 — 미해석 | Opaque | `body_text.rs:617` |

### 4.1 개체 서브트리 패턴 (CTRL_HEADER → SHAPE_COMPONENT → LIST_HEADER)

확장 컨트롤 하나는 여러 레코드의 논리적 묶음이다. 대표 패턴:

```
CTRL_HEADER(gso )                         ← 그리기 개체 진입, 역순 ctrl_id
└── SHAPE_COMPONENT                        ← 개체 요소(변환행렬·스타일)
    ├── SHAPE_COMPONENT_PICTURE            ← 그림이면 BinItem ID (오프셋 71)
    └── LIST_HEADER                        ← 글상자면 문단 리스트 진입
        └── PARA_HEADER …                  ← 개체 안 문단(재귀)

CTRL_HEADER(tbl )                          ← 표
├── TABLE                                  ← 행/열/★행별 셀 개수
├── LIST_HEADER                            ← 셀 1을 염
│   └── PARA_HEADER …                      ← 셀 1 문단
└── LIST_HEADER …                          ← 셀 2 … (형제로 나열, 다음 LIST_HEADER가 셀 경계)

CTRL_HEADER(secd)                          ← 구역 정의
├── PAGE_DEF                               ← 용지(의미파싱)
├── FOOTNOTE_SHAPE / PAGE_BORDER_FILL      ← Opaque
```

리더는 표 셀/글상자 안 문단을 `collect_paragraph_lists`(`body_text.rs:578`)로 재귀 수집한다.
GenericControl은 이 평탄화된 `paragraph_lists`를 텍스트 추출에만 쓰고, **무손실 재직렬화는 원본
중첩 서브트리 `raw_children`**로 한다(평탄화 손실 방지, `control.rs:202`).

**★ 코드 실측이 스펙과 다른 지점:**
- **TABLE "Row Size" 배열**: 스펙 문언과 달리 실측상 **행 높이가 아니라 행별 셀 개수**다
  (`row_cell_counts`, `body_text.rs:443`, `control.rs:161`).
- **셀 LIST_HEADER 34B**: 의미 파싱하는 prefix 길이(`parse_cell_header` 필드 합 4+4+2+2+2+2+4+4+8+2,
  나머지는 `header_tail` raw). 구판의 "46B" 표기는 표 69 개체 공통 속성 길이(46B)와의 혼동 —
  2026-07-19 스펙 전수 감사에서 정정.
- **그리기 개체 테두리 선 정보 13B**: 스펙 표 86은 선 굵기 INT16 포함 11B를 선언하나 실측은
  굵기 4B(INT32)로 총 13B(`shape_draw.rs:393,414-415` — 2026-07-19 감사에서 ★ 목록 보완).
- **CHAR_SHAPE 취소선 비트(18~20)**: DIFFSPEC 의미라 신뢰하지 않고 `strike:false` 고정
  (가짜 취소선 방지, `doc_info.rs:249`).

**구현 주의(정오 아님):** ctrl_id는 `MAKE_4CHID`가 만든 UINT32이며 파일 전체의 little-endian
규칙을 따른다. 따라서 논리 ID `secd`가 바이트열에서는 `dces`로 나타난다. 이를 4문자 바이트로
표현하는 리더는 경계에서 뒤집지만(`body_text.rs:268`), UINT32로 유지하는 리더는 다시 뒤집으면 안 된다.

---

## 5. 컨트롤 문자(0~31) 분류표 (표 C)

PARA_TEXT의 WCHAR 배열을 소비할 때 **코드 0~31의 분류가 위치 산수의 기준**이다. 8 WCHAR 컨트롤을
하나라도 잘못 세면 이후 모든 위치 계산(`nchars == Σ wchar_width`)이 어긋난다. 분류의 단일 진실은
`char_kind`(`paragraph.rs:27`)이며, 이름은 `ctrl_char` 모듈(`paragraph.rs:41`), 텍스트 추출 처리는
`text.rs:44`가 담당한다. 스펙 근거는 **§3.2.3 본문의 표 6(제어 문자)**이다(코드 주석의 §4.2.4,
이 문서 구판의 §4.3.2 표기는 모두 오기 — Phase 2 감사 C7·C8에서 확정, 2026-07-18).

| 코드 | 분류 | WCHAR 폭 | 잘 알려진 의미 | 텍스트 추출 |
|---|---|---|---|---|
| 0 | Char | 1 | 사용 안 함/구분 | 버림 |
| 1 | Extended | 8 | 예약 | 컨트롤 디스패치 |
| 2 | Extended | 8 | 구역/단 정의 (secd/cold) | SectionDef/ColumnDef |
| 3 | Extended | 8 | 필드 시작 (%clk 등) | 필드 컨트롤 |
| 4 | Inline | 8 | 필드 끝 | 버림(필드 값 경계) |
| 5 | Inline | 8 | 예약 | 버림 |
| 6 | Inline | 8 | 예약 | 버림 |
| 7 | Inline | 8 | 예약 | 버림 |
| 8 | Inline | 8 | title mark(제목 표시) | 버림 |
| 9 | Inline | 8 | 탭 | `\t` |
| 10 | Char | 1 | 줄 나눔 | `\n` |
| 11 | Extended | 8 | 그리기 개체/표 (gso/tbl) | Table/Picture/Generic |
| 12 | Extended | 8 | 예약 | 컨트롤 디스패치 |
| 13 | Char | 1 | 문단 끝 | 문단 경계 개행 |
| 14 | Extended | 8 | 예약 | 컨트롤 디스패치 |
| 15 | Extended | 8 | 숨은 설명 | 기본 제외(`include_hidden`) |
| 16 | Extended | 8 | 머리말/꼬리말 (head/foot) | 기본 제외(`include_header_footer`) |
| 17 | Extended | 8 | 각주/미주 (fn/en) | 포함 |
| 18 | Extended | 8 | 자동 번호 (atno) | 포함 |
| 19 | Inline | 8 | 예약 | 버림 |
| 20 | Inline | 8 | 예약 | 버림 |
| 21 | Extended | 8 | 쪽 컨트롤 (pgnp/pghd/nwno) | 포함 |
| 22 | Extended | 8 | 책갈피/찾아보기 표식 (bokm) | 포함 |
| 23 | Extended | 8 | 덧말/글자 겹침 | 포함 |
| 24 | Char | 1 | 하이픈 | `-` |
| 25 | Char | 1 | 예약 | 버림 |
| 26 | Char | 1 | 예약 | 버림 |
| 27 | Char | 1 | 예약 | 버림 |
| 28 | Char | 1 | 예약 | 버림 |
| 29 | Char | 1 | 예약 | 버림 |
| 30 | Char | 1 | 묶음 빈칸 | ` ` |
| 31 | Char | 1 | 고정폭 빈칸 | ` ` |

분류별 폭: **Char = 1 WCHAR**, **Inline = 8 WCHAR**(`[코드, 정보 6 WCHAR, 코드]` 자체 완결),
**Extended = 8 WCHAR**(별도 CTRL_HEADER를 가리킴, payload 앞 4B = 역순 ctrl_id).
`ctrl_mask`(PARA_HEADER 오프셋 4)는 힌트일 뿐 — 리더는 실제 PARA_TEXT를 순회해 컨트롤을 센다.

---

## 6. 확장 컨트롤 ctrl ID 카탈로그 (표 D)

CTRL_HEADER(코드 §5의 Extended)가 가리키는 확장 컨트롤을 **정방향 ctrl_id**로 조회한다. hwp5 리더는
`parse_control`(`body_text.rs:253`)에서 `secd`/`tbl `/`gso `(그림)만 의미 파싱하고 나머지는
GenericControl로 문단 리스트를 수집한다. hwpx는 `section.rs`가 OWPML 요소를 같은 ctrl_id/코드로 매핑
한다(양 포맷이 IR에서 동일 의미).

### 6.1 구조·개체·구역 컨트롤

| ctrl ID | 이름 | 스펙 § | hwp-cli 상태 | hwpx 대응 요소 | 근거 코드 |
|---|---|---|---|---|---|
| `secd` | 구역 정의 | 4.3.10.1 | 부분파싱+raw(PAGE_DEF 의미파싱, 나머지 raw/Opaque) | `hp:secPr` | `body_text.rs:338`, `section.rs:136` |
| `cold` | 단 정의 | 4.3.10.2 | 부분파싱(ColumnDef, 렌더용) | `hp:ctrl > hp:colPr` | `body_text.rs:555`, `section.rs:377` |
| `tbl ` | 표 | 4.3.9.1 | 의미파싱(TABLE + 셀) | `hp:tbl` | `body_text.rs:380`, `section.rs:691` |
| `gso ` | 그리기 개체(공통) | 4.3.9 | 그림=부분파싱(Picture), 그 외=raw(렌더) | `hp:pic` / `hp:rect`·`ellipse`·`line`·`arc`·`polygon`·`curve` | `body_text.rs:309`, `section.rs:178`·`995` |
| `eqed` | 수식 | 4.3.9.3 | 부분파싱(스크립트, 렌더 조판) | `hp:equation` | `body_text.rs:517`, `section.rs:1130` |
| `head` | 머리말 | 4.3.10.3 | Generic(문단 수집, 페이로드 8B) | `hp:ctrl > hp:header` | `section.rs:399`·`588` |
| `foot` | 꼬리말 | 4.3.10.3 | Generic(문단 수집, 페이로드 8B) | `hp:ctrl > hp:footer` | `section.rs:399`·`588` |
| `fn  ` | 각주 | 4.3.10.4 | Generic(문단 수집) | `hp:ctrl > hp:footNote` | `section.rs:589` |
| `en  ` | 미주 | 4.3.10.4 | Generic(문단 수집) | `hp:ctrl > hp:endNote` | `section.rs:590` |
| `atno` | 자동 번호 | 4.3.10.5 | Generic(페이로드 12B 합성) | `hp:ctrl > hp:autoNum` | `section.rs:465`·`593` |
| `nwno` | 새 번호 지정 | 4.3.10.6 | Generic(페이로드 6B 합성) | `hp:ctrl > hp:newNum` | `section.rs:475`·`596` |
| `pghd` | 감추기 | 4.3.10.7 | Generic(페이로드 4B 비트맵 합성) | `hp:ctrl > hp:pageHiding` | `section.rs:446`·`595` |
| `pgnp` | 쪽 번호 위치 | 4.3.10.9 | Generic(페이로드 12B 합성) | `hp:ctrl > hp:pageNum` | `section.rs:415`·`594` |
| `bokm` | 책갈피 | 4.3.10.11 | Generic(이름 CTRL_DATA) | `hp:ctrl > hp:bookmark` | `section.rs:562` |

*홀/짝수 조정(§4.3.10.8)·찾아보기 표식(§4.3.10.10)·글자 겹침(§4.3.10.12)·덧말(§4.3.10.13)은 별도
의미 파싱 없이 Generic으로 통과 — 미지 ctrl_id는 `section.rs:597`이 요소 이름 앞 4바이트를 ctrl_id로
써서 코드 21로 방출한다.*

### 6.2 필드 컨트롤 (필드 시작, §4.3.10.15)

필드 = `FIELD_START`(문자 코드 3, 확장) … 표시 텍스트 … `FIELD_END`(코드 4). 종류는 ctrl_id로
구분하며, hwpx `fieldBegin type` 속성과 왕복 매핑된다. 12종 전수(`field.rs:37`·`56`·`91`):

| ctrl ID | 이름 | OWPML `fieldBegin type` | 근거 코드 |
|---|---|---|---|
| `%clk` | 누름틀 | `CLICK_HERE` | `field.rs:57`·`92` |
| `%fmu` | 계산식 | `FORMULA` | `field.rs:58`·`93` |
| `%hlk` | 하이퍼링크 | `HYPERLINK` | `field.rs:59`·`94` |
| `%mmg` | 메일머지 | `MAIL_MERGE` | `field.rs:60`·`95` |
| `%dte` | 날짜 | `DATE` | `field.rs:61`·`96` |
| `%ddt` | 문서날짜 | `DOCUMENT_DATE` | `field.rs:62`·`97` |
| `%xrf` | 상호참조 | `CROSS_REF` | `field.rs:63`·`98` |
| `%bmk` | 책갈피(필드) | `BOOKMARK` | `field.rs:64`·`99` |
| `%pat` | 파일경로 | `PATH` | `field.rs:65`·`100` |
| `%smr` | 문서요약 | `SUMMARY` | `field.rs:66`·`101` |
| `%usr` | 사용자정보 | `USER_INFO` | `field.rs:67`·`102` |
| `%unk` | 알수없음 | `UNKNOWN` | `field.rs:68`·`103` |

**hwp-cli 상태**: 전 종류 온디맨드 파싱(이름·명령·값 읽기, IR 불변; `field.rs:110`). 생성·편집은
`%clk`(누름틀)·`%hlk`(하이퍼링크)·`%bmk`/`bokm`(책갈피)를 지원한다. hwpx는 `hp:fieldBegin`(자식
`hp:parameters > stringParam name="Command"`) + `hp:fieldEnd`로 읽는다(`section.rs:516`). ★
정품 실측: `%hlk`는 커맨드 레코드 id가 **비영**이어야 한글이 하이퍼링크로 인식하고, FIELD_END
payload에 역순 ctrl_id 3B(`%` 제외)를 담아야 짝이 맺힌다(`field.rs:423`·`476`).

---

## 7. 스펙 § ↔ 코드 인덱스

주요 주제별 교차 참조. "담당 코드"는 해당 주제의 진입 함수다.

| 스펙 § | 주제 | 담당 코드(파일:함수) | 이 문서 |
|---|---|---|---|
| 3.1~3.2 | 파일/스토리지 구조 | `container.rs:list_streams`·`body_sections`·`is_record_stream` | §1 |
| 3.2.1 / 4.1 | 파일 인식 정보 / 레코드 구조 | `file_header.rs:parse`, `record/header.rs` | §1, §2 |
| 4.1 | 레코드 헤더 비트 패킹 | `record/header.rs`, `record/scan.rs`, `record/tree.rs` | §2, [02]§5·§6 |
| 4.2.1 | 문서 속성 | `doc_info.rs:parse_document_properties` | 표 A |
| 4.2.2 | 아이디 매핑 헤더 | `doc_info.rs:parse_doc_info`(ID_MAPPINGS 분기) | 표 A §3.1 |
| 4.2.3~4.2.11 | DocInfo 테이블 항목 | `doc_info.rs:parse_id_mapping_child` | 표 A |
| 4.2.6 | 글자 모양(★취소선 비트) | `doc_info.rs:parse_char_shape` | 표 A |
| 4.3.1~4.3.4 | 문단 헤더·텍스트·글자모양·레이아웃 | `body_text.rs:parse_paragraph` | 표 B |
| 4.3.2 | 컨트롤 문자 분류 | `paragraph.rs:char_kind`, `text.rs:extract_into` | §5 |
| 4.3.6 | 컨트롤 헤더(역순 ctrl_id) | `body_text.rs:parse_control` | §4.1, §6 |
| 4.3.9.1 | 표 개체(★행별 셀 개수) | `body_text.rs:parse_table` | 표 B, §4.1 |
| 4.3.9.2.* | 그리기 개체 기하 | `hwp-render/src/shape_draw.rs` | 표 B, [02]§10 |
| 4.3.9.3 | 수식 개체 | `body_text.rs:parse_eqed`·`find_eqedit_script` | 표 B, §6.1 |
| 4.3.9.4 | 그림 개체(BinItem ID) | `body_text.rs:parse_picture_gso` | 표 B |
| 4.3.10.1.1 | 용지 설정 | `body_text.rs:parse_page_def` | 표 B |
| 4.3.10.2~4.3.10.11 | 개체 외 컨트롤(단/머리말/각주/쪽/책갈피) | `section.rs`(hwpx 합성), `body_text.rs:parse_generic` | §6.1 |
| 4.3.10.15 | 필드 | `hwp-convert/src/field.rs`, `section.rs:parse_ctrl` | §6.2 |

---

## 8. 완성도 요약 (12번 갭 문서 입력)

**표 A/B에서 `Opaque`인 행** (해석 소비단 없음, 왕복 보존만):

- DocInfo: `DOC_DATA`, `COMPATIBLE_DOCUMENT`, `LAYOUT_COMPATIBILITY`,
  `TRACKCHANGE`, `MEMO_SHAPE`, `FORBIDDEN_CHAR`, `TRACK_CHANGE`, `TRACK_CHANGE_AUTHOR`
- 본문: `PARA_RANGE_TAG`, `FOOTNOTE_SHAPE`, `PAGE_BORDER_FILL`, `SHAPE_COMPONENT_OLE`,
  `SHAPE_COMPONENT_TEXTART`, `FORM_OBJECT`, `MEMO_LIST`, `CHART_DATA`, `VIDEO_DATA`,
  `SHAPE_COMPONENT_UNKNOWN`

**`raw보존`인 행** (해석 없이 보존하되 소비단/재방출 슬롯 있음):

- DocInfo: `TAB_DEF`, `BULLET`
- 본문: `SHAPE_COMPONENT`, `SHAPE_COMPONENT_LINE`·`RECTANGLE`·`ELLIPSE`·`ARC`·`POLYGON`·`CURVE`,
  `SHAPE_COMPONENT_CONTAINER`(이상 렌더가 기하 파싱), `CTRL_DATA`

**`상수 미정의`**: `RESERVED`(0x1D / BEGIN+13) — `tag.rs`에 상수 없음, Opaque 경유 보존.

이들이 12번 문서(기능 격차)의 분석 대상이다. `Opaque`는 완전 미해석(그리나 무손실 왕복 보장),
`raw보존`은 렌더 소비단이 부분 해석하지만 IR에 의미 필드로는 올라오지 않은 것이다.
