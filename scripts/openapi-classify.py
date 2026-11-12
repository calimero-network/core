#!/usr/bin/env python3
"""Classify a change to the node's OpenAPI document by what it does to an SDK generated from it.

    python3 scripts/openapi-classify.py BASE REVISION [--markdown OUT] [--footer OUT]

BASE and REVISION are two versions of `docs/public/openapi.json` (an empty or missing BASE
is a first vendoring). Prints one JSON object:

    {"class": "breaking" | "additive" | "other" | "none",
     "type": "feat!" | "feat" | "chore" | "",     # the conventional-commit type to title it with
     "summary": "2 breaking, 1 additive, 0 other",
     "breaking": [{"where", "change", "id"}, ...], "additive": [...], "other": [...]}

`--markdown` writes the three lists for a pull request body, `--footer` the
`BREAKING-CHANGE:` footer for the commit (empty unless the class is breaking). The SDKs'
semantic-release reads `feat(api)!:` / `BREAKING-CHANGE:` as major, `feat` as minor and
`chore` as patch, so the class decides the SDK's next version.

Each change is judged from a caller of a generated client, which is stricter than an HTTP
client in a few places:

- the structural diff is oasdiff's (`oasdiff changelog`, pinned by scripts/install-oasdiff.sh,
  found as $OASDIFF or on PATH). Its errors and warnings are breaking (a removed request
  property or parameter is a warning there, and drops an argument here); its infos are
  additive, except those in OTHER_IDS, which change no generated code;
- a few infos break generated code and count as breaking (BREAKING_IDS): a removed optional
  response property is a field the SDK's model no longer has, a removed operationId or
  component schema is a method or a type that is gone, and removing a deprecated route
  still removes a method;
- oasdiff ignores the Calimero extensions, which the SDKs generate from (see EXTENSIONS): an
  operation that gains `x-calimero-step-up`, `x-calimero-operator` or `x-calimero-sealed`,
  needs stronger user verification, stops being retry-safe or moves to another
  `x-calimero-group` (another facade namespace) breaks its callers; losing one is additive;
  a new or dropped `x-calimero-codes` entry, or a reworded `x-calimero-who`, is other.

A document that changed in none of these ways (descriptions, examples) is `other`; an
identical one is `none`. Tests: scripts/tests/openapi-classify-test.py.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

METHODS = ("get", "put", "post", "delete", "patch", "head", "options", "trace")
ORDER = ("breaking", "additive", "other")

# oasdiff infos that remove something a generated client exposes.
BREAKING_IDS = {
    "response-optional-property-removed",
    "api-operation-id-removed",
    "api-schema-removed",
}
# oasdiff findings that change no generated code (core deprecates through
# crates/node/api-deprecations.toml, not sunset dates).
OTHER_IDS = {
    "api-deprecated-sunset-missing",
    "api-deprecated-sunset-parse",
    "api-sunset-date-changed-earlier",
    "api-sunset-date-changed-too-small",
    "api-sunset-date-too-small",
    "response-non-success-status-added",
    "response-non-success-status-removed",
    "api-tag-added",
    "api-tag-removed",
}

# Operation-level extensions: name -> how to judge (before, after).
UV_RANK = {None: 0, "discouraged": 0, "preferred": 1, "required": 2}


def flag(gained: str, lost: str, gaining_breaks: bool = True):
    """A boolean extension: gaining it breaks callers (or, with gaining_breaks=False, losing it does)."""

    def judge(before, after, _op_before, _op_after):
        if bool(before) == bool(after):
            return None
        breaks = bool(after) == gaining_breaks
        return ("breaking" if breaks else "additive", gained if after else lost)

    return judge


def user_verification(before, after, _ob, _oa):
    b, a = UV_RANK.get(before, 1), UV_RANK.get(after, 1)
    if a > b:
        return "breaking", f"user verification raised from `{before}` to `{after}`"
    if a < b:
        return "additive", f"user verification lowered from `{before}` to `{after}`"
    return "other", f"user verification `{before}` became `{after}`"


def step_up_when(before, after, op_before, op_after):
    stepped = op_before.get("x-calimero-step-up") and op_after.get("x-calimero-step-up")
    if stepped and before and not after:
        return "breaking", "now always needs a step-up (was only when " + str(before) + ")"
    if stepped and after and not before:
        return "additive", "now needs a step-up only when " + str(after)
    return "other", f"`x-calimero-step-up-when` changed to {json.dumps(after)}"


def retry_requires(before, after, op_before, op_after):
    if op_before.get("x-calimero-retry-safe") and op_after.get("x-calimero-retry-safe") and after:
        return "breaking", f"a retry now needs `{after}`" + (f" (was `{before}`)" if before else "")
    return "other", f"`x-calimero-retry-requires` changed to {json.dumps(after)}"


def group(before, after, _ob, _oa):
    return "breaking", f"moved from facade group `{before}` to `{after}`"


def who(before, after, _ob, _oa):
    return "other", f"who may call it: {after}"


EXTENSIONS = {
    "x-calimero-step-up": flag("now needs a step-up (`x-calimero-step-up`)", "no longer needs a step-up"),
    "x-calimero-operator": flag("now needs an operator signature (`x-calimero-operator`)", "no longer needs an operator signature"),
    "x-calimero-sealed": flag("now travels only in the sealed channel (`x-calimero-sealed`)", "no longer needs the sealed channel"),
    "x-calimero-retry-safe": flag("is now retry-safe", "is no longer retry-safe (`x-calimero-retry-safe`)", gaining_breaks=False),
    "x-calimero-user-verification": user_verification,
    "x-calimero-step-up-when": step_up_when,
    "x-calimero-retry-requires": retry_requires,
    "x-calimero-group": group,
    "x-calimero-who": who,
}


def load(path: str) -> dict:
    p = Path(path)
    if not p.is_file() or not p.read_text(encoding="utf-8").strip():
        return {}
    return json.loads(p.read_text(encoding="utf-8"))


def operations(doc: dict):
    for path, item in (doc.get("paths") or {}).items():
        for method in METHODS:
            op = (item or {}).get(method)
            if isinstance(op, dict):
                yield f"{method.upper()} {path}", op


def codes(op: dict) -> dict:
    return {
        status: set(resp.get("x-calimero-codes") or [])
        for status, resp in (op.get("responses") or {}).items()
        if isinstance(resp, dict)
    }


def extension_changes(base: dict, revision: dict) -> list[tuple[str, str, str, str]]:
    """(class, where, change, id) for each Calimero extension that moved on an operation in both."""
    found = []
    after_ops = dict(operations(revision))
    for where, ob in operations(base):
        oa = after_ops.get(where)
        if oa is None:
            continue  # oasdiff reports the removal
        names = sorted(k for k in set(ob) | set(oa) if k.startswith("x-calimero-") and k != "x-calimero-codes")
        for name in names:
            b, a = ob.get(name), oa.get(name)
            if b == a:
                continue
            judge = EXTENSIONS.get(name)
            verdict = judge(b, a, ob, oa) if judge else ("other", f"`{name}` changed to {json.dumps(a)}")
            if verdict:
                found.append((verdict[0], where, verdict[1], name))
        cb, ca = codes(ob), codes(oa)
        for status in sorted(set(cb) | set(ca)):
            added = sorted(ca.get(status, set()) - cb.get(status, set()))
            removed = sorted(cb.get(status, set()) - ca.get(status, set()))
            if added:
                found.append(("other", where, f"may also answer {status} " + ", ".join(f"`{c}`" for c in added), "x-calimero-codes"))
            if removed:
                found.append(("other", where, f"no longer answers {status} " + ", ".join(f"`{c}`" for c in removed), "x-calimero-codes"))
    return found


def oasdiff(base: str, revision: str) -> list[dict]:
    binary = os.environ.get("OASDIFF") or shutil.which("oasdiff")
    if not binary:
        sys.exit("oasdiff not found: run scripts/install-oasdiff.sh, or set OASDIFF")
    out = subprocess.run(
        [binary, "changelog", base, revision, "--format", "json", "--level", "INFO", "--allow-external-refs=false"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(out or "[]") or []


def structural_class(change: dict) -> str:
    cid = change.get("id", "")
    if cid in OTHER_IDS:
        return "other"
    if cid in BREAKING_IDS or cid.endswith("-with-deprecation") or change.get("level", 1) >= 2:
        return "breaking"
    return "additive"


def classify(base_path: str, revision_path: str) -> dict:
    base, revision = load(base_path), load(revision_path)
    lists: dict[str, list] = {k: [] for k in ORDER}
    if base == revision:
        cls = "none"
    elif not base:
        lists["additive"].append({"where": "", "change": "first vendoring of the document", "id": "first-vendoring"})
        cls = "additive"
    else:
        with tempfile.TemporaryDirectory() as tmp:
            # oasdiff reads both through files; copy so an empty or odd path is never passed on.
            b, r = Path(tmp, "base.json"), Path(tmp, "revision.json")
            b.write_text(json.dumps(base), encoding="utf-8")
            r.write_text(json.dumps(revision), encoding="utf-8")
            for change in oasdiff(str(b), str(r)):
                where = f"{change.get('operation', '')} {change.get('path', '')}".strip()
                lists[structural_class(change)].append({"where": where, "change": change.get("text", ""), "id": change.get("id", "")})
        for cls_, where, text, cid in extension_changes(base, revision):
            lists[cls_].append({"where": where, "change": text, "id": cid})
        cls = next((k for k in ORDER if lists[k]), "other")
        if not any(lists.values()):
            lists["other"].append({"where": "", "change": "descriptions, examples or other text only", "id": "text-only"})
    for k in ORDER:  # stable, deduplicated (oasdiff may report one change per media type)
        seen, unique = set(), []
        for item in sorted(lists[k], key=lambda i: (i["where"], i["id"], i["change"])):
            key = (item["where"], item["change"])
            if key not in seen:
                seen.add(key)
                unique.append(item)
        lists[k] = unique
    return {
        "class": cls,
        "type": {"breaking": "feat!", "additive": "feat", "other": "chore", "none": ""}[cls],
        "summary": ", ".join(f"{len(lists[k])} {k}" for k in ORDER),
        **lists,
    }


def line(item: dict) -> str:
    return f"`{item['where']}`: {item['change']}" if item["where"] else item["change"]


def markdown(result: dict, limit: int = 200) -> str:
    if result["class"] == "none":
        return "The API document did not change.\n"
    out = []
    titles = {
        "breaking": "Breaking: callers of the generated client must change",
        "additive": "Additive",
        "other": "Other (no generated signature changes)",
    }
    for k in ORDER:
        items = result[k]
        out += [f"**{titles[k]}** ({len(items)})", ""]
        if not items:
            out.append("None.")
        for item in items[:limit]:
            out.append(f"- {line(item)}")
        if len(items) > limit:
            out.append(f"- ... and {len(items) - limit} more")
        out.append("")
    return "\n".join(out).rstrip() + "\n"


def footer(result: dict, limit: int = 30) -> str:
    """The commit footer semantic-release reads as a major release; empty unless breaking.

    Only the first line carries the keyword; the list below it belongs to the same note."""
    items = result["breaking"]
    if result["class"] != "breaking":
        return ""
    lines = ["BREAKING-CHANGE: core's node API changed in a way callers of the generated client see:"]
    lines += [f"- {line(i)}" for i in items[:limit]]
    if len(items) > limit:
        lines.append(f"- ... and {len(items) - limit} more (see the pull request)")
    return "\n".join(lines) + "\n"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("base")
    parser.add_argument("revision")
    parser.add_argument("--markdown", help="write the change lists, as Markdown, here")
    parser.add_argument("--footer", help="write the BREAKING-CHANGE footer here (empty unless breaking)")
    args = parser.parse_args(argv)
    result = classify(args.base, args.revision)
    if args.markdown:
        Path(args.markdown).write_text(markdown(result), encoding="utf-8")
    if args.footer:
        Path(args.footer).write_text(footer(result), encoding="utf-8")
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
