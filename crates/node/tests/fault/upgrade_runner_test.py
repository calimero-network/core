"""Unit tests of the upgrade-path runner itself, with no cluster:

    python3 -I crates/node/tests/fault/upgrade_runner_test.py
"""

from __future__ import annotations

import ast
import contextlib
import io
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rollout  # noqa: E402
import rv2  # noqa: E402
import upgrade_deep  # noqa: E402
import upgrade_path  # noqa: E402


def world(jobs):
    return upgrade_path.World(None, None, {}, upgrade_path.say, jobs=jobs)


def fake(name, body=None, group="", exclusive=False, step="seed"):
    """A case whose `step` runs `body(world)`."""
    attrs = {"name": name, "group": group, "exclusive": exclusive, step: lambda self, w: body and body(w)}
    return type(f"Fake_{name}", (upgrade_path.Case,), attrs)()


def run(cases, jobs, step="seed", fatal=False):
    """(results, what the run printed, whether it raised) of `step` over `cases`."""
    out, results, raised = io.StringIO(), [], None
    with contextlib.redirect_stdout(out):
        try:
            upgrade_path.run_step(world(jobs), cases, step, results, fatal=fatal)
        except upgrade_path.Failed as e:
            raised = str(e)
    return results, out.getvalue(), raised


class Spans:
    """When each case ran, from inside it."""

    def __init__(self):
        self.span, self.lock = {}, threading.Lock()

    def case(self, name, seconds=0.2, **kw):
        def body(w):
            start = time.time()
            time.sleep(seconds)
            with self.lock:
                self.span[name] = (start, time.time())
        return fake(name, body, **kw)

    def overlap(self, a, b):
        (a0, a1), (b0, b1) = self.span[a], self.span[b]
        return a0 < b1 and b0 < a1


class Concurrency(unittest.TestCase):
    def test_cases_of_a_phase_run_at_once_up_to_the_jobs(self):
        sp = Spans()
        t0 = time.time()
        run([sp.case(f"c{i}", 0.3) for i in range(4)], jobs=4)
        self.assertLess(time.time() - t0, 0.9)
        self.assertTrue(sp.overlap("c0", "c3"))

    def test_one_job_runs_them_one_by_one_in_order(self):
        sp = Spans()
        run([sp.case(f"c{i}", 0.05) for i in range(3)], jobs=1)
        self.assertFalse(sp.overlap("c0", "c1"))
        self.assertEqual(sorted(sp.span, key=lambda n: sp.span[n][0]), ["c0", "c1", "c2"])

    def test_the_jobs_bound_the_cases_at_once(self):
        now, peak, lock = [0], [0], threading.Lock()
        def body(w):
            with lock:
                now[0] += 1
                peak[0] = max(peak[0], now[0])
            time.sleep(0.1)
            with lock:
                now[0] -= 1
        run([fake(f"c{i}", body) for i in range(6)], jobs=2)
        self.assertEqual(peak[0], 2)

    def test_a_phase_can_take_fewer_jobs_than_the_world(self):
        now, peak, lock = [0], [0], threading.Lock()
        def body(w):
            with lock:
                now[0] += 1
                peak[0] = max(peak[0], now[0])
            time.sleep(0.05)
            with lock:
                now[0] -= 1
        with contextlib.redirect_stdout(io.StringIO()):
            upgrade_path.run_step(world(8), [fake(f"c{i}", body) for i in range(6)], "seed", [], jobs=3)
        self.assertEqual(peak[0], 3)

    def test_a_group_runs_in_order_and_never_at_once(self):
        sp = Spans()
        run([sp.case("a1", 0.1, group="a"), sp.case("a2", 0.1, group="a"), sp.case("b", 0.1)], jobs=4)
        self.assertFalse(sp.overlap("a1", "a2"))
        self.assertLess(sp.span["a1"][1], sp.span["a2"][0] + 1e-3)
        self.assertTrue(sp.overlap("a1", "b"))

    def test_an_exclusive_case_runs_alone_after_the_others(self):
        sp = Spans()
        run([sp.case("alone", 0.1, exclusive=True), sp.case("x", 0.2), sp.case("y", 0.2)], jobs=4)
        self.assertFalse(sp.overlap("alone", "x") or sp.overlap("alone", "y"))
        self.assertGreaterEqual(sp.span["alone"][0], max(sp.span["x"][1], sp.span["y"][1]) - 1e-3)

    def test_results_come_back_in_case_order(self):
        results, _, _ = run([fake("slow", lambda w: time.sleep(0.2)), fake("fast")], jobs=2)
        self.assertEqual([r["case"] for r in results], ["slow", "fast"])


class Reporting(unittest.TestCase):
    def test_a_failure_names_its_case_and_phase_and_the_others_still_run(self):
        def boom(w):
            raise upgrade_path.Failed("the roles differ")
        results, out, raised = run([fake("good"), fake("bad", boom), fake("also-good")], jobs=3)
        self.assertEqual({r["case"]: r["status"] for r in results}, {"good": "pass", "bad": "fail", "also-good": "pass"})
        self.assertIn("FAIL bad.seed (seed) in", out)
        self.assertIn("the roles differ", out)
        self.assertIsNone(raised)

    def test_a_case_log_is_printed_whole(self):
        def chatty(name):
            def body(w):
                for i in range(4):
                    w.note(f"{name} line {i}")
                    time.sleep(0.02)
            return fake(name, body)
        _, out, _ = run([chatty("p"), chatty("q"), chatty("r")], jobs=3)
        lines = [l.strip() for l in out.splitlines() if " line " in l]
        for name in "pqr":
            at = [i for i, l in enumerate(lines) if l.startswith(f"{name} ")]
            self.assertEqual(at, list(range(at[0], at[0] + 4)), lines)

    def test_a_fatal_step_stops_the_run_naming_the_case(self):
        ran = []
        def boom(w):
            raise upgrade_path.Failed("no seed")
        _, out, raised = run([fake("first", boom), fake("second", lambda w: ran.append(1))], jobs=1, fatal=True)
        self.assertIn("first.seed failed, so the run stops: Failed: no seed", raised)
        self.assertEqual(ran, [])

    def test_an_unexpected_error_prints_its_traceback_with_the_case(self):
        def broken(w):
            {}["missing"]
        _, out, _ = run([fake("odd", broken)], jobs=2)
        self.assertIn("KeyError", out)
        self.assertIn("FAIL odd.seed (seed)", out)


class Names(unittest.TestCase):
    def claims(self, a, b, group_a="", group_b=""):
        def claim(name, wait):
            def body(w):
                time.sleep(wait)
                w.claim(name)
            return body
        results, out, _ = run([fake("one", claim(a, 0), group=group_a), fake("two", claim(b, 0.1), group=group_b)], jobs=2)
        return {r["case"]: r for r in results}, out

    def test_two_cases_claiming_one_name_fail_naming_both(self):
        results, _ = self.claims("rob", "rob")
        self.assertEqual(results["one"]["status"], "pass")
        self.assertEqual(results["two"]["status"], "fail")
        self.assertRegex(results["two"]["error"], r"'rob' is used by both one and two")

    def test_different_names_do_not_collide(self):
        results, _ = self.claims("rob", "ron")
        self.assertEqual({r["status"] for r in results.values()}, {"pass"})

    def test_cases_of_one_group_may_share_a_name(self):
        results, _ = self.claims("rob", "rob", "g", "g")
        self.assertEqual({r["status"] for r in results.values()}, {"pass"})


def literal_names_by_case():
    """account name -> the cases (or groups) whose class bodies name it in `person`, `people` or `account`."""
    users: dict[str, set] = {}
    for path in sorted((Path(__file__).resolve().parent / "upgrade_cases").glob("[0-9]*.py")):
        for cls in [n for n in ast.parse(path.read_text()).body if isinstance(n, ast.ClassDef)]:
            group = next((n.value.value for n in cls.body if isinstance(n, ast.Assign)
                          and any(getattr(t, "id", "") == "group" for t in n.targets)), "")
            for call in [n for n in ast.walk(cls) if isinstance(n, ast.Call)]:
                f = call.func
                if getattr(f, "id", getattr(f, "attr", "")) in ("person", "people", "account"):
                    for a in call.args:
                        if isinstance(a, ast.Constant) and isinstance(a.value, str):
                            users.setdefault(a.value, set()).add(group or f"{path.name}:{cls.name}")
    return users


class CaseNames(unittest.TestCase):
    def test_no_two_cases_name_one_account(self):
        shared = {n: sorted(c) for n, c in literal_names_by_case().items() if len(c) > 1}
        self.assertEqual(shared, {}, "cases run at once: give each its own account names")


class FakeFleet:
    """`placement` and `operate("move")` of a `Fleet`, over apps on nodes 1-3 and new nodes 4-6."""

    def __init__(self, apps, failing=()):
        self.replicas = {a: [1, 2, 3] for a in apps}
        self.failing = set(failing)
        self.lock = threading.Lock()
        self.inflight = self.peak = 0

    def placement(self):
        with self.lock:
            return {"apps": [{"app": a, "replicas": list(r)} for a, r in self.replicas.items()],
                    "nodes": [{"id": i, "apps": 0} for i in range(1, 7)]}

    def operate(self, *args):
        _, app, _, target = args
        with self.lock:
            self.inflight += 1
            self.peak = max(self.peak, self.inflight)
        try:
            time.sleep(0.05)
            if app in self.failing:
                raise rv2.CliError(args, "refused\n")
            with self.lock:
                self.replicas[app] = [int(n) for n in target.split(",")]
        finally:
            with self.lock:
                self.inflight -= 1

    def on_new(self):
        return sorted(a for a, r in self.replicas.items() if set(r) <= {4, 5, 6})


class MoveApps(unittest.TestCase):
    def move(self, fleet, jobs, mixed=None):
        return rollout.move_apps(fleet, {1, 2, 3}, [4, 5, 6], lambda s: None, mixed, jobs=jobs)

    def test_the_first_app_moves_alone_then_the_mixed_checks_run_then_the_rest_move_at_once(self):
        fleet = FakeFleet([f"a{i:02}" for i in range(12)])
        seen = []
        moved = self.move(fleet, 4, lambda: seen.append((fleet.on_new(), fleet.inflight)))
        self.assertEqual(seen, [(["a00"], 0)])
        self.assertEqual(sorted(moved), sorted(fleet.replicas))
        self.assertEqual(len(fleet.on_new()), 12)
        self.assertEqual(fleet.peak, 4)

    def test_one_job_moves_them_one_by_one(self):
        fleet = FakeFleet([f"a{i}" for i in range(5)])
        self.move(fleet, 1)
        self.assertEqual(fleet.peak, 1)
        self.assertEqual(len(fleet.on_new()), 5)

    def test_a_refused_move_fails_the_roll_naming_the_app_after_the_others_moved(self):
        fleet = FakeFleet(["a0", "a1", "a2", "a3"], failing=["a2"])
        with mock.patch.object(rollout, "MOVE_TIMEOUT_S", 0.3), self.assertRaisesRegex(rollout.RolloutError, "a2"):
            self.move(fleet, 4)
        self.assertEqual(fleet.on_new(), ["a0", "a1", "a3"])

    def test_with_no_app_to_move_the_mixed_checks_still_run_once(self):
        ran = []
        self.move(FakeFleet([]), 4, lambda: ran.append(1))
        self.assertEqual(ran, [1])

    def test_each_choice_counts_toward_the_load_of_the_next(self):
        apps = [{"app": f"a{i}", "replicas": [1, 2, 3]} for i in range(4)]
        placement = {"nodes": [{"id": i, "apps": 0} for i in range(1, 8)]}
        plan = rollout._plan(apps, [4, 5, 6, 7], placement)
        loads = [sum(n in target for _, target in plan) for n in (4, 5, 6, 7)]
        self.assertEqual(loads, [3, 3, 3, 3])


class ThisTree(unittest.TestCase):
    def test_the_build_is_waited_for_once_however_many_cases_ask(self):
        calls = []
        w = upgrade_path.World(None, None, {}, upgrade_path.say, build=lambda: calls.append(1))
        w.this_tree()
        w.this_tree()
        self.assertEqual(calls, [1])

    def test_a_failed_build_fails_every_case_that_needs_it(self):
        def failed():
            raise upgrade_path.Failed("this tree's build failed (exit 101)")
        w = upgrade_path.World(None, None, {}, upgrade_path.say, build=failed)
        for _ in range(2):
            with self.assertRaisesRegex(upgrade_path.Failed, "exit 101"):
                w.this_tree()

    def test_a_tree_built_already_needs_no_wait(self):
        upgrade_path.World(None, None, {}, upgrade_path.say).this_tree()


class WaitForBuild(unittest.TestCase):
    def status(self, text=None):
        path = Path(tempfile.mkdtemp()) / "build.status"
        if text is not None:
            path.write_text(text)
        return path

    def test_a_build_that_exits_zero_lets_the_roll_go_on(self):
        upgrade_path.wait_for_build(self.status("0\n"), timeout=1)

    def test_a_failed_build_fails_the_run_with_its_exit_status(self):
        with self.assertRaisesRegex(upgrade_path.Failed, r"build failed \(exit 101\)"):
            upgrade_path.wait_for_build(self.status("101\n"), timeout=1)

    def test_a_build_that_never_ends_fails_the_run(self):
        with self.assertRaisesRegex(upgrade_path.Failed, "did not end within"):
            upgrade_path.wait_for_build(self.status(), timeout=0.1)


class ViewRetry(unittest.TestCase):
    def person(self, outcomes):
        p = upgrade_deep.Person.__new__(upgrade_deep.Person)
        calls = iter(outcomes)

        def http(*a, **kw):
            o = next(calls)
            if isinstance(o, Exception):
                raise o
            return o

        p.http = http
        return p

    def test_timeout_is_retried(self):
        timeout = rv2.HttpError(503, {"error": "TIMEOUT"})
        with mock.patch.object(upgrade_deep.time, "sleep"):
            self.assertEqual(self.person([timeout, {"result": 1}]).view("app", "m"), {"result": 1})

    def test_other_refusal_is_not_retried(self):
        refused = rv2.HttpError(503, {"error": "CLUSTER_NOT_UPGRADED"})
        with self.assertRaises(rv2.HttpError):
            self.person([refused, {"result": 1}]).view("app", "m")

    def test_gives_up_after_the_tries(self):
        timeout = rv2.HttpError(503, {"error": "TIMEOUT"})
        with mock.patch.object(upgrade_deep.time, "sleep"), self.assertRaises(rv2.HttpError):
            self.person([timeout] * upgrade_deep.VIEW_TRIES).view("app", "m")


if __name__ == "__main__":
    unittest.main()
