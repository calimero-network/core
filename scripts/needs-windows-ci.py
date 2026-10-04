#!/usr/bin/env python3
"""Decide whether a pull request needs the Windows jobs in platforms.yml.

Windows runs nightly on master and whenever a release is cut. On a pull request
it runs only when the change can reach code that Linux never compiles, which is
what these rules name:

- any Cargo manifest, the lockfile, a build script, the toolchain pin or
  .cargo/: a dependency or build change is how a Windows build breaks without a
  single line of Windows code moving (aws-lc-sys and librocksdb-sys both need
  Windows-only tooling);
- a Rust file that holds OS-conditional code, before or after the change:
  `cfg(windows)`, `cfg(unix)`, `target_os`, `target_family`, `std::os::windows`,
  `windows_sys`. Linux CI compiles none of the Windows half of such a file;
- the Windows lanes' own inputs: their workflow, this script, the scenarios and
  fixtures they run, the merobox setup they install with.

A change that reaches Windows only through a caller in ANOTHER file (a signature
changed here, used under `cfg(windows)` there) is not caught on the PR; the
nightly run catches it within a day, and the release build before anything ships.
Add the `ci:platforms` label to a PR to run every platform lane on it.

Reads changed paths on stdin, one per line. Prints `true` or `false` on stdout
and the reasons on stderr. Fails closed: anything it cannot read counts as
needing Windows.

Usage: needs-windows-ci.py [--base REV] < changed-paths
"""

from __future__ import annotations

import argparse
import fnmatch
import re
import subprocess
import sys
from pathlib import Path

ALWAYS = (
    "Cargo.lock",
    "Cargo.toml",
    "*/Cargo.toml",
    "build.rs",
    "*/build.rs",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/*",
    ".github/workflows/platforms.yml",
    ".github/actions/setup-merobox/*",
    "scripts/needs-windows-ci.py",
    "scripts/tests/needs-windows-ci-test.py",
    "scripts/e2e-fixture-registry.sh",
    "workflows/auth-seam.yml",
    "apps/kv-store/*",
    "crates/utils/fs/*",
    "crates/merod/tests/watchdog_live.rs",
)

OS_CONDITIONAL = re.compile(
    r"cfg(_attr)?!?\s*\([^\n]*\b(windows|unix|target_os|target_family)\b"
    r"|\bstd::os::(windows|unix)\b"
    r"|\bwindows_sys\b"
)


def always(path: str) -> bool:
    # fnmatch's `*` crosses `/`, so `*/Cargo.toml` matches at any depth.
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in ALWAYS)


def contents(path: str, base: str | None) -> list[str]:
    """The file as the PR leaves it and as the base had it; a missing side is skipped."""
    texts = []
    head = Path(path)
    if head.is_file():
        texts.append(head.read_text(encoding="utf-8", errors="replace"))
    if base:
        shown = subprocess.run(
            ["git", "show", f"{base}:{path}"], capture_output=True, text=True, errors="replace"
        )
        if shown.returncode == 0:
            texts.append(shown.stdout)
    return texts


def reasons(paths: list[str], base: str | None) -> list[str]:
    found = []
    for path in paths:
        if always(path):
            found.append(f"{path}: always runs Windows")
        elif path.endswith(".rs"):
            texts = contents(path, base)
            if not texts:
                found.append(f"{path}: unreadable on both sides")
            elif any(OS_CONDITIONAL.search(text) for text in texts):
                found.append(f"{path}: holds OS-conditional code")
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="git revision the PR is based on")
    args = parser.parse_args()

    paths = [line.strip() for line in sys.stdin if line.strip()]
    if not paths:
        print("no changed paths given; failing closed", file=sys.stderr)
        print("true")
        return 0
    if args.base and subprocess.run(
        ["git", "cat-file", "-e", f"{args.base}^{{commit}}"], capture_output=True
    ).returncode != 0:
        print(f"base {args.base} is not available; failing closed", file=sys.stderr)
        print("true")
        return 0

    found = reasons(paths, args.base)
    for reason in found:
        print(reason, file=sys.stderr)
    if not found:
        print(f"none of {len(paths)} changed paths reaches Windows-only code", file=sys.stderr)
    print("true" if found else "false")
    return 0


if __name__ == "__main__":
    sys.exit(main())
