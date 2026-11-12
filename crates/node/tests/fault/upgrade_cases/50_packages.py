"""Deep cases: packages (installs, versions, uninstalls, dev bundles), publisher bindings,
app upgrades with a state migration and organization-wide upgrade runs."""

from __future__ import annotations

import json
import time

import ed25519_sign
from rv2 import b64url
from upgrade_deep import (DeepCase, Failed, check, eq, install, join, kv_wasm, new_org, people,
                          person, same_read, snap, subset, until)
from upgrade_path import case


@case
class Packages(DeepCase):
    name = "packages"
    area = "Packages"
    seeds = ("two signed releases installed from the registry and the second uninstalled again; an app on "
             "the first with data; an app from a dev bundle (raw component)")
    verifies = ("the package and its versions; both apps answer from the new nodes; on the new release the "
                "uninstalled version installs again and the app upgrades to it keeping its data")

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
        pia.raw("post", "packages/install", {"package": s["package"], "version": s["v2"]})
        pia.raw("post", f"apps/{s['app']}/upgrade", {"version": s["v2"]})
        check(s["v2"] in json.dumps(pia.get(f"apps/{s['app']}")), f"the app runs {s['v2']} after the upgrade")
        eq(pia.result(s["app"], "get", {"key": "release"}), s["v1"], "data across an upgrade on the new release")
        self._record(w, s)


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

        def app(name, org=None):
            body = {"name": name, "package": p, "version": v1, **({"org": org} if org else {})}
            a = uli.raw("post", "apps", body)["appId"]
            uli.call(a, "set", {"id": "n", "text": f"{name} on 1.0.0"})
            return a

        s = {"package": p, "v1": v1, "v2": v2, "org": org, "notes": {}}
        s["up"], s["stay"] = app("up"), app("stay")
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
