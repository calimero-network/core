#!/usr/bin/env python3
"""Write MeroKit's facade over the generated MeroAPI client.

Usage: scripts/generate-facade.py OPENAPI.json TYPES.swift OUT.swift

One `async throws` method per operation swift-openapi-generator generated,
on a group named after the operation's first tag (`client.orgs`,
`client.apps`, ...): the path parameters, the JSON body's properties and the
query parameters as labelled arguments, the typed success value as the
answer. `confirm` and `operatorProof` are no arguments: the client answers a
`412 CONFIRM_REQUIRED` and a `428 OPERATOR_SIGNATURE_REQUIRED` itself
(`StepUpMiddleware`; the document's `x-calimero-step-up` and
`x-calimero-operator`), with the `passkey:` or `operatorSigner:` given to the
call, else the client's own. An operation the document marks
`x-calimero-retry-safe` is retried on a 503 or a lost connection, with
failover (one marked `x-calimero-retry-requires: idempotencyKey` only when
the call gives one). A binary body whose document lists `*/*` takes a
`contentType:` argument (`apps.blobUpload(app:body:contentType:)`): alone,
it defaults to `application/octet-stream`; beside a named type, it is
optional and nil sends that type. An optional body property the
document makes nullable is a `T??` argument: `.some(nil)` sends an explicit
`null` (`orgs.teamUpdate(parent: .some(nil))` moves a team to the top).
Refusals throw MeroKit's errors.

The Swift names and types are read from the generated Types.swift (so they
are the generator's, whatever its naming), the tags and step-ups from the
document. Called by scripts/generate-openapi.sh; never edit the output.
"""
import json
import os
import re
import sys

# The operationId prefix a group's method names drop (`org_get` -> `client.orgs.get`).
PREFIXES = {"apps": "app", "auth": "account", "consent": "consent", "deployments": "deployment",
            "orgs": "org", "ops": "ops", "packages": "package"}
STEP_UP_FIELDS = ("confirm", "operatorProof")
SWIFT_KEYWORDS = {"default", "public", "private", "internal", "func", "var", "let", "in", "is", "as", "operator",
                  "protocol", "struct", "class", "enum", "case", "import", "return", "self", "Self", "static",
                  "subscript", "throws", "try", "where", "while", "repeat", "extension", "init", "deinit"}


def camel(snake):
    parts = [p for p in snake.split("_") if p]
    return parts[0] + "".join(p[:1].upper() + p[1:] for p in parts[1:])


def split_params(text):
    """`a: T, b: U = x` -> [(a, T, default?)], at bracket depth 0."""
    out, depth, cur = [], 0, ""
    for ch in text:
        if ch in "<[(":
            depth += 1
        elif ch in ">])":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur)
    params = []
    for item in out:
        item = item.strip()
        name, rest = item.split(":", 1)
        if " = " in rest:
            typ, default = rest.split(" = ", 1)
        else:
            typ, default = rest, None
        params.append((name.strip().strip("`"), typ.strip(), default.strip() if default else None))
    return params


def init_params(lines, start, indent):
    """The parameters of the `public init(` at `indent` after line `start` (None if none before `stop`)."""
    head = " " * indent + "public init("
    for i in range(start, len(lines)):
        line = lines[i]
        if line.startswith(head):
            if line.rstrip().endswith(") {"):
                return split_params(line[len(head):line.rstrip().rindex(") {")])
            body = []
            j = i + 1
            while not lines[j].strip().startswith(") {"):
                body.append(lines[j].strip())
                j += 1
            return split_params(" ".join(body))
    raise SystemExit(f"no init at indent {indent} after line {start}")


def component_structs(lines):
    """`Components.Schemas` structs: name -> their init's parameters."""
    out = {}
    for i, line in enumerate(lines):
        m = re.match(r"^        public struct (\w+): Codable", line)
        if m:
            try:
                out[m.group(1)] = init_params(lines, i, 12)
            except SystemExit:
                pass
    return out


def operations(types):
    """Each generated operation: its id, Swift name, inputs and success answer."""
    lines = types.split("\n")
    structs = component_structs(lines)
    starts = [i for i, l in enumerate(lines) if re.match(r"^    public enum (\w+) \{$", l)]
    ops = {}
    for n, start in enumerate(starts):
        end = starts[n + 1] if n + 1 < len(starts) else len(lines)
        block = lines[start:end]
        name = re.match(r"^    public enum (\w+) \{$", lines[start]).group(1)
        found = re.search(r'public static let id: Swift\.String = "([^"]+)"', "\n".join(block[:3]))
        if not found:
            continue
        op_id = found.group(1)
        op = {"name": name, "path": [], "query": [], "body": None, "bodyCases": [], "bodyOptional": False,
              "answer": None}
        out_at = next(i for i, l in enumerate(block) if l.startswith("        @frozen public enum Output:"))
        for i, l in enumerate(block[:out_at]):
            if l == "            public struct Path: Sendable, Hashable {":
                op["path"] = init_params(block, i, 16)
            elif l == "            public struct Query: Sendable, Hashable {":
                op["query"] = init_params(block, i, 16)
            elif l.startswith("            public var body: "):
                op["bodyOptional"] = l.endswith("?")
        body_at = next((i for i, l in enumerate(block[:out_at])
                        if l.startswith("            @frozen public enum Body:")), None)
        if body_at is not None:
            op["bodyCases"] = [m.group(1) for m in (re.match(r"^                case (\w+)\(", block[i])
                                                    for i in range(body_at, out_at)) if m]
            for i in range(body_at, out_at):
                m = re.match(r"^                case (\w+)\((.+)\)$", block[i])
                if m:
                    case, typ = m.groups()
                    payload_at = next((j for j in range(body_at, out_at)
                                       if block[j].startswith("                public struct JsonPayload:")), None)
                    if typ == f"Operations.{name}.Input.Body.JsonPayload" and payload_at is not None:
                        op["body"] = ("fields", case, init_params(block, payload_at, 20))
                    else:
                        component = re.match(r"^Components\.Schemas\.(\w+)$", typ)
                        if component and component.group(1) in structs:
                            # A body that is one shared object (`ConfirmOnly`): its fields, as an inline body's.
                            op["body"] = ("fields", case, structs[component.group(1)])
                        else:
                            op["body"] = ("whole", case, typ)
                    break
        ok_at = next((i for i in range(out_at, len(block))
                      if block[i] == "            public struct Ok: Sendable, Hashable {"), None)
        if ok_at is None:
            raise SystemExit(f"{op_id}: no 200 answer")
        op["answer"] = ("none", None)
        for i in range(ok_at, len(block)):
            if block[i] == "            }":
                break
            mm = re.match(r"^                    case (\w+)\((.+)\)$", block[i])
            if mm:
                op["answer"] = mm.groups()
                break
        ops[op_id] = op
    return ops


def method_name(op_id, prefix):
    name = op_id
    if prefix and name.startswith(prefix + "_") and len(name) > len(prefix) + 1:
        name = name[len(prefix) + 1:]
    name = camel(name)
    return f"`{name}`" if name in SWIFT_KEYWORDS else name


def doc(text):
    return [line.rstrip() for line in text.strip().split("\n")]


def nullable_fields(document, spec):
    """The JSON body's optional properties whose schema admits `null`."""
    schema = spec.get("requestBody", {}).get("content", {}).get("application/json", {}).get("schema", {})
    if "$ref" in schema:
        schema = document["components"]["schemas"][schema["$ref"].split("/")[-1]]
    required = set(schema.get("required", []))
    out = set()
    for name, prop in schema.get("properties", {}).items():
        kind = prop.get("type")
        if name not in required and (
            (isinstance(kind, list) and "null" in kind) or {"type": "null"} in prop.get("anyOf", [])
        ):
            out.add(name)
    return out


def taken_names(root):
    """The member names CalimeroClient already declares (in its own files), which a group may not take."""
    names = set()
    for file in ("Cloud/CalimeroClient.swift", "Cloud/CalimeroClient+Deprecated.swift", "Api/CalimeroApi.swift"):
        path = os.path.join(root, "Sources", "MeroKit", file)
        if not os.path.exists(path):
            continue
        with open(path, encoding="utf-8") as f:
            text = f.read()
        names.update(re.findall(r"^    (?:public |private |)(?:nonisolated )?(?:static )?(?:func|var|let) (\w+)", text, re.M))
    return names


def group_names(group, taken):
    prop = group if group not in taken and group not in SWIFT_KEYWORDS else group + "Api"
    return prop, group[:1].upper() + group[1:]


def facade(document, ops, taken):
    by_group = {}
    for path, items in document["paths"].items():
        for verb, spec in items.items():
            if not isinstance(spec, dict) or spec.get("operationId") not in ops:
                continue
            group = spec.get("x-calimero-group")
            if not group:
                raise SystemExit(f"{spec['operationId']}: no x-calimero-group")
            by_group.setdefault(group, []).append((verb.upper(), path, spec))
    out = []
    w = out.append
    w("// swift-format-ignore-file")
    w("// swiftlint:disable all")
    w("// Generated by scripts/generate-facade.py from openapi/openapi.json and Sources/MeroAPI/Generated/Types.swift.")
    w("// Do not edit: run scripts/generate-openapi.sh.")
    w("import Foundation")
    w("import MeroAPI")
    w("import OpenAPIRuntime")
    w("")
    for tag in sorted(by_group):
        prop, typ = group_names(tag, taken)
        prefix = PREFIXES.get(tag)
        w("extension CalimeroClient {")
        w(f"    /// The `{tag}` routes of the node API, one method per operation.")
        w(f"    public nonisolated var {prop}: {typ} {{ {typ}(client: self) }}")
        w("")
        w(f"    /// The `{tag}` routes of the node API (``CalimeroClient/{prop}``).")
        w(f"    public struct {typ}: Sendable {{")
        w("        let client: CalimeroClient")
        names = set()
        for verb, path, spec in sorted(by_group[tag], key=lambda x: x[2]["operationId"]):
            op_id = spec["operationId"]
            op = ops[op_id]
            name = method_name(op_id, prefix)
            if name in names:
                raise SystemExit(f"{op_id}: {typ}.{name} is taken")
            names.add(name)
            step_up = bool(spec.get("x-calimero-step-up"))
            operator = bool(spec.get("x-calimero-operator"))
            retry_safe = bool(spec.get("x-calimero-retry-safe"))
            retry_requires = spec.get("x-calimero-retry-requires")
            args, call = [], []
            labels = set()

            def arg(label, swift_type, default, where=""):
                """Add an argument; one whose name a path parameter took is `{where}{Name}`."""
                if label in labels:
                    if not where:
                        raise SystemExit(f"{op_id}: argument {label} appears twice")
                    label = where + label[:1].upper() + label[1:]
                    if label in labels:
                        raise SystemExit(f"{op_id}: argument {label} appears twice")
                labels.add(label)
                args.append(f"{label}: {swift_type}" + (f" = {default}" if default else ""))
                return label

            for p, t, d in op["path"]:
                arg(p, t, d)
            body_expr = None
            fields_args = []
            explicit_nulls = []
            content_type = False
            if op["body"]:
                kind, case, rest = op["body"]
                if kind == "fields":
                    nullable = nullable_fields(document, spec)
                    fields = []
                    for p, t, d in rest:
                        if p in STEP_UP_FIELDS:
                            continue
                        if p in nullable and t.endswith("?"):
                            # `T??`: `.some(nil)` sends an explicit `null` (the document gives it a meaning).
                            label = arg(p, t + "?", d, "body")
                            explicit_nulls.append((p, label))
                            fields.append((p, f"{label} ?? nil"))
                        else:
                            fields.append((p, arg(p, t, d, "body")))
                    fields_args = fields
                    if fields or not op["bodyOptional"]:
                        body_expr = f".{case}(.init({', '.join(f'{p}: {a}' for p, a in fields)}))"
                elif rest == "OpenAPIRuntime.HTTPBody":
                    arg("body", "Data", None)
                    body_expr = f".{case}(HTTPBody(body))"
                    if case == "any":
                        # `*/*` alone: the caller names the type (the generator would send `*/*`).
                        arg("contentType", "Swift.String", '"application/octet-stream"')
                        content_type = True
                    elif "any" in op["bodyCases"]:
                        # `*/*` beside a named type (`blob_upload`: `application/octet-stream`, `*/*`):
                        # the node stores the type the request names, so the caller may name it; nil
                        # sends the first listed type. The `*/*` case carries it (the transport sets
                        # the header), the listed case keeps the generator's own.
                        label = arg("contentType", "Swift.String?", "nil")
                        body_expr = f"{label} == nil ? .{case}(HTTPBody(body)) : .any(HTTPBody(body))"
                        content_type = True
                else:
                    arg("body", rest, None)
                    body_expr = f".{case}(body)"
            query = [(p, arg(p, t, d, "query")) for p, t, d in op["query"]]
            if step_up:
                arg("passkey", "(any PasskeyProvider)?", "nil")
            if operator:
                arg("operatorSigner", "(any OperatorSigner)?", "nil")
            parts = []
            if op["path"]:
                parts.append(f".init({', '.join(f'{p}: {p}' for p, _, _ in op['path'])})")
                parts[-1] = "path: " + parts[-1]
            if op["query"]:
                parts.append("query: .init(" + ", ".join(f"{p}: {a}" for p, a in query) + ")")
            if body_expr:
                parts.append("body: " + body_expr)
            input_expr = f"Operations.{op['name']}.Input({', '.join(parts)})"
            gen_method = camel(op_id)
            case, answer_type = op["answer"]
            if case == "none":
                returns, extract = "", "_ = try await api.{m}(input).ok"
            elif answer_type == "OpenAPIRuntime.HTTPBody" and case in ("textEventStream", "plainText"):
                returns, extract = " -> HTTPBody", f"try await api.{{m}}(input).ok.body.{case}"
            elif answer_type == "OpenAPIRuntime.HTTPBody":
                returns = " -> Data"
                extract = f"try await Data(collecting: api.{{m}}(input).ok.body.{case}, upTo: .max)"
            else:
                returns, extract = f" -> {answer_type}", f"try await api.{{m}}(input).ok.body.{case}"
            extract = extract.format(m=gen_method)
            w("")
            summary = spec.get("summary", op_id)
            w(f"        /// {summary}: `{verb} {path}` (`{op_id}`).")
            if spec.get("description"):
                w("        ///")
                for line in doc(spec["description"]):
                    w(f"        /// {line}" if line else "        ///")
            if returns:
                w("        @discardableResult")
            w(f"        public func {name}({', '.join(args)}) async throws{returns} {{")
            w(f"            let input = {input_expr}")
            if explicit_nulls:
                w("            var nulls: Set<String> = []")
                for key, label in explicit_nulls:
                    w(f"            if case .some(.none) = {label} {{ nulls.insert(\"{key}\") }}")
            overrides = []
            if step_up:
                overrides.append("passkey: passkey")
            if operator:
                overrides.append("operatorSigner: operatorSigner")
            if retry_safe and retry_requires:
                needed = next((a for p, a in fields_args if p == retry_requires), None)
                if needed is not None and needed.endswith(" ?? nil"):
                    needed = needed[: -len(" ?? nil")]
                if needed is None:
                    raise SystemExit(f"{op_id}: retry requires {retry_requires}, which is no argument")
                overrides.append(f"retry: {needed} != nil")
            elif retry_safe:
                overrides.append("retry: true")
            if explicit_nulls:
                overrides.append("nulls: nulls")
            if content_type:
                overrides.append("contentType: contentType")
            perform = f"client.perform({', '.join(overrides)})" if overrides else "client.perform"
            w(f"            return try await {perform} {{ api in")
            w(f"                {extract}")
            w("            }")
            w("        }")
        w("    }")
        w("}")
        w("")
    return "\n".join(out).rstrip() + "\n"


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as f:
        document = json.load(f)
    with open(sys.argv[2], encoding="utf-8") as f:
        ops = operations(f.read())
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    with open(sys.argv[3], "w", encoding="utf-8") as f:
        f.write(facade(document, ops, taken_names(root)))


if __name__ == "__main__":
    main()
