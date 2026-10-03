#!/usr/bin/env python3
"""path-dep-closure.py must name exactly the workspace packages cargo resolves.

Caches key on its output, so a directory it misses is a source change that can
reuse a stale binary, and that failure is silent. Cargo's own resolve, normal
and build edges only, is the reference.

    python3 scripts/tests/path-dep-closure-test.py
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SCRIPT = ROOT / "scripts" / "path-dep-closure.py"
# (package, its directory, whether its own dev-dependencies count): every binary a
# cache is keyed for, and every test suite scripts/ci-scope.py scopes a job on.
PACKAGES = [
    ("cargo-mero", "tools/cargo-mero", False),
    ("merod", "crates/merod", False),
    ("cargo-mero", "tools/cargo-mero", True),
    ("storage-cost", "tools/storage-cost", True),
]


def cargo_closure(meta, name, dev):
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    start = next(i for i, p in packages.items() if p["name"] == name and p["source"] is None)
    seen, stack = set(), [start]
    while stack:
        node = stack.pop()
        if node in seen:
            continue
        seen.add(node)
        kinds = (None, "build", "dev") if dev and node == start else (None, "build")
        for dep in nodes[node]["deps"]:
            if any(kind["kind"] in kinds for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return {
        Path(packages[i]["manifest_path"]).parent.relative_to(ROOT).as_posix()
        for i in seen
        if packages[i]["source"] is None
    }


def main():
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    )
    failed = False
    for name, directory, dev in PACKAGES:
        label = f"{name}{' --dev' if dev else ''}"
        ours = set(
            subprocess.run(
                [sys.executable, str(SCRIPT), *(["--dev"] if dev else []), directory],
                check=True, capture_output=True, text=True,
            ).stdout.split()
        )
        cargos = cargo_closure(meta, name, dev)
        if ours == cargos:
            print(f"ok   {label}: {len(ours)} workspace packages")
            continue
        failed = True
        print(f"FAIL {label}")
        for missing in sorted(cargos - ours):
            print(f"  cargo builds {missing}, which the closure misses")
        for extra in sorted(ours - cargos):
            print(f"  the closure lists {extra}, which cargo does not build")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
