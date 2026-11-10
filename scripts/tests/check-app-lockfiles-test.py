#!/usr/bin/env python3
"""check-app-lockfiles.py must refuse an app lockfile change that comes without a manifest it resolves.

    python3 scripts/tests/check-app-lockfiles-test.py
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPT = ROOT / "scripts" / "check-app-lockfiles.py"
KV = "apps/kv-store"
NESTED = "apps/v2-fixtures/kv"

# (label, changed paths, expected exit status, substrings the output must contain)
CASES = [
    ("empty list passes", [], 0, []),
    ("unrelated path passes", ["crates/node/src/lib.rs"], 0, []),
    ("app source alone passes", [f"{KV}/src/lib.rs"], 0, []),
    ("lockfile alone fails", [f"{KV}/Cargo.lock"], 1, [f"{KV}/Cargo.lock", f"git checkout origin/master -- {KV}/Cargo.lock"]),
    ("lockfile with unrelated path fails", [f"{KV}/Cargo.lock", "README.md"], 1, [f"{KV}/Cargo.lock"]),
    ("lockfile with its manifest passes", [f"{KV}/Cargo.lock", f"{KV}/Cargo.toml"], 0, []),
    ("nested fixture with its manifest passes", [f"{NESTED}/Cargo.lock", f"{NESTED}/Cargo.toml"], 0, []),
    ("lockfile with another app's manifest fails", [f"{KV}/Cargo.lock", "apps/rooms/Cargo.toml"], 1, [f"{KV}/Cargo.lock"]),
    ("lockfile with a path dependency's manifest passes", [f"{KV}/Cargo.lock", "crates/sdk/Cargo.toml"], 0, []),
    ("lockfile with a dev-dependency's manifest passes", [f"{KV}/Cargo.lock", "crates/app-build/Cargo.toml"], 0, []),
    ("lockfile with an unrelated crate's manifest fails", [f"{KV}/Cargo.lock", "crates/merod/Cargo.toml"], 1, [f"{KV}/Cargo.lock"]),
    ("lockfile with the root manifest passes", [f"{KV}/Cargo.lock", "Cargo.toml"], 0, []),
    ("lockfile with only the root lockfile fails", [f"{KV}/Cargo.lock", "Cargo.lock"], 1, [f"{KV}/Cargo.lock"]),
    ("the root lockfile alone is not an app lockfile", ["Cargo.lock"], 0, []),
    ("a removed app's lockfile with its manifest passes", ["apps/gone/Cargo.lock", "apps/gone/Cargo.toml"], 0, []),
    ("a removed app's lockfile alone fails", ["apps/gone/Cargo.lock"], 1, ["apps/gone/Cargo.lock"]),
    (
        "every offending lockfile is named",
        [f"{KV}/Cargo.lock", "apps/rooms/Cargo.lock", f"{NESTED}/Cargo.lock", f"{NESTED}/Cargo.toml"],
        1,
        [f"{KV}/Cargo.lock", "apps/rooms/Cargo.lock"],
    ),
]


def run(paths):
    done = subprocess.run(
        [sys.executable, str(SCRIPT)], input="\n".join(paths) + "\n", capture_output=True, text=True, cwd=ROOT
    )
    return done.returncode, done.stdout + done.stderr


def check(label, paths, want, expect):
    got, out = run(paths)
    ok = got == want and all(s in out for s in expect)
    print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
    if not ok:
        print(f"     exit {got}, want {want}; output:\n{out}")
    return ok


def main():
    results = [check(*case) for case in CASES]
    # Every real app lockfile must pass beside its own manifest, so no app is rejected by the rule itself.
    for lock in sorted(ROOT.glob("apps/**/Cargo.lock")):
        rel = lock.relative_to(ROOT).as_posix()
        results.append(check(f"{rel} with its manifest passes", [rel, rel.replace("Cargo.lock", "Cargo.toml")], 0, []))
    failed = results.count(False)
    print(f"{len(results) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
