[한국어](release-readiness.md) · [English](release-readiness.en.md)

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
- [ ] `git status --short --untracked-files=all` 검토, 무관한 사용자 변경 제외
- [ ] 준비 점검 실행 자체가 커밋·푸시·태그·패키지 업로드·릴리스를 수행하지 않음

릴리스는 7종 스모크 픽스처가 모든 실제 문서 형태를 커버한다거나, 한컴 픽셀 동등성을 제공한다거나,
플랫폼 간 래스터 바이트가 동일함을 증명한다고 주장해서는 안 된다.
