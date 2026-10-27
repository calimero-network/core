#!/usr/bin/env python3
"""Build what the release-v2 ports of core's workflows install, into
target/cloud-v2-wasm/ (override with CORE_WASM_DIR).

- `dist/<package>-<version>.mpk`: every bundle
  `workflows/app-migration/build-wasms.sh` makes (`cargo mero bundle --dev`,
  same `--package` / `--app-version`, the no-ABI bundle and the
  frontend-release pair), plus its `app.wasm` as `<crate>.wasm`;
- `<crate>.wasm` for kv-store, kv-store-with-handlers and scaffolding-e2e
  (`cargo mero build`, so each carries its ABI, which the node reads).

The list comes from scripts/cloud-v2-wasm-map.py, so the two agree. Each
crate's res/ is left as it was (a bundle build rewrites it).
Usage: scripts/build-cloud-v2-fixtures.py [--force] [name-filter ...]
"""

import gzip
import importlib.util
import io
import os
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(os.environ.get("CORE_WASM_DIR") or ROOT / "target/cloud-v2-wasm")

spec = importlib.util.spec_from_file_location(
    "wasm_map", Path(__file__).with_name("cloud-v2-wasm-map.py")
)
wasm_map = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wasm_map)


def extract_wasm(mpk: Path, dest: Path) -> None:
    with tarfile.open(fileobj=io.BytesIO(gzip.decompress(mpk.read_bytes()))) as tar:
        for m in tar.getmembers():
            if m.name.lstrip("./") == "app.wasm":
                dest.write_bytes(tar.extractfile(m).read())
                return
    raise RuntimeError(f"{mpk}: no app.wasm")


def run(cmd: list[str]) -> bool:
    env = {**os.environ, "CARGO_INCREMENTAL": "0"}
    r = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stdout[-1500:], r.stderr[-3000:], sep="\n", file=sys.stderr)
    return r.returncode == 0


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--force"]
    force = "--force" in sys.argv
    (OUT / "dist").mkdir(parents=True, exist_ok=True)
    failed = []
    for f in wasm_map.fixtures():
        label = f"{f['package']}@{f['version']}"
        if args and not any(a in label or a in f["dir"] for a in args):
            continue
        mpk = OUT / f["mpk"]
        dest = OUT / f"{f['crate'].replace('-', '_')}.wasm"
        if mpk.exists() and dest.exists() and not force:
            print(f"have {label}", flush=True)
            continue
        print(f"bundling {label} from {f['dir']}", flush=True)
        cmd = [
            "cargo", "mero", "bundle", "--dev", "--no-icon", "--no-logo",
            "--manifest-path", f"{f['dir']}/Cargo.toml",
            "--package", f["package"], "--app-version", f["version"],
            "-o", str(mpk),
        ]  # fmt: skip
        if f.get("no_abi"):
            cmd.append("--no-abi")
        if f.get("frontend_dir"):
            cmd += ["--frontend-dir", f["frontend_dir"]]
        # `cargo mero bundle` also writes the crate's res/ (a `--no-abi` build
        # a WASM without its ABI); other tests read res/ as `cargo mero build`
        # leaves it, so it is put back as it was.
        res = ROOT / f["dir"] / "res"
        kept = {q: q.read_bytes() for q in res.glob("*")} if res.is_dir() else {}
        try:
            built = run(cmd) and mpk.exists()
        finally:
            for q in res.glob("*") if res.is_dir() else []:
                if q not in kept:
                    q.unlink()
            for q, data in kept.items():
                if not q.exists() or q.read_bytes() != data:
                    q.write_bytes(data)
        if not built:
            failed.append(label)
            continue
        extract_wasm(mpk, dest)
    for app in wasm_map.PLAIN_APPS:
        if args and not any(a in app for a in args):
            continue
        crate = wasm_map.crate_of(ROOT / "apps" / app).replace("-", "_")
        dest = OUT / f"{crate}.wasm"
        if dest.exists() and not force:
            print(f"have {app}", flush=True)
            continue
        print(f"building {app}", flush=True)
        manifest = ROOT / "apps" / app / "Cargo.toml"
        if not run(["cargo", "mero", "build", "--manifest-path", str(manifest)]):
            failed.append(app)
            continue
        shutil.copy2(manifest.parent / "res" / f"{crate}.wasm", dest)
    if failed:
        print(f"failed: {', '.join(failed)}", file=sys.stderr)
        return 1
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
