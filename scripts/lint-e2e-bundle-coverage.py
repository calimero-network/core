#!/usr/bin/env python3
"""Lint e2e workflows for scenarios that install a bundle the workflow never builds.

A merobox scenario installs a bundle by path, `dist/<package>-<version>.mpk`, but
nothing ties that path to the `cargo mero bundle` steps that fill `dist/`. Rename
a build step, or add an app without adding its step, and the scenario still looks
correct: it fails at run time with "Application path not found", naming the file
rather than the workflow that was supposed to build it.

The released-image lane sat red for days that way -- its one bundle step was
labelled multi-service but built the single-service app, so the multi-service
`.mpk` was never packaged. Nothing gates on that lane, so the failure was only
ever a red X nobody had to read.
"""

import re
import sys
import tomllib
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
# The workflows that build bundles and then run merobox scenarios against them.
WORKFLOW_GLOB = ".github/workflows/e2e-rust-apps*.yml"
# `(cd apps/<dir> && cargo mero bundle ...)` -- the only way `dist/` gets filled.
BUNDLE_RE = re.compile(r"cd\s+(apps/[\w.-]+)\s*&&\s*cargo\s+mero\s+bundle")
SCENARIO_RE = re.compile(r"(workflows/[\w./-]+\.yml)")


def app_packages():
    """App directory -> the package id its manifest declares."""
    packages = {}
    for manifest in ROOT.glob("apps/*/Cargo.toml"):
        with manifest.open("rb") as fh:
            meta = tomllib.load(fh)
        package = meta.get("package", {}).get("metadata", {}).get("calimero", {}).get("package")
        if package:
            packages[manifest.parent.relative_to(ROOT).as_posix()] = package
    return packages


def strings(node):
    """Every string in a parsed workflow, whatever nests it (matrix entries included)."""
    if isinstance(node, str):
        yield node
    elif isinstance(node, dict):
        for value in node.values():
            yield from strings(value)
    elif isinstance(node, list):
        for value in node:
            yield from strings(value)


def resolve(reference, app_dirs):
    """A scenario reference is relative to whichever app dir the step runs from."""
    for app_dir in app_dirs:
        candidate = ROOT / app_dir / reference
        if candidate.is_file():
            return candidate
    candidate = ROOT / reference
    return candidate if candidate.is_file() else None


def installed_packages(scenario, packages):
    """Package ids a scenario's install steps name, longest id first so -multi wins."""
    try:
        doc = yaml.safe_load(scenario.read_text()) or {}
    except yaml.YAMLError:
        return set()
    used = set()
    for step in doc.get("steps") or []:
        path = step.get("path") if isinstance(step, dict) else None
        if not path or not path.endswith(".mpk"):
            continue
        stem = Path(path).name
        for package in sorted(packages, key=len, reverse=True):
            if stem.startswith(package + "-"):
                used.add(package)
                break
    return used


def check(workflow, packages):
    doc = yaml.safe_load(workflow.read_text()) or {}
    values = list(strings(doc))

    app_dirs = {d for value in values for d in BUNDLE_RE.findall(value)}
    built = {packages[d] for d in app_dirs if d in packages}
    references = {r for value in values for r in SCENARIO_RE.findall(value)}

    problems = []
    for reference in sorted(references):
        scenario = resolve(reference, app_dirs)
        if scenario is None:
            continue
        for package in sorted(installed_packages(scenario, packages.values()) - built):
            problems.append(
                f"{workflow.relative_to(ROOT)}: {reference} installs {package}, "
                f"which no `cargo mero bundle` step in this workflow builds"
            )
    return problems


def main():
    packages = app_packages()
    workflows = sorted(ROOT.glob(WORKFLOW_GLOB))
    problems = [p for w in workflows for p in check(w, packages)]

    print(f"checked {len(workflows)} workflow(s) against {len(packages)} app bundle(s)")
    if not problems:
        print("OK")
        return 0
    for problem in problems:
        print(f"ERROR {problem}")
    print(
        f"\n{len(problems)} scenario install(s) reference a bundle the workflow never "
        "builds.\nAdd the missing `(cd apps/<app> && cargo mero bundle ...)` step, or "
        "point the scenario at a bundle the workflow does build."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
