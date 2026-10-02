#!/usr/bin/env python3
"""Tests for ci-scope.py against this repository's own dependency graph.

Both wrong answers are silent: running a job too often only costs time, but
skipping one lands a change its gate never saw. Each case pins one rule.

    python3 scripts/tests/ci-scope-test.py
"""

import fnmatch
import importlib.util
import re
import subprocess
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "ci-scope.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
NARROW = {"storage_cost", "wasm_size", "cargo_mero_e2e", "apps"}
ALL = NARROW | {"rust"}
R = {"rust"}

CASES = [
    # (name, changed paths, ci-checks jobs that must run; every other job must skip)
    ("a node-only change runs only the workspace jobs", ["crates/node/src/lib.rs"], R),
    ("merod runs only the workspace jobs", ["crates/merod/src/main.rs", "crates/server/src/lib.rs"], R),
    ("storage reaches all of them", ["crates/storage/src/lib.rs"], ALL),
    ("the sdk reaches every app build", ["crates/sdk/src/env.rs"], ALL),
    ("cargo-mero reaches its users", ["tools/cargo-mero/src/build.rs"], R | {"wasm_size", "cargo_mero_e2e", "apps"}),
    ("a cargo-mero fixture is its e2e's input", ["tools/cargo-mero/tests/fixtures/demo-app/src/lib.rs"], R | {"cargo_mero_e2e"}),
    ("one app reaches the app builds", ["apps/blobs/src/lib.rs"], R | {"apps"}),
    ("a size-gated app reaches the size gate", ["apps/kv-store/src/lib.rs"], R | {"wasm_size", "apps"}),
    ("its own script reaches the storage gate", ["scripts/check-storage-cost.sh"], R | {"storage_cost"}),
    ("a lint script runs the workspace jobs", ["scripts/check-naming.sh"], R),
    ("a file no job reads runs nothing", ["scripts/profiling/collect.sh", ".github/workflows/fuzz.yml"], set()),
    ("a crate README is inert", ["crates/meroctl/README.md"], set()),
    ("AGENTS.md and its CLAUDE.md link are inert", ["crates/storage/AGENTS.md", "crates/node/CLAUDE.md"], set()),
    ("site docs are inert", ["docs/src/content/docs/protocol/sync.mdx"], set()),
    ("a bundled guide is code", ["apps/kv-store/GUIDE.md"], R | {"wasm_size", "apps"}),
    ("an include_str! guide is code", ["tools/cargo-mero/tests/fixtures/multi-app/GUIDE.md"], R | {"cargo_mero_e2e"}),
    ("docs beside code do not hide the code", ["crates/storage/AGENTS.md", "crates/storage/src/lib.rs"], ALL),
    ("the lockfile reaches every job", ["Cargo.lock"], ALL),
    ("the root manifest reaches every job", ["Cargo.toml"], ALL),
    ("the workflow reaches every job", [".github/workflows/ci-checks.yml"], ALL),
    ("this script reaches every job", ["scripts/ci-scope.py"], ALL),
    ("a deleted file in a closure still counts", ["crates/storage/src/this-file-was-deleted.rs"], ALL),
    ("no input fails closed", [], ALL),
]

RELEASE_CASES = [
    # (name, changed paths, whether release.yml builds)
    ("a crate change builds", ["crates/node/src/lib.rs"], True),
    ("a tool change builds", ["tools/merodb/src/main.rs"], True),
    ("the image Dockerfile builds", [".github/workflows/deps/prebuilt.Dockerfile"], True),
    ("crate docs do not build", ["crates/node/AGENTS.md", "crates/storage/README.md"], False),
    ("an app does not build", ["apps/kv-store/src/lib.rs"], False),
    ("site docs do not build", ["docs/src/content/docs/index.mdx"], False),
    ("no input fails closed", [], True),
]


def run(paths, *args):
    out = subprocess.run(
        [sys.executable, str(SCRIPT), *args], input="".join(p + "\n" for p in paths),
        capture_output=True, text=True, check=True,
    ).stdout
    verdicts = dict(line.split("=", 1) for line in out.split())
    return {job for job, value in verdicts.items() if value == "true"}, set(verdicts)


def scripts_ci_checks_runs():
    """Every scripts/ path ci-checks.yml names: each is an input of the `rust` verdict."""
    text = (SCRIPT.parent.parent / ".github" / "workflows" / "ci-checks.yml").read_text()
    return sorted({m.removeprefix("./") for m in re.findall(r"(?:\./)?scripts/[\w./-]+", text)})


def main():
    failures = 0
    for name, paths, want in CASES:
        runs, jobs = run(paths)
        if jobs != ALL:
            print(f"FAIL {name}: reported jobs {sorted(jobs)}, expected {sorted(ALL)}")
            failures += 1
        elif runs != want:
            print(f"FAIL {name}: runs {sorted(runs)}, expected {sorted(want)}")
            failures += 1
        else:
            print(f"ok   {name}")
    for name, paths, want in RELEASE_CASES:
        runs, jobs = run(paths, "--workflow", "release")
        if jobs != {"build"} or (runs == {"build"}) != want:
            print(f"FAIL release: {name}: got {sorted(runs)} of {sorted(jobs)}")
            failures += 1
        else:
            print(f"ok   release: {name}")
    spec = importlib.util.spec_from_file_location("ci_scope", SCRIPT)
    ci_scope = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(ci_scope)
    inputs = ci_scope.WORKFLOWS["ci-checks"][1]
    for script in scripts_ci_checks_runs():
        if not any(fnmatch.fnmatchcase(script, g) for g in inputs):
            print(f"FAIL ci-checks.yml runs {script}, which is not an input of `rust` in ci-scope.py")
            failures += 1
    if failures:
        return 1
    print(f"all {len(CASES) + len(RELEASE_CASES)} ci-scope cases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
