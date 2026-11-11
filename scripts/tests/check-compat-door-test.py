#!/usr/bin/env python3
"""Tests for check-compat-door.py: a one-way door names its reader or declares its cutover.

    python3 scripts/tests/check-compat-door-test.py
"""

import importlib.util
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-compat-door.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("check_compat_door", SCRIPT)
door = importlib.util.module_from_spec(spec)
spec.loader.exec_module(door)

PLAIN = "diff --git a/crates/node/src/api.rs b/crates/node/src/api.rs\n+fn x() {}\n"
BUMP = "diff --git a/crates/log/src/format.rs b/crates/log/src/format.rs\n" + "-pub const STORE_" + "FORMAT: u32 = 1;\n+pub const STORE_" + "FORMAT: u32 = 2;\n"
WIT = "diff --git a/crates/app-wit/wit/calimero-app.wit b/crates/app-wit/wit/calimero-app.wit\n" + "+" + "package calimero:app@0.5.2;\n"
CHANGELOG = "diff --git a/CHANGELOG.md b/CHANGELOG.md\n+- Sealed files move to segments. **Hard cutover**: no reader for whole-file records.\n"
ONE_WAY = "## Merge Danger\n\n**Door:** one-way\n"

CASES = [
    # (name, body, diff, problems expected)
    ("no door and no version moved: nothing to declare", "## Description\n", PLAIN, 0),
    ("a two-way door needs nothing more", "**Door:** two-way", PLAIN, 0),
    ("a one-way door alone fails", ONE_WAY, PLAIN, 1),
    ("a one-way door with its reader passes", ONE_WAY + "Reader: `format::MIGRATIONS[1]`, test `a_v1_store_opens`\n", BUMP, 0),
    ("an empty reader line does not count", ONE_WAY + "Reader:\n", PLAIN, 1),
    ("a hard cutover with its CHANGELOG line passes", ONE_WAY + "**Hard cutover:** clusters with sealed files are wiped\n", PLAIN + CHANGELOG, 0),
    ("a hard cutover without the CHANGELOG line fails", ONE_WAY + "Hard cutover: clusters are wiped\n", PLAIN, 1),
    ("a store format bump must state its door", "## Description\n", BUMP, 1),
    ("a WIT version move must state its door", "## Description\n", WIT, 1),
    ("a door in an HTML comment is not a door", "<!--\n**Door:** two-way\n-->", BUMP, 1),
    ("a door in a code fence is not a door", "```\nDoor: two-way\n```", BUMP, 1),
    ("both doors at once fail", "Door: one-way\nDoor: two-way\nReader: x\n", PLAIN, 1),
    ("the CHANGELOG line must be in CHANGELOG.md", ONE_WAY + "Hard cutover: x\n",
     "diff --git a/docs/x.md b/docs/x.md\n+Hard cutover\n", 1),
]

TEMPLATE = (Path(__file__).resolve().parent.parent.parent / ".github" / "pull_request_template.md").read_text()
CASES += [
    ("the template as it comes declares a two-way door", TEMPLATE, BUMP, 0),
    ("the template turned one-way needs its reader", TEMPLATE.replace("**Door:** two-way", "**Door:** one-way"), PLAIN, 1),
    ("its shapes list is not a reader",
     TEMPLATE.replace("**Door:** two-way", "**Door:** one-way").replace("two-way", "x"), PLAIN, 1),
]

failures = []
for name, body, diff, want in CASES:
    got = door.problems(body, diff)
    if len(got) != want:
        failures.append(f"{name}: got {got}, want {want} problem(s)")
if failures:
    print("\n".join(failures))
    sys.exit(1)
print(f"check-compat-door: {len(CASES)} cases pass")
