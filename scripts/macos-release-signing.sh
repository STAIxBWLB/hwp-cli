#!/usr/bin/env bash
# macOS release signing helpers. Private material stays in restricted runner-temporary files.
set -euo pipefail

fail() { echo "::error::$*" >&2; exit 1; }

if [ "${1:-}" = --dry-run ]; then
    # This path neither reads credentials nor runs security, codesign, or notarytool.
    bash -n "$0"
    echo 'macOS signing dry-run: require six Actions secrets; import Developer ID Application into a temporary keychain'
    echo 'macOS signing dry-run: require APPLE_TEAM_ID repository variable; pin Developer ID signer team, runtime and Timestamp'
    echo 'macOS signing dry-run: ZIP signed ahwp; notarytool submit --wait --output-format json; require Accepted'
    echo 'macOS signing dry-run: compare ahwp/hwp bytes; verify both signatures and Gatekeeper install acceptance (3 attempts, 10 seconds apart); clean temporary keychain'
    echo 'macOS signing dry-run: no signing, notarization, upload, or Gatekeeper acceptance was performed'
    exit 0
fi

require_secrets() {
    local name
    for name in "$@"; do
        if [ -z "${!name:-}" ]; then
            fail "$name is missing. Set $name in repository Settings > Secrets and variables > Actions, then rerun Release."
        fi
    done
}

require_team() {
    [ -n "${APPLE_TEAM_ID:-}" ] || fail 'APPLE_TEAM_ID is missing. Set APPLE_TEAM_ID in repository Settings > Secrets and variables > Actions > Variables, then rerun Release.'
    [[ "$APPLE_TEAM_ID" =~ ^[A-Za-z0-9]{10}$ ]] || fail 'APPLE_TEAM_ID must contain exactly 10 ASCII letters or digits. Correct it in repository Settings > Secrets and variables > Actions > Variables, then rerun Release.'
}

require_macos() {
    [ "$(uname -s)" = Darwin ] || fail 'This operation requires a macOS runner. Use --dry-run for a credential-free readiness check.'
}

owned_directory() {
    case "$1" in
        "${RUNNER_TEMP:?}/ahwp-signing."*) ;;
        *) fail 'Refusing a signing directory outside the helper-owned runner temporary prefix.' ;;
    esac
    [ ! -L "$1" ] && [ -f "$1/.ahwp-signing" ] || fail 'The signing directory has no helper ownership marker. Check the prepare step output.'
}

write_notary_key() {
    local key_file="$1" material="$APPLE_API_KEY"
    case "$material" in
        '-----BEGIN PRIVATE KEY-----'*) ;;
        *)
            if ! material=$(printf '%s' "$material" | tr -d '[:space:]' | base64 --decode 2>/dev/null && printf '.'); then
                fail 'APPLE_API_KEY must be PEM or base64 PEM from the App Store Connect .p8. Correct it in repository Actions secrets.'
            fi
            material="${material%.}"
            ;;
    esac
    case "$material" in
        '-----BEGIN PRIVATE KEY-----'*) ;;
        *) fail 'APPLE_API_KEY must be PEM or base64 PEM from the App Store Connect .p8. Correct it in repository Actions secrets.' ;;
    esac
    (umask 077; printf '%s' "$material" >"$key_file")
}

verify_binary() {
    local binary="$1" details
    require_team
    codesign --verify --strict --verbose=2 -R "anchor apple generic and certificate leaf[subject.OU] = \"$APPLE_TEAM_ID\"" "$binary" || fail 'Code signature verification failed. Rebuild and sign the macOS asset with the configured Developer ID Application certificate.'
    if ! details="$(codesign -dv --verbose=4 "$binary" 2>&1)"; then
        fail 'Cannot inspect the code signature. Rebuild and sign the macOS asset before uploading it.'
    fi
    printf '%s\n' "$details" | grep -E '^Authority=Developer ID Application:' >/dev/null ||
        fail 'The asset is not signed by Developer ID Application. Check APPLE_CERTIFICATE in repository Actions secrets; ad-hoc signing is forbidden.'
    printf '%s\n' "$details" | grep -E '^CodeDirectory .*flags=.*[(]runtime[)]' >/dev/null ||
        fail 'The asset lacks hardened runtime. Set the upload action codesign-options input to runtime and rebuild.'
    if ! printf '%s\n' "$details" | grep -E '^Timestamp=.' >/dev/null ||
        printf '%s\n' "$details" | grep -E '^Timestamp=(none|None|Not set)' >/dev/null; then
        fail 'The asset lacks a secure timestamp. Check access to the Apple timestamp service and rebuild the Developer ID-signed asset.'
    fi
    echo 'Developer ID Application authority, hardened runtime, and secure timestamp verified.'
}

verify_gatekeeper() {
    local binary="$1" assessment attempt
    for attempt in 1 2 3; do
        if assessment="$(spctl --assess --type install --verbose=2 "$binary" 2>&1)" &&
            printf '%s\n' "$assessment" | grep -E '(^|: )accepted$' >/dev/null &&
            printf '%s\n' "$assessment" | grep -Fx 'source=Notarized Developer ID' >/dev/null; then
            echo 'Gatekeeper accepted the executable with source=Notarized Developer ID.'
            return
        fi
        if [ "$attempt" -lt 3 ]; then sleep 10; fi
    done
    fail 'Gatekeeper install assessment did not accept a Notarized Developer ID asset after 3 attempts, 10 seconds apart (20 seconds of retry delay). Ticket propagation may be delayed; check notarization and network access, then rerun Release.'
}

extract_binaries() {
    python3 - "$1" "$2" "$3" <<'PY'
import pathlib
import shutil
import sys
import tarfile

archive, destination, names = sys.argv[1], pathlib.Path(sys.argv[2]), sys.argv[3].split(",")
destination.mkdir(parents=True, exist_ok=True)
try:
    with tarfile.open(archive, "r:*") as source:
        for name in names:
            matches = [member for member in source.getmembers() if member.name == name]
            if len(matches) != 1:
                raise ValueError("missing or duplicate executable")
            member = matches[0]
            if not member.isfile() and not (name == "hwp" and member.islnk() and member.linkname == "ahwp"):
                raise ValueError("executable is not a regular file or the expected alias")
            with source.extractfile(member) as input_file, (destination / name).open("wb") as output_file:
                shutil.copyfileobj(input_file, output_file)
            (destination / name).chmod(0o755)
except (OSError, tarfile.TarError, ValueError, KeyError):
    sys.exit("::error::The macOS archive has invalid or missing executable members. Rebuild and republish the release assets.")
PY
}

case "${1:-}" in
    prepare)
        require_secrets APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD KEYCHAIN_PASSWORD APPLE_API_KEY APPLE_API_KEY_ID APPLE_API_ISSUER_ID
        require_macos
        : "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required on the release runner}"
        work="$(mktemp -d "${RUNNER_TEMP:?}/ahwp-signing.XXXXXX")"
        chmod 700 "$work"
        touch "$work/.ahwp-signing"
        printf 'directory=%s\n' "$work" >>"$GITHUB_OUTPUT"
        keychain="$work/signing.keychain-db"
        (umask 077; printf '%s' "$APPLE_CERTIFICATE" | base64 --decode >"$work/certificate.p12") ||
            fail 'APPLE_CERTIFICATE is not valid base64. Set it from the Developer ID Application .p12 in repository Actions secrets.'
        security create-keychain -p "$KEYCHAIN_PASSWORD" "$keychain" ||
            fail 'Cannot create the temporary signing keychain. Check KEYCHAIN_PASSWORD in repository Actions secrets and runner keychain access, then rerun Release.'
        security list-keychains -d user -s "$keychain" ||
            fail 'Cannot select the temporary signing keychain. Check the macOS runner user keychain settings, then rerun Release.'
        security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$keychain" ||
            fail 'Cannot unlock the temporary signing keychain. Correct KEYCHAIN_PASSWORD in repository Actions secrets, then rerun Release.'
        security set-keychain-settings -t 21600 -u "$keychain" ||
            fail 'Cannot configure the temporary signing keychain. Check the macOS runner user keychain settings, then rerun Release.'
        security import "$work/certificate.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign >/dev/null ||
            fail 'Cannot import the Developer ID certificate. Correct APPLE_CERTIFICATE and APPLE_CERTIFICATE_PASSWORD in repository Actions secrets, then rerun Release.'
        security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$keychain" >/dev/null ||
            fail 'Cannot authorize codesign access to the signing key. Correct KEYCHAIN_PASSWORD in repository Actions secrets and check the imported APPLE_CERTIFICATE, then rerun Release.'
        rm -f "$work/certificate.p12"
        identity="$(security find-identity -v -p codesigning "$keychain" | awk '/"Developer ID Application:/ {print $2}')" ||
            fail 'Cannot inspect signing identities. Check APPLE_CERTIFICATE in repository Actions secrets and the macOS runner keychain settings, then rerun Release.'
        [[ "$identity" =~ ^[[:xdigit:]]{40}$ ]] ||
            fail 'APPLE_CERTIFICATE must contain exactly one valid Developer ID Application identity. Correct the repository Actions secret and rerun Release.'
        printf 'identity=%s\n' "$identity" >>"$GITHUB_OUTPUT"
        ;;
    notarize)
        require_secrets APPLE_API_KEY APPLE_API_KEY_ID APPLE_API_ISSUER_ID
        require_macos
        archive="${2:?archive is required}"; work="${3:?signing directory is required}"
        owned_directory "$work"
        extract_binaries "$archive" "$work/notary" ahwp
        verify_binary "$work/notary/ahwp"
        key_file="$work/notary-auth.p8"
        trap 'rm -f "$key_file"' EXIT
        write_notary_key "$key_file"
        python3 - "$work/notary/ahwp" "$work/notary.zip" <<'PYZIP'
import sys
import zipfile
with zipfile.ZipFile(sys.argv[2], "w", compression=zipfile.ZIP_DEFLATED) as archive:
    archive.write(sys.argv[1], "ahwp")
PYZIP
        notary_submit_failed=false
        if ! xcrun notarytool submit "$work/notary.zip" --key "$key_file" \
            --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" \
            --wait --output-format json >"$work/notary-result.json" 2>"$work/notary-error.log"; then
            cat "$work/notary-error.log" >&2
            notary_submit_failed=true
        fi
        notary_result_accepted=false
        if python3 - "$work/notary-result.json" <<'PY'
import json
import re
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        result = json.load(handle)
except (OSError, ValueError):
    result = None

if isinstance(result, dict) and result.get("status") == "Accepted":
    sys.exit(0)

def bounded(value, pattern):
    return value if isinstance(value, str) and re.fullmatch(pattern, value) else "unavailable"

submission_id = bounded(result.get("id"), r"[A-Za-z0-9-]{1,128}") if isinstance(result, dict) else "unavailable"
status = bounded(result.get("status"), r"[A-Za-z][A-Za-z ]{0,63}") if isinstance(result, dict) else "invalid JSON"
print(f"Notarization result: id={submission_id}; status={status}", file=sys.stderr)
if submission_id != "unavailable":
    print(f"Inspect with xcrun notarytool log {submission_id} using the configured API credentials.", file=sys.stderr)
else:
    print("No usable submission id; inspect xcrun notarytool history using the configured API credentials.", file=sys.stderr)
sys.exit(1)
PY
        then
            notary_result_accepted=true
        fi
        if [ "$notary_submit_failed" = true ]; then
            fail 'Notarization submission failed. Check the three APPLE_API_* repository Actions secrets and Apple Notary service access, then rerun Release.'
        fi
        if [ "$notary_result_accepted" != true ]; then
            fail 'Notarization was not Accepted. Inspect the submission with xcrun notarytool history and xcrun notarytool log using the configured API credentials, correct signing or credentials, and rerun Release.'
        fi
        # A bare Mach-O executable cannot be stapled. Gatekeeper retrieves its ticket online.
        echo 'Apple notarization status: Accepted. Bare executable ticket is available to Gatekeeper online.'
        ;;
    verify-archive)
        require_team
        require_macos
        work="$(mktemp -d "${RUNNER_TEMP:?}/ahwp-verify.XXXXXX")"
        trap 'rm -rf "$work"' EXIT
        extract_binaries "${2:?archive is required}" "$work" ahwp,hwp
        cmp "$work/ahwp" "$work/hwp" || fail 'The hwp alias differs from ahwp. Repack the archive with identical signed executable bytes.'
        verify_binary "$work/ahwp"
        verify_binary "$work/hwp"
        verify_gatekeeper "$work/ahwp"
        verify_gatekeeper "$work/hwp"
        ;;
    cleanup)
        work="${2:?signing directory is required}"
        owned_directory "$work"
        if [ -f "$work/signing.keychain-db" ]; then
            security delete-keychain "$work/signing.keychain-db" >/dev/null || fail 'Cannot delete the temporary signing keychain. Inspect runner cleanup before retrying Release.'
        fi
        rm -rf "$work"
        ;;
    *) fail 'Usage: macos-release-signing.sh --dry-run | prepare | notarize ARCHIVE DIRECTORY | verify-archive ARCHIVE | cleanup DIRECTORY' ;;
esac
