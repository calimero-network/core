#!/usr/bin/env python3
"""Fail a PR that touches a trust-boundary path without answering the "Trust boundary" block.

Reads the PR body from $PR_BODY and the changed paths, one per line, from stdin.
The required items are the ones in the PR template, so the two cannot drift.

    git diff --name-only --no-renames HEAD^1 HEAD | python3 scripts/check-trust-boundary.py
"""

import os
import re
import sys
from pathlib import Path

TEMPLATE = Path(__file__).resolve().parent.parent / ".github" / "pull_request_template.md"
TRUST_BOUNDARY_PATHS = (  # a directory, or the `<path>.rs` module file beside it
    ".github/workflows",
    "crates/account",
    "crates/auth",
    "crates/authz",
    "crates/bundle",
    "crates/context",
    "crates/governance-store",
    "crates/governance-types",
    "crates/network",
    "crates/node",
    "crates/runtime/src/logic/host_functions",
    "crates/server",
)
SECTION_TITLE = "trust boundary"
INVISIBLE_CHARS = " ​‌‍⁠﻿"  # render as nothing or as a plain space
HEADING_RE = re.compile(r"^ {0,3}#{1,6}\s+(.*?)\s*#*\s*$")
FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})")
HIDING_RE = re.compile(r"<!--|-->|<(/?)(script|style|textarea|details)\b[^>]*>", re.IGNORECASE)
CODE_SPAN_RE = re.compile(r"(`+)(?!`).*?(?<!`)\1(?!`)")
LIST_LINE_RE = re.compile(r"^ {0,3}[-*+]\s")
BOX_RE = re.compile(r"\[[ xX]\]")
ITEM_RE = re.compile(
    r"^ {0,3}[-*+]\s+(?P<question>.+?):\s*\[(?P<yes>[ xX])\]\s*yes\s+\[(?P<no>[ xX])\]\s*no\b"
)


def visible_lines(markdown):
    """Yield each line as rendered: fenced code, HTML comments and hiding tags removed."""
    fence = None  # the opening fence run, closed only by a run of the same char at least as long
    in_comment = False
    depth = dict.fromkeys(("script", "style", "textarea", "details"), 0)
    for line in markdown.splitlines():
        hidden = in_comment or any(depth.values())
        marker = None if hidden else FENCE_RE.match(line)
        if fence:
            if marker and marker[1][0] == fence[0] and len(marker[1]) >= len(fence):
                fence = None
            continue
        if marker:
            fence = marker[1]
            continue
        shown, pos = [], 0
        masked = CODE_SPAN_RE.sub(lambda span: " " * len(span[0]), line)  # a tag in `code` is text
        for token in HIDING_RE.finditer(masked):
            if not (in_comment or any(depth.values())):
                shown.append(line[pos : token.start()])
            pos = token.end()
            text = token[0]
            if in_comment:
                in_comment = text != "-->"
            elif text == "<!--":
                in_comment = True
            elif token[2]:
                tag = token[2].lower()
                depth[tag] = max(0, depth[tag] + (-1 if token[1] else 1))
        if not (in_comment or any(depth.values())):
            shown.append(line[pos:])
        yield "".join(shown)


def section_items(markdown):
    """Map each question in the Trust boundary section to its (yes, no) answers, plus malformed lines."""
    items, malformed = {}, []
    in_section = False
    for line in visible_lines(markdown):
        heading = HEADING_RE.match(line)
        if heading:
            in_section = heading.group(1).strip().lower() == SECTION_TITLE
            continue
        if not in_section:
            continue
        item = ITEM_RE.match(line)
        if any(c in line for c in INVISIBLE_CHARS):
            malformed.append(f"invisible or non-breaking space in: {line.strip()}")
        elif item:
            question = " ".join(item["question"].split())
            items.setdefault(question, []).append((item["yes"] != " ", item["no"] != " "))
        elif LIST_LINE_RE.match(line) and BOX_RE.search(line):
            malformed.append(f"not in the `<question>: [ ] yes [ ] no` shape: {line.strip()}")
    return items, malformed


def problems(body, template):
    required, _ = section_items(template)
    if not required:
        return [f"no Trust boundary items found in {TEMPLATE.name}"]
    answers, found = section_items(body)
    for question in required:
        given = answers.get(question, [])
        if not given:
            found.append(f"missing: {question}")
        elif len(given) > 1:
            found.append(f"listed more than once: {question}")
        elif sum(given[0]) != 1:
            found.append(f"tick exactly one of yes/no: {question}")
    found += [f"not a template item: {q}" for q in answers if q not in required]
    return found


def is_trust_boundary(path):
    return any(path.startswith(p + "/") or path == p + ".rs" for p in TRUST_BOUNDARY_PATHS)


def main():
    touched = sorted(p for p in sys.stdin.read().splitlines() if is_trust_boundary(p))
    if not touched:
        print("No trust-boundary paths touched.")
        return 0
    found = problems(os.environ.get("PR_BODY") or "", TEMPLATE.read_text())
    if not found:
        print(f"Trust boundary block answered ({len(touched)} trust-boundary paths touched).")
        return 0
    print("This PR touches trust-boundary paths:", *(f"  {p}" for p in touched), sep="\n")
    print("Answer the Trust boundary block from .github/pull_request_template.md in the PR body:")
    for problem in found:
        print(f"::error::Trust boundary {problem}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
