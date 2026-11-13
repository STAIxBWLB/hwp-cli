#!/usr/bin/env bash
# Exercise the release helper with fake credentials and fake macOS tools only.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
helper="$root/scripts/macos-release-signing.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/runner" "$tmp/home" "$tmp/tmp"
export HOME="$tmp/home" TMPDIR="$tmp/tmp"
export RUNNER_TEMP="$tmp/runner" GITHUB_OUTPUT="$tmp/output" MOCK_LOG="$tmp/codesign.log" MOCK_CALLS="$tmp/native-calls.log"
export APPLE_CERTIFICATE=ZmFrZS1jZXJ0aWZpY2F0ZQ==
export APPLE_CERTIFICATE_PASSWORD=fixture-certificate-password KEYCHAIN_PASSWORD=fixture-keychain-password
export APPLE_API_KEY=$'-----BEGIN PRIVATE KEY-----\nfixture-private-key\n-----END PRIVATE KEY-----\n'
export APPLE_API_KEY_ID=fixture-api-key-id APPLE_API_ISSUER_ID=fixture-api-issuer-id

cat >"$tmp/bin/uname" <<'SH'
#!/usr/bin/env bash
echo uname >>"$MOCK_CALLS"
echo Darwin
SH
cat >"$tmp/bin/security" <<'SH'
#!/usr/bin/env bash
echo security >>"$MOCK_CALLS"
case "$1" in
create-keychain) touch "${!#}" ;;
find-identity)
    if [ "${MOCK_IDENTITY:-valid}" = valid ]; then
        echo '  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Developer ID Application: Fixture"'
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
--verify) [ "${MOCK_VERIFY:-valid}" = valid ] ;;
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
if [ "${MOCK_NOTARY:-Accepted}" = malformed ]; then
    echo not-json
else
    printf '{"status":"%s"}\n' "${MOCK_NOTARY:-Accepted}"
fi
SH
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"

expect_failure() {
    local message="$1"; shift
    if "$@" >"$tmp/log" 2>&1; then
        echo "FAIL: $message unexpectedly passed" >&2
        exit 1
    fi
    grep -F "$message" "$tmp/log" >/dev/null || { cat "$tmp/log"; exit 1; }
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
for altered in (False, True):
    with tarfile.open(root / ("altered.tar.gz" if altered else "signed.tar.gz"), "w:gz") as archive:
        for name, data in (("ahwp", b"signed executable\n"), ("hwp", b"altered\n" if altered else b"signed executable\n")):
            entry = tarfile.TarInfo(name)
            if name == "hwp" and not altered:
                entry.type = tarfile.LNKTYPE
                entry.linkname = "ahwp"
                archive.addfile(entry)
            else:
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
PY

for status in Invalid malformed InProgress; do
    expect_failure 'Notarization was not Accepted' env MOCK_NOTARY="$status" bash "$helper" notarize "$tmp/signed.tar.gz" "$work"
done
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

expect_failure 'alias differs from ahwp' bash "$helper" verify-archive "$tmp/altered.tar.gz"
: >"$MOCK_LOG"
bash "$helper" verify-archive "$tmp/signed.tar.gz" >/dev/null
for name in ahwp hwp; do
    grep -Fx -- "--verify:$name" "$MOCK_LOG" >/dev/null
    grep -Fx -- "-dv:$name" "$MOCK_LOG" >/dev/null
done
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
python3 - "$root/.github/workflows/release.yml" <<'PYTEST'
import pathlib
import re
import sys
workflow = pathlib.Path(sys.argv[1]).read_text()
jobs = dict(re.findall(r"(?ms)^  ([a-z][a-z0-9-]*):\n(.*?)(?=^  [a-z][a-z0-9-]*:\n|\Z)", workflow.split("\njobs:\n", 1)[1]))
upload = jobs["upload-assets"]
assert "taiki-e/upload-rust-binary-action@f0d45ae91ee7b8ee928de7a9d04d893a08bcbec6" in upload
assert "codesign: ${{ steps.macos-signing.outputs.identity }}" in upload
assert "codesign-options: runtime" in upload
assert upload.index("macos-release-signing.sh prepare") < upload.index("taiki-e/upload-rust-binary-action@")
for name in ("APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "KEYCHAIN_PASSWORD", "APPLE_API_KEY", "APPLE_API_KEY_ID", "APPLE_API_ISSUER_ID"):
    assert "secrets." + name in upload
assert "always() && runner.os == 'macOS'" in upload
assert "needs: alias-assets" in jobs["verify-macos-assets"]
assert "gh release download" in jobs["verify-macos-assets"]
assert "macos-release-signing.sh verify-archive" in jobs["verify-macos-assets"]
assert "needs: [skill-bundle, verify-macos-assets]" in jobs["attest"]
assert "needs: [skill-bundle, attest, verify-macos-assets]" in jobs["publish-release"]
PYTEST
echo 'macOS release signing mock checks: OK (no credentials or native services used)'
