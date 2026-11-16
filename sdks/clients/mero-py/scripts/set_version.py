#!/usr/bin/env python3
"""Write a release's version into Cargo.toml (and Cargo.lock) for the build.

    python scripts/set_version.py 1.0.0-rc.5      # prints 1.0.0rc5

The release workflow runs this in each build job, with the version of the
`v*` tag semantic-release pushed, so the sdist and every wheel carry it:
maturin reads the version from Cargo.toml (pyproject.toml declares it
dynamic) and writes its PEP 440 form (`1.0.0-rc.5` -> `1.0.0rc5`). Nothing is
committed: the tag is where a release's version is recorded, and the
Cargo.toml on master keeps whatever it says until the next hand edit.

Prints the PEP 440 version, which the publish job checks each file against.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAME = "calimero-client-py"
# What semantic-release makes: MAJOR.MINOR.PATCH, or a pre-release of it.
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-(alpha|beta|rc)\.(\d+))?$")
PEP440 = {"alpha": "a", "beta": "b", "rc": "rc"}


def pep440(version: str) -> str:
    m = SEMVER.match(version)
    if not m:
        raise ValueError(f"not a release version: {version!r}")
    major, minor, patch, pre, n = m.groups()
    return f"{major}.{minor}.{patch}" + (f"{PEP440[pre]}{n}" if pre else "")


def set_version(root: Path, version: str) -> str:
    """Write `version` into root's Cargo.toml and Cargo.lock; its PEP 440 form."""
    out = pep440(version)
    cargo = root / "Cargo.toml"
    text = cargo.read_text()
    # The first line-initial `version = ` is [package]'s: dependencies state
    # theirs inline (`{ version = ... }`), and [package] comes first.
    if not text.startswith("[package]\n"):
        raise ValueError("Cargo.toml does not start with [package]")
    text, n = re.subn(
        r'(?m)^version = "[^"]*"', f'version = "{version}"', text, count=1
    )
    if n != 1:
        raise ValueError("Cargo.toml has no [package] version")
    cargo.write_text(text)
    lock = root / "Cargo.lock"
    if lock.exists():
        text, n = re.subn(
            rf'(\[\[package\]\]\nname = "{re.escape(NAME)}"\nversion = )"[^"]*"',
            rf'\1"{version}"',
            lock.read_text(),
            count=1,
        )
        if n != 1:
            raise ValueError(f"Cargo.lock has no {NAME} package")
        lock.write_text(text)
    return out


def main(argv) -> int:
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        print(set_version(ROOT, argv[0]))
    except ValueError as e:
        print(e, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
