#!/usr/bin/env python3
"""No always-on log line in core may carry user data or text from outside the node.

A fleet node ships its journal to a central store that operators can read: on
every mero-tee profile, vector sends each line merod writes to the operator's
`logs-endpoint`. That is deliberate and load-bearing -- a confidential VM has
no shell, so the journal is the only account it can give of itself -- but it
means the default log level decides what an operator sees about the people
using the node.

Calimero's promise is that an operator never sees user data. Content encryption
holds that for the data itself; this gate holds it for the metadata around it.
`merod run` logs at `info,openraft=warn` (`main` in crates/merod/src/main.rs;
nothing sets `RUST_LOG` on a fleet image), so every `info!`, `warn!` and
`error!` reaches the operator.

REFUSED at info!, warn! and error!:

  * CONTENT   - a field named for user- or app-authored content: WASM log
                output, migration logs, call arguments, request payloads.
  * TEXT      - text from outside the node, through Display or Debug: a value
                or inline format argument named `e`, `err`, `error`, `reason`,
                `why`, `body`, `response`, `message`, `payload`, `text`,
                `output`, `line`, `email`, `display_name`, ... (RISKY_NAMES).
                An error's words can quote what it failed on: an upstream's
                answer, a peer's error body, a serde_json snippet of the input,
                an app's trap message. Log what the node named instead: a kind
                or code (`e.kind()`, `FailureKind`, `ApiError::code`), an id,
                a count, a length (`body.len()`), a host, a route template.

REFUSED at info! only:

  * PRINCIPAL - the statement names a person: account, author, member, device,
                executor, admitted identity. At warn!/error! a refusal that
                names the account it refused is how an authorization failure
                is diagnosed, and those paths are rare rather than per-request.

All of these belong at `debug!`, which is off in production and available by
setting `RUST_LOG` on a node you are actively debugging -- or, for an error, in
the API answer to the authenticated caller who caused it.

DELIBERATELY NOT REFUSED: a tenant resource id (app/group/run/blob) on its own.
Those are how a node is diagnosed at all, and demoting them wholesale would
leave a shell-less machine undiagnosable.

The check reads FIELDS ONLY: string literals are blanked first, so a message
that merely contains the word "account" is not a finding -- except for inline
format arguments (`"{e:#}"`), which are values, not words.

A statement whose text is safe for a reason the check cannot see (the error
type's words are the node's own, the line only prints on a development node)
goes in ALLOWLIST, one line each: `<file> | <a quoted piece of the statement> |
<why it is safe>`. An entry that matches nothing is reported, so the list
shrinks as sites are fixed.

Usage: python3 scripts/check-no-user-data-at-info.py
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "crates"
ALLOWLIST = Path(__file__).resolve().parent / "check-no-user-data-at-info.allow"

SKIP = re.compile(r"/tests?/|_test\.rs|/tests\.rs|/testing\.rs|/mock|/examples?/|/benches/")
TEST_MODULE = re.compile(r"^[ \t]*#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\([\w:]+\))?\s+)?mod \w+\s*\{", re.M)
STRING_LITERAL = re.compile(r'"(?:[^"\\]|\\.)*"')
INFO_OPEN = re.compile(r"(^|[^a-z_])(tracing::)?info!\s*\(")
ALWAYS_ON_OPEN = re.compile(r"(^|[^a-z_])(tracing::)?(info|warn|error)!\s*\(")

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

# A value named for text the node did not write: the last segment of the logged
# expression (`e`, `e.message`, `resp.body`, `self.reason`), after any accessor
# that only presents it (PRESENTATION). `e.kind()`, `body.len()`, `body.group`
# end in something else, a fact the node names, and pass.
RISKY_NAMES = frozenset(
    """e err error errors why reason cause detail details message msg body response
    resp payload text output stdout stderr panic line query headers email
    display_name username user_name org_name team_name client_data""".split()
)
PRESENTATION = frozenset(
    """to_string as_str as_ref as_deref display clone to_owned trim unwrap_or_default
    to_string_lossy""".split()
)
PATH = r"[A-Za-z_]\w*(?:\s*\.\s*[A-Za-z_]\w*(?:\(\))?)*"
# `%e`, `?e.message`, `error = %e`, `reason = why`: a sigil or a field's value.
SIGIL_VALUE = re.compile(r"(?:[%?]|=\s*[%?]?)\s*&?\s*(" + PATH + r")")
# `warn!(why, ...)`, `"{}", e`: a bare argument (a field shorthand or a format argument).
BARE_ARG = re.compile(r"(?<=[(,])\s*&?\s*(" + PATH + r")\s*(?=[,)])")
# `"{e:#}"`, `"{why}"`: an inline format argument inside a literal.
INLINE_ARG = re.compile(r"(?<!\{)\{(" + PATH + r")(?::[^}]*)?\}")
# `reason = g.stopped().unwrap_or_default()`: a field NAMED for outside text,
# whatever expression fills it, unless that ends in a fact (`error = %e.kind()`).
NAMED_FIELD = re.compile(r"(?:^|[,(\s])(\w+)\s*=(?!=)\s*[%?]?\s*&?\s*([^,]*)")
FACTS = frozenset(
    """len kind code status is_empty count id as_millis as_secs classify""".split()
)


def last_segment(expr: str) -> str:
    parts = [p.strip().removesuffix("()") for p in expr.split(".")]
    while len(parts) > 1 and parts[-1] in PRESENTATION:
        parts.pop()
    return parts[-1]


def risky(expr: str) -> bool:
    return last_segment(expr) in RISKY_NAMES


# A function's arguments, innermost first: `kind(&e)` logs what `kind` returns,
# not `e`. A macro's (`format!("{}", e)`, `warn!(...)` itself) are kept.
CALL_ARGS = re.compile(r"(?<=[A-Za-z0-9_])\(([^()]+)\)")


def outside_text(body: str) -> list[str]:
    """The risky expressions a statement formats through Display or Debug."""
    found = []
    for lit in STRING_LITERAL.findall(body):
        found += [m for m in INLINE_ARG.findall(lit) if risky(m)]
    fields = STRING_LITERAL.sub('""', body)
    # `()` stands in as one character, so `kind(e.as_ref())` reduces to `kind()`.
    fields = fields.replace("()", "\0")
    while True:
        blanked = CALL_ARGS.sub("\0", fields)
        if blanked == fields:
            break
        fields = blanked
    fields = fields.replace("\0", "()")
    for pattern in (SIGIL_VALUE, BARE_ARG):
        found += [m for m in pattern.findall(fields) if risky(m)]
    for name, value in NAMED_FIELD.findall(fields):
        head = re.match(PATH, value.strip())
        fact = head and last_segment(head.group(0)) in FACTS
        if name in RISKY_NAMES and not fact:
            found.append(f"{name} = {value.strip()}")
    return sorted(set(" ".join(f.split()) for f in found))


def without_test_modules(text: str) -> str:
    """`text` with each inline `#[cfg(test)] mod x { ... }` blanked, lines kept."""
    out, at = [], 0
    for m in TEST_MODULE.finditer(text):
        if m.start() < at:
            continue
        depth, j = 1, m.end()
        while j < len(text) and depth:
            depth += {"{": 1, "}": -1}.get(text[j], 0)
            j += 1
        out.append(text[at : m.start()])
        out.append("\n" * text.count("\n", m.start(), j))
        at = j
    out.append(text[at:])
    return "".join(out)


def statement_ranges(lines: list[str], opener: re.Pattern[str] = INFO_OPEN):
    """Yield (start, end) line indices of every matching call, parens balanced."""
    for i, line in enumerate(lines):
        m = opener.search(line)
        if not m:
            continue
        depth, started, j = 0, False, i
        while j < len(lines) and j < i + 30:
            text = lines[j][m.end() - 1 :] if j == i else lines[j]
            for ch in STRING_LITERAL.sub('""', text):
                if ch == "(":
                    depth += 1
                    started = True
                elif ch == ")":
                    depth -= 1
                if started and depth <= 0:
                    break
            if started and depth <= 0:
                break
            j += 1
        yield i, j


def scan(rel: str, text: str) -> list[tuple[str, int, str, str]]:
    """Every finding in one file: (path, line, why, the statement on one line)."""
    findings = []
    lines = without_test_modules(text).split("\n")
    for start, end in statement_ranges(lines, ALWAYS_ON_OPEN):
        body = "\n".join(lines[start : end + 1])
        fields = STRING_LITERAL.sub('""', body)
        is_info = bool(INFO_OPEN.search(lines[start]))
        why = []
        if CONTENT.search(fields):
            why.append("logs user- or app-authored content")
        if is_info and PRINCIPAL.search(fields):
            why.append("names a principal (account/member/device/author) at info!")
        text_from_outside = outside_text(body)
        if text_from_outside:
            why.append(
                "formats text from outside the node: " + ", ".join(text_from_outside)
            )
        if why:
            findings.append((rel, start + 1, "; ".join(why), " ".join(body.split())))
    return findings


def read_allowlist(text: str) -> list[tuple[str, str, str]]:
    """(file, quoted piece, reason) per entry; refuses an entry with no reason."""
    entries = []
    for n, raw in enumerate(text.split("\n"), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = [p.strip() for p in line.split(" | ", 2)]
        if len(parts) != 3 or not all(parts):
            raise SystemExit(f"{ALLOWLIST.name}:{n}: want `<file> | <piece> | <reason>`")
        entries.append(tuple(parts))
    return entries


def allowed(finding, entries) -> int | None:
    rel, _line, _why, snippet = finding
    for k, (file, piece, _reason) in enumerate(entries):
        if file == rel and piece in snippet:
            return k
    return None


def run(root: Path, allowlist_text: str) -> tuple[list, list, list]:
    """(findings not allowed, allowlist entries that matched nothing, all entries)."""
    entries = read_allowlist(allowlist_text)
    used: set[int] = set()
    left = []
    for path in sorted(root.rglob("*.rs")):
        rel = str(path.relative_to(root.parent))
        if SKIP.search(rel):
            continue
        for f in scan(rel, path.read_text(errors="replace")):
            k = allowed(f, entries)
            if k is None:
                left.append(f)
            else:
                used.add(k)
    stale = [e for k, e in enumerate(entries) if k not in used]
    return left, stale, entries


def main() -> int:
    text = ALLOWLIST.read_text() if ALLOWLIST.exists() else ""
    findings, stale, entries = run(ROOT, text)
    for file, piece, _ in stale:
        print(f"[no-user-data-in-logs] note: allowlist entry matches nothing, remove it: {file} | {piece}")
    if not findings:
        print(
            f"[no-user-data-in-logs] OK: no info!/warn!/error! carries user data "
            f"({len(entries) - len(stale)} allowlisted)"
        )
        return 0

    print("[no-user-data-in-logs] FAIL: these log lines reach the operator\n")
    for rel, line, why, snippet in findings:
        print(f"  {rel}:{line}")
        print(f"    {why}")
        print(f"    {snippet[:160]}\n")
    print(
        f"{len(findings)} finding(s). merod logs at INFO in production and ships its\n"
        "journal to a store operators can read. Log a kind, code, id, count or length\n"
        "instead, move the detail to debug! or to the API answer of the caller who\n"
        "caused it, or allowlist it in scripts/check-no-user-data-at-info.allow with\n"
        "the reason its text is the node's own."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
