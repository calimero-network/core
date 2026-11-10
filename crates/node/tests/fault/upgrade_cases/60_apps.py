"""Deep cases: app instances: state and full history, access kinds, xcall, search, presence,
transfer and move, usage and storage figures."""

from __future__ import annotations

from upgrade_deep import (VOLATILE, DeepCase, Failed, anyone, check, decision, decisions, eq, events, host,
                          hosts, join, kv_wasm, new_org, people, person, same_decisions, same_read, scrub, snap, subset,
                          team_id, until)
from upgrade_path import case


@case
class History(DeepCase):
    name = "app-state-history"
    area = "Apps"
    seeds = "an app with 40 writes (sets, overwrites, removals) and app metadata"
    verifies = ("every entry through each node, the length, the full event history from index 0 (the event "
                "stream's replay), the app record, its storage figures and ABI; a write on the new release "
                "extends the history after the old events")

    def populate(self, w):
        hal = person(w, "hal")
        app = hal.deploy(kv_wasm(w), "history")
        for i in range(30):
            hal.call(app, "set", {"key": f"k{i:02}", "value": f"v{i}"})
        for i in range(0, 30, 6):
            hal.call(app, "remove", {"key": f"k{i:02}"})
        for i in range(5):
            hal.call(app, "set", {"key": f"k{i + 1:02}", "value": f"again{i}"})
        hal.raw("post", f"apps/{app}/metadata", {"metadata": {"color": "teal"}})
        s = {"app": app}
        self._record(w, s)
        check(len(s["events"]) == 40, f"40 events from index 0, got {len(s['events'])}")
        return s

    def _record(self, w, s):
        hal, app = person(w, "hal"), s["app"]
        s["entries"] = hal.result(app, "entries")
        s["len"] = hal.result(app, "len")
        s["events"] = events(hal, app, 0, 10_000, timeout=5)
        s["record"] = snap(hal, f"apps/{app}")
        s["storage"] = snap(hal, f"apps/{app}/storage")
        s["abi"] = hal.get(f"apps/{app}/abi")

    def check(self, w, s):
        hal, app = person(w, "hal"), s["app"]
        for n in w.members():
            eq(hal.result(app, "entries", node=n), s["entries"], f"entries through node {n}")
        eq(hal.result(app, "len"), s["len"], "len")
        eq(events(hal, app, 0, len(s["events"]), timeout=15), s["events"], "the event history from index 0")
        same_read(hal, f"apps/{app}", s["record"], "the app")
        same_read(hal, f"apps/{app}/storage", s["storage"], "its storage figures")
        eq(hal.get(f"apps/{app}/abi"), s["abi"], "its ABI")

    def exercise(self, w, s):
        hal = person(w, "hal")
        old = s["events"]
        hal.call(s["app"], "set", {"key": "new", "value": "release"})
        more = events(hal, s["app"], 0, len(old) + 1, timeout=15)
        eq(more[:len(old)], old, "the history after a write on the new release")
        check(len(more) == len(old) + 1, f"the new write's event: {len(more)} events, want {len(old) + 1}")
        self._record(w, s)


@case
class AccessKinds(DeepCase):
    name = "app-access-kinds"
    area = "Apps"
    seeds = "an organization's app of each access kind: owner, organization, listed accounts, public, team"
    verifies = ("who reads and writes each (the owner, a member, a listed account, an outsider, no "
                "credential), allowed and refused as before; access changed on the new release")

    def populate(self, w):
        ace, mo, lis, oz = people(w, "ace", "mo", "lis", "oz")
        del oz  # an outsider
        org = new_org(ace, "Access Org")
        join(ace, org, mo, lis)
        team = team_id(ace.raw("post", f"orgs/{org}/teams", {"name": "few", "join": "invite"}))
        apps = {}
        for kind, access in (("owner", {"kind": "owner"}), ("org", {"kind": "org"}),
                             ("accounts", {"kind": "accounts", "accounts": [lis.id]}),
                             ("public", {"kind": "public"}), ("team", {"kind": "team", "team": team})):
            app = ace.deploy(kv_wasm(w), f"access-{kind}", org=org)
            ace.raw("post", f"apps/{app}/access", {"access": access})
            ace.call(app, "set", {"key": "k", "value": kind})
            apps[kind] = app
        s = {"org": org, "apps": apps}
        s["decisions"] = self._decide(w, s)
        for want in ("mo reads org", "lis reads accounts", "oz reads public"):
            check(s["decisions"][want] == "ok", f"{want}: {s['decisions'][want]}")
        for want in ("mo reads owner", "oz reads org", "mo reads accounts", "mo reads team", "anonymous reads org"):
            check(s["decisions"][want] != "ok", f"{want} was allowed")
        return s

    def _decide(self, w, s):
        out = {}
        for kind, app in s["apps"].items():
            for who in ("ace", "mo", "lis", "oz"):
                p = person(w, who)
                out[f"{who} reads {kind}"] = decision(lambda p=p, app=app: p.view(app, "get", {"key": "k"}))
                out[f"{who} writes {kind}"] = decision(
                    lambda p=p, app=app: p.call(app, "set", {"key": "w", "value": p.name}))
            out[f"anonymous reads {kind}"] = decision(
                lambda app=app: anyone(w, "POST", f"apps/{app}/views", {"method": "get", "args": {"key": "k"}}))
        return out

    def check(self, w, s):
        same_decisions(self._decide(w, s), s["decisions"], "access kinds")

    def exercise(self, w, s):
        ace, mo, lis = people(w, "ace", "mo", "lis")
        app = s["apps"]["accounts"]
        ace.raw("post", f"apps/{app}/access", {"access": {"kind": "accounts", "accounts": [lis.id, mo.id]}})
        until("mo reads once listed", lambda: mo.view(app, "get", {"key": "k"}))
        s["decisions"] = self._decide(w, s)


def _xcalls(w, s, node=None):
    """`GET /apps/{a}/xcalls`, from a node that holds the app (only one answers it)."""
    return scrub(person(w, "xia").get(f"apps/{s['a']}/xcalls", node=node or host(w, s["a"])))


def _nonempty(v):
    if not v:
        raise Failed(f"empty: {v!r}")
    return v


@case
class Xcall(DeepCase):
    name = "xcall"
    area = "Apps"
    seeds = ("two apps of an organization; writes relayed from one to the other (xcall), one refused by the "
             "callee, one dead-lettered")
    verifies = ("the relayed writes, what `onXcallFailed` recorded, the xcall list and its dead letters on every "
                "replica; a relay lands in the mixed cluster (across releases while the two apps sit on different "
                "ones) and on the new release")

    def populate(self, w):
        xia = person(w, "xia")
        org = new_org(xia, "Xcall Org")
        s = {"org": org, "a": xia.deploy(kv_wasm(w), "xc-a", org=org), "b": xia.deploy(kv_wasm(w), "xc-b", org=org),
             "relayed": {}}
        self._relay(w, s, "seed")
        # A method that allows no app: refused, a final answer. A missing target: dead-lettered,
        # and the source's onXcallFailed hears of it.
        xia.call(s["a"], "relayToSet", {"app": s["b"], "key": "nope", "value": "x"})
        xia.call(s["a"], "relay", {"app": "no-such-app", "key": "lost", "value": "x"})
        s["failures"] = until("the dead letter is reported", lambda: _nonempty(xia.result(s["a"], "xcallFailures")))
        check(xia.result(s["b"], "get", {"key": "nope"}) is None, "a method that allows no app ran")
        s["dead"] = _xcalls(w, s)["deadLetters"]
        check(len(s["dead"]) == 1 and s["dead"][0]["app"] == "no-such-app", f"the dead letter: {s['dead']}")
        return s

    def _relay(self, w, s, tag):
        xia = person(w, "xia")
        key = f"relay-{tag}"
        xia.call(s["a"], "relay", {"app": s["b"], "key": key, "value": tag})
        until(f"the {tag} relay lands", lambda: eq(xia.result(s["b"], "get", {"key": key}), tag, key))
        s["relayed"][key] = tag

    def check(self, w, s):
        xia = person(w, "xia")
        for k, v in s["relayed"].items():
            eq(xia.result(s["b"], "get", {"key": k}), v, f"relayed {k}")
        eq(xia.result(s["a"], "xcallFailures"), s["failures"], "xcall failures")
        # Every replica answers it from its own state: each keeps the dead letters the old
        # release recorded (a replica that caught up by replaying the log included).
        for n in hosts(w, s["a"]):
            dead = _xcalls(w, s, n)["deadLetters"]
            lost = [d for d in s["dead"] if d not in dead]
            check(not lost, f"replica {n} lost the dead letters {lost} (it lists {dead})")

    def during_mix(self, w, s):
        self.check(w, s)
        self._relay(w, s, "mixed")

    def exercise(self, w, s):
        self._relay(w, s, "new")


@case
class Search(DeepCase):
    name = "full-text-search"
    area = "Apps"
    seeds = "notes in an app's full-text index (title and body), one dropped"
    verifies = ("each search answers the same notes through every node (each rebuilds its index); a note "
                "added on the new release is found")

    QUERIES = ("raft", "upgrade", "zebra", "stripes")

    def populate(self, w):
        sol = person(w, "sol")
        app = sol.deploy(kv_wasm(w), "search")
        for i, (t, b) in {"n1": ("Raft basics", "leaders and followers"), "n2": ("Upgrade plan", "move every app"),
                          "n3": ("Zebra notes", "stripes and raft"), "n4": ("Gone", "upgrade later")}.items():
            sol.call(app, "note", {"id": i, "title": t, "body": b})
        sol.call(app, "dropNote", {"id": "n4"})
        found = {q: sorted(sol.result(app, "findNotes", {"text": q})) for q in self.QUERIES}
        eq(found["raft"], ["n1", "n3"], "search raft as seeded")
        return {"app": app, "found": found}

    def check(self, w, s):
        sol = person(w, "sol")
        for n in w.members():
            for q, want in s["found"].items():
                eq(sorted(sol.result(s["app"], "findNotes", {"text": q}, node=n)), want, f"search {q!r} on node {n}")

    def exercise(self, w, s):
        person(w, "sol").call(s["app"], "note", {"id": "n5", "title": "Raft after", "body": "the roll"})
        s["found"]["raft"] = sorted([*s["found"]["raft"], "n5"])
        self.check(w, s)


@case
class Presence(DeepCase):
    name = "presence"
    area = "Apps"
    seeds = "presence published in an app (ephemeral: it expires by design, so nothing is carried)"
    verifies = "presence publishes and reads back through every node, in the mixed cluster and after"

    def populate(self, w):
        pip = person(w, "pip")
        return {"app": pip.deploy(kv_wasm(w), "presence"), "seq": 0}

    def check(self, w, s):
        pip = person(w, "pip")
        for n in w.members():
            s["seq"] += 1
            pip.http("POST", f"apps/{s['app']}/presence", {"seq": s["seq"], "state": "6869"}, node=n)
            until(f"presence through node {n}", lambda n=n: check(
                any(e.get("account") == pip.id
                    for e in pip.http("GET", f"apps/{s['app']}/presence", node=n)["present"]),
                f"pip is not present on node {n}"))


@case
class TransferMove(DeepCase):
    name = "app-transfer-move"
    area = "Apps"
    seeds = "an app handed to another account (transfer); an app moved to another organization with its members"
    verifies = ("the owner and organization of each, their data; the former owner refused; the moved app's "
                "members reach it in its new organization; handed back on the new release")

    def populate(self, w):
        tess, ted, mae = people(w, "tess", "ted", "mae")
        given = tess.deploy(kv_wasm(w), "given")
        tess.call(given, "set", {"key": "k", "value": "given"})
        tess.raw("post", f"apps/{given}/transfer", {"to": ted.id})
        src, dst = new_org(tess, "Source Org"), new_org(tess, "Target Org")
        join(tess, src, mae)
        moved = tess.deploy(kv_wasm(w), "moved", org=src)
        mae.call(moved, "set", {"key": "k", "value": "moved"})
        tess.raw("post", f"apps/{moved}/move", {"toOrg": dst, "bringMembers": True})
        s = {"given": given, "moved": moved, "src": src, "dst": dst, "owner": "ted"}
        s["records"] = {"given": snap(ted, f"apps/{given}"), "moved": snap(tess, f"apps/{moved}")}
        eq(s["records"]["moved"].get("org"), dst, "the moved app's organization")
        s["decisions"] = self._decide(w, s)
        check(s["decisions"]["tess reads given"] != "ok", f"the former owner reads: {s['decisions']}")
        return s

    def _decide(self, w, s):
        tess, ted, mae = people(w, "tess", "ted", "mae")
        return decisions({
            "ted reads given": lambda: ted.view(s["given"], "get", {"key": "k"}),
            "tess reads given": lambda: tess.view(s["given"], "get", {"key": "k"}),
            "mae reads moved": lambda: mae.view(s["moved"], "get", {"key": "k"}),
            "mae in target": lambda: mae.get(f"orgs/{s['dst']}"),
        })

    def check(self, w, s):
        tess = person(w, "tess")
        same_read(person(w, s["owner"]), f"apps/{s['given']}", s["records"]["given"], "the transferred app")
        same_read(tess, f"apps/{s['moved']}", s["records"]["moved"], "the moved app")
        same_decisions(self._decide(w, s), s["decisions"], "after transfer and move")
        eq(person(w, s["owner"]).result(s["given"], "get", {"key": "k"}), "given", "the transferred app's data")
        eq(tess.result(s["moved"], "get", {"key": "k"}), "moved", "the moved app's data")

    def exercise(self, w, s):
        ted, tess = people(w, "ted", "tess")
        ted.raw("post", f"apps/{s['given']}/transfer", {"to": tess.id})
        s["owner"] = "tess"
        s["records"]["given"] = snap(tess, f"apps/{s['given']}")
        s["decisions"] = self._decide(w, s)
        eq((s["decisions"]["tess reads given"], s["decisions"]["ted reads given"] != "ok"), ("ok", True),
           "handed back on the new release")


# The quotas are the operator's (`ops-state` changes them); the figures of today grow.
USAGE_VOLATILE = VOLATILE | {"developerFuelToday", "developerWritesToday", "today", "quotas"}


@case
class Usage(DeepCase):
    name = "usage-and-storage"
    area = "Metering"
    seeds = "an organization's app with writes; the account's and the organization's usage"
    verifies = ("the app's per-day usage history (earlier days unchanged, today's figures only grow), the "
                "account's usage counts and the organization's usage")

    def populate(self, w):
        una = person(w, "una")
        org = new_org(una, "Usage Org")
        app = una.deploy(kv_wasm(w), "metered", org=org)
        for i in range(10):
            una.call(app, "set", {"key": f"u{i}", "value": "x" * 100})
        return {"org": org, "app": app, "usage": una.get(f"apps/{app}/usage"),
                "me": snap(una, "auth/usage", USAGE_VOLATILE),
                "org_usage": snap(una, f"orgs/{org}/usage", USAGE_VOLATILE | {"apps"})}

    def check(self, w, s):
        una = person(w, "una")
        u = una.get(f"apps/{s['app']}/usage")
        old = s["usage"]
        today = old.get("today", {}).get("date")
        eq([d for d in u.get("days", []) if d.get("date") != today],
           [d for d in old.get("days", []) if d.get("date") != today], "the app's daily usage")
        now = u.get("today", {})
        if now.get("date") == today:
            for f in ("runs", "writes", "bytesWritten"):
                check(int(now[f]) >= int(old["today"][f]), f"today's {f} shrank: {now[f]} < {old['today'][f]}")
        same_read(una, "auth/usage", s["me"], "the account's usage", USAGE_VOLATILE)
        same_read(una, f"orgs/{s['org']}/usage", s["org_usage"], "the organization's usage", USAGE_VOLATILE | {"apps"})
        subset(una.result(s["app"], "len"), 10, "the metered app")
