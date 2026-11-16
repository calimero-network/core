#!/usr/bin/env python3
"""Write the copy of core's docs/public/openapi.json that OpenAPI Generator reads.

    python3 codegen/prepare-spec.py <openapi.json> <out.json>

Two changes:

- An inline request body that is a `oneOf` (the consent request) moves to `components/schemas` as
  `<OperationId>Request`, so `schemaMappings` can name the hand-written type that writes it.
  OpenAPI Generator 7.26's kotlinx output renders such a `oneOf` as a sealed class no branch
  extends.
- An answer whose JSON schema is `{ type: null }` alone (`POST /apps/{app}/transfer` answers
  `null`, since core 92ac297a1) loses its content, so the operation returns `Unit`. The generator
  names such a schema as a model `Null` it never writes; `ApiClient` reads no body for `Unit`,
  which is all a `null` answer says.
- Every other union (`anyOf` / `oneOf` with more than one branch besides `null`) becomes free-form
  JSON (`{}`, a `JsonElement`), unless `codegen/build.gradle.kts` maps it to a hand-written type
  (MAPPED below). OpenAPI Generator 7.26's kotlinx output gets them all wrong: a union of scalars
  (`passkeys: integer | "3+"`) or of a string and an object becomes a class that cannot read the
  scalar, a `oneOf` of objects either a sealed class no branch extends (`AppScope`,
  `Run.final.outcome`) or one class whose every branch's fields are required (an app's
  `replicas`). Each failed to decode what the node answers.
"""

import json
import sys


def hoist_one_of_bodies(spec):
    schemas = spec.setdefault("components", {}).setdefault("schemas", {})
    for item in spec["paths"].values():
        for op in item.values():
            if not isinstance(op, dict):
                continue
            for media in op.get("requestBody", {}).get("content", {}).values():
                schema = media.get("schema", {})
                if "oneOf" in schema:
                    name = "".join(p[:1].upper() + p[1:] for p in op["operationId"].split("_")) + "Request"
                    schemas[name] = schema
                    HOISTED.add(name)
                    media["schema"] = {"$ref": f"#/components/schemas/{name}"}


def drop_null_answers(spec):
    for item in spec["paths"].values():
        for op in item.values():
            if not isinstance(op, dict):
                continue
            for response in op.get("responses", {}).values():
                content = response.get("content", {})
                media = content.get("application/json")
                if media is not None and media.get("schema") == {"type": "null"}:
                    del content["application/json"]
                    if not content:
                        del response["content"]


# The components `schemaMappings` maps to mero-core's own sealed types: their unions stay.
MAPPED = {"Access", "Deployer", "OutboundSelection"}


def is_null(branch):
    return isinstance(branch, dict) and branch.get("type") == "null" and len(branch) == 1


def free_form_unions(spec):
    def walk(node):
        if isinstance(node, list):
            for item in node:
                walk(item)
            return
        if not isinstance(node, dict):
            return
        for key in ("anyOf", "oneOf"):
            branches = node.get(key)
            if branches is None:
                continue
            rest = [b for b in branches if not is_null(b)]
            if len(rest) == 1 and "$ref" in rest[0]:
                continue  # `T | null`: the generator writes a nullable T
            kept = {k: v for k, v in node.items() if k in ("description", "title")}
            node.clear()
            node.update(kept)
            return
        for value in node.values():
            walk(value)

    walk(spec["paths"])
    for name, schema in spec.get("components", {}).get("schemas", {}).items():
        if name not in MAPPED | HOISTED:
            walk(schema)


# The request bodies hoist_one_of_bodies named, mapped to hand-written types as well.
HOISTED = set()


def main():
    spec = json.load(open(sys.argv[1]))
    hoist_one_of_bodies(spec)
    drop_null_answers(spec)
    free_form_unions(spec)
    with open(sys.argv[2], "w") as out:
        json.dump(spec, out, indent=1, sort_keys=False)


if __name__ == "__main__":
    main()
