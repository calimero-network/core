#!/usr/bin/env python3
"""Classify `group-join-mesh-not-ready` joiner logs across master runs.

The scenario partitions node-2 from node-1, has it join while partitioned, then
heals and waits for governance to converge. Because the partition happens BEFORE
the join, *every* run degrades — and only about half enter the failure mode at
all. So the job's own conclusion answers a different question than the one worth
asking: a green run says nothing about whether a recovery path was needed, and a
red one does not say which path was missing.

This asks the useful question instead. For each run it records whether the joiner
degraded, and which of the known recovery paths fired:

  #3462  beacon-driven pull       — a beacon proves a peer is reachable
  #3466  post-heal re-dial        — never yet observed firing in CI
  #3508  subscription repair      — stale mesh table, re-announce
  #3513  namespace-pull fallback  — no subscribers, fall back to a known holder

The point is the per-run breakdown. A degraded run with no path firing is the
gap; a batch where every degraded run shows a path is evidence the known set is
complete, which is a different and stronger finding than "it stopped failing".

Usage:
    scripts/classify-mesh-not-ready.py --runs 12
    scripts/classify-mesh-not-ready.py --runs 20 --keep /tmp/artifacts

Requires `gh` authenticated against calimero-network/core.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

WORKFLOW = "e2e-rust-apps.yml"
JOB_ARTIFACT_PREFIX = "logs-group-join-mesh-not-ready"

# Marker -> the change that introduced the path. Matched against the joiner's
# log as substrings, deliberately: these are tracing messages, and a regex over
# them would break on the field ordering tracing is free to change.
RECOVERY_PATHS: dict[str, str] = {
    "#3462 beacon pull": "stranded member: unverifiable beacon signals a reachable peer",
    "#3466 post-heal re-dial": "discovery book empty on disconnect; re-dialing from the peer cache",
    "#3508 subscription repair": "re-announcing to rebuild the mesh",
    "#3513 namespace-pull fallback": "falling back to a peer",
}

# A run "degraded" if the joiner lost its peer at all. Without this the counts
# are meaningless: a run that never degraded needed no recovery, and counting it
# as "no path fired" would report a gap that is not there.
DEGRADED_MARKERS = (
    "No peers",
    "no mesh peers",
    "partition",
    "Connection closed",
)

CONVERGED_MARKER = "Sync verification failed"


def sh(*args: str) -> str:
    return subprocess.run(args, capture_output=True, text=True, check=False).stdout


def recent_runs(limit: int) -> list[dict]:
    raw = sh(
        "gh", "run", "list",
        "--workflow", WORKFLOW,
        "--branch", "master",
        "--limit", str(limit),
        "--json", "databaseId,conclusion,headSha,createdAt",
    )
    try:
        return json.loads(raw or "[]")
    except json.JSONDecodeError:
        return []


def artifact_for(run_id: int, into: Path) -> Path | None:
    """Download the joiner-log artifact for one run, or None if absent."""
    raw = sh("gh", "api", f"repos/calimero-network/core/actions/runs/{run_id}/artifacts",
             "--jq", ".artifacts[] | select(.name | startswith(\"%s\")) | .id" % JOB_ARTIFACT_PREFIX)
    ids = [line for line in raw.split() if line.strip()]
    if not ids:
        return None
    dest = into / str(run_id)
    dest.mkdir(parents=True, exist_ok=True)
    blob = dest / "logs.zip"
    with blob.open("wb") as fh:
        proc = subprocess.run(
            ["gh", "api", f"repos/calimero-network/core/actions/artifacts/{ids[0]}/zip"],
            stdout=fh, stderr=subprocess.DEVNULL, check=False,
        )
    if proc.returncode != 0 or blob.stat().st_size == 0:
        return None
    try:
        with zipfile.ZipFile(blob) as zf:
            zf.extractall(dest)
    except zipfile.BadZipFile:
        return None
    return dest


def classify(logs: Path) -> dict:
    text = []
    for path in logs.rglob("*"):
        if path.is_file() and path.suffix in {".log", ".txt", ""}:
            try:
                text.append(path.read_text(errors="replace"))
            except OSError:
                continue
    blob = "\n".join(text)
    return {
        "degraded": any(m in blob for m in DEGRADED_MARKERS),
        "failed_to_converge": CONVERGED_MARKER in blob,
        "paths": [name for name, marker in RECOVERY_PATHS.items() if marker in blob],
        "bytes": len(blob),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=12,
                    help="how many recent master runs to inspect (the issue asks for >= 10)")
    ap.add_argument("--keep", type=Path, default=None,
                    help="keep downloaded artifacts here instead of a temp dir")
    args = ap.parse_args()

    runs = recent_runs(args.runs)
    if not runs:
        print("no runs found — is `gh` authenticated for calimero-network/core?", file=sys.stderr)
        return 1

    workdir = args.keep or Path(tempfile.mkdtemp(prefix="mesh-not-ready-"))
    workdir.mkdir(parents=True, exist_ok=True)

    rows = []
    for run in runs:
        rid = run["databaseId"]
        logs = artifact_for(rid, workdir)
        if logs is None:
            rows.append((rid, run["headSha"][:9], run["conclusion"], None))
            continue
        rows.append((rid, run["headSha"][:9], run["conclusion"], classify(logs)))

    print(f"{'run':<12} {'sha':<10} {'job':<10} {'degraded':<9} {'converged':<10} paths")
    print("-" * 92)
    degraded = 0
    unexplained = []
    for rid, sha, conclusion, c in rows:
        if c is None:
            print(f"{rid:<12} {sha:<10} {str(conclusion):<10} {'-':<9} {'-':<10} (no artifact)")
            continue
        if c["degraded"]:
            degraded += 1
        conv = "no" if c["failed_to_converge"] else "yes"
        paths = ", ".join(c["paths"]) or "NONE"
        print(f"{rid:<12} {sha:<10} {str(conclusion):<10} "
              f"{('yes' if c['degraded'] else 'no'):<9} {conv:<10} {paths}")
        if c["degraded"] and not c["paths"]:
            unexplained.append(rid)

    print()
    print(f"runs inspected: {len(rows)}   degraded: {degraded}")
    if unexplained:
        print(f"DEGRADED WITH NO RECOVERY PATH: {unexplained}")
        print("Each of these is a candidate fourth cause — read its joiner log directly.")
    else:
        print("Every degraded run shows at least one recovery path firing.")
        print("That is the evidence #3524 asks for; note WHICH paths, since #3466")
        print("has never been observed firing and absence there is its own finding.")

    if args.keep is None:
        shutil.rmtree(workdir, ignore_errors=True)
    else:
        print(f"\nartifacts kept in {workdir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
