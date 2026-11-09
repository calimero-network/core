#!/usr/bin/env python3
"""Fail when a scenario file on disk would never run.

The scenario runner (`cargo test -p calimero-node --test scenarios`) discovers:
- app-level scenarios, `apps/<app>/scenarios/*.json`;
- cluster scenarios, `apps/<app>/scenarios/cluster/**/*.json` and
  `workflows/**/scenarios/**/*.json`.
`*.org.json` files are org specs, not scenarios. A file under any directory named
`scenarios` that it would not discover must be listed, with a reason, in
`crates/node/tests/scenarios-exclude.txt`. The runner makes the same check when it
runs (crates/node/tests/scenarios.rs, `check_coverage`), but it runs in e2e.yml, which
is not a required check; this one runs in ci-checks' `scope` job, which `Rust`
requires, and needs no build.

Keep `discover()` here in step with the runner's; e2e.yml's scenario shards compare
this script's `--list` with the runner's `--list` and fail if they differ.

Usage: scripts/check-scenario-discovery.py [--list]
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXCLUDE = ROOT / "crates/node/tests/scenarios-exclude.txt"
SKIP_DIRS = {"target", "node_modules", "dist"}


def _json(paths):
    return [p for p in paths if p.is_file() and p.suffix == ".json" and not p.name.endswith(".org.json")]


def discover() -> list[str]:
    found = []
    for app in sorted((ROOT / "apps").iterdir()) if (ROOT / "apps").is_dir() else []:
        scen = app / "scenarios"
        if scen.is_dir():
            found += _json(scen.iterdir())
            if (scen / "cluster").is_dir():
                found += _json((scen / "cluster").rglob("*"))
    wf = ROOT / "workflows"
    if wf.is_dir():
        for d in wf.rglob("scenarios"):
            if d.is_dir():
                found += _json(d.rglob("*"))
    return sorted({str(p.relative_to(ROOT)) for p in found})


def on_disk() -> list[str]:
    out = []

    def walk(d: Path, inside: bool):
        for e in sorted(d.iterdir()):
            if e.is_dir():
                if e.name.startswith(".") or e.name in SKIP_DIRS:
                    continue
                walk(e, inside or e.name == "scenarios")
            elif inside and e.name.endswith(".json") and not e.name.endswith(".org.json"):
                out.append(str(e.relative_to(ROOT)))

    walk(ROOT, False)
    return sorted(out)


def exclusions() -> tuple[dict[str, str], list[str]]:
    out, problems = {}, []
    for line in EXCLUDE.read_text().splitlines() if EXCLUDE.exists() else []:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        path, _, why = line.partition(" ")
        if not why.strip():
            problems.append(f"{EXCLUDE.relative_to(ROOT)}: `{line}` gives no reason")
        out[path] = why.strip()
    return out, problems


def main(argv: list[str]) -> int:
    found = discover()
    if "--list" in argv:
        print("\n".join(found))
        return 0
    if not found:
        print("::error::no scenario files found", file=sys.stderr)
        return 1
    excluded, problems = exclusions()
    disk = on_disk()
    found_set, disk_set = set(found), set(disk)
    problems += [f"{p}: neither discovered nor in scenarios-exclude.txt" for p in disk if p not in found_set and p not in excluded]
    problems += [f"{p}: excluded in scenarios-exclude.txt but not on disk" for p in excluded if p not in disk_set]
    problems += [f"{p}: excluded in scenarios-exclude.txt but discovered" for p in excluded if p in found_set]
    for p in problems:
        print(f"::error::{p}", file=sys.stderr)
    if problems:
        return 1
    print(f"{len(found)} scenario files, all discovered ({len(excluded)} excluded)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
