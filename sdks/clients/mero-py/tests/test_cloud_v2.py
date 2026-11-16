"""
CloudClient against a real 3-node merod cluster (Calimero 1.0).

Needs `merod` and the kv-store fixture from the core the client builds against
(Cargo.toml's pin):

    cargo build -p merod
    cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release

Paths default to a sibling `core` checkout; override with MEROD_BINARY (or
MEROD_V2_BINARY) and KV_STORE_WASM. Skipped when the binary is missing.
"""

import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

import pytest

from calimero_client_py import CloudClient, CloudError, SoftPasskey

CORE = Path(__file__).resolve().parents[2] / "core"
BINARY = (
    os.environ.get("MEROD_BINARY")
    or os.environ.get("MEROD_V2_BINARY")
    or str(CORE / "target/debug/merod")
)
KV_STORE_WASM = os.environ.get(
    "KV_STORE_WASM",
    str(CORE / "target/wasm32-unknown-unknown/app-release/kv_store.wasm"),
)
ORIGIN = "http://localhost:3000"

pytestmark = pytest.mark.skipif(
    not (Path(BINARY).exists() and Path(KV_STORE_WASM).exists()),
    reason="merod or the kv-store fixture is not built",
)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Cluster:
    def __init__(self, n: int = 3, flags=()):
        """`flags` are added to every node's `merod run`."""
        ports = [free_port() for _ in range(n)]
        self.urls = [f"http://127.0.0.1:{p}" for p in ports]
        peers = ",".join(f"{i + 1}={u}/" for i, u in enumerate(self.urls))
        logs = None if os.environ.get("MEROD_V2_LOGS") else subprocess.DEVNULL
        self.procs = []
        self.dirs = [tempfile.mkdtemp(prefix="merod-") for _ in ports]
        for i, (port, url) in enumerate(zip(ports, self.urls)):
            # `merod run FLAGS`: a node from flags alone, no `init`.
            args = [
                BINARY,
                "run",
                "--node-id",
                str(i + 1),
                "--listen",
                f"127.0.0.1:{port}",
                "--public-url",
                f"{url}/",
                "--data-dir",
                self.dirs[i],
                "--peers",
                peers,
                "--cluster",
                "py-test",
                "--dev",
                "--origins",
                ORIGIN,
            ]
            args += list(flags)
            if i == 0:
                args.append("--bootstrap")
            self.procs.append(subprocess.Popen(args, stdout=logs, stderr=logs))
        deadline = time.time() + 30
        while True:
            try:
                with urllib.request.urlopen(f"{self.urls[0]}/health", timeout=1) as r:
                    if json.load(r).get("systemLeader"):
                        break
            except OSError:
                pass
            if time.time() > deadline:
                self.stop()
                raise RuntimeError("cluster did not start")
            time.sleep(0.2)
        # Let the leader register nodes and mint the first session key.
        time.sleep(1.5)

    def client(self, i: int, allow_mock: bool = True) -> CloudClient:
        return CloudClient(self.urls[i], allow_mock=allow_mock)

    def stop(self, i=None):
        for j in range(len(self.procs)) if i is None else [i]:
            p = self.procs[j]
            if p is not None:
                p.kill()
                p.wait()
                self.procs[j] = None
        # The whole cluster stopped: its data goes too (a single node stopped
        # for a failover test keeps its directory).
        if i is None:
            for d in self.dirs:
                shutil.rmtree(d, ignore_errors=True)


@pytest.fixture(scope="module")
def cluster():
    c = Cluster(3)
    yield c
    c.stop()


@pytest.fixture(scope="module")
def wasm() -> bytes:
    return Path(KV_STORE_WASM).read_bytes()


def person(cluster, i, name):
    c = cluster.client(i)
    pk = SoftPasskey("localhost", ORIGIN)
    c.sign_up(pk, name)
    return c, pk


def code(e: CloudError) -> str:
    return e.args[2]


def test_refuses_a_mock_quote_unless_allowed(cluster):
    with pytest.raises(CloudError, match="mock quote refused"):
        cluster.client(0, allow_mock=False).post("auth/passkey/login/begin")


def test_sign_up_deploy_call_view_confirm(cluster, wasm):
    alice, pk = person(cluster, 0, "alice")
    me = alice.json.auth.me()
    assert me["via"] == "session"

    # The same passkey signs in on another node.
    again = cluster.client(1)
    s = again.sign_in(pk)
    assert s["accountId"] == me["accountId"]

    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    # `confirmMethods` is not in core's document for `POST /apps`, so the
    # facade cannot send it: the generic path call does.
    app = alice.post(
        "apps", {"name": "kv", "bundle": bundle, "confirmMethods": ["clear"]}
    )["appId"]

    r = alice.json.apps.call(
        app=app, method="set", args={"key": "a", "value": "1"}, idempotency_key="k1"
    )
    assert r["index"] > 0
    assert len(r["events"]) == 1
    replay = alice.json.apps.call(
        app=app, method="set", args={"key": "a", "value": "other"}, idempotency_key="k1"
    )
    assert replay["index"] == r["index"]

    assert (
        again.json.apps.view(app=app, method="get", args={"key": "a"})["result"] == "1"
    )
    assert (
        again.json.apps.view(
            app=app,
            method="get",
            args={"key": "a"},
            consistency="stale",
            min_index=r["index"],
        )["result"]
        == "1"
    )

    events = alice.events(app, limit=1, timeout=5)
    assert len(events) == 1 and events[0]["kind"]

    # A confirm method needs a fresh passkey assertion over the exact call.
    with pytest.raises(CloudError) as e:
        alice.json.apps.call(app=app, method="clear", args={})
    assert code(e.value) == "CONFIRM_REQUIRED"
    alice.json.apps.call(app=app, method="clear", args={}, confirm=pk)
    assert alice.json.apps.view(app=app, method="len")["result"] == 0

    # Others cannot reach an owner-only app.
    bob, _ = person(cluster, 2, "bob")
    with pytest.raises(CloudError) as e:
        bob.json.apps.call(app=app, method="set", args={"key": "x", "value": "y"})
    assert code(e.value) == "NOT_FOUND"

    # A read-only API key reads, never writes.
    # An API key is approved with the account's passkey (412 first).
    with pytest.raises(CloudError) as e:
        alice.post("auth/apikeys", {"readOnly": True, "apps": [app]})
    assert code(e.value) == "CONFIRM_REQUIRED"
    key = alice.json.auth.apikey_create(read_only=True, apps=[app], confirm=pk)["key"]
    agent = cluster.client(2)
    agent.use_api_key(key)
    agent.json.apps.view(app=app, method="len")
    with pytest.raises(CloudError) as e:
        agent.json.apps.call(app=app, method="set", args={"key": "x", "value": "y"})
    assert code(e.value) == "FORBIDDEN"

    # Refresh rotates the session, and a saved session can be reused.
    fresh = alice.refresh()
    assert fresh["sessionToken"] != s["sessionToken"]
    later = cluster.client(1)
    later.use_session(fresh)
    assert later.json.auth.me()["accountId"] == me["accountId"]


def test_organizations_gate_apps(cluster, wasm):
    alice, alice_pk = person(cluster, 0, "alice-org")
    bob, _ = person(cluster, 1, "bob-org")
    bob_id = bob.json.auth.me()["accountId"]
    org = alice.post("orgs", {"name": "Acme"})["orgId"]
    team = alice.post(f"orgs/{org}/teams", {"name": "eng", "join": "invite"})["teamId"]
    # An invite grants access: the node asks for the inviter's passkey first.
    with pytest.raises(CloudError) as e:
        alice.post(f"orgs/{org}/invites", {"role": "member", "teams": [team]})
    assert code(e.value) == "CONFIRM_REQUIRED"
    token = alice.post_confirmed(
        f"orgs/{org}/invites", {"role": "member", "teams": [team]}, alice_pk
    )["token"]
    bob.post(f"invites/{token}/redeem")

    app = alice.json.apps.create(
        name="team-kv",
        bundle=alice.json.packages.bundle_put(body=wasm)["hash"],
        **{"org": org},
    )["appId"]
    bob.json.apps.call(app=app, method="set", args={"key": "from", "value": "bob"})

    # A DELETE with no body is asked for the passkey; one carrying a proof
    # (here a stale one) is judged on that proof: the body reaches the node.
    with pytest.raises(CloudError) as e:
        alice.delete(f"orgs/{org}/members/{bob_id}")
    assert code(e.value) == "CONFIRM_REQUIRED"
    stale = {"confirm": {"challengeId": "1.none", "assertion": {}}}
    with pytest.raises(CloudError) as e:
        alice.delete(f"orgs/{org}/members/{bob_id}", stale)
    assert code(e.value) != "CONFIRM_REQUIRED", e.value.args
    alice.delete_confirmed(f"orgs/{org}/members/{bob_id}", alice_pk)
    time.sleep(0.3)
    with pytest.raises(CloudError) as e:
        bob.json.apps.call(app=app, method="set", args={"key": "x", "value": "y"})
    assert code(e.value) == "NOT_FOUND"


def test_blobs_belong_to_their_app(cluster, wasm):
    alice, _pk = person(cluster, 0, "alice-blobs")
    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    app = alice.json.apps.create(name="files", bundle=bundle)["appId"]
    other = alice.json.apps.create(name="other", bundle=bundle)["appId"]
    data = b"%PDF-1.7 " * 500
    up = alice.json.apps.blob_upload(app=app, body=data)
    assert up["size"] == len(data)
    assert alice.json.apps.blob_download(app=app, id=up["blobId"]) == data
    assert alice.json.apps.blob_download(app=app, id=up["id"]) == data
    with pytest.raises(CloudError) as e:
        alice.json.apps.blob_download(app=other, id=up["blobId"])
    assert code(e.value) == "NOT_FOUND"


def test_a_second_passkey_is_another_device(cluster):
    laptop, laptop_key = person(cluster, 0, "alice-devices")
    phone_key = SoftPasskey()
    # Approved by a passkey the account already has, never the new one.
    with pytest.raises(ValueError):
        laptop.add_passkey(phone_key, phone_key)
    cred = laptop.add_passkey(phone_key, laptop_key)
    phone = cluster.client(1)
    phone.sign_in(phone_key)
    a, b = laptop.json.auth.me(), phone.json.auth.me()
    assert a["accountId"] == b["accountId"]
    assert b["passkeyId"] == cred != a["passkeyId"]


def test_keeps_serving_after_a_node_dies(cluster, wasm):
    alice, pk = person(cluster, 1, "alice-failover")
    app = alice.json.apps.create(
        name="kv-ha", bundle=alice.json.packages.bundle_put(body=wasm)["hash"]
    )["appId"]
    alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})
    cluster.stop(0)
    other = cluster.client(2)
    other.sign_in(pk)
    for _ in range(40):
        try:
            other.json.apps.call(
                app=app,
                method="set",
                args={"key": "k2", "value": "v2"},
                idempotency_key="after-failover",
            )
            break
        except CloudError:
            time.sleep(0.25)
    else:
        pytest.fail("no write after failover")
    assert (
        other.json.apps.view(app=app, method="get", args={"key": "k"})["result"] == "v"
    )
