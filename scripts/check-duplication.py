#!/usr/bin/env python3
"""Fail if a crate gains a near-duplicate function it did not have before.

Duplication in this workspace does not arrive by decision. It arrives because a
new collection type, a new op variant or a new plane is added by copying the
nearest existing one, by an author with no way to know how many copies already
exist. Two measured examples: `js_collections.rs` reached 5,817 lines with a
third of its function bodies redundant across 26 clone families, and a *fourth*
copy of the projection ancestry walk landed within days of a PR unifying the
first three.

So this is a ratchet, not a threshold. The workspace has hundreds of
near-duplicate pairs and demanding zero would be absurd; demanding that a crate
not grow *new* ones is enforceable today, and every cleanup that lands can be
locked in by refreshing the baseline.

**It reports pairs, never verdicts.** Similarity is syntactic, so the detector
cannot tell a genuine copy from two operations that merely look alike — it groups
`crdt_map_get` with `crdt_authored_map_owner_of`, which are different questions
about different planes. That is why a new pair fails the build with the pair
named rather than being auto-rejected on a score: a human decides whether it is a
copy to fold or two honest neighbours to baseline, and the baseline demands a
reason so the second answer has to be argued rather than accumulated.

Pairs are keyed by (crate, function name, function name) and never by line
number, so unrelated edits above a function do not churn the baseline. Renaming a
function does show up as a new pair; that is a one-line baseline edit with a
visible cause.

Usage:
    check-duplication.py                 # gate: fail on any new pair
    check-duplication.py --update        # rewrite the baseline from the tree
    check-duplication.py --list CRATE    # show the pairs found in one crate

Exit 0 when no crate gained a pair, 1 otherwise.
"""

from __future__ import annotations

import argparse
import difflib
import itertools
import json
import os
import re
import subprocess
import sys

BASELINE = "scripts/duplication-baseline.json"

# Locked here rather than passed in: the baseline is only meaningful against
# fixed parameters, and a knob would let a failing run be argued away by
# loosening the gate instead of the code.
MIN_BODY_LINES = 10
MIN_BODY_CHARS = 300
SIMILARITY = 0.82

# Test trees are excluded, and so are inline `#[cfg(test)]` modules — see
# `strip_test_modules`. Skipping the separate files while scanning the inline ones
# was an inconsistency rather than a choice: they are the same category of code.
SKIP_PATH_PARTS = ("/tests/", "/target/", "/node_modules/")
SKIP_FILE_NAMES = ("tests.rs",)


def rust_files() -> list[str]:
    """Every non-test Rust source file tracked by git, so an untracked scratch
    file cannot fail somebody else's build."""
    tracked = subprocess.run(
        ["git", "ls-files", "*.rs"], capture_output=True, text=True, check=True
    ).stdout.split()
    out = []
    for path in tracked:
        if any(part in f"/{path}" for part in SKIP_PATH_PARTS):
            continue
        if os.path.basename(path) in SKIP_FILE_NAMES:
            continue
        out.append(path)
    return sorted(out)


def crate_of(path: str) -> str:
    """The crate a file belongs to: its directory up to `src/`.

    `crates/context/primitives/src/x.rs` is `crates/context/primitives`, not
    `crates/context` — a nested primitives crate is its own unit, and merging the
    two would let a pair added in one be offset by a pair removed in the other.
    """
    marker = "/src/"
    return path[: path.index(marker)] if marker in path else os.path.dirname(path)


def strip_test_modules(src: str) -> str:
    """Remove every `#[cfg(test)] mod … { … }` block.

    Test code is excluded on purpose, and it is nearly half of what the extractor
    would otherwise see (1,894 of 4,180 bodies when this was measured). Two
    reasons, and the second is the load-bearing one:

    * A test legitimately repeats structure — arrange, act, assert, with one input
      varied. Flagging that buries the production copies the gate exists to catch.
    * This workspace deliberately duplicates in tests. The oracle pattern keeps a
      pre-refactor implementation verbatim precisely so it shares no code with what
      it checks (`inheritance_climb.rs`, `ancestry_oracle`). A gate that flagged
      those would be arguing against the technique that makes the refactors
      verifiable.

    Brace-matched rather than cut at the first occurrence, so a file with a test
    module in the middle keeps the production code after it.
    """
    out, i = [], 0
    for m in re.finditer(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{", src):
        if m.start() < i:
            continue
        depth, j = 0, m.end() - 1
        while j < len(src):
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        out.append(src[i : m.start()])
        i = j + 1
    out.append(src[i:])
    return "".join(out)


def signature_end(src: str, start: int) -> int | None:
    """Index of the `{` or `;` that ends the signature beginning at `start`.

    Tracked at bracket depth zero, so a `;` inside `[u8; 32]` or a `{` inside a
    default const-generic expression does not end the signature early. Returns
    `None` if neither is found within a plausible signature span — a `where`
    clause across several lines is normal, a thousand characters is not.
    """
    depth = 0
    limit = min(len(src), start + 1000)
    i = start
    while i < limit:
        c = src[i]
        if c in "<([":
            depth += 1
        elif c in ">)]":
            # `->` is not a closing bracket; skip the arrow's `>`.
            if not (c == ">" and i > 0 and src[i - 1] == "-"):
                depth -= 1
        elif depth <= 0 and c in "{;":
            return i
        i += 1
    return None


def functions(path: str):
    """Yield (name, normalised_body) for each function with a real body.

    Bodies are found by brace matching from the signature. Three things here were
    each learned from getting them wrong:

    * `macro_rules!` templates are stripped — their contents are not code at the
      site they appear, and matching inside them produces thousands of phantom
      pairs.
    * A signature terminated by `;` is a trait or `extern` declaration with no
      body; scanning past it finds an unrelated brace further down the file.
    * That `;` must be found at **bracket depth zero**. Searching for the first
      `;` anywhere fails on this codebase specifically, because `[u8; 32]` is how
      every hash and key is spelled — so `fn f(..) -> Vec<[u8; 32]> {` looks
      terminated by a semicolon and the function is skipped. That silently hid
      more than half the functions in some files.
    """
    try:
        src = open(path, encoding="utf-8").read()
    except (OSError, UnicodeDecodeError):
        return
    src = strip_test_modules(src)
    src = re.sub(r"macro_rules!\s*\w+\s*\{", "MACRODEF {", src)
    for m in re.finditer(
        r"\n\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+(\w+)",
        src,
    ):
        end = signature_end(src, m.end())
        if end is None or src[end] == ";":
            continue
        brace = end
        depth, j = 0, brace
        while j < len(src):
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        body = src[brace : j + 1]
        if body.count("\n") < MIN_BODY_LINES:
            continue
        norm = re.sub(r"\s+", " ", re.sub(r"//[^\n]*", "", body)).strip()
        if len(norm) < MIN_BODY_CHARS:
            continue
        yield m.group(1), norm


def pairs_by_crate() -> dict[str, set[tuple[str, str]]]:
    """Near-duplicate function-name pairs, per crate.

    Compared within a crate only. A cross-crate pair is usually a shared shape
    that cannot be folded without a new dependency edge, which is a design call
    rather than something a gate should demand.
    """
    per_crate: dict[str, list[tuple[str, str, str]]] = {}
    for path in rust_files():
        stem = os.path.splitext(os.path.basename(path))[0]
        per_crate.setdefault(crate_of(path), []).extend(
            (name, body, stem) for name, body in functions(path)
        )

    found: dict[str, set[tuple[str, str]]] = {}
    for crate, fns in per_crate.items():
        hits = set()
        for (n1, b1, f1), (n2, b2, f2) in itertools.combinations(fns, 2):
            if n1 == n2 and f1 == f2:
                # Same name in ONE file is an overload or a variant of a single
                # type — folding those needs a shared trait, a design decision
                # rather than something a gate should demand.
                #
                # Across files it is the opposite, and it is the biggest
                # duplication pattern in this workspace: `sorted_set::insert`
                # beside `unordered_set::insert`, `entry` in both map files,
                # `watch_app_and_await` in two meroctl subcommands. Skipping
                # every same-name pair — which this did at first — blinded the
                # gate to exactly the parallel-implementation growth it exists to
                # stop, and to a finding the audit had already recorded.
                continue
            if abs(len(b1) - len(b2)) > 0.2 * max(len(b1), len(b2)):
                continue
            if difflib.SequenceMatcher(None, b1, b2).ratio() >= SIMILARITY:
                # Names alone identify a pair, except when they are equal — then
                # the file stems are what tell two parallel implementations apart,
                # and without them several distinct pairs would collapse to one
                # baseline entry.
                key = (
                    (f"{n1}@{f1}", f"{n2}@{f2}") if n1 == n2 else (n1, n2)
                )
                hits.add(tuple(sorted(key)))
        if hits:
            found[crate] = hits
    return found


def load_baseline(path: str) -> dict:
    if not os.path.exists(path):
        return {"crates": {}}
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline", default=BASELINE)
    ap.add_argument("--update", action="store_true", help="rewrite the baseline")
    ap.add_argument("--list", metavar="CRATE", help="print one crate's pairs")
    args = ap.parse_args()

    found = pairs_by_crate()

    if args.list:
        for a, b in sorted(found.get(args.list, ())):
            print(f"  {a}  <->  {b}")
        return 0

    if args.update:
        payload = {
            "_comment": (
                "Near-duplicate function pairs accepted per crate, keyed by name so "
                "line movement does not churn this file. Regenerate with "
                "scripts/check-duplication.py --update, and say in the PR why any "
                "ADDED pair is two honest neighbours rather than a copy to fold."
            ),
            "_parameters": {
                "min_body_lines": MIN_BODY_LINES,
                "min_body_chars": MIN_BODY_CHARS,
                "similarity": SIMILARITY,
            },
            "crates": {
                crate: [list(p) for p in sorted(pairs)]
                for crate, pairs in sorted(found.items())
            },
        }
        with open(args.baseline, "w", encoding="utf-8") as fh:
            json.dump(payload, fh, indent=2)
            fh.write("\n")
        total = sum(len(v) for v in found.values())
        print(f"wrote {args.baseline}: {total} pair(s) across {len(found)} crate(s)")
        return 0

    baseline = load_baseline(args.baseline)
    accepted = {
        crate: {tuple(p) for p in pairs}
        for crate, pairs in baseline.get("crates", {}).items()
    }

    added: list[tuple[str, tuple[str, str]]] = []
    removed: list[tuple[str, tuple[str, str]]] = []
    for crate in sorted(set(found) | set(accepted)):
        now, was = found.get(crate, set()), accepted.get(crate, set())
        added.extend((crate, p) for p in sorted(now - was))
        removed.extend((crate, p) for p in sorted(was - now))

    if removed:
        print(f"::notice::{len(removed)} baselined pair(s) are gone — nice:")
        for crate, (a, b) in removed[:20]:
            print(f"  {crate}: {a} <-> {b}")
        print(
            "  Lock the win in with `scripts/check-duplication.py --update` "
            "so it cannot come back unnoticed."
        )

    if added:
        print(
            f"::error::{len(added)} new near-duplicate function pair(s). Either fold "
            "them, or baseline them with a reason in the PR."
        )
        for crate, (a, b) in added:
            print(f"  {crate}: {a} <-> {b}")
        print(
            "\nThis is a syntactic match, so it can flag two operations that merely "
            "look alike (a `get` and an `owner_of` over the same shape). If that is "
            "what these are, say so and run --update; the point is that somebody "
            "looked."
        )
        return 1

    total = sum(len(v) for v in found.values())
    print(f"no new near-duplicate pairs ({total} baselined across {len(found)} crates)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
