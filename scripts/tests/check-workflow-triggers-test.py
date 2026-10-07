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


def main():
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        for i, (name, text, want) in enumerate(CASES):
            path = Path(tmp) / f"case{i}.yml"
            path.write_text(text)
            got = subprocess.run([sys.executable, "-I", str(SCRIPT), str(path)], capture_output=True, text=True).returncode
            ok = got == want
            failures += not ok
            print(f"  {'ok  ' if ok else 'FAIL'}  {name}" + ("" if ok else f" (exit {got}, want {want})"))
    print(f"\n{len(CASES) - failures} passed, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
