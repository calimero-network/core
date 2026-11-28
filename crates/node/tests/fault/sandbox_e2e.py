"""Real-process end-to-end test of sandbox runs: 3 replicas, a sandbox job node and a plain one,
all `merod run`.

The sandbox fixture (`apps/fixture-sandbox`) is built as an app component and a sandbox
component and installed from a signed package carrying both. The operator makes a `sandbox`
pool of the sandbox job node (`merod run --job-node --sandbox`, id 4) beside a `standard` pool of
the plain one (id 5). Scenarios: a Python script over a sealed CSV runs in the sandbox and its
collected output is kept; the script cannot reach the network, the node's files or another run;
a failing script is the app's error; and only the sandbox job node is ever given the sandbox
component. The sandbox needs root on Linux (namespaces, mounts, cgroups).

Binaries from `RV2_BIN_DIR` (default target/release); work dir and logs: `SANDBOX_E2E_DIR`
(default target/sandbox-e2e).

    sudo python3 -I crates/node/tests/fault/sandbox_e2e.py
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compute_e2e import E2E, OPERATOR_SEED, RESULTS, check, say, upload  # noqa: E402
from rv2 import BIN, ORIGIN, ROOT, Client, PackageRegistry, http  # noqa: E402

BASE_PORT = 7680  # replicas 1-3, the sandbox job node 4, the plain job node 5
PACKAGE = "com.calimero.sandbox-fixture"
VERSION = "1.0.0"
SANDBOX_NODE, PLAIN_NODE = 4, 5
WAIT = 180

CSV = b"name,amount\nalice,3\nbob,5\nalice,4\ncarol,1\nbob,2\n"

SUM_SCRIPT = """
import csv, collections, json
totals = collections.Counter()
for row in csv.DictReader(open("input-0")):
    totals[row["name"]] += int(row["amount"])
json.dump(dict(sorted(totals.items())), open("out.txt", "w"))
print(sum(totals.values()))
"""

PROBE_SCRIPT = """
import os, socket
seen = []
for path in ["/etc/shadow", "/root", "/home", "/proc/1/root/etc/shadow"]:
    if os.path.exists(path):
        seen.append(path)
try:
    s = socket.socket(); s.settimeout(2); s.connect(("1.1.1.1", 443)); net = "CONNECTED"
except OSError:
    net = "blocked"
print(os.getuid(), net, sorted(os.listdir("/workspace")), seen)
"""


class SandboxE2E(E2E):
    """`E2E` whose job node 4 runs sandboxes."""

    def args(self, i):
        args = super().args(i)
        return args + ["--sandbox"] if i == SANDBOX_NODE else args


def build_components(wd):
    app, sandbox = wd / "app.wasm", wd / "sandbox.wasm"
    subprocess.run([str(BIN / "meroctl"), "app", "build", "apps/fixture-sandbox", "--out", str(app),
                    "--sandbox-out", str(sandbox), "--target-dir", str(wd / "build")], cwd=ROOT, check=True)
    return app, sandbox


def pack(wd, app, sandbox):
    mpk = wd / f"{PACKAGE}-{VERSION}.mpk"
    subprocess.run([str(BIN / "meroctl"), "app", "pack", "--wasm", str(app), "--sandbox", str(sandbox),
                    "--package", PACKAGE, "--app-version", VERSION, "--out", str(mpk)], cwd=ROOT, check=True)
    return mpk


class Rig:
    def __init__(self, cl, app, key):
        self.cl, self.app, self.key, self.client = cl, app, key, Client(cl, app, key)

    def start(self, script, inputs):
        return self.client.call(1, "startAnalyze", {"script": script, "inputs": inputs}, timeout=60)["result"]

    def record(self, run):
        return http("GET", self.cl.url(1) + f"apps/{self.app}/runs/{run}", headers=self.client.h)

    def outcome(self, run):
        deadline = time.time() + WAIT
        while time.time() < deadline:
            r = self.client.view(1, "outcome", {"run": run})["result"]
            if r:
                return json.loads(r)
            time.sleep(0.5)
        raise TimeoutError(f"run {run} did not end: {self.record(run)}")


def result_text(outcome):
    """The run's return value: the script's stdout, a JSON string in base64."""
    import base64

    raw = outcome.get("result")
    return json.loads(base64.b64decode(raw)) if raw else None


def scenarios(cl, rig):
    say("\n== 1 a Python script over a sealed CSV runs in the sandbox; its output is kept")
    csv_id, _ = upload(cl, rig.key, rig.app, data=CSV)
    run = rig.start(SUM_SCRIPT, [csv_id])
    out = rig.outcome(run)
    rec = rig.record(run)
    nodes = [a["node"] for a in rec.get("attempts", [])]
    say(f"  run record: {json.dumps(rec)[:600]}")
    check("1 sandbox run", out["status"] == "done" and result_text(out) == "15\n" and len(out["outputs"]) == 1
          and nodes == [SANDBOX_NODE],
          f"{len(CSV)} byte CSV, script sums amounts and writes out.txt",
          f"status {out['status']} stdout {result_text(out)!r} outputs {out['outputs']} attempts on {nodes}")

    say("\n== 2 the script sees no network, none of the node's files and nothing of the first run")
    run2 = rig.start(PROBE_SCRIPT, [])
    out2 = rig.outcome(run2)
    text = result_text(out2) or ""
    check("2 isolation", out2["status"] == "done" and text.startswith("65534 blocked ['script.py'] []"),
          "a second run, no inputs, probes /etc/shadow, /root, /home, the host's root and 1.1.1.1:443",
          f"status {out2['status']} stdout {text.strip()!r}")

    say("\n== 3 a failing script is the app's error, and keeps no output")
    run3 = rig.start("raise SystemExit(3)", [])
    out3 = rig.outcome(run3)
    check("3 failing script", out3["status"] == "failed" and out3["reason"] == "app-error" and not out3["outputs"],
          "the script exits 3", f"status {out3['status']} reason {out3['reason']} outputs {out3['outputs']}")

    say("\n== 4 only the sandbox job node is given the sandbox component")
    logs = {j: (cl.workdir / f"node{j}.log").read_text(errors="replace") for j in (SANDBOX_NODE, PLAIN_NODE)}
    prepared = {j: logs[j].count("job node prepared a component") for j in logs}
    check("4 sandbox component placement", prepared[SANDBOX_NODE] >= 1 and prepared[PLAIN_NODE] == 0,
          "pools: standard (job node 5), sandbox (job node 4)",
          f"components prepared: sandbox node {prepared[SANDBOX_NODE]}, plain node {prepared[PLAIN_NODE]}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--workdir", default=os.environ.get("SANDBOX_E2E_DIR", str(ROOT / "target" / "sandbox-e2e")))
    args = ap.parse_args()
    if os.geteuid() != 0:
        raise SystemExit("the sandbox needs root: run with sudo")
    wd = Path(args.workdir)
    shutil.rmtree(wd, ignore_errors=True)
    wd.mkdir(parents=True)
    app_wasm, sandbox_wasm = build_components(wd)
    mpk = pack(wd, app_wasm, sandbox_wasm)
    registry = PackageRegistry(wd / "registry")
    cl = SandboxE2E(wd, base_port=BASE_PORT, cluster="sandboxe2e", registry=registry)
    try:
        cl.start()
        (wd / "cfg").mkdir(parents=True, exist_ok=True)
        (wd / "operator.key").write_text(OPERATOR_SEED)
        cl.cli(1, "login", "--dev-passkey", str(wd / "dev-passkey.json"), "--name", "owner", "--origin", ORIGIN)
        org = cl.cli(1, "org", "create", "Acme")["orgId"]
        package, version = registry.publish(mpk)
        cl.cli(1, "app", "install", f"{package}@{version}")
        app = cl.cli(1, "instance", "create", "--name", "sandbox", "--package", package, "--version", version,
                     "--org", org)["appId"]
        key = cl.cli(1, "account", "api-key", "create", "--label", "sandbox-e2e",
                     "--dev-passkey", str(wd / "dev-passkey.json"))["key"]
        cl.operate("set-pools", json.dumps({
            "standard": {"runsPerNode": 2, "maxMemoryMib": 4096},
            "sandbox": {"runsPerNode": 1, "maxMemoryMib": 4096, "sandbox": True},
        }))
        identity = cl.ops(1, "ops/cluster")["identity"]
        for j, pool in ((SANDBOX_NODE, "sandbox"), (PLAIN_NODE, "standard")):
            cl.start_job(j, identity)
            cl.operate("add-node", str(j), cl.url(j), "--role", "job", "--pool", pool)
        time.sleep(3)
        scenarios(cl, Rig(cl, app, key))
    finally:
        cl.stop()
        registry.stop()
        say("\nSUMMARY")
        for name, ok in RESULTS:
            say(f"  {'PASS' if ok else 'FAIL'} {name}")
    if not RESULTS or not all(ok for _, ok in RESULTS):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
