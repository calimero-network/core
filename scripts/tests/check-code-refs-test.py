#!/usr/bin/env python3
"""Tests for check-code-refs.py: every reference in a comment leads somewhere.

    python3 scripts/tests/check-code-refs-test.py
"""

import importlib.util
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-code-refs.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("check_code_refs", SCRIPT)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

failures = []


def expect(name, got, want):
    if got != want:
        failures.append(f"{name}: got {got!r}, want {want!r}")


def tags(text):
    return [m.group(0) for m in check.TAG.finditer(text) if m.group(0) not in check.NOT_TAGS]


def issues(text):
    return [m.group(1) for m in check.ISSUE.finditer(text)]


# Tags: a short id is a finding; a type, an algorithm or code in backticks is not.
expect("a decision id", tags("/// HTTP through the gate (S9, R14)."), ["S9", "R14"])
expect("a review finding", tags("//! the log store (review finding F6)."), ["F6"])
expect("a ruling", tags("/// I1 (Ruling 39): calls run off the async workers."), ["I1", "Ruling 39"])
expect("types and algorithms", tags("/// a U64 as a decimal string, signed with ES256 over SHA256"), [])
expect("a version is not a tag", tags("/// since 1.0.0-rc.36 and v0.27"), [])

# Inline code is not prose.
prose = check.BACKTICKED.sub(" ", "/// the app clock starts at `T0`; `RC16` is a release (D20)")
expect("backticks are stripped before tags", tags(prose), ["D20"])

# Issues: a bare number is a finding; a link, an anchor or an entity is not.
expect("a bare issue", issues("// here at once (#4723)."), ["4723"])
expect("an issue at the start", issues("#5210 broke it"), ["5210"])
expect("an html entity", issues("&#1234;"), [])
expect("a fragment", issues("see page#1234"), [])
expect("a short number", issues("step #12"), [])

# Paths.
expect("a repository path", check.looks_like_path("crates/node/src/api.rs"), True)
expect("a relative Markdown path", check.looks_like_path("../docs/src/content/docs/protocol/xcall.mdx"), True)
expect("a placeholder", check.looks_like_path("crates/<crate>/AGENTS.md"), False)
expect("a glob", check.looks_like_path("crates/*/Cargo.toml"), False)
expect("one segment", check.looks_like_path("crates"), False)
expect("not under a known root", check.looks_like_path("src/lib.rs"), False)

# Heading anchors, as github-slugger makes them.
expect("words and code", check.slug("The `cluster` version"), "the-cluster-version")
expect("punctuation goes", check.slug("Changing a stored format (v2)"), "changing-a-stored-format-v2")
expect("a link keeps its words", check.slug("See [Upgrades](/operate/upgrades/)"), "see-upgrades")

with tempfile.TemporaryDirectory() as d:
    page = Path(d) / "page.mdx"
    page.write_text("# Title\n\n## Format versions\n\n```sh\n# not a heading\n```\n\n## Format versions\n")
    expect("anchors, a repeat numbered, a fence skipped",
           check.page_anchors(page), {"title", "format-versions", "format-versions-1"})

    rs = Path(d) / "lib.rs"
    rs.write_text('let u = "https://x/y"; // (D20)\nfn f() {}\n/// doc (R8)\n')
    expect("only a Rust file's comments are prose",
           [(n, t.strip()) for n, t in check.prose_lines(rs)], [(1, "// (D20)"), (3, "/// doc (R8)")])

    md = Path(d) / "AGENTS.md"
    md.write_text("prose (S9)\n```\ncode (S8)\n```\nmore\n")
    expect("a Markdown file outside fences", [n for n, _ in check.prose_lines(md)], [1, 5])

# Real pages: the docs this repository links from code resolve.
expect("a page", check.site_page("protocol/overview/") is not None, True)
expect("a missing page", check.site_page("protocol/no-such-page/"), None)
expect("the cluster version anchor", "the-cluster-version" in check.page_anchors(check.site_page("protocol/upgrades")), True)
expect("a redirect is known", "protocol/sync" in check.redirects(), True)

if failures:
    print("\n".join(failures))
    sys.exit(1)
print("check-code-refs: all tests pass")
