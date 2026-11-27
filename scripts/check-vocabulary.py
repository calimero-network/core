#!/usr/bin/env python3
"""Fail when the API document or the docs use a word the glossary retired.

The glossary (docs/src/content/docs/protocol/glossary.mdx, "Retired words") names the
words v1 used and what replaced them: an app instance is not a context, an
organization not a namespace, a team not a group or subgroup, a signed package not an
application. One vocabulary holds across meroctl, the API, the SDKs and the docs;
meroctl's command tree is held to it by its own test (`help_uses_the_glossary_words`
in crates/meroctl/src/main.rs), and this script holds the rest:

- the node's API document, docs/public/openapi.json, from which every SDK's generated
  layer comes: no route, parameter, field, schema, operation or tag is named with a
  retired word, and no description uses one;
- the docs site's pages and the root README: no meroctl command line spells a command
  or flag the retired way (`meroctl context`, `meroctl namespace`, `meroctl app
  install`, `--context`, ...), no page names the `contextId` field, and no prose
  calls a thing by its retired name;
- the glossary lists every word this script retires.

What may still say a retired word, and why, is in ALLOWED (a phrase) and EXEMPT (a
page that is about the old names: a migration guide, the glossaries, the guides to
the SDKs' 0.x-named app layer).

    python3 scripts/check-vocabulary.py
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GLOSSARY = "docs/src/content/docs/protocol/glossary.mdx"
OPENAPI = "docs/public/openapi.json"
DOCS = "docs/src/content/docs"

# (retired word, its pattern in prose, the glossary's word for it)
RETIRED = [
    ("context", r"\bcontexts?\b", "app instance"),
    ("namespace", r"\bnamespaces?\b", "organization"),
    ("subgroup", r"\bsubgroups?\b", "team"),
    ("application", r"\bapplications?\b", "package (a signed .mpk) or app"),
]
# A name in the API document (a path, field, parameter, schema, operation or tag) never
# holds one. `group` is not here: in the API it is a Raft group (`App.group`).
IDENTIFIER = re.compile(r"context|namespace|subgroup|application", re.IGNORECASE)

# Phrases that keep a retired word on purpose: removed from a line before it is judged.
ALLOWED = [
    # The id SDK 0.12 apps saw, which every /apps/{app} route still accepts (glossary).
    r"\bcontext ids?\b",
    # The name a page gives a v1 thing it maps: "v1's contexts", "(v1's application)".
    r"\bv1's (?:\w+ )?(?:contexts?|namespaces?|subgroups?|applications?)\b",
    # A media type.
    r"\bapplication/[\w.+-]+",
    # The web platform's term (WebAuthn runs only in a secure context).
    r"\bsecure context\b",
]
ALLOWED_RE = re.compile("|".join(ALLOWED), re.IGNORECASE)

# Pages about the old names themselves.
EXEMPT = {
    GLOSSARY,  # defines the retired words
    f"{DOCS}/clients/mero-js/understand/glossary.mdx",  # maps v1's names for mero-js users
    f"{DOCS}/clients/mero-kotlin/understand/glossary.mdx",  # maps 0.x names
    f"{DOCS}/clients/mero-swift/guides/app-session.mdx",  # MeroAppBackend keeps 0.x names
    f"{DOCS}/clients/mero-kotlin/guides/app-session.mdx",  # MeroAppBackend keeps 0.x names
    f"{DOCS}/clients/mero-kotlin/guides/organizations.mdx",  # OrgInvitation links keep 0.x's keys
}
EXEMPT_NAME = re.compile(r"migrat", re.IGNORECASE)  # migration guides

# meroctl's global options that take a value, so `meroctl --home ./bob org join` is `org`.
VALUE_OPTIONS = {"--home", "--api", "--node", "--tee-profiles", "--min-release", "--api-key",
                 "--ops-token", "--operator-key-file"}
RETIRED_COMMANDS = {"context": "instance", "namespace": "org", "ns": "org", "group": "team"}
PACKAGE_COMMANDS = {"install", "list", "get", "versions", "get-latest-version", "pipelines",
                    "uninstall", "watch", "origins"}
RETIRED_FLAGS = {"--context": "--instance", "--namespace": "--org", "--group": "--team"}
CONTEXT_ID = re.compile(r"\bcontext_?ids?\b", re.IGNORECASE)


def retired_in(text: str) -> list[tuple[str, str]]:
    """The retired words `text` uses as prose, each with its replacement."""
    text = ALLOWED_RE.sub(" ", text)
    return [(m.group(0), new) for _, pat, new in RETIRED for m in re.finditer(pat, text, re.IGNORECASE)]


def meroctl_problems(line: str) -> list[str]:
    """What a meroctl command line in `line` spells the retired way."""
    out = []
    for m in re.finditer(r"\bmeroctl\b([^`\n|]*)", line):
        tokens = m.group(1).split()
        words, i = [], 0
        while i < len(tokens) and len(words) < 2:
            t = tokens[i]
            if t.startswith("-"):
                if t in VALUE_OPTIONS:
                    i += 1
            else:
                words.append(t)
            i += 1
        top, sub = (words + [None, None])[:2]
        if top in RETIRED_COMMANDS:
            out.append(f"`meroctl {top}` is `meroctl {RETIRED_COMMANDS[top]}`")
        if top == "app" and sub in PACKAGE_COMMANDS:
            out.append(f"`meroctl app {sub}` is `meroctl package {sub}`")
        for t in tokens:
            flag = t.split("=", 1)[0]
            if flag in RETIRED_FLAGS:
                out.append(f"`{flag}` is `{RETIRED_FLAGS[flag]}`")
    return out


def strip_markup(line: str) -> str:
    """A prose line without its code spans, link targets, HTML/JSX tags and URLs."""
    line = re.sub(r"`[^`]*`", " ", line)
    line = re.sub(r"\]\([^)]*\)", "]", line)
    line = re.sub(r"<[^>]*>", " ", line)
    line = re.sub(r"\b(?:href|link|slug)\s*[:=]\s*['\"][^'\"]*['\"]", " ", line)
    return re.sub(r"https?://\S+", " ", line)


def page_problems(rel: str, text: str) -> list[str]:
    exempt = rel in EXEMPT or EXEMPT_NAME.search(Path(rel).name)
    problems = []
    fence = False
    for n, line in enumerate(text.splitlines(), 1):
        at = f"{rel}:{n}"
        if line.lstrip().startswith(("```", "~~~")):
            fence = not fence
            continue
        if exempt or re.search(r"\bv1\b", line):
            continue
        problems += [f"{at}: {p}" for p in meroctl_problems(line)]
        if CONTEXT_ID.search(ALLOWED_RE.sub(" ", line)):
            problems.append(f"{at}: `contextId` is gone from the API: name an instance by its `appId`")
        if fence:
            continue
        for old, new in retired_in(strip_markup(line)):
            problems.append(f"{at}: \"{old}\": say {new}")
    return problems


def openapi_problems(doc: dict) -> list[str]:
    problems = []

    def name(kind: str, where: str, value: str) -> None:
        if IDENTIFIER.search(value):
            problems.append(f"{OPENAPI}: {kind} `{value}` ({where}) uses a retired word")

    def walk(node: object, where: str) -> None:
        if isinstance(node, dict):
            for key in ("description", "summary", "title"):
                if isinstance(node.get(key), str):
                    for old, new in retired_in(node[key]):
                        problems.append(f"{OPENAPI}: {where} {key}: \"{old}\": say {new}")
            if isinstance(node.get("properties"), dict):
                for prop in node["properties"]:
                    name("field", where, prop)
            for key, value in node.items():
                walk(value, f"{where}/{key}")
        elif isinstance(node, list):
            for i, value in enumerate(node):
                walk(value, f"{where}[{i}]")

    for path, ops in doc.get("paths", {}).items():
        name("route", path, path)
        for method, op in ops.items():
            if not isinstance(op, dict):
                continue
            if "operationId" in op:
                name("operation", f"{method} {path}", op["operationId"])
            for param in op.get("parameters", []):
                name("parameter", f"{method} {path}", param.get("name", ""))
    for schema in doc.get("components", {}).get("schemas", {}):
        name("schema", "components", schema)
    for tag in doc.get("tags", []):
        name("tag", "tags", tag.get("name", ""))
    walk(doc, "")
    return problems


def glossary_problems(text: str) -> list[str]:
    section = text.split("## Retired words", 1)
    if len(section) < 2:
        return [f"{GLOSSARY}: no \"## Retired words\" section"]
    rows = section[1].split("\n## ", 1)[0]
    return [f"{GLOSSARY}: \"Retired words\" does not list `{word}`"
            for word, _, _ in RETIRED if not re.search(rf"^\|[^|\n]*\b{word}", rows, re.MULTILINE)]


def problems(root: Path = ROOT) -> list[str]:
    out = glossary_problems((root / GLOSSARY).read_text())
    out += openapi_problems(json.loads((root / OPENAPI).read_text()))
    pages = sorted((root / DOCS).rglob("*.md*")) + [root / "README.md"]
    for page in pages:
        out += page_problems(page.relative_to(root).as_posix(), page.read_text())
    return out


def main() -> int:
    found = problems()
    for p in found:
        print(p)
    if found:
        print(f"\n{len(found)} use(s) of a retired word; the glossary's words: {GLOSSARY}, "
              "\"Retired words\".", file=sys.stderr)
        return 1
    print("the API document and the docs use the glossary's words")
    return 0


if __name__ == "__main__":
    sys.exit(main())
