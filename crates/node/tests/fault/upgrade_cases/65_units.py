"""Deep cases: units and the apps an instance uses: an instance created with the apps its
release creates (`uses`, `mode = "create"`), upgraded and deleted whole; an app attached to an
instance's `attach` reference, and a person's grant letting that instance act as them there.

The packages are the release's own units examples (`apps/calendar`, `apps/planner`,
`apps/booking`), built from its tag and packed from each crate's metadata and
`calimero.lock` by its own meroctl, and its kv component packed as a unit
(scripts/build-upgrade-deep-fixtures.sh)."""

from __future__ import annotations

import json

from upgrade_deep import (DeepCase, Failed, check, decision, eq, install, join, new_org, people, person, refused,
                          scrub, subset, until)
from upgrade_path import case

CALENDAR = "com.calimero.example.calendar"
PLANNER = "com.calimero.example.planner"
BOOKING = "com.calimero.example.booking"
KV_UNIT = "com.calimero.upgrade.kv-unit"


def _create(p, name, package, version, org):
    return p.raw("post", "apps", {"name": name, "package": package, "version": version, "org": org})


def _release(p, app):
    r = p.get(f"apps/{app}").get("release") or {}
    return r.get("package"), r.get("version")


def _gone(p, app, what):
    refused(("NOT_FOUND", "FORBIDDEN"), lambda: p.get(f"apps/{app}"), what)


def _run_done(p, org, run):
    status = p.get(f"orgs/{org}/upgrades/{run}")
    if status.get("finished") is not True:
        raise Failed(f"the unit's upgrade run is still going: {json.dumps(status)[:300]}")
    return status


def _update(p, app):
    """What `GET /apps/{app}/update` says that the signed manifests decide: whether the
    instance is a unit and the update's release, class, reasons and destinations."""
    u = p.get(f"apps/{app}/update")
    a = u.get("available")
    keep = ("package", "version", "source", "class", "reasons", "egress")
    return {"unit": u.get("unit"), "available": a and {k: a[k] for k in keep if k in a}}


@case
class Units(DeepCase):
    name = "units"
    area = "Apps"
    since = "1.0.0-rc.32"
    seeds = ("planner 0.1.0 and 0.2.0, whose releases `use` a calendar with `mode = \"create\"`, and calendar 0.1.0 "
             "and 0.1.1: a planner instance created with its calendar (`links`) and events scheduled through it; a "
             "second unit upgraded to 0.2.0 in one consent (`upgrade-unit`, its calendar to the 0.1.1 the release "
             "pins); a third unit left to delete; a kv unit whose member reads the instance that created it")
    verifies = ("each instance's `links` and its member's record (name, organization, owner, release); the agenda the "
                "planner reads from its calendar through every node; the upgraded unit's run and both releases; a "
                "member deleted alone is refused `UNIT_MEMBER`; the release's `uses` on its capability card, the unit's "
                "destinations (`egress`) and its update's verdict; the kv member's `unitOf` and the instance's `linked` through every node; on the new release the first unit schedules again "
                "and upgrades in one consent keeping its agenda, and the third is deleted whole with its calendar")

    def populate(self, w):
        uri = person(w, "uri")
        for mpk in ("calendar-0.1.0.mpk", "calendar-0.1.1.mpk", "planner-0.1.0.mpk", "planner-0.2.0.mpk"):
            install(uri, w, mpk)
        org = new_org(uri, "Units Org")
        s = {"org": org, "units": {}, "agenda": [], "deleted": False}
        for name in ("kept", "moved", "doomed"):
            made = _create(uri, f"plan-{name}", PLANNER, "0.1.0", org)
            app, cal = made["appId"], (made.get("links") or {}).get("calendar")
            check(cal, f"plan-{name} was created without its calendar: {made}")
            s["units"][name] = {"app": app, "calendar": cal}
        kept = s["units"]["kept"]
        for i, at in enumerate(("2026-12-01T09:00", "2026-12-02T10:00")):
            title = f"standup {i}"
            uri.call(kept["app"], "schedule", {"title": title, "at": at})
            s["agenda"].append({"title": title, "at": at, "private": False})

        moved = s["units"]["moved"]
        run = uri.raw("post", f"apps/{moved['app']}/upgrade-unit", {"version": "0.2.0"})
        eq(run.get("targets"), {moved["calendar"]: {"package": CALENDAR, "version": "0.1.1"}},
           "the unit's targets: its calendar to the release planner 0.2.0 pins")
        s["run"] = run["runId"]
        s["run_status"] = scrub(until("the unit's upgrade run finishes", lambda: _run_done(uri, org, s["run"]), 90))
        eq(s["run_status"].get("failed"), {}, "the unit's upgrade run failed nothing")
        uri.call(moved["app"], "schedule", {"title": "after the upgrade", "at": "2026-12-03T11:00"})
        s["moved_agenda"] = [{"title": "after the upgrade", "at": "2026-12-03T11:00", "private": False}]

        s["versions"] = {"kept": ("0.1.0", "0.1.0"), "moved": ("0.2.0", "0.1.1"), "doomed": ("0.1.0", "0.1.0")}
        s["members"] = {k: scrub(uri.get(f"apps/{u['calendar']}")) for k, u in s["units"].items()}
        s["card"] = uri.get(f"packages/{PLANNER}/0.1.0/capabilities").get("uses")
        check(s["card"], "planner 0.1.0's card names no `uses`")
        s["egress"] = scrub(uri.get(f"apps/{kept['app']}/egress"))
        s["update"] = _update(uri, kept["app"])
        eq((s["update"]["unit"], (s["update"]["available"] or {}).get("version")), (True, "0.2.0"),
           "the first unit's update")

        # A kv unit: its member names the instance that created it (`env::unit_of`), and the
        # instance reads its member with `env::linked`.
        for mpk in ("kv-secret-1.0.0.mpk", "kv-unit-1.0.0.mpk"):
            install(uri, w, mpk)
        made = _create(uri, "kv-unit", KV_UNIT, "1.0.0", org)
        s["kv"] = {"app": made["appId"], "member": (made.get("links") or {}).get("member")}
        check(s["kv"]["member"], f"kv-unit was created without its member: {made}")
        return s

    def check(self, w, s):
        uri = person(w, "uri")
        for name, u in s["units"].items():
            if name == "doomed" and s["deleted"]:
                _gone(uri, u["app"], "the unit deleted whole")
                _gone(uri, u["calendar"], "the deleted unit's calendar")
                continue
            app = uri.get(f"apps/{u['app']}")
            eq(app.get("links"), {"calendar": u["calendar"]}, f"plan-{name}'s links")
            member = uri.get(f"apps/{u['calendar']}")
            subset(scrub(member), s["members"][name], f"plan-{name}'s calendar")
            eq((member.get("name"), member.get("org"), member.get("owner")),
               (f"plan-{name}.calendar", s["org"], uri.id), f"plan-{name}'s calendar is the unit's")
            want = s["versions"][name]
            eq((_release(uri, u["app"]), _release(uri, u["calendar"])),
               ((PLANNER, want[0]), (CALENDAR, want[1])), f"plan-{name}'s releases")
            eq(decision(lambda u=u: uri.raw("delete", f"apps/{u['calendar']}")), "UNIT_MEMBER",
               f"plan-{name}'s calendar deleted alone")
        kept, moved = s["units"]["kept"], s["units"]["moved"]
        for n in w.members():
            eq(uri.result(kept["app"], "agenda", node=n), s["agenda"], f"the first unit's agenda through node {n}")
        eq(uri.result(moved["app"], "agenda"), s["moved_agenda"], "the upgraded unit's agenda")
        subset(scrub(uri.get(f"orgs/{s['org']}/upgrades/{s['run']}")), s["run_status"], "the unit's upgrade run")
        subset(uri.get(f"packages/{PLANNER}/0.1.0/capabilities").get("uses"), s["card"], "planner 0.1.0's `uses`")
        subset(scrub(uri.get(f"apps/{kept['app']}/egress")), s["egress"], "the first unit's destinations")
        subset(_update(uri, kept["app"]), s["update"], "the first unit's update")
        kv = s["kv"]
        eq(uri.get(f"apps/{kv['app']}").get("links"), {"member": kv["member"]}, "kv-unit's links")
        for n in w.members():
            eq(uri.result(kv["member"], "unitOf", node=n), kv["app"], f"the member's unit through node {n}")
            eq(uri.result(kv["app"], "unitOf", node=n), None, f"the instance is no member, through node {n}")
            eq(uri.result(kv["app"], "linked", {"name": "member"}, node=n), kv["member"],
               f"the instance reads its member with `linked`, through node {n}")

    def exercise(self, w, s):
        uri = person(w, "uri")
        kept, doomed = s["units"]["kept"], s["units"]["doomed"]
        uri.call(kept["app"], "schedule", {"title": "after the roll", "at": "2026-12-04T12:00"})
        s["agenda"].append({"title": "after the roll", "at": "2026-12-04T12:00", "private": False})

        run = uri.raw("post", f"apps/{kept['app']}/upgrade-unit", {"version": "0.2.0"})
        eq(run.get("targets"), {kept["calendar"]: {"package": CALENDAR, "version": "0.1.1"}},
           "the first unit's targets on the new release")
        status = until("the unit's upgrade run finishes", lambda: _run_done(uri, s["org"], run["runId"]), 90)
        eq(status.get("failed"), {}, "the unit's upgrade run on the new release failed nothing")
        s["versions"]["kept"] = ("0.2.0", "0.1.1")
        s["members"]["kept"] = scrub(uri.get(f"apps/{kept['calendar']}"))
        s["update"] = _update(uri, kept["app"])
        eq(s["update"]["available"], None, "nothing newer once the unit runs 0.2.0")
        s["egress"] = scrub(uri.get(f"apps/{kept['app']}/egress"))

        gone = uri.raw("delete", f"apps/{doomed['app']}")
        eq(gone.get("members"), [doomed["calendar"]], "the unit deleted whole names its calendar")
        s["deleted"] = True
        self.check(w, s)


def _free(s, who):
    """The offered slots `who` is still free at: booking reads the busy times of whoever it
    calls the calendar as, the person while they grant it, else itself."""
    taken = s["mine"] if (who == "bree" and s["granted"]) else s["booking"]
    busy = {e["at"] for e in taken}
    return [x for x in s["slots"] if x not in busy]


@case
class AttachedAppDelegation(DeepCase):
    name = "attached-app-delegation"
    area = "Apps"
    since = "1.0.0-rc.31"
    seeds = ("a booking page whose release `uses` a calendar with `mode = \"attach\"` and the methods it calls as the "
             "person; its owner attaches their calendar instance (`PUT /apps/{app}/links/calendar`); a member books "
             "a slot before granting it (the event is booking's own), grants it with their passkey "
             "(`PUT /apps/{app}/delegations/{target}`) and books one that lands in their own calendar")
    verifies = ("the attached app (`attached`, the link's row with its release and `inRange`) and the member's grant "
                "with its methods; booking reaches the attached calendar (`env::linked`) and acts as the member there "
                "(`env::via`): the member's own events, each one's free slots through every node, and a private "
                "booking through the app refused; on the new release a booking lands in the member's calendar, the "
                "grant is taken back and the next booking is booking's own again")

    SLOTS = ["2026-12-07T09:00", "2026-12-07T10:00", "2026-12-07T11:00", "2026-12-07T12:00"]

    def populate(self, w):
        bran, bree = people(w, "bran", "bree")
        for mpk in ("calendar-0.1.0.mpk", "booking-0.1.0.mpk"):
            install(bran, w, mpk)
        org = new_org(bran, "Booking Org")
        join(bran, org, bree)
        cal = _create(bran, "bran-calendar", CALENDAR, "0.1.0", org)["appId"]
        booking = _create(bran, "bran-booking", BOOKING, "0.1.0", org)["appId"]
        row = bran.raw("put", f"apps/{booking}/links/calendar", {"app": cal})
        eq((row.get("app"), row.get("mode"), row.get("inRange")), (cal, "attach", True), "the attached calendar")
        # Only the owner as themselves (`Caller::Account`, a passkey session) sets the slots.
        bran.raw("post", f"apps/{booking}/intents", {"method": "setSlots", "args": {"slots": self.SLOTS}})
        s = {"org": org, "cal": cal, "booking": [], "mine": [], "app": booking, "slots": self.SLOTS,
             "granted": False}
        self._book(w, s, self.SLOTS[0], "before the grant")
        bree.raw("put", f"apps/{booking}/delegations/{cal}", {})
        s["granted"] = True
        self._book(w, s, self.SLOTS[1], "with the grant")
        s["private"] = decision(lambda: bree.call(booking, "book", {"at": self.SLOTS[2], "title": "secret",
                                                                       "private": True}))
        check(s["private"] != "ok", "a private booking through booking was taken")
        s["link"] = scrub(self._link(bran, s))
        return s

    def _book(self, w, s, at, title):
        person(w, "bree").call(s["app"], "book", {"at": at, "title": title})
        (s["mine"] if s["granted"] else s["booking"]).append({"title": title, "at": at, "private": False})

    @staticmethod
    def _link(bran, s):
        rows = bran.get(f"apps/{s['app']}/links").get("links") or []
        return next((r for r in rows if r.get("name") == "calendar"), None)

    def check(self, w, s):
        bran, bree = people(w, "bran", "bree")
        eq(bran.get(f"apps/{s['app']}").get("attached"), {"calendar": s["cal"]}, "booking's attached calendar")
        subset(scrub(self._link(bran, s)), s["link"], "booking's calendar reference")
        granted = {s["cal"]: ["addEvent", "myEvents"]} if s["granted"] else {}
        eq(bree.get(f"apps/{s['app']}/delegations").get("delegations") or {}, granted, "bree's grant to booking")
        eq(bree.result(s["cal"], "myEvents"), s["mine"], "bree's own events in the calendar")
        for n in w.members():
            eq(bree.result(s["app"], "freeSlots", node=n), _free(s, "bree"), f"bree's free slots through node {n}")
            eq(bran.result(s["app"], "freeSlots", node=n), _free(s, "bran"), f"bran's free slots through node {n}")
        if s["granted"]:
            eq(decision(lambda: bree.call(s["app"], "book", {"at": s["slots"][2], "title": "secret",
                                                             "private": True})),
               s["private"], "a private booking through booking")

    def exercise(self, w, s):
        bree = person(w, "bree")
        self._book(w, s, self.SLOTS[3], "after the roll")
        eq(bree.result(s["cal"], "myEvents"), s["mine"], "the booking on the new release in bree's calendar")
        bree.raw("delete", f"apps/{s['app']}/delegations/{s['cal']}", approve=False)
        s["granted"] = False
        self._book(w, s, self.SLOTS[2], "after the grant")
        self.check(w, s)
