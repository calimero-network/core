#!/usr/bin/env python3
"""Fail a PR that calls itself a one-way door without a reader or a declared cutover.

A one-way door (`Door: one-way` in the PR body, the "Merge Danger" or "Compatibility" block)
writes something the release before cannot read, or drops a reader of what earlier releases
wrote. Earlier data and clusters survive it only through a reader, so the body says which:

    Reader: <the migration or decoder for the old shape, and the test that reads it>
    Hard cutover: <what breaks and for whom>

and a hard cutover also adds a "Hard cutover" line to CHANGELOG.md in the same PR (mdma's
release_compat CUTOVERS takes the release that ships it). A PR that moves a format version
(`STORE_FORMAT`, `SNAPSHOT_FORMAT`, `ENTRY_FORMAT`, the WIT package version or
`SUPPORTED_ABI_VERSIONS`) must state its door at all.

Reads the body from $PR_BODY and the PR's diff from stdin:

    git diff HEAD^1 HEAD | python3 scripts/check-compat-door.py

The policy: docs/src/content/docs/operate/compatibility.mdx.
"""

from __future__ import annotations

import importlib.util
import os
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
_spec = importlib.util.spec_from_file_location(
    "check_trust_boundary", Path(__file__).resolve().parent / "check-trust-boundary.py"
)
_tb = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_tb)

DOOR_RE = re.compile(r"^\W*door\W*:\W*(one-way|two-way)\b", re.IGNORECASE)
READER_RE = re.compile(r"^\W*reader\W*:\s*\S", re.IGNORECASE)
CUTOVER_RE = re.compile(r"^\W*hard cutover\W*:\s*\S", re.IGNORECASE)
# A diff line that moves a format version: added or removed, in the files that hold them.
VERSION_LINE_RE = re.compile(
    r"^[+-](?![+-])\s*(?:pub\s+)?const\s+(?:STORE_FORMAT|SNAPSHOT_FORMAT|ENTRY_FORMAT)\b"
    r"|^[+-](?![+-])\s*package\s+calimero:app@"
    r"|^[+-](?![+-])\s*pub\s+const\s+SUPPORTED_ABI_VERSIONS\b",
    re.MULTILINE,
)


def changelog_adds_cutover(diff: str) -> bool:
    """Whether the diff adds a line naming a hard cutover to CHANGELOG.md."""
    in_changelog = False
    for line in diff.splitlines():
        if line.startswith("diff --git "):
            in_changelog = line.endswith(" b/CHANGELOG.md")
        elif in_changelog and line.startswith("+") and not line.startswith("+++"):
            if "hard cutover" in line.lower():
                return True
    return False


def problems(body: str, diff: str) -> list[str]:
    lines = list(_tb.visible_lines(body))
    doors = [m[1].lower() for line in lines if (m := DOOR_RE.match(line))]
    moves_a_version = bool(VERSION_LINE_RE.search(diff))
    if len(set(doors)) > 1:
        return ["the body says both `Door: one-way` and `Door: two-way`"]
    if not doors:
        if moves_a_version:
            return [
                "this PR moves a format version (STORE_FORMAT, SNAPSHOT_FORMAT, ENTRY_FORMAT, the WIT "
                "package or SUPPORTED_ABI_VERSIONS): say `Door: one-way` or `Door: two-way` in the body"
            ]
        return []
    if doors[0] == "two-way":
        return []
    reader = any(READER_RE.match(line) for line in lines)
    cutover = any(CUTOVER_RE.match(line) for line in lines)
    if not reader and not cutover:
        return [
            "`Door: one-way` needs `Reader: <the migration or decoder for the old shape, and its test>` "
            "or `Hard cutover: <what breaks and for whom>`"
        ]
    if cutover and not changelog_adds_cutover(diff):
        return ["`Hard cutover:` needs a line in CHANGELOG.md saying \"Hard cutover\" in this PR"]
    return []


def main() -> int:
    found = problems(os.environ.get("PR_BODY") or "", sys.stdin.read())
    for problem in found:
        print(f"::error::Compatibility: {problem}")
    if not found:
        print("Compatibility: nothing to declare, or declared.")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
