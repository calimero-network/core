"""Deep cases: an organization's name and its member defaults."""

from __future__ import annotations

from upgrade_deep import DeepCase, check, eq, install, join, new_org, people, scrub, subset
from upgrade_path import case

NAME = "Renamed Org"
CAPS = 0b110  # INVITE_MEMBERS and JOIN_OPEN_TEAMS


@case
class OrgRenameDefaults(DeepCase):
    name = "org-rename-defaults"
    area = "Organizations"
    seeds = ("an organization renamed, a member's all-apps flag and the organization's default capabilities set "
             "(`POST /orgs/{org}/name`, `/all-apps`, `/capabilities`), and an app team made after them")
    verifies = ("the name, the default capabilities and the app team's rows as the owner and the member read the "
                "organization on every node, and the name in the member's account; on the new release a second "
                "rename and other default capabilities")

    def _record(self, w, s):
        wren, wade = people(w, "wren", "wade")
        s["owner_read"] = scrub(wren.get(f"orgs/{s['org']}"))
        s["member_read"] = scrub(wade.get(f"orgs/{s['org']}"))
        eq(s["owner_read"].get("name"), s["name"], "the organization's name")
        eq(s["owner_read"].get("defaultCapabilities"), s["caps"], "the default capabilities")

    def populate(self, w):
        wren, wade, wynn = people(w, "wren", "wade", "wynn")
        org = new_org(wren, "Names Org")
        join(wren, org, wade, wynn)
        wren.raw("post", f"orgs/{org}/name", {"name": NAME})
        wren.raw("post", f"orgs/{org}/all-apps", {"account": wade.id, "allApps": True})
        wren.raw("post", f"orgs/{org}/capabilities", {"capabilities": CAPS})
        package, version = install(wren, w, "kv-c-1.0.0.mpk")
        wren.raw("post", "apps", {"name": "names", "package": package, "version": version, "org": org})
        wren.raw("post", f"orgs/{org}/app-teams", {"apps": [{"package": package}]})
        s = {"org": org, "name": NAME, "caps": CAPS}
        self._record(w, s)
        check(s["owner_read"].get("teams"), f"no app team: {s['owner_read']}")
        return s

    def check(self, w, s):
        wren, wade = people(w, "wren", "wade")
        for n in w.members():
            subset(scrub(wren.get(f"orgs/{s['org']}", node=n)), s["owner_read"], f"the owner's read on node {n}")
            subset(scrub(wade.get(f"orgs/{s['org']}", node=n)), s["member_read"], f"the member's read on node {n}")
            named = [o["name"] for o in wade.get("auth/me", node=n)["orgs"] if o["orgId"] == s["org"]]
            eq(named, [s["name"]], f"the organization's name in the member's account on node {n}")

    def exercise(self, w, s):
        wren = people(w, "wren")[0]
        s["name"], s["caps"] = "Renamed Again", 0b100
        wren.raw("post", f"orgs/{s['org']}/name", {"name": s["name"]})
        wren.raw("post", f"orgs/{s['org']}/capabilities", {"capabilities": s["caps"]})
        self._record(w, s)
        self.check(w, s)
