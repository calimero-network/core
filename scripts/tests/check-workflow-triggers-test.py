#!/usr/bin/env python3
"""Tests for check-workflow-triggers.py: the repository's workflows pass, each case fails with its error.

    python3 scripts/tests/check-workflow-triggers-test.py
"""

import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-workflow-triggers.py"
RUN_GUARD = "github.event.workflow_run.head_repository.full_name == github.repository"
HEAD = '{uses: actions/checkout@v7, with: {ref: "${{ github.event.workflow_run.head_sha }}"}}'
EVIL_ACTION = {".github/actions/evil/action.yml": f"runs:\n  using: composite\n  steps:\n    - {HEAD}\n"}


def workflow(*steps, condition="", on="workflow_run", needs=""):
    gate = f"  gate:\n    runs-on: x\n    if: {RUN_GUARD}\n    steps: []\n" if needs else ""
    head = f"    if: {condition}\n" if condition else ""
    head += f"    needs: {needs}\n" if needs else ""
    return f"on: {on}\njobs:\n{gate}  build:\n    runs-on: x\n{head}    steps:\n" + "".join(f"      - {s}\n" for s in steps)


CASES = [
    ("unguarded head checkout", "checks out", workflow(HEAD)),
    ("checkout of another repository", "checks out", workflow("{uses: actions/checkout@v7, with: {repository: someone/else}}")),
    ("another trigger's guard", "checks out", workflow(HEAD, condition=RUN_GUARD, on="pull_request_target")),
    ("guard or always()", "checks out", workflow(HEAD, condition=f"{RUN_GUARD} || always()")),
    ("event_name of the trigger or guard", "checks out", workflow(HEAD, condition=f"github.event_name == 'workflow_run' || {RUN_GUARD}")),
    ("negated guard", "checks out", workflow(HEAD, condition=f'"!({RUN_GUARD})"')),
    ("guard inside a string", "checks out", workflow(HEAD, condition=f"github.event.workflow_run.name == 'x && {RUN_GUARD} && y'")),
    ("always() drops a needed job's guard", "checks out", workflow(HEAD, condition="always()", needs="gate")),
    ("unknown checkout action", "unrecognised checkout action", workflow("{uses: someone/checkout-pr@v1}")),
    ("gh pr checkout", "fetches code", workflow("{run: gh pr checkout 5}")),
    ("another run's artifact", "downloads another run's artifact", workflow('{uses: actions/download-artifact@v8, with: {run-id: "1"}}')),
    ("artifacts_url", "reads `artifacts_url`", workflow("{run: 'echo ${{ github.event.workflow_run.artifacts_url }}'}")),
    ("reusable workflow", "calls a reusable workflow", "on: workflow_run\njobs:\n  a:\n    uses: ./.github/workflows/x.yml\n"),
    ("cache save with an open branch", "not read-only", workflow("{uses: actions/cache/save@v6, if: \"github.event_name == 'push' || always()\"}")),
    ("save-if no is a string", "set `save-if: false`", workflow("{uses: ./.github/actions/setup-rust-ci, with: {save-if: no}}")),
    ("cache input", "turns on a cache write", workflow("{uses: actions/setup-node@v7, with: {cache: npm}}")),
    ("local action checking out a head", "evil: checks out", workflow("{uses: ./.github/actions/evil}"), EVIL_ACTION),
    ("non-composite local action", "cannot read", workflow("{uses: ./.github/actions/node}"), {".github/actions/node/action.yml": "runs:\n  using: node20\n"}),
    ("unparsable yaml", "cannot be checked", "on: [workflow_run\n"),
    ("alias", "aliases", "on: workflow_run\nx: &a [1]\ny: *a\njobs: {}\n"),
    ("duplicate key", "duplicate key", workflow('{uses: actions/checkout@v7, with: {ref: "${{ github.sha }}", ref: x}}')),
    ("capitalised On", "another case", "On: workflow_run\njobs: {}\n"),
    ("unbalanced if", "unbalanced", workflow(HEAD, condition=f"({RUN_GUARD}")),
    ("closing parenthesis first", "unbalanced", workflow(HEAD, condition=f"{RUN_GUARD} && x) || (y")),
    ("malformed if", "malformed", workflow(HEAD, condition=f"{RUN_GUARD} || || true")),
    ("if mixing text and braces", "mixes text", workflow(HEAD, condition=f"${{{{ {RUN_GUARD} }}}} || always()")),
]


def run(*args):
    return subprocess.run([sys.executable, "-I", str(SCRIPT), *args], capture_output=True, text=True)


def main():
    real = run()
    results = [("the repository's workflows pass", real.returncode == 0, real.stdout)]
    for name, fragment, text, *files in CASES:
        with tempfile.TemporaryDirectory() as tmp:
            for rel, content in {**(files[0] if files else {}), ".github/workflows/case.yml": text}.items():
                (Path(tmp) / rel).parent.mkdir(parents=True, exist_ok=True)
                (Path(tmp) / rel).write_text(content)
            got = run(str(Path(tmp) / ".github/workflows/case.yml"))
        results.append((name, got.returncode == 1 and fragment in got.stdout, got.stdout))
    for name, ok, out in results:
        print(f"  {'ok  ' if ok else 'FAIL'}  {name}" + ("" if ok else f"\n{out}"))
    failures = sum(not ok for _, ok, _ in results)
    print(f"\n{len(results) - failures} passed, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
