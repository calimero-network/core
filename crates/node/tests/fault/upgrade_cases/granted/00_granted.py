"""Granted-run cases (`upgrade_path.py --governance granted`): a cluster deployed with owners
holding release approvals and granting deployers, whose deployments the operator switches to
granted before anything else is seeded.

The first case makes that cluster and sets the run's two hooks: every account signed up after
it gets a deployer grant from the cluster's owner (except those `ungranted` names), and the
rollout's release approval is each organization's own."""

from __future__ import annotations

import urllib.parse

from upgrade_deep import (DeepCase, Failed, check, decisions, eq, fixture, install, join, kv_wasm, new_org, ops,
                          people, person, refused, same_decisions, signed_ops, subset)
from upgrade_path import FROM_RELEASE, TO_RELEASE, case

OWNER = "cluster-owner"
EARLY = "gil-early"  # granted by the operator, before the owner is named
OLDER = "2.3.999"  # a mero-tee release older than every node's
PASSED = "2001-01-01T00:00:00Z"
AHEAD = "2999-01-01T00:00:00Z"
FAR = "9.0.0"  # a release announced with a deadline far ahead


def ungranted(w, *names):
    """People the sign-up hook leaves without a deployer grant."""
    w.shared.setdefault("ungranted", set()).update(names)
    return people(w, *names)


def _grant_on_sign_up(w):
    def grant(account):
        if account.name not in w.shared.get("ungranted", ()):
            person(w, OWNER).raw("post", "deployers", {"deployer": {"account": account.id}})
    return grant


def _approve_everywhere(w):
    """Every organization that approves the old nodes' release approves `release` too,
    through a harness account holding `APPROVE_RELEASES` there (its owner or an admin),
    as a dedicated cluster's owners would before their apps may move onto the new nodes.
    One that never approved the old release (a stalled one) is left as it is."""
    def approve(release):
        approvers = {}
        for p in [person(w, name) for name in list(w._accounts)]:
            try:
                me = p.http("GET", "auth/me")
            except Exception:  # noqa: BLE001 -- an account whose key a case revoked
                continue
            for o in me.get("orgs", []):
                if o["role"] in ("owner", "admin"):
                    approvers.setdefault(o["orgId"], p)
        after, waiting = None, []
        while True:
            page = ops(w, "GET", "ops/orgs" + (f"?after={after}" if after else ""))
            for o in page["orgs"]:
                held = o["approvedReleases"]
                if held is not None and FROM_RELEASE in held and release not in held:
                    waiting.append(o["org"])
            after = page.get("next")
            if not after:
                break
        missing = [o for o in waiting if o not in approvers]
        check(not missing, f"organizations no harness account approves for: {missing}")
        for org in waiting:
            approvers[org].raw("post", f"orgs/{org}/releases", {"version": release})
        w.note(f"{len(waiting)} organizations approved {release}")
    return approve


def _deployers(w, node=None):
    got = ops(w, "GET", "deployers", node=node)
    got["grants"] = {g["key"]: g.get("until") for g in got["grants"]}
    return got


@case
class GrantedGovernance(DeepCase):
    name = "granted-governance"
    governance = ("granted",)
    area = "Governance"
    since = "1.0.0-rc.34"
    seeds = ("a cluster deployed with `--release-approvals owner --deployers owner`: the operator grants an account "
             "while it still may (`POST /ops/deployers`), names the cluster's owner (`POST /ops/cluster-owner`) and "
             "switches deployments to granted (`POST /ops/deployments/granted`); from then on the owner grants "
             "every new account (`POST /deployers`), and each organization approves the new release in the rollout")
    verifies = ("`GET /deployers` on every node: `deployments: granted` though every node runs with the default "
                "`--deployments open`, `deployers: owner`, the owner and the operator's grant; a second switch, the "
                "operator's grant and its release approval are refused as on the old release")

    def seed(self, w):
        super().seed(w)
        if self.seed_error:
            raise Failed(self.seed_error)  # every later case needs this cluster

    def populate(self, w):
        owner, early = people(w, OWNER, EARLY)
        signed_ops(w, "POST", "ops/deployers", {"deployer": {"account": early.id}})
        signed_ops(w, "POST", "ops/cluster-owner", {"owner": {"account": owner.id}})
        signed_ops(w, "POST", "ops/deployments/granted", {})
        w.on_sign_up = _grant_on_sign_up(w)
        w.approve_release = _approve_everywhere(w)
        s = {"decisions": self._decide(w)}
        check("ok" not in s["decisions"].values(), f"refused on the granted cluster: {s['decisions']}")
        return s

    def _decide(self, w):
        return decisions({
            "switch again": lambda: signed_ops(w, "POST", "ops/deployments/granted", {}),
            "operator grants": lambda: signed_ops(w, "POST", "ops/deployers",
                                                  {"deployer": {"account": person(w, EARLY).id}}),
            "operator approves a release": lambda: signed_ops(w, "POST", "ops/releases", {"version": FAR}),
        })

    def check(self, w, s):
        owner, early = people(w, OWNER, EARLY)
        for n in w.members():
            got = _deployers(w, n)
            subset(got, {"deployments": "granted", "deployers": "owner", "owner": {"account": owner.id}},
                   f"the deployer rules on node {n}")
            check(f"account:{early.id}" in got["grants"], f"the operator's grant on node {n}: {got['grants']}")
        same_decisions(self._decide(w), s["decisions"], "the operator on the granted cluster")


@case
class GrantedInstalls(DeepCase):
    name = "granted-installs"
    governance = ("granted",)
    area = "Deployments"
    since = "1.0.0-rc.34"
    seeds = ("a granted account's install (`POST /packages/install`) and app of it; an ungranted member of its "
             "organization makes an instance of that release (no install, no grant)")
    verifies = ("both apps read back; an ungranted account's install, bundle upload and `POST /apps` of a registry "
                "release refused `403 DEPLOY_NOT_GRANTED` as on the old release; on the new release the granted "
                "account installs another package")

    def populate(self, w):
        dev = person(w, "gus-dev")
        mate, nils = ungranted(w, "mia-mate", "nils-nogrant")
        org = new_org(dev, "Granted Org")
        join(dev, org, mate)
        package, version = install(dev, w, "kv-b-1.0.0.mpk")
        apps = {}
        for p in (dev, mate):
            apps[p.name] = p.raw("post", "apps", {"name": p.name, "package": package, "version": version,
                                                  "org": org})["appId"]
            p.call(apps[p.name], "set", {"key": "k", "value": p.name})
        s = {"apps": apps, "decisions": self._decide(w), "installed": [package]}
        eq(set(s["decisions"].values()), {"DEPLOY_NOT_GRANTED"}, f"the ungranted account: {s['decisions']}")
        return s

    def _decide(self, w):
        nils = person(w, "nils-nogrant")
        package, version = w.registry.publish(fixture(w, "kv-c-1.0.0.mpk"))
        return decisions({
            "install": lambda: nils.raw("post", "packages/install", {"package": package, "version": version}),
            "bundle": lambda: nils.deploy(kv_wasm(w), "nils-raw"),
            "create from the registry": lambda: nils.raw("post", "apps", {"name": "n", "package": package,
                                                                          "version": version}),
        })

    def check(self, w, s):
        for name, app in s["apps"].items():
            eq(person(w, name).result(app, "get", {"key": "k"}), name, f"{name}'s app")
        same_decisions(self._decide(w), s["decisions"], "an ungranted account's installs")
        listed = {p["package"] for p in person(w, "gus-dev").get("packages")["packages"]}
        check(set(s["installed"]) <= listed, f"the granted account's packages: {sorted(listed)}")

    def exercise(self, w, s):
        package, _ = install(person(w, "gus-dev"), w, "kv-c-1.0.0.mpk")
        s["installed"].append(package)
        self.check(w, s)


@case
class OwnerDeployers(DeepCase):
    name = "owner-deployers"
    governance = ("granted",)
    area = "Deployments"
    since = "1.0.0-rc.34"
    seeds = ("the cluster owner's grants (`POST /deployers`, its passkey): an organization's until a date, whose "
             "owner installs on it; an account's, revoked again (`DELETE /deployers/{key}`) after it installed")
    verifies = ("`GET /deployers` as the owner on every node; an account reads no grants; the revoked account's "
                "release still installed and its next install refused `DEPLOY_NOT_GRANTED`; on the new release the "
                "owner grants another account, which installs, and revokes the organization's grant")

    def populate(self, w):
        owner = person(w, OWNER)
        olga, rex = ungranted(w, "olga-org", "rex-revoked")
        org = new_org(olga, "Owner Granted Org")
        owner.raw("post", "deployers", {"deployer": {"org": org}, "until": AHEAD})
        owner.raw("post", "deployers", {"deployer": {"account": rex.id}})
        install(olga, w, "kv-a-1.0.0.mpk")
        package, _ = install(rex, w, "kv-b-1.0.0.mpk")
        owner.raw("delete", "deployers/" + urllib.parse.quote(f"account:{rex.id}", safe=""))
        return {"org": org, "rex_package": package, "grants": {f"org:{org}": AHEAD}, "refused": ["rex-revoked"]}

    def check(self, w, s):
        owner = person(w, OWNER)
        for n in w.members():
            got = {g["key"]: g.get("until") for g in owner.get("deployers", node=n)["grants"]}
            subset({k: got.get(k) for k in s["grants"]}, s["grants"], f"the owner's grants on node {n}")
            check(not any(k.endswith(person(w, r).id) for k in got for r in s["refused"]),
                  f"a revoked grant is back on node {n}: {got}")
        refused("FORBIDDEN", lambda: person(w, "rex-revoked").get("deployers"), "an account reads the deployers")
        listed = {p["package"] for p in person(w, "rex-revoked").get("packages")["packages"]}
        check(s["rex_package"] in listed, f"the revoked account's release is gone: {sorted(listed)}")
        for name in s["refused"]:
            refused("DEPLOY_NOT_GRANTED", lambda p=person(w, name): install(p, w, "kv-c-1.0.0.mpk"),
                    f"{name} installs")

    def exercise(self, w, s):
        owner = person(w, OWNER)
        (tia,) = ungranted(w, "tia-late")
        owner.raw("post", "deployers", {"deployer": {"account": tia.id}})
        install(tia, w, "kv-c-1.0.0.mpk")
        owner.raw("delete", "deployers/" + urllib.parse.quote(f"org:{s['org']}", safe=""))
        s["grants"] = {f"account:{tia.id}": None}
        s["refused"].append("olga-org")
        self.check(w, s)


@case
class OwnerReleaseApprovals(DeepCase):
    name = "owner-release-approvals"
    governance = ("granted",)
    area = "Governance"
    since = "1.0.0-rc.34"
    seeds = ("an organization's own release approvals (`POST /orgs/{org}/releases`, passkey): an older release "
             "added to the one it was made through, and an app of it holding data")
    verifies = ("`GET /ops/orgs` keeps every approval seeded, plus the new release its owner approved in the "
                "rollout; the app moved onto the new nodes on that approval and reads back; a member without "
                "`APPROVE_RELEASES` is refused as on the old release")

    def populate(self, w):
        rho, max_ = people(w, "rho-owner", "max-member")
        org = new_org(rho, "Approving Org")
        join(rho, org, max_)
        rho.raw("post", f"orgs/{org}/releases", {"version": OLDER})
        app = rho.deploy(kv_wasm(w), "approved", org=org)
        rho.call(app, "set", {"key": "k", "value": "held"})
        s = {"org": org, "app": app, "approved": sorted(_approved(w, org)), "decisions": self._decide(w, org)}
        eq(set(s["approved"]), {FROM_RELEASE, OLDER}, "the organization's approvals")
        check(s["decisions"]["member approves"] != "ok", f"a member approves: {s['decisions']}")
        return s

    def _decide(self, w, org):
        return decisions({"member approves": lambda: person(w, "max-member").raw(
            "post", f"orgs/{org}/releases", {"version": FAR})})

    def check(self, w, s):
        have = _approved(w, s["org"])
        check(set(s["approved"]) <= have, f"approvals lost: {sorted(have)}, seeded {s['approved']}")
        if w.phase != "seed" and w.phase != "mixed":
            check(TO_RELEASE in have, f"the rollout's approval of {TO_RELEASE}: {sorted(have)}")
        eq(person(w, "rho-owner").result(s["app"], "get", {"key": "k"}), "held", "the approving organization's app")
        same_decisions(self._decide(w, s["org"]), s["decisions"], "a member's approval")


def _approved(w, org):
    after = None
    while True:
        page = ops(w, "GET", "ops/orgs" + (f"?after={after}" if after else ""))
        for o in page["orgs"]:
            if o["org"] == org:
                return set(o["approvedReleases"] or [])
        after = page.get("next")
        if not after:
            raise Failed(f"organization {org} is not listed")


@case
class ReleaseDeadlines(DeepCase):
    name = "release-deadlines"
    governance = ("granted",)
    area = "Governance"
    since = "1.0.0-rc.34"
    seeds = (f"the operator's release deadlines (`POST /ops/releases/{{version}}/deadline`): a passed one on the "
             f"old nodes' release (so no node retires) and one announced far ahead; an organization that approves "
             f"only an older release ({OLDER}) is stalled, one on the old release is not")
    verifies = ("`GET /releases` lists both deadlines; the stalled organization's new app and install are refused "
                "`409 RELEASE_DEADLINE` as on the old release, while the other's are not; on the new release the "
                "stalled organization approves the new release and creates an app, and the far deadline is lifted")

    def populate(self, w):
        stan, ora = people(w, "stan-stalled", "ora-ontime")
        org = new_org(stan, "Stalled Org")
        stan.raw("post", f"orgs/{org}/releases", {"version": OLDER})
        stan.raw("post", f"orgs/{org}/releases", {"version": FROM_RELEASE, "approved": False})
        other = new_org(ora, "On Time Org")
        signed_ops(w, "POST", f"ops/releases/{FROM_RELEASE}/deadline", {"after": PASSED})
        signed_ops(w, "POST", f"ops/releases/{FAR}/deadline", {"after": AHEAD})
        s = {"org": org, "deadlines": {FROM_RELEASE: PASSED, FAR: AHEAD}, "stalled": True,
             "apps": {"ora-ontime": ora.deploy(kv_wasm(w), "on-time", org=other)}}
        s["decisions"] = self._decide(w, s)
        eq(s["decisions"], {"create": "RELEASE_DEADLINE", "install": "RELEASE_DEADLINE"}, "the stalled organization")
        return s

    def _decide(self, w, s):
        stan = person(w, "stan-stalled")
        return decisions({
            "create": lambda: stan.deploy(kv_wasm(w), "stalled", org=s["org"]),
            "install": lambda: install(stan, w, "kv-c-1.0.0.mpk"),
        })

    def check(self, w, s):
        got = {d["version"]: d["after"] for d in ops(w, "GET", "releases")["deadlines"]}
        eq(got, s["deadlines"], "the release deadlines")
        if s["stalled"]:
            same_decisions(self._decide(w, s), s["decisions"], "the stalled organization")
        for name, app in s["apps"].items():
            eq(person(w, name).get(f"apps/{app}")["appId"], app, f"{name}'s app")

    def exercise(self, w, s):
        stan = person(w, "stan-stalled")
        stan.raw("post", f"orgs/{s['org']}/releases", {"version": TO_RELEASE})
        s["apps"]["stan-stalled"] = stan.deploy(kv_wasm(w), "unstalled", org=s["org"])
        s["stalled"] = False
        signed_ops(w, "POST", f"ops/releases/{FAR}/deadline", {"after": None})
        del s["deadlines"][FAR]
        self.check(w, s)
