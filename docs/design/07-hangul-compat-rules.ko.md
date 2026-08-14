[한국어](07-hangul-compat-rules.ko.md) · [English](07-hangul-compat-rules.md)

## 개요 — 이 문서가 프로젝트의 가장 값진 자산인 이유

hwp-cli는 pyhwp·자체 리더/렌더로는 100% 통과하지만 **한컴오피스 한글이 실제로 열면 손상·빈 화면·검은 바로 거부/오표시되는** 규칙들을 한 개씩 실기(정품 한글 렌더)와 정답지(사용자가 한글로 직접 만들어 준 `.hwp/.hwpx`) 바이트 대조로 잡아냈다. 이 규칙들은 공개 스펙(HWP 5.0 §)에도 없거나 명시되지 않은 **암묵적 불변식**이라, 정적 분석·스펙 독해·외부 파서 검증으로는 원리적으로 발견 불가능했다. 핵심 관통 명제:

> **"우리 렌더러·pyhwp는 관대하고, 한글만 엄격하다(= 한글 특정)."**
> 관대한 도구가 통과시키는 파일을 한글은 거부한다. 그래서 "재읽기 무경고 + 렌더 육안 통과"는 필요조건일 뿐이고, **진짜 정답지는 오직 한글 실기 결과**다.

아래 카탈로그는 각 규칙을 `[증상 | 원인 | 수정 | 정답지·근거 | 파일:함수(커밋)]` 표로 정리하고, 규칙마다 **"왜 정적분석이 아니라 실기·정답지로만 잡혔는지"** 교훈을 붙였다.

> **색인 유지보수**: 여기의 규칙 ID(A1…E6)는 명세 보완 색인
> [19-hwp5-spec-supplement](19-hwp5-spec-supplement.ko.md)가 인용한다. 규칙을 추가하거나 번호를
> 바꾸면 그쪽 해당 행도 갱신한다.

---

## 진단 방법론 (규칙보다 먼저 습득한 메타 자산)

| 기법 | 내용 | 왜 필요했나 |
|---|---|---|
| **정품 바이트 대조** | 사용자가 한글로 저장한 정답지(가나다·다문단·첫째문단·테스트·테스트2·도형정답지2.hwp/hwpx)와 우리 합성 바이트를 레코드 단위로 diff | 한글의 수용 판정은 블랙박스라, "정상 표본과 바이트가 무엇이 다른가"만이 관측 가능한 신호 |
| **멀티에이전트 4축 병렬 진단** | spec / web / 표본대조 / 손상추적 4축 + 적대적 검증(28 에이전트) | 손상 원인이 5개가 동시에 얽혀(75fb581·1f0139b) 단일 가설로는 격리 불가 |
| **이분 탐색(bisection)** | 빈 셀 있는 표만 손상 → 빈 셀=빈 문단으로 격리 (4b57b8a) | 복합 문서에서 손상 유발 요소를 원자 단위로 좁힘 |
| **주입 진단(injection)** | 정답지 문서에 우리 요소만 주입 → "요소 vs 문맥" 판별 (b472070) | 우리 타원 요소는 정상 렌더됨을 확인 → 문제는 요소가 아니라 z-order 문맥으로 결정적 격리 |
| **배치 진단(placement)** | 도형 N개 배치·위치를 바꿔가며 렌더/미렌더 경계 관찰 (1438a1e) | run당 도형 수 한계(~21)라는 비스펙 한계를 스무고개로 발견 |
| **적대적 가설 기각** | "PARA_SHAPE 58B가 필수" 등 그럴듯한 가설을 표본 반례로 기각 | work_report 46B도 통과 → 길이가 아니라 **버전 정합**이 핵심임을 확정 |

---

## 타입드 보존 게이트

네이티브 writer는 `hwp-preservation-report-v1` 형식의 content-free ledger를 제공한다. event
code, resource class, disposition은 닫힌 enum이며 개수만 집계한다. 문서 텍스트, 패키지 이름,
컨테이너 경로, payload 일부, hash는 공개 보고서에 포함하지 않는다.

- writer omission과 표현 불가 payload는 typed loss event로 기록한다. 기존 `Vec<String>` API는
  호환 wrapper로 유지하지만, 발행 정책은 `DROP:` prefix를 파싱하지 않는다.
- 원본 없는 HWP/HWPX 작성은 typed loss event가 하나라도 있으면 atomic publication 전에 거부한다.
- 같은 포맷 변환·편집은 HWP stream/storage 또는 HWPX entry inventory도 대조한다. 예상하지 않은
  제거 또는 opaque non-target 항목 변경은 `--strict` 없이도 발행을 차단한다.
- 포맷 간 `--strict` 변환은 asset, control, relationship, metadata의 의미 손실과 IR이 옮길 수
  없는 패키지/컨테이너 수준 자산(HWPX 잉여 엔트리 → HWP, HWP XMLTemplate/DocHistory 슬롯 →
  HWPX)을 거부한다. `--loss-report <PATH>`는 성공 시에도 ledger를 JSON으로 기록한다.
  non-strict 변환은 typed ledger를 명시적으로 반환하는 경우에만 발행할 수 있다.

이 게이트는 알려진 silent loss를 막지만 한컴 호환성을 인증하지 않는다. 현재 HWP full rewriter에는
이슈 #90의 source-preserving repair와 독립 한컴 열기 검증이 여전히 필요하다. package-surgical HWPX
편집도 #90의 후속 단계로 남아 있다.

---

## A. 파일 "손상/변조" 게이트 (열기 자체가 거부되는 규칙)

가장 치명적인 계층. 한글이 파일을 열 때 `"파일이 손상되었습니다"` / `"문서가 변조되었을 가능성 — 보안 수준을 낮춤"` 팝업을 띄우면 내용이 아예 안 보인다. 5.1.x 버전 선언과 레코드 레이아웃의 정합성을 한글이 검사한다.

| # | 증상 | 원인 | 수정 | 정답지·근거 | 파일:함수 (커밋) |
|---|---|---|---|---|---|
| A1 | "손상된 파일" 즉시 거부 | cfb 크레이트 기본 CFB **V4**(4096B 섹터). 한글은 **V3**(512B)만 수용. 레코드가 바이트 동일해도 컨테이너 버전으로 거부 | `create_with_version(Version::V3)` 강제 + 보조 스트림(DocOptions/_LinkDoc/Scripts/HwpSummaryInformation) 표본 동형 동봉 | pyhwp는 olefile이 V4도 읽어 **통과했던 사각지대** | `hwp5/src/write.rs:167` (1dcf49d) |
| A2 | "보안 경고(변조 가능성)" | FileHeader **EncryptVersion=0**. 정품 표본 6개 전부 비암호인데 `encver=4`(한글 7.0+ 저장 마커). 0이면 거부 | `encrypt_version: 4` | 표본 전수 실측 = 4 | `hwp5/src/write.rs:144` (75fb581) |
| A3 | 5.1.x 합성본만 "변조" (원본 왕복은 정상) | 문서를 **5.1.0.1로 선언**하면서 레코드 레이아웃이 구형. `PARA_SHAPE 54→58B`, `PARA_HEADER 22→24B`(5.0.3.2+ 변경추적 병합 UINT16). 버전-레이아웃 불일치를 변조로 판정 | 버전 게이트: 합성(5.1.x)만 58/24B, 구버전 왕복(5.0.2.x)은 22B 유지 | 정상 표본 hello_world(5.1.0.1) 대조 | `hwp5/src/write.rs:emit_paragraph` (1f0139b) |
| A4 | 5.1.x 합성본 "손상" (보안 낮춰도) | **COMPATIBLE_DOCUMENT 서브트리 누락**. 5.1.x는 DocInfo에 필수인데 1차 워크플로가 구버전(work_report 5.0.2.4=면제)만 보고 "필수 아님" 오판 | source≠hwp5일 때 ID_MAPPINGS 직후 `COMPATIBLE_DOCUMENT(0x1E,4B=0)>LAYOUT_COMPATIBILITY(0x1F,20B=0)+TRACKCHANGE(0x20,1032B)` 추가 | 정품 가나다(5.1.1.0)·hello_world 둘 다 보유, 실측 바이트 복제 | `hwp5/src/write.rs:1260-1272` (5844ec8) |
| A5 | 빈 셀 있는 표만 "손상 + 본문 비어있음" | 빈 문단이 `chars=[0x0d]`가 되어 `PARA_TEXT=[0x0d]`로 방출. 한글은 **'내용이 문단끝뿐인 PARA_TEXT'를 손상**으로 판정 | `char_count>1`일 때만 PARA_TEXT 방출. 빈 문단은 `nchars=1` 유지하되 PARA_TEXT **생략**(암묵적 문단끝) | 이분탐색으로 빈 셀=손상 격리. 정품 work_report·한라대 빈 문단 전수 = nchars=1 + PARA_TEXT 없음 | `hwp5/src/write.rs:1605-1618` (4b57b8a) |
| A6 | GFM 표 빈 셀 있는 md 변환본 "손상" | GFM `\| \|` 빈 셀에 PARA_HEADER 미부착 → `LIST_HEADER nparas=0`. 한글이 손상 처리 | 셀 종료 시 `flush_paragraph_inner(force=true)`로 빈 셀도 문단 1개 보장 + 짧은 행 누락 칸 충전 | 정품 전수: 빈 셀도 문단 1개 보유 | `hwp-convert/from_markdown.rs:265` (f64165f) |
| A7 | 손상 + pyhwp 크래시 | 빈 문단에 **PARA_CHAR_SHAPE run 0개**. 한글 불변식 `PARA_HEADER 수 == PARA_CHAR_SHAPE 수` 위반 | 빈 문단에 `(0, 현재모양)` run 1개 충전 | 정품 전수 불변식 | `hwp-convert/from_markdown.rs:515` (f64165f) |
| A8 | 합성본 비정상 판정 | DOCUMENT_PROPERTIES **시작번호(쪽/각주/미주/그림/표/수식) = 0** + PARA_HEADER **instance_id=0** | `max(1)` 적용 + 합성 경로(source≠hwp5)에 `0x10000001`부터 유니크 부여. hwp5 원본 왕복은 원본값(0 포함) 보존 | 진단2(우리 바이트동일 왕복) vs sample_m6(실패) 값 비교. 표본 시작번호 전부 1, instance_id 전부 non-zero 고유 | `hwp5/src/write.rs:emit_doc_info` (9efd9ce) |
| A9 | 왕복 hwp "손상" (도형 포함 문서) | gso `SHAPE_COMPONENT` 중첩이 LIST_HEADER/문단을 잃고 형제로 hoist돼 레코드 트리 파괴 | `GenericControl.raw_children`로 원본 자식 서브트리 무손실 보존·재방출 (이후 ㉕에서 안전 저하로 전환) | 왕복본 손상 지배 원인 | `hwp-model/src/control.rs`, `hwp5/src/write.rs` (75fb581) |
| A10 | 손상 유발 잔가지 | PARA_HEADER `ctrl_mask`에 CharCtrl(문단끝13 등 문자형) 포함 → 잘못된 bit13; ID_MAPPINGS 무조건 18패딩(구버전=16); secd에 FOOTNOTE_SHAPE×2·PAGE_BORDER_FILL×3 누락; TAB_DEF/NUMBERING dangling ref | ctrl_mask에서 CharCtrl 제외, 버전별 ID_MAPPINGS(5.0.2.x=16/5.0.3.2+=18), secd 필수 자식 합성, TAB 3개+NUMBERING 1개 기본값 합성 | 정품 hello_world 실측 바이트 | `hwp5/src/write.rs` (75fb581, 1f0139b) |
| A11 | **hwpx** 문서를 열면 한글 전체 **먹통(무한 행)** — 팝업조차 없음 | **★원인 확정(2026-07-18, Phase 2 감사 C15)**: 본문 탭이 IR에서 `Text('\t')`로 모델링돼 `<hp:t>` 안에 **raw 0x09 바이트**가 그대로 방출됨 — 한글은 hp:t 내 raw 제어문자를 만나면 먹통. 탭 정의(tabPr)는 완전 무죄(E/F/G 이분탐색: 탭 수술 없는 베이스 F1도 먹통, 실문서 왕복 E1은 정상 — 베이스에만 탭 문자 존재). 1차 가설(naked tabItem)은 반증됐으나 그때 확립한 `hp:switch>case(HWPUNIT, pos=X)/default(pos=2X)` 구조는 정품 실측 사실로 유지 | 탭은 항상 `InlineCtrl(9)`→`<hp:tab/>` 불변식: from_markdown 유입 차단 + write 방어선(제어문자 정화) + read 정규화(오염 파일 하위호환) | 실기 3회 이분탐색(D3→E→F/G) + F1↔C1 전 파트 diff + 정품 삼각 대조 | `hwp-convert/from_markdown.rs`, `hwpx/write/section.rs·templates.rs(esc)`, `read/section.rs` |
| A12 | **hwpx** 본문 탭이 **폭 0으로 무시**(텍스트 밀착 — 파일은 정상 열림) | `<hp:tab/>`을 `<hp:t>` **밖** 형제로, 속성 없이 방출. 한글은 **`<hp:t>` 안 mixed content** 탭만 인식: `<hp:t>개요<hp:tab width=".." leader=".." type=".."/>1</hp:t>` | in-t 중첩 방출 + 속성 유도: **type=hwp5 탭종류+1**(LEFT→1, RIGHT→2 실측; CENTER→3, DECIMAL→4 외삽), **leader**: NONE→0·DASH→3 실측(표 25 코드와 순서 다름 — SOLID→1·DOT→2는 자기일관 근사, 미확인 코드는 DASH 강등), **width**=저장 시점 레이아웃 실측값이나 한글이 열 때 재계산(정품에서 폭이 텍스트 길이에 반비례함으로 입증) → 근사값(4000) 허용 | 실기(D3 폭0 밀착) → 정품 " .hwpx" 인라인 탭 91개 전수 역산(type2/leader3=RIGHT/DASH 51개, type1/leader0=기본 40개) | `hwpx/src/write/section.rs`(tab_xml), `read/section.rs`(in-t tab arm) |

**A 계층 교훈 — 왜 정적분석이 아니라 실기였나:**
- pyhwp/olefile은 **관대한 파서**라 V4 컨테이너·nparas=0·PARA_TEXT=[0x0d]·시작번호 0을 전부 통과시킨다. 외부 검증 통과가 곧 한글 통과를 의미하지 않는다는 것을 A1(CFB V3)에서 뼈저리게 확인.
- 손상 원인은 **단일이 아니라 5개가 동시**에 걸려 있어(75fb581) 한 가설을 고쳐도 팝업이 그대로였다. 멀티에이전트 병렬 + 이분탐색 없이는 격리 불가.
- 결정적 통찰은 **"길이가 아니라 버전 정합"**(A3): "58B가 보편 규격"이라는 그럴듯한 가설을 work_report 46B 통과라는 반례로 기각. 정적으로는 "표본이 58B니 58B가 맞다"로 오판했을 것.

---

## B. 렌더 정합 — 검은 바 / 빈 내용 / 다문단 / 세로 위치

파일은 열리지만 글자가 검은 막대로 덮이거나, 둘째 문단부터 사라지거나, 문단 사이 여백이 뭉개지는 계층. 5.1.x의 줄 배치 캐시(PARA_LINE_SEG)와 nchars 최상위 비트의 의미가 핵심.

| # | 증상 | 원인 | 수정 | 정답지·근거 | 파일:함수 (커밋) |
|---|---|---|---|---|---|
| B1 | **검은 바**(글자 자리마다 검은 막대, 글자 안 보임) | `char_shape.shade_color` 기본값 **0(=불투명 검정 음영)**. 한글이 글자 칸마다 검은 배경 하이라이트로 그려 검정 글자가 안 보임 | `shade_color=0xFFFFFFFF`('없음' 표식). `shadow_color=0xC0C0C0`, `shadow_gap=(10,10)`, PARA_SHAPE `attr1=0x180`(줄나눔+줄격자), `border_fill_id=2`도 정품 동형 | 정품 전수 shade_color≠0 (가나다=0x00C0C0C0, hello_world=0xFFFFFFFF). **face_id=0 가설 기각**(정품 hello도 face_id=0인데 정상 → 무해) | `hwp-convert/from_markdown.rs:51-57` (dad441b) |
| B2 | 5.1.x 본문이 **0높이로 그려져 빈 내용/검은 바** | 본문 문단에 **PARA_LINE_SEG 부재**. 5.1.x 정품은 본문 문단 lineseg 100% 보유(work_report 5.0.2.4는 없이 재계산). bit31 SET인데 lineseg 0개 = 모순 | `synthesize_linesegs`로 합성 경로 문단당 줄 배치 합성. 정품 공식: `line_height=글자크기, baseline_gap=base×0.85, line_spacing=base×0.6(160%), flags=0x00060000` | 정품 가나다 PARA_LINE_SEG 바이트 완전 동일. 표 셀 문단도 재귀 처리 | `hwp5/src/write.rs`, `hwp-render/lineseg.rs:synthesize_linesegs` (a7abdfc) |
| B3 | nchars **bit31** 다루기 (revert saga) | bit31(0x80000000) = "PARA_LINE_SEG 캐시가 내용과 정합" 선언(bit31=1 ⟺ 그 문단에 lineseg 존재). "정합한다고 선언 + 캐시 0개"면 검은바/손상 | lineseg를 실제 방출할 때만 bit31 SET → **이후 폐기(e41b440 revert)**: 부정확 lineseg가 '변조' 재유발, 다문단 v_pos 모순. 최종은 lineseg 합성 + 마지막 문단만 bit31 | work_report 73/73, 가나다 1/1 bit31=1. **한 번 revert된 규칙** — 정적으로는 못 잡음 | `hwp5/src/write.rs` (e32a2a8→e41b440→a7abdfc) |
| B4 | **다문단 둘째부터 안 보임** | nchars **bit31을 모든 문단에 SET**. bit31은 사실 **'리스트(섹션/셀)의 마지막 문단' 표식**. 첫 문단에 SET하면 한글이 그것을 마지막으로 보고 뒤 문단 무시 | `set_last_para_flag`: 각 리스트(섹션·표셀·글상자)의 **마지막 문단만** `chars_flags\|0x80`, 나머지 클리어 | 정품 다문단.hwp(5.1.1.0): 섹션 4문단 중 문단4만 SET. 단일 문단(가나다)이 정상이었던 건 1문단=마지막이라 **우연히** 맞았던 것 | `hwp5/src/write.rs:252 set_last_para_flag` (ae73e3c) |
| B5 | 문단 사이 여백 뭉개짐(제목 위 여백 사라짐) | 합성 줄 배치 v_pos에 문단 위/아래 간격(spacing_top/bottom) 미반영 → 압축 표시 | v_pos에 `문단 사이 간격 = 앞 아래간격 + 이 위간격` 가산(섹션 첫 문단 위간격 제외, v_pos=0) | 세로 위치 어긋남 실측 | `hwp-render/lineseg.rs:50-62` (7686444) |
| B6 | **멀티페이지 문서만 "손상"** | 합성 v_pos가 페이지 리셋 없이 섹션 단조 누적(최대 354408) → 페이지 본문 높이(75686) 초과 → 손상 | md 출처는 `content_h` 초과 시 v_pos=0 리셋(다음 페이지), 표는 잔여 부족 시 통째로 다음 페이지. hwpx 출처는 원본 linesegarray 보존(덮어쓰지 않음) | 정품 한라대 hwpx: 본문 vertpos 페이지마다 0 리셋, 최댓값 59668<본문높이, flags 리셋 시도 0x60000 유지 | `hwp-render/lineseg.rs:38-48`, `convert.rs` (78d478b) |
| B7 | **hwp5 글머리 마커만 미표시**(열림·각주·번호는 정상) | 합성 BULLET 레코드를 스펙 표 42대로 20B·문자@8로 만듦. 실전 레이아웃은 **25B** — 오프셋 [8..12]에 번호 글자모양 id(0xFFFFFFFF, NUMBERING과 동형) 4B가 먼저 오고 **글머리표 문자는 @12**. 한글이 @12에서 널을 읽어 마커를 안 그림(스펙 표 42는 총길이 자기모순 이력이 있는 오기) | `make_bullet_data`를 정품 25B 레이아웃으로. **리더도 @8→@12 교정** — 기존엔 정품 파일의 글머리 문자를 글자모양 id 하위워드(0xFFFF)로 오독 중이었음 | 실기(H2 마커 누락) → 정품 제주한라대 사업계획서.hwp BULLET 5개 전수 바이트 대조(구조 동일·문자만 상이) | `hwp5/src/write.rs:make_bullet_data`, `doc_info.rs`(리더 교정) |
| B8 | (규칙 확립) hwp5 취소선은 **쓰기 전용 bit18** | CHAR_SHAPE attr bit18(§4.2.7 표35 취소선 여부)은 **읽기로 믿으면 안 됨** — 코퍼스 실측: 변경추적 삭제표시 템플릿(고정값 0x3c0400f8)이 bit18을 세워 한 파일의 92%가 가짜 취소선. 리더는 strike:false 유지 | 합성(md 출신 등 취소선임이 확실한 경우)만 attr\|=1<<18 기록 — **실기 확정(2026-07-19 H2): bit18 단독으로 한글이 취소선을 렌더**(모양 bit26~29 불요). 최악의 경우도 미표시일 뿐 손상 없음 | 코퍼스 attr 전수 실측(평문 표본 bit18=0, 오염 패턴 격리) + H2 실기 | `hwp5/src/write.rs:emit_char_shape`(쓰기), `doc_info.rs`(읽기 불신 유지) |

**B 계층 교훈:**
- 검은 바(B1)의 진짜 원인은 **한 개 UINT32 필드의 기본값**이었다. 렌더러·pyhwp는 shade_color=0을 "음영 없음"으로 관대하게 해석하지만, 한글만 "불투명 검정 하이라이트"로 그린다. 스펙에 "0=불투명"이라는 경고가 없어 정적으로는 무해해 보인다.
- bit31의 의미(B3/B4)는 스펙에 "줄배치 캐시 정합"으로만 적혀 있었고, **실제로는 '리스트 마지막 문단' 표식**이라는 이중 의미를 정품 다문단.hwp 실측으로만 알아냈다. 단일 문단 표본만 보면 두 해석이 구별 안 돼(1문단=마지막) 우연히 맞아 넘어갔다 — **표본 다양성 부족이 정적 오판을 만든다.**
- B3는 **한 번 채택했다가 revert**한 유일한 규칙. 정적 추론("정품이 항상 bit31=1이니 항상 SET")이 실기에서 검은바를 유발 → 되돌림. 규칙의 참·거짓은 오직 실기가 판정한다.

---

## C. 표(Table) 레이아웃 정합

| # | 증상 | 원인 | 수정 | 정답지·근거 | 파일:함수 (커밋) |
|---|---|---|---|---|---|
| C1 | 표 다음 본문이 표와 겹쳐 "손상" | 표 앵커 문단 v_pos를 `line_advance`(1600, 1줄)만 진행 → 표 다음 본문이 표 위에 겹침 | 표 있는 문단은 `v_pos = 진입값 + Σ표높이`로 보정 | 정품 첫째문단.hwp: 본문+표(3x7)+본문에서 표 advance=4412 | `hwp-render/lineseg.rs:88` (0e2d568) |
| C2 | 표 높이 계산 상수 | `표높이 = Σ_행 max(상margin + 줄블록 + 하margin) + **566**(TABLE_BLOCK_PADDING, 2.0mm)`, `줄블록 = 셀 마지막 lineseg.v_pos + line_height` | table_height 공식 구현 | 3x7: 3×(141+1000+141)+566 = **4412 EXACT**; work_report 1x2(2줄셀)=6048 일치(두 표본 교차검증) | `hwp-render/lineseg.rs:194 table_height` (0e2d568) |
| C3 | 빈 표 셀 손상 | (A6/A7과 동일 축) 빈 셀 nparas=0 / char_shape run 0 | 셀당 문단 1개 + char_shape run 1개 보장 | 정품 빈 셀 60개 전수 nchars=1 | `hwp-convert/from_markdown.rs` (f64165f) |

**C 계층 교훈:** 566(2.0mm 셀블록 패딩)은 **스펙에 없는 경험 상수**로, 두 정답지(첫째문단 3x7=4412, work_report 1x2=6048)에서 **동시에 정확히 맞아떨어져야** 채택했다. 단일 표본이면 우연의 일치와 구별 불가 — 교차 실측이 상수 확정의 유일한 방법. (한계: base 1000/줄간격 160% 기준이라 셀 글자크기가 다르면 부정확 가능 — 현 writer는 항상 본문 1000이라 안전.)

---

## D. 그리기 개체(도형) — annual_report 6쪽 링 다이어그램 (㉙~㊱)

가장 길고 어려운 조사. 표지·인포그래픽 도형이 한글에서만 대량 미렌더되는 문제를, 사용자 정답지(테스트2.hwpx·도형정답지2.hwpx)와 주입/배치 진단으로 8라운드에 걸쳐 격리.

| # | 증상 | 원인 | 수정 | 정답지·근거 | 파일:함수 (커밋) |
|---|---|---|---|---|---|
| D1 | 표지 **빈 화면**(도형 다수 미렌더) | `<hp:rect>` 등에 한글 필수 요소 통째 누락: `hc:pt0~pt3`(외곽 4모서리), `hc:fillBrush`, `hp:shadow`, pos flowWithText/allowOverlap, textWrap | Rect/Ellipse/Arc에 bbox 4모서리 pt0~3 방출, fillBrush 항상, shadow NONE, `textWrap=IN_FRONT_OF_TEXT`, 부유도형 flowWithText=0/allowOverlap=1 | 정답지 테스트2.hwpx 바이트 대조(잔여차=linesegarray[재계산]·shapeComment[주석]만). **우리 렌더·pyhwp는 문서순+bbox로 정상 = 한글 특정** | `hwpx/write/section.rs:661 write_shape_element` (99d6b87) |
| D2 | 글상자 텍스트 배치 어긋남 | 도형 텍스트 문단에 `<hp:linesegarray>` 누락(convert 기본 preserve_linesegs=false) | write_draw_text 내부 write_paragraph 호출에 `preserve_linesegs=true` 강제(도형 텍스트만) | 정품 실측: 한글은 글상자 문단 lineseg 없으면 재계산 | `hwpx/write/section.rs:591 write_draw_text` (0e397de) |
| D3 | 표지 **거의 빈 화면**(143개 도형) | 원본 gso z-order 전부 고유(1~143)인데 parse_gso_header가 offset16까지만 읽고 write_shape_element가 `zOrder="0"` 하드코딩 → 전 도형 z=0. 한글이 동일 z를 undefined 순서로 그려 덮개 도형이 내용 가림 | parse_gso_header에 z-order(offset20) 추가, 실값 방출 | 개별 도형은 렌더되는 work_report와 구조 동일 → z-order가 유일 차이 | `hwpx/write/section.rs:parse_gso_header` (241f8d3) |
| D4 | 타원/호 링 15개 미렌더 | 타원/호에 pt0~3(사각형 모서리)를 넣었으나 한글은 **center/축 기반**으로 정의. SC_ARC(0x51)는 gso.rs가 "v1 제외"로 미파싱 → 호 4개 통째 드롭 | 타원=`center/ax1/ax2/start-end`, 호=`center/ax1/ax2`, pt0~3는 Rect 전용. SC_ARC(0x51) 파싱 추가(BYTE kind+center+ax1+ax2 25B) | 정답지 도형정답지2.hwpx. ★부수성과: annual hwp→hwpx **DROP 80+→0** | `hwpx/write/section.rs`, `hwp-convert/gso.rs:280` (43948ff) |
| D5 | 타원(링) 여전히 미렌더 | 유일 차이 = `curSz`. 정품 타원/호는 `<hp:curSz width="0" height="0"/>`(미리사이즈 없음 표식), 우리는 (w,h) | 타원/호는 curSz=(0,0), 사각형 등은 (w,h) 유지 | 정답지 도형정답지2 값 대조(center/ax/start-end/fillBrush 전부 이미 동일) | `hwpx/write/section.rs:704` (73910e8) |
| D6 | 도넛·중앙원 미렌더(호만 보임) | ㉙에서 "무채움도 #FFFFFF 방출"로 바꿔, 투명이어야 할 **큰 가이드 동심원(무채움)이 불투명 흰 원반**이 되어 뒤 도넛을 덮음 | fillBrush를 **채움 있을 때만** 방출. 무채움(fill=0xFFFFFFFF)은 fillBrush 생략(투명) | fill 플래그 파싱: 원본 큰 타원 fill=0x0(무채움). **우리 렌더·pyhwp는 관대, 한글만 불투명 = 한글 특정** | `hwpx/write/section.rs:725` (7efac19) |
| D7 | 도넛 4개 미렌더 | 그룹 도형(도넛=회색외곽+흰구멍 2타원/1 gso)에서 두 타원이 **같은 z** → 한글이 z 충돌 시 하나만 그리고 스킵. 중복 z=94/96/98/100=4개 도넛 | write_gso가 gso당 다중 도형에 고유 z: `zorder*Z_SCALE(64)+도형인덱스` | **주입 진단**: 정답지에 우리 타원 주입 → 정상 렌더(z 고유) → 문제는 요소 아닌 문맥(z 충돌)으로 결정적 격리 | `hwpx/write/section.rs:820 write_gso` (b472070) |
| D8 | **링 전부 미렌더(호만)** — 근본원인 | 한글은 **한 `<hp:run>`에서 앞쪽 ~21개 도형만 렌더**하고 나머지 버림. write_paragraph가 char_shape 같으면 한 run에 몰아넣어(6쪽=35개/run), 22번째 이후 타원(위치22~34)이 전부 잘림. 호(12~15)·다각형(16~19)은 한계 안이라 렌더 | run당 도형 수를 세어 `SHAPE_RUN_LIMIT(12)` 넘으면 같은 char_shape로 run 강제 분할 | **실기 확정**: annual_run분할.hwpx(run당12)에서 6쪽 도넛4+중앙원+호 전부 표시. 3쪽(도형29개, 타원 위치7~20 초반)은 렌더된 것과 대조 | `hwpx/write/section.rs:94 SHAPE_RUN_LIMIT / write_paragraph` (1438a1e) |
| D9 | 호가 **전체 타원 루프**로 렌더(우리 렌더러) | shape_draw.rs arc 경로가 arc를 ellipse와 동일 취급(bbox 전체 타원). reader가 arc center/ax1/ax2를 버려 points 비어있음 | reader가 center/ax1/ax2 포착, 렌더러가 3점으로 **1/4 타원호를 큐빅 베지에**(어파인 불변 → 비수직 전단축도 정확) | 변환 hwpx 6쪽 렌더가 원본 hwp 직접렌더와 호까지 완전 동일 | `hwp-render/shape_draw.rs:192`, `hwpx/read/section.rs` (a5aae3f) |
| D10 | 호가 **pinwheel(바람개비)**로 어긋남(한글) | 한글 OWPML arc는 center/ax1/ax2를 **수직 두 축 타원**으로만 해석. 변환기가 gso 행렬(회전+비균등 스케일=전단)을 3점에 구워 두 축이 비수직 → pinwheel | gso.rs geometry()에서 arc 두 축을 이등분선 기준 ±45°·평균 길이로 **등방화**(수직 원형 1/4호 근사). 회전·위치 완벽 보존, 미세 타원율만 손실 | 정답지 도형정답지2 실측(한글은 수직축만 해석). 축 내적≈0·길이 동일 확인 | `hwp-convert/gso.rs:geometry` (0ebeef2) |

**D 계층 교훈:**
- D8(run당 ~21개 도형 한계)는 **어떤 스펙에도 없는 렌더러 내부 한계**다. 우리 렌더러·pyhwp는 문서순으로 전부 그리므로 정적으로는 관측 불가. "3쪽은 되고 6쪽은 안 됨"의 diff → run당 도형 위치 분석 → 배치 진단(4도형 4처리 1렌더)이라는 실험 설계 없이는 발견 불가능했다.
- D6·D1·D3·D7·D10은 전부 **"우리 렌더·pyhwp는 관대, 한글만 엄격"** 패턴: 무채움을 투명으로, z=0을 문서순으로, pt0~3 없어도 bbox로, 비수직 축도 그대로 — 우리 도구는 다 통과. 한글만 거부. 정적 분석의 통과가 무의미한 이유의 집약.
- **주입 진단(D7)이 결정적**이었다: 정답지에 우리 요소만 이식해 "요소 자체는 정상, 문맥(z충돌)이 문제"를 분리. 순수 정적 분석은 "요소 vs 문맥"을 구별할 수단이 없다.

---

## E. 하이퍼링크 / 필드 (클릭 이동 게이트)

파랑+밑줄로 보이기만 하고 클릭이 안 되는 문제. 필드는 FIELD_START↔FIELD_END 짝맞춤·instance id·종류별 attr 4계층이 모두 맞아야 작동.

| # | 증상 | 원인 | 수정 | 정답지·근거 | 파일:함수 (커밋) |
|---|---|---|---|---|---|
| E1 | 하이퍼링크가 **평문 취급**(링크로 인식 안 됨) | 표시 텍스트에 하이퍼링크 글자모양(파랑+밑줄) 없음 | create_hyperlink이 `#0000FF+밑줄` CharShape 확보·적용 (shade_color=0 방지 포함) | 정품 work_report "설치하기"는 별도 charPr | `hwp-convert/field.rs:548,710` (cea2b66) |
| E2 | hwp5 하이퍼링크 미작동(hwpx는 작동) | 필드 **instance id=0**. 한글은 id=0 필드를 하이퍼링크로 인식 안 함 | FNV-1a 해시로 URL별 결정론적 비영 id | 정품 %hlk id=0xd707bf6d(비영). hwpx B4는 id 비영이라 작동한 것과 대조 | `hwp-convert/field.rs:472` (87bd62e) |
| E3 | 종류별 필드 attr 어긋남 | hwpx 읽기 경로가 %hlk·%fmu 모두 attr=0으로 방출 | 종류별: `%hlk=(0x00008800,0)`, `%fmu=(0,0x08)`, 기타=(0,0) | 정품 실측 | `hwp-convert/field.rs:make_field_command_data` (87bd62e) |
| E4 | 파랑+밑줄은 되나 **클릭 이동 안 됨** | %hlk attr가 `0x00008800`(work_report 복제, bit 0x2000 누락). 한글은 이 비트 없으면 클릭 이동 안 함 | attr `0x8800→0xa800`(정품 실측) | 한글 제작 정품 %hlk = 0x0000a800 | `hwp-convert/field.rs:477` (241f8d3) |
| E5 | 여전히 **클릭 이동 안 됨**(최종 원인) | FIELD_END payload 전부 0. 한글은 FIELD_START↔END를 **ctrl_id로 짝지어** 필드를 닫는데 END가 0이면 미완성 | `field_end_payload(ctrl_id)`: 역순 ctrl_id 3B(% 제외)+0. hwpx는 LIFO로 짝 START 찾음 | 정품 테스트.hwp: %hlk END=`6b 6c 68 00`(="klh\0"). attr·id·글자모양·command 다 동일한데 END만 달랐음 | `hwp-convert/field.rs:420 field_end_payload` (39c728c) |
| E6 | 왕복 hwp 손상(글상자 포함) | gso 역합성 SHAPE_COMPONENT 252B 템플릿이 정품 239B와 13B 어긋남. 한글 자가검증 불가라 재합성은 손상 재발 위험 | **안전 저하**(degrade): 글상자(텍스트 보유)는 문단을 본문으로 **hoist**(텍스트·필드 보존), 순수 장식은 드롭 | 정답지 대조. 손상 제거 + 텍스트 보존 | `hwp5/src/write.rs:467 degrade_hwpx_gso` (cea2b66) |

**E 계층 교훈:** 하이퍼링크 클릭은 **4개 조건(글자모양·비영id·attr 0xa800·END payload)이 전부 AND**여야 작동한다. 실기를 4회 반복하며 한 번에 하나씩(E1→E2→E4→E5) 벗겨냈다. 정답지가 "attr·id·글자모양·command 다 같은데 END만 다른" 최소 반례(E5)를 제공해서야 마지막 조건을 격리 — 정품 정답지 없이는 "다 맞는데 왜 안 되지"에서 멈췄을 것. **한 번에 한 변수만 다른 정답지가 곧 진리표**다.

---

## F. 미해결 / 조사중 (원인 조사중 = 속성 충실도 유력)

| # | 증상 | 현재 상태·가설 | 근거·방향 | 파일:함수 |
|---|---|---|---|---|
| F1 | **글상자 드롭**(왕복 hwp에서 글상자 박스 자체 소실) | **의도적 안전 저하**로 잠정 해결(E6): 텍스트는 본문으로 hoist해 보존하되 도형 래퍼는 생략. 근본 해결(무손실 gso 재합성)은 **속성 충실도**(SHAPE_COMPONENT 239B 정품과 테두리/채움/attr/zorder/desc 전 필드 바이트 일치)가 확보돼야 가능 — 유력 원인 | ㉕에서 252B 템플릿이 13B 어긋나 손상. 정품 239B 전 필드 실측 대조 필요. 한글 자가검증 불가 = 실기로만 검증 가능 | `hwp5/src/write.rs:degrade_hwpx_gso` |
| F2 | **페이지 오버플로**(합성 멀티페이지 세로 넘침) | md 출처는 content_h 리셋으로 방어(B6), hwpx 출처는 원본 linesegarray 보존. 남은 리스크 = 폰트 셰이핑 줄바꿈이 정품과 미세하게 달라 페이지 경계가 어긋나는 경우 — **줄 배치 속성(seg_width/line_height/spacing) 충실도**가 유력 원인 | 정품 한라대 max v_pos 59668 정확 재현 확인, md는 72712<75686 방어. 다양한 글자크기·다단 문서 실기 확대 필요 | `hwp-render/lineseg.rs:synthesize_linesegs / compute_linesegs` |

**F 계층 방향:** 두 미해결 항목 모두 **"속성 충실도(정품 바이트와의 전-필드 일치)가 충분히 높으면 자연 해소"**된다는 것이 유력 가설. 지금까지 해결한 모든 규칙이 결국 "정품 정답지와 바이트가 다른 필드를 하나씩 맞추면 한글이 수용"이라는 동일 원리였으므로, 남은 것도 실기 반복 + 정답지 확보로 좁혀야 한다. 정적 분석은 "무엇이 충분한 충실도인가"의 기준을 제공하지 못한다 — 오직 한글이 판정한다.

---

## 부록 1. 정답지(Ground-truth) 자산 목록

| 정답지 | 버전 | 무엇의 진리표 | 확정한 규칙 |
|---|---|---|---|
| hello_world.hwp | 5.1.0.1 | 정상 5.1.x 최소 표본 | A3(58/24B), A4(COMPATIBLE), B1(shade), B2(lineseg) |
| 가나다.hwp | 5.1.1.0 | 사용자 제작 단일 문단 | A4, B1, B2 lineseg 공식 |
| 다문단.hwp | 5.1.1.0 | 다문단 bit31 분포 | B4(마지막 문단만 bit31) |
| 첫째문단입니다.hwp | 5.1.1.0 | 본문+표+본문 | C1/C2(표높이 4412 EXACT) |
| work_report.hwp | 5.0.2.4 | 구버전(면제 규칙) | A3 버전게이트, C2 교차검증(6048), E1/E4 |
| 테스트.hwp | — | 순수 텍스트+한글 하이퍼링크 | E5(FIELD_END ctrl_id) |
| 테스트2.hwpx | — | 사각형+글자 | D1(pt0~3/fillBrush/shadow/pos/textWrap) |
| 도형정답지2.hwpx | — | 타원+호 | D4(center/ax), D5(curSz 0,0), D10(수직축) |
| 타원진단/annual_run분할 등 | — | 주입·배치 진단 파일 | D7(z충돌), D8(run 한계) |

## 부록 2. 파일:함수 인덱스

- `hwp5/src/write.rs`: `create_with_version`(A1), `encrypt_version`(A2), `emit_paragraph`(A3 58/24B, A5 PARA_TEXT), `emit_doc_info`(A4 COMPATIBLE, A8 시작번호/id), `set_last_para_flag`(B4), `degrade_hwpx_gso`(E6/F1)
- `hwp-convert/from_markdown.rs`: `default_header`(B1 shade_color), `flush_paragraph_inner`(A6/A7 빈셀)
- `hwp-render/lineseg.rs`: `synthesize_linesegs`(B2/B6 페이지리셋), `table_height`(C2 566), 문단간격(B5)
- `hwpx/write/section.rs`: `write_shape_element`(D1), `write_draw_text`(D2), `parse_gso_header`(D3 z), `SHAPE_RUN_LIMIT`/`write_paragraph`(D8), fillBrush(D6), curSz(D5), `write_gso`(D7 Z_SCALE)
- `hwp-convert/gso.rs`: SC_ARC 파싱(D4), `geometry`(D10 등방화)
- `hwp-render/shape_draw.rs:192`: arc 큐빅 베지에(D9)
- `hwp-convert/field.rs`: `make_field_command_data`(E2/E3/E4), `field_end_payload`(E5), 하이퍼링크 charPr(E1)

---

## G. 원본 보존형 네이티브 HWP 편집

| # | 현상 | 원인 | 수정 | 정답지 |
|---|---|---|---|---|
| G1 | 정품 복합 문서의 텍스트 하나를 편집해도 손상 경고 | section 전체 재방출 시 `CTRL_DATA`, `PAGE_DEF`, `PAGE_BORDER_FILL` 같은 level-sensitive typed-control 자식의 순서 이동 | source CFB를 복사하고 변경 section stream 안에서도 미변경 문단/control subtree를 원본에서 이식 | BodyText 이분 탐색 + 정품 복합 제안서 일부; 미변경 subtree 바이트 동일 |
| G2 | stale line layout 제거 또는 전체 재생성 시 손상 경고·layout churn | 변경된 5.1.x 문단에는 내용과 정합한 `PARA_LINE_SEG`가 필요하고, 미변경 문단은 원본 cache를 유지해야 함 | 편집본에 layout을 합성한 뒤 의미상 미변경 네이티브 문단을 불변 snapshot 값으로 복원 | 한글에서 no-op, metadata, text, paragraph 삽입, table-cell 편집, image 삽입 모두 경고 없이 열림 |
| G3 | 같은 포맷 no-op이 보조 stream 또는 CFB metadata 변경 | 새 container 조립으로 미지 stream/storage/directory 속성을 재현할 수 없음 | no-op은 exact file copy, 편집은 mutation plan이 지목한 stream만 교체 | exact-file 비교 + `MemoExtended`, 미변경 BinData, 미지 entry 보존 |

한글 acceptance는 선택적 smoke test가 아니라 네이티브 writer 계약이다. 내부 재읽기와 typed
preservation 검사를 먼저 통과한 뒤 [체크리스트](../hancom-verification-checklist.ko.md)의 편집 6종을
실제 한글에서 열어야 한다.

---

## 종합 교훈 — 왜 이 프로젝트는 "실기·정답지"에 목숨을 걸었나

1. **관대한 도구는 거짓 통과를 준다.** pyhwp·자체 렌더가 100% 통과해도 한글은 거부한다(A1 CFB V3, D6 무채움, D8 run한계 전부 우리 도구는 통과). "재읽기 무경고"는 필요조건일 뿐 충분조건이 아니다.
2. **스펙에 없는 암묵 불변식이 지배한다.** bit31=마지막문단(B4), run당 21도형(D8), 566 셀패딩(C2), shade_color=0의 검은 하이라이트(B1)는 어떤 문서에도 안 적혀 있다. 정품 바이트만이 유일한 명세.
3. **버전 정합 > 필드 존재.** "58B가 규격"이 아니라 "5.1.x 선언엔 5.1.x 레이아웃"(A3). 표본 하나로는 길이·정합을 구별 못 한다 — 구버전(work_report)·신버전(hello) 교차 표본이 있어야 진짜 규칙이 보인다.
4. **최소 반례 정답지가 진리표다.** E5는 "다 같고 END만 다른" 정답지 덕에 마지막 변수를 격리했다. 사용자가 한글로 한 변수만 바꿔 만들어 준 파일이 곧 실험 대조군.
5. **규칙은 실기가 판정하고, 틀리면 revert한다.** B3(bit31 항상 SET)은 정적으로 옳아 보였지만 실기에서 검은바를 유발해 되돌렸다(e41b440). 참·거짓의 최종 심판은 오직 정품 한글 렌더.
