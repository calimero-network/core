"""Deep cases: governance read paths (outbound approvals, the cluster's releases) and the
operator's state (quotas, per-organization overrides, pools, sign-up invites, counts, backups)."""

from __future__ import annotations

import json

from upgrade_deep import (DeepCase, anyone, check, decision, eq, install, new_org, ops, person, scrub, snap,
                          subset)
from upgrade_path import case


PREFIX = "https://api.example.com/"


def _versions(v):
    if isinstance(v, dict):
        for k in ("releases", "approved", "approvedReleases"):
            if k in v:
                return _versions(v[k])
        return []
    return [r if isinstance(r, str) else (r.get("version") or r.get("release")) for r in v or []]


@case
class Approvals(DeepCase):
    name = "outbound-approvals"
    area = "Governance"
    seeds = "an organization's outbound approval of the prefix a package's release declares"
    verifies = ("the approval as the organization's owner reads it on every node; every release the cluster "
                "approved stays approved after the roll")

    def populate(self, w):
        oli = person(w, "oli")
        package, _ = install(oli, w, "kv-secret-1.0.0.mpk")  # declares the prefix it is approved for
        org = new_org(oli, "Outbound Org")
        oli.raw("post", f"orgs/{org}/outbound", {"package": package, "prefixes": [PREFIX], "replace": True})
        s = {"org": org}
        s["me"] = scrub(oli.get("auth/me")).get("outboundApprovals")
        s["org_read"] = snap(oli, f"orgs/{org}")
        check(PREFIX in json.dumps(s["org_read"].get("outboundApprovals")), f"the approval is not read back: {s['org_read']}")
        s["releases"] = _versions(anyone(w, "GET", "releases"))
        return s

    def check(self, w, s):
        oli = person(w, "oli")
        eq(oli.get("auth/me").get("outboundApprovals"), s["me"], "the account's outbound approvals")
        for n in w.members():
            subset(scrub(oli.get(f"orgs/{s['org']}", node=n)), s["org_read"], f"the organization's approvals on node {n}")
        now = _versions(anyone(w, "GET", "releases"))
        lost = [v for v in s["releases"] if v not in now]
        check(not lost, f"approved releases dropped: {lost} (now {now})")


@case
class UpgradePolicies(DeepCase):
    name = "upgrade-policies"
    area = "Governance"
    since = "1.0.0-rc.34"
    seeds = "an organization's update policy asking for a second approver, with a two-day hold (`org upgrade-policy`)"
    verifies = ("the policy as the organization's owner reads it on every node; dropped on the new release and "
                "read back as the defaults")

    def populate(self, w):
        pia = person(w, "pia")
        org = new_org(pia, "Policy Org")
        pia.raw("post", f"orgs/{org}/upgrade-policy", {"policy": {"secondApprover": True, "holdHours": 48}})
        s = {"org": org, "policy": pia.get(f"orgs/{org}/upgrade-policy")}
        eq(s["policy"]["policy"], {"autoApply": "never", "holdHours": 48, "secondApprover": True},
           "the update policy as set")
        return s

    def check(self, w, s):
        pia = person(w, "pia")
        for n in w.members():
            eq(pia.get(f"orgs/{s['org']}/upgrade-policy", node=n), s["policy"],
               f"the organization's update policy on node {n}")

    def exercise(self, w, s):
        pia = person(w, "pia")
        pia.raw("post", f"orgs/{s['org']}/upgrade-policy", {"policy": None})
        s["policy"] = pia.get(f"orgs/{s['org']}/upgrade-policy")
        eq(s["policy"]["source"], "default", "the update policy dropped")


@case
class OpsState(DeepCase):
    name = "ops-state"
    area = "Ops"
    seeds = "account quotas changed, a per-organization quota override, a compute pool"
    verifies = ("quotas, overrides, pools, the cluster's sign-up invites and policy as before; a quota changed "
                "and a backup written on each new node")

    def populate(self, w):
        org = new_org(person(w, "quin"), "Quota Org")
        w.fleet.operate("set-quotas", json.dumps({"maxApiKeysPerAccount": 49}))
        w.fleet.operate("set-quotas", json.dumps({"org": org, "maxConcurrentRuns": 2, "maxFileBytes": 123456789}))
        # Defining pools takes the cluster off its one implicit pool: keep `standard` (where a
        # job node with no pool serves, RUNS_PER_JOB_NODE runs each) the pool every run goes
        # to (the smallest that fits), and add one beside it.
        w.fleet.operate("set-pools", json.dumps({"standard": {"runsPerNode": 2, "maxMemoryMib": 1_000_000},
                                                 "batch": {"runsPerNode": 1, "maxMemoryMib": 2_000_000}}))
        s = {"org": org}
        self._record(w, s)
        check(s["quotas"].get("maxApiKeysPerAccount") == 49 and org in json.dumps(s["quotas"].get("orgs")),
              f"the quota changes: {s['quotas']}")
        check("batch" in json.dumps(s["pools"]), f"the pool: {s['pools']}")
        return s

    def _record(self, w, s):
        s["quotas"] = ops(w, "GET", "ops/quotas")
        s["pools"] = ops(w, "GET", "ops/pools")
        s["invites"] = _answer(lambda: ops(w, "GET", "ops/sign-up/invites"))
        s["signup"] = anyone(w, "GET", "sign-up")

    def check(self, w, s):
        subset(ops(w, "GET", "ops/quotas"), s["quotas"], "quotas and overrides")
        subset(scrub(ops(w, "GET", "ops/pools"), {"nodes", "running"}), scrub(s["pools"], {"nodes", "running"}), "pools")
        subset(_answer(lambda: ops(w, "GET", "ops/sign-up/invites")), s["invites"], "cluster sign-up invites")
        subset(anyone(w, "GET", "sign-up"), s["signup"], "the sign-up policy")

    def exercise(self, w, s):
        w.fleet.operate("set-quotas", json.dumps({"maxApiKeysPerAccount": 48}))
        self._record(w, s)
        eq(s["quotas"].get("maxApiKeysPerAccount"), 48, "a quota changed on the new release")
        for n in w.members():
            out = ops(w, "POST", "ops/backup", {}, node=n)
            check(out is not None and "written" in out, f"a backup on node {n}: {out}")


def _answer(f):
    """`f()`'s answer, or its refusal as `{"error": ..}`: compared the same way after the roll."""
    from rv2 import HttpError

    try:
        return f()
    except HttpError as e:
        return {"error": e.body.get("error") if isinstance(e.body, dict) else e.status}


@case
class BackupKeys(DeepCase):
    name = "backup-keys"
    area = "Ops"
    since = "1.0.0-rc.20"
    seeds = "an organization's and an account's backup key (what the recovery disk seals their apps' files to)"
    verifies = ("each key as its holder reads it, or the same refusal where governance gives app upgrades "
                "to the operator; how many apps each covers")

    def populate(self, w):
        rae = person(w, "rae")
        org = new_org(rae, "Backup Org")
        s = {"org": org, "decisions": {
            "org key": decision(lambda: rae.raw("post", f"orgs/{org}/backup-key", {"publicKey": "ab" * 32})),
            "own key": decision(lambda: rae.raw("post", "auth/backup-key", {"publicKey": "cd" * 32})),
        }}
        s["org_key"] = scrub(rae.get(f"orgs/{org}/backup-key"))
        s["own_key"] = scrub(rae.get("auth/backup-key"))
        return s

    def check(self, w, s):
        rae = person(w, "rae")
        subset(scrub(rae.get(f"orgs/{s['org']}/backup-key")), s["org_key"], "the organization's backup key")
        subset(scrub(rae.get("auth/backup-key")), s["own_key"], "the account's backup key")
