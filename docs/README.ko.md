[한국어](README.ko.md) · [English](README.md)

# docs

파서·렌더러 작업에 참고하는 HWP 5.0 포맷 스펙 자료에 대한 안내.

## 문서 구성

| 경로 | 내용 |
|---|---|
| [design/](design/) | 설계 지식 정본. 시작점은 [00-overview](design/00-overview.ko.md) |
| [manual/cli-reference.ko.md](manual/cli-reference.ko.md) | clap 정의에서 자동 생성되는 CLI 레퍼런스(수동 편집 금지) |
| [manual/ai-integrations.ko.md](manual/ai-integrations.ko.md) | AI 클라이언트 설정: MCP 등록, 에이전트 스킬, claude.ai 번들 |
| [manual/amazon-quick-desktop.ko.md](manual/amazon-quick-desktop.ko.md) | Amazon Quick Desktop 설정, end-to-end 검증, 에이전트 지침, 문제 해결 |
| [release-readiness.ko.md](release-readiness.ko.md) | 릴리스 전 게이트 체크리스트 |
| [hancom-verification-checklist.ko.md](hancom-verification-checklist.ko.md) | 한글 실기 검증 체크리스트 |

사용자용 문서는 영문 정본(`NAME.md`)과 한국어 페어(`NAME.ko.md`)를 함께 둔다.
`manual/cli-reference*.md`는 clap 정의에서 두 언어 모두 자동 생성한다(수동 편집 금지).

## 스펙 문서는 저장소에 동봉하지 않는다

HWP 5.0 파일 형식 명세("한글 문서 파일 형식 5.0 / HWP Document File Formats 5.0")의 저작권은
(주)한글과컴퓨터에 있다. 한컴 공개 문서 라이선스는 자유로운 열람·복사·배포를 허용하되 배포는
**수정되지 않은 원본 또는 그 복사본**으로 제한한다. 따라서 텍스트 추출본·페이지 캡처 같은 파생물은
재배포 허용 범위 밖이며, 이 저장소는 스펙 문서를 **동봉하지 않고** 공식 배포처 링크만 제공한다.

- **공식 다운로드**: <https://store.hancom.com/etc/hwpDownload.do>
  (HWP·PDF 양식, `한글문서파일형식_5.0_revision1.3`)
- 코드 주석은 스펙을 섹션 번호로 인용한다(예: `한글문서파일형식 5.0 §4.x`). 공정 이용 범위다.

> 로컬에서 스펙 원본(`hwp5_spec.pdf`, `한글문서파일형식_5.0_revision1.3.hwp` 등)을 `docs/`에 두고
> 작업할 수 있으나, 이 파일들은 루트 `.gitignore`로 추적되지 않는다.

## 고지

본 제품은 한글과컴퓨터의 한글 문서 파일(.hwp) 공개 문서를 참고하여 개발하였습니다.
