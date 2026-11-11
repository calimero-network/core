"""Deep cases: organizations: members and roles, capabilities, invites, ownership,
deletion and the recovery key."""

from __future__ import annotations

import uuid

import ed25519_sign
from rv2 import b64url
from upgrade_deep import (DeepCase, check, decisions, eq, invite, join, kv_wasm, new_org, people, person,
                          refused, roles, same_decisions, same_read, snap, team_id)
from upgrade_path import case


@case
class Members(DeepCase):
    name = "org-members-roles"
    area = "Organizations"
    seeds = ("an organization with metadata, an admin, members (one granted CREATE_TEAM), a guest, member "
             "metadata, a removed member, a member who left, an organization app")
    verifies = ("the organization as the owner, a member and a guest read it; the members' security view; "
                "who reaches the app (members yes; removed, departed and outsider no; the guest as before) "
                "and who may read security and write metadata; CREATE_TEAM makes a team on the new release, "
                "a plain member is refused; metadata written there")

    def populate(self, w):
        olga, adam, mia, mox, gus, rex, lea, otto = people(w, "olga", "adam", "mia", "mox", "gus", "rex", "lea", "otto")
        del otto  # an outsider: signed up, in no organization
        org = new_org(olga, "Olga Inc")
        olga.raw("post", f"orgs/{org}/metadata", {"metadata": {"plan": "pro", "seats": 9}})
        join(olga, org, adam, role="admin")
        join(olga, org, mia, mox, rex, lea)
        join(olga, org, gus, role="guest")
        olga.raw("post", f"orgs/{org}/members/{mia.id}/metadata", {"metadata": {"desk": "4F"}})
        olga.raw("post", f"orgs/{org}/capabilities", {"account": mia.id, "capabilities": (1 << 5) | (1 << 2)})
        app = olga.deploy(kv_wasm(w), "org-app", org=org)
        mia.call(app, "set", {"key": "by", "value": "mia"})
        olga.raw("delete", f"orgs/{org}/members/{rex.id}")
        lea.raw("delete", f"orgs/{org}/members/{lea.id}")
        s = {"org": org, "app": app, "made": 0}
        self._record(w, s)
        s["decisions"] = self._decide(w, s)
        check(s["decisions"]["mia writes"] == "ok" and s["decisions"]["rex reads"] != "ok",
              f"organization access as seeded: {s['decisions']}")
        return s

    def _record(self, w, s):
        s["reads"] = {who: snap(person(w, who), f"orgs/{s['org']}") for who in ("olga", "mia", "gus")}
        s["security"] = snap(person(w, "olga"), f"orgs/{s['org']}/members?detail=security")

    def _decide(self, w, s):
        app, org = s["app"], s["org"]
        p = lambda n: person(w, n)  # noqa: E731
        return decisions({
            "mia writes": lambda: p("mia").call(app, "set", {"key": "m", "value": "1"}),
            "mox reads": lambda: p("mox").view(app, "get", {"key": "by"}),
            "gus reads": lambda: p("gus").view(app, "get", {"key": "by"}),
            "rex reads": lambda: p("rex").view(app, "get", {"key": "by"}),
            "lea reads": lambda: p("lea").view(app, "get", {"key": "by"}),
            "otto reads": lambda: p("otto").view(app, "get", {"key": "by"}),
            "otto org": lambda: p("otto").get(f"orgs/{org}"),
            "rex org": lambda: p("rex").get(f"orgs/{org}"),
            "mox security": lambda: p("mox").get(f"orgs/{org}/members?detail=security"),
            "adam security": lambda: p("adam").get(f"orgs/{org}/members?detail=security"),
            "mox org metadata": lambda: p("mox").raw("post", f"orgs/{org}/metadata", {"metadata": {"plan": "x"}}),
        })

    def check(self, w, s):
        for who, want in s["reads"].items():
            same_read(person(w, who), f"orgs/{s['org']}", want, "the organization")
        same_read(person(w, "olga"), f"orgs/{s['org']}/members?detail=security", s["security"], "members' security")
        same_decisions(self._decide(w, s), s["decisions"], "organization access")
        eq(person(w, "mox").result(s["app"], "get", {"key": "by"}), "mia", "the organization app's data")

    def exercise(self, w, s):
        olga, mia, mox = people(w, "olga", "mia", "mox")
        s["made"] += 1
        made = mia.raw("post", f"orgs/{s['org']}/teams", {"name": f"mia-{s['made']}", "join": "invite"})
        check(team_id(made), f"a team made with CREATE_TEAM on the new release: {made}")
        refused(("REJECTED", "FORBIDDEN"),
                lambda: mox.raw("post", f"orgs/{s['org']}/teams", {"name": "mox", "join": "invite"}),
                "a member without CREATE_TEAM makes a team")
        olga.raw("post", f"orgs/{s['org']}/metadata", {"metadata": {"plan": "enterprise"}})
        self._record(w, s)
        eq(s["reads"]["olga"].get("metadata"), {"plan": "enterprise"}, "metadata written on the new release")


@case
class Invites(DeepCase):
    name = "org-invites"
    area = "Organizations"
    seeds = ("invites: a multi-use one used once, an unused single-use one, a used-up one, a revoked one, "
             "a guest invite into one team (app-only access)")
    verifies = ("the invite list; the revoked and used-up invites are refused; the guest reaches its team's "
                "app only; the unused and multi-use invites made on the old release redeem on the new one")

    def populate(self, w):
        ivy, jo, kay, lee, ned, gil = people(w, "ivy", "jo", "kay", "lee", "ned", "gil")
        del kay, ned  # they redeem on the new release
        org = new_org(ivy, "Ivy Ltd")
        team = team_id(ivy.raw("post", f"orgs/{org}/teams", {"name": "support", "join": "invite"}))
        app = ivy.deploy(kv_wasm(w), "support-app", org=org)
        other = ivy.deploy(kv_wasm(w), "org-wide", org=org)
        ivy.raw("post", f"apps/{app}/access", {"access": {"kind": "team", "team": team}})
        for a in (app, other):
            ivy.call(a, "set", {"key": "t", "value": "1"})
        multi = invite(ivy, org, uses=3)
        jo.raw("post", f"invites/{multi}/redeem", {})
        single = invite(ivy, org, uses=1)
        used = invite(ivy, org, uses=1)
        lee.raw("post", f"invites/{used}/redeem", {})
        listed = {i.get("inviteKey") for i in ivy.get(f"orgs/{org}/invites")["invites"]}
        revoked = invite(ivy, org)
        new_key = ({i.get("inviteKey") for i in ivy.get(f"orgs/{org}/invites")["invites"]} - listed).pop()
        ivy.raw("delete", f"orgs/{org}/invites/{new_key}")
        gil.raw("post", f"invites/{invite(ivy, org, role='guest', teams=[team])}/redeem", {})
        s = {"org": org, "team": team, "app": app, "other": other, "multi": multi, "single": single,
             "used": used, "revoked": revoked, "invites": snap(ivy, f"orgs/{org}/invites"), "redeemed": False}
        s["decisions"] = self._decide(w, s)
        return s

    def _decide(self, w, s):
        gil, ned = people(w, "gil", "ned")
        return decisions({
            "gil reads team app": lambda: gil.view(s["app"], "get", {"key": "t"}),
            "gil writes team app": lambda: gil.call(s["app"], "set", {"key": "g", "value": "1"}),
            "gil reads org app": lambda: gil.view(s["other"], "get", {"key": "t"}),
            "ned reads org app": lambda: ned.view(s["other"], "get", {"key": "t"}),
        })

    def check(self, w, s):
        ivy, ned = people(w, "ivy", "ned")
        if not s["redeemed"]:
            same_read(ivy, f"orgs/{s['org']}/invites", s["invites"], "invites")
            same_decisions(self._decide(w, s), s["decisions"], "invited access")
        for label in ("revoked", "used"):
            refused(("REJECTED", "NOT_FOUND", "BAD_REQUEST", "FORBIDDEN", "GONE"),
                    lambda t=s[label]: person(w, "otto2").raw("post", f"invites/{t}/redeem", {}),
                    f"a {label} invite redeemed")
        eq(roles(ivy, s["org"]).get(person(w, "gil").id), "guest", "the guest's role")
        if not s["redeemed"]:
            check(ned.id not in roles(ivy, s["org"]), "ned joined without redeeming")

    def exercise(self, w, s):
        ivy, kay, ned = people(w, "ivy", "kay", "ned")
        kay.raw("post", f"invites/{s['single']}/redeem", {})
        ned.raw("post", f"invites/{s['multi']}/redeem", {})
        got = roles(ivy, s["org"])
        eq((got.get(kay.id), got.get(ned.id)), ("member", "member"), "invites made on the old release, redeemed")
        eq(ned.result(s["other"], "get", {"key": "t"}), "1", "a member who joined on the new release reads")
        refused(("REJECTED", "NOT_FOUND", "BAD_REQUEST", "FORBIDDEN", "GONE"),
                lambda: person(w, "otto2").raw("post", f"invites/{s['single']}/redeem", {}),
                "a single-use invite used on the new release, used again")
        s["redeemed"] = True


@case
class Ownership(DeepCase):
    name = "org-ownership"
    area = "Organizations"
    seeds = "an organization whose owner handed it to a member"
    verifies = "the roles after the hand-over; the former owner is refused owner-only actions; the new owner changes a role on the new release"

    def populate(self, w):
        owen, tia = people(w, "owen", "tia")
        org = new_org(owen, "Owen Co")
        join(owen, org, tia)
        owen.raw("post", f"orgs/{org}/members", {"account": tia.id, "role": "owner"})
        got = roles(tia, org)
        eq(got.get(tia.id), "owner", "tia owns it")
        return {"org": org, "roles": got}

    def check(self, w, s):
        owen, tia = people(w, "owen", "tia")
        eq(roles(tia, s["org"]), s["roles"], "roles after a hand-over")
        refused(("REJECTED", "FORBIDDEN"), lambda: owen.raw("delete", f"orgs/{s['org']}"),
                "the former owner deletes the organization")
        refused(("REJECTED", "FORBIDDEN"),
                lambda: owen.raw("post", f"orgs/{s['org']}/members", {"account": owen.id, "role": "owner"}),
                "the former owner takes it back")

    def exercise(self, w, s):
        tia, owen = people(w, "tia", "owen")
        tia.raw("post", f"orgs/{s['org']}/members", {"account": owen.id, "role": "member"})
        s["roles"][owen.id] = "member"
        self.check(w, s)


@case
class Deleted(DeepCase):
    name = "org-deleted"
    area = "Organizations"
    seeds = "an organization with an app and a team, deleted"
    verifies = "the organization and its app stay gone; the owner makes a new one on the new release"

    def populate(self, w):
        dora = person(w, "dora")
        org = new_org(dora, "Doomed")
        app = dora.deploy(kv_wasm(w), "doomed-app", org=org)
        dora.raw("post", f"orgs/{org}/teams", {"name": "t", "join": "everyone"})
        dora.raw("delete", f"orgs/{org}")
        return {"org": org, "app": app}

    def check(self, w, s):
        dora = person(w, "dora")
        refused("NOT_FOUND", lambda: dora.get(f"orgs/{s['org']}"), "a deleted organization")
        refused("NOT_FOUND", lambda: dora.view(s["app"], "len"), "a deleted organization's app")
        check(all(o["orgId"] != s["org"] for o in dora.get("auth/me")["orgs"]), "the deleted organization is listed")

    def exercise(self, w, s):
        dora = person(w, "dora")
        org = new_org(dora, "Reborn")
        check(any(o["orgId"] == org for o in dora.get("auth/me")["orgs"]), "a new organization on the new release")
        self.check(w, s)


@case
class RecoveryKey(DeepCase):
    name = "org-recovery-key"
    area = "Organizations"
    seeds = "an organization with a recovery key, and a recovery a member started with it on the old release"
    verifies = ("the recovery key and the pending recovery; finishing early and a recovery signed with another "
                "key are refused; the owner keeps the organization; the owner cancels it on the new release and "
                "the member starts it again")

    SEED = bytes(range(1, 33))

    def _start(self, w, s, who):
        p = person(w, who)
        nonce = uuid.uuid4().hex
        msg = b"calimero.org-recovery.v1\0" + f"{s['org']}\0{p.id}\0{nonce}".encode()
        sig = b64url(ed25519_sign.sign(self.SEED, msg))
        return p.raw("post", f"orgs/{s['org']}/recovery/start", {"nonce": nonce, "signature": sig}, approve=False)

    def populate(self, w):
        rhea, rob = people(w, "rhea", "rob")
        org = new_org(rhea, "Rhea Org")
        join(rhea, org, rob)
        rhea.raw("post", f"orgs/{org}/recovery", {"publicKey": b64url(ed25519_sign.public_key(self.SEED))})
        s = {"org": org}
        started = self._start(w, s, "rob")
        check(started.get("readyAt"), f"recovery/start: {started}")
        s["recovery"] = rhea.get(f"orgs/{org}").get("recovery")
        check(s["recovery"], f"no recovery in the organization after a start: {rhea.get(f'orgs/{org}')}")
        return s

    def check(self, w, s):
        rhea, rob = people(w, "rhea", "rob")
        eq(rhea.get(f"orgs/{s['org']}").get("recovery"), s["recovery"], "the recovery key and the pending recovery")
        refused(("RECOVERY_NOT_READY", "CONFLICT"), lambda: rob.raw("post", f"orgs/{s['org']}/recovery/finish", {}),
                "a recovery finished before readyAt")
        bad = {"nonce": "n1", "signature": b64url(ed25519_sign.sign(bytes(32), b"x"))}
        refused(("WRONG_RECOVERY_KEY", "FORBIDDEN"),
                lambda: rob.raw("post", f"orgs/{s['org']}/recovery/start", bad, approve=False),
                "a recovery signed with another key")
        eq(roles(rhea, s["org"]).get(rhea.id), "owner", "the owner while a recovery is pending")

    def exercise(self, w, s):
        rhea = person(w, "rhea")
        rhea.raw("post", f"orgs/{s['org']}/recovery/cancel", {})
        self._start(w, s, "rob")
        s["recovery"] = rhea.get(f"orgs/{s['org']}").get("recovery")
        self.check(w, s)
