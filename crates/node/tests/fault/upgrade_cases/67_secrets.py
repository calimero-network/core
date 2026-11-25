"""Deep cases: secrets: a release that declares one, an instance's own value of it, and a
deployment's value reaching the member a unit from that deployment created."""

from __future__ import annotations

import json

from upgrade_deep import DeepCase, check, decision, eq, install, join, new_org, people, person, scrub, subset
from upgrade_path import case

SECRET = "com.calimero.upgrade.kv-secret"
UNIT = "com.calimero.upgrade.kv-unit"


def _row(p, app):
    return (p.get(f"apps/{app}/secrets").get("secrets") or {}).get("API_KEY") or {}


@case
class InstanceSecrets(DeepCase):
    name = "instance-secrets"
    area = "Apps"
    since = "1.0.0-rc.31"
    seeds = ("a release that binds a secret to a header of its outbound host (`secrets`), an instance of it whose "
             "owner set its own value, and one with none")
    verifies = ("each instance's secret as its owner reads it: the declared name, its binding in words, `source` (`app` with the value's "
                "fingerprint, `cluster` without one) and never the value; an undeclared name and a member's value "
                "are refused; on the new release the owner sets a new value (a new fingerprint) and clears the other")

    def populate(self, w):
        sia, sven = people(w, "sia", "sven")
        install(sia, w, "kv-secret-1.0.0.mpk")
        org = new_org(sia, "Secrets Org")
        join(sia, org, sven)
        own, plain = (sia.raw("post", "apps", {"name": n, "package": SECRET, "version": "1.0.0", "org": org})["appId"]
                      for n in ("own-secret", "no-secret"))
        eq(_row(sia, own).get("source"), "cluster", "a declared secret nobody set")
        set_ = sia.raw("put", f"apps/{own}/secrets/API_KEY", {"value": "sia-key-1"})
        check("sia-key-1" not in json.dumps(set_), f"the answer carries the value: {set_}")
        s = {"org": org, "apps": {"own": own, "plain": plain}, "set": {"own": set_["fingerprint"], "plain": None}}
        s["refused"] = self._refusals(w, s)
        for k, v in s["refused"].items():
            check(v != "ok", f"{k} was allowed")
        return s

    def _refusals(self, w, s):
        sia, sven = people(w, "sia", "sven")
        own = s["apps"]["own"]
        return {
            "an undeclared name": decision(lambda: sia.raw("put", f"apps/{own}/secrets/OTHER", {"value": "x"})),
            "a member sets it": decision(lambda: sven.raw("put", f"apps/{own}/secrets/API_KEY", {"value": "x"})),
        }

    def check(self, w, s):
        sia = person(w, "sia")
        for k, app in s["apps"].items():
            listed = sia.get(f"apps/{app}/secrets")
            check("sia-key" not in json.dumps(listed), f"the list carries a value: {listed}")
            row = (listed.get("secrets") or {}).get("API_KEY") or {}
            eq(row.get("binding"), "sends API_KEY as the X-Api-Key header to api.example.com, any path", f"{k}'s binding")
            if s["set"][k]:
                eq((row.get("source"), row.get("fingerprint")), ("app", s["set"][k]), f"{k}'s own value")
            else:
                eq(row.get("source"), "cluster", f"{k} without a value of its own")
        eq(self._refusals(w, s), s["refused"], "who may set an instance's secret")

    def exercise(self, w, s):
        sia = person(w, "sia")
        own = s["apps"]["own"]
        set_ = sia.raw("put", f"apps/{own}/secrets/API_KEY", {"value": "sia-key-2"})
        check(set_["fingerprint"] != s["set"]["own"], f"a new value has a new fingerprint: {set_}")
        s["set"]["own"] = set_["fingerprint"]
        sia.raw("put", f"apps/{s['apps']['plain']}/secrets/API_KEY", {"value": "sia-key-3"})
        sia.raw("delete", f"apps/{s['apps']['plain']}/secrets/API_KEY")
        self.check(w, s)


@case
class DeploymentUnitSecrets(DeepCase):
    name = "deployment-unit-secrets"
    area = "Deployments"
    since = "1.0.0-rc.32"
    seeds = ("a deployment offering a release that declares a secret and one that creates (`uses`, "
             "`mode = \"create\"`) an app of the first, the deployment's value of that secret, and a customer's "
             "instance of each from the deployment, the unit with its member")
    verifies = ("the customer's instances and the unit's member all on the deployment (`unit_offer`), the member "
                "and the offered app's instance reading the secret from the deployment (`source: deployment`) and "
                "the customer refused to see the "
                "deployment's values; the deployment's fingerprint and the offered apps that declare or use it; on the new "
                "release a new value, the member still on it, and another customer instance from the deployment gets "
                "its member there too")

    def populate(self, w):
        dana, cyd = people(w, "dana", "cyd")
        for mpk in ("kv-secret-1.0.0.mpk", "kv-unit-1.0.0.mpk"):
            install(dana, w, mpk)
        d = dana.raw("post", "deployments", {"name": "dana-suite", "owner": {"account": dana.id}})["id"]
        dana.raw("post", f"deployments/{d}/apps", {"app": "suite", "package": UNIT, "version": "1.0.0"})
        dana.raw("post", f"deployments/{d}/apps", {"app": "keys", "package": SECRET, "version": "1.0.0"})
        set_ = dana.raw("put", f"deployments/{d}/secrets/API_KEY", {"value": "dana-key-1"})
        check("dana-key-1" not in json.dumps(set_), f"the answer carries the value: {set_}")
        org = new_org(cyd, "Customer Org")
        s = {"deployment": d, "org": org, "fingerprint": set_["fingerprint"], "units": []}
        self._customer(w, s, "suite-1")
        s["keys"] = cyd.raw("post", "apps", {"name": "keys-1", "deployment": d, "package": SECRET, "org": org})["appId"]
        return s

    def _customer(self, w, s, name):
        cyd = person(w, "cyd")
        made = cyd.raw("post", "apps", {"name": name, "deployment": s["deployment"], "package": UNIT, "org": s["org"]})
        member = (made.get("links") or {}).get("member")
        check(member, f"{name} was created without its member: {made}")
        s["units"].append({"app": made["appId"], "member": member})

    def check(self, w, s):
        dana, cyd = people(w, "dana", "cyd")
        d = s["deployment"]
        for u in s["units"]:
            eq(cyd.get(f"apps/{u['app']}").get("links"), {"member": u["member"]}, "the customer's unit")
            for app in (u["app"], u["member"]):
                eq(cyd.get(f"apps/{app}").get("deployment"), d, f"{app} on the deployment")
            eq(_row(cyd, u["member"]).get("source"), "deployment", "the member's secret, from the deployment")
        listed = dana.get(f"deployments/{d}/secrets")
        check("dana-key" not in json.dumps(listed), f"the list carries a value: {listed}")
        row = (listed.get("secrets") or {}).get("API_KEY") or {}
        subset(scrub(row), {"set": True, "fingerprint": s["fingerprint"]}, "the deployment's secret")
        eq(sorted(row.get("apps") or []), ["keys", "suite"], "the offered apps that declare or use it")
        eq(cyd.get(f"apps/{s['keys']}").get("deployment"), d, "the offered app's instance on the deployment")
        eq(_row(cyd, s["keys"]).get("source"), "deployment", "the offered app's secret, from the deployment")
        check(decision(lambda: cyd.get(f"deployments/{d}/secrets")) != "ok",
              "a customer read the deployment's secrets")

    def exercise(self, w, s):
        dana = person(w, "dana")
        set_ = dana.raw("put", f"deployments/{s['deployment']}/secrets/API_KEY", {"value": "dana-key-2"})
        check(set_["fingerprint"] != s["fingerprint"], f"a new value has a new fingerprint: {set_}")
        s["fingerprint"] = set_["fingerprint"]
        self._customer(w, s, "suite-2")
        self.check(w, s)
