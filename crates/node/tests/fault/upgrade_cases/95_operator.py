"""Deep cases: what the operator decides over its ops routes: packages it installs, the price
table, an app's replica set, release deadlines. Each is read on every node (`w.members()`)."""

from __future__ import annotations

import json

from upgrade_deep import (DeepCase, anyone, check, decision, eq, fixture, signed_ops, ops, people, scrub, subset)
from upgrade_path import case

PACKAGE = "com.calimero.upgrade.kv-op"
FAR = "2099-01-01T00:00:00Z"  # after every run: a deadline that stalls nobody


def _installed(w, node):
    """The operator's record of `PACKAGE`: its releases and who installed it."""
    got = ops(w, "GET", "ops/packages", node=node)
    mine = [p for p in got["packages"] if p["package"] == PACKAGE]
    check(len(mine) == 1, f"node {node}: ops/packages lists {PACKAGE} {len(mine)} times: {got}")
    return mine[0]


def _versions(record):
    return sorted(v["version"] for v in record["versions"])


@case
class OperatorPackages(DeepCase):
    name = "operator-packages"
    area = "Packages"
    seeds = ("three releases of a package the operator installed (`ops install --path`), one of them uninstalled "
             "again, and an instance an account made of one")
    verifies = ("the operator's list and the catalog an account who is not the installer reads on every node "
                "(`owner: operator`, the releases left); the release the instance runs cannot be uninstalled; "
                "on the new release the operator installs another release, the account upgrades the instance "
                "to it and the operator uninstalls the release it left")

    def populate(self, w):
        opal = people(w, "opal")[0]
        for v in ("1.0.0", "1.1.0"):
            w.fleet.operate("install", "--path", str(fixture(w, f"kv-op-{v}.mpk")))
        app = opal.raw("post", "apps", {"name": "operator-package", "package": PACKAGE, "version": "1.0.0"})["appId"]
        opal.call(app, "set", {"key": "k", "value": "v1"})
        w.fleet.operate("uninstall", PACKAGE, "--version", "1.1.0")
        s = {"app": app, "release": "1.0.0", "versions": ["1.0.0"], "value": "v1"}
        self._record(w, s)
        return s

    def _record(self, w, s):
        s["operator"] = scrub(_installed(w, w.members()[0]))
        s["catalog"] = scrub(people(w, "orrin")[0].get(f"packages/{PACKAGE}", node=w.members()[0]))
        eq(_versions(s["operator"]), s["versions"], "the operator's releases")
        eq(s["operator"].get("owner"), "operator", "the installer")

    def check(self, w, s):
        opal, orrin = people(w, "opal", "orrin")
        for n in w.members():
            subset(scrub(_installed(w, n)), s["operator"], f"the operator's {PACKAGE} on node {n}")
            subset(scrub(orrin.get(f"packages/{PACKAGE}", node=n)), s["catalog"], f"the catalog on node {n}")
            eq(sorted(v["version"] for v in orrin.get(f"packages/{PACKAGE}", node=n)["versions"]), s["versions"],
               f"the releases an account who did not install it sees on node {n}")
            app = opal.get(f"apps/{s['app']}", node=n)
            check(s["release"] in json.dumps(app.get("release") or app), f"node {n}: the instance runs {app}")
            got = decision(lambda n=n: w.fleet.operate("uninstall", PACKAGE, "--version", s["release"], node=n))
            check(got != "ok", f"node {n}: the operator uninstalled {s['release']} under a running instance")
        eq(opal.result(s["app"], "get", {"key": "k"}), s["value"], "the instance's state")

    def exercise(self, w, s):
        opal = people(w, "opal")[0]
        w.fleet.operate("install", "--path", str(fixture(w, "kv-op-1.2.0.mpk")))
        opal.cli("instance", "update", s["app"], "--version", "1.2.0", "--dev-passkey", str(opal.passkey))
        s["release"] = "1.2.0"
        w.fleet.operate("uninstall", PACKAGE, "--version", "1.0.0")
        s["versions"] = ["1.2.0"]
        opal.call(s["app"], "set", {"key": "k", "value": "v2"})
        s["value"] = "v2"
        self._record(w, s)
        self.check(w, s)


@case
class ReplicaSets(DeepCase):
    name = "replica-sets"
    area = "Apps"
    seeds = ("an app with data on the cluster's three nodes, a move of it to its own replica set (answers "
             "`unchanged`) and a move to two nodes (refused `TOO_FEW`)")
    verifies = ("its replicas in `GET /ops/placement` (three or more, all running nodes, the new ones after the "
                "rollout's moves) and its data read on every node; the same two moves on the new release")

    @staticmethod
    def _replicas(w, app):
        for a in w.fleet.placement()["apps"]:
            if a["app"] == app:
                return sorted(a["replicas"])
        raise AssertionError(f"{app} is not placed")

    def _moves(self, w, app):
        own = ",".join(map(str, self._replicas(w, app)))
        w.fleet.operate("move", app, "--replicas", own)
        got = decision(lambda: w.fleet.operate("move", app, "--replicas", ",".join(own.split(",")[:2])))
        eq(got, "TOO_FEW", "a move to two nodes")

    def populate(self, w):
        mo = people(w, "moss")[0]
        app = mo.deploy(fixture(w, "kv-dev.wasm"), "moved")
        s = {"app": app, "written": {}}
        for i in range(4):
            mo.call(app, "set", {"key": f"k{i}", "value": f"v{i}"})
            s["written"][f"k{i}"] = f"v{i}"
        self._moves(w, app)
        return s

    def check(self, w, s):
        mo = people(w, "moss")[0]
        got = self._replicas(w, s["app"])
        check(len(got) >= 3 and set(got) <= set(w.fleet.members), f"the replicas of {s['app']}: {got}")
        for n in w.members():
            for k, v in s["written"].items():
                eq(mo.result(s["app"], "get", {"key": k}, node=n), v, f"{k} read on node {n}")

    def exercise(self, w, s):
        mo = people(w, "moss")[0]
        self._moves(w, s["app"])
        mo.call(s["app"], "set", {"key": "after", "value": "move"})
        s["written"]["after"] = "move"
        self.check(w, s)


@case
class ReleaseDeadlines(DeepCase):
    name = "release-deadlines"
    area = "Governance"
    seeds = ("a deadline for a release no node runs, far ahead (`POST /ops/releases/{version}/deadline`, signed "
             "with the operator key)")
    verifies = ("the public `GET /releases` lists it on every node; on the new release the operator lifts it "
                "(`DELETE`) and sets another, and the list shows only that one")

    @staticmethod
    def _listed(w, node):
        return {d["version"]: d["after"] for d in anyone(w, "GET", "releases", node=node)["deadlines"]}

    def populate(self, w):
        signed_ops(w, "POST", "ops/releases/9.9.9/deadline", {"after": FAR})
        s = {"deadlines": {"9.9.9": FAR}}
        self.check(w, s)
        return s

    def check(self, w, s):
        for n in w.members():
            eq(self._listed(w, n), s["deadlines"], f"the release deadlines on node {n}")

    def exercise(self, w, s):
        signed_ops(w, "DELETE", "ops/releases/9.9.9/deadline")
        signed_ops(w, "POST", "ops/releases/9.9.8/deadline", {"after": FAR})
        s["deadlines"] = {"9.9.8": FAR}
        self.check(w, s)
