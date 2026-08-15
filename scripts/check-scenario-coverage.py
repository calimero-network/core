#!/usr/bin/env python3
"""Fail if a merobox scenario exists on disk but runs in no CI matrix.

A scenario file that nothing runs looks maintained and gates nothing. This has
happened repeatedly: replacing a glob with an enumeration orphaned fourteen
scenarios at once and named none of them in the diff, a later parallelisation
dropped one of an app's two scenarios, and scenarios added afterwards were
written by authors with no way to know a matrix entry was also required.

A scenario counts as registered only when it is the argument of an actual
`merobox bootstrap run`. Resolution works from each workflow's own invocation
rather than a hardcoded directory layout, so a new e2e workflow arranged
differently is followed automatically:

    merobox bootstrap run "workflows/app-migration/${WORKFLOW}.yml"   # stem matrix + job env
    cd apps/${{ matrix.app }} && merobox bootstrap run "${{ matrix.file }}"
    FUZZY_CONFIG="workflows/.../fuzzy-test.yml"; merobox bootstrap run "$FUZZY_CONFIG"

Nothing else counts. Scanning for any string that merely *names* a scenario
would re-admit the drift this exists to catch: a path in an `echo`, a step
name, or a `#` comment inside a `run:` block (which YAML keeps as part of the
string, unlike a real YAML comment) would mark an unrun scenario as covered.

Accepted exclusions live in scripts/scenario-coverage-baseline.json as
{path: reason}; a reason is required so an exclusion has to be argued for
rather than accumulated.

Usage: check-scenario-coverage.py [--baseline FILE]
Exit 0 when every scenario is registered or baselined, 1 otherwise.
"""

from __future__ import annotations

import argparse
import glob
import itertools
import json
import os
import re
import sys

import yaml

SCENARIO_GLOBS = ("apps/*/workflows/**/*.y*ml", "workflows/**/*.y*ml")

def scenarios_on_disk() -> list[str]:
    found: set[str] = set()
    for pattern in SCENARIO_GLOBS:
        found.update(glob.glob(pattern, recursive=True))
    return sorted(found)


def matrix_combinations(job: dict) -> list[dict]:
    """Every matrix combination for a job, following GitHub's own semantics.

    Plain axes cross-product with each other, `include` entries are added on
    top, and `exclude` entries drop any combination they are a subset of.
    Getting this wrong in either direction breaks the gate: a missed
    combination hides a registration, an invented one reports a false dangling
    entry.
    """
    matrix = (job.get("strategy") or {}).get("matrix")
    if not isinstance(matrix, dict):
        return []

    axes = {
        k: v
        for k, v in matrix.items()
        if k not in ("include", "exclude") and isinstance(v, list)
    }
    combos: list[dict] = []
    if axes:
        keys = list(axes)
        for values in itertools.product(*(axes[k] for k in keys)):
            combos.append(dict(zip(keys, values)))

    for entry in matrix.get("exclude", []) or []:
        if isinstance(entry, dict):
            combos = [
                c for c in combos
                if not all(c.get(k) == v for k, v in entry.items())
            ]

    combos.extend(dict(e) for e in matrix.get("include", []) or [] if isinstance(e, dict))
    return combos


# A path may embed a `${{ matrix.x }}` expression, whose spaces must not end the
# token — so match expressions and non-space runs alternately rather than \S+.
_TOKEN = r'(?:\$\{\{[^}]*\}\}|[^\s"\\])+'
RUN_RE = re.compile(r'merobox\s+bootstrap\s+run\s+\\?\s*(?:"(%s)"|(%s))' % (_TOKEN, _TOKEN))
CD_RE = re.compile(r'^\s*cd\s+"?(%s)"?' % _TOKEN, re.M)
# A literal shell assignment in the same run block, e.g.
#   FUZZY_CONFIG="workflows/fuzzy-tests/kv-store/fuzzy-test.yml"
# Resolving these is what lets the gate insist every registration comes from an
# actual `merobox bootstrap run` argument rather than from any string that
# happens to name a scenario.
ASSIGN_RE = re.compile(r'^\s*([A-Za-z_][A-Za-z0-9_]*)="([^"$]+)"\s*$', re.M)


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
            before = run[: m.start()]
            cds = [c.group(1) for c in CD_RE.finditer(before)]
            shell = {a.group(1): a.group(2) for a in ASSIGN_RE.finditer(before)}
            for name, value in shell.items():
                template = template.replace("${%s}" % name, value).replace("$%s" % name, value)
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
        except yaml.YAMLError as err:
            # Skipping an unparseable workflow would drop its registrations and
            # report its scenarios as orphans — a confusing failure that hides
            # the real one. Say what actually broke.
            raise SystemExit(f"{wf}: could not parse as YAML, so its registrations "
                             f"cannot be read:\n{err}")
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
