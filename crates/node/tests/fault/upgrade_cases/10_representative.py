"""The representative data set: accounts and keys, an organization with teams and
app teams, a signed package's app instance with history, plain blobs, sealed
files (several segments) and a finished compute run.

Every value here is written through the public API on the old release and read
back on the new one; `extend` adds the same kinds on the new release.
"""

from __future__ import annotations

import hashlib
import json

from upgrade_path import Case, NoUploadSessions, case

MIB = 1 << 20


def payload(tag: str, size: int) -> bytes:
    """`size` deterministic bytes named by `tag` (sha256 counter stream)."""
    out, n = bytearray(), 0
    seed = tag.encode()
    while len(out) < size:
        out += hashlib.sha256(seed + n.to_bytes(8, "big")).digest()
        n += 1
    return bytes(out[:size])


def members_of(org_json):
    """`GET /orgs/{org}`'s members: `{account: role}`."""
    return set(org_json.get("members") or {})


@case
class Accounts(Case):
    """Accounts with passkeys, meroctl sessions and API keys."""

    name = "accounts-and-keys"

    def seed(self, w):
        self.ids = {}
        for name in ("alice", "bob", "carol", "dave"):
            a = w.account(name)
            self.ids[name] = a.id
        alice = w.account("alice")
        self.extra_key = alice.cli("account", "api-key", "create", "--label", "upgrade-extra",
                                   "--dev-passkey", str(alice.passkey))["key"]
        self.labels = {"suite-alice", "upgrade-extra"}

    def verify(self, w):
        for name, account_id in self.ids.items():
            a = w.account(name)
            # The session meroctl saved on the old release: refreshed by the client if a
            # removed node issued it, never a new sign-in.
            me = a.cli("whoami")
            w.check(me.get("accountId") == account_id and me.get("via") == "session",
                    f"{name}'s saved session answers {me}, want {account_id}")
            me = a.http("GET", "auth/me")
            w.check(me.get("accountId") == account_id, f"{name}'s API key answers {me}")
        alice = w.account("alice")
        me = alice.http("GET", "auth/me", key=self.extra_key)
        w.check(me.get("accountId") == self.ids["alice"], f"alice's second key answers {me}")
        keys = alice.cli("account", "api-key", "list")  # a passkey session's route
        keys = keys.get("keys", keys) if isinstance(keys, dict) else keys
        labels = {k.get("label") for k in keys}
        w.check(self.labels <= labels, f"alice's keys {sorted(labels)}, want {sorted(self.labels)}")

    def extend(self, w):
        erin = w.account("erin")  # signs up on the new release
        self.ids["erin"] = erin.id
        alice = w.account("alice")
        alice.cli("account", "api-key", "create", "--label", "upgrade-new", "--dev-passkey", str(alice.passkey))
        self.labels.add("upgrade-new")


@case
class Organization(Case):
    """An organization with members, a team with a member, an open invite and app teams."""

    name = "organization"

    def seed(self, w):
        alice, bob = w.account("alice"), w.account("bob")
        self.org = alice.cli("namespace", "create", "Upgrade Org")["orgId"]
        w.shared["org"] = self.org
        token = alice.raw("post", f"orgs/{self.org}/invites", {"role": "member", "teams": []})["token"]
        bob.raw("post", f"invites/{token}/redeem", {})
        self.team = alice.raw("post", f"orgs/{self.org}/teams", {"name": "eng", "join": "invite"})["teamId"]
        alice.raw("post", f"orgs/{self.org}/teams/{self.team}/members", {"account": bob.id})
        # An invite made on the old release, redeemed on the new one (extend).
        self.pending = alice.raw("post", f"orgs/{self.org}/invites", {"role": "member", "teams": []})["token"]
        self.members = {alice.id, bob.id}
        self.teams = {self.team: ("eng", {alice.id, bob.id})}

    def verify(self, w):
        alice, dave = w.account("alice"), w.account("dave")
        org = alice.http("GET", f"orgs/{self.org}")
        got = set(members_of(org))
        w.check(self.members <= got, f"members {sorted(got)}, want {sorted(self.members)}")
        for team, (name, want) in self.teams.items():
            t = alice.http("GET", f"orgs/{self.org}/teams/{team}")
            have = {m["account"] for m in t["members"]}
            w.check(t.get("name") == name and want == have,
                    f"team {name}: {t.get('name')} with {sorted(have)}, want {sorted(want)}")
        bob_view = w.account("bob").http("GET", f"orgs/{self.org}")
        w.check(self.team in (bob_view.get("teams") or {}), f"bob does not see team eng: {json.dumps(bob_view)[:400]}")
        bob = w.account("bob")
        w.check(bob.http("GET", f"orgs/{self.org}").get("orgId") == self.org, "bob reads the org")
        w.expect_refused("NOT_FOUND", lambda: dave.http("GET", f"orgs/{self.org}"))

    def extend(self, w):
        alice, carol = w.account("alice"), w.account("carol")
        carol.raw("post", f"invites/{self.pending}/redeem", {})
        self.members.add(carol.id)
        ops = alice.raw("post", f"orgs/{self.org}/teams", {"name": "ops", "join": "invite"})["teamId"]
        alice.raw("post", f"orgs/{self.org}/teams/{ops}/members", {"account": carol.id})
        self.teams[ops] = ("ops", {alice.id, carol.id})


@case
class KvPackage(Case):
    """The release's signed kv package, an app instance of it in the org with history,
    an app team, a read-only key; upgraded on the new release to a build of this tree."""

    name = "kv-package"

    def seed(self, w):
        alice, bob = w.account("alice"), w.account("bob")
        org = w.shared["org"]
        self.package, self.version = w.registry.publish(w.fixtures["kv_from"])
        alice.raw("post", "packages/install", {"package": self.package, "version": self.version})
        self.app = alice.raw("post", "apps", {"name": "kv", "package": self.package, "version": self.version,
                                              "org": org})["appId"]
        w.shared["kv_app"] = self.app
        alice.raw("post", f"orgs/{org}/app-teams", {"apps": [{"package": self.package}]})
        self.expected = {}
        for i in range(30):  # history: writes by two members, overwrites, removals
            who = alice if i % 2 else bob
            who.call(self.app, "set", {"key": f"k{i % 20}", "value": f"v{i}"})
            self.expected[f"k{i % 20}"] = f"v{i}"
        for k in ("k3", "k7"):
            alice.call(self.app, "remove", {"key": k})
            self.expected.pop(k)
        alice.call(self.app, "note", {"id": "n1", "title": "upgrade", "body": "written on the old release"})
        self.index = int(alice.call(self.app, "set", {"key": "last", "value": "old"})["index"])
        self.expected["last"] = "old"
        self.read_key = alice.cli("account", "api-key", "create", "--label", "kv-read", "--read-only",
                                  "--context", self.app, "--dev-passkey", str(alice.passkey))["key"]
        self.release = self.version

    def verify(self, w):
        alice, bob, dave = w.account("alice"), w.account("bob"), w.account("dave")
        got = alice.view(self.app, "entries")["result"]
        w.check(got == self.expected, f"entries {json.dumps(got)[:400]} != {json.dumps(self.expected)[:400]}")
        replicas = next(a["replicas"] for a in w.fleet.placement()["apps"] if a["app"] == self.app)
        for node in replicas:  # every replica holds the whole history
            got = w.wait(f"node {node} catches up to index {self.index}",
                         lambda n=node: alice.view(self.app, "entries", node=n, stale=True,
                                                   min_index=self.index)["result"] == self.expected or None)
            w.check(got, f"node {node} answers other entries")
        teams = alice.http("GET", f"orgs/{w.shared['org']}/app-teams")
        w.check(self.package in json.dumps(teams), f"no app team for {self.package}: {json.dumps(teams)[:400]}")
        found = bob.view(self.app, "findNotes", {"text": "upgrade"})["result"]
        w.check(found == ["n1"], f"notes found {found}, want ['n1']")
        got = alice.view(self.app, "len", key=self.read_key)["result"]
        w.check(got == len(self.expected), f"len through the read-only key {got}, want {len(self.expected)}")
        w.expect_refused("FORBIDDEN", lambda: alice.call(self.app, "set", {"key": "x", "value": "y"},
                                                         key=self.read_key))
        w.expect_refused("NOT_FOUND", lambda: dave.call(self.app, "get", {"key": "k0"}))
        app = alice.http("GET", f"apps/{self.app}")
        release = json.dumps(app.get("release") or app)
        w.check(self.release in release, f"the app runs {release[:300]}, want release {self.release}")

    def extend(self, w):
        alice = w.account("alice")
        _, nxt = w.registry.publish(w.fixtures["kv_next"])
        alice.raw("post", "packages/install", {"package": self.package, "version": nxt})
        alice.cli("context", "update", self.app, "--version", nxt, "--dev-passkey", str(alice.passkey))
        self.release = nxt
        got = alice.view(self.app, "entries")["result"]
        w.check(got == self.expected, f"after the upgrade to {nxt}: entries {json.dumps(got)[:300]}")
        self.index = int(alice.call(self.app, "set", {"key": "last", "value": "new"})["index"])
        self.expected["last"] = "new"
        alice.call(self.app, "set", {"key": "after", "value": "upgrade"})
        self.expected["after"] = "upgrade"
        self.index = int(alice.call(self.app, "remove", {"key": "k0"})["index"])
        self.expected.pop("k0")


@case
class Blobs(Case):
    """Plain blobs of the kv app, one referenced from its state."""

    name = "blobs"

    def seed(self, w):
        alice = w.account("alice")
        self.app = w.shared["kv_app"]
        self.blobs = {}
        for tag, size in (("small", 1000), ("large", 3 * MIB + 17)):
            self._upload(alice, tag, size)
        alice.call(self.app, "attach", {"name": "large", "blob": self.blobs["large"][0]})

    def _upload(self, who, tag, size):
        data = payload(f"blob-{tag}", size)
        up = who.raw_bytes("POST", f"apps/{self.app}/blobs", data)
        self.blobs[tag] = (up["id"], hashlib.sha256(data).hexdigest(), size)

    def verify(self, w):
        alice, bob = w.account("alice"), w.account("bob")
        for tag, (blob, sha, size) in self.blobs.items():
            for node in w.members():  # whichever node is asked serves it
                data = alice.download(f"apps/{self.app}/blobs/{blob}", node=node)
                w.check(len(data) == size and hashlib.sha256(data).hexdigest() == sha,
                        f"blob {tag} through node {node}: {len(data)} bytes, sha256 {hashlib.sha256(data).hexdigest()}")
        attached = bob.view(self.app, "file", {"name": "large"})["result"]
        w.check(attached and self.blobs["large"][0] in json.dumps(attached), f"attached blob {attached}")

    def extend(self, w):
        self._upload(w.account("bob"), "new", 2 * MIB + 5)


@case
class FilesAndCompute(Case):
    """Sealed files (one of three segments) of a compute app from a signed package, and a
    compute run finished on the old release; on the new one a run reads the old files."""

    name = "files-and-compute"

    def seed(self, w):
        alice = w.account("alice")
        org = w.shared["org"]
        package, version = w.registry.publish(w.fixtures["compute_from"])
        alice.raw("post", "packages/install", {"package": package, "version": version})
        self.app = alice.raw("post", "apps", {"name": "compute", "package": package, "version": version,
                                              "org": org})["appId"]
        self.files = {}
        try:
            self._upload(w, alice, "segments", 40 * MIB + 123)  # three 16 MiB segments
        except NoUploadSessions:
            # Only for an experiment from a release older than any `.github/upgrade-from`
            # (1.0.0-rc.14): its largest file is one request.
            w.note("the old release has no upload sessions: its largest file, 16 MiB, instead")
            self._upload(w, alice, "segments", 16 * MIB)
        self._upload(w, alice, "single", 1 * MIB)
        self.runs = {}
        self._digest(w, "old", ["segments", "single"])

    def _upload(self, w, who, tag, size):
        data = payload(f"file-{tag}", size)
        up = who.upload_file(self.app, data)
        self.files[tag] = {"id": up["id"], "size": size, "sha256": up["sha256"]}

    def _digest(self, w, label, inputs):
        alice = w.account("alice")
        ids = [self.files[t]["id"] for t in inputs]
        run = alice.call(self.app, "startDigest", {"inputs": ids}, timeout=60)["result"]
        outcome = w.wait(f"compute run {label} ends", lambda: alice.view(self.app, "outcome", {"run": run})["result"],
                         240)
        outcome = json.loads(outcome)
        w.check(outcome["status"] == "done" and outcome["outputs"] == 1, f"run {label}: {outcome}")
        record = alice.http("GET", f"apps/{self.app}/runs/{run}")
        out = [o["id"] for o in (record.get("final") or {}).get("outputs", [])]
        w.check(len(out) == 1, f"run {label} record outputs {record}")
        info = alice.file_info(self.app, out[0])
        total = sum(self.files[t]["size"] for t in inputs)
        w.check(info.get("size") == total, f"run {label}'s output {info}, want {total} bytes")
        self.runs[label] = {"run": run, "outcome": outcome, "output": out[0], "size": total}

    def verify(self, w):
        alice = w.account("alice")
        for tag, f in self.files.items():
            got = alice.file_info(self.app, f["id"])
            w.check(got.get("size") == f["size"] and got.get("sha256", f["sha256"]) == f["sha256"],
                    f"file {tag}: {got}, want {f}")
        if w.phase != "mixed":
            listed = {f["id"] for f in alice.http("GET", f"apps/{self.app}/files")["files"]}
            missing = {t for t, f in self.files.items() if f["id"] not in listed}
            w.check(not missing, f"files not listed: {sorted(missing)}")
        for label, r in self.runs.items():
            outcome = json.loads(alice.view(self.app, "outcome", {"run": r["run"]})["result"] or "null")
            w.check(outcome == r["outcome"], f"run {label}'s recorded outcome {outcome}, want {r['outcome']}")
            record = alice.http("GET", f"apps/{self.app}/runs/{r['run']}")
            w.check(record.get("state") == "done", f"run {label} record {json.dumps(record)[:300]}")
            info = alice.file_info(self.app, r["output"])
            w.check(info.get("size") == r["size"], f"run {label}'s output {info}")

    def extend(self, w):
        # The old release's sealed files, read on the new release by a new job node.
        self._digest(w, "new-over-old", ["segments", "single"])
        self._upload(w, w.account("alice"), "new", 20 * MIB + 9)
        self._digest(w, "new", ["new", "segments"])
