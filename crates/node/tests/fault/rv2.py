"""Black-box helpers for a release-v2 (`merod run --dev`) cluster, one process per node.

Adapted from the release-v2 review's tests/raft/rv2.py. Ports 7600-7699, above every
merobox workflow's, so a fault run and the workflows can share a machine. Plain HTTP
is allowed by --dev (sealed_required = !dev, crates/merod/src/main.rs). Auth uses an
API key minted through meroctl with a file passkey. The binaries are the workspace's
release builds (in `RV2_BIN_DIR`, default `target/release`):

    cargo build --release -p merod -p meroctl
"""

from __future__ import annotations

import atexit
import base64
import hashlib
import hmac
import json
import os
import shutil
import signal
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
BIN = Path(os.environ.get("RV2_BIN_DIR", ROOT / "target" / "release"))
MEROD = str(BIN / "merod")
CLI = str(BIN / "meroctl")
ORIGIN = "http://localhost:3000"


def b64url(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def dev_storage_key(cluster: str) -> bytes:
    # crates/node/src/main.rs: sha256("calimero-dev-storage-key|{cluster}")
    return hashlib.sha256(f"calimero-dev-storage-key|{cluster}".encode()).digest()


def cluster_secret(cluster: str) -> str:
    # crates/node/src/node.rs Node::start: HMAC-SHA256(storage_key, "calimero.cluster.v1|{cluster}")
    mac = hmac.new(dev_storage_key(cluster), f"calimero.cluster.v1|{cluster}".encode(), hashlib.sha256)
    return b64url(mac.digest())


class HttpError(Exception):
    def __init__(self, status, body):
        super().__init__(f"{status}: {body}")
        self.status = status
        self.body = body


def http(method: str, url: str, body=None, headers=None, timeout=30.0):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            b = json.loads(raw)
        except Exception:
            b = raw.decode(errors="replace")
        raise HttpError(e.code, b) from None


_LIVE: set = set()


def stop_all():
    """Kills every node any Cluster of this process started (also run at exit and on a signal)."""
    for cluster in list(_LIVE):
        cluster.stop()
    _LIVE.clear()


def _on_signal(signum, _frame):
    stop_all()
    raise SystemExit(128 + signum)


atexit.register(stop_all)
signal.signal(signal.SIGTERM, _on_signal)
signal.signal(signal.SIGINT, _on_signal)
signal.signal(signal.SIGHUP, _on_signal)


class Cluster:
    def __init__(self, workdir: Path, base_port: int = 7600, n: int = 3, cluster: str = "rv2raft", log=True):
        self.workdir = Path(workdir)
        self.base_port = base_port
        self.n = n
        self.cluster = cluster
        self.ops_token = "opsrv2raft" + "0" * 22
        self.procs: dict[int, subprocess.Popen | None] = {i: None for i in self.ids}
        self.paused: set[int] = set()
        self.log = log
        self.secret = cluster_secret(cluster)
        self.node_env: dict[int, dict] = {}

    @property
    def ids(self):
        return list(range(1, self.n + 1))

    def url(self, i):
        return f"http://127.0.0.1:{self.base_port + i}/"

    def dir(self, i):
        return self.workdir / f"node{i}"

    def args(self, i):
        peers = ",".join(f"{j}={self.url(j)}" for j in self.ids)
        a = [MEROD, "run", "--node-id", str(i), "--listen", f"127.0.0.1:{self.base_port + i}",
             "--public-url", self.url(i), "--data-dir", str(self.dir(i)), "--peers", peers,
             "--cluster", self.cluster, "--dev", "--origins", ORIGIN, "--ops-token", self.ops_token]
        if i == 1:
            a.append("--bootstrap")
        return a

    def start_node(self, i, wait=True):
        assert self.procs.get(i) is None
        self.dir(i).mkdir(parents=True, exist_ok=True)
        out = open(self.workdir / f"node{i}.log", "ab") if self.log else subprocess.DEVNULL
        env = dict(os.environ)
        env.update(self.node_env.get(i, {}))
        # Its own session: a signal to the runner's process group does not reach the node, and
        # stop_all (atexit, SIGINT, SIGTERM) kills every child the runner started.
        self.procs[i] = subprocess.Popen(self.args(i), stdout=out, stderr=out, env=env,
                                         start_new_session=True)
        _LIVE.add(self)
        if wait:
            self.wait_healthy(i)

    def start(self, timeout=60):
        for i in self.ids:
            self.start_node(i, wait=False)
        deadline = time.time() + timeout
        while time.time() < deadline:
            hs = [self.health(i) for i in self.ids]
            if all(hs) and all(h.get("systemLeader") for h in hs):
                time.sleep(1.5)
                return
            time.sleep(0.2)
        raise TimeoutError("cluster did not form")

    def kill(self, i):
        p = self.procs.get(i)
        if p is None:
            return
        try:
            p.send_signal(signal.SIGCONT)
        except ProcessLookupError:
            pass
        p.kill()
        p.wait()
        self.procs[i] = None
        self.paused.discard(i)

    def pause(self, i):
        p = self.procs.get(i)
        if p is not None:
            p.send_signal(signal.SIGSTOP)
            self.paused.add(i)

    def resume(self, i):
        p = self.procs.get(i)
        if p is not None:
            p.send_signal(signal.SIGCONT)
        self.paused.discard(i)

    def stop(self):
        for i in self.ids:
            self.kill(i)

    def running(self):
        return [i for i, p in self.procs.items() if p is not None and p.poll() is None]

    def health(self, i):
        try:
            return http("GET", self.url(i) + "health", timeout=1)
        except Exception:
            return None

    def wait_healthy(self, i, timeout=30):
        deadline = time.time() + timeout
        while time.time() < deadline:
            p = self.procs.get(i)
            if p is not None and p.poll() is not None:
                raise RuntimeError(f"node {i} exited {p.returncode}")
            if self.health(i):
                return
            time.sleep(0.2)
        raise TimeoutError(f"node {i} not healthy")

    def ops_status(self, i, timeout=2):
        return http("GET", self.url(i) + "ops/status", headers={"authorization": f"Bearer {self.ops_token}"}, timeout=timeout)

    def app_status(self, i, app):
        try:
            s = self.ops_status(i)
        except Exception:
            return None
        for g in s.get("groups", []):
            if g["app"] == app:
                return g["status"]
        return None

    def app_leader(self, app):
        """The node that claims leadership of the app group (highest term wins)."""
        best = None
        for i in self.running():
            if i in self.paused:
                continue
            st = self.app_status(i, app)
            if st and st.get("is_leader"):
                if best is None or st["term"] > best[1]:
                    best = (i, st["term"])
        return best[0] if best else None

    def raft_rpc(self, i, group, rpc, timeout=5):
        return http("POST", self.url(i) + "internal/raft", {"group": group, "rpc": rpc},
                    headers={"x-cluster-auth": self.secret}, timeout=timeout)

    # ---- CLI-driven setup (meroctl; its saved sessions under the run's own home)
    def cli(self, i, *args, env_extra=None):
        env = dict(os.environ)
        env.pop("CALIMERO_API_KEY", None)
        env.update(env_extra or {})
        out = subprocess.run([CLI, "--home", str(self.workdir / "cfg"), "--api", self.url(i),
                              "--allow-mock", *args], env=env,
                             capture_output=True, text=True, timeout=120)
        if out.returncode != 0:
            raise RuntimeError(f"cli {args}: {out.stderr}")
        return json.loads(out.stdout)


class Client:
    def __init__(self, cluster: Cluster, app: str, key: str):
        self.c = cluster
        self.app = app
        self.h = {"authorization": f"Bearer {key}"}

    def call(self, node, method, args, idem=None, timeout=30):
        body = {"method": method, "args": args}
        if idem:
            body["idempotencyKey"] = idem
        return http("POST", self.c.url(node) + f"apps/{self.app}/intents", body, self.h, timeout)

    def view(self, node, method, args, stale=False, timeout=30):
        body = {"method": method, "args": args}
        if stale:
            body["consistency"] = "stale"
        return http("POST", self.c.url(node) + f"apps/{self.app}/views", body, self.h, timeout)


def copy_dir(src: Path, dst: Path):
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)
