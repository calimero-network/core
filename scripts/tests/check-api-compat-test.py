#!/usr/bin/env python3
"""Tests for check-api-compat.py: what it reads out of Rust and WIT, and what it lets through.

Both wrong answers matter: a removal it misses ships a break to every earlier client, and a
removal it invents blocks a change that broke nothing. Each case pins one rule.

    python3 scripts/tests/check-api-compat-test.py
"""

import importlib.util
import sys
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-api-compat.py"
sys.dont_write_bytecode = True  # imports a sibling script; leave no __pycache__ in the tree
spec = importlib.util.spec_from_file_location("check_api_compat", SCRIPT)
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)

ROUTER = """
pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/auth/keys/{key}", post(label).delete(revoke))
        .route(
            "/apps/{app}/upgrade",
            post(upgrade), // a comment: get(nothing)
        )
        .route(path, get(dynamic))
        .route("/metrics", axum::routing::get(plain))
}

#[cfg(test)]
mod tests {
    fn r() { Router::new().route("/test-only", get(x)); }
}
"""

API = """
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub app_id: String,
    #[serde(rename = "orgID")]
    pub org: String,
    #[serde(skip)]
    pub internal: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_write_ms: Option<u64>,
}

#[derive(Deserialize)]
struct Body { raw_name: String }

fn h() -> Value {
    json!({ "status": "ok", "nested": { "innerKey": format!("a: {}", 1) }, "list": [ { "item": 1 } ] })
}
"""

WIT_A = """package calimero:app@0.5.1;

interface state {
  get: func(key: list<u8>) -> option<list<u8>>; // a comment
}
"""


class Fake(c.Tree):
    """A tree of files held in a dict."""

    def __init__(self, files):
        super().__init__("fake")
        self.content = files

    def files(self, prefix):
        return sorted(f for f in self.content if f == prefix or f.startswith(prefix.rstrip("/") + "/"))

    def read(self, path):
        return self.content.get(path)


def tree(router=ROUTER, api=API, wit=WIT_A, abi=(1,), sdk_abi=1, deprecations="", version="1.0.0-rc.16"):
    return Fake(
        {
            "crates/node/src/api.rs": router + api,
            c.WIT: wit,
            c.ABI_HOST: f"pub const SUPPORTED_ABI_VERSIONS: &[u64] = &[{', '.join(map(str, abi))}];",
            c.ABI_SDK: f"const ABI_VERSION: u64 = {sdk_abi};",
            c.DEPRECATIONS: deprecations,
            c.WORKSPACE_MANIFEST: f'[workspace.metadata.workspaces]\nversion = "{version}"\n',
        }
    )


def removed(text):
    return ROUTER.replace(text, "")


FAILURES = []
COUNT = [0]


def case(name, got, want):
    COUNT[0] += 1
    if got != want:
        FAILURES.append(f"{name}:\n  got  {got!r}\n  want {want!r}")


case(
    "routes: one per method, multi-line and full paths, no test modules, no dynamic paths",
    c.routes_in(ROUTER),
    {"GET /health", "POST /auth/keys/{key}", "DELETE /auth/keys/{key}", "POST /apps/{app}/upgrade", "GET /metrics"},
)
case(
    "fields: json! keys at any depth, serde names as renamed, skipped fields left out",
    c.json_keys(API) | c.serde_fields(API),
    {"status", "nested", "innerKey", "list", "item", "appId", "orgID", "lastWriteMs", "raw_name"},
)
case("an unchanged tree passes", c.compare(tree(), tree()), [])
case("adding a route or field passes", c.compare(tree(), tree(api=API + 'fn x() { json!({ "added": 1 }); }')), [])
case(
    "a removed route fails",
    [p.split(":")[0] for p in c.compare(tree(), tree(router=removed('.route("/health", get(health))')))],
    ["route removed"],
)
case(
    "a removed method of a kept path fails",
    len(c.compare(tree(), tree(router=ROUTER.replace(".delete(revoke)", "")))),
    1,
)
case(
    "a field gone from the whole module fails",
    [p.split(":")[0] for p in c.compare(tree(), tree(api=API.replace('"innerKey"', '"inner"')))],
    ["JSON field no longer in the API"],
)
DEPRECATED = """
[[deprecated]]
route = "GET /health"
since = "1.0.0-rc.13"
date = 2026-10-01
remove_after = "1.0.0-rc.16"
replacement = "GET /healthz"
"""
case(
    "a deprecated route goes once its remove_after release is reached",
    c.compare(tree(deprecations=DEPRECATED), tree(router=removed('.route("/health", get(health))'))),
    [],
)
case(
    "not before",
    len(c.compare(tree(deprecations=DEPRECATED), tree(router=removed('.route("/health", get(health))'), version="1.0.0-rc.15"))),
    1,
)
case(
    "a deprecation kept for fewer releases than the policy is refused",
    any("at least 3" in p for p in c.check_entries(DEPRECATED.replace("rc.13", "rc.14"))),
    True,
)
case(
    "a deprecation needs a date, a replacement and one name",
    len(c.check_entries('[[deprecated]]\nroute = "GET /x"\nfield = "y"\nsince = "1.0.0-rc.1"\nremove_after = "1.0.0-rc.9"\n')),
    3,
)
CUTOVER = '[[cutover]]\nfield = "innerKey"\npr = 4999\nwhy = "renamed; mero-js follows in the same release"\n'
case(
    "a declared cutover lets a removal through at once",
    c.compare(tree(), tree(api=API.replace('"innerKey"', '"inner"'), deprecations=CUTOVER)),
    [],
)
case(
    "a cutover needs its PR and why",
    len(c.check_entries('[[cutover]]\nroute = "GET /x"\n')),
    1,
)
WIT_ADDED = WIT_A.replace("}\n", "  set: func(key: list<u8>, value: list<u8>);\n}\n")
case(
    "the WIT changed without its version fails",
    [p.split(" changed")[0] for p in c.compare(tree(), tree(wit=WIT_ADDED))],
    [c.WIT],
)
case("an additive WIT change with a patch bump passes", c.compare(tree(), tree(wit=WIT_ADDED.replace("0.5.1", "0.5.2"))), [])
case("a WIT comment or blank line needs no bump", c.compare(tree(), tree(wit=WIT_A.replace("// a comment", "// reworded") + "\n")), [])
WIT_REMOVED = "package calimero:app@0.5.2;\n\ninterface state {\n}\n"
case(
    "a removal in a patch bump fails",
    any("patch is additive only" in p for p in c.compare(tree(), tree(wit=WIT_REMOVED))),
    True,
)
case(
    "a removal with a minor bump still needs a declared cutover",
    any("cutover" in p for p in c.compare(tree(), tree(wit=WIT_REMOVED.replace("0.5.2", "0.6.0")))),
    True,
)
case(
    "with the cutover it passes",
    c.compare(
        tree(),
        tree(wit=WIT_REMOVED.replace("0.5.2", "0.6.0"), deprecations='[[cutover]]\nwit = "0.6.0"\npr = 1\nwhy = "x"\n'),
    ),
    [],
)
case("an ABI version dropped fails", len(c.compare(tree(abi=(1, 2)), tree(abi=(2,), sdk_abi=2))), 1)
case("a new ABI version kept beside the old passes", c.compare(tree(), tree(abi=(1, 2), sdk_abi=2)), [])
case("the SDK's ABI must be one the host supports", len(c.compare(tree(), tree(sdk_abi=2))), 1)
case("rc releases count by rc number", c.releases_between("1.0.0-rc.15", "1.0.0-rc.18"), 3)
case("final releases count by minor", c.releases_between("1.0.0", "1.3.0"), 3)
case("an rc sorts before its release", c.version_key("1.0.0-rc.9") < c.version_key("1.0.0-rc.10") < c.version_key("1.0.0"), True)

# The real tree: something to compare, and nothing a test module declares.
real = c.Tree(None)
case("this tree has routes", len(c.routes(real)) > 100, True)
case("this tree's own deprecations file is well formed", c.check_entries(real.read(c.DEPRECATIONS)), [])

if FAILURES:
    print("\n".join(FAILURES))
    sys.exit(1)
print(f"check-api-compat: {COUNT[0]} cases pass")
