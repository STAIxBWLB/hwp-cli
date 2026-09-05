#!/usr/bin/env bash
# scripts/tests/release-readiness-selfcheck.sh
#
# Regression harness for .github/workflows/release-readiness.yml. Every case extracts the step's
# shell out of the workflow file and runs it against a fixture, so what is tested is the workflow
# that ships rather than a second copy of its logic. Nothing here dispatches a run, and nothing
# touches the repository: all fixtures live in a temporary directory.
#
# The regressions it pins, one per finding of the 2026-09-05 review:
#
#   1. The dispatch commit and the evaluated commit are recorded as separate fields, and
#      scripts/check-verification-block.sh refuses a record whose evaluated_sha is not the commit
#      being released.
#   2. A gate script from the evaluated ref cannot touch the evidence: no target step is handed
#      the ledger path, the ledger name is unpredictable, and the assembled record carries only
#      what the controller steps wrote.
#   3. A ref that is neither a tag of this repository nor an ancestor of origin/main is refused,
#      and every step that runs evaluated-ref code is guarded by that refusal.
#   4. readme-pinned-tag: fully qualified tag refs are compared instead of skipped, a tag that is
#      not the evaluated commit fails, and README.ko.md has to carry the pin on its own.
#
# PyYAML reads the workflow. Without it the harness reports SKIP and exits 0: it is a developer
# gate, not a release gate, and the release gate it protects is scripts/check-verification-block.sh.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

WORKFLOW=.github/workflows/release-readiness.yml
ROOT="$PWD"

if ! python3 -c "import yaml" 2>/dev/null; then
    echo "== release-readiness self-check: SKIP (PyYAML is not installed)"
    exit 0
fi

fail=0
check() { # <description> <rc already evaluated by the caller>
    if [ "$2" = "0" ]; then
        printf '  ok   %s\n' "$1"
    else
        printf '  FAIL %s\n' "$1" >&2
        fail=1
    fi
}
same() { # <description> <actual> <expected>
    if [ "$2" = "$3" ]; then
        printf '  ok   %s\n' "$1"
    else
        printf '  FAIL %s\n         actual:   %s\n         expected: %s\n' "$1" "$2" "$3" >&2
        fail=1
    fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
steps="$tmp/steps"
mkdir -p "$steps"

# --- extract every named `run:` step, and the checklist the assembly enforces ------------------
python3 - "$WORKFLOW" "$steps" <<'PY'
import os
import re
import sys

import yaml

workflow, out = sys.argv[1], sys.argv[2]
job = yaml.safe_load(open(workflow, encoding="utf-8"))["jobs"]["readiness"]
for step in job["steps"]:
    name, run = step.get("name"), step.get("run")
    if name and run:
        slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
        with open(os.path.join(out, slug + ".sh"), "w", encoding="utf-8") as handle:
            handle.write(run)
PY
[ -s "$steps/trust-policy-for-the-evaluated-ref.sh" ] ||
    { echo "self-check: the workflow has no trust step to extract" >&2; exit 1; }

echo "== release-readiness self-check"

# --- 1. structure: what the target steps are and are not given ---------------------------------
echo "-- isolation, structural"
structure="$(python3 - "$WORKFLOW" <<'PY'
import re
import sys

import yaml

problems = []
workflow = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
job = workflow["jobs"]["readiness"]
steps = job["steps"]
guard = "steps.trust.outputs.trusted == 'yes'"
if not [s for s in steps if s.get("continue-on-error")]:
    problems.append("no step is marked continue-on-error, so no step is a target step")
emitting = set()
for step in steps:
    name = step.get("name") or step.get("uses")
    env = step.get("env") or {}
    run = step.get("run") or ""
    is_target = bool(step.get("continue-on-error"))
    if is_target:
        leaked = sorted(
            k
            for k, v in env.items()
            if k in ("GATES_FILE", "CHECK_RUNS_FILE", "RECORD_PATH")
            or ".outputs.entry" in str(v)
            or "ledger" in str(v)
        )
        if leaked:
            problems.append(f"target step {name!r} is handed the evidence through {leaked}")
        if guard not in (step.get("if") or ""):
            problems.append(f"target step {name!r} is not guarded by the trust policy")
        if "GITHUB_OUTPUT" in run:
            problems.append(f"target step {name!r} writes a step output")
    if "GITHUB_ENV" in run:
        problems.append(f"step {name!r} writes to $GITHUB_ENV, which every later step receives")
    if "ledger" in str(env):
        problems.append(f"step {name!r} is handed a shared ledger path")
    if "GATES_FILE" in run:
        if 'GATES_FILE="$(mktemp)"' not in run:
            problems.append(f"controller step {name!r} does not create its own ledger file")
        if "entry<<GATE_ENTRY_EOF" not in run:
            problems.append(f"controller step {name!r} does not emit its entries as a step output")
        elif re.search(r"^\s*exit ", run[: run.index("entry<<GATE_ENTRY_EOF")], re.M):
            problems.append(f"controller step {name!r} can exit before it emits its entries")
        elif step.get("id"):
            emitting.add(step["id"])
        else:
            problems.append(f"controller step {name!r} emits an output without an id")
trust_at = next((n for n, s in enumerate(steps) if s.get("id") == "trust"), None)
if trust_at is None:
    problems.append("no step has id: trust")
else:
    first_target = min(n for n, s in enumerate(steps) if s.get("continue-on-error"))
    if trust_at > first_target:
        problems.append("the trust policy runs after a step that executes evaluated-ref code")
assembly = next(s for s in steps if s.get("name") == "Assemble the run record")
wired = {
    v.split("steps.", 1)[1].split(".outputs", 1)[0]
    for k, v in (assembly.get("env") or {}).items()
    if k.startswith("GATE_")
}
unread = sorted(emitting - wired)
if unread:
    problems.append(f"controller steps whose entries the record never reads: {unread}")
if workflow["permissions"] != {"contents": "read", "checks": "read"}:
    problems.append("the workflow permissions are no longer contents: read + checks: read")
if job.get("permissions"):
    problems.append("the job overrides the read-only permissions")
print("\n".join(problems))
PY
)"
[ -z "$structure" ] || printf '%s\n' "$structure" >&2
check "no target step is handed the ledger, and every one is behind the trust policy" \
    "$([ -z "$structure" ] && echo 0 || echo 1)"

# --- 2. the trust policy, executed --------------------------------------------------------------
echo "-- trust policy"
origin="$tmp/origin.git"
work="$tmp/work"
git init -q --bare "$origin"
git init -q -b main "$work"
(
    cd "$work"
    git config user.email selfcheck@example.invalid
    git config user.name selfcheck
    echo one >a
    git add -A
    git commit -qm one
    git checkout -q -b side
    echo side >b
    git add -A
    git commit -qm side
    git checkout -q main
    echo two >a
    git commit -qam two
    git tag v9.9.9 side
    git remote add origin "$origin"
    git push -q origin main
    git push -q origin v9.9.9
) || { echo "self-check: fixture repository setup failed" >&2; exit 1; }
main_sha="$(git -C "$work" rev-parse main)"
side_sha="$(git -C "$work" rev-parse side)"

trust() { # <ref> <sha> -> prints "<rc> <trusted>"
    local out="$tmp/gh_out" rc
    : >"$out"
    rm -f "$tmp/record.json"
    (
        cd "$work" &&
            RUNNER_TEMP="$tmp" \
                READINESS_REF="$1" EVALUATED_SHA="$2" WORKFLOW_SOURCE_SHA="$(printf 'b%.0s' {1..40})" \
                RUN_URL="https://github.com/STAIxBWLB/hwp-cli/actions/runs/424242" \
                RECORD_PATH="$tmp/record.json" GITHUB_OUTPUT="$out" \
                bash "$steps/trust-policy-for-the-evaluated-ref.sh"
    ) >"$tmp/trust.log" 2>&1
    rc=$?
    printf '%s %s' "$rc" "$(sed -n 's/^trusted=//p' "$out")"
}
same "a commit reachable from origin/main is trusted" "$(trust main "$main_sha")" "0 yes"
same "a tag of this repository is trusted" "$(trust v9.9.9 "$side_sha")" "0 yes"
same "a fully qualified tag ref is trusted" "$(trust refs/tags/v9.9.9 "$side_sha")" "0 yes"
git -C "$work" tag -d v9.9.9 >/dev/null
git -C "$work" push -q origin :refs/tags/v9.9.9
same "a ref that is neither a tag nor an ancestor of main is refused" \
    "$(trust side "$side_sha")" "1 no"
python3 - "$tmp/record.json" "$side_sha" <<'PY'
import json
import sys

record = json.load(open(sys.argv[1], encoding="utf-8"))
assert record["result"] == "refused", record["result"]
assert record["schema"] == "hwp-release-readiness-record-v1", record["schema"]
assert record["evaluated_sha"] == sys.argv[2], record["evaluated_sha"]
assert [g["name"] for g in record["gates"]] == ["trust-policy"], record["gates"]
PY
check "the refusal writes one trust-policy outcome and nothing else" "$?"

# --- 3. evidence a target step cannot reach -----------------------------------------------------
echo "-- ledger isolation, executed"
export RUNNER_TEMP="$tmp/runner"
mkdir -p "$RUNNER_TEMP/gates"

# out_value <github-output file> <key>: read one step output, plain or heredoc-delimited.
out_value() {
    python3 - "$1" "$2" <<'PYEOF'
import sys

path, key = sys.argv[1], sys.argv[2]
lines = open(path, encoding="utf-8").read().splitlines()
index = 0
while index < len(lines):
    line = lines[index]
    if line.startswith(key + "<<"):
        delimiter = line.split("<<", 1)[1]
        body = []
        index += 1
        while index < len(lines) and lines[index] != delimiter:
            body.append(lines[index])
            index += 1
        print("\n".join(body))
        raise SystemExit(0)
    if line.startswith(key + "="):
        print(line.split("=", 1)[1])
        raise SystemExit(0)
    index += 1
raise SystemExit(1)
PYEOF
}

# A stand-in for a gate script of the evaluated ref, run with the environment a target step gets:
# it hunts for the evidence and forges entries wherever it can write.
cat >"$tmp/hostile.sh" <<'EOF'
env | grep -iE 'ledger|gates_file|check_runs|record_path|github_output' >"$HOSTILE_OUT/env-hits" || true
for guess in "$RUNNER_TEMP/gates.tsv" "$RUNNER_TEMP/ledger" "$RUNNER_TEMP/gates/ledger" \
    "$PWD/gates.tsv" "$RUNNER_TEMP/release-readiness-record.json"; do
    printf 'fmt\tpass\tforged by the evaluated ref\n' >>"$guess" 2>/dev/null || true
done
# and anything that looks like a ledger, wherever it is
find "$RUNNER_TEMP" /tmp -maxdepth 2 -type f \( -name 'ledger*' -o -name '*gates*' \) -newer "$0" \
    -exec sh -c 'printf "clean-tree\tpass\tforged by the evaluated ref\n" >>"$1" 2>/dev/null' _ {} \; 2>/dev/null
exit 1
EOF
mkdir -p "$tmp/hostile"
(cd "$work" && env -i PATH="$PATH" HOME="$HOME" RUNNER_TEMP="$RUNNER_TEMP" \
    HOSTILE_OUT="$tmp/hostile" bash "$tmp/hostile.sh") >/dev/null 2>&1
check "nothing in a target step's environment names the evidence" \
    "$([ ! -s "$tmp/hostile/env-hits" ] && echo 0 || echo 1)"
check "the simulated gate script did write its forgery somewhere" \
    "$(grep -rqF 'forged by the evaluated ref' "$RUNNER_TEMP" && echo 0 || echo 1)"

# The controller records the gate from the outcome the runner holds, after the hostile step ran:
# a failing target step becomes exactly one `fail` line, emitted as this step's own output.
fmt_out="$tmp/gate-fmt-output"
: >"$fmt_out"
(cd "$work" && RUNNER_TEMP="$RUNNER_TEMP" GITHUB_OUTPUT="$fmt_out" OUTCOME=failure \
    bash "$steps/gate-fmt.sh") >/dev/null 2>&1
fmt_entry="$(out_value "$fmt_out" entry)"
same "the controller records the failing target step once, as fail" \
    "$fmt_entry" "$(printf 'fmt\tfail\tcargo fmt --all --check')"
check "the controller entry carries nothing the simulated gate script wrote" \
    "$(printf '%s' "$fmt_entry" | grep -qF 'forged' && echo 1 || echo 0)"

# --- 4. the record, and the release gate that reads it ------------------------------------------
echo "-- record fields and the release binding"
assembly="$steps/assemble-the-run-record.sh"
record_dir="$tmp/record"
mkdir -p "$record_dir"
cat >"$record_dir/Cargo.toml" <<'EOF'
[workspace.package]
version = "9.9.9"
EOF
# Every checklist entry except fmt, which comes from the controller step run above.
rest="$(python3 - "$assembly" <<'PYEOF'
import ast
import re
import sys

source = open(sys.argv[1], encoding="utf-8").read()
names = ast.literal_eval(re.search(r"CHECKLIST = (\[.*?\])", source, re.S).group(1))
print("\n".join(f"{name}\tpass\trecorded by a controller step" for name in names if name != "fmt"))
PYEOF
)"
evaluated="$(printf 'a%.0s' {1..40})"
dispatched="$(printf 'b%.0s' {1..40})"
released_elsewhere="$(printf 'c%.0s' {1..40})"
run_url="https://github.com/STAIxBWLB/hwp-cli/actions/runs/424242"
(cd "$record_dir" && GATE_FMT="$fmt_entry" GATE_REST="$rest" CHECK_RUNS="" READINESS_REF=v9.9.9 \
    READINESS_SHA="$evaluated" WORKFLOW_SOURCE_SHA="$dispatched" PDFINFO_VERSION="" \
    ORACLE_STATUS="" ORACLE_LOCK_SHA256="" RUN_URL="$run_url" \
    RECORD_PATH="$record_dir/record.json" bash "$assembly") >/dev/null 2>&1
check "the assembly writes a record from the controller entries" \
    "$([ -f "$record_dir/record.json" ] && echo 0 || echo 1)"
python3 - "$record_dir/record.json" "$evaluated" "$dispatched" <<'PYEOF'
import json
import sys

record = json.load(open(sys.argv[1], encoding="utf-8"))
assert record["evaluated_sha"] == sys.argv[2], record["evaluated_sha"]
assert record["workflow_source_sha"] == sys.argv[3], record["workflow_source_sha"]
assert record["evaluated_sha"] != record["workflow_source_sha"]
assert record["result"] == "fail", record["result"]
gates = {gate["name"]: gate for gate in record["gates"]}
assert gates["fmt"]["status"] == "fail", gates["fmt"]
assert not any("forged" in gate["evidence"] for gate in record["gates"])
PYEOF
check "the record keeps the two commits apart and carries no forged gate" "$?"

# The same assembly with every gate passing, which is what the release gate is shown next.
(cd "$record_dir" && GATE_ALL="$(printf '%s\n%s\n' "$(printf 'fmt\tpass\tcargo fmt --all --check')" "$rest")" \
    CHECK_RUNS="" READINESS_REF=v9.9.9 READINESS_SHA="$evaluated" WORKFLOW_SOURCE_SHA="$dispatched" \
    PDFINFO_VERSION="" ORACLE_STATUS="" ORACLE_LOCK_SHA256="" RUN_URL="$run_url" \
    RECORD_PATH="$record_dir/green.json" bash "$assembly") >/dev/null 2>&1
check "a run with every gate passing records result pass" \
    "$(python3 -c "import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))['result'] == 'pass' else 1)" \
        "$record_dir/green.json" && echo 0 || echo 1)"

# The release boundary: same record, two candidate release commits.
changelog="$tmp/CHANGELOG.md"
cat >"$changelog" <<'EOF'
# Changelog

## [9.9.9]

**Added**

- a released thing
EOF
HWP_CHANGELOG="$changelog" bash scripts/release_verification_block.sh 9.9.9 "$run_url" >/dev/null 2>&1
check "the verification block fixture was written" "$?"
HWP_CHANGELOG="$changelog" HWP_READINESS_RECORD="$record_dir/green.json" \
    bash "$ROOT/scripts/check-verification-block.sh" 9.9.9 "$evaluated" >/dev/null 2>&1
check "the release gate accepts the record of the commit being released" "$?"
if HWP_CHANGELOG="$changelog" HWP_READINESS_RECORD="$record_dir/green.json" \
    bash "$ROOT/scripts/check-verification-block.sh" 9.9.9 "$released_elsewhere" >/dev/null 2>&1; then
    rejected=1
else
    rejected=0
fi
check "the release gate rejects a record that evaluated another commit" "$rejected"
if HWP_CHANGELOG="$changelog" HWP_READINESS_RECORD="$record_dir/record.json" \
    bash "$ROOT/scripts/check-verification-block.sh" 9.9.9 "$evaluated" >/dev/null 2>&1; then
    rejected=1
else
    rejected=0
fi
check "the release gate rejects a record whose gates did not all pass" "$rejected"

# --- 5. readme-pinned-tag ------------------------------------------------------------------------
echo "-- readme-pinned-tag"
readme="$tmp/readme"
mkdir -p "$readme"
(
    cd "$readme"
    git init -q -b main .
    git config user.email selfcheck@example.invalid
    git config user.name selfcheck
    printf 'curl -sSfL https://example.invalid/install.sh | sh -s -- --tag v9.9.9\n' >README.md
    cp README.md README.ko.md
    git add -A
    git commit -qm readme
    git tag v9.9.9
) || { echo "self-check: readme fixture setup failed" >&2; exit 1; }
readme_sha="$(git -C "$readme" rev-parse HEAD)"
readme_gate() { # <ref> <sha> -> prints the recorded status
    local out="$tmp/readme-output"
    : >"$out"
    (cd "$readme" && GITHUB_OUTPUT="$out" READINESS_REF="$1" EVALUATED_SHA="$2" \
        bash "$steps/gate-readme-pinned-tag.sh") >/dev/null 2>&1
    out_value "$out" entry | cut -f2
}
same "a bare tag with both READMEs pinned passes" "$(readme_gate v9.9.9 "$readme_sha")" "pass"
same "a fully qualified tag ref is compared, not skipped" \
    "$(readme_gate refs/tags/v9.9.9 "$readme_sha")" "pass"
same "a branch ref is not applicable" "$(readme_gate main "$readme_sha")" "n/a"
same "a tag that is not the evaluated commit fails" \
    "$(readme_gate v9.9.9 "$(printf 'd%.0s' {1..40})")" "fail"
: >"$readme/README.ko.md"
same "a README.ko.md with no install snippet fails" "$(readme_gate v9.9.9 "$readme_sha")" "fail"
rm -f "$readme/README.ko.md"
same "a missing README.ko.md fails" "$(readme_gate v9.9.9 "$readme_sha")" "fail"
printf 'curl | sh -s -- --tag v0.0.1\n' >"$readme/README.ko.md"
same "a README.ko.md pinning another version fails" "$(readme_gate v9.9.9 "$readme_sha")" "fail"

if [ "$fail" -ne 0 ]; then
    echo "== release-readiness self-check: FAILED" >&2
    exit 1
fi
echo "== release-readiness self-check: OK"
