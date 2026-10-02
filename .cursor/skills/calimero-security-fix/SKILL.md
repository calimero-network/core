---
name: calimero-security-fix
description: Fixes a security finding in calimero-network/core - reproduce it through the real entry point, commit a failing test first, add the limit or gate in the shared function every caller goes through, review for bypasses, and open a neutral PR. Use when asked to fix a vulnerability, an audit finding, a DoS or an authorization gap in core.
---

# Fix a security finding in calimero-network/core

The rules a fix must satisfy are in the root [AGENTS.md](../../../AGENTS.md#security-trust-boundaries) ("Security: trust boundaries").
This skill is the workflow; it does not repeat them.

## Steps

1. **Reproduce through the real entry point**, the way an attacker would: a gossip message, a sync stream, an HTTP request or a guest call, not a direct call to the inner function.
   If the finding cannot be reproduced that way, report it as unreachable instead of fixing it.
2. **Commit the test first** and watch it fail on master for the stated reason, not for a setup error.
   Keep that commit separate so a reviewer can check it out and see the failure.
3. **Add the limit constant** for any unbounded input, named and declared with the other constants of its file.
   If an authorization matrix harness exists on master, add a row for the new case.
4. **Fix in the shared function** that every caller goes through.
   Grep every caller of the function you touch; a guard in the one handler the report names leaves the sibling paths open.
5. **Run two review rounds with fresh agents**, each looking for bypasses, regressions for honest users (a member who left and rejoined, a rotated key, an inherited subgroup member), and tests that would pass without the fix.
6. **Rebase onto current master and rerun the gates** right before opening: `./scripts/check-like-ci.py`, which includes the feature-gated test jobs (`--list` shows them).
7. **Use a neutral title and body**: say what the code now does, not how to exploit what it did, and answer the template's Trust boundary block.
8. **Check open PRs** for the same files and for a schema version number someone else has already claimed.
9. **Pair SDK changes** with the matching mero-js PR, named in the body as `sdk-ref: <branch>`.

## Mistakes to avoid

- Merging on stale CI: a green run from before the last rebase proves nothing about the merged code.
- A wire or signed-format change without its mero-js PR: the SDK breaks on the next release.
