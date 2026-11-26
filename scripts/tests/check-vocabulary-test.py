#!/usr/bin/env python3
"""Tests for check-vocabulary.py: the API document and the docs use the glossary's words.

    python3 scripts/tests/check-vocabulary-test.py
"""

import importlib.util
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-vocabulary.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("check_vocabulary", SCRIPT)
vocab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vocab)

PAGE = "docs/src/content/docs/build/example.mdx"

PAGES = [
    # (name, text, problems expected)
    ("the glossary's words pass", "Create an app instance in an organization, on a team.\n", 0),
    ("a context is an app instance", "Each context has its own log.\n", 1),
    ("a namespace is an organization", "Invite people to the namespace.\n", 1),
    ("a subgroup is a team", "Subgroups nest.\n", 1),
    ("an application is a package", "Install the application first.\n", 1),
    ("a Raft group is a group", "The system group and each app group elect a leader.\n", 0),
    ("the context id is the glossary's", "The API takes the app id or its context id.\n", 0),
    ("v1's name for a thing may be given", "App instances (v1's contexts) and organizations (v1's namespaces).\n", 0),
    ("a media type is not an application", "Sent as `application/json` or application/octet-stream.\n", 0),
    ("a secure context is the web's", "Passkeys work only in a secure context.\n", 0),
    ("a code span is not prose", "WIT's `context` interface.\n", 0),
    ("a link target is not prose", "See [App instance lifecycle](/protocol/context-lifecycle/).\n", 0),
    ("prose in a code block is not judged", "```rust\n// the context\n```\n", 0),
    ("a retired command is caught", "```sh\nmeroctl namespace create acme\n```\n", 1),
    ("a retired command after a global option", "Run `meroctl --home ./bob group get o t`.\n", 1),
    ("app install is package install", "```sh\nmeroctl app install --path a.mpk\n```\n", 1),
    ("app build stays app", "```sh\nmeroctl app build && meroctl app pack .\n```\n", 0),
    ("a retired flag is caught", "```sh\nmeroctl instance create --name n --namespace o\n```\n", 1),
    ("an option's value is not the command", "```sh\nmeroctl --node group org list\n```\n", 0),
    ("contextId is gone", "It answers `{ appId, contextId }`.\n", 1),
    ("a line about v1 may name v1's commands", "v1's `meroctl context create` is `meroctl instance create`.\n", 0),
]


def main() -> int:
    failed = 0
    for name, text, want in PAGES:
        got = vocab.page_problems(PAGE, text)
        if len(got) != want:
            failed += 1
            print(f"FAIL {name}: want {want} problem(s), got {got}")
    exempt = vocab.page_problems(vocab.GLOSSARY, "A context is an app instance.\n")
    if exempt:
        failed += 1
        print(f"FAIL the glossary may name the retired words: {exempt}")
    migration = vocab.page_problems("docs/src/content/docs/clients/x/migrating-from-v1.mdx", "contexts\n")
    if migration:
        failed += 1
        print(f"FAIL a migration guide may name the retired words: {migration}")

    doc = {
        "paths": {
            "/apps/{app}": {"get": {"operationId": "app_get", "parameters": [{"name": "app"}],
                                    "description": "One app instance; `{app}` may be its context id."}},
            "/contexts/{context}": {"get": {"operationId": "context_get",
                                            "parameters": [{"name": "context"}]}},
        },
        "components": {"schemas": {
            "App": {"properties": {"appId": {}, "group": {}}, "description": "An app instance."},
            "TeamView": {"properties": {"apps": {"items": {"properties": {"contextId": {}}}}}},
            "NamespaceView": {"properties": {}, "description": "A namespace."},
        }},
        "tags": [{"name": "Apps", "description": "App instances (contexts)."}],
    }
    got = vocab.openapi_problems(doc)
    want = {
        "route `/contexts/{context}`", "operation `context_get`", "parameter `context`",
        "field `contextId`", "schema `NamespaceView`", "description: \"namespace\"",
        "description: \"contexts\"",
    }
    missing = [w for w in want if not any(w in p for p in got)]
    if missing or len(got) != len(want):
        failed += 1
        print(f"FAIL the API document: missing {missing}, got {got}")

    gloss = "## Retired words\n\n| Retired | Say |\n| --- | --- |\n| context | app instance |\n"
    if len(vocab.glossary_problems(gloss)) != 3:
        failed += 1
        print(f"FAIL a glossary missing three retired words: {vocab.glossary_problems(gloss)}")

    total = len(PAGES) + 4
    print(f"{total - failed}/{total} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
