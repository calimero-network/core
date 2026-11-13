#!/usr/bin/env python3
"""Prove a pipeline end to end on a real cluster, in one command.

    scripts/pipeline-e2e.py --api https://node.example [--home DIR] [--key FILE] [--timeout 300] [--keep]
    scripts/pipeline-e2e.py --local            # starts 3 replicas and a job node of this tree's merod

Packs the compute fixture and its fan-out pipeline into a signed package (or takes `--package-file`),
installs it and creates the compute-only app, uploads three sealed files, runs the pipeline, and
checks the run, its outputs byte for byte, its receipt and the step cache's answer to a re-run.
Each step prints PASS or FAIL; the exit code is 1 if any failed.

Credentials are meroctl's: CALIMERO_API_KEY in the environment (never an argument), or the session
saved under `--home` by `meroctl login`. Binaries come from RV2_BIN_DIR (default target/release; build
them with `cargo build --release -p merod -p meroctl -p mero-sign`); `mero-app-build` is on PATH
(scripts/install-mero-app-build.sh). The cluster needs a job node, and a publisher key it trusts for `--key`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "crates" / "node" / "tests" / "fault"))
from compute_e2e import E2E, OPERATOR_SEED  # noqa: E402
from rv2 import BIN, CLI, ORIGIN, PackageRegistry, stop_all  # noqa: E402

PIPELINE = "fanout"
FIXTURE = ROOT / "apps" / "v2-fixtures" / "compute"
SIZES = (300_000, 120_000, 70_000)
BASE_PORT = 7685  # 7686-7689 locally, inside rv2's 7600-7699 range
JOB = 4
RESULTS: list[tuple[str, bool]] = []


def say(msg=""):
    print(msg, flush=True)


def step(name, fn):
    """Runs one step, prints its PASS or FAIL line, and returns its value (None on FAIL)."""
    start = time.time()
    try:
        value = fn()
        RESULTS.append((name, True))
        say(f"PASS {name} ({time.time() - start:.1f}s)")
        return value
    except Exception as e:  # a failed step must not stop the report
        RESULTS.append((name, False))
        say(f"FAIL {name}: {str(e)[:600]}")
        return None


def need(ok, what):
    if not ok:
        raise AssertionError(what)


class Meroctl:
    def __init__(self, api, home, allow_mock):
        self.base = [CLI, "--home", str(home), "--api", api] + (["--allow-mock"] if allow_mock else [])

    def __call__(self, *args, timeout=180):
        out = subprocess.run(self.base + list(args), capture_output=True, text=True, timeout=timeout)
        if out.returncode != 0:
            raise RuntimeError(f"meroctl {' '.join(args[:3])}: {out.stderr.strip()[-400:]}")
        return json.loads(out.stdout) if out.stdout.strip() else None


def build_package(wd, key, version):
    """The fixture's compute component and pipeline, packed and signed; the package file."""
    mero_sign = str(BIN / "mero-sign")
    if key is None:
        key = wd / "publisher.key"
        subprocess.run([mero_sign, "generate-key", "--output", str(key)], check=True, capture_output=True)
    wasm = wd / "compute.wasm"
    subprocess.run(["mero-app-build", str(FIXTURE), "--out", str(wd / "app.wasm"), "--compute-out", str(wasm),
                    "--target-dir", str(wd / "build")], cwd=ROOT, check=True, capture_output=True)
    mpk = wd / "pipeline-e2e.mpk"
    subprocess.run([sys.executable, "-I", "scripts/pack-component.py", "--compute", str(wasm),
                    "--pipeline", str(FIXTURE / "pipelines" / "fanout.yaml"), "--mero-sign", mero_sign,
                    "--key", str(key), "--package", "com.calimero.pipeline-e2e", "--app-version", version,
                    "--out", str(mpk)], cwd=ROOT, check=True, capture_output=True)
    return mpk


def data(i):
    return random.Random(i).randbytes(SIZES[i])


def run_pipeline(ctl, app, ids, timeout):
    started = ctl("pipeline", "run", app, PIPELINE, "--input", "docs=" + ",".join(ids))
    deadline = time.time() + timeout
    while time.time() < deadline:
        view = ctl("pipeline", "get", app, started["run"])
        if view["state"] != "running":
            return started["run"], view
        time.sleep(1)
    raise TimeoutError(f"run {started['run']} still running after {timeout}s")


def prove(ctl, mpk, version, args, wd):
    """Every step against the cluster `ctl` reaches; returns the app to clean up."""
    package = "com.calimero.pipeline-e2e"
    ns = ["--namespace", args.namespace] if args.namespace else []

    def create():
        ctl("app", "install", "--path", str(mpk))
        return ctl("context", "create", "--name", f"pipeline-e2e-{version.split('.')[-1]}", "--package", package,
                   "--version", version, *ns)["appId"]

    app = step("install the package and create the compute-only app", create)
    if not app:
        return None

    def upload():
        ids = []
        for i in range(len(SIZES)):
            f = wd / f"in-{i}.bin"
            f.write_bytes(data(i))
            up = ctl("file", "upload", str(f), "--context", app)
            need(up["size"] == SIZES[i] and up["sha256"] == hashlib.sha256(data(i)).hexdigest(), f"file {i}: {up}")
            ids.append(up["id"])
        return ids

    ids = step("upload 3 sealed files", upload)
    run = view = None
    if ids:
        got = step("run fan-out and collect to its end",
                   lambda: run_pipeline(ctl, app, ids, args.timeout))
        run, view = got or (None, None)
    if view:
        step("the run succeeded", lambda: need(view["state"] == "succeeded", f"state {view['state']}: {view}"))
        step("no step came from the cache", lambda: need(
            not any(s["cached"] for s in view["steps"].values()), f"{view['steps']}"))

        def outputs():
            out = wd / "out"
            ctl("pipeline", "download", app, run, "--out", str(out))
            first = data(0) + data(1)
            if args.tamper:
                first = first[:-1] + bytes([first[-1] ^ 1])
            need((out / "each-0.bin").read_bytes() == first, "each-0.bin is not the first two files' bytes")
            need((out / "each-1.bin").read_bytes() == data(2), "each-1.bin is not the third file's bytes")
            # digest XORs its input with the run's keystream, so its bytes are checked by size here and by the re-run's
            need((out / "all-0.bin").stat().st_size == sum(SIZES), "all-0.bin is not the files' total size")
            return hashlib.sha256((out / "all-0.bin").read_bytes()).hexdigest()

        first_digest = step("outputs match the expected bytes", outputs)

        def receipt():
            got = ctl("pipeline", "receipt", app, run, "--verify")
            need(got["verified"] is True and len(got["outputs"]) == 3 and all(o["matches"] for o in got["outputs"]),
                 f"receipt: {got}")

        step("the receipt verifies against the outputs", receipt)

        def rerun():
            again_run, again = run_pipeline(ctl, app, ids, args.timeout)
            need(again["state"] == "succeeded", f"state {again['state']}")
            need(all(s["cached"] for s in again["steps"].values()), f"not cached: {again['steps']}")
            ctl("pipeline", "download", app, again_run, "--out", str(wd / "again"))
            need(hashlib.sha256((wd / "again" / "all-0.bin").read_bytes()).hexdigest() == first_digest,
                 "the cached collect output differs from the first run's")

        step("a re-run is answered from the step cache", rerun)
    return app


def clean(ctl, app):
    try:
        ctl("context", "delete", app)
        ctl("app", "uninstall", "com.calimero.pipeline-e2e")
    except Exception as e:  # leaving a test app behind is reported, not fatal
        say(f"note: cleanup incomplete: {str(e)[:200]}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--api", help="the node's URL (a cluster that has a job node)")
    ap.add_argument("--local", action="store_true", help="start a local cluster (3 replicas, 1 job node) instead")
    ap.add_argument("--home", help="meroctl's home, holding the saved session (default: a scratch one)")
    ap.add_argument("--allow-mock", action="store_true", help="trust mock attestation (a development cluster)")
    ap.add_argument("--namespace", help="create the app in this organization")
    ap.add_argument("--key", help="the publisher key file to sign with (default: a generated one)")
    ap.add_argument("--package-file", help="a built package to install instead of building one")
    ap.add_argument("--timeout", type=int, default=300, help="seconds a run may take")
    ap.add_argument("--keep", action="store_true", help="leave the app and the work directory in place")
    ap.add_argument("--tamper", action="store_true", help="expect a wrong byte, to see the check fail")
    args = ap.parse_args()
    if bool(args.api) == args.local:
        ap.error("give --api URL or --local")

    wd = Path(tempfile.mkdtemp(prefix="pipeline-e2e-"))
    version = f"1.0.{int(time.time())}"
    cluster = registry = app = ctl = None
    try:
        mpk = Path(args.package_file) if args.package_file else None
        if mpk is None:
            mpk = step("build and sign the package", lambda: build_package(wd, args.key and Path(args.key), version))
        if mpk and args.package_file:
            version = json.loads(subprocess.run(["tar", "-xzOf", str(mpk), "manifest.json"], capture_output=True,
                                                check=True).stdout)["appVersion"]
        if args.local:
            registry = PackageRegistry(wd / "registry")
            cluster = E2E(wd, base_port=BASE_PORT, cluster="pipelinee2e", registry=registry)
            cluster.start()
            (wd / "operator.key").write_text(OPERATOR_SEED)
            args.api, args.allow_mock = cluster.url(1), True
        ctl = Meroctl(args.api, args.home or wd / "cfg", args.allow_mock)
        if args.local:
            os.environ.pop("CALIMERO_API_KEY", None)
            ctl("login", "--dev-passkey", str(wd / "dev-passkey.json"), "--name", "owner", "--origin", ORIGIN)
            args.namespace = ctl("namespace", "create", "pipeline-e2e")["orgId"]
            cluster.start_job(JOB, cluster.ops(1, "ops/cluster")["identity"])
            cluster.operate("add-node", str(JOB), cluster.url(JOB), "--role", "job")
            time.sleep(3)
        if mpk:
            app = prove(ctl, mpk, version, args, wd)
    finally:
        if app and not args.keep:
            clean(ctl, app)
        stop_all()
        if registry:
            registry.stop()
        if not args.keep:
            shutil.rmtree(wd, ignore_errors=True)
    failed = [n for n, ok in RESULTS if not ok]
    say(f"\n{'PASS' if not failed else 'FAIL'}: {len(RESULTS) - len(failed)}/{len(RESULTS)} steps"
        + (f"; failed: {', '.join(failed)}" if failed else ""))
    if args.keep:
        say(f"kept {wd}")
    return 1 if failed or not RESULTS else 0


if __name__ == "__main__":
    sys.exit(main())
