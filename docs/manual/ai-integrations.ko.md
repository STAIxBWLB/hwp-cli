[한국어](ai-integrations.ko.md) · [English](ai-integrations.md)

# AI 클라이언트 연동

`hwp`는 AI 클라이언트용 연동 표면을 두 가지 제공한다:

- MCP를 구사하는 클라이언트용 **MCP stdio 서버**(`hwp mcp`, 16개 도구),
- CLI와 MCP 사용법을 에이전트에게 가르치는 **에이전트 스킬**(이 저장소의
  `skills/hwp/SKILL.md`). 바이너리에 임베드되어 있어 `hwp skill export`로 풀어낼 수 있다.

어느 표면을 쓰든 `hwp mcp --root <dir>`로 도구가 만지는 파일 경로를 지정 디렉터리 아래로
제한하는 것을 권장하고, 에이전트가 쓴 파일은 `hwp validate`로 검증한다.

## Claude Code / Claude Desktop

MCP 서버를 등록한다(Claude Code: `.mcp.json` 또는 `claude mcp add`; Claude Desktop:
`claude_desktop_config.json`):

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

Claude Code는 에이전트 스킬을 바로 소비할 수도 있다:

```sh
hwp skill export --install claude-code   # ~/.claude/skills/hwp/SKILL.md에 기록
```

## Codex CLI

`~/.codex/config.toml`에 추가한다:

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
툴체인 불필요 — 사전 빌드 릴리스 아카이브를 받는다):

```sh
curl -fsSL https://raw.githubusercontent.com/STAIxBWLB/hwp-cli/main/scripts/install.sh | sh
```

이후 MCP 서버 등록은 위 Codex CLI와 같다.

## Kiro / Kimi

둘 다 표준 stdio MCP 서버 등록을 받는다 — Claude Code와 같은 형태다:

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
스킬 디렉터리 관례는 클라이언트마다 다르므로 `hwp skill export -o <dir>`로 원하는 위치에
풀어내고 클라이언트가 그 경로를 가리키게 한다.

## claude.ai (웹)

claude.ai 코드 실행 샌드박스는 네트워크가 레지스트리로 제한되어 런타임에 바이너리를 받을
수 없다. 그래서 매 릴리스에 `hwp-skill-claude-web.zip`을 함께 올린다 — `SKILL.md`(zip
루트), `bootstrap.sh`, 그리고 Linux x86_64 `bin/hwp`를 번들로 묶은 것이다:

1. [최신 릴리스](https://github.com/STAIxBWLB/hwp-cli/releases)에서
   `hwp-skill-claude-web.zip`을 받는다.
2. claude.ai에서 Settings → Capabilities → Skills로 가서 zip을 업로드한다.
3. 코드 실행이 켜진 채팅에서 세션당 한 번 `bash bootstrap.sh`를 실행한다: 번들된 바이너리를
   `~/.local/bin`에 설치하고 MCP 등록 스니펫을 출력한다. 이후 Claude는 샌드박스 안에서
   `hwp`를 CLI로 직접 구동한다.

## Amazon Quick Suite

Quick Suite는 현재 로컬 MCP 표면이 없다. 먼저 문서를 변환하고 결과를 업로드한다:

```sh
hwp convert input.hwp -o output.docx   # 또는: -o output.pdf
```

원격 HTTP MCP 엔드포인트(Quick Suite가 MCP 클라이언트로 소비할 수 있는 형태)는
[#52](https://github.com/STAIxBWLB/hwp-cli/issues/52)에서 별도 추적한다.

## 업스트림 스킬 vs 다운스트림 `hwpx` 스킬

이 저장소는 **범용** 스킬 [`skills/hwp/SKILL.md`](../../skills/hwp/SKILL.md)를 제공한다:
바이너리 빠른 참조, MCP 사용법, 안전 규칙. 의도적으로 영문 단일이다(소비자는 에이전트이며,
단일 정본 언어가 이중 유지보수를 피한다).

별도 `STAIxBWLB/skills` 저장소의 한국 공문서 스킬 `skills/hwpx`는 **다운스트림**이다: 이
범용 스킬을 감싸 워크스페이스 특화 템플릿(기안문/보고서 프리셋, 문서 관례)을 얹는다. 여기에
합치지 않는 것도 의도적이다 — 이 저장소는 포맷/도구킷 계층으로 두고, 사이트 특화 문서
정책은 다운스트림 계층이 담당한다.
