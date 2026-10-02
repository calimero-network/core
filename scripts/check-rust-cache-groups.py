#!/usr/bin/env python3
"""Every job of a rust-cache group must hash the same environment.

setup-rust-ci's rust-cache key carries a hash of every non-empty variable named
CARGO*, CC*, CFLAGS*, CXX*, CMAKE* or RUST* the cache step sees. Jobs that share
a group (`shared-key`) but differ in one get different restore keys, so all but
the saver's variant miss the cache, and nothing says so. For every job that runs
setup-rust-ci this computes what that step sees - workflow, job and step `env`,
then the cache step's own `env`, which pins or blanks what changes nothing in the
cache - and fails when a group's jobs disagree, or when a variable is exported
through $GITHUB_ENV before the cache step, where it cannot be read statically.

    python3 scripts/check-rust-cache-groups.py
"""

import re
import sys
from collections import defaultdict
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
ACTION = ".github/actions/setup-rust-ci"
# rust-cache's own list (src/config.ts, `envPrefixes`).
HASHED_PREFIXES = ("CARGO", "CC", "CFLAGS", "CXX", "CMAKE", "RUST")
# rust-cache saves <workspace>/target whatever CARGO_TARGET_DIR says.
TARGET_DIR = "${{ github.workspace }}/target"
EXPORT = re.compile(r"\b([A-Z_][A-Z0-9_]*)=[^\n]*>>\s*\"?\$\{?GITHUB_ENV")


def hashed(name):
    return name.startswith(HASHED_PREFIXES)


def cache_step_env():
    action = yaml.safe_load((ROOT / ACTION / "action.yml").read_text())
    steps = [s for s in action["runs"]["steps"] if str(s.get("uses", "")).startswith("Swatinem/rust-cache@")]
    if len(steps) != 1:
        sys.exit(f"{ACTION}: expected one rust-cache step, found {len(steps)}")
    return {k: str(v) for k, v in (steps[0].get("env") or {}).items()}


def exported_by(step):
    """Hashed variables a step writes to $GITHUB_ENV, its own or a local action's."""
    texts = [str(step.get("run", ""))]
    uses = str(step.get("uses", ""))
    if uses.startswith("./"):
        action = ROOT / uses / "action.yml"
        if action.is_file():
            texts.append(action.read_text())
    return sorted({m.group(1) for text in texts for m in EXPORT.finditer(text) if hashed(m.group(1))})


def main():
    pinned = cache_step_env()
    groups = defaultdict(list)
    errors = []
    for path in sorted((ROOT / ".github" / "workflows").glob("*.y*ml")):
        workflow = yaml.safe_load(path.read_text()) or {}
        wf_env = workflow.get("env") or {}
        for job_id, job in (workflow.get("jobs") or {}).items():
            exported = []
            for step in job.get("steps") or []:
                if str(step.get("uses", "")) != f"./{ACTION}":
                    exported += exported_by(step)
                    continue
                where = f"{path.name}:{job_id}"
                seen = {**wf_env, **(job.get("env") or {}), **(step.get("env") or {})}
                target_dir = seen.get("CARGO_TARGET_DIR")
                if target_dir is not None and str(target_dir) != TARGET_DIR:
                    errors.append(f"{where}: CARGO_TARGET_DIR={target_dir} is not the {TARGET_DIR} rust-cache saves")
                for name in exported:
                    if name not in pinned:
                        errors.append(f"{where}: {name} is exported through $GITHUB_ENV before the cache step")
                seen = {k: str(v) for k, v in seen.items() if hashed(k)}
                seen.update(pinned)
                env = tuple(sorted((k, v) for k, v in seen.items() if v))
                group = (step.get("with") or {}).get("shared-key", "")
                groups[group].append((where, env))
    for group, jobs in sorted(groups.items()):
        if len({env for _, env in jobs}) > 1:
            lines = "\n".join(f"    {where}: {dict(env)}" for where, env in jobs)
            errors.append(f"group {group!r} hashes different environments, so its jobs cannot share it:\n{lines}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        print(f"\nAlign the job, or pin/blank the variable in {ACTION}'s rust-cache step.", file=sys.stderr)
        return 1
    for group, jobs in sorted(groups.items()):
        print(f"{group}: {len(jobs)} job(s), {dict(jobs[0][1])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
