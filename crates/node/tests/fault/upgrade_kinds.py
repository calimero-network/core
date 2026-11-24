"""The system log entries the upgrade cases' seeds leave out, written the way a user or the operator writes
them, for `upgrade_fixture.py`: the fixture's log holds an entry of every kind (`upgrade_replay.rs`), and a
kind the seeds do not reach is written here. Each step makes its own accounts and objects.

The kinds no step reaches on one node are `NOT_RECORDED` in `upgrade_replay.rs`, each with why.
"""

from __future__ import annotations

import base64
import time
import uuid

import ed25519_sign
from rv2 import HttpError
from upgrade_deep import install, join, new_org, signed_ops, ops, people, team_id

FAR = "2099-01-01T00:00:00Z"  # a time no run reaches: a deadline or an invite that never ends


def organization(w):
    """A rename, an app on every new team, the default capabilities, the guest switch, a member's rows
    in two teams set at once and one taken away."""
    owner, member = people(w, "ona", "mel")
    org = new_org(owner, "Kinds Org")
    join(owner, org, member)
    teams = [team_id(owner.raw("post", f"orgs/{org}/teams", {"name": n, "join": "invite"})) for n in ("one", "two")]
    owner.raw("post", f"orgs/{org}/name", {"name": "Kinds Org, renamed"})
    owner.raw("post", f"orgs/{org}/all-apps", {"account": member.id, "allApps": True})
    owner.raw("post", f"orgs/{org}/capabilities", {"capabilities": 1 << 2 | 1 << 1})
    owner.raw("post", f"orgs/{org}/settings", {"allowGuests": False})
    owner.raw("post", f"orgs/{org}/members/{member.id}/teams", {"teams": [{"team": t} for t in teams]})
    owner.raw("delete", f"orgs/{org}/members/{member.id}/teams", {"teams": teams[1:]})


def org_recovery(w):
    """An organization's recovery started by a member and cancelled by its owner."""
    owner, member = people(w, "ora", "mik")
    org = new_org(owner, "Recovery Org")
    join(owner, org, member)
    seed = bytes(range(2, 34))
    owner.raw("post", f"orgs/{org}/recovery", {"publicKey": base64.urlsafe_b64encode(
        ed25519_sign.public_key(seed)).rstrip(b"=").decode()})
    nonce = uuid.uuid4().hex
    msg = b"calimero.org-recovery.v1\0" + f"{org}\0{member.id}\0{nonce}".encode()
    sig = base64.urlsafe_b64encode(ed25519_sign.sign(seed, msg)).rstrip(b"=").decode()
    member.raw("post", f"orgs/{org}/recovery/start", {"nonce": nonce, "signature": sig}, approve=False)
    owner.raw("post", f"orgs/{org}/recovery/cancel", {})


def account(w):
    """A recovery code made and removed; a session's refresh token used, then used again."""
    p = people(w, "ari")[0]
    p.raw("post", "auth/recovery", {})
    p.raw("delete", "auth/recovery", {})
    token = p.session()["refresh_token" if "refresh_token" in p.session() else "refreshToken"]
    p.http("POST", "auth/refresh", {"refreshToken": token})
    try:
        p.http("POST", "auth/refresh", {"refreshToken": token})
    except HttpError:
        pass  # the reuse is what the node logs


def app(w):
    """An instance made, an invite for it revoked, the instance deleted."""
    p = people(w, "abe")[0]
    package, version = install(p, w, "kv-a-1.0.0.mpk")
    made = p.raw("post", "apps", {"name": "gone", "package": package, "version": version})["appId"]
    p.raw("post", f"apps/{made}/invites", {"uses": 1})
    key = p.get(f"apps/{made}/invites")["invites"][0]["inviteKey"]
    p.raw("delete", f"apps/{made}/invites/{key}")
    p.raw("delete", f"apps/{made}")


def deployment(w):
    """A deployment's secret cleared, an offer withdrawn, the deployment deleted."""
    p = people(w, "dee")[0]
    package, version = install(p, w, "kv-secret-1.0.0.mpk")
    d = p.raw("post", "deployments", {"name": "dee-suite", "owner": {"account": p.id}})["id"]
    p.raw("post", f"deployments/{d}/apps", {"app": "keys", "package": package, "version": version})
    p.raw("put", f"deployments/{d}/secrets/API_KEY", {"value": "dee-key"})
    p.raw("delete", f"deployments/{d}/secrets/API_KEY")
    p.raw("delete", f"deployments/{d}/apps/keys")
    p.raw("delete", f"deployments/{d}")


def outbound(w):
    """An organization's outbound approval for a package that declares a prefix."""
    p = people(w, "olo")[0]
    package, _ = install(p, w, "kv-secret-1.0.0.mpk")
    org = new_org(p, "Outbound Kinds Org")
    p.raw("post", f"orgs/{org}/outbound", {"package": package, "prefixes": ["https://api.example.com/"],
                                           "replace": True})


def origin(w):
    """A frontend origin of a package registered and removed again."""
    p = people(w, "ora")[0]
    package, _ = install(p, w, "kv-k-1.0.0.mpk")
    p.raw("post", f"package-origins/{package}", {"origin": "https://kinds.example.com"})
    p.raw("delete", f"package-origins/{package}", {"origin": "https://kinds.example.com"})


def operator_decisions(w):
    """The operator's sign-up policy and invite, price table, release approval and deadline."""
    signed_ops(w, "POST", "ops/sign-up", {"policy": "invite"})
    month = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 30 * 24 * 3600))
    made = signed_ops(w, "POST", "ops/sign-up/invites", {"token": uuid.uuid4().hex, "uses": 2, "expiresAt": month})
    key = made["inviteKey"]
    signed_ops(w, "POST", f"ops/sign-up/invites/{key}/revoke", {})
    signed_ops(w, "POST", "ops/sign-up", {"policy": "open"})
    table = {"version": (ops(w, "GET", "ops/prices").get("version") or 0) + 1, "fuelPerVcpuSecond": 1,
             "vcpuSecond": 1, "gibSecond": 1, "connectorGib": 1, "storageGibMonth": 1, "egressGib": 1}
    signed_ops(w, "POST", "ops/prices", {"table": table})
    signed_ops(w, "POST", "ops/releases", {"version": "9.9.9", "approved": True})
    signed_ops(w, "POST", "ops/releases", {"version": "9.9.9", "approved": False})
    signed_ops(w, "POST", "ops/releases/9.9.9/deadline", {"after": FAR})
    signed_ops(w, "DELETE", "ops/releases/9.9.9/deadline")


def fleet(w, job):
    """The job node removed from the directory, then the one-way switch to granted deployments."""
    w.fleet.retire_job(job)
    signed_ops(w, "POST", "ops/deployments/granted", {})


def record_every_kind(w, job):
    for step in (organization, org_recovery, account, app, deployment, outbound, operator_decisions):
        step(w)
    fleet(w, job)
