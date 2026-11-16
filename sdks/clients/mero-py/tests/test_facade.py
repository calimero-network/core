"""
The facade (`calimero_client_py.facade`, generated): one method per operation
of core's OpenAPI document on `Client`, grouped by tag. The table below is the
route methods of 1.0.0rc3 (removed in 1.0.0rc4): each call now as its facade method, and
what that sends and answers.

Runs on the stand-in native client of test_openapi_client.py: no node.
"""

import base64
import json

import pytest

from calimero_client_py import CloudError, FeatureNotActive, SoftPasskey, facade
from calimero_client_py.openapi.models import (
    InviteCreateResponse200,
    OrgCreateResponse200,
)

from test_openapi_client import (
    ROOT,
    Native,
    answer,
    challenge_details,
    client_on,
    refusal,
)

# ------------------------------------------------------------ the facade


def test_every_operation_is_one_method():
    document = json.loads((ROOT / "openapi" / "openapi.json").read_text())
    ids = set()
    for item in document["paths"].values():
        for op in item.values():
            if not isinstance(op, dict) or "operationId" not in op:
                continue
            ok = op["responses"].get("200", {}).get("content", {})
            if "text/event-stream" in ok:
                continue  # the streams are Client.events / presence_stream
            ids.add(op["operationId"])
    found = set()
    for attr, group in facade.GROUPS.items():
        assert isinstance(getattr(client_on(Native()), attr), (group, object))
        for name in dir(group):
            if not name.startswith("_"):
                found.add((attr, name))
    assert len(found) == len(ids)
    for oid in ids:
        assert hasattr(facade, "_" + oid.upper()), oid


def test_a_method_takes_flattened_keywords_and_answers_the_model():
    native = Native(
        refusal(412, "CONFIRM_REQUIRED", challenge_details()),
        answer(200, {"token": "t-1"}),
    )
    client = client_on(native)
    client.use_passkey(SoftPasskey())
    r = client.orgs.invite_create(org="o 1", role="member", teams=["t"], ttl_secs=60)
    assert isinstance(r, InviteCreateResponse200)
    assert r.token == "t-1"
    first, second = native.sent
    assert first["path"] == "/orgs/o%201/invites"
    assert first["body"] == {"role": "member", "teams": ["t"], "ttlSecs": 60}
    assert second["body"]["confirm"]["challengeId"] == "ch-1"


def test_keywords_only():
    with pytest.raises(TypeError):
        client_on(Native()).orgs.get("o1")


def test_confirm_overrides_the_clients_passkey():
    own = SoftPasskey()
    native = Native(
        refusal(412, "CONFIRM_REQUIRED", challenge_details()),
        answer(200, {"token": "t"}),
    )
    client = client_on(native)
    client.use_passkey(SoftPasskey())
    client.orgs.invite_create(org="o", role="member", confirm=own)
    sent = native.sent[1]["body"]["confirm"]["assertion"]
    assert sent["credentialId"] == own.credential_id


def test_confirm_false_raises_the_412():
    native = Native(refusal(412, "CONFIRM_REQUIRED", challenge_details()))
    client = client_on(native)
    client.use_passkey(SoftPasskey())
    with pytest.raises(CloudError) as e:
        client.orgs.invite_create(org="o", role="member", confirm=False)
    assert e.value.args[1:3] == (412, "CONFIRM_REQUIRED")
    assert len(native.sent) == 1


def test_a_ready_confirm_goes_in_the_body():
    native = Native(answer(200, {"token": "t"}))
    ready = {
        "challengeId": "c",
        "assertion": {
            "credentialId": "x",
            "authenticatorData": "y",
            "clientDataJson": "z",
            "signature": "s",
        },
    }
    client_on(native).orgs.invite_create(org="o", role="member", confirm=ready)
    assert native.sent[0]["body"]["confirm"] == ready


def test_a_closed_body_takes_only_the_fields_it_names():
    # Since core 92ac297a1 `POST /ops/quotas` is the closed `QuotaChanges`:
    # each limit is a keyword argument by its Python name, sent by its JSON
    # name, and a key that is no limit is refused before anything is sent
    # (the node would answer `400 UNKNOWN_QUOTA`).
    native = Native(answer(200, {}))
    client_on(native).ops.quotas_set(max_apps_per_account=3, operator_proof=False)
    assert native.sent[0]["body"] == {"maxAppsPerAccount": 3}
    with pytest.raises(TypeError):
        client_on(native).ops.quotas_set(maxAppsPerAccount=3, operator_proof=False)
    with pytest.raises(TypeError):
        client_on(native).orgs.create(name="o", colour="red")
    assert len(native.sent) == 1


def test_operator_proof_takes_a_seed_or_false():
    details = {"nonce": "n", "message": base64.b64encode(b"m").decode()}
    native = Native(
        refusal(428, "OPERATOR_SIGNATURE_REQUIRED", details),
        answer(200, {"status": "s", "unchanged": True}),
    )
    client_on(native).ops.quotas_set(operator_proof="ab" * 32)
    assert native.sent[1]["body"]["operatorProof"] == {
        "nonce": "n",
        "signature": native.operator_sign(b"m", "ab" * 32),
    }
    native = Native(refusal(428, "OPERATOR_SIGNATURE_REQUIRED", details))
    with pytest.raises(CloudError):
        client_on(native).ops.quotas_set(operator_proof=False)


def test_feature_not_active_is_raised_by_a_generated_method():
    # During a rollout a node refuses a command an older node could not apply.
    details = {"feature": "app_move", "needs": 3, "active": None}
    native = Native(
        refusal(
            409, "FEATURE_NOT_ACTIVE", details, message="not every node supports it"
        )
    )
    with pytest.raises(FeatureNotActive) as e:
        client_on(native).apps.move(app="a1", to_org="o2", confirm=False)
    assert isinstance(e.value, CloudError)
    assert e.value.args[1:3] == (409, "FEATURE_NOT_ACTIVE")
    assert json.loads(e.value.args[3]) == details
    # And through the JSON facade, as raw.
    native = Native(refusal(409, "FEATURE_NOT_ACTIVE", details))
    with pytest.raises(FeatureNotActive):
        client_on(native).json.apps.move(app="a1", to_org="o2", confirm=False)
    # Another 409 stays a plain CloudError.
    native = Native(refusal(409, "CONFLICT"))
    with pytest.raises(CloudError) as e:
        client_on(native).apps.move(app="a1", to_org="o2", confirm=False)
    assert type(e.value) is CloudError


def test_a_refusal_raises_cloud_error():
    native = Native(refusal(404, "NOT_FOUND", message="no such org"))
    with pytest.raises(CloudError) as e:
        client_on(native).orgs.get(org="nope")
    assert e.value.args == ("404 NOT_FOUND: no such org", 404, "NOT_FOUND", "null")


def test_a_raw_body_is_body():
    native = Native(answer(200, {"blobId": "b", "id": "i", "size": 3, "index": 1}))
    client_on(native).apps.blob_upload(app="a", body=b"xyz")
    sent = native.sent[0]
    assert sent["raw"] == b"xyz"
    assert sent["headers"]["content-type"] == "application/octet-stream"
    native = Native(answer(200, {"blobId": "b", "id": "i", "size": 3, "index": 1}))
    client_on(native).apps.blob_upload(app="a", body=b"xyz", content_type="image/png")
    assert native.sent[0]["headers"]["content-type"] == "image/png"


def test_an_enum_query_takes_its_value():
    native = Native(answer(200, {}))
    with pytest.raises(Exception):
        # The answer is not an attestation document; the request is what counts.
        client_on(native).public.attestation(nonce="00", purpose="kms")
    assert "purpose=kms" in native.sent[0]["path"]


def test_a_call_with_an_idempotency_key_is_retried():
    native = Native(
        answer(200, {"index": 1, "result": 2, "events": [], "receipt": None})
    )
    client_on(native)._op("intents", "app", body={"method": "m", "idempotencyKey": "k"})
    assert (native.sent[0]["mode"], native.sent[0]["retry"]) == ("repeatable", True)


def test_packages_is_a_group():
    native = Native(answer(200, {"packages": [{"name": "p"}]}))
    client = client_on(native)
    assert client.json.packages.list()["packages"] == [{"name": "p"}]
    assert callable(client.packages.get)


# ------------------------------------------------------------ rc3's route methods, as facade calls

PK = SoftPasskey()

# (call, method, path, body sent first, answer, what it returns): each rc3
# route method's call rewritten onto the facade, and what it sends and answers.
CASES = [
    (
        lambda c: c.json.auth.me(),
        "GET",
        "/auth/me",
        None,
        {"accountId": "a"},
        {"accountId": "a"},
    ),
    (
        lambda c: c.json.auth.apikey_create(label="ci", confirm=PK),
        "POST",
        "/auth/apikeys",
        {"label": "ci"},
        {"keyId": "k", "key": "cal_x"},
        {"keyId": "k", "key": "cal_x"},
    ),
    (lambda c: c.json.auth.logout(), "POST", "/auth/logout", None, {}, {}),
    (lambda c: c.json.auth.logout_all(), "POST", "/auth/logout-all", None, {}, {}),
    (
        lambda c: c.json.auth.sessions_list()["sessions"],
        "GET",
        "/auth/sessions",
        None,
        {"sessions": [1]},
        [1],
    ),
    (
        lambda c: c.json.auth.session_revoke(session="s/1"),
        "DELETE",
        "/auth/sessions/s%2F1",
        None,
        {},
        {},
    ),
    (
        lambda c: c.json.auth.activity_list(after=5)["activity"],
        "GET",
        "/auth/activity?after=5",
        None,
        {"activity": []},
        [],
    ),
    (
        lambda c: c.json.auth.activity_list()["activity"],
        "GET",
        "/auth/activity",
        None,
        {"activity": [1]},
        [1],
    ),
    (
        lambda c: c.json.auth.passkey_remove(cred="cr", confirm=PK),
        "DELETE",
        "/auth/passkeys/cr",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.auth.apikey_list()["keys"],
        "GET",
        "/auth/apikeys",
        None,
        {"keys": []},
        [],
    ),
    (
        lambda c: c.json.auth.apikey_revoke(key="k1", confirm=PK),
        "DELETE",
        "/auth/apikeys/k1",
        {},
        {},
        {},
    ),
    (lambda c: c.json.auth.usage(), "GET", "/auth/usage", None, {"u": 1}, {"u": 1}),
    (
        lambda c: c.json.auth.passkey_label(cred="cr", label="phone"),
        "POST",
        "/auth/passkeys/cr",
        {"label": "phone"},
        {},
        {},
    ),
    (
        lambda c: c.json.auth.alias_set(alias="kv", app="app1"),
        "PUT",
        "/auth/aliases/kv",
        {"app": "app1"},
        {},
        {},
    ),
    (
        lambda c: c.json.auth.alias_get(alias="kv"),
        "GET",
        "/auth/aliases/kv",
        None,
        {"alias": "kv", "appId": "a"},
        {"alias": "kv", "appId": "a"},
    ),
    (
        lambda c: c.json.auth.alias_list()["aliases"],
        "GET",
        "/auth/aliases",
        None,
        {"aliases": {"kv": "a"}},
        {"kv": "a"},
    ),
    (
        lambda c: c.json.auth.alias_delete(alias="kv"),
        "DELETE",
        "/auth/aliases/kv",
        None,
        {},
        {},
    ),
    (
        lambda c: c.json.auth.recovery_create(confirm=PK)["code"],
        "POST",
        "/auth/recovery",
        {},
        {"code": "XXXX"},
        "XXXX",
    ),
    (
        lambda c: c.json.auth.recovery_remove(confirm=PK),
        "DELETE",
        "/auth/recovery",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.auth.account_recovery_cancel(confirm=PK),
        "POST",
        "/auth/recovery/cancel",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.apps.transfer(app="a1", to="acc", confirm=PK),
        "POST",
        "/apps/a1/transfer",
        {"to": "acc"},
        {"ok": 1},
        {"ok": 1},
    ),
    (
        lambda c: c.json.apps.move(app="a1", to_org="o2", confirm=PK),
        "POST",
        "/apps/a1/move",
        {"toOrg": "o2"},
        {},
        {},
    ),
    (
        lambda c: c.json.apps.presence_publish(
            app="a1", seq=3, state=(b"\x01\xff").hex()
        ),
        "POST",
        "/apps/a1/presence",
        {"seq": 3, "state": "01ff"},
        {},
        {},
    ),
    (
        lambda c: c.json.apps.presence_publish(app="a1", seq=4),
        "POST",
        "/apps/a1/presence",
        {"seq": 4},
        {},
        {},
    ),
    (
        lambda c: c.json.apps.presence_get(app="a1"),
        "GET",
        "/apps/a1/presence",
        None,
        {"present": []},
        {"present": []},
    ),
    (
        lambda c: c.json.apps.create(
            name="n", package="com.x", version="1.0.0", **{"org": "o"}
        )["appId"],
        "POST",
        "/apps",
        {"name": "n", "package": "com.x", "version": "1.0.0", "org": "o"},
        {"appId": "a9"},
        "a9",
    ),
    (
        lambda c: c.json.apps.create(name="n", bundle="h", **{"org": "o"})["appId"],
        "POST",
        "/apps",
        {"name": "n", "bundle": "h", "org": "o"},
        {"appId": "a9"},
        "a9",
    ),
    (
        lambda c: c.json.apps.call(
            app="a1", method="set", args={"k": 1}, idempotency_key="key"
        ),
        "POST",
        "/apps/a1/intents",
        {"method": "set", "args": {"k": 1}, "idempotencyKey": "key"},
        {"index": 7, "result": 1, "extra": 0},
        {"index": 7, "result": 1, "extra": 0},
    ),
    (
        lambda c: c.json.apps.call(app="a1", method="pay", args=None, confirm=PK),
        "POST",
        "/apps/a1/intents",
        {"method": "pay", "args": None},
        {"index": 1, "result": None, "events": [{"kind": "k", "data": 1}]},
        {"index": 1, "result": None, "events": [{"kind": "k", "data": 1}]},
    ),
    (
        lambda c: c.json.apps.view(
            app="a1",
            method="get",
            consistency="stale",
            min_index=4,
        ),
        "POST",
        "/apps/a1/views",
        {"method": "get", "consistency": "stale", "minIndex": 4},
        {"index": 4, "result": "v"},
        {"index": 4, "result": "v"},
    ),
    (
        lambda c: c.json.apps.view(app="a1", method="get"),
        "POST",
        "/apps/a1/views",
        {"method": "get"},
        {"index": 4, "result": "v"},
        {"index": 4, "result": "v"},
    ),
    (lambda c: c.json.apps.list()["apps"], "GET", "/apps", None, {"apps": [1]}, [1]),
    (
        lambda c: c.json.apps.get(app="a1"),
        "GET",
        "/apps/a1",
        None,
        {"appId": "a1"},
        {"appId": "a1"},
    ),
    (
        lambda c: c.json.apps.access(app="a1", access={"kind": "public"}),
        "POST",
        "/apps/a1/access",
        {"access": {"kind": "public"}},
        {},
        {},
    ),
    (
        lambda c: c.json.apps.upgrade(app="a1", version="2.0.0"),
        "POST",
        "/apps/a1/upgrade",
        {"version": "2.0.0"},
        {"index": 1},
        {"index": 1},
    ),
    (lambda c: c.json.apps.delete(app="a1"), "DELETE", "/apps/a1", None, {}, {}),
    (
        lambda c: c.json.apps.storage(app="a1"),
        "GET",
        "/apps/a1/storage",
        None,
        {"keys": 1},
        {"keys": 1},
    ),
    (
        lambda c: c.json.apps.abi(app="a1"),
        "GET",
        "/apps/a1/abi",
        None,
        {"abi": {}},
        {"abi": {}},
    ),
    (
        lambda c: c.json.apps.metadata(app="a1", metadata=None),
        "POST",
        "/apps/a1/metadata",
        {"metadata": None},
        {},
        {},
    ),
    (
        lambda c: c.json.apps.blob_list(app="a1"),
        "GET",
        "/apps/a1/blobs",
        None,
        {"blobs": []},
        {"blobs": []},
    ),
    (
        lambda c: c.json.apps.blob_info(app="a1", id="b1"),
        "GET",
        "/apps/a1/blobs/b1/info",
        None,
        {"id": "b1"},
        {"id": "b1"},
    ),
    (
        lambda c: c.json.apps.blob_delete(app="a1", id="b1"),
        "DELETE",
        "/apps/a1/blobs/b1",
        None,
        {"deleted": True},
        {"deleted": True},
    ),
    (
        lambda c: c.json.orgs.create(name="Acme", recovery_key="pk")["orgId"],
        "POST",
        "/orgs",
        {"name": "Acme", "recoveryKey": "pk"},
        {"orgId": "o1"},
        "o1",
    ),
    (
        lambda c: c.json.orgs.get(org="o1"),
        "GET",
        "/orgs/o1",
        None,
        {"orgId": "o1"},
        {"orgId": "o1"},
    ),
    (
        lambda c: c.json.orgs.members(org="o1", detail="security")["members"],
        "GET",
        "/orgs/o1/members?detail=security",
        None,
        {"members": []},
        [],
    ),
    (
        lambda c: c.json.orgs.set_member(
            org="o1", account="acc", role="admin", confirm=PK
        ),
        "POST",
        "/orgs/o1/members",
        {"account": "acc", "role": "admin"},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.remove_member(org="o1", account="acc", confirm=PK),
        "DELETE",
        "/orgs/o1/members/acc",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.capabilities(org="o1", capabilities=3, confirm=PK),
        "POST",
        "/orgs/o1/capabilities",
        {"capabilities": 3},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.team_create(org="o1", name="eng", join="invite")[
            "teamId"
        ],
        "POST",
        "/orgs/o1/teams",
        {"name": "eng", "join": "invite"},
        {"teamId": "t1"},
        "t1",
    ),
    (
        lambda c: c.json.orgs.app_team_list(org="o1"),
        "GET",
        "/orgs/o1/app-teams",
        None,
        {"mode": "shared"},
        {"mode": "shared"},
    ),
    (
        lambda c: c.json.orgs.app_teams_scope(org="o1", confirm=PK, shared=True),
        "POST",
        "/orgs/o1/app-teams",
        {"shared": True},
        {"mode": "shared"},
        {"mode": "shared"},
    ),
    (
        lambda c: c.json.orgs.team_get(org="o1", team="t1"),
        "GET",
        "/orgs/o1/teams/t1",
        None,
        {"teamId": "t1"},
        {"teamId": "t1"},
    ),
    (
        lambda c: c.json.orgs.team_update(org="o1", team="t1", name="x", parent=None),
        "POST",
        "/orgs/o1/teams/t1",
        {"name": "x", "parent": None},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.team_delete(org="o1", team="t1"),
        "DELETE",
        "/orgs/o1/teams/t1",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.team_set_member(
            org="o1", team="t1", account="acc", confirm=PK, role="readOnly"
        ),
        "POST",
        "/orgs/o1/teams/t1/members",
        {"account": "acc", "role": "readOnly"},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.team_remove_member(
            org="o1", team="t1", account="acc", confirm=PK
        ),
        "DELETE",
        "/orgs/o1/teams/t1/members/acc",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.member_set_teams(
            org="o1",
            account="acc",
            teams=[
                t if isinstance(t, dict) else {"team": t}
                for t in ["t1", {"team": "t2", "role": "lead"}]
            ],
            confirm=PK,
        ),
        "POST",
        "/orgs/o1/members/acc/teams",
        {"teams": [{"team": "t1"}, {"team": "t2", "role": "lead"}]},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.member_remove_teams(
            org="o1", account="acc", teams=["t1"], confirm=PK
        ),
        "DELETE",
        "/orgs/o1/members/acc/teams",
        {"teams": ["t1"]},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.invite_create(org="o1", role="member", confirm=PK)[
            "token"
        ],
        "POST",
        "/orgs/o1/invites",
        {"role": "member"},
        {"token": "tok"},
        "tok",
    ),
    (
        lambda c: c.json.orgs.invite_redeem(token="tok"),
        "POST",
        "/invites/tok/redeem",
        None,
        {"orgId": "o1"},
        {"orgId": "o1"},
    ),
    (
        lambda c: c.json.orgs.delete(org="o1", confirm=PK),
        "DELETE",
        "/orgs/o1",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.metadata(org="o1", metadata={"a": 1}),
        "POST",
        "/orgs/o1/metadata",
        {"metadata": {"a": 1}},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.member_metadata(org="o1", account="acc", metadata=None),
        "POST",
        "/orgs/o1/members/acc/metadata",
        {"metadata": None},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.recovery_set(org="o1", public_key=None, confirm=PK),
        "POST",
        "/orgs/o1/recovery",
        {"publicKey": None},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.recovery_cancel(org="o1", confirm=PK),
        "POST",
        "/orgs/o1/recovery/cancel",
        {},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.recovery_finish(org="o1", confirm=PK),
        "POST",
        "/orgs/o1/recovery/finish",
        {},
        {"org": "o1"},
        {"org": "o1"},
    ),
    (
        lambda c: c.json.orgs.release_approval(version="1.0.0", confirm=PK, org="o1"),
        "POST",
        "/orgs/o1/releases",
        {"version": "1.0.0"},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.outbound_approval(
            package="com.x", confirm=PK, org="o1", prefixes=[], replace=True
        ),
        "POST",
        "/orgs/o1/outbound",
        {"package": "com.x", "prefixes": [], "replace": True},
        {},
        {},
    ),
    (
        lambda c: c.json.orgs.upgrade(
            org="o1", package="com.x", version="2", confirm=PK
        ),
        "POST",
        "/orgs/o1/upgrade",
        {"package": "com.x", "version": "2"},
        {"runId": "r"},
        {"runId": "r"},
    ),
    (
        lambda c: c.json.orgs.upgrade_status(org="o1", run="r"),
        "GET",
        "/orgs/o1/upgrades/r",
        None,
        {"runId": "r"},
        {"runId": "r"},
    ),
    (
        lambda c: c.json.orgs.upgrade_retry(org="o1", run="r", confirm=PK),
        "POST",
        "/orgs/o1/upgrades/r/retry",
        {},
        {"runId": "r2"},
        {"runId": "r2"},
    ),
    (
        lambda c: c.json.packages.install(package="com.x", version="1"),
        "POST",
        "/packages/install",
        {"package": "com.x", "version": "1"},
        {},
        {},
    ),
    (
        lambda c: c.json.packages.get(package="com.x"),
        "GET",
        "/packages/com.x",
        None,
        {"name": "com.x"},
        {"name": "com.x"},
    ),
    (
        lambda c: c.json.packages.uninstall(package="com.x"),
        "DELETE",
        "/packages/com.x",
        None,
        {},
        {},
    ),
    (
        lambda c: c.json.packages.version_uninstall(package="com.x", version="1"),
        "DELETE",
        "/packages/com.x/1",
        None,
        {},
        {},
    ),
    (
        lambda c: c.json.packages.origins_list(package="com.x"),
        "GET",
        "/package-origins/com.x",
        None,
        {"origins": []},
        {"origins": []},
    ),
    (
        lambda c: c.json.packages.origins_add(
            package="com.x", origin="https://a.example", confirm=PK
        ),
        "POST",
        "/package-origins/com.x",
        {"origin": "https://a.example"},
        {},
        {},
    ),
    (
        lambda c: c.json.packages.origins_add(
            package="com.x",
            origin="https://a.example",
            confirm=PK,
            rp_id="example",
            served=True,
        ),
        "POST",
        "/package-origins/com.x",
        {"origin": "https://a.example", "rpId": "example", "served": True},
        {},
        {},
    ),
    (
        lambda c: c.json.packages.origins_remove(
            package="com.x", origin="https://a.example", confirm=PK
        ),
        "DELETE",
        "/package-origins/com.x",
        {"origin": "https://a.example"},
        {"removed": True},
        {"removed": True},
    ),
    (
        lambda c: c.json.ops.status(),
        "GET",
        "/ops/status",
        None,
        {"node": 1},
        {"node": 1},
    ),
    (
        lambda c: c.json.ops.register_node(id=4, addr="https://n4/", zone="z"),
        "POST",
        "/ops/nodes",
        {"id": 4, "addr": "https://n4/", "zone": "z"},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.remove_node(id=4),
        "DELETE",
        "/ops/nodes/4",
        {},
        {"status": "s"},
        {"status": "s"},
    ),
    (
        lambda c: c.json.ops.set_replicas(app="a1", replicas=[1, 2, 4]),
        "POST",
        "/ops/apps/a1/replicas",
        {"replicas": [1, 2, 4]},
        {"status": "s"},
        {"status": "s"},
    ),
    (
        lambda c: c.json.ops.backup()["written"],
        "POST",
        "/ops/backup",
        None,
        {"written": 3},
        3,
    ),
    (
        lambda c: c.json.ops.quotas_get(),
        "GET",
        "/ops/quotas",
        None,
        {"apps": 1},
        {"apps": 1},
    ),
    (
        lambda c: c.json.ops.quotas_set(max_apps_per_account=2),
        "POST",
        "/ops/quotas",
        {"maxAppsPerAccount": 2},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.release(version="1.0.0", approved=not (True)),
        "POST",
        "/ops/releases",
        {"version": "1.0.0", "approved": False},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.outbound(package="com.x", prefixes="manifest"),
        "POST",
        "/ops/outbound",
        {"package": "com.x", "prefixes": "manifest"},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.upgrade(app="a1", version="2"),
        "POST",
        "/ops/apps/a1/upgrade",
        {"version": "2"},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.org_upgrade(org="o1", package="com.x", version="2"),
        "POST",
        "/ops/orgs/o1/upgrade",
        {"package": "com.x", "version": "2"},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.served(),
        "GET",
        "/ops/served-origins",
        None,
        {"origins": []},
        {"origins": []},
    ),
    (
        lambda c: c.json.ops.package_list(),
        "GET",
        "/ops/packages",
        None,
        {"packages": []},
        {"packages": []},
    ),
    (
        lambda c: c.json.ops.package_install(package="com.x", version="1"),
        "POST",
        "/ops/packages/install",
        {"package": "com.x", "version": "1"},
        {},
        {},
    ),
    (
        lambda c: c.json.ops.package_version_uninstall(package="com.x", version="1"),
        "DELETE",
        "/ops/packages/com.x/1",
        None,
        {},
        {},
    ),
]


@pytest.mark.parametrize("case", CASES, ids=[str(i) for i in range(len(CASES))])
def test_a_facade_call_sends_what_the_route_takes(case):
    call, method, path, body, reply, returns = case
    native = Native(answer(200, reply))
    got = call(client_on(native))
    (sent,) = native.sent
    assert (sent["method"], sent["path"], sent["body"]) == (method, path, body)
    assert got == returns


def test_the_byte_routes():
    native = Native(
        answer(200, {"hash": "h"}),
        (200, [("content-type", "application/octet-stream")], b"\x00\x01"),
        (200, [("content-type", "text/plain")], b"metric 1\n"),
        answer(200, {"name": "p"}),
    )
    client = client_on(native)
    assert client.json.packages.bundle_put(body=b"\0asm")["hash"] == "h"
    assert client.json.apps.blob_download(app="a1", id="b1") == b"\x00\x01"
    assert client.json.ops.metrics() == "metric 1\n"
    assert client.json.packages.upload(body=b"mpk") == {"name": "p"}
    assert native.sent[0]["headers"]["content-type"] == "application/wasm"
    assert native.sent[0]["raw"] == b"\0asm"
    assert native.sent[3]["path"] == "/packages"


def test_the_typed_answer_is_the_generated_model():
    native = Native(answer(200, {"orgId": "o1"}))
    r = client_on(native).orgs.create(name="Acme")
    assert isinstance(r, OrgCreateResponse200)
    assert r.org_id == "o1"


def test_an_operator_signer_makes_the_proof():
    details = {
        "subject": "s",
        "method": "__set_quotas",
        "args": "{}",
        "nonce": "n-9",
        "cluster": "c",
        "message": base64.b64encode(b"m").decode(),
    }
    native = Native(
        refusal(428, "OPERATOR_SIGNATURE_REQUIRED", details),
        answer(200, {}),
        operator_seed="ignored",
    )
    seen = []

    def signer(d):
        seen.append(d)
        return {"nonce": d["nonce"], "signature": "sig"}

    client = client_on(native)
    client.use_operator_signer(signer)
    assert client.json.ops.quotas_set() == {}
    assert seen[0]["cluster"] == "c"
    assert native.sent[1]["body"]["operatorProof"] == {
        "nonce": "n-9",
        "signature": "sig",
    }


def test_json_answers_what_the_node_sent():
    native = Native(answer(200, {"orgId": "o1", "name": "A", "extra": [1]}))
    # No `owner`, which the document requires: the model could not read it.
    assert client_on(native).json.orgs.get(org="o1") == {
        "orgId": "o1",
        "name": "A",
        "extra": [1],
    }
