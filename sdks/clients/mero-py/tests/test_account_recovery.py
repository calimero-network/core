"""
Account recovery (core `docs/specs/account-recovery.md`, core #4844): a
person who lost every passkey gets back in with the recovery code they made
while signed in. The code and a new passkey start a recovery that waits 72
hours; a passkey the account still has cancels it; after, the new passkey
finishes it and signs in. Waiting out the delay is core's own test
(`tests/recovery.rs`, which moves the nodes' clocks); here the recovery stops
at `409 RECOVERY_NOT_READY`.

The code's key is checked against core's derivation everywhere; the cluster
test has the same requirements as test_cloud_v2.py (a merod binary from a
core with #4844: master after 1.0.0-rc.10).
"""

import hashlib

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from calimero_client_py import CloudClient, CloudError, LoginRequired, SoftPasskey
from test_cloud_v2 import ORIGIN, Cluster, code, person
from test_cloud_v2 import pytestmark as needs_merod

# core: crates/node/src/proof.rs, `RECOVERY_CODE_DOMAIN`.
RECOVERY_CODE_DOMAIN = b"calimero.account-recovery-code.v1\0"


def core_recovery_key(canonical: str) -> str:
    """Core's `proof::recovery_key`, written out: `SHA-256(domain || canonical
    code)` is the Ed25519 seed, the public key hex."""
    seed = hashlib.sha256(RECOVERY_CODE_DOMAIN + canonical.encode()).digest()
    return (
        Ed25519PrivateKey.from_private_bytes(seed).public_key().public_bytes_raw().hex()
    )


def test_a_codes_key_is_the_one_core_derives():
    canonical = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    # Pinned, so a change on either side shows here and not only on a node.
    assert (
        CloudClient.recovery_code_key(canonical)
        == "88736a23467015382f969b313598e1349b61839bd057843f679ab9eb92d637ef"
    )
    for c in (canonical, "Z" * 32, "0" * 32):
        assert CloudClient.recovery_code_key(c) == core_recovery_key(c)


def test_a_code_reads_back_however_it_is_written_down():
    canonical = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    for typed in (
        "0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV-WXYZ",
        "o123 4567 89ab cdef ghjk mnpq rstv wxyz",
        "OL23456789abcdefghjkmnpqrstvwxyz",
    ):
        assert CloudClient.normalize_recovery_code(typed) == canonical
        assert CloudClient.recovery_code_key(typed) == core_recovery_key(canonical)
    for bad in ("ABCD-EFGH", canonical + "U", canonical + " " * 80, ""):
        with pytest.raises(ValueError):
            CloudClient.normalize_recovery_code(bad)


def test_a_mistyped_code_is_refused_before_anything_is_sent():
    # No node at this address: a ValueError means nothing was sent.
    c = CloudClient("http://127.0.0.1:9", allow_mock=True)
    with pytest.raises(ValueError):
        c.start_account_recovery("not-a-code", SoftPasskey("localhost", ORIGIN))


@pytest.fixture(scope="module")
def cluster():
    c = Cluster(3)
    yield c
    c.stop()


@needs_merod
def test_a_recovery_code_starts_a_recovery_the_account_can_cancel(cluster):
    alice, old = person(cluster, 0, "alice-account-recovery")
    account = alice.json.auth.me()["accountId"]
    assert alice.json.auth.me()["recoveryCode"] is False

    first = alice.json.auth.recovery_create(confirm=old)["code"]
    assert alice.json.auth.me()["recoveryCode"] is True

    # Someone with the code starts one, signed out, on another node.
    rescuer = cluster.client(1)
    device = SoftPasskey("localhost", ORIGIN)
    started = rescuer.start_account_recovery(first, device)
    assert started["accountId"] == account
    assert started["credentialId"] == device.credential_id
    assert started["readyAt"] - started["startedAt"] == 72 * 3600
    me = alice.json.auth.me()
    assert me["recoveryPending"]["credentialId"] == device.credential_id
    assert me["recoveryCode"] is False  # spent
    assert len(me["passkeys"]) == 1

    # Too early to finish, named or not.
    for account_id in (account, None):
        with pytest.raises(CloudError) as e:
            rescuer.finish_account_recovery(device, account_id)
        assert code(e.value) == "RECOVERY_NOT_READY"

    # The account's own passkey cancels it; the spent code is refused.
    alice.json.auth.account_recovery_cancel(confirm=old)
    assert alice.json.auth.me()["recoveryPending"] is None
    with pytest.raises(LoginRequired):
        cluster.client(2).start_account_recovery(
            first, SoftPasskey("localhost", ORIGIN)
        )

    # A fresh code, as written down, starts again; removing the code is the
    # account's too.
    second = alice.json.auth.recovery_create(confirm=old)["code"]
    typed = second.lower().replace("-", " ")
    again = cluster.client(2).start_account_recovery(
        typed, SoftPasskey("localhost", ORIGIN)
    )
    assert again["accountId"] == account
    with pytest.raises(CloudError) as e:
        cluster.client(1).start_account_recovery(
            alice.json.auth.recovery_create(confirm=old)["code"],
            SoftPasskey("localhost", ORIGIN),
        )
    assert code(e.value) == "RECOVERY_PENDING"
    alice.json.auth.account_recovery_cancel(confirm=old)
    alice.json.auth.recovery_remove(confirm=old)
    assert alice.json.auth.me()["recoveryCode"] is False
