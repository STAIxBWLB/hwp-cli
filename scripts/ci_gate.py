#!/usr/bin/env python3
"""Small, dependency-free policies shared by CI and release formula gating."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.parse import urlencode

GATES = {"lint", "pdf-parity", "test"}


class GateError(RuntimeError):
    pass


def command(*args):
    try:
        return subprocess.run(args, check=True, capture_output=True, text=True, timeout=30).stdout
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as error:
        raise GateError(f"{args[0]} command failed; inspect authentication/network and rerun CI") from error


def git(*args):
    return command("git", *args).strip()


def gh(*args):
    return command("gh", *args)


def api(endpoint):
    return json.loads(gh("api", endpoint))


def requires_full(paths):
    """Former workflow path policy, now inside decision so ci-ok is always reported."""
    for name in paths:
        if name.startswith("Formula/"):
            continue
        gate_input = (name in {"README.md", "README.ko.md", "CHANGELOG.md",
                               "skills/hwp/SKILL.md", "skills/hwp/SKILL.ko.md"}
                      or name.startswith(("docs/manual/", "scripts/")))
        if gate_input or not name.endswith(".md"):
            return True
    return False


def changed_paths(base, head):
    # Treat renames as deletion+addition so moving Rust code to a docs path cannot hide it.
    return command("git", "diff", "--no-renames", "--name-only", "-z", base, head).split("\0")[:-1]


def check_formula_pr(pr, repo, branch, head):
    if (pr.get("state") != "open" or pr.get("draft") is not False
            or pr.get("base", {}).get("ref") != "main"
            or pr.get("head", {}).get("ref") != branch
            or pr.get("head", {}).get("sha") != head
            or pr.get("head", {}).get("repo", {}).get("full_name") != repo):
        raise GateError("formula PR identity/state changed; inspect the PR and rerun the release")


def path_decision(event_name, event, sha, ref, repo):
    """Skip only an immutable verified path set; ordinary manual runs remain full."""
    if git("rev-parse", "HEAD") != sha:
        raise GateError("checkout differs from event SHA; rerun CI with its default checkout")
    if event_name == "workflow_dispatch":
        number = event.get("inputs", {}).get("formula_pr", "")
        if not number:
            return "full"
        if not str(number).isdigit() or not ref.startswith("refs/heads/brew/formula-v"):
            raise GateError("invalid formula dispatch; use an open formula PR or omit formula_pr")
        pr = api(f"repos/{repo}/pulls/{number}")
        check_formula_pr(pr, repo, ref.removeprefix("refs/heads/"), sha)
        base = git("merge-base", sha, pr["base"]["sha"])
        paths = changed_paths(base, sha)
        if not paths or any(not name.startswith("Formula/") for name in paths):
            raise GateError("formula dispatch contains non-formula changes; run ordinary full CI")
        return "formula"
    if event_name == "pull_request":
        pr = event["pull_request"]
        # GITHUB_SHA is the tested merge, not the mutable current PR head/files API.
        parents = git("rev-list", "--parents", "-n", "1", sha).split()
        if parents != [sha, pr["base"]["sha"], pr["head"]["sha"]]:
            raise GateError("PR merge parents differ from event; run full CI")
        base = pr["base"]["sha"]
    elif event_name == "push":
        base = event["before"]
        if not re.fullmatch(r"[0-9a-f]{40}", base) or base == "0" * 40:
            raise GateError("push base is unavailable; run full CI")
    else:
        return "full"
    return "full" if requires_full(changed_paths(base, sha)) else "paths"


def aggregate(needs):
    if set(needs) != GATES | {"decision"}:
        raise GateError("aggregate dependency set differs; update ci-ok needs and policy together")
    decision = needs["decision"]
    if decision.get("result") != "success":
        raise GateError("CI decision did not succeed; inspect decision and rerun CI")
    outputs = decision.get("outputs", {})
    if outputs.get("run_full") == "true" and outputs.get("skip_kind") == "none":
        expected = "success"
    elif outputs.get("run_full") == "false" and outputs.get("skip_kind") in {"paths", "dedupe", "formula"}:
        expected = "skipped"
    else:
        raise GateError("CI decision outputs are missing/invalid; inspect decision and rerun CI")
    for name in sorted(GATES):
        if needs[name].get("result") != expected:
            raise GateError(f"{name} did not report expected {expected}; inspect its job and rerun CI")



def merge_verified_formula(repo, number, branch, head):
    """Called only after successful exact-head ci-ok; seal both possible merge attempts."""
    endpoint = f"repos/{repo}/pulls/{number}"
    check_formula_pr(api(endpoint), repo, branch, head)
    try:
        gh("pr", "merge", str(number), "--repo", repo, "--auto", "--squash",
           "--match-head-commit", head)
    except GateError as error:
        cause = error.__cause__
        if (not isinstance(cause, subprocess.CalledProcessError)
                or "pull request is in clean status" not in (cause.stderr or "").lower()):
            raise
        # No required-check configuration: GitHub refuses auto-merge on a clean PR.
        # CI has already succeeded, but identity and mergeability must still be current.
        current = api(endpoint)
        check_formula_pr(current, repo, branch, head)
        if current.get("mergeable") is not True or current.get("mergeable_state") != "clean":
            raise GateError("formula PR is not clean and mergeable; inspect its checks and rerun release") from error
        gh("pr", "merge", str(number), "--repo", repo, "--squash",
           "--match-head-commit", head)


def formula_gate(repo, branch, head, timeout=600, poll=10, clock=time.monotonic, sleep=time.sleep):
    """Dispatch CI and merge only its successful exact-head ci-ok, never an unchecked fallback."""
    if (not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo)
            or not re.fullmatch(r"brew/formula-v[0-9A-Za-z.+-]+", branch)
            or not re.fullmatch(r"[0-9a-f]{40}", head)):
        raise GateError("invalid formula repo/branch/SHA; inspect release inputs and rerun")
    owner = repo.split("/")[0]
    query = urlencode({"state": "open", "head": f"{owner}:{branch}", "base": "main"})
    pulls = api(f"repos/{repo}/pulls?{query}")
    if len(pulls) != 1:
        raise GateError("expected one open formula PR; open or inspect the formula PR and rerun")
    pr = pulls[0]
    number = pr["number"]
    check_formula_pr(pr, repo, branch, head)
    runs_url = f"repos/{repo}/actions/workflows/ci.yml/runs?head_sha={head}&event=workflow_dispatch&per_page=100"
    before = {run["id"] for run in api(runs_url)["workflow_runs"]}
    # GITHUB_TOKEN PR events may require approval; explicit dispatch always starts a run.
    # https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow
    gh("workflow", "run", "ci.yml", "--repo", repo, "--ref", branch, "-f", f"formula_pr={number}")
    deadline = clock() + timeout
    while clock() < deadline:
        check_formula_pr(api(f"repos/{repo}/pulls/{number}"), repo, branch, head)
        runs = [run for run in api(runs_url)["workflow_runs"]
                if run["id"] not in before and run.get("head_sha") == head
                and run.get("head_branch") == branch and run.get("event") == "workflow_dispatch"]
        if runs:
            run = max(runs, key=lambda item: item["id"])
            if run.get("status") == "completed":
                jobs = api(f"repos/{repo}/actions/runs/{run['id']}/jobs?filter=latest&per_page=100")["jobs"]
                checks = [job for job in jobs if job.get("name") == "ci-ok"]
                if (run.get("conclusion") != "success" or len(checks) != 1
                        or checks[0].get("status") != "completed"
                        or checks[0].get("conclusion") != "success"):
                    raise GateError("formula ci-ok did not succeed; inspect its CI run and rerun release")
                merge_verified_formula(repo, number, branch, head)
                return
        print("Waiting for exact-head formula ci-ok", flush=True)
        sleep(poll)
    raise GateError("formula ci-ok did not complete before deadline; inspect CI and rerun release")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["paths", "aggregate", "formula"])
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--branch")
    parser.add_argument("--head")
    args = parser.parse_args()
    try:
        if args.mode == "paths":
            event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
            try:
                result = path_decision(os.environ["GITHUB_EVENT_NAME"], event,
                                       os.environ["GITHUB_SHA"], os.environ["GITHUB_REF"], args.repo)
            except (GateError, KeyError, ValueError) as error:
                if event.get("inputs", {}).get("formula_pr"):
                    raise GateError("formula identity/path proof failed; inspect dispatch and rerun release") from error
                print("::warning::Path proof unavailable; running full CI", file=sys.stderr)
                result = "full"
            print(result)
        elif args.mode == "aggregate":
            aggregate(json.loads(os.environ["NEEDS_JSON"]))
            print("ci-ok: all required results satisfied")
        else:
            formula_gate(args.repo, args.branch, args.head)
    except GateError as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    except (KeyError, ValueError, TypeError, OSError):
        print("::error::CI gate inputs/API response are missing or malformed; inspect event data and authentication, then rerun CI", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
