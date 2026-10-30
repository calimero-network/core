"""Real-process end-to-end test of compute actions: 3 replicas and 2 job nodes, all `merod run`.

Scenarios: happy path, a job node killed mid-run, restart determinism, cancel, the three caps.
Forged finals and per-run key isolation need a forger inside the cluster, so the in-process
tests that prove them are run and quoted. Release binaries (`RV2_BIN_DIR`, default
target/release) and the compute fixture (cached by the node tests under target/app-components):

    python3 -I crates/node/tests/fault/compute_e2e.py [--skip-cargo-tests]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rv2 import MEROD, ORIGIN, ROOT, Client, Cluster, http  # noqa: E402

BASE_PORT = 7650
JOBS = (4, 5)
SMALL_MIB = 5
BIG_MIB = 40  # long enough to kill the job node mid-run
WAIT = 180
CANCEL_DEADLINE = 20  # the leader's wait for a job node to report a cancel
SETTLE = 12  # poll rounds after a resume, in seconds
UPLOAD_TRIES = 5  # FILES_UNAVAILABLE is retryable on an overloaded runner
IN_PROCESS = [
    ("forged final", "compute_runs", "a_final_not_signed_by_the_assigned_job_node_changes_nothing"),
    ("key isolation", "job_runs", "a_job_node_takes_work_only_from_its_cluster_leader_with_its_runs_token_and_keys"),
    ("seed determinism", "job_runs", "a_run_streams_its_input_and_seals_its_output_and_a_second_node_writes_the_same_bytes"),
]
RESULTS: list[tuple[str, bool]] = []


def say(msg=""):
    print(msg, flush=True)


def check(name, ok, before, after):
    say(f"  BEFORE: {before}")
    say(f"  AFTER:  {after}")
    say(f"{'PASS' if ok else 'FAIL'} {name}")
    RESULTS.append((name, bool(ok)))


class E2E(Cluster):
    """The replicas plus job nodes (ids 4 and 5), which are `merod run --job-node` processes."""

    identity = ""

    def args(self, i):
        if i <= self.n:
            return super().args(i)
        return [MEROD, "run", "--job-node", "--node-id", str(i), "--listen", f"127.0.0.1:{self.base_port + i}",
                "--data-dir", str(self.dir(i)), "--cluster", self.cluster, "--dev",
                "--cluster-identity", self.identity]

    def start_job(self, i, identity):
        self.identity = identity
        self.procs[i] = None
        self.start_node(i)

    def stop(self):
        for i in list(self.procs):
            self.kill(i)

    def ops(self, i, path):
        return http("GET", self.url(i) + path, headers={"authorization": f"Bearer {self.ops_token}"})

    def ops_text(self, i, path):
        req = urllib.request.Request(self.url(i) + path, headers={"authorization": f"Bearer {self.ops_token}"})
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.read().decode()

    def job_load(self, i):
        return http("GET", self.url(i) + "job/load", timeout=5)


def compute_wasm():
    found = sorted((ROOT / "target" / "app-components").glob("compute-*.wasm"), key=lambda p: p.stat().st_mtime)
    if not found:
        raise SystemExit("no compute component: run cargo test --release -p calimero-node --test compute_runs")
    return str(found[-1])


def cargo_tests():
    """The in-process tests for what needs a forger inside the cluster; also builds the fixture."""
    out = {}
    for label, target, name in IN_PROCESS:
        r = subprocess.run(["cargo", "test", "--release", "-p", "calimero-node", "--test", target, name,
                            "--", "--exact"], cwd=ROOT, capture_output=True, text=True,
                           env={**os.environ, "CARGO_INCREMENTAL": "0"})
        out[label] = (target, name, r.returncode == 0 and "1 passed" in r.stdout, r.stdout.strip().splitlines()[-3:])
    return out


def upload(cl, key, app, size):
    data = bytes((i * 7 + 3) % 251 for i in range(1 << 20)) * size
    req = urllib.request.Request(cl.url(1) + f"apps/{app}/blobs", data=data, method="POST",
                                 headers={"authorization": f"Bearer {key}", "content-type": "application/octet-stream"})
    for tries in range(1, UPLOAD_TRIES + 1):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                return json.loads(r.read())["id"], hashlib.sha256(data).hexdigest()
        except urllib.error.HTTPError as e:
            if e.code != 503 or tries == UPLOAD_TRIES or json.loads(e.read()).get("error") != "FILES_UNAVAILABLE":
                raise
            time.sleep(1)


class Rig:
    def __init__(self, cl, app, key):
        self.cl, self.app, self.client = cl, app, Client(cl, app, key)

    def start(self, method, args):
        return self.client.call(1, method, args, timeout=60)["result"]

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

    def done(self):
        return int(self.client.view(1, "done", {})["result"])

    def until(self, what, pred, timeout=60):
        deadline = time.time() + timeout
        while time.time() < deadline:
            v = pred()
            if v:
                return v
            time.sleep(0.1)
        raise TimeoutError(what)

    def compute_series(self):
        leader = self.cl.app_leader(self.app) or 1
        text = self.cl.ops_text(leader, "ops/metrics")
        return [line for line in text.splitlines() if line.startswith(("calimero_app_compute_", "calimero_app_calls_total"))
                and (self.app in line) and ("compute" in line)]


def stuck_cancel(cl, rig):
    say("\n== 7 stuck cancel: the job node is frozen, the leader still ends the run")
    run = rig.start("startRun", {"method": "spin"})
    node = rig.until("assigned", lambda: (rig.record(run).get("attempts") or [{}])[0].get("node"))
    pid = cl.procs[node].pid
    cl.pause(node)
    before_results = rig.done()
    before = f"run {rig.record(run)['state']} on job node {node} (pid {pid}, SIGSTOP), results {before_results}"
    try:
        t0 = time.time()
        rig.client.call(1, "cancel", {"run": run})
        out = rig.outcome(run)
        took = time.time() - t0
    finally:
        cl.resume(node)
    results = rig.done()
    time.sleep(SETTLE)
    rec = rig.record(run)
    after_out = rig.outcome(run)
    check("7 stuck cancel", out["status"] == "cancelled" and took <= CANCEL_DEADLINE + 15
          and results == before_results + 1 and rec["state"] == "cancelled"
          and after_out == out and rig.done() == results,
          before,
          f"cancelled after {took:.1f}s (deadline {CANCEL_DEADLINE}s), fuel {out['fuel']}, results {results}; "
          f"{SETTLE}s after SIGCONT state {rec['state']}, results {rig.done()}, outcome unchanged {after_out == out}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-cargo-tests", action="store_true")
    ap.add_argument("--workdir", default=str(ROOT / ".scratch" / "compute-actions" / "e2e" / "work"))
    args = ap.parse_args()
    offset = int(os.environ.get("WORKFLOW_PORT_OFFSET", "0"))
    wd = Path(args.workdir)
    shutil.rmtree(wd, ignore_errors=True)
    wd.mkdir(parents=True)
    in_process = {} if args.skip_cargo_tests else cargo_tests()
    wasm = compute_wasm()
    cl = E2E(wd, base_port=BASE_PORT + offset, cluster="computee2e")
    try:
        cl.start()
        (wd / "cfg").mkdir(parents=True, exist_ok=True)
        cl.cli(1, "login", "--dev-passkey", str(wd / "dev-passkey.json"), "--name", "owner", "--origin", ORIGIN)
        org = cl.cli(1, "namespace", "create", "Acme")["orgId"]
        app = cl.cli(1, "context", "create", "--name", "compute", "--path", wasm, "--namespace", org)["appId"]
        key = cl.cli(1, "account", "api-key", "create", "--label", "compute-e2e")["key"]
        identity = cl.ops(1, "ops/cluster")["identity"]
        for j in JOBS:
            cl.start_job(j, identity)
            cl.cli(1, "--ops-token", cl.ops_token, "ops", "add-node", str(j), cl.url(j), "--role", "job")
        time.sleep(3)
        rig = Rig(cl, app, key)
        scenarios(cl, rig, in_process)
    finally:
        cl.stop()
        say("\nSUMMARY")
        for name, ok in RESULTS:
            say(f"  {'PASS' if ok else 'FAIL'} {name}")
    if not RESULTS or not all(ok for _, ok in RESULTS):
        raise SystemExit(1)


def scenarios(cl, rig, in_process):
    say("\n== 1 happy path: 5 MiB input, digest on a job node")
    file1, plain_sha = upload(cl, rig.client.h["authorization"][7:], rig.app, SMALL_MIB)
    before = {"done": rig.done(), "series": len(rig.compute_series())}
    run = rig.start("startDigest", {"inputs": [file1]})
    out1 = rig.outcome(run)
    rec = rig.record(run)
    say(f"  run record: {json.dumps(rec)[:600]}")
    series = rig.compute_series()
    for line in series:
        say(f"  series: {line}")
    check("1 happy path", out1["status"] == "done" and out1.get("sha256") and out1["outputs"] == 1
          and rec["state"] == "done" and rig.done() == 1 and series,
          f"results recorded {before['done']}, input sha256 {plain_sha[:16]}",
          f"status {out1['status']} attempt {out1['attempt']} output sha256 {out1.get('sha256')} results {rig.done()}")

    say("\n== 2 kill the assigned job node mid-run")
    big, _ = upload(cl, rig.client.h["authorization"][7:], rig.app, BIG_MIB)
    run2 = rig.start("startDigest", {"inputs": [big]})
    rig.until("attempt assigned", lambda: rig.record(run2).get("attempts"))
    first = rig.record(run2)["attempts"][0]["node"]
    before = f"attempt 1 on job node {first} (pid {cl.procs[first].pid}), results {rig.done()}"
    cl.kill(first)
    out2 = rig.outcome(run2)
    rec2 = rig.record(run2)
    nodes = [a["node"] for a in rec2["attempts"]]
    check("2 kill assigned job node", out2["status"] == "done" and len(nodes) == 2 and nodes[1] != first
          and rig.done() == 2, before,
          f"attempts on nodes {nodes}, status {out2['status']}, results {rig.done()} (one new)")

    say("\n== 3 restart determinism")
    say("  digest XORs a keystream seeded per run (noise_seed of the app file key and the run id), so two")
    say("  run ids never produce equal bytes and the sha256 of scenario 2 is not comparable to scenario 1.")
    say("  The restart-with-the-same-seed property is proved in process by the test named here:")
    t = in_process.get("seed determinism")
    if t:
        say(f"  cargo test --release -p calimero-node --test {t[0]} {t[1]}: {'ok' if t[2] else 'FAILED'}")
        check("3 byte-identical restart (in-process)", t[2], "two job nodes, one assignment", "; ".join(t[3][-2:]))
    else:
        say("  (skipped: --skip-cargo-tests)")

    say("\n== 4 cancel")
    run4 = rig.start("startRun", {"method": "spin"})
    node4 = rig.until("assigned", lambda: (rig.record(run4).get("attempts") or [{}])[0].get("node"))
    before = f"run on job node {node4}, load {cl.job_load(node4)}"
    rig.client.call(1, "cancel", {"run": run4})
    out4 = rig.outcome(run4)
    load = rig.until("load 0", lambda: cl.job_load(node4)["running"] == 0 or None, timeout=30) if cl.job_load(node4)["running"] else True
    check("4 cancel", out4["status"] == "cancelled" and out4["fuel"] != "0" and load, before,
          f"status {out4['status']} fuel {out4['fuel']} load {cl.job_load(node4)}")

    say("\n== 5 caps")
    for method, reason in (("spin", "time"), ("hog", "memory"), ("burn", "fuel")):
        r = rig.start("startRun", {"method": method})
        o = rig.outcome(r)
        check(f"5 cap {method}", o["status"] == "failed" and o["reason"] == reason and o["outputs"] == 0,
              f"{method} started", f"status {o['status']} reason {o['reason']} outputs {o['outputs']}")

    say("\n== 6 forged final and unassigned-node decryption (in-process, a forger must sit inside the cluster)")
    for label in ("forged final", "key isolation"):
        t = in_process.get(label)
        if not t:
            say(f"  {label}: skipped (--skip-cargo-tests)")
            continue
        say(f"  cargo test --release -p calimero-node --test {t[0]} {t[1]}: {'ok' if t[2] else 'FAILED'}")
        check(f"6 {label} (in-process)", t[2], t[1], "; ".join(t[3][-2:]))

    stuck_cancel(cl, rig)


if __name__ == "__main__":
    main()
