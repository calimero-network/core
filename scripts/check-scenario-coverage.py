#!/usr/bin/env python3
"""Fail if a merobox workflow exists on disk but no e2e.yml shard runs it.

A workflow file that nothing runs looks maintained and gates nothing. Every
YAML file under workflows/ and apps/*/workflows/ must be either

- a workflow for this node (`backend: cloud-v2`) that some shard of
  .github/workflows/e2e.yml runs, or
- marked `not_applicable: <reason>` (the runner reports it and does not run
  it; the reason is required, so an exclusion has to be argued for).

A workflow counts as run only through an actual invocation in e2e.yml: a `run:`
line calling `scripts/run-workflows.py --shard <k>/<N>` with no file
arguments, in a job whose matrix gives `<k>` every value 1..N. The files those
shards run are what the runner itself selects (`all_workflows()` and
`shard()` in scripts/run-workflows.py), so a file outside its globs, or a
shard missing from the matrix, is caught. A path merely named in a comment,
an `echo` or a step name registers nothing, and neither does `--smoke` (a fixed
list, scripts/workflows-smoke.json, which release.yml runs against a
PR's Docker image): every workflow on that list must exist and be on
scripts/workflows-baseline.json, so the image is gated on passes.

Usage: check-scenario-coverage.py [--root DIR]
Exit 0 when every workflow is run or marked not_applicable, 1 otherwise.
Self-test: scripts/tests/check-scenario-coverage-test.py.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import sys
from pathlib import Path

import yaml

sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree

GLOBS = ("workflows/**/*.y*ml", "apps/*/workflows/**/*.y*ml")
RUNNER = "scripts/run-workflows.py"
SMOKE = "scripts/workflows-smoke.json"
BASELINE = "scripts/workflows-baseline.json"
SHARD_CALL = re.compile(
    r"^\s*python3?\s+" + re.escape(RUNNER) + r"\b(?P<args>[^\n]*(?:\\\n[^\n]*)*)", re.M
)


def load_runner(root: Path):
    spec = importlib.util.spec_from_file_location("run_workflows", root / RUNNER)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def on_disk(root: Path) -> list[str]:
    found: set[str] = set()
    for pattern in GLOBS:
        found.update(str(p.relative_to(root)) for p in root.glob(pattern) if p.is_file())
    return sorted(found)


def shard_counts(e2e: dict) -> tuple[set[int], list[str]]:
    """The shard counts N whose every shard 1..N some job runs, and problems."""
    problems: list[str] = []
    covered: set[int] = set()
    for name, job in (e2e.get("jobs") or {}).items():
        matrix = ((job or {}).get("strategy") or {}).get("matrix") or {}
        for step in (job or {}).get("steps") or []:
            run = step.get("run") if isinstance(step, dict) else None
            if not isinstance(run, str):
                continue
            for m in SHARD_CALL.finditer(run):
                text = re.sub(r"\$\{\{\s*(.*?)\s*\}\}", r"${{\1}}", m.group("args"))
                args = [a for a in text.split() if a != "\\"]  # line continuations
                shard = next((a for i, a in enumerate(args) if i and args[i - 1] == "--shard"), None)
                if "--list" in args or "--smoke" in args:
                    continue
                positional = [
                    a for i, a in enumerate(args)
                    if not a.startswith("-") and not (i and args[i - 1] in ("--shard", "--json"))
                ]  # fmt: skip
                if positional:
                    problems.append(f"job {name}: {RUNNER} runs named files only: {positional}")
                    continue
                if shard is None:
                    covered.add(1)  # every workflow, unsharded
                    continue
                k, _, n = shard.partition("/")
                if not n.isdigit():
                    problems.append(f"job {name}: --shard {shard} names no shard count")
                    continue
                n_i = int(n)
                km = re.fullmatch(r"\$\{\{matrix\.(\w+)\}\}", k)
                if km:
                    values = matrix.get(km.group(1))
                    got = {int(v) for v in values} if isinstance(values, list) else set()
                else:
                    got = {int(k)} if k.isdigit() else set()
                missing = set(range(1, n_i + 1)) - got
                if missing:
                    problems.append(
                        f"job {name}: shards {sorted(missing)} of {n_i} are run by no matrix entry"
                    )
                else:
                    covered.add(n_i)
    return covered, problems


def check(root: Path) -> list[str]:
    e2e_path = root / ".github/workflows/e2e.yml"
    e2e = yaml.safe_load(e2e_path.read_text()) or {}
    counts, problems = shard_counts(e2e)
    runner = load_runner(root)
    runnable = set(runner.all_workflows())
    run_by_shards: set[str] = set()
    for n in counts:
        for k in range(1, n + 1):
            run_by_shards.update(runner.shard(sorted(runnable), k, n))
    if not counts:
        problems.append(f"e2e.yml runs {RUNNER} in no shard")
    for f in on_disk(root):
        try:
            wf = yaml.safe_load((root / f).read_text())
        except yaml.YAMLError as e:
            problems.append(f"{f}: not YAML ({e.__class__.__name__})")
            continue
        if not isinstance(wf, dict) or wf.get("backend") != "cloud-v2":
            problems.append(f"{f}: not a merobox cloud-v2 workflow (backend: cloud-v2); nothing runs it")
            continue
        na = wf.get("not_applicable")
        if na is not None:
            if not str(na).strip():
                problems.append(f"{f}: not_applicable needs a reason")
            continue
        if f not in run_by_shards:
            problems.append(f"{f}: run by no e2e.yml shard")
    problems += check_smoke(root)
    return problems


def check_smoke(root: Path) -> list[str]:
    """Every workflow on the smoke list exists and is on the baseline."""
    if not (root / SMOKE).exists():
        return []
    smoke = json.loads((root / SMOKE).read_text())
    baseline = set(json.loads((root / BASELINE).read_text())) if (root / BASELINE).exists() else set()
    problems = []
    for f in smoke.get("workflows", []):
        if not (root / f).is_file():
            problems.append(f"{SMOKE}: {f} does not exist")
        elif f not in baseline:
            problems.append(f"{SMOKE}: {f} is not on {BASELINE}, so its failure gates nothing")
    if not smoke.get("workflows"):
        problems.append(f"{SMOKE}: lists no workflows")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = ap.parse_args()
    problems = check(args.root.resolve())
    for p in problems:
        print(f"UNCOVERED {p}")
    if problems:
        return 1
    print(f"every workflow under {', '.join(GLOBS)} runs in an e2e.yml shard")
    return 0


if __name__ == "__main__":
    sys.exit(main())
