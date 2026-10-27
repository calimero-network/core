#!/usr/bin/env python3
"""Run the release-v2 workflows: core's (workflows/**/*.yml) and every app's
(apps/*/workflows/**/*.yml).

Each runs on its own 3-node merod cluster through merobox's cloud-v2 backend
(`merobox.cloud_v2.run_workflow_outcome`). Needs MEROD_BINARY (or merod on
PATH) and the fixtures built (scripts/build-cloud-v2-fixtures.py: signed `.mpk`
releases and `cargo mero build` WASM, so each carries its ABI); CORE_WASM_DIR
defaults to target/cloud-v2-wasm. The blobs size sweep's random fixtures
(apps/blobs/scripts/gen-blob-fixtures.sh) are made when a workflow that reads
them is selected.

Usage: scripts/cloud-v2-workflows.py [--shard K/N | --smoke] [--json out.json]
                                     [--verbose] [--list] [workflow.yml ...]

`--smoke` runs the fixed list in cloud-v2-workflows-smoke.json (one workflow
per area; release.yml runs it against the merod inside a PR's Docker image),
whose `fixtures` are the build-cloud-v2-fixtures.py filters it needs.

`--shard K/N` runs the K-th of N shards (1-based): the fuzzy loads (the nightly
soak's long runs) are dealt one per shard, then the rest are packed by their
measured durations (cloud-v2-workflows-durations.json, the default for one not
measured yet), longest first onto the shard with the least so far, so every
shard is the same set on every run and about as long as the others.

Exit status is non-zero when a workflow in cloud-v2-workflows-baseline.json
(the paths that pass) fails. Many workflows were migrated from v1 by `merobox
cloud port`; their headers list the v1 steps that have no release-v2
counterpart.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path(__file__).with_name("cloud-v2-workflows-baseline.json")
DURATIONS = Path(__file__).with_name("cloud-v2-workflows-durations.json")
SMOKE = Path(__file__).with_name("cloud-v2-workflows-smoke.json")
#: Seconds assumed for a workflow with no measured duration.
DEFAULT_SECONDS = 20.0
BLOB_FIXTURES = "static/generated/"


def all_workflows() -> list[str]:
    """Every release-v2 workflow, as a path relative to the repository root."""
    files = sorted(ROOT.glob("workflows/**/*.yml")) + sorted(
        ROOT.glob("apps/*/workflows/**/*.yml")
    )
    return [str(f.relative_to(ROOT)) for f in files]


def shard(files: list[str], k: int, n: int) -> list[str]:
    """The K-th of N duration-balanced shards (1-based), in path order."""
    took = json.loads(DURATIONS.read_text()) if DURATIONS.exists() else {}
    load = [0.0] * n
    picked: list[list[str]] = [[] for _ in range(n)]
    # The fuzzy loads run 15 minutes each in the nightly soak, so they are
    # dealt out one per shard first; a shard holding two would run twice as long.
    soak = sorted(f for f in files if "/fuzzy-tests/" in f)
    for i, f in enumerate(soak):
        picked[i % n].append(f)
        load[i % n] += took.get(f, DEFAULT_SECONDS)
    rest = [f for f in files if f not in soak]
    for f in sorted(rest, key=lambda f: (-took.get(f, DEFAULT_SECONDS), f)):
        i = min(range(n), key=lambda i: (load[i], i))
        load[i] += took.get(f, DEFAULT_SECONDS)
        picked[i].append(f)
    return sorted(picked[k - 1])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("workflows", nargs="*")
    ap.add_argument("--shard", help="K/N: run the K-th of N shards")
    ap.add_argument("--smoke", action="store_true", help=f"run {SMOKE.name}'s list")
    ap.add_argument("--json", type=Path)
    ap.add_argument("--list", action="store_true", help="print the selection and exit")
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()
    if args.smoke and (args.workflows or args.shard):
        ap.error("--smoke picks its own workflows: no files, no --shard")
    files = args.workflows or all_workflows()
    if args.smoke:
        files = json.loads(SMOKE.read_text())["workflows"]
    if args.shard:
        k, n = (int(x) for x in args.shard.split("/"))
        if not 1 <= k <= n:
            ap.error("--shard K/N needs 1 <= K <= N")
        files = shard(files, k, n)
    if args.list:
        print(*files, sep="\n")
        return 0
    os.environ.setdefault("CORE_WASM_DIR", str(ROOT / "target/cloud-v2-wasm"))
    # The soak (fuzzy_test) runs for its own duration nightly; here, briefly.
    os.environ.setdefault("MEROBOX_FUZZY_SECONDS", "30")
    if any(BLOB_FIXTURES in Path(f).read_text() for f in files):
        subprocess.run([ROOT / "apps/blobs/scripts/gen-blob-fixtures.sh"], check=True)
    from merobox.cloud_v2.workflow import run_workflow_outcome

    results = []
    for f in files:
        path = Path(f).resolve()
        rel = str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)
        start = time.time()
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                outcome = run_workflow_outcome(str(path))
        except Exception as e:  # a crash in the harness is a failure, not an abort
            outcome = "failed"
            buf.write(f"\n{type(e).__name__}: {e}")
        ok = outcome == "passed"
        took = round(time.time() - start, 1)
        out = buf.getvalue()
        reason = "" if ok else (out.strip().splitlines() or ["?"])[-1][:300]
        results.append(
            {"workflow": rel, "ok": ok, "outcome": outcome, "seconds": took, "reason": reason}
        )
        tag = {"passed": "ok  ", "not_applicable": "N/A "}.get(outcome, "FAIL")
        print(f"{tag} {took:6.1f}s  {rel}  {reason}", flush=True)
        if args.verbose and not ok:
            print(out)
    if args.json:
        args.json.write_text(json.dumps(results, indent=2) + "\n")
    passed = {r["workflow"] for r in results if r["ok"]}
    baseline = set(json.loads(BASELINE.read_text())) if BASELINE.exists() else set()
    regressed = sorted(baseline & {r["workflow"] for r in results} - passed)
    total = sum(r["seconds"] for r in results)
    print(f"\n{len(passed)}/{len(results)} workflows pass ({total:.0f}s)")
    if regressed:
        print("REGRESSED:", *regressed, sep="\n  ")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
