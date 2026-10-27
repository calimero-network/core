# calimero-cloud-node (the inside of `merod`)

The release-v2 cloud node: an attested TEE node that hosts the cluster's system
group and the app groups placed on it. Design: the "Calimero Confidential Cloud"
architecture spec. This crate is the library; the `merod` binary
(`crates/merod`: `init`, `run`, `config`, `local`, `inspect`, `kms`) turns its
configuration into a `Config` and calls `run`, and `meroctl` (`crates/meroctl`)
is its command line.

## Mental model

- **One Raft log per app** (`calimero-log`). The leader runs the WASM call
  (`app::Executor`, today's `calimero-runtime` over a copy-on-write `Overlay`) and
  appends `{ call, outcome { result, diff, events } }`. Followers apply the diff;
  nothing is re-executed. `base_index` guards against applying a stale diff.
- **System group** (`system.rs`, group 1): fleet, app directory, bundles, accounts,
  passkeys, API keys, refresh tokens, session keys, organizations, teams, invites.
  Commands carry their `actor`; `apply` checks authority, so the log decides.
- **Routing** (`node.rs`): system writes go to the system leader, calls and strong
  views to the app's leader, over **peer sealed channels** (`internal/*`) whose key
  is attested like any client's, plus the cluster secret (HMAC of K_r).
- **API** (`api.rs`), all inside the sealed channel except `/attestation` and `/health`:
  `/auth/*` (passkeys, refresh, API keys), `/bundles`, `/apps`, `/apps/{app}/{intents,views,events,upgrade,access}`,
  `/orgs/*`, `/invites/{token}/redeem`, `/internal/*` (peers), `/ops/*` (mdma: counts only, no read path).
- **Reserved methods** (`node::reserved_method`, `__*`): the node's own commands
  (`HOST_METHODS`: `__upgrade`, `__blob`, `__blob_forget`) run only for a
  `CallRequest::host` request, which only the API builds after judging who may;
  `/intents` and `/views` refuse a `__` method (`400 RESERVED_METHOD`), and an
  xcall or follow-up naming one is refused at the leader. Regression:
  `the_nodes_own_commands_cannot_be_called_as_methods`.
- **Ceremonies** (`api::Pending`): a challenge lives on the node that began it, its id `{node}.{random}`; finishing elsewhere takes it from that node (`internal/challenge/take`), so it is used once. Regression: `a_sign_in_begun_on_one_node_finishes_on_another`.
- **Confirm methods**: `intents` answers `412 CONFIRM_REQUIRED` with a challenge over
  the exact call; the retry carries a passkey assertion, which is logged as consent.
- **Bundles are blobs, not log entries**: `POST /bundles` stores the WASM in every
  node's sealed blob store (`internal/blob`, majority required), then the system log
  records only `{hash, size}`. A node missing a blob fetches it from a peer
  (`internal/blob/get`) on first use. Log entries are capped at
  `calimero_log::MAX_ENTRY_BYTES` (1 MiB): openraft gives each append one heartbeat.
- **Packages** (`packages.rs`, `system::PackageRecord`): signed `.mpk` releases, v1's
  install model. `POST /packages/install { package, version }` fetches from the
  operator's registry (`--registry-url`, `calimero-app-downloader`'s `HttpRegistry`);
  `POST /packages` takes an uploaded file. Before `InstallPackage` is logged the
  node checks the publisher signature and every artifact digest
  (`calimero_bundle::VerifiedBundle`), `minRuntimeVersion` (build.rs's
  `CALIMERO_RELEASE_VERSION`), the package's pinned signer (first release wins,
  `apply` re-checks) and that each service's WASM compiles. The log keeps the
  `.mpk`'s hash; a node that runs a release it does not hold fetches it itself
  (`Node::release_wasm`: the registry for a registry release, never a peer; a peer
  for an upload), refuses bytes whose hash differs, and verifies them again.
  `POST /apps { package, version?, service? }` creates from a release (installing
  it from the registry first); upgrade takes `{ version }`; `DELETE /apps/{app}`;
  `DELETE /packages/{p}[/{v}]` is refused while an app runs the release (a WASM
  another release ships stays). Raw `POST /bundles` is development only
  (`allow_unsigned_bundles`, on with `--dev`).
- **Governance** (`system::Governance`, `Authority`, shared with clients in
  `calimero_cloud_client::attest`): three decisions, each held by exactly one
  party chosen at deployment, `owner` (default) or `operator`:
  `--release-approvals` (which mero-tee releases may hold the data),
  `--app-upgrades`, `--outbound-approvals`. The first leader writes
  `SystemCmd::Genesis` before any account exists (`CreateAccount` waits for it);
  a second genesis is refused, so a restart with other flags changes nothing.
  Every receipt-key quote binds it (`receipt_key_binding(key, governance)`,
  `AttestationDoc::governance`), so `verify_app_cluster` proves it. Operator
  side: `POST /ops/releases` (`operator_releases`, which `approved_releases`
  then answers), `POST /ops/apps/{app}/upgrade` (an installed release;
  `UpgradeApp { by_operator }`), `POST /ops/outbound` (`operator_outbound`,
  `ReleaseScope::Cluster`). The operator installs the releases it upgrades
  to: `POST /ops/packages` (an `.mpk`), `/ops/packages/install` (the
  registry), `GET /ops/packages`, `DELETE /ops/packages/{p}[/{v}]` (only what
  it installed), recorded as `system::OPERATOR`, which no account can be and
  which no account quota counts. Owner side: the passkey routes below. Whichever
  party does not hold a decision gets 403 (API) and a refusal (`apply`).
- **Owner approvals** (where the owner holds the decision; `system::ReleaseScope`, `Org/Account::approved_releases`): the
  mero-tee releases that may hold an org's (or an account's own) apps; the one it
  began on is approved at creation, others by the owner with a passkey assertion
  (`POST /orgs/{org}/releases`, `/auth/releases`, logged as consent). Enforced by the
  replicas that hold the data: `Node::may_send` refuses Raft entries/snapshots to a
  peer whose attested release (`HttpAttestor::seen_release`) is not approved,
  `Node::place` skips such nodes, `/ops/apps/{app}/replicas` refuses them. Upgrades
  are the app's owner's, or, for an org app, of whoever holds `MANAGE_APPLICATION`
  on its team or org-wide (`SystemState::manages_apps`), with their own passkey
  assertion over the exact upgrade (`consent` on `UpgradeApp`).
- **Outbound HTTPS** (`calimero-outbound`, v1's gate): `Config::outbound` from
  `--settings` (`[outbound]`). `Node::outbound_for` hands a client only to
  `call_local`'s runs (calls, follow-ups, timers; not a read-only member's), for
  a bundle that is a signed release declaring prefixes; identity = (package,
  pinned signer), approval = `Org/Account::outbound_approvals[signer]`, set by
  the account, or for an org whoever holds `MANAGE_APPLICATION` org-wide, with
  `SetOutboundApproval` (`POST /orgs/{org}/outbound`,
  `/auth/outbound`, passkey consent; `merge_approval` at apply). The request is
  made inside the propose lock, on its own thread (`run_blocking`); a leader-race
  re-run sends it again.
- **Receipts**: every `/intents` and `/views` answer carries a `Receipt` signed by the
  answering node's receipt key (Ed25519, in memory, made at start), over
  `(app, index, SHA-256(JCS(result)), node)`. The key is attested on its own
  (`GET /attestation?purpose=receipts`, report data `nonce || receipt_key_binding`),
  and `GET /apps/{app}/attestation` gathers every replica's
  (`Client::verify_app_cluster`).
- **The host's key space** (`app::RESERVED`, `\0calimero.*`): private spaces, writer
  sets, blob marks. App code's overlay answers nothing there and a run that tries is
  refused; the host writes with `Overlay::set_host` and reads with
  `Storage::host_get`. Rotations follow v1's host rules (`rotation_refusal`).
- **Access is judged fresh**: every request carrying a credential, every gated request and every client call's leader
  read the system state through `sys_fresh` (a read index), so a removal committed
  elsewhere holds before the next request.
- **Joining nodes**: a node started without `--bootstrap` waits; once
  `/ops/nodes` registers it, the system leader adds it to the system group as a
  learner (`Node::admit_registered_nodes`, from `reconcile`), it catches up and
  learns the directory, and replica moves (`/ops/apps/{app}/replicas`) can put
  apps on it.
- **Removing nodes**: `DELETE /ops/nodes/{id}` (system leader; `421 NOT_LEADER` elsewhere) writes `RemoveNode` (refused while an app keeps a replica there) and changes the system group's voters, promoting a registered learner while fewer than 3 are left; `SystemState::removed_nodes` keeps `reconcile` from registering it again from `--peers`. Regression: `a_node_is_removed_once_no_app_keeps_a_replica_on_it`.
- **Transfers**: `POST /apps/{app}/transfer { to }` (owner, passkey consent; an org app only to a member). Regression: `an_owner_transfers_an_app_with_their_passkey`.
- **Ops credentials and metrics**: `--ops-token-sha256` gives the node only the
  token's hash (machine metadata then holds nothing usable); `GET /ops/metrics`
  is Prometheus text (directory counts, this node's groups, streams, system
  membership, build release), never names or state. `--metrics-listen` (`Config::metrics_listen`) serves the same text
  at plain-HTTP `GET /metrics` for a scraper, nothing else (`api::plain_metrics`).
- **Ops replica moves** run on the app group's leader; any other replica answers
  `421 NOT_LEADER` with `details.leaderUrl` before writing anything.
- **Forwards are bounded**: a call or read forwarded to an app's leader waits at most
  `FORWARD_TIMEOUT` (5 s, `peer_forward`), then retries at whoever leads now; a frozen
  leader would otherwise hold a write for the 30 s HTTP timeout. Calls without a
  client key get a node key (`node:…`, kept 10 min) first, so a retry replays a call
  that did commit. System-log forwards keep the long timeout: some system commands
  are not idempotent yet. Regression: merobox `workflow-examples/cloud-v2/frozen-leader.yml`.
- **Leader races**: a call whose diff was built on replaced state applies as a no-op
  (`conflict`, no idempotency record); `Node::call` re-runs it instead of failing.
- **Upgrades plan their migration from the ABIs** (`services::plan_upgrade`, v1's
  `plan_upgrade`): without a named `migrate`, an upgrade one state version up runs
  the target's `#[app::migrate]` edge; a downgrade, a jump or a missing edge is
  refused, and so is a target whose ABI strips authorship from a field
  (`calimero_wasm_abi::downgrade::identity_downgrades`, v1's L1 gate; regression:
  `an_upgrade_that_strips_authorship_is_refused`). Bundles without an ABI upgrade code-only. The migration runs as v1's
  does (`Executor::migrate`): the export returns the new root state, the app's
  `__calimero_migration_check` must pass, and the host writes the root entry
  (`write_root_state`). The directory records the new bundle only once the
  upgrade succeeded.
- **Upgrade runs** (v1's group upgrade, cascade, status, retry): `POST
  /orgs/{org}/upgrade { package, version, team?, cascade? }` (the org owner, one
  passkey assertion for the run) or `POST /ops/orgs/{org}/upgrade` (where the
  operator holds app upgrades) records a `types::UpgradeRun` in the org
  (`StartUpgradeRun`, newest `UPGRADE_RUNS_KEPT`), then upgrades its apps one by
  one in a background task; each `UpgradeApp`/`UpgradeFailed` notes the app on
  the run and on `AppRecord::last_upgrade`. `GET /orgs/{org}/upgrades/{run}`
  follows it; `.../retry` runs the failed apps again as a new run. A release
  missing from the cluster is installed from the registry first (owner runs).
- **Aliases** (v1's): `Account::aliases`, `PUT/GET/DELETE /auth/aliases/{alias}`
  and `GET /auth/aliases`; an alias names only an app its account reaches.
- **Metadata and org lifecycle** (v1's group/member/context metadata, device
  labels, owner-delete): `Org::metadata` (admins or `MANAGE_METADATA`),
  `Org::member_metadata` (the member or an admin), `AppRecord::metadata` (owner
  or org admin), each at most `MAX_METADATA_BYTES` (16 KiB, team metadata too);
  `Account::passkey_labels` (`POST /auth/passkeys/{cred} { label }`).
  `DELETE /orgs/{org}` is the owner's, with a passkey assertion (412 first),
  and takes the org's teams, invites and apps. Ownership moves with
  `SetOrgRole { role: owner }`; leaving is `RemoveOrgMember` on oneself.
- **Devices**: a person is their account, and each passkey is a device: the
  session names the one it was opened with (`SessionClaims::passkey`,
  `passkeyId` in `/auth/me`); `auth/passkeys/add` adds one.
- **Init**: `POST /apps` takes `initArgs` (JSON) for the app's `init`, kept on the
  `AppRecord`; a failed init answers `422 INIT_FAILED` (`AppState::init_error`).
- **Identity inside the app**: `env::account_id` and `env::executor_id` are both the
  caller's account (no devices in release-v2); the context id is `SHA-256(app_id)`.
  `env::caller_org` is the caller's role and teams in the app's organization
  (`app::caller_org`, passed through `Module::run_cloud`).
- **Private storage** (`env::private_storage_*`): v1 kept it on the member's node,
  unsynced. Here it is the caller's own space in the app's state (`app::private_prefix`):
  replicated and sealed, but a run is handed only its caller's space.
- **Shared-storage writers**: a cell's rotated writer set is kept in the app's state
  (`app::writers_key`), written with the run that rotated it; v1 published governance ops.
- **Context ids**: an app answers to its context id (`hex(SHA-256(app_id))`, what the app
  sees as `env::context_id()`) wherever the API takes an app id (`Node::canonical_app_id`).
- **Blobs** (`env::blob_*`, `POST/GET /apps/{app}/blobs[/{id}]`): bytes live in the
  cluster's blob store (`hex(SHA-256)`, a majority before the answer, a missing one
  fetched from a peer), and the app's state marks which are its own
  (`app::blob_key`). A run's blob host is `services::AppBlobs`: a run writes blobs
  onto its outcome (never through the host), the leader stores them before
  proposing and spreads them after the entry commits; a run opens only blobs its
  app's state marks. An upload commits `AppCmd::Blob`. Ids are base58 (what apps
  print, v1's `BlobId`) or hex in the API. `GET /apps/{app}/blobs[/{id}/info]`
  list them from a scan of the app's state (`AppMachine::storage_stats`, on a
  replica). `DELETE /apps/{app}/blobs/{id}` (owner or org admin) commits
  `AppCmd::BlobForget`, then `Node::collect_blob` deletes the bytes on every
  node (`internal/blob/delete`) unless another app still marks the same content
  (asked of each app's leader, `blob_held_strong`: a lagging replica could miss
  a mark just committed; an app whose leader does not answer counts as holding
  it) or it is a bundle or a release `.mpk`. Known window: the same content
  uploaded to another app between that check and the deletion loses its bytes.
  A blob past `MAX_BLOB_BYTES` (47 MiB) is refused `413 TOO_LARGE` by the node,
  one past the sealed cap (64 MiB) by the envelope, in the clear; the client
  reports both as `TOO_LARGE`. Regressions:
  `a_blob_too_large_to_reach_the_other_nodes_is_refused_up_front`,
  `an_upload_past_the_sealed_cap_is_answered_413`.
- **App frontends** (`app_host.rs`, v1's app hosts): with `--apps-domain D`
  (`merod local`: `localhost`), each package's frontend is at
  `{label}.D` (`app_host::label`, 52 base32 characters), from its release's
  `.mpk` (`Node::release_mpk`, verified before a byte is served, opened ones
  cached). Outermost layer: only `GET`/`HEAD` of files, `/` → newest release,
  client routes and relative tails as v1, v1's CSP plus the cluster's API
  origins in `connect-src`, `/.well-known/calimero.json`; an unknown label under
  D is a 404, never a node route. App origins pass CORS, and a passkey ceremony
  from one opens a session bound to its package (`SessionClaims::app`, carried
  by refresh): `Principal::within` keeps it to that package's apps (gate, app
  reads and writes, listings), `unbound`/`holder` refuse it credentials, API
  keys, approvals, upgrades, uninstalls and uploads, and it creates instances
  of its own package only. Its step-ups come from its own origin only.
- **Presence** (`presence.rs`, v1's ephemeral awareness channel): in memory on
  every node, never in a log. `POST /apps/{app}/presence { seq, state }` (anyone
  who reaches the app; one entry per account and device) applies it here and
  passes it to every other node (`internal/presence`); `GET .../presence` and
  `GET .../presence/stream` (SSE: `snapshot`, then `presence` diffs). v1's rules:
  newest seq wins, a retract is a tombstone, a heartbeat (same seq and state)
  refreshes and is passed on too, `PRESENCE_TTL_MS` (7 s) sweep each second,
  `MAX_AUTHORS_PER_APP` (512, the stalest evicted), 4 KiB of state.
- **Inspection** (v1's context storage, application ABI, usage):
  `GET /apps/{app}/storage` (keys, bytes, blobs), `GET /apps/{app}/abi` (the
  bundle's embedded ABI), `GET /auth/usage` (the account's apps, bundle bytes,
  API keys, owned orgs, and the quotas).
- **Search** (`services::Search`): v1's per-context tantivy indexes, in memory per
  app and built from the app's `__calimero_search_*` exports. A view brings them
  up to the very state it reads first: from the entities entries touched
  (`Outcome::touched`, from the run's `StorageDelta`; kept for the last
  `TOUCHED_RING` entries), else a full scan. Derived, never replicated.
- **Follow-ups** (`AppState::pending`): an entry's event handlers
  (`app::emit!((e, "handler"))`, `tee:` for a TEE handler) and its `xcall`s are
  queued when it applies; the leader runs them (`Node::drive`, woken by `kick`)
  as entries that consume them (`Trigger::FollowUp`), or, for an xcall, calls
  the target with an idempotency key and then commits `AppCmd::Done`. A new
  leader neither skips nor repeats one. v1's rules hold: only `#[app::handler]`
  methods run for an event; an xcall reaches only `#[app::xcall]` entry points
  (`from_same_app`: the same bundle), only within the caller's organization
  (outside one: its owner), as `Caller::App` with `env::xcall_origin`; cascades
  stop at `MAX_FOLLOWUP_DEPTH`. Handlers and xcalls run once, not per replica.
- **Timers and the TEE**: the cluster is every app's TEE. The leader fires each
  `#[app::tee(every = "..")]` method once per period (`Trigger::Timer`, the
  last tick per method in `AppState::timer_ticks`). Such a run and a `tee:`
  handler author as `AccountId::TEE_AUTHORITY` with `tee_trigger`, open and
  seal with the app's TEE key (`backup::tee_key`, from K_r), and seal a
  member's share to the key the node keeps for them (`backup::member_key`):
  their account is their one device (`SealingContext::stand_ins`), and only
  their own runs open with it. ABI facts (timers, handlers, xcall entry
  points) come from the bundle's embedded ABI (`services::BundleFacts`), so
  apps are built with `cargo mero build`; a bundle without one gets none.
- **Teams are v1's subgroups** (`types::Org`): nested (`parent`, moved with
  `UpdateTeam`), open (`TeamJoin::Everyone`: whoever reaches the parent, the
  organization at the top, is a member, unless denied `caps::JOIN_OPEN_TEAMS`)
  or restricted (direct rows only). A direct row has a role (`TeamRole`: lead,
  member, read-only) and capabilities (`caps`, v1's `MemberCapabilities` bits;
  the organization keeps a default). An app deployed with `access: team` is
  reached by the team's members; a member whose direct row there is read-only
  runs calls, but one that would write is refused (`403`, `node::READ_ONLY`;
  v1 discarded the writes). A team's creator leads it. Below admin, creating an app in the organization needs `caps::CREATE_APP` (on its team, or org-wide) and minting an invite `caps::INVITE_MEMBERS` (org-wide, or on each team it names); neither is in `caps::DEFAULT`, as in v1, and a team's leads hold both there. `caps::MANAGE_APPLICATION` (v1's) lets a member upgrade the org's apps (on the app's team, or org-wide), start org upgrade runs, and approve the org's outbound hosts (org-wide grants only); a personal app stays its owner's (`SystemState::manages_apps`; regressions: `a_member_needs_the_capability_to_create_apps_or_invite`, `manage_application_lets_a_member_upgrade_and_approve_outbound`). A member who left, or
  whom a manager removed, does not come back by inheritance until a manager adds them again or an invite names the team (`Team::removed`; regression: `leaving_an_open_team_sticks_until_someone_lets_you_back_in`). Reads are filtered: a
  team one neither reaches nor manages is not listed (`GET /orgs/{org}`) and
  answers `404` (`GET /orgs/{org}/teams/{team}`). `DeleteTeam` takes the teams
  below it and their apps. Metadata (`POST /orgs/{org}/teams/{team}`) is the
  app's (a folder's name and colour).
- **App state on disk** (`app.rs`): `AppMachine` holds no state in memory but its
  small `Head` (bundle, pending follow-ups, timers, ring bounds). The app's keys are
  hidden rows of its group (`rows::KV`), idempotency records too (`rows::IDEM`, expired
  oldest first from a ring per TTL class), events a ring of `EVENT_RING` (`rows::EVENT`).
  Runs read through `StateRead` (`KvView` for the app, a `BTreeMap` in tests); the
  system group stays in memory (`Machine::IN_MEMORY`). Backups (`Node::backup`) copy the
  group's sealed snapshot file; `backup::latest` opens one with K_r.
- **Fresh system reads**: `Node::sys_fresh` is a read index (the leader names
  its applied index, `internal/system/index`; this replica waits for it). Org
  and team reads use it, and so does an access check before it refuses.
- **Declared views**: a call (`/intents`) of a method the ABI declares `#[app::view]`
  runs as a strong view: it writes nothing and may search (v1's rule).
- **Browsers**: `config.origins` are the CORS origins too (`lib.rs`, outermost layer).
- **Storage key** (`kms.rs`): K_r = HKDF(KMS key, cluster). A TDX node takes it from
  mero-kms only (`kms::key_source`: `--storage-key`/`--storage-key-file` need `--dev`),
  fetched by merod itself (`--kms-url --kms-identity`; `calimero-kms-client`:
  release-policy-verified KMS, key sealed to the TD). The boot script uses the same
  client for the data disk's key (`merod kms disk-key`). The cluster's nodes share
  the disk identity, so they share K_r.
- **Peers' quotes** are judged like a client's (`calimero_cloud_client::Measurements`):
  in TDX mode the signed mero-tee release the peer names, profiles from
  `--peer-profiles` (default: this node's `MERO_TEE_PROFILE`), floor `--peer-min-release`.
- **Session keys** rotate: the system leader adds K_s v+1 once the current key is
  `session_key_rotation_secs` old (default 6 h); the previous key keeps verifying.
- **Quotas** (`system::Quotas`, set with `POST /ops/quotas`) live in the system state and
  are checked at apply, so every node enforces the same limits; refusals are `429 QUOTA_EXCEEDED`.
- **Read-your-writes across nodes**: lookups of system objects (`Node::sys_until`) wait
  up to 2 s for the local replica to apply a write committed elsewhere.

## Not done yet (prototype)

rollback counter (proposed: a mero-kms epoch, restored nodes rejoin as learners),
keys per app from mero-kms, placement across zones, snapshot restore into a
running cluster, event audiences, search indexes that survive a restart (rebuilt from state now).

## Commands

```bash
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release   # test fixture
# fixtures whose ABI the node reads (handlers, timers, xcall, search, blobs):
for a in blobs kv-store-with-handlers search-chat xcall-example tee-cards tee-dice \
  migrations/migration-suite-v1 migrations/migration-suite-v2-add-field \
  migrations/scenario-migration-check-fail-v1 migrations/scenario-migration-check-fail-v2 \
  migrations/scenario-identity-downgrade-v1 migrations/scenario-identity-downgrade-v2; do
  cargo mero build --manifest-path apps/$a/Cargo.toml; done
cargo test -p calimero-cloud-node    # 3-node cluster in process, sealed, real WASM
# calls/s and p50/p99 through that cluster (harness: tests/harness/cluster.rs), as JSON:
cargo bench -p calimero-cloud-node --bench cluster_throughput -- --secs 15 --out cluster.json
cargo run -p merod -- --help
cargo run -p merod -- local   # dev cluster: 3 nodes on :2501-2503 (`local.rs`)
cargo run -p merod -- --node n1 inspect [--storage-key-file F]   # a stopped node's store (v1's merodb; `inspect.rs`)
```
