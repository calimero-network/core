#!/usr/bin/env python3
"""No `info!` in core may name a user.

A fleet node ships its journal to a central store that operators can read. That
is deliberate and load-bearing -- a confidential VM has no shell, so the journal
is the only account it can give of itself -- but it means the default log level
decides what an operator sees about the people using the node.

Calimero's promise is that an operator never sees user data. Content encryption
holds that for the data itself; this gate holds it for the metadata around it.
`merod` runs at INFO in production (`DEFAULT_DIRECTIVES` in
crates/merod/src/main.rs, and nothing sets `RUST_LOG` on a fleet image), so an
`info!` naming an account is an account id in the operator's log store.

REFUSED at info!:

  * CONTENT   - the logged value is user- or app-authored: WASM log output,
                migration logs, call arguments, request payloads. The worst of
                these was `WASM_LOG`, which shipped an application's own log
                lines -- message bodies, names, amounts, whatever the app
                chose to print -- at INFO, tagged with the context id.
  * PRINCIPAL - the statement names a person: account, author, member, device,
                executor, admitted identity. `perform_intent` logged
                `author = %warrant.author_account` with the context and method
                on the delegated-execution path, which every fleet relay has
                open by design: a complete record of which user did what, when.

Both belong at `debug!`, which is off in production and available by setting
`RUST_LOG` on a node you are actively debugging.

DELIBERATELY NOT REFUSED: a tenant resource id (context/group/blob) on its own,
in sync, network and lifecycle code. Those are how a node is diagnosed at all,
there are ~300 of them, and demoting them wholesale would leave a shell-less
machine undiagnosable -- trading a real operational capability for a marginal
privacy gain. If that trade is ever wanted it should be a deliberate, separate
decision, not a side effect of this gate.

The check reads FIELDS ONLY: string literals are blanked first, so a message
that merely contains the word "account" -- `"account-follow handler started"`,
or merod's deliberate "Created the admin account from ..." notice -- is not a
finding. Matching those was the first version's bug.

Usage: python3 scripts/check-no-user-data-at-info.py
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "crates"

SKIP = re.compile(r"/tests?/|_test\.rs|/mock|/examples?/|/benches/")
STRING_LITERAL = re.compile(r'"(?:[^"\\]|\\.)*"')
INFO_OPEN = re.compile(r"(^|[^a-z_])(tracing::)?info!\s*\(")
# Content must not appear at ANY always-on level. An `error!` carrying a
# caller's request body is strictly worse than an `info!` doing it: ERROR is on
# everywhere, including on a node nobody is debugging.
ALWAYS_ON_OPEN = re.compile(r"(^|[^a-z_])(tracing::)?(warn|error)!\s*\(")

# The FIELD NAME must be exactly a content field, immediately before its `=`.
# Anything derived is fine and is what these sites should log instead:
# `payload_len = request.payload.len()` is a length, `groups = ?payload_only_groups`
# is a list of ids, `len = payload.len()` is a size. Only the body itself is
# refused. Matching the substring instead flagged all three.
CONTENT = re.compile(
    r"(?:^|[,(\s])(log_content|migration_log|args_json|args|payload"
    r"|plaintext|plaintext_prefix|entry_value)\s*="
)
PRINCIPAL = re.compile(
    r"(?:[%?]|\b)(account|author|member|device|executor|admin_identity"
    r"|invitee|inviter|signer_account|identity)\s*(?:=|,|\))"
)


def statement_ranges(lines: list[str], opener: re.Pattern[str] = INFO_OPEN):
    """Yield (start, end) line indices of every matching call, parens balanced."""
    for i, line in enumerate(lines):
        if not opener.search(line):
            continue
        depth, started, j = 0, False, i
        while j < len(lines) and j < i + 30:
            for ch in lines[j]:
                if ch == "(":
                    depth += 1
                    started = True
                elif ch == ")":
                    depth -= 1
            if started and depth <= 0:
                break
            j += 1
        yield i, j


def main() -> int:
    findings = []
    for path in sorted(ROOT.rglob("*.rs")):
        rel = path.relative_to(ROOT.parent)
        if SKIP.search(str(rel)):
            continue
        lines = path.read_text(errors="replace").split("\n")
        for start, end in statement_ranges(lines):
            body = "\n".join(lines[start : end + 1])
            fields = STRING_LITERAL.sub('""', body)
            why = []
            if CONTENT.search(fields):
                why.append("logs user- or app-authored content")
            if PRINCIPAL.search(fields):
                why.append("names a principal (account/member/device/author)")
            if why:
                findings.append((rel, start + 1, "; ".join(why),
                                 " ".join(body.split())[:120]))

        # Content is refused at warn!/error! too. Principals are NOT: a refusal
        # that names the account it refused is how an authorization failure is
        # diagnosed, and those paths are rare rather than per-request.
        for start, end in statement_ranges(lines, ALWAYS_ON_OPEN):
            body = "\n".join(lines[start : end + 1])
            fields = STRING_LITERAL.sub('""', body)
            if CONTENT.search(fields):
                findings.append((rel, start + 1,
                                 "logs user- or app-authored content at an always-on level",
                                 " ".join(body.split())[:120]))

    if not findings:
        print("[no-user-data-at-info] OK: no info! statement names a user")
        return 0

    print("[no-user-data-at-info] FAIL: these must be debug!, not info!\n")
    for rel, line, why, snippet in findings:
        print(f"  {rel}:{line}")
        print(f"    {why}")
        print(f"    {snippet}\n")
    print(
        f"{len(findings)} finding(s). merod runs at INFO in production and ships its\n"
        "journal to a store operators can read, so these would put user data there.\n"
        "Use debug! -- it is available via RUST_LOG on a node being debugged."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
