#!/usr/bin/env python3
"""Decide which CI jobs a change can affect.

A job is scoped by what it builds: the workspace packages it compiles, followed
through scripts/path-dep-closure.py (normal and build dependencies, plus the
roots' own dev-dependencies for a job that runs their tests), and the files it
reads beyond them. A change runs the job when it touches any of those, or a file
every job depends on (the lockfile, the root manifest, the toolchain, this
script, the workflow itself).

Documentation is inert: a Markdown file that no code reads changes no build,
test or bundle, so it runs nothing. Markdown that IS read counts as code:
anything named by include_str!/include_bytes! in a Rust source, and every
`guide`/`readme` a Cargo manifest names (a bundle embeds its guide).

A workflow that must report a required check on every PR (ci-checks.yml's `Rust`,
release.yml's `Build Binaries`) runs on every PR and asks this for a whole-
workflow verdict instead of using a `paths:` filter, which would need a stub
workflow to report the check whenever it skipped: `rust` and `build` are true
when any non-documentation file matches what that workflow used to filter on.

Reads changed paths on stdin, one per line. Prints `<job>=true|false` for every
job of the workflow, and the reasons on stderr. Fails closed: no input, an
error, or a path it cannot place runs every job.

Usage: ci-scope.py [--workflow ci-checks|release] < changed-paths
"""

from __future__ import annotations

import fnmatch
import importlib.util
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree

_spec = importlib.util.spec_from_file_location("path_dep_closure", ROOT / "scripts" / "path-dep-closure.py")
path_dep_closure = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(path_dep_closure)

DOC_SUFFIXES = (".md", ".mdx")

# Inputs of every scoped job.
EVERY_JOB = (
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    ".cargo/*",
    ".github/workflows/ci-checks.yml",
    ".github/actions/setup-rust-ci/*",
    "scripts/ci-scope.py",
    "scripts/path-dep-closure.py",
)



# workflow -> (its whole-workflow job, the inputs that run it)
WORKFLOWS = {
    "ci-checks": (
        "rust",
        (
            *EVERY_JOB,
            "crates/*",
            "apps/*",
            "tools/*",
            # Every script ci-checks.yml runs (ci-scope-test.py keeps this complete),
            # and what those scripts read.
            "scripts/check-like-ci.py",
            "scripts/check-naming.sh",
            "scripts/check-no-user-data-at-info.py",
            "scripts/check-rust-cache-groups.py",
            "scripts/check-scenario-discovery.py",
            "scripts/tests/check-like-ci.test.sh",
            "scripts/tests/ci-scope-test.py",
            "scripts/tests/ci-paths-test.py",
            "scripts/tests/path-dep-closure-test.py",
            "scripts/pack-component.py",
            "scripts/tests/pack-component-test.py",
            "scripts/install-mero-app-build.sh",
            "scripts/install-mero-compute.sh",
            # calimero-node's openapi test compares it with what the registry builds.
            "docs/public/openapi.json",
            "scripts/fetch-release.sh",
            "scripts/build-release-fixtures.sh",
            "scripts/build-upgrade-deep-fixtures.sh",
            "scripts/upgrade-dev-bundle.py",
            "scripts/upgrade-path.sh",
        ),
    ),
    "release": (
        "build",
        (
            "Cargo.lock",
            "Cargo.toml",
            "rust-toolchain.toml",
            ".cargo/*",
            "crates/*",
            "tools/*",
            ".github/workflows/release.yml",
            ".github/workflows/deps/*",
            ".github/actions/*",
            # COPYed into the profiling image by prebuilt.profiling.Dockerfile.
            "scripts/profiling/*",
            "scripts/ci-scope.py",
        ),
    ),
}


# ci-checks.yml job -> (package dirs it builds, whether their dev-dependencies count, extra
# globs). A job that builds only part of the workspace goes here, scoped by what it compiles.
JOBS: dict = {
    # The upgrade path: a cluster on the pinned release rolled onto this tree's merod.
    # merod's closure is every crate a node runs (node, storage, the log, the runtime,
    # sealed, attestation); the test and the scripts it runs are the rest. The old
    # release's fixtures come from its tag, so this tree's copies of them do not count.
    "upgrade_path": (
        # apps/v2-fixtures/kv: this tree's kv, the release the app upgrades to.
        lambda: ["crates/merod", "crates/meroctl", "tools/mero-sign", "crates/app-build", "apps/v2-fixtures/kv"],
        False,
        (
            ".github/upgrade-from",
            "crates/node/tests/fault/*",
            "scripts/fetch-release.sh",
            "scripts/build-release-fixtures.sh",
            "scripts/build-upgrade-deep-fixtures.sh",
            "scripts/upgrade-dev-bundle.py",
            "scripts/upgrade-path.sh",
            "scripts/pack-component.py",
            "scripts/install-mero-app-build.sh",
        ),
    ),
}


def tracked(pattern):
    out = subprocess.run(["git", "ls-files", "--", pattern], cwd=ROOT, check=True, capture_output=True, text=True)
    return out.stdout.split()


def code_read_docs():
    """Markdown some build, test or bundle reads, as repo-relative paths."""
    found = set()
    include = re.compile(r'include_(?:str|bytes)!\s*\(\s*"([^"]+)"')
    for source in tracked("*.rs"):
        text = (ROOT / source).read_text(encoding="utf-8", errors="replace")
        for target in include.findall(text):
            found.add((ROOT / source).parent / target)
    for manifest_path in tracked("*Cargo.toml"):
        with (ROOT / manifest_path).open("rb") as fh:
            meta = tomllib.load(fh)
        base = (ROOT / manifest_path).parent
        for table in (meta.get("package", {}), meta.get("workspace", {}).get("package", {})):
            if isinstance(table.get("readme"), str):
                found.add(base / table["readme"])
        for owner in (meta.get("package", {}), meta.get("workspace", {})):
            guide = owner.get("metadata", {}).get("calimero", {}).get("guide")
            if isinstance(guide, str):
                found.add(base / guide)
    docs = set()
    for path in found:
        resolved = path.resolve()
        if resolved.suffix in DOC_SUFFIXES and resolved.is_relative_to(ROOT):
            docs.add(resolved.relative_to(ROOT).as_posix())
    return docs


def is_inert(path, read_docs):
    return path.endswith(DOC_SUFFIXES) and path not in read_docs


def package_dirs():
    """Every directory holding a Cargo.toml, nested ones included (an app's own crates)."""
    return {Path(m).parent.as_posix() for m in tracked("*Cargo.toml")}


def owner(path, packages):
    """The deepest package directory holding `path`, so a nested package owns its own files."""
    best = None
    for d in packages:
        if (d == "." or path.startswith(d + "/")) and (best is None or len(d) > len(best)):
            best = d
    return best


def matches(path, globs):
    # fnmatch's `*` crosses `/`, so `apps/*` covers everything under apps/.
    return any(fnmatch.fnmatchcase(path, g) for g in globs)


def decide(paths, workflow):
    """{job: (runs, reason)} for the given changed paths."""
    whole, inputs = WORKFLOWS[workflow]
    jobs = [whole, *(JOBS if workflow == "ci-checks" else ())]
    read_docs = code_read_docs()
    code = [p for p in paths if not is_inert(p, read_docs)]
    if not code:
        return {job: (False, f"only documentation changed ({len(paths)} files)") for job in jobs}
    verdicts = {}
    hit = next((p for p in code if matches(p, inputs)), None)
    verdicts[whole] = (True, f"{hit} is an input") if hit else (False, "none of its inputs changed")
    if workflow != "ci-checks":
        return verdicts
    every = [p for p in code if matches(p, EVERY_JOB)]
    packages = package_dirs()
    owners = {p: owner(p, packages) for p in code}
    for job, (roots, dev, extra) in JOBS.items():
        if every:
            verdicts[job] = (True, f"{every[0]} is an input of every job")
            continue
        directories = set(path_dep_closure.closure(roots(), dev))
        hit = next((p for p in code if owners[p] in directories or matches(p, extra)), None)
        verdicts[job] = (True, f"{hit} is an input") if hit else (False, "none of its inputs changed")
    return verdicts


def main():
    args = sys.argv[1:]
    workflow = args[args.index("--workflow") + 1] if "--workflow" in args else "ci-checks"
    if workflow not in WORKFLOWS:
        print(f"unknown workflow {workflow!r}; known: {', '.join(WORKFLOWS)}", file=sys.stderr)
        return 2
    whole = WORKFLOWS[workflow][0]
    jobs = [whole, *(JOBS if workflow == "ci-checks" else ())]
    paths = [line.strip() for line in sys.stdin if line.strip()]
    try:
        if not paths:
            raise ValueError("no changed paths given")
        verdicts = decide(paths, workflow)
    except Exception as e:  # every failure runs every job: skipping is the silent direction
        print(f"failing closed: {e}", file=sys.stderr)
        verdicts = {job: (True, "failing closed") for job in jobs}
    for job, (runs, reason) in verdicts.items():
        print(f"{job}: {'runs' if runs else 'skips'} - {reason}", file=sys.stderr)
        print(f"{job}={'true' if runs else 'false'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
