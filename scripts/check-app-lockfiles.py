#!/usr/bin/env python3
"""Keep every app's and Rust client's Cargo.lock current, and refuse a rewrite that nothing required.

Each app under apps/ and each client under sdks/clients/ with a Cargo.lock is its own
workspace with its own lockfile, which no other build updates. Two rules, for each:

- Current: `cargo update --workspace --locked` passes, so the lockfile matches its
  manifests; a crate manifest change must bring every app lockfile it reaches along.
- Required: a lockfile that differs from the base's changed only because the base's
  copy no longer matched this tree's manifests (a catch-up or a dependency change).
  A rewrite of a lockfile that was already current is churn from a local build.

    python3 scripts/check-app-lockfiles.py                # against the merge base with origin/master
    python3 scripts/check-app-lockfiles.py --base HEAD^1  # CI: the PR's merge commit against its base
"""

import argparse
import subprocess
import sys
from pathlib import Path

LOCKFILES = [":(glob)apps/**/Cargo.lock", ":(glob)sdks/clients/*/Cargo.lock"]  # workspaces no other build updates
ROOT = Path(subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip())


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, check=True).stdout


def current(lockfile):
    """Whether `lockfile` matches its manifests, and cargo's report when it does not."""
    done = subprocess.run(
        ["cargo", "update", "--workspace", "--locked", "--manifest-path", f"{lockfile.parent}/Cargo.toml"],
        cwd=ROOT, capture_output=True, text=True,
    )
    return done.returncode == 0, done.stderr


def base_copy_current(lockfile, base):
    """Whether the base's copy of `lockfile` would match this tree's manifests (None: the base has none)."""
    try:
        old = git("show", f"{base}:{lockfile.as_posix()}")
    except subprocess.CalledProcessError:
        return None
    path = ROOT / lockfile
    new = path.read_bytes()
    try:
        path.write_bytes(old)
        return current(lockfile)[0]
    finally:
        path.write_bytes(new)


def default_base():
    try:
        return git("merge-base", "origin/master", "HEAD").decode().strip()
    except subprocess.CalledProcessError:
        return "HEAD^1"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the revision this tree merges into (default: merge base with origin/master)")
    base = parser.parse_args().base or default_base()

    listed = git("ls-files", "--cached", "--others", "--exclude-standard", "--", *LOCKFILES).decode()
    lockfiles = sorted(Path(p) for p in set(listed.split()) if (ROOT / p).is_file())
    changed = set(git("diff", "--name-only", "--no-renames", base, "--", *LOCKFILES).decode().split())
    failed = False
    for lockfile in lockfiles:
        ok, report = current(lockfile)
        if not ok:
            failed = True
            print(f"{lockfile} does not match its manifests.", file=sys.stderr)
            print(f"  Update it:  cargo update --workspace --manifest-path {lockfile.parent}/Cargo.toml", file=sys.stderr)
            print("".join(f"    {line}\n" for line in report.strip().splitlines()), end="", file=sys.stderr)
        elif lockfile.as_posix() in changed and base_copy_current(lockfile, base):
            failed = True
            print(f"{lockfile} changed, but the base's copy already matches its manifests.", file=sys.stderr)
            print(f"  Restore it:  git checkout origin/master -- {lockfile}", file=sys.stderr)
    if failed:
        print(
            "A lockfile changes only when its manifests need it to: a dependency change in a Cargo.toml it"
            " resolves, or a catch-up of one that fell behind. To pin a newer dependency, raise it in a Cargo.toml.",
            file=sys.stderr,
        )
    else:
        print(f"{len(lockfiles)} app and client lockfiles current; none rewritten without need against {base}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
