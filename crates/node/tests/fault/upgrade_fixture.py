"""Writes the replay fixture of a release: one node's data directory as that release's merod
left it, and the answers it gave, for `crates/node/tests/upgrade_replay.rs`.

    scripts/fetch-release.sh 1.0.0-rc.15 target/upgrade-from
    python3 -I crates/node/tests/fault/upgrade_fixture.py --from-dir target/upgrade-from \\
        --out crates/node/tests/fixtures/upgrade/1.0.0-rc.15

Run it once per release, with that release's merod and meroctl (`--from-dir`,
laid out by `scripts/fetch-release.sh`), and commit the directory it writes:

- `datadir.tar.gz`: `node1/`, the data directory of a one-node cluster
  (`merod run --dev`, cluster `upgrade-replay`, so its storage key is the dev
  key of that name) after the data set below was written through the public API
  and the node stopped. The data set stays under the log's snapshot interval
  (1000 entries), so the directory holds the whole system log and no system
  snapshot: a node opening it rebuilds its system state by applying every entry.
- `expected.json`: the release, the node's port and the API keys the test
  sends, and every request below with the status and body the release
  answered (`reads`), read just before the node stopped.

The replay test opens the directory on this tree's node and sends the same
requests: each answer must hold every field the old one held, with the same
value, a refusal the same code (a new release may add fields). Then it writes.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--from-dir", required=True, help="the release's merod, meroctl, VERSION, kv-store-test-fixture.mpk")
    ap.add_argument("--out", required=True)
    ap.add_argument("--port", type=int, default=7790)
    args = ap.parse_args(argv)
    frm = Path(args.from_dir).resolve()
    os.environ["RV2_MEROD"] = str(frm / "merod")
    os.environ["RV2_MEROCTL"] = str(frm / "meroctl")
    sys.path.insert(0, str(HERE))
    import rv2
    import suite
    from rv2 import Cluster, HttpError

    version = (frm / "VERSION").read_text().strip()
    out = Path(args.out)
    work = Path(tempfile.mkdtemp(prefix="upgrade-fixture-"))
    kv = work / "kv.wasm"
    with tarfile.open(frm / "kv-store-test-fixture.mpk") as tar:
        member = next(m for m in tar.getmembers() if m.name.lstrip("./") == "app.wasm")
        kv.write_bytes(tar.extractfile(member).read())

    cl = Cluster(work, base_port=args.port - 1, n=1, cluster="upgrade-replay")
    cl.start()
    try:
        alice, bob, carol = (suite.Account(cl, n, i).sign_up() for i, n in enumerate(("alice", "bob", "carol")))
        pk = str(alice.passkey)
        # Accounts: a second passkey, labelled; a recovery code; scoped keys.
        alice.cli("account", "device", "add", "--dev-passkey", str(alice.home / "second.json"), "--approve-with", pk)
        me = alice.raw("get", "auth/me")
        second = me["passkeys"][1]["id"]
        alice.raw("post", f"auth/passkeys/{second}", {"label": "laptop"})
        alice.raw("post", "auth/recovery", {})
        # An organization: metadata, a member, a removed member, a team, an open invite.
        org = alice.raw("post", "orgs", {"name": "Replay Org"})["orgId"]
        alice.raw("post", f"orgs/{org}/metadata", {"metadata": {"plan": "replay"}})
        bob.raw("post", f"invites/{alice.raw('post', f'orgs/{org}/invites', {'role': 'member', 'teams': []})['token']}/redeem", {})
        carol.raw("post", f"invites/{alice.raw('post', f'orgs/{org}/invites', {'role': 'member', 'teams': []})['token']}/redeem", {})
        alice.raw("delete", f"orgs/{org}/members/{carol.id}")
        team = alice.raw("post", f"orgs/{org}/teams", {"name": "core", "join": "invite"})
        team = team.get("teamId") or team.get("id")
        alice.raw("post", f"orgs/{org}/teams/{team}/members", {"account": bob.id})
        alice.raw("post", f"orgs/{org}/invites", {"role": "member", "teams": [team], "uses": 5})
        # Apps: an organization app with history and notes, a team app, an owner app.
        app = alice.deploy(kv, "replay-kv", org=org)
        for i in range(20):
            (alice if i % 2 else bob).call(app, "set", {"key": f"k{i % 12}", "value": f"v{i}"})
        alice.call(app, "remove", {"key": "k3"})
        alice.call(app, "note", {"id": "n1", "title": "Replay", "body": "the old log, applied again"})
        alice.raw("post", f"apps/{app}/metadata", {"metadata": {"color": "teal"}})
        # One upload of the bundle (its bytes are in the log once): the other apps name it.
        bundle = alice.raw("get", f"apps/{app}")["bundle"]
        team_app = alice.cli("context", "create", "--name", "replay-team", "--bundle", bundle,
                             "--namespace", org)["appId"]
        alice.raw("post", f"apps/{team_app}/access", {"access": {"kind": "team", "team": team}})
        bob.call(team_app, "set", {"key": "t", "value": "team"})
        own = alice.cli("context", "create", "--name", "replay-own", "--bundle", bundle)["appId"]
        alice.call(own, "set", {"key": "o", "value": "own"})
        ro = alice.cli("account", "api-key", "create", "--label", "ro", "--read-only", "--context", app,
                       "--dev-passkey", pk)["key"]
        blob = work / "blob.bin"
        blob.write_bytes(bytes(range(256)) * 40)
        up = alice.cli("blob", "upload", "--context", app, str(blob))
        blob_id = up.get("blobId") or up.get("id")
        alice.call(app, "attach", {"name": "doc", "blob": blob_id})

        keys = {"alice": alice.key, "bob": bob.key, "carol": carol.key, "ro": ro}
        view = lambda m, a=None: {"method": m, "args": a or {}}  # noqa: E731
        requests = [
            ("alice", "GET", "auth/me", None),
            ("bob", "GET", "auth/me", None),
            ("alice", "GET", f"orgs/{org}", None),
            ("bob", "GET", f"orgs/{org}", None),
            ("carol", "GET", f"orgs/{org}", None),
            ("alice", "GET", f"orgs/{org}/teams/{team}", None),
            ("alice", "GET", f"orgs/{org}/invites", None),
            ("alice", "GET", f"apps/{app}", None),
            ("alice", "GET", f"apps/{app}/storage", None),
            ("alice", "GET", f"apps/{app}/abi", None),
            ("alice", "GET", f"apps/{app}/blobs/{blob_id}/info", None),
            ("alice", "GET", "auth/usage", None),
            ("alice", "POST", f"apps/{app}/views", view("entries")),
            ("bob", "POST", f"apps/{app}/views", view("len")),
            ("bob", "POST", f"apps/{app}/views", view("findNotes", {"text": "replay"})),
            ("alice", "POST", f"apps/{app}/views", view("file", {"name": "doc"})),
            ("ro", "POST", f"apps/{app}/views", view("get", {"key": "k1"})),
            ("ro", "POST", f"apps/{app}/intents", view("set", {"key": "x", "value": "y"})),
            ("carol", "POST", f"apps/{app}/views", view("len")),
            ("bob", "POST", f"apps/{team_app}/views", view("get", {"key": "t"})),
            ("bob", "POST", f"apps/{own}/views", view("get", {"key": "o"})),
            ("alice", "POST", f"apps/{own}/views", view("get", {"key": "o"})),
            ("ops", "GET", "ops/quotas", None),
            ("anon", "GET", "sign-up", None),
        ]
        reads = []
        for who, method, path, body in requests:
            bearer = cl.ops_token if who == "ops" else keys.get(who)
            headers = {"authorization": f"Bearer {bearer}"} if bearer else {}
            try:
                answer, status = rv2.http(method, cl.url(1) + path, body, headers), 200
            except HttpError as e:
                answer, status = e.body, e.status
            reads.append({"as": who, "method": method, "path": path, "body": body, "status": status, "answer": answer})
        health = rv2.http("GET", cl.url(1) + "health")
        status = cl.ops_status(1)
    finally:
        cl.stop()

    snaps = cl.dir(1) / "snapshots"
    held = [p.name for p in snaps.iterdir()] if snaps.exists() else []
    out.mkdir(parents=True, exist_ok=True)
    # Left out: what the node derives again (its compiled components) and RocksDB's
    # informational logs.
    def keep(info):
        name = info.name.split("/", 1)[1] if "/" in info.name else ""
        if name.startswith("components") or name.startswith("store/LOG"):
            return None
        return info

    with tarfile.open(out / "datadir.tar.gz", "w:gz") as tar:
        tar.add(cl.dir(1), arcname="node1", filter=keep)
    (out / "expected.json").write_text(json.dumps({
        "release": version,
        "merod": subprocess.run([str(frm / "merod"), "--version"], capture_output=True, text=True).stdout.strip(),
        "nodeRelease": health.get("release"),
        "cluster": "upgrade-replay",
        "port": args.port,
        "systemApplied": status["system"],
        "snapshotsHeld": held,
        "keys": keys,
        "apps": {"org": app, "team": team_app, "own": own},
        "reads": reads,
    }, indent=1, sort_keys=True) + "\n")
    shutil.rmtree(work, ignore_errors=True)
    print(f"upgrade fixture of {version}: {out} ({(out / 'datadir.tar.gz').stat().st_size} bytes, "
          f"{len(reads)} reads, snapshots held: {held or 'none'})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
