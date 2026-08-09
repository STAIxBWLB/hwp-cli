[한국어](amazon-quick-desktop.ko.md) · [English](amazon-quick-desktop.md)

# Amazon Quick Desktop: HWP MCP 설정과 문제 해결

이 문서는 Amazon Quick Desktop에 `hwp`를 로컬 MCP 커넥터로 설정하는 사람과 AI 에이전트를 위한
실행 절차다. Windows에서 실제로 확인한 전체 흐름 — 최신 바이너리 하나 설치, HWP 스킬 설치,
커넥터 등록, 실제 파일 쓰기 검증, Quick이 커넥터를 비활성화하거나 잃어버렸을 때의 복구 — 을
순서대로 다룬다.

Quick 릴리스에 따라 UI 이름과 내부 파일명이 바뀔 수 있다. Quick 내부 설정 파일을 직접 편집하기보다
이 문서의 UI 절차와 import JSON을 사용한다.

## 정상 구성 요소

| 구성요소 | 역할 | Windows 확인값 |
|---|---|---|
| `hwp.exe` | MCP stdio 서버 실행 | 안정된 절대 경로의 최신 바이너리 하나 |
| HWP MCP 커넥터 | HWP 도구 16개 노출 | `hwp.exe mcp ...` |
| HWP 스킬 | Quick 에이전트에게 도구 사용 시점과 방법 안내 | 활성 Quick 프로필의 `skills/hwp/SKILL.md` |
| 교환 root | Quick과 MCP 자식 프로세스가 파일을 주고받는 경계 | `C:\TEMP` |
| 폰트 디렉터리 | Windows 렌더링 폰트 공급 | `C:\Windows\Fonts` |

커넥터와 스킬은 서로 별개다. 스킬 설치는 바이너리를 설치하거나 커넥터를 만들지 않는다. 커넥터에
도구 16개가 표시된 뒤에도 파일 쓰기는 실패할 수 있으므로, 실제 생성·검증 smoke test까지 해야 한다.

## 1. 최신 `hwp.exe` 하나 설치하고 확인

[최신 릴리스](https://github.com/STAIxBWLB/hwp-cli/releases)에서 Windows x86_64 아카이브와
`.sha256` 파일을 받는다. Amazon Quick Windows 경로 정규화에는 hwp-cli v0.8.2 이상이 필요하다.

- `hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- `hwp-vX.Y.Z-x86_64-pc-windows-msvc.sha256`

압축을 풀기 전에 체크섬을 대조한다. 아래 예시 경로는 실제로 받은 버전에 맞게 바꾼다.

```powershell
$Archive = "C:\path\to\hwp-vX.Y.Z-x86_64-pc-windows-msvc.zip"
$Checksum = [IO.Path]::ChangeExtension($Archive, ".sha256")
$Expected = ((Get-Content -LiteralPath $Checksum) -split '\s+')[0].ToLowerInvariant()
$Actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Actual -ne $Expected) { throw "hwp archive checksum mismatch" }
```

`hwp.exe`를 Quick이 실행할 수 있는 안정된 위치에 압축 해제하고, 커넥터 등록 뒤에는 옮기지 않는다.
Windows Quick에서 확인한 배치 예시는 다음과 같다.

```text
%USERPROFILE%\.quickwork\profiles\PROFILE_ID\skills\hwp\bin\hwp.exe
```

`PROFILE_ID`는 `%USERPROFILE%\.quickwork\profiles.json`의 활성 항목이다. Quick이 접근할 수 있는 다른
안정된 위치도 사용할 수 있다. 정확한 절대 경로를 기록하고 PowerShell에서 바이너리를 확인한다.

```powershell
$Hwp = "C:\absolute\path\to\hwp.exe"
& $Hwp --version
```

Quick에도 이 경로를 그대로 입력한다. 다른 터미널의 PATH에 의존하면 Quick이 남아 있는 구버전을
실행할 수 있다. 첫 파일 쓰기가 `\\?\C:\...` 같은 Windows verbatim 경로와 함께 계속 실패하면
`hwp --version`이 v0.8.2 이상인지 확인한다.

## 2. Windows 교환 root 만들기

Quick 내장 파일 도구와 로컬 MCP 자식 프로세스는 항상 같은 파일 권한을 받지 않는다. **Local folders
and access permissions**에 사용자 프로필 폴더를 추가해도 MCP 자식 프로세스가 거부될 수 있다.
확인된 샌드박스 교환 디렉터리부터 사용한다.

```powershell
New-Item -ItemType Directory -Path C:\TEMP -Force
```

커넥터의 `--root`를 `C:\TEMP`로 지정한다. HWP 도구를 부르기 전에 입력 `.hwp`, `.hwpx`, Markdown,
JSON, 이미지, 템플릿을 이곳으로 복사한다. 모든 MCP 입력·출력 경로를 이 root 아래에 유지하고, 작업이
끝나면 Quick 내장 파일 도구나 Explorer로 최종 artifact를 목적 폴더에 복사한다.

`--root`는 호환 설정인 동시에 보안 경계다. 권한 오류를 피하려고 제거하지 않는다.

## 3. 활성 Quick 프로필에 HWP 스킬 설치

현재 바이너리를 실행한다.

```powershell
& $Hwp skill export --install amazon-quick
```

이 명령은 `%USERPROFILE%\.quickwork\profiles.json`을 읽어 유효한 `last_active` 프로필 또는 유일한
유효 프로필을 고르고, 그 안의 `skills\hwp\SKILL.md`만 쓴다. `hwp.exe`를 복사하거나 MCP 커넥터·
에이전트를 만들거나 publish하지 않는다.

Quick 프로필이 여러 개이거나 자동 선택이 모호하면 프로필 ID 또는 절대 프로필 디렉터리를 지정한다.

```powershell
& $Hwp skill export --install amazon-quick --quick-profile enterprise-example
& $Hwp skill export --install amazon-quick --quick-profile "C:\absolute\path\to\quick\profile"
```

스킬을 설치하거나 교체한 뒤 Quick을 재시작하거나 새로고침한다.

## 4. 로컬 MCP 커넥터 등록

Quick Desktop에서 **Settings → Capabilities → Connectors → + Create → MCP server → Local**로
이동한다. 릴리스에 따라 이름이 조금 다를 수 있다. 인자 경계를 정확하게 보존하는 import JSON을
권장한다.

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
        "C:\\TEMP"
      ]
    }
  }
}
```

`command`만 실제 경로로 바꾼다. JSON에서 이중 백슬래시는 일반 Windows 백슬래시를 표현할 뿐,
실제 경로를 바꾸지 않는다.

직접 입력할 때는 다음 값을 사용한다.

| 항목 | 값 |
|---|---|
| Name | `hwp` |
| Command | 앞에서 검증한 정확한 `hwp.exe` 절대 경로 |
| Arguments | `mcp --font-dir C:\Windows\Fonts --root C:\TEMP` |
| Description | `Read, write, edit, render, validate, and convert HWP/HWPX documents.` |
| Timeout | `30`초 |

Arguments 입력란은 셸이 아니다. 경로 앞뒤에 작은따옴표나 큰따옴표 문자를 넣지 않는다. 다음은 잘못된
예다.

```text
mcp --font-dir 'C:\Windows\Fonts' --root 'C:\TEMP'
```

Quick은 이 따옴표를 제거하지 않고 그대로 넘길 수 있다. 그러면 `hwp`는 따옴표가 이름에 포함된 폴더를
찾고, root 확인에 실패해 즉시 종료하며 MCP handshake가 닫힌다. JSON 형식은 각 token을 배열의 별도
항목으로 유지해 이 문제를 피한다.

**Test connection**을 선택하고 Quick의 명령 실행 확인을 승인한다. **Connected**, **16 tools
available**이 표시되어야 한다. 이어서 **Add MCP**를 선택하고 다시 승인한 뒤 연결을 새로고침한다.
`hwp`가 활성화되어 있고 **16 tools, Connected**로 표시되는지 확인한다.

## 5. 실제 end-to-end smoke test 실행

“16 tools available”에서 멈추지 않는다. 새 Quick 대화를 열고 다음 prompt를 붙여넣는다.

```text
셸 명령이 아니라 HWP MCP 도구를 사용하라.
1. 다음 Markdown으로 hwp_new를 호출해 C:\TEMP\quick-hwp-smoke.hwpx를 생성하라.
   # Quick MCP smoke test

   Amazon Quick can create HWPX files through hwp MCP.
2. C:\TEMP\quick-hwp-smoke.hwpx에 hwp_validate를 호출하라.
3. 같은 파일에 plain 형식으로 hwp_read를 호출하라.
4. 정확한 출력 경로와 검증 결과를 보고하라. valid가 true가 아니면 성공이라고 말하지 마라.
```

정상 설치는 파일을 만들고 다음과 같은 검증 결과를 반환한다.

```json
{
  "valid": true,
  "errors": [],
  "warnings": []
}
```

시각 결과가 중요하면 1쪽에 `hwp_render`를 추가로 호출하고 출력도 `C:\TEMP` 아래에 쓴다. 이 단계는
문서 생성과 별도로 폰트 접근·렌더링을 확인한다.

## 6. Quick 에이전트에 지속 지침 추가

중복 이름이나 오래된 커넥터 지침이 남지 않도록 HWP 역할의 에이전트는 하나만 유지한다. HWP 커넥터와
설치된 HWP 스킬을 활성화하고 다음과 같은 instructions를 추가한다.

```text
.hwp 또는 .hwpx 작업에는 설치된 hwp 스킬과 HWP MCP 도구를 사용한다.
Windows에서는 활성 커넥터가 다른 root를 명시적으로 노출하지 않는 한 C:\TEMP 아래 경로만 사용한다.
HWP 작업 전에 입력을 C:\TEMP로 복사하고 최종 artifact의 C:\TEMP 경로를 사용자에게 반환한다.
hwp_new, hwp_edit, hwp_fill, hwp_convert, hwp_compose, hwp_template 뒤에는 항상 hwp_validate를 호출한다.
페이지 모양이 중요하면 hwp_render도 호출해 요청된 페이지를 확인한다.
자동 생성된 MCP 서버 prefix를 하드코딩하지 말고 hwp_new/hwp_read 같은 도구 이름으로 선택한다.
Access is denied가 나오면 시도한 경로와 설정 root를 보고한다. root 제한을 제거하지 않는다.
커넥터 탐색만으로 성공이라 말하지 말고 요청 작업과 검증이 모두 통과해야 성공으로 판정한다.
```

OneDrive나 SharePoint 커넥터는 선택 사항이다. 원본이나 완성 파일을 `C:\TEMP` 안팎으로 복사할 때만
사용하며, 로컬 HWP MCP 커넥터를 대체하지 않는다.

## 일상 작업 흐름

1. 모든 원본 파일과 참조 asset을 `C:\TEMP`로 복사한다.
2. Quick에 정확한 입력·출력 경로를 준다. 예: “`C:\TEMP\input.hwpx`를 읽어 초안을 최종으로
   바꾸고 `C:\TEMP\final.hwpx`에 저장하라.”
3. 모든 쓰기 뒤 `hwp_validate`를 요구한다. 레이아웃이 중요하면 `hwp_render`도 요구한다.
4. 반환된 경로와 검증 결과를 확인한 뒤 artifact를 열거나 검사한다.
5. 검증한 출력을 `C:\TEMP`에서 승인된 목적지로 복사한다. 목적지 사본을 확인한 뒤에만 교환 파일을
   정리한다.

활용 prompt 예:

- “`C:\TEMP\input.hwp`를 요약하고 표 목록을 보여줘.”
- “`C:\TEMP\input.hwpx`를 `C:\TEMP\input.md`로 변환해줘.”
- “이 Markdown으로 `C:\TEMP\report.hwpx`를 만들고 검증한 뒤 1쪽을 렌더해줘.”
- “`C:\TEMP\template.hwpx`의 slot을 채워 `C:\TEMP\filled.hwpx`에 쓰고 검증해줘.”

## 증상별 문제 해결

| 증상 | 예상 원인 | 복구 |
|---|---|---|
| `hwp.exe`가 시작하지 않거나 `--version`이 실패함 | 잘못된 바이너리·아키텍처, 차단되거나 불완전한 압축 해제 | 다시 다운로드하고 SHA-256을 확인한 뒤 Windows x86_64 아카이브를 풀고 정확한 절대 command를 테스트한다 |
| Test connection에 도구가 없거나 서버가 즉시 종료함 | 없거나 읽을 수 없는 `--root`, Arguments 안의 따옴표 문자, 오타, 오래된 command 경로 | `C:\TEMP` 존재 확인, 위 JSON import, 셸 따옴표 제거, `hwp.exe --version` 검증 |
| **Connected, 16 tools**인데 `hwp_new`가 `Access is denied (os error 5)`를 반환함 | 전송은 정상이나 MCP 자식이 요청 경로를 쓸 수 없거나, 구버전이 `\\?\...`를 Quick 샌드박스에 전달함 | 입력·출력을 모두 `C:\TEMP` 아래로 옮기고 `--root C:\TEMP` 유지, `hwp` 업그레이드, 재시작 뒤 smoke test |
| **Local folders and access permissions**에 추가한 경로도 실패함 | 그 설정은 Quick 내장 파일 도구를 제어하며 로컬 MCP 자식에는 동일하게 적용되지 않을 수 있음 | 내장 도구로 파일을 `C:\TEMP`에 복사한 뒤 그곳에서 HWP 도구를 호출한다 |
| `os error 2` | 경로가 실제로 없음. Desktop이 OneDrive로 이동됐을 수 있음 | 실제 경로 확인, 목적 디렉터리 생성, 또는 `C:\TEMP`에 staging |
| 반복 실패 뒤 커넥터가 비활성화됨 | 시작·handshake 실패가 반복되어 Quick이 자동 비활성화함 | command/root 수정·저장, 커넥터를 명시적으로 다시 활성화, 새로고침, 필요하면 Quick 재시작 |
| 커넥터 수정·재import 뒤 `Unknown tool` | Quick이 새 내부 커넥터/tool prefix를 만들었으나 대화는 예전 이름을 보유함 | 연결 새로고침과 새 대화 시작, 예전 생성 이름 대신 HWP 스킬·도구 다시 로드 |
| 에이전트 publish 때 `assetDescriptor contains prohibited HTML/script content` | 구버전 스킬의 angle-bracket placeholder를 Quick이 markup으로 분류함 | 최신 `hwp`에서 스킬 재설치, Quick 새로고침, 다시 publish |
| 생성은 되지만 렌더링 실패 | 폰트 디렉터리가 없거나 접근 불가 | 먼저 `--font-dir` 없이 생성 검증, 그다음 `C:\Windows\Fonts`를 추가하고 `hwp_render` 재시도 |
| Path is outside allowed roots | MCP root 정책이 정상적으로 경로를 거부함 | asset을 `C:\TEMP`로 복사하거나 실제 지원되는 root 추가. 모든 root를 제거하지 않는다 |

### Windows 경로 수정이 필요한 이유

Rust의 Windows canonicalization은 같은 경로를 `\\?\C:\TEMP\quick-hwp-smoke.hwpx` 같은 verbatim
형식으로 반환할 수 있다. Quick은 MCP handshake 때 일반 `C:\TEMP` root를 받아들이고도, 이후 `hwp`가
private atomic staging 디렉터리를 만들 때 이 verbatim 표기를 거부할 수 있다. 현재 `hwp`는 downstream
파일 I/O 전에 verbatim drive/UNC 경로를 일반 Windows 표기로 정규화하면서 root containment 검사는
fail-closed 상태로 유지한다.

이 차이 때문에 커넥터 탐색은 성공하지만 첫 쓰기만 실패하는 혼동이 생긴다. 항상 `hwp_new`와
`hwp_validate`로 data plane까지 검증한다.

## 선택적 로컬 진단

진단 목적으로만 사용한다. Quick 실행 중 내부 파일을 직접 편집하지 않는다.

- 활성 프로필 레지스트리: `%USERPROFILE%\.quickwork\profiles.json`
- 프로필별 MCP snapshot: `%USERPROFILE%\.quickwork\profiles\PROFILE_ID\mcp_config.json`
- 일반적인 Windows backend log: `%LOCALAPPDATA%\Temp\quickwork-backend.log`

저장된 커넥터 key는 `hwp`가 아니라 `hwp-...`처럼 생성될 수 있고, Quick은 `autoDisabled`를 포함한
내부 `_quick` metadata를 추가할 수 있다. 이는 Quick 구현 상세이므로 직접 수정하지 말고 UI에서
커넥터를 고치고 활성화한다.

선택적 log 확인 명령:

```powershell
Get-Content "$env:LOCALAPPDATA\Temp\quickwork-backend.log" -Tail 300 |
  Select-String "UserMCP|Loaded.*servers|total tools|hwp"
```

정상 기동 시 “Started ... with 16 tools”, “Loaded 1/1 servers (0 failed), 16 total tools”와 같은
메시지가 보인다. 로그 문구와 위치는 안정된 API 계약이 아니다.

## 완료 체크리스트

- 커넥터 command가 검증한 `hwp.exe` 절대 경로 하나를 사용함
- 커넥터가 분리된 JSON 인자를 사용하며 셸 따옴표가 들어 있지 않음
- `C:\TEMP`가 존재하고 Windows MCP root로 설정됨
- 현재 HWP 스킬이 활성 Quick 프로필에 설치됨
- Test connection이 **Connected**, **16 tools available**을 표시함
- 새로고침·재시작 뒤에도 커넥터가 활성 상태임
- `C:\TEMP\quick-hwp-smoke.hwpx`에서 `hwp_new`, `hwp_validate`, `hwp_read`가 성공함
- `hwp_validate`가 `valid: true`를 반환하고, 모양이 중요하면 렌더링도 확인함
- 에이전트가 모든 쓰기 뒤 검증하고 최종 artifact 경로를 반환함

Amazon Quick Web은 이 로컬 stdio 흐름을 실행할 수 없다. 현재 변환·업로드 방법과 계획된 remote MCP
구조는 [AI 클라이언트 연동](ai-integrations.ko.md#amazon-quick-web)을 참고한다.
