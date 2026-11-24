#!/usr/bin/env python3
"""Tests for check-no-user-data-at-info.py: no always-on log line carries text from outside the node.

    python3 scripts/tests/check-no-user-data-at-info-test.py
"""

import importlib.util
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-no-user-data-at-info.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("check_no_user_data", SCRIPT)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

FLAGGED = [
    # (name, a statement the check must refuse)
    ("an error through Display at warn!", 'warn!(app = %id, error = %e, "x failed");'),
    ("an error through Debug at error!", 'error!(error = ?err, "x failed");'),
    ("an error's chain in a format! field", 'warn!(node = id, error = %format!("{e:#}"), "x");'),
    ("an error as an inline format argument", 'warn!("rollback checkpoint failed: {e:#}");'),
    ("an error as a positional format argument", 'warn!("Failed to read tar entry {}: {}", i, e);'),
    ("an ApiError's message", 'warn!(app = %id, error = %e.message, "x");'),
    ("an upstream's body", 'warn!(status, body = %resp.body, "upstream answered");'),
    ("a refusal's words as a shorthand field", 'warn!(%reason, "outbound request refused");'),
    ("a bare shorthand field", 'warn!(why, "x");'),
    ("a field named for a reason, whatever fills it",
     'error!(group = g.id, reason = g.stopped().unwrap_or_default(), "stopped");'),
    ("an email at info!", 'info!(email = %user.email, "signed up");'),
    ("an error message spread over lines", 'tracing::warn!(\n    group,\n    error = %e,\n    "x"\n);'),
    ("an error's text at info!", 'info!(output = %stdout.to_string(), "ran");'),
    ("a principal at info!", 'info!(account = %a, "joined");'),
    ("content at warn!", 'warn!(payload = %p, "x");'),
]

PASSES = [
    # (name, a statement the check must let through)
    ("an error's kind", 'warn!(error = %e.kind(), "x");'),
    ("an ApiError's code and status", 'warn!(app = %id, status = e.status, code = %e.code, "x");'),
    ("a body's length", 'warn!(response_bytes = body.len(), "x");'),
    ("an id taken from a body", 'warn!(group = body.group, seen, "x");'),
    ("a kind computed from the reason", 'warn!(host, refusal = refusal_kind(&reason), "refused");'),
    ("a kind computed from an error", 'warn!(%addr, error_kind = %crate::log_safe::kind(e.as_ref()), "x");'),
    ("the words in a message are not values", 'warn!(group, "the error reason and message body");'),
    ("debug! is off in production", 'debug!(error = %e, "x");'),
    ("a principal at warn! names the refused account", 'warn!(account = %a, "refused");'),
    ("an escaped brace is not an argument", 'warn!(count, "{{e}} is literal");'),
    ("a host and a route template", 'info!(host = %h, prefix = %p, status, "outbound request");'),
]


def findings(text: str, rel: str = "crates/x/src/lib.rs"):
    return check.scan(rel, text)


failures = []
for name, stmt in FLAGGED:
    if not findings(stmt + "\n"):
        failures.append(f"not flagged: {name}: {stmt}")
for name, stmt in PASSES:
    got = findings(stmt + "\n")
    if got:
        failures.append(f"flagged: {name}: {got[0][2]}")

# A unit-test module is not a node's code.
in_tests = 'fn f() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() { warn!(error = %e, "x"); }\n}\n'
if findings(in_tests):
    failures.append("a #[cfg(test)] module was scanned")
# A declared test module (`mod tests;`) hides nothing after it, and code after an
# inline one is scanned again, at its own line.
after = '#[cfg(test)]\nmod queue_tests;\n\nfn g() { warn!(error = %e, "x"); }\n'
if [f[1] for f in findings(after)] != [4]:
    failures.append(f"code after `mod tests;` was not scanned: {findings(after)}")
after_inline = in_tests + 'fn g() {\n    warn!(error = %e, "x");\n}\n'
if [f[1] for f in findings(after_inline)] != [8]:
    failures.append(f"code after an inline test module: {findings(after_inline)}")

# The planted line, and what the allowlist does with it.
PLANTED = 'fn send() {\n    warn!(context = %ctx, %reason, "outbound request refused");\n}\n'
with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp) / "crates"
    (root / "outbound" / "src").mkdir(parents=True)
    (root / "outbound" / "src" / "lib.rs").write_text(PLANTED)
    (root / "outbound" / "src" / "tests.rs").write_text('fn t() { warn!(error = %e, "x"); }\n')

    left, stale, _ = check.run(root, "")
    if [(f[0], f[1]) for f in left] != [("crates/outbound/src/lib.rs", 2)]:
        failures.append(f"the planted line: got {left}")
    elif "reason" not in left[0][2]:
        failures.append(f"the planted line names no field: {left[0][2]}")

    allow = (
        'crates/outbound/src/lib.rs | "outbound request refused" | a test reason\n'
        'crates/outbound/src/lib.rs | "no such line" | gone\n'
    )
    left, stale, _ = check.run(root, allow)
    if left:
        failures.append(f"an allowlisted line still flagged: {left}")
    if [e[1] for e in stale] != ['"no such line"']:
        failures.append(f"a stale entry not reported: {stale}")

    try:
        check.run(root, 'crates/outbound/src/lib.rs | "outbound request refused"\n')
        failures.append("an entry without a reason was taken")
    except SystemExit:
        pass

if failures:
    print("\n".join(failures))
    sys.exit(1)
print(f"check-no-user-data-at-info: {len(FLAGGED) + len(PASSES) + 7} cases pass")
