"""
Organization recovery through CloudClient (core `docs/specs/org-recovery.md`):
the owner sets a recovery key, an account holding the seed starts a recovery
that waits 72 hours, the owner can cancel it, a wrong seed and a non-owner are
refused, and rotating the key makes the old seed useless. Waiting out the
delay is core's own test (`an_owner_who_lost_every_passkey_recovers_the_org`);
here the recovery stops at `409 RECOVERY_NOT_READY`.

The wire bytes are checked against core's pinned vectors everywhere; the
cluster test has the same requirements as test_cloud_v2.py (a merod binary,
from core 1.0.0-rc.10 or later: the first release with these routes).
"""

import pytest

from calimero_client_py import CloudClient, CloudError
from test_cloud_v2 import Cluster, code, person
from test_cloud_v2 import pytestmark as needs_merod


@pytest.fixture(scope="module")
def cluster():
    c = Cluster(3)
    yield c
    c.stop()


def test_the_seed_signs_the_bytes_core_verifies():
    # core: calimero_identity::recovery::the_wire_bytes_are_pinned_for_the_clients
    seed = "01" * 32
    assert (
        CloudClient.recovery_public_key(seed)
        == "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w"
    )
    assert CloudClient.sign_recovery(seed, "o", "a", "n") == (
        "bbbHMaH2SC50zQADskIDS5xg8WxZFrtxdasJzhzCOMZh-aBrUxeDKro2_UUIyU4PzH5LmsS1gQRQmVfx3E1eDw"
    )
    fresh = CloudClient.new_recovery_seed()
    assert len(fresh) == 71 and fresh.count("-") == 7
    with pytest.raises(ValueError):
        CloudClient.recovery_public_key("not a seed")


@needs_merod
def test_an_org_is_recovered_only_with_its_current_seed(cluster):
    alice, apk = person(cluster, 0, "alice-recovery")
    bob, bpk = person(cluster, 1, "bob-recovery")
    seed = CloudClient.new_recovery_seed()
    org = alice.json.orgs.create(
        name="Recoverable", recovery_key=CloudClient.recovery_public_key(seed)
    )["orgId"]
    assert alice.json.orgs.get(org=org)["recovery"][
        "publicKey"
    ] == CloudClient.recovery_public_key(seed)

    # A wrong seed is refused, and so is a non-owner managing the key.
    with pytest.raises(CloudError) as e:
        bob.start_recovery(org, CloudClient.new_recovery_seed())
    assert code(e.value) == "WRONG_RECOVERY_KEY"
    with pytest.raises(CloudError):
        bob.json.orgs.recovery_set(
            org=org,
            public_key=CloudClient.recovery_public_key(CloudClient.new_recovery_seed()),
            confirm=bpk,
        )

    # The seed starts one; it waits, and the owner cancels it.
    started = bob.start_recovery(org, seed)
    assert started["readyAt"] - started["startedAt"] == 72 * 3600
    assert (
        alice.json.orgs.get(org=org)["recovery"]["pending"]["account"]
        == bob.json.auth.me()["accountId"]
    )
    with pytest.raises(CloudError) as e:
        bob.start_recovery(org, seed)
    assert code(e.value) == "RECOVERY_PENDING"
    with pytest.raises(CloudError) as e:
        bob.json.orgs.recovery_finish(org=org, confirm=bpk)
    assert code(e.value) == "RECOVERY_NOT_READY"
    alice.json.orgs.recovery_cancel(org=org, confirm=apk)
    assert alice.json.orgs.get(org=org)["recovery"]["pending"] is None

    # Rotating the key leaves the old seed useless.
    new_seed = CloudClient.new_recovery_seed()
    alice.json.orgs.recovery_set(
        org=org, public_key=CloudClient.recovery_public_key(new_seed), confirm=apk
    )
    with pytest.raises(CloudError) as e:
        bob.start_recovery(org, seed)
    assert code(e.value) == "WRONG_RECOVERY_KEY"
    bob.start_recovery(org, new_seed)
    alice.json.orgs.recovery_set(
        org=org, public_key=None, confirm=apk
    )  # removing it ends the recovery too
    assert "recovery" not in alice.json.orgs.get(org=org)
