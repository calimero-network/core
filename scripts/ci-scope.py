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

CARGO_MERO_SETUP = (".github/actions/setup-cargo-mero/*", "scripts/setup-cargo-mero.sh")

# What the apps job's cache key holds beyond the app trees (.github/actions/wasm-apps):
# the job must run whenever that key can change, or master never saves the new set.
WASM_APPS = (
    ".github/actions/wasm-apps/*",
    ".github/actions/rust-toolchain/*",
    ".github/workflows/e2e-rust-apps.yml",
    "scripts/check-embedded-abi.sh",
)


# workflow -> (its whole-workflow job, the inputs that run it)
WORKFLOWS = {
    "ci-checks": (
        "rust",
        (
            *EVERY_JOB,
            "crates/*",
            "apps/*",
            "e2e-tests/*",
            "tools/*",
            # Every script ci-checks.yml runs (ci-scope-test.py keeps this complete),
            # and what those scripts read.
            "scripts/build-all-apps.sh",
            "scripts/check-embedded-abi.sh",
            "scripts/check-like-ci.py",
            "scripts/check-naming.sh",
            "scripts/check-no-user-data-at-info.py",
            "scripts/check-rust-cache-groups.py",
            "scripts/check-scenario-coverage.py",
            "scripts/scenario-coverage-baseline.json",
            "scripts/check-storage-cost.sh",
            "scripts/check-wasm-size.sh",
            "scripts/tests/check-like-ci.test.sh",
            "scripts/tests/ci-scope-test.py",
            "scripts/tests/ci-paths-test.py",
            "scripts/tests/path-dep-closure-test.py",
            *CARGO_MERO_SETUP,
            ".github/actions/wasm-apps/*",
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


def app_packages():
    """Every package under apps/, which scripts/build-all-apps.sh builds."""
    return sorted(p.parent.relative_to(ROOT).as_posix() for p in ROOT.glob("apps/**/Cargo.toml"))


def cargo_mero_fixtures():
    """The apps cargo-mero's pipeline test builds; separate workspaces that path-depend on crates/."""
    return sorted(
        p.parent.relative_to(ROOT).as_posix()
        for p in (ROOT / "tools/cargo-mero/tests/fixtures").glob("**/Cargo.toml")
    )


# ci-checks.yml job -> (package dirs it builds, whether their dev-dependencies count, extra globs)
JOBS = {
    "storage_cost": (lambda: ["tools/storage-cost"], True, ("scripts/check-storage-cost.sh",)),
    "wasm_size": (
        lambda: ["apps/kv-store", "apps/kv-store-with-handlers", "apps/scaffolding-e2e", "tools/cargo-mero"],
        False,
        ("scripts/check-wasm-size.sh", *CARGO_MERO_SETUP),
    ),
    "cargo_mero_e2e": (
        lambda: ["tools/cargo-mero", *cargo_mero_fixtures()],
        True,
        ("tools/cargo-mero/*",),
    ),
    # tools/calimero-abi: build-all-apps.sh runs mero-abi over every wasm it built,
    # whether or not cargo-mero keeps linking it.
    "apps": (
        lambda: [*app_packages(), "tools/cargo-mero", "tools/calimero-abi"],
        False,
        ("apps/*", "scripts/build-all-apps.sh", *CARGO_MERO_SETUP, *WASM_APPS),
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
    """Every directory holding a Cargo.toml, nested ones included (cargo-mero's fixtures)."""
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
