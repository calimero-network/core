#!/usr/bin/env python3
"""Tests for e2e-scenario-groups.py, against the repository's own manifest.

The grouping fails silently in both directions: a scenario left out of every
group simply never runs, and one placed twice runs twice while looking fine.
Each case pins one property the e2e workflow relies on.

    python3 scripts/tests/e2e-scenario-groups-test.py
"""

import copy
import importlib.util
import json
import random
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPT = ROOT / "scripts" / "e2e-scenario-groups.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree

spec = importlib.util.spec_from_file_location("e2e_scenario_groups", SCRIPT)
g = importlib.util.module_from_spec(spec)
spec.loader.exec_module(g)

MANIFEST = g.load_manifest(ROOT / g.DEFAULT_MANIFEST)
DURATIONS = g.load_durations(ROOT / g.DEFAULT_DURATIONS)
GROUPS = g.plan(MANIFEST, DURATIONS)

failures = []


def case(name):
    def register(fn):
        try:
            fn()
            print(f"ok   {name}")
        except AssertionError as err:
            failures.append(name)
            print(f"FAIL {name}: {err}")
        return fn
    return register


def placed(groups):
    return [s["workflow"] for grp in groups for s in grp["scenarios"]]


def partition(groups):
    return sorted(sorted(s["workflow"] for s in grp["scenarios"]) for grp in groups)


@case("every scenario is in exactly one group")
def _():
    names = placed(GROUPS)
    expected = [s["workflow"] for s in MANIFEST]
    assert sorted(names) == sorted(expected), (
        f"missing {sorted(set(expected) - set(names))}, extra {sorted(set(names) - set(expected))}"
    )
    assert len(names) == len(set(names)), "a scenario is in two groups"


@case("the group count is GROUP_COUNT, and no group is empty")
def _():
    assert len(GROUPS) == g.GROUP_COUNT, f"{len(GROUPS)} groups"
    assert all(grp["scenarios"] for grp in GROUPS)
    assert len({grp["group"] for grp in GROUPS}) == len(GROUPS), "two groups share a name"


@case("each group runs one image, the one its scenarios declare")
def _():
    for grp in GROUPS:
        assert {s["image"] for s in grp["scenarios"]} == {grp["image"]}, grp["group"]


@case("every per-scenario field reaches the runner unchanged")
def _():
    planned = {s["workflow"]: s for grp in GROUPS for s in grp["scenarios"]}
    for scenario in MANIFEST:
        got = planned[scenario["workflow"]]
        for field in ("file", "app", "image", "registry_fetch"):
            assert got[field] == scenario[field], f"{scenario['workflow']}.{field}"


@case("the manifest keeps the raw fields the matrix had, defaults included")
def _():
    import yaml

    raw = yaml.safe_load((ROOT / g.DEFAULT_MANIFEST).read_text())["scenarios"]
    planned = {s["workflow"]: s for grp in GROUPS for s in grp["scenarios"]}
    for entry in raw:
        got = planned[entry["workflow"]]
        assert got["image"] == entry.get("image", "merod:local"), entry["workflow"]
        assert got["registry_fetch"] == entry.get("registry_fetch", "true"), entry["workflow"]


@case("every manifest entry names a scenario file that exists")
def _():
    missing = [
        f"apps/{s['app']}/{s['file']}"
        for s in MANIFEST
        if not (ROOT / "apps" / s["app"] / s["file"]).is_file()
    ]
    assert not missing, missing


@case("the durations file names only scenarios the manifest has")
def _():
    stale = sorted(set(DURATIONS) - {s["workflow"] for s in MANIFEST})
    assert not stale, f"stale durations: {stale}"


@case("the same inputs give byte-identical output, from the CLI too")
def _():
    assert json.dumps(g.plan(MANIFEST, DURATIONS)) == json.dumps(GROUPS)
    runs = [
        subprocess.run([sys.executable, str(SCRIPT)], capture_output=True, text=True, check=True).stdout
        for _ in range(2)
    ]
    assert runs[0] == runs[1], "two CLI runs differ"
    assert json.loads(runs[0]) == {"include": GROUPS}


@case("every image runs in exactly one test job, and verify refuses one that does not")
def _():
    assert set(g.JOBS) == set(g.IMAGES), f"JOBS {sorted(g.JOBS)} vs IMAGES {sorted(g.IMAGES)}"
    saved = dict(g.JOBS)
    try:
        del g.JOBS["merod:local-dht"]
        try:
            g.verify(MANIFEST, GROUPS)
        except g.ManifestError:
            return
        raise AssertionError("verify accepted an image no test job runs")
    finally:
        g.JOBS.clear()
        g.JOBS.update(saved)


@case("the test jobs together run every group exactly once, from the CLI too")
def _():
    jobs = sorted(set(g.JOBS.values()))
    split = [grp for job in jobs for grp in g.for_job(GROUPS, job)]
    names = [grp["group"] for grp in split]
    assert len(names) == len(set(names)), "a group is in two jobs"
    assert sorted(names) == sorted(grp["group"] for grp in GROUPS), "a group is in no job"
    assert sorted(placed(split)) == sorted(placed(GROUPS))
    for job in jobs:
        out = subprocess.run(
            [sys.executable, str(SCRIPT), "--job", job], capture_output=True, text=True, check=True
        ).stdout
        assert json.loads(out) == {"include": g.for_job(GROUPS, job)}, job
        assert all(g.JOBS[grp["image"]] == job for grp in g.for_job(GROUPS, job)), job


@case("an unknown job, or one with no groups, is refused")
def _():
    for job, groups in (("nope", GROUPS), ("mock-tee", [grp for grp in GROUPS if grp["image"] != "merod:local-mock-tee"])):
        try:
            g.for_job(groups, job)
        except g.ManifestError:
            continue
        raise AssertionError(f"for_job accepted {job!r}")
    bad = subprocess.run([sys.executable, str(SCRIPT), "--job", "nope"], capture_output=True, text=True)
    assert bad.returncode != 0 and not bad.stdout, "the CLI printed a matrix for an unknown job"


@case("the e2e workflow plans every job and runs each one's groups in exactly one test job")
def _():
    import yaml

    wf = yaml.safe_load((ROOT / ".github" / "workflows" / "e2e-rust-apps.yml").read_text())
    jobs = wf["jobs"]
    plan_run = "\n".join(step.get("run", "") for step in jobs["plan"]["steps"])
    outputs = jobs["plan"]["outputs"]
    for job in sorted(set(g.JOBS.values())):
        assert f"--job {job}" in plan_run, f"the plan step never runs --job {job}"
        assert f"matrix-{job}=" in plan_run, f"the plan step never writes matrix-{job}"
        assert f"matrix-{job}" in outputs, f"plan does not output matrix-{job}"
        matrix = f"${{{{ fromJSON(needs.plan.outputs.matrix-{job}) }}}}"
        runners = [
            name for name, body in jobs.items()
            if (body.get("strategy") or {}).get("matrix") == matrix
        ]
        assert len(runners) == 1, f"matrix-{job} is run by {runners}, not exactly one job"
        assert "scripts/e2e-run-scenario-group.sh" in "\n".join(
            step.get("run", "") for step in jobs[runners[0]]["steps"]
        ), f"{runners[0]} does not run the groups"
    # Anything that reads every group's logs must wait for every job that writes them.
    runners = {
        name for name, body in jobs.items()
        if "needs.plan.outputs.matrix" in str((body.get("strategy") or {}).get("matrix", ""))
    }
    for name, body in jobs.items():
        steps = body.get("steps") or []
        if any((step.get("with") or {}).get("pattern") == "logs-*" for step in steps):
            needs = body.get("needs")
            needs = {needs} if isinstance(needs, str) else set(needs or [])
            assert runners <= needs, f"{name} reads every group's logs but needs only {sorted(needs)}"


@case("which scenarios share a group does not depend on manifest order")
def _():
    shuffled = copy.deepcopy(MANIFEST)
    random.Random(7).shuffle(shuffled)
    assert partition(g.plan(shuffled, DURATIONS)) == partition(GROUPS)


@case("within a group, scenarios keep manifest order")
def _():
    order = {s["workflow"]: n for n, s in enumerate(MANIFEST)}
    for grp in GROUPS:
        positions = [order[s["workflow"]] for s in grp["scenarios"]]
        assert positions == sorted(positions), grp["group"]


@case("an image's longest scenarios land in different groups")
def _():
    for image in g.IMAGES:
        groups = [grp for grp in GROUPS if grp["image"] == image]
        members = [s for s in MANIFEST if s["image"] == image]
        if not groups:
            continue
        longest = sorted(members, key=lambda s: -DURATIONS.get(s["workflow"], g.DEFAULT_SECONDS))
        top = {s["workflow"] for s in longest[: len(groups)]}
        homes = {grp["group"] for grp in groups for s in grp["scenarios"] if s["workflow"] in top}
        assert len(homes) == len(top), f"{image}: {sorted(top)} share a group"


@case("the groups are balanced: none runs past 1.25x the mean or its one longest scenario")
def _():
    loads = [grp["estimated_seconds"] for grp in GROUPS]
    mean = sum(loads) / len(loads)
    longest = max(DURATIONS.get(s["workflow"], g.DEFAULT_SECONDS) for s in MANIFEST)
    assert max(loads) <= max(1.25 * mean, longest + g.DEFAULT_SECONDS), loads


@case("a hung scenario times out alone, inside its group's timeout")
def _():
    for grp in GROUPS:
        total = 0
        for s in grp["scenarios"]:
            est = DURATIONS.get(s["workflow"], g.DEFAULT_SECONDS)
            assert s["timeout_seconds"] >= max(g.SCENARIO_TIMEOUT_FLOOR_SECONDS, 3 * est), s["workflow"]
            total += s["timeout_seconds"]
        assert grp["timeout_minutes"] * 60 >= total + g.GROUP_SETUP_ALLOWANCE_SECONDS, grp["group"]


@case("the one-scenario setup steps run in exactly the groups that need them")
def _():
    for grp in GROUPS:
        names = [s["workflow"] for s in grp["scenarios"]]
        assert grp["setup_node"] == any(n.startswith("ephemeral-") for n in names), grp["group"]
        assert grp["blob_fixtures"] == ("blob-cross-node-sizes" in names), grp["group"]
    assert sum(grp["setup_node"] for grp in GROUPS) >= 1
    assert sum(grp["blob_fixtures"] for grp in GROUPS) == 1


@case("a scenario with no measured duration is still placed, as DEFAULT_SECONDS")
def _():
    extra = MANIFEST + [{
        "workflow": "not-yet-timed", "file": "workflows/x.yml", "app": "kv-store",
        "image": "merod:local", "registry_fetch": "true",
    }]
    groups = g.plan(extra, DURATIONS)
    assert "not-yet-timed" in placed(groups)
    assert len(placed(groups)) == len(extra)


@case("a grouping that drops or repeats a scenario is refused")
def _():
    dropped = copy.deepcopy(GROUPS)
    dropped[0]["scenarios"].pop()
    repeated = copy.deepcopy(GROUPS)
    repeated[1]["scenarios"].append(repeated[0]["scenarios"][0])
    for broken in (dropped, repeated):
        try:
            g.verify(MANIFEST, broken)
        except g.ManifestError:
            continue
        raise AssertionError("verify accepted a broken grouping")


@case("the manifest refuses what the runner would silently ignore")
def _():
    good = {"workflow": "a", "file": "workflows/a.yml", "app": "kv-store"}
    bad = {
        "unknown field": [dict(good, retries=2)],
        "duplicate name": [good, dict(good, file="workflows/b.yml")],
        "duplicate file": [good, dict(good, workflow="b")],
        "unknown image": [dict(good, image="merod:latest")],
        "registry_fetch not \"false\"": [dict(good, registry_fetch=False)],
        "missing app": [{"workflow": "a", "file": "workflows/a.yml"}],
    }
    import yaml

    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "m.yml"
        path.write_text(yaml.safe_dump({"scenarios": [good]}))
        assert g.load_manifest(path)[0]["image"] == "merod:local"
        for label, entries in bad.items():
            path.write_text(yaml.safe_dump({"scenarios": entries}))
            try:
                g.load_manifest(path)
            except g.ManifestError:
                continue
            raise AssertionError(f"accepted a manifest with {label}")


if failures:
    print(f"\n{len(failures)} failing case(s)")
    sys.exit(1)
print(f"\nall cases pass: {len(MANIFEST)} scenarios in {len(GROUPS)} groups")
