"""Deep cases: app instances: state and full history, access kinds, public links and their
visitors, xcall, search, presence, transfer and move, usage and storage figures."""

from __future__ import annotations

import hashlib
import time

import ed25519_sign
from upgrade_deep import (VOLATILE, _http, DeepCase, Failed, anyone, check, decision, decisions, eq, events, host,
                          hosts, join, kv_wasm, new_org, people, person, same_decisions, same_read, scrub, snap, subset,
                          team_id, until)
from upgrade_deep import not_covered
from upgrade_path import case


def settled_days(old, new):
    """The rows of two `GET /apps/{app}/usage` answers that must agree: the dates both
    `days` windows hold, without either answer's `today` (still counting). The window
    is the last 30 days and rolls at midnight UTC, so a verify on a later day than the
    populate sees its oldest row gone and the populate's `today` closed into `days`."""
    skip = {old.get("today", {}).get("date"), new.get("today", {}).get("date")}
    both = ({d.get("date") for d in old.get("days", [])} & {d.get("date") for d in new.get("days", [])}) - skip
    pick = lambda a: [d for d in a.get("days", []) if d.get("date") in both]
    return pick(new), pick(old)


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
    since = "1.0.0-rc.33"
    seeds = "an organization's app with each kind of grant alone: none (the owner), the organization, a named account, anyone, a team"
    verifies = ("who reads and writes each (the owner, a member, the named account, an outsider, no credential), "
                "allowed and refused as before (`anyone` admits the outsider, never a caller without an account); "
                "each instance's grants as the owner reads them; a grant added on the new release")

    def populate(self, w):
        ace, mo, lis, oz = people(w, "ace", "mo", "lis", "oz")
        del oz  # an outsider
        org = new_org(ace, "Access Org")
        join(ace, org, mo, lis)
        team = team_id(ace.raw("post", f"orgs/{org}/teams", {"name": "few", "join": "invite"}))
        apps, grants = {}, {}
        for kind, granted in (("owner", []), ("org", [{"kind": "org"}]),
                              ("account", [{"kind": "account", "account": lis.id}]),
                              ("anyone", [{"kind": "anyone"}]), ("team", [{"kind": "team", "team": team}])):
            app = ace.deploy(kv_wasm(w), f"access-{kind}", org=org)
            ace.raw("post", f"apps/{app}/access", {"grants": granted})
            ace.call(app, "set", {"key": "k", "value": kind})
            apps[kind], grants[kind] = app, granted
        s = {"org": org, "apps": apps, "grants": grants}
        s["decisions"] = self._decide(w, s)
        for want in ("mo reads org", "lis reads account", "oz reads anyone"):
            check(s["decisions"][want] == "ok", f"{want}: {s['decisions'][want]}")
        for want in ("mo reads owner", "oz reads org", "mo reads account", "mo reads team", "anonymous reads org",
                     "anonymous reads anyone"):
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
        ace = person(w, "ace")
        for kind, app in s["apps"].items():
            eq(ace.get(f"apps/{app}").get("grants"), s["grants"][kind], f"the {kind} instance's grants")
        same_decisions(self._decide(w, s), s["decisions"], "access kinds")

    def exercise(self, w, s):
        ace, mo, lis = people(w, "ace", "mo", "lis")
        app = s["apps"]["account"]
        s["grants"]["account"] = [{"kind": "account", "account": lis.id}, {"kind": "account", "account": mo.id}]
        ace.raw("post", f"apps/{app}/access", {"grants": s["grants"]["account"]})
        until("mo reads once named", lambda: mo.view(app, "get", {"key": "k"}))
        s["decisions"] = self._decide(w, s)


@case
class AccessGrants(DeepCase):
    name = "app-access-grants"
    area = "Apps"
    since = "1.0.0-rc.33"
    seeds = ("an organization's instance whose access is a list of grants: a team, a member outside it named with "
             "an app-defined scope, and an outsider named (a guest, whom the release does not allow); grants "
             "wait for the cluster version that brings them, so they are set after the roll")
    verifies = ("who reads and writes it (the owner, the team's member, the named member, the guest, a member "
                "neither in the team nor named, no credential) and the grants with their scope as the owner reads "
                "them; a guest sees nothing of it; on the new release the team's grant goes and the organization's "
                "takes its place")

    def populate(self, w):
        gib, gaz, gia, gow, gem = people(w, "gib", "gaz", "gia", "gow", "gem")
        org = new_org(gib, "Grants Org")
        team = team_id(gib.raw("post", f"orgs/{org}/teams", {"name": "deal", "join": "invite"}))
        join(gib, org, gaz, teams=(team,))
        join(gib, org, gia, gem)
        app = gib.deploy(kv_wasm(w), "grants", org=org)
        gib.call(app, "set", {"key": "k", "value": "grants"})
        grants = [{"kind": "team", "team": team},
                  {"kind": "account", "account": gia.id, "scope": {"channel": "partners"}},
                  {"kind": "account", "account": gow.id}]
        until("the cluster version takes grants", lambda: gib.raw(
            "post", f"apps/{app}/access", {"grants": grants}), timeout=120)
        s = {"org": org, "team": team, "app": app, "grants": grants}
        s["decisions"] = self._decide(w, s)
        for want in ("gaz reads", "gia reads"):
            check(s["decisions"][want] == "ok", f"{want}: {s['decisions'][want]}")
        for want in ("gow reads", "anonymous reads"):
            check(s["decisions"][want] != "ok", f"{want} was allowed")
        return s

    def _decide(self, w, s):
        out, app = {}, s["app"]
        for who in ("gib", "gaz", "gia", "gow", "gem"):
            p = person(w, who)
            out[f"{who} reads"] = decision(lambda p=p: p.view(app, "get", {"key": "k"}))
            out[f"{who} writes"] = decision(lambda p=p: p.call(app, "set", {"key": "w", "value": p.name}))
        out["anonymous reads"] = decision(
            lambda: anyone(w, "POST", f"apps/{app}/views", {"method": "get", "args": {"key": "k"}}))
        return out

    def check(self, w, s):
        eq(person(w, "gib").get(f"apps/{s['app']}").get("grants"), s["grants"], "the grants as the owner reads them")
        same_decisions(self._decide(w, s), s["decisions"], "access grants")

    def exercise(self, w, s):
        gib, gaz = people(w, "gib", "gaz")
        s["grants"] = [{"kind": "org"}] + s["grants"][1:]
        gib.raw("post", f"apps/{s['app']}/access", {"grants": s["grants"]})
        until("the organization's members read once it is granted",
              lambda: person(w, "gem").view(s["app"], "get", {"key": "k"}))
        gaz.view(s["app"], "get", {"key": "k"})
        s["decisions"] = self._decide(w, s)


@case
class TeamDeletedKeepingApps(DeepCase):
    name = "team-deleted-keeping-apps"
    area = "Teams"
    since = "1.0.0-rc.33"
    seeds = ("an organization's two instances only a team admits, one also naming a member outside the team; the "
             "team deleted with `keepApps`, which waits for the cluster version that brings it, so it is done "
             "after the roll")
    verifies = ("both instances kept with their data: the one naming a member keeps that grant alone, the other an "
                "empty grant list; who reads each (the owner, the named member, the team's former member, no "
                "credential); on the new release another team deleted without the flag takes its instance")

    def populate(self, w):
        kora, kurt, kobi = people(w, "kora", "kurt", "kobi")
        org = new_org(kora, "Keep Org")
        team = team_id(kora.raw("post", f"orgs/{org}/teams", {"name": "keep", "join": "invite"}))
        join(kora, org, kurt, teams=(team,))
        join(kora, org, kobi)
        named = kora.deploy(kv_wasm(w), "kept-named", org=org)
        alone = kora.deploy(kv_wasm(w), "kept-alone", org=org)
        for app in (named, alone):
            kora.call(app, "set", {"key": "k", "value": "kept"})
        until("the cluster version takes grants", lambda: kora.raw("post", f"apps/{named}/access", {
            "grants": [{"kind": "team", "team": team}, {"kind": "account", "account": kobi.id}]}), timeout=120)
        kora.raw("post", f"apps/{alone}/access", {"grants": [{"kind": "team", "team": team}]})
        out = kora.raw("delete", f"orgs/{org}/teams/{team}", {"keepApps": True})
        eq(sorted(out.get("keptApps") or []), sorted([named, alone]), "the instances kept")
        s = {"org": org, "named": named, "alone": alone, "kobi": kobi.id}
        s["decisions"] = self._decide(w, s)
        for want in ("kora reads kept-named", "kobi reads kept-named", "kora reads kept-alone"):
            check(s["decisions"][want] == "ok", f"{want}: {s['decisions'][want]}")
        for want in ("kurt reads kept-named", "kurt reads kept-alone", "kobi reads kept-alone"):
            check(s["decisions"][want] != "ok", f"{want} was allowed")
        return s

    def _decide(self, w, s):
        out = {}
        for key in ("named", "alone"):
            app = s[key]
            for who in ("kora", "kurt", "kobi"):
                p = person(w, who)
                out[f"{who} reads kept-{key}"] = decision(lambda p=p, app=app: p.view(app, "get", {"key": "k"}))
            out[f"anonymous reads kept-{key}"] = decision(
                lambda app=app: anyone(w, "POST", f"apps/{app}/views", {"method": "get", "args": {"key": "k"}}))
        return out

    def check(self, w, s):
        kora = person(w, "kora")
        eq(kora.get(f"apps/{s['named']}").get("grants"), [{"kind": "account", "account": s["kobi"]}],
           "the kept instance's other grant")
        eq(kora.get(f"apps/{s['alone']}").get("grants"), [], "the kept instance with none left")
        eq(kora.result(s["alone"], "get", {"key": "k"}), "kept", "its data")
        same_decisions(self._decide(w, s), s["decisions"], "instances kept with their team deleted")

    def exercise(self, w, s):
        kora = person(w, "kora")
        team = team_id(kora.raw("post", f"orgs/{s['org']}/teams", {"name": "gone", "join": "invite"}))
        app = kora.deploy(kv_wasm(w), "goes", org=s["org"])
        kora.raw("post", f"apps/{app}/access", {"grants": [{"kind": "team", "team": team}]})
        out = kora.raw("delete", f"orgs/{s['org']}/teams/{team}")
        eq(out.get("apps"), [app], "the default deletes the instance only the team admitted")


class Visitor:
    """A visitor of a public link: its own Ed25519 key (`seed`), proving each request over the
    link's key, its own and the time (`proof::visitor_message`) in a `vis1.` bearer credential."""

    def __init__(self, token: str, seed: int):
        self.token = token
        self.seed = bytes([seed]) * 32
        self.key = ed25519_sign.public_key(self.seed).hex()

    @property
    def link(self) -> str:
        """`proof::public_link_key`: the public key of the token's seed."""
        seed = hashlib.sha256(b"calimero.public-link.v1\0" + self.token.encode()).digest()
        return ed25519_sign.public_key(seed).hex()

    def name(self) -> str:
        return f"visitor:{self.link}:{self.key}"

    def bearer(self) -> str:
        at = int(time.time())
        msg = b"calimero.visitor.v1\0" + self.link.encode() + b"\0" + self.key.encode() + b"\0" + str(at).encode()
        return f"vis1.{self.token}.{self.key}.{at}.{ed25519_sign.sign(self.seed, msg).hex()}"

    def call(self, w, app, method, args, node):
        return _http("POST", w.fleet.url(node) + f"apps/{app}/intents", {"method": method, "args": args},
                     self.bearer(), 60)["result"]

    def view(self, w, app, method, args, node):
        return _http("POST", w.fleet.url(node) + f"apps/{app}/views", {"method": method, "args": args},
                     self.bearer(), 60)["result"]


@case
class PublicLinks(DeepCase):
    name = "public-links"
    area = "Apps"
    since = "1.0.0-rc.33"
    seeds = ("an organization's private instance with a public link (label, rate) and a revoked one; a visitor "
             "with a key of its own signed the guestbook (a `#[app::public]` method) through the link")
    verifies = ("the instance's links as its owner lists them (never the token); the visitor, named by link and key "
                "(`Caller::Visitor`), calls the public view through every node and is refused a method that is not "
                "public; the revoked link's visitor is refused; what the visitor wrote, as the owner reads it; on "
                "the new release the visitor signs again and a link made there admits a visitor of its own")

    def populate(self, w):
        pella = person(w, "pella")
        org = new_org(pella, "Links Org")
        app = pella.deploy(kv_wasm(w), "guestbook", org=org)
        made = pella.raw("post", f"apps/{app}/public-links", {"label": "Survey", "ratePerMinute": 60})
        gone = pella.raw("post", f"apps/{app}/public-links", {"label": "Old"})
        pella.raw("delete", f"apps/{app}/public-links/{gone['link']}")
        v = Visitor(made["token"], 1)
        eq(v.link, made["link"], "the link's key, from its token")
        s = {"org": org, "app": app, "links": {made["token"]: 1}, "revoked": gone["token"], "notes": {}}
        self._sign(w, s, v, "hello")
        s["refused"] = decision(lambda: v.call(w, app, "set", {"key": "k", "value": "v"}, w.members()[0]))
        check(s["refused"] != "ok", "a visitor called a method that is not public")
        s["listed"] = scrub(pella.get(f"apps/{app}/public-links"))
        check(made["token"] not in str(s["listed"]), "a listing carries the token")
        eq([x.get("label") for x in s["listed"].get("links") or []], ["Survey"], "the links listed")
        return s

    def _sign(self, w, s, v, note):
        n = w.members()[0]
        eq(v.call(w, s["app"], "signGuestbook", {"note": note}, n), v.name(), "the guestbook's signer")
        s["notes"][v.name()] = note

    def check(self, w, s):
        pella = person(w, "pella")
        subset(scrub(pella.get(f"apps/{s['app']}/public-links")), s["listed"], "the instance's links")
        for token, seed in s["links"].items():
            v = Visitor(token, seed)
            for n in w.members():
                eq(v.view(w, s["app"], "whoamiPublic", {}, n), v.name(), f"the visitor through node {n}")
            eq(decision(lambda v=v: v.call(w, s["app"], "set", {"key": "k", "value": "v"}, w.members()[0])),
               s["refused"], "a visitor calling a method that is not public")
        for who, note in s["notes"].items():
            eq(pella.result(s["app"], "get", {"key": f"sign:{who}"}), note, "what the visitor wrote")
        gone = Visitor(s["revoked"], 3)
        check(decision(lambda: gone.view(w, s["app"], "whoamiPublic", {}, w.members()[0])) != "ok",
              "the revoked link's visitor was let in")

    def exercise(self, w, s):
        pella = person(w, "pella")
        token = next(iter(s["links"]))
        self._sign(w, s, Visitor(token, 1), "again, on the new release")
        made = pella.raw("post", f"apps/{s['app']}/public-links", {"label": "New"})
        s["links"][made["token"]] = 2
        self._sign(w, s, Visitor(made["token"], 2), "through the new link")
        s["listed"] = scrub(pella.get(f"apps/{s['app']}/public-links"))


def _xcalls(w, s, node=None):
    """`GET /apps/{a}/xcalls`, from a node that holds the app (only one answers it)."""
    return scrub(person(w, "xan").get(f"apps/{s['a']}/xcalls", node=node or host(w, s["a"])))


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
        xan = person(w, "xan")
        org = new_org(xan, "Xcall Org")
        s = {"org": org, "a": xan.deploy(kv_wasm(w), "xc-a", org=org), "b": xan.deploy(kv_wasm(w), "xc-b", org=org),
             "relayed": {}}
        self._relay(w, s, "seed")
        # A method that allows no app: refused, a final answer. A missing target: dead-lettered,
        # and the source's onXcallFailed hears of it.
        xan.call(s["a"], "relayToSet", {"app": s["b"], "key": "nope", "value": "x"})
        xan.call(s["a"], "relay", {"app": "no-such-app", "key": "lost", "value": "x"})
        s["failures"] = until("the dead letter is reported", lambda: _nonempty(xan.result(s["a"], "xcallFailures")))
        check(xan.result(s["b"], "get", {"key": "nope"}) is None, "a method that allows no app ran")
        s["dead"] = _xcalls(w, s)["deadLetters"]
        check(len(s["dead"]) == 1 and s["dead"][0]["app"] == "no-such-app", f"the dead letter: {s['dead']}")
        return s

    def _relay(self, w, s, tag):
        xan = person(w, "xan")
        key = f"relay-{tag}"
        xan.call(s["a"], "relay", {"app": s["b"], "key": key, "value": tag})
        until(f"the {tag} relay lands", lambda: eq(xan.result(s["b"], "get", {"key": key}), tag, key))
        s["relayed"][key] = tag

    def check(self, w, s):
        xan = person(w, "xan")
        for k, v in s["relayed"].items():
            eq(xan.result(s["b"], "get", {"key": k}), v, f"relayed {k}")
        eq(xan.result(s["a"], "xcallFailures"), s["failures"], "xcall failures")
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
class XcallBatch(DeepCase):
    name = "xcall-batch"
    area = "Apps"
    since = "1.0.0-rc.31"
    seeds = ("three apps of an organization; a run that calls two of them together (`app::join!`, one `xcall.batch`) "
             "twice")
    verifies = ("each target's count and the answers of the joined calls; the batch runs again in the mixed cluster "
                "(its targets may sit on different releases) and on the new release, each target counting once per run")

    def populate(self, w):
        bix = person(w, "bix")
        org = new_org(bix, "Batch Org")
        s = {"org": org, "count": 0, **{k: bix.deploy(kv_wasm(w), f"batch-{k}", org=org) for k in ("a", "b", "c")}}
        for _ in range(2):
            self._bump(w, s)
        return s

    def _bump(self, w, s):
        got = person(w, "bix").call(s["a"], "bumpBoth", {"a": s["b"], "b": s["c"], "key": "n", "spins": 1})["result"]
        s["count"] += 1
        eq(got, [s["count"], s["count"]], "the joined calls' answers")

    def check(self, w, s):
        bix = person(w, "bix")
        for k in ("b", "c"):
            eq(bix.result(s[k], "get", {"key": "n"}), str(s["count"]), f"batch-{k}'s count")

    def during_mix(self, w, s):
        self.check(w, s)
        self._bump(w, s)
        self.check(w, s)

    def exercise(self, w, s):
        self._bump(w, s)
        self.check(w, s)


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
    seeds = ("an app handed to another member of its organization (transfer); an app moved to another "
             "organization with its members")
    verifies = ("the owner and organization of each, their data; the moved app's members reach it in its new "
                "organization; handed back on the new release")

    def populate(self, w):
        tess, ted, mae = people(w, "tess", "ted", "mae")
        home = new_org(tess, "Home Org")
        # Organizations are ordinary (1.0.0-rc.32): an app goes only to a member of its
        # organization, so ted joins tess's first.
        join(tess, home, ted)
        given = tess.deploy(kv_wasm(w), "given", org=home)
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
        eq((s["records"]["given"].get("owner"), s["records"]["given"].get("org")), (ted.id, home),
           "the transferred app's owner, in its organization")
        s["decisions"] = self._decide(w, s)
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
        # The app stays in tess's organization, where both are members.
        org = ted.get(f"apps/{s['given']}")["org"]
        ted.raw("post", f"apps/{s['given']}/transfer", {"to": tess.id})
        s["owner"] = "tess"
        s["records"]["given"] = snap(tess, f"apps/{s['given']}")
        s["decisions"] = self._decide(w, s)
        # The app stays in its organization, so ted, a member of it, still reaches it; the
        # app is tess's.
        eq((s["records"]["given"].get("owner"), s["records"]["given"].get("org")), (tess.id, org),
           "handed back on the new release, in its organization")
        eq((s["decisions"]["tess reads given"], s["decisions"]["ted reads given"]), ("ok", "ok"),
           "the new owner and the organization's owner read it")


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
        got, want = settled_days(old, u)
        eq(got, want, "the app's daily usage")
        now = u.get("today", {})
        if now.get("date") == today:
            for f in ("runs", "writes", "bytesWritten"):
                check(int(now[f]) >= int(old["today"][f]), f"today's {f} shrank: {now[f]} < {old['today'][f]}")
        same_read(una, "auth/usage", s["me"], "the account's usage", USAGE_VOLATILE)
        same_read(una, f"orgs/{s['org']}/usage", s["org_usage"], "the organization's usage", USAGE_VOLATILE | {"apps"})
        subset(una.result(s["app"], "len"), 10, "the metered app")


# What no case in this file seeds yet, and why (the matrix lists it as not covered).

@case
class AppOrigins(DeepCase):
    name = "app-origins"
    area = "Apps"
    since = "1.0.0-rc.39"
    seeds = "a web origin registered for one instance by its owner"
    verifies = ("the owner's list as they read it; a stranger refused the list and the registration; on every "
                "node a sign-in begun from the origin names its host as its rp-id; on the new release the origin "
                "removed and another registered, after a restart too; a removed origin is refused "
                "`ORIGIN_NOT_REGISTERED`. The assertion itself is not signed")

    def _path(self, s):
        return f"apps/{s['app']}/origins"

    def _decisions(self, w, s):
        zola = person(w, "zola")
        return decisions({"stranger lists": lambda: zola.get(self._path(s)),
                          "stranger registers": lambda: zola.raw("post", self._path(s),
                                                                 {"origin": "https://zola-app.example.test"})})

    def _begin(self, w, origin, node):
        got = decision(lambda: anyone(w, "POST", "auth/passkey/login/begin", {"origin": origin}, node=node))
        if got != "ok":
            return got
        return anyone(w, "POST", "auth/passkey/login/begin", {"origin": origin}, node=node)["options"]["rpId"]

    def _ceremonies(self, w, s):
        for node in w.members():
            eq(self._begin(w, s["origin"], node), s["origin"].split("://", 1)[1], f"{s['origin']}'s rp-id on node {node}")
            for origin in s["gone"]:
                eq(self._begin(w, origin, node), "ORIGIN_NOT_REGISTERED", f"{origin} on node {node}")

    def _read(self, w, s):
        s["read"] = snap(person(w, "yara"), self._path(s))
        eq([o["origin"] for o in s["read"]["origins"]], [s["origin"]], "the instance's origins")

    def populate(self, w):
        yara = person(w, "yara")
        s = {"app": yara.deploy(kv_wasm(w), "origins"), "origin": "https://store-yara.example.test",
             "gone": ["https://never-yara.example.test"]}
        yara.raw("post", self._path(s), {"origin": s["origin"]})
        s["decisions"] = self._decisions(w, s)
        check("ok" not in s["decisions"].values(), f"a stranger handled the origins: {s['decisions']}")
        self._read(w, s)
        self._ceremonies(w, s)
        return s

    def check(self, w, s):
        yara = person(w, "yara")
        until("the origins read back", lambda: same_read(yara, self._path(s), s["read"], "the instance's origins"), 15)
        same_decisions(self._decisions(w, s), s["decisions"], "a stranger and the origins")
        self._ceremonies(w, s)

    def exercise(self, w, s):
        yara = person(w, "yara")
        yara.raw("delete", self._path(s), {"origin": s["origin"]})
        s["gone"].append(s["origin"])
        s["origin"] = "https://store-yara-2.example.test"
        yara.raw("post", self._path(s), {"origin": s["origin"]})
        self._read(w, s)
        self.check(w, s)


not_covered("Apps", "Only an owner or admin of an app's organization approves its upgrades, runs and upgrade keys "
            "(`SystemState::approves_upgrades`, `owner_approves` on `UpgradeApp`, `StartUpgradeRun` and `CreateApiKey`)",
            "it changes who may, not a stored shape: an entry without `owner_approves` reads and replays as it applied "
            "below cluster version 1.0.0-rc.33 (`Feature::OwnerApprovedUpgrades`), and the owner's upgrade after the "
            "roll is already the `kv-package` case's (`extend`: the organization's owner upgrades on the new release). "
            "`system::tests::only_an_organizations_owner_and_admins_approve_its_upgrades` covers the replay, and "
            "`a_member_needs_the_capability_to_create_apps_or_invite`, `the_leader_checks_upgrade_authority_again` and "
            "`an_upgrade_key_is_an_owners_or_admins_of_the_apps_organization` the refusals on one build")

not_covered("Apps", "A green update applying on its own after its hold (`UpgradeApp::auto`, `lastUpgrade.auto`)",
            "the hold is at least a day (`MIN_HOLD_HOURS`) of logged time from the release's install, which a deep run "
            "cannot wait on either release (the tests move the update clock with a testing hook a release `merod` "
            "lacks), so no automatic update is seeded or made; the policy that asks for one is the "
            "`upgrade-policies` case's, and the restore point every upgrade takes and its undo are "
            "`restore-points-undo`'s. An upgrade entry and a record without `auto` read and replay as before. "
            "`system::tests` (`a_green_update_applies_on_its_own_after_its_hold_under_the_policy`, "
            "`a_deployment_instances_hold_runs_from_its_offer`) and `tests/auto_updates.rs` cover it on one build")

not_covered("Apps", "An undo refused to a release no longer installed (`UndoUpgrade::release_installed`, `UNDO_RELEASE_GONE`)",
            "it adds a refusal, not a stored shape: an entry without the flag replays as it applied below cluster "
            "version 1.0.0-rc.36 (`Feature::UndoToInstalledRelease`), and the undo itself is `restore-points-undo`'s. "
            "The refusal needs a release uninstalled while another release's WASM keeps the bundle, which the case's "
            "single-WASM releases do not make. `system::tests::an_undo_never_returns_to_a_release_that_is_no_longer_installed` "
            "and `tests/auto_updates.rs::an_undo_is_refused_to_a_release_that_was_uninstalled` cover it on one build")

not_covered("Apps", "An instance's outbound audit (`GET /apps/{app}/outbound-log`, `meroctl instance outbound-log`)",
            "it stores nothing: each node keeps its runs' requests in memory (`calimero_outbound::OutboundAudit`), which "
            "a restart or a roll empties by design, so there is no data to carry across releases; the suite's clusters "
            "also run with outbound off. `an_instances_owner_reads_its_outbound_requests_without_values` "
            "(`component_cluster.rs`) and `calimero_outbound`'s audit tests cover it on one build")

not_covered("Apps", "Host work charged to fuel, and at most `MAX_SEARCHES_PER_RUN` searches per request (`calimero_app_host`'s fuel table)",
            "it changes what a run may spend, not a stored shape: followers apply a call's diff and never run it, so "
            "the log replays alike on either release, and a search cap answers a view, which is never logged. The "
            "deep cases' calls and searches run far within the default fuel on both releases. `tests/engine.rs` "
            "(`reading_megabytes_of_state_costs_fuel`, `a_view_makes_at_most_max_searches_per_run`, "
            "`a_guest_cannot_spend_fuel_its_host_calls_already_owe`, `refused_xcall_arguments_cost_fuel`, "
            "`stdout_costs_fuel_after_the_log_is_full`) covers it on one build")

not_covered("Apps", "Timers (`#[app::every]`) firing across the roll",
            "the only timer the suite's apps carry is the kv fixture's `beat`, declared `#[app::every(\"1h\")]` and run on "
            "the nodes' wall clock: one tick needs an hour, and a roll takes minutes, so no tick falls in any phase and "
            "there is nothing to seed or read. The tests move the app clock with a test-only clock "
            "that a release `merod` lacks, and no flag or route moves it; `ticks_run_once_per_slot`, "
            "`a_tick_runs_once_across_failover` and `missed_ticks_collapse_into_one` (`component_cluster.rs`) cover ticks "
            "on one build")

not_covered("Apps", "The consent page's install sheet (`install`), update sheets with their class, policy and "
            "approval step (\"Approve (1 of 2)\", \"Approve and update (2 of 2)\"), an update policy changed on the page "
            "(`updatePolicy`), the node's updates page (`updates`, `/updates/{id}`, `POST /consent/{id}/open`) and "
            "`GET /apps/{app}/update`'s `summary`",
            "pages and reads that store nothing of their own: a consent request lives in one node's memory for minutes, "
            "and what the pages approve is logged by the routes they run (`POST /apps`, `SetUpgradePolicy`, "
            "`ApproveUpgrade`, `UpgradeApp`), whose stored shapes are the `upgrade-policies` and "
            "`update-classes-approvals` cases'. "
            "`an_install_sheet_shows_what_gets_created_and_where_data_can_go` and "
            "`an_update_shows_its_class_and_takes_two_approvers_on_the_pages` (`consent_page.rs`) cover them on one build")
not_covered("Apps", "A view's arguments held to `MAX_ARGS_BYTES` (`413 ARGS_TOO_LARGE` before it runs, as a call's)",
            "it adds a refusal of a request, not a stored shape: a view logs nothing, and the API refuses before any "
            "node runs it, so there is no data to carry across releases. "
            "`oversized_view_args_are_refused_before_running` (`component_cluster.rs`) covers it on one build")

not_covered("Apps", "An xcall chain ending at its budget (`MAX_XCALL_CHAIN`, `Feature::XcallBudget`): a message sent "
            "with none left is dead-lettered",
            "the chain needs `hop`, which the published kv fixture the old release seeds with does not have; what a "
            "chain stores is an outbox row and a dead letter, whose old shapes (no budget) the `xcall` case reads "
            "back. `an_app_that_xcalls_itself_stops_at_the_chain_budget` (`component_cluster.rs`) covers it on one "
            "build")
not_covered("Apps", "A pinned instance (`lineage: pinned`, `AppRecord::lineage`) held to its package, publisher and "
            "forward order",
            "no release before 1.0.0-rc.39 installs a pinning release (`Feature::PinnedLineage`), so the old build "
            "seeds none; `tests/it/lineage.rs` covers the refusals end to end and `system/tests.rs` the rule at apply")
