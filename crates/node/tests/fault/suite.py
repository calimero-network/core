"""Process-level tests of a `merod run --dev` cluster: one process per node.

The replacement for merobox's cloud-v2 backend in core: its examples
(`workflow-examples/cloud-v2/*.yml`), its fuzzy load with the soak report, and the
release-image smoke. Every test starts its own 3-node cluster (rv2.py) on ports
7600-7699 (+ WORKFLOW_PORT_OFFSET), drives it over plain HTTP with API keys and
through meroctl, and stops it.

    python3 -I crates/node/tests/fault/suite.py                 # the PR set
    python3 -I crates/node/tests/fault/suite.py --nightly       # longer soak, the fault test, 3 iterations
    python3 -I crates/node/tests/fault/suite.py --smoke --merod PATH   # the release image's merod
    python3 -I crates/node/tests/fault/suite.py --only frozen-leader,kv-soak --iterations 5
    python3 -I crates/node/tests/fault/suite.py --list

Needs release merod and meroctl (`cargo build --release -p merod -p meroctl`, or
`--merod`/`--meroctl`), kv-store's WASM (`cargo build -p kv-store --target
wasm32-unknown-unknown --profile app-release`) and, for `packages`, two signed
releases of it (`(cd apps/wasmer-fixtures/kv-store && cargo mero bundle --dev --no-icon
--app-version 1.0.0)` and `1.1.0`, into `dist/`). See README.md beside this file.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import shutil
import subprocess
import sys
import threading
import time
import traceback
import uuid
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rv2  # noqa: E402
import soak  # noqa: E402
from rv2 import ORIGIN, ROOT, CliError, Cluster, HttpError, PackageRegistry  # noqa: E402

HERE = Path(__file__).resolve().parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")


class Failed(AssertionError):
    pass


def check(cond, what):
    if not cond:
        raise Failed(what)


def code_of(e):
    """The node's error code of a refusal (`{"error": CODE}`), or None."""
    if isinstance(e, HttpError) and isinstance(e.body, dict):
        return e.body.get("error")
    return None


def transport(e):
    """No answer from the node: refused, reset or timed out (not a refusal)."""
    return not isinstance(e, HttpError)


# ----------------------------------------------------------------- accounts


class Account:
    """A person: a file passkey, a meroctl home (its session), an API key for HTTP.

    `index` is its place in the test's accounts: as merobox did (each v1 member
    talked to its own node), an account sends to node `index % n + 1` first and
    skips to the next running node only when one does not answer."""

    def __init__(self, cl: Cluster, name: str, index: int):
        self.c = cl
        self.name = name
        self.index = index
        self.home = cl.workdir / "accounts" / name
        self.passkey = self.home / "dev-passkey.json"
        self.id = None
        self.key = None

    def cli(self, *args, node=None):
        return self.c.cli(node or self.order()[0], *args, home=self.home)

    def sign_up(self):
        self.home.mkdir(parents=True, exist_ok=True)
        self.cli("login", "--dev-passkey", str(self.passkey), "--name", self.name, "--origin", ORIGIN)
        self.id = self.cli("whoami")["accountId"]
        self.key = self.cli("account", "api-key", "create", "--label", f"suite-{self.name}",
                            "--dev-passkey", str(self.passkey))["key"]
        return self

    def raw(self, method, path, body=None):
        """Any route as this account; a `412 CONFIRM_REQUIRED` (invites, members) is
        answered with its passkey, as the person would."""
        args = ["raw", method, path] + ([json.dumps(body)] if body is not None else [])
        return self.cli(*args, "--dev-passkey", str(self.passkey))

    def order(self, node=None):
        if node is not None:
            return [int(node)]
        running = self.c.running() or self.c.ids
        home = self.index % len(running)
        return running[home:] + running[:home]

    def _send(self, node, f):
        last = None
        for n in self.order(node):
            try:
                return f(n)
            except Exception as e:  # noqa: BLE001
                if not transport(e) or node is not None:
                    raise
                last = e
        raise last or Failed("no node to reach")

    def call(self, app, method, args=None, node=None, key=None, idem=None, timeout=30):
        h = {"authorization": f"Bearer {key or self.key}"}
        body = {"method": method, "args": args or {}}
        if idem:
            body["idempotencyKey"] = idem
        return self._send(node, lambda n: rv2.http("POST", self.c.url(n) + f"apps/{app}/intents", body, h, timeout))

    def view(self, app, method, args=None, node=None, key=None, stale=False, min_index=0, timeout=30):
        h = {"authorization": f"Bearer {key or self.key}"}
        body = {"method": method, "args": args or {}}
        if stale:
            body["consistency"] = "stale"
        if min_index:
            body["minIndex"] = int(min_index)
        return self._send(node, lambda n: rv2.http("POST", self.c.url(n) + f"apps/{app}/views", body, h, timeout))

    def wait_for(self, app, method, args, equals, node=None, stale=False, min_index=0, timeout=20):
        """merobox's `wait_for`: the view, until its result equals `equals`."""
        deadline = time.time() + timeout
        last = None
        while True:
            try:
                out = self.view(app, method, args, node=node, stale=stale, min_index=min_index, timeout=5)
                last = out["result"]
                if last == equals:
                    return out
            except Exception as e:  # noqa: BLE001
                last = str(e)[:200]
            if time.time() > deadline:
                raise Failed(f"wait_for {method}{args} on node {node}: still {last!r} after {timeout}s")
            time.sleep(0.25)

    def deploy(self, wasm, name="app", org=None, confirm=()):
        args = ["context", "create", "--name", name, "--path", str(wasm)]
        if org:
            args += ["--namespace", org]
        for m in confirm:
            args += ["--confirm", m]
        return self.cli(*args)["appId"]


def expect_refused(code, f, retry_for=0.0):
    """`f()` must be refused with `code` (merobox's `expected_error`), within `retry_for` s."""
    deadline = time.time() + retry_for
    while True:
        try:
            out = f()
            got = f"success {str(out)[:120]}"
        except CliError as e:
            if e.has(code):
                return
            got = e.stderr.strip()[-200:]
        except HttpError as e:
            if code_of(e) == code:
                return
            got = str(e)[:200]
        if time.time() >= deadline:
            raise Failed(f"expected {code}, got {got}")
        time.sleep(0.25)


def find_leader(cl, app, timeout=10):
    """merobox's `find_leader`: the node leading the app's group, and a follower."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        leader = cl.app_leader(app)
        if leader:
            follower = next(i for i in cl.ids if i != leader)
            return leader, follower
        time.sleep(0.2)
    raise Failed(f"no leader for {app} within {timeout}s")


def ops_status(cl):
    for i in cl.running():
        if i in cl.paused:
            continue
        try:
            return cl.ops_status(i, timeout=5)
        except Exception:  # noqa: BLE001
            continue
    raise Failed("no node answers /ops/status")


# ----------------------------------------------------------------- tests

TESTS = {}


def test(name, port, replaces, smoke=False, pr=True):
    def register(f):
        TESTS[name] = {"f": f, "port": port, "replaces": replaces, "smoke": smoke, "pr": pr,
                       "doc": (f.__doc__ or "").strip().splitlines()[0]}
        return f
    return register


@test("kv-failover", 7620, "workflow-examples/cloud-v2/kv-store-failover.yml", smoke=True)
def kv_failover(ctx, cl):
    """A KV store keeps serving through a crash, a frozen node and a restart; replicas catch up."""
    alice = ctx.accounts(cl, "alice")[0]
    app = alice.deploy(ctx.kv_wasm, "kv")
    first = alice.call(app, "set", {"key": "a", "value": "1"})["index"]
    cl.kill(1)  # crash node 1
    alice.wait_for(app, "get", {"key": "a"}, "1")  # through the survivors
    second = alice.call(app, "set", {"key": "b", "value": "2"})["index"]  # two of three
    cl.start_node(1)  # back on its data
    cl.pause(2)  # frozen: unreachable, not gone
    third = alice.call(app, "set", {"key": "c", "value": "3"}, node=3)["index"]
    cl.resume(2)
    alice.wait_for(app, "get", {"key": "c"}, "3", node=2, stale=True, min_index=third)
    alice.wait_for(app, "get", {"key": "b"}, "2", node=1, stale=True, min_index=second)
    check(int(second) > int(first), f"index {second} after {first}")
    check(int(third) > int(second), f"index {third} after {second}")


@test("frozen-leader", 7625, "workflow-examples/cloud-v2/frozen-leader.yml", smoke=True)
def frozen_leader(ctx, cl):
    """The app's leader stops answering (SIGSTOP); a write through a follower lands within 10 s."""
    alice = ctx.accounts(cl, "alice")[0]
    app = alice.deploy(ctx.kv_wasm)
    alice.call(app, "set", {"key": "a", "value": "1"})
    leader, follower = find_leader(cl, app)
    cl.pause(leader)
    t0 = time.monotonic()
    alice.call(app, "set", {"key": "b", "value": "2"}, node=follower)
    took = time.monotonic() - t0
    ctx.note(f"write through node {follower} with leader {leader} frozen: {took:.2f}s")
    check(took <= 10, f"the write waited {took:.1f}s, over the 10 s budget")
    cl.resume(leader)
    alice.wait_for(app, "get", {"key": "b"}, "2", node=leader)


@test("orgs-and-keys", 7630, "workflow-examples/cloud-v2/orgs-and-keys.yml", smoke=True)
def orgs_and_keys(ctx, cl):
    """Organizations gate apps, scoped API keys, passkey confirmations, removal."""
    alice, bob, carol = ctx.accounts(cl, "alice", "bob", "carol")
    org = alice.cli("namespace", "create", "Acme")["orgId"]
    token = alice.raw("post", f"orgs/{org}/invites", {"role": "member", "teams": []})["token"]
    bob.raw("post", f"invites/{token}/redeem", {})
    app = alice.deploy(ctx.kv_wasm, "team-kv", org=org, confirm=["clear"])
    bob.call(app, "set", {"key": "from", "value": "bob"})  # a member writes
    expect_refused("NOT_FOUND", lambda: carol.call(app, "get", {"key": "from"}))  # not in the org
    # clear needs a fresh passkey assertion: refused without, done with alice's passkey.
    expect_refused("CONFIRM_REQUIRED", lambda: alice.cli("call", "--context", app, "clear"))
    alice.cli("call", "--context", app, "clear", "--dev-passkey", str(alice.passkey))
    # Minting a key needs a passkey assertion too: refused without, made with alice's.
    expect_refused("CONFIRM_REQUIRED", lambda: alice.cli("account", "api-key", "create", "--label", "service"))
    key = alice.cli("account", "api-key", "create", "--label", "service", "--read-only",
                    "--context", app, "--dev-passkey", str(alice.passkey))["key"]
    got = alice.view(app, "len", key=key)["result"]
    check(got == 0, f"len through the read-only key: {got!r}, want 0")
    expect_refused("FORBIDDEN", lambda: alice.call(app, "set", {"key": "x", "value": "y"}, key=key))
    alice.raw("delete", f"orgs/{org}/members/{bob.id}")
    expect_refused("NOT_FOUND", lambda: bob.call(app, "set", {"key": "x", "value": "y"}), retry_for=5)


@test("packages", 7635, "workflow-examples/cloud-v2/packages.yml", smoke=True)
def packages(ctx, cl):
    """Signed releases from the registry, verified on every node, upgraded by version, uninstalled."""
    alice = ctx.accounts(cl, "alice")[0]
    package, first = cl.registry.publish(ctx.kv_mpk)
    alice.raw("post", "packages/install", {"package": package, "version": first})
    app = alice.raw("post", "apps", {"name": "kv", "package": package, "version": first})["appId"]
    alice.call(app, "set", {"key": "k", "value": "v1"})
    for node in cl.ids:  # each node runs the app from its own verified copy
        got = alice.view(app, "get", {"key": "k"}, node=node, stale=True)["result"]
        check(got == "v1", f"node {node} answers {got!r}, want 'v1'")
    _, nxt = cl.registry.publish(ctx.kv_mpk_next)
    check(nxt != first, f"the next release is {nxt}, the same version as the first")
    alice.raw("post", "packages/install", {"package": package, "version": nxt})
    alice.cli("context", "update", app, "--version", nxt, "--dev-passkey", str(alice.passkey))
    got = alice.view(app, "get", {"key": "k"})["result"]
    check(got == "v1", f"after the upgrade: {got!r}, want 'v1'")
    try:
        alice.raw("delete", f"packages/{package}")
        raise Failed("a package an app still runs was uninstalled")
    except CliError:
        pass
    alice.raw("delete", f"apps/{app}")
    alice.raw("delete", f"packages/{package}")
    left = alice.raw("get", "packages")
    left = left.get("packages", left) if isinstance(left, dict) else left
    check(left == [], f"packages left: {left}")


@test("rolling-upgrade", 7640, "workflow-examples/cloud-v2/rolling-upgrade.yml")
def rolling_upgrade(ctx, cl):
    """Every node restarted on another merod build in turn, under a steady write load; nothing lost."""
    nxt = ctx.upgrade_merod(cl)
    check(Path(nxt).read_bytes() != Path(cl.binaries[1]).read_bytes(),
          "the upgrade is to the same build: a restart on one binary proves nothing")
    alice = ctx.accounts(cl, "alice")[0]
    app = alice.deploy(ctx.kv_wasm)
    alice.call(app, "set", {"key": "before", "value": "upgrade"})

    # The load: keyed writes, each retried with its key until it is answered.
    acked, stop, errors, retries = {}, threading.Event(), [], []

    def writer():
        n = 0
        while not stop.is_set():
            k, idem = f"load-{n}", uuid.uuid4().hex
            for _ in range(40):
                try:
                    alice.call(app, "set", {"key": k, "value": str(n)}, idem=idem, timeout=10)
                    acked[k] = str(n)
                    break
                except Exception as e:  # noqa: BLE001
                    retries.append(f"{type(e).__name__} {code_of(e) or str(e)[:80]}")
                    if not transport(e) and code_of(e) not in ("UNAVAILABLE", "TIMEOUT", "NOT_LEADER"):
                        errors.append(str(e)[:200])
                        break
                    time.sleep(0.25)
            n += 1

    def progress(more, timeout=30):
        """Wait until `more` further writes are acknowledged: the load runs, not stalls."""
        want, deadline = len(acked) + more, time.time() + timeout
        while len(acked) < want:
            check(time.time() < deadline, f"the load stalled: {len(acked)} acknowledged, waited for {want}")
            check(t.is_alive(), f"the writer stopped: {errors[:3]}")
            time.sleep(0.1)

    t = threading.Thread(target=writer, daemon=True)
    t.start()
    progress(10)
    for node in cl.ids:
        cl.restart(node, binary=nxt)
        alice.wait_for(app, "get", {"key": "before"}, "upgrade")  # through the mixed cluster
        # The node is back in the app's group before the next one goes down, and
        # writes keep landing meanwhile.
        deadline = time.time() + 30
        while not cl.app_status(node, app):
            check(time.time() < deadline, f"node {node} did not rejoin the app's group within 30 s")
            time.sleep(0.2)
        progress(10)
    stop.set()
    t.join(timeout=60)
    alice.call(app, "set", {"key": "after", "value": "upgrade"})  # writes on the new build
    apps = ops_status(cl)["apps"]
    check(apps == 1, f"ops status counts {apps} apps, want 1")
    check(not errors, f"refused writes under the upgrade: {errors[:3]}")
    entries = alice.view(app, "entries")["result"]
    lost = {k: v for k, v in acked.items() if entries.get(k) != v}
    ctx.note(f"{len(acked)} acknowledged writes during the upgrade, {len(retries)} retried answers"
             + (f" (first: {sorted(set(retries))[:3]})" if retries else ""))
    check(len(acked) > 0, "no write was acknowledged during the upgrade")
    check(not lost, f"acknowledged writes lost: {list(lost)[:10]}")


# The kv-store fuzzy load: workflows/fuzzy-tests/kv-store/scenarios/fuzzy-test.json, the same
# operations and weights, timed per call as merobox's fuzzy_test did.
SOAK_NAME = "KV Store Fuzzy Load Test"
SOAK_ACCOUNTS = ["fuzzy-kv-node-1", "fuzzy-kv-node-2", "fuzzy-kv-node-3", "fuzzy-kv-node-4"]


@test("kv-soak", 7650, "workflows/fuzzy-tests/kv-store/scenarios/fuzzy-test.json (fuzzy_test)", smoke=True)
def kv_soak(ctx, cl):
    """kv-store's fuzzy load across 4 members for --soak-seconds; per-call latency and throughput report."""
    accts = ctx.accounts(cl, *SOAK_ACCOUNTS)
    owner = accts[0]
    org = owner.cli("namespace", "create", "mesh")["orgId"]
    for a in accts[1:]:
        token = owner.raw("post", f"orgs/{org}/invites", {"role": "member", "teams": []})["token"]
        a.raw("post", f"invites/{token}/redeem", {})
    app = owner.deploy(ctx.kv_wasm, "Create Mesh for KV Store", org=org)
    for i in range(10):
        owner.call(app, "set", {"key": f"initial_key_{i}", "value": f"initial_value_{i}"})

    rng = random.Random(ctx.seed)
    by_name = {a.name: a for a in accts}
    metrics = soak.SoakMetrics()

    def timed(op, who, method, args):
        t0 = time.perf_counter()
        ok = False
        try:
            out = who.call(app, method, args)
            ok = True
            return out["result"]
        finally:
            took = time.perf_counter() - t0
            busy[0] += took
            metrics.call(op, f"call {method}", took, ok)

    def rkey():
        return f"key_{rng.randint(1, 10000)}"

    def value(tag):
        return f"{tag}_{uuid.UUID(int=rng.getrandbits(128))}_{int(time.time())}"

    def set_and_verify(op):
        k, v = rkey(), value("value")
        timed(op, rng.choice(accts), "set", {"key": k, "value": v})
        got = timed(op, rng.choice(accts), "get", {"key": k})
        check(got is not None and v in str(got), f"get {k}: {got!r}, want {v!r}")

    def write_burst(op):
        for i in range(12):
            timed(op, accts[i % 4], "set", {"key": rkey(), "value": value("burst")})

    def random_read(op):
        timed(op, rng.choice(accts), "get", {"key": rkey()})

    def list_entries(op):
        timed(op, rng.choice(accts), "entries", {})

    def cross_node_write_read(op):
        k, v = f"consistency_{uuid.UUID(int=rng.getrandbits(128))}", f"check_{int(time.time())}"
        timed(op, by_name["fuzzy-kv-node-1"], "set", {"key": k, "value": v})
        time.sleep(1.0)  # the workflow's 2 s wait, capped at 1 s as merobox did
        got = timed(op, by_name["fuzzy-kv-node-3"], "get", {"key": k})
        check(got is not None and v in str(got), f"cross-node get {k}: {got!r}, want {v!r}")

    ops = [("set_and_verify", 40, set_and_verify), ("write_burst", 25, write_burst),
           ("random_read", 15, random_read), ("list_entries", 10, list_entries),
           ("cross_node_write_read", 3, cross_node_write_read)]
    weights = [w for _, w, _ in ops]
    threshold = 99.0
    failures = {}
    deadline = time.time() + ctx.soak_seconds
    while time.time() < deadline:
        name, _, f = rng.choices(ops, weights)[0]
        busy = [0.0]
        try:
            f(name)
            metrics.operation(name, busy[0], True)
        except Exception as e:  # noqa: BLE001
            metrics.operation(name, busy[0], False)
            why = f"{name}: {str(e)[:120]}"
            failures[why] = failures.get(why, 0) + 1
    metrics.stop()
    final = owner.call(app, "len")["result"]
    check(final is not None, "final len unanswered")
    summary = metrics.summary()
    for op, s in summary["operations"].items():
        lat = s["calls"]["latency_ms"]
        ctx.note(f"{op}: {s['attempts']} ops, {s['calls']['count']} calls ({s['calls']['per_s']:.1f}/s), "
                 f"p50 {lat['p50']:.1f} ms, p95 {lat['p95']:.1f} ms, p99 {lat['p99']:.1f} ms")
    for why, n in sorted(failures.items(), key=lambda x: -x[1])[:5]:
        ctx.note(f"{n}x {why}")
    if ctx.soak_report:
        soak.append_report(ctx.soak_report, {
            "workflow": SOAK_NAME,
            "file": "crates/node/tests/fault/suite.py::kv-soak",
            "step": SOAK_NAME,
            "nodes": cl.n,
            "duration_requested": ctx.soak_seconds,
            "success_threshold": threshold,
            **summary,
        })
    check(summary["operations_total"] > 0, "no operation ran")
    check(summary["pass_rate"] >= threshold,
          f"{summary['pass_rate']:.1f}% of {summary['operations_total']} operations passed, under {threshold}%")


@test("rooms-fault", 7610, "(core) rooms_fault.py: randomized nemesis, linearizability, idempotency", pr=False)
def rooms_fault(ctx, cl):
    """Rooms under random kills and freezes of leader and followers (rooms_fault.py, A4)."""
    cl.stop()  # the fault test starts its own cluster on this test's ports
    # Its base port before WORKFLOW_PORT_OFFSET, which rooms_fault.py adds itself.
    args = [sys.executable, "-I", str(HERE / "rooms_fault.py"), "--iterations", str(ctx.rooms_iterations),
            "--seed", str(ctx.seed), "--workdir", str(cl.workdir / "rooms"),
            "--base-port", str(TESTS["rooms-fault"]["port"])]
    if ctx.rooms_wasm:
        args += ["--wasm", ctx.rooms_wasm]
    env = dict(os.environ, RV2_MEROD=rv2.MEROD, RV2_MEROCTL=rv2.CLI)
    out = subprocess.run(args, capture_output=True, text=True, env=env, check=False)
    tail = out.stdout.strip().splitlines()[-3:]
    for line in tail:
        ctx.note(line[:300])
    done = [ln for ln in out.stdout.splitlines() if ln.startswith("DONE")]
    check(out.returncode == 0 and done, f"rooms_fault.py exited {out.returncode}: {out.stderr[-400:]}")
    check("failed_iterations=0" in done[-1], done[-1])


# ----------------------------------------------------------------- runner


class Ctx:
    def __init__(self, args):
        self.args = args
        self.seed = args.seed
        self.offset = int(os.environ.get("WORKFLOW_PORT_OFFSET", "0"))
        self.workroot = Path(args.workdir)
        self.kv_wasm = args.kv_wasm
        self.kv_mpk = args.kv_mpk
        self.kv_mpk_next = args.kv_mpk_next
        self.rooms_wasm = args.rooms_wasm
        self.soak_seconds = args.soak_seconds
        self.soak_report = args.soak_report
        self.rooms_iterations = args.rooms_iterations
        self.notes = []

    def port_of(self, name):
        return TESTS[name]["port"] + self.offset

    def note(self, s):
        self.notes.append(s)
        print(f"    {s}", flush=True)

    def accounts(self, cl, *names):
        return [Account(cl, n, i).sign_up() for i, n in enumerate(names)]

    def upgrade_merod(self, cl):
        """The build nodes move to: --upgrade-merod, else a copy of merod with one byte
        appended (CI's merod-next: another file, the same code)."""
        if self.args.upgrade_merod:
            return self.args.upgrade_merod
        nxt = cl.workdir / "merod-next"
        shutil.copyfile(rv2.MEROD, nxt)
        with open(nxt, "ab") as f:
            f.write(b"\0")
        nxt.chmod(0o755)
        return str(nxt)


def need(path, what, how):
    if not path or not Path(path).exists():
        raise SystemExit(f"{what} not found ({path}): {how}")


def default_rooms():
    found = sorted((TARGET / "app-components").glob("rooms-*.wasm"), key=lambda p: p.stat().st_mtime)
    return str(found[-1]) if found else None


def run_one(ctx, name, iteration):
    spec = TESTS[name]
    base = ctx.port_of(name)
    if not (7600 <= spec["port"] and spec["port"] + 3 <= 7699):
        raise SystemExit(f"{name}: ports 7600-7699 only (before WORKFLOW_PORT_OFFSET)")
    wd = ctx.workroot / f"{name}-{iteration}"
    shutil.rmtree(wd, ignore_errors=True)
    wd.mkdir(parents=True)
    registry = PackageRegistry(wd / "registry") if name == "packages" else None
    cl = Cluster(wd, base_port=base, cluster=f"suite-{name}", binary=rv2.MEROD, registry=registry)
    ctx.notes = []
    t0 = time.time()
    status, error = "pass", None
    try:
        if name != "rooms-fault":
            cl.start()
        spec["f"](ctx, cl)
    except Exception as e:  # noqa: BLE001
        status, error = "fail", f"{type(e).__name__}: {e}"
        if not isinstance(e, Failed):
            traceback.print_exc()
    finally:
        cl.stop()
        if registry:
            registry.stop()
    secs = round(time.time() - t0, 1)
    if status == "pass" and not ctx.args.keep:
        shutil.rmtree(wd, ignore_errors=True)
    return {"test": name, "iteration": iteration, "status": status, "seconds": secs,
            "error": error, "notes": ctx.notes, "replaces": spec["replaces"], "workdir": str(wd)}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="list the tests and exit")
    ap.add_argument("--only", help="comma-separated test names")
    ap.add_argument("--smoke", action="store_true", help="the release-image set (tests marked smoke), short soak")
    ap.add_argument("--nightly", action="store_true",
                    help="every test, a 900 s soak, rooms-fault 50 iterations, 3 iterations of each")
    ap.add_argument("--iterations", type=int, help="run each test this many times (default 1, nightly 3)")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--merod", help="the merod to run (default RV2_MEROD or target/release/merod)")
    ap.add_argument("--meroctl", help="the meroctl to drive it with (default RV2_MEROCTL or target/release/meroctl)")
    ap.add_argument("--upgrade-merod", help="rolling-upgrade's target build (default: merod with a byte appended)")
    ap.add_argument("--kv-wasm", default=os.environ.get("KV_STORE_WASM")
                    or str(TARGET / "wasm32-unknown-unknown" / "app-release" / "kv_store.wasm"))
    ap.add_argument("--kv-mpk", default=os.environ.get("KV_STORE_MPK")
                    or str(ROOT / "dist" / "com.calimero.kv-store-1.0.0.mpk"))
    ap.add_argument("--kv-mpk-next", default=os.environ.get("KV_STORE_MPK_NEXT")
                    or str(ROOT / "dist" / "com.calimero.kv-store-1.1.0.mpk"))
    ap.add_argument("--rooms-wasm", default=None, help="the Rooms component (default: target/app-components)")
    ap.add_argument("--rooms-iterations", type=int, help="rooms-fault's iterations (default 5, nightly 50)")
    ap.add_argument("--soak-seconds", type=float,
                    help="kv-soak's duration (default SOAK_SECONDS or MEROBOX_FUZZY_SECONDS, else 30; nightly 900)")
    ap.add_argument("--soak-report", default=os.environ.get("SOAK_REPORT") or os.environ.get("MEROBOX_SOAK_REPORT"),
                    help="append kv-soak's report here (soak.py's format)")
    ap.add_argument("--json", help="write the results here")
    ap.add_argument("--workdir", default=str(TARGET / "fault-suite"))
    ap.add_argument("--keep", action="store_true", help="keep the work directories of passing tests")
    args = ap.parse_args(argv)

    if args.list:
        for name, s in TESTS.items():
            sets = ",".join(x for x, on in (("pr", s["pr"]), ("smoke", s["smoke"]), ("nightly", True)) if on)
            print(f"{name:16} [{sets}] {s['doc']}\n{'':16} replaces {s['replaces']}")
        return 0

    if args.merod:
        rv2.MEROD = str(Path(args.merod).resolve())
    if args.meroctl:
        rv2.CLI = str(Path(args.meroctl).resolve())
    env_secs = os.environ.get("SOAK_SECONDS") or os.environ.get("MEROBOX_FUZZY_SECONDS")
    if args.soak_seconds is None:
        args.soak_seconds = float(env_secs) if env_secs else (900.0 if args.nightly else 30.0)
    if args.iterations is None:
        args.iterations = 3 if args.nightly else 1
    if args.rooms_iterations is None:
        args.rooms_iterations = 50 if args.nightly else 5
    if args.only:
        names = [n.strip() for n in args.only.split(",") if n.strip()]
        unknown = [n for n in names if n not in TESTS]
        if unknown:
            ap.error(f"unknown tests {unknown}; --list shows them")
    elif args.smoke:
        names = [n for n, s in TESTS.items() if s["smoke"]]
    elif args.nightly:
        names = list(TESTS)
    else:
        names = [n for n, s in TESTS.items() if s["pr"]]

    need(rv2.MEROD, "merod", "cargo build --release -p merod, or --merod PATH")
    need(rv2.CLI, "meroctl", "cargo build --release -p meroctl, or --meroctl PATH")
    if set(names) - {"rooms-fault"}:
        need(args.kv_wasm, "kv_store.wasm",
             "cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release, or --kv-wasm")
    if "packages" in names:
        how = "(cd apps/wasmer-fixtures/kv-store && cargo mero bundle --dev --no-icon --app-version 1.0.0 / 1.1.0)"
        need(args.kv_mpk, "kv-store 1.0.0 .mpk", how + ", or --kv-mpk")
        need(args.kv_mpk_next, "kv-store 1.1.0 .mpk", how + ", or --kv-mpk-next")
    if "rooms-fault" in names:
        args.rooms_wasm = args.rooms_wasm or default_rooms()
        need(args.rooms_wasm, "the Rooms component", "cargo test --release -p calimero-node --test rooms")

    ctx = Ctx(args)
    print(f"merod {rv2.MEROD}\nmeroctl {rv2.CLI}\ntests {names} x{args.iterations}, "
          f"soak {args.soak_seconds:.0f}s, ports +{ctx.offset}", flush=True)
    results = []
    t0 = time.time()
    for it in range(args.iterations):
        for name in names:
            print(f"== {name} (iteration {it + 1}/{args.iterations})", flush=True)
            r = run_one(ctx, name, it)
            results.append(r)
            print(f"   {r['status'].upper()} {name} in {r['seconds']}s" + (f": {r['error']}" if r["error"] else ""),
                  flush=True)
    failed = [r for r in results if r["status"] != "pass"]
    total = round(time.time() - t0, 1)
    print(f"DONE {len(results) - len(failed)}/{len(results)} passed in {total}s"
          + (f"; failed: {sorted({r['test'] for r in failed})}" if failed else ""), flush=True)
    if args.json:
        Path(args.json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json).write_text(json.dumps({"merod": rv2.MEROD, "seconds": total, "results": results},
                                              indent=2) + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
