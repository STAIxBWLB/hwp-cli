[한국어](release-readiness.ko.md) · [English](release-readiness.md)

# 릴리스 준비 체크리스트

깨끗한 체크아웃에서 실행한다. 이 체크리스트는 게이트를 기록할 뿐이며, 커밋·푸시·태그·패키지 업로드·
릴리스 게시를 승인하지 않는다.

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `scripts/check-structured-corpus.sh`
- [ ] 로컬 소스 빌드 기준으로 Linux·macOS·Windows CI 매트릭스 전부 green (green 신호는 커밋
      자체의 check run이거나, ci.yml이 중복 제거한 main 푸시의 경우 트리가 동일한 병합 PR
      head의 green run일 수 있음)
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
- [ ] 도구 개수 표기가 서버와 일치: `ahwp mcp`의 `tools/list` 길이가 `README*.md`,
      `docs/manual/*.md`, `skills/hwp/SKILL*.md`의 모든 "N개 도구" 표기와 같은지 확인
- [ ] 번들 `skills/hwp/SKILL.md`·`SKILL.ko.md`가 이번 릴리스가 추가한 명령과 MCP 도구를 모두
      담고 있는지 확인 — 에이전트가 실제로 읽는 것은 export된 스킬이며 그 내용을 검사하는
      게이트는 없다
- [ ] 퇴역한 다운스트림 `STAIxBWLB/skills`의 `skills/hwpx`가
      [design/23-hwpx-skill-absorption](design/23-hwpx-skill-absorption.ko.md)의 패리티
      매트릭스 — 퇴역(RET-01, 2026-08-27 종료)이 무엇을 대체했는지 남긴 기록 — 와 여전히
      맞는지 확인
- [ ] 로컬 전용 픽스처(`fixtures/hwp5/`, `fixtures/hwpx/`, `fixtures/pdf-parity/private/`)를 둔
      상태에서 로컬 `HWP_REQUIRE_FIXTURES=1 scripts/check.sh` 통과(`fixtures/README.ko.md`의
      선택 세트는 집계만 된다). CI와 준비 점검 워크플로는 데이터 정책상 이 픽스처가 없으므로 이
      플래그를 설정하지 않고(항상 실패한다) `skipped-for-missing-fixtures=N (optional=M)`만
      출력한다. 녹색 CI가 검증한 범위는 그 N개 스킵을 뺀 나머지다(#275)
- [ ] `bash scripts/macos-release-signing.sh --dry-run` 통과. helper 문법과 서명 계획만 확인하며,
      Apple 자격증명이나 실제 서명·공증을 검증한 결과가 아님

릴리스는 7종 스모크 픽스처가 모든 실제 문서 형태를 커버한다거나, 한컴 픽셀 동등성을 제공한다거나,
플랫폼 간 래스터 바이트가 동일함을 증명한다고 주장해서는 안 된다.

## macOS 서명과 공증

macOS 릴리스 작업에는 저장소의 `Settings > Secrets and variables > Actions`에 등록한
Actions secret이 필요합니다. 값이 없으면 업로드 action 실행 전에 해당 macOS 작업을 중단하며,
ad-hoc 서명으로 대체하지 않습니다.

- `APPLE_CERTIFICATE`: Developer ID Application `.p12`의 base64 인코딩 값
- `APPLE_CERTIFICATE_PASSWORD`: 해당 `.p12`의 암호
- `KEYCHAIN_PASSWORD`: 임시 서명 keychain의 암호
- `APPLE_API_KEY`: App Store Connect `.p8` PEM 내용(base64 PEM도 허용)
- `APPLE_API_KEY_ID`: App Store Connect API key ID
- `APPLE_API_ISSUER_ID`: App Store Connect issuer ID

저장소의 `Settings > Secrets and variables > Actions > Variables`에는 Actions **variable**
`APPLE_TEAM_ID`도 등록합니다. 값은 ASCII 대문자 또는 숫자로 된 10자리 Apple Developer team
ID이며, 비밀값이 아닌 공개 서명자 메타데이터입니다. 값이 없거나 형식이 잘못되면 빌드 전 준비
단계와 이후 검증 단계에서 중단합니다.

workflow는 인증서를 접근이 제한된 임시 keychain에 가져온 뒤, 유효한 Developer ID Application
identity 하나를 pinned upload action에 전달하고 `codesign-options: runtime`을 지정합니다.
Developer ID 서명은 기본적으로 timestamp를 포함합니다. helper는 secure timestamp, hardened
runtime, Developer ID Application authority를 확인한 뒤, 서명된 실행 파일을 ZIP으로 묶어
`notarytool submit --wait`에 제출합니다. 하나라도 없으면 거부하며, 공증은 `Accepted`일 때만
통과합니다. 임시 인증서,
API key, keychain은 helper와 항상 실행되는 cleanup 단계에서 제거합니다.

Linux의 alias 재포장 이후 macOS 작업이 각 아키텍처의 archive를 다운로드해 `ahwp`와 `hwp`의
바이트가 같은지 비교합니다. 두 코드 서명은 Apple anchor와 `vars.APPLE_TEAM_ID`의 인증서
leaf team ID 요구 조건으로 검증합니다. 두 명령에 `spctl --assess --type install --verbose=2`도
실행하며, 성공 종료와 `accepted`, `source=Notarized Developer ID`를 모두 요구합니다. ticket
전파 지연을 위해 각 명령을 최대 3회 검사하고 회차 사이에 10초를 기다립니다(명령당 재시도
대기 최대 20초). 횟수를 소진하면 재시도 한도, 마지막 검사 출력과 다음 진단 방법을 알리고
작업을 실패 처리합니다. 이 작업이 통과해야 attestation과 공개 릴리스가 진행됩니다. 단일 실행
파일에는 ticket을 staple할 수 없으므로 Gatekeeper가 공증 ticket을 온라인으로 조회합니다.

readiness는 자격증명을 사용하지 않는 `--dry-run` 경로만 실행합니다. shell mock은 Apple 서비스
없이 실패 경로를 검사합니다. 어느 결과도 실제 자격증명, 공증 또는 Gatekeeper의 동작을 증명하지
않습니다. 첫 실제 검증은 소유자가 승인한 `v1.6.3` 릴리스에서 수행합니다. 두 macOS
`verify-macos-assets` 작업 결과를 기록하고, 다운로드한 두 명령을 Gatekeeper로 검사합니다.
빈 임시 디렉터리에서 Apple Silicon용으로 실행하는 예시는 다음과 같습니다.

```bash
gh release download v1.6.3 --repo STAIxBWLB/agent-hwp --pattern 'ahwp-v1.6.3-aarch64-apple-darwin.tar.gz'
tar -xzf ahwp-v1.6.3-aarch64-apple-darwin.tar.gz
APPLE_TEAM_ID="$(gh variable get APPLE_TEAM_ID --repo STAIxBWLB/agent-hwp)"
signer_requirement="anchor apple generic and certificate leaf[subject.OU] = \"$APPLE_TEAM_ID\""
codesign --verify --strict --verbose=2 -R "=$signer_requirement" ./ahwp
codesign -dv --verbose=4 ./ahwp
codesign --verify --strict --verbose=2 -R "=$signer_requirement" ./hwp
codesign -dv --verbose=4 ./hwp
spctl --assess --type install --verbose=2 ./ahwp
spctl --assess --type install --verbose=2 ./hwp
```

`x86_64-apple-darwin` archive에서도 서명과 Gatekeeper 검사를 반복합니다. authority는 Developer ID
Application이어야 하며, 서명에는 hardened runtime과 secure timestamp가 있어야 합니다.
Gatekeeper의 기대 출력은 `accepted`와 `source=Notarized Developer ID`입니다. readiness는
태그를 만들거나 이 실제 검증을 수행하지 않습니다.
