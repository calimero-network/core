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

`--frontend <dir>` ships a static web build (every file under the directory, as
frontend/<path>; `--frontend-entry`, default index.html, is the page served at `/`),
checked with the node's rules (crates/bundle/src/frontend.rs) before signing.
`--outbound <https prefix>` (repeatable) declares a URL prefix the app's runs may
send requests under (`is_outbound_url`); the app's organization still approves it.
`--upgrades owner|publisher` says who upgrades the package's instances (absent: owner).

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

# crates/bundle/src/frontend.rs: the file types a node serves, the path rule and the cap.
FRONTEND_TYPES = {"html", "js", "mjs", "css", "json", "webmanifest", "txt", "svg", "png", "jpg",
                  "jpeg", "gif", "webp", "ico", "woff", "woff2", "ttf", "wasm", "webm", "xml"}
FRONTEND_PUNCTUATION = set("._~@+-")
MAX_FRONTEND_BYTES = 32 * 1024 * 1024


def frontend_files(root: Path, entry: str) -> tuple[str, list[tuple[str, bytes]]]:
    """The static build under `root` as (archive path, bytes), refused as the node would."""
    if not root.is_dir():
        sys.exit(f"--frontend {root}: not a directory")
    files, seen, total = [], set(), 0
    for f in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = f.relative_to(root).as_posix()
        if not all(part and part not in (".", "..")
                   and all(c.isascii() and (c.isalnum() or c in FRONTEND_PUNCTUATION) for c in part)
                   for part in rel.split("/")):
            sys.exit(f"--frontend: {rel}: path parts may hold only ASCII letters, digits and {''.join(sorted(FRONTEND_PUNCTUATION))} (FRONTEND_UNSAFE_PATH)")
        if rel.lower() in seen:
            sys.exit(f"--frontend: {rel} differs from another file only in case (FRONTEND_DUPLICATE_PATH)")
        seen.add(rel.lower())
        ext = rel.rsplit("/", 1)[-1].rpartition(".")[2].lower() if "." in rel.rsplit("/", 1)[-1] else ""
        if ext not in FRONTEND_TYPES:
            sys.exit(f"--frontend: {rel}: a node serves only {', '.join(sorted(FRONTEND_TYPES))} (FRONTEND_UNSUPPORTED_TYPE)")
        data = f.read_bytes()
        total += len(data)
        files.append((f"frontend/{rel}", data))
    if total > MAX_FRONTEND_BYTES:
        sys.exit(f"--frontend: {total} bytes, over the {MAX_FRONTEND_BYTES} byte cap (FRONTEND_TOO_LARGE)")
    entry_path = f"frontend/{entry}"
    if entry_path not in {name for name, _ in files}:
        sys.exit(f"--frontend-entry {entry}: not a file of the build (FRONTEND_NO_ENTRY)")
    if not entry.lower().endswith(".html"):
        sys.exit(f"--frontend-entry {entry}: the entry must be an .html file (FRONTEND_ENTRY_NOT_HTML)")
    return entry_path, files


def is_outbound_url(url: str) -> bool:
    """`calimero_bundle::is_outbound_url`: https, a host, no credentials, query or fragment."""
    if not url.startswith("https://"):
        return False
    rest = url[len("https://"):]
    authority = rest.split("/")[0].split("?")[0].split("#")[0]
    return (len(url.encode()) <= 2048 and authority != "" and "@" not in authority
            and "?" not in url and "#" not in url
            and not any(c.isspace() or ord(c) < 32 or ord(c) == 127 for c in url))


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--wasm", help="the app component to pack (omit for a compute-only package)")
    ap.add_argument("--compute", help="the compute component of the same build, packed as compute.wasm")
    ap.add_argument("--pipeline", action="append", default=[],
                    help="a pipeline YAML file to ship (repeatable)")
    ap.add_argument("--frontend", help="a static web build directory to ship as frontend/")
    ap.add_argument("--frontend-entry", default="index.html",
                    help="the page under --frontend served at / (default index.html)")
    ap.add_argument("--outbound", action="append", default=[],
                    help="an https:// URL prefix the app's runs may send requests under (repeatable)")
    ap.add_argument("--upgrades", choices=["owner", "publisher"],
                    help="who upgrades the package's instances (default: owner)")
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
    frontend = []
    entry = None
    if args.frontend:
        entry, frontend = frontend_files(Path(args.frontend), args.frontend_entry)
    for url in args.outbound:
        if not is_outbound_url(url):
            sys.exit(f"--outbound {url}: an https:// prefix with a host, no credentials, query or fragment")
    manifest = {
        "version": "1.0",
        "package": args.package,
        "appVersion": args.app_version,
        "minRuntimeVersion": args.min_runtime_version,
        **{key: {"path": name, "hash": hashlib.sha256(data).hexdigest(), "size": len(data)}
           for key, (name, data) in parts.items()},
        **({"pipelines": [{"path": name, "hash": hashlib.sha256(data).hexdigest(), "size": len(data)}
                          for name, data in pipelines]} if pipelines else {}),
        **({"frontend": {"entry": entry,
                         "files": [{"path": name, "hash": hashlib.sha256(data).hexdigest(), "size": len(data)}
                                   for name, data in frontend]}} if frontend else {}),
        **({"outbound": args.outbound} if args.outbound else {}),
        **({"upgrades": args.upgrades} if args.upgrades else {}),
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
        for name, data in [("manifest.json", signed), *parts.values(), *pipelines, *frontend]:
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    print(f"wrote {out} ({args.package} {args.app_version})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
