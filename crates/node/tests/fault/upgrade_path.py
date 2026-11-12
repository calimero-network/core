"""The upgrade path: a cluster on a released merod rolled forward to this tree's, as production does.

Every other test runs one build on fresh state. This one starts a 3-node cluster
(and a job node) on the merod of a published release (`--from-dir`, which
`scripts/fetch-release.sh` fills from the release's own assets), writes data
through the public API with that release's meroctl, rolls the cluster onto this
tree's merod exactly as mdma rolls a Calimero Cloud cluster (rollout.py:
approve the release, add 3 nodes, move every app, remove the old nodes), then
reads everything back, writes new data, restarts a new node from its data
directory and reads everything again.

    python3 -I crates/node/tests/fault/upgrade_path.py --from-dir target/upgrade-from \\
        --to-merod target/release/merod --to-meroctl target/release/meroctl \\
        --kv-next dist/upgrade-path/kv-next.mpk --pipelines-mpk dist/upgrade-path/pipelines.mpk

**Cases.** What is written and checked lives in cases (`upgrade_cases/NN_*.py`,
loaded in file-name order), each a `Case` with four steps the runner calls:

| Step | When | Cluster | meroctl |
| --- | --- | --- | --- |
| `seed(w)` | before the rollout | every node on the old release | the old release's |
| `mixed(w)` | once the first app has moved | old and new nodes, apps on both | the old release's |
| `verify(w)` | after the rollout, and again after a new node restarts from disk | new nodes only | this tree's |
| `extend(w)` | between the two `verify`s | new nodes only | this tree's |

`mixed` defaults to `verify`, so `verify` reads only (it runs up to three
times); what `extend` writes, `verify` checks from then on (keep the
expectation on the case). A case reaches the cluster through `w` (`World`):
accounts (`w.account(name)`, signed up on first use), the fleet (`w.fleet`), the
package registry (`w.registry`), the fixtures (`w.fixtures`), values other cases
left (`w.shared`) and `w.check(cond, what)`. A failing `seed` stops the run (the
rest would test nothing); a failing `mixed`, `verify` or `extend` is reported
and the next case runs, so one run lists every breakage.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import shutil
import sys
import time
import traceback
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
# Cases import `upgrade_path` for `Case` and `case`: the running script is that module.
sys.modules.setdefault("upgrade_path", sys.modules[__name__])
import rollout  # noqa: E402
import rv2  # noqa: E402
import suite  # noqa: E402
from rollout import Fleet, wait  # noqa: E402
from rv2 import ROOT, HttpError, PackageRegistry  # noqa: E402

HERE = Path(__file__).resolve().parent
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
BASE_PORT = 7670  # replicas 1-6, job nodes 9 and 10 (+ WORKFLOW_PORT_OFFSET)
OLD_IDS = [1, 2, 3]
NEW_IDS = [4, 5, 6]
OLD_JOB, NEW_JOB = 9, 10
# The mero-tee releases the two sets of nodes attest (mock quotes): distinct, so
# the operator's approval and the release gates are on the path.
FROM_RELEASE = "2.3.1000"
TO_RELEASE = "2.3.1001"
UPLOAD_TRIES = 5


class Failed(AssertionError):
    pass


# ----------------------------------------------------------------- cases

CASES: list[type] = []


def case(cls):
    """Register a `Case` subclass; the runner calls it in registration order."""
    CASES.append(cls)
    return cls


class Case:
    """One kind of data: written on the old cluster, read back on the new one."""

    name = ""

    def seed(self, w: World):
        """Write it on the old cluster (old merod, old meroctl)."""

    def mixed(self, w: World):
        """Read it while old and new nodes both hold apps."""
        self.verify(w)

    def verify(self, w: World):
        """Read all of it on the rolled cluster: read-only, it runs again after a restart."""

    def extend(self, w: World):
        """Write more on the rolled cluster; `verify` checks it from then on."""


def load_cases(directory: Path):
    """Import every `NN_*.py` of `directory` in name order; their `@case`s register."""
    for path in sorted(directory.glob("[0-9]*.py")):
        spec = importlib.util.spec_from_file_location(f"upgrade_cases.{path.stem}", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    return CASES


# ----------------------------------------------------------------- the world


class Account(suite.Account):
    """`suite.Account` that talks only to replica nodes (job nodes serve no API).

    meroctl keeps a session per API address; a Cloud cluster has one address in
    front of every node, so before meroctl reaches another node the session it
    last used (refreshed or not) is what that address holds: the session made on
    the old release, issued by a node that is gone, is what the new nodes see."""

    session_url = None

    def cli(self, *args, node=None):
        target = node or self.order()[0]
        self._carry_session(self.c.url(target))
        out = self.c.cli(target, *args, home=self.home)
        self.session_url = self.c.url(target)
        return out

    def _carry_session(self, url):
        path = self.home / "meroctl" / "sessions.json"
        if self.session_url in (None, url) or not path.exists():
            return
        sessions = json.loads(path.read_text())
        if self.session_url in sessions:
            sessions[url] = sessions[self.session_url]
            path.write_text(json.dumps(sessions, indent=2))

    def order(self, node=None):
        if node is not None:
            return [int(node)]
        up = [i for i in self.c.members if i in self.c.running() and i not in self.c.paused] or self.c.members
        home = self.index % len(up)
        return up[home:] + up[:home]

    def http(self, method, path, body=None, key=None, node=None, timeout=60):
        h = {"authorization": f"Bearer {key or self.key}"}
        return self._send(node, lambda n: rv2.http(method, self.c.url(n) + path, body, h, timeout))

    def raw_bytes(self, method, path, data, key=None, node=None, content_type="application/octet-stream",
                  timeout=120):
        """`method path` with a raw body; the JSON answer."""
        h = {"authorization": f"Bearer {key or self.key}", "content-type": content_type}
        return self._send(node, lambda n: _raw(method, self.c.url(n) + path, data, h, timeout))

    def download(self, path, key=None, node=None, timeout=120):
        h = {"authorization": f"Bearer {key or self.key}"}
        return self._send(node, lambda n: _get_bytes(self.c.url(n) + path, h, timeout))

    def upload_file(self, app, data, node=None):
        """A sealed file (`/apps/{app}/files`): one request up to a part, else an upload session."""
        base = f"apps/{app}/files"
        try:
            self.http("GET", base, node=node)
        except HttpError as e:
            if e.status != 404:
                raise
            # 1.0.0-rc.14 and older (experiments only): a sealed file was a blob upload
            # with `?sealed=true`, one request, no files routes.
            if len(data) > rollout_part_bytes():
                raise NoUploadSessions(f"{len(data)} bytes: the node has no upload sessions") from None
            return self._check_upload(data, self.raw_bytes("POST", f"apps/{app}/blobs?sealed=true", data,
                                                           node=node))
        if len(data) <= rollout_part_bytes():
            return self._check_upload(data, _retry_unavailable(lambda: self.raw_bytes("POST", base, data, node=node)))
        u = self.http("POST", base + "/uploads", {"size": len(data)}, node=node)
        parts, step = f"{base}/uploads/{u['uploadId']}", u["partBytes"]
        for k in range(u["parts"]):
            _retry_unavailable(lambda k=k: self.raw_bytes("PUT", f"{parts}/parts/{k}",
                                                          data[k * step:(k + 1) * step], node=node))
        return self._check_upload(data, _retry_unavailable(lambda: self.http("POST", parts + "/complete", {},
                                                                             node=node)))

    def file_info(self, app, file_id, node=None):
        """A sealed file's `{size, sha256}` (`GET /apps/{app}/files/{id}`; 1.0.0-rc.14 and
        older answered it at `/blobs/{id}/info`)."""
        try:
            return self.http("GET", f"apps/{app}/files/{file_id}", node=node)
        except HttpError as e:
            if e.status != 404 or not isinstance(e.body, str):
                raise
            return self.http("GET", f"apps/{app}/blobs/{file_id}/info", node=node)

    @staticmethod
    def _check_upload(data, answer):
        want = hashlib.sha256(data).hexdigest()
        if answer.get("sha256") != want:
            raise Failed(f"upload of {len(data)} bytes answered {answer}, want sha256 {want}")
        return answer


class NoUploadSessions(Failed):
    """A release before upload sessions (1.0.0-rc.14 and older): one file is one request."""


def rollout_part_bytes():
    return 16 << 20  # the most one file request carries (node PART_BYTES)


def _raw(method, url, data, headers, timeout):
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            body = json.loads(raw)
        except ValueError:
            body = raw.decode(errors="replace")
        raise HttpError(e.code, body) from None


def _get_bytes(url, headers, timeout):
    req = urllib.request.Request(url, method="GET", headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        raise HttpError(e.code, e.read().decode(errors="replace")) from None


def _retry_unavailable(request):
    """`request()`, again while the node answers `503 FILES_UNAVAILABLE` (a resent part replays)."""
    for tries in range(1, UPLOAD_TRIES + 1):
        try:
            return request()
        except HttpError as e:
            busy = e.status == 503 and isinstance(e.body, dict) and e.body.get("error") == "FILES_UNAVAILABLE"
            if not busy or tries == UPLOAD_TRIES:
                raise
            time.sleep(1)
    return None


class World:
    """What a case sees: the fleet, its accounts, the registry, fixtures, shared values."""

    def __init__(self, fleet: Fleet, registry: PackageRegistry, fixtures: dict, phase_log):
        self.fleet = fleet
        self.registry = registry
        self.fixtures = fixtures
        self.shared: dict = {}
        self.phase = "seed"
        self._accounts: dict[str, Account] = {}
        self._log = phase_log

    def account(self, name) -> Account:
        """The account `name`, signed up (on the cluster as it is now) on first use."""
        if name not in self._accounts:
            self._accounts[name] = Account(self.fleet, name, len(self._accounts)).sign_up()
        return self._accounts[name]

    def check(self, cond, what):
        if not cond:
            raise Failed(f"[{self.phase}] {what}")

    def expect_refused(self, code, f, retry_for=0.0):
        try:
            suite.expect_refused(code, f, retry_for)
        except suite.Failed as e:
            raise Failed(f"[{self.phase}] {e}") from None

    def note(self, s):
        self._log(f"    {s}")

    def members(self):
        """The replica nodes up now."""
        return [i for i in self.fleet.members if i in self.fleet.running()]

    def wait(self, what, pred, timeout=60):
        try:
            return wait(what, pred, timeout)
        except rollout.RolloutError as e:
            raise Failed(f"[{self.phase}] {e}") from None


# ----------------------------------------------------------------- the run


def say(s=""):
    print(s, flush=True)


def run_step(w, cases, step, results, fatal=False, label=None):
    w.phase = label or step
    say(f"== {w.phase}")
    for c in cases:
        t0 = time.time()
        try:
            getattr(c, step)(w)
            status, error = "pass", None
        except Exception as e:  # noqa: BLE001
            status, error = "fail", f"{type(e).__name__}: {e}"
            if not isinstance(e, Failed):
                traceback.print_exc()
        took = round(time.time() - t0, 1)
        results.append({"case": c.name, "step": w.phase, "status": status, "seconds": took, "error": error})
        say(f"   {status.upper()} {c.name}.{step} ({w.phase}) in {took}s" + (f": {error}" if error else ""))
        if error and fatal:
            raise Failed(f"{c.name}.{step} failed, so the run stops: {error}")


def restart_from_disk(w, node):
    """Stop new node `node` and start it again on its data directory."""
    say(f"== restart node {node} from its data directory")
    w.fleet.restart(node)
    w.wait("the system group has a leader and 3 voters again",
           lambda: rollout.system_ready(w.fleet, len(w.fleet.members)), 60)
    groups = {a["app"] for a in w.fleet.placement()["apps"] if node in a["replicas"]}
    for app in groups:
        w.wait(f"node {node} back in {app}'s group", lambda a=app: w.fleet.app_status(node, a), 60)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--from-dir", required=True,
                    help="the release's assets, as scripts/fetch-release.sh lays them out: merod, meroctl, "
                         "kv-store-test-fixture.mpk, VERSION, and the compute fixture's compute.mpk")
    ap.add_argument("--to-merod", default=str(TARGET / "release" / "merod"))
    ap.add_argument("--to-meroctl", default=str(TARGET / "release" / "meroctl"))
    ap.add_argument("--kv-next", required=True,
                    help="kv built from this tree, signed as a later release of the kv package (the app upgrade)")
    ap.add_argument("--pipelines-mpk", required=True,
                    help="the compute fixture built from this tree with its pipeline, as a compute-only package "
                         "(pipeline cases seed it after the roll)")
    ap.add_argument("--cases", default=str(HERE / "upgrade_cases"))
    ap.add_argument("--only", help="comma-separated case names")
    ap.add_argument("--workdir", default=str(TARGET / "upgrade-path"))
    ap.add_argument("--json", help="write the results here")
    ap.add_argument("--keep", action="store_true", help="keep the work directory of a passing run")
    args = ap.parse_args(argv)

    frm = Path(args.from_dir).resolve()
    fixtures = {
        "from_version": (frm / "VERSION").read_text().strip(),
        "kv_from": frm / "kv-store-test-fixture.mpk",
        "compute_from": frm / "compute.mpk",
        "kv_next": Path(args.kv_next).resolve(),
        "pipelines": Path(args.pipelines_mpk).resolve(),
    }
    old_merod, old_meroctl = str(frm / "merod"), str(frm / "meroctl")
    new_merod, new_meroctl = str(Path(args.to_merod).resolve()), str(Path(args.to_meroctl).resolve())
    for p in (old_merod, old_meroctl, new_merod, new_meroctl, *[v for k, v in fixtures.items() if k != "from_version"]):
        if not Path(p).exists():
            raise SystemExit(f"missing {p}")

    load_cases(Path(args.cases))
    only = {n.strip() for n in (args.only or "").split(",") if n.strip()}
    cases = [c() for c in CASES if not only or c.name in only]
    say(f"upgrade path: merod {fixtures['from_version']} ({old_merod}) -> {new_merod}")
    say(f"cases: {[c.name for c in cases]}")

    offset = int(os.environ.get("WORKFLOW_PORT_OFFSET", "0"))
    wd = Path(args.workdir)
    shutil.rmtree(wd, ignore_errors=True)
    wd.mkdir(parents=True)
    registry = PackageRegistry(wd / "registry")
    fleet = Fleet(wd, base_port=BASE_PORT + offset, cluster="upgrade-path", binary=old_merod,
                  meroctl=old_meroctl, release=FROM_RELEASE, registry=registry)
    w = World(fleet, registry, fixtures, say)
    results: list[dict] = []
    timings: dict = {}
    t0 = time.time()
    try:
        fleet.start()
        fleet.start_job(OLD_JOB, old_merod, FROM_RELEASE)
        run_step(w, cases, "seed", results, fatal=True)

        def mixed():
            run_step(w, cases, "mixed", results)

        w.phase = "rollout"
        timings = rollout.roll_forward(fleet, new_merod, TO_RELEASE, NEW_IDS, log=say, on_mixed=mixed)
        # Clients move to this tree's meroctl; the job node is replaced as the replicas were.
        fleet.meroctl = new_meroctl
        fleet.start_job(NEW_JOB, new_merod, TO_RELEASE)
        fleet.retire_job(OLD_JOB)
        run_step(w, cases, "verify", results)
        run_step(w, cases, "extend", results)
        w.phase = "restart"
        restart_from_disk(w, NEW_IDS[0])
        run_step(w, cases, "verify", results, label="verify after restart")
    except Exception as e:  # noqa: BLE001
        results.append({"case": "(runner)", "step": w.phase, "status": "fail", "seconds": 0,
                        "error": f"{type(e).__name__}: {e}"})
        if not isinstance(e, (Failed, rollout.RolloutError)):
            traceback.print_exc()
        say(f"   FAIL {w.phase}: {e}")
    finally:
        fleet.stop()
        registry.stop()
    total = round(time.time() - t0, 1)
    failed = [r for r in results if r["status"] != "pass"]
    say(f"DONE {len(results) - len(failed)}/{len(results)} steps passed in {total}s "
        f"({', '.join(f'{k} {v:.0f}s' for k, v in timings.items())})"
        + (f"; failed: {[(r['case'], r['step']) for r in failed]}" if failed else ""))
    if args.json:
        Path(args.json).parent.mkdir(parents=True, exist_ok=True)
        Path(args.json).write_text(json.dumps({
            "from": fixtures["from_version"], "to": new_merod, "seconds": total, "rollout": timings,
            "results": results, "workdir": str(wd)}, indent=2) + "\n")
    if not failed and not args.keep:
        shutil.rmtree(wd, ignore_errors=True)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
