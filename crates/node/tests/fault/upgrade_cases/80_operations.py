"""Deep cases: governance read paths (outbound approvals, the cluster's releases) and the
operator's state (quotas, per-organization overrides, pools, sign-up invites, counts, backups)."""

from __future__ import annotations

import hashlib
import json
import random
import time
from pathlib import Path

from upgrade_deep import (DeepCase, anyone, check, decision, eq, kv_wasm, new_org, ops, ops_text,
                          person, refused, same_decisions, same_read, scrub, signed_ops, snap, subset, until)
from upgrade_deep import not_covered
from rv2 import HttpError
from upgrade_path import case


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
    seeds = "an organization's outbound approval for a package and an account's own"
    verifies = ("each approval as its holder reads it (or the same refusal, where governance gives it to the "
                "operator); every release the cluster approved stays approved after the roll")

    def populate(self, w):
        oli = person(w, "oli")
        org = new_org(oli, "Outbound Org")
        s = {"org": org, "decisions": {
            "org outbound": decision(lambda: oli.raw("post", f"orgs/{org}/outbound", {
                "package": "com.calimero.upgrade.kv-a", "prefixes": ["https://api.example.com/"], "replace": True})),
            "own outbound": decision(lambda: oli.raw("post", "auth/outbound", {
                "package": "com.calimero.upgrade.kv-a", "prefixes": ["https://own.example.com/"], "replace": True})),
        }}
        s["me"] = scrub(oli.get("auth/me")).get("outboundApprovals")
        s["org_read"] = snap(oli, f"orgs/{org}")
        s["releases"] = _versions(anyone(w, "GET", "releases"))
        return s

    def check(self, w, s):
        oli = person(w, "oli")
        eq(oli.get("auth/me").get("outboundApprovals"), s["me"], "the account's outbound approvals")
        same_read(oli, f"orgs/{s['org']}", s["org_read"], "the organization's approvals")
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


def _backup(w, node):
    """How many files `POST /ops/backup` wrote on `node`. The harness's nodes share one backup directory, so
    a write racing another's fails with a 500 `No such file or directory`: only that is tried again."""
    deadline = time.time() + 20
    while True:
        try:
            out = ops(w, "POST", "ops/backup", {}, node=node)
            break
        except HttpError as e:
            if e.status != 500 or "No such file or directory" not in str(e.body) or time.time() > deadline:
                raise
            time.sleep(0.5)
    check(out is not None and isinstance(out.get("written"), int), f"a backup on node {node}: {out}")
    return out["written"]


@case
class OpsState(DeepCase):
    name = "ops-state"
    area = "Ops"
    seeds = "account quotas changed, a per-organization quota override, a compute pool"
    verifies = ("quotas, overrides, pools, the cluster's sign-up policy as before (its invites are `sign-up-invites`'); a quota changed "
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
        s["signup"] = anyone(w, "GET", "sign-up")

    def check(self, w, s):
        subset(ops(w, "GET", "ops/quotas"), s["quotas"], "quotas and overrides")
        subset(scrub(ops(w, "GET", "ops/pools"), {"nodes", "running"}), scrub(s["pools"], {"nodes", "running"}), "pools")
        subset(anyone(w, "GET", "sign-up"), s["signup"], "the sign-up policy")

    def exercise(self, w, s):
        w.fleet.operate("set-quotas", json.dumps({"maxApiKeysPerAccount": 48}))
        self._record(w, s)
        eq(s["quotas"].get("maxApiKeysPerAccount"), 48, "a quota changed on the new release")
        written = 0
        for n in w.members():
            written += _backup(w, n)
        check(written > 0, "no node wrote a backup")


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


DIGEST_COMPARE_SECS = 60  # `DIGEST_COMPARE_EVERY`: how often an app's leader compares its replicas' digests
DIVERGED = "calimero_group_diverged{"  # a series present only while a replica's digest differs
DIVERGENCES = "calimero_group_divergences_total"
COMPARISONS = "calimero_group_digest_comparisons_total{"  # a release before this one does not count them
COMPARISONS_HELP = "# HELP calimero_group_digest_comparisons_total"


def _divergences(w):
    """`{node: times it found a replica diverge}`, failing while any node shows a differing replica now."""
    found = {}
    for node in w.members():
        text = ops_text(w, "ops/metrics", node)
        lines = text.splitlines()
        check(not [x for x in lines if x.startswith(DIVERGED)], f"node {node} reports a diverged replica: "
              f"{[x for x in lines if x.startswith(DIVERGED)]}")
        counts = [x.split()[-1] for x in lines if x.startswith(DIVERGENCES + " ")]
        check(len(counts) == 1, f"node {node}: {len(counts)} {DIVERGENCES} samples")
        found[node] = int(counts[0])
    return found


def _comparisons(w, apps):
    """`{app: comparisons that could tell, over every node}` (the leader of a group counts them), or `None`
    while no node is on a release that counts them."""
    texts = [ops_text(w, "ops/metrics", node) for node in w.members()]
    if not any(COMPARISONS_HELP in t for t in texts):
        return None
    groups = {a["app"]: a["group"] for a in w.fleet.placement()["apps"]}
    total = {a: 0 for a in apps}
    for text in texts:
        for line in text.splitlines():
            if line.startswith(COMPARISONS):
                for app in apps:
                    if line.startswith(f'{COMPARISONS}group="{groups[app]}"}}'):
                        total[app] += int(line.split()[-1])
    return total


@case
class StateDigests(DeepCase):
    name = "state-digests"
    area = "Ops"
    since = "1.0.0-rc.31"
    seeds = "two apps with data written, replicated on the old nodes (each leader compares its replicas' state digests)"
    verifies = ("on every node no replica is reported diverged and none ever was (`calimero_group_diverged`, "
                "`calimero_group_divergences_total`), in the mixed cluster, after the roll and after a restart; "
                "once every node runs the new release each app's leader has compared its replicas "
                "(`calimero_group_digest_comparisons_total` above zero for both groups) and compares again after "
                "new writes and after a restart, which tells a comparison that agreed from one that never ran")

    def populate(self, w):
        dax = person(w, "dax")
        s = {"apps": [dax.deploy(kv_wasm(w), f"digest-{i}") for i in range(2)]}
        for a in s["apps"]:
            for i in range(20):
                dax.call(a, "set", {"key": f"k{i:02}", "value": f"v{i}"})
        eq(_divergences(w), {n: 0 for n in w.members()}, "divergences while seeding")
        return s

    def during_mix(self, w, s):
        eq(_divergences(w), {n: 0 for n in w.members()}, "divergences while the cluster is mixed")

    def check(self, w, s):
        self.during_mix(w, s)

        def compared():
            seen = _comparisons(w, s["apps"])
            check(seen is None or all(seen.values()), f"no digest comparison has run for every app yet: {seen}")

        until("each app's leader has compared its replicas", compared, DIGEST_COMPARE_SECS + 45)

    def exercise(self, w, s):
        dax = person(w, "dax")
        before = _comparisons(w, s["apps"])
        for a in s["apps"]:
            for i in range(20):
                dax.call(a, "set", {"key": f"new{i:02}", "value": f"n{i}"})
        time.sleep(DIGEST_COMPARE_SECS + 15)
        after = _comparisons(w, s["apps"])
        check(all(after[a] > before[a] for a in s["apps"]), f"a leader did not compare again: {before} then {after}")
        self.check(w, s)


INVITE_DAYS = 30


def _invite(w, tag):
    """A cluster sign-up invite the operator issues: `(token, its key)`."""
    token = f"deep-signup-{tag}-{'x' * 16}"
    expires = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + INVITE_DAYS * 86400))
    made = signed_ops(w, "POST", "ops/sign-up/invites", {"token": token, "uses": 1, "expiresAt": expires})
    return token, made["inviteKey"]


SIGN_UP_WANT = {"live": "ok", "revoked": "SIGN_UP_REFUSED", "unknown": "SIGN_UP_REFUSED"}  # what each kind of token gets
UNKNOWN_INVITE = "deep-signup-unknown-" + "y" * 16


@case
class SignUpInvites(DeepCase):
    name = "sign-up-invites"
    area = "Accounts"
    since = "1.0.0-rc.18"
    seeds = "two cluster sign-up invites issued by the operator (a token each), one of them revoked"
    verifies = ("the invite list as the operator reads it; on every node, new ones included, a sign-up beginning "
                "with a live token is taken and one with a revoked or an unknown token is refused `SIGN_UP_REFUSED`, "
                "for the tokens the old release issued and revoked as for the new release's; on the new release "
                "one more invite is issued and another issued and revoked, the old live one left live")

    def _begin(self, w, token, node):
        return decision(lambda: anyone(w, "POST", "auth/passkey/register/begin", {"name": "deep", "invite": token},
                                       node=node))

    def _judge(self, w, s):
        for node in w.members():
            for kind, tokens in (("live", s["live"]), ("revoked", s["revoked"]), ("unknown", [UNKNOWN_INVITE])):
                for token in tokens:
                    eq(self._begin(w, token, node), SIGN_UP_WANT[kind], f"a {kind} token's sign-up on node {node}")

    def populate(self, w):
        s = {"live": [], "revoked": [], "keys": {}}
        for tag, kind in (("live", "live"), ("revoked", "revoked")):
            token, key = _invite(w, tag)
            s[kind].append(token)
            s["keys"][token] = key
        signed_ops(w, "POST", f"ops/sign-up/invites/{s['keys'][s['revoked'][0]]}/revoke", {})
        self._record(w, s)
        keys = json.dumps(s["invites"])
        check(s["keys"][s["live"][0]] in keys and s["keys"][s["revoked"][0]] not in keys, f"the invite list: {s['invites']}")
        return s

    def _record(self, w, s):
        s["invites"] = ops(w, "GET", "ops/sign-up/invites")
        self._judge(w, s)

    def check(self, w, s):
        subset(ops(w, "GET", "ops/sign-up/invites"), s["invites"], "the cluster sign-up invites")
        self._judge(w, s)

    def exercise(self, w, s):
        # The old release's live and revoked tokens stay in the table, so a restart still judges them.
        new_live, key = _invite(w, "new-live")
        s["live"].append(new_live)
        s["keys"][new_live] = key
        new_revoked, key = _invite(w, "new-revoked")
        s["keys"][new_revoked] = key
        signed_ops(w, "POST", f"ops/sign-up/invites/{key}/revoke", {})
        s["revoked"].append(new_revoked)
        self._record(w, s)
        listed = json.dumps(s["invites"])
        check(s["keys"][new_live] in listed and key not in listed and s["keys"][s["live"][0]] in listed,
              f"the invite list after the roll: {s['invites']}")


@case
class RestoreBackup(DeepCase):
    name = "restore-backup"
    area = "Ops"
    since = "1.0.0-rc.18"
    seeds = ("an app with a key and a sealed file, backed up on every node (`POST /ops/backup`), then given another "
             "key and another file: the second ones are not in the backup")
    verifies = ("the app answers with both keys as before; on the new release the operator restores the old "
                "release's backup by name (`ops restore --backup`), not a newer one, and the restore names that "
                "backup: the app holds the first key and the first file (size and SHA-256) and neither of the "
                "second, and takes new writes")

    def _keys(self, w, s):
        rowan = person(w, "rowan")
        return {k: rowan.result(s["app"], "get", {"key": k}) for k in s["want"]}

    def _newest_backup(self, w, app):
        """The newest backup file of `app` in the harness's backup directory (`<dir>/<app>/<name>`)."""
        files = [f.name for f in (Path(w.fleet.workdir) / "backups" / app).iterdir()
                 if not f.name.startswith(".") and not f.name.endswith(".manifest")]
        check(files, f"no backup of {app} was written")
        return max(files)

    def populate(self, w):
        rowan = person(w, "rowan")
        app = rowan.deploy(kv_wasm(w), "restore-me")
        rowan.call(app, "set", {"key": "kept", "value": "before the backup"})
        kept = random.Random(21).randbytes(70_000)
        kept_id = rowan.a.upload_file(app, kept)["id"]
        written = sum(_backup(w, node) for node in w.members())
        check(written > 0, "no node wrote a backup")
        rowan.call(app, "set", {"key": "lost", "value": "after the backup"})
        lost_id = rowan.a.upload_file(app, random.Random(22).randbytes(70_000))["id"]
        s = {"app": app, "want": {"kept": "before the backup", "lost": "after the backup"},
             "kept": {"id": kept_id, "size": len(kept), "sha256": hashlib.sha256(kept).hexdigest()}, "lost": lost_id}
        s["backup"] = self._newest_backup(w, app)
        eq(self._keys(w, s), s["want"], "the app's keys before a restore")
        return s

    def _file(self, w, s):
        info = person(w, "rowan").http("GET", f"apps/{s['app']}/files/{s['kept']['id']}")
        eq((int(info["size"]), info["sha256"]), (s["kept"]["size"], s["kept"]["sha256"]), "the first file")

    def check(self, w, s):
        eq(self._keys(w, s), s["want"], "the app's keys")
        until("the first file reads back", lambda: self._file(w, s), 30)
        if not s["want"]["lost"]:
            refused("NOT_FOUND", lambda: person(w, "rowan").http("GET", f"apps/{s['app']}/files/{s['lost']}"),
                    "the file written after the backup")

    def exercise(self, w, s):
        out = w.fleet.operate("restore", s["app"], "--backup", s["backup"])
        eq((out.get("backup") or {}).get("name"), s["backup"], "the backup the restore names")
        s["want"] = {"kept": "before the backup", "lost": None}
        got = until("the restored app answers", lambda: self._keys(w, s), 60)
        eq(got, s["want"], "the restored app's keys: the write after the backup is gone")
        person(w, "rowan").call(s["app"], "set", {"key": "again", "value": "after the restore"})
        s["want"]["again"] = "after the restore"
        self.check(w, s)


def _job_nodes(w):
    """The ids of the job nodes the directory holds (`GET /ops/placement`)."""
    return sorted(n["id"] for n in ops(w, "GET", "ops/placement")["nodes"] if str(n.get("role")).lower() == "job")


def _attested(w, states=("admitted",)):
    """`{job node: its attestation answer}` as the node's check reports it: each in one of `states`."""
    got = {}
    for job in _job_nodes(w):
        got[job] = until(f"job node {job} is attested", lambda job=job: _in_states(
            ops(w, "GET", f"ops/job-nodes/{job}/attestation"), job, states), 30)
    check(got, "no job node is registered")
    return got


def _in_states(answer, job, states):
    check(answer.get("state") in states and answer.get("node") == job, f"job node {job}: {answer}")
    return answer


@case
class JobNodeAttestation(DeepCase):
    name = "job-node-attestation"
    area = "Ops"
    since = "1.0.0-rc.25"
    seeds = "the job node the cluster registered (compute runs use it); nothing is stored beyond its registration"
    verifies = ("the operator's read of each registered job node's attestation answers `admitted`: the old "
                "release's nodes judging the old job node, then every new node judging that old job node once it "
                "serves again after the roll (and a compute run started then is placed on it and ends), then the new nodes "
                "judging the new one; while the cluster is mixed the "
                "old job node, which the pipelines case stopped, may be `unreachable`; a replica's id is refused `404`")

    def populate(self, w):
        _attested(w)
        return {}

    def check(self, w, s, states=("admitted",)):
        _attested(w, states)
        replica = w.members()[0]
        refused(("NOT_FOUND", "404"), lambda: ops(w, "GET", f"ops/job-nodes/{replica}/attestation"),
                "the attestation of a replica node")

    def during_mix(self, w, s):
        # The pipelines case stops the old job node before the roll, so it is still registered and unreachable.
        self.check(w, s, ("admitted", "unreachable"))

    def with_old_job(self, w, s):
        for job in sorted(w.fleet.jobs):
            for node in w.members():
                until(f"node {node} admits the old job node {job}", lambda: _in_states(
                    ops(w, "GET", f"ops/job-nodes/{job}/attestation", node=node), job, ("admitted",)), 60)
        self._run_places_on(w, sorted(w.fleet.jobs)[0])

    def _run_places_on(self, w, job):
        """A compute run started now, with the old job node the only one registered, is placed on it and ends."""
        jory = person(w, "jory")
        package, version = w.registry.publish(w.fixtures["compute_from"])
        jory.raw("post", "packages/install", {"package": package, "version": version})
        org = new_org(jory, "Old Job Org")
        app = jory.raw("post", "apps", {"name": "old-job", "package": package, "version": version, "org": org})["appId"]
        file_id = jory.a.upload_file(app, random.Random(31).randbytes(50_000))["id"]
        run = jory.call(app, "startDigest", {"inputs": [file_id]}, timeout=60)["result"]

        def placed():
            nodes = [a["node"] for a in jory.get(f"apps/{app}/runs/{run}")["attempts"]]
            check(nodes, "the run is not placed yet")
            return nodes

        eq(until("the run is placed", placed, 120)[0], job, "the node the run was placed on")

        def ended():
            outcome = jory.result(app, "outcome", {"run": run})
            check(outcome, "the run has no outcome yet")
            return json.loads(outcome)

        eq(until("the run ends", ended, 240)["status"], "done", "the run on the old job node")


# What no case in this file seeds yet, and why (the matrix lists it as not covered).

not_covered("Governance", "The egress switch (`POST /orgs/{org}/egress`, `POST /apps/{app}/egress`; `Org::egress_off`, "
            "`AppRecord::egress_off`, `SetEgress`)",
            "it ships as a hard cutover: `SetEgress` is ungated and a node of an earlier release cannot decode it, so a "
            "cluster is recreated rather than rolled, and no release before it has the switch to seed. "
            "`tests/it/egress.rs` covers the switch, its refusals and the consent page on one build")

not_covered("Governance", "A release deadline that passes while a cluster rolls (`SystemState::retired`: the old nodes are "
            "placed no new app mid-roll)",
            "a deadline is judged at the command's own time, so the granted run sets a passed one on the old nodes' own "
            "release, which retires no node; one on the new release would stop placement on the old nodes before the roll "
            "and needs a deadline timed to the rollout. `a_release_deadline_stops_new_work_but_not_running_apps` "
            "(`cluster.rs`) covers retired nodes on one build")

not_covered("Operations", "Peers' quotes queued apart from anonymous `/attestation` requests (`attest::Lane::Peer`, "
            "`x-calimero-peer-attest`)",
            "it stores nothing and changes no answer: a node of an earlier release sends no header and is queued with "
            "anyone, as before, so a roll has no data to carry. `a_flood_of_attestations_does_not_keep_peers_from_attesting` "
            "(`cluster.rs`) and `a_peer_passes_a_full_anonymous_queue` (`attest.rs`) cover it on one build")

not_covered("Operations", "Interval quotes (`GET /attestation?form=interval`, `attest::IntervalQuotes`, `IntervalProof`)",
            "it stores nothing: the interval key and its quote live in memory and are made again at every start, so a "
            "roll has no data to carry. While a cluster rolls, the new nodes ask the old ones for the form, which they "
            "ignore and answer with a quote over the nonce as before, and the old nodes ask the new ones for nothing "
            "new, so every roll in the upgrade path attests across both releases. "
            "`new_clients_attest_and_sign_in_while_the_quote_queue_is_full` (`cluster.rs`) and "
            "`an_interval_answer_verifies_for_its_own_request_only` (`attest.rs`) cover it on one build")
