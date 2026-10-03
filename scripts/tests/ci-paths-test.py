#!/usr/bin/env python3
"""No documentation-only change may start a code workflow, and no code may stop doing so.

For every `paths:` filter in .github/workflows, using GitHub's own matching
(patterns in order, a later `!pattern` excludes, a later pattern re-includes):

- no tracked Markdown file that ci-scope.py calls inert may match it, so a
  docs-only PR does not start a build, test or e2e it cannot change;
- every Markdown file some code reads (include_str!, a manifest's guide or
  readme) that the filter's own patterns reach must still match it, so excluding
  docs never hides an input.

Workflows that exist for documentation are exempt. A filter edited by hand that
reaches docs again fails here, naming the file.

    python3 scripts/tests/ci-paths-test.py
"""

import importlib.util
import re
import subprocess
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent.parent
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
DOCS_WORKFLOWS = {"docs-ci.yml", "docs-site.yml", "doc-update.yaml"}

spec = importlib.util.spec_from_file_location("ci_scope", ROOT / "scripts" / "ci-scope.py")
ci_scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci_scope)


def glob_regex(pattern):
    """GitHub's filter glob: `**` crosses `/`, `*` and `?` do not."""
    out, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out, i = out + "(?:.*/)?", i + 3
        elif pattern.startswith("**", i):
            out, i = out + ".*", i + 2
        elif pattern[i] == "*":
            out, i = out + "[^/]*", i + 1
        elif pattern[i] == "?":
            out, i = out + "[^/]", i + 1
        else:
            out, i = out + re.escape(pattern[i]), i + 1
    return re.compile(out + r"\Z")


def selects(patterns, path):
    """Whether a `paths:` list selects `path`: the last pattern that matches decides."""
    chosen = False
    for raw in patterns:
        negated = raw.startswith("!")
        if glob_regex(raw[1:] if negated else raw).match(path):
            chosen = not negated
    return chosen


def reaches(patterns, path):
    """Whether the list's positive patterns alone would select `path`."""
    return any(glob_regex(p).match(path) for p in patterns if not p.startswith("!"))


def path_filters(workflow):
    doc = yaml.safe_load(workflow.read_text())
    on = doc.get(True, doc.get("on")) or {}
    if not isinstance(on, dict):
        return
    for event in ("pull_request", "push"):
        spec = on.get(event)
        if isinstance(spec, dict) and isinstance(spec.get("paths"), list):
            yield event, spec["paths"]


def main():
    read_docs = ci_scope.code_read_docs()
    tracked = subprocess.run(
        ["git", "ls-files", "--", "*.md", "*.mdx"], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout.split()
    inert = [p for p in tracked if p not in read_docs]
    failures = 0
    for workflow in sorted((ROOT / ".github" / "workflows").glob("*.y*ml")):
        if workflow.name in DOCS_WORKFLOWS:
            continue
        for event, patterns in path_filters(workflow):
            label = f"{workflow.name} on {event}"
            leaks = [p for p in inert if selects(patterns, p)]
            hidden = [p for p in sorted(read_docs) if reaches(patterns, p) and not selects(patterns, p)]
            if leaks:
                failures += 1
                print(f"FAIL {label}: {len(leaks)} documentation file(s) start it, e.g. {leaks[0]}")
                print('     end its paths with "!**/*.md" and "!**/*.mdx"')
            if hidden:
                failures += 1
                print(f"FAIL {label}: excludes {', '.join(hidden)}, which code reads; re-include it after the exclusions")
            if not leaks and not hidden:
                print(f"ok   {label}")
    if failures:
        return 1
    print(f"no documentation-only change starts a code workflow ({len(inert)} docs, {len(read_docs)} read by code)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
