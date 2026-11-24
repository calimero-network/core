"""Deep cases: deployments (a deployment's secrets for the apps its offered releases use, its offer
moving and its customers' instances kept to it) and the operator's deployer grants."""

from __future__ import annotations

import gzip
import hashlib
import io
import json
import tarfile
import urllib.parse
from pathlib import Path

import ed25519_sign
from rv2 import b64url
from upgrade_deep import (DeepCase, check, eq, install, kv_wasm, new_org, ops, people, person, refused, scratch,
                          signed_ops, subset, until)
from upgrade_path import case

USED = "com.calimero.upgrade.used-secrets"
NOTES = "com.calimero.upgrade.uses-secrets"
HOST = "api.example.com"


def _leb128(n: int) -> bytes:
    out = bytearray()
    while True:
        b, n = n & 0x7F, n >> 7
        out.append(b | (0x80 if n else 0))
        if not n:
            return bytes(out)


def _component(w, name: str) -> bytes:
    """The kv dev component with a custom section naming it: a hash of its own, as
    scripts/upgrade-dev-bundle.py makes one per package."""
    section = b"calimero-upgrade-deep"
    body = _leb128(len(section)) + section + name.encode()
    return kv_wasm(w).read_bytes() + b"\x00" + _leb128(len(body)) + body


def _pack(p, w, package: str, version: str, wasm: bytes, extra: dict) -> Path:
    """A `.mpk` of `wasm` whose manifest adds `extra` (`secrets`, `uses`), signed with the
    development key by the meroctl the case runs with (this tree's, after the roll)."""
    d = scratch(w, f"{package}-{version}")
    manifest = {
        "version": "1.0", "package": package, "appVersion": version, "minRuntimeVersion": "0.1.0",
        "wasm": {"path": "app.wasm", "hash": hashlib.sha256(wasm).hexdigest(), "size": len(wasm)},
        **extra,
    }
    (d / "manifest.json").write_text(json.dumps(manifest))
    p.cli("app", "sign", str(d / "manifest.json"), "--dev")
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w") as tar:
        for name, data in (("manifest.json", (d / "manifest.json").read_bytes()), ("app.wasm", wasm)):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    out = d / f"{package}-{version}.mpk"
    out.write_bytes(gzip.compress(buf.getvalue()))
    return out


@case
class DeploymentUsedSecrets(DeepCase):
    name = "deployment-used-secrets"
    area = "Deployments"
    since = "1.0.0-rc.32"
    seeds = ("a deployment offering a release that uses (`uses`) another package, whose release declares a "
             "secret, and the deployment's value of that secret; the release's `uses` and the value wait for "
             "the cluster version that brings them, so they are made after the roll")
    verifies = ("the deployment's secrets as its manager reads them: the value's fingerprint, the release that "
                "declares it (`declaredBy`) and the offered app that uses it; an outsider is refused the list, "
                "and a name nothing it reaches declares is refused; a new value set on the new release")

    def populate(self, w):
        dex = person(w, "dex")
        used = _pack(dex, w, USED, "1.0.0", _component(w, "used-secrets"), {
            "outbound": [f"https://{HOST}/"],
            "secrets": {"NEAR_AI_API_KEY": {"hosts": [HOST], "required": True}},
        })
        notes = _pack(dex, w, NOTES, "1.0.0", _component(w, "uses-secrets"), {
            "uses": {"inference": {"package": USED, "range": "^1", "version": "1.0.0"}},
        })
        for mpk in (used, notes):
            package, version = w.registry.publish(mpk)
            dex.raw("post", "packages/install", {"package": package, "version": version})
        d = dex.raw("post", "deployments", {"name": "dex-labs", "owner": {"account": dex.id}})["id"]
        dex.raw("post", f"deployments/{d}/apps", {"app": "notes", "package": NOTES, "version": "1.0.0"})

        # A release installed before every node keeps `uses` records none: installing it
        # again once they do fills them in, and then the secret it reaches is taken.
        def set_secret():
            dex.raw("post", "packages/install", {"package": NOTES, "version": "1.0.0"})
            return dex.raw("put", f"deployments/{d}/secrets/NEAR_AI_API_KEY", {"value": "dex-key-1"})

        set_ = until("the cluster version takes a secret an offered release uses", set_secret, timeout=120)
        check("dex-key-1" not in json.dumps(set_), f"the answer carries the value: {set_}")
        return {"deployment": d, "fingerprint": set_["fingerprint"]}

    def check(self, w, s):
        dex, dot = people(w, "dex", "dot")
        d = s["deployment"]
        listed = dex.get(f"deployments/{d}/secrets")
        row = listed.get("secrets", {}).get("NEAR_AI_API_KEY", {})
        eq(row.get("set"), True, "the deployment holds the secret")
        eq(row.get("fingerprint"), s["fingerprint"], "the secret's fingerprint")
        eq(row.get("apps"), ["notes"], "the offered app that uses it")
        eq(row.get("declaredBy"), [{"package": USED, "version": "1.0.0"}], "the release that declares it")
        check("dex-key" not in json.dumps(listed), f"the list carries a value: {listed}")
        refused(("FORBIDDEN", "NOT_FOUND"), lambda: dot.get(f"deployments/{d}/secrets"),
                "an outsider reads the deployment's secrets")
        refused(("UNDECLARED_SECRET",), lambda: dex.raw("put", f"deployments/{d}/secrets/OTHER", {"value": "x"}),
                "a name nothing the deployment reaches declares")

    def exercise(self, w, s):
        dex = person(w, "dex")
        set_ = dex.raw("put", f"deployments/{s['deployment']}/secrets/NEAR_AI_API_KEY", {"value": "dex-key-2"})
        check(set_["fingerprint"] != s["fingerprint"], f"a new value has a new fingerprint: {set_}")
        s["fingerprint"] = set_["fingerprint"]
        self.check(w, s)


OFFERED = "com.calimero.upgrade.kv-o"
# The releases of the offered package the fixtures pack (kv-o-<v>.mpk): the deployment offers
# 1.0.0, then 1.1.0 (on the old release), then 1.2.0 (on the new one); 1.3.0 is never offered.
OFFERED_VERSIONS = ("1.0.0", "1.1.0", "1.2.0", "1.3.0")


def _update(p, app):
    """`GET /apps/{app}/update` as `p` reads it: the update offered, who approves it."""
    u = p.get(f"apps/{app}/update")
    a = u.get("available")
    return {"available": a and {k: a.get(k) for k in ("package", "version", "source")},
            "mayApprove": u.get("mayApprove"), "approvers": u.get("approvers")}


@case
class DeploymentOfferUpgrades(DeepCase):
    name = "deployment-offer-upgrades"
    area = "Deployments"
    # Offers and `OUTSIDE_DEPLOYMENT` are rc.32's, but `GET /apps/{app}/update` (and its
    # `owner-and-admins` approvers, `Feature::OwnerApprovedUpgrades`) came with rc.33.
    since = "1.0.0-rc.33"
    seeds = ("a deployment offering release 1.0.0 of a package whose 1.1.0, 1.2.0 and 1.3.0 are installed too, and a "
             "customer's two instances from it; the customer's owner refused an upgrade outside the offer "
             "(`409 OUTSIDE_DEPLOYMENT`), then, the deployment offering 1.1.0, accepting it on one instance without "
             "naming a version")
    verifies = ("the deployment's offer; each instance's release and data, on the deployment; `GET /apps/{app}/update` "
                "(`available` with `source: deployment` where the instance runs another release than the offer, none "
                "where it runs the offer; `mayApprove` and `approvers` for the owner); every installed release but "
                "the offer and the one it runs refused `OUTSIDE_DEPLOYMENT` to each instance; on the new release the "
                "other instance takes the offer naming its version, the deployment offers 1.2.0 and the first "
                "instance accepts it naming none")

    def populate(self, w):
        dawn, cato = people(w, "dawn", "cato")
        for v in OFFERED_VERSIONS:
            install(dawn, w, f"kv-o-{v}.mpk")
        d = dawn.raw("post", "deployments", {"name": "dawn-offers", "owner": {"account": dawn.id}})["id"]
        dawn.raw("post", f"deployments/{d}/apps", {"app": "kv", "package": OFFERED, "version": "1.0.0"})
        org = new_org(cato, "Offer Customer Org")
        s = {"deployment": d, "org": org, "offer": "1.0.0", "apps": {}, "running": {}}
        for k in ("early", "late"):
            app = cato.raw("post", "apps", {"name": f"offer-{k}", "deployment": d, "package": OFFERED,
                                            "org": org})["appId"]
            cato.call(app, "set", {"key": "made", "value": k})
            s["apps"][k], s["running"][k] = app, "1.0.0"
        self.check(w, s)
        dawn.raw("post", f"deployments/{d}/apps", {"app": "kv", "package": OFFERED, "version": "1.1.0"})
        s["offer"] = "1.1.0"
        cato.raw("post", f"apps/{s['apps']['early']}/upgrade", {})
        s["running"]["early"] = "1.1.0"
        return s

    def check(self, w, s):
        dawn, cato = people(w, "dawn", "cato")
        d = s["deployment"]
        offers = [(o["app"], o["package"], o["version"]) for o in dawn.get(f"deployments/{d}").get("apps", [])]
        eq(offers, [("kv", OFFERED, s["offer"])], "the deployment's offer")
        for k, app in s["apps"].items():
            rec = cato.get(f"apps/{app}")
            eq(rec.get("deployment"), d, f"the {k} instance on the deployment")
            eq((rec.get("release") or {}).get("version"), s["running"][k], f"the {k} instance's release")
            eq(cato.result(app, "get", {"key": "made"}), k, f"the {k} instance's data")
            want = None
            if s["running"][k] != s["offer"]:
                want = {"package": OFFERED, "version": s["offer"], "source": "deployment"}
            eq(_update(cato, app), {"available": want, "mayApprove": True, "approvers": "owner-and-admins"},
               f"the {k} instance's update as its owner reads it")
            for v in OFFERED_VERSIONS:
                if v in (s["offer"], s["running"][k]):
                    continue
                refused("OUTSIDE_DEPLOYMENT", lambda app=app, v=v: cato.raw("post", f"apps/{app}/upgrade",
                                                                            {"version": v}),
                        f"the {k} instance upgraded to {v}, which the deployment does not offer")

    def exercise(self, w, s):
        dawn, cato = people(w, "dawn", "cato")
        late = s["apps"]["late"]
        cato.raw("post", f"apps/{late}/upgrade", {"version": s["offer"]})
        s["running"]["late"] = s["offer"]
        dawn.raw("post", f"deployments/{s['deployment']}/apps", {"app": "kv", "package": OFFERED, "version": "1.2.0"})
        s["offer"] = "1.2.0"
        early = s["apps"]["early"]
        eq(_update(cato, early)["available"], {"package": OFFERED, "version": "1.2.0", "source": "deployment"},
           "the new offer, before it is accepted")
        cato.raw("post", f"apps/{early}/upgrade", {})
        s["running"]["early"] = "1.2.0"
        self.check(w, s)


@case
class DeploymentNamedToCustomers(DeepCase):
    name = "deployment-named-to-customers"
    area = "Deployments"
    since = "1.0.0-rc.36"
    seeds = "a deployment offering a release, and a customer's instance from it"
    verifies = ("`GET /apps/{app}/update` names the instance's deployment with its id and its owner's kind and name "
                "(names are not unique), as `GET /deployments/{id}` names the owner; on the new release a second "
                "instance from it is named the same way")

    def populate(self, w):
        drew, cass = people(w, "drew", "cass")
        install(drew, w, "kv-o-1.0.0.mpk")
        d = drew.raw("post", "deployments", {"name": "drew-labs", "owner": {"account": drew.id}})["id"]
        drew.raw("post", f"deployments/{d}/apps", {"app": "kv", "package": OFFERED, "version": "1.0.0"})
        org = new_org(cass, "Named Customer Org")
        app = cass.raw("post", "apps", {"name": "named-1", "deployment": d, "package": OFFERED, "org": org})["appId"]
        return {"deployment": d, "org": org, "apps": [app]}

    def check(self, w, s):
        drew, cass = people(w, "drew", "cass")
        d = s["deployment"]
        owner = drew.get(f"deployments/{d}")["owner"]
        eq(owner.get("kind"), "account", "the deployment's owner kind")
        for app in s["apps"]:
            eq(cass.get(f"apps/{app}/update").get("deployment"), {"id": d, "name": "drew-labs", "owner": owner},
               f"instance {app}'s update names its deployment and owner")

    def exercise(self, w, s):
        cass = person(w, "cass")
        s["apps"].append(cass.raw("post", "apps", {"name": "named-2", "deployment": s["deployment"],
                                                   "package": OFFERED, "org": s["org"]})["appId"])
        self.check(w, s)


def _recovery_key(seed: bytes) -> str:
    return b64url(ed25519_sign.public_key(seed))


@case
class DeploymentNeedsRecovery(DeepCase):
    name = "deployment-needs-recovery"
    area = "Deployments"
    since = "1.0.0-rc.36"
    seeds = "an organization with a recovery key, and a deployment it owns"
    verifies = ("the organization keeps its key; removing it is refused (`409 RECOVERY_KEY_REQUIRED`) while it owns "
                "the deployment, before the passkey; on the new release rotating it succeeds, and another "
                "organization without a key is refused a deployment")

    def populate(self, w):
        ruth = person(w, "ruth")
        org = new_org(ruth, "Recoverable Deployer Org")
        ruth.raw("post", f"orgs/{org}/recovery", {"publicKey": _recovery_key(b"\x11" * 32)})
        d = ruth.raw("post", "deployments", {"name": "ruth-labs", "owner": {"org": org}})["id"]
        return {"org": org, "deployment": d, "key": _recovery_key(b"\x11" * 32)}

    def check(self, w, s):
        ruth = person(w, "ruth")
        eq((ruth.get(f"orgs/{s['org']}").get("recovery") or {}).get("publicKey"), s["key"],
           "the deploying organization's recovery key")
        eq(ruth.get(f"deployments/{s['deployment']}").get("owner", {}).get("kind"), "org", "the org's deployment")
        refused("RECOVERY_KEY_REQUIRED", lambda: ruth.raw("post", f"orgs/{s['org']}/recovery", {"publicKey": None},
                                                         approve=False),
                "removing the recovery key of an organization that owns a deployment")

    def exercise(self, w, s):
        ruth = person(w, "ruth")
        s["key"] = _recovery_key(b"\x12" * 32)
        ruth.raw("post", f"orgs/{s['org']}/recovery", {"publicKey": s["key"]})
        bare = new_org(ruth, "Keyless Deployer Org")
        refused("RECOVERY_KEY_REQUIRED", lambda: ruth.raw("post", "deployments", {"name": "keyless",
                                                                                 "owner": {"org": bare}},
                                                         approve=False),
                "a deployment of an organization without a recovery key")
        self.check(w, s)


def _grants(w, node=None):
    """`GET /deployers` as the operator, each grant without its time of granting."""
    got = ops(w, "GET", "deployers", node=node)
    got["grants"] = sorted(({k: g.get(k) for k in ("key", "deployer", "until", "grantedBy")} for g in got["grants"]),
                           key=lambda g: g["key"])
    return got


@case
class DeployerGrants(DeepCase):
    name = "deployer-grants"
    governance = ("open",)  # the granted run names an owner, who alone grants then
    area = "Deployments"
    seeds = ("the operator's deployer grants on the open cluster (`POST /ops/deployers`, signed with the operator "
             "key): an account's without an end, an organization's until a date, and an account's revoked again "
             "(`DELETE /ops/deployers/{key}`)")
    verifies = ("`GET /deployers` as the operator on every node: `deployments: open`, `deployers: operator`, no "
                "owner, and each grant (key, deployer, end, `operator`); an account refused the list and the owner's "
                "grant route, a grant without the operator's signature answered `428`; on the new release the "
                "organization's grant revoked and another account's granted")

    def populate(self, w):
        grey, hank, iris = people(w, "grey", "hank", "iris")
        org = new_org(iris, "Deployer Org")
        signed_ops(w, "POST", "ops/deployers", {"deployer": {"account": grey.id}})
        signed_ops(w, "POST", "ops/deployers", {"deployer": {"org": org}, "until": "2099-01-01T00:00:00Z"})
        signed_ops(w, "POST", "ops/deployers", {"deployer": {"account": hank.id}})
        signed_ops(w, "DELETE", "ops/deployers/" + urllib.parse.quote(f"account:{hank.id}", safe=""))
        s = {"org": org, "read": _grants(w)}
        eq({(g["key"], g["until"]) for g in s["read"]["grants"]},
           {(f"account:{grey.id}", None), (f"org:{org}", "2099-01-01T00:00:00Z")}, "the grants as made")
        return s

    def check(self, w, s):
        grey, hank = people(w, "grey", "hank")
        for n in w.members():
            subset(_grants(w, n), s["read"], f"the deployer grants on node {n}")
        subset(s["read"], {"deployments": "open", "deployers": "operator", "owner": None}, "the deployer rules")
        refused("FORBIDDEN", lambda: grey.get("deployers"), "a granted account reads the deployers")
        refused("FORBIDDEN", lambda: grey.raw("post", "deployers", {"deployer": {"account": hank.id}}),
                "an account grants a deployer where the operator grants them")
        refused("OPERATOR_SIGNATURE_REQUIRED", lambda: ops(w, "POST", "ops/deployers",
                                                           {"deployer": {"account": hank.id}}),
                "a grant without the operator's signature")

    def exercise(self, w, s):
        jett = person(w, "jett")
        signed_ops(w, "DELETE", "ops/deployers/" + urllib.parse.quote(f"org:{s['org']}", safe=""))
        signed_ops(w, "POST", "ops/deployers", {"deployer": {"account": jett.id}, "until": "2098-06-01T00:00:00Z"})
        s["read"] = _grants(w)
        eq(len(s["read"]["grants"]), 2, f"the grants after a revocation and a grant: {s['read']['grants']}")
        self.check(w, s)
