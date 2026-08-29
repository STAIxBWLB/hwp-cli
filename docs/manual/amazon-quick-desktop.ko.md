[한국어](amazon-quick-desktop.ko.md) · [English](amazon-quick-desktop.md)

# Amazon Quick Desktop: HWP MCP 설정과 문제 해결

이 문서는 Amazon Quick Desktop에 `hwp`를 로컬 MCP 커넥터로 설정하는 사람과 AI 에이전트를 위한
실행 절차다. Windows에서 실제로 확인한 전체 흐름 — 최신 바이너리 하나 설치, HWP 스킬 설치,
커넥터 등록, 실제 파일 쓰기 검증, Quick이 커넥터를 비활성화하거나 잃어버렸을 때의 복구 — 을
순서대로 다룬다.

Quick 릴리스에 따라 UI 이름과 내부 파일명이 바뀔 수 있다. Quick 내부 설정 파일을 직접 편집하기보다
이 문서의 UI 절차와 import JSON을 사용한다.

검증 기준: 2026-08-09 Windows의 Amazon Quick Desktop `0.1000.2660`, hwp-cli `0.8.3`.

이 기준선은 Windows 실기에서 마지막으로 확인한 시점을 남긴 기록이다. 아래 문서의 도구 개수,
도구 이름, 명령 표면은 2026-08-29에 hwp-cli v0.13.0의 CLI·MCP 정의를 읽어 대조해 갱신했으며,
Windows 절차 자체를 다시 실행하지는 않았다. 아래에 적은 `hwp-cli v0.8.2 이상` 요건은 지금도
유효하다.

## 정상 구성 요소

| 구성요소 | 역할 | Windows 확인값 |
|---|---|---|
| `hwp.exe` | MCP stdio 서버 실행 | 안정된 절대 경로의 최신 바이너리 하나 |
| HWP MCP 커넥터 | HWP 도구 20개 노출 | `hwp.exe mcp ...` |
| HWP 스킬 | Quick 에이전트에게 도구 사용 시점과 방법 안내 | 활성 Quick 프로필의 `skills/hwp/SKILL.md` |
| 교환 root | Low 무결성 MCP 자식과 파일을 주고받는 경계 | `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace` |
| 폰트 디렉터리 | Windows 렌더링 폰트 공급 | `C:\Windows\Fonts` |

커넥터와 스킬은 서로 별개다. 스킬 설치는 바이너리를 설치하거나 커넥터를 만들지 않는다. 커넥터에
도구 20개가 표시된 뒤에도 파일 쓰기는 실패할 수 있으므로, 실제 생성·검증 smoke test까지 해야 한다.

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

Quick 내장 파일 도구와 로컬 MCP 자식 프로세스는 같은 파일 권한을 받지 않는다. 확인한 Windows
빌드에서 Quick은 `hwp.exe`를 Low mandatory integrity(`S-1-16-4096`)로 시작한다. `C:\TEMP`와
`%LOCALAPPDATA%\Temp` 같은 일반 폴더는 보통 Medium 무결성이다. 이 경로에서는 MCP 커넥터가
시작하고 도구 20개를 노출해도, `hwp_new`가 private staging 디렉터리를 만드는 첫 쓰기에서
`Access is denied (os error 5)`가 발생할 수 있다.

Windows가 기본 제공하는 `LocalLow` 아래에 전용 자식 디렉터리를 만든다. 관리자 권한 없이 Low
무결성 레이블을 상속한다.

```powershell
$QuickHwpRoot = Join-Path $env:USERPROFILE 'AppData\LocalLow\hwp-quick-workspace'
New-Item -ItemType Directory -Path $QuickHwpRoot -Force | Out-Null
$QuickHwpRoot
icacls.exe $QuickHwpRoot
```

`icacls` 출력에 `Mandatory Label\Low Mandatory Level:(I)(OI)(CI)(NW)`와 동등한 상속 항목이
있어야 한다. 없다면 상속을 켠 상태로 `LocalLow` 아래에 새 자식 디렉터리를 만든다. `C:\TEMP` 전체의
무결성 수준을 낮추거나, 광범위한 쓰기 권한을 추가하거나, MCP root를 제거하지 않는다.

출력된 절대 경로를 커넥터의 `--root`로 지정한다. 아래 JSON과 MCP 도구 인자에서는 `YOUR_NAME`을
실제 Windows 계정 폴더명으로 바꾼다. Quick Arguments와 MCP 도구는 셸 확장을 하지 않으므로
`%USERPROFILE%` 문자열을 그대로 넘기지 않는다. HWP 도구를 부르기 전에 입력 `.hwp`, `.hwpx`,
Markdown, JSON, 이미지, 템플릿을 교환 root로 복사한다. 모든 MCP 입력·출력 경로를 이 root 아래에
유지하고, 작업이 끝나면 Quick 내장 파일 도구나 Explorer로 검증된 artifact를 목적 폴더에 복사한다.

`--root`는 호환 설정인 동시에 보안 경계다. 권한 오류를 피하려고 제거하지 않는다.

## 3. 활성 Quick 프로필에 HWP 스킬 설치

현재 바이너리를 실행한다.

```powershell
& $Hwp skill export --install amazon-quick
```

이 명령은 `%USERPROFILE%\.quickwork\profiles.json`을 읽어 유효한 `last_active` 프로필 또는 유일한
유효 프로필을 고르고, 그 안의 `skills\hwp\SKILL.md`만 쓴다. 공문서 파일(`SKILL.ko.md`,
`official-documents(.ko).md`, `references/`, `templates/`)은 Quick 경로에 설치되지
**않으며**, 명령이 그 사실을 알리는 문구를 표시 언어와 무관하게 한국어로 출력한다. `hwp.exe`를
복사하거나 MCP 커넥터·에이전트를 만들거나 publish하지 않는다. export는 심볼릭 링크 대상을
거부하고 발행이 실패하면 이전 트리를 복원하므로, 기존 스킬 디렉터리 위에 다시 설치해도 안전하다.

Quick 프로필이 여러 개이거나 자동 선택이 모호하면 프로필 ID 또는 절대 프로필 디렉터리를 지정한다.

```powershell
& $Hwp skill export --install amazon-quick --quick-profile enterprise-example
& $Hwp skill export --install amazon-quick --quick-profile "C:\absolute\path\to\quick\profile"
```

`--quick-profile`은 프로필 ID 하나 또는 절대 경로를 받는다. `profiles\enterprise-example`처럼
여러 구성요소로 된 상대 경로는 거부하므로, 그런 경우에는 ID만 넘긴다.

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
        "C:\\Users\\YOUR_NAME\\AppData\\LocalLow\\hwp-quick-workspace"
      ]
    }
  }
}
```

`command`를 실제 값으로 바꾸고, `--root`에는 위 교환 root 생성 단계가 출력한 절대 경로를 그대로 넣는다
(표준 프로필은 `C:\Users\<계정>\...` 형태지만, 리디렉션된 프로필은 접두사가 다르므로 반드시 출력된
경로를 사용한다). JSON에서 이중 백슬래시는 일반 Windows 백슬래시를 표현할 뿐, 실제 경로를 바꾸지 않는다.

직접 입력할 때는 다음 값을 사용한다.

| 항목 | 값 |
|---|---|
| Name | `hwp` |
| Command | 앞에서 검증한 정확한 `hwp.exe` 절대 경로 |
| Arguments | `mcp --font-dir C:\Windows\Fonts --root C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace` |
| Description | `Read, write, edit, render, validate, lint, convert, merge, split, and compare HWP/HWPX documents.` |
| Timeout | `30`초 |

Arguments 입력란은 셸이 아니다. 경로 앞뒤에 작은따옴표나 큰따옴표 문자를 넣지 않는다. 다음은 잘못된
예다.

```text
mcp --font-dir 'C:\Windows\Fonts' --root 'C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace'
```

Quick은 이 따옴표를 제거하지 않고 그대로 넘길 수 있다. 그러면 `hwp`는 따옴표가 이름에 포함된 폴더를
찾고, root 확인에 실패해 즉시 종료하며 MCP handshake가 닫힌다. JSON 형식은 각 token을 배열의 별도
항목으로 유지해 이 문제를 피한다.

**Test connection**을 선택하고 Quick의 명령 실행 확인을 승인한다. **Connected**, **20 tools
available**이 표시되어야 한다. 이어서 **Add MCP**를 선택하고 다시 승인한 뒤 연결을 새로고침한다.
`hwp`가 활성화되어 있고 **20 tools, Connected**로 표시되는지 확인한다.

## 5. 실제 end-to-end smoke test 실행

“20 tools available”에서 멈추지 않는다. 새 Quick 대화를 열고 다음 prompt를 붙여넣는다.

```text
셸 명령이 아니라 HWP MCP 도구를 사용하라.
1. 다음 Markdown으로 hwp_new를 호출해 C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\quick-hwp-smoke.hwpx를 생성하라.
   # Quick MCP smoke test

   Amazon Quick can create HWPX files through hwp MCP.
2. C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\quick-hwp-smoke.hwpx에 hwp_validate를 호출하라.
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

시각 결과가 중요하면 1쪽에 `hwp_render`를 추가로 호출하고 출력도 같은 `hwp-quick-workspace`
root 아래에 쓴다. 이 단계는 문서 생성과 별도로 폰트 접근·렌더링을 확인한다.

## 6. Quick 에이전트에 지속 지침 추가

중복 이름이나 오래된 커넥터 지침이 남지 않도록 HWP 역할의 에이전트는 하나만 유지한다. HWP 커넥터와
설치된 HWP 스킬을 활성화하고 다음과 같은 instructions를 추가한다.

```text
.hwp 또는 .hwpx 작업에는 설치된 hwp 스킬과 HWP MCP 도구를 사용한다.
Windows Quick Desktop에서는 MCP 커넥터에 설정된 절대 LocalLow root 아래 경로만 사용한다.
확인된 root는 C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace이며 도구 호출 전에 YOUR_NAME을 바꾼다.
HWP 작업 전에 입력을 그 root로 복사하고 최종 artifact도 그 아래 절대 경로로 반환한다.
hwp_new, hwp_edit, hwp_fill, hwp_convert, hwp_compose, hwp_template, hwp_merge, hwp_split 뒤에는 항상 hwp_validate를 호출한다.
공문서를 넘기기 전에는 hwp_lint를 호출하고 error 등급 지적을 모두 보고한다.
hwp_merge와 hwp_split 응답의 preservation 필드를 반드시 읽고, 그 확인 없이 무손실이라고 말하지 않는다.
hwp_compare는 읽기 전용이며 identical을 돌려준다. 차이가 있는 것은 실패가 아니라 정상 결과다.
페이지 모양이 중요하면 hwp_render도 호출해 요청된 페이지를 확인한다.
자동 생성된 MCP 서버 prefix를 하드코딩하지 말고 hwp_new/hwp_read 같은 도구 이름으로 선택한다.
Access is denied가 나오면 시도한 경로와 설정 root를 보고한다. root 제한을 제거하지 않는다.
커넥터 탐색만으로 성공이라 말하지 말고 요청 작업과 검증이 모두 통과해야 성공으로 판정한다.
```

OneDrive나 SharePoint 커넥터는 선택 사항이다. 원본이나 완성 파일을 설정된 교환 root 안팎으로
복사할 때만 사용하며, 로컬 HWP MCP 커넥터를 대체하지 않는다.

## 일상 작업 흐름

1. 모든 원본 파일과 참조 asset을 설정된 교환 root(`%USERPROFILE%\AppData\LocalLow` 아래)로 복사한다.
2. Quick에 정확한 입력·출력 경로를 준다. 예:
   “`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\input.hwpx`를 읽어 초안을 최종으로
   바꾸고 `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\final.hwpx`에 저장하라.”
3. 모든 쓰기 뒤 `hwp_validate`를 요구한다. 레이아웃이 중요하면 `hwp_render`도 요구한다.
4. 반환된 경로와 검증 결과를 확인한 뒤 artifact를 열거나 검사한다.
5. 검증한 출력을 교환 root에서 승인된 목적지로 복사한다. 목적지 사본을 확인한 뒤에만 교환 파일을
   정리한다.

활용 prompt 예:

- “`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\input.hwp`를 요약하고 표 목록을 보여줘.”
- “`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\input.hwpx`를 `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\input.md`로 변환해줘.”
- “이 Markdown으로 `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\report.hwpx`를 만들고 검증한 뒤 1쪽을 렌더해줘.”
- “`C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\template.hwpx`의 slot을 채워 `C:\Users\YOUR_NAME\AppData\LocalLow\hwp-quick-workspace\filled.hwpx`에 쓰고 검증해줘.”
- “`…\part1.hwpx`와 `…\part2.hwpx`를 `…\report.hwpx`로 병합하고 보존하지 못한 항목을 알려줘.”
- “`…\report.hwpx`를 `…\fragments\`로 나누고 조각 경로를 알려줘.”
- “`…\draft.hwpx`와 `…\final.hwpx`를 비교해서 어느 문단이 다른지 알려줘.”

## 0.8.3 기준선 이후 추가된 도구

이 런북을 처음 쓸 때 커넥터가 노출한 도구는 열여섯 개였다. 그 뒤로 네 개가 늘었고, 기존 도구
몇 가지도 0.8.3 시절 에이전트 지침에는 없는 인수를 얻었다.

| 도구 | 새로 생긴 것 |
|---|---|
| `hwp_lint` | 한국어 공문서 표기·구조 규칙 열 가지를 검사하고 `hwp-lint-report-v1` findings JSON을 반환. 공문서가 에이전트 손을 떠나기 전의 게이트 |
| `hwp_merge` | 문서 두 개 이상을 이어 붙이며 입력 하나가 Section 하나가 된다. 보존 손실 원장을 반환 |
| `hwp_split` | 기본은 Section 하나당 조각 하나, `pages` 범위로도 나눌 수 있다. 발행된 조각 경로를 반환 |
| `hwp_compare` | 읽기 전용 문단·구조 비교. 차이가 있어도 `isError`가 되지 않으므로 `identical`을 읽는다 |
| `hwp_new` | 공문서 인수: `template`, `preset`, `margin_top`/`margin_bottom`/`margin_left`/`margin_right`, `doc_head`, `doc_foot`, `notice_head`, `notice_foot`, `press_head` |
| `hwp_edit` | 라벨 옆 값 칸을 채우는 `set_cell_by_label`과 `label_table`, 프로필의 표 모양을 적용하는 `style_tables` |
| `hwp_read`, `hwp_convert`, `hwp_render` | 보호 문서용 호출 단위 `password`. 호출 사이에 캐시되지 않는다 |

`hwp_merge`와 `hwp_split`은 strict가 기본값이 아니다. 병합은 첫 입력 이후의 패키지
passthrough를 항상 버리므로, 무손실이라고 가정하지 말고 응답의 `preservation` 필드를 읽는다.

## 증상별 문제 해결

| 증상 | 예상 원인 | 복구 |
|---|---|---|
| `hwp.exe`가 시작하지 않거나 `--version`이 실패함 | 잘못된 바이너리·아키텍처, 차단되거나 불완전한 압축 해제 | 다시 다운로드하고 SHA-256을 확인한 뒤 Windows x86_64 아카이브를 풀고 정확한 절대 command를 테스트한다 |
| Test connection에 도구가 없거나 서버가 즉시 종료함 | 없거나 읽을 수 없는 `--root`, 바꾸지 않은 `YOUR_NAME`, Arguments 안의 따옴표 문자, 오타, 오래된 command 경로 | `LocalLow` 자식 생성, 실제 계정 절대 경로 치환, 위 JSON import, 셸 따옴표 제거, `hwp.exe --version` 검증 |
| **Connected, 20 tools**인데 `hwp_new`가 `Access is denied (os error 5)`를 반환함 | 탐색에는 읽기 권한만 필요했다. 요청 목적지가 Medium 무결성(대표적으로 `C:\TEMP`, `%LOCALAPPDATA%\Temp`)이거나 구버전이 `\\?\...`를 전달함 | `--root`와 모든 도구 경로를 전용 `LocalLow` 자식으로 지정, `icacls`로 Low 레이블 확인, hwp-cli v0.8.2 이상 사용, 재시작 뒤 smoke test |
| **Local folders and access permissions**에 추가한 경로도 실패함 | 이 설정은 Quick 내장 읽기·검색 도구를 제어하며 Medium 폴더를 Low 무결성 MCP 자식이 쓸 수 있게 만들지 않음 | 내장 도구나 Explorer로 파일을 설정된 `LocalLow` 교환 root에 복사한 뒤 HWP 도구 호출 |
| `os error 2` | 경로가 실제로 없거나 `YOUR_NAME`을 바꾸지 않았거나 Desktop이 OneDrive로 파일을 이동함 | 실제 절대 경로 확인, 목적 디렉터리 생성, 또는 설정된 교환 root에 staging |
| 반복 실패 뒤 커넥터가 비활성화됨 | 시작·handshake 실패가 반복되어 Quick이 자동 비활성화함 | command/root 수정·저장, 커넥터를 명시적으로 다시 활성화, 새로고침, 필요하면 Quick 재시작 |
| 커넥터 수정·재import 뒤 `Unknown tool` | Quick이 새 내부 커넥터/tool prefix를 만들었으나 대화는 예전 이름을 보유함 | 연결 새로고침과 새 대화 시작, 예전 생성 이름 대신 HWP 스킬·도구 다시 로드 |
| 에이전트 publish 때 `assetDescriptor contains prohibited HTML/script content` | 구버전 스킬의 angle-bracket placeholder를 Quick이 markup으로 분류함 | 최신 `hwp`에서 스킬 재설치, Quick 새로고침, 다시 publish |
| 생성은 되지만 렌더링 실패 | 폰트 디렉터리가 없거나 접근 불가 | 먼저 `--font-dir` 없이 생성 검증, 그다음 `C:\Windows\Fonts`를 추가하고 `hwp_render` 재시도 |
| Path is outside allowed roots | MCP root 정책이 정상적으로 경로를 거부함 | asset을 설정된 `LocalLow` root로 복사하거나 실제 지원되는 Low 무결성 root 추가. 모든 root를 제거하지 않는다 |

### Windows 경로 수정과 `LocalLow`가 모두 필요한 이유

서로 독립적인 Windows 제약 두 가지가 같은 형태의 오류를 만들었다. 첫째, Rust의 Windows
canonicalization은 같은 경로를 `\\?\C:\...` 같은 verbatim 형식으로 반환할 수 있다. hwp-cli
v0.8.2 이상은 downstream 파일 I/O 전에 안전한 verbatim drive/UNC 경로를 일반 Windows 표기로
정규화하면서 root containment 검사는 fail-closed 상태로 유지한다.

둘째, Quick은 MCP 자식을 Low mandatory integrity(`S-1-16-4096`)로 시작한다. 일반 Medium 폴더는
시작·도구 탐색에 필요한 읽기는 허용하면서도 쓰기에 사용하는 atomic staging 디렉터리는 거부할 수
있다. `AppData\LocalLow`는 상속 가능한 Low 레이블을 가지므로 전용 `hwp-quick-workspace` 자식에서
root 범위를 넓히지 않고 탐색과 쓰기를 모두 수행할 수 있다.

두 제약 때문에 커넥터 탐색은 성공하지만 첫 쓰기만 실패하는 혼동이 생긴다. 항상 `hwp_new`와
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

정상 기동 시 “Started ... with 20 tools”, “Loaded 1/1 servers (0 failed), 20 total tools”와 같은
메시지가 보인다. 로그 문구와 위치는 안정된 API 계약이 아니다.

## 완료 체크리스트

- 커넥터 command가 검증한 `hwp.exe` 절대 경로 하나를 사용함
- 커넥터가 분리된 JSON 인자를 사용하며 셸 따옴표가 들어 있지 않음
- `%USERPROFILE%\AppData\LocalLow\hwp-quick-workspace`가 존재하고 Low 레이블을 상속하며 Windows MCP root로 설정됨
- 현재 HWP 스킬이 활성 Quick 프로필에 설치됨
- Test connection이 **Connected**, **20 tools available**을 표시함
- 새로고침·재시작 뒤에도 커넥터가 활성 상태임
- 절대 `LocalLow` smoke-test 경로에서 `hwp_new`, `hwp_validate`, `hwp_read`가 성공함
- 같은 root 아래에 둔 그 smoke-test 문서의 사본 두 개로 `hwp_merge`, `hwp_split`,
  `hwp_compare`가 성공함
- `hwp_validate`가 `valid: true`를 반환하고, 모양이 중요하면 렌더링도 확인함
- 에이전트가 모든 쓰기 뒤 검증하고 최종 artifact 경로를 반환함

Amazon Quick Web은 이 로컬 stdio 흐름을 실행할 수 없다. 현재 변환·업로드 방법과 계획된 remote MCP
구조는 [AI 클라이언트 연동](ai-integrations.ko.md#amazon-quick-web)을 참고한다.
