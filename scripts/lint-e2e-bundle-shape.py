#!/usr/bin/env python3
"""Lint merobox scenarios for install/step combinations the node cannot execute.

A bundle manifest carries EITHER a top-level `wasm` OR a named `services[]` array,
never both, so a context created against a multi-service bundle must name the
service it runs. `create_context` takes `service_name`; `create_mesh` has no such
field on any released merobox, so it cannot drive a multi-service bundle at all.

Without this check the mismatch is invisible until CI: a scenario repointed at a
multi-service `.mpk` installs fine, then every `create_context` returns HTTP 500
with "bundle manifest declares no top-level wasm" — a message that names the
bundle, not the scenario that chose it. Sixty-four scenarios failed that way at
once, and the shape only became clear from a node-log artifact.
"""

import sys
import tomllib
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
# Scenario roots, matching the sets e2e-rust-apps.yml runs.
SCENARIO_GLOBS = ("apps/*/workflows/**/*.yml", "workflows/**/*.yml")
CONTEXT_STEPS = ("create_context", "create_mesh")


def multi_service_packages():
    """Package ids whose `[package.metadata.calimero]` declares named services."""
    found = set()
    for manifest in ROOT.glob("apps/**/Cargo.toml"):
        with manifest.open("rb") as fh:
            meta = tomllib.load(fh)
        calimero = meta.get("package", {}).get("metadata", {}).get("calimero", {})
        if calimero.get("package") and calimero.get("services"):
            found.add(calimero["package"])
    return found


def installed_packages(steps, multi):
    """Multi-service package ids a scenario's install steps name."""
    used = set()
    for step in steps:
        path = step.get("path")
        if step.get("type") != "install_application" or not path:
            continue
        stem = Path(path).name
        if not stem.endswith(".mpk"):
            continue
        # `<package>-<version>.mpk`; longest id wins so `-multi` beats its prefix.
        for package in sorted(multi, key=len, reverse=True):
            if stem.startswith(package + "-"):
                used.add(package)
                break
    return used


def check(scenario, multi):
    try:
        doc = yaml.safe_load(scenario.read_text()) or {}
    except yaml.YAMLError as exc:
        return [f"{scenario.relative_to(ROOT)}: unparseable ({exc})"]

    steps = [s for s in (doc.get("steps") or []) if isinstance(s, dict)]
    used = installed_packages(steps, multi)
    if not used:
        return []

    named = ", ".join(sorted(used))
    problems = []
    for step in steps:
        kind = step.get("type")
        if kind not in CONTEXT_STEPS:
            continue
        label = step.get("name", kind)
        if kind == "create_mesh":
            problems.append(
                f"{scenario.relative_to(ROOT)}: step '{label}' uses create_mesh, which "
                f"cannot name a service, against multi-service bundle {named}"
            )
        elif not step.get("service_name"):
            problems.append(
                f"{scenario.relative_to(ROOT)}: step '{label}' omits service_name "
                f"against multi-service bundle {named}"
            )
    return problems


def main():
    multi = multi_service_packages()
    if not multi:
        print("no multi-service app manifests found; nothing to check")
        return 0

    scenarios = sorted({p for g in SCENARIO_GLOBS for p in ROOT.glob(g)})
    problems = [p for s in scenarios for p in check(s, multi)]

    print(f"checked {len(scenarios)} scenarios against {len(multi)} multi-service bundle(s)")
    if not problems:
        print("OK")
        return 0
    for problem in problems:
        print(f"ERROR {problem}")
    print(
        f"\n{len(problems)} step(s) drive a multi-service bundle without naming a service.\n"
        "Point the scenario at a single-service bundle, or give each create_context a "
        "service_name (create_mesh cannot select one)."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
