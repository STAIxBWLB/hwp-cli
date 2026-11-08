"""Hermetic CI aggregation and exact-head formula policies (#412). No live gh calls."""
from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ci_gate", ROOT / "scripts/ci_gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
REPO, BRANCH = "STAIxBWLB/hwp-cli", "brew/formula-v1.3.2"
HEAD, BASE, MERGE = "a" * 40, "b" * 40, "c" * 40


def pr():
    return {"number": 17, "state": "open", "draft": False,
            "base": {"ref": "main", "sha": BASE},
            "head": {"ref": BRANCH, "sha": HEAD, "repo": {"full_name": REPO}}}


def run(run_id=101, **updates):
    value = {"id": run_id, "head_sha": HEAD, "head_branch": BRANCH,
             "event": "workflow_dispatch", "status": "completed", "conclusion": "success"}
    value.update(updates)
    return value


def ci_ok(**updates):
    value = {"name": "ci-ok", "status": "completed", "conclusion": "success"}
    value.update(updates)
    return value


class FormulaHarness:
    """Controlled API responses and clock; no process, network, dispatch or merge is real."""
    def __init__(self, snapshots=None, jobs=None, old=None, current=None, pulls=None):
        self.snapshots = snapshots if snapshots is not None else [[run()]]
        self.jobs = jobs if jobs is not None else [ci_ok()]
        self.old = old if old is not None else [run(90)]
        self.current = current if current is not None else [pr()]
        self.pulls = pulls if pulls is not None else [pr()]
        self.run_reads = self.pr_reads = self.now = 0
        self.api_calls, self.gh_calls, self.sleeps = [], [], []
        self.fail_api = self.fail_gh = None

    def api(self, endpoint):
        self.api_calls.append(endpoint)
        if self.fail_api and self.fail_api(endpoint):
            raise gate.GateError("synthetic API failure")
        if "/pulls?" in endpoint:
            return deepcopy(self.pulls)
        if endpoint == f"repos/{REPO}/pulls/17":
            value = self.current[min(self.pr_reads, len(self.current) - 1)]
            self.pr_reads += 1
            return deepcopy(value)
        if "/actions/workflows/ci.yml/runs?" in endpoint:
            self.run_reads += 1
            value = self.old if self.run_reads == 1 else self.snapshots[min(self.run_reads - 2, len(self.snapshots) - 1)]
            return {"workflow_runs": deepcopy(value)}
        if re.fullmatch(rf"repos/{re.escape(REPO)}/actions/runs/[0-9]+/jobs\?filter=latest&per_page=100", endpoint):
            return {"jobs": deepcopy(self.jobs)}
        raise AssertionError(f"unexpected API endpoint: {endpoint}")

    def gh(self, *args):
        self.gh_calls.append(args)
        if self.fail_gh and self.fail_gh(args):
            raise gate.GateError("synthetic gh failure")
        return ""

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds

    @property
    def merges(self):
        return [args for args in self.gh_calls if args[:2] == ("pr", "merge")]

    def invoke(self, **overrides):
        args = dict(repo=REPO, branch=BRANCH, head=HEAD, timeout=3, poll=1,
                    clock=lambda: self.now, sleep=self.sleep)
        args.update(overrides)
        with patch.object(gate, "api", side_effect=self.api), patch.object(gate, "gh", side_effect=self.gh), redirect_stdout(io.StringIO()):
            return gate.formula_gate(**args)


class PathPolicyTests(unittest.TestCase):
    def test_formula_and_non_gate_markdown_are_lightweight(self):
        for paths in [[], ["Formula/hwp.rb"], ["Formula/nested/hwp.rb"], ["docs/design/note.md"], ["AGENTS.md"], ["notes.md", "Formula/hwp.rb"]]:
            with self.subTest(paths=paths):
                self.assertFalse(gate.requires_full(paths))

    def test_code_and_every_gate_input_require_full(self):
        for name in ["crates/hwp-cli/src/main.rs", "Cargo.lock", ".github/workflows/ci.yml", "README.md", "README.ko.md", "CHANGELOG.md", "skills/hwp/SKILL.md", "skills/hwp/SKILL.ko.md", "docs/manual/cli-reference.md", "docs/manual/other.json", "scripts/policy.md", "scripts/check.sh"]:
            with self.subTest(name=name):
                self.assertTrue(gate.requires_full(["Formula/hwp.rb", name]))

    def test_changed_paths_uses_immutable_refs_without_rename_hiding(self):
        with patch.object(gate, "command", return_value="crates/old.rs\0docs/new.md\0") as command:
            self.assertEqual(gate.changed_paths(BASE, HEAD), ["crates/old.rs", "docs/new.md"])
        command.assert_called_once_with("git", "diff", "--no-renames", "--name-only", "-z", BASE, HEAD)

    def decide(self, event_name, event, sha=HEAD, ref="refs/heads/main", paths=None, parents=None, checkout=None, pull=None):
        def git(*args):
            if args == ("rev-parse", "HEAD"):
                return checkout if checkout is not None else sha
            if args == ("rev-list", "--parents", "-n", "1", sha):
                return parents if parents is not None else f"{sha} {BASE} {HEAD}"
            if args == ("merge-base", sha, BASE):
                return BASE
            raise AssertionError(f"unexpected git call: {args}")
        with patch.object(gate, "git", side_effect=git), patch.object(gate, "changed_paths", return_value=paths if paths is not None else ["Formula/hwp.rb"]) as changed, patch.object(gate, "api", return_value=pull if pull is not None else pr()) as api:
            result = gate.path_decision(event_name, event, sha, ref, REPO)
            return result, changed.call_args_list, api.call_args_list

    def test_push_formula_only_skips_but_code_runs_full(self):
        self.assertEqual(self.decide("push", {"before": BASE})[0], "paths")
        self.assertEqual(self.decide("push", {"before": BASE}, paths=["Formula/hwp.rb", "crates/a.rs"])[0], "full")

    def test_pull_request_uses_event_merge_parents_without_mutable_api(self):
        event = {"pull_request": {"base": {"sha": BASE}, "head": {"sha": HEAD}}}
        result, calls, api = self.decide("pull_request", event, sha=MERGE, ref="refs/pull/17/merge")
        self.assertEqual(result, "paths")
        self.assertEqual(calls[0].args, (BASE, MERGE))
        self.assertEqual(api, [])

    def test_unproven_merge_checkout_or_push_base_is_refused(self):
        event = {"pull_request": {"base": {"sha": BASE}, "head": {"sha": HEAD}}}
        for parents in [f"{MERGE} {BASE} {'d' * 40}", f"{MERGE} {HEAD} {BASE}", f"{MERGE} {BASE}"]:
            with self.subTest(parents=parents), self.assertRaises(gate.GateError):
                self.decide("pull_request", event, sha=MERGE, parents=parents)
        with self.assertRaises(gate.GateError):
            self.decide("push", {"before": BASE}, checkout="d" * 40)
        for before in ["0" * 40, "missing", "A" * 40]:
            with self.subTest(before=before), self.assertRaises(gate.GateError):
                self.decide("push", {"before": before})

    def test_ordinary_dispatch_and_other_events_are_full(self):
        self.assertEqual(self.decide("workflow_dispatch", {"inputs": {}})[0], "full")
        self.assertEqual(self.decide("schedule", {})[0], "full")

    def test_formula_dispatch_requires_formula_only_immutable_paths(self):
        result, calls, api = self.decide("workflow_dispatch", {"inputs": {"formula_pr": "17"}}, ref=f"refs/heads/{BRANCH}")
        self.assertEqual(result, "formula")
        self.assertEqual(calls[0].args, (BASE, HEAD))
        self.assertEqual(api[0].args, (f"repos/{REPO}/pulls/17",))
        for paths in [[], ["crates/a.rs"], ["Formula/hwp.rb", "scripts/check.sh"]]:
            with self.subTest(paths=paths), self.assertRaises(gate.GateError):
                self.decide("workflow_dispatch", {"inputs": {"formula_pr": "17"}}, ref=f"refs/heads/{BRANCH}", paths=paths)

    def test_formula_dispatch_refuses_invalid_number_ref_and_pr_identity(self):
        for number, ref in [("17;echo bad", f"refs/heads/{BRANCH}"), ("17", "refs/heads/main")]:
            with self.subTest(number=number), self.assertRaises(gate.GateError):
                self.decide("workflow_dispatch", {"inputs": {"formula_pr": number}}, ref=ref)
        wrong = pr()
        wrong["head"]["sha"] = "d" * 40
        with self.assertRaises(gate.GateError):
            self.decide("workflow_dispatch", {"inputs": {"formula_pr": "17"}}, ref=f"refs/heads/{BRANCH}", pull=wrong)

    def test_main_ordinary_proof_failure_runs_full_but_formula_fails(self):
        for formula in [False, True]:
            with self.subTest(formula=formula), tempfile.TemporaryDirectory() as tmp:
                event = Path(tmp) / "event.json"
                event.write_text(json.dumps({"inputs": {"formula_pr": "17"} if formula else {}}))
                out, err = io.StringIO(), io.StringIO()
                env = {"GITHUB_EVENT_PATH": str(event), "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": HEAD, "GITHUB_REF": f"refs/heads/{BRANCH}"}
                with patch.dict(os.environ, env), patch.object(gate, "path_decision", side_effect=gate.GateError("proof unavailable")), patch("sys.argv", ["ci_gate.py", "paths", "--repo", REPO]), redirect_stdout(out), redirect_stderr(err):
                    result = gate.main()
                self.assertEqual(result, 1 if formula else 0)
                self.assertEqual(out.getvalue().strip(), "" if formula else "full")


class AggregateTests(unittest.TestCase):
    def needs(self, full=True, kind="none"):
        return {"decision": {"result": "success", "outputs": {"run_full": "true" if full else "false", "skip_kind": kind}}, **{name: {"result": "success" if full else "skipped"} for name in gate.GATES}}

    def test_full_and_only_justified_skips_succeed(self):
        gate.aggregate(self.needs())
        for kind in ["paths", "dedupe", "formula"]:
            with self.subTest(kind=kind):
                gate.aggregate(self.needs(False, kind))

    def test_every_gate_rejects_non_success_or_missing_state_on_full_run(self):
        for name in gate.GATES:
            for state in ["failure", "cancelled", "skipped", "neutral", "timed_out", "action_required", "queued", "unknown", None]:
                with self.subTest(name=name, state=state):
                    needs = self.needs()
                    needs[name] = {} if state is None else {"result": state}
                    with self.assertRaises(gate.GateError):
                        gate.aggregate(needs)

    def test_justified_skip_requires_all_gates_skipped(self):
        for name in gate.GATES:
            for state in ["success", "failure", "cancelled", "neutral", "unknown", None]:
                with self.subTest(name=name, state=state):
                    needs = self.needs(False, "paths")
                    needs[name] = {} if state is None else {"result": state}
                    with self.assertRaises(gate.GateError):
                        gate.aggregate(needs)

    def test_decision_missing_unknown_or_inconsistent_outputs_fail(self):
        for outputs in [{}, {"run_full": "true"}, {"skip_kind": "none"}, {"run_full": True, "skip_kind": "none"}, {"run_full": "true", "skip_kind": "paths"}, {"run_full": "false", "skip_kind": "none"}, {"run_full": "false", "skip_kind": "unknown"}]:
            with self.subTest(outputs=outputs):
                needs = self.needs()
                needs["decision"]["outputs"] = outputs
                with self.assertRaises(gate.GateError):
                    gate.aggregate(needs)
        for state in ["failure", "cancelled", "skipped", "neutral", None]:
            with self.subTest(state=state):
                needs = self.needs()
                needs["decision"]["result"] = state
                with self.assertRaises(gate.GateError):
                    gate.aggregate(needs)

    def test_missing_or_extra_dependency_fails(self):
        for name in ["decision", *gate.GATES]:
            with self.subTest(name=name):
                needs = self.needs()
                del needs[name]
                with self.assertRaises(gate.GateError):
                    gate.aggregate(needs)
        needs = self.needs()
        needs["unaccounted_gate"] = {"result": "success"}
        with self.assertRaises(gate.GateError):
            gate.aggregate(needs)


class FormulaGateTests(unittest.TestCase):
    def test_pending_then_success_merges_once_with_exact_head(self):
        fake = FormulaHarness(snapshots=[[], [run(status="in_progress", conclusion=None)], [run()]])
        fake.invoke()
        self.assertEqual(fake.sleeps, [1, 1])
        self.assertEqual(fake.gh_calls[0], ("workflow", "run", "ci.yml", "--repo", REPO, "--ref", BRANCH, "-f", "formula_pr=17"))
        self.assertEqual(fake.merges, [("pr", "merge", "17", "--repo", REPO, "--auto", "--squash", "--match-head-commit", HEAD)])
        self.assertIn(f"repos/{REPO}/actions/runs/101/jobs?filter=latest&per_page=100", fake.api_calls)

    def test_plus_metadata_branch_is_urlencoded_without_changing_identity(self):
        branch = "brew/formula-v1.3.2+build.7"
        value = pr()
        value["head"]["ref"] = branch
        fake = FormulaHarness(pulls=[value], current=[value], snapshots=[[run(head_branch=branch)]])
        fake.invoke(branch=branch)
        endpoint = next(endpoint for endpoint in fake.api_calls if "/pulls?" in endpoint)
        query = parse_qs(urlsplit("https://example.invalid/" + endpoint).query)
        self.assertEqual(query["head"], ["STAIxBWLB:" + branch])
        self.assertEqual(len(fake.merges), 1)
        self.assertIn(branch, fake.gh_calls[0])

    def test_completed_run_bad_or_missing_conclusion_never_merges(self):
        for state in ["failure", "cancelled", "skipped", "neutral", "timed_out", "action_required", None]:
            with self.subTest(state=state):
                fake = FormulaHarness(snapshots=[[run(conclusion=state)]])
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(fake.merges, [])

    def test_ci_ok_must_exist_once_and_be_completed_success(self):
        cases = [[], [ci_ok(), ci_ok()], [ci_ok(name="other")], [ci_ok(status="in_progress")], [ci_ok(status=None)]]
        cases += [[ci_ok(conclusion=state)] for state in ["failure", "cancelled", "skipped", "neutral", "timed_out", None]]
        for jobs in cases:
            with self.subTest(jobs=jobs):
                fake = FormulaHarness(jobs=jobs)
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(fake.merges, [])

    def test_old_run_ids_or_wrong_run_identity_never_authorize_merge(self):
        fake = FormulaHarness(snapshots=[[run(90), run(101, head_sha="d" * 40), run(102, head_branch="other"), run(103, event="pull_request")]])
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(fake.merges, [])
        self.assertFalse(any("/jobs?" in endpoint for endpoint in fake.api_calls))

    def test_newer_failed_run_is_not_hidden_by_older_success(self):
        fake = FormulaHarness(snapshots=[[run(101), run(102, conclusion="failure")]])
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(fake.merges, [])

    def test_stale_head_before_observation_or_at_final_seal_never_merges(self):
        stale = pr()
        stale["head"]["sha"] = "d" * 40
        for current in [[stale], [pr(), stale]]:
            with self.subTest(current=current):
                fake = FormulaHarness(current=current)
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(fake.merges, [])

    def test_wrong_pr_repo_branch_base_state_or_draft_never_dispatches(self):
        for mutation in ["repo", "branch", "base", "closed", "draft", "stale"]:
            with self.subTest(mutation=mutation):
                value = pr()
                if mutation == "repo": value["head"]["repo"]["full_name"] = "other/repo"
                elif mutation == "branch": value["head"]["ref"] = "feature/other"
                elif mutation == "base": value["base"]["ref"] = "release"
                elif mutation == "closed": value["state"] = "closed"
                elif mutation == "draft": value["draft"] = True
                else: value["head"]["sha"] = "d" * 40
                fake = FormulaHarness(pulls=[value])
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(fake.gh_calls, [])

    def test_no_or_multiple_formula_prs_never_dispatches(self):
        for pulls in [[], [pr(), pr()]]:
            with self.subTest(pulls=pulls):
                fake = FormulaHarness(pulls=pulls)
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(fake.gh_calls, [])

    def test_invalid_identity_inputs_never_call_api_or_gh(self):
        for inputs in [{"repo": "bad/repo/extra"}, {"repo": "owner/repo;echo bad"}, {"branch": "main"}, {"head": "short"}, {"head": "A" * 40}]:
            with self.subTest(inputs=inputs):
                fake = FormulaHarness()
                with self.assertRaises(gate.GateError):
                    fake.invoke(**inputs)
                self.assertEqual(fake.api_calls, [])
                self.assertEqual(fake.gh_calls, [])

    def test_pending_run_times_out_without_merge(self):
        fake = FormulaHarness(snapshots=[[run(status="queued", conclusion=None)]])
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(fake.now, 3)
        self.assertEqual(fake.merges, [])

    def test_api_failure_at_each_phase_never_merges(self):
        for phase in ["pull_list", "run_snapshot", "pr_refresh", "jobs", "final_seal"]:
            with self.subTest(phase=phase):
                fake = FormulaHarness()
                if phase == "pull_list": fake.fail_api = lambda endpoint: "/pulls?" in endpoint
                elif phase == "run_snapshot": fake.fail_api = lambda endpoint: "/workflows/ci.yml/runs?" in endpoint
                elif phase == "pr_refresh": fake.fail_api = lambda endpoint: endpoint.endswith("/pulls/17")
                elif phase == "jobs": fake.fail_api = lambda endpoint: "/jobs?" in endpoint
                else: fake.fail_api = lambda endpoint: endpoint.endswith("/pulls/17") and fake.pr_reads >= 1
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(fake.merges, [])

    def test_dispatch_failure_never_merges(self):
        fake = FormulaHarness()
        fake.fail_gh = lambda args: args[:2] == ("workflow", "run")
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(fake.merges, [])

    def test_failed_auto_merge_has_no_immediate_fallback(self):
        fake = FormulaHarness()
        fake.fail_gh = lambda args: args[:2] == ("pr", "merge")
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(len(fake.merges), 1)
        self.assertIn("--auto", fake.merges[0])
        self.assertIn("--match-head-commit", fake.merges[0])


class WorkflowTests(unittest.TestCase):
    """Static tests intentionally fail old YAML at the aggregate/path-filter assertions."""
    def jobs(self):
        content = (ROOT / ".github/workflows/ci.yml").read_text()
        jobs = content.split("\njobs:\n", 1)[1]
        starts = list(re.finditer(r"(?m)^  ([A-Za-z][A-Za-z0-9_-]*):\s*$", jobs))
        return content, {match.group(1): jobs[match.end():starts[index + 1].start() if index + 1 < len(starts) else len(jobs)] for index, match in enumerate(starts)}

    def test_ci_ok_always_covers_all_existing_gates_and_decision(self):
        _, jobs = self.jobs()
        self.assertIn("ci-ok", jobs)
        aggregate = jobs["ci-ok"]
        self.assertRegex(aggregate, r"(?m)^    if:\s*(?:\$\{\{\s*)?always\(\)")
        needs = re.search(r"(?m)^    needs:\s*\[([^\]]+)\]", aggregate)
        self.assertIsNotNone(needs, "keep ci-ok dependencies explicit in one YAML list")
        self.assertEqual({item.strip().strip("'\"") for item in needs.group(1).split(",")}, {"decision", "lint", "pdf-parity", "test"})
        self.assertIn("ci_gate.py aggregate", aggregate)
        self.assertIn("NEEDS_JSON", aggregate)

    def test_no_workflow_path_filter_and_typed_formula_dispatch_input(self):
        content, _ = self.jobs()
        events = content.split("\njobs:\n", 1)[0]
        self.assertNotRegex(events, r"(?m)^\s+paths(?:-ignore)?:")
        self.assertRegex(events, r"workflow_dispatch:[\s\S]*?inputs:[\s\S]*?formula_pr:")
        dispatch = events.split("formula_pr:", 1)[1]
        self.assertRegex(dispatch, r"(?m)^\s+type:\s*string\s*$")
        self.assertRegex(dispatch, r"(?m)^\s+required:\s*false\s*$")

    def test_existing_check_names_and_three_platforms_unchanged(self):
        _, jobs = self.jobs()
        for name in ["decision", "lint", "pdf-parity"]:
            self.assertRegex(jobs[name], rf"(?m)^    name:\s*{re.escape(name)}\s*$")
        self.assertIn("name: test (${{ matrix.os }})", jobs["test"])
        for platform in ["ubuntu-latest", "macos-latest", "windows-latest"]:
            self.assertIn(f"os: {platform}", jobs["test"])

    def test_release_state_classifier_allows_only_success_or_absent_legacy_fallback(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        classifier = re.search(r'case "\$state" in[\s\S]*?\n\s*esac', release)
        self.assertIsNotNone(classifier, "exercise the actual verify-ci state classifier")
        states = {
            "completed/success": ("true", "false"),
            "absent": ("false", "true"),
            "completed/skipped": ("false", "false"),
            "completed/neutral": ("false", "false"),
            "completed/failure": ("false", "false"),
            "completed/cancelled": ("false", "false"),
            "queued/pending": ("false", "false"),
            "in_progress/pending": ("false", "false"),
            "completed/pending": ("false", "false"),
            "unknown": ("false", "false"),
        }
        script = ('set -euo pipefail\nall_green=true\nfallback_ok=true\nstate="$1"\n'
                  + classifier.group() + '\nprintf "%s %s\\n" "$all_green" "$fallback_ok"\n')
        for state, expected in states.items():
            with self.subTest(state=state):
                result = subprocess.run(["bash", "-c", script, "verify-ci-state", state],
                                        capture_output=True, text=True, check=True, timeout=3)
                self.assertEqual(tuple(result.stdout.strip().split()), expected)

    def test_release_formula_calls_gate_without_unchecked_merge(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        update = release.split("\n  update-formula:\n", 1)[1]
        self.assertIn("ci_gate.py formula", update)
        self.assertNotRegex(update, r"gh\s+pr\s+merge")


if __name__ == "__main__":
    unittest.main()
