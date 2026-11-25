"""Deep cases: a cluster founded again from its system backup (`merod run --bootstrap --restore-from`)."""

from __future__ import annotations

import subprocess
import time

from rollout import OPERATOR_PUBLIC
from rv2 import ORIGIN
from upgrade_deep import DeepCase, check, eq, kv_wasm, new_org, ops, people, scratch, until
from rollout import system_ready
from upgrade_path import case

PORT_OFFSET = 30  # beside the fleet's node ports: the founded cluster is a node of its own


@case
class Refound(DeepCase):
    name = "system-refound"
    area = "Ops"
    seeds = "an organization and an app with data, written on the old release"
    verifies = ("the organization as its owner reads it on every node; on the new release the nodes write their "
                "system backups (`POST /ops/backup`), stop (founding refuses while a node of the cluster answers), "
                "and a new `merod run --bootstrap --restore-from` of that directory, on an empty data directory, "
                "founds a cluster again from it: its owner signs in there with the same passkey and reads the "
                "organization, written on the old release, by name; the nodes then start again on their data")

    def populate(self, w):
        rue = people(w, "rue")[0]
        org = new_org(rue, "Refound Org")
        app = rue.deploy(kv_wasm(w), "refounded", org=org)
        rue.call(app, "set", {"key": "k", "value": "v"})
        return {"org": org, "name": "Refound Org", "app": app}

    def check(self, w, s):
        rue = people(w, "rue")[0]
        for n in w.members():
            eq(rue.get(f"orgs/{s['org']}", node=n)["name"], s["name"], f"the organization on node {n}")

    def _backup_everywhere(self, w):
        for n in w.members():
            # Nodes share one backup directory, so two writers can race on it.
            until(f"a backup on node {n}", lambda n=n: check("written" in ops(w, "POST", "ops/backup", {}, node=n),
                                                              "no backup written"), 30)

    def exercise(self, w, s):
        rue = people(w, "rue")[0]
        self._backup_everywhere(w)
        node = w.members()[0]
        stopped = [n for n in w.fleet.running()]
        for n in stopped:
            w.fleet.kill(n)
        node_id = PORT_OFFSET
        here = scratch(w, "refound")
        port = w.fleet.base_port + node_id
        args = [w.fleet.binaries[node], "run", "--node-id", "1", "--listen", f"127.0.0.1:{port}",
                "--public-url", w.fleet.url(node_id), "--data-dir", str(here / "node1"),
                "--peers", f"1={w.fleet.url(node_id)}", "--cluster", w.fleet.cluster, "--dev", "--origins", ORIGIN,
                "--ops-token", w.fleet.ops_token, "--operator-key", OPERATOR_PUBLIC, "--release-approvals", "operator",
                "--tee-release", w.fleet.release[node], "--apps-domain", "localhost", "--bootstrap",
                "--restore-from", str(w.fleet.workdir / "backups"), "--backup-dir", str(here / "backups")]
        log = open(here / "node.log", "ab")
        proc = subprocess.Popen(args, stdout=log, stderr=log, start_new_session=True)
        try:
            def serving():
                check(proc.poll() is None, f"the founding node exited {proc.returncode}: see {here / 'node.log'}")
                eq((w.fleet.health(node_id) or {}).get("systemLeader"), 1, "the founded cluster's system leader")

            until("the founded cluster serves", serving, 60)
            eq(rue.get("auth/me", node=node_id)["accountId"], rue.id, "the account that signs in on the founded cluster")
            eq(rue.get(f"orgs/{s['org']}", node=node_id)["name"], s["name"], "the organization on the founded cluster")
            apps = ops(w, "GET", "ops/apps", node=node_id)["apps"]
            check(any(a["app"] == s["app"] for a in apps), f"the app is not listed on the founded cluster: {apps[:3]}")
        finally:
            proc.kill()
            proc.wait()
            log.close()
            for n in stopped:
                w.fleet.start_node(n)
            w.wait("the system group has a leader and its voters again", lambda: system_ready(w.fleet, len(w.members())), 90)
