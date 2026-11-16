#!/usr/bin/env python3
"""Generate mero-core's API facade from core's OpenAPI document and the generated client.

    python3 codegen/facade.py <openapi.json> <generated kotlin root>

Run by `./gradlew -p codegen generateMeroClient` after OpenAPI Generator, never by hand. It writes
`com/calimero/mero/api/facade/*.kt` under the generated root: one class per `x-calimero-group`
(`client.orgs`, `client.apps`, ...), one suspend method per operation, the request body's fields
flattened into named parameters, answering the generated success model. A body's `confirm` and
`operatorProof` are never parameters: the transport answers `412 CONFIRM_REQUIRED` and
`428 OPERATOR_SIGNATURE_REQUIRED` itself, with the client's passkey and operator signer, or the
`passkey` / `operator` a method that may need them takes. A body field whose `null` the spec gives
a meaning (`type: [T, "null"]`) is an `Explicit<T>?`: left out when null, sent (as `null` too)
when given.

It reads the generator's own output for the names it chose (methods, parameters, body models),
so the facade cannot drift from the client it calls; the spec gives the groups, the routes, the
summaries and which step-ups an operation answers. It also writes `Routes.kt`: which routes the
transport retries, which need an idempotency key, which travel in the clear.
"""

import json
import os
import re
import sys

# An operation's facade group is its `x-calimero-group` (`apps`, `orgs`, `auth`, ...): the property on
# CalimeroClient, and `<Group>Facade` the class. A method drops the group's own word from the front of
# its operation id (`org_get` in `orgs` is `get`), unless that leaves two methods one name.
def group_prefixes(group):
    return [group, group[:-1]] if group.endswith("s") else [group]

# Tag -> the generated API class and the MeroApi property that holds it.
APIS = {
    "Accounts": ("AccountsApi", "accounts"),
    "Apps": ("AppsApi", "apps"),
    "Consent": ("ConsentApi", "consent"),
    "Ops": ("OpsApi", "ops"),
    "Organizations": ("OrganizationsApi", "organizations"),
    "Packages": ("PackagesApi", "packages"),
    "Public": ("PublicApi", "publicApi"),
    "Sign-up and deployments": ("SignUpAndDeploymentsApi", "signUpAndDeployments"),
}

HANDLED_BY_TRANSPORT = {"confirm", "operatorProof"}
KEYWORDS = {"package", "as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in",
            "interface", "is", "null", "object", "return", "super", "this", "throw", "true", "try",
            "typealias", "typeof", "val", "var", "when", "while"}


def camel(op_id):
    parts = op_id.split("_")
    return parts[0] + "".join(p[:1].upper() + p[1:] for p in parts[1:])


def ident(name):
    bare = name.strip("`")
    return f"`{bare}`" if bare in KEYWORDS else bare


def split_params(text):
    out, depth, cur = [], 0, ""
    for ch in text:
        if ch in "<(":
            depth += 1
        elif ch in ">)":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    params = []
    for p in out:
        m = re.match(r"(`?\w+`?)\s*:\s*([^=]+?)\s*(=\s*(.+))?$", p)
        params.append({"name": m.group(1), "type": m.group(2).strip(), "default": m.group(4)})
    return params


ENUM = re.compile(r"^\s+enum class (\w+)\(", re.M)


SIG = re.compile(r"^    suspend fun (\w+)\((.*)\) : (.+?) = withContext", re.M)


def read_api(path):
    ops = {}
    text = open(path).read()
    cls = os.path.basename(path)[:-3]
    nested = set(ENUM.findall(text))
    for m in SIG.finditer(text):
        name = m.group(1)
        if name.endswith("WithHttpInfo"):
            continue
        params = split_params(m.group(2))
        for p in params:
            bare = p["type"].rstrip("?").strip()
            if bare in nested:  # a query enum the API class declares (PublicApi.PurposeAttestation)
                p["type"] = p["type"].replace(bare, f"com.calimero.mero.api.apis.{cls}.{bare}", 1)
        ops[name] = {"params": params, "returns": m.group(3).strip()}
    return ops


FIELD = re.compile(r"@SerialName\(value = \"([^\"]+)\"\)\s*\n\s*val (`?\w+`?): ([^\n]+?),?\s*$", re.M)


def read_model(models, name):
    path = os.path.join(models, f"{name}.kt")
    if not os.path.exists(path):
        return None
    text = open(path).read()
    if not re.search(r"^data class ", text, re.M):
        return None
    fields = []
    for m in FIELD.finditer(text):
        decl = m.group(3).strip()
        typ, _, default = decl.partition("=")
        fields.append({"wire": m.group(1), "name": m.group(2), "type": typ.strip(), "default": default.strip() or None})
    return fields


def kdoc(op, method, path, extra):
    lines = [op.get("summary", "").strip() or f"{method.upper()} {path}", ""]
    desc = (op.get("description") or "").strip()
    if desc:
        lines += desc.splitlines() + [""]
    lines.append(f"`{method.upper()} {path}` (`{op['operationId']}`).")
    lines += extra
    body = "\n".join(("     * " + l).rstrip() for l in lines).replace("*/", "*&#47;")
    return f"    /**\n{body}\n     */\n"


def shorten(gen, group):
    for prefix in group_prefixes(group):
        if gen.startswith(prefix) and len(gen) > len(prefix) and gen[len(prefix)].isupper():
            return gen[len(prefix)].lower() + gen[len(prefix) + 1:]
    return gen


def route_regex(path):
    return "^" + re.sub(r"\{[^}]+\}", "[^/]+", path.lstrip("/")) + "$"


def write_routes(out_dir, routes):
    """The transport's per-route facts from the spec: which a 503 may be retried on, which need an
    idempotency key for that, which travel in the clear."""
    def entries(pred):
        rows = sorted({(m.upper(), route_regex(p)) for p, m, op in routes if pred(op)})
        return "\n".join(f'            Route("{m}", Regex("{r}")),' for m, r in rows)

    def binary_entries():
        rows = set()
        for p, m, op in routes:
            for media, body in op.get("requestBody", {}).get("content", {}).items():
                if body.get("schema", {}).get("format") == "binary":
                    rows.add((m.upper(), route_regex(p), "application/octet-stream" if media == "*/*" else media))
        return "\n".join(f'            Route("{m}", Regex("{r}")) to "{t}",' for m, r, t in sorted(rows))

    text = f"""// Generated by codegen/facade.py from openapi/openapi.json. Do not edit: run
// `./gradlew -p codegen generateMeroClient`.
package com.calimero.mero.api.facade

/** One operation's method and path (relative to the node, no leading slash) as a pattern. */
internal class Route(
    val method: String,
    val path: Regex,
) {{
    fun matches(
        method: String,
        path: String,
    ): Boolean = this.method == method && this.path.matches(path)
}}

/** What the spec's `x-calimero-*` extensions tell the transport about each route. */
internal object Routes {{
    /** `x-calimero-retry-safe`: a 503 or a lost connection is retried, and the request fails over. */
    val retrySafe: List<Route> =
        listOf(
{entries(lambda op: op.get("x-calimero-retry-safe") is True)}
        )

    /** `x-calimero-retry-requires: idempotencyKey`: retried only with one, so the transport adds it. */
    val idempotencyKey: List<Route> =
        listOf(
{entries(lambda op: op.get("x-calimero-retry-requires") == "idempotencyKey")}
        )

    /**
     * The media type of each binary request body. OpenAPI Generator 7.26 writes no `Content-Type`
     * for one it does not know (`application/wasm`, the any-type wildcard), and its ApiClient then labels the bytes
     * `application/json`; the transport sends them as the spec says instead (the wildcard as octet-stream).
     */
    val binaryBody: Map<Route, String> =
        mapOf(
{binary_entries()}
        )

    /** `x-calimero-sealed: false`: answered in the clear (`/health`, `/attestation`). */
    val plain: List<Route> =
        listOf(
{entries(lambda op: op.get("x-calimero-sealed") is False)}
        )
}}
"""
    open(os.path.join(out_dir, "Routes.kt"), "w").write(text)


def nullable(schema):
    t = schema.get("type")
    branches = schema.get("anyOf", []) + schema.get("oneOf", [])
    return (isinstance(t, list) and "null" in t) or any(b.get("type") == "null" for b in branches)


def meaningful_nulls(spec, op):
    """The body fields whose `null` the spec gives a meaning (`type: [T, "null"]`): sent as `null`
    when the caller says so, left out when the caller says nothing."""
    schema = op.get("requestBody", {}).get("content", {}).get("application/json", {}).get("schema", {})
    if "$ref" in schema:
        schema = spec["components"]["schemas"][schema["$ref"].split("/")[-1]]
    return {k for k, v in schema.get("properties", {}).items() if nullable(v)}


def main():
    spec_path, root = sys.argv[1], sys.argv[2]
    spec = json.load(open(spec_path))
    base = os.path.join(root, "com", "calimero", "mero", "api")
    apis_dir, models = os.path.join(base, "apis"), os.path.join(base, "models")
    out_dir = os.path.join(base, "facade")
    os.makedirs(out_dir, exist_ok=True)
    for f in os.listdir(out_dir):
        os.remove(os.path.join(out_dir, f))

    generated = {tag: read_api(os.path.join(apis_dir, f"{cls}.kt")) for tag, (cls, _) in APIS.items()}
    by_group = {}
    routes = []
    for path, item in spec["paths"].items():
        for method, op in item.items():
            if method not in ("get", "post", "put", "delete", "patch"):
                continue
            if op["tags"][0] not in APIS:
                raise SystemExit(f"{op['operationId']}: tag {op['tags'][0]} has no generated API class in APIS")
            by_group.setdefault(op["x-calimero-group"], []).append((path, method, op))
            routes.append((path, method, op))
    write_routes(out_dir, routes)

    for group in sorted(by_group):
        prop, cls = group, group[:1].upper() + group[1:] + "Facade"
        ops = sorted(by_group[group], key=lambda t: t[2]["operationId"])
        names = {}
        for path, method, op in ops:
            gen = camel(op["operationId"])
            names.setdefault(shorten(gen, group), []).append(gen)
        methods = []
        for path, method, op in ops:
            gen = camel(op["operationId"])
            tag = op["tags"][0]
            api_cls, api_prop = APIS[tag]
            if gen not in generated[tag]:
                raise SystemExit(f"{op['operationId']}: no {gen} in {api_cls}")
            short = shorten(gen, group)
            if len(names[short]) > 1 or short in KEYWORDS:
                short = gen
            sig = generated[tag][gen]
            responses = op.get("responses", {})
            confirms = op.get("x-calimero-step-up") is True or "412" in responses
            proves = op.get("x-calimero-operator") is True or "428" in responses

            required, optional, call_args, taken = [], [], [], set()
            explicit = meaningful_nulls(spec, op)
            null_checks = []
            body_expr = None
            for p in sig["params"]:
                fields = read_model(models, p["type"].rstrip("?").strip())
                if fields is None:
                    # A body the spec maps to a hand-written type is named after it, not its package
                    # (`comCalimeroMeroConsentRequest` -> `consentRequest`).
                    local = p["name"].strip("`")
                    if local.startswith("comCalimeroMero"):
                        local = local[len("comCalimeroMero"):]
                        local = local[:1].lower() + local[1:]
                    (optional if p["default"] else required).append(f"{ident(local)}: {p['type']}" + (f" = {p['default']}" if p["default"] else ""))
                    taken.add(local)
                    call_args.append(f"{ident(p['name'])} = {ident(local)}")
                    continue
                model = p["type"].rstrip("?").strip()
                ctor = []
                for f in fields:
                    if f["wire"] in HANDLED_BY_TRANSPORT:
                        continue
                    pname = f["name"].strip("`")
                    if pname in taken:
                        pname = "body" + pname[0].upper() + pname[1:]
                    taken.add(pname)
                    if f["wire"] in explicit:
                        # Absent (null), a value, or `Explicit(null)`: the field sent as `null`.
                        inner = f["type"].rstrip("?").strip()
                        optional.append(f"{ident(pname)}: com.calimero.mero.Explicit<{inner}>? = null")
                        ctor.append(f"{ident(f['name'])} = {ident(pname)}?.value")
                        null_checks.append(f'"{f["wire"]}".takeIf {{ {ident(pname)} != null && {ident(pname)}.value == null }}')
                        continue
                    decl = f"{ident(pname)}: {f['type']}"
                    if f["default"]:
                        optional.append(f"{decl} = {f['default']}")
                    else:
                        required.append(decl)
                    ctor.append(f"{ident(f['name'])} = {ident(pname)}")
                if p["default"] and not ctor and not null_checks:
                    body_expr = None  # nothing but what the transport adds: send no body
                    continue
                body_expr = f"{model}({', '.join(ctor)})"
                call_args.append(f"{ident(p['name'])} = {body_expr}")
            extra = []
            if confirms:
                optional.append("passkey: PasskeyProvider? = null")
                extra.append("")
                extra.append("Answers `412 CONFIRM_REQUIRED` first: approved with [passkey], else the client's ([CalimeroClient.passkey]); with neither, [com.calimero.mero.ConfirmRequired] reaches the caller.")
            if proves:
                optional.append("operator: DeviceSigner? = null")
                extra.append("")
                extra.append("Answers `428 OPERATOR_SIGNATURE_REQUIRED` first: signed by [operator], else the client's ([CalimeroClient.operator]); with neither, [com.calimero.mero.OperatorSignatureRequired] reaches the caller.")
            extra.append("")
            extra.append("@throws com.calimero.mero.CalimeroError the node's refusal, typed by [com.calimero.mero.errorFrom].")
            params = required + optional
            nulls = f"setOfNotNull({', '.join(null_checks)})" if null_checks else "emptySet()"
            stepup = f"{'passkey' if confirms else 'null'}, {'operator' if proves else 'null'}, {nulls}"
            sig_text = ",\n".join(f"        {x}" for x in params)
            head = f"    suspend fun {ident(short)}(\n{sig_text},\n    )" if params else f"    suspend fun {ident(short)}()"
            call = f"api({stepup}).{api_prop}.{gen}({', '.join(call_args)})"
            methods.append(kdoc(op, method, path, extra) + f"{head}: {sig['returns']} =\n        {call}\n")

        text = f"""// Generated by codegen/facade.py from openapi/openapi.json and the OpenAPI Generator client
// beside it. Do not edit: run `./gradlew -p codegen generateMeroClient`.
@file:Suppress("LongParameterList", "TooManyFunctions", "MaxLineLength", "unused")

package com.calimero.mero.api.facade

import com.calimero.mero.MeroApi
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.api.models.*
import com.calimero.mero.crypto.DeviceSigner

/**
 * The `{group}` operations of core's node API, one suspend method each (`CalimeroClient.{prop}`).
 * Every request is sealed to the attested node the client uses; a refusal throws the
 * [com.calimero.mero.CalimeroError] it means.
 */
class {cls} internal constructor(
    private val api: (PasskeyProvider?, DeviceSigner?, Set<String>) -> MeroApi,
) {{
{chr(10).join(methods)}}}
"""
        open(os.path.join(out_dir, f"{cls}.kt"), "w").write(text)


if __name__ == "__main__":
    main()
