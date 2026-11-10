#!/usr/bin/env bash
# Isolated installer bridge regressions (#510). No real HOME, downloads or installed binaries.
# Every download is served by the curl stub from an owner-authored local archive.
# Run with: bash scripts/tests/install-bridge.sh
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
installer="$repo_root/scripts/install.sh"
self="$script_dir/$(basename "$0")"

fail() { printf '  FAIL %s\n' "$*" >&2; exit 1; }

run_case() {
    local case_name="$1"
    local scratch
    scratch="$(mktemp -d)"
    case_scratch="$scratch"
    trap 'rm -rf "$case_scratch"' EXIT
    local fake_bin="$scratch/bin"
    local fake_home="$scratch/fake home"
    local destination="$scratch/install with spaces"
    local archive_dir="$scratch/archives"
    local canonical="$scratch/canonical"
    local legacy="$scratch/legacy"
    local tag=v1.6.0
    local target=x86_64-unknown-linux-gnu
    local scenario=canonical
    local mode=flags
    local expected_failure=0
    case "$case_name" in
        canonical-flags) ;;
        canonical-env) mode=env ;;
        latest-redirect) mode=latest ;;
        legacy-only) scenario=legacy; tag=v1.5.0 ;;
        canonical-transport) scenario=transport; expected_failure=1 ;;
        canonical-http-500) scenario=http500; expected_failure=1 ;;
        checksum-transport) scenario=checksum_transport; expected_failure=1 ;;
        checksum-missing) scenario=checksum_missing; expected_failure=1 ;;
        checksum-mismatch) scenario=checksum_mismatch; expected_failure=1 ;;
        *) fail "unknown case: $case_name" ;;
    esac
    mkdir -p "$fake_bin" "$fake_home" "$destination" "$archive_dir" "$canonical" "$legacy" "$scratch/tmp"
    local real_cp real_mv
    real_cp="$(command -v cp)"
    real_mv="$(command -v mv)"

    # Tiny fixture executables only print their own identity. They never invoke an updater.
    printf '%s\n' '#!/bin/sh' "printf '%s\\n' 'ahwp fixture $tag'" >"$canonical/ahwp"
    printf '%s\n' '#!/bin/sh' "printf '%s\\n' 'hwp legacy fixture $tag'" >"$legacy/hwp"
    chmod 755 "$canonical/ahwp" "$legacy/hwp"
    ln -s ahwp "$canonical/hwp"
    local canonical_base="ahwp-$tag-$target"
    local legacy_base="hwp-$tag-$target"
    tar -czf "$archive_dir/$canonical_base.tar.gz" -C "$canonical" ahwp hwp
    tar -czf "$archive_dir/$legacy_base.tar.gz" -C "$legacy" hwp
    local base digest
    for base in "$canonical_base" "$legacy_base"; do
        if command -v sha256sum >/dev/null 2>&1; then
            digest="$(sha256sum "$archive_dir/$base.tar.gz" | awk '{print $1}')"
        else
            digest="$(shasum -a 256 "$archive_dir/$base.tar.gz" | awk '{print $1}')"
        fi
        printf '%s  %s.tar.gz\n' "$digest" "$base" >"$archive_dir/$base.sha256"
    done
    printf '%064d\n' 0 >"$scratch/bad.sha256"
    printf '%s\n' '#!/bin/sh' "printf '%s\\n' 'old ahwp'" >"$scratch/old-ahwp"
    printf '%s\n' '#!/bin/sh' "printf '%s\\n' 'old hwp'" >"$scratch/old-hwp"
    "$real_cp" "$scratch/old-ahwp" "$destination/ahwp"
    "$real_cp" "$scratch/old-hwp" "$destination/hwp"
    chmod 755 "$destination/ahwp" "$destination/hwp"
    : >"$scratch/downloads.log"
    : >"$scratch/publish.log"

    cat >"$fake_bin/uname" <<'STUB'
#!/bin/sh
case "$1" in
    -s) printf '%s\n' Linux ;;
    -m) printf '%s\n' x86_64 ;;
    *) exit 99 ;;
esac
STUB
    cat >"$fake_bin/curl" <<'STUB'
#!/bin/sh
set -eu
output=
write_out=
url=
fail_http=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o|--output) output="$2"; shift 2 ;;
        -w|--write-out) write_out="$2"; shift 2 ;;
        --proto|--tls-max|--connect-timeout|--max-time|--retry|--retry-max-time|--retry-delay)
            shift 2 ;;
        -f|--fail|--fail-with-body) fail_http=1; shift ;;
        -[!-]*) case "$1" in *f*) fail_http=1 ;; esac; shift ;;
        --*) shift ;;
        https://*) url="$1"; shift ;;
        *) printf '%s\n' 'curl fixture rejected an unknown argument' >&2; exit 99 ;;
    esac
done
[ -n "$url" ] || exit 99
case "$url" in
    "https://github.com/STAIxBWLB/hwp-cli/releases/latest")
        name=latest
        effective="https://github.com/STAIxBWLB/hwp-cli/releases/tag/$FIXTURE_TAG"
        ;;
    "https://github.com/STAIxBWLB/hwp-cli/releases/download/$FIXTURE_TAG/"*)
        name="${url##*/}"
        effective="$url"
        ;;
    *) printf '%s\n' 'curl fixture refused an unexpected URL; no network was used' >&2; exit 99 ;;
esac
printf '%s\n' "$name" >>"$FIXTURE_DOWNLOAD_LOG"
status=200
transport=0
source_file="$FIXTURE_ARCHIVES/$name"
case "$name" in
    latest) source_file=/dev/null ;;
    ahwp-*.tar.gz)
        case "$FIXTURE_SCENARIO" in
            legacy) status=404 ;;
            transport) status=000; transport=28 ;;
            http500) status=500 ;;
        esac
        ;;
    ahwp-*.sha256)
        case "$FIXTURE_SCENARIO" in
            checksum_transport) status=000; transport=28 ;;
            checksum_missing) status=404 ;;
            checksum_mismatch) source_file="$FIXTURE_BAD_CHECKSUM" ;;
        esac
        ;;
    hwp-*.tar.gz|hwp-*.sha256)
        # Valid legacy assets expose accidental fallback after a canonical failure.
        # Only the canonical-success cases deliberately omit those legacy assets.
        [ "$FIXTURE_SCENARIO" != canonical ] || status=404
        ;;
    *) status=404 ;;
esac
if [ "$transport" -eq 0 ] && [ "$status" = 200 ]; then
    [ -f "$source_file" ] || [ "$source_file" = /dev/null ] || exit 99
    if [ "$output" = /dev/null ]; then
        :
    elif [ -n "$output" ]; then
        "$FIXTURE_REAL_CP" "$source_file" "$output"
    else
        cat "$source_file"
    fi
elif [ -n "$output" ] && [ "$transport" -eq 0 ]; then
    printf '%s\n' 'fixture HTTP failure' >"$output"
fi
if [ -n "$write_out" ]; then
    printf '%s' "$write_out" |
        sed "s@%{http_code}@$status@g; s@%{url_effective}@$effective@g"
fi
[ "$transport" -eq 0 ] || exit "$transport"
if [ "$status" != 200 ] && [ "$fail_http" -eq 1 ]; then
    exit 22
fi
exit 0
STUB
    cat >"$fake_bin/cp" <<'STUB'
#!/bin/sh
set -eu
last=
for last do :; done
case "$last" in
    "$FIXTURE_DESTINATION/ahwp"|"$FIXTURE_DESTINATION/hwp")
        printf '%s\n' 'fixture rejected a non-atomic copy to a final executable' >&2
        exit 97
        ;;
esac
exec "$FIXTURE_REAL_CP" "$@"
STUB
    cat >"$fake_bin/mv" <<'STUB'
#!/bin/sh
set -eu
source_file=
destination=
for argument do
    case "$argument" in -*) continue ;; esac
    source_file="$destination"
    destination="$argument"
done
case "$destination" in
    "$FIXTURE_DESTINATION/ahwp"|"$FIXTURE_DESTINATION/hwp")
        name="${destination##*/}"
        [ "${source_file%/*}" = "$FIXTURE_DESTINATION" ] || {
            printf '%s\n' 'publication must rename a same-directory temporary executable' >&2
            exit 97
        }
        [ -f "$source_file" ] && [ -x "$source_file" ] || exit 97
        cmp -s "$destination" "$FIXTURE_OLD_FILES/old-$name" || {
            printf '%s\n' 'the old executable changed before its atomic rename' >&2
            exit 97
        }
        printf '%s\n' "$name" >>"$FIXTURE_PUBLISH_LOG"
        ;;
esac
exec "$FIXTURE_REAL_MV" "$@"
STUB
    chmod 755 "$fake_bin"/*

    local env_tag="$tag"
    local env_dir="$destination"
    case "$mode" in
        flags) env_tag=v9.9.9; env_dir="$scratch/unused-env-dir"; set -- --tag "$tag" --dir "$destination" ;;
        env) set -- ;;
        latest) env_tag=; set -- --dir "$destination" ;;
    esac
    local status
    set +e
    env HOME="$fake_home" TMPDIR="$scratch/tmp" ENV=/dev/null BASH_ENV=/dev/null \
        PATH="$fake_bin:$PATH" HWP_TAG="$env_tag" HWP_INSTALL_DIR="$env_dir" \
        FIXTURE_TAG="$tag" FIXTURE_SCENARIO="$scenario" FIXTURE_ARCHIVES="$archive_dir" \
        FIXTURE_DOWNLOAD_LOG="$scratch/downloads.log" FIXTURE_BAD_CHECKSUM="$scratch/bad.sha256" \
        FIXTURE_REAL_CP="$real_cp" FIXTURE_REAL_MV="$real_mv" \
        FIXTURE_DESTINATION="$destination" FIXTURE_OLD_FILES="$scratch" \
        FIXTURE_PUBLISH_LOG="$scratch/publish.log" \
        /bin/sh "$installer" "$@" >"$scratch/stdout" 2>"$scratch/stderr"
    status=$?
    set -e

    if [ "$expected_failure" -eq 1 ]; then
        [ "$status" -ne 0 ] || fail "$case_name: canonical failure must stop the install"
        [ -s "$scratch/stderr" ] || fail "$case_name: blocked install needs a diagnostic"
        if grep -E '^hwp-' "$scratch/downloads.log" >/dev/null; then
            fail "$case_name: a transport/HTTP/checksum failure must not fall back to legacy"
        fi
        cmp -s "$scratch/old-ahwp" "$destination/ahwp" || fail "$case_name: ahwp changed on download failure"
        cmp -s "$scratch/old-hwp" "$destination/hwp" || fail "$case_name: hwp changed on download failure"
        [ ! -s "$scratch/publish.log" ] || fail "$case_name: publication began after a failed download"
    else
        if [ "$status" -ne 0 ]; then
            cat "$scratch/stderr" >&2
            fail "$case_name: isolated installer should succeed"
        fi
        local expected="$canonical/ahwp"
        [ "$scenario" != legacy ] || expected="$legacy/hwp"
        for name in ahwp hwp; do
            [ -x "$destination/$name" ] || fail "$case_name: missing executable $name"
            cmp -s "$expected" "$destination/$name" || fail "$case_name: wrong installed payload for $name"
            [ "$(grep -c -F -x "$name" "$scratch/publish.log")" = 1 ] ||
                fail "$case_name: $name was not published once through an atomic rename"
        done
        if [ "$scenario" = legacy ]; then
            grep -F -x "$legacy_base.tar.gz" "$scratch/downloads.log" >/dev/null ||
                fail "$case_name: absent canonical asset must try the legacy archive"
        elif grep -E '^hwp-' "$scratch/downloads.log" >/dev/null; then
            fail "$case_name: canonical success must not fetch legacy assets"
        fi
    fi
    local first
    first="$(sed -n '1p' "$scratch/downloads.log")"
    if [ "$mode" = latest ]; then
        [ "$first" = latest ] || fail "$case_name: latest redirect was not resolved"
        grep -F -x "$canonical_base.tar.gz" "$scratch/downloads.log" >/dev/null ||
            fail "$case_name: latest release must select the canonical archive"
    else
        [ "$first" = "$canonical_base.tar.gz" ] || fail "$case_name: canonical archive was not tried first"
    fi
    [ ! -e "$fake_home/.local/bin" ] || fail "$case_name: explicit temp destination must not install into HOME"
    [ ! -e "$scratch/unused-env-dir" ] || fail "$case_name: --dir must override HWP_INSTALL_DIR"
}

if [ "${1:-}" = --case ]; then
    run_case "$2"
    exit 0
fi

failures=0
for case_name in canonical-flags canonical-env latest-redirect legacy-only \
    canonical-transport canonical-http-500 checksum-transport checksum-missing checksum-mismatch; do
    # A subprocess keeps errexit active inside each fixture and cleanup independent across cases.
    if env BASH_ENV=/dev/null ENV=/dev/null bash "$self" --case "$case_name"; then
        printf '  ok   %s\n' "$case_name"
    else
        printf '  FAIL %s\n' "$case_name" >&2
        failures=$((failures + 1))
    fi
done
[ "$failures" -eq 0 ] || exit 1
printf '%s\n' '== installer bridge: OK (canonical/legacy/fail-closed/atomic-per-file, isolated) =='
