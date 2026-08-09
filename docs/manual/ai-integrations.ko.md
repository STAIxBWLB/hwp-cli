[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI 클라이언트 연동

`hwp`는 AI 클라이언트용 연동 표면을 두 가지 제공한다.

- MCP를 구사하는 클라이언트용 **MCP stdio 서버**(`hwp mcp`, 16개 도구)
- CLI와 MCP 사용법을 에이전트에게 가르치는 **에이전트 스킬**(이 저장소의
  `skills/hwp/SKILL.md`). 바이너리에 임베드되어 있으며 `hwp skill export`로 풀어낼 수 있다.

어느 표면을 쓰든 `hwp mcp --root {dir}`로 도구가 만지는 파일 경로를 지정 디렉터리 아래로
제한하고, 에이전트가 쓴 파일은 `hwp validate`로 검증하는 것을 권장한다.

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
hwp skill export --install claude-code   # ~/.claude/skills/hwp/SKILL.md에 기록
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
hwp skill export --install codex         # ~/.codex/skills/hwp/SKILL.md에 기록
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
스킬 디렉터리 관례는 클라이언트마다 다르므로 `hwp skill export -o {dir}`로 원하는 위치에
풀어내고 클라이언트가 그 경로를 가리키게 한다.

## claude.ai (웹)

claude.ai 코드 실행 샌드박스는 네트워크가 레지스트리로 제한되어 런타임에 바이너리를 받을
수 없다. 그래서 매 릴리스에 `hwp-skill-claude-web.zip`을 함께 올린다. zip 루트의
`SKILL.md`, `bootstrap.sh`, Linux x86_64 `bin/hwp`가 포함된다.

1. [최신 릴리스](https://github.com/STAIxBWLB/hwp-cli/releases)에서
   `hwp-skill-claude-web.zip`을 받는다.
2. claude.ai에서 Settings → Capabilities → Skills로 가서 zip을 업로드한다.
3. 코드 실행이 켜진 채팅에서 세션당 한 번 `bash bootstrap.sh`를 실행한다. 번들된 바이너리를
   `~/.local/bin`에 설치하고 MCP 등록 스니펫을 출력한다. 이후 Claude는 샌드박스 안에서
   `hwp`를 CLI로 직접 구동한다.

## Amazon Quick Desktop

Amazon Quick Desktop은 `hwp mcp`를 로컬 stdio 커넥터로 실행할 수 있다. HWP 도구 16개가
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
**Connected**, **16 tools available**을 표시해야 한다. 이어서 **Add MCP**를 선택하고 확인 창을
다시 승인한 뒤 연결을 새로고침한다. `Hwp`가 활성화되어 있고 **16 tools, Connected**로 표시되는지
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
**Connected**, **16 tools available**가 표시되고 새로고침 뒤에도 활성 상태를 유지한다.

### 3. publish-safe HWP 스킬 설치

```sh
hwp skill export --install amazon-quick
```

명령은 `~/.quickwork/profiles.json`을 읽어 유효한 `last_active` 프로필을 우선 선택하고, 유효한
프로필이 하나뿐이면 그것을 사용한다. 해당 프로필 안의 `skills/hwp/SKILL.md`만 쓴다. 에이전트나
커넥터를 생성하거나 publish하지 않는다.

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

쓰기 작업 뒤에는 `hwp_validate`를 호출해야 한다. 시각 결과가 중요하면 `hwp_render`도 사용한다.

### 5. HWP 전용 에이전트 하나로 구성

중복 이름과 오래된 지시를 피하기 위해 같은 역할의 에이전트는 하나만 유지한다. HWP MCP 커넥터를
활성화하고, 설치된 `hwp` 스킬 사용, 쓰기 후 검증, 설정된 root 준수를 instructions에 명시한다.
OneDrive/SharePoint 커넥터는 원본이나 결과가 그 위치에 있을 때만 선택적으로 추가한다.

publish 중 `assetDescriptor contains prohibited HTML/script content` 오류가 발생하면 최신 `hwp`
바이너리에서 스킬을 다시 설치한다. 현재 스킬은 `{file}` 같은 중괄호 placeholder를 사용하며 Quick이
HTML로 오인할 수 있는 angle-bracket markup을 포함하지 않는다.

### Desktop 확인 체크리스트

- `hwp --version`이 의도한 최신 바이너리를 표시함
- 커넥터 테스트가 **Connected**, **16 tools available**을 표시함
- 새로고침 뒤에도 커넥터가 활성화되어 있고 **16 tools, Connected**를 표시함
- 테스트 HWPX에서 `hwp_new`, `hwp_read`, `hwp_validate`, `hwp_render`가 성공함(Windows에서는
  `%USERPROFILE%\AppData\LocalLow\hwp-quick-workspace` 아래에서 테스트)
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

## 업스트림 스킬 vs 다운스트림 `hwpx` 스킬

이 저장소는 **범용** 스킬 [`skills/hwp/SKILL.md`](../../skills/hwp/SKILL.md)를 제공한다.
바이너리 빠른 참조, MCP 사용법, 안전 규칙이 들어 있다. 소비자가 에이전트이므로 영문 단일 정본을
유지해 이중 유지보수를 피한다.

별도 `STAIxBWLB/skills` 저장소의 한국 공문서 스킬 `skills/hwpx`는 **다운스트림**이다. 이 범용
스킬을 감싸 워크스페이스 특화 템플릿(기안문/보고서 preset과 문서 관례)을 얹는다. 이 저장소는
포맷/도구킷 계층으로 유지하고 사이트 특화 문서 정책은 다운스트림 계층에서 다룬다.
