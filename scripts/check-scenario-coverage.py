#!/usr/bin/env python3
"""Fail if a merobox scenario exists on disk but runs in no CI matrix.

A scenario file that nothing runs looks maintained and gates nothing. This has
happened repeatedly: replacing a glob with an enumeration orphaned fourteen
scenarios at once and named none of them in the diff, a later parallelisation
dropped one of an app's two scenarios, and scenarios added afterwards were
written by authors with no way to know a matrix entry was also required.

Resolution works from each workflow's own `merobox bootstrap run` template
rather than a hardcoded directory layout, so a new e2e workflow with a
different arrangement is followed automatically:

    merobox bootstrap run "workflows/app-migration/${WORKFLOW}.yml"   # stem matrix
    cd apps/${{ matrix.app }} && merobox bootstrap run "${{ matrix.file }}"

Accepted exclusions live in scripts/scenario-coverage-baseline.json as
{path: reason}; a reason is required so an exclusion has to be argued for
rather than accumulated.

Usage: check-scenario-coverage.py [--baseline FILE]
Exit 0 when every scenario is registered or baselined, 1 otherwise.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys

import yaml

SCENARIO_GLOBS = ("apps/*/workflows/**/*.yml", "workflows/**/*.yml")

# `e2e-convergence-baseline.txt` names scenarios but is a lint artifact, not a
# runner: the convergence linter parses those files and never boots a node.
# Treating it as an anchor hid apps/sync-test — a crate cargo refused to build,
# whose two scenarios could not have run even if they were registered.
NOT_AN_ANCHOR = ("e2e-convergence-baseline.txt",)


def scenarios_on_disk() -> list[str]:
    found: set[str] = set()
    for pattern in SCENARIO_GLOBS:
        found.update(glob.glob(pattern, recursive=True))
    return sorted(found)


def walk_strings(node):
    """Every string value in a parsed YAML document."""
    if isinstance(node, str):
        yield node
    elif isinstance(node, dict):
        for v in node.values():
            yield from walk_strings(v)
    elif isinstance(node, list):
        for v in node:
            yield from walk_strings(v)


def matrix_combinations(job: dict) -> list[dict]:
    """Every matrix combination for a job, as dicts of key -> value.

    Handles both `include:` entries (dicts) and plain key lists, which is the
    difference between the app matrix and the stem matrix.
    """
    matrix = (job.get("strategy") or {}).get("matrix")
    if not isinstance(matrix, dict):
        return []
    combos = [dict(e) for e in matrix.get("include", []) if isinstance(e, dict)]
    for key, values in matrix.items():
        if key == "include" or not isinstance(values, list):
            continue
        combos.extend({key: v} for v in values if isinstance(v, (str, int)))
    return combos


# A path may embed a `${{ matrix.x }}` expression, whose spaces must not end the
# token — so match expressions and non-space runs alternately rather than \S+.
_TOKEN = r'(?:\$\{\{[^}]*\}\}|[^\s"\\])+'
RUN_RE = re.compile(r'merobox\s+bootstrap\s+run\s+\\?\s*(?:"(%s)"|(%s))' % (_TOKEN, _TOKEN))
CD_RE = re.compile(r'^\s*cd\s+"?(%s)"?' % _TOKEN, re.M)


def run_templates(job: dict) -> list[tuple[str, str]]:
    """(cwd, path) template pairs the job hands to `merobox bootstrap run`.

    The path is resolved against the last `cd` before it in the same run block,
    which is how both layouts express themselves: a literal `cd apps/<app>` in
    the release workflow, and `cd apps/${{ matrix.app }}` in the matrix one.
    """
    pairs: list[tuple[str, str]] = []
    for step in job.get("steps", []) or []:
        run = step.get("run") if isinstance(step, dict) else None
        if not run:
            continue
        for m in RUN_RE.finditer(run):
            template = m.group(1) or m.group(2)
            cds = [c.group(1) for c in CD_RE.finditer(run[: m.start()])]
            pairs.append((cds[-1] if cds else "", template))
    return pairs


def substitute(template: str, combo: dict, env: dict) -> str:
    """Resolve `${{ matrix.k }}` and `${VAR}` against one matrix combination.

    Job-level env is resolved first so `WORKFLOW: ${{ matrix.workflow }}`
    followed by `${WORKFLOW}` in the run body resolves in one pass.
    """
    out = template
    for key, value in combo.items():
        out = out.replace("${{ matrix.%s }}" % key, str(value))
    for name, value in env.items():
        resolved = value
        for key, mval in combo.items():
            resolved = resolved.replace("${{ matrix.%s }}" % key, str(mval))
        out = out.replace("${%s}" % name, resolved).replace("$%s" % name, resolved)
    return out


def registered_scenarios() -> tuple[set[str], list[tuple[str, str]]]:
    """Scenario paths CI runs, plus (workflow, path) pairs resolving to nothing."""
    registered: set[str] = set()
    dangling: list[tuple[str, str]] = []

    for wf in sorted(glob.glob(".github/workflows/*.y*ml")):
        try:
            doc = yaml.safe_load(open(wf, encoding="utf-8"))
        except yaml.YAMLError:
            continue
        if not isinstance(doc, dict):
            continue
        for job in (doc.get("jobs") or {}).values():
            if not isinstance(job, dict):
                continue
            env = {k: str(v) for k, v in (job.get("env") or {}).items()}
            templates = run_templates(job)
            if not templates:
                continue
            combos = matrix_combinations(job) or [{}]
            for cwd, template in templates:
                for combo in combos:
                    path = substitute(template, combo, env)
                    base = substitute(cwd, combo, env)
                    # A shell variable this script cannot resolve (e.g. a config
                    # path assembled at runtime) is not a static registration;
                    # reporting it as dangling would be noise, not a finding.
                    if "$" in path or "$" in base:
                        continue
                    if any(ch in path for ch in "*?"):
                        continue
                    hit = next(
                        (c for c in (os.path.join(base, path) if base else path, path)
                         if os.path.isfile(c)),
                        None,
                    )
                    if hit:
                        registered.add(os.path.normpath(hit))
                    else:
                        dangling.append((os.path.basename(wf), path))

    # Literal scenario paths named outside a matrix (single-scenario steps).
    #
    # Scanned from the PARSED document, never the raw text: a path inside a `#`
    # comment is prose, not an anchor. `member-removed-partition-window-reconcile`
    # is named only by a comment explaining why it is excluded, and matching that
    # would have marked the one deliberately-unregistered scenario as covered.
    for f in glob.glob(".github/**/*.y*ml", recursive=True):
        if not os.path.isfile(f) or any(n in f for n in NOT_AN_ANCHOR):
            continue
        try:
            doc = yaml.safe_load(open(f, encoding="utf-8"))
        except yaml.YAMLError:
            continue
        for value in walk_strings(doc):
            for p in re.findall(r"(?:apps|workflows)/[\w./-]+\.ya?ml", value):
                if os.path.isfile(p):
                    registered.add(p)

    return registered, dangling


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline", default="scripts/scenario-coverage-baseline.json")
    args = ap.parse_args()

    baseline: dict[str, str] = {}
    if os.path.isfile(args.baseline):
        baseline = json.load(open(args.baseline, encoding="utf-8"))

    scenarios = scenarios_on_disk()
    registered, dangling = registered_scenarios()

    unregistered = [s for s in scenarios if s not in registered]
    accepted = [s for s in unregistered if s in baseline]
    orphaned = [s for s in unregistered if s not in baseline]
    stale_baseline = [p for p in baseline if p in registered or not os.path.isfile(p)]

    if accepted:
        print(f"::notice::{len(accepted)} baselined (accepted-unregistered) scenario(s):")
        for s in accepted:
            print(f"  - {s} — {baseline[s]}")

    failed = False

    if orphaned:
        print(
            "\nScenario(s) in no CI matrix (add a matrix entry, or add to "
            f"{args.baseline} with a reason):"
        )
        for s in orphaned:
            print(f"  {s}")
        failed = True

    if dangling:
        print("\nMatrix entr(ies) pointing at a scenario that does not exist:")
        for wf, p in dangling:
            print(f"  {wf}: {p}")
        failed = True

    if stale_baseline:
        print(
            f"\nStale entr(ies) in {args.baseline} — now registered or deleted, "
            "so the exclusion is obsolete:"
        )
        for p in stale_baseline:
            print(f"  {p}")
        failed = True

    if failed:
        return 1

    print(
        f"OK: {len(scenarios) - len(accepted)}/{len(scenarios)} scenarios registered; "
        f"{len(accepted)} baselined."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
