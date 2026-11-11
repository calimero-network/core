"""Deep cases: teams (open, closed, child, read-only and removed rows) and app teams (ScopeApps)."""

from __future__ import annotations

from upgrade_deep import (DeepCase, check, decision, install, join, kv_wasm, new_org, people, person,
                          same_decisions, same_read, snap, team_id, until)
from upgrade_path import case


@case
class Teams(DeepCase):
    name = "teams"
    area = "Teams"
    seeds = ("an open team; a closed team with a lead, a member, a read-only member and a removed member; a "
             "private child team; an app on each")
    verifies = ("each team as before; who reads and writes each team's app (the closed team excludes "
                "non-members, read-only reads only, the removed member is refused, the open team admits the "
                "organization, the child only its members); a member added on the new release writes")

    WHO = ("sue", "rod", "ray", "nat", "cid")

    def populate(self, w):
        tom, sue, rod, ray, nat, cid = people(w, "tom", *self.WHO)
        org = new_org(tom, "Tom Teams")
        join(tom, org, sue, rod, ray, nat, cid)

        def mk(name, how, parent=None):
            body = {"name": name, "join": how, **({"parent": parent} if parent else {})}
            return team_id(tom.raw("post", f"orgs/{org}/teams", body))

        teams = {"open": mk("general", "everyone"), "closed": mk("secret", "invite")}
        teams["child"] = mk("inner", "invite", teams["closed"])

        def add(t, p, **kw):
            tom.raw("post", f"orgs/{org}/teams/{teams[t]}/members", {"account": p.id, **kw})

        add("closed", sue, lead=True)
        add("closed", rod, role="readOnly")
        add("closed", ray)
        add("closed", cid)
        add("child", cid)
        tom.raw("delete", f"orgs/{org}/teams/{teams['closed']}/members/{ray.id}")
        apps = {}
        for label, t in teams.items():
            app = tom.deploy(kv_wasm(w), f"team-{label}", org=org)
            tom.raw("post", f"apps/{app}/access", {"access": {"kind": "team", "team": t}})
            tom.call(app, "set", {"key": "k", "value": label})
            apps[label] = app
        s = {"org": org, "teams": teams, "apps": apps}
        s["reads"] = {t: snap(tom, f"orgs/{org}/teams/{tid}") for t, tid in teams.items()}
        s["decisions"] = self._decide(w, s)
        for want in ("sue writes closed", "rod reads closed", "nat reads open", "cid writes child"):
            check(s["decisions"][want] == "ok", f"{want}: {s['decisions'][want]}")
        for want in ("rod writes closed", "ray reads closed", "nat reads closed", "sue reads child"):
            check(s["decisions"][want] != "ok", f"{want} was allowed")
        return s

    def _decide(self, w, s):
        out = {}
        for who in self.WHO:
            p = person(w, who)
            for label, app in s["apps"].items():
                out[f"{who} reads {label}"] = decision(lambda p=p, app=app: p.view(app, "get", {"key": "k"}))
                out[f"{who} writes {label}"] = decision(
                    lambda p=p, app=app: p.call(app, "set", {"key": p.name, "value": "1"}))
        return out

    def check(self, w, s):
        tom = person(w, "tom")
        for t, tid in s["teams"].items():
            same_read(tom, f"orgs/{s['org']}/teams/{tid}", s["reads"][t], f"team {t}")
        same_decisions(self._decide(w, s), s["decisions"], "team access")

    def exercise(self, w, s):
        tom, nat = people(w, "tom", "nat")
        tom.raw("post", f"orgs/{s['org']}/teams/{s['teams']['closed']}/members", {"account": nat.id})
        until("nat writes the closed team's app once added",
              lambda: nat.call(s["apps"]["closed"], "set", {"key": "nat", "value": "in"}))
        s["decisions"].update({"nat writes closed": "ok", "nat reads closed": "ok"})
        s["reads"]["closed"] = snap(tom, f"orgs/{s['org']}/teams/{s['teams']['closed']}")


@case
class AppTeams(DeepCase):
    name = "app-teams"
    area = "Teams"
    seeds = ("two packages' instances in an organization given app teams (ScopeApps): one closed keeping "
             "one member, one open")
    verifies = ("the app-team list; the kept member and the open team's members reach their apps, the member "
                "who lost the closed one is refused it; the closed one opened again on the new release admits them")

    def populate(self, w):
        ada, kip, lux = people(w, "ada", "kip", "lux")
        org = new_org(ada, "Ada Apps")
        join(ada, org, kip, lux)
        pa, va = install(ada, w, "kv-b-1.0.0.mpk")
        pb, vb = install(ada, w, "kv-c-1.0.0.mpk")
        apps = {"a": ada.raw("post", "apps", {"name": "a", "package": pa, "version": va, "org": org})["appId"],
                "b": ada.raw("post", "apps", {"name": "b", "package": pb, "version": vb, "org": org})["appId"]}
        for label, app in apps.items():
            ada.call(app, "set", {"key": "k", "value": label})
        ada.raw("post", f"orgs/{org}/app-teams", {"apps": [{"package": pa, "name": "A team"},
                                                            {"package": pb, "name": "B team"}]})
        # Closing A's app team keeps kip; everyone else loses it.
        ada.raw("post", f"orgs/{org}/app-teams", {"apps": [{"package": pa, "join": "invite", "keep": [kip.id]}]})
        s = {"org": org, "apps": apps, "packages": {"a": pa, "b": pb}}
        s["list"] = snap(ada, f"orgs/{org}/app-teams")
        s["decisions"] = self._decide(w, s)
        check(s["decisions"]["kip reads a"] == "ok" and s["decisions"]["lux reads a"] != "ok"
              and s["decisions"]["lux reads b"] == "ok", f"app-team access as seeded: {s['decisions']}")
        return s

    def _decide(self, w, s):
        out = {}
        for who in ("kip", "lux"):
            p = person(w, who)
            for label, app in s["apps"].items():
                out[f"{who} reads {label}"] = decision(lambda p=p, app=app: p.view(app, "get", {"key": "k"}))
        return out

    def check(self, w, s):
        same_read(person(w, "ada"), f"orgs/{s['org']}/app-teams", s["list"], "app teams")
        same_decisions(self._decide(w, s), s["decisions"], "app-team access")

    def exercise(self, w, s):
        ada, lux = people(w, "ada", "lux")
        ada.raw("post", f"orgs/{s['org']}/app-teams", {"apps": [{"package": s["packages"]["a"], "join": "everyone"}]})
        until("lux reaches the app team opened on the new release", lambda: lux.view(s["apps"]["a"], "get", {"key": "k"}))
        s["decisions"]["lux reads a"] = "ok"
        s["list"] = snap(ada, f"orgs/{s['org']}/app-teams")
