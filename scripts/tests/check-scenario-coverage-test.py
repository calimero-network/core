#!/usr/bin/env python3
"""Tests for check-scenario-coverage.py, on small trees built in a temporary
directory (with the real runner script, so its globs and sharding are the
ones checked), then once against this repository.

Both wrong answers are silent: a workflow the check calls covered that no
shard runs gates nothing, and one it calls uncovered blocks a good PR. Each
case pins one rule.

    python3 scripts/tests/check-scenario-coverage-test.py
"""

import importlib.util
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPT = ROOT / "scripts/check-scenario-coverage.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree

spec = importlib.util.spec_from_file_location("coverage", SCRIPT)
coverage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(coverage)

E2E_SHARDED = """
jobs:
  scenarios:
    strategy:
      matrix:
        shard: [1, 2, 3]
    steps:
      - run: |
          python3 scripts/run-workflows.py --shard ${{ matrix.shard }}/3 \\
            --json out-${{ matrix.shard }}.json
"""

WF = "backend: cloud-v2\ncluster: {nodes: 3}\nsteps: [{type: wait, seconds: 0}]\n"


def tree(e2e: str, files: dict[str, str]) -> Path:
    root = Path(tempfile.mkdtemp(prefix="coverage-test-"))
    (root / "scripts").mkdir()
    shutil.copy(ROOT / "scripts/run-workflows.py", root / "scripts/run-workflows.py")
    (root / ".github/workflows").mkdir(parents=True)
    (root / ".github/workflows/e2e.yml").write_text(e2e)
    for path, text in files.items():
        p = root / path
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    return root


def wf() -> str:
    return WF


CASES = [
    # (name, e2e.yml, files, substrings the problems must name; empty: passes)
    (
        "every shard of a sharded job covers core's and the apps' workflows",
        E2E_SHARDED,
        {
            "workflows/a.yml": wf(),
            "workflows/sub/b.yml": wf(),
            "apps/x/workflows/c.yml": wf(),
            "apps/x/workflows/deep/d.yml": wf(),
        },
        [],
    ),
    (
        "a shard the matrix leaves out uncovers the run",
        E2E_SHARDED.replace("[1, 2, 3]", "[1, 3]"),
        {"workflows/a.yml": wf()},
        ["shards [2] of 3"],
    ),
    (
        "a v1 workflow left on disk runs nowhere",
        E2E_SHARDED,
        {"workflows/a.yml": wf(), "apps/x/workflows/v1.yml": "nodes: {count: 2}\nsteps: []\n"},
        ["apps/x/workflows/v1.yml: not a release-v2 workflow"],
    ),
    (
        "not_applicable with a reason is accepted",
        E2E_SHARDED,
        {"apps/x/workflows/na.yml": "backend: cloud-v2\nnot_applicable: no counterpart\n"},
        [],
    ),
    (
        "not_applicable needs its reason",
        E2E_SHARDED,
        {"apps/x/workflows/na.yml": "backend: cloud-v2\nnot_applicable: ''\n"},
        ["needs a reason"],
    ),
    (
        "a path named in a comment or an echo registers nothing",
        """
jobs:
  scenarios:
    steps:
      - run: |
          # python3 scripts/run-workflows.py --shard 1/1
          echo scripts/run-workflows.py
""",
        {"workflows/a.yml": wf()},
        ["in no shard", "workflows/a.yml: run by no e2e.yml shard"],
    ),
    (
        "a run of named files only covers nothing by itself",
        """
jobs:
  scenarios:
    steps:
      - run: python3 scripts/run-workflows.py workflows/a.yml
""",
        {"workflows/a.yml": wf()},
        ["runs named files only"],
    ),
    (
        "a --smoke run covers nothing by itself",
        """
jobs:
  scenarios:
    steps:
      - run: python3 scripts/run-workflows.py --smoke --json out.json
""",
        {"workflows/a.yml": wf()},
        ["in no shard", "workflows/a.yml: run by no e2e.yml shard"],
    ),
    (
        "a smoke list of baseline workflows passes",
        E2E_SHARDED,
        {
            "workflows/a.yml": wf(),
            "scripts/workflows-smoke.json": '{"workflows": ["workflows/a.yml"]}',
            "scripts/workflows-baseline.json": '["workflows/a.yml"]',
        },
        [],
    ),
    (
        "a smoke workflow off the baseline, or missing, is refused",
        E2E_SHARDED,
        {
            "workflows/a.yml": wf(),
            "scripts/workflows-smoke.json": '{"workflows": ["workflows/a.yml", "workflows/gone.yml"]}',
            "scripts/workflows-baseline.json": "[]",
        },
        ["workflows/a.yml is not on", "workflows/gone.yml does not exist"],
    ),
    (
        "an unsharded run covers everything",
        """
jobs:
  scenarios:
    steps:
      - run: python3 scripts/run-workflows.py --json out.json
""",
        {"workflows/a.yml": wf(), "apps/x/workflows/c.yml": wf()},
        [],
    ),
]


def main() -> int:
    failed = 0
    for name, e2e, files, want in CASES:
        root = tree(e2e, files)
        try:
            problems = coverage.check(root)
        finally:
            shutil.rmtree(root)
        text = "\n".join(problems)
        ok = all(w in text for w in want) and (bool(problems) == bool(want))
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
        if not ok:
            failed += 1
            print(f"     want {want}, got:\n       " + "\n       ".join(problems or ["(none)"]))
    r = subprocess.run([sys.executable, str(SCRIPT)], cwd=ROOT, capture_output=True, text=True)
    ok = r.returncode == 0
    print(f"{'ok  ' if ok else 'FAIL'} this repository's workflows are all covered")
    if not ok:
        failed += 1
        print(r.stdout[-2000:])
    print(f"{len(CASES) + 1 - failed}/{len(CASES) + 1} check-scenario-coverage cases passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
