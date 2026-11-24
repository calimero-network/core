"""Writes the replay fixture of a release: one node's data directory as that release's merod
left it, and the answers it gave, for `crates/node/tests/upgrade_replay.rs`.

    scripts/fetch-release.sh 1.0.0-rc.35 target/upgrade-from
    scripts/build-release-fixtures.sh 1.0.0-rc.35 target/upgrade-from
    scripts/build-upgrade-deep-fixtures.sh target/upgrade-from
    python3 -I crates/node/tests/fault/upgrade_fixture.py --from-dir target/upgrade-from \\
        --out target/upgrade-replay/1.0.0-rc.35
    UPGRADE_REPLAY_DIR=target/upgrade-replay cargo test -p calimero-node --test upgrade_replay

Run it with the pinned release's merod and meroctl (`--from-dir`, laid out by
`scripts/fetch-release.sh`); `scripts/upgrade-path.sh` does so on every `Upgrade path` run,
so what it writes is not committed. Each fixture it writes (`<out>` and `<out>-cases`) holds:

- `datadir.tar.zst`: `node1/`, the data directory of a one-node cluster
  (`merod run --dev`, cluster `upgrade-replay`, so its storage key is the dev
  key of that name) after the data set below, and then every upgrade case's seed
  (`upgrade_cases/`), was written through the public API and the node stopped.
  The data set stays under the log's snapshot interval (1000 entries), so the
  directory holds the whole system log and no system snapshot: a node opening it
  rebuilds its system state by applying every entry, and the log holds an entry
  of every kind a release writes (`upgrade_replay.rs` checks it).
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
SNAPSHOT_EVERY = 1000  # applied entries after which a group's log takes a snapshot (`calimero_log::Config`)
# Seeds the recording leaves out, with why: it runs in CI on one node and stays small.
SKIPPED = {
    "files-and-compute": "it uploads a 40 MiB file",
    "sealed-files-sessions": "it uploads two files of 32 MiB and one of 2 MiB",
    "replica-sets": "it moves an app among three nodes",
}


def one_node_fleet(Fleet):
    """A `Fleet` whose node writes no periodic backup: a backup snapshots the system group, which
    purges the log before it, and the fixture keeps the whole log."""

    class OneNode(Fleet):
        def args(self, i):
            a = super().args(i)
            if "--backup-dir" in a:
                at = a.index("--backup-dir")
                del a[at : at + 2]
            return a

    return OneNode


def pack_pipelines(frm: Path, work: Path) -> Path:
    """The pipeline cases' package, packed by the release's own meroctl from the compute
    component and pipeline it built from its tag (`scripts/build-release-fixtures.sh`)."""
    src = frm / "src" / "apps"
    fanout = src / "fixture-compute" / "pipelines" / "fanout.yaml"
    if not fanout.exists():
        fanout = src / "v2-fixtures" / "compute" / "pipelines" / "fanout.yaml"
    mpk = work / "pipelines.mpk"
    subprocess.run([str(frm / "meroctl"), "app", "pack", "--compute", str(frm / "compute-compute.wasm"),
                    "--pipeline", str(fanout), "--package", "com.calimero.upgrade.pipelines",
                    "--app-version", "1.0.0", "--out", str(mpk)], check=True, capture_output=True)
    return mpk


def seed_cases(w) -> None:
    """Every upgrade case's seed, as the upgrade path writes it on the old release: the log then
    holds the entries those cases' features write. A case that fails to seed stops the recording."""
    import upgrade_path  # imported once main has set the release's binaries (RV2_MEROD)

    upgrade_path.load_cases(HERE / "upgrade_cases")
    cases = [c() for c in upgrade_path.CASES if c.name not in SKIPPED]
    results: list[dict] = []
    upgrade_path.run_step(w, cases, "seed", results)
    failed = [r["case"] for r in results if r["status"] != "pass"]
    failed += [c.name for c in cases if getattr(c, "seed_error", None)]
    if failed:
        raise SystemExit(f"these cases did not seed on the one-node cluster: {sorted(set(failed))}")


def deep_reads(w, keys: dict) -> list:
    """What the seeded cases' accounts and the operator list: each account's own reads, each
    organization it is in, and the operator's reports of the cluster."""
    reads = [("anon", "GET", "releases", None), ("ops", "GET", "ops/apps", None), ("ops", "GET", "ops/orgs", None),
             ("ops", "GET", "ops/packages", None), ("ops", "GET", "ops/pools", None),
             ("ops", "GET", "ops/prices", None), ("ops", "GET", "ops/sign-up/invites", None)]
    for name, account in w._accounts.items():
        who = f"p:{name}"
        keys[who] = account.key
        mine = ["auth/me", "auth/apps", "auth/aliases", "apps", "packages", "deployments"]
        reads += [(who, "GET", path, None) for path in mine]
        me = account.http("GET", "auth/me", node=1)
        for org in me.get("orgs", []):
            reads += [(who, "GET", f"orgs/{org['orgId']}", None), (who, "GET", f"orgs/{org['orgId']}/apps", None)]
    return reads


def record(frm: Path, out: Path, port: int, part: str) -> None:
    """One fixture: the `base` part is the small data set and the kinds no seed writes, the `cases` part
    every upgrade case's seed. Two clusters, since a log holds 1000 entries before it takes a snapshot."""
    import rv2
    import upgrade_kinds
    import upgrade_path
    from rv2 import HttpError, PackageRegistry
    from rollout import Fleet

    version = (frm / "VERSION").read_text().strip()
    work = Path(tempfile.mkdtemp(prefix="upgrade-fixture-"))
    kv = work / "kv.wasm"
    with tarfile.open(frm / "kv-store-test-fixture.mpk") as tar:
        member = next(m for m in tar.getmembers() if m.name.lstrip("./") == "app.wasm")
        kv.write_bytes(tar.extractfile(member).read())
    pipelines = pack_pipelines(frm, work)

    registry = PackageRegistry(work / "registry")
    cl = one_node_fleet(Fleet)(work, base_port=port - 1, cluster="upgrade-replay", binary=str(frm / "merod"),
               meroctl=str(frm / "meroctl"), release=upgrade_path.FROM_RELEASE, registry=registry, n=1)
    fixtures = {"from_version": version, "kv_from": frm / "kv-store-test-fixture.mpk",
                "compute_from": frm / "compute.mpk", "pipelines": pipelines}
    w = upgrade_path.World(cl, registry, fixtures, upgrade_path.say)
    cl.start()
    try:
        cl.start_job(upgrade_path.OLD_JOB, str(frm / "merod"), upgrade_path.FROM_RELEASE)
        keys, requests, apps = {}, [], {}
        if part == "base":
            alice, bob, carol = (upgrade_path.Account(cl, n, i).sign_up() for i, n in enumerate(("ada", "ben", "cai")))
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
            team_app = alice.cli("instance", "create", "--name", "replay-team", "--bundle", bundle,
                                 "--org", org)["appId"]
            alice.raw("post", f"apps/{team_app}/access", {"grants": [{"kind": "team", "team": team}]})
            bob.call(team_app, "set", {"key": "t", "value": "team"})
            own = alice.cli("instance", "create", "--name", "replay-own", "--bundle", bundle)["appId"]
            alice.call(own, "set", {"key": "o", "value": "own"})
            ro = alice.cli("account", "api-key", "create", "--label", "ro", "--read-only", "--instance", app,
                           "--dev-passkey", pk)["key"]
            blob = work / "blob.bin"
            blob.write_bytes(bytes(range(256)) * 40)
            up = alice.cli("blob", "upload", "--instance", app, str(blob))
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
            apps = {"org": app, "team": team_app, "own": own}
            upgrade_kinds.record_every_kind(w, upgrade_path.OLD_JOB)
        else:
            # An account and an app the replay test writes to once it has replayed the log.
            wes = upgrade_path.Account(cl, "wes", 0).sign_up()
            keys, apps = {"alice": wes.key}, {"org": wes.deploy(kv, "replay-kv")}
            seed_cases(w)
            cl.retire_job(upgrade_path.OLD_JOB)  # the replaying node has no job node to count
        reads = []
        for who, method, path, body in requests + deep_reads(w, keys):
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
        registry.stop()

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

    # zstd: fast, and far smaller than gzip on the seeded packages, which repeat each other's bytes.
    plain = work / "datadir.tar"
    with tarfile.open(plain, "w") as tar:
        tar.add(cl.dir(1), arcname="node1", filter=keep)
    subprocess.run(["zstd", "-3", "-q", "-f", str(plain), "-o", str(out / "datadir.tar.zst")], check=True)
    (out / "expected.json").write_text(json.dumps({
        "release": version,
        "merod": subprocess.run([str(frm / "merod"), "--version"], capture_output=True, text=True).stdout.strip(),
        "nodeRelease": health.get("release"),
        "cluster": "upgrade-replay",
        "port": port,
        "systemApplied": status["system"],
        "snapshotsHeld": held,
        "everyKind": True,
        "part": part,
        "node": {"teeRelease": cl.release[1], "appsDomain": "localhost"},
        "keys": keys,
        "apps": apps,
        "reads": reads,
    }, indent=1, sort_keys=True) + "\n")
    shutil.rmtree(work, ignore_errors=True)
    print(f"upgrade fixture of {version}: {out} ({(out / 'datadir.tar.zst').stat().st_size} bytes, "
          f"{len(reads)} reads, snapshots held: {held or 'none'}, {status['system']['applied']} entries)")
    if status["system"]["applied"] >= SNAPSHOT_EVERY:
        raise SystemExit(f"the {part} part holds {status['system']['applied']} entries: the log takes a snapshot at "
                         f"{SNAPSHOT_EVERY} and purges what it has before; split its cases over another part")



def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--from-dir", required=True, help="the release's merod, meroctl, VERSION, kv-store-test-fixture.mpk")
    ap.add_argument("--out", required=True, help="the base fixture's directory; the cases' is beside it, with -cases")
    ap.add_argument("--port", type=int, default=7790)
    args = ap.parse_args(argv)
    frm = Path(args.from_dir).resolve()
    os.environ["RV2_MEROD"] = str(frm / "merod")
    os.environ["RV2_MEROCTL"] = str(frm / "meroctl")
    sys.path.insert(0, str(HERE))
    out = Path(args.out)
    record(frm, out, args.port, "base")
    record(frm, out.with_name(out.name + "-cases"), args.port, "cases")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
