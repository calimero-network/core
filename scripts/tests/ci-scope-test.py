#!/usr/bin/env python3
"""ci-scope.py against this repository's graph, and every scripts/tests/ file run by a workflow.
A wrong scope lands a change its gate never saw; a test no workflow runs guards nothing."""

import fnmatch
import importlib.util
import re
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "ci-scope.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
ALL = {"rust", "upgrade_path", "standalone_apps"}
R = {"rust"}
RU = {"rust", "upgrade_path"}
RA = {"rust", "standalone_apps"}

CASES = [
    # (name, changed paths, ci-checks jobs that must run; every other job must skip)
    ("a node-only change skips the standalone apps", ["crates/node/src/lib.rs"], RU),
    ("merod runs the upgrade path, not the standalone apps", ["crates/merod/src/main.rs", "crates/server/src/lib.rs"], RU),
    ("the upgrade path's test runs the upgrade path", ["crates/node/tests/fault/upgrade_path.py"], RU),
    ("the pinned release runs the upgrade path", [".github/upgrade-from"], RU),
    ("the release fetcher runs the upgrade path (ci-checks.yml runs it)", ["scripts/fetch-release.sh"], RU),
    ("a tool no node links skips the upgrade path", ["crates/search/bench/src/main.rs"], R),
    ("the component SDK runs the workspace jobs and the upgrade path (this tree's kv)", ["crates/sdk/src/lib.rs"], ALL),
    ("a component app runs the workspace jobs and the standalone apps", ["apps/kv-store/src/lib.rs"], RA),
    ("a plugin crate runs the workspace jobs", ["sdks/plugins/connectors/nearai/src/lib.rs"], R),
    ("a lint script runs the workspace jobs", ["scripts/check-naming.sh"], R),
    ("the guest builder runs every job (the apps' tests build with it)", ["crates/app-build/src/lib.rs"], ALL),
    ("the app engine runs the standalone apps (their harness runs on it)", ["crates/app-engine-wasmtime/src/lib.rs"], ALL),
    ("a file no job reads runs nothing", ["scripts/profiling/collect.sh", ".github/workflows/fuzz.yml"], set()),
    ("a crate README is inert", ["crates/meroctl/README.md"], set()),
    ("AGENTS.md and its CLAUDE.md link are inert", ["crates/log/AGENTS.md", "crates/node/CLAUDE.md"], set()),
    ("site docs are inert", ["docs/src/content/docs/protocol/sync.mdx"], set()),
    ("the OpenAPI document a node test checks runs the workspace jobs", ["docs/public/openapi.json"], R),
    ("an app's guide is inert unless code reads it", ["apps/kv-store/GUIDE.md"], set()),
    ("docs beside code do not hide the code", ["crates/log/AGENTS.md", "crates/log/src/lib.rs"], RU),
    ("the lockfile reaches every job", ["Cargo.lock"], ALL),
    ("the root manifest reaches every job", ["Cargo.toml"], ALL),
    ("the workflow reaches every job", [".github/workflows/ci-checks.yml"], ALL),
    ("this script reaches every job", ["scripts/ci-scope.py"], ALL),
    ("a deleted file in a crate still counts", ["crates/log/src/this-file-was-deleted.rs"], RU),
    ("no input fails closed", [], ALL),
]

EVERY = ""  # an empty filter runs every test
FILTER_CASES = [
    # (name, changed paths, the `test_filter` printed: the tests a PR's `Rust tests` runs)
    ("a crate runs its tests and its dependents'", ["crates/log/src/lib.rs"], "rdeps(=calimero-log)"),
    ("a crate's own test file counts as the crate", ["crates/node/tests/it/rooms.rs"], "rdeps(=calimero-node)"),
    ("two crates run both", ["crates/meroctl/src/main.rs", "crates/log/src/lib.rs"], "rdeps(=calimero-log)|rdeps(=meroctl)"),
    ("a deleted file counts as its crate", ["crates/log/src/this-file-was-deleted.rs"], "rdeps(=calimero-log)"),
    ("docs beside code do not widen it", ["crates/log/AGENTS.md", "crates/log/src/lib.rs"], "rdeps(=calimero-log)"),
    ("a crate the guests compile from runs every test", ["crates/sdk/src/lib.rs"], EVERY),
    ("a dependency of the guests runs every test", ["crates/primitives/src/lib.rs"], EVERY),
    ("an app runs every test (tests build and read apps/)", ["apps/kv-store/src/lib.rs"], EVERY),
    ("the OpenAPI document a node test reads runs every test", ["docs/public/openapi.json"], EVERY),
    ("a script runs every test", ["scripts/check-naming.sh"], EVERY),
    ("the lockfile runs every test", ["Cargo.lock"], EVERY),
    ("a crate plus a file outside crates runs every test", ["crates/log/src/lib.rs", "apps/kv-store/src/lib.rs"], EVERY),
    ("no input fails closed", [], EVERY),
]

def run(paths, *args):
    out = subprocess.run(
        [sys.executable, str(SCRIPT), *args], input="".join(p + "\n" for p in paths),
        capture_output=True, text=True, check=True,
    ).stdout
    verdicts = dict(line.split("=", 1) for line in out.split())
    selected = verdicts.pop("test_filter", None)
    return {job for job, value in verdicts.items() if value == "true"}, set(verdicts), selected


def scripts_ci_checks_runs():
    """Every scripts/ path ci-checks.yml names: each is an input of the `rust` verdict."""
    text = (SCRIPT.parent.parent / ".github" / "workflows" / "ci-checks.yml").read_text()
    return sorted({m.removeprefix("./") for m in re.findall(r"(?:\./)?scripts/[\w./-]+", text)})


def script_tests_no_workflow_runs():
    """Each scripts/tests/ file no workflow names: a test CI never runs guards nothing."""
    workflows = SCRIPT.parent.parent / ".github" / "workflows"
    text = "".join(p.read_text() for p in sorted(workflows.glob("*.yml")))
    tests = sorted(p for p in (SCRIPT.parent / "tests").iterdir() if p.is_file())
    return [f"scripts/tests/{p.name}" for p in tests if f"scripts/tests/{p.name}" not in text]


def main():
    failures = 0
    for name, paths, want in CASES:
        runs, jobs, _ = run(paths)
        if jobs != ALL:
            print(f"FAIL {name}: reported jobs {sorted(jobs)}, expected {sorted(ALL)}")
            failures += 1
        elif runs != want:
            print(f"FAIL {name}: runs {sorted(runs)}, expected {sorted(want)}")
            failures += 1
        else:
            print(f"ok   {name}")
    for name, paths, want in FILTER_CASES:
        _, _, selected = run(paths)
        if selected != want:
            print(f"FAIL {name}: test_filter={selected!r}, expected {want!r}")
            failures += 1
        else:
            print(f"ok   {name}")
    spec = importlib.util.spec_from_file_location("ci_scope", SCRIPT)
    ci_scope = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(ci_scope)
    inputs = ci_scope.WORKFLOWS["ci-checks"][1]
    for script in scripts_ci_checks_runs():
        if not any(fnmatch.fnmatchcase(script, g) for g in inputs):
            print(f"FAIL ci-checks.yml runs {script}, which is not an input of `rust` in ci-scope.py")
            failures += 1
    for test in script_tests_no_workflow_runs():
        print(f"FAIL {test} is run by no workflow: run it in one, or delete it")
        failures += 1
    if failures:
        return 1
    print(f"all {len(CASES) + len(FILTER_CASES)} ci-scope cases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
