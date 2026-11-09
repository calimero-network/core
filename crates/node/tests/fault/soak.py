"""Latency and throughput of a soak (fuzzy load) run, and the report CI shows.

A soak run times every request it sends to the cluster (one round trip each) and
every operation that passes (its requests, less its waits). At the end it reports,
per operation and per request kind (`call set`, `view get`, ...), how many ran, how
many failed, the calls per second over the run and the p50/p95/p99 latency in ms.

The report file is `{"version": 1, "runs": [...]}`, each run appended, so one file
holds every soak of a job: the format merobox's `fuzzy_test` wrote to
`MEROBOX_SOAK_REPORT`, which `scripts/soak-summary.sh` renders. Render one with

    python3 -I crates/node/tests/fault/soak.py report REPORT.json [...] --title TITLE
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
import time
from collections.abc import Iterable
from pathlib import Path
from typing import Any

VERSION = 1
PERCENTILES = (50, 95, 99)


def percentile(values: list[float], p: float) -> float:
    """Nearest-rank percentile of `values` (unsorted is fine); 0 when empty."""
    if not values:
        return 0.0
    ranked = sorted(values)
    rank = max(1, math.ceil(p / 100 * len(ranked)))
    return ranked[min(rank, len(ranked)) - 1]


def latency(seconds: list[float]) -> dict[str, float]:
    """p50/p95/p99, mean and max of `seconds`, in milliseconds."""
    ms = [s * 1000 for s in seconds]
    out = {f"p{p}": round(percentile(ms, p), 2) for p in PERCENTILES}
    out["mean"] = round(sum(ms) / len(ms), 2) if ms else 0.0
    out["max"] = round(max(ms), 2) if ms else 0.0
    return out


class _Series:
    def __init__(self) -> None:
        self.seconds: list[float] = []
        self.failed = 0

    def add(self, seconds: float, ok: bool) -> None:
        self.seconds.append(seconds)
        if not ok:
            self.failed += 1

    def summary(self, elapsed: float) -> dict[str, Any]:
        n = len(self.seconds)
        return {
            "count": n,
            "failed": self.failed,
            "per_s": round(n / elapsed, 2) if elapsed > 0 else 0.0,
            "latency_ms": latency(self.seconds),
        }


class SoakMetrics:
    """What one soak run measured."""

    def __init__(self, clock=time.monotonic) -> None:
        self.clock = clock
        self.started = clock()
        self.ended: float | None = None
        self.attempts: dict[str, int] = {}
        self.failed_ops: dict[str, int] = {}
        self.op_latency: dict[str, list[float]] = {}
        self.calls: dict[str, _Series] = {}
        self.by_step: dict[str, _Series] = {}

    def operation(self, name: str, seconds: float, ok: bool) -> None:
        self.attempts[name] = self.attempts.get(name, 0) + 1
        if ok:
            self.op_latency.setdefault(name, []).append(seconds)
        else:
            self.failed_ops[name] = self.failed_ops.get(name, 0) + 1

    def call(self, op: str, step: str, seconds: float, ok: bool) -> None:
        """One round trip `step` (e.g. `call set`) of operation `op`."""
        self.calls.setdefault(op, _Series()).add(seconds, ok)
        self.by_step.setdefault(step, _Series()).add(seconds, ok)

    def stop(self) -> None:
        self.ended = self.clock()

    @property
    def elapsed(self) -> float:
        return (self.ended if self.ended is not None else self.clock()) - self.started

    def summary(self) -> dict[str, Any]:
        elapsed = self.elapsed
        every = _Series()
        operations = {}
        for name in sorted(self.attempts):
            calls = self.calls.get(name, _Series())
            every.seconds += calls.seconds
            every.failed += calls.failed
            attempts = self.attempts[name]
            failed = self.failed_ops.get(name, 0)
            operations[name] = {
                "attempts": attempts,
                "passed": attempts - failed,
                "failed": failed,
                "ops_per_s": round(attempts / elapsed, 2) if elapsed > 0 else 0.0,
                "latency_ms": latency(self.op_latency.get(name, [])),
                "calls": calls.summary(elapsed),
            }
        attempts = sum(self.attempts.values())
        passed = attempts - sum(self.failed_ops.values())
        return {
            "seconds": round(elapsed, 2),
            "operations_total": attempts,
            "operations_passed": passed,
            "pass_rate": round(100.0 * passed / attempts, 2) if attempts else 0.0,
            "calls": every.summary(elapsed),
            "operations": operations,
            "steps": {k: v.summary(elapsed) for k, v in sorted(self.by_step.items())},
        }


def append_report(path: str | os.PathLike, run: dict) -> None:
    """Add `run` to the report at `path` (created if missing or unreadable)."""
    p = Path(path)
    report: dict[str, Any] = {"version": VERSION, "runs": []}
    try:
        old = json.loads(p.read_text())
        if isinstance(old, dict) and isinstance(old.get("runs"), list):
            report["runs"] = old["runs"]
    except (OSError, ValueError):
        pass
    report["runs"].append(run)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_name(p.name + ".tmp")
    tmp.write_text(json.dumps(report, indent=2) + "\n")
    tmp.replace(p)


def load_runs(paths: Iterable[str | os.PathLike]) -> list[dict]:
    runs: list[dict] = []
    for path in paths:
        report = json.loads(Path(path).read_text())
        runs += report.get("runs", [])
    return runs


def markdown(runs: list[dict], title: str = "Soak latency and throughput") -> str:
    """Per run, a row per operation and a total row; latencies in ms."""
    lines = [f"### {title}", ""]
    if not runs:
        return "\n".join([*lines, "No soak ran.", ""])
    lines += [
        "| Workflow | Operation | Ops | Pass % | Ops/s | Calls | Calls/s "
        "| p50 ms | p95 ms | p99 ms |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for run in runs:
        wf = str(run.get("workflow", "?")).replace("|", "\\|")
        for name, op in run.get("operations", {}).items():
            c = op["calls"]
            rate = 100.0 * op["passed"] / op["attempts"] if op["attempts"] else 0.0
            lat = c["latency_ms"]
            lines.append(
                f"| {wf} | {name} | {op['attempts']} | {rate:.1f} "
                f"| {op['ops_per_s']:.2f} | {c['count']} | {c['per_s']:.2f} "
                f"| {lat['p50']:.1f} | {lat['p95']:.1f} | {lat['p99']:.1f} |"
            )
        c = run["calls"]
        lat = c["latency_ms"]
        lines.append(
            f"| {wf} | **all** ({run['seconds']:.0f}s) | {run['operations_total']} "
            f"| {run['pass_rate']:.1f} "
            f"| {run['operations_total'] / run['seconds'] if run['seconds'] else 0:.2f} "
            f"| {c['count']} | {c['per_s']:.2f} "
            f"| {lat['p50']:.1f} | {lat['p95']:.1f} | {lat['p99']:.1f} |"
        )
    lines += [
        "",
        "Latency is per call (one round trip to a node); Calls/s is over the "
        "whole run, so an operation's share adds up to the total.",
        "",
    ]
    return "\n".join(lines)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Render soak reports as a Markdown table.")
    sub = ap.add_subparsers(dest="cmd", required=True)
    rep = sub.add_parser("report", help="REPORT.json ... as Markdown, on stdout")
    rep.add_argument("reports", nargs="+")
    rep.add_argument("--title", default="Soak latency and throughput")
    args = ap.parse_args(argv)
    sys.stdout.write(markdown(load_runs(args.reports), args.title))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
