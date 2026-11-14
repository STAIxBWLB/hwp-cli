[한국어](mcp-server.ko.md) · [English](mcp-server.md)

# MCP 서버

`ahwp mcp`가 노출하는 도구, 클라이언트 설정 방법, 에이전트가 수행하는 read → edit → rewrite 왕복을 다룬다. 클라이언트별 설정은 [ai-integrations.ko.md](ai-integrations.ko.md)에, 모든 클라이언트가 따르는 규약은 [agent-conventions.ko.md](agent-conventions.ko.md)에 있다.

## MCP 서버 실행하기

`ahwp mcp`는 tokio나 SDK 없이 `serde_json`만으로 동기 JSON-RPC 2.0(stdio, 줄 단위)을 구현한 MCP
서버다. 프로토콜 버전은 `initialize`에서 협상한다: 클라이언트의 `protocolVersion`이
`2025-06-18`·`2025-03-26`·`2024-11-05` 중 하나면 그대로 돌려주고, 아니면 최신 지원 버전으로
응답한다. stdout은 프로토콜 전용이고 로그는 stderr로 나간다.

클라이언트별 설정(Claude Code/Desktop, Codex CLI/cloud, Kiro, Kimi, claude.ai 스킬 업로드,
Amazon Quick Desktop)과 번들 에이전트 스킬(`ahwp skill export`):
[docs/manual/ai-integrations.ko.md](ai-integrations.ko.md).
Windows 복사·실행 설정, 생성·검증 acceptance test, 재사용 가능한 에이전트 지침, 증상별 복구는 전용
[Amazon Quick Desktop 가이드](amazon-quick-desktop.ko.md)에 정리했다.

Amazon Quick Desktop은 로컬 stdio 서버를 실행해 23개 도구를 모두 노출할 수 있다. 스킬 설치와
Windows `LocalLow` 워크스페이스 설정은
[AI 클라이언트 연동](ai-integrations.ko.md#amazon-quick-desktop)에 있다.

Amazon Quick Web은 로컬 stdio 프로세스를 시작할 수 없다. 이제 `ahwp serve`가 같은 도구를
컨테이너 배포용 HTTP로 제공한다(`POST /mcp`,
[배포 설계](../design/22-remote-mcp-deployment.ko.md) 참고). 다만 이 서버는 앞단에 신뢰된
edge가 있다고 전제하는 private hop이다. 인증된 Streamable HTTP, tenant 격리, artifact 전송은
여전히 [Remote MCP transport](../design/20-remote-mcp.ko.md)의 후속 작업이다.

### 노출 도구 (23종)

| 도구 | 필수 인자 | 기능 |
|---|---|---|
| `hwp_info` | `path` | 포맷/버전/속성/스트림 진단 |
| `hwp_read` | `path` | 본문 추출(`plain`/`markdown`/`json`/`html`/`csv`, 머리말·꼬리말/숨은 설명/세그먼트 옵션). UTF-8 byte 페이지네이션(기본 256 KiB, 최대 1 MiB) |
| `hwp_grep` | `path`, `pattern` | 문단 텍스트 검색. `{matches, count, truncated}` 반환 — 0건이어도 정상 결과 |
| `hwp_list_fields` | `path` | 필드/누름틀 목록 |
| `hwp_list_bookmarks` | `path` | 책갈피(bokm) 목록 |
| `hwp_styles` | `path` | 이름 있는 스타일 목록 (해석된 속성, `used_by` 세그먼트 id), `hwp-styles-v1` 형식 |
| `hwp_slots` | `path` | `{{name}}` 자리표시자 목록 |
| `hwp_render` | `path` | 페이지를 PNG로 렌더(base64, 응답 최대 16 MiB)하거나 `output_path`로 PNG/SVG/PDF 파일 작성(dpi 36..=600) |
| `hwp_edit` | `input`, `output` | typed JSON 작업으로 strict atomic 편집 |
| `hwp_convert` | `input`, `output` | 포맷 변환(MCP의 `strict` 기본값은 true) |
| `hwp_new` | `output` | markdown/JSON IR과 metadata에서 새 문서 생성 |
| `hwp_compose` | `output`, `spec`/`spec_path` | DocumentSpec v1/v2를 CLI와 같은 경로로 합성 |
| `hwp_template` | `output`, `template`(+`data`) | TemplateSpec/Data v1을 bounded expansion |
| `hwp_certify` | `input`, `policy`, `report` | 인증 실행 및 원자적 리포트 게시 |
| `hwp_fill` | `input`, `output`, `values` | hwpx 템플릿 `{{name}}` 치환(패키지 보존) |
| `hwp_diff` | `input`, `ref` | 지정 페이지를 렌더해 기준 PNG와 비교 |
| `hwp_validate` | `path` | 구조 검증 `{valid, errors, warnings}` |
| `hwp_lint` | `path` | 공문서 표기법·구조 규칙 10개를 `hwp-lint-report-v1` 리포트로 반환 |
| `hwp_merge` | `inputs`, `output` | 문서 두 개 이상을 합치고 보존 손실 원장을 반환 |
| `hwp_split` | `input`, `out_dir` | Section 단위(또는 `pages` 범위)로 나누거나 `from`/`to`로 형식 있는 범위 하나를 추출하고, 발행된 조각 경로와 범위 추출 시 `hwp-range-extraction-v1` 보고서를 반환 |
| `hwp_compare` | `a`, `b` | 읽기 전용 문단·구조 비교를 `hwp-compare-report-v1`으로 반환. 차이가 있어도 `isError`가 아니므로 `identical`을 읽는다 |
| `hwp_put_file` | `name`, `content` | base64 콘텐츠를 세션 워크스페이스에 저장. 원격 배포에 기존 문서를 넣는 유일한 경로다. 도구 인자는 경로를 받으므로 먼저 올린 뒤 그 이름을 넘긴다. 복호 512 KiB 상한 |
| `hwp_get_file` | `path` | 워크스페이스 파일을 base64로 반환(영수증 블록 + embedded resource). 상한을 넘으면 자르지 않고 거절한다. 중간 산출물은 워크스페이스에 두고 최종 결과물만 받는다. 반환된 base64는 클라이언트 메시지 스트림에 쌓인다 |

### 클라이언트 설정 예

```json
{
  "mcpServers": {
    "hwp": {
      "command": "ahwp",
      "args": ["mcp", "--font-dir", "<repo>/fonts", "--root", "<repo>"]
    }
  }
}
```

`--root`(반복 가능)는 도구가 다루는 모든 파일 경로 — 입력·출력, 중첩 이미지/부분 경로, spec
`base_dir`, 호출당 `font_dir` — 를 지정 디렉터리 아래로 제한한다. 이 루트는 compose/template spec
내부 참조에도 적용되어, spec이 참조하는 이미지·비주얼 에셋과 `reference_hwpx` 패키지도 허용 루트
아래여야 한다. `--root` 없이 실행하면 종전처럼
제한 없이 동작하며, 기동 시 stderr에 한 줄 경고를 출력한다.

### read → edit → rewrite 왕복

1. **읽기** `hwp_read`(`format=json`)가 문서 전체를 IR로 내보낸다. 표·이미지 참조·서식·미해석 레코드까지 담긴다.
2. **편집** `hwp_edit`로 치환·셀 설정·필드 채우기를 적용한다. IR만 바꾸므로 이미지·서식·opaque 레코드가
   보존되고, 편집된 문단의 줄 배치만 무효화되어 writer가 재합성한다.
3. **확인** `hwp_render`가 결과 페이지를 PNG로 돌려주어 에이전트가 변경을 눈으로 검증한다. `hwp_diff`로
   한글 기준 렌더와 정량 비교할 수 있다.

편집된 hwp는 writer 합성 경로에서 한글 문단 불변식(줄 배치·문단끝 `0x0d`·nchars 등)을 다시 세우므로
한글에서 정상 문서로 열린다.
