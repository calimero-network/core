#!/usr/bin/env python3
"""Run core's ported workflows (workflows/cloud-v2/**/*.yml).

Each runs on its own 3-node merod cluster through merobox's cloud-v2 backend
(`merobox.cloud_v2.run_workflow`). Needs MEROD_BINARY (or merod on PATH) and
the fixtures built (scripts/build-cloud-v2-fixtures.py: signed `.mpk` releases
and `cargo mero build` WASM, so each carries its ABI); CORE_WASM_DIR defaults
to target/cloud-v2-wasm.

Usage: scripts/cloud-v2-workflows.py [--json out.json] [--verbose] [workflow.yml ...]
Exit status is non-zero when a workflow in cloud-v2-workflows-baseline.json fails.

The ports come from `merobox cloud port` (regenerate with
scripts/port-workflows-to-cloud-v2.sh); each file's header lists what it
dropped. A port marked `not_applicable` is not run and is no pass.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASELINE = Path(__file__).with_name("cloud-v2-workflows-baseline.json")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("workflows", nargs="*", type=Path)
    ap.add_argument("--json", type=Path)
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()
    os.environ.setdefault("CORE_WASM_DIR", str(ROOT / "target/cloud-v2-wasm"))
    # The soak (fuzzy_test) runs for its own duration nightly; here, briefly.
    os.environ.setdefault("MEROBOX_FUZZY_SECONDS", "30")
    from merobox.cloud_v2.workflow import run_workflow_outcome

    files = args.workflows or sorted(ROOT.glob("workflows/cloud-v2/**/*.yml"))
    results = []
    for f in files:
        path = f.resolve()
        rel = str(path.relative_to(ROOT)) if path.is_relative_to(ROOT) else str(path)
        start = time.time()
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                outcome = run_workflow_outcome(str(f))
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
    print(f"\n{len(passed)}/{len(results)} ported workflows pass")
    if regressed:
        print("REGRESSED:", *regressed, sep="\n  ")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
