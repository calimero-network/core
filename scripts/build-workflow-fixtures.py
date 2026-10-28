#!/usr/bin/env python3
"""Build what the release-v2 workflows (workflows/**, apps/*/workflows/**)
install, into target/workflow-wasm/ (override with CORE_WASM_DIR), which the
workflows name as `{{env.CORE_WASM_DIR}}`:

- `dist/<package>-<version>.mpk`: every migration fixture
  `workflows/app-migration/build-wasms.sh` bundles (`cargo mero bundle --dev`,
  same `--package` / `--app-version`, the no-ABI bundle and the
  frontend-release pair), plus its `app.wasm` as `<crate>.wasm`: a workflow
  installs the signed release by name and upgrades by version; and
  outbound-probe's release (outbound.yml installs it: outbound requests are
  served only to a signed release declaring its URL prefixes);
- `<crate>.wasm` for every app under apps/ (`cargo mero build`, so each carries
  its ABI, which the node reads): a workflow uploads it as a bundle;
- `dist/<package>-1.0.0.mpk` for a multi-service manifest
  (scaffolding-e2e-multi): its services name the crates they run, built above
  as themselves, and the signed release is what a workflow installs to deploy
  a service by name.

Each crate's res/ is left as it was (a bundle build rewrites it).
Usage: scripts/build-workflow-fixtures.py [--force] [name-filter ...]
"""

import gzip
import io
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(os.environ.get("CORE_WASM_DIR") or ROOT / "target/workflow-wasm")
BUILD_WASMS = ROOT / "workflows/app-migration/build-wasms.sh"

#: Crates under apps/ that are not apps a workflow installs, with why.
NOT_APPS = {
    "abi_conformance_resolved": "an ABI generator fixture (its ABI is what is tested)",
    "state-schema-conformance": "a state-schema generator fixture",
    "migration-harness-example": "an rlib: a `#[app::migrate]` example run by TestHost",
    "migrations": "the migration fixtures (FIXTURES, built by build-wasms.sh's list)",
    "rooms": "a calimero:app component (the v2 SDK), built by calimero-app-build for wasm32-wasip2, not cargo mero; the node's tests and the fault script run it",
}


def _cargo(app_dir: Path) -> dict:
    return tomllib.loads((app_dir / "Cargo.toml").read_text())


def apps() -> list[dict]:
    """Every app under apps/ a workflow can install: its directory, crate,
    package id and services (a multi-service manifest builds no crate of its
    own: each service names the crate it runs)."""
    out = []
    for cargo in sorted(ROOT.glob("apps/*/Cargo.toml")):
        d = cargo.parent
        if d.name in NOT_APPS:
            continue
        data = _cargo(d)
        pkg = data["package"]
        meta = pkg.get("metadata", {}).get("calimero", {})
        services = {s["name"]: s.get("crate", pkg["name"]) for s in meta.get("services", [])}
        if "cdylib" not in data.get("lib", {}).get("crate-type", []) and not services:
            continue
        out.append(
            {
                "app": d.name,
                "crate": None if services else pkg["name"],
                "package": meta.get("package") or f"com.calimero.{d.name}",
                "services": services,
            }
        )
    return out


def crate_of(app_dir: Path) -> str:
    return tomllib.loads((app_dir / "Cargo.toml").read_text())["package"]["name"]


def fixtures() -> list[dict]:
    """Every bundle build-wasms.sh makes: package, version, crate, dist name."""
    out = []
    suites = re.findall(r'^\s*"(apps/migrations/[^"]+)"', BUILD_WASMS.read_text(), re.M)
    for suite in suites:
        name = Path(suite).name
        base = re.sub(r"-v[0-9].*$", "", name)
        version = re.sub(r".*-v([0-9]+).*", r"\1", name)
        out.append(
            {
                "dir": suite,
                "crate": crate_of(ROOT / suite),
                "package": f"com.calimero.{base}",
                "version": f"{version}.0.0",
            }
        )
    out.append(
        {
            "dir": "apps/migrations/migration-suite-v2-add-field",
            "crate": "migration-suite-noabi",
            "package": "com.calimero.migration-suite-noabi",
            "version": "2.0.0",
            "no_abi": True,
        }
    )
    for v in (1, 2):
        out.append(
            {
                "dir": "apps/migrations/migration-suite-v1",
                "crate": f"frontend-release-v{v}",
                "package": "com.calimero.frontend-release",
                "version": f"{v}.0.0",
                "frontend_dir": f"apps/migrations/frontend-release/v{v}",
            }
        )
    # outbound-probe as the signed release it must be: the node hands an
    # outbound client only to a release that declares its URL prefixes.
    out.append(
        {
            "dir": "apps/outbound-probe",
            "crate": "outbound-probe",
            "package": "com.calimero.outbound-probe",
            "version": "1.0.0",
        }
    )
    for f in out:
        f["mpk"] = f"dist/{f['package']}-{f['version']}.mpk"
    return out




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
    for f in fixtures():
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
    for app in [a["app"] for a in apps() if a["crate"]]:
        if args and not any(a in app for a in args):
            continue
        crate = crate_of(ROOT / "apps" / app).replace("-", "_")
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
    # A multi-service manifest as the signed release it is, so a workflow
    # installs it and deploys each service by name.
    for a in apps():
        if a["crate"] or (args and not any(x in a["app"] for x in args)):
            continue
        mpk = OUT / "dist" / f"{a['package']}-1.0.0.mpk"
        if mpk.exists() and not force:
            print(f"have {a['app']}", flush=True)
            continue
        print(f"bundling {a['app']} (services: {', '.join(a['services'])})", flush=True)
        cmd = [
            "cargo", "mero", "bundle", "--dev", "--no-icon", "--no-logo",
            "--manifest-path", f"apps/{a['app']}/Cargo.toml",
            "--app-version", "1.0.0", "-o", str(mpk),
        ]  # fmt: skip
        if not (run(cmd) and mpk.exists()):
            failed.append(a["app"])
    if failed:
        print(f"failed: {', '.join(failed)}", file=sys.stderr)
        return 1
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
