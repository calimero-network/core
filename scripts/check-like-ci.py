#!/usr/bin/env python3
"""Run CI's own checks locally, by reading the workflow instead of copying it.

Why this exists
---------------
The commands are already written down in `.github/workflows/ci-checks.yml`, and
a second hand-maintained copy of them is worse than none: it drifts silently,
and it drifts in the direction of running *less* than CI does. Three PRs in a
row went red for exactly that reason, each on a different narrowing:

* per-crate `cargo clippy` instead of `--workspace --all-targets`, which missed
  a `-D warnings` error in a test file;
* `cargo test -p <crate> --lib`, which never builds a crate's `tests/`
  integration targets;
* default features, which never compiles the `mock-attestation` module that CI
  lints and tests in its own step.

Every one of those looked like "I ran the tests". So this script does not hold a
list of commands -- it *reads the job* and runs the steps it finds, in order. Add
a step to CI and it appears here with no edit; change a flag in CI and the local
run changes with it.

Usage
-----
    ./scripts/check-like-ci.py --list             # what CI runs, in order
    ./scripts/check-like-ci.py                    # run all of it
    ./scripts/check-like-ci.py --only clippy      # just the clippy steps
    ./scripts/check-like-ci.py --skip deny --skip machete
    ./scripts/check-like-ci.py --job wasm-size    # a different job

Failures do not stop the run, mirroring `if: ${{ !cancelled() }}` on the job's
steps: CI reports every step it could, and so should this. That is also the
behaviour worth having locally -- one pass tells you everything to fix rather
than one thing at a time. `--fail-fast` opts out.

Exit status is 0 only if every step that ran passed.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci-checks.yml"
DEFAULT_JOB = "rust"

# Prerequisites CI has and a workstation may not. Reported once, up front, as
# warnings rather than errors: a step that needs one will fail on its own and say
# why, but knowing beforehand is the difference between "my change broke this"
# and "this machine cannot run this step".
#
# Each entry is (predicate, message). The predicate returns True when the
# prerequisite is MISSING.
PREREQS = [
    (
        lambda: shutil.which("ifconfig") is None,
        "`ifconfig` is missing, and the Cargo test step derives TEST_HOSTS from it.\n"
        "    The step still runs; TEST_HOSTS just starts with an empty host. Install\n"
        "    net-tools for a faithful mirror of CI.",
    ),
    (
        lambda: not os.environ.get("CALIMERO_AUTH_FRONTEND_SRC")
        and not os.environ.get("CALIMERO_AUTH_FRONTEND_FETCH_TOKEN"),
        "mero-auth's build script fetches a frontend archive from GitHub. CI passes a\n"
        "    token for it; locally the fetch may be blocked or rate-limited, which fails\n"
        "    the build script rather than any check. Point CALIMERO_AUTH_FRONTEND_SRC at\n"
        "    a local directory to skip the fetch.",
    ),
    (
        lambda: not os.environ.get("CALIMERO_WEBUI_SRC")
        and not os.environ.get("CALIMERO_WEBUI_FETCH_TOKEN"),
        "calimero-server's build script fetches the admin dashboard the same way.\n"
        "    Point CALIMERO_WEBUI_SRC at a local directory to skip that fetch.",
    ),
]


def load_steps(workflow: Path, job: str) -> list[tuple[str, str, dict]]:
    """The named job's `run` steps as (name, script, env), in workflow order.

    Steps that use an action (`uses:`) carry no script to run -- checkout and
    toolchain setup are the local machine's existing state -- so they are
    dropped here rather than reported as skipped, which would be noise on every
    single run.
    """
    with workflow.open() as handle:
        doc = yaml.safe_load(handle)

    jobs = doc.get("jobs") or {}
    if job not in jobs:
        raise SystemExit(
            f"no job {job!r} in {workflow.relative_to(REPO_ROOT)}; "
            f"available: {', '.join(sorted(jobs))}"
        )

    workflow_env = doc.get("env") or {}
    job_env = jobs[job].get("env") or {}

    steps = []
    for index, step in enumerate(jobs[job].get("steps") or []):
        script = step.get("run")
        if not script:
            continue
        name = step.get("name") or f"step {index}"
        env = {**workflow_env, **job_env, **(step.get("env") or {})}
        steps.append((name, script, env))
    return steps


def resolve_env(raw: dict) -> dict:
    """CI's env, minus what only makes sense on a runner.

    A `${{ ... }}` expression cannot be evaluated here, so those entries are
    dropped: `CARGO_TARGET_DIR` pointed at the runner's workspace and a secret
    token has no local value. Anything already exported wins, so a caller can
    set `CALIMERO_AUTH_FRONTEND_SRC` and have it survive.
    """
    env = dict(os.environ)
    for key, value in raw.items():
        if isinstance(value, str):
            if "${{" in value:
                continue
            rendered = value
        elif isinstance(value, bool):
            # YAML reads `yes`/`no`/`true`/`false` as booleans, so a perfectly
            # ordinary workflow value arrives here not-a-string. Actions renders
            # these lowercased; dropping them instead (the first version of this
            # function did) loses env the step was written to rely on.
            rendered = "true" if value else "false"
        elif isinstance(value, (int, float)):
            rendered = str(value)
        else:
            continue
        env.setdefault(key, rendered)
    # CI runs with incremental compilation off. Matching it keeps the local
    # target directory closer to CI's and, on a small disk, is the difference
    # between finishing and running out of space.
    env.setdefault("CARGO_INCREMENTAL", "0")
    return env


def matches(name: str, patterns: list[str]) -> bool:
    return any(re.search(pattern, name, re.IGNORECASE) for pattern in patterns)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--workflow", type=Path, default=DEFAULT_WORKFLOW)
    parser.add_argument("--job", default=DEFAULT_JOB, help=f"default: {DEFAULT_JOB}")
    parser.add_argument("--list", action="store_true", help="print the steps and exit")
    parser.add_argument(
        "--only", action="append", default=[], metavar="PATTERN", help="run only matching steps"
    )
    parser.add_argument(
        "--skip", action="append", default=[], metavar="PATTERN", help="skip matching steps"
    )
    parser.add_argument("--fail-fast", action="store_true", help="stop at the first failure")
    parser.add_argument("--dry-run", action="store_true", help="print commands, run nothing")
    args = parser.parse_args()

    steps = load_steps(args.workflow, args.job)

    if args.list:
        print(f"{args.workflow.relative_to(REPO_ROOT)} :: job {args.job!r}\n")
        for number, (name, script, _) in enumerate(steps, start=1):
            first = script.strip().splitlines()[0]
            suffix = " …" if len(script.strip().splitlines()) > 1 else ""
            print(f"{number:2}. {name}\n    {first}{suffix}")
        return 0

    selected = [
        (name, script, env)
        for name, script, env in steps
        if (not args.only or matches(name, args.only)) and not matches(name, args.skip)
    ]
    if not selected:
        print("no steps selected", file=sys.stderr)
        return 1

    warnings = [message for missing, message in PREREQS if missing()]
    if warnings and not args.dry_run:
        print("=== prerequisites CI has that this machine may not ===")
        for message in warnings:
            print(f"  ! {message}")
        print()

    results: list[tuple[str, str, float]] = []
    stopped_early = False
    for number, (name, script, raw_env) in enumerate(selected, start=1):
        header = f"[{number}/{len(selected)}] {name}"
        print(f"\n=== {header} ===", flush=True)
        if args.dry_run:
            print(script.strip())
            continue

        started = time.monotonic()
        # `bash -e` matches the workflow's `shell: bash -e {0}`, so a multi-line
        # step stops at its first failing command here exactly as it does in CI.
        completed = subprocess.run(
            ["bash", "-e", "-c", script],
            cwd=REPO_ROOT,
            env=resolve_env(raw_env),
            check=False,
        )
        elapsed = time.monotonic() - started
        status = "ok" if completed.returncode == 0 else f"FAILED ({completed.returncode})"
        results.append((name, status, elapsed))
        print(f"--- {header}: {status} in {elapsed:.1f}s ---", flush=True)

        if completed.returncode != 0 and args.fail_fast:
            print("\nstopping: --fail-fast", file=sys.stderr)
            stopped_early = True
            break

    if args.dry_run:
        return 0

    print("\n=== summary ===")
    width = max(len(name) for name, _, _ in results)
    for name, status, elapsed in results:
        print(f"  {name:<{width}}  {status:<12} {elapsed:6.1f}s")

    failed = [name for name, status, _ in results if status != "ok"]
    ran = len(results)
    # Three different reasons a step has no result, kept apart because they mean
    # different things to whoever reads this: filtered out by --only/--skip,
    # never reached because --fail-fast stopped the run, or simply absent. Rolled
    # together as "skipped" this line once said "1 not selected" about a step
    # that was very much selected and just never ran.
    unreached = len(selected) - ran if stopped_early else 0
    deselected = len(steps) - len(selected)
    tail = "".join(
        [
            f", {unreached} not reached" if unreached else "",
            f", {deselected} not selected" if deselected else "",
        ]
    )
    print(f"\n{ran - len(failed)}/{ran} passed{tail}")
    if failed:
        print("failed: " + ", ".join(failed))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
