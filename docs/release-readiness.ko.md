[한국어](release-readiness.ko.md) · [English](release-readiness.md)

# 릴리스 준비 체크리스트

깨끗한 체크아웃에서 실행한다. 이 체크리스트는 게이트를 기록할 뿐이며, 커밋·푸시·태그·패키지 업로드·
릴리스 게시를 승인하지 않는다.

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `scripts/check-structured-corpus.sh`
- [ ] 로컬 소스 빌드 기준으로 Linux·macOS·Windows CI 매트릭스 전부 green
- [ ] 코퍼스 manifest·run·artifact JSON이 고정 스키마를 통과
- [ ] manifest의 모든 pin과 `corpus/structured-v1/TRACKED_FILES.txt`가 Git 인덱스에 존재
- [ ] 코퍼스 잡이 시스템 폰트 설치나 `HWP_FONT_DIR`에 의존하지 않음
- [ ] `scripts/fetch-corpus-fonts.sh`가 빈 `fonts/`에서 고정 폰트 해시를 재현
- [ ] 코퍼스를 배포에 포함한다면 릴리스 아카이브·라이선스 목록에 Noto Sans KR OFL과 metadata 포함
- [ ] 독립 오라클은 digest 고정 이미지를 실제로 빌드·검증하기 전까지 부분 구현으로 유지
- [ ] 비공개 한글 열기 검증 receipt를 `hancom-verification-receipt-v1` 스키마로 기록
      (인증 `hancom_open` 증거)
- [ ] 비공개 PDF parity run 적격; 매니페스트에 선언한 `gate_exclusions` 전 항목을 릴리스
      노트에 열거하고 사유 기재. 현재 4개: `fonts`(오라클 자신이 문서의 `substFont` 선언대로
      대체하므로 해당 케이스에서 `substitution_free`는 도달 불가 — 21-pdf-parity §4.5),
      그리고 `text`·`raster`·`roi`(측정·보고는 유지, 격차는 21-pdf-parity §4.6에 기록)
- [ ] 릴리스 카피에 제외 게이트와 측정된 격차를 명시. 제외가 있는 프로파일에 대해
      "한컴 동등성" 주장 금지
- [ ] `git status --short --untracked-files=all` 검토, 무관한 사용자 변경 제외
- [ ] 준비 점검 실행 자체가 커밋·푸시·태그·패키지 업로드·릴리스를 수행하지 않음
- [ ] `README.md`·`README.ko.md`의 버전 고정 예시(`install.sh --tag` 스니펫)를 이번 릴리스
      버전으로 갱신
- [ ] 도구 개수 표기가 서버와 일치: `hwp mcp`의 `tools/list` 길이가 `README*.md`,
      `docs/manual/*.md`, `skills/hwp/SKILL*.md`의 모든 "N개 도구" 표기와 같은지 확인
- [ ] 번들 `skills/hwp/SKILL.md`·`SKILL.ko.md`가 이번 릴리스가 추가한 명령과 MCP 도구를 모두
      담고 있는지 확인 — 에이전트가 실제로 읽는 것은 export된 스킬이며 그 내용을 검사하는
      게이트는 없다
- [ ] 퇴역한 다운스트림 `STAIxBWLB/skills`의 `skills/hwpx`가
      [design/23-hwpx-skill-absorption](design/23-hwpx-skill-absorption.ko.md)의 패리티
      매트릭스 — 퇴역(RET-01, 2026-08-27 종료)이 무엇을 대체했는지 남긴 기록 — 와 여전히
      맞는지 확인

릴리스는 7종 스모크 픽스처가 모든 실제 문서 형태를 커버한다거나, 한컴 픽셀 동등성을 제공한다거나,
플랫폼 간 래스터 바이트가 동일함을 증명한다고 주장해서는 안 된다.
