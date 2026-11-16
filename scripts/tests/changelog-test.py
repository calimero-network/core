#!/usr/bin/env python3
"""Tests for changelog.py: entry files in changelog.d/ are checked and gathered into a release.

    python3 scripts/tests/changelog-test.py
"""

import importlib.util
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "changelog.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("changelog", SCRIPT)
cl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cl)

FIX = "### Fixed\n\n- **A fix** (`calimero-log`): it was wrong.\n"
ADD = "### Added\n\n- **A feature** (`meroctl`): it is new.\n  Two lines.\n"
BOTH = "### Fixed\n\n- **Another fix**: x.\n\n### Added\n\n- **Another feature**: y.\n"
HEAD = "# Changelog\n\n## [Unreleased]\n\n<!-- pointer -->\n\n"
OLD = "## [1.0.0-rc.1] - 2026-01-01\n\n### Added\n\n- **The first**: z.\n"

failures = []


def expect(name, got, want):
    if got != want:
        failures.append(f"{name}:\n  got  {got!r}\n  want {want!r}")


# parse: what a well-formed entry is.
for name, text, want_problems in [
    ("one section", FIX, 0),
    ("two sections", BOTH, 0),
    ("a leading comment is fine", "<!-- why -->\n" + FIX, 0),
    ("an unknown section", "### Fixes\n\n- x\n", 1),
    ("a section with no bullet", "### Fixed\n\nprose only\n", 1),
    ("text before any heading", "A fix.\n\n" + FIX, 1),
    ("a release heading inside an entry", "## [1.0.0]\n\n" + FIX, 1),
    ("an empty file", "", 1),
]:
    expect(f"parse: {name}", len(cl.parse(text, "x.md")[1]), want_problems)

# gather: sections in release order, entries in the order given.
body, problems = cl.gather([("a.md", FIX), ("b.md", ADD), ("c.md", BOTH)])
expect("gather: no problems", problems, [])
expect(
    "gather: order",
    body,
    "### Added\n\n- **A feature** (`meroctl`): it is new.\n  Two lines.\n\n- **Another feature**: y."
    "\n\n### Fixed\n\n- **A fix** (`calimero-log`): it was wrong.\n\n- **Another fix**: x.",
)

# release: a dated section under an empty Unreleased that keeps its pointer comment.
changelog = HEAD + OLD
body, _ = cl.release_body(changelog, [])
expect("release: nothing to gather", body, "")
out = cl.release(changelog, "1.0.0-rc.2", "2026-02-02", cl.gather([("a.md", FIX)])[0])
expect(
    "release: section placed",
    out,
    "# Changelog\n\n## [Unreleased]\n\n<!-- pointer -->\n\n## [1.0.0-rc.2] - 2026-02-02\n\n"
    "### Fixed\n\n- **A fix** (`calimero-log`): it was wrong.\n\n" + OLD,
)

# release: entries still written under Unreleased (from before changelog.d) are kept, first.
legacy = "# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- **Old style**: w.\n\n" + OLD
tmp = Path(tempfile.mkdtemp())
try:
    (tmp / "a.md").write_text(FIX)
    body, problems = cl.release_body(legacy, [tmp / "a.md"])
    expect("release: legacy problems", problems, [])
    expect("release: legacy first", body, "### Fixed\n\n- **Old style**: w.\n\n- **A fix** (`calimero-log`): it was wrong.")
    out = cl.release(legacy, "1.0.0-rc.2", "2026-02-02", body)
    expect("release: legacy emptied", cl.unreleased_text(out), "")

    # entry_files: README is not an entry; merge order follows the commit that added each file.
    repo = tmp / "repo"
    (repo / "changelog.d").mkdir(parents=True)
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    for i, name in enumerate(["zeta.md", "alpha.md"]):
        (repo / "changelog.d" / name).write_text(FIX)
        subprocess.run(git + ["add", "."], cwd=repo, check=True)
        env_date = f"2026-01-0{i + 1}T00:00:00Z"
        subprocess.run(
            git + ["commit", "-q", "-m", name, "--date", env_date],
            cwd=repo,
            check=True,
            env={"GIT_COMMITTER_DATE": env_date, "PATH": "/usr/bin:/bin"},
        )
    (repo / "changelog.d" / "README.md").write_text("# not an entry\n")
    (repo / "changelog.d" / "new.md").write_text(FIX)  # not committed yet: last
    expect(
        "entry_files: merge order",
        [p.name for p in cl.entry_files(repo / "changelog.d")],
        ["zeta.md", "alpha.md", "new.md"],
    )
finally:
    shutil.rmtree(tmp)

# The repository's own entries are well formed.
expect("this repository: check", cl.main(["check"]), 0)

if failures:
    print("\n".join(failures))
    sys.exit(1)
print("changelog: all cases pass")
