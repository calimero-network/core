#!/usr/bin/env python3
"""Fail a PR that changes an example app's Cargo.lock without changing a manifest that lockfile resolves.

Test builds rewrite these lockfiles on their own. An app lockfile may change only
with its own Cargo.toml, the root Cargo.toml (crates inherit their versions from
it), or the Cargo.toml of a path dependency it builds or tests with. The root
Cargo.lock does not count: every app is its own workspace and never reads it.
Reads the changed paths, one per line, from stdin.

    git diff --name-only --no-renames HEAD^1 HEAD | python3 scripts/check-app-lockfiles.py
"""

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("path_dep_closure", ROOT / "scripts" / "path-dep-closure.py")
path_dep_closure = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(path_dep_closure)


def resolved_manifests(app):
    """The Cargo.toml paths whose change may legitimately rewrite the lockfile in `app`."""
    try:
        dirs = path_dep_closure.closure([app], dev=True)
    except FileNotFoundError:  # the app or a dependency is gone, so only its own manifest can answer
        dirs = [app]
    return {"Cargo.toml", f"{app}/Cargo.toml", *(f"{d}/Cargo.toml" for d in dirs)}


def main():
    changed = {line.strip() for line in sys.stdin if line.strip()}
    offending = sorted(
        path for path in changed
        if path.startswith("apps/") and path.endswith("/Cargo.lock")
        and not resolved_manifests(path.removesuffix("/Cargo.lock")) & changed
    )
    for path in offending:
        print(f"{path} changed with no Cargo.toml it resolves.", file=sys.stderr)
        print(f"  Restore it:  git checkout origin/master -- {path}", file=sys.stderr)
    if offending:
        print(
            "A test build rewrites these lockfiles; only a dependency change in the app's Cargo.toml,"
            " the root Cargo.toml or a crates/*/Cargo.toml it depends on belongs in a PR.",
            file=sys.stderr,
        )
    return 1 if offending else 0


if __name__ == "__main__":
    sys.exit(main())
