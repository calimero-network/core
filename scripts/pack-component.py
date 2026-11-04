#!/usr/bin/env python3
"""Pack a `calimero:app` component into a signed single-service `.mpk`.

    python3 -I scripts/pack-component.py --wasm kv.wasm --package com.calimero.kv-store \
        --app-version 1.0.0 --out dist/com.calimero.kv-store-1.0.0.mpk

Builds the manifest the node reads (crates/bundle: version, package, appVersion,
minRuntimeVersion and the `app.wasm` artifact), signs it with `mero-sign sign
--dev` (the development key, which a public registry refuses; pass `--key` for
another), and writes manifest.json and app.wasm as a tar.gz. Build the component
first with `cargo run -p calimero-app-build --bin mero-app-build -- <crate-dir>
--out <file>`.

`--mero-sign` (default MERO_SIGN, else `cargo run -q -p mero-sign --`) names the
signer.
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
    ap.add_argument("--wasm", required=True, help="the component to pack")
    ap.add_argument("--package", required=True, help="the package id, e.g. com.calimero.kv-store")
    ap.add_argument("--app-version", required=True, help="the release's version, e.g. 1.0.0")
    ap.add_argument("--min-runtime-version", default="0.1.0")
    ap.add_argument("--out", required=True, help="the .mpk to write")
    ap.add_argument("--key", help="sign with this key file instead of the development key")
    ap.add_argument("--mero-sign", default=os.environ.get("MERO_SIGN"),
                    help="the mero-sign binary (default: cargo run -q -p mero-sign --)")
    args = ap.parse_args(argv)

    wasm = Path(args.wasm).read_bytes()
    if wasm[:8] != b"\0asm\x0d\0\x01\0":
        sys.exit(f"{args.wasm}: not a WASM component (an SDK 0.12 core module is refused by the node)")
    manifest = {
        "version": "1.0",
        "package": args.package,
        "appVersion": args.app_version,
        "minRuntimeVersion": args.min_runtime_version,
        "wasm": {"path": "app.wasm", "hash": hashlib.sha256(wasm).hexdigest(), "size": len(wasm)},
    }
    signer = shlex.split(args.mero_sign) if args.mero_sign else ["cargo", "run", "-q", "-p", "mero-sign", "--"]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "manifest.json"
        path.write_text(json.dumps(manifest))
        how = ["--key", args.key] if args.key else ["--dev"]
        subprocess.run(signer + ["sign", str(path)] + how, check=True, cwd=ROOT, stdout=subprocess.DEVNULL)
        signed = path.read_bytes()

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(out, "w:gz") as tar:
        for name, data in (("manifest.json", signed), ("app.wasm", wasm)):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    print(f"wrote {out} ({args.package} {args.app_version})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
