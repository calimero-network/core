#!/usr/bin/env python3
"""Rewrite core's OpenAPI document into the dialect swift-openapi-generator reads.

Usage: scripts/openapi-normalize.py IN.json OUT.json

openapi/openapi.json stays byte for byte what core publishes; the generator
(1.10.3) reads this script's output instead. Each rewrite keeps the
document's meaning:

1. A nullable `anyOf` property (`[{$ref}, {type: null}]`, or several
   branches and `null`) loses its `null` branch and leaves its object's
   `required` list: the generator skips a `null` branch with a warning, and
   would wrap a lone reference in a one-field struct. An optional property
   is what the schema says. (Core spells every other nullable
   `type: [T, "null"]`, which needs nothing.)
2. `contentEncoding: base64url` is dropped: core keeps it on purpose (the
   value travels as base64url text), but OpenAPIKit knows only RFC 2045's
   encodings and refuses the document. The value stays a string.
3. A discriminated `oneOf` of inline objects (`Access`, the consent request
   body) has each branch moved to a component of its own,
   `<Owner><Value>` (`AccessTeam`, `ConsentPageCreateBodyUpgrade`), with a
   `discriminator.mapping` from its `kind`. Given inline branches the
   generator emits an enum with no cases at all; given references it emits
   one case per branch, decoded by `kind`.
4. Every non-2xx response is dropped. MeroKit's transport throws a refusal as
   its own typed error (`errorFrom`) before the generated client reads it, so
   the per-status types would never be reached. The document keeps them.
5. A success answer whose JSON schema is only `{type: null}` (the body is
   the JSON value `null`, as `POST /apps/{app}/transfer` answers) loses its
   content: the generator cannot model a null-only schema and skips it with
   a warning, and an answer with no content to decode is what the caller
   gets either way.
"""
import json
import sys


def without_null(schema):
    """`anyOf: [..., {type: null}]` without its null branch (None if it has none)."""
    if not isinstance(schema, dict):
        return None
    branches = schema.get("anyOf")
    if not isinstance(branches, list) or {"type": "null"} not in branches:
        return None
    rest = [b for b in branches if b != {"type": "null"}]
    if len(rest) == 1:
        return dict(rest[0])
    out = {k: v for k, v in schema.items() if k != "anyOf"}
    out["anyOf"] = rest
    return out


def normalize(node):
    if isinstance(node, dict):
        if node.get("contentEncoding") == "base64url":
            del node["contentEncoding"]
        properties = node.get("properties")
        if isinstance(properties, dict):
            for name, schema in properties.items():
                rewritten = without_null(schema)
                if rewritten is not None:
                    properties[name] = rewritten
                    if name in node.get("required", []):
                        node["required"] = [r for r in node["required"] if r != name]
        for value in node.values():
            normalize(value)
    elif isinstance(node, list):
        for value in node:
            normalize(value)


def pascal(text):
    return "".join(part[:1].upper() + part[1:] for part in text.replace("-", "_").split("_") if part)


def hoist_discriminated(document):
    """Move a discriminated oneOf's inline branches into named components."""
    components = document.setdefault("components", {}).setdefault("schemas", {})

    def hoist(schema, owner):
        discriminator = schema.get("discriminator")
        branches = schema.get("oneOf")
        if not discriminator or not isinstance(branches, list) or "mapping" in discriminator:
            return
        key = discriminator["propertyName"]
        mapping = {}
        refs = []
        for branch in branches:
            if "$ref" in branch:
                refs.append(branch)
                continue
            values = branch.get("properties", {}).get(key, {}).get("enum") or [branch["properties"][key].get("const")]
            name = owner + pascal(str(values[0]))
            components[name] = branch
            ref = "#/components/schemas/" + name
            mapping[str(values[0])] = ref
            refs.append({"$ref": ref})
        schema["oneOf"] = refs
        discriminator["mapping"] = mapping

    for name, schema in list(components.items()):
        hoist(schema, name)
    for path_item in document.get("paths", {}).values():
        for operation in path_item.values():
            if not isinstance(operation, dict):
                continue
            for media in operation.get("requestBody", {}).get("content", {}).values():
                hoist(media.get("schema", {}), pascal(operation.get("operationId", "")) + "Body")


def drop_refusals(document):
    for item in document.get("paths", {}).values():
        for operation in item.values():
            if isinstance(operation, dict) and "responses" in operation:
                operation["responses"] = {
                    status: answer for status, answer in operation["responses"].items() if status.startswith("2")
                }


def drop_null_answers(document):
    for item in document.get("paths", {}).values():
        for operation in item.values():
            if not isinstance(operation, dict):
                continue
            for status, answer in operation.get("responses", {}).items():
                content = answer.get("content") if isinstance(answer, dict) else None
                if not status.startswith("2") or not isinstance(content, dict):
                    continue
                for media in [m for m, body in content.items() if body.get("schema") == {"type": "null"}]:
                    del content[media]
                if not content:
                    del answer["content"]


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as f:
        document = json.load(f)
    normalize(document)
    hoist_discriminated(document)
    drop_refusals(document)
    drop_null_answers(document)
    with open(sys.argv[2], "w", encoding="utf-8") as f:
        json.dump(document, f, indent=1)
        f.write("\n")


if __name__ == "__main__":
    main()
