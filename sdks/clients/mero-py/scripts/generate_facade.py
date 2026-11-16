#!/usr/bin/env python3
"""Generate calimero_client_py/facade.py: one method per operation.

Run by scripts/generate_openapi_client.py after it generated
calimero_client_py/openapi (it imports that package, from `--tree`, to read
each call's parameters and body model), never by hand.

Each operation becomes a method of the group its `x-calimero-group` names
(`client.orgs`, `client.apps`, ...), named after its operationId in snake
case with the group's own prefix dropped (`org_get` -> `client.orgs.get`,
`ops_status` -> `client.ops.status`; a name that would then collide keeps the
prefix; `RENAMES` names the rest). How the transport treats it comes from the
document too: `x-calimero-step-up` (a passkey answers its 412),
`x-calimero-operator` (the operator key answers its 428),
`x-calimero-retry-safe` / `x-calimero-retry-requires` (sent again, or to the
next node, while the node is unavailable; only once the named field is set),
and a `421 NOT_LEADER` among its answers (followed to the leader). Its
arguments are keyword-only, the path's, the query's and the JSON body's fields
flattened (a raw-byte body is `body=`); it returns the generated model of the
success answer and raises `CloudError` on a refusal. A step-up is answered
from the client (`Client.use_passkey`, `Client.use_operator_key`); `confirm=`
and `operator_proof=` override it per call. The event streams are not
operations here: `Client.events` and `Client.presence_stream` read them.
"""

import argparse
import importlib
import inspect
import json
import keyword
import sys
import typing
from enum import Enum
from pathlib import Path

# operationIds whose snake case does not say what the call does.
RENAMES = {"intents": "call", "views": "view"}

# Sent once: a passkey ceremony, which its caller repeats with the passkey.
ONCE = {
    "register_begin",
    "register_finish",
    "login_begin",
    "login_finish",
    "account_recovery_begin",
    "account_recovery_start",
    "account_recovery_finish",
}
# Answers that are streams: not methods here.
STREAMS = ("text/event-stream",)

OVERRIDES = ("confirm", "operator_proof")


def class_name(group: str) -> str:
    return "".join(w.capitalize() for w in group.split("_"))


def prefixes(group: str):
    """What an operationId of `group` drops: the group's name, and its
    singular (`orgs` -> `org_get` is `get`)."""
    return [group] + ([group[:-1]] if group.endswith("s") else [])


def method_name(operation_id: str, prefixes) -> str:
    if operation_id in RENAMES:
        return RENAMES[operation_id]
    for prefix in prefixes:
        if operation_id.startswith(prefix + "_"):
            rest = operation_id[len(prefix) + 1 :]
            if rest and not keyword.iskeyword(rest):
                return rest
    return operation_id


def python_name(prop: str) -> str:
    from openapi_python_client.utils import PythonIdentifier

    return str(PythonIdentifier(value=prop, prefix="field"))


def resolve(document: dict, schema: dict) -> dict:
    while "$ref" in schema:
        name = schema["$ref"].rsplit("/", 1)[1]
        schema = document["components"]["schemas"][name]
    return schema


# How core's document says an object with listed properties takes others
# too: it leaves out `additionalProperties` (JSON Schema admits them anyway)
# and says so in the schema's description. Every other body without
# `additionalProperties` is one whose other fields the node ignores, which
# the facade does not offer.
OTHER_PROPERTIES = (
    "Other properties than those listed may be present: "
    "the ones listed are those clients rely on."
)


def open_body(schema) -> bool:
    """Whether a body takes fields the document does not name."""
    rest = schema.get("additionalProperties")
    if rest is None:
        return OTHER_PROPERTIES in schema.get("description", "")
    return rest is not False


def enum_of(annotation):
    """The Enum class an annotation names (directly or in a Union), if any."""
    if isinstance(annotation, type) and issubclass(annotation, Enum):
        return annotation
    for arg in typing.get_args(annotation):
        found = enum_of(arg)
        if found is not None:
            return found
    return None


def success_type(module) -> str:
    ret = inspect.signature(module.sync).return_annotation
    names = []
    for arg in typing.get_args(ret) or (ret,):
        if arg is type(None):
            continue
        name = getattr(arg, "__name__", str(arg))
        if name != "Error":
            names.append(name)
    return " | ".join(names) or "None"


def doc_text(text: str) -> str:
    return text.replace("\\", "\\\\").replace('"""', "'''").strip()


def operations(document: dict):
    for path, item in document["paths"].items():
        for method, op in item.items():
            if isinstance(op, dict) and "operationId" in op:
                yield path, method.upper(), op


def generate(document: dict) -> str:
    groups = {}
    imports = set()
    for path, method, op in operations(document):
        ok = op["responses"].get("200", {}).get("content", {})
        if any(t in ok for t in STREAMS):
            continue
        groups.setdefault(op["x-calimero-group"], []).append((path, method, op))

    out = [
        '"""The operations of core\'s node API, one method each: generated, do not edit.',
        "",
        "Made by scripts/generate_facade.py from openapi/openapi.json and the",
        "generated calls in `calimero_client_py.openapi`. Reach them on a client:",
        '`client.orgs.invite_create(org=..., role="member")`. Arguments are',
        "keyword-only: the path's, the query's and the JSON body's fields; a",
        "raw-byte body is `body=`. Each returns the generated model of the success",
        "answer and raises `CloudError` on a refusal. A `412 CONFIRM_REQUIRED` is",
        "answered with the client's passkey (`Client.use_passkey`) and a `428",
        "OPERATOR_SIGNATURE_REQUIRED` with its operator key",
        "or signer (`Client.use_operator_key`, `Client.use_operator_signer`); per",
        "call, `confirm=` takes a passkey, a ready `confirm` value or `False`, and",
        "`operator_proof=` an operator seed (hex), a signer, a ready",
        "`operatorProof` value or `False`. `client.json.<group>` answers JSON.",
        '"""',
        "",
        "IMPORTS",
        "from ._facade import UNSET, Group, Operation",
        "",
    ]
    out_groups = []
    for group in sorted(groups):
        cls = class_name(group)
        ids = [op["operationId"] for _, _, op in groups[group]]
        names = {}
        for i in ids:
            names[i] = method_name(i, prefixes(group))
        taken = {}
        for i, n in names.items():
            taken.setdefault(n, []).append(i)
        for n, which in taken.items():
            if len(which) > 1:
                for i in which:
                    names[i] = i
        lines = [
            f"class {cls}(Group):",
            f'    """`client.{group}`: the operations core\'s document groups as `{group}`."""',
            "",
        ]
        for path, method, op in sorted(
            groups[group], key=lambda x: names[x[2]["operationId"]]
        ):
            oid = op["operationId"]
            pkg = attr_pkg(op["tags"][0])
            module = importlib.import_module(
                f"calimero_client_py.openapi.api.{pkg}.{oid}"
            )
            imports.add(f"from .openapi.api.{pkg} import {oid} as _{oid}")
            sig = inspect.signature(module.sync_detailed)
            params = [p for n, p in sig.parameters.items() if n != "client"]
            path_params = [p for p in params if p.kind is p.POSITIONAL_OR_KEYWORD]
            query_params = [
                p for p in params if p.kind is p.KEYWORD_ONLY and p.name != "body"
            ]
            body_param = sig.parameters.get("body")
            args, path_exprs, query_exprs, body_exprs = [], [], [], []
            seen = set()
            argdocs = []

            def add(name, required, doc):
                if name in seen:
                    raise SystemExit(f"{oid}: argument {name} named twice")
                seen.add(name)
                args.append(name if required else f"{name}=UNSET")
                argdocs.append(f"            {name}: {doc}")

            for p in path_params:
                add(p.name, True, "path")
                path_exprs.append(p.name)
            for p in query_params:
                required = p.default is inspect.Parameter.empty
                add(p.name, required, "query")
                enum = enum_of(p.annotation)
                if enum is not None:
                    imports.add(f"from .openapi.models import {enum.__name__}")
                    query_exprs.append(
                        f"{p.name!r}: Operation.enum({enum.__name__}, {p.name})"
                    )
                else:
                    query_exprs.append(f"{p.name!r}: {p.name}")
            body_kind = "none"
            model = None
            more = False
            any_type = False
            if body_param is not None:
                ann = body_param.annotation
                ann_name = getattr(ann, "__name__", str(ann))
                if ann_name == "File":
                    body_kind = "bytes"
                    add("body", True, "the raw bytes")
                    body_exprs = None
                    # A body of any media type (`*/*`, a blob) is sent under
                    # the caller's, by default `application/octet-stream`.
                    any_type = any("*" in t for t in op["requestBody"]["content"])
                    if any_type:
                        args.append('content_type="application/octet-stream"')
                        argdocs.append(
                            "            content_type: the body's media type"
                        )
                elif typing.get_origin(ann) is typing.Union:
                    # One of several shapes (`oneOf`): the body is given whole.
                    body_kind = "oneof"
                    variants = [
                        getattr(a, "__forward_arg__", getattr(a, "__name__", str(a)))
                        for a in typing.get_args(ann)
                    ]
                    for v in variants:
                        imports.add(f"from .openapi.models import {v}")
                    model = "(" + ", ".join(variants) + ",)"
                    add("body", True, "the body, one of " + ", ".join(variants))
                    body_exprs = None
                elif ann is str:
                    body_kind = "text"
                    add("body", True, "the body, as text")
                    body_exprs = None
                else:
                    body_kind = "model"
                    model = ann.__name__
                    imports.add(f"from .openapi.models import {model}")
                    rb = op["requestBody"]
                    schema = resolve(
                        document, rb["content"]["application/json"]["schema"]
                    )
                    required = (
                        set(schema.get("required", [])) if rb.get("required") else set()
                    )
                    for prop, ps in schema.get("properties", {}).items():
                        name = python_name(prop)
                        if name in OVERRIDES or prop in ("confirm", "operatorProof"):
                            continue
                        ps = resolve(document, ps) if "$ref" in ps else ps
                        add(name, prop in required, f"body `{prop}`")
                        body_exprs.append(f"{prop!r}: {name}")
                    if open_body(schema):
                        # Fields the document does not name (`ops_quotas_set`:
                        # each limit by its key), as given.
                        body_exprs.append("**more")
                        more = True
            responses = op["responses"]
            # `x-calimero-step-up` marks the routes that can ask for a passkey
            # (`x-calimero-step-up-when` says when one asks only sometimes).
            confirm = bool(op.get("x-calimero-step-up"))
            operator = bool(op.get("x-calimero-operator"))
            leader = "NOT_LEADER" in responses.get("421", {}).get(
                "x-calimero-codes", []
            )
            retry_safe = bool(op.get("x-calimero-retry-safe"))
            retry_requires = op.get("x-calimero-retry-requires")
            if confirm:
                args.append("confirm=None")
                argdocs.append(
                    "            confirm: a passkey for the step-up, a ready `confirm`, or False"
                )
            if operator:
                args.append("operator_proof=None")
                argdocs.append(
                    "            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False"
                )
            if more:
                args.append("**more")
                argdocs.append(
                    "            more: further body fields, by their JSON names"
                )
            answer = "json"
            ok = responses.get("200", {}).get("content", {})
            if "text/plain" in ok:
                answer = "text"
            elif any("json" not in t for t in ok):
                answer = "bytes"
            flags = {
                "confirm": confirm,
                "operator": operator,
                "leader": leader,
                "once": oid in ONCE,
                "repeatable": retry_safe and not retry_requires,
                "retry_requires": repr(retry_requires) if retry_requires else None,
            }
            spec_name = f"_{oid.upper()}"
            out_groups.append(
                f"{spec_name} = Operation(_{oid}, {oid!r}, body={model or repr(body_kind)}, "
                f"answer={answer!r}, "
                + ", ".join(f"{k}={v}" for k, v in flags.items() if v)
                + ")"
            )
            summary = doc_text(op.get("summary", oid))
            description = doc_text(op.get("description", ""))
            sig_args = ", ".join(["self", "*"] + args) if args else "self"
            lines.append(f"    def {names[oid]}({sig_args}):")
            doc = [f'        """{summary} (`{method} {path}`, `{oid}`).', ""]
            if description:
                doc += [
                    "        " + l if l else "" for l in description.splitlines()
                ] + [""]
            if argdocs:
                doc += ["        Args:"] + argdocs + [""]
            doc += [f"        Returns: {success_type(module)}", '        """']
            lines += doc
            path_t = "(" + "".join(f"{e}, " for e in path_exprs) + ")"
            query_t = "{" + ", ".join(query_exprs) + "}"
            if body_exprs is None:
                body_t = "body"
            elif body_kind == "model":
                body_t = "{" + ", ".join(body_exprs) + "}"
            else:
                body_t = "None"
            extra = ""
            if confirm:
                extra += ", confirm=confirm"
            if operator:
                extra += ", operator_proof=operator_proof"
            if any_type:
                extra += ", content_type=content_type"
            lines.append(
                f"        return self._run({spec_name}, {path_t}, {query_t}, {body_t}{extra})"
            )
            lines.append("")
        out_groups.append("")
        out_groups.append("\n".join(lines))
    groups_table = ["", "", "# The client's attribute for each group.", "GROUPS = {"]
    for group in sorted(groups):
        groups_table.append(f"    {group!r}: {class_name(group)},")
    groups_table.append("}")
    text = (
        "\n".join(out) + "\n" + "\n".join(out_groups) + "\n".join(groups_table) + "\n"
    )
    return text.replace("IMPORTS", "\n".join(sorted(imports)))


def attr_pkg(tag: str) -> str:
    from openapi_python_client.utils import PythonIdentifier

    return str(PythonIdentifier(value=tag, prefix="tag"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--document", type=Path, required=True)
    parser.add_argument("--tree", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    sys.path.insert(0, str(args.tree))
    document = json.loads(args.document.read_text())
    args.out.write_text(generate(document))
    return 0


if __name__ == "__main__":
    sys.exit(main())
