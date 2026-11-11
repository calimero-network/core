"""Deep cases: accounts, passkeys, sessions, activity, the recovery code, API keys and aliases."""

from __future__ import annotations

import json
import time

from upgrade_deep import (DeepCase, anyone, check, decisions, eq, fixture, install, kv_wasm, people, person,
                          refused, same_decisions)
from upgrade_path import case


@case
class Passkeys(DeepCase):
    name = "account-passkeys"
    area = "Accounts"
    seeds = "an account with three passkeys: one labelled, one removed"
    verifies = ("the passkey list and labels; every kept passkey signs in on each node, the removed one is "
                "refused; a label set on the new release")

    def populate(self, w):
        pat = person(w, "pat")
        files = {"laptop": pat.home / "laptop.json", "lost": pat.home / "lost.json"}
        for f in files.values():
            pat.cli("account", "device", "add", "--dev-passkey", str(f), "--approve-with", str(pat.passkey))
        ids = [p["id"] for p in pat.get("auth/me")["passkeys"]]
        check(len(ids) == 3, f"three passkeys, got {ids}")
        pat.raw("post", f"auth/passkeys/{ids[1]}", {"label": "laptop"})
        pat.cli("account", "device", "revoke", ids[2], "--dev-passkey", str(pat.passkey))
        return {"kept": ids[:2], "labels": {ids[1]: "laptop"}, "files": {k: str(v) for k, v in files.items()},
                "tries": 0}

    def check(self, w, s):
        pat = person(w, "pat")
        me = pat.get("auth/me")
        eq([p["id"] for p in me["passkeys"]], s["kept"], "passkeys")
        eq({p["id"]: p["label"] for p in me["passkeys"] if p.get("label")}, s["labels"], "passkey labels")
        s["tries"] += 1
        for n in w.members():
            home = pat.home / f"laptop-{s['tries']}-{n}"
            w.fleet.cli(n, "login", "--dev-passkey", s["files"]["laptop"], home=home)
            eq(w.fleet.cli(n, "whoami", home=home)["accountId"], pat.id, f"the second passkey signs in on node {n}")
        refused(("PASSKEY_UNKNOWN_HERE", "UNAUTHENTICATED"),
                lambda: w.fleet.cli(w.members()[0], "login", "--dev-passkey", s["files"]["lost"],
                                    home=pat.home / f"lost-{s['tries']}"),
                "the removed passkey signs in")

    def exercise(self, w, s):
        pat = person(w, "pat")
        pat.raw("post", f"auth/passkeys/{s['kept'][0]}", {"label": "phone"})
        s["labels"][s["kept"][0]] = "phone"
        self.check(w, s)


def _tok(sess, field):
    return sess.get(field) or sess.get({"session_token": "sessionToken", "refresh_token": "refreshToken",
                                        "expires_at": "expiresAt"}[field])


@case
class Sessions(DeepCase):
    name = "account-sessions"
    area = "Accounts"
    seeds = "sessions opened on old nodes, one revoked; an account signed out everywhere"
    verifies = ("a refresh token an old node issued refreshes on every node; its session token is accepted "
                "while that node is in the cluster and refused once it is removed; the revoked session's and "
                "the signed-out account's refresh tokens are refused; a session revoked on the new release ends")

    def populate(self, w):
        sam, lou = people(w, "sam", "lou")
        n = w.members()
        keep = sam.session(n[0])
        other = sam.home / "other"
        w.fleet.cli(n[1 % len(n)], "login", "--dev-passkey", str(sam.passkey), home=other)
        gone = json.loads((other / "meroctl" / "sessions.json").read_text())[w.fleet.url(n[1 % len(n)])]
        listed = sam.http("GET", "auth/sessions", token=_tok(gone, "session_token"))["sessions"]
        revoked = next(x["session"] for x in listed if x.get("current"))
        sam.raw("delete", f"auth/sessions/{revoked}")
        lou_session = lou.session(n[0])
        lou.cli("logout", "--all", node=n[0])
        return {"keep": keep, "issuer": n[0], "gone": gone, "revoked": revoked, "lou": lou_session}

    def check(self, w, s):
        sam = person(w, "sam")
        for n in w.members():
            fresh = sam.http("POST", "auth/refresh", {"refreshToken": _tok(s["keep"], "refresh_token")}, node=n)
            eq(sam.http("GET", "auth/me", token=_tok(fresh, "session_token"), node=n)["accountId"], sam.id,
               f"a session refreshed on node {n} from an old node's refresh token")
            # The session token itself is good while the node that issued it is in the
            # cluster (and it lives, 15 minutes); once that node is removed, it is refused
            # and the client refreshes.
            old = lambda n=n: sam.http("GET", "auth/me", token=_tok(s["keep"], "session_token"), node=n)  # noqa: E731
            if s["issuer"] not in w.fleet.members:
                refused("UNAUTHENTICATED", old, f"a removed node's session token on node {n}")
            elif int(_tok(s["keep"], "expires_at")) > time.time() + 10:
                eq(old()["accountId"], sam.id, f"an old node's session token on node {n}")
            refused("UNAUTHENTICATED", lambda n=n: sam.http(
                "POST", "auth/refresh", {"refreshToken": _tok(s["gone"], "refresh_token")}, node=n),
                f"the revoked session's refresh token on node {n}")
            refused("UNAUTHENTICATED", lambda n=n: sam.http(
                "POST", "auth/refresh", {"refreshToken": _tok(s["lou"], "refresh_token")}, node=n),
                f"a refresh token from before logout --all on node {n}")
        live = {x["session"] for x in sam.get("auth/sessions")["sessions"]}
        check(s["revoked"] not in live, "the revoked session is listed again")

    def exercise(self, w, s):
        sam = person(w, "sam")
        n = w.members()[-1]
        home = sam.home / "on-new"
        w.fleet.cli(n, "login", "--dev-passkey", str(sam.passkey), home=home)
        mine = json.loads((home / "meroctl" / "sessions.json").read_text())[w.fleet.url(n)]
        sid = next(x["session"] for x in sam.http("GET", "auth/sessions", token=_tok(mine, "session_token"),
                                                  node=n)["sessions"] if x.get("current"))
        sam.raw("delete", f"auth/sessions/{sid}", node=w.members()[0])
        refused("UNAUTHENTICATED", lambda: sam.http("GET", "auth/me", token=_tok(mine, "session_token"), node=n),
                "a session revoked on the new release", retry_for=5)


@case
class Activity(DeepCase):
    name = "account-activity"
    area = "Accounts"
    seeds = "sign-ins and passkey approvals (an API key made, a confirm method called)"
    verifies = "every seeded activity entry is still listed; an approval and a sign-in on the new release add theirs"

    def populate(self, w):
        ann = person(w, "ann")
        ann.cli("account", "api-key", "create", "--label", "ci", "--dev-passkey", str(ann.passkey))
        app = ann.deploy(kv_wasm(w), "activity")
        ann.cli("call", "--context", app, "clear", "--dev-passkey", str(ann.passkey))
        log = ann.get("auth/activity")["activity"]
        kinds = {e["kind"] for e in log}
        check({"approval", "signIn"} <= kinds, f"activity kinds {kinds}: {log}")
        return {"log": log}

    def check(self, w, s):
        log = person(w, "ann").get("auth/activity")["activity"]
        have = {json.dumps(e, sort_keys=True) for e in log}
        lost = [e for e in s["log"] if json.dumps(e, sort_keys=True) not in have]
        check(not lost, f"activity entries lost: {lost[:3]}")

    def exercise(self, w, s):
        ann = person(w, "ann")
        before = ann.get("auth/activity")["activity"]
        # An approval, then a sign-in (a sign-in right after another of the same passkey
        # only moves that entry's time).
        ann.cli("account", "api-key", "create", "--label", "on-new", "--dev-passkey", str(ann.passkey))
        ann.sign_in(w.members()[0])
        after = ann.get("auth/activity")["activity"]
        eq([e["kind"] for e in after[:2]], ["signIn", "approval"], "the newest activity on the new release")
        eq(len(after), len(before) + 2, "activity entries after an approval and a sign-in on the new release")


@case
class RecoveryCode(DeepCase):
    name = "account-recovery-code"
    area = "Accounts"
    seeds = "an account recovery code made on the old release"
    verifies = ("`recoveryCode` stays set; the code begins a recovery on every node and a wrong code is "
                "refused; removed on the new release, it is refused")

    def populate(self, w):
        rita = person(w, "rita")
        made = rita.raw("post", "auth/recovery", {})
        code = made.get("code") or made.get("recoveryCode")
        check(code, f"no code in {made}")
        return {"code": code, "removed": False}

    def check(self, w, s):
        rita = person(w, "rita")
        eq(bool(rita.get("auth/me")["recoveryCode"]), not s["removed"], "recoveryCode")
        wrong = ("A" if s["code"][0] != "A" else "B") + s["code"][1:]
        refused(("UNAUTHENTICATED", "401"), lambda: anyone(w, "POST", "auth/recovery/begin", {"code": wrong}),
                "a wrong recovery code")
        for n in w.members():
            if s["removed"]:
                refused(("UNAUTHENTICATED", "401"),
                        lambda n=n: anyone(w, "POST", "auth/recovery/begin", {"code": s["code"]}, node=n),
                        f"a removed recovery code on node {n}")
            else:
                begun = anyone(w, "POST", "auth/recovery/begin", {"code": s["code"]}, node=n)
                check(begun.get("challengeId"), f"recovery/begin on node {n}: {begun}")

    def exercise(self, w, s):
        person(w, "rita").raw("delete", "auth/recovery")
        s["removed"] = True
        self.check(w, s)


@case
class ApiKeys(DeepCase):
    name = "api-keys"
    area = "Keys"
    seeds = "API keys: read-only for one app, one method of one app, with a TTL, revoked; an alias"
    verifies = ("each key allowed and refused exactly as before (reads, writes, its method, its app, another "
                "app); the revoked key stays refused; the key list and the alias; a key made and one revoked "
                "on the new release")

    def populate(self, w):
        kim = person(w, "kim")
        a = kim.deploy(kv_wasm(w), "keys-a")
        b = kim.deploy(kv_wasm(w), "keys-b")
        for app in (a, b):
            kim.call(app, "set", {"key": "k", "value": "v"})

        def mk(label, **body):
            return kim.raw("post", "auth/apikeys", {"label": label, "readOnly": False, "allowConfirm": False, **body})

        keys = {"ro": mk("ro", apps=[a], readOnly=True), "get": mk("get", apps=[a], methods=["get"]),
                "ttl": mk("ttl", ttlSecs=86400), "revoked": mk("revoked")}
        kim.raw("delete", f"auth/apikeys/{keys['revoked']['keyId']}")
        kim.raw("put", "auth/aliases/main", {"app": a})
        s = {"a": a, "b": b, "keys": {k: v["key"] for k, v in keys.items()},
             "ids": sorted(x["keyId"] for x in kim.get("auth/apikeys")["keys"])}
        s["decisions"] = self._decide(w, s)
        return s

    def _decide(self, w, s):
        kim = person(w, "kim")
        a, b, k = s["a"], s["b"], s["keys"]
        return decisions({
            "ro reads a": lambda: kim.view(a, "get", {"key": "k"}, key=k["ro"]),
            "ro writes a": lambda: kim.call(a, "set", {"key": "k", "value": "ro"}, key=k["ro"]),
            "ro reads b": lambda: kim.view(b, "get", {"key": "k"}, key=k["ro"]),
            "get reads a": lambda: kim.view(a, "get", {"key": "k"}, key=k["get"]),
            "get lists a": lambda: kim.view(a, "entries", key=k["get"]),
            "get writes a": lambda: kim.call(a, "set", {"key": "k", "value": "get"}, key=k["get"]),
            "ttl reads b": lambda: kim.view(b, "get", {"key": "k"}, key=k["ttl"]),
            "revoked reads a": lambda: kim.view(a, "get", {"key": "k"}, key=k["revoked"]),
            "revoked me": lambda: kim.http("GET", "auth/me", key=k["revoked"]),
            "unknown me": lambda: kim.http("GET", "auth/me", key="cal_" + "0" * 40),
            "ro me": lambda: kim.http("GET", "auth/me", key=k["ro"]),
            "ro makes a key": lambda: kim.http("POST", "auth/apikeys", {"label": "x", "readOnly": True,
                                                                         "allowConfirm": False}, key=k["ro"]),
        })

    def check(self, w, s):
        kim = person(w, "kim")
        same_decisions(self._decide(w, s), s["decisions"], "API keys")
        eq(sorted(x["keyId"] for x in kim.get("auth/apikeys")["keys"]), s["ids"], "key ids")
        eq(kim.get("auth/aliases/main").get("appId"), s["a"], "the alias")

    def exercise(self, w, s):
        kim = person(w, "kim")
        made = kim.raw("post", "auth/apikeys", {"label": "new", "apps": [s["b"]], "readOnly": True,
                                                "allowConfirm": False})
        eq(kim.result(s["b"], "get", {"key": "k"}, key=made["key"]), "v", "a key made on the new release")
        refused("FORBIDDEN", lambda: kim.call(s["b"], "set", {"key": "k", "value": "x"}, key=made["key"]),
                "a read-only key made on the new release writes")
        ttl = next(x["keyId"] for x in kim.get("auth/apikeys")["keys"] if x["label"] == "ttl")
        kim.raw("delete", f"auth/apikeys/{ttl}")
        refused("UNAUTHENTICATED", lambda: kim.view(s["b"], "get", {"key": "k"}, key=s["keys"]["ttl"]),
                "a key revoked on the new release", retry_for=5)
        s["decisions"]["ttl reads b"] = "UNAUTHENTICATED"
        s["ids"] = sorted([*(i for i in s["ids"] if i != ttl), made["keyId"]])


@case
class UpgradeKeys(DeepCase):
    name = "upgrade-keys"
    area = "Keys"
    seeds = "an upgrade key (an API key that may only move one app to releases of its package)"
    verifies = ("the key still cannot call or read the app; on the new release it upgrades the app to the "
                "next release, and is refused another app")

    def populate(self, w):
        uma = person(w, "uma")
        p, v1 = install(uma, w, "kv-u-1.0.0.mpk")
        app = uma.raw("post", "apps", {"name": "upgradable", "package": p, "version": v1})["appId"]
        other = uma.raw("post", "apps", {"name": "other", "package": p, "version": v1})["appId"]
        uma.call(app, "set", {"key": "k", "value": "kept"})
        key = uma.cli("account", "api-key", "create", "--label", "ci-upgrades", "--upgrades", app,
                      "--dev-passkey", str(uma.passkey))["key"]
        s = {"package": p, "v1": v1, "app": app, "other": other, "key": key}
        s["decisions"] = self._decide(w, s)
        check(s["decisions"]["reads"] != "ok", f"an upgrade key reads the app: {s['decisions']}")
        return s

    def _decide(self, w, s):
        uma = person(w, "uma")
        return decisions({
            "reads": lambda: uma.view(s["app"], "get", {"key": "k"}, key=s["key"]),
            "writes": lambda: uma.call(s["app"], "set", {"key": "k", "value": "x"}, key=s["key"]),
            "me": lambda: uma.http("GET", "auth/me", key=s["key"]),
        })

    def check(self, w, s):
        same_decisions(self._decide(w, s), s["decisions"], "the upgrade key")
        eq(person(w, "uma").result(s["app"], "get", {"key": "k"}), "kept", "the upgradable app's data")

    def exercise(self, w, s):
        uma = person(w, "uma")
        _, v2 = w.registry.publish(fixture(w, "kv-u-1.1.0.mpk"))
        uma.raw("post", "packages/install", {"package": s["package"], "version": v2})
        refused(("FORBIDDEN", "NOT_FOUND", "REJECTED"),
                lambda: uma.http("POST", f"apps/{s['other']}/upgrade", {"version": v2}, key=s["key"]),
                "the upgrade key on another app")
        uma.http("POST", f"apps/{s['app']}/upgrade", {"version": v2}, key=s["key"])
        app = uma.get(f"apps/{s['app']}")
        check(v2 in json.dumps(app), f"the app after the key's upgrade: {app}")
        eq(uma.result(s["app"], "get", {"key": "k"}), "kept", "data across the key's upgrade")

