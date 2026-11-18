"""
The rest of merod's API through CloudClient's typed methods: API keys,
organizations and teams, the app directory and access, upgrades, passkeys.

Same requirements as test_cloud_v2.py (a merod 1.0 binary and the kv-store
fixture); its own cluster, since that module kills a node.
"""

import base64
import hashlib
import io
import json
import tarfile
import time
from pathlib import Path

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from calimero_client_py import CloudError, LoginRequired, SoftPasskey
from test_cloud_v2 import KV_STORE_WASM, Cluster, code, person, pytestmark  # noqa: F401


@pytest.fixture(scope="module")
def cluster():
    c = Cluster(3)
    yield c
    c.stop()


@pytest.fixture(scope="module")
def wasm() -> bytes:
    return Path(KV_STORE_WASM).read_bytes()


def test_api_keys_are_scoped_listed_and_revoked(cluster, wasm):
    alice, pk = person(cluster, 0, "alice-keys")
    app = alice.json.apps.create(
        name="keyed", bundle=alice.json.packages.bundle_put(body=wasm)["hash"]
    )["appId"]
    alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})
    made = alice.json.auth.apikey_create(
        label="reader", apps=[app], read_only=True, confirm=pk
    )
    key_id, key = made["keyId"], made["key"]
    listed = {k["keyId"]: k for k in alice.json.auth.apikey_list()["keys"]}
    assert listed[key_id]["label"] == "reader" and listed[key_id]["readOnly"]

    agent = cluster.client(1)
    agent.use_api_key(key)
    assert (
        agent.json.apps.view(app=app, method="get", args={"key": "k"})["result"] == "v"
    )
    with pytest.raises(CloudError):
        agent.json.apps.call(app=app, method="set", args={"key": "k", "value": "w"})

    # A node from before revokes at once; this one asks for the passkey (412).
    alice.json.auth.apikey_revoke(key=key_id, confirm=pk)
    time.sleep(0.3)
    assert key_id not in {k["keyId"] for k in alice.json.auth.apikey_list()["keys"]}
    with pytest.raises(CloudError) as e:
        agent.json.apps.view(app=app, method="get", args={"key": "k"})
    assert code(e.value) == "UNAUTHENTICATED"


def test_orgs_and_teams_end_to_end(cluster, wasm):
    alice, pk = person(cluster, 0, "alice-teams")
    bob, _ = person(cluster, 1, "bob-teams")
    bob_id = bob.json.auth.me()["accountId"]
    org = alice.json.orgs.create(name="Initech")["orgId"]
    token = alice.json.orgs.invite_create(
        org=org, role="member", confirm=pk, ttl_secs=600, uses=1
    )["token"]
    bob.json.orgs.invite_redeem(token=token)
    assert alice.json.orgs.get(org=org)["members"][bob_id] == "member"
    with pytest.raises(CloudError):
        cluster.client(2).json.orgs.invite_redeem(token=token)  # one use only

    team = alice.json.orgs.team_create(org=org, name="eng", join="invite")[
        "teamId"
    ]  # restricted by default
    sub = alice.json.orgs.team_create(
        org=org, name="backend", join="everyone", parent=team
    )["teamId"]
    alice.json.orgs.team_set_member(
        org=org, team=team, account=bob_id, confirm=pk, role="readOnly"
    )
    app = alice.json.apps.create(
        name="eng-kv",
        bundle=alice.json.packages.bundle_put(body=wasm)["hash"],
        **{"org": org, "access": {"kind": "team", "team": team}},
    )["appId"]
    view = bob.json.orgs.team_get(org=org, team=team)
    assert view["myRole"] == "readOnly" and sub in view["children"]
    assert [a["appId"] for a in view["apps"]] == [app]
    with pytest.raises(CloudError) as e:
        bob.json.apps.call(app=app, method="set", args={"key": "k", "value": "bob"})
    assert e.value.args[1] == 403

    alice.json.orgs.team_set_member(
        org=org, team=team, account=bob_id, confirm=pk, role="member"
    )
    bob.json.apps.call(app=app, method="set", args={"key": "k", "value": "bob"})

    alice.json.orgs.team_update(
        org=org, team=team, name="engineering", metadata={"colour": "teal"}
    )
    view = alice.json.orgs.team_get(org=org, team=team)
    assert view["name"] == "engineering" and view["metadata"] == {"colour": "teal"}
    other = alice.json.orgs.team_create(org=org, name="ops", join="invite")["teamId"]
    alice.json.orgs.team_update(org=org, team=sub, parent=other)
    assert alice.json.orgs.team_get(org=org, team=sub)["parent"] == other

    alice.json.orgs.capabilities(org=org, capabilities=0, confirm=pk, account=bob_id)
    assert alice.json.orgs.get(org=org)["capabilities"][bob_id] == 0

    alice.json.orgs.team_remove_member(org=org, team=team, account=bob_id, confirm=pk)
    time.sleep(0.3)
    with pytest.raises(CloudError) as e:
        bob.json.orgs.team_get(org=org, team=team)
    assert code(e.value) == "NOT_FOUND"

    # A team with no apps (`other` keeps only `sub`) goes without a passkey.
    alice.json.orgs.team_delete(org=org, team=other)
    time.sleep(0.3)
    for gone in (other, sub):
        with pytest.raises(CloudError) as e:
            alice.json.orgs.team_get(org=org, team=gone)
        assert code(e.value) == "NOT_FOUND"
    # The team holds an app: deleting it takes alice's passkey, and the node's
    # ask names the apps that go (core#4891). A node from before deletes it at
    # once, so the 412 is checked only where it is asked.
    try:
        alice.json.orgs.team_delete(org=org, team=team)
    except CloudError as asked:
        assert code(asked) == "CONFIRM_REQUIRED", asked.args
        assert json.loads(asked.args[3])["apps"] == [app]
        assert alice.json.apps.get(app=app)["name"] == "eng-kv"
        alice.json.orgs.team_delete(org=org, team=team, confirm=pk)
    time.sleep(0.3)
    with pytest.raises(CloudError) as e:
        alice.json.apps.get(app=app)
    assert code(e.value) == "NOT_FOUND"

    alice.json.orgs.remove_member(org=org, account=bob_id, confirm=pk)
    time.sleep(0.3)
    assert bob_id not in alice.json.orgs.get(org=org)["members"]


def test_app_directory_access_and_upgrade(cluster, wasm):
    alice, alice_pk = person(cluster, 0, "alice-apps")
    carol, _ = person(cluster, 2, "carol-apps")
    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    app = alice.json.apps.create(name="dir", bundle=bundle)["appId"]
    assert app in {a["appId"] for a in alice.json.apps.list()["apps"]}
    assert alice.json.apps.get(app=app)["name"] == "dir"
    alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})

    with pytest.raises(CloudError):
        carol.json.apps.view(app=app, method="get", args={"key": "k"})
    alice.json.apps.access(
        app=app,
        access={"kind": "accounts", "accounts": [carol.json.auth.me()["accountId"]]},
    )
    time.sleep(0.3)
    assert (
        carol.json.apps.view(app=app, method="get", args={"key": "k"})["result"] == "v"
    )

    # The owner approves an upgrade with their passkey; without it the node asks.
    with pytest.raises(CloudError) as asked:
        alice.json.apps.upgrade(app=app, bundle=bundle)
    assert asked.value.args[2] == "CONFIRM_REQUIRED"
    out = alice.json.apps.upgrade(app=app, bundle=bundle, confirm=alice_pk)
    assert out["status"] == "ok", out
    assert (
        alice.json.apps.view(app=app, method="get", args={"key": "k"})["result"] == "v"
    )


def test_passkey_removal_and_sign_out(cluster):
    laptop, laptop_key = person(cluster, 0, "alice-signout")
    phone_key = SoftPasskey()
    cred = laptop.add_passkey(phone_key, laptop_key)
    laptop.json.auth.passkey_remove(cred=cred, confirm=laptop_key)
    time.sleep(0.3)
    with pytest.raises(CloudError):
        cluster.client(1).sign_in(phone_key)

    laptop.json.auth.logout()
    with pytest.raises(CloudError):
        laptop.refresh()


def test_a_team_moves_to_the_top(cluster):
    alice, _ = person(cluster, 0, "alice-move")
    org = alice.json.orgs.create(name="Movers")["orgId"]
    top = alice.json.orgs.team_create(org=org, name="top", join="invite")["teamId"]
    sub = alice.json.orgs.team_create(org=org, name="sub", parent=top, join="invite")[
        "teamId"
    ]
    alice.json.orgs.team_update(org=org, team=sub, parent=None)
    assert alice.json.orgs.team_get(org=org, team=sub)["parent"] is None


def test_aliases_org_deletion_and_signing_out_everywhere(cluster, wasm):
    alice, pk = person(cluster, 0, "alice-everywhere")
    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    app = alice.json.apps.create(name="kv-alias", bundle=bundle)["appId"]
    alice.put("auth/aliases/notes", {"app": app})
    assert alice.get("auth/aliases")["aliases"] == {"notes": app}

    org = alice.json.orgs.create(name="Doomed")["orgId"]
    with pytest.raises(CloudError) as e:
        alice.delete(f"orgs/{org}")
    assert code(e.value) == "CONFIRM_REQUIRED"
    alice.delete_confirmed(f"orgs/{org}", pk)
    with pytest.raises(CloudError):
        alice.get(f"orgs/{org}")

    laptop = cluster.client(1)
    laptop.sign_in(pk)
    alice.json.auth.logout_all()
    with pytest.raises(CloudError):
        laptop.get("auth/me")


def test_passkey_times_and_sessions_listed_and_ended_one_at_a_time(cluster):
    laptop, pk = person(cluster, 0, "alice-sessions")
    me = laptop.json.auth.me()
    mine = next(p for p in me["passkeys"] if p["id"] == me["passkeyId"])
    phone = cluster.client(1)
    phone.sign_in(pk)
    try:
        listed = laptop.json.auth.sessions_list()["sessions"]
    except CloudError as e:
        # 1.0.0-rc.14 and earlier: no `/auth/sessions` (a bare `404`), and
        # no passkey times.
        if e.args[1] != 404 or code(e) == "NOT_FOUND":
            raise
        assert mine.get("addedAt") is None, me
        pytest.skip("the node has no session list or passkey times (core #4894)")
    assert isinstance(mine["addedAt"], int), me
    assert mine["lastSignInAt"] >= mine["addedAt"], me

    assert len(listed) == 2, listed
    own = next(s for s in listed if s["current"])
    other = next(s for s in listed if not s["current"])
    for s in listed:
        assert s["passkey"] == me["passkeyId"], s
        assert s["app"] is None, s
        assert isinstance(s["lastActiveAt"], int), s
    assert [
        s["session"]
        for s in phone.json.auth.sessions_list()["sessions"]
        if s["current"]
    ] == [other["session"]]

    # The laptop ends the phone's session: the phone must sign in again.
    laptop.json.auth.session_revoke(session=other["session"])
    with pytest.raises(LoginRequired):
        phone.json.auth.me()
    assert [s["session"] for s in laptop.json.auth.sessions_list()["sessions"]] == [
        own["session"]
    ]
    with pytest.raises(CloudError) as e:
        laptop.json.auth.session_revoke(session=other["session"])
    assert (e.value.args[1], code(e.value)) == (404, "NOT_FOUND"), e.value
    assert laptop.json.auth.me()["accountId"] == me["accountId"]


def test_admins_see_their_members_lockout_risk(cluster):
    alice, pk = person(cluster, 0, "alice-security")
    bob, _ = person(cluster, 1, "bob-security")
    org = alice.json.orgs.create(name="Lockout Co")["orgId"]
    bob.json.orgs.invite_redeem(
        token=alice.json.orgs.invite_create(org=org, role="member", confirm=pk)["token"]
    )
    bob_id = bob.json.auth.me()["accountId"]
    try:
        members = alice.json.orgs.members(org=org, detail="security")["members"]
    except CloudError as e:
        # A node before core #4902: no `GET /orgs/{org}/members` (a bare
        # `404`, or `405` beside the existing POST).
        if e.args[1] not in (404, 405) or code(e) == "NOT_FOUND":
            raise
        pytest.skip("the node has no member security listing (core #4902)")
    assert len(members) == 2, members
    b = next(m for m in members if m["account"] == bob_id)
    assert (b["role"], b["passkeys"], b["recoveryCode"], b["sessions"]) == (
        "member",
        1,
        False,
        1,
    ), b
    assert len(b["lastActiveDay"]) == 10, b
    for m in members:
        assert sorted(m) == sorted(
            [
                "account",
                "name",
                "role",
                "teams",
                "passkeys",
                "recoveryCode",
                "lastActiveDay",
                "sessions",
            ]
        ), m
    # A member is refused; removed, bob is gone from the listing.
    with pytest.raises(CloudError) as e:
        bob.json.orgs.members(org=org, detail="security")["members"]
    assert e.value.args[1] == 403, e.value
    alice.json.orgs.remove_member(org=org, account=bob_id, confirm=pk)
    assert bob_id not in [
        m["account"]
        for m in alice.json.orgs.members(org=org, detail="security")["members"]
    ]


def test_activity_lists_sign_ins_and_approvals(cluster):
    laptop, pk = person(cluster, 0, "alice-activity")
    me = laptop.json.auth.me()
    try:
        listed = laptop.json.auth.activity_list()["activity"]
    except CloudError as e:
        # A node before core #4901: no `/auth/activity` (a bare `404`).
        if e.args[1] != 404 or code(e) == "NOT_FOUND":
            raise
        pytest.skip("the node has no activity list (core #4901)")
    assert [(a["kind"], a["passkey"], a["app"]) for a in listed] == [
        ("signIn", me["passkeyId"], None)
    ], listed
    signed_up = listed[0]["at"]

    laptop.json.auth.apikey_create(label="ci", confirm=pk)
    listed = laptop.json.auth.activity_list()["activity"]
    assert [(a["kind"], a["method"]) for a in listed] == [
        ("approval", "__create_api_key"),
        ("signIn", None),
    ], listed
    assert all(a["passkey"] == me["passkeyId"] for a in listed), listed
    assert all(
        a["at"] > signed_up
        for a in laptop.json.auth.activity_list(after=signed_up)["activity"]
    )

    # Each account reads only its own.
    bob, _ = person(cluster, 1, "bob-activity")
    bobs = bob.json.auth.activity_list()["activity"]
    assert len(bobs) == 1 and bobs[0]["passkey"] != me["passkeyId"], bobs


def test_blobs_presence_transfer_and_upgrade_runs_have_typed_calls(cluster, wasm):
    alice, pk = person(cluster, 0, "alice-typed")
    bob, _ = person(cluster, 1, "bob-typed")
    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    app = alice.json.apps.create(name="typed", bundle=bundle)["appId"]

    up = alice.json.apps.blob_upload(app=app, body=b"hello " * 100)
    listed = alice.json.apps.blob_list(app=app)
    assert any(b["id"] == up["id"] for b in listed["blobs"]), listed
    assert alice.json.apps.blob_info(app=app, id=up["id"])["size"] == 600
    alice.json.apps.blob_delete(app=app, id=up["id"])
    with pytest.raises(CloudError) as e:
        alice.json.apps.blob_info(app=app, id=up["id"])
    assert code(e.value) == "NOT_FOUND"

    alice.json.apps.presence_publish(app=app, seq=1, state=(b"typing").hex())
    seen = alice.json.apps.presence_get(app=app)
    assert seen, seen
    alice.json.apps.presence_publish(app=app, seq=2)

    with pytest.raises(CloudError) as e:
        alice.json.orgs.upgrade_status(
            org=alice.json.orgs.create(name="Runs")["orgId"], run="no-such-run"
        )
    assert code(e.value) == "NOT_FOUND"

    bob_id = bob.json.auth.me()["accountId"]
    alice.json.apps.transfer(app=app, to=bob_id, confirm=pk)
    assert bob.get(f"apps/{app}")["owner"] == bob_id


def test_an_app_moves_with_its_people_into_another_org(cluster, wasm):
    alice, pk = person(cluster, 0, "alice-move")
    bob, bob_pk = person(cluster, 1, "bob-move")
    bob_id = bob.json.auth.me()["accountId"]
    src = alice.json.orgs.create(name="MoveSource")["orgId"]
    dst = alice.json.orgs.create(name="MoveTarget")["orgId"]
    alice.json.orgs.set_member(org=src, account=bob_id, role="member", confirm=pk)
    app = alice.json.apps.create(
        name="moving",
        bundle=alice.json.packages.bundle_put(body=wasm)["hash"],
        **{"org": src},
    )["appId"]
    alice.json.apps.call(app=app, method="set", args={"key": "a", "value": "1"})
    try:
        moved = alice.json.apps.move(
            app=app, to_org=dst, confirm=pk, bring_members=True
        )
    except CloudError as e:
        # A node before core's `feat/move-app-to-org`: no `/apps/{app}/move`
        # (a bare `404`).
        if e.args[1] != 404 or code(e) == "NOT_FOUND":
            raise
        pytest.skip("the node has no app moves (core feat/move-app-to-org)")
    assert moved["appId"] == app and moved["from"] == src, moved
    assert moved["org"] == dst and moved["added"] == [bob_id], moved
    assert moved["access"] == {"kind": "org"}, moved
    assert alice.get(f"apps/{app}")["org"] == dst
    assert bob.json.apps.view(app=app, method="get", args={"key": "a"})["result"] == "1"
    # Bob, a member of the target only, moves nothing.
    with pytest.raises(CloudError) as e:
        bob.json.apps.move(app=app, to_org=src, confirm=bob_pk)
    assert e.value.args[1] == 403, e.value


def test_one_account_joins_and_leaves_several_teams_with_one_approval(cluster):
    alice, pk = person(cluster, 0, "alice-rows")
    bob, bob_pk = person(cluster, 1, "bob-rows")
    alice_id = alice.json.auth.me()["accountId"]
    bob_id = bob.json.auth.me()["accountId"]
    org = alice.json.orgs.create(name="Rows")["orgId"]
    alice.json.orgs.set_member(org=org, account=bob_id, role="member", confirm=pk)
    chat, blocks, notes = (
        alice.json.orgs.team_create(org=org, name=n, join="invite")["teamId"]
        for n in ("chat", "blocks", "notes")
    )

    def rows(team):
        return [
            m["account"]
            for m in alice.json.orgs.team_get(org=org, team=team)["members"]
        ]

    try:
        alice.json.orgs.member_set_teams(
            org=org,
            account=bob_id,
            teams=[
                t if isinstance(t, dict) else {"team": t}
                for t in [chat, {"team": blocks, "role": "readOnly"}, notes]
            ],
            confirm=pk,
        )
    except CloudError as e:
        # A node before core's `feat/batch-team-members`: no
        # `/orgs/{org}/members/{account}/teams` (a bare `404`).
        if e.args[1] != 404 or code(e) == "NOT_FOUND":
            raise
        pytest.skip("the node has no team batches (core feat/batch-team-members)")
    for team in (chat, blocks, notes):
        assert bob_id in rows(team)
    time.sleep(0.3)
    assert bob.json.orgs.team_get(org=org, team=blocks)["myRole"] == "readOnly"
    # Bob manages none of them: refused, nothing set.
    with pytest.raises(CloudError):
        bob.json.orgs.member_set_teams(
            org=org,
            account=alice_id,
            teams=[t if isinstance(t, dict) else {"team": t} for t in [chat]],
            confirm=bob_pk,
        )

    # Out of two with one approval; he leaves the third himself.
    alice.json.orgs.member_remove_teams(
        org=org, account=bob_id, teams=[chat, blocks], confirm=pk
    )
    assert bob_id not in rows(chat) and bob_id not in rows(blocks)
    assert bob_id in rows(notes)
    bob.json.orgs.member_remove_teams(
        org=org, account=bob_id, teams=[notes], confirm=bob_pk
    )
    time.sleep(0.3)
    assert bob_id not in rows(notes)


def test_receipts_verify_against_the_attested_cluster(cluster, wasm):
    alice, _pk = person(cluster, 0, "alice-receipts")
    app = alice.json.apps.create(
        name="receipted", bundle=alice.json.packages.bundle_put(body=wasm)["hash"]
    )["appId"]
    r = alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})
    attested = alice.verify_app_cluster(app)
    assert sorted(attested["keys"]) == [1, 2, 3], attested
    assert attested["governance"] == {
        "releases": "owner",
        "appUpgrades": "owner",
        "outbound": "owner",
    }
    receipt = r["receipt"]
    assert receipt["key"] == attested["keys"][receipt["node"]]


def test_session_aliases_metadata_and_usage(cluster, wasm):
    alice, pk = person(cluster, 0, "alice-more")
    session = alice.session()
    assert session["accountId"] == alice.json.auth.me()["accountId"]
    agent = cluster.client(1)
    assert agent.session() is None

    bundle = alice.json.packages.bundle_put(body=wasm)["hash"]
    app = alice.json.apps.create(name="more", bundle=bundle)["appId"]
    alice.json.auth.alias_set(alias="notes", app=app)
    assert alice.json.auth.alias_get(alias="notes")["appId"] == app
    assert alice.json.auth.alias_list()["aliases"] == {"notes": app}
    alice.json.auth.alias_delete(alias="notes")
    assert alice.json.auth.alias_list()["aliases"] == {}

    alice.json.apps.metadata(app=app, metadata={"colour": "teal"})
    assert alice.json.apps.get(app=app)["metadata"] == {"colour": "teal"}
    alice.json.apps.metadata(app=app, metadata=None)
    assert alice.json.apps.get(app=app).get("metadata") is None

    alice.json.apps.call(app=app, method="set", args={"key": "k", "value": "v"})
    assert alice.json.apps.storage(app=app)["keys"] >= 1
    assert "abi" in alice.json.apps.abi(app=app)
    assert "quotas" in alice.json.auth.usage() or alice.json.auth.usage()
    assert isinstance(alice.json.packages.list()["packages"], list)

    alice.json.auth.passkey_label(cred=pk.credential_id, label="laptop")
    alice.json.auth.passkey_label(cred=pk.credential_id, label="")

    org = alice.json.orgs.create(name="Meta")["orgId"]
    alice.json.orgs.metadata(org=org, metadata={"plan": "team"})
    assert alice.json.orgs.get(org=org)["metadata"] == {"plan": "team"}
    alice.json.orgs.member_metadata(
        org=org, account=alice.json.auth.me()["accountId"], metadata={"title": "owner"}
    )
    alice.json.orgs.delete(org=org, confirm=pk)
    with pytest.raises(CloudError) as e:
        alice.json.orgs.get(org=org)
    assert code(e.value) == "NOT_FOUND"

    alice.json.apps.delete(app=app)
    time.sleep(0.3)
    with pytest.raises(CloudError) as e:
        alice.json.apps.get(app=app)
    assert code(e.value) == "NOT_FOUND"


def test_presence_streams_a_snapshot_then_changes(cluster, wasm):
    alice, _pk = person(cluster, 0, "alice-presence")
    app = alice.json.apps.create(
        name="here", bundle=alice.json.packages.bundle_put(body=wasm)["hash"]
    )["appId"]
    alice.json.apps.presence_publish(app=app, seq=1, state=(b"typing").hex())
    frames = alice.presence_stream(app, limit=1, timeout=5)
    assert frames and frames[0]["kind"] == "snapshot", frames


# --------------------------------------------------- per-package web origins

NOTES = "com.test.py-notes"
OTHER = "com.test.py-other"
# The developer's own site for NOTES, and the rp-id the node resolves for it.
SITE = "https://notes.example.com"
SITE_RP = "notes.example.com"
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b64url(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def base58(b: bytes) -> str:
    n, out = int.from_bytes(b, "big"), ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + out


def mpk(package: str, version: str, wasm: bytes) -> bytes:
    """A signed single-service `.mpk` of `package@version`, as core's
    `mero-sign` makes one: the manifest canonicalized (RFC 8785; plain
    strings and integers, so sorted compact JSON is the same bytes), signed
    with Ed25519 over `sha256("calimero.bundle.manifest.v1\\0" || bytes)`.
    The WASM gets a custom section naming the package, so each package
    ships its own."""
    key = Ed25519PrivateKey.from_private_bytes(bytes([5] * 32))
    public = key.public_key().public_bytes_raw()
    tag = package.encode()
    wasm = wasm + bytes([0, 1 + len(tag), len(tag)]) + tag
    manifest = {
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": "did:key:z" + base58(bytes([0xED, 0x01]) + public),
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "hash": hashlib.sha256(wasm).hexdigest(),
            "size": len(wasm),
        },
    }
    canonical = json.dumps(
        manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()
    payload = hashlib.sha256(b"calimero.bundle.manifest.v1\0" + canonical).digest()
    manifest["signature"] = {
        "algorithm": "ed25519",
        "publicKey": b64url(public),
        "signature": b64url(key.sign(payload)),
    }
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w:gz") as tar:
        for name, data in (
            ("manifest.json", json.dumps(manifest).encode()),
            ("app.wasm", wasm),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return out.getvalue()


@pytest.fixture(scope="module")
def origins(cluster):
    """Skip on a node without per-package origins (before core #4846): an
    unknown route there is a bare `404`, an unknown package here
    `NOT_FOUND`."""
    probe, _ = person(cluster, 0, "origins-probe")
    try:
        probe.json.packages.origins_list(package="com.test.none")
    except CloudError as e:
        if code(e) != "NOT_FOUND":
            pytest.skip("the node has no per-package origins (core #4846)")


def test_a_packages_manager_registers_and_removes_its_web_origin(
    cluster, wasm, origins
):
    # The first installer manages a package: alice NOTES, bob OTHER.
    alice, alice_pk = person(cluster, 0, "alice-origins")
    bob, bob_pk = person(cluster, 1, "bob-origins")
    alice.json.packages.upload(body=mpk(NOTES, "1.0.0", wasm))
    bob.json.packages.upload(body=mpk(OTHER, "1.0.0", wasm))

    # Bob does not manage NOTES: he does not even see its origins.
    for refused in (
        lambda: bob.json.packages.origins_list(package=NOTES),
        lambda: bob.json.packages.origins_add(
            package=NOTES, origin=SITE, confirm=bob_pk
        ),
    ):
        with pytest.raises(CloudError) as e:
            refused()
        assert e.value.args[1] == 404, e.value
    # Nor is the site anybody's yet.
    with pytest.raises(CloudError) as e:
        cluster.client(2).sign_up(SoftPasskey(SITE_RP, SITE), "early", origin=SITE)
    assert code(e.value) == "ORIGIN_NOT_REGISTERED"

    # Without her passkey the node asks alice first. The step-up is the same
    # shape as every other one: what she approves (`package:{p}` /
    # `__add_origin` / `{origin, rpId}`) is bound inside the challenge, so the
    # retry below succeeding is what proves the node signed that statement.
    with pytest.raises(CloudError) as e:
        alice.post(f"package-origins/{NOTES}", {"origin": SITE})
    assert code(e.value) == "CONFIRM_REQUIRED"
    asked = json.loads(e.value.args[3])
    assert {"challengeId", "challenge", "rpId", "allowCredentials"} <= set(asked)
    with pytest.raises(CloudError) as e:
        alice.json.packages.origins_add(
            package=NOTES, origin=SITE, confirm=alice_pk, rp_id="example.org"
        )
    assert code(e.value) == "BAD_RP_ID"

    added = alice.json.packages.origins_add(
        package=NOTES, origin=SITE, confirm=alice_pk
    )
    assert (added["origin"], added["rpId"]) == (SITE, SITE_RP), added
    listed = alice.json.packages.origins_list(package=NOTES)
    assert [o["origin"] for o in listed["origins"]] == [SITE], listed
    # Another package cannot take it.
    with pytest.raises(CloudError) as e:
        bob.json.packages.origins_add(package=OTHER, origin=SITE, confirm=bob_pk)
    assert code(e.value) == "ORIGIN_TAKEN"

    # Carol signs up on the site: a session bound to NOTES, naming the site.
    carol_pk = SoftPasskey(SITE_RP, SITE)
    carol = cluster.client(2)
    carol.sign_up(carol_pk, "carol-origins", origin=SITE)
    me = carol.json.auth.me()
    assert (me["app"], me["origin"]) == (NOTES, SITE), me
    elsewhere = cluster.client(1)
    elsewhere.sign_in(carol_pk, origin=SITE)
    assert elsewhere.json.auth.me()["origin"] == SITE

    # Removing the origin ends those sessions and their refresh, and the site
    # may not begin a ceremony any more.
    removed = alice.json.packages.origins_remove(
        package=NOTES, origin=SITE, confirm=alice_pk
    )
    assert removed["removed"] is True, removed
    for session in (carol, elsewhere):
        with pytest.raises(LoginRequired):
            session.json.auth.me()
    with pytest.raises(CloudError) as e:
        cluster.client(0).sign_in(carol_pk, origin=SITE)
    assert code(e.value) == "ORIGIN_NOT_REGISTERED"
    assert alice.json.packages.origins_list(package=NOTES)["origins"] == []
    with pytest.raises(CloudError) as e:
        alice.json.packages.origins_remove(package=NOTES, origin=SITE, confirm=alice_pk)
    assert e.value.args[1] == 404, e.value


def test_an_admin_closes_and_opens_one_app_team(cluster, wasm):
    # core #4903: a package's own `join` (and `keep`) in POST /orgs/{org}/app-teams.
    alice, pk = person(cluster, 0, "alice-appteams")
    package = "com.test.appteams"
    alice.json.packages.upload(body=mpk(package, "1.0.0", wasm))
    org = alice.json.orgs.create(name="Appteams")["orgId"]
    assert alice.json.orgs.app_team_list(org=org) == {"mode": "legacy", "appTeams": []}

    made = alice.json.orgs.app_teams_scope(
        org=org, confirm=pk, apps=[{"package": package, "instances": []}]
    )
    assert made["mode"] == "standalone" and made["teams"][0]["created"]
    closed = alice.json.orgs.app_teams_scope(
        org=org, confirm=pk, apps=[{"package": package, "join": "invite", "keep": []}]
    )
    assert closed["mode"] == "shared" and closed["teams"][0]["join"] == "invite"
    assert alice.json.orgs.app_team_list(org=org)["appTeams"][0]["join"] == "invite"
    opened = alice.json.orgs.app_teams_scope(
        org=org, confirm=pk, apps=[{"package": package, "join": "everyone"}]
    )
    assert opened["mode"] == "standalone"
    with pytest.raises(CloudError):
        alice.json.orgs.app_teams_scope(
            org=org,
            confirm=pk,
            apps=[{"package": package, "join": "everyone"}],
            shared=True,
        )
