#!/usr/bin/env python3
"""Tests for the upgrade coverage matrix: generated from the deep cases, never kept by hand.

The matrix on contribute/testing is written by `upgrade_deep.py --write-matrix` when the
docs site builds, and what no case covers yet is a `not_covered(...)` call in the
`upgrade_cases/` file of its area, so a pull request that adds a case edits no shared table
or list. These checks keep it that way.

    python3 scripts/tests/upgrade-matrix-test.py
"""

import importlib.util
import json
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
FAULT = ROOT / "crates/node/tests/fault"
DOC = ROOT / "docs/src/content/docs/contribute/testing.mdx"
sys.dont_write_bytecode = True  # imports the suite's modules; leave no __pycache__ in the tree

spec = importlib.util.spec_from_file_location("upgrade_deep", FAULT / "upgrade_deep.py")
deep = importlib.util.module_from_spec(spec)
sys.modules["upgrade_deep"] = deep
spec.loader.exec_module(deep)

failures = []


def expect(name, ok, detail=""):
    if not ok:
        failures.append(f"{name}{': ' + detail if detail else ''}")


# The matrix: one row per case and per not-covered functionality, the same on every call.
first = deep.matrix()
expect("the matrix is the same when made twice (cases load once)", deep.matrix() == first)
rows = first.splitlines()[2:]
cases = [c for c in deep.upgrade_path.CASES]
expect("a row per case and per not-covered entry", len(rows) == len(cases) + len(deep.NOT_COVERED),
       f"{len(rows)} rows, {len(cases)} cases, {len(deep.NOT_COVERED)} not covered")
expect("no row twice", len(rows) == len(set(rows)))
expect("every row has five cells", all(r.count(" | ") == 4 for r in rows),
       next((r[:80] for r in rows if r.count(" | ") != 4), ""))

# Not covered: registered from the area files, each with its reason.
expect("the case files register what they do not cover", len(deep.NOT_COVERED) > 0)
for area, what, why in deep.NOT_COVERED:
    expect(f"not covered {what[:40]!r} names an area, a functionality and a reason", all([area, what, why]))
literal = re.search(r"^NOT_COVERED[^=\n]*=\s*\[\s*[^\]\s]", (FAULT / "upgrade_deep.py").read_text(), re.M)
expect("upgrade_deep.py keeps no list of not-covered entries (they live in the area files)", literal is None)

# --write-matrix: the generated file, rewritten only when it changes.
expect("upgrade_deep.py writes the matrix to a file (--write-matrix)", hasattr(deep, "write_matrix"))
if hasattr(deep, "write_matrix"):
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "nested" / "upgrade-matrix.md"
        expect("a first write writes", deep.write_matrix(out) is True)
        text = out.read_text()
        expect("the file is the note and the matrix", text == f"{deep.MATRIX_NOTE}\n\n{first}\n")
        mtime = out.stat().st_mtime_ns
        expect("an unchanged matrix is not written again", deep.write_matrix(out) is False)
        expect("an unchanged file keeps its time", out.stat().st_mtime_ns == mtime)
        out.write_text("stale\n")
        expect("a stale file is rewritten", deep.write_matrix(out) is True and out.read_text() == text)

# The docs include the generated file and keep no copy of the table.
doc = DOC.read_text()
expect("the testing page imports the generated matrix",
       "from '../../../generated/upgrade-matrix.md'" in doc and "<UpgradeMatrix />" in doc)
pasted = [line for line in doc.splitlines() if re.match(r"^\| .* \| (yes \(since|no: )", line)]
expect("the testing page holds no pasted matrix row", not pasted, pasted[0][:80] if pasted else "")
expect("the default output is the file the page imports",
       getattr(deep, "MATRIX_FILE", None) == ROOT / "docs/src/generated/upgrade-matrix.md")
expect("the generated file is not tracked", "src/generated/" in (ROOT / "docs/.gitignore").read_text().split())
scripts = json.loads((ROOT / "docs/package.json").read_text())["scripts"]
expect("npm run generate writes the matrix",
       "upgrade_deep.py --write-matrix src/generated/upgrade-matrix.md" in scripts.get("generate", ""))
for step in ("predev", "prestart", "prebuild"):
    expect(f"{step} generates", scripts.get(step) == "npm run generate")
expect("check builds through npm run build (so prebuild runs)", scripts.get("check", "").startswith("npm run build"))

if failures:
    print("FAIL\n  " + "\n  ".join(failures))
    sys.exit(1)
print(f"ok: {len(rows)} matrix rows ({len(cases)} cases, {len(deep.NOT_COVERED)} not covered)")
