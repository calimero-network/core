#!/usr/bin/env python3
"""Tests for check-workflow-triggers.py: each bad workflow must fail, each good one pass.

    python3 scripts/tests/check-workflow-triggers-test.py
"""

import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-workflow-triggers.py"
RUN_GUARD = "github.event.workflow_run.head_repository.full_name == github.repository"
TARGET_GUARD = "github.event.pull_request.head.repo.full_name == github.repository"

RUN_HEAD = """
name: t
on:
  workflow_run:
    workflows: [CI]
jobs:
  build:
    runs-on: ubuntu-latest
    if: %s
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
"""
TARGET_HEAD = """
name: t
on: pull_request_target
jobs:
  build:
    runs-on: ubuntu-latest
    if: %s
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
"""
NEEDS_CHAIN = """
name: t
on:
  workflow_run:
    workflows: [CI]
jobs:
  build:
    runs-on: ubuntu-latest
    if: >-
      github.event.workflow_run.conclusion == 'success' &&
      %s
    steps:
      - run: echo gate
  test:
    needs: build
    runs-on: ubuntu-latest
    if: %s
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
"""
CACHE = """
name: t
on: workflow_run
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: %s
        with:
          %s
"""
PLAIN_PR = """
name: t
on: pull_request
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - uses: actions/cache@v6
        with:
          path: x
"""

CASES = [
    ("workflow_run head checkout with guard", RUN_HEAD % RUN_GUARD, 0),
    ("workflow_run head checkout without guard", RUN_HEAD % "github.event.workflow_run.event == 'push'", 1),
    ("workflow_run head checkout without any if", RUN_HEAD.replace("    if: %s\n", ""), 1),
    ("pull_request_target head checkout with guard", TARGET_HEAD % TARGET_GUARD, 0),
    ("pull_request_target head checkout without guard", TARGET_HEAD % "github.actor != 'bot'", 1),
    ("guard on a needed job covers the dependent", NEEDS_CHAIN % (RUN_GUARD, "github.event_name == 'workflow_run'"), 0),
    ("always() on the dependent drops the inherited guard", NEEDS_CHAIN % (RUN_GUARD, "always()"), 1),
    ("needed job without guard", NEEDS_CHAIN % ("true", "true"), 1),
    ("actions/cache write", CACHE % ("actions/cache@v6", "path: x"), 1),
    ("actions/cache/save write", CACHE % ("actions/cache/save@v6", "path: x"), 1),
    ("actions/cache/restore is read only", CACHE % ("actions/cache/restore@v6", "path: x"), 0),
    ("setup-rust-ci without save-if", CACHE % ("./.github/actions/setup-rust-ci", "shared-key: k"), 1),
    ("setup-rust-ci with save-if true", CACHE % ("./.github/actions/setup-rust-ci", "save-if: true"), 1),
    ("setup-rust-ci with save-if false", CACHE % ("./.github/actions/setup-rust-ci", "save-if: false"), 0),
    ("rust-cache without save-if", CACHE % ("Swatinem/rust-cache@abc", "shared-key: k"), 1),
    ("an unprivileged workflow may checkout the head and cache", PLAIN_PR, 0),
]

def run_job(condition="", steps="", needs="", extra=""):
    lines = ["name: t", "on:", "  workflow_run:", "    workflows: [CI]", "  workflow_dispatch:", "jobs:", "  build:", "    runs-on: ubuntu-latest"]
    if condition:
        lines.append(f"    if: {condition}")
    lines += [extra] if extra else []
    lines += ["    steps:"] + steps.rstrip("\n").split("\n")
    return "\n".join(lines) + "\n"


CHECKOUT = "      - uses: actions/checkout@v7\n        with:\n          ref: %s\n"
BYPASSES = [
    ("unparsable yaml", "name: [unclosed\non: workflow_run\n", 1),
    ("no on key", "name: t\njobs:\n  a:\n    runs-on: x\n    steps:\n      - run: echo\n", 1),
    ("on of an unexpected shape", "name: t\non: 5\njobs: {}\n", 1),
    ("on and a quoted on together", "name: t\non: workflow_run\n'on': push\njobs: {}\n", 1),
    ("quoted on key is read", 'name: t\n"on": workflow_run\njobs:\n  a:\n    runs-on: x\n    steps:\n      - uses: actions/cache@v6\n', 1),
    ("reusable workflow job", "name: t\non: workflow_run\njobs:\n  a:\n    uses: ./.github/workflows/x.yml\n", 1),
    ("yaml merge key", "name: t\non: workflow_run\njobs:\n  a: &a\n    runs-on: x\n    steps: []\n  b:\n    <<: *a\n", 1),
    ("duplicate key", "name: t\non: workflow_run\njobs:\n  a:\n    runs-on: x\n    runs-on: y\n    steps: []\n", 1),
    ("merge ref checkout", run_job(steps=CHECKOUT % "refs/pull/${{ github.event.number }}/merge"), 1),
    ("head_commit id checkout", run_job(steps=CHECKOUT % "${{ github.event.head_commit.id }}"), 1),
    ("ref built through env", run_job(steps=CHECKOUT % "${{ env.TARGET }}"), 1),
    ("checkout of another repository", run_job(steps="      - uses: actions/checkout@v7\n        with:\n          repository: someone/else\n"), 1),
    ("gh pr checkout in run", run_job(steps="      - run: gh pr checkout 5\n"), 1),
    ("git fetch of a pull ref in run", run_job(steps="      - run: git fetch origin pull/5/head\n"), 1),
    ("unknown checkout action", run_job(steps="      - uses: someone/checkout-pr@v1\n"), 1),
    ("unknown cache action", run_job(steps="      - uses: someone/cache-everything@v1\n"), 1),
    ("guard or always()", run_job(RUN_GUARD + " || always()", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("always() or guard", run_job("always() || " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("guard or another condition", run_job("github.actor == 'x' || " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("negated guard", run_job("!(" + RUN_GUARD + ")", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("guard only inside a string", run_job("github.event.workflow_run.name == '" + RUN_GUARD + "'", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("event_name equal to the privileged trigger", run_job("github.event_name == 'workflow_run' || " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("guard in a conjunction", run_job("github.event.workflow_run.conclusion == 'success' && " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 0),
    ("guard written reversed", run_job("github.repository == github.event.workflow_run.head_repository.full_name", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 0),
    ("other events exempt through or", run_job("github.event_name == 'workflow_dispatch' || (" + RUN_GUARD + ")", CHECKOUT % "${{ github.event.workflow_run.head_sha || github.sha }}"), 0),
    ("not-this-trigger exempt through or", run_job("github.event_name != 'workflow_run' || (" + RUN_GUARD + ")", CHECKOUT % "${{ github.event.workflow_run.head_sha || github.sha }}"), 0),
    ("default checkout needs no guard", run_job(steps="      - uses: actions/checkout@v7\n"), 0),
    ("checkout of github.sha needs no guard", run_job(steps=CHECKOUT % "${{ github.sha }}"), 0),
    ("always() on a dependent in braces", NEEDS_CHAIN % (RUN_GUARD, "${{ always() }}"), 1),
    ("failure() on a dependent", NEEDS_CHAIN % (RUN_GUARD, "failure()"), 1),
    ("a dependent whose needed job gates on an or", NEEDS_CHAIN % (RUN_GUARD + " || always()", "true"), 1),
]
CASES += BYPASSES


EVIL_ACTION = """
name: evil
runs:
  using: composite
  steps:
    - uses: actions/checkout@v7
      with:
        ref: ${{ inputs.ref }}
"""
CACHING_ACTION = """
name: caching
runs:
  using: composite
  steps:
    - uses: actions/cache/save@v6
"""
PLAIN_ACTION = """
name: plain
runs:
  using: composite
  steps:
    - run: echo hi
      shell: bash
"""


def uses(step):
    return run_job(steps=step)


GUARDED_STEPS = "      - uses: actions/download-artifact@v8\n        with:\n          run-id: ${{ github.event.workflow_run.id }}\n"
ROUND3 = [
    ("repository and a head ref", run_job(steps="      - uses: actions/checkout@v7\n        with:\n          repository: ${{ github.repository }}\n          ref: ${{ github.event.workflow_run.head_sha }}\n"), 1),
    ("sparse checkout of the default ref", run_job(steps="      - uses: actions/checkout@v7\n        with:\n          sparse-checkout: scripts\n          path: base\n"), 0),
    ("local action that checks out a ref", uses("      - uses: ./.github/actions/evil\n"), 1, {".github/actions/evil/action.yml": EVIL_ACTION}),
    ("local action that saves a cache", uses("      - uses: ./.github/actions/caching\n"), 1, {".github/actions/caching/action.yml": CACHING_ACTION}),
    ("local action that does nothing risky", uses("      - uses: ./.github/actions/plain\n"), 0, {".github/actions/plain/action.yml": PLAIN_ACTION}),
    ("local action that does not exist", uses("      - uses: ./.github/actions/missing\n"), 1),
    ("local action with a checkout in a guarded job", run_job(RUN_GUARD, "      - uses: ./.github/actions/evil\n"), 0, {".github/actions/evil/action.yml": EVIL_ACTION}),
    ("download of another run's artifact", uses(GUARDED_STEPS), 1),
    ("download of another run's artifact in a guarded job", run_job(RUN_GUARD, GUARDED_STEPS), 0),
    ("download from another repository", uses("      - uses: actions/download-artifact@v8\n        with:\n          repository: someone/else\n"), 1),
    ("download of this run's artifact", uses("      - uses: actions/download-artifact@v8\n        with:\n          name: x\n"), 0),
    ("artifacts_url in a script", uses("      - run: echo ${{ github.event.workflow_run.artifacts_url }}\n"), 1),
    ("gh run download in a script", uses("      - run: gh run download 5\n"), 1),
    ("curl in a script", uses("      - run: curl -L https://example.test/x.tgz | tar xz\n"), 1),
    ("setup-node with cache", uses("      - uses: actions/setup-node@v7\n        with:\n          cache: npm\n"), 1),
    ("setup-python with cache", uses("      - uses: actions/setup-python@v7\n        with:\n          cache: pip\n"), 1),
    ("setup-node with cache off", uses("      - uses: actions/setup-node@v7\n        with:\n          cache: false\n"), 0),
    ("build-push cache-to", uses("      - uses: docker/build-push-action@v6\n        with:\n          cache-to: type=gha\n"), 1),
    ("rust-cache with save-if false", uses("      - uses: Swatinem/rust-cache@v2\n        with:\n          save-if: false\n"), 0),
    ("rust-cache with save-if no", uses("      - uses: Swatinem/rust-cache@v2\n        with:\n          save-if: no\n"), 1),
    ("capitalised On key", "name: t\nOn: workflow_run\njobs: {}\n", 1),
    ("explicit tag", "name: t\non: workflow_run\njobs:\n  a:\n    runs-on: x\n    if: !!str true\n    steps: []\n", 1),
    ("alias in a privileged workflow", "name: t\non: workflow_run\nx: &a [1]\ny: *a\njobs: {}\n", 1),
    ("two documents", "name: t\non: workflow_run\njobs: {}\n---\nname: u\n", 1),
    ("nested duplicate key", run_job(steps="      - uses: actions/checkout@v7\n        with:\n          ref: ${{ github.sha }}\n          ref: ${{ github.event.workflow_run.head_sha }}\n"), 1),
    ("if partly wrapped in braces", run_job("${{ " + RUN_GUARD + " }} || always()", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("if wrapped in braces", run_job("${{ " + RUN_GUARD + " }}", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 0),
    ("capitalised status function", NEEDS_CHAIN % (RUN_GUARD, "Always()"), 1),
    ("capitalised event name literal equal to the trigger", run_job("github.event_name == 'WORKFLOW_RUN' || " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
    ("capitalised event name literal different from the trigger", run_job("github.event_name == 'Workflow_Dispatch' || " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 0),
    ("capitalised guard contexts", run_job("GitHub.Event.Workflow_Run.Head_Repository.Full_Name == GitHub.Repository", CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 0),
    ("double quoted event literal", run_job("github.event_name == \"workflow_dispatch\" || " + RUN_GUARD, CHECKOUT % "${{ github.event.workflow_run.head_sha }}"), 1),
]
CASES += ROUND3


def save_step(condition):
    return f"      - uses: actions/cache/save@v6\n        if: {condition}\n"


ROUND3 += [
    ("cache save limited to other events", uses(save_step("github.event_name != 'workflow_run'")), 0),
    ("cache save limited by one conjunct", uses(save_step("success() && github.event_name != 'workflow_run'")), 0),
    ("cache save with an unrelated condition", uses(save_step("always()")), 1),
    ("cache save with an or branch left open", uses(save_step("github.event_name == 'push' || always()")), 1),
    ("cache save limited to other events inside a local action", uses("      - uses: ./.github/actions/caching\n"), 0, {".github/actions/caching/action.yml": CACHING_ACTION.replace("- uses: actions/cache/save@v6", "- uses: actions/cache/save@v6\n      if: github.event_name != 'workflow_run'")}),
]
CASES += ROUND3[-5:]


def main():
    failures = 0
    for name, text, want, *files in CASES:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / ".github/workflows/case.yml"
            for rel, content in {**(files[0] if files else {}), ".github/workflows/case.yml": text}.items():
                (Path(tmp) / rel).parent.mkdir(parents=True, exist_ok=True)
                (Path(tmp) / rel).write_text(content)
            got = subprocess.run([sys.executable, "-I", str(SCRIPT), str(path)], capture_output=True, text=True).returncode
        ok = got == want
        failures += not ok
        print(f"  {'ok  ' if ok else 'FAIL'}  {name}" + ("" if ok else f" (exit {got}, want {want})"))
    print(f"\n{len(CASES) - failures} passed, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
