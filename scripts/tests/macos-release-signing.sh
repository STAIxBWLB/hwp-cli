#!/usr/bin/env bash
# Exercise the release helper with fake credentials and fake macOS tools only.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
helper="${MACOS_SIGNING_HELPER:-$root/scripts/macos-release-signing.sh}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/runner" "$tmp/home" "$tmp/tmp"
export HOME="$tmp/home" TMPDIR="$tmp/tmp"
export RUNNER_TEMP="$tmp/runner" GITHUB_OUTPUT="$tmp/output" MOCK_LOG="$tmp/codesign.log" MOCK_CALLS="$tmp/native-calls.log"
export APPLE_CERTIFICATE=ZmFrZS1jZXJ0aWZpY2F0ZQ==
export APPLE_CERTIFICATE_PASSWORD=fixture-certificate-password KEYCHAIN_PASSWORD=fixture-keychain-password
export APPLE_API_KEY=$'-----BEGIN PRIVATE KEY-----\nfixture-private-key\n-----END PRIVATE KEY-----\n'
export APPLE_TEAM_ID=AB12CD34EF
export APPLE_API_KEY_ID=fixture-api-key-id APPLE_API_ISSUER_ID=fixture-api-issuer-id

cat >"$tmp/bin/uname" <<'SH'
#!/usr/bin/env bash
echo uname >>"$MOCK_CALLS"
echo Darwin
SH
cat >"$tmp/bin/security" <<'SH'
#!/usr/bin/env bash
echo security >>"$MOCK_CALLS"
[ "${MOCK_SECURITY_FAIL:-}" != "$1" ] || exit 1
case "$1" in
create-keychain) touch "${!#}" ;;
find-identity)
    if [ "${MOCK_IDENTITY:-valid}" != absent ]; then
        echo '  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Developer ID Application: Fixture"'
        if [ "${MOCK_IDENTITY:-valid}" = duplicate ]; then
            echo '  2) 1123456789ABCDEF0123456789ABCDEF01234567 "Developer ID Application: Other Fixture"'
        fi
    fi
    ;;
delete-keychain) rm -f "$2" ;;
esac
SH
cat >"$tmp/bin/codesign" <<'SH'
#!/usr/bin/env bash
echo codesign >>"$MOCK_CALLS"
name="${!#}"; name="${name##*/}"
printf '%s:%s\n' "$1" "$name" >>"$MOCK_LOG"
case "$1" in
--verify)
    [ "${MOCK_VERIFY:-valid}" = valid ] || exit 1
    requirement=''
    while [ "$#" -gt 0 ]; do
        if [ "$1" = -R ]; then requirement="$2"; break; fi
        shift
    done
    # codesign treats a plain requirement argument as a file, not inline text.
    case "$requirement" in
        =*) ;;
        '') ;;
        *) [ -f "$requirement" ] || exit 1 ;;
    esac
    # Model a different certificate team: only a pinned requirement detects it.
    if [ "${MOCK_TEAM:-valid}" = wrong ] && [ -n "$requirement" ]; then exit 1; fi
    if [ "${MOCK_REQUIRE_TEAM:-0}" = 1 ]; then
        [ "$requirement" = '=anchor apple generic and certificate leaf[subject.OU] = "AB12CD34EF"' ] || exit 1
    fi
    ;;
-dv)
    [ "${MOCK_AUTHORITY:-valid}" = absent ] || echo 'Authority=Developer ID Application: Fixture' >&2
    [ "${MOCK_RUNTIME:-valid}" = absent ] || echo 'CodeDirectory v=20500 size=100 flags=0x10000(runtime) hashes=1' >&2
    [ "${MOCK_TIMESTAMP:-valid}" = absent ] || echo 'Timestamp=Nov 13, 2026 at 12:00:00' >&2
    ;;
*) exit 1 ;;
esac
SH
cat >"$tmp/bin/xcrun" <<'SH'
#!/usr/bin/env bash
echo xcrun >>"$MOCK_CALLS"
case " $* " in *' --wait '*) ;; *) exit 1 ;; esac
case " $* " in *' --output-format json '*) ;; *) exit 1 ;; esac
case "$3" in *.zip) [ -f "$3" ] ;; *) exit 1 ;; esac
python3 - "$3" <<'PYZIPCHECK'
import sys
import zipfile
with zipfile.ZipFile(sys.argv[1]) as archive:
    assert archive.namelist() == ["ahwp"]
    assert archive.read("ahwp") == b"signed executable\n"
PYZIPCHECK
if [ "${MOCK_NOTARY:-Accepted}" = malformed ]; then
    echo not-json
else
    python3 - <<'PYRESULT'
import json
import os
print(json.dumps({"id": os.environ.get("MOCK_NOTARY_ID", "01234567-89ab-cdef-0123-456789abcdef"), "status": os.environ.get("MOCK_NOTARY", "Accepted"), "extra": "fixture-result-detail-not-requested"}))
PYRESULT
fi
if [ "${MOCK_SUBMIT_FAIL:-0}" = 1 ]; then
    echo 'HTTP 401: fixture notarization authentication failed' >&2
    exit 1
fi
SH
cat >"$tmp/bin/spctl" <<'SH'
#!/usr/bin/env bash
echo spctl >>"$MOCK_CALLS"
[ "$1 $2 $3 $4" = '--assess --type install --verbose=2' ] || exit 1
count=0
[ ! -f "$RUNNER_TEMP/assessment-count" ] || read -r count <"$RUNNER_TEMP/assessment-count"
count=$((count + 1))
printf '%s\n' "$count" >"$RUNNER_TEMP/assessment-count"
echo "assessment-detail: fixture-attempt-$count" >&2
case "${MOCK_SPCTL:-accepted}" in
    rejected) echo "${!#}: rejected"; exit 1 ;;
    wrong-source) echo "${!#}: accepted"; echo 'source=Developer ID'; exit 0 ;;
    nonzero-accepted) echo "${!#}: accepted"; echo 'source=Notarized Developer ID'; exit 1 ;;
    retry) if [ "$count" -lt 3 ]; then echo "${!#}: rejected"; exit 1; fi ;;
esac
echo "${!#}: accepted"
echo 'source=Notarized Developer ID'
SH
cat >"$tmp/bin/sleep" <<'SH'
#!/usr/bin/env bash
[ "$1" = 10 ] || exit 1
echo sleep >>"$MOCK_CALLS"
SH
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"

expect_failure() {
    local message="$1"; shift
    if "$@" >"$tmp/log" 2>&1; then
        echo "FAIL: $message unexpectedly passed" >&2
        exit 1
    fi
    grep -F "$message" "$tmp/log" >/dev/null || { cat "$tmp/log"; echo "FAIL: missing expected diagnostic: $message" >&2; exit 1; }
}

for name in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD KEYCHAIN_PASSWORD APPLE_API_KEY APPLE_API_KEY_ID APPLE_API_ISSUER_ID; do
    expect_failure "$name is missing" env -u "$name" bash "$helper" prepare
    grep -F 'Settings > Secrets and variables > Actions' "$tmp/log" >/dev/null
    if grep -E 'fixture-(private-key|certificate-password|keychain-password|api-key-id|api-issuer-id)|ZmFrZS1jZXJ0aWZpY2F0ZQ==' "$tmp/log" >/dev/null; then
        echo 'FAIL: missing-secret diagnostics exposed fixture credentials' >&2
        exit 1
    fi
done

expect_failure 'exactly one valid Developer ID Application' env MOCK_IDENTITY=absent bash "$helper" prepare
work="$(sed -n 's/^directory=//p' "$GITHUB_OUTPUT")"
bash "$helper" cleanup "$work"
: >"$GITHUB_OUTPUT"
bash "$helper" prepare
work="$(sed -n 's/^directory=//p' "$GITHUB_OUTPUT")"
grep -E '^identity=[[:xdigit:]]{40}$' "$GITHUB_OUTPUT" >/dev/null

python3 - "$tmp" <<'PY'
import io
import pathlib
import sys
import tarfile
root = pathlib.Path(sys.argv[1])
# 2.0 archives carry only `ahwp` (#510 Phase 4). The second archive names the executable
# only by the retired `hwp` name, which verify-archive must refuse.
for archive_name, member in (("signed.tar.gz", "ahwp"), ("legacy-only.tar.gz", "hwp")):
    with tarfile.open(root / archive_name, "w:gz") as archive:
        data = b"signed executable\n"
        entry = tarfile.TarInfo(member)
        entry.size = len(data)
        archive.addfile(entry, io.BytesIO(data))
PY

followup_case() {
    local selected="$1" operation expected
    : >"$MOCK_CALLS"
    rm -f "$RUNNER_TEMP/assessment-count"
    case "$selected" in
        missing-team)
            expect_failure 'APPLE_TEAM_ID is missing' env -u APPLE_TEAM_ID bash "$helper" verify-archive "$tmp/signed.tar.gz"
            grep -F 'Settings > Secrets and variables > Actions > Variables' "$tmp/log" >/dev/null
            [ ! -s "$MOCK_CALLS" ]
            ;;
        invalid-team)
            expect_failure 'exactly 10 uppercase ASCII letters or digits' env APPLE_TEAM_ID='invalid team' bash "$helper" verify-archive "$tmp/signed.tar.gz"
            [ ! -s "$MOCK_CALLS" ]
            ;;
        prepare-missing-team)
            expect_failure 'APPLE_TEAM_ID is missing' env -u APPLE_TEAM_ID bash "$helper" prepare
            grep -F 'Actions > Variables' "$tmp/log" >/dev/null
            [ ! -s "$MOCK_CALLS" ]
            ;;
        prepare-invalid-team|prepare-lowercase-team)
            expected='INVALID'
            [ "$selected" != prepare-lowercase-team ] || expected=ab12cd34ef
            expect_failure 'exactly 10 uppercase ASCII letters or digits' env APPLE_TEAM_ID="$expected" bash "$helper" prepare
            [ ! -s "$MOCK_CALLS" ]
            ;;
        verify-lowercase-team)
            expect_failure 'exactly 10 uppercase ASCII letters or digits' env APPLE_TEAM_ID=ab12cd34ef bash "$helper" verify-archive "$tmp/signed.tar.gz"
            [ ! -s "$MOCK_CALLS" ]
            ;;
        mismatch-remedy)
            expect_failure 'Code signature verification failed' env MOCK_TEAM=wrong bash "$helper" verify-archive "$tmp/signed.tar.gz"
            grep -F 'APPLE_TEAM_ID in repository Actions Variables' "$tmp/log" >/dev/null || { echo 'FAIL: signature refusal must name APPLE_TEAM_ID Variables' >&2; exit 1; }
            grep -F 'APPLE_CERTIFICATE in repository Actions Secrets' "$tmp/log" >/dev/null || { echo 'FAIL: signature refusal must name APPLE_CERTIFICATE Secrets' >&2; exit 1; }
            ;;
        gatekeeper-detail)
            expect_failure 'Gatekeeper install assessment did not accept' env MOCK_SPCTL=rejected bash "$helper" verify-archive "$tmp/signed.tar.gz"
            grep -Fx 'assessment-detail: fixture-attempt-3' "$tmp/log" >/dev/null || { echo 'FAIL: refusal must retain the final Gatekeeper assessment detail' >&2; exit 1; }
            if grep -E '^assessment-detail: fixture-attempt-[12]$' "$tmp/log" >/dev/null; then
                echo 'FAIL: refusal must print only the final Gatekeeper assessment' >&2; exit 1
            fi
            ;;
        wrong-team)
            expect_failure 'Code signature verification failed' env MOCK_TEAM=wrong bash "$helper" verify-archive "$tmp/signed.tar.gz"
            ;;
        designated-team)
            env MOCK_REQUIRE_TEAM=1 bash "$helper" verify-archive "$tmp/signed.tar.gz" >"$tmp/log" 2>&1 || {
                echo 'FAIL: verification must pass the designated team requirement to codesign' >&2; exit 1;
            }
            ;;
        gatekeeper-rejected|gatekeeper-source|gatekeeper-exit)
            case "$selected" in
                gatekeeper-rejected) expected=rejected ;;
                gatekeeper-source) expected=wrong-source ;;
                gatekeeper-exit) expected=nonzero-accepted ;;
            esac
            expect_failure 'Gatekeeper install assessment did not accept' env MOCK_SPCTL="$expected" bash "$helper" verify-archive "$tmp/signed.tar.gz"
            grep -F 'after 3 attempts, 10 seconds apart' "$tmp/log" >/dev/null
            grep -F 'rerun Release' "$tmp/log" >/dev/null
            [ "$(cat "$RUNNER_TEMP/assessment-count")" = 3 ]
            [ "$(grep -c '^sleep$' "$MOCK_CALLS")" = 2 ]
            ;;
        gatekeeper-retry)
            env MOCK_SPCTL=retry bash "$helper" verify-archive "$tmp/signed.tar.gz" >"$tmp/log" 2>&1
            # 2.0 archives carry ahwp alone (#510), so only ahwp is assessed.
            [ -f "$RUNNER_TEMP/assessment-count" ] && [ "$(cat "$RUNNER_TEMP/assessment-count")" = 3 ] || {
                echo 'FAIL: Gatekeeper retry must assess ahwp three times and nothing else' >&2; exit 1;
            }
            [ "$(grep -c '^sleep$' "$MOCK_CALLS")" = 2 ]
            [ "$(grep -c 'Gatekeeper accepted' "$tmp/log")" = 1 ]
            ;;
        duplicate-identity)
            expect_failure 'exactly one valid Developer ID Application' env MOCK_IDENTITY=duplicate bash "$helper" prepare
            ;;
        security-*)
            operation="${selected#security-}"
            case "$operation" in
                create-keychain) expected='Cannot create the temporary signing keychain' ;;
                list-keychains) expected='Cannot select the temporary signing keychain' ;;
                unlock-keychain) expected='Cannot unlock the temporary signing keychain' ;;
                set-keychain-settings) expected='Cannot configure the temporary signing keychain' ;;
                import) expected='Cannot import the Developer ID certificate' ;;
                set-key-partition-list) expected='Cannot authorize codesign access to the signing key' ;;
                find-identity) expected='Cannot inspect signing identities' ;;
                *) echo 'Unknown security test' >&2; exit 1 ;;
            esac
            expect_failure "::error::$expected" env MOCK_SECURITY_FAIL="$operation" bash "$helper" prepare
            grep -F 'rerun Release' "$tmp/log" >/dev/null
            grep -E 'KEYCHAIN_PASSWORD|APPLE_CERTIFICATE|runner user keychain settings' "$tmp/log" >/dev/null
            ;;
        *) echo 'Unknown follow-up test' >&2; exit 1 ;;
    esac
    if grep -E 'fixture-(private-key|certificate-password|keychain-password|api-key-id|api-issuer-id)|ZmFrZS1jZXJ0aWZpY2F0ZQ==' "$tmp/log" >/dev/null; then
        echo 'FAIL: follow-up diagnostic exposed fixture credentials' >&2
        exit 1
    fi
    echo "macOS signing follow-up case: $selected OK"
}

if [ -n "${MACOS_SIGNING_CASE:-}" ]; then
    followup_case "$MACOS_SIGNING_CASE"
    exit 0
fi
for selected in prepare-missing-team prepare-invalid-team prepare-lowercase-team verify-lowercase-team mismatch-remedy gatekeeper-detail missing-team invalid-team wrong-team designated-team gatekeeper-rejected gatekeeper-source gatekeeper-exit gatekeeper-retry duplicate-identity security-create-keychain security-list-keychains security-unlock-keychain security-set-keychain-settings security-import security-set-key-partition-list security-find-identity; do
    followup_case "$selected"
done

submission_id=01234567-89ab-cdef-0123-456789abcdef
for status in Invalid InProgress; do
    expect_failure 'Notarization was not Accepted' env MOCK_NOTARY="$status" bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
    grep -F "id=$submission_id; status=$status" "$tmp/log" >/dev/null
    grep -F "xcrun notarytool log $submission_id" "$tmp/log" >/dev/null
    if grep -F 'fixture-result-detail-not-requested' "$tmp/log" >/dev/null; then
        echo 'FAIL: diagnostic printed fields outside the public id/status receipt' >&2
        exit 1
    fi
done
expect_failure 'Notarization was not Accepted' env MOCK_NOTARY=malformed bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
grep -F 'id=unavailable; status=invalid JSON' "$tmp/log" >/dev/null
grep -F 'No usable submission id; inspect xcrun notarytool history' "$tmp/log" >/dev/null
expect_failure 'Notarization submission failed' env MOCK_SUBMIT_FAIL=1 MOCK_NOTARY=Invalid bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
grep -F 'HTTP 401: fixture notarization authentication failed' "$tmp/log" >/dev/null
grep -F "id=$submission_id; status=Invalid" "$tmp/log" >/dev/null
expect_failure 'Notarization was not Accepted' env MOCK_NOTARY=Invalid MOCK_NOTARY_ID=$'bad\n::error::injected' bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
grep -F 'id=unavailable; status=Invalid' "$tmp/log" >/dev/null
if grep -F 'injected' "$tmp/log" >/dev/null; then
    echo 'FAIL: unbounded submission id reached the diagnostic' >&2
    exit 1
fi
expect_failure 'Notarization was not Accepted' env MOCK_NOTARY=$'Invalid\n::error::injected' bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
grep -F "id=$submission_id; status=unavailable" "$tmp/log" >/dev/null
if grep -F 'injected' "$tmp/log" >/dev/null; then
    echo 'FAIL: unbounded notarization status reached the diagnostic' >&2
    exit 1
fi
expect_failure 'Code signature verification failed' env MOCK_VERIFY=invalid bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
expect_failure 'not signed by Developer ID Application' env MOCK_AUTHORITY=absent bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
expect_failure 'lacks hardened runtime' env MOCK_RUNTIME=absent bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
expect_failure 'lacks a secure timestamp' env MOCK_TIMESTAMP=absent bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
bash "$helper" notarize "$tmp/signed.tar.gz" "$work" >"$tmp/accepted.log"
grep -F 'Apple notarization status: Accepted' "$tmp/accepted.log" >/dev/null
[ ! -e "$work/notary-auth.p8" ]
encoded_key="$(printf '%s' "$APPLE_API_KEY" | base64)"
env APPLE_API_KEY="$encoded_key" bash "$helper" notarize "$tmp/signed.tar.gz" "$work" >/dev/null
expect_failure 'APPLE_API_KEY must be PEM or base64 PEM' env APPLE_API_KEY=invalid bash "$helper" notarize "$tmp/signed.tar.gz" "$work"

expect_failure 'invalid or missing executable members' bash "$helper" verify-archive "$tmp/legacy-only.tar.gz"
: >"$MOCK_LOG"
bash "$helper" verify-archive "$tmp/signed.tar.gz" >/dev/null
grep -Fx -- "--verify:ahwp" "$MOCK_LOG" >/dev/null
grep -Fx -- "-dv:ahwp" "$MOCK_LOG" >/dev/null
if grep -F ':hwp' "$MOCK_LOG" >/dev/null; then
    echo 'FAIL: verify-archive still checks the retired hwp name' >&2
    exit 1
fi
bash "$helper" cleanup "$work"
[ ! -e "$work" ]
mkdir -p "$RUNNER_TEMP/ahwp-signing.unowned"
expect_failure 'ownership marker' bash "$helper" cleanup "$RUNNER_TEMP/ahwp-signing.unowned"
[ -d "$RUNNER_TEMP/ahwp-signing.unowned" ]
expect_failure 'outside the helper-owned' bash "$helper" cleanup "$tmp"

: >"$MOCK_LOG"
: >"$MOCK_CALLS"
bash "$helper" --dry-run >"$tmp/dry-run.log"
grep -F 'no signing, notarization, upload, or Gatekeeper acceptance was performed' "$tmp/dry-run.log" >/dev/null
[ ! -s "$MOCK_LOG" ]
[ ! -s "$MOCK_CALLS" ]
python3 - "${MACOS_SIGNING_WORKFLOW:-$root/.github/workflows/release.yml}" <<'PYTEST'
import os
import pathlib
import re
import sys
workflow = pathlib.Path(sys.argv[1]).read_text()
jobs = dict(re.findall(r"(?ms)^  ([a-z][a-z0-9-]*):\n(.*?)(?=^  [a-z][a-z0-9-]*:\n|\Z)", workflow.split("\njobs:\n", 1)[1]))
upload = jobs["upload-assets"]
steps = dict(re.findall(r"(?ms)^      - name: ([^\n]+)\n(.*?)(?=^      - |\Z)", upload))
team_blocks = {
    "prepare": steps["Prepare macOS Developer ID signing"],
    "notarize": steps["Notarize the signed macOS executable"],
    "verify": jobs["verify-macos-assets"],
}
selected = os.environ.get("MACOS_SIGNING_WORKFLOW_CASE")
for name, block in team_blocks.items():
    if selected and selected != name:
        continue
    assert "APPLE_TEAM_ID: ${{ vars.APPLE_TEAM_ID }}" in block, f"{name}: signer team must use the repository APPLE_TEAM_ID variable"
    assert "secrets.APPLE_TEAM_ID" not in block, f"{name}: signer team must not use a secret"
assert "taiki-e/upload-rust-binary-action@f0d45ae91ee7b8ee928de7a9d04d893a08bcbec6" in upload
assert "codesign: ${{ steps.macos-signing.outputs.identity }}" in upload
assert "codesign-options: runtime" in upload
assert upload.index("macos-release-signing.sh prepare") < upload.index("taiki-e/upload-rust-binary-action@")
for name in ("APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "KEYCHAIN_PASSWORD", "APPLE_API_KEY", "APPLE_API_KEY_ID", "APPLE_API_ISSUER_ID"):
    assert "secrets." + name in upload
assert "always() && runner.os == 'macOS'" in upload
assert "needs: upload-assets" in jobs["verify-macos-assets"]
assert "alias-assets" not in jobs
assert "gh release download" in jobs["verify-macos-assets"]
assert "macos-release-signing.sh verify-archive" in jobs["verify-macos-assets"]
assert "needs: [skill-bundle, verify-macos-assets]" in jobs["attest"]
assert "needs: [skill-bundle, attest, verify-macos-assets]" in jobs["publish-release"]
PYTEST
echo 'macOS release signing mock checks: OK (no credentials or native services used)'
