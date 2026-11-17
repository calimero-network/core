#!/usr/bin/env python3
"""The SDKs under sdks/: what a change reaches, one version for all, regeneration.

Every SDK is a directory `sdks/<kind>/<name>/` (kind: clients, components)
holding an `sdk.json` that says what it is and how CI treats it:

    {
      "name": "mero-js",                      # = the directory name
      "kind": "client",                       # client | component
      "language": "typescript",
      "published": [{"registry": "npm", "name": "@calimero-network/mero-js"}],
      "version_files": [                      # every place the SDK states its version
        {"path": "package.json", "format": "json", "key": "version"}
      ],
      "depends_on": [],                       # other SDKs it builds against
      "inputs": ["docs/public/openapi.json"], # files outside its directory it reads
      "generate": "pnpm run generate",        # regenerates `generated` from core (run in its dir)
      "generated": ["src/generated"],         # paths (in its dir) `generate` owns
      "workflow": ".github/workflows/sdk-mero-js.yml"
    }

`format` is one of:
    json        `key` is a dotted path into the JSON document
    toml        `key` is a dotted path; the value is rewritten in place
    properties  `key` is a `KEY=VALUE` line
    regex       `pattern` has one group around the version

Commands:

    sdks.py list                  name, kind and directory of every SDK
    sdks.py scope < changed-paths `<name>=true|false` per SDK and `any=`, for sdks.yml
    sdks.py version-check         VERSION is core's release and every version file states it
    sdks.py version-sync [VER]    write VER (default: core's release) into VERSION and every version file
    sdks.py generate [NAME...]    run each SDK's `generate` (all when no name)
    sdks.py generated [NAME...]   print the generated paths, repo-relative

VERSION is core's release version, the one `[workspace.metadata.workspaces]`
`version` in the root Cargo.toml states (the release cut, see RELEASING.md):
`version-check` fails while the two differ, and `version-sync` with no
argument copies Cargo's into VERSION and every SDK. A release cut bumps
Cargo's version and runs `scripts/sdks.py version-sync` in the same commit.

Scope fails closed, as ci-scope.py does: no input runs every SDK. A change
reaches an SDK when it touches the SDK's directory, an SDK it depends on (and
what that one reaches), one of its `inputs`, its workflow, VERSION, or the
shared SDK machinery (this script, sdks.yml, sdks/ root files). Markdown
outside an SDK's own directory changes no build and runs nothing.
"""

from __future__ import annotations

import fnmatch
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SDKS = ROOT / "sdks"
VERSION_FILE = ROOT / "VERSION"
KINDS = {"clients": "client", "components": "component"}

# Files every SDK job depends on: a change here runs them all.
SHARED = (
    "VERSION",
    "scripts/sdks.py",
    "scripts/tests/sdks-test.py",
    ".github/workflows/sdks.yml",
    ".github/workflows/sdk-regenerate.yml",
    "sdks/package.json",
    "sdks/pnpm-workspace.yaml",
    "sdks/pnpm-lock.yaml",
    "sdks/.npmrc",
)

# A pre-release or release version, as core tags them (1.0.0-rc.30, 1.0.0).
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$")


class SdkError(Exception):
    pass


def read_version() -> str:
    version = VERSION_FILE.read_text(encoding="utf-8").strip()
    if not VERSION_RE.match(version):
        raise SdkError(f"VERSION holds {version!r}, not a version like 1.0.0-rc.30")
    return version


def core_version(root: Path = ROOT) -> str:
    """Core's release: the root Cargo.toml's [workspace.metadata.workspaces] version."""
    import tomllib

    with (root / "Cargo.toml").open("rb") as fh:
        meta = tomllib.load(fh)
    try:
        return str(meta["workspace"]["metadata"]["workspaces"]["version"])
    except KeyError as e:
        raise SdkError("Cargo.toml has no [workspace.metadata.workspaces] version") from e


def load_sdks(root: Path = ROOT) -> list[dict]:
    """Every sdks/<kind>/<name>/sdk.json, checked, sorted by name."""
    found = []
    base = root / "sdks"
    if not base.is_dir():
        return found
    for kind_dir, kind in KINDS.items():
        for manifest in sorted((base / kind_dir).glob("*/sdk.json")):
            sdk = json.loads(manifest.read_text(encoding="utf-8"))
            directory = manifest.parent
            where = manifest.relative_to(root).as_posix()
            if sdk.get("name") != directory.name:
                raise SdkError(f"{where}: name {sdk.get('name')!r} is not its directory {directory.name!r}")
            if not re.fullmatch(r"mero-[a-z0-9]+(?:-[a-z0-9]+)*", directory.name):
                raise SdkError(f"{where}: an SDK is named mero-<language> (got {directory.name!r})")
            if sdk.get("kind") != kind:
                raise SdkError(f"{where}: kind {sdk.get('kind')!r}, but it sits under sdks/{kind_dir}/")
            for field in ("language", "version_files", "workflow"):
                if field not in sdk:
                    raise SdkError(f"{where}: missing {field!r}")
            sdk["dir"] = directory.relative_to(root).as_posix()
            sdk.setdefault("depends_on", [])
            sdk.setdefault("inputs", [])
            sdk.setdefault("generated", [])
            sdk.setdefault("published", [])
            found.append(sdk)
    names = {s["name"] for s in found}
    for sdk in found:
        for dep in sdk["depends_on"]:
            if dep not in names:
                raise SdkError(f"{sdk['dir']}/sdk.json: depends on unknown SDK {dep!r}")
    return sorted(found, key=lambda s: s["name"])


# ---------------------------------------------------------------- scope


def _matches(path: str, pattern: str) -> bool:
    # A directory or a file named exactly, or a glob (fnmatch's `*` crosses `/`).
    pattern = pattern.rstrip("/")
    return path == pattern or path.startswith(pattern + "/") or fnmatch.fnmatchcase(path, pattern)


def _closure(sdk: dict, by_name: dict, seen: set | None = None) -> list[dict]:
    """The SDK and everything it depends on, transitively."""
    seen = set() if seen is None else seen
    if sdk["name"] in seen:
        return []
    seen.add(sdk["name"])
    out = [sdk]
    for dep in sdk["depends_on"]:
        out += _closure(by_name[dep], by_name, seen)
    return out


def reaches(sdk: dict, path: str, by_name: dict) -> str | None:
    """Why `path` runs `sdk`'s job, or None."""
    for item in _closure(sdk, by_name):
        own = item is sdk
        if _matches(path, item["dir"]):
            return f"{item['name']}'s directory" if not own else "its directory"
        if path.endswith((".md", ".mdx")):
            continue  # documentation outside its own directory builds nothing
        for pattern in item["inputs"]:
            if _matches(path, pattern):
                return f"{pattern} (an input of {item['name']})"
        if path == item["workflow"]:
            return f"{item['name']}'s workflow"
    if path.endswith((".md", ".mdx")):
        return None
    for pattern in SHARED:
        if _matches(path, pattern):
            return f"{pattern} (shared by every SDK)"
    return None


def scope(paths: list[str], sdks: list[dict]) -> tuple[dict[str, bool], list[str]]:
    by_name = {s["name"]: s for s in sdks}
    verdict = {s["name"]: False for s in sdks}
    why: list[str] = []
    paths = [p.strip() for p in paths if p.strip()]
    if not paths:
        why.append("no changed paths: every SDK runs")
        return {name: True for name in verdict}, why
    for sdk in sdks:
        for path in paths:
            reason = reaches(sdk, path, by_name)
            if reason:
                verdict[sdk["name"]] = True
                why.append(f"{sdk['name']}: {path} ({reason})")
                break
    return verdict, why


# ---------------------------------------------------------------- versions


def _dotted_get(doc, key: str):
    for part in key.split("."):
        doc = doc[part]
    return doc


def _toml_section_and_key(key: str) -> tuple[str, str]:
    section, _, name = key.rpartition(".")
    return section, name


def read_file_version(sdk: dict, spec: dict, root: Path = ROOT) -> str:
    path = root / sdk["dir"] / spec["path"]
    text = path.read_text(encoding="utf-8")
    fmt = spec["format"]
    if fmt == "json":
        return str(_dotted_get(json.loads(text), spec["key"]))
    if fmt == "toml":
        import tomllib

        return str(_dotted_get(tomllib.loads(text), spec["key"]))
    if fmt == "properties":
        m = re.search(rf"^{re.escape(spec['key'])}\s*=\s*(\S+)\s*$", text, re.M)
        if not m:
            raise SdkError(f"{path}: no {spec['key']}= line")
        return m.group(1)
    if fmt == "regex":
        m = re.search(spec["pattern"], text, re.M)
        if not m:
            raise SdkError(f"{path}: pattern {spec['pattern']!r} matches nothing")
        return m.group(1)
    raise SdkError(f"{path}: unknown version format {fmt!r}")


def write_file_version(sdk: dict, spec: dict, version: str, root: Path = ROOT) -> bool:
    """Rewrite one version file in place, keeping its formatting. True when it changed."""
    path = root / sdk["dir"] / spec["path"]
    text = path.read_text(encoding="utf-8")
    fmt = spec["format"]
    if fmt == "json":
        # Rewrite the one string, not the document: keep key order and indentation.
        doc = json.loads(text)
        old = _dotted_get(doc, spec["key"])
        last = spec["key"].split(".")[-1]
        pattern = re.compile(rf'("{re.escape(last)}"\s*:\s*)"{re.escape(str(old))}"')
        new_text, count = pattern.subn(lambda m: f'{m.group(1)}"{version}"', text, count=1)
        if count != 1:
            raise SdkError(f"{path}: could not find \"{last}\": \"{old}\"")
    elif fmt == "toml":
        section, name = _toml_section_and_key(spec["key"])
        lines = text.splitlines(keepends=True)
        current = ""
        done = False
        for i, line in enumerate(lines):
            header = re.match(r"^\s*\[([^\[\]]+)\]\s*$", line)
            if header:
                current = header.group(1).strip()
                continue
            if current == section and re.match(rf"^\s*{re.escape(name)}\s*=", line):
                lines[i] = re.sub(r'=\s*"[^"]*"', f'= "{version}"', line, count=1)
                done = True
                break
        if not done:
            raise SdkError(f"{path}: no {name} = \"...\" under [{section}]")
        new_text = "".join(lines)
    elif fmt == "properties":
        new_text, count = re.subn(
            rf"^({re.escape(spec['key'])}\s*=\s*)\S+\s*$", lambda m: f"{m.group(1)}{version}", text, count=1, flags=re.M
        )
        if count != 1:
            raise SdkError(f"{path}: no {spec['key']}= line")
    elif fmt == "regex":
        m = re.search(spec["pattern"], text, re.M)
        if not m:
            raise SdkError(f"{path}: pattern {spec['pattern']!r} matches nothing")
        new_text = text[: m.start(1)] + version + text[m.end(1) :]
    else:
        raise SdkError(f"{path}: unknown version format {fmt!r}")
    if new_text != text:
        path.write_text(new_text, encoding="utf-8")
        return True
    return False


def version_report(sdks: list[dict], version: str, root: Path = ROOT) -> list[str]:
    """Every version file that does not state `version`."""
    wrong = []
    for sdk in sdks:
        for spec in sdk["version_files"]:
            got = read_file_version(sdk, spec, root)
            if got != version:
                wrong.append(f"{sdk['dir']}/{spec['path']}: {got} (VERSION is {version})")
    return wrong


# ---------------------------------------------------------------- generate


def run_generate(sdks: list[dict]) -> int:
    status = 0
    for sdk in sdks:
        command = sdk.get("generate")
        if not command:
            print(f"{sdk['name']}: generates nothing")
            continue
        print(f"::group::{sdk['name']}: {command}", flush=True)
        code = subprocess.run(["bash", "-euo", "pipefail", "-c", command], cwd=ROOT / sdk["dir"]).returncode
        print("::endgroup::", flush=True)
        if code:
            print(f"::error::{sdk['name']}: `{command}` failed ({code})")
            status = code
    return status


def pick(sdks: list[dict], names: list[str]) -> list[dict]:
    if not names:
        return sdks
    by_name = {s["name"]: s for s in sdks}
    unknown = [n for n in names if n not in by_name]
    if unknown:
        raise SdkError(f"no such SDK: {', '.join(unknown)} (have: {', '.join(by_name)})")
    return [by_name[n] for n in names]


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    command, rest = argv[0], argv[1:]
    try:
        sdks = load_sdks()
        if command == "list":
            for sdk in sdks:
                print(f"{sdk['name']}\t{sdk['kind']}\t{sdk['dir']}")
        elif command == "scope":
            verdict, why = scope(sys.stdin.read().splitlines(), sdks)
            for line in why:
                print(line, file=sys.stderr)
            for name, runs in verdict.items():
                print(f"{name}={'true' if runs else 'false'}")
            print(f"any={'true' if any(verdict.values()) else 'false'}")
            print("sdks=" + json.dumps([n for n, runs in verdict.items() if runs]))
        elif command == "version-check":
            version = read_version()
            core = core_version()
            wrong = version_report(sdks, version)
            if version != core:
                wrong.insert(0, f"VERSION: {version} (core's release, Cargo.toml's workspace version, is {core})")
            for line in wrong:
                print(f"::error::{line}")
            if wrong:
                print(f"{len(wrong)} version file(s) differ from VERSION; run scripts/sdks.py version-sync")
                return 1
            print(f"every SDK states {version} ({sum(len(s['version_files']) for s in sdks)} files)")
        elif command == "version-sync":
            version = rest[0] if rest else core_version()
            if not VERSION_RE.match(version):
                raise SdkError(f"{version!r} is not a version like 1.0.0-rc.30")
            if not VERSION_FILE.exists() or VERSION_FILE.read_text(encoding="utf-8").strip() != version:
                VERSION_FILE.write_text(version + "\n", encoding="utf-8")
                print("VERSION")
            changed = [
                f"{sdk['dir']}/{spec['path']}"
                for sdk in sdks
                for spec in sdk["version_files"]
                if write_file_version(sdk, spec, version)
            ]
            print("\n".join(changed) if changed else f"every SDK already states {version}")
        elif command == "generate":
            return run_generate(pick(sdks, rest))
        elif command == "generated":
            for sdk in pick(sdks, rest):
                for path in sdk["generated"]:
                    print(f"{sdk['dir']}/{path}")
        else:
            print(f"unknown command {command!r}\n\n{__doc__}", file=sys.stderr)
            return 2
    except SdkError as e:
        print(f"::error::{e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
