[한국어](README.ko.md) · [English](README.md)

# docs

이 저장소의 모든 문서 페이지 지도다. 각 페이지가 답하는 질문과 읽을 때를 정리했다. 에이전트는 여기서
시작해 지금 질문에 답하는 페이지 하나를 찾는다. 사용자용 페이지는 영문 정본(`NAME.md`)과 한국어
페어(`NAME.ko.md`)를 함께 둔다.

## 필요한 페이지 찾기

| 경로 | 답하는 질문 | 읽을 때 |
|---|---|---|
| [../README.ko.md](../README.ko.md) | `ahwp`가 무엇인지, 설치 방법, 빠른 시작 | 프로젝트를 처음 볼 때 |
| [manual/capabilities.ko.md](manual/capabilities.ko.md) | 기능 목록, 구현 상태, 한계, 로드맵 | 기능을 쓰기 전에 있는지 확인할 때 |
| [manual/install.ko.md](manual/install.ko.md) | 모든 설치 경로, 폰트, 서버리스·컨테이너 빌드, 업데이트와 되돌리기 | README에 없는 플랫폼이나 빌드 단계에 설치할 때 |
| [manual/commands.ko.md](manual/commands.ko.md) | 모든 명령의 한 줄 요약과 예시 | 어떤 명령을 쓸지 고를 때 |
| [manual/cli-reference.ko.md](manual/cli-reference.ko.md) | clap 정의에서 자동 생성한 모든 명령의 모든 플래그(수동 편집 금지) | 정확한 플래그나 인자가 필요할 때 |
| [manual/markdown-export.ko.md](manual/markdown-export.ko.md) | HWP를 Markdown으로 바꾸는 규칙과 `--with-segments`의 출력 | 검색·RAG·추출 근거용으로 텍스트를 뽑을 때 |
| [manual/authoring.ko.md](manual/authoring.ko.md) | DocumentSpec/TemplateSpec 합성, 공문서, JSON IR 재생성 | 기존 문서를 고치지 않고 새로 만들 때 |
| [manual/certification.ko.md](manual/certification.ko.md) | `certify`가 검사하는 항목과 구조 코퍼스 게이트 실행 방법 | 작성한 파일이 정책을 만족하거나 재현 가능하다는 증거가 필요할 때 |
| [manual/mcp-server.ko.md](manual/mcp-server.ko.md) | MCP 도구, 클라이언트 설정, read → edit → rewrite 왕복 | AI 에이전트에서 MCP로 `ahwp`를 쓸 때 |
| [manual/agent-conventions.ko.md](manual/agent-conventions.ko.md) | 워크플로, 거부 코드, 보존 손실 원장, 종료 코드, 암호, 환경 변수 | 에이전트가 문서를 편집·채우기·바인딩·스타일 변경하거나 거부를 처리할 때 |
| [manual/ai-integrations.ko.md](manual/ai-integrations.ko.md) | 클라이언트별 설정(Claude, Codex, Kiro, Kimi, claude.ai, Amazon Quick)과 에이전트 스킬 | 특정 AI 클라이언트에 `ahwp`를 등록할 때 |
| [manual/amazon-quick-desktop.ko.md](manual/amazon-quick-desktop.ko.md) | Amazon Quick Desktop 설정, 확인 테스트, 에이전트 지침, 문제 해결 | Amazon Quick Desktop을 설정할 때 |
| [manual/development.ko.md](manual/development.ko.md) | 크레이트 구성, 빌드·테스트 명령 | 코드에 기여할 때 |
| [design/00-overview.ko.md](design/00-overview.ko.md) | 설계 기록의 색인과 원칙 | reader·writer·renderer를 바꿀 때 |
| [design/07-hangul-compat-rules.ko.md](design/07-hangul-compat-rules.ko.md) | 한글 실기에서 확립한 호환 규칙 | writer를 건드리기 전(필독) |
| [design/10-hwp5-structure-map.ko.md](design/10-hwp5-structure-map.ko.md) | HWP 5.0 레코드·컨트롤 카탈로그 | HWP5 레코드를 다룰 때 |
| [design/11-hwpx-structure-map.ko.md](design/11-hwpx-structure-map.ko.md) | OWPML 요소 카탈로그 | HWPX 요소를 다룰 때 |
| [design/12-feature-gaps.ko.md](design/12-feature-gaps.ko.md) | 근거와 난이도를 붙인 미구현 기능 목록 | 기능이 없다고 단정하기 전 |
| [design/22-remote-mcp-deployment.ko.md](design/22-remote-mcp-deployment.ko.md) | 공유 `ahwp serve` HTTP adapter와 Cloudflare·AgentCore tier | MCP 서버를 원격 배포할 때 |
| [design/23-hwpx-skill-absorption.ko.md](design/23-hwpx-skill-absorption.ko.md) | 퇴역한 `hwpx` 스킬과 네이티브 명령의 패리티 기록 | 예전 `hwpx` 스킬에서 옮겨 올 때 |
| [../deploy/aws/README.ko.md](../deploy/aws/README.ko.md) | AWS AgentCore 배포 절차 | AWS에 배포할 때 |
| [../deploy/cloudflare/README.md](../deploy/cloudflare/README.md) | Cloudflare 컨테이너 배포 절차 | Cloudflare에 배포할 때 |
| [release-readiness.ko.md](release-readiness.ko.md) | 릴리스 전 게이트 체크리스트 | 릴리스를 만들 때 |
| [hancom-verification-checklist.ko.md](hancom-verification-checklist.ko.md) | 한글 실기에서 작성 파일을 검증하는 방법 | 파일이 한글에서 열리는지 확인할 때 |
| [architecture/](architecture/) | 워크스페이스와 MCP 배포 경로를 담은 대화형 아키텍처 지도. `hwp-cli-rendered.html`을 브라우저로 연다. `hwp-cli.architecture.json`에서 Archify로 생성했고, `visual-check` 파일은 그 브라우저 검증 기록이다 | 크레이트 구성을 시각적으로 보고 싶을 때 |
| [agents-background.md](agents-background.md) | AGENTS.md 규칙의 배경 | AGENTS.md를 고칠 때 |

`manual/cli-reference*.md`는 clap 정의에서 두 언어 모두 자동 생성한다(수동 편집 금지).

## HWP 5.0 스펙 자료 안내

파서·렌더러 작업에 참고하는 HWP 5.0 포맷 스펙 자료에 대한 안내.

### 스펙 문서는 저장소에 동봉하지 않는다

HWP 5.0 파일 형식 명세("한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0")의 저작권은
(주)한글과컴퓨터에 있다. 한컴 공개 문서 라이선스는 자유로운 열람·복사·배포를 허용하되 배포는
**수정되지 않은 원본 또는 그 복사본**으로 제한한다. 따라서 텍스트 추출본·페이지 캡처 같은 파생물은
재배포 허용 범위 밖이며, 이 저장소는 스펙 문서를 **동봉하지 않고** 공식 배포처 링크만 제공한다.

- **공식 다운로드**: <https://store.hancom.com/etc/hwpDownload.do>
  (HWP·PDF 양식, `한글문서파일형식_5.0_revision1.3`)
- 코드 주석은 스펙을 섹션 번호로 인용한다(예: `한글문서파일형식 5.0 §4.x`). 공정 이용 범위다.

> 로컬에서 스펙 원본(`hwp5_spec.pdf`, `한글문서파일형식_5.0_revision1.3.hwp` 등)을 `docs/`에 두고
> 작업할 수 있으나, 이 파일들은 루트 `.gitignore`로 추적되지 않는다.

### 고지

본 제품은 한글과컴퓨터의 한글 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.
