"""The deep upgrade cases' toolkit, on top of the upgrade-path harness (`upgrade_path.py`).

`upgrade_path.py` rolls a cluster from a published release to this tree's merod
(rollout.py) and runs every case in `upgrade_cases/NN_*.py`. The deep cases
(`upgrade_cases/20_*.py` and on) cover each user-visible functionality: they are
`DeepCase`s, which add to the harness's `Case`:

- `area`, `seeds`, `verifies`: the coverage matrix's row (`--matrix` below
  prints the matrix, `--check-matrix DOC` fails when DOC's copy is stale);
- `since`: the first release with the functionality. Rolled from an older
  release the case is seeded on the rolled cluster instead (in `extend`) and
  checked from then on: a new feature's case runs from the PR that adds it, and
  becomes a full upgrade case once the suite rolls from a release that has it;
- `populate(w)` seeds and answers a JSON-able record, `check(w, s)` reads it
  back (allowed and refused access included) and runs right after seeding too,
  so a check that fails later failed because of the upgrade; `during_mix(w, s)`
  (default `check`) runs in the mixed cluster; `exercise(w, s)` writes on the
  rolled cluster and updates `s` for the checks after it.

`person(w, name)` is an account as these cases use it: the harness's account
(file passkey, meroctl home, API key) whose meroctl signs in again on a node that
holds no session of theirs (a node the rollout added).

    python3 -I crates/node/tests/fault/upgrade_deep.py --matrix
    python3 -I crates/node/tests/fault/upgrade_deep.py --check-matrix docs/src/content/docs/contribute/testing.mdx
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
import socket
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from contextlib import contextmanager
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.modules.setdefault("upgrade_deep", sys.modules[__name__])
import upgrade_path  # noqa: E402
from rv2 import ORIGIN, CliError, HttpError  # noqa: E402
from upgrade_path import Case, Failed  # noqa: E402

ROOT = HERE.parents[3]
MATRIX_BEGIN = "{/* upgrade-matrix:begin: python3 -I crates/node/tests/fault/upgrade_deep.py --matrix */}"
MATRIX_END = "{/* upgrade-matrix:end */}"

# What no case seeds, and why: (area, functionality, reason). A functionality
# that cannot be tested yet is listed here, never skipped silently.
NOT_COVERED = [
    ("Accounts", "Account recovery started and finished with a new passkey",
     "the start takes a WebAuthn registration made outside a session, which neither meroctl nor the "
     "suite's standard-library Python can make (P-256); the cluster tests cover it on one build. The "
     "recovery code (`account-recovery-code`) is covered"),
    ("Accounts", "Sign-up through a cluster sign-up invite, and issuing one",
     "meroctl's login takes no invite and has no command to issue one (an operator-signed request); "
     "the policy and the invite list are read back (`ops-state`)"),
    ("Accounts", ("Consent-page approvals (`/consent/requests`: release, upgrade and outbound approvals; the "
                  "organization kinds `createOrg`, `createInvite`, `acceptInvite`, `leaveOrg`, `removeMember`, "
                  "`setMemberRole`, `setMemberTeams`, `removeMemberTeams`, `revokeInvite`, `renameOrg`, "
                  "`deleteOrg`, `setCapabilities`, `setAllApps`) and the page's verifier link"),
     "a consent request is approved in a browser on the node's consent page; the cluster test "
     "`consent_page.rs` covers every kind on one build. What an approval writes is the same log entry as "
     "the API's route, which the organization and team cases cover; a `createOrg` asked from an app's "
     "frontend also makes that app's team in the same `CreateOrg`, the organization and team `ScopeApps` "
     "makes, which `app-teams` covers. The verifier link is a startup setting "
     "(`--attestation-verifier-url`) that stores nothing"),
    ("Apps", "Timers (`#[app::every]`) firing across the roll",
     "the fixture's timer is hourly on the nodes' real clock, which a process cluster cannot move; "
     "the cluster tests move the app clock"),
    ("Packages", "Served frontends and served origins (custom domains)",
     "the suite's packages carry no frontend, and the node routes a frontend by Host under the apps "
     "domain, which plain HTTP on 127.0.0.1 does not reach; the cluster test "
     "`app_frontends_are_served_and_their_sessions_are_bound` covers serving"),
    ("Governance", "Owner-held release approvals and release deadlines",
     "the rolled cluster gives release approvals to the operator (`--release-approvals operator`), as "
     "Calimero Cloud does; the operator's approval of the new release is the rollout's first step"),
    ("Deployments", "Deployer grants and deployments (`/deployers`, `/deployments`, `/ops/deployers`)",
     "not seeded yet: a grant is an operator-signed or passkey-approved decision these cases do not make "
     "yet; the cluster tests (`deployers.rs`, `deployments.rs`) cover them on one build"),
    ("Packages", "Multi-service packages (`services`) and instances of a named service (`--service`)",
     "not seeded yet: the suite installs the release's single-service kv bundle; `meroctl app pack --service` "
     "writes one from components now, and the node's install of a named service is covered on one build "
     "(`component_packages.rs`)"),
    ("Packages", "Registered web origins of a package (`/package-origins`)",
     "not seeded yet: an origin is added with its manager's passkey approval and is read only by the "
     "served-frontend path above"),
    ("Blobs and files", "Revoking a published blob's grants (`/blobs/{id}/revoke`)",
     "not seeded yet: a grant is a URL the app's `share` view mints, which these cases do not follow"),
    ("Pipelines", "Live pipeline events (`meroctl cloud compute pipeline watch`)",
     "the stream is many lines and the suite's meroctl runner reads one JSON answer; a stranger's refusal "
     "is covered, the stream itself by the cluster tests"),
    ("Ops", "Restoring a backup onto the new release",
     "`POST /ops/backup` is exercised on the new nodes (`ops-state`); a restore is a recovery drill, "
     "not a rollout"),
]


class DeepCase(Case):
    area = ""
    since = "1.0.0-rc.15"
    seeds = ""
    verifies = ""

    def populate(self, w) -> dict:
        raise NotImplementedError

    def check(self, w, s: dict):
        raise NotImplementedError

    def during_mix(self, w, s: dict):
        self.check(w, s)

    def exercise(self, w, s: dict):
        pass

    # ---- the harness's steps
    #
    # The harness stops the run when a seed fails, since its representative cases
    # build on each other. Deep cases stand alone: one that fails to seed is
    # reported by each later step (the run goes on and lists every breakage).
    def seed(self, w):
        self.s = None
        self.seed_error = None
        if release_key(w.fixtures["from_version"]) < release_key(self.since):
            w.note(f"{self.name}: {w.fixtures['from_version']} predates it (since {self.since}); seeded after the roll")
            return
        try:
            self.s = self.populate(w)
            self.check(w, self.s)
        except Exception as e:  # noqa: BLE001 -- reported by every later step of this case
            self.seed_error = f"seeding on the old release failed: {type(e).__name__}: {e}"
            w.note(f"FAIL {self.name}: {self.seed_error}")

    def _seeded(self):
        if self.seed_error:
            raise Failed(self.seed_error)
        return self.s is not None

    def mixed(self, w):
        if self._seeded():
            self.during_mix(w, self.s)

    def verify(self, w):
        if self._seeded():
            self.check(w, self.s)

    def extend(self, w):
        if not self._seeded():
            self.s = self.populate(w)
            self.check(w, self.s)
        self.exercise(w, self.s)


def release_key(v: str):
    """`1.0.0-rc.15` as a sortable key; a release sorts after its prereleases."""
    m = re.match(r"^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?", v or "")
    if not m:
        raise ValueError(f"not a release version: {v!r}")
    pre = m.group(4)
    parts = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split(".")) if pre else ()
    return (int(m.group(1)), int(m.group(2)), int(m.group(3)), 0 if pre else 1, parts)


# ------------------------------------------------------------------ checks


def check(cond, what):
    if not cond:
        raise Failed(what)


def _short(v, n=400):
    s = v if isinstance(v, str) else json.dumps(v, sort_keys=True, default=str)
    return s if len(s) <= n else s[:n] + "..."


def eq(got, want, what):
    if got != want:
        raise Failed(f"{what}: got {_short(got)}, want {_short(want)}")


def subset(got, want, what, path="$"):
    """`got` holds every key `want` holds, with the same value; a list holds the
    same items in the same order, each compared the same way: a new release may
    add fields (to an object, or to every object in a list), never drop or change
    one, nor add, drop or reorder a list's items."""
    if isinstance(want, dict):
        check(isinstance(got, dict), f"{what}: {path} is {_short(got)}, want an object")
        for k, v in want.items():
            check(k in got, f"{what}: {path}.{k} is gone (have {sorted(got)})")
            subset(got[k], v, what, f"{path}.{k}")
        return
    if isinstance(want, list):
        check(isinstance(got, list) and len(got) == len(want),
              f"{what}: {path} is {_short(got)}, want {_short(want)}")
        for i, (g, v) in enumerate(zip(got, want)):
            subset(g, v, what, f"{path}[{i}]")
        return
    if got != want:
        raise Failed(f"{what}: {path} is {_short(got)}, want {_short(want)}")


def code(e):
    """The refusal's code: the node's `error`, else the HTTP status, else meroctl's text."""
    if isinstance(e, HttpError):
        if isinstance(e.body, dict) and e.body.get("error"):
            return e.body["error"]
        return str(e.status)
    if isinstance(e, CliError):
        m = re.search(r"\b(\d{3}) ([A-Z_]+)\b", e.stderr)
        return m.group(2) if m else e.stderr.strip()[-200:]
    raise e


def decision(f):
    """What a request is answered: `ok`, or the refusal's code."""
    try:
        f()
        return "ok"
    except (HttpError, CliError) as e:
        return code(e)


def decisions(table: dict) -> dict:
    return {k: decision(f) for k, f in table.items()}


def same_decisions(got: dict, want: dict, what: str):
    diff = {k: f"now {got.get(k)}, was {v}" for k, v in want.items() if got.get(k) != v}
    check(not diff, f"{what}: {diff}")


def refused(codes, f, what, retry_for=0.0):
    """`f()` is refused with one of `codes`; answers the code."""
    codes = (codes,) if isinstance(codes, str) else tuple(codes)
    deadline = time.time() + retry_for
    while True:
        got = decision(f)
        if got in codes:
            return got
        if time.time() >= deadline:
            raise Failed(f"{what}: answered {got}, want {'/'.join(codes)}")
        time.sleep(0.25)


def until(what, f, timeout=30.0, every=0.25):
    """`f()` until it neither raises nor fails a check; its answer."""
    deadline = time.time() + timeout
    while True:
        try:
            return f()
        except (Failed, HttpError, CliError) as e:
            if time.time() >= deadline:
                raise Failed(f"{what}: still failing after {timeout}s: {_short(str(e), 600)}") from None
        time.sleep(every)


# Fields that move on their own between two reads: a sign-in counts, sessions age,
# writes stamp times, replicas move to the new nodes, the operator approves releases.
VOLATILE = frozenset({"signCount", "lastSignInAt", "lastActiveAt", "lastActiveDay", "sessions", "replicas",
                      "lastWriteAt", "clusterReleases", "approvedReleases", "current", "fresh"})


def scrub(v, drop=VOLATILE):
    if isinstance(v, dict):
        return {k: scrub(x, drop) for k, x in v.items() if k not in drop}
    if isinstance(v, list):
        return [scrub(x, drop) for x in v]
    return v


def snap(p, path, drop=VOLATILE):
    """`GET path` as `p`, without the fields that move on their own."""
    return scrub(p.get(path), drop)


def same_read(p, path, want, what, drop=VOLATILE):
    """`GET path` as `p` still holds everything it held when seeded."""
    subset(snap(p, path, drop), want, f"{what} (GET {path} as {p.name})")


# ------------------------------------------------------------------ people


class Person:
    """An account of the harness (`upgrade_path.Account`) as the deep cases drive it."""

    def __init__(self, w, a):
        self.w = w
        self.a = a
        self.name = a.name
        self.id = a.id
        self.key = a.key
        self.passkey = a.passkey
        self.home = a.home

    def order(self, node=None):
        return self.a.order(node)

    # ---- sessions
    def sessions(self) -> dict:
        try:
            return json.loads((self.home / "meroctl" / "sessions.json").read_text())
        except FileNotFoundError:
            return {}

    def sign_in(self, node):
        """A passkey sign-in on `node`, saved as this person's session there."""
        self.w.fleet.cli(node, "login", "--dev-passkey", str(self.passkey), "--origin", ORIGIN, home=self.home)

    def session(self, node=None) -> dict:
        """This person's saved session on `node` (their first node), signing in there first if need be."""
        node = node if node is not None else self.order()[0]
        url = self.w.fleet.url(node)
        if url not in self.sessions():
            self.sign_in(node)
        return self.sessions()[url]

    def token(self, node=None) -> str:
        s = self.session(node)
        return s.get("session_token") or s.get("sessionToken")

    # ---- meroctl over a passkey session, plain HTTP over the API key
    def cli(self, *args, node=None):
        last = None
        for n in self.order(node):
            try:
                self.session(n)
                return self.w.fleet.cli(n, *args, home=self.home)
            except CliError as e:
                if node is not None or not _transport(e):
                    raise
                last = e
        raise last or Failed("no node answers")

    def raw(self, method, path, body=None, node=None, approve=True):
        """Any route over the passkey session; a `412 CONFIRM_REQUIRED` is answered
        with the passkey when `approve`, as the person would."""
        args = ["raw", method, path] + ([json.dumps(body)] if body is not None else [])
        if approve:
            args += ["--dev-passkey", str(self.passkey)]
        return self.cli(*args, node=node)

    def get(self, path, node=None):
        return self.raw("get", path, node=node, approve=False)

    def http(self, method, path, body=None, key=None, token=None, node=None, timeout=60):
        bearer = token or key or self.key
        return self.a._send(node, lambda n: _http(method, self.w.fleet.url(n) + path, body, bearer, timeout))

    def call(self, app, method, args=None, key=None, node=None, timeout=60):
        return self.http("POST", f"apps/{app}/intents", {"method": method, "args": args or {}}, key=key,
                         node=node, timeout=timeout)

    def view(self, app, method, args=None, key=None, node=None, stale=False, min_index=0):
        body = {"method": method, "args": args or {}}
        if stale:
            body["consistency"] = "stale"
        if min_index:
            body["minIndex"] = int(min_index)
        return self.http("POST", f"apps/{app}/views", body, key=key, node=node)

    def result(self, app, method, args=None, **kw):
        return self.view(app, method, args, **kw)["result"]

    def deploy(self, wasm, name, org=None):
        """A dev bundle (raw component) installed and an app of it created."""
        args = ["context", "create", "--name", name, "--path", str(wasm)]
        if org:
            args += ["--namespace", org]
        return self.cli(*args)["appId"]

    def __repr__(self):
        return f"Person({self.name})"


def app_label(package: str) -> str:
    """`package`'s frontend host label under the apps domain (`calimero_node::app_host::label`):
    SHA-256 of `calimero.app-host.v1`, a zero byte and the package, unpadded lowercase base32."""
    digest = hashlib.sha256(f"calimero.app-host.v1\0{package}".encode()).digest()
    return base64.b32encode(digest).decode().lower().rstrip("=")


class Bound:
    """`p` signed in on `package`'s frontend origin (`http://{label}.localhost:{port}`, the
    nodes run with `--apps-domain localhost`): a session bound to that package
    (`SessionClaims::app`), as the package's own web page holds one. meroctl signs a
    passkey file's ceremonies with the origin the file names, so each request here
    names the frontend's origin in `p`'s file while it runs (the counter it moves is
    kept), signs in again and keeps the session in a meroctl home of its own."""

    def __init__(self, p: Person, package: str):
        self.p = p
        self.package = package
        self.home = scratch(p.w, f"bound-{p.name}-{app_label(package)[:8]}")

    def origin(self, node) -> str:
        port = urllib.parse.urlsplit(self.p.w.fleet.url(node)).port
        return f"http://{app_label(self.package)}.localhost:{port}"

    @contextmanager
    def _on_origin(self, origin):
        f = Path(self.p.passkey)
        was = json.loads(f.read_text())["origin"]

        def write(o):
            k = json.loads(f.read_text())
            k["origin"] = o
            f.write_text(json.dumps(k))

        write(origin)
        try:
            yield
        finally:
            write(was)

    def raw(self, method, path, body=None, approve=True, node=None):
        """Any route over the bound session; a `412 CONFIRM_REQUIRED` answered with the
        passkey from the frontend's origin when `approve`."""
        n = node if node is not None else self.p.order()[0]
        args = ["raw", method, path] + ([json.dumps(body)] if body is not None else [])
        if approve and method.lower() != "get":
            args += ["--dev-passkey", str(self.p.passkey)]
        with self._on_origin(self.origin(n)):
            self.p.w.fleet.cli(n, "login", "--dev-passkey", str(self.p.passkey), home=self.home)
            return self.p.w.fleet.cli(n, *args, home=self.home)


def person(w, name) -> Person:
    people = w.shared.setdefault("deep_people", {})
    if name not in people:
        people[name] = Person(w, w.account(name))
    return people[name]


def people(w, *names):
    return [person(w, n) for n in names]


def hosts(w, app):
    """The running nodes that hold a replica of `app` (`GET /ops/placement`)."""
    for a in w.fleet.placement()["apps"]:
        if a["app"] == app:
            up = [n for n in a["replicas"] if n in w.members()]
            check(up, f"no running replica of {app}: {a['replicas']}")
            return up
    raise Failed(f"{app} is not placed")


def host(w, app):
    """A running replica of `app`: a few routes (`/apps/{app}/xcalls`) answer only on one,
    `503 UNAVAILABLE` elsewhere."""
    return hosts(w, app)[0]


def anyone(w, method, path, body=None, node=None):
    """A request with no credential."""
    node = node if node is not None else w.members()[0]
    return _http(method, w.fleet.url(node) + path, body, None, 30)


def ops(w, method, path, body=None, node=None):
    node = node if node is not None else w.members()[0]
    return _http(method, w.fleet.url(node) + path, body, w.fleet.ops_token, 30)


def _http(method, url, body, bearer, timeout):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    if bearer:
        req.add_header("authorization", f"Bearer {bearer}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            b = json.loads(raw)
        except ValueError:
            b = raw.decode(errors="replace")
        raise HttpError(e.code, b) from None


def _transport(e):
    """meroctl got no answer from the node (refused, reset, timed out): not a refusal."""
    s = e.stderr.lower()
    return any(t in s for t in ("connection refused", "error sending request", "timed out", "connection reset",
                                "tcp connect"))


def events(p: Person, app, frm, count, node=None, timeout=10.0):
    """Up to `count` events of `app` from index `frm` (`GET /apps/{app}/events`, server-sent)."""
    n = p.order(node)[0]
    req = urllib.request.Request(p.w.fleet.url(n) + f"apps/{app}/events?from={frm}")
    req.add_header("authorization", f"Bearer {p.key}")
    out = []
    deadline = time.time() + timeout
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            for raw in r:
                line = raw.decode(errors="replace").rstrip("\r\n")
                if line.startswith("data:"):
                    data = line[5:].strip()
                    try:
                        out.append(json.loads(data))
                    except ValueError:
                        out.append(data)
                    if len(out) >= count:
                        break
                if time.time() > deadline:
                    break
    except (socket.timeout, TimeoutError):
        pass
    except urllib.error.HTTPError as e:
        raise HttpError(e.code, e.read().decode(errors="replace")) from None
    return out


# ------------------------------------------------------------------ fixtures


def fixture(w, name) -> Path:
    """A file `scripts/build-upgrade-deep-fixtures.sh` built from the old release's own
    source beside the harness's fixtures (`<from-dir>/deep/`)."""
    p = Path(w.fixtures["kv_from"]).parent / "deep" / name
    check(p.exists(), f"{p} is missing: run scripts/build-upgrade-deep-fixtures.sh first")
    return p


def scratch(w, name) -> Path:
    d = Path(w.fleet.workdir) / "deep" / name
    d.mkdir(parents=True, exist_ok=True)
    return d


def kv_wasm(w) -> Path:
    return fixture(w, "kv-dev.wasm")


def install(p: Person, w, name):
    """Publish the signed package `name` (a deep fixture) and install it: `(package, version)`."""
    package, version = w.registry.publish(fixture(w, name))
    p.raw("post", "packages/install", {"package": package, "version": version})
    return package, version


def new_org(p: Person, name):
    return p.raw("post", "orgs", {"name": name})["orgId"]


def invite(owner: Person, org, role="member", teams=(), **extra):
    return owner.raw("post", f"orgs/{org}/invites", {"role": role, "teams": list(teams), **extra})["token"]


def join(owner: Person, org, *who, role="member", teams=()):
    for p in who:
        p.raw("post", f"invites/{invite(owner, org, role, teams)}/redeem", {})


def team_id(v):
    return v.get("teamId") or v.get("id") or v.get("team")


def roles(p: Person, org):
    """`{account: role}` of an organization as `p` reads it."""
    m = p.get(f"orgs/{org}").get("members")
    if isinstance(m, dict):
        return {k: (v if isinstance(v, str) else v.get("role")) for k, v in m.items()}
    return {x["account"]: x["role"] for x in m}


# ------------------------------------------------------------------ the matrix


def matrix() -> str:
    upgrade_path.load_cases(HERE / "upgrade_cases")
    rows = ["| Area | Case | Seeded on the old release | Checked in the mixed cluster, after the roll and after a restart | Covered |",
            "| --- | --- | --- | --- | --- |"]
    for cls in upgrade_path.CASES:
        c = cls()
        if isinstance(c, DeepCase):
            rows.append(f"| {c.area} | `{c.name}` | {c.seeds} | {c.verifies} | yes (since {c.since}) |")
        else:
            doc = " ".join((cls.__doc__ or "").split())
            rows.append(f"| Representative | `{c.name}` | {doc} | read back, extended, read again | yes |")
    for area, what, why in NOT_COVERED:
        rows.append(f"| {area} | {what} | - | - | no: {why} |")
    return "\n".join(rows)


def check_matrix(doc: Path) -> int:
    text = doc.read_text()
    if MATRIX_BEGIN not in text or MATRIX_END not in text:
        print(f"{doc}: no upgrade coverage matrix markers", file=sys.stderr)
        return 1
    have = text.split(MATRIX_BEGIN, 1)[1].split(MATRIX_END, 1)[0].strip()
    if have != matrix().strip():
        print(f"{doc}: the upgrade coverage matrix differs from the cases; paste the output of "
              f"`python3 -I crates/node/tests/fault/upgrade_deep.py --matrix` between its markers", file=sys.stderr)
        return 1
    print(f"{doc}: the upgrade coverage matrix matches the cases")
    return 0


if __name__ == "__main__":
    import argparse

    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--matrix", action="store_true", help="print the coverage matrix (markdown)")
    g.add_argument("--check-matrix", metavar="DOC", help="fail when DOC's copy of the matrix is stale")
    a = ap.parse_args()
    if a.matrix:
        print(matrix())
        raise SystemExit(0)
    raise SystemExit(check_matrix(Path(a.check_matrix)))
