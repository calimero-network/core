#!/usr/bin/env python3
"""Lint merobox log assertions for patterns that can no longer match anything.

`assert_log_present` and `assert_log_absent` compare a scenario's expectation
against a *string a program prints*. That coupling has a failure mode with no
symptom: rename the log line and the assertion does not break loudly — it stops
checking. An `absent` assertion passes vacuously forever; a `present` one fails
somewhere far from the rename, in a scenario nobody associates with logging.

Neither is caught by running the suite. A green `absent` proves nothing, and the
`present` failure arrives attached to whichever feature the PR happened to
touch. This is the same shape as the hex migration's "checks that silently
stopped checking": the assertion survived, the thing it asserted did not.

So the rule is narrow and mechanical: **every pattern must appear as a literal
somewhere it could plausibly be emitted from.** Two scopes, because logs come
from two places:

  crates/   the node's own tracing output
  apps/     a guest app's `app::log!`, which reaches the node log through the
            runtime and is what the blob scenarios assert on

A pattern matching neither cannot fire. That is either a rename nobody
propagated or a scenario asserting something that was never emitted, and both
are worth a build failure rather than a green tick.

ALLOWLIST, not a baseline. A handful of patterns legitimately match no source
literal because the string belongs to the Rust runtime rather than to this
repo — panic messages most of all. Each needs a reason, because "it does not
match" is exactly what a stale pattern looks like too, and the two are
indistinguishable without one.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent

# Where a log line can legitimately come from.
SEARCH_ROOTS = ("crates", "apps")

# Patterns that match no repo literal for a stated reason. A pattern is only
# allowed here when the string is emitted by something outside this repo.
ALLOWED: dict[str, str] = {
    "panicked at": "Rust's own panic handler formats this; no crate prints it",
    "thread 'main' panicked": "same — the runtime's wording, not ours",
}

ASSERTION_TYPES = ("assert_log_present", "assert_log_absent")


def scenarios() -> list[Path]:
    found = sorted(ROOT.glob("apps/*/workflows/*.yml"))
    found += sorted(ROOT.glob("workflows/**/*.yml"))
    return found


def patterns_in(path: Path) -> list[tuple[str, str, str]]:
    """Every (pattern, assertion type, step name) the scenario asserts on."""
    try:
        doc = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError:
        # Not this check's job to report — the scenario linters already do.
        return []
    if not isinstance(doc, dict):
        return []

    out = []
    for step in doc.get("steps") or []:
        if not isinstance(step, dict) or step.get("type") not in ASSERTION_TYPES:
            continue
        for pattern in step.get("patterns") or []:
            out.append((str(pattern), step["type"], str(step.get("name", ""))))
    return out


def emitted_somewhere(pattern: str) -> bool:
    """Does any source file contain something this pattern could match?

    Two passes, because merobox accepts BOTH shapes and scenarios use both.
    Most patterns are plain substrings of a formatted log line; some are
    regexes, which is how `Starting HashComparison sync \\(initiator\\)`
    escapes its parentheses. A fixed-string search alone reports every escaped
    pattern as dead — a false positive that would make this gate worse than no
    gate, since the first response to a lint that cries wolf is to delete it.

    Fixed-string first because it is the common case and cannot over-match.
    Extended-regex second, and deliberately permissive: a false NEGATIVE here
    costs a stale assertion surviving one more release, while a false POSITIVE
    costs a red build on work that has nothing to do with logging.
    """
    for root in SEARCH_ROOTS:
        for flags in ("-rlF", "-rlE"):
            result = subprocess.run(
                ["grep", flags, "--", pattern, root],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            if result.returncode == 0 and result.stdout.strip():
                return True
    return False


def main() -> int:
    findings: list[str] = []
    checked = 0
    unused_allowances = set(ALLOWED)

    for scenario in scenarios():
        for pattern, kind, step in patterns_in(scenario):
            checked += 1
            if pattern in ALLOWED:
                unused_allowances.discard(pattern)
                continue
            if emitted_somewhere(pattern):
                continue
            findings.append(
                f"{scenario.relative_to(ROOT)}\n"
                f"    {kind}: {pattern!r}\n"
                f"    step: {step}\n"
                f"    No literal containing this appears under "
                f"{'/, '.join(SEARCH_ROOTS)}/. It cannot match, so this "
                f"assertion no longer checks anything. Either the log line was "
                f"renamed and this was not, or it was never emitted. If the "
                f"string comes from outside this repo, add it to ALLOWED with "
                f"the reason."
            )

    print(f"checked {checked} log assertion(s)")

    # A stale allowance is the same hazard one level up: it excuses a pattern
    # nobody asserts any more, and the next person reads it as a live exception.
    for stale in sorted(unused_allowances):
        print(f"::warning::allowlist entry is unused and can be removed: {stale!r}")

    if findings:
        print(f"\n{len(findings)} assertion(s) can never match:\n")
        for finding in findings:
            print(f"  {finding}\n")
        return 1

    print("every pattern still matches a literal that could emit it")
    return 0


if __name__ == "__main__":
    sys.exit(main())
