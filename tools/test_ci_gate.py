"""Hermetic CI aggregation and exact-head formula policies (#412). No live gh calls."""
from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ci_gate", ROOT / "scripts/ci_gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)
REPO, BRANCH = "STAIxBWLB/agent-hwp", "brew/formula-v1.3.2"
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

    def test_all_skill_and_corpus_markdown_are_gate_inputs(self):
        for name in ["skills/hwp/official-documents.md", "skills/hwp/official-documents.ko.md", "skills/hwp/references/guide.md",
                     "skills/hwp/templates/form.md", "skills/hwp/other.md",
                     "corpus/README.md", "corpus/nested/probe.md"]:
            with self.subTest(name=name):
                self.assertTrue(gate.requires_full([name]), "gate input Markdown must not take the docs-only path")

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


class DiagnosticsTests(unittest.TestCase):
    def test_command_failure_keeps_trimmed_cause_and_operation_specific_next_action(self):
        cases = [
            (("gh", "workflow", "run", "ci.yml"), "dispatch rejected", r"(?i)(dispatch|workflow).*(permission|input|ref|inspect|rerun)"),
            (("gh", "pr", "merge", "17"), "merge rejected", r"(?i)(merge|PR|pull request).*(check|inspect|rerun)"),
            (("git", "diff", BASE, HEAD), "bad revision", r"(?i)(git|diff|revision|checkout).*(fetch|history|base|inspect|rerun)"),
        ]
        for args, cause, action in cases:
            with self.subTest(args=args):
                failure = subprocess.CalledProcessError(1, args, stderr=f"  {cause}  ")
                with patch.object(gate.subprocess, "run", side_effect=failure):
                    with self.assertRaises(gate.GateError) as caught:
                        gate.command(*args)
                message = str(caught.exception)
                self.assertIn(cause, message, "the actual command rejection must remain diagnosable")
                self.assertNotIn(f"  {cause}  ", message)
                self.assertRegex(message, action)
                self.assertIs(caught.exception.__cause__, failure)

    def test_long_command_error_keeps_cause_bound_and_merge_remedy(self):
        failure = subprocess.CalledProcessError(1, ["gh", "pr", "merge"], stderr="rejected " + "X" * 5000)
        with patch.object(gate.subprocess, "run", side_effect=failure):
            with self.assertRaises(gate.GateError) as caught:
                gate.command("gh", "pr", "merge", "17")
        message = str(caught.exception)
        self.assertLess(len(message), 1000)
        self.assertIn("rejected", message)
        self.assertIn("inspect PR checks", message)
        self.assertIn("rerun release", message)

    def test_command_failure_does_not_emit_multiline_workflow_commands_or_secrets(self):
        secret = "ghp_" + "A" * 36
        for stderr in [f"permission denied\n::error::forged\nAuthorization: Bearer {secret}",
                       f"authentication rejected: {secret}",
                       "Authorization: Bearer confidential-value"]:
            with self.subTest(stderr=stderr):
                failure = subprocess.CalledProcessError(1, ["gh", "api"], stderr=stderr)
                with patch.object(gate.subprocess, "run", side_effect=failure):
                    with self.assertRaises(gate.GateError) as caught:
                        gate.command("gh", "api", "private-argument")
                message = str(caught.exception)
                self.assertNotIn("\n", message)
                self.assertNotIn("\r", message)
                self.assertNotIn("::error::forged", message)
                self.assertNotIn(secret, message)
                self.assertNotIn("confidential-value", message)
                self.assertNotIn("private-argument", message)

    def test_main_preserves_path_proof_cause_for_formula_and_full_fallback(self):
        for formula in [False, True]:
            with self.subTest(formula=formula), tempfile.TemporaryDirectory() as tmp:
                event = Path(tmp) / "event.json"
                event.write_text(json.dumps({"inputs": {"formula_pr": "17"} if formula else {}}))
                out, err = io.StringIO(), io.StringIO()
                env = {"GITHUB_EVENT_PATH": str(event), "GITHUB_EVENT_NAME": "workflow_dispatch",
                       "GITHUB_SHA": HEAD, "GITHUB_REF": f"refs/heads/{BRANCH}"}
                with patch.dict(os.environ, env), patch.object(gate, "path_decision", side_effect=gate.GateError("immutable merge-base unavailable; fetch history")), patch("sys.argv", ["ci_gate.py", "paths", "--repo", REPO]), redirect_stdout(out), redirect_stderr(err):
                    result = gate.main()
                self.assertEqual(result, 1 if formula else 0)
                self.assertEqual(out.getvalue().strip(), "" if formula else "full")
                self.assertIn("immutable merge-base unavailable", err.getvalue())
                self.assertIn("fetch history", err.getvalue())

    def test_main_formula_failure_preserves_actual_dispatch_rejection(self):
        out, err = io.StringIO(), io.StringIO()
        failure = subprocess.CalledProcessError(1, ["gh", "workflow", "run"], stderr="workflow dispatch forbidden")
        with patch.object(gate, "api", side_effect=FormulaHarness().api), patch.object(gate.subprocess, "run", side_effect=failure), patch("sys.argv", ["ci_gate.py", "formula", "--repo", REPO, "--branch", BRANCH, "--head", HEAD]), redirect_stdout(out), redirect_stderr(err):
            result = gate.main()
        self.assertEqual(result, 1)
        self.assertIn("workflow dispatch forbidden", err.getvalue())


class MacosDecisionTests(unittest.TestCase):
    def pull(self, draft):
        pull_request = {"number": 1} if draft is gate else {"number": 1, "draft": draft}
        return {"pull_request": pull_request}

    def test_only_a_positive_draft_pull_request_defers(self):
        self.assertEqual(gate.macos_decision("pull_request", self.pull(True)), "defer")
        self.assertEqual(gate.macos_decision("pull_request", self.pull(False)), "run")

    def test_unknown_draft_state_is_an_error_never_a_silent_deferral(self):
        for draft in [gate, None, "true", "false", 0, 1, ""]:
            with self.subTest(draft=draft), self.assertRaises(gate.GateError):
                gate.macos_decision("pull_request", self.pull(draft))
        with self.assertRaises(gate.GateError):
            gate.macos_decision("pull_request", {})

    def test_other_events_always_run_macos(self):
        for name in ["push", "workflow_dispatch", "schedule"]:
            with self.subTest(name=name):
                self.assertEqual(gate.macos_decision(name, {"pull_request": {"draft": True}}), "run")
                self.assertEqual(gate.macos_decision(name, {}), "run")

    def run_mode(self, name, event):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "event.json"
            path.write_text(json.dumps(event))
            env = {**os.environ, "GITHUB_EVENT_NAME": name, "GITHUB_EVENT_PATH": str(path)}
            return subprocess.run(["python3", str(ROOT / "scripts/ci_gate.py"), "macos"], env=env, capture_output=True, text=True, timeout=10)

    def test_cli_defers_a_draft_and_runs_macos_when_the_state_is_unknown(self):
        draft = self.run_mode("pull_request", self.pull(True))
        self.assertEqual((draft.returncode, draft.stdout.strip()), (0, "defer"), draft.stderr)
        ready = self.run_mode("pull_request", self.pull(False))
        self.assertEqual((ready.returncode, ready.stdout.strip()), (0, "run"), ready.stderr)
        unknown = self.run_mode("pull_request", self.pull(None))
        self.assertEqual((unknown.returncode, unknown.stdout.strip()), (0, "run"), "an unknown draft state must run macOS, not defer it")
        self.assertIn("::warning::", unknown.stderr)
        push = self.run_mode("push", {})
        self.assertEqual(push.stdout.strip(), "run")


class AggregateTests(unittest.TestCase):
    def needs(self, full=True, kind="none", deferred="false"):
        return {"decision": {"result": "success", "outputs": {"run_full": "true" if full else "false", "skip_kind": kind, "macos_deferred": deferred}}, **{name: {"result": "success" if full else "skipped"} for name in gate.GATES}}

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
        for outputs in [{}, {"run_full": "true"}, {"skip_kind": "none"}, {"run_full": True, "skip_kind": "none"}, {"run_full": "true", "skip_kind": "paths"}, {"run_full": "false", "skip_kind": "none"}, {"run_full": "false", "skip_kind": "unknown"},
                        {"run_full": "true", "skip_kind": "none"}, {"run_full": "true", "skip_kind": "none", "macos_deferred": ""},
                        {"run_full": "true", "skip_kind": "none", "macos_deferred": True}, {"run_full": "true", "skip_kind": "none", "macos_deferred": "maybe"}]:
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

    def test_ready_pr_runs_macos_and_succeeds(self):
        gate.aggregate(self.needs(True, "none", "false"))

    def test_draft_pr_deferral_fails_with_the_exact_annotation(self):
        # Draft: the deferred leg is not in `test`, so every other gate still succeeded.
        with self.assertRaises(gate.DeferredError) as raised:
            gate.aggregate(self.needs(True, "none", "true"))
        self.assertEqual(str(raised.exception), "::error title=macOS deferred::draft PR; macOS runs when the PR is marked ready (gh pr ready)")
        self.assertEqual(gate.MACOS_DEFERRED_ANNOTATION, str(raised.exception))

    def test_deferral_never_hides_a_real_failure(self):
        needs = self.needs(True, "none", "true")
        needs["lint"] = {"result": "failure"}
        with self.assertRaises(gate.GateError) as raised:
            gate.aggregate(needs)
        self.assertNotIsInstance(raised.exception, gate.DeferredError, "the real failure is reported, not the deferral")

    def test_deferral_on_a_lightweight_run_is_inconsistent_and_fails(self):
        for kind in ["paths", "dedupe", "formula"]:
            with self.subTest(kind=kind), self.assertRaises(gate.GateError) as raised:
                gate.aggregate(self.needs(False, kind, "true"))
            self.assertNotIsInstance(raised.exception, gate.DeferredError)

    def test_main_prints_the_annotation_verbatim_and_fails_a_deferred_aggregate(self):
        for deferred, code in [("true", 1), ("false", 0)]:
            with self.subTest(deferred=deferred):
                env = {**os.environ, "NEEDS_JSON": json.dumps(self.needs(True, "none", deferred))}
                result = subprocess.run(["python3", str(ROOT / "scripts/ci_gate.py"), "aggregate"], env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, code, result.stdout + result.stderr)
                if deferred == "true":
                    self.assertEqual(result.stderr.strip(), gate.MACOS_DEFERRED_ANNOTATION)
                    self.assertNotIn("ci-ok: all required results satisfied", result.stdout)

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

    def test_generic_auto_merge_error_has_no_fallback(self):
        fake = FormulaHarness()
        fake.fail_gh = lambda args: args[:2] == ("pr", "merge")
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(len(fake.merges), 1)
        self.assertIn("--auto", fake.merges[0])
        self.assertIn("--match-head-commit", fake.merges[0])


class CheckedFormulaFallbackTests(unittest.TestCase):
    """A direct merge is allowed only after exact-head CI and the known clean-status error."""

    @staticmethod
    def ready_pr(**updates):
        value = pr()
        value.update(mergeable=True, mergeable_state="clean")
        value.update(updates)
        return value

    @staticmethod
    def command_error(args, stderr):
        cause = subprocess.CalledProcessError(1, ["gh", *args], output="", stderr=stderr)
        raise gate.GateError("gh command failed; inspect authentication/network and rerun CI") from cause

    def clean_auto_error(self, args):
        if args[:2] == ("pr", "merge") and "--auto" in args:
            self.command_error(args, "GraphQL: Pull request is in clean status (enablePullRequestAutoMerge)")
        return False

    def test_clean_status_error_uses_two_exact_head_merges_with_fresh_seal(self):
        fake = FormulaHarness(current=[pr(), pr(), self.ready_pr()])
        merge_observations = []

        def errors(args):
            if args[:2] == ("pr", "merge"):
                merge_observations.append(("--auto" in args, fake.pr_reads, list(fake.api_calls)))
            return self.clean_auto_error(args)

        fake.fail_gh = errors
        try:
            fake.invoke()
        except gate.GateError as error:
            self.fail(f"successful exact-head ci-ok plus a freshly sealed clean PR must permit the checked fallback: {error}")
        self.assertEqual(fake.merges, [
            ("pr", "merge", "17", "--repo", REPO, "--auto", "--squash", "--match-head-commit", HEAD),
            ("pr", "merge", "17", "--repo", REPO, "--squash", "--match-head-commit", HEAD),
        ])
        self.assertEqual(len(merge_observations), 2)
        self.assertGreaterEqual(merge_observations[0][1], 2, "auto merge follows an exact-head PR seal")
        self.assertGreater(merge_observations[1][1], merge_observations[0][1],
                           "the checked fallback must refresh identity after the auto-merge error")
        for _, _, api_calls in merge_observations:
            self.assertIn(f"repos/{REPO}/actions/runs/101/jobs?filter=latest&per_page=100", api_calls,
                          "neither merge attempt may bypass the successful exact-head ci-ok proof")

    def test_other_chained_command_errors_do_not_get_a_fallback(self):
        for stderr in ["permission denied", "Auto merge is not allowed for this repository",
                       "Pull request is not mergeable", "network request timed out"]:
            with self.subTest(stderr=stderr):
                fake = FormulaHarness(current=[pr(), pr(), self.ready_pr()])

                def errors(args):
                    if args[:2] == ("pr", "merge"):
                        self.command_error(args, stderr)
                    return False

                fake.fail_gh = errors
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(len(fake.merges), 1)
                self.assertIn("--auto", fake.merges[0])

    def test_clean_text_without_chained_called_process_error_is_not_sufficient(self):
        for other_cause in [False, True]:
            with self.subTest(other_cause=other_cause):
                fake = FormulaHarness(current=[pr(), pr(), self.ready_pr()])

                def errors(args):
                    if args[:2] == ("pr", "merge"):
                        if other_cause:
                            raise gate.GateError("gh command failed") from RuntimeError("Pull request is in clean status")
                        raise gate.GateError("Pull request is in clean status")
                    return False

                fake.fail_gh = errors
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(len(fake.merges), 1)

    def test_unmergeable_nonclean_or_unknown_refresh_never_gets_a_fallback(self):
        cases = [dict(mergeable=False), dict(mergeable=None), dict(mergeable_state="blocked"),
                 dict(mergeable_state="dirty"), dict(mergeable_state="unknown"),
                 dict(mergeable_state=None)]
        for updates in cases:
            with self.subTest(updates=updates):
                fake = FormulaHarness(current=[pr(), pr(), self.ready_pr(**updates)])
                fake.fail_gh = self.clean_auto_error
                with self.assertRaises(gate.GateError):
                    fake.invoke()
                self.assertEqual(len(fake.merges), 1)
        fake = FormulaHarness(current=[pr(), pr(), pr()])
        fake.fail_gh = self.clean_auto_error
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(len(fake.merges), 1, "absent mergeability fields are not a clean PR proof")

    def test_head_change_after_auto_merge_error_never_gets_a_fallback(self):
        stale = self.ready_pr()
        stale["head"]["sha"] = "d" * 40
        fake = FormulaHarness(current=[pr(), pr(), stale])
        fake.fail_gh = self.clean_auto_error
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(len(fake.merges), 1)

    def test_refresh_api_failure_after_auto_merge_error_never_gets_a_fallback(self):
        fake = FormulaHarness(current=[pr(), pr(), self.ready_pr()])
        fake.fail_gh = self.clean_auto_error
        fake.fail_api = lambda endpoint: endpoint.endswith("/pulls/17") and fake.pr_reads >= 2
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(len(fake.merges), 1)

    def test_checked_fallback_command_failure_has_no_retry(self):
        fake = FormulaHarness(current=[pr(), pr(), self.ready_pr()])

        def errors(args):
            if args[:2] == ("pr", "merge"):
                if "--auto" in args:
                    return self.clean_auto_error(args)
                self.command_error(args, "permission denied during checked fallback")
            return False

        fake.fail_gh = errors
        with self.assertRaises(gate.GateError):
            fake.invoke()
        self.assertEqual(len(fake.merges), 2, "no third merge attempt after checked-fallback failure")
        self.assertIn("--auto", fake.merges[0])
        self.assertNotIn("--auto", fake.merges[1])
        self.assertIn("--match-head-commit", fake.merges[1])


class WorkflowTests(unittest.TestCase):
    """Static tests intentionally fail old YAML at the aggregate/path-filter assertions."""
    def jobs(self):
        content = (ROOT / ".github/workflows/ci.yml").read_text()
        jobs = content.split("\njobs:\n", 1)[1]
        starts = list(re.finditer(r"(?m)^  ([A-Za-z_][A-Za-z0-9_-]*):\s*$", jobs))
        return content, {match.group(1): jobs[match.end():starts[index + 1].start() if index + 1 < len(starts) else len(jobs)] for index, match in enumerate(starts)}

    def test_ci_ok_always_covers_all_existing_gates_and_decision(self):
        _, jobs = self.jobs()
        self.assertIn("ci-ok", jobs)
        self.assertEqual(set(jobs), {"decision", "lint", "pdf-parity", "test", "ci-ok"},
                         "new gating jobs must be added to ci-ok and its policy")
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
        # The matrix comes from decision.test_os (#575); a ready or non-PR run lists all three.
        self.assertIn("os: ${{ fromJSON(needs.decision.outputs.test_os ||", jobs["test"])
        full = re.search(r"test_os='(\[[^']*\])'", jobs["decision"])
        self.assertIsNotNone(full)
        self.assertEqual(json.loads(full.group(1)), ["ubuntu-latest", "macos-latest", "windows-latest"])
        for platform in ["ubuntu-latest", "macos-latest", "windows-latest"]:
            self.assertIn(f"test ({platform})", jobs["decision"], "expected must keep every test leg, deferred or not")

    def test_pull_request_types_include_ready_for_review(self):
        content, _ = self.jobs()
        events = content.split("\njobs:\n", 1)[0]
        self.assertRegex(events, r"(?m)^  pull_request:\n    types: \[opened, synchronize, reopened, ready_for_review\]$")

    def decision_macos_step(self, event_name, event, run_full="true", draft_verdict=None):
        """Runs the real `Decide the macOS test leg` step with the real ci_gate.py."""
        _, jobs = self.jobs()
        step = jobs["decision"].split("      - name: Decide the macOS test leg\n", 1)[1]
        script = step.split("        run: |\n", 1)[1]
        script = "\n".join(line[10:] if line.startswith("          ") else line for line in script.splitlines())
        with tempfile.TemporaryDirectory() as tmp:
            out, summary, path = Path(tmp) / "out", Path(tmp) / "summary", Path(tmp) / "event.json"
            path.write_text(json.dumps(event))
            env = {**os.environ, "GITHUB_EVENT_NAME": event_name, "GITHUB_EVENT_PATH": str(path),
                   "GITHUB_OUTPUT": str(out), "GITHUB_STEP_SUMMARY": str(summary), "RUN_FULL": run_full}
            result = subprocess.run(["bash", "-c", "cd " + str(ROOT) + "\n" + script], env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            return dict(line.split("=", 1) for line in out.read_text().splitlines())

    def test_decision_drops_macos_only_on_a_full_draft_pr_run(self):
        full = ["ubuntu-latest", "macos-latest", "windows-latest"]
        draft = self.decision_macos_step("pull_request", {"pull_request": {"draft": True}})
        self.assertEqual(draft["macos_deferred"], "true")
        self.assertEqual(json.loads(draft["test_os"]), ["ubuntu-latest", "windows-latest"])
        ready = self.decision_macos_step("pull_request", {"pull_request": {"draft": False}})
        self.assertEqual((ready["macos_deferred"], json.loads(ready["test_os"])), ("false", full))
        unknown = self.decision_macos_step("pull_request", {"pull_request": {}})
        self.assertEqual((unknown["macos_deferred"], json.loads(unknown["test_os"])), ("false", full), "unknown draft state must run macOS")
        push = self.decision_macos_step("push", {})
        self.assertEqual((push["macos_deferred"], json.loads(push["test_os"])), ("false", full))
        # A lightweight draft run has no test legs, so ci-ok must keep its skipped-success rule.
        light = self.decision_macos_step("pull_request", {"pull_request": {"draft": True}}, run_full="false")
        self.assertEqual((light["macos_deferred"], json.loads(light["test_os"])), ("false", full))

    def test_every_check_run_reader_takes_the_latest_run_per_name(self):
        """AC4: a head with a failed draft ci-ok and a later successful ready ci-ok is verified by the latter.

        Runs the real jq filter of every reader (ci.yml decision, release.yml verify-ci x2,
        release-readiness.yml x2) against one check-runs payload that holds both runs.
        """
        if shutil.which("jq") is None:
            # The hosted ubuntu runner ships jq, and ci-ok runs this suite there: it must never
            # skip in CI. Only a local mirror without jq (the dgx02 verify container) may skip.
            if os.environ.get("GITHUB_ACTIONS") == "true":
                self.fail("jq is required to run the real latest-run filters in CI")
            self.skipTest("jq not installed; the hosted CI run enforces this test")
        filters = []
        for path in [".github/workflows/ci.yml", ".github/workflows/release.yml", ".github/workflows/release-readiness.yml"]:
            filters += [(path, m) for m in re.findall(r"""'(\[\.check_runs\[\] \| select\(\.name == \$n\)\][^']*)'""", (ROOT / path).read_text())]
        self.assertEqual([path for path, _ in filters].count(".github/workflows/ci.yml"), 1)
        self.assertEqual([path for path, _ in filters].count(".github/workflows/release.yml"), 2)
        self.assertEqual([path for path, _ in filters].count(".github/workflows/release-readiness.yml"), 2)
        def runs(*items):
            return json.dumps({"check_runs": [{"name": "ci-ok", "status": s, "conclusion": c, "started_at": a, "completed_at": b} for s, c, a, b in items]})
        draft_fail = ("completed", "failure", "2026-11-14T01:00:00Z", "2026-11-14T01:10:00Z")
        ready_pass = ("completed", "success", "2026-11-14T02:00:00Z", "2026-11-14T02:10:00Z")
        ready_pending = ("in_progress", None, "2026-11-14T02:30:00Z", None)
        cases = [
            (runs(draft_fail, ready_pass), "completed/success"),
            (runs(ready_pass, draft_fail), "completed/success"),
            (runs(ready_pass, ready_pending), "in_progress/pending"),
            (runs(ready_pending, ready_pass), "in_progress/pending"),
            # The old completed_at ordering sorted a pending run (null) first, so a newer pending
            # run was hidden behind the older failed draft run.
            (runs(draft_fail, ready_pending), "in_progress/pending"),
            (runs(("completed", "success", "2026-11-14T02:00:00Z", "2026-11-14T02:10:00Z"), ("completed", "failure", "2026-11-14T03:00:00Z", "2026-11-14T03:10:00Z")), "completed/failure"),
        ]
        for path, expression in filters:
            for payload, expected in cases:
                with self.subTest(path=path, expected=expected, payload=payload[:60]):
                    result = subprocess.run(["jq", "-r", "--arg", "n", "ci-ok", expression], input=payload, capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout.strip(), expected)

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

    def test_release_identical_tree_fallback_requires_same_six_green_checks_as_ci(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        prefix = release.split("          set -euo pipefail\n", 1)[1].split("          # Poll until", 1)[0]
        self.assertIn("check_green()", prefix, "exercise the real fallback function and its arrays")
        required = {"lint", "pdf-parity", "test (ubuntu-latest)", "test (macos-latest)", "test (windows-latest)", "ci-ok"}
        _, jobs = self.jobs()
        decision = jobs["decision"]
        expected = re.search(r'expected=\(([^\n]+)\)', decision)
        self.assertIsNotNone(expected)
        self.assertEqual(set(re.findall(r'"([^"\n]+)"', expected.group(1))), required)
        # This shim returns the requested named state directly, without jq or a live API.
        shim = r'''
          gh() { printf '{}\n'; }
          jq() {
            local n=""
            while [ "$#" -gt 0 ]; do
              if [ "$1" = "--arg" ] && [ "$2" = "n" ]; then n="$3"; break; fi
              shift
            done
            printf '%s\n' "$n" >> "$CHECK_NAMES"
            if [ "$n" = "$FAILED_NAME" ]; then printf '%s\n' "$FAILED_STATE";
            else printf 'completed/success\n'; fi
          }
'''
        cases = [("", "", True)] + [(name, state, False) for name in sorted(required)
                                                    for state in ["absent", "completed/skipped", "completed/failure"]]
        for failed_name, failed_state, green in cases:
            with self.subTest(name=failed_name, state=failed_state), tempfile.TemporaryDirectory() as tmp:
                log = Path(tmp) / "checks"
                env = {**os.environ, "CHECK_NAMES": str(log), "FAILED_NAME": failed_name,
                       "FAILED_STATE": failed_state, "GITHUB_REPOSITORY": REPO}
                script = "set -euo pipefail\n" + shim + prefix + '\ncheck_green "' + HEAD + '"\n'
                result = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True, timeout=3)
                self.assertEqual(result.returncode == 0, green, result.stdout + result.stderr)
                self.assertEqual(set(log.read_text().splitlines()), required,
                                 "an aggregate alone cannot prove that the identical tree ran the heavy checks")

    def test_decision_malformed_jq_falls_back_to_full_without_aborting(self):
        _, jobs = self.jobs()
        # Only the dedupe step: the macOS step that follows it is exercised by its own test.
        run_block = jobs["decision"].split("        run: |\n", 1)[1].split("\n      # #575:", 1)[0]
        script = "\n".join(line[10:] if line.startswith("          ") else line for line in run_block.splitlines())
        # Execute the actual decision shell with PATH shims; no Python policy, git or gh is real.
        for stage in ["selection", "number", "head", "state"]:
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                shims = {
                    "python3": "printf 'full\\n'\n",
                    "git": "printf 'unneeded git shim called\\n' >&2; exit 90\n",
                    "gh": "case \"$*\" in *'/pulls?'*) printf '[]\\n';; *'/git/commits/'*) printf 'tree\\n';; *'/check-runs?'*) printf '{}\\n';; *) exit 91;; esac\n",
                    "jq": r'''
case "$*" in
  *'sort_by(.number)'*) key=selection; output='{"number":17,"head":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}';;
  *'.check_runs'*) key=state; output=completed/success;;
  *'.head.sha'*) key=head; output=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;;
  *'.number'*) key=number; output=17;;
  *) exit 92;;
esac
if [ "$FAIL_JQ_STAGE" = "$key" ]; then printf 'malformed JSON fixture\n' >&2; exit 7; fi
printf '%s\n' "$output"
''',
                }
                for name, body in shims.items():
                    executable = directory / name
                    executable.write_text("#!/bin/bash\n" + body)
                    executable.chmod(0o700)
                output, summary = directory / "output", directory / "summary"
                env = {**os.environ, "PATH": str(directory) + os.pathsep + os.environ.get("PATH", ""),
                       "FAIL_JQ_STAGE": stage, "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": HEAD,
                       "GITHUB_REPOSITORY": REPO, "GITHUB_OUTPUT": str(output),
                       "GITHUB_STEP_SUMMARY": str(summary)}
                result = subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True, timeout=3)
                self.assertEqual(result.returncode, 0, "malformed API proof must run full CI: " + result.stderr)
                self.assertEqual(output.read_text().splitlines(), ["run_full=true", "skip_kind=none"])

    def test_readiness_notes_distinguish_manual_full_ci_from_lightweight_aggregate(self):
        readiness = (ROOT / ".github/workflows/release-readiness.yml").read_text()
        self.assertRegex(readiness, r"(?is)#.*ordinary.*workflow_dispatch.*full")
        self.assertRegex(readiness, r"(?is)#.*ci-ok.*(?:lightweight|skip)")

    def test_release_formula_calls_gate_without_unchecked_merge(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        update = release.split("\n  update-formula:\n", 1)[1]
        self.assertIn("ci_gate.py formula", update)
        self.assertNotRegex(update, r"gh\s+pr\s+merge")


if __name__ == "__main__":
    unittest.main()
