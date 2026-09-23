#!/usr/bin/env python3
"""Run CI's Rust checks locally by reading ci-checks.yml instead of copying it.

A hand-kept copy drifts toward running less than CI. This runs the `run` steps of
every job the `Rust` check needs, in order, so a CI change needs no edit here.

Usage
-----
    ./scripts/check-like-ci.py --list             # what CI runs, in order
    ./scripts/check-like-ci.py                    # run all of it
    ./scripts/check-like-ci.py --only clippy      # just the clippy steps
    ./scripts/check-like-ci.py --skip deny --skip machete
    ./scripts/check-like-ci.py --job wasm-size    # a different job

Failures do not stop the run, like `if: !cancelled()` in CI; `--fail-fast` opts out.
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
AGGREGATE_JOB = "rust"  # the required `Rust` check; its `needs` are the jobs run by default
INSTALL_ACTION = "taiki-e/install-action"  # steps whose `tool:` binaries CI installs for later steps

# (predicate, message) for things CI has and a workstation may not; True means missing.
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


def load_workflow(workflow: Path) -> dict:
    with workflow.open() as handle:
        return yaml.safe_load(handle)


def default_jobs(doc: dict) -> list[str]:
    """The jobs the aggregate `Rust` check waits on, so a new one is picked up here."""
    needs = ((doc.get("jobs") or {}).get(AGGREGATE_JOB) or {}).get("needs") or []
    return [needs] if isinstance(needs, str) else list(needs)


def installed_tools(doc: dict, job: str) -> list[str]:
    """Binaries the job installs through `taiki-e/install-action`, versions stripped."""
    tools = []
    for step in doc["jobs"][job].get("steps") or []:
        if INSTALL_ACTION in (step.get("uses") or ""):
            spec = (step.get("with") or {}).get("tool") or ""
            tools += [tool.split("@")[0].strip() for tool in spec.split(",") if tool.strip()]
    return tools


def load_steps(doc: dict, workflow: Path, job: str) -> list[tuple[str, str, dict]]:
    """The job's `run` steps as (name, script, env); `uses:` steps have nothing to run locally."""
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
        name = f"{job}: {step.get('name') or f'step {index}'}"
        env = {**workflow_env, **job_env, **(step.get("env") or {})}
        steps.append((name, script, env))
    return steps


def resolve_env(raw: dict) -> dict:
    """CI's env without the `${{ }}` expressions only a runner can evaluate; exported values win."""
    env = dict(os.environ)
    for key, value in raw.items():
        if isinstance(value, str):
            if "${{" in value:
                continue
            rendered = value
        elif isinstance(value, bool):
            # YAML reads true/false as booleans; Actions renders them lowercased.
            rendered = "true" if value else "false"
        elif isinstance(value, (int, float)):
            rendered = str(value)
        else:
            continue
        env.setdefault(key, rendered)
    # rust-cache turns incremental compilation off in CI.
    env.setdefault("CARGO_INCREMENTAL", "0")
    return env


def matches(name: str, patterns: list[str]) -> bool:
    return any(re.search(pattern, name, re.IGNORECASE) for pattern in patterns)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--workflow", type=Path, default=DEFAULT_WORKFLOW)
    parser.add_argument(
        "--job",
        action="append",
        default=[],
        help=f"repeatable; default: every job the {AGGREGATE_JOB!r} job needs",
    )
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

    doc = load_workflow(args.workflow)
    jobs = args.job or default_jobs(doc)
    steps = [step for job in jobs for step in load_steps(doc, args.workflow, job)]

    if args.list:
        print(f"{args.workflow.relative_to(REPO_ROOT)} :: jobs {', '.join(jobs)}\n")
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

    warnings = [message for missing, message in PREREQS if missing()] + [
        f"`{tool}` is not on PATH; CI installs it with {INSTALL_ACTION} for the {job} job."
        for job in dict.fromkeys(name.split(":")[0] for name, _, _ in selected)
        for tool in installed_tools(doc, job)
        if shutil.which(tool) is None
    ]
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
        # Actions runs `run:` steps with `bash -e`.
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
    # Deselected by --only/--skip and never reached after --fail-fast are reported apart.
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
