"""
The facade's typed methods against a real 3-node merod cluster: the
generated requests and models as a node answers them, through the sealed
channel. Needs what tests/test_cloud_v2.py needs; skipped without it.
"""

import pytest

from calimero_client_py import CloudError
from test_cloud_v2 import cluster, person, pytestmark, wasm  # noqa: F401

__all__ = ["pytestmark"]


def test_typed_calls_with_the_clients_passkey(cluster, wasm):  # noqa: F811
    alice, pk = person(cluster, 0, "alice-facade")
    alice.use_passkey(pk)
    org = alice.orgs.create(name="Facade Org").org_id
    # A step-up (412), answered from use_passkey.
    token = alice.orgs.invite_create(org=org, role="member").token
    assert token
    bundle = alice.packages.bundle_put(body=wasm).hash_
    app = alice.apps.create(name="facade-kv", bundle=bundle).app_id
    r = alice.apps.call(
        app=app, method="set", args={"key": "k", "value": "v"}, idempotency_key="f-1"
    )
    assert r.index >= 1
    assert alice.apps.get(app=app).app_id == app
    v = alice.apps.view(app=app, method="get", args={"key": "k"})
    assert v.to_dict()["result"] == "v"
    with pytest.raises(CloudError) as e:
        alice.orgs.get(org="no-such-org")
    assert e.value.args[1] in (403, 404)
