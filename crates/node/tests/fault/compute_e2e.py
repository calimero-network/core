"""Real-process end-to-end test of compute actions: 3 replicas and 2 job nodes, all `merod run`.

The app is built as two components (`app.wasm` and `compute.wasm`) and installed from a signed
package that carries both. Scenarios: happy path, a job node killed mid-run, restart determinism,
cancel, the three caps, an output the result method does not store, the quota, and the job
nodes' logs showing only the compute component. Forged finals, per-run key isolation and
the collection of unreferenced files after `BLOB_IDLE_MS` (an hour of entry time, which only an
in-process clock can pass) are proved by in-process tests that are run and quoted. Binaries from
`RV2_BIN_DIR` (default target/release); the compute fixture is built by `mero-app-build` (on PATH,
`scripts/install-mero-app-build.sh`), or taken from `COMPUTE_E2E_WASM` and `COMPUTE_E2E_COMPUTE_WASM` (both).
Work dir and logs: `COMPUTE_E2E_DIR` (default target/compute-e2e).
The operator's decisions (`/ops/nodes`, `/ops/quotas`) are signed with the key the nodes start with
(`--operator-key`), as the node tests' harness signs them.

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
from rv2 import BIN, MEROD, ORIGIN, ROOT, Client, Cluster, PackageRegistry, http  # noqa: E402

BASE_PORT = 7650
PACKAGE = "com.calimero.compute-fixture"
VERSION = "1.0.0"
JOBS = (4, 5)
SMALL_MIB = 5
VIEW_BYTES = 100_000  # a small input, so the scenarios that only need a result stay quick
BIG_MIB = 40  # long enough to kill the job node mid-run
WAIT = 180
CANCEL_DEADLINE = 20  # the leader's wait for a job node to report a cancel
SETTLE = 12  # poll rounds after a resume, in seconds
UPLOAD_TRIES = 5  # FILES_UNAVAILABLE is retryable on an overloaded runner
OPERATOR_SEED = "09" * 32  # the node tests' operator key (harness OPERATOR_SEED, [9; 32])
OPERATOR_PUBLIC = "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"  # its verify key
IN_PROCESS = [
    ("forged final", "compute_runs", "a_final_not_signed_by_the_assigned_job_node_changes_nothing"),
    ("key isolation", "job_runs", "a_job_node_takes_work_only_from_its_cluster_leader_with_its_runs_token_and_keys"),
    ("seed determinism", "job_runs", "a_run_streams_its_input_and_seals_its_output_and_a_second_node_writes_the_same_bytes"),
    ("kept output", "compute_runs", "an_output_the_result_method_stores_is_kept_past_the_idle_time_and_a_run_reads_it_back"),
    ("unkept output", "compute_runs", "an_output_the_result_method_does_not_store_is_collected_and_its_quota_released"),
    ("retired job node", "job_nodes", "a_job_node_on_a_retired_release_is_not_attested"),
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
            return super().args(i) + ["--operator-key", OPERATOR_PUBLIC]
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

    def operate(self, *args):
        """An operator decision through meroctl, which signs it with the operator key when the node asks."""
        return self.cli(1, "--ops-token", self.ops_token, "--operator-key-file", str(self.workdir / "operator.key"),
                        "ops", *args)

    def job_load(self, i):
        return http("GET", self.url(i) + "job/load", timeout=5)


def build_components(wd):
    """The fixture's app and compute components, from one build (or the two env overrides)."""
    app, compute = os.environ.get("COMPUTE_E2E_WASM"), os.environ.get("COMPUTE_E2E_COMPUTE_WASM")
    if bool(app) != bool(compute):
        raise SystemExit("set both COMPUTE_E2E_WASM and COMPUTE_E2E_COMPUTE_WASM, or neither")
    if app:
        return Path(app), Path(compute)
    app, compute = wd / "app.wasm", wd / "compute.wasm"
    subprocess.run(["mero-app-build", "apps/v2-fixtures/compute", "--out", str(app),
                    "--compute-out", str(compute), "--target-dir", str(wd / "build")], cwd=ROOT, check=True)
    return app, compute


def pack(wd, app, compute):
    """A signed package carrying both components, as a release ships them."""
    mpk = wd / f"{PACKAGE}-{VERSION}.mpk"
    subprocess.run([sys.executable, "-I", "scripts/pack-component.py", "--wasm", str(app), "--compute", str(compute),
                    "--package", PACKAGE, "--app-version", VERSION, "--out", str(mpk),
                    "--mero-sign", str(BIN / "mero-sign")], cwd=ROOT, check=True)
    return mpk


def cargo_tests():
    """The in-process tests for what needs a forger inside the cluster; also builds the fixture."""
    out = {}
    for label, target, name in IN_PROCESS:
        r = subprocess.run(["cargo", "test", "--release", "-p", "calimero-node", "--test", target, name,
                            "--", "--exact"], cwd=ROOT, capture_output=True, text=True,
                           env={**os.environ, "CARGO_INCREMENTAL": "0"})
        out[label] = (target, name, r.returncode == 0 and "1 passed" in r.stdout, r.stdout.strip().splitlines()[-3:])
    return out


def upload(cl, key, app, mib=0, length=0):
    """A sealed file of `mib` MiB (or `length` bytes): its blob id and plaintext SHA-256, as the answer gives them."""
    piece = bytes((i * 7 + 3) % 251 for i in range(1 << 20))
    data = piece * mib if mib else piece[:length]
    req = urllib.request.Request(cl.url(1) + f"apps/{app}/blobs?sealed=true", data=data, method="POST",
                                 headers={"authorization": f"Bearer {key}", "content-type": "application/octet-stream"})
    for tries in range(1, UPLOAD_TRIES + 1):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                answer = json.loads(r.read())
            assert answer["sha256"] == hashlib.sha256(data).hexdigest(), answer
            return answer["id"], answer["sha256"]
        except urllib.error.HTTPError as e:
            body = e.read()
            if e.code != 503 or tries == UPLOAD_TRIES or json.loads(body).get("error") != "FILES_UNAVAILABLE":
                raise RuntimeError(f"upload refused: {e.code} {body[:300]!r}") from None
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

    def output_ids(self, run):
        return [o["id"] for o in (self.record(run).get("final") or {}).get("outputs", [])]

    def info(self, blob):
        return http("GET", self.cl.url(1) + f"apps/{self.app}/blobs/{blob}/info", headers=self.client.h)

    def until_final(self, run, timeout=WAIT):
        return self.until(f"run {run} final", lambda: self.record(run)["state"] in ("done", "failed", "cancelled"), timeout)

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


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def only_compute_reaches_job_nodes(cl, rig):
    say("\n== 10 a job node is only ever given the compute component")
    app, compute = rig.components["app"], rig.components["compute"]
    logs = {j: (cl.workdir / f"node{j}.log").read_text(errors="replace") for j in JOBS}
    prepared = {j: [line for line in text.splitlines() if "job node prepared a component" in line]
                for j, text in logs.items()}
    held = {j: sorted({h for h in (app, compute) if any(h in line for line in lines)}) for j, lines in prepared.items()}
    anywhere = [j for j, text in logs.items() if app in text]
    check("10 job nodes hold only the compute component",
          compute != app and any(compute in h for h in held.values()) and not anywhere
          and all(set(h) <= {compute} for h in held.values()),
          f"app component {app[:12]}, compute component {compute[:12]}",
          "; ".join(f"job node {j} prepared {[h[:12] for h in held[j]]} ({len(prepared[j])} log lines)" for j in JOBS)
          + f"; the app's hash appears in job logs of {anywhere or 'none'}")


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


def unkept_output(cl, rig, in_process):
    say("\n== 8 an output the result method does not store")
    file8, _ = upload(cl, rig.client.h["authorization"][7:], rig.app, length=VIEW_BYTES)
    before_done = rig.done()
    run = rig.start("startUnkept", {"inputs": [file8]})
    rig.until_final(run)
    rec = rig.record(run)
    out = rig.output_ids(run)
    info = rig.info(out[0]) if out else {}
    check("8 unkept output is registered, then left to R8", rec["state"] == "done" and len(out) == 1
          and info.get("size") == VIEW_BYTES and rig.done() == before_done + 1,
          f"startUnkept on input {file8[:12]}, results {before_done}",
          f"state {rec['state']}, output {out[0][:12] if out else None} registered (size {info.get('size')}), "
          f"results {rig.done()}")
    for label in ("kept output", "unkept output"):
        t = in_process.get(label)
        if not t:
            say(f"  {label}: skipped (--skip-cargo-tests)")
            continue
        say("  BLOB_IDLE_MS is an hour of entry time, so the collection after it (bytes dropped on every")
        say("  replica, quota released) is proved by the in-process test named here:")
        say(f"  cargo test --release -p calimero-node --test {t[0]} {t[1]}: {'ok' if t[2] else 'FAILED'}")
        check(f"8 {label} (in-process)", t[2], t[1], "; ".join(t[3][-2:]))


def quota(cl, rig):
    say("\n== 9 quota: an output that would pass the organization's file quota")
    key = rig.client.h["authorization"][7:]
    used = sum(http("GET", cl.url(1) + "auth/usage", headers=rig.client.h)["fileBytes"].values())
    cap = used + (SMALL_MIB + 1 << 20)
    cl.operate("set-quotas", json.dumps({"maxFileBytesPerOrg": cap}))
    file9, _ = upload(cl, key, rig.app, SMALL_MIB)
    run = rig.start("startDigest", {"inputs": [file9]})
    out = rig.outcome(run)
    check("9 quota", out["status"] == "failed" and out["reason"] == "quota" and out["outputs"] == 0,
          f"{used} bytes charged, quota {cap}, input uploaded, digest started",
          f"status {out['status']} reason {out['reason']} outputs {out['outputs']}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-cargo-tests", action="store_true")
    ap.add_argument("--workdir", default=os.environ.get("COMPUTE_E2E_DIR", str(ROOT / "target" / "compute-e2e")))
    args = ap.parse_args()
    offset = int(os.environ.get("WORKFLOW_PORT_OFFSET", "0"))
    wd = Path(args.workdir)
    shutil.rmtree(wd, ignore_errors=True)
    wd.mkdir(parents=True)
    in_process = {} if args.skip_cargo_tests else cargo_tests()
    app_wasm, compute_wasm = build_components(wd)
    mpk = pack(wd, app_wasm, compute_wasm)
    registry = PackageRegistry(wd / "registry")
    cl = E2E(wd, base_port=BASE_PORT + offset, cluster="computee2e", registry=registry)
    try:
        cl.start()
        (wd / "cfg").mkdir(parents=True, exist_ok=True)
        (wd / "operator.key").write_text(OPERATOR_SEED)
        cl.cli(1, "login", "--dev-passkey", str(wd / "dev-passkey.json"), "--name", "owner", "--origin", ORIGIN)
        org = cl.cli(1, "namespace", "create", "Acme")["orgId"]
        package, version = registry.publish(mpk)
        cl.cli(1, "app", "install", f"{package}@{version}")
        app = cl.cli(1, "context", "create", "--name", "compute", "--package", package, "--version", version,
                     "--namespace", org)["appId"]
        key = cl.cli(1, "account", "api-key", "create", "--label", "compute-e2e",
                      "--dev-passkey", str(wd / "dev-passkey.json"))["key"]
        identity = cl.ops(1, "ops/cluster")["identity"]
        for j in JOBS:
            cl.start_job(j, identity)
            cl.operate("add-node", str(j), cl.url(j), "--role", "job")
        time.sleep(3)
        rig = Rig(cl, app, key)
        rig.components = {"app": sha256_file(app_wasm), "compute": sha256_file(compute_wasm)}
        scenarios(cl, rig, in_process)
    finally:
        cl.stop()
        registry.stop()
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

    say("\n== 1b a run reads a kept output back")
    small, _ = upload(cl, rig.client.h["authorization"][7:], rig.app, length=VIEW_BYTES)
    run_b = rig.start("startDigest", {"inputs": [small]})
    out_b = rig.outcome(run_b)
    kept = rig.output_ids(run_b)
    again = rig.start("startDigest", {"inputs": kept}) if kept else None
    out_again = rig.outcome(again) if again else {}
    check("1b kept output read back by a run", out_b["status"] == "done" and out_again.get("status") == "done"
          and out_again.get("outputs") == 1,
          f"{VIEW_BYTES} byte input, result method stores the output's BlobRef",
          f"output {kept[0][:12] if kept else None} taken as the input of a second run, "
          f"status {out_again.get('status')}, output sha256 {str(out_again.get('sha256'))[:16]}")

    say("\n== 2 kill the assigned job node mid-run")
    big, _ = upload(cl, rig.client.h["authorization"][7:], rig.app, BIG_MIB)
    done0 = rig.done()
    run2 = rig.start("startDigest", {"inputs": [big]})
    rig.until("attempt assigned", lambda: rig.record(run2).get("attempts"))
    first = rig.record(run2)["attempts"][0]["node"]
    before = f"attempt 1 on job node {first} (pid {cl.procs[first].pid}), results {rig.done()}"
    cl.kill(first)
    out2 = rig.outcome(run2)
    rec2 = rig.record(run2)
    nodes = [a["node"] for a in rec2["attempts"]]
    check("2 kill assigned job node", out2["status"] == "done" and len(nodes) == 2 and nodes[1] != first
          and rig.done() == done0 + 1, before,
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
    # A wall clock that ran out is a lost attempt: spin is retried and ends lost after MAX_ATTEMPTS.
    for method, reason, attempts in (("spin", "lost", 3), ("hog", "memory", None), ("burn", "fuel", None)):
        r = rig.start("startRun", {"method": method})
        o = rig.outcome(r)
        made = len(rig.record(r).get("attempts") or [])
        check(f"5 cap {method}",
              o["status"] == "failed" and o["reason"] == reason and o["outputs"] == 0 and attempts in (None, made),
              f"{method} started", f"status {o['status']} reason {o['reason']} outputs {o['outputs']} attempts {made}")

    say("\n== 6 forged final, unassigned-node decryption and a retired job node (in-process: a forger or a")
    say("  release the operator retired must sit inside the cluster)")
    for label in ("forged final", "key isolation", "retired job node"):
        t = in_process.get(label)
        if not t:
            say(f"  {label}: skipped (--skip-cargo-tests)")
            continue
        say(f"  cargo test --release -p calimero-node --test {t[0]} {t[1]}: {'ok' if t[2] else 'FAILED'}")
        check(f"6 {label} (in-process)", t[2], t[1], "; ".join(t[3][-2:]))

    stuck_cancel(cl, rig)
    unkept_output(cl, rig, in_process)
    quota(cl, rig)
    only_compute_reaches_job_nodes(cl, rig)


if __name__ == "__main__":
    main()
