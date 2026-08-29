[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI 클라이언트 연동

`hwp`는 AI 클라이언트용 연동 표면을 두 가지 제공한다.

- MCP를 구사하는 클라이언트용 **MCP stdio 서버**(`hwp mcp`, 20개 도구)
- CLI와 MCP 사용법을 에이전트에게 가르치는 **에이전트 스킬**(이 저장소의 `skills/hwp/` 트리).
  바이너리에 임베드되어 있으며 `hwp skill export`로 디렉터리 형태로 풀어낼 수 있다:
  `SKILL.md`, `SKILL.ko.md`, 공문서 가이드(`official-documents(.ko).md`), `references/`,
  `templates/`.

어느 표면을 쓰든 `hwp mcp --root {dir}`로 도구가 만지는 파일 경로를 지정 디렉터리 아래로
제한하고, 에이전트가 쓴 파일은 `hwp validate`로 검증하는 것을 권장한다.

## 모든 클라이언트에 공통인 규약

아래 내용은 어느 클라이언트가 구동하든 CLI와 MCP 서버에 똑같이 적용된다.

### 문서 단위 작업

v0.13.0부터 문서 하나씩이 아니라 문서 전체를 다루는 작업을 지원한다.

- `hwp merge {inputs...} -o {out}` / `hwp_merge` — 문서 두 개 이상을 인자 순서대로 이어 붙이며
  입력 하나가 Section 하나가 된다. 쪽·각주·개요 번호는 각 입력의 시작·계속 설정을 그대로
  유지하므로 병합한 뒤 다시 확인해야 한다.
- `hwp split {in} --out-dir {dir}` / `hwp_split` — 기본은 Section 하나당 조각 하나이고,
  `--pages`를 주면 쪽 범위로 나눈다. 쪽 경계는 다시 계산한 값이 아니라 한컴이 저장한 레이아웃
  캐시에서 얻은 추정값이라 한컴 자체 페이지 나눔과 다를 수 있다.
- `hwp compare {a} {b}` / `hwp_compare` — 문서 두 개의 문단·구조 차이를 보고하며 두 입력 모두
  수정하지 않는다. 렌더를 한컴 참조 PNG와 비교하는 `hwp diff`와는 다른 명령이다.

### 보존 손실 원장

`convert`·`merge`·`split`은 보존하지 못한 항목을 모두 `hwp-preservation-report-v1` 원장에
기록한다. CLI에서는 `--loss-report {file.json}`로 기록되고, MCP에서는 모든 `hwp_merge`·
`hwp_split` 응답의 `preservation` 필드로 돌아온다. `--strict`는 원장이 비어 있지 않으면 발행을
거부하지만, 두 명령 모두 기본값이 아니라 선택 사항이다. 병합은 첫 입력 이후의 패키지
passthrough를 항상 버리기 때문에, 기본값을 fail-closed로 두면 지극히 평범한 병합조차 거부된다.
손실이 없다고 가정하지 말고 원장을 확인해야 한다.

### 공문서 린트

`hwp lint {file}` / `hwp_lint`는 `.md`·`.hwp`·`.hwpx` 원본에 대해 한국어 공문서 표기·구조 규칙
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
      "command": "hwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

도구가 접근해야 하는 디렉터리마다 `--root`를 반복한다. `--root`가 하나도 없으면 서버는
제한 없이 동작하며 시작 시 stderr에 한 줄 경고를 출력한다.

Claude Code는 에이전트 스킬을 바로 소비할 수도 있다.

```sh
hwp skill export --install claude-code   # ~/.claude/skills/hwp/ 아래에 스킬 트리 설치
```

## Codex CLI

`~/.codex/config.toml`에 추가한다.

```toml
[mcp_servers.hwp]
command = "hwp"
args = ["mcp", "--root", "/path/to/workspace"]
```

에이전트 스킬 설치:

```sh
hwp skill export --install codex         # ~/.codex/skills/hwp/ 아래에 스킬 트리 설치
```

## Codex cloud

Codex cloud 환경은 셋업 스크립트로 컨테이너를 만든다. 거기에 바이너리를 설치한다(Rust
툴체인 불필요, 사전 빌드 릴리스 아카이브를 받는다).

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

이후 MCP 서버 등록은 위 Codex CLI와 같다.

## Kiro / Kimi

둘 다 Claude Code와 같은 형태의 표준 stdio MCP 서버 등록을 받는다.

```json
{
  "mcpServers": {
    "hwp": {
      "command": "hwp",
      "args": ["mcp", "--root", "/path/to/workspace"]
    }
  }
}
```

각 클라이언트의 MCP 설정에 넣는다(Kiro: `.kiro/settings/mcp.json`; Kimi: 자체 설정의 MCP 섹션).
스킬 디렉터리 관례는 클라이언트마다 다르므로 `hwp skill export -o {dir}`로 트리를 원하는 위치에
풀어내고 클라이언트가 그 경로를 가리키게 한다(이 명령은 `{dir}` 아래에 `SKILL.md`,
`SKILL.ko.md`, `official-documents(.ko).md`, `references/`, `templates/`를 쓰며, `-o`를 생략하면
`./hwp`에 쓴다). export는 심볼릭 링크 대상을 거부하고 트리를 디렉터리 교체 한 번으로 발행하며
발행이 실패하면 이전 트리를 복원하므로, 기존 스킬 디렉터리 위에 다시 설치해도 안전하다.

## claude.ai (웹)

claude.ai 코드 실행 샌드박스는 네트워크가 레지스트리로 제한되어 런타임에 바이너리를 받을
수 없다. 그래서 매 릴리스에 `hwp-skill-claude-web.zip`을 함께 올린다. zip 루트의
`SKILL.md`, `bootstrap.sh`, Linux x86_64 `bin/hwp`가 포함된다.

1. [최신 릴리스](https://github.com/STAIxBWLB/hwp-cli/releases)에서
   `hwp-skill-claude-web.zip`과 그 옆에 함께 게시되는 `hwp-skill-claude-web.zip.sha256`을 받아
   아카이브 해시를 대조한다.
2. claude.ai에서 Settings → Capabilities → Skills로 가서 zip을 업로드한다.
3. 코드 실행이 켜진 채팅에서 세션당 한 번 `bash bootstrap.sh`를 실행한다. 번들된 바이너리를
   `~/.local/bin`(`HWP_BIN_DIR`로 변경 가능)에 설치하고 `hwp --version`으로 동작을 확인한 뒤
   MCP 등록 스니펫을 출력한다. 이후 Claude는 샌드박스 안에서 `hwp`를 CLI로 직접 구동한다.
   여기서는 CLI만 쓸 수 있으므로 `merge`·`split`·`compare`도 MCP 도구가 아니라 명령으로
   실행한다.

## Amazon Quick Desktop

Amazon Quick Desktop은 `hwp mcp`를 로컬 stdio 커넥터로 실행할 수 있다. HWP 도구 20개가
모두 노출되는 연결을 실기 확인했다. Quick 릴리스에 따라 UI 이름은 달라질 수 있으며 아래 명칭은
현재 Desktop 흐름 기준이다.

바이너리 검증, 에이전트 지침, 실제 생성·검증 smoke test, 증상별 복구까지 포함한 Windows 중심
복사·실행 절차는 전용 [Amazon Quick Desktop 가이드](amazon-quick-desktop.ko.md)를 사용한다. 아래
요약은 다른 AI 클라이언트와 비교하기 위한 짧은 레퍼런스로 유지한다.

### 1. 최신 바이너리 하나로 정리

커넥터에는 실행 파일 절대 경로를 넣는다. Quick의 PATH 앞쪽에 남은 구버전을 실행하는 문제를
방지할 수 있다.

```sh
command -v hwp
hwp --version
# zsh/bash: 중복 설치 전부 확인
type -a hwp
```

Apple Silicon Homebrew에서는 흔히 `/opt/homebrew/bin/hwp`가 나온다. 이는 예시이지 모든 시스템의
고정 경로가 아니다. `~/.cargo/bin/hwp`가 구버전 중복본이고 Homebrew 설치본을 사용할 예정이면
`cargo uninstall hwp-cli`로 Cargo 등록을 제거한 뒤 다시 확인한다.

### 2. 로컬 MCP 커넥터 추가

**Settings → Capabilities → Connectors → + Create → MCP server → Local**로 이동해 입력한다.

| 항목 | 값 |
|---|---|
| Name | `hwp` |
| Command | `command -v hwp`에서 확인한 절대 경로 |
| Arguments (macOS 예시) | `mcp --font-dir /System/Library/Fonts --root /path/to/workspace` |
| Description | `Read, write, edit, render, validate, and convert HWP/HWPX documents.` |
| Timeout | `30`초(일반적으로 기본값이면 충분) |

`/System/Library/Fonts`는 macOS CJK 폰트 경로다. 다른 시스템에서는 알맞은 폰트 경로로
바꾸고, 렌더링이 필요 없으면 `--font-dir`를 생략할 수 있다. 도구가 접근해야 하는 위치마다
`--root /another/authorized/directory`를 반복한다. 무제한 파일 접근이 의도된 경우가 아니라면
모든 root를 생략하지 않는다.

**Test connection**을 선택하고 Quick의 명령 실행 확인 창에서 **Add server**를 승인한다. 테스트는
**Connected**, **20 tools available**을 표시해야 한다. 이어서 **Add MCP**를 선택하고 확인 창을
다시 승인한 뒤 연결을 새로고침한다. `Hwp`가 활성화되어 있고 **20 tools, Connected**로 표시되는지
확인한다.

같은 설정의 import JSON:

```json
{
  "mcpServers": {
    "hwp": {
      "command": "/absolute/path/to/hwp",
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
      "command": "C:\\absolute\\path\\to\\hwp.exe",
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
쓰기 무결성은 바꾸지 않는다. Quick은 `hwp.exe`를 Low mandatory integrity(`S-1-16-4096`)로
시작하지만 `C:\TEMP`와 `%LOCALAPPDATA%\Temp`는 보통 Medium이다. 이 불일치 때문에 커넥터 탐색은
성공해도 첫 atomic output staging 디렉터리가 `Access is denied (os error 5)`로 거부된다.
`AppData\LocalLow`의 자식은 광범위한 ACL 변경 없이 필요한 Low 레이블을 상속한다. HWP 작업 전에
입력을 전용 root로 옮기거나 복사하고, MCP 입력과 출력을 그 아래에 유지한 다음 검증된 artifact를
승인된 목적 폴더로 복사한다.

자동 비활성화된 커넥터의 설정을 바꾼 뒤에는 명시적으로 다시 활성화한다. 복구되면
**Connected**, **20 tools available**가 표시되고 새로고침 뒤에도 활성 상태를 유지한다.

### 3. publish-safe HWP 스킬 설치

```sh
hwp skill export --install amazon-quick
```

명령은 `~/.quickwork/profiles.json`을 읽어 유효한 `last_active` 프로필을 우선 선택하고, 유효한
프로필이 하나뿐이면 그것을 사용한다. 해당 프로필 안의 `skills/hwp/SKILL.md`만 쓴다 — 공문서
파일(`SKILL.ko.md`, `official-documents(.ko).md`, `references/`, `templates/`)은 Quick
경로에 설치되지 **않으며**, 명령이 그 사실을 알리는 문구를 출력한다. 에이전트나 커넥터를
생성하거나 publish하지 않는다.

프로필이 여러 개이거나 자동 선택이 불가능하면 프로필 ID 또는 절대 프로필 디렉터리를 지정한다.

```sh
hwp skill export --install amazon-quick --quick-profile enterprise-example
hwp skill export --install amazon-quick --quick-profile /absolute/path/to/quick/profile
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

publish 중 `assetDescriptor contains prohibited HTML/script content` 오류가 발생하면 최신 `hwp`
바이너리에서 스킬을 다시 설치한다. 현재 스킬은 `{file}` 같은 중괄호 placeholder를 사용하며 Quick이
HTML로 오인할 수 있는 angle-bracket markup을 포함하지 않는다.

### Desktop 확인 체크리스트

- `hwp --version`이 의도한 최신 바이너리를 표시함
- 커넥터 테스트가 **Connected**, **20 tools available**을 표시함
- 새로고침 뒤에도 커넥터가 활성화되어 있고 **20 tools, Connected**를 표시함
- 테스트 HWPX에서 `hwp_new`, `hwp_read`, `hwp_validate`, `hwp_render`가 성공함(Windows에서는
  설정된 LocalLow root 아래에서 테스트. 예: `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace`)
- 같은 root 아래에 둔 그 문서의 사본 두 개로 `hwp_merge`, `hwp_split`, `hwp_compare`가 성공하며,
  두 문서가 다를 때 `hwp_compare`가 오류가 아니라 `identical`을 보고함
- HWP 전용 에이전트가 하나만 존재하며 prohibited HTML/script 오류 없이 publish됨

## Amazon Quick Web

Quick Web은 클라우드에서 실행되므로 로컬 stdio 프로세스를 시작하거나 Desktop 로컬 파일시스템에
접근할 수 없다. 현재는 문서를 Quick이 읽을 수 있는 형식으로 변환한 뒤 결과물을 업로드한다:

```bash
hwp convert input.hwp -o output.docx   # 또는: -o output.pdf
```

편집된 결과물은 다운로드한 뒤 필요하면 `hwp convert`로 다시 변환한다. 제공되는 경우
Desktop/Outpost 실행 경로를 대신 사용할 수 있다. 로컬 `hwp mcp` 프로세스를 네트워크에 직접
노출하면 안 된다.

Web 네이티브 연동에는 인증된 Streamable HTTP MCP 서비스, tenant별 격리 저장소, 클라이언트 로컬
경로 인자 대신 content/artifact 전송이 필요하다. 이 릴리스에는 구현하지 않는다. 구현 계약은
[Remote MCP transport](../design/20-remote-mcp.ko.md)에 정리했고
[issue #52](https://github.com/STAIxBWLB/hwp-cli/issues/52)에서 추적한다.

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
