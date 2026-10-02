#!/usr/bin/env python3
"""Print the workspace directories a package's binary is built from.

Follows every normal and build dependency that resolves to a path in this
workspace, directly or through `workspace = true`, including target-specific
and optional ones. Dev-dependencies are skipped: they never reach the binary.
Registry and git dependencies are pinned by Cargo.lock, so they are not listed.

A cache keyed on these directories' git trees cannot go stale when a package
gains a dependency, which a hand-kept list would.

Usage: path-dep-closure.py <package-dir>...   (e.g. tools/cargo-mero)
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SECTIONS = ("dependencies", "build-dependencies")


def manifest(directory):
    with (directory / "Cargo.toml").open("rb") as fh:
        return tomllib.load(fh)


def dependency_tables(meta):
    for section in SECTIONS:
        yield meta.get(section, {})
    for target in meta.get("target", {}).values():
        for section in SECTIONS:
            yield target.get(section, {})


def path_deps(directory, workspace_deps):
    for table in dependency_tables(manifest(directory)):
        for name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            if spec.get("workspace"):
                spec = workspace_deps.get(name, {})
                if not isinstance(spec, dict):
                    continue
                base = ROOT
            else:
                base = directory
            if "path" in spec:
                yield (base / spec["path"]).resolve()


def main():
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    workspace_deps = manifest(ROOT).get("workspace", {}).get("dependencies", {})
    seen = set()
    stack = [(ROOT / arg).resolve() for arg in sys.argv[1:]]
    while stack:
        directory = stack.pop()
        if directory in seen:
            continue
        if not (directory / "Cargo.toml").is_file():
            print(f"no Cargo.toml in {directory}", file=sys.stderr)
            return 1
        seen.add(directory)
        stack.extend(path_deps(directory, workspace_deps))
    for directory in sorted(seen):
        print(directory.relative_to(ROOT).as_posix())
    return 0


if __name__ == "__main__":
    sys.exit(main())
