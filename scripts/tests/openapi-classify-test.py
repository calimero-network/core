#!/usr/bin/env python3
"""Tests for openapi-classify.py: each kind of API change gets the release an SDK owes it.

    OASDIFF=<path> python3 scripts/tests/openapi-classify-test.py

Needs oasdiff ($OASDIFF, or on PATH; scripts/install-oasdiff.sh puts the pinned one there).
Each case edits fixtures/openapi-classify-base.json and expects one class.
"""

import copy
import importlib.util
import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / "openapi-classify.py"
BASE = json.loads((HERE / "fixtures" / "openapi-classify-base.json").read_text())
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("openapi_classify", SCRIPT)
classify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(classify)

TEAMS = "/orgs/{org}/teams"


def edit(fn):
    def apply(doc):
        fn(doc)
        return doc

    return apply


def op(doc, method, path=TEAMS):
    return doc["paths"][path][method]


def create_body(doc):
    return op(doc, "post")["requestBody"]["content"]["application/json"]["schema"]


def added_route(doc):
    doc["paths"]["/orgs/{org}/teams/{team}"] = {
        "delete": {
            "operationId": "team_delete",
            "x-calimero-group": "orgs",
            "parameters": [
                {"name": "org", "in": "path", "required": True, "schema": {"type": "string"}},
                {"name": "team", "in": "path", "required": True, "schema": {"type": "string"}},
            ],
            "responses": {"204": {"description": "Deleted."}},
        }
    }


CASES = [
    # (name, edit, class expected)
    ("an identical document", lambda d: None, "none"),
    ("a reworded description changes nothing generated", edit(lambda d: op(d, "get").update(description="Every team.")), "other"),
    ("an added route", edit(added_route), "additive"),
    ("an added optional request field", edit(lambda d: create_body(d)["properties"].update(color={"type": "string"})), "additive"),
    ("an added response field", edit(lambda d: d["components"]["schemas"]["Team"]["properties"].update(size={"type": "integer"})), "additive"),
    ("a widened request enum", edit(lambda d: create_body(d)["properties"]["visibility"]["enum"].append("secret")), "additive"),
    ("a removed route", edit(lambda d: d["paths"].pop("/health")), "breaking"),
    ("a removed operation", edit(lambda d: d["paths"][TEAMS].pop("post")), "breaking"),
    ("a removed optional response field", edit(lambda d: d["components"]["schemas"]["Team"]["properties"].pop("parent")), "breaking"),
    ("a removed required response field",
     edit(lambda d: (d["components"]["schemas"]["Team"]["properties"].pop("name"), d["components"]["schemas"]["Team"]["required"].remove("name"))),
     "breaking"),
    ("a new required request field",
     edit(lambda d: (create_body(d)["properties"].update(owner={"type": "string"}), create_body(d)["required"].append("owner"))),
     "breaking"),
    ("a removed optional request field", edit(lambda d: create_body(d)["properties"].pop("note")), "breaking"),
    ("a narrowed request enum", edit(lambda d: create_body(d)["properties"]["visibility"]["enum"].remove("closed")), "breaking"),
    ("a changed field type", edit(lambda d: d["components"]["schemas"]["Team"]["properties"].update(id={"type": "integer"})), "breaking"),
    ("a renamed operationId", edit(lambda d: op(d, "get").update(operationId="teams_list")), "breaking"),
    ("an operation that now needs a step-up", edit(lambda d: op(d, "post").update({"x-calimero-step-up": True, "x-calimero-user-verification": "required"})), "breaking"),
    ("an operation that now needs an operator signature", edit(lambda d: op(d, "post").update({"x-calimero-operator": True})), "breaking"),
    ("a public route that is now sealed", edit(lambda d: op(d, "get", "/health").update({"x-calimero-sealed": True})), "breaking"),
    ("an operation that is no longer retry-safe", edit(lambda d: op(d, "get").pop("x-calimero-retry-safe")), "breaking"),
    ("an operation that moved facade group", edit(lambda d: op(d, "get").update({"x-calimero-group": "teams"})), "breaking"),
    ("an operation that now is retry-safe", edit(lambda d: op(d, "post").update({"x-calimero-retry-safe": True})), "additive"),
    ("a new x-calimero-codes entry", edit(lambda d: op(d, "post")["responses"]["409"]["x-calimero-codes"].append("TEAM_LIMIT")), "other"),
    ("a reworded x-calimero-who", edit(lambda d: op(d, "get").update({"x-calimero-who": "a member"})), "other"),
    ("an added route and a removed field together", edit(lambda d: (added_route(d), create_body(d)["properties"].pop("note"))), "breaking"),
]


def run(base, revision):
    with tempfile.TemporaryDirectory() as tmp:
        b, r = Path(tmp, "base.json"), Path(tmp, "revision.json")
        b.write_text(json.dumps(base) if base is not None else "")
        r.write_text(json.dumps(revision))
        return classify.classify(str(b), str(r))


def main() -> int:
    failures = []
    for name, change, expected in CASES:
        revision = copy.deepcopy(BASE)
        change(revision)
        result = run(BASE, revision)
        ok = result["class"] == expected
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {result['class']} ({result['summary']})")
        if not ok:
            failures.append(f"{name}: expected {expected}, got {result['class']}: {json.dumps(result)}")
            continue
        footer = classify.footer(result)
        body = classify.markdown(result)
        # semantic-release reads a line starting BREAKING-CHANGE as a major release, wherever it is.
        if (expected == "breaking") != footer.startswith("BREAKING-CHANGE: "):
            failures.append(f"{name}: footer {footer!r}")
        if any(l.startswith("BREAKING-CHANGE") for l in body.splitlines() + footer.splitlines()[1:]):
            failures.append(f"{name}: a BREAKING-CHANGE line outside the footer's first line")
        if result["type"] != {"breaking": "feat!", "additive": "feat", "other": "chore", "none": ""}[expected]:
            failures.append(f"{name}: type {result['type']}")

    first = run(None, BASE)
    print(f"{'ok  ' if first['class'] == 'additive' else 'FAIL'} a first vendoring: {first['class']}")
    if first["class"] != "additive":
        failures.append(f"a first vendoring: {first['class']}")

    for f in failures:
        print(f"::error::{f}")
    print(f"{len(CASES) + 1 - len(failures)}/{len(CASES) + 1} passed" if not failures else f"{len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
