"""
The Ops API through CloudClient: what mdma (and any operator) runs against a
merod 1.0 cluster with the node's ops token.

Same requirements as test_cloud_v2.py; its own cluster, started with an ops
token, the operator's key, a backup directory and every decision given to the
operator, so the operator's release, outbound and upgrade calls are allowed.
Every operator decision is signed with that key (`use_operator_key`).
"""

import tempfile
import time
from pathlib import Path

import pytest

from calimero_client_py import Client, CloudError
from test_cloud_v2 import KV_STORE_WASM, Cluster, code, person, pytestmark  # noqa: F401

OPS_TOKEN = "py-test-ops-token"
# The operator's Ed25519 seed; the nodes get its verify key (--operator-key).
OPERATOR_SEED = "11" * 32


@pytest.fixture(scope="module")
def cluster():
    backups = tempfile.mkdtemp(prefix="merod-backup-")
    c = Cluster(
        3,
        flags=[
            "--ops-token",
            OPS_TOKEN,
            "--operator-key",
            Client.operator_public_key(OPERATOR_SEED),
            "--backup-dir",
            backups,
            "--release-approvals",
            "operator",
            "--app-upgrades",
            "operator",
            "--outbound-approvals",
            "operator",
        ],
    )
    yield c
    c.stop()


@pytest.fixture(scope="module")
def wasm() -> bytes:
    return Path(KV_STORE_WASM).read_bytes()


def operator(cluster, i: int = 0, signed: bool = True) -> Client:
    c = Client(cluster.urls[i], allow_mock=True)
    c.use_api_key(OPS_TOKEN)
    if signed:
        c.use_operator_key(OPERATOR_SEED)
    return c


def test_decisions_need_the_operators_signature(cluster):
    unsigned = operator(cluster, signed=False)
    name, value = next(
        (k, v) for k, v in unsigned.json.ops.quotas_get().items() if isinstance(v, int)
    )
    with pytest.raises(CloudError) as e:
        unsigned.json.ops.quotas_set(**{name: value})
    assert (e.value.args[1], code(e.value)) == (428, "OPERATOR_SIGNATURE_REQUIRED")
    # A signature by another key is refused.
    with pytest.raises(CloudError) as e:
        unsigned.post_operator_signed(
            "ops/quotas", {name: value}, operator_key="22" * 32
        )
    assert e.value.args[1] == 403, e.value.args
    unsigned.post_operator_signed("ops/quotas", {name: value}, OPERATOR_SEED)


def test_status_metrics_and_quotas(cluster):
    ops = operator(cluster)
    status = ops.json.ops.status()
    assert sorted(n["id"] for n in status["nodes"]) == [1, 2, 3], status
    assert status["governance"]["appUpgrades"] == "operator"
    assert ops.json.ops.metrics().strip()

    quotas = ops.json.ops.quotas_get()
    name, value = next((k, v) for k, v in quotas.items() if isinstance(v, int))
    ops.json.ops.quotas_set(**{name: value + 1})
    after = ops.json.ops.quotas_get()
    assert after[name] == value + 1
    assert {k: v for k, v in after.items() if k != name} == {
        k: v for k, v in quotas.items() if k != name
    }
    with pytest.raises(CloudError) as e:
        ops.json.ops.quotas_set(**{"no_such_quota": 1})
    assert code(e.value) == "UNKNOWN_QUOTA"


def test_the_ops_token_is_required(cluster):
    alice, _pk = person(cluster, 0, "alice-not-ops")
    with pytest.raises(CloudError) as e:
        alice.json.ops.status()
    assert e.value.args[1] in (401, 403), e.value.args


def test_replica_moves_follow_the_leader(cluster, wasm):
    alice, _pk = person(cluster, 0, "alice-ops")
    app = alice.json.apps.create(
        name="moved", bundle=alice.json.packages.bundle_put(body=wasm)["hash"]
    )["appId"]
    alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})
    # Every node is asked; those that do not lead the app's group answer
    # NOT_LEADER, which the client follows.
    for i in range(3):
        out = operator(cluster, i).json.ops.set_replicas(app=app, replicas=[1, 2, 3])
        assert "status" in out, out
    assert (
        alice.json.apps.view(app=app, method="get", args={"key": "k"})["result"] == "v"
    )
    # Without the redirect, the nodes that do not lead refuse it (once
    # signed: the signature is asked for first).
    refused = 0
    for i in range(3):
        try:
            operator(cluster, i).post_operator_signed(
                f"ops/apps/{app}/replicas", {"replicas": [1, 2, 3]}
            )
        except CloudError as e:
            assert code(e) == "NOT_LEADER", e.args
            refused += 1
    assert refused == 2


def test_a_registered_node_is_removed(cluster):
    ops = operator(cluster, 1)
    ops.json.ops.register_node(id=4, addr="http://127.0.0.1:9/", zone="z1")
    assert 4 in {n["id"] for n in ops.json.ops.status()["nodes"]}
    # Removal runs on the system leader; asked of every node, it is followed there.
    operator(cluster, 2).json.ops.remove_node(id=4)
    time.sleep(0.5)
    assert 4 not in {n["id"] for n in ops.json.ops.status()["nodes"]}


def test_backup_releases_outbound_and_upgrades(cluster, wasm):
    alice, alice_pk = person(cluster, 0, "alice-governed")
    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    app = alice.json.apps.create(name="governed", bundle=bundle)["appId"]
    alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})

    written = sum(operator(cluster, i).json.ops.backup()["written"] for i in range(3))
    assert written >= 1

    ops = operator(cluster)
    ops.json.ops.release(version="9.9.9")
    ops.json.ops.release(version="9.9.9", approved=not (True))
    # Past the ops token and the operator's hold on outbound approvals, the
    # node looks the package up; none is installed here (no signed .mpk).
    with pytest.raises(CloudError) as e:
        ops.json.ops.outbound(
            package="com.example.kv", prefixes=["https://example.com/"]
        )
    assert (e.value.args[1], code(e.value)) == (400, "REJECTED"), e.value.args
    assert isinstance(ops.json.ops.package_list(), dict)

    # The operator holds app upgrades here, so the owner's passkey does not.
    with pytest.raises(CloudError):
        alice.json.apps.upgrade(app=app, bundle=bundle, confirm=alice_pk)
    out = ops.json.ops.upgrade(app=app, bundle=bundle)
    assert out["status"] == "ok", out
    assert (
        alice.json.apps.view(app=app, method="get", args={"key": "k"})["result"] == "v"
    )

    # An org-wide run over an org with no app on that package: a run, empty.
    org = alice.json.orgs.create(name="Governed")["orgId"]
    run = ops.json.ops.org_upgrade(org=org, package="com.example.kv", version="1.0.0")
    assert run["runId"], run
    assert (
        alice.json.orgs.upgrade_status(org=org, run=run["runId"])["runId"]
        == run["runId"]
    )
