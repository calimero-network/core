#!/usr/bin/env python3
"""Lint merobox scenarios for convergence assertions that race the thing they assert.

Three scenarios in one afternoon failed this way, all with the same shape: a step
acts or asserts on state that another node produced, before anything established
that the state had actually arrived. The failures are worse than flaky — they are
*misleading*. A missing barrier surfaces as `output: None`, or as the literal string
"undefined" reaching a base58 parser, or as "no mesh peer held the key". None of
those name a timing problem, so each one costs a real investigation into whichever
feature the PR happened to touch.

Two rules, each derived from an observed failure rather than invented:

R1 — INCOMPLETE BARRIER. A `wait_for_sync` that omits a node the scenario then uses
     before the next barrier. This is the `account-device-revoke-lockout` bug: the
     settle step listed nodes 1 and 2, and the very next step had the freshly paired
     node 3 write with a key it could only have received by sync.

R2 — MISSING BARRIER. A step that claims cross-node convergence with no barrier
     between it and the last mutation. This is the `32-authored-migrate-ux` bug:
     three reads asserted "converged" with nothing between them and the writes, and
     the one reading the *other* node's last write returned null.

R2 needs care to stay useful. A step named "Insert Hello from Node 1" *running on*
node 1 is a write naming its own actor, not a cross-node read — flagging those was
the first thing that made a draft of this check unusable, so a claim is only counted
when the step runs somewhere other than the node its name points at.

Enforced as a RATCHET, mirroring the endpoint-coverage gate: the findings that
already existed when this check landed live in `scripts/e2e-convergence-baseline.txt`
and only a NEW one fails the build. A hard gate would have failed on its first run
against nine pre-existing findings, and a check that fails on day one gets disabled
rather than fixed.

Exit 1 on any un-baselined finding. `# lint-convergence: ok <reason>` on the step suppresses it,
because a scenario that deliberately asserts non-convergence (a partition test
proving a write did NOT arrive) is a legitimate exception that must state itself.
"""

from __future__ import annotations

import glob
import pathlib
import re
import sys
from typing import Any

import yaml

BARRIER_TYPES = {"wait_for_sync"}
# A bare `wait: seconds` is deliberately NOT a barrier. Every failure this catches
# had one available and it did not help: a fixed sleep asserts nothing about
# arrival, so it converts a deterministic failure into a load-dependent one.
MUTATING_TYPES = {"call", "execute", "create_context", "join_context", "install_application"}
# `json_assert` is deliberately absent: it carries no `node`, so there is no way to
# tell which replica it reads. It compares values captured by earlier `call` steps,
# and those are where the barrier question actually belongs — blaming the assert
# reported one finding per assertion and pointed at the wrong step.
READING_TYPES = {"call", "execute"}

# Whether a `call` mutates decides whether it makes its node dirty, and getting this
# wrong is what made a first draft unusable: read-only calls marked their own node
# dirty, so the *next* read was flagged as racing a write that never happened.
# Unknown methods count as READS — this check is worth having only if its findings
# are credible, so it stays silent when it cannot tell.
READ_METHOD = re.compile(
    r"^(get|read|list|count|has|is|view|len|entries|iter|owner|schema|info|"
    r"remaining|status|value|text|state)_?|_(version|info|count|owner|status)$",
    re.I,
)

# A step whose name claims it is reading what another node produced.
CLAIMS_CONVERGENCE = re.compile(
    r"converg|\bsees\b|from node|other node|cross-?peer|replicat|both nodes", re.I
)
# The node a name points at ("... from node-2", "on node 2", "Node-3 writes").
NAMES_NODE = re.compile(r"node[-_ ]?(\d+)", re.I)
SUPPRESS = re.compile(r"lint-convergence:\s*ok\b", re.I)


def step_nodes(step: dict[str, Any]) -> set[str]:
    """Every node a step touches, whether named singly or as a list."""
    out: set[str] = set()
    node = step.get("node")
    if isinstance(node, str):
        out.add(node)
    nodes = step.get("nodes")
    if isinstance(nodes, list):
        out.update(n for n in nodes if isinstance(n, str))
    return out


def node_ordinal(name: str) -> str | None:
    """The trailing ordinal of a node name, which is what a step name refers to."""
    m = NAMES_NODE.search(name)
    return m.group(1) if m else None


def mutates(step: dict[str, Any]) -> bool:
    """Does this step change state, and so make its node dirty?

    Non-`call` step types in `MUTATING_TYPES` always do. A `call` is judged by its
    method name, erring toward "read" so an unrecognised method produces silence
    rather than a false finding.
    """
    if step.get("type") not in {"call", "execute"}:
        return True
    method = str(step.get("method", ""))
    return not READ_METHOD.search(method)


def suppressed(step: dict[str, Any]) -> bool:
    return any(SUPPRESS.search(str(v)) for v in step.values() if isinstance(v, str))


def lint(path: str) -> list[str]:
    try:
        doc = yaml.safe_load(open(path))
    except yaml.YAMLError as err:
        return [f"{path}: unparseable YAML ({err})"]
    if not isinstance(doc, dict):
        return []
    steps = doc.get("steps")
    if not isinstance(steps, list):
        return []

    findings: list[str] = []
    # Nodes that have mutated since the last barrier, and which nodes that barrier
    # covered. A barrier resets the first and records the second.
    dirty: set[str] = set()
    last_barrier_covered: set[str] | None = None
    last_barrier_name = ""

    for step in steps:
        if not isinstance(step, dict):
            continue
        stype = step.get("type")
        name = str(step.get("name", "<unnamed>"))
        touched = step_nodes(step)

        if stype in BARRIER_TYPES:
            # R1: does this barrier cover everything that has changed since the last?
            missed = dirty - touched
            if missed and not suppressed(step):
                findings.append(
                    f"{path}: R1 barrier '{name}' omits {sorted(missed)}, which mutated "
                    f"since the last barrier — a later step on those nodes races the sync "
                    f"this step was supposed to establish"
                )
            dirty = set()
            last_barrier_covered = touched
            last_barrier_name = name
            continue

        if stype in READING_TYPES and CLAIMS_CONVERGENCE.search(name) and not suppressed(step):
            # Only a genuine cross-node claim: the step must run somewhere other than
            # the node its own name points at, or it is a write naming its actor.
            target = node_ordinal(name)
            here = {node_ordinal(n) for n in touched}
            cross = target is None or target not in here
            if cross:
                if last_barrier_covered is None:
                    findings.append(
                        f"{path}: R2 '{name}' asserts cross-node convergence with no "
                        f"wait_for_sync anywhere before it"
                    )
                elif dirty - touched:
                    findings.append(
                        f"{path}: R2 '{name}' asserts cross-node convergence, but "
                        f"{sorted(dirty - touched)} mutated after the last barrier "
                        f"('{last_barrier_name}') — the read races those writes"
                    )
                elif not touched <= last_barrier_covered:
                    findings.append(
                        f"{path}: R2 '{name}' reads on {sorted(touched - last_barrier_covered)}, "
                        f"which the last barrier ('{last_barrier_name}') did not cover"
                    )

        if stype in MUTATING_TYPES and mutates(step):
            dirty |= touched

    return findings


BASELINE = pathlib.Path(__file__).with_name("e2e-convergence-baseline.txt")


def load_baseline() -> set[str]:
    """Accepted-uncovered findings, one per line; `#` comments and blanks ignored."""
    if not BASELINE.exists():
        return set()
    return {
        line.strip()
        for line in BASELINE.read_text().splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }


def main() -> int:
    patterns = sys.argv[1:] or [
        "apps/*/workflows/*.yml",
        "workflows/**/*.yml",
    ]
    paths = sorted({p for pat in patterns for p in glob.glob(pat, recursive=True)})
    if not paths:
        print("no scenarios matched", file=sys.stderr)
        return 1

    all_findings = [f for p in paths for f in lint(p)]
    baseline = load_baseline()
    findings = [f for f in all_findings if f not in baseline]
    accepted = len(all_findings) - len(findings)

    print(f"scanned {len(paths)} scenario(s)")
    if accepted:
        print(f"{accepted} baselined (accepted) finding(s) — burndown backlog")
    # A baseline entry that no longer matches is stale: the scenario was fixed (or
    # renamed) and the entry now silences nothing. Reported, never fatal, so a fix
    # is never punished with a red build.
    for stale in sorted(baseline - set(all_findings)):
        print(f"::notice::stale baseline entry, safe to delete: {stale}")
    if not findings:
        print("no new convergence races found")
        return 0

    print(f"\n{len(findings)} NEW finding(s):\n")
    for f in findings:
        print(f"  {f}")
    print(
        "\nEach is a step that reads or acts on another node's state without "
        "establishing that it arrived.\nAdd a `wait_for_sync` covering the nodes "
        "involved, or annotate the step with\n`# lint-convergence: ok <reason>` if it "
        "deliberately asserts non-convergence."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
