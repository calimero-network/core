#!/usr/bin/env python3
"""Write the copy of openapi/openapi.json that OpenAPI Generator reads.

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


def main():
    spec = json.load(open(sys.argv[1]))
    hoist_one_of_bodies(spec)
    drop_null_answers(spec)
    with open(sys.argv[2], "w") as out:
        json.dump(spec, out, indent=1, sort_keys=False)


if __name__ == "__main__":
    main()
