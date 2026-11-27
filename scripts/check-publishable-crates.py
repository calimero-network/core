#!/usr/bin/env python3
"""Check the workspace crates a release publishes to crates.io: every member whose `publish` is not false.

Without arguments, check that crates.io would accept each one: a description and a
license, no dependency it could not publish with (a git source, or a path to a crate
that is not published), and `cargo package --list` succeeding. With `--published X`,
check that crates.io serves each one at version X, waiting a bounded time for its index.

    python3 scripts/check-publishable-crates.py
    python3 scripts/check-publishable-crates.py --published 1.0.0-rc.38
"""

import argparse
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

INDEX = "https://index.crates.io"  # the sparse index cargo resolves against
ATTEMPTS = 10  # index polls before a missing version fails the check
DELAY = 30  # seconds between polls


def workspace_crates():
    """Every workspace member, by name, from `cargo metadata`."""
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], stdout=subprocess.PIPE, text=True, check=True
    ).stdout
    return {p["name"]: p for p in json.loads(out)["packages"]}


def publishable(crates):
    return sorted(name for name, p in crates.items() if p["publish"] != [])


def refusals(pkg, published):
    """Why crates.io would refuse `pkg`, given the names of the crates published with it."""
    if not (pkg["description"] or "").strip():
        yield "no description"
    if not (pkg["license"] or pkg["license_file"]):
        yield "no license or license-file"
    for dep in pkg["dependencies"]:
        if dep["kind"] == "dev":
            continue
        if (dep["source"] or "").startswith("git+"):
            yield f"dependency {dep['name']} comes from git"
        elif dep.get("path") and dep["name"] not in published:
            yield f"dependency {dep['name']} is a path to a crate that is not published"


def check_packaging(crates):
    names = publishable(crates)
    failed = False
    for name in names:
        for reason in refusals(crates[name], names):
            print(f"{name}: {reason}", file=sys.stderr)
            failed = True
    listed = subprocess.run(
        ["cargo", "package", "--list", "--allow-dirty", *(arg for name in names for arg in ("-p", name))],
        capture_output=True, text=True,
    )
    if listed.returncode != 0:
        print(listed.stderr, file=sys.stderr)
        failed = True
    if failed:
        return 1
    print(f"{len(names)} crates packageable: {', '.join(names)}")
    return 0


def index_path(name):
    name = name.lower()
    if len(name) <= 2:
        return f"{len(name)}/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def indexed_versions(name):
    try:
        with urllib.request.urlopen(f"{INDEX}/{index_path(name)}", timeout=30) as response:
            return {json.loads(line)["vers"] for line in response.read().decode().splitlines() if line}
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return set()
        raise


def check_published(crates, version):
    missing = publishable(crates)
    for attempt in range(1, ATTEMPTS + 1):
        missing = [name for name in missing if version not in indexed_versions(name)]
        if not missing:
            print(f"crates.io serves every publishable crate at {version}")
            return 0
        if attempt < ATTEMPTS:
            print(f"not yet at {version} on crates.io: {', '.join(missing)}; polling again in {DELAY}s")
            time.sleep(DELAY)
    print(f"not on crates.io at {version}: {', '.join(missing)}", file=sys.stderr)
    return 1


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--published", metavar="VERSION", help="check crates.io serves every crate at VERSION")
    args = parser.parse_args()
    crates = workspace_crates()
    return check_published(crates, args.published) if args.published else check_packaging(crates)


if __name__ == "__main__":
    sys.exit(main())
