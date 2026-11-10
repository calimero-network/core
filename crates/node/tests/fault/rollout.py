"""A cluster moved to another merod the way production moves it: mdma's rollout.

mdma (`dispatcher/app/rollouts.py`) never restarts a node on a new binary. It
adds new nodes on the new release beside the old ones and moves the data:

1. **approve**: the operator approves the new release for every app
   (`POST /ops/releases`, signed with the operator key), where governance gives
   the operator release approvals;
2. **add_nodes**: as many new-release nodes as old ones (at least 3), one at a
   time: each starts on an empty data directory, waits for the cluster key, and
   is registered (`POST /ops/nodes`, signed), which admits it to the system
   group as a learner;
3. **move_apps**: every app with a replica on an old node is moved onto new
   nodes (`POST /ops/apps/{app}/replicas`, signed), one app at a time, keeping
   its replica count (at least min(3, registered nodes)), until the placement
   (`GET /ops/placement`) shows it there;
4. **remove_old**: the old nodes leave one at a time (`DELETE /ops/nodes/{id}`,
   signed, at the system leader), each only while the system group has a leader
   and its full voter count, and the process stops (the VM goes).

So for the whole move a cluster runs two merod builds side by side: the system
group's Raft, every app group's Raft, the internal routes, blob and file
segment transfer and peer attestation all cross versions. `Fleet` is that
cluster on one machine (`merod run --dev`, one process per node, rv2.py) and
`roll_forward` drives it through the four steps, calling a hook once apps sit on
both releases. The upgrade-path test (`upgrade_path.py`) uses it; any test that
needs a cluster rolled from one merod to another can too.

A node's mero-tee release is what its (mock, `--dev`) attestation names:
`--tee-release`. The old and the new nodes name different releases, so the
release approval and every release gate on the way (`may_send`, replica moves,
internal routes) are exercised as in production, whatever versions the two
builds report.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from pathlib import Path

import rv2
from rv2 import Cluster, PackageRegistry

# The node tests' operator key (harness OPERATOR_SEED, [9; 32]), as compute_e2e.py signs with.
OPERATOR_SEED = "09" * 32
OPERATOR_PUBLIC = "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"
# mdma's MIN_NEW_NODES: a rollout adds at least this many nodes.
MIN_NEW_NODES = 3
MOVE_TIMEOUT_S = 180  # one app's move: its data streams to three new replicas
STEP_TIMEOUT_S = 120


class RolloutError(AssertionError):
    pass


class Fleet(Cluster):
    """A `Cluster` whose node set changes: founders `1..n` on one build, nodes
    joined later (any id) on another, job nodes (`--job-node`).

    Every node starts with `--operator-key`, `--release-approvals operator` and a
    `--tee-release` of its own (`release[i]`); `meroctl` is the command line
    `cli` runs (switch it with the cluster: a user's client is the one of their
    time)."""

    def __init__(self, workdir: Path, *, base_port: int, cluster: str, binary: str, meroctl: str,
                 release: str, registry: PackageRegistry | None = None, n: int = 3):
        super().__init__(workdir, base_port=base_port, n=n, cluster=cluster, binary=binary,
                         registry=registry)
        self.meroctl = meroctl
        self.release = {i: release for i in self.ids}
        self.joined: list[int] = []  # replica nodes started after the founders, in order
        self.jobs: set[int] = set()
        self.removed: set[int] = set()
        self.identity = ""
        (self.workdir / "operator.key").write_text(OPERATOR_SEED)

    # ---- the node set
    @property
    def members(self):
        """Replica nodes started and not removed, founders first."""
        return [i for i in [*super().ids, *self.joined] if i not in self.removed]

    def url(self, i):
        return f"http://127.0.0.1:{self.base_port + i}/"

    def args(self, i):
        if i in self.jobs:
            return [self.binaries[i], "run", "--job-node", "--node-id", str(i),
                    "--listen", f"127.0.0.1:{self.base_port + i}", "--data-dir", str(self.dir(i)),
                    "--cluster", self.cluster, "--dev", "--cluster-identity", self.identity,
                    "--tee-release", self.release[i]]
        a = super().args(i)
        if i not in range(1, self.n + 1):
            # A joining node knows every address up to its own; it never bootstraps.
            known = sorted({*range(1, self.n + 1), *self.joined, i})
            peers = ",".join(f"{j}={self.url(j)}" for j in known)
            a[a.index("--peers") + 1] = peers
        return a + ["--operator-key", OPERATOR_PUBLIC, "--release-approvals", "operator",
                    "--tee-release", self.release[i]]

    def running(self):
        return [i for i, p in self.procs.items() if p is not None and p.poll() is None]

    def stop(self):
        for i in list(self.procs):
            self.kill(i)

    def any_node(self):
        up = [i for i in self.members if i in self.running() and i not in self.paused]
        if not up:
            raise RolloutError("no replica node is running")
        return up[0]

    # ---- meroctl as the cluster's operator or as an account
    def cli(self, i, *args, env_extra=None, home=None):
        env = dict(os.environ)
        env.pop("CALIMERO_API_KEY", None)
        env.update(env_extra or {})
        home = Path(home) if home else self.workdir / "cfg"
        home.mkdir(parents=True, exist_ok=True)
        out = subprocess.run([self.meroctl, "--home", str(home), "--api", self.url(i), "--allow-mock", *args],
                             env=env, capture_output=True, text=True, timeout=180)
        if out.returncode != 0:
            raise rv2.CliError(args, out.stderr)
        return json.loads(out.stdout) if out.stdout.strip() else None

    def operate(self, *args, node=None):
        """An operator decision through meroctl, signed with the operator key when the node asks."""
        return self.cli(node or self.any_node(), "--ops-token", self.ops_token,
                        "--operator-key-file", str(self.workdir / "operator.key"), "ops", *args)

    def ops_get(self, path, node=None):
        return rv2.http("GET", self.url(node or self.any_node()) + path,
                        headers={"authorization": f"Bearer {self.ops_token}"}, timeout=10)

    def placement(self):
        return self.ops_get("ops/placement")

    def system(self, node=None):
        return self.ops_get("ops/status", node)["system"]

    # ---- joining and leaving
    def join(self, i, binary, release, register=True):
        """Start replica node `i` on `binary`, naming `release`, on an empty data
        directory, and register it (the operator's `add-node`)."""
        self.binaries[i] = binary
        self.release[i] = release
        self.procs.setdefault(i, None)
        self.joined.append(i)
        self.start_node(i)  # healthy once it serves /health, waiting for the cluster key
        if register:
            self.operate("add-node", str(i), self.url(i))
        wait(f"node {i} holds the cluster key and answers ops/status",
             lambda: _answers(self, i), STEP_TIMEOUT_S)

    def start_job(self, i, binary, release):
        """A job node (`merod run --job-node`) of the cluster, registered with `--role job`."""
        if not self.identity:
            self.identity = self.ops_get("ops/cluster")["identity"]
        self.binaries[i] = binary
        self.release[i] = release
        self.jobs.add(i)
        self.procs.setdefault(i, None)
        self.start_node(i)
        self.operate("add-node", str(i), self.url(i), "--role", "job")

    def retire_job(self, i):
        self.operate("remove-node", str(i))
        self.kill(i)
        self.jobs.discard(i)
        self.removed.add(i)

    def remove(self, i):
        """`DELETE /ops/nodes/{i}` (meroctl follows the system leader), then the process stops."""
        self.operate("remove-node", str(i))
        self.kill(i)
        self.removed.add(i)


def _answers(fleet, i):
    try:
        return fleet.ops_get("ops/status", node=i) is not None
    except Exception:  # noqa: BLE001 -- still waiting for the key
        return False


def wait(what, pred, timeout=STEP_TIMEOUT_S, every=0.5):
    """`pred()` until it is truthy (its value), else RolloutError naming `what` and its last answer."""
    deadline = time.time() + timeout
    last = None
    while True:
        try:
            got = pred()
            if got:
                return got
            last = got
        except Exception as e:  # noqa: BLE001 -- retried, reported if it never passes
            last = f"{type(e).__name__}: {str(e)[:300]}"
        if time.time() > deadline:
            raise RolloutError(f"{what}: not within {timeout}s (last: {last!r})")
        time.sleep(every)


def roll_forward(fleet: Fleet, binary: str, release: str, new_ids: list[int], *, log=print,
                 on_mixed=None) -> dict:
    """Move `fleet` onto `binary` (naming `release`) as mdma does; `new_ids` are the
    new nodes' ids, at least as many as the old ones and at least 3.

    `on_mixed()` runs once, after the first app has moved: new nodes hold some
    apps, old nodes the rest. Returns what each step took (seconds)."""
    old = list(fleet.members)
    if len(new_ids) < max(MIN_NEW_NODES, len(old)):
        raise RolloutError(f"{len(new_ids)} new nodes for {len(old)} old ones: mdma adds at least "
                           f"max({MIN_NEW_NODES}, old)")
    took = {}

    t = time.time()
    log(f"rollout: approve release {release} for every app (POST /ops/releases)")
    fleet.operate("release", release)
    took["approve"] = time.time() - t

    t = time.time()
    for i in new_ids:
        log(f"rollout: add node {i} on {Path(binary).name} ({release}); it joins as a learner")
        fleet.join(i, binary, release)
    voters = wait("every new node is in the system group and it has a leader",
                  lambda: _system_ready(fleet, len(old) + len(new_ids), new_ids))
    log(f"rollout: system group {voters}")
    took["add_nodes"] = time.time() - t

    t = time.time()
    moved, mixed_done = [], on_mixed is None
    while True:
        placement = fleet.placement()
        on_old = sorted((a for a in placement["apps"] if set(a["replicas"]) & set(old)), key=lambda a: a["app"])
        if not on_old:
            break
        app = on_old[0]
        count = max(len(app["replicas"]), min(3, len(placement["nodes"])))
        target = _choose(app, new_ids, placement, count)
        log(f"rollout: move {app['app']} {app['replicas']} -> {target}")
        _move(fleet, app["app"], target, log)
        moved.append(app["app"])
        if not mixed_done:
            mixed_done = True
            log("rollout: mixed cluster (apps on both releases): running the mixed checks")
            on_mixed()
    took["move_apps"] = time.time() - t
    if not mixed_done:
        log("rollout: no app to move; mixed checks run with every node on the cluster")
        on_mixed()

    t = time.time()
    for i in sorted(old, key=lambda n: (n == _leader(fleet), n)):
        wait(f"a system leader and full voters before removing node {i}",
             lambda: _system_ready(fleet, len(fleet.members)))
        log(f"rollout: remove old node {i} (DELETE /ops/nodes/{i})")
        wait(f"node {i} removed", lambda n=i: _try_remove(fleet, n), STEP_TIMEOUT_S, every=2)
    wait("the system group keeps a leader and 3 voters on the new nodes",
         lambda: _system_ready(fleet, len(fleet.members)))
    took["remove_old"] = time.time() - t
    log(f"rollout: done: {moved and len(moved)} apps moved, nodes {fleet.members}; "
        + ", ".join(f"{k} {v:.0f}s" for k, v in took.items()))
    return took


def _leader(fleet):
    try:
        return fleet.system().get("leader")
    except Exception:  # noqa: BLE001
        return None


def _system_ready(fleet, nodes, members=()):
    s = fleet.system()
    voters = [int(v) for v in s.get("voters") or []]
    learners = [int(v) for v in s.get("learners") or []]
    if s.get("leader") is None or len(voters) < min(3, nodes) or set(members) - set(voters) - set(learners):
        return None
    return {"leader": s["leader"], "voters": voters, "learners": learners}


def _move(fleet, app, target, log, attempts=3):
    """Send the move and poll the placement, as mdma does; an unanswered or refused
    request, or a placement that never shows it, is sent again next round."""
    for attempt in range(1, attempts + 1):
        try:
            fleet.operate("move", app, "--replicas", ",".join(map(str, target)))
        except rv2.CliError as e:
            log(f"    move {app} (attempt {attempt}) answered: {e.stderr.strip().splitlines()[0][:300]}")
        try:
            return wait(f"placement shows {app} on {target}", lambda: _placed(fleet, app, target),
                        MOVE_TIMEOUT_S // attempts)
        except RolloutError as e:
            if attempt == attempts:
                raise
            log(f"    {e}; sending the move again")
    return None


def _placed(fleet, app, target):
    for a in fleet.placement()["apps"]:
        if a["app"] == app:
            return sorted(a["replicas"]) == sorted(target)
    return False


def _choose(app, new_ids, placement, count):
    """Where an app goes: new nodes holding fewest apps first (mdma's `_choose`)."""
    load = {n["id"]: n.get("apps", 0) for n in placement["nodes"]}
    return sorted(sorted(new_ids, key=lambda n: (load.get(n, 0), n))[:count])


def _try_remove(fleet, i):
    try:
        fleet.remove(i)
        return True
    except rv2.CliError as e:
        # The last voter, or a leader still promoting: kept, retried as mdma retries.
        print(f"    remove {i} refused, retrying: {e.stderr.strip()[-200:]}", flush=True)
        return False
