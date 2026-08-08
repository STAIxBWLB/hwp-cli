[한국어](19-hwp5-spec-supplement.ko.md) · [English](19-hwp5-spec-supplement.md)

# HWP 5.0 명세 보완서 (Spec Supplement)

공개 스펙 「한글문서파일형식 5.0」이 담지 못한 네 가지, 즉 **정오표·버전-레이아웃 매트릭스·적합성 규칙·소비 의미론**을 한 곳에 모은 규범 색인이다. 목표는 이 문서군만으로 구현 언어와 무관하게, 한글(한컴오피스)이 수용하는 HWP 5.0 파일을 읽고 쓸 수 있게 하는 것이다. 각 항목은 사실을 한 줄로 확정하고, 발견 경위와 바이트 상세는 정본 문서로 링크한다. 여기 실린 모든 사실은 정품 한글 저장 파일과 실기(한글에서 열기) 게이트로 확정한 실측이다([00 §4](00-overview.ko.md) 정답지 방법론). 스펙 원문은 전재하지 않고 §·표 번호로만 인용한다([docs/README](../README.md) 저작권 정책).

## 다른 문서와의 역할 분담

| 문서 | 다루는 것 | 이 문서와의 관계 |
|---|---|---|
| [07-hangul-compat-rules](07-hangul-compat-rules.ko.md) | 실기 확정 규칙의 발견 서사(증상→원인→수정→정답지) | §3 적합성 규칙의 근거 데이터. 규칙 ID(A1…E6)를 그대로 인용 |
| [03-hwp5-write](03-hwp5-write.ko.md) | writer 구현 절차(레코드 조립·버전 게이트의 코드 앵커) | §2·§3의 절차적 상세 |
| [05-rendering](05-rendering.ko.md) | 렌더 산식(줄 배치 합성·표 높이·단위 환산) | §4 소비 의미론의 산식 정본 |
| [10-hwp5-structure-map](10-hwp5-structure-map.ko.md) | 레코드 전수 카탈로그(★ = 스펙-실측 불일치 표식) | §1 정오표의 원 위치 |
| [08-external-research](08-external-research.ko.md) | 외부 근거(표준·오픈소스·이슈 트래커) | §1의 외부 실증 항목 |

**유지보수 규약**: 새 사실은 실기·정답지로 확정한 뒤 07(진단 서사)·10(구조 지도)에 먼저 등재하고, 이 문서에는 해당 표에 한 줄을 추가한다. 이 문서는 사실의 첫 등재처가 아니라 색인이다.

**표기 주의**: ★ 기호는 문서마다 뜻이 다르다. 10에서는 스펙-실측 불일치를, 08에서는 중요 결론을 표시한다. 이 문서는 ★ 없이 표의 행으로만 말한다.

---

## 1. 정오표 (Errata) — 스펙 문언과 실측이 다른 지점

스펙 문언을 그대로 구현하면 틀리는 지점의 전수 목록이다. "실측 확정" 열이 규범이다.

| # | 대상 | 스펙 문언 요지 | 실측 확정 | 검증 근거 | 상세 |
|---|---|---|---|---|---|
| E-1 | TABLE 레코드 "Row Size" 배열 | 행 높이 | **행별 셀 개수** | 정품 표 문서 전수 대조 | [10 §4.1](10-hwp5-structure-map.ko.md) |
| E-2 | 그리기 개체 테두리 선 정보(표 86) | 총 11B, 선 굵기 INT16 | 총 **13B**, 굵기 **INT32** | 2026-07-19 스펙 전수 감사 | [10 §4.1](10-hwp5-structure-map.ko.md) |
| E-3 | BULLET 레코드(표 42) | 20B, 글머리표 문자 @8 | **25B** — @8~12에 번호 글자모양 id 4B가 먼저 오고 문자는 **@12**. 표 42는 총길이가 자기모순인 이력이 있는 오기 | 정품 BULLET 5개 전수 바이트 대조 + 실기(마커 미표시 재현) | [07 B7](07-hangul-compat-rules.ko.md) |
| E-4 | PAGE_BORDER_FILL(표 135) | 자기모순: 선언 12B ≠ 필드 합 14B | **14B** — 속성 u32 + 4방향 gap u16×4 + 테두리ID u16. BOTH/EVEN/ODD는 레코드 순서로 구분 | 정품 236파일·714레코드 전수 스윕(2026-07-19) | [08 기능별 근거](08-external-research.ko.md) |
| E-5 | COLDEF 단 정의(표 138·139) | 14B | 실파일 **16B**(외부 실증. 자체 전수 실측은 미실시 — 구현 시 정답지로 확정할 것) | hwp.js 이슈 #58 | [08 생태계](08-external-research.ko.md) |
| E-6 | CHAR_SHAPE attr 취소선 비트 18~20(§4.2.7 표 35) | 취소선 여부·모양 | 야생 파일에서 **읽기 신뢰 불가** — 변경추적 삭제표시 템플릿이 bit18을 오염시켜 가짜 취소선을 만든다(실측 한 파일의 92%). 쓰기는 bit18 단독으로 렌더됨(실기 확정) | 코퍼스 attr 전수 실측 + 실기 | [07 B8](07-hangul-compat-rules.ko.md) |
| E-7 | PARA_HEADER nchars bit31 | 줄 배치 캐시 정합 표식 | 실제 소비는 **'리스트(구역·표 셀·글상자)의 마지막 문단' 표식**(이중 의미). 잘못 켜면 이후 문단이 통째로 미표시 | 정품 다문단 표본 bit31 분포 실측 + 실기(revert 이력 포함) | [07 B3·B4](07-hangul-compat-rules.ko.md) |
| E-8 | CTRL_HEADER/ExtCtrl ctrl_id | 4문자 코드(예: `secd`) | payload에 **바이트 역순 저장**(`dces` → `secd`) — 읽을 때 뒤집고, 쓸 때 역순으로 기록한다. 같은 역순이 FIELD_END payload의 역순 ctrl_id 3B(§3.4)로도 나타난다 | 정품 파일 바이트 대조 | [10 §4.1](10-hwp5-structure-map.ko.md) |

### 1.1 스펙이 정본인 혼동 지점 (정오 아님)

다음은 스펙이 옳고 구현·구판 문서가 틀렸던 지점이다. 정오표와 반대 방향이므로 구분해 둔다.

- 레코드의 DocInfo/BodyText 소속은 **스펙 표 13이 정본**이다. MEMO_SHAPE(+76)·FORBIDDEN_CHAR(+78)·TRACK_CHANGE(+80)·TRACK_CHANGE_AUTHOR(+81)는 태그 값이 본문 수치 대역이지만 의미상 DocInfo 레코드다([10 §3](10-hwp5-structure-map.ko.md) 경고 참조).
- 제어 문자(0~31) 분류의 스펙 근거는 **§3.2.3 본문의 표 6**이다. 과거 코드 주석의 §4.2.4와 문서 구판의 §4.3.2 표기는 모두 오기였다(2026-07-18 정정, [10 §5](10-hwp5-structure-map.ko.md)).
- 셀 LIST_HEADER의 의미 파싱 prefix는 **34B**다. 구판 문서의 "46B"는 표 69(개체 공통 속성 46B)와의 혼동이었다([10 §4.1](10-hwp5-structure-map.ko.md)).

---

## 2. 버전-레이아웃 매트릭스

한글은 FileHeader의 선언 버전(DWORD `0xMMnnPPrr`, [03 §2](03-hwp5-write.ko.md))과 레코드 레이아웃의 정합을 검사하고, 어긋나면 손상/변조로 거부한다. 이 검사는 양방향이다. 즉 신버전 선언에 구형 레이아웃을 쓰면 거부되고([07 A3](07-hangul-compat-rules.ko.md)), 구버전 선언에 신형 패딩을 쓰면 그것도 거부된다(ID_MAPPINGS를 무조건 18개로 패딩해 5.0.2.x 문서가 손상 판정된 실증 — [07 A10](07-hangul-compat-rules.ko.md)). 스펙은 이 변천을 한 곳에 정리하지 않으므로 아래 표가 이를 한자리에 색인한다. 절차적 상세의 정본은 02(읽기)·03(쓰기)이다.

| 경계(선언 버전 ≥) | 레코드 | 변화 | 근거 |
|---|---|---|---|
| 5.0.1.0 | TABLE | tail에 영역 속성 크기 u16 추가 | [03 §4](03-hwp5-write.ko.md) |
| 5.0.2.1 | CHAR_SHAPE | tail[0..2]에 border_fill_id u16 추가 | [02](02-hwp5-read.ko.md), [03 §4](03-hwp5-write.ko.md) |
| 5.0.2.1 | ID_MAPPINGS | 카운트 15 → 16 (메모 모양 추가) | [03 §4](03-hwp5-write.ko.md) |
| 5.0.2.5 | PARA_SHAPE | tail 12B째의 line_spacing i32(신형 줄간격) 유효 | [02](02-hwp5-read.ko.md) |
| 5.0.3.2 | PARA_HEADER | 22B → **24B** (변경추적 병합 문단 여부 u16, 표 58) | [03 §4](03-hwp5-write.ko.md), [07 A3](07-hangul-compat-rules.ko.md) |
| 5.0.3.2 | ID_MAPPINGS | 카운트 16 → **18** (변경추적·변경추적 사용자) | [03 §4](03-hwp5-write.ko.md), [07 A10](07-hangul-compat-rules.ko.md) |
| 5.1.0.1 | PARA_SHAPE | 54B → **58B** (후행 4B=0). 누락 시 무결성 위반 경고 | [03 §4](03-hwp5-write.ko.md), [07 A3](07-hangul-compat-rules.ko.md) |
| 5.1.0.1 | CHAR_SHAPE | 합성 규격 **74B** (border_fill_id u16 + 취소선색 u32 tail 포함) | [03 §4](03-hwp5-write.ko.md) |
| 5.1.x | DocInfo 루트 | **COMPATIBLE_DOCUMENT(0x1E) 서브트리 필수** — 자식 LAYOUT_COMPATIBILITY(0x1F, 20B=0) + TRACKCHANGE(0x20, 1032B, data[0]=0x38). 누락 시 거부. 구버전(5.0.2.x)은 면제 | [03 §5](03-hwp5-write.ko.md), [07 A4](07-hangul-compat-rules.ko.md) |

읽기 쪽은 같은 경계를 tail 길이로 추론하고([02](02-hwp5-read.ko.md)), 쓰기 쪽은 선언 버전으로 분기한다([03 §4](03-hwp5-write.ko.md)의 version_target 유도). 두 방향이 같은 경계를 공유하며, 이 표는 그 경계를 한자리에 모아 둔 목록이다.

---

## 3. 적합성 체크리스트 — 한글이 수용하는 파일의 조건

공개 스펙에 없는 "적합성(conformance) 장"의 대체물이다. 각 규칙은 위반 시 한글이 거부하거나 오표시하는 필수 조건(MUST)이다. 근거 열의 A·B·C·E는 [07](07-hangul-compat-rules.ko.md)의 규칙 ID다. 판정은 세 계층으로 나뉜다. ① 파일이 열리는가(손상/변조 팝업 없음) ② 내용이 정상 렌더되는가 ③ 기능이 동작하는가(하이퍼링크 클릭 등). 주의할 점은, pyhwp 같은 관대한 파서의 통과가 어느 계층도 보증하지 않는다는 것이다(07 종합 교훈 1).

### 3.1 컨테이너·FileHeader (계층 ①)

| 규칙 | 위반 시 한글 동작 | 근거 |
|---|---|---|
| CFB 컨테이너는 V3(512B 섹터)여야 한다 | "손상된 파일" 즉시 거부 | A1, [03 §1](03-hwp5-write.ko.md) |
| FileHeader EncryptVersion=4 (비암호 문서 포함) | "변조 가능성" 보안 경고 | A2, [03 §2](03-hwp5-write.ko.md) |
| 레코드 스트림(/DocInfo·/BodyText·/Scripts)은 zlib 헤더 없는 raw deflate로 압축하고 속성 플래그 bit0과 정합시킨다 | 열기 실패 | [03 §1](03-hwp5-write.ko.md) |
| 보조 스트림(DocOptions/_LinkDoc, Scripts 2종, HwpSummaryInformation)을 동봉한다 | 손상 판정에 관여(A1 수정의 일부) | A1, [03 §1·§9](03-hwp5-write.ko.md) |

### 3.2 DocInfo (계층 ①·②)

| 규칙 | 위반 시 한글 동작 | 근거 |
|---|---|---|
| 선언 버전과 레코드 길이가 §2 매트릭스대로 정합해야 한다 | 손상/변조 거부 | A3·A10 |
| 5.1.x 선언이면 COMPATIBLE_DOCUMENT 서브트리 필수 | 손상 거부(보안 수준을 낮춰도) | A4 |
| DOCUMENT_PROPERTIES 시작번호 6종(쪽·각주·미주·그림·표·수식)은 1 이상 | 비정상 판정 | A8 |
| ID_MAPPINGS 카운트 배열과 실제 자식 레코드 수가 일치해야 한다 | 손상 판정 | A10, [03 §4](03-hwp5-write.ko.md) |
| CHAR_SHAPE shade_color는 0 금지 — '없음'은 0xFFFFFFFF (COLORREF의 '없음' 표식은 문맥마다 다르다) | 글자 칸마다 불투명 검정 음영("검은 바") | B1, [05 §7](05-rendering.ko.md) |
| PARA_SHAPE의 tab_def_id·numbering_id는 실존 항목을 가리켜야 한다(dangling 금지) | 손상 판정 | A10 |
| SECTION_DEF는 필수 자식(FOOTNOTE_SHAPE×2, PAGE_BORDER_FILL×3)을 갖는다 | 손상 판정 | A10, [03 §10](03-hwp5-write.ko.md) |

### 3.3 BodyText 문단 (계층 ①·②)

| 규칙 | 위반 시 한글 동작 | 근거 |
|---|---|---|
| 문단마다 PARA_CHAR_SHAPE run이 1개 이상 — PARA_HEADER 수 == PARA_CHAR_SHAPE 수 | 손상 판정(관대한 외부 파서는 크래시) | A7 |
| 모든 문단은 문단끝(0x0d)으로 끝난다 | 불변식 연쇄 위반 | [03 §6](03-hwp5-write.ko.md)(a) |
| 빈 문단은 nchars=1로 두고 PARA_TEXT 레코드를 생략한다 — 내용이 문단끝뿐인 PARA_TEXT 금지 | "손상 + 본문 비어 있음" | A5, [03 §6](03-hwp5-write.ko.md)(b) |
| nchars bit31은 각 리스트(구역·표 셀·글상자)의 마지막 문단에만 켠다 | 첫 문단을 마지막으로 오인해 이후 문단 미표시 | B4, E-7 |
| ctrl_mask에는 확장·인라인 컨트롤 비트만 넣는다(문단끝 13, 줄나눔 10 등 문자형 제외) | "선언된 컨트롤이 실제로 없음" 손상 판정 | A10, [03 §6](03-hwp5-write.ko.md)(f) |
| PARA_CHAR_SHAPE에서 연속 동일 id run을 병합한다 | 손상 판정 | [03 §6](03-hwp5-write.ko.md)(d) |
| PARA_HEADER instance_id는 0 금지(문서 내 고유 비영 값) | 비정상 판정 | A8 |
| 구역 첫 문단 break_type에 0x03(구역/단 나눔)을 켠다 | 구역 구조 어긋남 | [03 §6](03-hwp5-write.ko.md)(e) |
| 5.1.x 본문 문단은 PARA_LINE_SEG를 보유해야 한다(합성 문서는 줄 배치를 생성). 단 내용을 수정한 왕복은 캐시를 제거해 재계산을 유도한다 — 부정확한 캐시는 "변조"를 재유발한다 | 0높이 렌더("빈 내용"·검은 바) 또는 변조 경고 | B2·B3 |
| 표의 빈 셀도 문단 1개를 갖는다(LIST_HEADER nparas ≥ 1) | 손상 판정 | A6, C3 |

### 3.4 필드·하이퍼링크 (계층 ③)

하이퍼링크 클릭은 네 조건의 AND다. ① 표시 텍스트에 링크 글자모양(파랑+밑줄) ② 필드 instance id 비영 ③ %hlk 커맨드 attr=0x0000a800 ④ FIELD_END payload에 역순 ctrl_id 3B('%' 제외). 하나라도 빠지면 외관만 링크이고 클릭이 동작하지 않는다(E1·E2·E4·E5, 상세는 [07 E 계층](07-hangul-compat-rules.ko.md)).

**범위 주의**: HWPX 쪽 적합성(mimetype 첫 엔트리·Stored 압축, hp:t 안 raw 제어문자 금지 등)은 [04](04-hwpx-owpml.ko.md)·[07 A11·A12](07-hangul-compat-rules.ko.md)·[11](11-hwpx-structure-map.ko.md)이 다룬다.

---

## 4. 미문서 소비 의미론 — 한글이 값을 소비하는 방식

필드 정의는 스펙에 있으나 소비 방식이 없는 지점의 색인이다. 산식 자체는 정본 문서에 한 번만 둔다.

| 주제 | 요지 | 산식·상세 정본 |
|---|---|---|
| 줄 배치(PARA_LINE_SEG) 합성 | 기본 줄간격은 글자크기의 160%. base·line_advance·baseline_gap(85%) 산식과 표준 flags 0x0006_0000 | [05 §2.3](05-rendering.ko.md), B2 |
| 저장된 lineseg의 지위 | 있으면 1급 입력으로 신뢰하고 재계산하지 않는다. 없으면 합성한다 | [01](01-architecture-ir.ko.md), [05 §2](05-rendering.ko.md) |
| v_pos는 페이지 상대 좌표 | 페이지마다 0으로 리셋해야 한다. 구역 단조 누적은 손상 판정 | B6, [05 §2.2](05-rendering.ko.md) |
| 표 높이 | Σ행 max(rowH) + **566** HWPUNIT(2.0mm). 스펙에 없는 경험 상수로, 두 정답지 교차 실측으로 확정 | C2, [05 §2.4](05-rendering.ko.md) |
| run당 도형 렌더 한계 | 한 run에서 앞쪽 약 21개 도형만 렌더(정확 한계 미상). 구현은 run당 12개로 보수 분할 | D8, [04 §7.2](04-hwpx-owpml.ko.md) |
| 탭 "재계산" 2종 구분 | 렌더 탭 스톱은 `floor(acc/40)×40 + 40`으로 계산한다. 이와 별개로 HWPX `hp:tab`의 width 속성은 한글이 열 때 재계산하므로 근사값이 허용된다(후자는 HWPX 규칙) | [05 §2.3](05-rendering.ko.md) / A12 |
| 다단 lineseg | col_start=0, seg_width=단 폭으로 저장된다. 단 인덱스는 저장돼 있지 않으므로 v_pos 리셋 경계에서 유도해야 한다 | [05 §1.8](05-rendering.ko.md) |
| HWPUNIT 환산 | PARA_SHAPE 여백류만 /200, 그 외 모든 HWPUNIT는 /100 | [05 §7](05-rendering.ko.md) |
