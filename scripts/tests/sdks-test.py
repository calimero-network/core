#!/usr/bin/env python3
"""Tests for scripts/sdks.py: scope, version check and sync, manifest checks.

    python3 scripts/tests/sdks-test.py
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
_spec = importlib.util.spec_from_file_location("sdks", ROOT / "scripts" / "sdks.py")
sdks = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(sdks)

FAILURES: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    if not ok:
        FAILURES.append(f"{name}: {detail}")


def fixture(root: Path) -> None:
    def sdk(kind_dir: str, name: str, manifest: dict, files: dict[str, str]) -> None:
        d = root / "sdks" / kind_dir / name
        d.mkdir(parents=True)
        (d / "sdk.json").write_text(json.dumps({"name": name, **manifest}))
        for rel, text in files.items():
            (d / rel).parent.mkdir(parents=True, exist_ok=True)
            (d / rel).write_text(text)

    (root / "VERSION").write_text("1.0.0-rc.30\n")
    sdk(
        "clients",
        "mero-js",
        {
            "kind": "client",
            "language": "typescript",
            "version_files": [{"path": "package.json", "format": "json", "key": "version"}],
            "inputs": ["docs/public/openapi.json"],
            "generated": ["src/generated"],
            "workflow": ".github/workflows/sdk-mero-js.yml",
        },
        {"package.json": '{\n  "name": "@calimero-network/mero-js",\n  "version": "36.0.0",\n  "dependencies": {"x": "1.0.0"}\n}\n'},
    )
    sdk(
        "components",
        "mero-react",
        {
            "kind": "component",
            "language": "typescript",
            "version_files": [{"path": "package.json", "format": "json", "key": "version"}],
            "depends_on": ["mero-js"],
            "workflow": ".github/workflows/sdk-mero-react.yml",
        },
        {"package.json": '{"name": "@calimero-network/mero-react", "version": "11.8.0"}\n'},
    )
    sdk(
        "clients",
        "mero-py",
        {
            "kind": "client",
            "language": "python",
            "version_files": [
                {"path": "pyproject.toml", "format": "toml", "key": "project.version"},
                {"path": "Cargo.toml", "format": "toml", "key": "package.version"},
            ],
            "inputs": ["docs/public/openapi.json"],
            "workflow": ".github/workflows/sdk-mero-py.yml",
        },
        {
            "pyproject.toml": '[build-system]\nrequires = ["maturin"]\n\n[project]\nname = "mero-py"\nversion = "0.9.0"\n',
            "Cargo.toml": '[package]\nname = "mero-py"\nversion = "0.9.0"\n\n[dependencies]\nserde = { version = "1" }\n',
        },
    )
    sdk(
        "clients",
        "mero-kotlin",
        {
            "kind": "client",
            "language": "kotlin",
            "version_files": [{"path": "gradle.properties", "format": "properties", "key": "VERSION_NAME"}],
            "workflow": ".github/workflows/sdk-mero-kotlin.yml",
        },
        {"gradle.properties": "GROUP=network.calimero\nVERSION_NAME=0.2.0\n"},
    )
    sdk(
        "clients",
        "mero-swift",
        {
            "kind": "client",
            "language": "swift",
            "version_files": [{"path": "README.md", "format": "regex", "pattern": r'calimero-network/swift-sdk", exact: "([^"]+)"'}],
            "workflow": ".github/workflows/sdk-mero-swift.yml",
        },
        {"README.md": '.package(url: "https://github.com/calimero-network/swift-sdk", exact: "0.5.0"),\n'},
    )


def test_scope(root: Path) -> None:
    loaded = sdks.load_sdks(root)
    names = [s["name"] for s in loaded]
    check("load", names == ["mero-js", "mero-kotlin", "mero-py", "mero-react", "mero-swift"], str(names))

    def verdict(*paths):
        v, _ = sdks.scope(list(paths), loaded)
        return sorted(n for n, runs in v.items() if runs)

    check("no input runs all", verdict() == names, str(verdict()))
    check("own dir", verdict("sdks/clients/mero-py/src/lib.rs") == ["mero-py"], str(verdict("sdks/clients/mero-py/src/lib.rs")))
    check(
        "a dependency runs its dependents",
        verdict("sdks/clients/mero-js/src/index.ts") == ["mero-js", "mero-react"],
        str(verdict("sdks/clients/mero-js/src/index.ts")),
    )
    check("react alone", verdict("sdks/components/mero-react/src/a.tsx") == ["mero-react"], "")
    check(
        "openapi reaches its readers and their dependents",
        verdict("docs/public/openapi.json") == ["mero-js", "mero-py", "mero-react"],
        str(verdict("docs/public/openapi.json")),
    )
    check("VERSION runs all", verdict("VERSION") == names, str(verdict("VERSION")))
    check("shared script runs all", verdict("scripts/sdks.py") == names, "")
    check("own workflow", verdict(".github/workflows/sdk-mero-kotlin.yml") == ["mero-kotlin"], "")
    check("core code runs none", verdict("crates/node/src/lib.rs") == [], str(verdict("crates/node/src/lib.rs")))
    check("own docs run it", verdict("sdks/clients/mero-swift/README.md") == ["mero-swift"], "")
    check("other docs run none", verdict("docs/src/content/docs/index.mdx", "README.md") == [], "")


def test_versions(root: Path) -> None:
    loaded = sdks.load_sdks(root)
    wrong = sdks.version_report(loaded, "1.0.0-rc.30", root)
    check("all differ before sync", len(wrong) == 6, "\n".join(wrong))
    for sdk in loaded:
        for spec in sdk["version_files"]:
            sdks.write_file_version(sdk, spec, "1.0.0-rc.30", root)
    check("none differ after sync", sdks.version_report(loaded, "1.0.0-rc.30", root) == [], "")
    pkg = (root / "sdks/clients/mero-js/package.json").read_text()
    check("json keeps other fields", '"x": "1.0.0"' in pkg and '"version": "1.0.0-rc.30"' in pkg, pkg)
    cargo = (root / "sdks/clients/mero-py/Cargo.toml").read_text()
    check("toml rewrites only [package]", 'serde = { version = "1" }' in cargo and 'version = "1.0.0-rc.30"' in cargo, cargo)
    readme = (root / "sdks/clients/mero-swift/README.md").read_text()
    check("regex", 'swift-sdk", exact: "1.0.0-rc.30"' in readme, readme)
    props = (root / "sdks/clients/mero-kotlin/gradle.properties").read_text()
    check("properties", "VERSION_NAME=1.0.0-rc.30" in props and "GROUP=network.calimero" in props, props)
    # Idempotent: a second sync changes nothing.
    again = [sdks.write_file_version(s, spec, "1.0.0-rc.30", root) for s in loaded for spec in s["version_files"]]
    check("sync is idempotent", not any(again), str(again))


def test_manifest_errors(root: Path) -> None:
    bad = root / "sdks" / "clients" / "calimero-go"
    bad.mkdir(parents=True)
    (bad / "sdk.json").write_text(json.dumps({"name": "calimero-go", "kind": "client", "language": "go", "version_files": [], "workflow": "x"}))
    try:
        sdks.load_sdks(root)
        check("rejects a non mero-<language> name", False, "loaded")
    except sdks.SdkError as e:
        check("rejects a non mero-<language> name", "mero-<language>" in str(e), str(e))
    bad.joinpath("sdk.json").unlink()
    bad.rmdir()

    wrong_kind = root / "sdks" / "components" / "mero-vue"
    wrong_kind.mkdir(parents=True)
    (wrong_kind / "sdk.json").write_text(json.dumps({"name": "mero-vue", "kind": "client", "language": "ts", "version_files": [], "workflow": "x"}))
    try:
        sdks.load_sdks(root)
        check("rejects a kind that is not its folder", False, "loaded")
    except sdks.SdkError as e:
        check("rejects a kind that is not its folder", "kind" in str(e), str(e))


def test_generate_container() -> None:
    """A Swift generator runs in the pinned image when swift is not installed (or forced)."""
    sdk = {"name": "mero-swift", "dir": "sdks/clients/mero-swift", "toolchain": "swift"}
    plain = {"name": "mero-js", "dir": "sdks/clients/mero-js"}
    real_which = sdks.shutil.which
    try:
        sdks.shutil.which = lambda tool: "/usr/bin/" + tool
        check("local swift runs bash", sdks.generate_argv(sdk, "gen")[0] == "bash", "")
        check("forced container uses docker", sdks.generate_argv(sdk, "gen", {"SDKS_GENERATE_IN_CONTAINER": "1"})[0] == "docker", "")
        sdks.shutil.which = lambda tool: "/usr/bin/docker" if tool == "docker" else None
        argv = sdks.generate_argv(sdk, "gen")
        check("missing swift uses docker", argv[0] == "docker" and sdks.SWIFT_IMAGE_TAG in argv, str(argv))
        check("container runs as the caller", "--user" in argv and f"{os.getuid()}:{os.getgid()}" in argv, str(argv))
        check("container keeps its own cache", any(a.startswith("MERO_OPENAPI_CACHE=") for a in argv), str(argv))
        check("no toolchain runs bash", sdks.generate_argv(plain, "gen")[0] == "bash", "")
        sdks.shutil.which = lambda tool: None
        try:
            sdks.generate_argv(sdk, "gen")
            check("no swift and no docker is an error", False, "returned")
        except sdks.SdkError as e:
            check("no swift and no docker is an error", "docker" in str(e), str(e))
    finally:
        sdks.shutil.which = real_which


def test_repository() -> None:
    """The real tree: manifests load, and every SDK names a workflow that exists."""
    loaded = sdks.load_sdks(ROOT)
    for sdk in loaded:
        check(f"{sdk['name']} workflow exists", (ROOT / sdk["workflow"]).is_file(), sdk["workflow"])
        for spec in sdk["version_files"]:
            check(f"{sdk['name']} {spec['path']} exists", (ROOT / sdk["dir"] / spec["path"]).is_file(), "")
    check("VERSION is core's release", sdks.read_version() == sdks.core_version(), f"{sdks.read_version()} vs {sdks.core_version()}")
    if loaded:
        version = sdks.read_version()
        wrong = sdks.version_report(loaded, version)
        check("every SDK states VERSION", not wrong, "\n".join(wrong))


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        fixture(root)
        test_scope(root)
        test_versions(root)
        test_manifest_errors(root)
    test_generate_container()
    test_repository()
    for failure in FAILURES:
        print(f"FAIL {failure}")
    if FAILURES:
        return 1
    print("sdks-test: all passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
