#!/usr/bin/env python3
"""Fail a change that breaks what earlier releases' clients and apps rely on without saying so.

Compares this tree with the base it merges into (`--base`, default the merge base with
origin/master) and fails on:

- **a route removed**: every `.route("PATH", get(..).post(..))` of `crates/node/src`, as
  `METHOD PATH`, peer routes (`/internal/*`, a job node's `/job/*`) included, since a node
  of the previous release still calls them during a rolling upgrade;
- **a JSON field removed**: every key the API module names (`crates/node/src/api.rs` and
  `api/`: `json!` objects, and the fields of its serde structs, renamed as serde renames
  them) that no longer appears anywhere in it;
- **the WIT changed without its version**: `crates/app-wit/wit/calimero-app.wit` changed
  while `package calimero:app@X.Y.Z` did not move; an additive change needs at least a
  patch bump, a removed or changed line a minor bump and a declared cutover;
- **an ABI version dropped**: `SUPPORTED_ABI_VERSIONS` (`crates/app-host/src/abi.rs`)
  losing a version, or not holding the SDK's `ABI_VERSION` (`crates/sdk-macros/src/logic.rs`).

A removal passes when `crates/node/api-deprecations.toml` allows it: a `[[deprecated]]`
entry whose `remove_after` release is this tree's version or earlier, or a `[[cutover]]`
entry naming it. Entries are read from both trees, so the change that removes a route
may drop its entry. The policy: docs/src/content/docs/operate/compatibility.mdx.

    python3 scripts/check-api-compat.py                  # against the merge base with origin/master
    python3 scripts/check-api-compat.py --base HEAD^1    # CI: the PR's merge commit against its base
    python3 scripts/check-api-compat.py --list           # what this tree exposes
"""

from __future__ import annotations

import argparse
import datetime
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROUTE_DIR = "crates/node/src"
API_FILES = ("crates/node/src/api.rs", "crates/node/src/api/")
WIT = "crates/app-wit/wit/calimero-app.wit"
ABI_HOST = "crates/app-host/src/abi.rs"
ABI_SDK = "crates/sdk-macros/src/logic.rs"
DEPRECATIONS = "crates/node/api-deprecations.toml"
WORKSPACE_MANIFEST = "Cargo.toml"
# Proposed: a deprecated route or field stays at least this many releases (docs: compatibility).
MIN_DEPRECATION_RELEASES = 3

METHODS = ("get", "post", "put", "delete", "patch")
METHOD_RE = re.compile(r"\b(" + "|".join(METHODS) + r")\s*\(")
STRING_RE = re.compile(r'"((?:[^"\\]|\\.)*)"')
KEY_RE = re.compile(r'"([A-Za-z_][A-Za-z0-9_]*)"\s*:(?!:)')
TEST_MOD_RE = re.compile(r"#\[cfg\(test\)\]\s*(?:pub(?:\([a-z]+\))?\s+)?mod\s+\w+\s*\{")
WIT_PACKAGE_RE = re.compile(r"^\s*package\s+calimero:app@(\d+)\.(\d+)\.(\d+)\s*;", re.MULTILINE)
SEMVER_RE = re.compile(r"(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?")


# ----------------------------------------------------------------------------- sources


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


class Tree:
    """Files of one side: a git revision, or the working tree when `rev` is None."""

    def __init__(self, rev: str | None):
        self.rev = rev

    def files(self, prefix: str) -> list[str]:
        if self.rev is None:
            base = ROOT / prefix
            if base.is_file():
                return [prefix]
            return sorted(str(p.relative_to(ROOT)) for p in base.rglob("*.rs")) if base.is_dir() else []
        out = git("ls-tree", "-r", "--name-only", self.rev, "--", prefix)
        return sorted(f for f in out.splitlines() if f.endswith((".rs", ".wit", ".toml")))

    def read(self, path: str) -> str | None:
        if self.rev is None:
            p = ROOT / path
            return p.read_text() if p.exists() else None
        try:
            return git("show", f"{self.rev}:{path}")
        except subprocess.CalledProcessError:
            return None


# ----------------------------------------------------------------------------- Rust text


def strip_comments(src: str) -> str:
    """Rust source with `//` and `/* */` comments blanked; string literals kept."""
    out, i, n = [], 0, len(src)
    while i < n:
        c = src[i]
        if c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            out.append(src[i : j + 1])
            i = j + 1
        elif src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
        elif src.startswith("/*", i):
            j = src.find("*/", i + 2)
            i = n if j < 0 else j + 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def balanced(src: str, open_at: int) -> int:
    """The index just past the bracket that closes the one at `open_at` (strings skipped)."""
    pairs = {"(": ")", "{": "}", "[": "]"}
    close = pairs[src[open_at]]
    depth, i = 0, open_at
    while i < len(src):
        c = src[i]
        if c == '"':
            i += 1
            while i < len(src) and src[i] != '"':
                i += 2 if src[i] == "\\" else 1
        elif c == src[open_at]:
            depth += 1
        elif c == close:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return len(src)


def without_tests(src: str) -> str:
    """Rust source with comments and `#[cfg(test)] mod x { .. }` blocks removed."""
    src = strip_comments(src)
    while m := TEST_MOD_RE.search(src):
        src = src[: m.start()] + src[balanced(src, m.end() - 1) :]
    return src


def routes_in(src: str) -> set[str]:
    """`METHOD PATH` of every `.route("PATH", ..)` whose path is a literal."""
    src = without_tests(src)
    found = set()
    for m in re.finditer(r"\.route\s*\(", src):
        body = src[m.end() : balanced(src, m.end() - 1) - 1]
        path = re.match(r'\s*"([^"]+)"\s*,', body)
        if not path:
            continue
        for method in METHOD_RE.findall(body[path.end() :]):
            found.add(f"{method.upper()} {path[1]}")
    return found


def camel(name: str) -> str:
    head, *rest = name.split("_")
    return head + "".join(p[:1].upper() + p[1:] for p in rest)


def serde_fields(src: str) -> set[str]:
    """The JSON names of the fields of every serde struct in `src`."""
    found = set()
    for m in re.finditer(r"#\[derive\(([^)]*)\)\]", src):
        if "Serialize" not in m[1] and "Deserialize" not in m[1]:
            continue
        item = re.compile(r"((?:\s*#\[[^\]]*\])*)\s*(?:pub(?:\([a-z]+\))?\s+)?struct\s+\w+(?:<[^>]*>)?\s*\{")
        s = item.match(src, m.end())
        if not s:
            continue
        rename_all = re.search(r'rename_all\s*=\s*"([^"]+)"', s[1] or "")
        body = src[s.end() : balanced(src, s.end() - 1) - 1]
        for f in re.finditer(r"((?:\s*#\[[^\]]*\])*)\s*(?:pub(?:\([a-z]+\))?\s+)?(r#)?([a-z_][a-z0-9_]*)\s*:(?!:)", body):
            attrs = f[1] or ""
            if re.search(r"serde\([^)]*\b(skip|flatten)\b", attrs):
                continue
            renamed = re.search(r'serde\([^)]*\brename\s*=\s*"([^"]+)"', attrs)
            if renamed:
                found.add(renamed[1])
            elif rename_all and rename_all[1] == "camelCase":
                found.add(camel(f[3]))
            else:
                found.add(f[3])
    return found


def json_keys(src: str) -> set[str]:
    """Every object key of every `json!(..)` in `src`."""
    found = set()
    for m in re.finditer(r"\bjson!\s*\(", src):
        found |= set(KEY_RE.findall(src[m.end() : balanced(src, m.end() - 1)]))
    return found


# ----------------------------------------------------------------------------- surfaces


def routes(tree: Tree) -> set[str]:
    out: set[str] = set()
    for f in tree.files(ROUTE_DIR):
        if f.endswith(".rs"):
            out |= routes_in(tree.read(f) or "")
    return out


def fields(tree: Tree) -> set[str]:
    out: set[str] = set()
    for prefix in API_FILES:
        for f in tree.files(prefix):
            if f.endswith(".rs"):
                src = without_tests(tree.read(f) or "")
                out |= json_keys(src) | serde_fields(src)
    return out


def wit_version(text: str) -> tuple[int, int, int] | None:
    m = WIT_PACKAGE_RE.search(text)
    return (int(m[1]), int(m[2]), int(m[3])) if m else None


def wit_lines(text: str) -> list[str]:
    """The WIT's meaningful lines: comments, blank lines and the package line left out."""
    lines = []
    for line in text.splitlines():
        line = line.split("//", 1)[0].strip()
        if line and not WIT_PACKAGE_RE.match(line):
            lines.append(" ".join(line.split()))
    return lines


def abi_versions(tree: Tree) -> tuple[set[int], int | None]:
    host = tree.read(ABI_HOST) or ""
    sdk = tree.read(ABI_SDK) or ""
    m = re.search(r"SUPPORTED_ABI_VERSIONS\s*:\s*&\[u64\]\s*=\s*&\[([^\]]*)\]", host)
    supported = {int(v) for v in re.findall(r"\d+", m[1])} if m else set()
    v = re.search(r"const\s+ABI_VERSION\s*:\s*u64\s*=\s*(\d+)", sdk)
    return supported, int(v[1]) if v else None


# ----------------------------------------------------------------------------- policy


def version_key(v: str) -> tuple | None:
    m = SEMVER_RE.fullmatch(v.strip().removeprefix("v"))
    if not m:
        return None
    pre = m[4]
    tail = (1,) if pre is None else (0, tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split(".")))
    return (int(m[1]), int(m[2]), int(m[3]), tail)


def releases_between(since: str, until: str) -> int | None:
    """How many releases `until` is after `since`: rc numbers within one version, patch or
    minor numbers across final versions; None when the two cannot be compared that way."""
    a, b = SEMVER_RE.fullmatch(since), SEMVER_RE.fullmatch(until)
    if not a or not b:
        return None
    rc = lambda m: int(m[4].split(".")[1]) if m[4] and re.fullmatch(r"rc\.\d+", m[4]) else None  # noqa: E731
    if a.group(1, 2, 3) == b.group(1, 2, 3) and rc(a) is not None and rc(b) is not None:
        return rc(b) - rc(a)
    if a[4] is None and b[4] is None:
        if a[1] == b[1] and a[2] == b[2]:
            return int(b[3]) - int(a[3])
        if a[1] == b[1]:
            return int(b[2]) - int(a[2])
        return MIN_DEPRECATION_RELEASES if int(b[1]) > int(a[1]) else None
    return None


def load_entries(*texts: str | None) -> tuple[list[dict], list[dict]]:
    deprecated: list[dict] = []
    cutovers: list[dict] = []
    for text in texts:
        if not text:
            continue
        doc = tomllib.loads(text)
        deprecated += [d for d in doc.get("deprecated", []) if d not in deprecated]
        cutovers += [c for c in doc.get("cutover", []) if c not in cutovers]
    return deprecated, cutovers


def check_entries(text: str | None) -> list[str]:
    """What is wrong with this tree's deprecations file."""
    if not text:
        return []
    try:
        doc = tomllib.loads(text)
    except tomllib.TOMLDecodeError as e:
        return [f"{DEPRECATIONS}: {e}"]
    problems = []
    unknown = set(doc) - {"deprecated", "cutover"}
    if unknown:
        problems.append(f"{DEPRECATIONS}: unknown tables {sorted(unknown)}")
    for d in doc.get("deprecated", []):
        what = d.get("route") or d.get("field")
        name = f"{DEPRECATIONS}: deprecated {what!r}"
        if bool(d.get("route")) == bool(d.get("field")):
            problems.append(f"{name}: name exactly one of `route` or `field`")
        if d.get("route") and not re.fullmatch(r"(GET|POST|PUT|DELETE|PATCH) /\S*", d["route"]):
            problems.append(f"{name}: `route` is `METHOD /path`, as the router names it")
        for key in ("since", "remove_after"):
            if version_key(str(d.get(key, ""))) is None:
                problems.append(f"{name}: `{key}` must be a release version")
        if not isinstance(d.get("date"), datetime.date):
            problems.append(f"{name}: `date` must be a TOML date (the day the deprecation merged)")
        if not d.get("replacement"):
            problems.append(f"{name}: say what replaces it (`replacement`), or \"nothing\"")
        gap = releases_between(str(d.get("since", "")), str(d.get("remove_after", "")))
        if gap is not None and gap < MIN_DEPRECATION_RELEASES:
            problems.append(
                f"{name}: `remove_after` is {gap} release(s) after `since`; the policy keeps a "
                f"deprecated route or field at least {MIN_DEPRECATION_RELEASES}"
            )
        extra = set(d) - {"route", "field", "since", "date", "remove_after", "replacement"}
        if extra:
            problems.append(f"{name}: unknown keys {sorted(extra)}")
    for c in doc.get("cutover", []):
        name = f"{DEPRECATIONS}: cutover {c}"
        if not any(c.get(k) for k in ("route", "field", "wit", "abi")):
            problems.append(f"{name}: name the `route`, `field`, `wit` version or `abi` version it removes")
        if not isinstance(c.get("pr"), int) or not c.get("why"):
            problems.append(f"{name}: needs `pr` (the PR number) and `why`")
        extra = set(c) - {"route", "field", "wit", "abi", "pr", "why"}
        if extra:
            problems.append(f"{name}: unknown keys {sorted(extra)}")
    return problems


def allowed(kind: str, name: object, current: str, deprecated: list[dict], cutovers: list[dict]) -> bool:
    if any(c.get(kind) == name for c in cutovers):
        return True
    now = version_key(current)
    return any(
        d.get(kind) == name and now is not None and (v := version_key(str(d.get("remove_after", "")))) is not None and now >= v
        for d in deprecated
    )


def workspace_version(tree: Tree) -> str:
    doc = tomllib.loads(tree.read(WORKSPACE_MANIFEST) or "")
    return str(doc.get("workspace", {}).get("metadata", {}).get("workspaces", {}).get("version", ""))


def compare(base: Tree, head: Tree) -> list[str]:
    head_entries = head.read(DEPRECATIONS)
    problems = check_entries(head_entries)
    deprecated, cutovers = load_entries(base.read(DEPRECATIONS), head_entries)
    current = workspace_version(head)
    how = (
        f"deprecate it first (a `[[deprecated]]` entry in {DEPRECATIONS}, removable "
        f"{MIN_DEPRECATION_RELEASES} releases later) or declare a hard cutover (`[[cutover]]`, "
        "CHANGELOG, mdma CUTOVERS); docs: operate/compatibility"
    )

    for r in sorted(routes(base) - routes(head)):
        if not allowed("route", r, current, deprecated, cutovers):
            problems.append(f"route removed: {r}: {how}")
    for f in sorted(fields(base) - fields(head)):
        if not allowed("field", f, current, deprecated, cutovers):
            problems.append(f"JSON field no longer in the API: {f!r}: {how}")

    old_wit, new_wit = base.read(WIT), head.read(WIT)
    if old_wit and new_wit:
        ov, nv = wit_version(old_wit), wit_version(new_wit)
        old_lines, new_lines = wit_lines(old_wit), wit_lines(new_wit)
        if ov and nv and old_lines != new_lines:
            removed = [line for line in old_lines if line not in new_lines]
            label = f"calimero:app@{'.'.join(map(str, nv))}"
            if nv <= ov:
                problems.append(f"{WIT} changed but its version did not move ({label}): an additive change is a patch bump")
            elif removed and nv[:2] == ov[:2]:
                problems.append(
                    f"{WIT}: {len(removed)} line(s) removed or changed (first: {removed[0]!r}) in a patch bump "
                    f"({label}); a patch is additive only, so components built on the earlier patch still link"
                )
            if removed and nv[:2] != ov[:2] and not allowed("wit", label.split("@")[1], current, [], cutovers):
                problems.append(
                    f"{WIT}: {label} drops what components built on {'.'.join(map(str, ov))} import; "
                    f"declare it as a `[[cutover]] wit = \"{label.split('@')[1]}\"` with the CHANGELOG entry"
                )
    old_supported, _ = abi_versions(base)
    supported, sdk_abi = abi_versions(head)
    for v in sorted(old_supported - supported):
        if not allowed("abi", v, current, [], cutovers):
            problems.append(f"{ABI_HOST}: SUPPORTED_ABI_VERSIONS drops {v}, which apps built so far declare: {how}")
    if sdk_abi is not None and supported and sdk_abi not in supported:
        problems.append(f"{ABI_SDK}: ABI_VERSION {sdk_abi} is not in {ABI_HOST}'s SUPPORTED_ABI_VERSIONS {sorted(supported)}")
    return problems


def default_base() -> str:
    try:
        return git("merge-base", "origin/master", "HEAD").strip()
    except subprocess.CalledProcessError:
        return "HEAD^1"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the revision this tree merges into (default: merge base with origin/master)")
    parser.add_argument("--list", action="store_true", help="print this tree's routes, fields, WIT and ABI versions")
    args = parser.parse_args()
    head = Tree(None)
    if args.list:
        for r in sorted(routes(head)):
            print(f"route {r}")
        for f in sorted(fields(head)):
            print(f"field {f}")
        print(f"wit calimero:app@{'.'.join(map(str, wit_version(head.read(WIT) or '') or ()))}")
        supported, sdk_abi = abi_versions(head)
        print(f"abi supported {sorted(supported)} sdk {sdk_abi}")
        return 0
    base = Tree(args.base or default_base())
    problems = compare(base, head)
    for p in problems:
        print(f"::error::{p}" if "GITHUB_ACTIONS" in os.environ else p, file=sys.stderr)
    if not problems:
        print(f"no route, field, WIT or ABI version removed against {base.rev}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
