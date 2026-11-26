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
import functools
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
import ed25519_sign  # noqa: E402
import upgrade_path  # noqa: E402
from rollout import OPERATOR_SEED  # noqa: E402
from rv2 import ORIGIN, CliError, HttpError  # noqa: E402
from upgrade_path import Case, Failed  # noqa: E402

ROOT = HERE.parents[3]
MATRIX_BEGIN = "{/* upgrade-matrix:begin: python3 -I crates/node/tests/fault/upgrade_deep.py --matrix */}"
MATRIX_END = "{/* upgrade-matrix:end */}"

# What no case seeds, and why: (area, functionality, reason). A functionality
# that cannot be tested yet is listed here, never skipped silently.
NOT_COVERED = [
    ("Packages", "A passkey sign-in completed from a registered origin (a session bound to the package and naming it)",
     "the harness's passkeys are the dev passkey's, which holds a credential under the cluster's rp-id only, and an "
     "origin's own rp-id needs a credential made for that site in a browser; `package-origins` covers the begin "
     "(the rp-id each registered origin gets and the refusal of any other), and the finish on a frontend the node "
     "serves is `Bound`'s (the packages' own frontends)"),
    ("Governance", "The egress switch (`POST /orgs/{org}/egress`, `POST /apps/{app}/egress`; `Org::egress_off`, "
     "`AppRecord::egress_off`, `SetEgress`)",
     "it ships as a hard cutover: `SetEgress` is ungated and a node of an earlier release cannot decode it, so a "
     "cluster is recreated rather than rolled, and no release before it has the switch to seed. "
     "`tests/it/egress.rs` covers the switch, its refusals and the consent page on one build"),
    ("Apps", "Only an owner or admin of an app's organization approves its upgrades, runs and upgrade keys "
     "(`SystemState::approves_upgrades`, `owner_approves` on `UpgradeApp`, `StartUpgradeRun` and `CreateApiKey`)",
     "it changes who may, not a stored shape: an entry without `owner_approves` reads and replays as it applied "
     "below cluster version 1.0.0-rc.33 (`Feature::OwnerApprovedUpgrades`), and the owner's upgrade after the "
     "roll is already the `kv-package` case's (`extend`: the organization's owner upgrades on the new release). "
     "`system::tests::only_an_organizations_owner_and_admins_approve_its_upgrades` covers the replay, and "
     "`a_member_needs_the_capability_to_create_apps_or_invite`, `the_leader_checks_upgrade_authority_again` and "
     "`an_upgrade_key_is_an_owners_or_admins_of_the_apps_organization` the refusals on one build"),
    ("Deployments", "A deployment's customer accepting its offer on the consent page",
     "the page is approved in a browser, and a consent request lives in one node's memory for minutes; what it "
     "logs is the `UpgradeApp` that `POST /apps/{app}/upgrade` without a version logs, which "
     "`deployment-offer-upgrades` covers with `GET /apps/{app}/update`'s `source: deployment`. "
     "`a_customer_accepts_its_deployments_offer_on_the_page` (`consent_page.rs`) covers the page on one build"),
    ("Apps", "A green update applying on its own after its hold (`UpgradeApp::auto`, `lastUpgrade.auto`)",
     "the hold is at least a day (`MIN_HOLD_HOURS`) of logged time from the release's install, which a deep run "
     "cannot wait on either release (the tests move the update clock with a testing hook a release `merod` "
     "lacks), so no automatic update is seeded or made; the policy that asks for one is the "
     "`upgrade-policies` case's, and the restore point every upgrade takes and its undo are "
     "`restore-points-undo`'s. An upgrade entry and a record without `auto` read and replay as before. "
     "`system::tests` (`a_green_update_applies_on_its_own_after_its_hold_under_the_policy`, "
     "`a_deployment_instances_hold_runs_from_its_offer`) and `tests/auto_updates.rs` cover it on one build"),
    ("Apps", "An undo refused to a release no longer installed (`UndoUpgrade::release_installed`, `UNDO_RELEASE_GONE`)",
     "it adds a refusal, not a stored shape: an entry without the flag replays as it applied below cluster "
     "version 1.0.0-rc.36 (`Feature::UndoToInstalledRelease`), and the undo itself is `restore-points-undo`'s. "
     "The refusal needs a release uninstalled while another release's WASM keeps the bundle, which the case's "
     "single-WASM releases do not make. `system::tests::an_undo_never_returns_to_a_release_that_is_no_longer_installed` "
     "and `tests/auto_updates.rs::an_undo_is_refused_to_a_release_that_was_uninstalled` cover it on one build"),
    ("Apps", "An instance's outbound audit (`GET /apps/{app}/outbound-log`, `meroctl instance outbound-log`)",
     "it stores nothing: each node keeps its runs' requests in memory (`calimero_outbound::OutboundAudit`), which "
     "a restart or a roll empties by design, so there is no data to carry across releases; the suite's clusters "
     "also run with outbound off. `an_instances_owner_reads_its_outbound_requests_without_values` "
     "(`component_cluster.rs`) and `calimero_outbound`'s audit tests cover it on one build"),
    ("Apps", "Host work charged to fuel, and at most `MAX_SEARCHES_PER_RUN` searches per request (`calimero_app_host`'s fuel table)",
     "it changes what a run may spend, not a stored shape: followers apply a call's diff and never run it, so "
     "the log replays alike on either release, and a search cap answers a view, which is never logged. The "
     "deep cases' calls and searches run far within the default fuel on both releases. `tests/engine.rs` "
     "(`reading_megabytes_of_state_costs_fuel`, `a_view_makes_at_most_max_searches_per_run`, "
     "`a_guest_cannot_spend_fuel_its_host_calls_already_owe`, `refused_xcall_arguments_cost_fuel`, "
     "`stdout_costs_fuel_after_the_log_is_full`) covers it on one build"),
    ("Keys", "An API key made with follow-ups (`POST /auth/apikeys { then }`): confirm calls approved by the "
     "key's one passkey prompt",
     "it adds no stored shape: the key's record is unchanged and each follow-up is an ordinary call entry of its "
     "app with the key's consent; `CreateApiKey::then` is new in the log only behind cluster version "
     "1.0.0-rc.38 (`Feature::KeyFollowUps`), and an entry without it replays as before. "
     "`tests/it/key_follow_ups.rs` and `system::tests::credentials_need_the_accounts_own_passkey` cover it on one build"),
    ("Accounts", "Account recovery started and finished with a new passkey",
     "the start takes a WebAuthn registration made outside a session, which neither meroctl nor the "
     "suite's standard-library Python can make (P-256); the cluster tests cover it on one build. The "
     "recovery code (`account-recovery-code`) is covered"),
    ("Accounts", "Finishing a sign-up with a cluster sign-up invite (`register/finish { invite }`, which spends a use)",
     "the finish carries a WebAuthn registration made by a new P-256 passkey, which the suite's standard-library "
     "Python cannot make; meroctl's `login --dev-passkey` registers a new file but takes no invite, and the "
     "release's meroctl is the one that seeds. Issuing, listing and revoking an invite and the node's refusal or "
     "acceptance of a token at `register/begin` are covered (`sign-up-invites`); "
     "`tests/sign_up.rs` covers the finish and the spent use on one build"),
    ("Accounts", ("Consent-page approvals (`/consent/requests`: release, upgrade and outbound approvals; the "
                  "organization kinds `createOrg`, `createInvite`, `acceptInvite`, `leaveOrg`, `removeMember`, "
                  "`setMemberRole`, `setMemberTeams`, `removeMemberTeams`, `revokeInvite`, `renameOrg`, "
                  "`deleteOrg`, `setCapabilities`, `setAllApps`, `setEgress`) and the page's verifier link"),
     "a consent request is approved in a browser on the node's consent page; the cluster test "
     "`consent_page.rs` covers every kind on one build. What an approval writes is the same log entry as "
     "the API's route, which the organization and team cases cover; a `createOrg` asked from an app's "
     "frontend also makes that app's team in the same `CreateOrg`, the organization and team `ScopeApps` "
     "makes, which `app-teams` covers. The verifier link is a startup setting "
     "(`--attestation-verifier-url`) that stores nothing"),
    ("Apps", "Timers (`#[app::every]`) firing across the roll",
     "the only timer the suite's apps carry is the kv fixture's `beat`, declared `#[app::every(\"1h\")]` and run on "
     "the nodes' wall clock: one tick needs an hour, and a roll takes minutes, so no tick falls in any phase and "
     "there is nothing to seed or read. The tests move the app clock with a test-only clock "
     "that a release `merod` lacks, and no flag or route moves it; `ticks_run_once_per_slot`, "
     "`a_tick_runs_once_across_failover` and `missed_ticks_collapse_into_one` (`component_cluster.rs`) cover ticks "
     "on one build"),
    ("Packages", "Served frontends and served origins (custom domains)",
     "the suite's packages carry no frontend, and the node routes a frontend by Host under the apps "
     "domain, which plain HTTP on 127.0.0.1 does not reach; the cluster test "
     "`app_frontends_are_served_and_their_sessions_are_bound` covers serving"),
    ("Governance", "A release deadline that passes while a cluster rolls (`SystemState::retired`: the old nodes are "
     "placed no new app mid-roll)",
     "a deadline is judged at the command's own time, so the granted run sets a passed one on the old nodes' own "
     "release, which retires no node; one on the new release would stop placement on the old nodes before the roll "
     "and needs a deadline timed to the rollout. `a_release_deadline_stops_new_work_but_not_running_apps` "
     "(`cluster.rs`) covers retired nodes on one build"),
    ("Pipelines", "Live pipeline events (`meroctl cloud compute pipeline watch`)",
     "the stream is many lines and the suite's meroctl runner reads one JSON answer; a stranger's refusal "
     "is covered, the stream itself by the cluster tests"),
    ("Apps", "The consent page's install sheet (`install`), update sheets with their class, policy and "
     "approval step (\"Approve (1 of 2)\", \"Approve and update (2 of 2)\"), an update policy changed on the page "
     "(`updatePolicy`), the node's updates page (`updates`, `/updates/{id}`, `POST /consent/{id}/open`) and "
     "`GET /apps/{app}/update`'s `summary`",
     "pages and reads that store nothing of their own: a consent request lives in one node's memory for minutes, "
     "and what the pages approve is logged by the routes they run (`POST /apps`, `SetUpgradePolicy`, "
     "`ApproveUpgrade`, `UpgradeApp`), whose stored shapes are the `upgrade-policies` and "
     "`update-classes-approvals` cases'. "
     "`an_install_sheet_shows_what_gets_created_and_where_data_can_go` and "
     "`an_update_shows_its_class_and_takes_two_approvers_on_the_pages` (`consent_page.rs`) cover them on one build"),
    ("Deployments", "A customer's instance outside every deployment on a granted cluster, refused at create and "
     "on an upgrade into a deployer's package (`403 DEPLOY_NOT_GRANTED`; `CreateApp { offers_only }` and "
     "`UpgradeApp` at apply, `Feature::OffersOnly`)",
     "a refusal that stores nothing, on a granted cluster, which the open upgrade-path cluster is not and cannot "
     "become without turning every case that installs after the switch red; an entry without `offers_only` "
     "replays as it applied below cluster version 1.0.0-rc.36. "
     "`on_a_granted_cluster_a_customer_creates_only_from_an_offer` (`deployments.rs`) and "
     "`system::tests::on_a_granted_cluster_an_instance_outside_every_deployment_is_a_deployers` and "
     "`..._keeps_out_of_a_deployers_package` (the replay) cover it on one build"),
]


class DeepCase(Case):
    area = ""
    since = "1.0.0-rc.15"
    # The first release that no longer has what the case seeds (a release from it on
    # cannot make that data, so a roll from it carries none): `None` while every
    # release still makes it. Rolls from a release before it run the case as usual.
    until = None
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
    def _retired(self, w):
        return self.until is not None and release_key(w.fixtures["from_version"]) >= release_key(self.until)

    def seed(self, w):
        self.s = None
        self.seed_error = None
        if self._retired(w):
            w.note(f"{self.name}: {w.fixtures['from_version']} no longer makes what it seeds (until {self.until}); skipped")
            return
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
        if self._retired(w):
            return
        if self._seeded():
            self.during_mix(w, self.s)

    def old_job(self, w):
        if self._retired(w):
            return
        if self._seeded():
            self.with_old_job(w, self.s)

    def with_old_job(self, w, s: dict):
        pass

    def verify(self, w):
        if self._retired(w):
            return
        if self._seeded():
            self.check(w, self.s)

    def extend(self, w):
        if self._retired(w):
            return
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


@functools.cache
def cut_fields() -> frozenset:
    """Answer fields a hard cutover removed from the API (`[[cutover]] field = ...` in
    `crates/node/api-deprecations.toml`): a release before it answered them, this one may not."""
    import tomllib

    doc = tomllib.loads((ROOT / "crates/node/api-deprecations.toml").read_text())
    return frozenset(c["field"] for c in doc.get("cutover", []) if "field" in c)


def subset(got, want, what, path="$"):
    """`got` holds every key `want` holds, with the same value; a list holds the
    same items in the same order, each compared the same way: a new release may
    add fields (to an object, or to every object in a list), never change one or
    drop one a hard cutover did not remove (`cut_fields`), nor add, drop or
    reorder a list's items."""
    if isinstance(want, dict):
        check(isinstance(got, dict), f"{what}: {path} is {_short(got)}, want an object")
        for k, v in want.items():
            if k not in got and k in cut_fields():
                continue
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
        args = ["instance", "create", "--name", name, "--path", str(wasm)]
        if org:
            args += ["--org", org]
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


def ops_text(w, path, node):
    """A plain-text ops route (`ops/metrics`) as `node` answers it."""
    req = urllib.request.Request(w.fleet.url(node) + path, headers={"authorization": f"Bearer {w.fleet.ops_token}"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return r.read().decode()


def signed_ops(w, method, path, body=None):
    """An operator decision over the ops route: the node answers `428 OPERATOR_SIGNATURE_REQUIRED`
    with the bytes to sign, and the retry carries the operator key's signature over them."""
    try:
        return ops(w, method, path, body)
    except HttpError as e:
        if e.status != 428 or not isinstance(e.body, dict):
            raise
        details = e.body["details"]
    seed = bytes.fromhex(OPERATOR_SEED)
    sig = ed25519_sign.sign(seed, base64.b64decode(details["message"])).hex()
    return ops(w, method, path, {**(body or {}), "operatorProof": {"nonce": details["nonce"], "signature": sig}})


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
            run = "" if len(c.governance) > 1 else f", {c.governance[0]} run only"
            rows.append(f"| {c.area} | `{c.name}` | {c.seeds} | {c.verifies} | yes (since {c.since}{run}) |")
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
