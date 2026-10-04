#!/usr/bin/env python3
"""Print the workspace directories a package's binary is built from.

Follows every normal and build dependency that resolves to a path in this
workspace, directly or through `workspace = true`, including target-specific
and optional ones. Dev-dependencies are skipped: they never reach the binary.
With --dev, the named packages' own dev-dependencies are followed too, which is
what their tests compile (a dependency's dev-dependencies never are).
Registry and git dependencies are pinned by Cargo.lock, so they are not listed.

A cache keyed on these directories' git trees cannot go stale when a package
gains a dependency, which a hand-kept list would.

Usage: path-dep-closure.py [--dev] <package-dir>...   (e.g. tools/cargo-mero)
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SECTIONS = ("dependencies", "build-dependencies")
DEV_SECTIONS = SECTIONS + ("dev-dependencies",)


def manifest(directory):
    with (directory / "Cargo.toml").open("rb") as fh:
        return tomllib.load(fh)


def dependency_tables(meta, sections):
    for section in sections:
        yield meta.get(section, {})
    for target in meta.get("target", {}).values():
        for section in sections:
            yield target.get(section, {})


def path_deps(directory, workspace_deps, sections):
    for table in dependency_tables(manifest(directory), sections):
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


def closure(package_dirs, dev=False):
    """Workspace-relative directories the packages are built from (with --dev, tested with)."""
    workspace_deps = manifest(ROOT).get("workspace", {}).get("dependencies", {})
    roots = [(ROOT / d).resolve() for d in package_dirs]
    seen = set()
    stack = list(roots)
    while stack:
        directory = stack.pop()
        if directory in seen:
            continue
        if not (directory / "Cargo.toml").is_file():
            raise FileNotFoundError(f"no Cargo.toml in {directory}")
        seen.add(directory)
        sections = DEV_SECTIONS if dev and directory in roots else SECTIONS
        stack.extend(path_deps(directory, workspace_deps, sections))
    return sorted(d.relative_to(ROOT).as_posix() for d in seen)


def main():
    args = sys.argv[1:]
    dev = "--dev" in args
    dirs = [a for a in args if a != "--dev"]
    if not dirs:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        found = closure(dirs, dev)
    except FileNotFoundError as e:
        print(e, file=sys.stderr)
        return 1
    print("\n".join(found))
    return 0


if __name__ == "__main__":
    sys.exit(main())
