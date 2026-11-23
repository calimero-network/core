"""Deep cases: packages (installs, versions, uninstalls, dev bundles), publisher bindings,
app upgrades with a state migration and organization-wide upgrade runs."""

from __future__ import annotations

import json
import time

import ed25519_sign
from rv2 import b64url
from upgrade_deep import (DeepCase, Failed, check, decision, eq, install, join, kv_wasm, new_org, people,
                          person, refused, same_read, snap, subset, until)
from upgrade_path import case


@case
class Packages(DeepCase):
    name = "packages"
    area = "Packages"
    seeds = ("two signed releases installed from the registry and the second uninstalled again; an app on "
             "the first with data; an app from a dev bundle (raw component)")
    verifies = ("the package and its versions; both apps answer from the new nodes; on the new release the "
                "uninstalled version installs again and the app upgrades to it keeping its data; the release's "
                "capability card reads with its ABI")

    def populate(self, w):
        pia = person(w, "pia")
        p, v1 = install(pia, w, "kv-a-1.0.0.mpk")
        _, v2 = install(pia, w, "kv-a-1.1.0.mpk")
        app = pia.raw("post", "apps", {"name": "pkg-app", "package": p, "version": v1})["appId"]
        pia.call(app, "set", {"key": "release", "value": v1})
        pia.raw("delete", f"packages/{p}/{v2}")
        dev = pia.deploy(kv_wasm(w), "dev-bundle")
        pia.call(dev, "set", {"key": "dev", "value": "yes"})
        s = {"package": p, "v1": v1, "v2": v2, "app": app, "dev": dev}
        self._record(w, s)
        check(v2 not in json.dumps(s["read"]), f"version {v2} still installed: {s['read']}")
        return s

    def _record(self, w, s):
        pia = person(w, "pia")
        s["read"] = snap(pia, f"packages/{s['package']}")
        s["apps"] = {a: snap(pia, f"apps/{a}") for a in (s["app"], s["dev"])}

    def check(self, w, s):
        pia = person(w, "pia")
        same_read(pia, f"packages/{s['package']}", s["read"], "the package")
        for a, rec in s["apps"].items():
            same_read(pia, f"apps/{a}", rec, "an app")
        eq(pia.result(s["app"], "get", {"key": "release"}), s["v1"], "the package app's data")
        eq(pia.result(s["dev"], "get", {"key": "dev"}), "yes", "the dev bundle app's data")

    def exercise(self, w, s):
        pia = person(w, "pia")
        # Her two instances made her an organization each: an install names one.
        org = pia.get(f"apps/{s['app']}")["org"]
        pia.raw("post", "packages/install", {"package": s["package"], "version": s["v2"], "org": org})
        pia.raw("post", f"apps/{s['app']}/upgrade", {"version": s["v2"]})
        check(s["v2"] in json.dumps(pia.get(f"apps/{s['app']}")), f"the app runs {s['v2']} after the upgrade")
        eq(pia.result(s["app"], "get", {"key": "release"}), s["v1"], "data across an upgrade on the new release")
        card = pia.get(f"packages/{s['package']}/{s['v1']}/capabilities")
        eq((card["package"], card["version"]), (s["package"], s["v1"]), "the release's capability card")
        check({"get", "set"} <= set(card["abi"]["methods"]), f"the card's ABI lacks the kv methods: {card['abi']}")
        self._record(w, s)


@case
class ReleaseVisibility(DeepCase):
    name = "release-visibility"
    area = "Packages"
    seeds = ("kv-b 1.0.0 installed from the registry by an account whose organization holds a second member, "
             "who never installed it (a release recorded before it was public or private)")
    verifies = ("the release from before keeps exactly its rule: the member lists it on every node and, on the "
                "new release, creates an instance of it, and an account sharing no organization with its installer "
                "does not list it on any node; kv-c installed on the new release, which the registry serves "
                "to anyone, is public: an account in no organization lists it and creates an instance of it")

    def populate(self, w):
        ines, ivo = people(w, "ines", "ivo")
        org = new_org(ines, "Ines Packages")
        join(ines, org, ivo)
        p, v = install(ines, w, "kv-b-1.0.0.mpk")
        return {"org": org, "before": [p, v], "after": None}

    def check(self, w, s):
        ivo, isla = people(w, "ivo", "isla")
        p, v = s["before"]
        for n in w.members():
            check(v in json.dumps(ivo.get(f"packages/{p}", node=n)), f"ivo lists {p}@{v} on node {n}")
            refused(("NOT_FOUND", "404"), lambda n=n: isla.get(f"packages/{p}", node=n),
                    f"isla, in no organization of its installer, does not list {p} on node {n}")
        if s["after"]:
            p, v = s["after"]
            check(v in json.dumps(isla.get(f"packages/{p}")), f"isla lists the public {p}@{v}")

    def exercise(self, w, s):
        ines, ivo, isla = people(w, "ines", "ivo", "isla")
        p, v = s["before"]
        ivo.raw("post", "apps", {"name": "vis-before", "package": p, "version": v, "org": s["org"]})
        p, v = install(ines, w, "kv-c-1.0.0.mpk")
        isla.raw("post", "apps", {"name": "vis-after", "package": p, "version": v})
        s["after"] = [p, v]


def _did_key(pub: bytes) -> str:
    """The did:key signer id of an Ed25519 public key (multicodec 0xed01, base58btc)."""
    data = b"\xed\x01" + pub
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    n, out = int.from_bytes(data, "big"), ""
    while n:
        n, r = divmod(n, 58)
        out = alphabet[r] + out
    return "did:key:z" + "1" * (len(data) - len(data.lstrip(b"\0"))) + out


@case
class Publishers(DeepCase):
    name = "publisher-binding"
    area = "Packages"
    seeds = "a package signing key bound to an organization (its publisher)"
    verifies = "the binding and its generation as anyone reads them; a second key bound on the new release"

    def _bind(self, w, s, seed):
        pam = person(w, "pam")
        pub = ed25519_sign.public_key(seed)
        signer = _did_key(pub)
        gen = int(pam.http("GET", f"publishers/{signer}").get("generation") or 0)
        msg = f"calimero-publisher-binding/v1\n{w.fleet.cluster}\n{s['org']}\n{signer}\n{gen}".encode()
        pam.raw("post", f"orgs/{s['org']}/publishers", {"signerId": signer, "publicKey": b64url(pub),
                                                         "signature": b64url(ed25519_sign.sign(seed, msg))})
        s["bindings"][signer] = pam.http("GET", f"publishers/{signer}")
        check(s["bindings"][signer].get("bound") is True, f"the binding of {signer}: {s['bindings'][signer]}")

    def populate(self, w):
        s = {"org": new_org(person(w, "pam"), "Pam Publishing"), "bindings": {}}
        self._bind(w, s, bytes(range(40, 72)))
        return s

    def check(self, w, s):
        for signer, want in s["bindings"].items():
            subset(person(w, "pam").http("GET", f"publishers/{signer}"), want, f"the binding of {signer}")

    def exercise(self, w, s):
        self._bind(w, s, bytes(range(80, 112)))


def _run_id(v):
    return v.get("runId") or v.get("id") or v.get("run")


def _finished(status, apps):
    """The run's status once every app of it is done and the run says it has
    finished: `done` fills before `finished` is set, so a status read between
    the two is not the one every later read answers."""
    if sorted(status.get("done") or []) != sorted(apps) or status.get("finished") is False:
        raise Failed(f"the run is still going: {json.dumps(status)[:300]}")
    return status


@case
class Migrations(DeepCase):
    name = "app-upgrade-migration"
    area = "Upgrades"
    seeds = ("notes 1.0.0 and 2.0.0 (state version 1 to 2) installed; an app upgraded to 2.0.0, one left on "
             "1.0.0, two upgraded by an organization-wide upgrade run")
    verifies = ("each app's release and notes; the run's status as a member reads it; the app left on 1.0.0 "
                "upgrades on the new release keeping its note and gains 2.0.0's methods; a new run there")

    def populate(self, w):
        uli, ula = people(w, "uli", "ula")
        p, v1 = install(uli, w, "notes-1.0.0.mpk")
        _, v2 = install(uli, w, "notes-2.0.0.mpk")
        org = new_org(uli, "Upgrade Org")
        join(uli, org, ula)
        # The apps upgraded one by one live in an organization of their own: since organizations
        # are ordinary (1.0.0-rc.32), an instance naming none goes to its creator's only
        # organization, which would put them in the run's.
        own = new_org(uli, "Own Org")

        def app(name, org=None):
            body = {"name": name, "package": p, "version": v1, **({"org": org} if org else {})}
            a = uli.raw("post", "apps", body)["appId"]
            uli.call(a, "set", {"id": "n", "text": f"{name} on 1.0.0"})
            return a

        s = {"package": p, "v1": v1, "v2": v2, "org": org, "notes": {}}
        s["up"], s["stay"] = app("up", own), app("stay", own)
        run_apps = [app(f"org-{i}", org) for i in range(2)]
        uli.raw("post", f"apps/{s['up']}/upgrade", {"version": v2})
        s["run"] = _run_id(uli.raw("post", f"orgs/{org}/upgrade", {"package": p, "version": v2, "cascade": False}))
        s["status"] = until("the upgrade run finishes",
                            lambda: _finished(snap(uli, f"orgs/{org}/upgrades/{s['run']}"), run_apps), 90)
        for a in (s["up"], s["stay"], *run_apps):
            s["notes"][a] = uli.result(a, "get", {"id": "n"})
            check(s["notes"][a] and "on 1.0.0" in s["notes"][a], f"app {a}'s note: {s['notes'][a]!r}")
        s["records"] = {a: snap(uli, f"apps/{a}") for a in s["notes"]}
        check(v2 in json.dumps(s["records"][run_apps[0]]), f"the run upgraded {run_apps[0]}: {s['records'][run_apps[0]]}")
        return s

    def check(self, w, s):
        uli, ula = people(w, "uli", "ula")
        for a, rec in s["records"].items():
            same_read(uli, f"apps/{a}", rec, f"app {a}")
            eq(uli.result(a, "get", {"id": "n"}), s["notes"][a], f"app {a}'s note")
        same_read(ula, f"orgs/{s['org']}/upgrades/{s['run']}", s["status"], "the upgrade run, as a member")

    def exercise(self, w, s):
        uli = person(w, "uli")
        uli.raw("post", f"apps/{s['stay']}/upgrade", {"version": s["v2"]})
        eq(uli.result(s["stay"], "get", {"id": "n"}), s["notes"][s["stay"]], "the note across an upgrade on the new release")
        check(uli.result(s["stay"], "exists", {"id": "n"}) is True, "2.0.0's `exists` on the app upgraded on the new release")
        s["records"][s["stay"]] = snap(uli, f"apps/{s['stay']}")
        late = uli.raw("post", "apps", {"name": "late", "package": s["package"], "version": s["v1"], "org": s["org"]})["appId"]
        uli.call(late, "set", {"id": "n", "text": "late on 1.0.0"})
        run = _run_id(uli.raw("post", f"orgs/{s['org']}/upgrade", {"package": s["package"], "version": s["v2"],
                                                                    "cascade": False}))
        until("a run on the new release finishes",
              lambda: check(late in (uli.get(f"orgs/{s['org']}/upgrades/{run}").get("done") or []),
                            f"{late} not upgraded yet"), 90)
        s["notes"][late] = "late on 1.0.0"
        s["records"][late] = None
        # The run may touch the organization's other instances too: record them all once
        # two reads a second apart agree.
        def settled():
            first = {a: snap(uli, f"apps/{a}") for a in s["records"]}
            time.sleep(1)
            again = {a: snap(uli, f"apps/{a}") for a in s["records"]}
            eq(again, first, "the apps' records while the run settles")
            return again
        s["records"] = until("the apps settle after the run", settled, 60)
        check(s["v2"] in json.dumps(s["records"][late]), f"the run on the new release upgraded {late}: {s['records'][late]}")


def _notes(w, owner, org_name):
    """notes 1.0.0 and 2.0.0 installed by `owner` (2.0.0 changes the state version: a red
    update) and an organization of theirs: `(package, v1, v2, org)`."""
    p, v1 = install(owner, w, "notes-1.0.0.mpk")
    _, v2 = install(owner, w, "notes-2.0.0.mpk")
    return p, v1, v2, new_org(owner, org_name)


def _notes_app(owner, name, package, version, org, text):
    a = owner.raw("post", "apps", {"name": name, "package": package, "version": version, "org": org})["appId"]
    owner.call(a, "set", {"id": "n", "text": text})
    return a


def _set_v2(owner, app, text):
    """A note written on notes 2.0.0, whose `set` is a `confirm` method: with the owner's passkey."""
    owner.raw("post", f"apps/{app}/intents", {"method": "set", "args": {"id": "n", "title": "n", "text": text}})


def _red(update, version):
    """`GET /apps/{app}/update` once it offers `version` as a red update."""
    available = update.get("available") or {}
    if available.get("version") != version or available.get("class") != "red":
        raise Failed(f"no red update to {version} yet: {json.dumps(update)[:400]}")
    return update


@case
class UpdateApprovals(DeepCase):
    name = "update-classes-approvals"
    area = "Upgrades"
    since = "1.0.0-rc.34"
    seeds = ("notes 1.0.0 and 2.0.0 (red: its state version changes) in an organization whose update policy asks "
             "for a second approver; one app upgraded by an admin on the owner's first approval, one with the "
             "owner's first approval still waiting (`instance approve-update`)")
    verifies = ("the upgraded app's `lastUpgrade` (class red, its reasons, the first approver) and the waiting "
                "approval as the admin reads them on every node; the owner alone is refused "
                "`SECOND_APPROVAL_REQUIRED`; on the new release the admin applies the update the old release's "
                "approval covers, recorded red with that approver, and the app keeps its note")

    def populate(self, w):
        vic, val = people(w, "vic", "val")
        p, v1, v2, org = _notes(w, vic, "Approval Org")
        join(vic, org, val, role="admin")
        vic.raw("post", f"orgs/{org}/upgrade-policy", {"policy": {"secondApprover": True}})
        s = {"org": org, "package": p, "v1": v1, "v2": v2, "notes": {}}
        for k in ("approved", "waiting"):
            s[k] = _notes_app(vic, k, p, v1, org, f"{k} on 1.0.0")
            s["notes"][s[k]] = f"{k} on 1.0.0"
        until("the red update is offered", lambda: _red(val.get(f"apps/{s['waiting']}/update"), v2), 30)
        for a in (s["approved"], s["waiting"]):
            vic.raw("post", f"apps/{a}/upgrade-approvals", {"version": v2})
        val.raw("post", f"apps/{s['approved']}/upgrade", {"version": v2})
        s["records"] = {}
        self._record(w, s, s["approved"])
        self._record(w, s, s["waiting"])
        last = s["records"][s["approved"]]["lastUpgrade"]
        eq((last.get("ok"), last.get("class"), last.get("approvedBy"), last.get("by")), (True, "red", vic.id, val.id),
           "the red update's record")
        check(last.get("reasons"), f"the red update's reasons: {last}")
        s["pending"] = val.get(f"apps/{s['waiting']}/update")["pendingApprovals"]
        check(len(s["pending"]) == 1 and s["pending"][0].get("by") == vic.id
              and s["pending"][0].get("class") == "red", f"the waiting approval: {s['pending']}")
        return s

    def _record(self, w, s, app):
        s["records"][app] = snap(person(w, "vic"), f"apps/{app}")

    def check(self, w, s):
        vic, val = people(w, "vic", "val")
        for a, rec in s["records"].items():
            same_read(vic, f"apps/{a}", rec, f"app {a}")
            eq(vic.result(a, "get", {"id": "n"}), s["notes"][a], f"app {a}'s note")
        for n in w.members():
            eq(val.get(f"apps/{s['waiting']}/update", node=n)["pendingApprovals"], s["pending"],
               f"the waiting approvals on node {n}")
        if s["pending"]:
            eq(decision(lambda: vic.raw("post", f"apps/{s['waiting']}/upgrade", {"version": s["v2"]})),
               "SECOND_APPROVAL_REQUIRED", "the owner alone upgrading the app whose approval waits")
            listed = val.get(f"orgs/{s['org']}/updates")
            check(s["waiting"] in json.dumps(listed), f"the organization's updates lack {s['waiting']}: {listed}")

    def exercise(self, w, s):
        vic, val = people(w, "vic", "val")
        val.raw("post", f"apps/{s['waiting']}/upgrade", {"version": s["v2"]})
        self._record(w, s, s["waiting"])
        last = s["records"][s["waiting"]]["lastUpgrade"]
        eq((last.get("ok"), last.get("class"), last.get("approvedBy"), last.get("by")), (True, "red", vic.id, val.id),
           "the update the old release's approval covered, applied on the new release")
        check(s["v2"] in json.dumps(s["records"][s["waiting"]].get("release")), f"the app runs {s['v2']}: {last}")
        s["pending"] = []
        eq(val.get(f"apps/{s['waiting']}/update")["pendingApprovals"], [], "the approval spent")


def _undone(p, app, version):
    """`app`'s record once its undo has restored it onto `version`."""
    rec = p.get(f"apps/{app}")
    if (rec.get("release") or {}).get("version") != version or rec.get("restore") is not None:
        raise Failed(f"{app} is not restored onto {version} yet: {json.dumps(rec)[:400]}")
    return rec


@case
class RestorePoints(DeepCase):
    name = "restore-points-undo"
    area = "Upgrades"
    since = "1.0.0-rc.34"
    seeds = ("two notes apps upgraded from 1.0.0 to 2.0.0, each taking a restore point, and written to after; "
             "one of them undone back to 1.0.0 and its note from before (`instance undo`)")
    verifies = ("the kept app's restore point and `lastUpgrade` as its owner reads them, its undo offered while "
                "the restore point's node is in the cluster and refused `RESTORE_POINT_UNAVAILABLE` once the roll "
                "took that node; the undone app on 1.0.0 with its note from before and nothing left to undo; on "
                "the new release the undone app is upgraded again, written to and undone back to that note")

    def populate(self, w):
        rex = person(w, "rex")
        p, v1, v2, org = _notes(w, rex, "Restore Org")
        s = {"org": org, "package": p, "v1": v1, "v2": v2, "records": {}, "notes": {}}
        for k in ("kept", "undone"):
            s[k] = _notes_app(rex, k, p, v1, org, "before")
            rex.raw("post", f"apps/{s[k]}/upgrade", {"version": v2})
            _set_v2(rex, s[k], "after")
        point = rex.get(f"apps/{s['kept']}").get("restorePoint") or {}
        eq((point.get("fromRelease") or {}).get("version"), v1, "the kept app's restore point")
        check(isinstance((point.get("file") or {}).get("node"), int), f"the restore point's file: {point}")
        s["point_node"] = point["file"]["node"]
        plan = rex.get(f"apps/{s['kept']}/undo")
        eq(plan.get("available"), True, f"the kept app's undo: {plan}")
        part = plan["apps"][0]
        eq((part["from"]["version"], part["to"]["version"], (part.get("lostWrites") or {}).get("any")),
           (v2, v1, True), "what the kept app's undo reverts")
        self._undo(w, s, s["undone"])
        # 2.0.0 keeps a note as its title and text on two lines.
        s["notes"] = {s["kept"]: "n\nafter", s["undone"]: "before"}
        for a in (s["kept"], s["undone"]):
            s["records"][a] = snap(rex, f"apps/{a}")
        return s

    def _undo(self, w, s, app):
        rex = person(w, "rex")
        # On 1.0.0-rc.34 the restore point's node may be asked to seed before it applied the
        # undo (the race #5221 fixes): `409 CONFLICT` when that node took the request, `503
        # UNAVAILABLE` when another did and its seed call was refused. Until the restore finishes
        # an undo may be sent again, which seeds it again, so send it again as a person would.
        deadline = time.time() + 30
        while (got := decision(lambda: rex.raw("post", f"apps/{app}/undo", {}))) in ("CONFLICT", "UNAVAILABLE"):
            if time.time() >= deadline:
                break
            time.sleep(1)
        eq(got, "ok", f"undoing {app}")
        rec = until("the undo restores the app", lambda: _undone(rex, app, s["v1"]), 90)
        check(rec.get("restorePoint") is None and (rec.get("lastUpgrade") or {}).get("undo") is True,
              f"the undone app's record: {rec}")
        eq(rex.result(app, "get", {"id": "n"}), "before", "the undone app's note")

    def check(self, w, s):
        rex = person(w, "rex")
        for a, rec in s["records"].items():
            same_read(rex, f"apps/{a}", rec, f"app {a}")
            eq(rex.result(a, "get", {"id": "n"}), s["notes"][a], f"app {a}'s note")
        eq((rex.get(f"apps/{s['undone']}/undo").get("reason") or {}).get("code"), "NO_RESTORE_POINT",
           "the undone app's undo")
        plan = rex.get(f"apps/{s['kept']}/undo")
        if s["point_node"] in w.fleet.members:
            eq(plan.get("available"), True, f"the kept app's undo while node {s['point_node']} is in: {plan}")
            eq([(x["from"]["version"], x["to"]["version"]) for x in plan["apps"]], [(s["v2"], s["v1"])],
               "what the kept app's undo reverts")
        else:
            eq((plan.get("available"), (plan.get("reason") or {}).get("code")),
               (False, "RESTORE_POINT_UNAVAILABLE"), f"the kept app's undo once node {s['point_node']} left")
            eq(decision(lambda: rex.raw("post", f"apps/{s['kept']}/undo", {})), "RESTORE_POINT_UNAVAILABLE",
               "undoing the kept app once its restore point's node left")

    def exercise(self, w, s):
        rex = person(w, "rex")
        a = s["undone"]
        rex.raw("post", f"apps/{a}/upgrade", {"version": s["v2"]})
        _set_v2(rex, a, "after, on the new release")
        point = rex.get(f"apps/{a}").get("restorePoint") or {}
        check((point.get("file") or {}).get("node") in w.members(), f"a restore point on a new node: {point}")
        self._undo(w, s, a)
        s["records"][a] = snap(rex, f"apps/{a}")
