[한국어](12-feature-gaps.ko.md) · [English](12-feature-gaps.md)

# 기능 격차 카탈로그 (Feature Gaps) + 난이도·의존성 로드맵

이 문서는 hwp-cli가 **아직 못 하는 것**을 한 곳에 모은 단일 카탈로그다. 포맷 지도(10·11)가
"무엇이 존재하고 우리가 그것을 어떻게 처리하는가"를 사실로 기술했다면, 12번은 그 처리 상태가
**실기·합성·렌더에서 어떤 결함으로 드러나는가**를 평가하고, 각 갭에 난이도·가치·의존성을 붙여
복원 우선순위를 세운다.

## 0. 이 문서의 위치

### 0.1 다른 문서와의 역할 분담

| 문서 | 역할 | 12와의 관계 |
|---|---|---|
| [07-hangul-compat-rules.ko.md](07-hangul-compat-rules.ko.md) §F | 실기에서 드러난 **미해결 이슈의 조사 서사**(F1 글상자 드롭·F2 페이지 오버플로) | 12는 **링크로 승계**한다. 서사를 재서술하지 않고 요약+포인터만 둔다(→ §7 GG) |
| [00-overview.ko.md](00-overview.ko.md) §5 | 현재 상태 **요약 스냅숏** | 12가 그 스냅숏을 항목 단위로 편다 |
| [10-hwp5-structure-map.ko.md](10-hwp5-structure-map.ko.md) §8 | hwp5 레코드 중 **미해석(Opaque)·raw보존** 목록 | 12 §2·§3의 **근거 데이터**(무손실 보존이 실제로 무엇을 잃는가) |
| [11-hwpx-structure-map.ko.md](11-hwpx-structure-map.ko.md) §5 | hwpx read↔write **대칭성 매트릭스**(미구현·정보소실·왕복비대칭) | 12 §2·§4·§5의 **근거 데이터** |
| [08-external-research.ko.md](08-external-research.ko.md) | 외부 근거 — 표준·오픈소스·**생태계 기능 대조**(deep-research) | §10 GJ·§14 로드맵의 수요·구현 선례 근거 |
| **12(이 문서)** | **전 기능 갭의 단일 카탈로그 + 로드맵** | — |

상태 라벨(Opaque/raw보존/skip 등)의 정본은 **10·11**이다. 라벨을 바꿔야 하면 거기부터 고치고
12는 따라온다. 스펙 § 번호·태그 이름은 사실 인용이며 문구는 전재하지 않는다([README](../README.ko.md)).

### 0.2 ID 규약

- 갭 ID는 `계열-번호` 형식(`GA-1`, `GB-6`). 계열:
  - **GA~GG** (초판): 입력 게이트 / 개체 타입 / 레이아웃·조판 / 수식 / 변환 매트릭스 / 필드·양식 / 렌더 정밀도
  - **GH~GM** (2026-07-08 재수색 추가): 내보내기 손실 / 들여오기 한계 / 미지원 포맷·레거시 / 편집 프리미티브 / 텍스트 추출 옵션 / CLI 명령·워크플로
- 07§F에서 승계한 항목은 원 번호를 병기한다: `GG-1 (=07§F1)`.
- GE 중 **hwpx→hwpx 왕복에서만** 손실되는 특수 부류는 `GE-α`(§5.2),
  **IR 경유 되쓰기에서 부속 데이터가 상수/재생성으로 대체**되는 부류는 `GE-β`(§5.3)로 분리한다.

### 0.3 "미구현 vs 무손실 보존" 구별 원칙 (이 문서의 핵심 판정 기준)

같은 레코드라도 **어느 경로에서 보느냐**에 따라 갭이기도 하고 아니기도 하다. 판정의 단일 기준:

> **Opaque 보존은 왕복에서는 갭이 아니다. 합성(포맷 간 변환)과 렌더에서만 갭이다.**

- hwp5의 `OpaqueRecord`(서브트리째 보존, [10](10-hwp5-structure-map.ko.md) §0 상태표)는
  `hwp5→hwp5` 왕복에서 **바이트를 잃지 않는다** → 레코드 수준에서는 그 경로의 갭이 아니다.
  2026-08-14부터 네이티브 HWP 되쓰기는 불변 source CFB를 복사한 뒤 계획된 stream만 교체하여
  부속 stream, storage, 미변경 binary payload까지 보존한다. package-surgical HWPX 편집도 같은 날
  착수되어 해소 이력(§0.5)의 세 번째 #90 항목을 참조한다.
- 같은 레코드를 `hwp5→hwpx`로 **합성**하려면 의미를 해석해 OWPML로 다시 써야 하는데, 그 지식이
  없으므로 **드롭**된다 → 합성 경로에선 갭.
- 렌더러가 그 개체(차트·OLE 등)를 그리려면 페이로드 해석이 필요한데 안 되므로 **빈자리** →
  렌더 경로에선 갭.

그래서 각 항목의 **영향 경로**(읽기/왕복/합성/렌더)를 반드시 명시한다. "현 동작"이 `Opaque 보존`인데
"영향 경로"에 왕복이 없으면, 그건 결함이 아니라 **설계된 무손실**이다.

### 0.4 항목 스키마

각 갭은 아래 표 형식으로 기술한다.

| 열 | 뜻 |
|---|---|
| **ID** | `계열-번호`. 07 승계 항목은 원 번호 병기 |
| **현상** | 사용자·재구현자가 관측하는 결함 |
| **근거 코드** | `파일:줄` — 실제 파일 대조로 확인한 위치 |
| **스펙/포맷 근거** | HWP 5.0 § 또는 OWPML 요소명 |
| **현 동작** | `거부` / `Opaque 보존(왕복 무손실)` / `드롭(소실)` / `근사` |
| **영향 경로** | `읽기` / `왕복` / `합성`(포맷 간) / `렌더` 중 어디서 갭인가 |
| **난이도** | `S`=자료구조만 / `M`=정답지 필요 / `L`=실기 반복 필요 |

`crates/` 접두어는 생략한다(`hwp5/src/write.rs` = `crates/hwp5/src/write.rs`).

> **등재 이력**: 초판(GA~GG) → 2026-07-08 재수색(GH~GM 신설, GE-α/β 분리) → **2026-07-19 스펙
> 전수 대조 감사**(재구성 스펙 md §3·§4.2·§4.3·§4.4 전 구역 ↔ 코드 ↔ 본 카탈로그 3중 대조,
> TODO.md Phase 2.1): GB-13~15·GD-4·GE-9~13·GE-α9·GE-β7~β8·GF-4~5·GG-21~24 추가, GE-4/5/6
> write 측 근거 보강, GG-15/GG-19 범위 확장, GB-8/GF-2 문구 정밀화.

### 0.5 해소 이력

해소된 항목은 카탈로그에서 지우지 않고 해당 행에 ✅와 날짜를 남긴다(무엇이 갭이었는지가 곧 지식이므로).

- **2026-08-14 (이슈 #90 보존 게이트)**: 네이티브 writer에 closed-enum, content-free
  `hwp-preservation-report-v1` ledger를 추가했다. 원본 없는 작성과 같은 포맷 write는 writer omission
  또는 예상하지 않은 container/package loss가 있으면 atomic하게 실패하고, 포맷 간 strict 변환은
  semantic asset, control, relationship, metadata도 비교한다. 이는 silent publication을 해소한 것이며
  writer 자체의 갭을 해소한 것은 아니며, 다음 해소 이력에 HWP repair와 HWPX repair를 차례로
  기록한다.

- **2026-08-14 (이슈 #90 source-preserving HWP writer)**: 같은 포맷 HWP `convert`, `edit`, IR
  `fill`이 불변 input snapshot을 기준으로 쓴다. 무수정은 exact file copy이며, 편집은 stream
  mutation plan을 만들고 미변경 CFB entry와 BinData를 바이트 동일하게 유지한다. BodyText는
  미변경 subtree를 보존하고 변경·삽입 문단에만 line layout을 합성한다. 비공개 복합 문서로
  no-op, metadata, text, paragraph, table-cell, image를 검증했으며 한글 손상 경고 없이 모두
  통과했다. package-surgical HWPX 편집은 다음 항목에서 해소한다.

- **2026-08-14 (이슈 #90 package-surgical HWPX 편집 + 포맷 간 손실 감지)**: hwpx reader가
  IR이 모델링하지 못하는 run 수준 컨트롤(예: `hp:container`)의 원문 XML을
  `GenericControl.hwpx_raw_xml`에 보존하고 writer가 그대로 재방출해, 전체 되쓰기에서도
  드롭되지 않는다. writer가 재생성하지 않는 패키지 엔트리(원본 META-INF override,
  DocOptions, `Contents/memoExtended.xml`, 추가 Preview)는 `Document.hwpx_extra_entries`로
  옮겨 보존하고, 본문이 참조하지 않는 BinData 엔트리도 드롭 대신 통과시켜 재생성된
  content.hpf manifest에 등재한다. 모든 같은 포맷 HWPX `edit`은
  `hwpx::patch::rewrite_document_staged`를 거쳐, 변경된 콘텐츠 엔트리(header.xml /
  content.hpf / section*.xml — before/after IR 비교로 판정)만 IR에서 재직렬화하고 나머지
  ZIP 엔트리는 바이트 그대로 raw-copy한다. 삽입 이미지는 원본 OPF manifest id를 보존한 채
  새 BinData 엔트리로 추가된다. 포맷 간 변환은 타깃 포맷이 표현 못 하는 패키지 자산
  (HWP→HWPX 방향의 hwp5 XMLTemplate/DocHistory 슬롯, HWPX→HWP 방향의 hwpx 잉여 엔트리)을
  typed `hwp-preservation-report-v1` 이벤트로 집계해 `--strict`는 이런 손실에서 fail-closed로
  실패하고, `hwp convert --loss-report <PATH>`는 어느 쪽이든 JSON verdict를 기록한다.
  과정에서 선재 writer 결함 2건(인라인 `hp:pic` zOrder, 테두리 없는 lineShape 색상)도
  고쳤다. 비공개 복합 문서 검증(패키지 36엔트리, BinData 24 — WMF 7 포함, `hp:container`
  5): metadata·text·paragraph·table-cell·image HWPX 편집 모두 엔트리 집합 동일, opaque
  엔트리 전량 바이트 동일(미디어 24→24, 이미지 삽입 후 25; container 5→5), `hwp validate`
  클린, non-strict hwp→hwpx는 미디어 24개 전량 보존(종전 9), strict 변환은 양방향 모두
  fail-closed, 8개 결과물 모두 한글에서 손상/복구 대화상자 없이 열림. #90 잔여(PR 7+):
  pagination/font 게이트, 인증.

- **2026-08-15 (이슈 #90 PR 6 — 벡터 이미지·중첩 컨테이너)**: HWPX `hp:container` 자식을
  `gso_shapes`+`GenericControl.container_box`로 파싱해 컨테이너 원점 기준으로 렌더한다
  (중첩 컨테이너는 오프셋 누적 평탄화). 원문 XML은 재직렬화 원본으로 그대로 유지.
  WMF 그림 바이너리는 bounded 순수 Rust 해석기(`hwp-render/src/wmf.rs`)가 레이아웃 시점에
  해석한다 — window/DC 상태, pen/brush/font 객체, even-odd/winding 채움 폴리곤·폴리라인,
  DIB 블릿(1bpp 마스크+컬러 투명 페어, 패턴 브러시는 density-blend 단색 근사), CP949
  `ExtTextOut` 텍스트 — 세 백엔드 모두 자홍 placeholder 대신 실제 내용을 그린다.
  부분집합 밖 레코드는 typed bounded-skip(`wmf_unsupported_record_omitted`), 손상
  스트림은 placeholder(`wmf_parse_invalid_placeholder`)로 fallback하며 둘 다 parity
  성공으로 세지 않는다. 비공개 복합 문서 parity: `unsupported_control_omitted`·
  `image_decode_placeholder` 소거(unsupported 8→0), 도형 페이지 raster 오차 개선
  (최악 페이지 bad-pixel 0.43→0.38), WMF 텍스트 검색 가능화. 잔여 델타는 PR 7의
  pagination/font 작업 범위. hwpx→hwp 변환은
  settings.xml/version.xml/preview 슬롯 손실을 아직 typed 이벤트로 집계하지 않는다.
  content.hpf 재생성은 모델링된 manifest 항목/메타데이터만 유지하므로, 비모델 manifest
  항목은 고아 엔트리로 남는다(raw-copy는 되지만 목록에는 미등재).

- **2026-08-15 (이슈 #90 PR 7a — CELL 행 초과 높이 페이지네이션)**: 렌더러가 CELL 정책
  표에서 현재 쪽의 남은 용량보다 큰 행을 잘라내지 않고 페이지를 넘겨 그린다. 명시
  플래그 없는 `v_pos` 리셋은 CELL fragment의 남은 용량에 soft하게 채우고, cached
  content를 넘는 선언 행 높이는 행 단위 빈 continuation fragment로 보존한다. row-span은
  span 단위로 판단해(하나로 옮길 수 있으면 새 페이지에서 통째로 유지, 셀 단편화가 일어난
  행과 교차하거나 새 페이지에도 안 들어가면 typed
  `table_cell_fragmentation_incomplete`로 보고) 쪽 하단의 잘린 조각(sliver)도
  봉인한다. 전체 동작 계약은 [21-pdf-parity §6](21-pdf-parity.ko.md) 참조.

- **2026-08-15 (이슈 #90 PR 7b — 글꼴 identity 게이트)**: 렌더 리포트가 글꼴 단위의
  해시화한 요청/해결 identity, 요청 weight 상태, face index,
  `font_resolution_complete`를 실는다. v2 parity `fonts` 게이트도 이에 맞춰 강화:
  대체 없는 렌더 + 해석 완결 + 해결된 모든 face의 바이트 해시가 매니페스트 고정 집합에
  속해야 하며, identity 증거가 없으면 fail-closed. 인증은 가시 중복 글꼴 행을
  중복 제거한다. PR 6 항목에 남겨둔 PR 7 잔여 델타의 font 절반을 해소했고,
  pagination 절반은 위의 PR 7a다.

- **2026-08-16 (이슈 #90 PR 8a — 인증 증거 검사)**: 인증에 선택적·fail-closed 증거
  검사 둘을 닫힌 스키마에 additive로 추가했다. `preservation`은
  `hwp-preservation-report-v1` 아티팩트를 읽어 무손실(zero-loss) 예산을 강제하고,
  `hancom_open`은 편집 산출물이 한글에서 손상·복구 없이 열렸다는
  `hancom-verification-receipt-v1` attestation(신규 스키마)을 읽는다. 증거가
  실패하거나 형식이 잘못되면 `overall=failed`로 강제하고, 검사를 생략하면 기존
  verdict 형태를 유지한다.

- **2026-08-16 (이슈 #90 PR 8b, 진행 중 — 매니페스트 선언 parity 게이트 제외)**: v2
  parity 매니페스트가 선택 항목 `gate_exclusions` 배열(중복 없는 게이트 이름, 최대
  8개)을 받아, 로컬 비공개 프로파일이 특정 게이트를 측정은 하되 차단하지 않는
  항목으로 선언할 수 있다. 측정과 `passed_gates`/`failed_gates` 보고는 그대로이고,
  적격 판정은 `blocking_failed_gates`(실패에서 제외를 뺀 것)로 계산한다. 각
  스코어카드와 스코어보드가 `excluded_gates` + `blocking_failed_gates`를 필수
  필드로 에코하므로 숨은 완화가 아니다. 제외를 선언하지 않으면 스키마가 기존
  strict 규칙을 유지해 공개 CI 프로파일은 변하지 않는다. 계약 상세는
  [21-pdf-parity §4.3](21-pdf-parity.ko.md) 참조.

- **2026-08-14 (이슈 #77 위치 지정·개수 지정 행/열 삽입)**: `--add-row`가 append 전용에서
  `TABLE[:AT[:COUNT[:TEMPLATE_ROW]]]`로, `--add-col`이 `TABLE[:AT[:COUNT]]`로 확장(`AT`
  생략·`end`면 끝에 추가. MCP `add_row`/`add_col`에도 optional `at`/`count`/
  `template_row` 필드 추가). 삽입은 먼저 논리 그리드를 만들어 검증하고, 경계를
  가로지르는 병합의 `row_span`/`col_span`을 늘리며, 스팬이 덮는 좌표 아래에는 셀을
  만들지 않는다. 새 1×1 셀의 서식은 `TEMPLATE_ROW`의 각 열을 덮는 가시 셀에서 투영
  (병합·세로 병합에 덮인 행도 서식 기증 가능, 텍스트는 복제하지 않음). 템플릿 생략 시
  append는 레거시 '깨끗한 행' 해소자를 유지하고, 위치 삽입은 경계 이하의 가장 가까운
  행을 쓴다. 열 삽입은 전체 폭 재분배 정책을 유지하고, 새 문단 instance ID는 문서
  최댓값 위로 부여하며, 모든 실패(범위·u16 오버플로·불변식 위반)는 아무것도 발행하지
  않는다. opaque HWPX 컨테이너 안의 표는 여전히 fail-closed.

- **2026-08-15 (이슈 #78 표 깊은 복제)**: `--clone-table "원본표=>앵커[=>blank|keep]"`
  (MCP `clone_table`의 `source_table`/`anchor`/`text_mode`)이 원본 표를 깊은 복제해
  앵커 문단 뒤에 삽입한다 — 기하·병합 토폴로지·너비/높이·테두리·채우기·문단/글자
  서식 보존. `blank`(기본)는 논리 셀마다 빈 서식 문단 1개만 남기고 원본 텍스트와
  콘텐츠 컨트롤(필드·책갈피·하이퍼링크·그림·수식·중첩 콘텐츠)을 모두 제거한다.
  `keep`은 중첩 표와 그림까지 복제하되 모든 문단 instance ID를 문서 최댓값 위로
  재부여하고 gso 개체 식별자(보존 common_data 오프셋 32의 ID, 또는 writer ID 합성을
  좌우하는 placement z-order)를 새 값으로 고쳐 원본과 가변 ID를 공유하지 않는다.
  바이너리 에셋은 그 자리에서 재사용. keep 모드는 모델이 안전하게 재매핑할 수 없는
  opaque `Generic` 컨트롤(필드·수식·글상자)이 있으면 조용히 빼는 대신 원자적으로
  중단한다. 앵커 매칭은 톱레벨 문단만(`--add-table`과 같은 의미), 모든 실패는
  아무것도 발행하지 않는다.

- **2026-07-15**: GA-5(버전 게이트), GE-α1~α5·α7(글자효과·밑줄모양·번호형식 hwpx 왕복),
  GE-β4(요약정보 필드), GH-1·GH-2(md/html 링크·이미지), GL-1(추출 옵션 CLI 노출) —
  Opus 4.8 병렬 구현, 전체 테스트 236 통과, E2E 스모크(링크/이미지/media 디렉토리/validate) 확인.
- **2026-07-15 (2차 — 1차 실기 피드백 반영)**: 실기에서 C6 번호 미표시·C8 날짜 누락·C9 주제 누락
  발견 → **GE-α8**(문단↔번호 heading 역방출) 해소, C8 요약정보에 **PID 0x14 한국어 날짜 문자열**
  추가(정품 40종 실측 = 작성일시 KST 파생 — 한글 '날짜' 표시의 원천), C9 content.hpf 메타를
  정품 형식으로 전면 정합(subject/keyword meta 형식, CreatedDate/ModifiedDate ISO, date 한국어;
  **hwpx 날짜 방출 갭도 함께 해소**). FILETIME 변환 유틸은 `hwp-model/src/units.rs` 공용.
  전체 테스트 247 통과. **★실기 게이트 통과(2026-07-15)**: 1차 실기에서 C1~C5·C7(글자효과)
  정상, C6·C8·C9 결함 발견 → 2차 수정 후 재검에서 **C6 번호 표시·C8 날짜·C9 주제/날짜 모두
  정상 확인**. 이 단락의 해소 항목 전체가 실기 확정됐다.
- **2026-07-15 (3차 — 저비용 배치)**: **GC-4**(탭 정의 — IR TabDef 신설, hwp5 §4.2.7 의미
  파싱+raw 병행, hwpx tabPr 왕복; 렌더 반영은 잔존), **GC-5**(secPr 미해석 자식 원문
  pass-through), **GC-8·GC-9**(내어쓰기 음수 렌더, 페이지 걸친 문단 배경 분할),
  **GE-β5**(settings.xml·version.xml 원문 pass-through; hp:switch 잔존), **GM-7**(`edit --seal`
  도장 날인 — 부유·글 앞 배치) 구현. 전체 테스트 260 통과, clippy 0.
  ~~⚠ GM-7(D1/D2 도장)·GC-4(D3 사용자 탭)는 실기 확인 대기~~ → **전부 실기 확정(2026-07-18)**.
- **2026-07-18 (4차 — Phase 2 스펙 감사)**: 사용자 재구성 스펙 md 기반 전면 감사(15건 확정,
  적대 검증 반증 4건) → 수정: **C15**(탭 raw 0x09 — A11 먹통 원인, 탭=InlineCtrl(9) 불변식 3중
  방어), **탭 in-t 방출**(A12 — bare 탭 폭0 무시, 정품 91개 역산으로 type/leader 대응표 확정),
  **C9**(표 공통속성 44B→46B), **C10**(쪽나눔 자리 holdAnchor 오기록 제거). 나머지 11건은
  주석·설계문서·스펙md 오류로 정정(TODO.md §1.4). 실기 4라운드 끝에 **D1·D2·D3 전부 통과** —
  도장 날인·사용자 탭 실기 확정. 전체 테스트 268 통과.
- **2026-07-18 (PR #8 — 외부 기여, 대조 감사 완료)**: 고충실도 markdown 내보내기 —
  **GH-3·GH-4·GH-5·GH-6·GH-8 md 경로 해소**(각주 `[^N]` 마커, 병합셀 HTML 폴백, 셀 내 블록,
  리스트 `- `/`N. `, 수식 `$..$`·글자효과 스팬) + `--media-dir`·convert 텍스트 옵션 확장.
  부수 수리: **OUTLINE heading idRef +1 밀림 실기 버그 수정**(정품 idRef=0), 번호/글머리 정의
  id 비연속·중복 관용화, **`hh:bullets` write 신설**(이전엔 글머리표 정의가 hwpx 쓰기에서 조용히
  소실 — 미등재 갭이었음), 리스트 로직 hwp-render→hwp-model 이동(허브-스포크 정리).
  대조 감사 확인: exporter 전용이라 **GI(들여오기)는 무변경 → md 왕복 비대칭 심화**(GI-1·GI-2
  우선순위 상승), html/odt 경로 잔존, GH-5 중첩 표 전용 테스트 부재.
- **2026-07-19 (GI 배치)**: **GI-1·GI-2 해소** — from_markdown이 취소선·각주(`[^N]`/`[^eN]`)·
  순서/중첩 목록을 IR로 역생성, #8 내보내기와 **md 왕복 폐쇄**(start 보존 포함, E2E 확인).
  hwpx write에 `footNote/endNote` 방출 신설(기존 DROP), hwp5에 각주·NUMBERING/BULLET 합성.
  이 과정에서 선재 결함 2건 추가 해소: 품의 코퍼스 hwp→hwpx의 번호 idRef dangling —
  **GE-7 신설 후 같은 날 근본 해소**(hwp5 read −1/write +1 경계 정규화, 임시 phantom 방어
  원복, 경계 왕복 테스트 락) — 및 C6 검증 단언 0-based 정정.
  검증 세트 25/0(H1/H2 신설), 전체 테스트 297. **★실기 확정(2026-07-19, H 3라운드)**:
  H1(hwpx) 1차 완전 통과. H2(hwp5)는 실기가 결함 2건을 추가 적발·해소 — ① BULLET 실전
  레이아웃 25B·문자@12(스펙 표 42 오기, 정품 5레코드 대조 — [07](07-hangul-compat-rules.ko.md)
  **B7**) ② 취소선 쓰기 전용 bit18(변경추적 오염으로 읽기 불신 — **B8**, bit18 단독 렌더 실기
  입증). 최종: 합성 각주·번호/글머리 목록·취소선 전부 hwp5·hwpx 양 경로 실기 확정.
- **2026-07-19 (GI 마무리)**: **GI-3·GI-4 해소** — md 이미지 임베드(base_dir 상대경로,
  인라인 Picture — insert_image 검증 경로 재사용, #8 media 추출과 바이트 왕복)·인라인 코드
  서식(함초롬돋움+연회색 음영, 다중 글꼴 테이블 배선 정합 확인). **GI 계열 전체 종결.**
  세트 26/0(I1 신설), 전체 테스트 303. 잔여: 코드 백틱 재수출 미복원(범위 밖 명시).
- **2026-07-19 (GC-2·GC-3)**: 선행 조사(정품 236파일 전수 스윕)로 레이아웃 확정·08 "반증"
  이력 종결·가치 재평가 후 구현 — **교차변환 손실 차단**(raw 병행, extras=identity 정본 유지,
  출처별 단일 방출 3단) + **쪽 테두리 렌더 신규**(정답지 4변 수치 일치, 두 에이전트 독립
  교차검증). 세트 27/0(J1 — 3층위: validate·승계 XML·렌더 잉크 0.95+). 전체 테스트 305.
  잔여: hwpx read enrich(hwpx 직접 렌더 시 테두리), EVEN/ODD·본문기준(정품 표본 부재).
  **⚠J1 실기 대기**(hwp5 raw→hwpx pageBorderFill 실속성 방출 = 새 방출 형태).
- **2026-07-30 (GG-13)**: 쪽번호 렌더 해소 — 문서 시작번호·PAGE `nwno` 재시작·`pghd` 숨김,
  `pgnp` 위치 1~10(안쪽/바깥쪽 홀짝 반전)·장식·지원 번호형식, 본문/머리말/꼬리말 PAGE `atno`
  동적 치환을 공용 DisplayList 단계에 구현. PNG·SVG·PDF가 같은 결과를 사용하며, GE-4
  (`pgnp formatType` HWPX 변환 DIGIT 고정)은 잔존하고, GG-16(머리말/꼬리말 종류 선택)은 PR 9에서 해소.
- **2026-08-13 (PDF parity PR 2, [issue #79](https://github.com/STAIxBWLB/hwp-cli/issues/79))**:
  PR #81에서 렌더러 쪽 표 쪽 분할 구현. `body_bottom`을 넘는 표가 `Table.attr` bits 0-1(pageBreak
  NONE/TABLE/CELL)·bit2(repeatHeader)와 `Cell.list_attr` bit18(제목 셀)을 따른다 — NONE은
  통째로 다음 쪽으로, TABLE/CELL은 행 경계에서 나누고 이어지는 쪽에 제목 행을 다시 그리며,
  `treat_as_char` 표는 "한 글자"라 나누지 않는다(GE-8). `row_span`이 가로지르는 경계는
  제외한다. 분할과 한 쪽보다 큰 분할 불가 행 묶음은 타입드 이슈로 보고한다
  (`TableSplitAcrossPages` info, `TableRowTooTallClipped`). 페이지를 건너간 문단의
  stale-`para_top` 앵커 버그도 수정.
  신규 모델 접근자 `Table::page_break_policy/repeat_header`, `Cell::is_header/vert_align`.
  셀 내부 분할(pageBreak=CELL)은 행 경계 분할로 근사. 이는 구현 상태이며 한컴 대등성
  인증은 아님. 잔존: 한컴 검증 라운드(제목 줄 자동 반복 정답지)와 다단 인식 표 분할.

---

## 1. GA — 입력 게이트 (읽기 자체가 거부되는 것)

가장 앞단. 파일을 열자마자 **의도적으로 거부**하는 부류다. 이들은 "버그"가 아니라 미구현을 명시적
에러로 알리는 설계지만, 실문서에서 만나면 파이프라인 전체가 막히므로 갭으로 기록한다.

| ID | 현상 | 근거 코드 | 스펙/포맷 근거 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GA-1 | 암호화 HWP5 문서를 열면 `Hwp5Error::Encrypted`로 즉시 거부 | `hwp5/src/file_header.rs:60,136`(ENCRYPTED bit1·`is_encrypted`), `container.rs:102`(`check_body_readable`), `error.rs:40` | §3.2.1 FileHeader 속성 bit1 | 거부 | 읽기 | L |
| GA-2 | 배포용(ViewText) 문서 거부 — `/ViewText/Section*`에 본문이 있어도 접근 전 차단 | `hwp5/src/file_header.rs:61,140`(DISTRIBUTION bit2·`is_distribution`), `container.rs:105`, `error.rs:43` | §3.2.1 bit2, §3.2.3 ViewText | 거부 | 읽기 | **M**★ |
| GA-3 | DRM·공인인증서 보안 문서에 **전용 거부 경로 없음** — 플래그는 인식(`info` 표시)하나 게이트는 `is_encrypted`(bit1)만 검사. DRM 전용 플래그만 선 문서는 명확한 거부 대신 하위 파싱 실패로 떨어질 수 있음 | `hwp5/src/file_header.rs:63,67,69`(DRM·CERT_ENCRYPTED·CERT_DRM 플래그), `:151`(`attribute_names`만 소비), `container.rs:101`(게이트는 bit1/bit2뿐) | §3.2.1 bit4·bit8·bit10 | 거부(불완전) | 읽기 | L |
| GA-4 | **전자 서명 문서 미처리** — FileHeader bit7(전자서명)·bit9(예비)는 이름만 인식, `DigitalSignature`·`PublicKeyInfo` 스트림은 게이트도 카탈로그도 없어 하위 파싱 실패로 낙하 가능 | `hwp5/src/file_header.rs:66,68`(HAS_SIGNATURE·SIGNATURE_SPARE 이름만), `container.rs:101`(게이트는 bit1/bit2뿐), [10](10-hwp5-structure-map.ko.md) §1 | §3.2.1 bit7·bit9, §3.2.8 서명 스트림 | 침묵(게이트 없음) | 읽기 | L |
| GA-5 | **버전 무검사 침묵 허용** — parse는 시그니처만 검사하고 버전 필드를 게이트하지 않아 5.1.x·미래 버전 전부 통과. 합성은 5.1.x 표본 상수 길이라 PARA_HEADER 24/22B 외 버전별 레코드 길이 차는 게이팅 안 됨 | `hwp5/src/file_header.rs:91-115`(버전 무검사), `write.rs:113`(5.0.3.2 분기 하나뿐), `:1072-1089`(파싱 실패 시 5.1.0.1 기본) | §3.2.1 버전 필드 | ✅ **해소(2026-07-15)** — major≠5는 `UnsupportedVersion` 거부, 5.x 전부 허용 | 읽기·왕복 | S |

**GA 교훈:** GA-1(암호화)·GA-3(DRM)·GA-4(서명)는 **복호화·인증 자체가 목표**라 정품 파일과 크립토
역설계(L)가 없으면 손댈 수 없다. ★단 **GA-2(배포용)는 L이 아니라 M** — 한컴 공식 스펙
「한글문서파일형식\_배포용문서\_revision1.2」가 복호화 알고리즘 전체(DISTRIBUTE_DOC_DATA 256B
레코드, 난수 배열, SHA1 유도 키, AES-128 ECB)를 공개하고 있고 pyhwp가 2014년부터 구현한 선례가
있다([08](08-external-research.ko.md) 생태계 대조). GA-3·GA-4는 "명확한 거부 메시지" 국소 개선(S)으로
사용성만 먼저 올릴 수 있고, GA-5는 버전 비교 한 줄이면 되는 즉시 개선 항목이다.

---

## 2. GB — 개체 타입 (레코드·요소는 있으나 의미 미해석)

가장 큰 계열. 레코드/요소가 **존재하고 스캔·왕복은 되지만**, 페이로드를 의미로 해석하지 않아
합성·렌더에서 빈자리가 되는 개체들이다. 핵심은 **포맷별 동작 차이**다:

- **hwp5** = `OpaqueRecord`로 서브트리째 보존 → `hwp5→hwp5` 왕복 무손실([10](10-hwp5-structure-map.ko.md) §8 Opaque 목록).
- **hwpx read** = `GenericControl` fallback → 개체 고유 속성은 버리고 **자식 subList 텍스트만** IR에 남김([11](11-hwpx-structure-map.ko.md) §3.3).
- **hwpx write** = 그 Generic이 알려진 ctrl_id도 gso_shapes도 아니고 보존된 원문 XML도 없으면 최종 `DROP` → **텍스트까지 소실**. 단 같은 포맷 경로에서는 2026-08-14(#90)부터 reader가 컨트롤 원문 XML(`GenericControl.hwpx_raw_xml`)을 보존하고 writer가 그대로 재방출하므로 이 DROP은 발화하지 않는다.

따라서 같은 개체가 "hwp5 왕복=무손실 / hwpx 왕복=소실 / 합성=소실 / 렌더=빈자리"로 경로마다 다르다
(GB-6은 2026-08-14, #90부터 hwpx 왕복 예외).

| ID | 개체(hwp5 태그 / hwpx 요소) | 근거 코드 | 스펙/포맷 근거 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GB-1 | **차트**(`CHART_DATA` 0x5F / `hp:chart` ooxmlchart) | hwp5 `body_text.rs:617`(Opaque), hwpx 미구현 `write/section.rs:364`(DROP), [11](11-hwpx-structure-map.ko.md) §5(c) | §4.3.9.6 | hwp5=Opaque 보존 / hwpx=드롭(텍스트도 없음=완전 소실) | 왕복(hwpx만)·합성·렌더 | L / hwpx 생성=**M**★ |
| GB-2 | **OLE 개체**(`SHAPE_COMPONENT_OLE` 0x54 / `hp:ole`) | hwp5 `body_text.rs:617`, hwpx `write/section.rs:364`, [10](10-hwp5-structure-map.ko.md) 표 B | §4.3.9.5 | hwp5=Opaque 보존 / hwpx=드롭 | 왕복(hwpx만)·합성·렌더 | L |
| GB-3 | **동영상**(`VIDEO_DATA` 0x62 / `hp:video`) | hwp5 `body_text.rs:617`, hwpx `write/section.rs:364` | §4.3.9.8 | hwp5=Opaque 보존 / hwpx=드롭 | 왕복(hwpx만)·합성·렌더 | L |
| GB-4 | **글맵시**(`SHAPE_COMPONENT_TEXTART` 0x5A / `hp:textart`) | hwp5 `body_text.rs:617`, hwpx `read/section.rs:191`(fallback 텍스트)→`write/section.rs:364`(DROP) | §4.3.9(글맵시) | hwp5=Opaque 보존 / hwpx=텍스트만 fallback 후 드롭 | 왕복(hwpx만)·합성·렌더 | M |
| GB-5 | **양식 개체**(`FORM_OBJECT` 0x5B / `hp:formObject`) | hwp5 `body_text.rs:617`, hwpx `read/section.rs:191`→`:364` | §4.3.9(양식) | hwp5=Opaque 보존 / hwpx=텍스트만 후 드롭 | 왕복(hwpx만)·합성·렌더 | M |
| GB-6 | **묶음 개체**(`SHAPE_COMPONENT_CONTAINER` 0x56 / `hp:container`) — hwp5는 raw보존이라 **렌더까지 됨**(자식 재귀). hwpx는 2026-08-14(#90)부터 같은 포맷 되쓰기에서 원문 XML을 보존·재방출(왕복 무손실)하고, 2026-08-15(#90 PR 6a)부터 컨테이너 자식을 `gso_shapes`+`GenericControl.container_box`로 파싱해 **렌더도 지원**(자식 도형을 컨테이너 원점 기준으로 배치, 중첩 컨테이너는 오프셋 누적 평탄화, 컨테이너 텍스트는 상자 안 조판). 남은 손실: hwp→hwpx 합성은 묶음을 형제 도형으로 평탄화하고, hwpx→hwp5는 typed `OpaqueControlUnrepresentable` 실패 유지 | hwp5 렌더 `hwp-render/src/shape_draw.rs`([10](10-hwp5-structure-map.ko.md) §8 raw보존), hwpx `read/section.rs`(`collect_container`), 렌더 arm `hwp-render/src/layout.rs` | §4.3.9.7 | hwp5=raw보존(렌더 O) / hwpx=원문 XML 보존(왕복 무손실)+렌더 O | 합성 | M |
| GB-7 | **메모**(`MEMO_LIST` 0x5D 본문 + `MEMO_SHAPE` 0x5C DocInfo / hwpx `hp:` 미방출) | hwp5 `body_text.rs:617`·`doc_info.rs:148`(Opaque), hwpx 네임스페이스 선언만([11](11-hwpx-structure-map.ko.md) §2) | §4.3(메모)·§4.2 표13 | hwp5=Opaque 보존 / hwpx=미구현 | 왕복(hwpx만)·합성·렌더 | M |
| GB-8 | **변경추적·편집이력**(`TRACKCHANGE` 0x20·`TRACK_CHANGE` 0x60·`TRACK_CHANGE_AUTHOR` 0x61·`PARA_RANGE_TAG` 0x46 / hwpx `hhs:` history — PARA_RANGE_TAG 용도는 스펙상 **형광펜·교정부호 등 영역 마킹**도 포함(§4.3.5), 변경추적만이 아님) | hwp5 `doc_info.rs:148`·`body_text.rs:73`(Opaque), hwpx 미구현([11](11-hwpx-structure-map.ko.md) §5(c)) | §4.2 표13·§4.3.5 | hwp5=Opaque 보존 / hwpx=미구현 | 왕복(hwpx만)·합성 | L |
| GB-9 | **문서 임의·배포 데이터**(`DOC_DATA` 0x1B·`DISTRIBUTE_DOC_DATA` 0x1C·`COMPATIBLE_DOCUMENT` 0x1E·`LAYOUT_COMPATIBILITY` 0x1F) | hwp5 `doc_info.rs:57`(Opaque). 단 writer는 COMPATIBLE/LAYOUT을 **별도 합성**([07](07-hangul-compat-rules.ko.md) A4) | §4.2.12~4.2.15 | hwp5=Opaque 보존(+합성 처리 有) / hwpx=미구현 | 합성(부분 해소) | L |
| GB-10 | **바탕쪽**(hwpx `hm:` master-page — hwp5 대응 개체 없음) | hwpx read·write 모두 없음([11](11-hwpx-structure-map.ko.md) §2·§5(c)) | OWPML master-page | 미구현 | 왕복·합성·렌더 | M |
| GB-11 | **미지 개체·금칙문자**(`SHAPE_COMPONENT_UNKNOWN` 0x73·`FORBIDDEN_CHAR` 0x5E) | hwp5 `body_text.rs:617`·`doc_info.rs:57`(Opaque) | §4.2 표13 | hwp5=Opaque 보존 / hwpx=미구현 | 왕복(hwpx만) | L |
| GB-12 | **참고문헌(Bibliography) 스토리지 미포착** — read가 IR로 안 올리고 write가 미방출 → **IR 경유 되쓰기에서 소실**(identity 왕복은 무관) | hwp5 read/write 분기 없음([10](10-hwp5-structure-map.ko.md) §1 트리 — 2026-07-08 보완 등재) | §3.2.12 Bibliography(.XML 저장) | 드롭(되쓰기) | 되쓰기 | S |
| GB-13 | ~~**캡션 완전 미해석**~~ — 표/그림/도형/OLE의 캡션(표 71~73)이 IR 필드·파서·hwpx 요소 어디에도 없음(3개 크레이트 `caption` 전수 grep 0건). hwp5 왕복은 common_data raw에 묻혀 무손실이나 합성·렌더에서 "표 1"류 캡션 텍스트가 빈자리 | hwp5 캡션 분기 부재(`body_text.rs` LIST_HEADER 캡션 판정 없음), hwpx read/write 요소명 부재 | §4.3.9 표 71~73 | ✅ **해소(2026-08-14, PR 9)** — Table/Picture/GenericControl에 `Caption` IR(side·direction·gap·width·last_width·paragraphs) 추가. hwp5는 pyhwp `TableCaption`/`GShapeObjectCaption` 판별로 캡션을 글상자 목록과 분리하고 HWPUNIT 범위를 포화 처리해 재합성한다. hwpx `<hp:caption>`은 표·그림·일반 도형에서 왕복하고, 텍스트 추출은 시각적 캡션 순서를 보존하며, 렌더러는 분할 표 캡션과 일반/미지원 GSO 캡션을 side·gap에 따라 배치한다. 검증 항목: listflags 상위 비트 미왕복, 스펙 표 71/72 길이 불일치는 표 72 준거 | 합성·렌더 | M |
| GB-14 | **NUMBERING 시작번호·확장 수준 미파싱** — 문단머리정보(표 39·40)를 읽고 버리며(`_attr`·`_width`·`_dist`), 전역·수준별 시작번호는 파싱 없이 전 레벨 `start: 1` 하드코딩. 수준 8~10 확장 필드는 IR(7레벨 고정)에 개념 자체가 없음 | `hwp5/src/doc_info.rs:452-479`(`read_level`, `start: 1`) | §4.2.8 | 근사(start=1 고정) | 읽기(dump/json)·합성 | M(레코드 후미 레이아웃 정품 대조) |
| GB-15 | **이미지·체크 글머리표 미해석** — BULLET에서 글머리 문자 1개만 추출, 이미지 글머리 여부/ID·이미지 정보(대비·밝기·효과)·체크 문자 등은 raw로만 보존 | `hwp5/src/doc_info.rs:125-138` | §4.2.9 | hwp5=raw 보존 / 합성·렌더=드롭 | 합성·렌더 | S~M(실사용 빈도 낮음 — 우선순위 최하) |

**GB 교훈:** hwp5→hwp5 왕복만 보면 GB 전체가 "무손실"이라 갭이 안 보인다(그게 §0.3의 함정). 결함은
**hwpx 왕복·포맷 간 합성·렌더**에서만 터진다. GB-6(묶음)은 특히 미묘하다 — hwp5는 렌더까지 되는데
hwpx에서는 해석이 안 됐다. 2026-08-14(#90)부터 원문 XML이 같은 포맷 되쓰기에서 살아남고,
2026-08-15(#90 PR 6a)부터 hwpx 컨테이너도 렌더돼 손실은 합성으로 한정된다. 이 계열의 복원은 대부분 **정품 파일에 그 개체를 담아 페이로드를 역설계**
(M/L)해야 하므로 정답지 확보가 선행 조건이다([00](00-overview.ko.md) §4).
★예외가 **GB-1의 hwpx 경로**다: HWPX에서 차트는 OLE가 아니라 **OOXML DrawingML `chartSpace`
XML 파트**(`Chart/chartN.xml` + manifest 등재 + `hp:chart chartIDRef`)여서, 기존 hwpx 쓰기
인프라만으로 생성·해석이 가능하다(kordoc v3.16 구현 선례 — [08](08-external-research.ko.md) 생태계 대조).

---

## 3. GC — 레이아웃·조판

문서는 열리고 텍스트도 보이지만, **조판 속성**(방향·테두리·각주 모양·탭·다단·들여쓰기)이 미반영/
근사되는 계열이다. hwp5 Opaque(왕복 무손실)이거나 hwpx skip(왕복 소실)이거나 렌더 무시로 갈린다.

| ID | 현상 | 근거 코드 | 스펙/포맷 근거 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GC-1 | **세로쓰기 미지원** — 방향이 항상 가로로 고정 방출 | hwpx `write/header.rs:335`(`textDir="LTR"` 상수), `write/section.rs:460`(`textDirection="HORIZONTAL"` 상수) | OWPML `secPr@textDirection`, `paraPr@textDir` | 근사(가로 고정) | 합성·렌더 | M |
| GC-2 | **쪽 테두리/배경 미반영** — 2026-07-19 조사로 재정의: 레이아웃(14B)은 정품 714레코드 전수로 확정·코드 왕복 이미 올바름(08 "반증" 이력 종결). 실질 갭 = ①hwp5→hwpx 교차변환 손실(상수 대체) ②렌더 전무. hwpx↔hwpx는 GC-5 pass-through로 이미 무손실 | 정답지: 제안요청서_11.19 hwp(BOTH=id7 실테두리, BF#7=4면 실선 0.4mm 검정) | §4.3.10.1.3(표135 길이 선언 오기 — TODO §1.4) | ✅ **해소·실기 확정(2026-07-19)** — raw 병행(extras=identity 정본 유지)+출처별 단일 방출(hwpx 원문→hwp5 raw 해석→상수 3단), 쪽 테두리 렌더 신규(정답지 4변 수치 일치·이중 독립 검증). J1 실기: 34쪽 전 쪽 테두리 표시·무손상 확인. 잔여: hwpx 직접 렌더 미표시(read enrich 후속), EVEN/ODD·본문기준(정품 표본 부재) | 합성(hwp5→hwpx)·렌더 | S~M |
| GC-3 | **각주/미주 모양 미반영** — 2026-07-19 조사로 재정의: 레이아웃(28B, 구분선 길이 4B)은 정품 476레코드 전수 확정. **코퍼스 전수에서 attr 커스텀 0건 + 현 렌더 하드코딩이 이미 모든 정품과 일치** → 렌더는 후순위 타당. 실질 갭 = hwp5→hwpx 교차변환 손실뿐 | 정품 5개 고유값 전부 기본형 | §4.3.10.1.2(구분선 길이 자료형 오기 — TODO §1.4) | ✅ **해소(2026-07-19)** — 각주/미주 모양 raw 병행+hwpx 방출(28B 해석). 렌더는 후순위 유지(현 하드코딩이 정품 전수와 일치) | 합성(hwp5→hwpx) | S |
| GC-4 | **탭 정의 손실**(사용자 탭 위치·채움문자) | IR `TabDef/TabItem` 신설, hwp5 `parse_tab_def`(§4.2.7, raw 병행 보존·identity 불변), hwpx `tabPr/tabItem` 왕복 — ★1차 실기에서 naked tabItem이 **한글 먹통** 유발 → 정품 `hp:switch` 구조로 교정([07](07-hangul-compat-rules.ko.md) **A11**) | §4.2.7 `TAB_DEF` / `hh:tabPr` | ✅ **해소·실기 확정(2026-07-18, 4차)** — 실기 결함 2건을 이분탐색·정답지 대조로 해소: raw 0x09 먹통([07](07-hangul-compat-rules.ko.md) **A11**)과 bare 탭 폭0 무시(**A12** in-t 방출·속성 유도). 렌더 반영은 잔존 | 왕복(hwpx만)·렌더 | S |
| GC-5 | **구역 속성 skip**(grid/startNum/visibility/lineNumberShape) | hwpx `parse_sec_pr`가 미해석 자식 **원문 XML pass-through**(`secpr_raw_children`+pagePr 센티넬), write는 원문 재방출(없으면 기존 상수) | OWPML `secPr` 자식 | ✅ **해소(2026-07-15 3차)** — 의미 파싱이 아닌 원문 보존 | 왕복(hwpx만)·합성 | S |
| GC-6 | **글상자 다단 미지원** — 연결/다단 글상자를 단일 단으로 근사 렌더 | `hwp-render/src/layout.rs:864`(`v1 단일 단 — hwp5 arm의 다단은 미지원`), `:788` | §4.3.10.2 단 정의 | 근사(단일 단) | 렌더 | S |
| GC-7 | **홀/짝수 조정 미해석** — 별도 의미 파싱 없이 Generic 통과 | hwpx `read/section.rs:597`(미지 ctrl → 코드 21 Generic), [10](10-hwp5-structure-map.ko.md) §6.1 각주 | §4.3.10.8 | Generic 보존(미해석) | 합성·렌더 | S |
| GC-8 | **내어쓰기(음수 들여쓰기) 렌더 무시** | `hwp-render/src/layout.rs` 본문·셀 양 경로에서 음수 허용(경계 클램프만), 테스트 `내어쓰기_첫줄이_왼쪽` | §4.2.10 문단모양 들여쓰기 | ✅ **해소(2026-07-15 3차)** | 렌더 | S |
| GC-9 | **문단 배경이 페이지를 걸치면 생략** | 배경을 페이지별 조각 Rect로 분할, 테스트 `페이지_걸친_문단배경_조각` | §4.2.5 테두리/배경 | ✅ **해소(2026-07-15 3차)** | 렌더 | S |

**GC 교훈:** GC-2·GC-3(쪽 테두리·각주 모양)은 **공문서에 빈출**하므로 가치가 높다. 셋 다 hwp5는
이미 무손실 보존(Opaque)이라 **정보는 갖고 있고**, 막힌 지점은 "그 페이로드를 의미로 해석해
hwpx/렌더로 내보내는 것"이다 → 정답지로 레코드 레이아웃을 확정하면(M) 풀린다. GC-4~GC-9는
대부분 자료구조·렌더 국소 수정(S).

---

## 4. GD — 수식

수식은 mini-TeX 조판기로 대부분 렌더되지만([05](05-rendering.ko.md), 커밋 `ff4184b` 이후), 다음
구성은 아직 근사·미조판이다. 근거는 조판기 헤더 주석이 명시한 **알려진 미지원 목록**이다.

| ID | 현상 | 근거 코드 | 스펙/포맷 근거 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GD-1 | **행렬(matrix) 미조판** — 열 정렬 문자 `&`를 조판하지 않고 공백으로 취급 | `hwp-render/src/equation.rs:10`(미지원 명시), `:59`(`'&' => … 열 정렬(matrix) — v1은 공백 취급`) | §4.3.9.3 수식 스크립트 | 근사(공백 취급) | 렌더 | M |
| GD-2 | **큰연산자 극한 미배치** — `sum`·`int` 심볼은 나오나 아래·위 극한을 연산자에 붙여 배치하지 못함 | `hwp-render/src/equation.rs:10`(미지원 명시), `:216`(`sum`→∑), `:217`(`int`→∫) | §4.3.9.3 | 근사(첨자 배치) | 렌더 | M |
| GD-3 | **복잡 구분자 미지원**(크기 자동조절 괄호 등) | `hwp-render/src/equation.rs:10`(`복잡 구분자`) | §4.3.9.3 | 근사 | 렌더 | M |
| GD-4 | **EQEDIT 자체 속성 미파싱** — 수식 레코드(표 105)의 글자크기(HWPUNIT)·색상·baseline·버전정보·폰트이름을 IR `Equation`이 갖지 않음(script/크기/위치뿐). 렌더는 개체 상자 크기 역산으로 폰트 크기를 근사하므로 소스 지정값과 다를 수 있음 | `hwp-model/src/control.rs:270`(`Equation` 구조), `hwp5/src/body_text.rs:592`(`find_eqedit_script` — 스크립트만 추출) | §4.3.9.3 표 105 | 근사(역산) | 렌더·합성 | S~M |

**GD 교훈:** 세 항목 모두 **정품 수식 정답지**(정답지 α+β/2 정합처럼)로 조판 메트릭을 맞춰야
확정되므로 M. 왕복 자체는 스크립트 원문을 raw로 보존하므로([10](10-hwp5-structure-map.ko.md) 표 B
`EQEDIT`) 갭은 **렌더 경로에 국한**된다. 같은 언어(Rust) 구현체 rhwp가 `MATRIX`/`PMATRIX`/
`BMATRIX`/`DMATRIX` 조판을 이미 구현한 선례가 있어 참조 가능하다([08](08-external-research.ko.md)
생태계 대조).

---

## 5. GE — 변환 매트릭스 (방향별 손실)

포맷 간 **합성**에서만 나타나는 손실이다(왕복 아님). 두 부류로 나눈다: (§5.1) 합성 시 의도적
저하·상수 대체, (§5.2) `GE-α` — hwp5로는 보존되나 **hwpx 쓰기에서만** 손실되는 왕복 비대칭.

### 5.1 GE — 합성 방향 손실

| ID | 현상 | 근거 코드 | 스펙/포맷 근거 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GE-1 | **hwpx→hwp5 도형 의도적 저하** — 글상자는 텍스트를 본문으로 hoist하고 도형 래퍼 생략, 순수 장식은 드롭(무손실 gso 재합성 미확보) | `hwp5/src/write.rs:467`(`degrade_hwpx_gso`), `:510`(경고) | §4.3.9 개체 | 드롭(안전 저하) | 합성(hwpx→hwp5) | L |
| GE-2 | **이미지 바이너리 미발견 시 그림 드롭** — bin_ref가 가리키는 스트림을 못 찾으면 그림 생략 | `hwp5/src/write.rs:726`(`DROP: 이미지 바이너리 스트림을 찾지 못해 생략`) | §4.3.9.4 그림 | 드롭(소실) | 합성 | S |
| GE-3 | **colPr 단별폭·구분선 미수집** — 등폭·구분선 없음으로 가정, 불균등 단 손실 | `hwpx/src/read/section.rs:375`(`colSz·colLine 자식은 v1 미수집`), `:392` | §4.3.10.2 / `hp:colPr` | 드롭→상수 | 합성·렌더 | S |
| GE-4 | **pgnp 쪽번호 서식 DIGIT 고정** — 아라비아 숫자만 매핑, 그 외 형식(원문자·로마·가나다 등) 소실. **write도 대칭 결함**: hwp5-origin `g.data`의 서식 필드를 안 읽고 `formatType="DIGIT"` 고정(2026-07-19 감사 보강) | read `hwpx/src/read/section.rs:429`(`build_pgnp:415`) + write `hwpx/src/write/section.rs:307-325` | §4.3.10.9 / `hp:pageNum` | 근사(DIGIT 고정) | 합성(양방향) | S |
| GE-5 | **nwno 새 번호 종류 PAGE 고정** — 번호 값만 취하고 종류는 PAGE로 고정. **write도 대칭 결함**: `g.data[0..4]`(종류)를 버리고 `numType="PAGE"` 고정(2026-07-19 감사 보강) | read `hwpx/src/read/section.rs:473`(`build_nwno`) + write `hwpx/src/write/section.rs:343-352` | §4.3.10.6 / `hp:newNum` | 근사(종류 고정) | 합성(양방향) | S |
| GE-6 | **atno 자동번호 페이로드 상수** — 표준 12B 상수로 합성. **write도 대칭 결함**: `g.data`를 아예 안 읽고 `<hp:autoNum numType="PAGE"/>` 고정 — hwp5 원본이 그림/표/수식 번호(표 143)여도 전부 쪽번호로 오기록(2026-07-19 감사 보강) | read `hwpx/src/read/section.rs:465`(`build_atno`) + write `hwpx/src/write/section.rs:353-358` | §4.3.10.5 / `hp:autoNum` | 근사(상수) | 합성(양방향) | S |
| GE-7 | **번호 id 규약 이원화** — IR은 0-based 규약인데 hwp5 read만 on-disk 1-based를 그대로 올려(PR #8이 hwpx만 정규화) hwp5→hwpx에서 idRef dangling·off-by-one 발생했었음 | hwp5 `doc_info.rs parse_para_shape`(head 2/3 −1 정규화) ↔ `write.rs emit_para_shape`(+1 복원), 규약 주석 `hwp-model/header.rs` | — | ✅ **해소(2026-07-19)** — 경계 ±1 정규화 완료(경계 왕복 테스트 락). roundtrip 바이트 게이트는 fixture 전수에 head 2/3 부재 실측으로 안전 확인. 부수효과: hwp5 출신 번호목록이 이제 올바른 정의를 가리킴(기존 off-by-one 폴백) | 합성(hwp5→hwpx) | S |
| GE-8 | **페이지 걸침 표의 분할 실패(hwp→hwpx)** — 여러 페이지에 걸쳐야 할 긴 표가 변환본에서 분할되지 않고 페이지 하단을 뚫고 넘침(쪽번호 겹침·행 잘림). J1 실기 34쪽 전수 판정에서 발견(4·6쪽) — 페이지 걸침 표의 변환 실기는 최초(기존 A5A6 표는 단일 페이지라 미노출) | **원인 확정(정답지 삼각대조)**: pageBreak은 무죄(원본 attr=2=CELL 정합) — 진범은 **표 셀의 `linesegarray` 전면 드롭**. 한글은 쪽 경계 셀 분할에 줄별 세로위치가 필요(정품 원저작·한글 자신의 변환본 모두 셀 줄배치 100% 유지, 우리만 0). 수정: 표 셀 문단 줄배치 강제 방출(글상자 선례) + pageBreak/repeatHeader/noAdjust를 IR attr에서 방출(고정값은 합성 폴백만) | 스펙 표 75/76(bits0-1 분할 — "나누지않음" 표기 오탈), OWPML `hp:tbl@pageBreak` | ❌ **1차 수정 실기 실패(2026-07-19)** — linesegarray 가설 기각(방출해도 미분할 — 한글은 조판 재계산이라 캐시가 필요조건이 아니었음. 단 줄배치·attr 원본 충실 방출 자체는 정품 정합이라 유지). treatAsChar=1도 정품 실측상 무죄. → **진범 확정(표분할정답지 직대조, 2026-07-19)**: ① **treatAsChar 고정 1 방출**(원본은 표별 혼재 — 페이지 걸침 표는 0(부유). 글자처럼 취급 표는 "한 글자"라 분할 불가 → 관통) ② **sz height 행합산 재계산**(원본 공통 속성 값 무시 — 문제 표만 2배 이상 부풀림). pageBreak은 최종 무죄. 수정 완료(2026-07-19): TABLE 개체 공통 속성(표 69)을 `GsoPlacement`로 IR 승계 → hwpx `hp:pos`/`hp:sz` 방출(고정값·재계산은 합성 폴백만). **정답지 전수 대조 33/33 표 완전 일치**(treatAsChar·sz — 사용자 한글 저장본 기준). hwp5 identity 무영향(common_data raw 유지). ✅ **실기 확정(2026-07-19)** — 4·6쪽 표 정상 분할 확인, 3라운드(linesegarray 기각→정답지 직대조→승계 수정) 종결 | 합성(hwp5→hwpx)·실기 렌더 | M |
| GE-9 | **부유 그림 배치 소실(hwp5→hwpx)** — `parse_picture_gso`가 z_order·세로/가로 오프셋을 **하드코딩 0**으로 채우는데, hwpx write는 그 값을 그대로 floating `<hp:pos>`에 방출 → 떠 있는 그림이 원위치와 무관하게 좌상단(offset 0)에 뭉침. GE-8이 TABLE만 `GsoPlacement` 승계로 고치며 남은 비대칭 — 같은 해법 적용 가능 | `hwp5/src/body_text.rs:309-336`(`z_order: 0, vert_offset: 0, horz_offset: 0`) ↔ `hwpx/src/write/section.rs:1563`(그 값을 그대로 방출) | §4.3.9 표 69 | 근사(0 고정) | 합성(hwp5→hwpx) | S(GsoPlacement 재사용) |
| GE-10 | **개체 공통 후반부 미해석** — instance_id·쪽나눔 방지·**개체 설명문(접근성 대체텍스트)**이 전 개체 타입 공통으로 IR 필드 없음. hwp5 왕복은 common_data raw로 무손실이나 hwpx 합성에서 설명문이 항상 드롭 | `hwp-model/src/control.rs:462`(`GsoPlacement` 32B에서 중단), hwpx `write/section.rs:1558,1566`(desc 속성 미방출) | §4.3.9 표 69 | 드롭(합성) | 합성(hwp5→hwpx) | S~M |
| GE-11 | **FACE_NAME 부속 정보 합성 드롭** — 대체 글꼴(렌더 폴백에는 실사용)·PANOSE 10필드·기본 글꼴이 hwpx `write_fontfaces`에서 무참조 → hwp5→hwpx 변환본은 대체 글꼴 정보 상실. hwp5 소스의 `type_info: None` 하드코딩이 근본 원인 | hwp5 `doc_info.rs:174-211`(파싱 O, `fonts.rs:118` 렌더 사용) ↔ hwpx `write/header.rs`(무참조), `doc_info.rs:208`(`type_info: None`) | §4.2.4 표 20~22 | 드롭(합성) | 합성(hwp5→hwpx) | M(PANOSE→typeInfo 매핑 확정 필요) |
| GE-12 | **문서 시작번호 hwpx read 미파싱** — write는 `<hh:beginNum>`을 방출하지만 read에 파싱이 없어(전체 grep 0건) hwpx 소스의 페이지/각주/미주/그림/표/수식 시작번호가 IR에 안 올라옴 | write `hwpx/src/write/header.rs:53-59` ↔ read 부재 | §4.2.1 / `hh:beginNum` | 드롭(read) | 왕복(hwpx→hwpx)·합성(hwpx→hwp5) | S(GE-3 계열 동일 패턴) |
| GE-13 | **스타일 종류(PARA/CHAR) hwpx 양방향 무시** — read는 `type` 속성을 안 읽고(attr 항상 0) write는 `type="PARA"` 고정 → **글자 스타일이 항상 문단 스타일로 오기록**, hwpx→hwpx 왕복에서도 매번 유실 | read `hwpx/src/read/header.rs:589-597` ↔ write `hwpx/src/write/header.rs:527-538` | §4.2.11 표 48 / `hh:style@type` | 근사(PARA 고정) | 왕복(hwpx→hwpx)·합성 | S |

| GE-14 | **수식(eqed) hwpx 쓰기 드롭** — read는 `<hp:equation>`을(hwp5는 EQEDIT 스크립트를) IR `Equation`으로 올리지만 writer arm이 없어 generic fallback으로 빠짐 → hwpx를 편집·변환만 해도 수식이 통째로 사라짐 | write `hwpx/src/write/section.rs`(`write_equation`) ↔ read `hwpx/src/read/section.rs`(`parse_equation`), hwp5 `body_text.rs`(`parse_eqed`) | §4.3.9.3 / `hp:equation` | ✅ **해소(2026-07-27)** — run 직속 `<hp:equation>` + `hp:sz`/`hp:pos`/`hp:script` 방출(`수식_hwpx_왕복` 락). ⚠ 수식 전용 속성(version·baseLine·baseUnit·font)은 **정답지 미확보 표준 추정값** — 한글 실기 확정 대기 | 왕복(hwpx→hwpx)·합성(hwp5→hwpx) | S |
| GE-15 | **hp:script 엔티티·CDATA 유실(read)** — `read_element_text`가 `Event::GeneralRef`/`CData`를 무시해 `x &lt; y` 같은 수식 스크립트의 특수문자가 읽기에서 사라짐(`hp:t` 파서에는 있던 해석이 이 경로만 누락) | `hwpx/src/read/section.rs`(`resolve_entity` 공용 헬퍼 — `parse_text`와 공유) | XML 1.0 §4.6 미리 정의 엔티티 | ✅ **해소(2026-07-27)** — 참조 5종+숫자 참조 해석, CDATA 구획 수집. writer `esc()`와 짝을 이루는 역변환 | 왕복(hwpx→hwpx)·추출 | S |

| GE-16 | **속성값 엔티티 이중 이스케이프(hwpx read)** — `attr()`이 quick-xml raw 값을 그대로 올려(`A&amp;B` → IR `A&amp;amp;B`가 아니라 `A&amp;B` 원문), writer `esc()`가 다시 감싸 왕복마다 `&amp;`가 늘어남. 책갈피·필드·스타일 이름, 수식 `script` 속성 전부 해당 | `hwpx/src/read/xml.rs`(`attr` — unescape 적용) | XML 1.0 §4.6 | ✅ **해소(2026-07-27)** — 속성값을 읽는 즉시 엔티티 해석(해석 불가 참조는 원문 유지). 적대적 점검(codex) 발견 | 왕복(hwpx→hwpx)·합성 | S |
| GE-17 | **XML 비문자(U+FFFE·U+FFFF) 방출** — `esc()`가 C0 제어문자만 걸러 비문자를 그대로 흘림 → well-formed하지 않은 패키지가 되어 파서·한글이 **파일 전체를 거부** | `hwpx/src/write/templates.rs`(`esc`) | XML 1.0 §2.2 Char 범위 | ✅ **해소(2026-07-27)** — C0와 동일하게 제거(`esc_금지문자_제거`). 적대적 점검(codex) 발견 | 모든 hwpx 쓰기 | S |

> **2026-07-27 (GE-14~GE-17)**: 외부 기여 PR #7의 진단(각주/미주·수식 writer arm 부재)에서
> 각주/미주는 `e433462`(실측판)로 선행 해소됐고, 남은 수식 방출 arm + 엔티티 해석만 별도
> 구현. **적대적 점검(codex, 한컴 공식 OWPML 모델 소스 대조)** 으로 4건을 추가 수정했다:
> ① `lineMode="0"` → 열거값 `CHAR`(`enumdef.h` `g_EquationLineList` = LINE|CHAR, 기본 CHAR),
> ② hwpx 출신 수식의 비기본 속성·공통 자식 원문 pass-through(`Equation::raw_attrs`·`raw_props`
> — 이전엔 zOrder·textWrap·baseUnit·글자색·수식 글꼴·PAGE 기준 배치가 전부 기본값으로 재작성),
> ③ hwp5 출신 부유 수식 배치를 gso 공통 헤더에서 복원(`gso_pos_xml` 재사용 — 그림 GE-9와 동형),
> ④ GE-16·GE-17. 수식을 run당 개체 한도 카운트에도 포함시켰다(과소 계상 = 개체 유실).
>
> **잔여**: `<hp:equation>` 합성 경로의 수식 전용 상수(version·baseLine·baseUnit·font)는
> 여전히 **정답지 미확보 표준 추정값**이다(코퍼스에 수식 든 hwpx 부재). 한글 실기(`L1_수식.hwpx`,
> `docs/hancom-verification-checklist.md` §L)에서 수식 표시가 깨지면 정품 저장본 속성으로 교체할 것.
> read가 `hp:script`를 `trim()`하는 점(앞뒤 공백 비보존)도 미해결이나 조판 영향은 없다.

> **2026-07-19 (GK 배치)**: GK-1·GK-2 구현 — 정품 병합 표 1,816개 전수 실측으로 저장 구조
> 5규칙 확정(만장일치) 후 프리미티브 4종+CLI 4종+불변식 게이트. 정품 병합 표 회귀·왕복·세트
> 30/0(K1~K3 신설). 전체 테스트 322. **✅K 시리즈 실기 확정(2026-07-19)** — 병합·열 조작
> 편집 프리미티브 완결.

### 5.2 GE-α — hwpx 왕복 비대칭 (read는 해석, hwpx write만 손실)

특수 부류. 아래 속성은 read가 IR로 **정확히 해석**하므로 `hwp5`로는 나간다. 그러나 hwpx writer가
상수/미방출로 눌러 **`hwpx→hwpx` 왕복에서만** 사라진다([11](11-hwpx-structure-map.ko.md) §5(b)).
공통 원인은 `write/header.rs`의 국소 상수화이므로 **한 파일 수정으로 독립 복원** 가능한 게 특징이다.

| ID | 속성 | 근거 코드 (read ↔ write) | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|
| GE-α1 | 글자 **그림자**(charPr shadow) | read `hwpx/src/read/header.rs:245` ↔ write `write_char_properties` | ✅ **해소(2026-07-15)** — IR 기반 방출 | 왕복(hwpx→hwpx)·합성 | S |
| GE-α2 | 글자 **외곽선**(charPr outline) | read `read/header.rs:259` ↔ write 동상 | ✅ **해소(2026-07-15)** | 왕복(hwpx→hwpx) | S |
| GE-α3 | **양각·음각**(emboss/engrave) | read `read/header.rs:266,271` ↔ write 동상 | ✅ **해소(2026-07-15)** | 왕복(hwpx→hwpx) | S |
| GE-α4 | **위·아래 첨자**(supscript/subscript) | read `read/header.rs:234,239` ↔ write 동상 | ✅ **해소(2026-07-15)** | 왕복(hwpx→hwpx) | S |
| GE-α5 | **밑줄 모양**(underline shape) | read `read/header.rs:204`(IR `underline_shape` 신설) ↔ write 동상 | ✅ **해소(2026-07-15)** | 왕복(hwpx→hwpx) | S |
| GE-α6 | **그러데이션 중심·step** | read `read/section.rs:1217`(`parse_gradation`, angle만) ↔ write `write/section.rs:764`(center/step 상수) | 근사(중심·단계 상수) | 왕복(hwpx→hwpx)·렌더 | M |
| GE-α7 | **번호 형식**(numbering paraHead) | read `read/header.rs:333` ↔ write `write_numberings` | ✅ **해소(2026-07-15)** — `numbering_levels` 기반, 다중 번호정의 itemCnt도 수정 | 왕복(hwpx→hwpx) | S |
| GE-α8 | **문단↔번호 연결**(paraPr heading) — read는 해석(attr1 bits23-27 + numbering_id)하나 write가 `type="NONE"` 고정이었음 | read `read/header.rs:309` ↔ write `write_para_properties` | ✅ **해소(2026-07-15 2차)** — OUTLINE/NUMBER/BULLET 역방출, 실기(C6)에서 발견된 결함 | 왕복(hwpx→hwpx)·합성 | S |
| GE-α9 | **머리말/꼬리말 적용쪽(applyPageType)** — read는 BOTH/EVEN/ODD를 정확히 해석(hwp5-origin도 raw 8B에 적용쪽 보존)하나 write가 `applyPageType="BOTH"` 상수 방출 → 홀·짝 구분 머리말/꼬리말이 hwpx 왕복·합성에서 전부 "양쪽"으로 오기록. **hwpx 파일 자체의 훼손**이라(한글에서 열어도 동일 증상) GG-16(렌더 무시)보다 심각. 서적형 홀짝 머리말은 공문서·논문 빈출. 2026-07-19 스펙 전수 감사 발견 | read `read/section.rs:506-517`(`head_foot_data`) ↔ write `write/section.rs:863-901`(`:879` 상수) | ✅ **해소(2026-08-14 문서 정합 반영 — 코드는 5db1c6a에서 수정)** — write가 `header_footer_apply_page`(`write/section.rs:907-918`)로 보존된 적용쪽을 방출 | 왕복(hwpx→hwpx)·합성 | S |

> **잔여 소갭(α5 관련):** 밑줄 모양 중 **물결(WAVE)**은 reader `line_type_code`에 매핑이 없어
> SOLID로 강등된다 — 점선·이중선 등은 정상 왕복. C 시리즈 실기 세트 제작(2026-07-15) 중 발견.

### 5.3 GE-β — IR 되쓰기 부속 데이터 손실 (2026-07-08 재수색 추가)

또 하나의 특수 부류. 본문 레코드가 아닌 **부속 스트림·메타데이터**가 IR에 올라오지 않아, 편집을
거치는 **IR 경유 되쓰기**(read→IR→write)에서 상수/재생성으로 대체되는 손실이다. §0.3의 "Opaque
무손실"과 달리 이들은 Opaque 보존조차 안 되므로 **같은 포맷 되쓰기에서도 소실**된다(무수정 identity
재직렬화는 바이트 복사라 무관). 참고: PrvText(미리보기 **텍스트**)는 매번 본문에서 재생성되므로
stale 갭이 아니다 — 갭은 아래 항목들이다.

| ID | 대상 | 근거 코드 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|
| GE-β1 | **미리보기 이미지(PrvImage / Preview/PrvImage.png)** — ✅ HWPX raw pass-through는 2026-08-14 해소. HWP read는 아직 원본 preview를 IR로 포착하지 않고 writer는 명시적으로 렌더한 option만 사용 | hwp5 `write.rs`(opts 제공 시만), hwpx `read/mod.rs` + `write/mod.rs`, `patch.rs` | HWPX 보존, HWP 재생성 또는 생략 | HWP 되쓰기 | S(렌더러 재활용 시) |
| GE-β2 | **Scripts(매크로)** — 원본 JScript를 버리고 한글 빈 문서 표본 상수로 대체 | hwp5 `write.rs:213-221`(표본 바이트 상수), hwpx `patch.rs:4` | 드롭→상수 | 되쓰기 | S |
| GE-β3 | **DocOptions 부속 스트림** — `_LinkDoc`은 524B 0 상수, DRM·서명 6스트림은 미방출 | `write.rs:208-210`, [10](10-hwp5-structure-map.ko.md) §1 | 드롭/상수 | 되쓰기 | M |
| GE-β4 | **요약정보 필드 소실** — 작성/수정일시·마지막저장자·설명 | `summary.rs`·`write.rs`·`hwp-model/src/document.rs`·hwpx `templates.rs` | ✅ **해소(2026-07-15)** — Metadata에 description/last_saved_by/create_time/modify_time(raw FILETIME u64) 추가, read/write 왕복. 인쇄일시·통계는 잔존(기본값 방출) | 되쓰기 | S |
| GE-β5 | **hwpx settings.xml·version.xml** 상수 대체 | `Document.hwpx_settings_xml/hwpx_version_xml` 원문 pass-through(없으면 기존 상수), JSON 왕복 포함 | ✅ **해소(2026-07-15 3차)** — `hp:switch`(section 내부)는 잔존 | 되쓰기 | S |
| GE-β6 | **임베디드 폰트** — `isEmbedded="0"` 하드코딩, 폰트 BinData·hwp5 typeInfo 소실 | hwpx `write/header.rs:84,98,105`, `read/header.rs:132-135`, hwp5 `doc_info.rs:201`(`type_info: None`) | 드롭(플래그·바이너리) | 되쓰기·렌더 | M |
| GE-β7 | **XMLTemplate 스토리지 침묵 소실** — FileHeader bit5는 표시용 디코드뿐, read 화이트리스트·write 스토리지 시퀀스 양쪽에 `/XMLTemplate` 부재 → IR 경유 되쓰기에서 **경고 없이** 드롭(전자정부 서식 등 XML 스키마 바인딩 문서). 2026-07-19 스펙 전수 감사 발견 | `hwp5/src/read.rs:20-84`(화이트리스트), `write.rs:168-228`(생성 시퀀스 부재), `file_header.rs:64,166`(비트만) | §3.2.10 | 드롭(되쓰기·무경고) | 되쓰기 | S(raw pass-through 슬롯) |
| GE-β8 | **DocHistory 스토리지 침묵 소실** — 문서 이력(잠금 버전·날짜·작성자·설명·DiffML·최근본)이 되쓰기에서 **경고 없이** 통째 소실. §4.4 레코드 8종은 별도 태그 공간인데 상수·파서 전무. 2026-07-19 스펙 전수 감사 발견 | `read.rs`·`write.rs` `/DocHistory` 전무(grep 0건), `tag.rs` §4.4 태그 공간 부재, `file_header.rs:65,167`(비트만) | §3.2.11·§4.4 | 드롭(되쓰기·무경고) | 되쓰기 | S(보존)/M(내용 해석 — LASTDOCDATA 플래그 값은 스펙 미기재라 정품 실측 필요) |

**GE 교훈:** GE-1(도형 저하)은 07§F1과 같은 뿌리(gso 무손실 재합성 미확보)라 L이다. 반면
**GE-α 전체는 정답지 없이 자료구조만으로 풀 수 있는 저비용 항목**이다 — read가 이미 해석하고
있으니 write에 대응 요소만 방출하면 된다. `write/header.rs` 국소 수정으로 독립적이며, GA~GD·GG의
어떤 것에도 의존하지 않는다(→ §14 의존 그래프에서 "즉시 착수 가능" 노드). **GE-β는 "충실도 보존
fill"(`patch.rs`)이 이미 우회 경로**임에 유의 — hwpx 한정으로 패키지를 통째 보존하며 텍스트만
치환하므로, GE-β가 문제되는 것은 구조 편집이 필요한 IR 경유 경로뿐이다. 근본 해법은 IR에
"부속 스트림 pass-through" 슬롯을 추가하는 것(대부분 S).

---

## 6. GF — 필드·양식

필드는 12종 전수 온디맨드 파싱되지만([10](10-hwp5-structure-map.ko.md) §6.2), 생성·해석 범위에 갭이 있다.

| ID | 현상 | 근거 코드 | 스펙/포맷 근거 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GF-1 | **미지 필드 %unk 폴백** — 매핑 안 되는 필드 종류·OWPML type을 `%unk`/`UNKNOWN`으로 뭉갬 | `hwp-convert/src/field.rs:69`(`_ => "UNKNOWN"`), `:87`(`_ => *b"%unk"`), `:104` | §4.3.10.15 / `fieldBegin@type` | 근사(폴백) | 왕복·합성 | S |
| GF-2 | **찾아보기 표식·덧말·글자겹침 미해석** — 의미 파싱 없이 Generic으로만 보존(문단 리스트 없는 마커성 컨트롤 — 숨은설명은 콘텐츠 손실이라 GF-5로 분리, 2026-07-19) | hwpx `read/section.rs:597`(미지 ctrl → 코드 21 Generic), [10](10-hwp5-structure-map.ko.md) §6.1 각주 | §4.3.10.10·§4.3.10.12·§4.3.10.13 | Generic 보존(미해석) | 합성·렌더 | M |
| GF-3 | **신규 필드 생성 제약** — 기존 이름의 값만 채울 수 있고 새 필드 생성 없음. 편집 생성은 `%clk`·`%hlk`·`%bmk`/`bokm`만 | `hwp-convert/src/field.rs`(생성 지원 종류 한정), [README](../README.ko.md) §범위와 한계(`신규 필드 생성은 없다`) | §4.3.10.15 | 미구현(생성) | 편집 | M |
| GF-4 | **필드 22/34종 미인식 → hwp5→hwpx에서 필드 통째 DROP** — `is_field_ctrl_id`가 스펙 표 128의 34종 중 12종만 인식. 변경추적 필드 19종(`FIELD_REVISION_*`)·메모(`%%me`)·개인정보 보안(`%cpr`)·차례(`%toc`)는 어떤 write arm에도 안 걸려 catch-all DROP → **필드 전체 소실**(GF-1의 "종류 뭉갬"보다 상위 손실 등급). hwp5→hwp5는 무손실(`is_field_ctrl_id`를 hwp5 crate가 안 씀). 2026-07-19 스펙 전수 감사 발견 | `hwp-convert/src/field.rs:37-53` ↔ 게이트 사용처 `hwpx/src/write/section.rs:271`, catch-all `:386-391` | §4.3.10.15 표 128 | 드롭(합성) | 합성(hwp5→hwpx) | S(인식 목록 확장+fieldBegin type 매핑) |
| GF-5 | **숨은설명(tcmt) 본문 소실(hwp5→hwpx)** — GF-2 그룹 중 유일하게 문단 리스트를 갖는 컨트롤. hwp5 read는 폴백으로 본문 문단까지 IR에 담아오지만 hwpx write에 전용 arm이 없어 catch-all DROP → **마커가 아니라 텍스트 콘텐츠가 사라짐**(fn/en arm 선례로 방출 가능). 2026-07-19 스펙 전수 감사 발견 | hwp5 `body_text.rs:625-676`(`collect_paragraph_lists` 폴백) ↔ hwpx `write/section.rs:386-391`(catch-all DROP) | §4.3.10.14 | 드롭(콘텐츠) | 합성(hwp5→hwpx) | S |

**GF 교훈:** GF-1은 폴백이 있어 파일이 깨지진 않으나 종류 정보가 뭉개진다(S). GF-2의 겹침·덧말은
GB-10 계열과 접하며(제어문자 23), 의미 렌더를 하려면 정답지가 필요하다(M).

---

## 7. GG — 렌더 정밀도

### 7.1 07§F 승계

07§F가 **조사 서사**로 다룬 미해결 이슈를 여기서 카탈로그 항목으로 승계한다. **서사는 07이 정본**
이며 여기서는 요약+링크만 둔다(재서술 금지 원칙, §0.1).

| ID | 현상 | 근거 코드 | 상태·방향 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|---|
| GG-1 (=07§F1) | **글상자 드롭** — 왕복 hwp에서 글상자 박스 자체 소실(텍스트는 본문 hoist로 보존) | `hwp5/src/write.rs:467`(`degrade_hwpx_gso`) | [07§F1](07-hangul-compat-rules.ko.md) 승계. 근본 해결은 SHAPE_COMPONENT 239B **속성 충실도** 확보 필요 | 드롭(안전 저하) | 합성(hwpx→hwp5) | L |
| GG-2 (=07§F2) | **페이지 오버플로** — 합성 멀티페이지 세로 넘침(md는 content_h 리셋으로 방어) | `hwp-render/src/lineseg.rs`(`synthesize_linesegs`) | [07§F2](07-hangul-compat-rules.ko.md) 승계. 줄배치 속성 충실도가 유력 원인 | 근사 | 렌더·합성 | L |
| GG-3 (=U2) | **양쪽정렬 근사** — 양쪽(0)·배분(4)·나눔(5)을 동일 처리, 공백 우선 분배, 마지막 줄 미적용 | `hwp-render/src/layout.rs`(`align_line`/`justify_line`), [05](05-rendering.ko.md) §1.4 | 0은 공백 우선·마지막 줄 제외 유지, 4(배분)는 후행 gap 포함·마지막 줄 적용, 5(나눔)는 후행 gap 제외·마지막 줄 적용 | ✅ **해소(2026-08-14, PR 7)** — 4/5의 마지막 줄 의미론은 한컴 확인 대상 | 렌더 | M |
| GG-4 (=U4) | **자간 근사** — `spacing_pt = size_pt × spacings[lang]/100`를 pt 실수로 단순 적용 | `hwp-render/src/shape.rs`(`letter_spacing_pt`), [05](05-rendering.ko.md) §3.2 | HWPUNIT 정수 도메인 half-up 반올림(상대 크기·첨자 체인 포함) 후 pt 변환 | ✅ **해소(2026-08-14, PR 7)** — 줄 끝 자간 계상 여부는 한컴 확인 대상 | 렌더 | M |

**U1·U3에 대하여:** 00§5는 "U2(양쪽정렬)·U4(자간)"만 명명한다. `U1`·`U3`은 docs 전체와 git 이력
어디에도 정의가 없어(추측 금지 원칙) **의도적으로 제외**했다. U-계열이 U1~U4 완전 열거로 확정되면
이 표에 추가한다.

**GG 교훈:** GG-1·GG-2는 07§F의 관통 가설("속성 충실도가 충분히 높으면 자연 해소")을 그대로
따른다 — 정답지 확보 + 실기 반복(L)이 유일한 길. GG-3·GG-4는 렌더 국소지만 정품 렌더와의
픽셀 대조(M)가 있어야 확정된다.

### 7.2 렌더 속성 갭 (2026-07-08 재수색 추가)

`crates/hwp-render/` 전수 재수색으로 확정한, IR에는 있으나(또는 raw에 보존돼 있으나) 렌더가
반영하지 않는 속성들. 영향 경로는 전부 **렌더**다(별도 표기 없으면).

| ID | 현상 | 근거 코드 | 현 동작 | 난이도 |
|---|---|---|---|---|
| GG-5 | **셀 테두리 선 종류 무시** — `BorderLine.line_type`(점선·이중선) 미반영으로 모든 셀 테두리가 실선 1줄이었음 | `hwp-render/src/border.rs`(`border_strokes`), `layout.rs`(`draw_table_rows`) | ✅ **해소(2026-08-13, PR 5)**: 점선 계열(코드 2~7)은 `Stroke.dash`, 이중선 계열(8~11)은 오프셋 병렬 선으로 렌더(굵기 분할은 근사 — 한컴 라운드 확인 대상). `Item::Line` 삭제, 모든 테두리 emit은 `Item::Path` | S |
| GG-6 | **문단 테두리 선 종류 무시** — GG-5와 같은 뿌리, 경로만 다름 | `layout.rs`(`draw_para_bg_slice`) | ✅ **해소(2026-08-13, PR 5)**: GG-5와 같은 `border_strokes` 헬퍼 | S |
| GG-7 | **셀·문단 배경 무늬(hatch)·그러데이션 무시** — `BorderFill`이 단색 `bg_color`만 모델링(무늬·그러데이션은 tail raw)이었음 | `hwp-model/src/header.rs`(`hatch`/`gradient`), `hwp-render/src/display.rs`(`Fill::Hatch`), `layout.rs`(`bg_fill_item`) | ✅ **해소(2026-08-14, PR 8)**: hwp5 tail 파싱(무늬색/종류, 그러데이션 블록) + hwpx `hc:gradation` 왕복. 셀·문단·글자 배경이 `Item::Path`의 `Fill::Gradient`/`Fill::Hatch`로 세 백엔드 렌더(png 선분, svg `<pattern>`, pdf 평탄 선). 무늬 간격/굵기는 근사. hwpx는 무늬 종류 표현 불가(색만) | M |
| GG-8 | **강조점(dot emphasis) 미렌더** — `CharShape.attr` bits 21~24가 보존되나 접근자·렌더 모두 없었음 | `hwp-model/src/header.rs`(`emphasis_kind`), `hwp-render/src/layout.rs`(`push_run`) | ✅ **해소(2026-08-13, PR 6)**: 13종 전부(DOT_ABOVE~DOT_BELOW, hwplib `EmphasisSort` 순서) 글리프별 렌더. hwpx `symMark` 왕복. spec rev1.2 표 35와 hwplib의 3/4 TILDE/CARON 순서 충돌은 한컴 눈대조 대상 | S |
| GG-9 | **밑줄 모양(이중·점선·물결)·'글자 위' 밑줄 미렌더** — kind==1(아래)만 인식, 모양 비트(4~7) 접근자 없음 | `hwp-model/src/header.rs`(`underline_shape_code`), `hwp-render/src/border.rs`(`decor_strokes`) | ✅ **해소(2026-08-13, PR 6)**: kind==3(위) 렌더 + 0-기반 모양 코드(solid~double-wave, hwplib `BorderType2`) 적용 — 점선 계열은 `Stroke.dash`, 이중·가중은 오프셋 스트로크, 물결은 cubic 경로. 위 밑줄 y·물결 상수는 한컴 라운드 확인용 플레이스홀더 | S |
| GG-10 | **취소선 모양 무시** — 이중 취소선 등 미반영, 실선 1줄 고정 | `hwp-render/src/shape.rs`(`strike_shape`), `border.rs`(`decor_strokes`) | ✅ **해소(2026-08-13, PR 6)**: bits 26~29(B8 관측 기반)가 GG-9와 같은 장식 스트로크 표를 구동. hwpx 취소선 모양 왕복. 3D 코드는 실선 강등 | S |
| GG-11 | **글자 그림자 오프셋 무시** — `CharShape.shadow_gap` 미사용, 고정 대각 오프셋(0.05~0.06em) | `hwp-model/src/header.rs`(`shadow_gap`), `hwp-render` `png.rs`/`pdf.rs`/`svg.rs` | ✅ **해소(2026-08-13, PR 6)**: 세 백엔드 모두 축별 `size_pt * gap/100` 적용. (0,0)이면 종전 0.06em 폭백 유지(정품 기본값은 한컴 확인 대상) | S |
| GG-12 | **개요(outline) 번호 부분 지원** — 기본 head_type 1 마커는 렌더하지만 사용자 정의 개요·재시작과 확인된 한글 14자 이후 순번은 모델링 및 정답지 검증 전 | `hwp-model/src/list.rs`(`ListState::marker_for_render`), `hwp-render/src/layout.rs` | 근사(기본 7수준 형식, 빈 문단·글상자 범위 지원) | M |
| GG-13 | ~~**쪽번호 미렌더** — 페이지 카운터 부재, pgnp/atno 컨트롤은 skipped 집계 후 미렌더~~ | `hwp-render/src/page_number.rs`, `layout.rs`(`PageNumberState`), `shape.rs`(`shape_range_page`) | ✅ **해소(2026-07-30)** — 시작·재시작·숨김, pgnp 위치/장식/지원 서식, PAGE atno 동적 치환. 미지원 서식은 십진 경고 폴백; GE-4는 별도 잔존(GG-16은 PR 9 해소) | M |
| GG-14 | ~~**미주(endnote) 배치 근사** — 문서/구역 끝이 아니라 **앵커 페이지 하단**에 각주와 동일 렌더(GC-3의 '모양'과 별개인 '위치' 문제)~~ | `hwp-render/src/footnote.rs:35-72`, `layout.rs`(노트 수집 시 kind 분리, 구역 끝 플러시) | ✅ **해소(2026-08-14, PR 9)** — 레이아웃이 페이지 노트를 `NoteKind`로 분리: 각주는 페이지별 하단 플러시 유지(예약 높이도 각주 전용), 미주는 구역 전체에 누적해 마지막 본문 뒤 마무리 블록으로 렌더(공간 부족 시 새 쪽) | M |
| GG-15 | **이미지 회전·자르기(imgClip)·반전·밝기/대비·그림 효과 미렌더** — `Item::Image`에 변환 필드가 없고 그림 효과(표 108~116)는 미해석이었음 | `hwp-model/src/control.rs`(`Picture`), `hwp-render/src/display.rs`(`Item::Image`), `layout.rs`(Picture emit) | ⚠ **부분 해소(2026-08-14, PR 8)**: 회전/반전/자르기/밝기/대비를 파싱(hwp5 레코드 오프셋 + hwpx 속성)해 세 백엔드 렌더(png Transform+선행 자르기, pdf 행렬+clip, svg transform+clipPath). 잔여: 그림 효과는 파싱 후 `picture_effects_unsupported` 경고로 보고(렌더 안 함), hwp5 반전 비트 미확정, 밝기/대비 픽셀 맵은 선형 근사 | M |
| GG-16 | ~~**머리말/꼬리말 홀수/짝수/첫쪽 구분 무시** — 최초 head/foot 하나를 모든 페이지에 반복(GC-7 구역 EVEN_ADJUST와 별개)~~ | `layout.rs`(`head_foot_apply`, `select_furniture`) | ✅ **해소(2026-08-14, PR 9)** — 모든 head/foot 컨트롤을 적용쪽 값(data bits 0-1: BOTH/EVEN/ODD)과 함께 수집하고 출력 쪽번호 패리티로 선택(폴백 BOTH→첫 항목 — 단일 머리말 구역 동작 불변). 첫쪽 전용 furniture는 소스 포맷 표현이 없어 근사로 잔존 | S |
| GG-17 | **단 구분선 미렌더** — `ColumnDef.divider`가 양쪽 reader에서 드롭되고 렌더러도 미사용이었음 | `hwp-model/src/control.rs`, `hwpx/src/read/section.rs`(`colLine`), `hwp-render/src/layout.rs` | ⚠ **부분 해소(2026-08-13, PR 5)**: hwpx `hp:colLine` 읽기·쓰기 + 단 사이 구분선 렌더. 보류: hwp5 coldef 구분선 파싱(바이트 오프셋 미확정 — 로컬 픽스처 전부 동일한 단일 단 COLDEF, `hwp5/src/body_text.rs`의 `TODO(GG-17)`), hwpx → hwp5 합성 방향의 구분선 | S |
| GG-18 | **줄간격 모델 근사(합성 한정)** — attr1&0x3로 판정, 고정(1)·최소(3)를 동일 처리, 여백만(2)을 비율로 오해. 실파일은 캐시 lineseg라 무관 | `hwp-render/src/lineseg.rs`(`line_advance_hu`, `compute_linesegs`), `hwp-model/src/header.rs`(`line_spacing_type`) | ✅ **해소(2026-08-14, PR 7)**: 버전 인식 `line_spacing`/`line_spacing_type` — 비율 `base*v/100`, 고정 정확히 `v/2`(클램프 제거), 여백만 `base + v/2`, 최소 `max(base, v/2)`(줄별 자연 높이는 `base` 근사, 한컴 확인 대상) | M |
| GG-19 | **금칙처리·외톨이줄 보호·한 줄 입력 등 ParaShape 속성 비트 다수 미지원(합성 한정)** — 그리디 줄바꿈만. 범위 확장(2026-07-19 감사): attr1의 줄나눔 기준·공백 최소값·다음 문단과 함께·문단 보호·앞쪽 쪽나눔·**세로 정렬**·테두리 연결·여백 무시·문단 꼬리 모양(표 44), attr2의 한영/한글숫자 간격 자동 조정(표 45)도 접근자 없이 raw 왕복만 | `lineseg.rs:301-333`, `hwp5/src/doc_info.rs:269-319`(attr1 접근자 3종뿐) | 근사(합성 경로) | M |
| GG-20 | **인라인 제어문자 폭 무시** — 고정폭 빈칸·하이픈·묶음 빈칸 등이 폭 계산에 미반영 | `hwp-render/src/shape.rs`(조각 루프, `fw_space_run`), `layout.rs`·`lineseg.rs`(줄바꿈 guard) | ✅ **해소(2026-08-14, PR 7)**: HYPHEN은 실제 `-` 셰이핑. NB_SPACE는 식별 가능한 U+00A0 source를 보존하면서 일반 공백 폭으로 셰이핑하고 양옆 줄바꿈을 금지. FW_SPACE는 고정 1em 폭 | S |
| GG-21 | **hwp5 직접렌더 경로 도형 선종류·화살촉 미적용** — `dash_pattern`/`arrowheads`가 hwpx ShapeGeom 경로에만 연결되고 hwp5 raw 경로는 `Stroke::solid` 고정이었음 | `hwp-render/src/shape_draw.rs`(`hwp5_line_style` + `draw_component` SC_LINE arm) | ✅ **해소(2026-08-13, PR 5)**: raw 경로가 선 속성을 `hwp5_line_style`로 매핑해 `dash_pattern` 적용 + `arrowheads` 방출(시작·끝 비트는 유무로 평탄화 — hwpx 경로와 동일). 화살촉 **모양·크기**는 양 경로 모두 고정 삼각형 유지 | S |
| GG-22 | **글자 단위 테두리/배경 미렌더** — `CharShape.border_fill_id`는 hwp5·hwpx 파싱/왕복 완비인데 렌더가 무참조(렌더의 border_fill_id 참조는 전부 ParaShape 경로) | `hwp-model` CharShape ↔ `hwp-render/src/layout.rs`(`push_run` — 종전 무참조) | ✅ **해소(2026-08-13, PR 6)**: 런 단위 배경 Rect(글리프보다 먼저 emit) + 4변 테두리(`border_rectangle_items`) — 문단 배경 로직 재사용. 상자 메트릭(y-0.80em~y+0.25em)은 한컴 라운드 확인용 플레이스홀더 | S~M |
| GG-23 | **타원→호 변환 미해석** — 타원 레코드(표 96) 60B 중 28B만 읽어 변환된 타원이 항상 완전한 타원으로 렌더됐음 | `hwp-render/src/shape_draw.rs`(SC_ELLIPSE, `ellipse_arc_path`) | ✅ **해소(2026-08-14, PR 8)**: 호 변환 플래그(attr bit1)·start/end 좌표·호 종류(bits 2~9)를 읽고 `arc_path`의 축 벡터 일반화로 호/부채꼴/현을 렌더. 종류 값 매핑(0/1/2=호/부채꼴/현)과 sweep 방향은 한컴 정답지 대기 | M |
| GG-24 | **대각선 테두리 선 종류·BORDER_FILL 효과 비트 미렌더** — 대각선 자체는 `diagonal_dirs` 패스 이후 렌더되나 `line_type`은 드롭, attr의 3D·그림자 효과 비트는 raw 보존 뿐 | `hwp-model` BorderFill.diagonal ↔ `hwp-render/src/border.rs` | ⚠ **부분 해소(2026-08-13, PR 5)**: 대각선도 `border_strokes`로 `line_type` 반영(GG-5와 같은 헬퍼). 3D·그림자 효과 비트는 raw 보존 유지 | S(빈도 낮음) |

> 재수색에서 **갭이 아님**으로 확인된 것(오보고 방지): 장평(x_scale), 양각/음각/외곽선/글자그림자
> on-off, 셀 세로정렬·셀 여백·자동 행높이, 위/아래 첨자·글자 음영·밑줄 색 — 전부 렌더됨.
> GE-α1~α3는 hwpx **write 왕복** 전용 갭이지 렌더 미지원이 아니다.

---

## 8. GH — 내보내기(md/HTML/ODT) 손실 (2026-07-08 재수색 추가)

IR→텍스트 포맷 출력에서 잃는 것들. `hwp-convert/src/{markdown,html,odt}.rs`가 대상이다.

| ID | 현상 | 근거 코드 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|
| GH-1 | **하이퍼링크 URL 드롭(md/html)** | `markdown.rs`·`html.rs`·`field.rs`(`hyperlink_url` 헬퍼 신설) | ✅ **해소(2026-07-15)** — md `[표시](URL)`, html `<a href>`, md 왕복 보존 테스트 | 내보내기 | S |
| GH-2 | **이미지 드롭(md/html)** | `markdown.rs`(`MarkdownOptions.media_dir`)·`html.rs`·`image.rs`(`image_kind` 헬퍼) | ✅ **해소(2026-07-15)** — html=data URI 임베드, convert .md=`<스템>.media/` 사이드카 추출(cat stdout은 기존 유지) | 내보내기 | S |
| GH-3 | **각주/미주가 마커 없이 본문 인라인 흡수(md/html/odt 공통)** — `[^n]`·`<text:note>` 미사용 | `markdown.rs`, `html.rs`, `odt.rs:181-199` | ✅ **md 해소(2026-07-18)** — 본문 `[^N]`/`[^eN]` 마커 + 문서 끝 정의(GFM 풋노트). ✅ **html 해소(2026-08-01)** — `<sup id="fnref-N">` 앵커 + `<section class="footnotes">` 정의(표현 전용). ✅ **odt 해소(2026-08-01)** — `<text:note>`(citation+body). 전 경로 해소 | 내보내기 | S |
| GH-4 | **병합 셀 평탄화** — col_span/row_span을 어떤 출력도 반영 안 함(colspan/rowspan·columns-spanned 미방출) | `markdown.rs`, `html.rs`, `odt.rs:203-243` | ✅ **md 해소(2026-07-18)** — 병합 셀 있으면 HTML `<table>`(colspan/rowspan) 폴백 → 단, GFM 표 유지는 무병합 표만. ✅ **html 해소(2026-08-01)** — 점유 격자로 colspan/rowspan 방출, from_html이 역산(계약 18). **실기 확정(P1)**: 부분 조합 산출물의 colspan/rowspan 병합 표가 한글에서 정상 표시. ✅ **odt 해소(2026-08-01)** — number-columns/rows-spanned + covered-table-cell. 전 경로 해소 | 내보내기 | S |
| GH-5 | **셀 내 블록(중첩표·이미지) 드롭** — 셀은 인라인 텍스트만 취하고 블록 버퍼 폐기 | `odt.rs:215`(blk 폐기), `markdown.rs`, `html.rs` | ✅ **md 해소(2026-07-18)** — 중첩 표·블록 수식 감지 시 HTML 표 폴백, 셀 fragment를 등장 순서대로 보존하고 이미지도 안전하게 참조. html/odt는 기존 대신 ✅ **html 해소(2026-08-01)** — 셀 블록 버퍼 폐기 제거, 중첩 표·그림 보존(중첩 표 단위 테스트 포함). ✅ **odt 해소(2026-08-01)** — 셀 블록 보존. 전 경로 해소. ⚠대조 감사 노트: 코드 분기(`Control::Table => true`)는 확인됐으나 "셀 안 실제 중첩 표" 전용 단위 테스트는 부재(수식·이미지로 간접 검증만) | 내보내기 | M |
| GH-6 | **리스트 평문화(md)** — 헤딩만 인식, 글머리표/번호 문단을 `- `/`1. ` 구문으로 복원 안 함 | `markdown.rs` + `hwp-model/src/list.rs`(render에서 이동, SSOT) | ✅ **해소(2026-07-18)** — `- `/`N. ` 목록 + 부모 마커 폭 기준 들여쓰기, 정의별 번호 카운터와 구역별 재시작, 번호 형식 합성(숫자 외는 리터럴 마커) | 내보내기 | S |
| GH-7 | **ODT 페이지 레이아웃 미재현** — 여백·다단·머리말 위치 생략(모듈 주석에 명시) | `odt.rs:3-5` | 근사(생략) | 내보내기 | M |
| GH-8 | **수식·글자효과 드롭(md)** — eqed 스크립트 미방출, 밑줄/취소선/위·아래첨자 평문화 | `markdown.rs` | ✅ **해소(2026-07-18)** — 수식 인라인 `$..$`/블록 `$$..$$`(HWP 스크립트 원문), `<u>`·`~~`·`<sup>`·`<sub>` 스팬 | 내보내기 | S |

## 9. GI — 들여오기(markdown/JSON) 한계 (2026-07-08 재수색 추가)

| ID | 현상 | 근거 코드 | 현 동작 | 영향 경로 | 난이도 |
|---|---|---|---|---|---|
| GI-1 | **GFM 확장 미파싱**(취소선·각주) | `from_markdown.rs` — STRIKETHROUGH·FOOTNOTES 활성, 취소선→strike run, `[^N]`/`[^eN]`→각주/미주 컨트롤 역생성(#8 내보내기 구조와 대칭) | ✅ **해소·실기 확정(2026-07-19)** — md→IR→md·hwpx·hwp5 왕복 폐쇄, H1·H2 실기 통과(각주·취소선 포함). 작업목록(TASKLISTS)은 IR 대응 부재로 의도적 제외 | 들여오기 | S |
| GI-2 | **순서·중첩 리스트 뭉개짐** | `from_markdown.rs` — 순서=NUMBER heading+번호정의(start 보존), 글머리=BULLET, 중첩=head_level. IR 번호 참조 0-based 규약 확립 | ✅ **해소(2026-07-19)** — 왕복 폐쇄(start=3도 보존). 잔여: hwp5 저장 시 start가 1로 리셋(NUMBERING 바이트에 start 인코딩 후속), 각주 안 목록 v1 제외 | 들여오기 | S |
| GI-3 | **markdown 이미지 `![alt](url)` 드롭** | `from_markdown_with(MarkdownImportOptions{base_dir})` — 로컬 경로 임베드(인라인 Picture+BinStream, 자연 크기·본문폭 축소), 원격/부재는 경고+alt 폴백. #8 media 추출과 바이트 왕복 | ✅ **해소(2026-07-19)** — insert_image 검증 경로 재사용(실기 리스크 낮음) | 들여오기 | S |
| GI-4 | **인라인 코드 서식 소실** | 함초롬돋움(글꼴 테이블 인덱스1, 7슬롯) + 연회색 음영 CharShape run. 양 writer 다중 글꼴 배선 정합 확인 | ✅ **해소(2026-07-19)** — 잔여: md 재수출 시 백틱 미복원(font 기반이라 감지 불가, 범위 밖) | 들여오기 | S |
| GI-5 | **from_json 이미지 바이트 조건부** — `--embed-bin` 없으면 bin `data`가 skip이라 유실 | `hwp-convert/src/lib.rs:39,68-96` | 부분(조건부) | 들여오기 | S |

## 10. GJ — 미지원 포맷·레거시 (2026-07-08 재수색 추가)

입력/출력 포맷 축의 갭. 수요·선례 근거는 [08](08-external-research.ko.md) 생태계 대조 참조.

| ID | 현상 | 근거 | 현 동작 | 난이도 |
|---|---|---|---|---|
| GJ-1 | **DOCX 입출력 부재** — 가장 흔한 상호운용 요구. MS가 공식 배치 변환기(HwpConverter+BATCHHWPCONV)를 배포할 정도의 수요인데 OSS HWP→DOCX는 무주공산 | `hwp-convert/src/docx.rs` | ✅ **출력 해소(2026-08-01)** — `convert --to docx` (`hwp-convert::docx` — 문단/스타일, gridSpan/vMerge·중첩 표, 그림, 하이퍼링크, 번호, 각주/미주, 수식은 스크립트 폴리백). 입력은 미해소(L급 완전 왕복) | M(출력) / L(입력) |
| GJ-2 | **HWPML(.hml) 입출력 부재** — 한컴 공식 스펙(HWPML rev1.2 Part II)·KS 표준 존재, kordoc 구현 선례 | grep 무일치. hwpml은 네임스페이스 URI로만 등장 | 미구현 | M |
| GJ-3 | **HWP 3.x 레거시 침묵 거부** — `V3.00` 시그니처 감지 없이 generic "시그니처 불일치" 에러. 공식 스펙(3.0 rev1.2 Part I) 존재, rhwp·kordoc·LibreOffice hwpfilter 선례 | `hwp-cli/src/format.rs:22-38`(CFB/ZIP만) | 침묵 거부 | 감지=S / 파싱=M~L |
| GJ-4 | **RTF 입출력 부재** | grep 무일치 | 미구현 | M |
| GJ-5 | **표→CSV 추출 부재** — 표를 데이터로 뽑는 경로 없음(수요의 정량 근거는 미검증 — [08] caveat) | grep 무일치 | ✅ **해소(2026-08-01)** — `cat --format csv` + `convert --to csv` (`hwp-convert::csv`, RFC 4180) | S |
| GJ-6 | **`.txt` 확장자 추론 실패** — `convert -o out.txt`가 에러, 평문은 `cat`→stdout뿐 | `hwp-cli/src/commands/convert.rs:195-213`(txt arm 없음) | ✅ **해소(2026-08-01)** — ConvertFormat::Txt + `.txt` 추론, stdout(`-`)로도 출력 | S |
| GJ-7 | **HTML/ODT/PDF 역방향 입력 부재** — 입력은 hwp5/hwpx/json/markdown만(출력 전용 4포맷) | `hwp-cli/src/commands/cat.rs:18-44` | **HTML 부분 해소(2026-08-01)** — 계약 XHTML 부분집합 입력(`from_html`, md 혼합 경로 포함, docs/design/18). ODT/PDF 입력은 미구현 | 부분 | S(HTML) / L(ODT·PDF) |
| GJ-8 | **HWPX 배포용 문서** — 어느 구현체도 미지원(H2Orestart #42 오픈). HWP5용 공식 배포 스펙이 HWPX 변형을 커버하는지 미확인 | [08](08-external-research.ko.md) 미해결 질문 | 미구현 | L |

## 11. GK — 편집 프리미티브 부재 (2026-07-08 재수색 추가)

`edit`/`structure`/`format` 계열에 없는 조작. 전부 "부재 확인(grep)"이며 근거는
`hwp-convert/src/{edit,structure,format}.rs`·`hwp-cli/src/main.rs:113-165`(Edit 플래그 전수).

| ID | 현상 | 비고 | 난이도 |
|---|---|---|---|
| GK-1 | **셀 병합/분할** — `merge_cells`(부분 겹침 거부)·`split_cell`(A5~A7 규격 빈 셀) + CLI `--merge-cells`/`--split-cell` | `edit.rs`(재귀 로케이터, set-cell과 인덱스 일치) — 정품 병합 표 **1,816개 전수 실측 5규칙**(피병합 셀 미저장·행우선·면적 타일링·row_cell_counts·영역 크기)이 사양. 조작 후 불변식 게이트 | ✅ **해소·실기 확정(2026-07-19)** — K1(hwpx)·K2(hwp5) 병합 표시·손상 없음 확인 |
| GK-2 | **열 추가/삭제** — `add_col`(전체 폭 유지 균등 재분배, #9 정책 계승)·`delete_table_column`; 병합 셀 표도 지원(span 걸침 확대/축소), 삽입 위치 지정 + CLI `--add-col "표\|표:위치"`/`--delete-col` | `edit.rs` — 정품 병합 표 9종 회귀(불변식 위반 0) + tbl9 전체 폭 보존 테스트(#9) | ✅ **해소·실기 확정(2026-07-19)** — K3 열 구조 정확 확인 |
| GK-3 | **표 신규 삽입 없음** — from_markdown은 표를 만들지만 앵커 기반 삽입 프리미티브 없음 | — | ✅ **해소(2026-08-01)** — `edit --add-table "앵커=>행JSON"` (`edit::add_table`, 불변식 게이트) | S |
| GK-4 | **문단모양 편집이 정렬 한정** — 줄간격·들여쓰기·좌우 여백·문단 간격 변경 없음 | `format.rs:211-245`(attr1 정렬 비트만) | ✅ **해소(2026-08-01)** — `edit --set-para "찾기=>키:값"` (line-spacing/indent/left/right/top/bottom, `format::set_para_props`) | S |
| GK-5 | **머리말/꼬리말 편집 없음** — 추출 포함/제외만 가능 | `text.rs:62-66` | M |
| GK-6 | **페이지 설정 변경 없음** — 여백·용지·방향(PageDef는 new 시 상수 주입만) | `from_markdown.rs:562-573` | ✅ **해소(2026-08-01)** — `edit --set-page "키:값"` (치수 mm·orientation, `format::set_page_def`) | S |
| GK-7 | **명명 스타일 적용/생성 없음** — 직접 모양 조작만, "제목1" 스타일 링크 편집 없음 | `format.rs` 전체 | M |
| GK-8 | **개체 삭제 없음** — 이미지/필드/표/책갈피 삭제 불가(삽입·문단/행 삭제만 — 비대칭) | `edit.rs`·`field.rs`·`image.rs` | ✅ **해소(2026-08-01)** — `edit --delete-image/--delete-table(n|앵커)/--delete-field/--delete-bookmark` (`edit::delete_object` — 컨트롤+앵커 문자+FIELD_END 수술) | S |
| GK-9 | **add-row/delete-row 표 인덱싱이 set-cell과 불일치**(톱레벨 전용 vs 재귀 깊이 우선) — 잠복 버그 | `structure.rs` 구 nth_table | ✅ **해소(2026-07-18)** — 재귀 로케이터로 단일화, add-row도 병합 거부·깨끗한 템플릿 행 자동 선택 | S |

## 12. GL — 텍스트 추출 옵션 (2026-07-08 재수색 추가)

| ID | 현상 | 근거 코드 | 난이도 |
|---|---|---|---|
| GL-1 | **TextOptions(머리말/숨은설명 토글)가 CLI 미노출** → ✅ **해소(2026-07-15)** — `cat --with-header-footer`·`--with-hidden` 플래그 추가(plain·markdown 적용). PR #8(2026-07-18)이 `convert -o *.md`까지 확장 | `hwp-model/src/text.rs` ↔ `main.rs`·`commands/{cat,convert}.rs` | S |
| GL-2 | **각주/미주 분리·제외 불가** — 항상 본문에 포함(강제), 각주만 뽑기/빼기 없음 | `text.rs:62-66`(`_ => true`) | S |
| GL-3 | **표 제외·페이지/구역 범위 추출 없음** — 전량 추출만 | `text.rs:20-40` | S |

## 13. GM — CLI 명령·워크플로 (2026-07-08 재수색 추가)

서브커맨드 전수(`main.rs`: info·cat·convert·render·new·diff·edit·fields·bookmarks·slots·fill·
validate·mcp·dump) 기준 부재 목록. 수요 근거는 [08](08-external-research.ko.md) 생태계 대조.

| ID | 현상 | 수요·선례 근거 | 난이도 |
|---|---|---|---|
| GM-1 | **배치/glob/디렉토리 처리 없음** — 전 명령이 단일 파일 인자 | MS BATCHHWPCONV·H2Orestart headless가 배치 수요 실증 | ✅ **해소(2026-08-01)** — convert 다중 입력 + `--out-dir` (파일명 `<스템>.<확장자>`) | S |
| GM-2 | **stdin 입력·stdout 파이프 미흡** — convert/edit은 출력 파일 필수, `-` 미지원(cat만 stdout) | 유닉스 CLI 관례 | ✅ **해소(2026-08-01)** — convert 입력 `-`(stdin 스테이징)·출력 `-`(텍스트 포맷 stdout) | S |
| GM-3 | **문서 병합 없음** — 여러 hwp를 하나로 | pyhwpx 쿡북 정식 챕터(33개→99쪽 병합), 현행 해법은 Windows COM 전용·불안정 | M |
| GM-4 | **문서 분할/페이지 추출 없음** — render `--pages`는 이미지용 | pyhwpx 쿡북(100쪽→1쪽씩 분할 저장) | M |
| GM-5 | **텍스트 검색(grep) 명령 없음** — edit `--replace`만 존재 | — | ✅ **해소(2026-08-01)** — `hwp grep <패턴> <파일>` (문단 재귀 검색, 미일치 종료 코드 1, `--ignore-case`) | S |
| GM-6 | **메타데이터 일괄 편집/덤프 없음** — `--set-meta`는 new/edit 국소 | — | ✅ **이미 충족(2026-08-01 정정)** — 덤프는 `hwp info`(metadata JSON), 편집은 `edit --set-meta`(title/author/subject/keywords 반복)로 커버됨 | S |
| GM-7 | **도장/서명 자동 날인** — `edit --seal "앵커=>이미지@크기mm"` 구현(부유·글 앞 Picture, 앵커 텍스트 유지, 기본 20mm) | `hwp-convert/src/image.rs insert_seal` | ✅ **실기 확정(2026-07-16, D1·D2 통과)** — 실기 3회 반복으로 확정: hwpx=`IN_FRONT_OF_TEXT`+`allowOverlap=1`+오프셋, hwp5=attr `0x04aa4310`(글앞·PARA·본문제한 해제, §4.3.9.1 비트 표 대조) |
| GM-8 | **문서 내용 비교 없음** — `diff`는 렌더 픽셀 비교 전용, 텍스트/구조 비교 없음 | kordoc compare_documents 선례 | M |
| GM-9 | **AI 연동이 client 환경별로 다름** - Amazon Quick Desktop은 publish-safe skill을 active profile에 설치하고 local stdio connector를 실행할 수 있으나, Quick Web은 `hwp mcp`를 시작하거나 desktop path를 공유할 수 없음. Tenant-isolated artifact를 사용하는 hosted authenticated Streamable HTTP service는 미구현 | Desktop profile 탐색/설치는 `skill export --install amazon-quick`으로 ✅ **해소(2026-08-09)**. Web은 [20-remote-mcp](20-remote-mcp.ko.md)와 [issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52)의 open item | Desktop S / Web L |

## 14. 로드맵 — 난이도 × 가치 + 의존 그래프

### 14.1 난이도 × 가치 매트릭스

**가치**는 실문서 출현 빈도 + 실사용 수요([08](08-external-research.ko.md) 생태계 대조) 기준.

| | **난이도 S**(자료구조만) | **난이도 M**(정답지 필요) | **난이도 L**(실기 반복) |
|---|---|---|---|
| **가치 高**(빈출) | GC-4·GC-5(탭·구역속성), GC-8·GC-9(내어쓰기·문단배경) — ✅해소(2026-07-15): ~~GE-α1~α5·α7, GH-1·GH-2, GL-1, GA-5, GE-β4~~ / ✅해소(2026-07-18, md): ~~GH-3·GH-4·GH-5·GH-6, GH-8~~ | GG-3·GG-4(양쪽정렬·자간), GF-2(찾아보기·겹침), **GA-2★**(배포용 읽기 — 공식 스펙 공개), ~~GJ-1 출력~~(DOCX 내보내기 2026-08-01 해소), **GK-1**(셀 병합), **GK-2**(열 삭제 — 추가는 07-19 해소) — ✅GC-2·GC-3은 07-19 해소(J1 실기 대기) | GG-1·GG-2(글상자 드롭·오버플로) |
| **가치 中** | GC-6(글상자 다단), GE-2~GE-6(그림 드롭·단·번호 합성), GF-1(%unk), **GB-12**(참고문헌), **GE-β1·β2·β5**(미리보기·스크립트·설정), ~~GG-16~~(렌더 국소 — PR 9 해소, GG-5·GG-6·GG-8~GG-11·GG-17·GG-20·GG-21·GG-22는 2026-08-13/14 해소), **GH-3·GH-4·GH-5**(html/odt 각주 마커·병합셀·셀 블록 — md는 2026-07-18 해소), ~~GJ-5·GJ-6~~(csv·txt, 08-01 해소) — ✅GI 계열 전체·GE-7은 07-19 해소, ~~GK-3·GK-4·GK-6·GK-8~~, ~~GM-1·GM-2·GM-5~~, **GM-6**(이미 충족 정정)·GM-7(날인, 07-16 해소) — ✅ 2026-08-01 배치 해소 | GB-4~GB-7·GB-10(글맵시·양식·묶음·메모·바탕쪽), GC-1(세로쓰기), GD-1~GD-3(수식 — rhwp 선례), GE-α6(그러데이션), GF-3(필드 생성), **GB-1 hwpx 차트 생성★**(chartSpace — kordoc 선례), **GJ-2·GJ-3**(hml·HWP3.x — 공식 스펙 공개), **GG-7·GG-12~GG-15·GG-18·GG-19**(렌더 픽셀 대조), **GE-β3·β6**(DocOptions·임베디드 폰트), **GH-7**(ODT 레이아웃), **GK-5·GK-7**(머리말 편집·스타일), **GM-3·GM-4·GM-8**(병합·분할·비교) | GB-2·GB-3(OLE·동영상), **GJ-1 완전 왕복**(docx 들여오기 포함 시), **GM-9 Web**(인증형 hosted MCP·tenant 격리·운영, Desktop은 해소) |
| **가치 低**(드묾) | GA-3·GA-4(거부 메시지), **GI-5**(embed-bin), **GL-2·GL-3**(추출 세분) | **GJ-4**(rtf) | GA-1(암호화), GB-8·GB-9·GB-11(변경추적 등), **GJ-7**(역방향 입력), **GJ-8**(HWPX 배포용) |

**읽는 법:** 좌상단(S·高)이 **가성비 최상** — GE-α(글자효과 왕복)에 더해 **GH-1·GH-2**(md/html
링크·이미지 — ODT 임베드 패턴 재사용)와 **GL-1**(clap 플래그만 추가)이 새 진입점이다.
★는 2026-07-08 재평가: **GA-2 배포용은 공식 복호화 스펙 공개로 L→M**, **GB-1 차트의 hwpx
경로는 OOXML chartSpace라 L→M**. 우하단(L·低)은 우선순위 최하.

### 14.2 의존 그래프

```
[정답지 확보]  ──선행──▶  GB-1~7(개체 렌더)  ──필요──▶  10/11 레코드 구조 해석
   │                       GC-2/GC-3(쪽테두리·각주모양) ── FOOTNOTE_SHAPE/PAGE_BORDER_FILL 의미해석
   │                       GD-1~3(수식 조판)  ── 정품 수식 메트릭
   │                       GG-1/GG-2(속성 충실도) ── 실기 반복(07§F)
   │                       GG-7/GG-12~15/GG-18/19 ── 정품 렌더 픽셀 대조
   │
[공식 스펙 존재 — 역설계 불요] ──▶ GA-2(배포용 복호화 — 배포용문서 rev1.2)
   │                              GJ-2(HWPML — 스펙 Part II)   GJ-3(HWP 3.x — 스펙 Part I)
   │                              (단, 스펙-실파일 불일치 사례가 있어 실파일 코퍼스 검증은 별도)
   │
[독립·즉시 착수] ──▶ GE-α6     (그러데이션 중심·step — α1~α5·α7·α8은 ✅해소 2026-07-15)
                    GE-2       (write.rs 국소, 그림 드롭 경고→복구)
                    GA-3/GA-4  (거부 메시지 — GA-5 버전 게이트는 ✅해소)
                    GC-4 렌더  (탭 위치·채움을 hwp-render/tab.rs에 반영 — 왕복은 ✅해소)
                    (✅해소: GH-1/GH-2, GL-1, GC-4/5/8/9, GE-β5 · GM-7 구현=실기 대기)
   │
[수요 최상] ──▶ GJ-1(DOCX 출력) ──품질 선행──▶ GH-1/GH-2/GH-4 (링크·이미지·병합셀 정리가
                                               DOCX 매핑의 기초 데이터가 됨)
   │
[구체 Web consumer] ──▶ GM-9 Web ──선행──▶ protocol core 분리 + artifact model
                                   ──후속──▶ OAuth resource server + Streamable HTTP + tenant 운영
```

**의존 규칙 요약:**
- **GB 개체 렌더**는 10/11의 레코드/요소 구조 해석이 선행돼야 한다(현재 Opaque/fallback이라 의미
  필드가 IR에 없음). 또한 대부분 **정답지 확보가 선행**([00](00-overview.ko.md) §4 정답지 방법론).
- **GC-2·GC-3**(쪽테두리·각주모양)은 hwp5가 이미 Opaque로 정보를 보존하므로, "정답지로 레코드
  레이아웃 확정 → IR 의미 필드 승격 → hwpx/렌더 방출"의 3단계다.
- **GE-α 전체**는 read가 이미 해석 완료라 **어떤 것에도 의존하지 않는 독립 노드**다. write 대응
  요소 방출만 추가하면 되는 최단 경로.
- **GG-1·GG-2**는 07§F의 미해결과 동일 뿌리(속성 충실도)라 **실기 반복 + 정답지**가 공동 선행.

### 14.3 정답지 선행 항목 (실기·정품 파일 필요)

아래는 [00](00-overview.ko.md) §4 정답지 방법론에 따라 **정품 한글 파일 확보가 선행돼야** 착수 가능한
항목이다(추측 조판 금지). 나머지(특히 GE-α·GH·GL·GC 국소·렌더 국소)는 정답지 없이 자료구조/렌더
만으로 진행 가능하다.

- **GB-1~GB-7, GB-10**: 차트·OLE·동영상·글맵시·양식·묶음·메모·바탕쪽 — 해당 개체를 담은 정품 파일
- **GC-1, GC-2, GC-3**: 세로쓰기·쪽테두리·각주모양 — 해당 조판을 쓴 정품 파일
- **GD-1~GD-3**: 행렬·큰연산자·복잡 구분자를 포함한 정품 수식
- **GG-1, GG-2**: 07§F 서사대로 실기 반복 필요
- **GG-13~GG-15, GG-19**: 정품 렌더와의 픽셀 대조로 확정(GG-12는 PR 3, GG-18은 PR 7, GG-7·GG-23은 PR 8, GG-14·GG-16은 PR 9에서 해소)
- **GA-2, GJ-2, GJ-3**: 공식 스펙으로 착수 가능하되, 스펙-실파일 불일치 사례가 알려져 있어
  ([08](08-external-research.ko.md) — 단 정의 14 vs 16B) 정품 코퍼스 검증을 병행
- **판정 유보 3건(2026-07-19 스펙 전수 감사 — 갭 여부 자체가 미확정, 정품 실측 전 등재 금지)**:
  ① 호(SC_ARC) 첫 필드를 1B로 읽는 현행 코드 vs 스펙 표 101의 UINT32 선언(`shape_draw.rs:571-576`) —
  호 도형 정품 바이트 대조 필요 ② 그리기 개체 hflip/vflip(표 83 attr bit0/1) 미독 — 렌더 행렬에
  음수 스케일로 내포됐을 가능성이 있어 반전 도형 정품 대조 필요 ③ ParaShape 줄간격 종류를 구버전
  attr1 bit0-1에서만 파생하는 현행 코드(`doc_info.rs:297`) vs 표 46 attr3(5.0.2.5+, "최소" 포함) —
  한글이 신버전에서도 attr1을 동기 기록하는지 "최소" 줄간격 정품 파일로 확인 필요

---

**요약:** 초판의 저비용·고가치 진입점(GE-α 글자효과, GH-1·GH-2 링크·이미지, GL-1 추출 옵션,
GA-5 버전 게이트, GE-β4 요약정보)은 **2026-07-15에 일괄 해소**됐다(§0.5). 다음 진입점은
**GC-8·GC-9**(내어쓰기·문단배경, S)와 **GE-β5·GM-7**(설정 pass-through·
도장 날인, S)이고, 고가치·고난도의 정공법은 **GC-2·GC-3**(공문서 빈출 쪽테두리·각주모양)과
**GA-2**(배포용 읽기 — 공식 스펙 공개로 재평가된 M)이 남았다. 과거 최대 수요였던 **GJ-1**(DOCX 출력)은
2026-08-01 해소(내보내기 전용, 입력은 L급 미해소) — 다음 대형 항목은 GA-2와 GM-3/4/8 계열이다.
