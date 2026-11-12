#!/usr/bin/env python3
"""Pack a `calimero:app` component into a signed single-service `.mpk`.

    python3 -I scripts/pack-component.py --wasm kv.wasm --package com.calimero.kv-store \
        --app-version 1.0.0 --out dist/com.calimero.kv-store-1.0.0.mpk

Builds the manifest the node reads (crates/bundle: version, package, appVersion,
minRuntimeVersion and the `app.wasm` artifact), signs it with `mero-sign sign
--dev` (the development key, which a public registry refuses; pass `--key` for
another), and writes manifest.json and app.wasm as a tar.gz. Build the component
first with `mero-app-build <crate-dir> --out <file>`. An app with compute methods also has a compute component
(`--compute-out <file>` of the same build); `--compute <file>` packs it as
compute.wasm, the only component a job node is ever given. `--compute` without `--wasm`
packs a compute-only package, which job nodes run and nothing calls. `--pipeline <file.yaml>`
(repeatable) ships a pipeline over the compute methods as pipelines/<name>.yaml; the node checks it at install.

`--mero-sign` (default MERO_SIGN, else `cargo run -q -p mero-sign --`) names the
signer. `--previous <old.mpk>` has it compare the component's ABI with the last
release's first (`mero-sign sign --previous`): it warns about changes that break
the app's callers and refuses a state change made without a state version bump.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import shlex
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--wasm", help="the app component to pack (omit for a compute-only package)")
    ap.add_argument("--compute", help="the compute component of the same build, packed as compute.wasm")
    ap.add_argument("--pipeline", action="append", default=[],
                    help="a pipeline YAML file to ship (repeatable)")
    ap.add_argument("--package", required=True, help="the package id, e.g. com.calimero.kv-store")
    ap.add_argument("--app-version", required=True, help="the release's version, e.g. 1.0.0")
    ap.add_argument("--min-runtime-version", default="0.1.0")
    ap.add_argument("--out", required=True, help="the .mpk to write")
    ap.add_argument("--key", help="sign with this key file instead of the development key")
    ap.add_argument("--previous", help="the last release (.mpk, .wasm or ABI JSON) to check the ABI against")
    ap.add_argument("--mero-sign", default=os.environ.get("MERO_SIGN"),
                    help="the mero-sign binary (default: cargo run -q -p mero-sign --)")
    args = ap.parse_args(argv)

    if not args.wasm and not args.compute:
        sys.exit("give --wasm, --compute or both")
    parts = {}
    if args.wasm:
        parts["wasm"] = ("app.wasm", Path(args.wasm).read_bytes())
    if args.compute:
        parts["compute"] = ("compute.wasm", Path(args.compute).read_bytes())
    for key, (_, data) in parts.items():
        if data[:8] != b"\0asm\x0d\0\x01\0":
            sys.exit(f"{getattr(args, key)}: not a WASM component (an SDK 0.12 core module is refused by the node)")
    pipelines = [(f"pipelines/{Path(f).name}", Path(f).read_bytes()) for f in args.pipeline]
    if len({name for name, _ in pipelines}) != len(pipelines):
        sys.exit("two --pipeline files have the same name")
    manifest = {
        "version": "1.0",
        "package": args.package,
        "appVersion": args.app_version,
        "minRuntimeVersion": args.min_runtime_version,
        **{key: {"path": name, "hash": hashlib.sha256(data).hexdigest(), "size": len(data)}
           for key, (name, data) in parts.items()},
        **({"pipelines": [{"path": name, "hash": hashlib.sha256(data).hexdigest(), "size": len(data)}
                          for name, data in pipelines]} if pipelines else {}),
    }
    signer = shlex.split(args.mero_sign) if args.mero_sign else ["cargo", "run", "-q", "-p", "mero-sign", "--"]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "manifest.json"
        path.write_text(json.dumps(manifest))
        # Beside the manifest, where `--previous` reads the new ABI from.
        for name, data in parts.values():
            (Path(tmp) / name).write_bytes(data)
        how = ["--key", args.key] if args.key else ["--dev"]
        if args.previous:
            how += ["--previous", str(Path(args.previous).resolve())]
        subprocess.run(signer + ["sign", str(path)] + how, check=True, cwd=ROOT, stdout=subprocess.DEVNULL)
        signed = path.read_bytes()

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(out, "w:gz") as tar:
        for name, data in [("manifest.json", signed), *parts.values(), *pipelines]:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    print(f"wrote {out} ({args.package} {args.app_version})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
