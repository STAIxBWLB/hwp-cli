[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI 클라이언트 연동

`ahwp`는 AI 클라이언트용 연동 표면을 두 가지 제공한다.

- MCP를 구사하는 클라이언트용 **MCP stdio 서버**(`ahwp mcp`, 23개 도구)
- CLI와 MCP 사용법을 에이전트에게 가르치는 **에이전트 스킬**(이 저장소의 `skills/hwp/` 트리).
  바이너리에 임베드되어 있으며 `ahwp skill export`로 디렉터리 형태로 풀어낼 수 있다:
  `SKILL.md`, `SKILL.ko.md`, 공문서 가이드(`official-documents(.ko).md`), `references/`,
  `templates/`.

어느 표면을 쓰든 `ahwp mcp --root {dir}`로 도구가 만지는 파일 경로를 지정 디렉터리 아래로
제한하고, 에이전트가 쓴 파일은 `ahwp validate`로 검증하는 것을 권장한다.

## 모든 클라이언트에 공통인 규약

아래 내용은 어느 클라이언트가 구동하든 CLI와 MCP 서버에 똑같이 적용된다.

### 원본 양식 워크플로

기존 양식을 완성할 때는 제공된 네이티브 HWP/HWPX 양식을 우선한다. Markdown은 재작성이
목적일 때 선택한다. 여기의 #417-#420 전체 워크플로는 v1.5.0부터
포함된다. v1.4.0 이하 릴리스에는 `document.original_form` 인증이 없다. 작업을
선택하기 전에 바이너리 버전과
[버전별 기능·제한 증거](../../skills/hwp/references/original-form-capabilities-v1.json)를 확인한다.

구체적인 명령, 검토할 JSON 입력, 증거 범위는
[정본 원본 양식 절차](../../skills/hwp/references/editing-recipes.ko.md#원본-양식-워크플로)를 따른다.

1. 원본 해시·개별 위치·주소를 발견한다. 필요한 리소스와 함께 네이티브 범위를 추출하고
   원본→출력 주소 맵을 유지한다.
2. 추출한 원본에서 discovery를 다시 한다. 명시적인 claim과 복제 대상을 검토하고 바인딩
   계획을 dry-run해 로컬 preview를 확인한다. 같은 원본 바이트에 채우기·복제를 원자적으로 적용한다.
3. 수정한 배분값은 명시적인 숫자 제약과 호출자가 제공한 합계를 사용해 그 검토 원본에
   재적용한다. 스타일·쪽 나눔 전에 주소를 새로 얻고, 들여쓰기를 두 번 적용해 no-op 비교를 확인한다.
4. 전달할 파일을 검증하고 고정된 글꼴로 렌더링한다. 검토한 셀 값·표 행열 수·양식 시작점을
   기준으로 인증하고 원본·계획·출력·글꼴·렌더 식별자를 유지한다.
5. 사람이 한컴에서 정확히 그 전달 파일을 다시 열고 내보낸다. 네이티브 호환성을 주장하기
   전에 별도의 영수증을 유지한다.

선행 하이픈 등 임의의 텍스트에는 구분자 기반 플래그보다 구조화된 `--ops` JSON을 사용한다.
Discovery 목록·기대값 계획·정확한 preview는 문서 내용을 담으므로 로컬에 둔다. 기본 영수증은
그 값을 노출하지 않고 결과를 식별한다. 정본 절차에서 IR 문자 배열과 일반 문자열의 차이,
자체 완결 리소스 내보내기에 `convert --to json --embed-bin`이 필요한 이유도 확인한다.

MCP에서도 같은 단계 순서로 `hwp_split`, `hwp_slots`, `hwp_edit`, `hwp_render`, `hwp_certify`를
사용한다. `hwp_read`는 기존 markdown `with_segments`만 제공하며 CLI segments v2나 JSON과
segments의 조합은 제공하지 않는다. 공개된 주소를 새로 얻는다. `apply_form_bindings`는 배열이
아닌 객체 하나이며 `verify` 속성이 없다. 이 단독 바인딩 연산에 스타일 편집을 섞지 않는다.
MCP `insert_para`는 CLI의 인라인 `style`/`char` 필드를 제공하지 않는다. 삽입한 제목 문단의
주소를 새로 얻고 별도의 공개된 `apply_style` 요청으로 이름 있는 스타일을 적용한다.
MCP `hwp_render`에는 `report`나 `layout_json` 인자가 없다. 타입이 있는 layout/report 산출물
내보내기는 CLI 전용이다. MCP `hwp_certify`는 자체 정책·렌더 증거를 리포트 디렉터리에 계속 발행한다.

정책 결과와 제한을 각각 읽는다. 구조적 유효성·보존·렌더 진단의 완전성·시각적 수용·사람이
수행한 한컴 증거는 구분한다. 한정된 네이티브 렌더나 PDF 성공은 시험한 fixture·글꼴 환경·선택한
정책 범위의 증거다. `SKILL.md`만으로 시작하는 Quick/web 설치에서는
`ahwp skill export -o ./hwp-guide`로 전체 번들 절차와 기능 JSON을 풀어낼 수 있다.

### 문서 단위 작업

v0.13.0부터 문서 하나씩이 아니라 문서 전체를 다루는 작업을 지원한다.

- `ahwp merge {inputs...} -o {out}` / `hwp_merge` — 문서 두 개 이상을 인자 순서대로 이어 붙이며
  입력 하나가 Section 하나가 된다. 쪽·각주·개요 번호는 각 입력의 시작·계속 설정을 그대로
  유지하므로 병합한 뒤 다시 확인해야 한다.
  후속 HWPX의 그림 항목 참조는 각 원본의 의미를 유지하며, ID가 같아도 누락된 대상을
  다른 입력의 이미지로 연결하지 않는다. 병합 중 첫 manifest의 참조 규칙을 바꾸지 않으며,
  선언된 누락 대상을 다른 입력으로 채우지 않는다.
  native 출력에서 제거된 이미지는 `BinaryAssetRemoved`로 보고한다. 이 손실을 허용할 수
  없으면 `--strict`로 발행을 거부한다.
- `ahwp split {in} --out-dir {dir}` / `hwp_split` — 기본은 Section 하나당 조각 하나이고,
  `--pages`를 주면 쪽 범위로 나눈다. 쪽 경계는 다시 계산한 값이 아니라 한컴이 저장한 레이아웃
  캐시에서 얻은 추정값이라 한컴 자체 페이지 나눔과 다를 수 있다. `--from`/`--to`(`hwp_split`은
  `from`/`to`)를 주면 제목(`ahwp cat`이 출력하는 문단 자신의 텍스트), 책갈피, `구역.문단` 주소를 지점으로 하는 형식 있는 범위 하나를
  대신 추출하며, 범위가 쓰는 바이너리가 함께 들어간다. 문단·표의 이동 위치와 바이너리의 유지·
  미검증 유지·제외·출력 형식이 버린 것(HWP5는 BIN_DATA 표가 이름 붙인 바이너리만 기록한다)을
  담은 `hwp-range-extraction-v1` 보고서는 `--range-report {file.json}`를
  줄 때에만 파일로 기록되고, `hwp_split`은 항상 `range` 필드로 돌려준다. 바이너리는 조각 전체가
  쓰지 않음이 증명될 때에만 제외된다. 도구가 읽을 수 없는 참조(이미지 채움, 이미지 글머리표일 수 있는
  글머리표, 원본 도형·OLE·동영상 개체, XML로 읽을 수 없는 패키지 부품)가 조각에 있으면 모든 바이너리가 함께 가며 보고서가
  그 자리를 나열한다. 설정·버전 XML, 해석할 수 없는 XML 속성, 잘린 참조 레코드와 알 수 없는
  원본 태그도 포함한다. 추출 중 입력을 바꾸지 않아야 한다. 파싱 전후 해시 검사는 중간에 파일이
  A에서 B로 바뀌었다가 A로 돌아오는 교체를 검출할 수 없다. 보고서는 내용을 담지 않아 지점을 제목·책갈피 텍스트가 아니라 종류와
  해석된 주소로만 가리킨다. 조각은 구역의 쪽·단·머리말·꼬리말·쪽 번호·새 번호 컨트롤을 가져오며,
  범위가 구역 첫 문단 뒤에서 시작하면 그 문단의 감추기·홀짝 조정 컨트롤은 원본의 그 쪽에만
  작용하므로 가져오지 않고, 첫 문단에서 시작하는 범위는 그 문단을 통째로 가져온다.
- `ahwp compare {a} {b}` / `hwp_compare` — 문서 두 개의 문단·구조 차이를 보고하며 두 입력 모두
  수정하지 않는다. 렌더를 한컴 참조 PNG와 비교하는 `ahwp diff`와는 다른 명령이다.

### 이름 있는 문단 스타일 바꾸기

편집 전에 현재 스타일과 문단 주소를 확인한다.

```bash
ahwp styles doc.hwpx --format json
ahwp cat doc.hwpx --format json --with-segments --segments v2 > doc.segments.json
```

`kind: "paragraph"`인 스타일을 선택한다. 아래 예시의 `body`는 기존 기본 이름이고,
`at: {section: 0, paragraph: 1}`은 선택한 문단이다. 둘 다 현재 문서에서 확인한 값으로
바꾼다. 주소에는 v2 또는 스타일의 `used_by`에서 얻은
`{"id":"CURRENT_PARA_SEGMENT_ID"}`도 쓸 수 있다. ID는 원본 스냅샷에 속하므로 편집 뒤에는
새로 조회한다.

다음 배열을 `restyle.json`으로 저장한다.

```json
[
  {"op":"create_style","name":"Body restyled","based_on":"body","kind":"paragraph","size":"13pt"},
  {"op":"apply_style","address":{"at":{"section":0,"paragraph":1}},"style":"Body restyled","clear_direct":false}
]
```

```bash
ahwp edit doc.hwpx -o styled.hwpx --ops restyle.json
```

같은 배열을 `ahwp edit doc.hwpx -o styled.hwpx --ops -`의 stdin으로 전달할 수도 있다.
이름 있는 스타일 연산에는 별도 플래그가 없다. MCP는 다른 편집 배열보다 먼저
create/update/apply 순서로 스타일 배열을 실행한다. 다른 순서가 필요하면 순서가 있는
`--ops` 배열을 사용한다.

새 스타일의 사용 문단을 함께 바꾸려면
`[{"op":"update_style","style":"Body restyled","size":"14pt"}]`를 `update-style.json`으로
저장하고 `ahwp edit styled.hwpx -o updated.hwpx --ops update-style.json`을 실행한다.
스타일 선택자는 정확한 기본 이름 또는 정수 ID를 받는다. `update_style`은 바뀐 정의를
copy-on-write로 추가하므로 다른 스타일과 직접 서식은 자체 모양을 유지한다.
`apply_style`은 기본적으로 직접 지정한 글자·문단 서식을 보존한다. `clear_direct: true`를
명시하면 둘 다 대상 스타일로 초기화한다. 현재 이 연산은 글자 스타일이 아닌 문단 스타일을
편집한다.

두 호출에 대응하는 `hwp_edit` MCP 인수는 다음과 같다.

```json
{"input":"doc.hwpx","output":"styled.hwpx","create_style":[{"name":"Body restyled","based_on":"body","kind":"paragraph","size":"13pt"}],"apply_style":[{"address":{"at":{"section":0,"paragraph":1}},"style":"Body restyled","clear_direct":false}]}
```

```json
{"input":"styled.hwpx","output":"updated.hwpx","update_style":[{"style":"Body restyled","size":"14pt"}]}
```

동일한 적용·갱신 요청을 반복하면 의미 변경 없이 성공한다. 생성을 반복하면 중복 이름으로
원자적으로 거부한다. 오래된 ID도 발행 전에 거부한다. 영향을 받는 사용 문단이 재생성할 수
없는 원본 레코드나 XML 안에 있으면 위치를 알리고 거부한다. `ahwp styles`로 다른 스타일을
선택하거나 원본을 복구한 뒤 다시 시도한다.

```bash
ahwp validate updated.hwpx
ahwp certify updated.hwpx --policy policy.json --report ./style-certification
```

문서에 맞는 정책 파일과 아직 없는 리포트 디렉터리를 지정한다. 현재 `certify`에는
`--policy`가 필수이며 정책을 생략한 기본 정책 명령은 없다. 리포트를 읽고 모양이 중요하면
렌더 결과도 확인한다. `native_only` 통과는 해당 정책 아래 네이티브 구현의 검사 결과이며,
한컴에서 파일을 열거나 렌더했다는 증거가 아니다. 한컴 열기 증거는 별도 영수증으로 제공한다.
`.hwp` 경로에도 같은 순서를 쓸 수 있으며 출력 형식 제한과 거부 진단은 계속 적용된다.

### 모든 일치 대신 run 하나 또는 구간 하나만 치환

문단 주소를 준 `replace`는 그 문단 안에서 `from`이 일치하는 곳을 모두 바꾸므로, `from`이 그
문단에 정확히 한 번만 나온다는 것을 아는 호출자에게만 쓸 수 있다. 선택한 run 하나, 또는 그
안의 구간 하나만 바꾸려면 run을 주소로 지정한다. v2 엔벨로프의 `run` 세그먼트는 원본 문단
안에서 차지하는 `source_chars` 구간을 절대 UTF-16 코드 단위로 실으므로, 그 숫자를 변환 없이
주소의 `chars`로 그대로 쓴다.

```bash
ahwp cat doc.hwpx --format json --with-segments --segments v2 > doc.segments.json
```

```json
[{"op":"replace","address":{"id":"RUN_ID_FROM_THE_ENVELOPE","chars":[12,17]},"from":"선택 구간","to":"{{slot}}"}]
```

`chars`가 없는 `at.run`은 지정한 run의 텍스트 전체를 바꾼다.

```json
[{"op":"replace","address":{"at":{"section":0,"paragraph":3,"run":1}},"from":"기존 텍스트","to":"새 텍스트"}]
```

두 형태 모두 지정한 run 또는 구간만 건드린다. 같은 문단의 다른 위치, run 경계를 걸친 곳,
필드 값 안에 같은 문구가 있어도 그대로 둔다. `from`은 선택자가 아니라 드리프트 검사이므로,
주소가 덮는 텍스트가 `from`과 정확히 같지 않으면 아무것도 바꾸지 않고 그 요청을 주소와 구간을
붙여 미적용으로 보고하며, `--allow-partial`이 없으면 실행이 실패한다. run id와 중첩 문단 id는
`<checksum>.<path>` 모양이 같으므로 `id`로 run을 지정할 때는 `chars`를 함께 주고, 창 없이 run을
지정하려면 `at.run`을 쓴다.

묶음 실행에서 주소는 문서를 읽은 시점에 해석되므로, 앞선 연산이 위쪽 문단을 삽입하거나 삭제해도
run 지정은 제 문단을 따라간다. 다만 그 문단 자체의 길이가 바뀌면 유지되지 않는다. 앞선
`replace`의 `from`과 `to` 길이가 다르면 사전 검사에서 거부하고, 앞선 `set_indent_level`이 앞쪽
공백을 지우면 아무것도 쓰지 않고 거부한다. 묶음에서 run 지정을 먼저 두거나 두 편집을 나누어
실행한다.

### 기존 문서에 구성 요소 삽입

`insert_blocks`는 DocumentSpec v1 블록(문단, 목록, 표, 이미지, 필드, 수식, 쪽·단 나누기)을
지정한 문단 앞이나 뒤에 문서의 서식 그대로 넣는다. 문단 주소는 `ahwp cat doc.hwpx --format
json --with-segments --segments v2`에서 얻고, 구성 요소를 `component.json`으로 저장한다.

```json
[
  {"op":"insert_blocks","address":{"at":{"section":0,"paragraph":2}},"position":"after",
   "blocks":[
     {"type":"paragraph","runs":[{"type":"text","text":"결재:"}]},
     {"type":"table","columns":[{"width_mm":40},{"width_mm":40}],
      "rows":[{"cells":[{"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"기안"}]}]},
                       {"blocks":[{"type":"paragraph","runs":[{"type":"text","text":"결재"}]}]}]}]},
     {"type":"image","path":"logo.png","width_mm":20}
   ]}
]
```

```bash
ahwp edit doc.hwpx -o with-component.hwpx --ops component.json --report report.json
```

알아둘 규칙은 다음과 같다.

- 블록의 스타일 이름은 문서가 가진 스타일에서 먼저 찾는다. 문서에 없는 이름은 연산의
  `styles` 정의로 만들거나(`missing_style: "create"`, 기본값) 거부한다(`"refuse"`). 보고서에
  적용된 정책이 적힌다. 목록 이름은 `lists`에 정의가 있어야 한다.
- 이미지 경로는 `--ops` 파일이 있는 디렉터리 기준 상대 경로이며 그 밖으로 나갈 수 없다.
  `--ops -`로 표준 입력에서 읽으면 디렉터리가 없어 이미지 블록을 거부한다. MCP에서는
  `base_dir`(샌드박스 검사를 받는 디렉터리)을 넘기며, 서버의 `--root` 샌드박스가 모든 자산
  읽기를 묶는다.
- 표와 이미지는 들어갈 본문 단이나 표 셀의 폭으로 검사한다. 가로 구역은 회전한 용지 폭,
  다단 구역은 한 단의 폭, 셀 안은 셀 폭에서 셀 여백을 뺀 값이다. `position: "before"`는
  주소 앞의 단 구성을 쓴다. 폭을 넘는 블록은 아무것도 쓰기 전에 거부한다.
- 수식은 HWPX에만 삽입한다. HWP 쓰기는 수식 레코드가 없어, HWP 출력은 HWPX 대안을 알리며
  거부한다. 구역 나누기는 지원하지 않는다.
- 편집은 원자적이다. 하나라도 거부되면 `--allow-partial`이어도 출력이 쓰이지 않는다.

보고서의 해당 연산 항목은 삽입된 대상의 id를 알려 주므로 다음 호출에서 인덱스를 추측할 필요가
없다. `pieces_touched`는 삽입된 문단 수이고, `changed`에는 `ahwp cat`이 그 문단 안에서 발행할
세그먼트마다 생성 쌍(`before`는 null)이 하나씩 있다. 문단 id 다음에 그 런 id가 오고, 표나
이미지 블록이면 표·셀·이미지 id도 있다. 보고서 상한(10,000쌍)을 넘는 아주 큰 구성 요소는 `changed`에 문서 순서로 앞 10,000쌍만 담고
`changed_total`에 실제 개수를 적는다. 빠진 id는 `ahwp cat`이 보여 주는 값과 같다.
`set_format`에는 런 id를 넘긴다.

```json
[{"op":"set_format","address":{"id":"RUN_ID_FROM_THE_REPORT"},"bold":"on"}]
```

id는 해당 연산 직후의 문서를 기준으로 한다. 이후 편집을 하면 새로 얻어야 한다. 결과에는
다른 문서와 똑같이 `ahwp validate`와 `ahwp certify`를 적용한다. 필드 블록은 네이티브 렌더가
생략된 컨트롤을 보고하게 하는데, 삽입 오류가 아니라 렌더러의 한계다.

대응하는 `hwp_edit` MCP 인수는 다음과 같다(항목은 `op` 태그를 뺀 연산이다).

```json
{"input":"doc.hwpx","output":"with-component.hwpx","base_dir":"/work/assets","insert_blocks":[{"address":{"at":{"section":0,"paragraph":2}},"position":"after","blocks":[{"type":"image","path":"logo.png","width_mm":20}]}]}
```

### 네이티브 양식 바인딩 검토와 적용

실제로 편집할 원본에서 정확한 개별 위치를 발견한다.

```bash
ahwp slots source.hwpx --json --forms --occurrences > local-catalog.json
```

버전 1 목록은 `source_sha256`, 파싱 IR의 `structural_sha256`, 원본에 결속된 개별 위치 ID를
담는다. 문서 텍스트가 포함되므로 명시적인 검토를 위해 로컬에 보관한다. 셀 문단은 목록의
대상이며 이웃 레이블은 근거이지 자동 값 소유 관계가 아니다. 연결 가능한 겹치지 않는 범위를
명시적으로 선택한다. 캡션·원본 보존/혼합 내용 대상·네이티브 이름 있는 필드는 미지원으로
보이되 네이티브 필드는 `ahwp fields`로 안내한다. 이 거부를 광범위한 텍스트 치환으로 우회하지 않는다.
`label_occurrences`는 같은 종류·정확히 같은 레이블의 횟수를 세고 `label_ambiguous`는
반복 레이블을 표시한다. 검토할 모호성을 드러낼 뿐 자동으로 한 위치를 선택하지 않는다.

다음 요청 구조를 `OPS.json`으로 저장한다. 예시 해시와 개별 위치 ID는 목록의 정확한 값으로
바꾸고 의도한 범위, 입력 값과 예산을 검토한다.

```json
[
  {
    "op": "apply_form_bindings",
    "plan": {
      "version": 1,
      "source_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "structural_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
      "bindings": [
        {
          "key": "course.name",
          "targets": [
            {
              "occurrence_id": "occ-2222222222222222222222222222222222222222222222222222222222222222",
              "copies": [
                0
              ],
              "max_chars": 100,
              "allow_reflow": true
            }
          ]
        }
      ],
      "repeats": [],
      "constraints": []
    },
    "values": {
      "course.name": "Reviewed course title"
    },
    "receipt": "./binding-receipt.json",
    "preview": "./binding-preview.json"
  }
]
```

```bash
ahwp edit source.hwpx -o bound.hwpx --ops OPS.json --verify
ahwp validate bound.hwpx
```

`apply_form_bindings`는 배치의 유일한 연산이어야 한다. 영수증은 필수이며 문서·선택한 로컬
미리보기와 함께 원자적으로 발행한다. `--allow-partial`, `--report`와 함께 쓰지 말고 이 연산의
영수증을 사용한다. 미리보기에는 정확한 값과 주소가 들어간다. 기본 영수증·로그는 레이블이나
값이 아닌 식별자·해시·횟수·개별 위치 ID를 담는다. 결과를 확인하고 원본·계획·출력 식별자를 유지한다.
영수증에는 네이티브·미리보기 경고 건수도 담으며 원래 메시지는 명시적으로 요청한
미리보기의 `native_warnings`에만 둔다. 미리보기 폰트 대체는 의미 손실과 다르며
렌더·폰트 수용 검사는 별도로 수행한다.

대안으로 편집 호출에 `--dry-run`을 추가한다. 비공개 네이티브 후보를 작성·검증한 다음
`published: false`인 영수증과 요청한 미리보기를 발행하며 네이티브 출력은 발행하지 않는다.
이는 검토 단계이며 변경된 원본 바이트에 적용해도 안전하다는 약속이 아니다.

HWP와 HWPX 모두 같은 형식으로만 적용한다. 다른 네이티브 형식을 쓰려면 먼저 변환하고 변환된
원본에서 새로 발견한다. 수정한 값은 같은 최초 원본 바이트와 검토된 계획에 다시 적용할 수
있다. 이전 출력을 오래된 계획에 넣거나 이전 출력 위에서 다시 복제하지 않는다. 원본이
바뀌면 새 발견과 검토가 필요하다.

대상마다 `max_chars`와 명시적인 `allow_reflow`가 필요하다. `copies` 기본값은 원본을 뜻하는
`[0]`이며 추가 복제는 명시적으로 선택한다. `max_chars`는 Unicode scalar 예산이다.
재배치 허용이 없으면 교체 WCHAR 길이는 이전 범위를 넘을 수 없고 LF·TAB도
거부한다. 검토된 허용은 유계 텍스트 증가를 허용할 뿐 폰트 축소·그리드/서식 변경·쪽 안의
배치 적합성을 증명하지 않는다. 렌더·폰트·한컴 증거는 별도 #419 수용 단계이며 의미 영수증이
그 증거를 제공하지 않는다.

반복에는 지원되는 최상위 표 ID와 텍스트가 고유한 삽입 앵커인 기존 본문 문단 ID를 사용한다.
`copies`는 원본을 포함하므로 네 개면 세 번 복제한다. 개수와 정확히 일치하는 고유한 호출자
지정 `profile_ids`를 주고 대상의 복제 인덱스를 각각 선택한다(0은 원본). 반환된 원본→복제
맵을 확인한다. 추출한 양식이면 실제 #417 범위 리포트와 원본→출력 맵을 유지한다. 이동한
표 인덱스를 추론하거나 IR을 수동으로 자르지 않는다. 일반 배치의 #358 스냅샷 규칙은 유지된다.

제약은 명시적 키와 호출자 제공 총계에 대한 정수 `equal`, `sum`이다. 정수는 부호 없는 ASCII
숫자와 선택적인 올바른 세 자리 쉼표 묶음이며 `i64::MAX` 이하다. `1,000`, `1000`은 같고
부호·단위·소수·공백은 거부한다. 덧셈은 오버플로를 검사하며 불일치는 거부한다. 표현식
평가기나 암묵적인 총계 계산은 없다.

MCP에서는 `hwp_slots`에 `path`, `forms: true`, `occurrences: true`를 주면 같은 목록을 받는다.
`hwp_edit`에는 `input`, `output`, `apply_form_bindings` **객체** 하나를 전달한다. 배열이 아니다.
그 객체는 위의 `plan`, `values`, `receipt`, 선택적인 `preview`를 담고 `op`는 제외한다.
스키마 검사·쓰기 경로 검사를 거쳐 같은 ops 정규화와 엔진을 사용한다. MCP `verify` 플래그는
없으며 네이티브 검증이 연산에 포함된다.

필드·상한·증거 경계는 [바인딩 설계](../design/25-form-binding-plan.ko.md)와 버전 계약
`schemas/form-discovery-v1.schema.json`, `schemas/form-binding-plan-v1.schema.json`을 참고한다.

### 문단을 새 쪽에서 시작하기와 공백 들여쓰기 바꾸기

양식 쪽 나눔에는 문단 단위 연산 세 개가 있다. 문단 주소는 `ahwp cat doc.hwpx --format json
--with-segments --segments v2`에서 얻는다. 각 연산은 `--ops`의 한 항목이며, MCP `hwp_edit`에서는
같은 이름의 배열이다.

```json
[
  {"op":"set_break_before","address":{"at":{"section":0,"paragraph":5}},"value":"on"},
  {"op":"set_indent_level","address":{"at":{"section":0,"paragraph":7}},
   "level":2,"step":"5mm","hanging":"3mm"}
]
```

- `set_break_before`는 본문 문단 한 개의 '문단 앞 쪽 나눔'을 켜거나(`on`) 끈다(`off`). HWP에서는
  문단 머리의 나눔 종류, HWPX에서는 `pageBreak`이다. 한글이 저장한 정품 파일에서 나타나는 위치가
  이 머리 비트뿐이므로, 표 셀 안의 문단과 구역의 첫 문단은 원인과 해결 방법을 알리며 거부한다.
  단 나눔은 제공하지 않는다. 줄 배치 캐시는 쪽 기준 상대 위치이므로, 변경하면 그 문단과 다음 쪽
  나눔 문단 앞까지의 후속 문단의 저장된 줄 배치도 버려 새 쪽 맨 위부터 다시 계산한다(변경이 없는
  반복은 아무것도 건드리지 않는다). 이미 적용된 요청을 반복하면 성공하고 아무것도 바꾸지 않는다.
- `set_indent_level`은 문단 한 개의 앞쪽 공백 문자(ASCII 공백, 전각 공백, 줄바꿈 없는 공백)만
  지우고 들여쓰기를 문단 모양에 둔다. `margin_left`는 `level` 곱하기 `step`이고 첫 줄은
  `hanging`(기본 `0mm`)만큼 내어 쓴다. `level`은 0에서 7이다. 단어, 안쪽 간격, 라벨은 입력한
  그대로다. 문단 맨 앞의 보이지 않는 컨트롤(구역의 첫 문단이 가진 구역·단 정의, 필드 시작, 책갈피,
  쪽 컨트롤)은 제자리에 두고 그 뒤의 공백을 지운다. 남은 첫 글자는 원래의 글자 모양을 유지하므로
  굵은 라벨 앞의 일반 공백을 지워도 라벨이 굵기를 잃지 않는다. 보이는 선행 요소(자동 번호, 탭,
  개체) 뒤의 공백은 구분자이므로 남기고, 공백만 있는 문단은 그대로 둔다. 형광펜·교정 부호 같은
  HWP 구간 표시(range tag)가 있는 문단은, 표시가 글자 위치를 가리키는데 그 위치를 옮기지 않으므로
  (#470) 공백을 지워야 할 때 거부한다. 표시를 먼저 지우거나, 아무것도 지우지 않는 `set_para`의
  `left_mm`·`indent_mm`을 쓴다. 두 번째 실행은 아무것도 지우지 않고 문단 모양도 추가하지 않으며
  문단의 네이티브 줄 배치를 그대로 두고 `pieces_touched` 0으로 보고한다.
- `keep_together`(`keep_with_next`, `keep_lines`, `break_before_shape`)는 아무것도 쓰기 전에 항상
  거부한다. 쓸 수 있는 한글 저장 파일 중 이 문단 모양 비트를 켠 것이 없어, 추측 없이는 저장 방식을
  정할 수 없다. 거부를 풀려면 한글에서 해당 옵션을 켜 저장한 문서를 받아 바이트를 먼저 비교해야
  한다. 새 쪽에서 시작하려면 `set_break_before`를 쓴다.

두 연산 모두 문단 하나를 주소로 가리키므로, 낡은 `id`는 다른 주소 지정 연산처럼 거부된다.
`set_format`의 run 주소는 일괄 편집 이전 문서를 기준으로 해석되므로, 같은 문단의 앞쪽 공백을
지우거나(`set_indent_level`) 글을 바꾸는 연산이 먼저 오는 일괄 편집은 원인을 알리며 아무것도
쓰지 않고 거부한다. `set_format`을 앞에 두거나 편집을 나누어 실행한다.
결과는 구조 기준이며, 이 문서는 한글에서 보이는 쪽 수를 보장하지 않는다. 근거와 한계는
`docs/design/07-hangul-compat-rules.ko.md`(J1~J3)에 있다.

### 거부된 편집 분류

`ahwp edit`가 거부되어도 `--report`로 지정한 `edit-report-v1` 파일은 기록되고, `--dry-run`도
보고서를 출력한다. 보고서(스키마 버전 1.1)는 최상위 `refusal` 객체를 담고, 연산 하나가
거부했으면 그 연산 항목도 `status: "failed"`와 함께 같은 객체를 담는다. MCP `hwp_edit`는 오류
결과의 `structuredContent.refusal`로 같은 객체를 돌려준다. stderr는 사람이 읽는 한국어 문장을
그대로 유지하고, MCP 오류 텍스트도 같은 문장을 담는다. `refusal.message`는 코드마다 정해진 고정
문장이며 요청 내용을 인용하지 않는다(#348). 인자를 다 읽은 뒤 나온 MCP 거부는 `report` 인자
경로에도 같은 보고서를 쓴다. MCP 인자를 읽는 단계에서 나온 거부(예: `keep_together`)는
`structuredContent`로만 돌아오고, 아직 편집 계획이 없으므로 `report` 인자 경로에는 파일을 쓰지
않는다. 그 밖의 인자 오류(형식이 잘못되었거나 허용 경로 밖인 인자)는 `structuredContent.refusal`
없이 돌아올 수 있다. `apply_form_bindings` 배치는 거부되어도 receipt만 남긴다.

```json
{"code":"insert_blocks.unknown_style","values":{"style":"Heading 9","missing_style":"refuse"},"message":"..."}
```

분기는 `message`가 아니라 `code`로 한다. 사용자가 입력한 값(앵커, 스타일 이름)은 `values`에
입력 그대로 들어간다. 이슈 경로는 세그먼트 목록이라 `a; layout_overflow at $.z: b`라는 이름의
스타일도 세그먼트 하나로 남아 두 번째 이슈처럼 보일 수 없다.

| `code` | `values` |
| --- | --- |
| `range_tag_shift` | 없음 |
| `keep_together_unsupported` | `op_index`, `requested`(keep 필드 이름) |
| `section_owner_insert`, `section_owner_move` | 없음 |
| `add_table.anchor_not_found` | `anchor` |
| `insert_blocks.unknown_style` | `style`, `missing_style` |
| `insert_blocks.invalid_definition` | `style`와 `detail`, 또는 `issues`와 `container_width_mm` |
| `insert_blocks.style_not_paragraph`, `insert_blocks.duplicate_style`, `insert_blocks.style_cycle` | `style` |
| `insert_blocks.style_id_overflow` | `style`, `style_id`, `max_style_id` |
| `layout_overflow`, `insert_blocks.invalid_spec` | `issues`(`code`, 목록인 `path`, `message`, 넘친 경우 `measured_width_mm`), `container_width_mm` |
| `preservation_loss` | `events`(`code`, `resource`, `disposition`, `count`) |
| `unapplied_requests`, `no_applicable_edits` | 없음. 실패한 `ops` 항목이 놓친 요청을 알려 준다 |
| `unclassified` | 없음. 위에 없는 모든 거부 |

### 보존 손실 원장

`convert`·`merge`·`split`은 보존하지 못한 항목을 모두 `hwp-preservation-report-v1` 원장에
기록한다. CLI에서는 `--loss-report {file.json}`를 줄 때에만 원장이 파일로 기록되고(손실이
있으면 stderr에 한 줄 요약이 출력된다), MCP에서는 모든 `hwp_merge`·`hwp_split` 응답의
`preservation` 필드로 돌아온다. `--strict`는 원장이 비어 있지 않으면 발행을
거부하지만, 두 명령 모두 기본값이 아니라 선택 사항이다. 병합은 첫 입력 이후의 패키지
passthrough를 항상 버리기 때문에, 기본값을 fail-closed로 두면 지극히 평범한 병합조차 거부된다.
손실이 없다고 가정하지 말고 원장을 확인해야 한다.

### 공문서 린트

`ahwp lint {file}` / `hwp_lint`는 `.md`·`.hwp`·`.hwpx` 원본에 대해 한국어 공문서 표기·구조 규칙
열 가지를 검사하고, `--json`을 주면 `hwp-lint-report-v1` 계약
(`rule_id`/`severity`/`line`/`col`/`message`)을 출력한다. 공문서가 에이전트 손을 떠나기 전의
게이트로 쓰고, 지적 하나로 파이프라인을 멈춰야 한다면 `--strict`를 함께 준다.

### 종료 코드

관례가 의도적으로 다르기 때문에 "0이 아니면 실패"라는 해석은 틀렸다.

| 명령 | 관례 |
|---|---|
| `compare` | diff(1) 관례: 0은 동일, 1은 차이 발견, 2는 실행 자체가 실패 |
| `lint` | 항상 0; `--strict`일 때만 error 등급 지적에서 1 |
| `grep` | 일치가 없으면 1 — 오류가 아니라 정상 결과 |
| `validate`, `new --strict`, `convert --strict`, `merge --strict`, `split --strict` | 성공은 0, 실패는 0이 아님 |

MCP에는 종료 코드가 없다. `hwp_compare`는 `identical`을, `hwp_grep`은 `count`를 돌려주며 두
경우 모두 `isError`는 false이므로, 에이전트는 호출이 실패했다고 보지 말고 해당 필드를 읽어야
한다.

### 암호

호출 단위 `password` 인수를 받는 MCP 도구는 여섯 개다: `hwp_read`·`hwp_convert`·`hwp_render`·
`hwp_merge`·`hwp_split`·`hwp_compare`. 이 값은 호출 사이에 캐시되지 않으며 알림에서도 제거된다.
CLI에서는 프로세스 인수에 노출되는 `--password`보다 `--password-stdin`을 우선한다. 암호가
리포트·영수증·생성 파일·명령 기록·지속 환경변수에 남지 않게 해야 한다.

### 환경 변수

- `HWP_FONT_DIR` — `--font-dir`를 주지 않았을 때 쓰는 기본 폰트 디렉터리. 렌더와 PDF 출력에는
  CJK 폰트가 필요한데, 클라이언트의 인수 배열에 `--font-dir`를 끼워 넣는 것보다 이 쪽이 편할
  때가 많다.
- `HWP_LANG` — 도움말과 메시지 언어(`en` / `ko`). `--lang`이 이 값보다 우선한다.
- `HWP_BIN_DIR` — claude.ai 번들의 `bootstrap.sh`가 바이너리를 설치할 위치(기본
  `~/.local/bin`).

## Claude Code / Claude Desktop

MCP 서버를 등록한다(Claude Code: `.mcp.json` 또는 `claude mcp add`; Claude Desktop:
`claude_desktop_config.json`).

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

도구가 접근해야 하는 디렉터리마다 `--root`를 반복한다. `--root`가 하나도 없으면 서버는
제한 없이 동작하며 시작 시 stderr에 한 줄 경고를 출력한다.

Claude Code는 에이전트 스킬을 바로 소비할 수도 있다.

```sh
ahwp skill export --install claude-code   # ~/.claude/skills/hwp/ 아래에 스킬 트리 설치
```

## Codex CLI

`~/.codex/config.toml`에 추가한다.

```toml
[mcp_servers.hwp]
command = "ahwp"
args = ["mcp", "--root", "/path/to/workspace"]
```

에이전트 스킬 설치:

```sh
ahwp skill export --install codex         # ~/.codex/skills/hwp/ 아래에 스킬 트리 설치
```

## Codex cloud

Codex cloud 환경은 셋업 스크립트로 컨테이너를 만든다. 거기에 바이너리를 설치한다(Rust
툴체인 불필요, 사전 빌드 릴리스 아카이브를 받는다).

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/agent-hwp/main/scripts/install.sh | sh
```

이후 MCP 서버 등록은 위 Codex CLI와 같다.

## Kiro / Kimi

둘 다 Claude Code와 같은 형태의 표준 stdio MCP 서버 등록을 받는다.

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

각 클라이언트의 MCP 설정에 넣는다(Kiro: `.kiro/settings/mcp.json`; Kimi: 자체 설정의 MCP 섹션).
스킬 디렉터리 관례는 클라이언트마다 다르므로 `ahwp skill export -o {dir}`로 트리를 원하는 위치에
풀어내고 클라이언트가 그 경로를 가리키게 한다(이 명령은 `{dir}` 아래에 `SKILL.md`,
`SKILL.ko.md`, `official-documents(.ko).md`, `references/`, `templates/`를 쓰며, `-o`를 생략하면
`./hwp`에 쓴다). export는 심볼릭 링크 대상을 거부하고 트리를 디렉터리 교체 한 번으로 발행하며
발행이 실패하면 이전 트리를 복원하므로, 기존 스킬 디렉터리 위에 다시 설치해도 안전하다.

## claude.ai (웹)

claude.ai 코드 실행 샌드박스는 네트워크가 레지스트리로 제한되어 런타임에 바이너리를 받을
수 없다. 그래서 매 릴리스에 `hwp-skill-claude-web.zip`을 함께 올린다. zip 루트의
`SKILL.md`, `bootstrap.sh`, Linux x86_64 `bin/ahwp`, 라이선스 파일(`LICENSE-MIT`, `LICENSE-APACHE`,
`NOTICE`)이 포함된다.

1. [최신 릴리스](https://github.com/STAIxBWLB/agent-hwp/releases)에서
   `hwp-skill-claude-web.zip`과 그 옆에 함께 게시되는 `hwp-skill-claude-web.zip.sha256`을 받아
   아카이브 해시를 대조한다.
2. claude.ai에서 Settings → Capabilities → Skills로 가서 zip을 업로드한다.
3. 코드 실행이 켜진 채팅에서 세션당 한 번 `bash bootstrap.sh`를 실행한다. 번들된 바이너리를
   `~/.local/bin`(`HWP_BIN_DIR`로 변경 가능)에 `ahwp`로 설치하고 `ahwp --version`으로
   동작을 확인한 뒤 MCP 등록 스니펫을 출력한다. 이후 Claude는 샌드박스 안에서 `ahwp`를 CLI로
   직접 구동한다. 여기서는 CLI만 쓸 수 있으므로 `merge`·`split`·`compare`도 MCP 도구가
   아니라 명령으로 실행한다.

## Amazon Quick Desktop

Amazon Quick Desktop은 `ahwp mcp`를 로컬 stdio 커넥터로 실행할 수 있다. HWP 도구 23개가
모두 노출되는 연결을 실기 확인했다. Quick 릴리스에 따라 UI 이름은 달라질 수 있으며 아래 명칭은
현재 Desktop 흐름 기준이다.

바이너리 검증, 에이전트 지침, 실제 생성·검증 smoke test, 증상별 복구까지 포함한 Windows 중심
복사·실행 절차는 전용 [Amazon Quick Desktop 가이드](amazon-quick-desktop.ko.md)를 사용한다. 아래
요약은 다른 AI 클라이언트와 비교하기 위한 짧은 레퍼런스로 유지한다.

### 1. 최신 바이너리 하나로 정리

커넥터에는 실행 파일 절대 경로를 넣는다. Quick의 PATH 앞쪽에 남은 구버전을 실행하는 문제를
방지할 수 있다.

```sh
command -v ahwp
ahwp --version
# zsh/bash: 중복 설치 전부 확인
type -a ahwp
```

Apple Silicon Homebrew에서는 흔히 `/opt/homebrew/bin/ahwp`가 나온다. 이는 예시이지 모든 시스템의
고정 경로가 아니다. `~/.cargo/bin/ahwp`가 구버전 중복본이고 Homebrew 설치본을 사용할 예정이면
`cargo uninstall agent-hwp`로 Cargo 등록을 제거한 뒤 다시 확인한다.

### 2. 로컬 MCP 커넥터 추가

**Settings → Capabilities → Connectors → + Create → MCP server → Local**로 이동해 입력한다.

| 항목 | 값 |
|---|---|
| Name | `hwp` |
| Command | `command -v ahwp`에서 확인한 절대 경로 |
| Arguments (macOS 예시) | `mcp --font-dir /System/Library/Fonts --root /path/to/workspace` |
| Description | `Read, write, edit, render, validate, and convert HWP/HWPX documents.` |
| Timeout | `30`초(일반적으로 기본값이면 충분) |

`/System/Library/Fonts`는 macOS CJK 폰트 경로다. 다른 시스템에서는 알맞은 폰트 경로로
바꾸고, 렌더링이 필요 없으면 `--font-dir`를 생략할 수 있다. 도구가 접근해야 하는 위치마다
`--root /another/authorized/directory`를 반복한다. 무제한 파일 접근이 의도된 경우가 아니라면
모든 root를 생략하지 않는다.

**Test connection**을 선택하고 Quick의 명령 실행 확인 창에서 **Add server**를 승인한다. 테스트는
**Connected**, **23 tools available**을 표시해야 한다. 이어서 **Add MCP**를 선택하고 확인 창을
다시 승인한 뒤 연결을 새로고침한다. `Hwp`가 활성화되어 있고 **23 tools, Connected**로 표시되는지
확인한다.

같은 설정의 import JSON:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "/absolute/path/to/ahwp",
      "args": [
        "mcp",
        "--font-dir",
        "/System/Library/Fonts",
        "--root",
        "/path/to/workspace"
      ]
    }
  }
}
```

#### Windows 샌드박스 호환 설정

Windows에서는 커넥터를 등록하기 전에 시스템이 제공하는 Low 무결성 디렉터리 아래에 전용 자식을
만든다.

```powershell
$QuickHwpRoot = Join-Path $env:USERPROFILE 'AppData\LocalLow\hwp-quick-workspace'
New-Item -ItemType Directory -Path $QuickHwpRoot -Force | Out-Null
icacls.exe $QuickHwpRoot
```

출력에 상속된 Low mandatory label이 있어야 한다. Arguments는 환경 변수를 확장하지 않으므로 아래
커넥터 JSON의 `YOUR_NAME`을 실제 Windows 계정 폴더명으로 바꾼다.

```json
{
  "mcpServers": {
    "hwp": {
      "command": "C:\\absolute\\path\\to\\ahwp.exe",
      "args": [
        "mcp",
        "--font-dir",
        "C:\\Windows\\Fonts",
        "--root",
        "C:\\Users\\YOUR_NAME\\AppData\\LocalLow\\hwp-quick-workspace"
      ]
    }
  }
}
```

각 인자는 JSON 배열의 별도 항목으로 유지하고 Windows 경로에 셸 따옴표를 덧붙이지 않는다.
Quick의 **Local folders and access permissions**는 내장 읽기·검색 도구를 제어하며 로컬 MCP 자식의
쓰기 무결성은 바꾸지 않는다. Quick은 `ahwp.exe`를 Low mandatory integrity(`S-1-16-4096`)로
시작하지만 `C:\TEMP`와 `%LOCALAPPDATA%\Temp`는 보통 Medium이다. 이 불일치 때문에 커넥터 탐색은
성공해도 첫 atomic output staging 디렉터리가 `Access is denied (os error 5)`로 거부된다.
`AppData\LocalLow`의 자식은 광범위한 ACL 변경 없이 필요한 Low 레이블을 상속한다. HWP 작업 전에
입력을 전용 root로 옮기거나 복사하고, MCP 입력과 출력을 그 아래에 유지한 다음 검증된 artifact를
승인된 목적 폴더로 복사한다.

자동 비활성화된 커넥터의 설정을 바꾼 뒤에는 명시적으로 다시 활성화한다. 복구되면
**Connected**, **23 tools available**가 표시되고 새로고침 뒤에도 활성 상태를 유지한다.

### 3. publish-safe HWP 스킬 설치

```sh
ahwp skill export --install amazon-quick
```

명령은 `~/.quickwork/profiles.json`을 읽어 유효한 `last_active` 프로필을 우선 선택하고, 유효한
프로필이 하나뿐이면 그것을 사용한다. 해당 프로필 안의 `skills/hwp/SKILL.md`만 쓴다 — 공문서
파일(`SKILL.ko.md`, `official-documents(.ko).md`, `references/`, `templates/`)은 Quick
경로에 설치되지 **않으며**, 명령이 그 사실을 알리는 문구를 출력한다. 에이전트나 커넥터를
생성하거나 publish하지 않는다.

프로필이 여러 개이거나 자동 선택이 불가능하면 프로필 ID 또는 절대 프로필 디렉터리를 지정한다.

```sh
ahwp skill export --install amazon-quick --quick-profile enterprise-example
ahwp skill export --install amazon-quick --quick-profile /absolute/path/to/quick/profile
```

Quick 실행 중에 설치했다면 Quick을 재시작하거나 새로고침한다.

### 4. 도구 사용

일반 Quick 채팅이나 HWP 전용 에이전트에서 다음과 같이 요청할 수 있다.

- "이 HWP 파일을 요약하고 표 목록을 보여줘."
- "이 HWPX 문서를 Markdown으로 변환해줘."
- "이 Markdown으로 HWPX 보고서를 만들고 결과를 검증해줘."
- "'초안'을 '최종'으로 바꾸고 1번 표 2행 3열을 수정한 뒤 1쪽을 렌더해줘."
- "템플릿 슬롯을 확인하고 이름과 날짜를 채운 뒤 결과를 검증해줘."
- "이 HWPX 세 개를 이 순서대로 병합하고 보존하지 못한 항목을 알려줘."
- "이 문서를 구역별 파일로 나누고 조각 경로를 알려줘."
- "이 두 문서를 비교해서 어느 문단이 다른지 알려줘."
- "이 공문서를 린트해서 error 등급 지적을 모두 보여줘."

쓰기 작업 뒤에는 `hwp_validate`를 호출해야 한다. 시각 결과가 중요하면 `hwp_render`도 사용하고,
공문서를 넘기기 전에는 `hwp_lint`를 거치며, `hwp_merge`·`hwp_split` 응답의 `preservation` 필드는
손실이 없다고 가정하지 말고 반드시 읽는다.

### 5. HWP 전용 에이전트 하나로 구성

중복 이름과 오래된 지시를 피하기 위해 같은 역할의 에이전트는 하나만 유지한다. HWP MCP 커넥터를
활성화하고, 설치된 `hwp` 스킬 사용, 쓰기 후 검증, 설정된 root 준수를 instructions에 명시한다.
OneDrive/SharePoint 커넥터는 원본이나 결과가 그 위치에 있을 때만 선택적으로 추가한다.

publish 중 `assetDescriptor contains prohibited HTML/script content` 오류가 발생하면 최신 `ahwp`
바이너리에서 스킬을 다시 설치한다. 현재 스킬은 `{file}` 같은 중괄호 placeholder를 사용하며 Quick이
HTML로 오인할 수 있는 angle-bracket markup을 포함하지 않는다.

### Desktop 확인 체크리스트

- `ahwp --version`이 의도한 최신 바이너리를 표시함
- 커넥터 테스트가 **Connected**, **23 tools available**을 표시함
- 새로고침 뒤에도 커넥터가 활성화되어 있고 **23 tools, Connected**를 표시함
- 테스트 HWPX에서 `hwp_new`, `hwp_read`, `hwp_validate`, `hwp_render`가 성공함(Windows에서는
  설정된 LocalLow root 아래에서 테스트. 예: `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace`)
- 같은 root 아래에 둔 그 문서의 사본 두 개로 `hwp_merge`, `hwp_split`, `hwp_compare`가 성공하며,
  두 문서가 다를 때 `hwp_compare`가 오류가 아니라 `identical`을 보고함
- HWP 전용 에이전트가 하나만 존재하며 prohibited HTML/script 오류 없이 publish됨

## Amazon Quick Web

Quick Web은 클라우드에서 실행되므로 로컬 stdio 프로세스를 시작하거나 Desktop 로컬 파일시스템에
접근할 수 없다. 현재는 문서를 Quick이 읽을 수 있는 형식으로 변환한 뒤 결과물을 업로드한다:

```bash
ahwp convert input.hwp -o output.docx   # 또는: -o output.pdf
```

편집된 결과물은 다운로드한 뒤 필요하면 `ahwp convert`로 다시 변환한다. 제공되는 경우
Desktop/Outpost 실행 경로를 대신 사용할 수 있다. 로컬 `ahwp mcp` 프로세스를 네트워크에 직접
노출하면 안 된다.

Web 네이티브 연동에는 인증된 Streamable HTTP MCP 서비스, tenant별 격리 저장소, 클라이언트 로컬
경로 인자 대신 content/artifact 전송이 필요하다. 이 릴리스에는 구현하지 않는다. 구현 계약은
[Remote MCP transport](../design/20-remote-mcp.ko.md)에 정리했고
[issue #52](https://github.com/STAIxBWLB/agent-hwp/issues/52)에서 추적한다.

## `hwpx` 스킬 흡수 완료

이 저장소는 **단일 번들** 스킬 [`skills/hwp/`](../../skills/hwp/SKILL.ko.md)를 제공한다.
바이너리 빠른 참조, MCP 사용법, 종료 코드와 보존 손실 원장 규약, 안전 규칙에 공문서 표면 —
마크다운 계약, 문서별 레시피, 규정 참고문서, 마크다운 템플릿 — 을 더했다. v0.12.0부터는
네이티브 편집 크로스워크([`references/editing-recipes.ko.md`](../../skills/hwp/references/editing-recipes.ko.md))를,
v0.13.0부터는 실제 문서로 검증한 편집 레시피 세 가지(analyze, edit-section, guard)를 함께
담고 있다.

별도 `STAIxBWLB/skills` 저장소의 기존 사용자 범위 스킬 `skills/hwpx`는 이 번들 스킬로
**흡수되어 퇴역했다**([skills#35](https://github.com/STAIxBWLB/skills/issues/35),
2026-08-27 종료). 더 이상 업스트림/다운스트림 구분은 없으며, 퇴역한 스킬은 설치하거나
호출하지 않는다. 기존 `./hwpx` 서브커맨드와 네이티브 커맨드의 패리티는
[23-hwpx-skill-absorption](../design/23-hwpx-skill-absorption.ko.md)의 매트릭스에 추론이 아닌
날짜 있는 증거로 검증되어 기록되어 있다.
