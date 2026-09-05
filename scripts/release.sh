#!/usr/bin/env bash
# scripts/release.sh X.Y.Z <readiness-run-url>
#
# 워크스페이스 버전(Cargo.toml [workspace.package] version)을 bump 하고 커밋 + 태그를
# 만든다. main 은 보호 브랜치라 이 커밋을 직접 밀 수 없다 — 브랜치에서 실행하고 PR 로
# 머지한 뒤, 머지 커밋에 태그를 다시 붙인다(태그 푸시가 release.yml 을 트리거한다):
#
#     git switch -c chore/release-v0.2.0
#     scripts/release.sh 0.2.0 && git tag -d v0.2.0   # 브랜치 태그는 버린다
#     gh pr create ... && gh pr merge --squash        # CHANGELOG 절 마감도 같은 PR 에서
#     git switch main && git pull                     # main CI 가 초록인지 확인 후
#     git tag -a v0.2.0 -m v0.2.0 && git push origin v0.2.0
#
# 두 번째 인수는 release-readiness 워크플로 실행 URL이다. docs/release-readiness.md는 릴리스
# 문구가 제외된 parity 게이트와 그 측정 거리를 밝히도록 요구하므로, bump 전에
# scripts/release_verification_block.sh 가 그 URL로 해당 버전 절에 마커로 둘러싸인
# **Verification** 블록을 쓰고, scripts/check-verification-block.sh 가 그 실행이 실제로
# release-readiness.yml 의 성공한 실행이며 릴리스 대상 커밋을 평가했는지 GitHub API 로
# 확인한다. 어느 하나라도 어긋나면 파일을 건드리기 전에(또는 되돌린 뒤) 거부한다.
# 우회 플래그는 없다.
#
#     scripts/release.sh 0.18.0 https://github.com/STAIxBWLB/hwp-cli/actions/runs/1234567890
#
# 자체 점검:  scripts/release.sh --self-test
set -euo pipefail

# 시맨틱 버전(프리릴리스는 하이픈으로 시작): 0.2.0, 1.10.3, 0.2.0-rc1, 0.2.0-rc.1
semver_ok() { printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; }
# Cargo.toml [workspace.package] 섹션의 version 값을 뽑는다 (awk — GNU/BSD 이식성).
extract_version() {
  awk '/^\[workspace\.package\]/{p=1;next} /^\[/{p=0} p&&/^version[[:space:]]*=/{sub(/^version[[:space:]]*=[[:space:]]*"/,"");sub(/".*/,"");print;exit}' "$1"
}

# --- 자체 점검(git 부작용 없음) ---------------------------------------------
if [ "${1:-}" = "--self-test" ]; then
  for v in 0.2.0 1.10.3 0.2.0-rc1 10.0.0; do
    semver_ok "$v" || { echo "self-test FAIL: '$v' 는 통과해야 함"; exit 1; }
  done
  for v in "" v1.2.3 1.2 1.2.3.4 1.2.x abc; do
    semver_ok "$v" && { echo "self-test FAIL: '$v' 는 거부돼야 함"; exit 1; }
  done
  cur=$(extract_version "$(git rev-parse --show-toplevel)/Cargo.toml")
  semver_ok "$cur" || { echo "self-test FAIL: Cargo.toml version '$cur' 파싱 불가"; exit 1; }
  echo "self-test OK (semver 규칙 + Cargo.toml=$cur)"
  exit 0
fi

# --- 릴리스 준비 -------------------------------------------------------------
ver="${1:-}"
readiness_url="${2:-}"
[ -n "$ver" ] || { echo "usage: scripts/release.sh X.Y.Z <readiness-run-url>" >&2; exit 2; }
semver_ok "$ver" || { echo "오류: 시맨틱 버전 형식이 아닙니다: '$ver' (예: 0.2.0, 0.2.0-rc1)" >&2; exit 1; }

cd "$(git rev-parse --show-toplevel)"

[ -z "$(git status --porcelain)" ] || { echo "오류: 작업 트리가 깨끗하지 않습니다. 커밋/스태시 후 다시 실행하세요." >&2; exit 1; }
if git rev-parse -q --verify "refs/tags/v$ver" >/dev/null; then
  echo "오류: 태그 v$ver 가 이미 존재합니다." >&2; exit 1
fi

# 릴리스 준비 증거 게이트 — 파일을 하나라도 고치기 전에 거부한다.
if [ -z "$readiness_url" ]; then
  cat >&2 <<EOF
오류: release-readiness 실행 URL이 없습니다. 버전 bump, 커밋, 태그를 모두 중단합니다.
      릴리스 준비 워크플로 .github/workflows/release-readiness.yml 은 Phase 4 plan 04-04
      이 추가한다(PR 대기 중). 그 PR 이 머지되기 전에는 dispatch 할 수 없으므로 릴리스도
      진행할 수 없다. 머지된 뒤 릴리스 대상 커밋(지금의 HEAD)으로 dispatch 하고, 그 실행
      URL 을 두 번째 인수로 넘긴다:
        scripts/release.sh $ver https://github.com/STAIxBWLB/hwp-cli/actions/runs/<run-id>
EOF
  exit 1
fi
bash scripts/release_verification_block.sh "$ver" "$readiness_url"
# 블록 자체 + 인용된 실행(성공 여부, 워크플로, head_sha)까지 확인한다. release.yml 도 태그
# 커밋에 대해 같은 스크립트를 돌린다. 실패하면 방금 쓴 블록을 되돌려 트리를 원상복구한다
# (위에서 트리가 깨끗함을 이미 확인했다).
if ! bash scripts/check-verification-block.sh "$ver" "$(git rev-parse HEAD)"; then
  git checkout -- CHANGELOG.md
  echo "오류: 릴리스 준비 증거가 확인되지 않았습니다. CHANGELOG.md 변경을 되돌렸고, 태그를 만들지 않습니다." >&2
  exit 1
fi

old=$(extract_version Cargo.toml)
[ "$old" != "$ver" ] || { echo "오류: 이미 버전이 $ver 입니다." >&2; exit 1; }

# [workspace.package] 섹션의 첫 version 라인만 교체.
perl -0pi -e 's/(\[workspace\.package\][^\[]*?\nversion = ")[^"]*(")/${1}'"$ver"'${2}/s' Cargo.toml

new=$(extract_version Cargo.toml)
if [ "$new" != "$ver" ]; then
  echo "오류: Cargo.toml 버전 교체 실패(현재: '$new'). 수동 확인 필요." >&2
  git checkout -- Cargo.toml
  exit 1
fi

# Cargo.lock 의 워크스페이스 크레이트 버전만 재잠금. --workspace 는 워크스페이스
# 패키지만 대상이고, --offline 로 레지스트리 조회를 막아 외부 의존성은 절대 bump 되지
# 않게 한다(릴리스 커밋에 원치 않는 dependency 변경 혼입 방지).
cargo update --workspace --offline >/dev/null

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): v$ver"
git tag -a "v$ver" -m "v$ver"

cat <<EOF
✅ v$ver 준비 완료 ($old → $ver, 커밋 + 태그 생성).
   푸시하면 릴리스 CI(테스트 통과 + 버전 확인 후 빌드)가 트리거됩니다:
     git push origin main && git push origin v$ver
EOF
