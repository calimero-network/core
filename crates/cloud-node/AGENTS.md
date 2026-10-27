# calimero-cloud-node (`merod-v2`)

The release-v2 cloud node: an attested TEE node that hosts the cluster's system
group and the app groups placed on it. Design: the "Calimero Confidential Cloud"
architecture spec.

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
- **Confirm methods**: `intents` answers `412 CONFIRM_REQUIRED` with a challenge over
  the exact call; the retry carries a passkey assertion, which is logged as consent.
- **Bundles are blobs, not log entries**: `POST /bundles` stores the WASM in every
  node's sealed blob store (`internal/blob`, majority required), then the system log
  records only `{hash, size}`. A node missing a blob fetches it from a peer
  (`internal/blob/get`) on first use. Log entries are capped at
  `calimero_log::MAX_ENTRY_BYTES` (1 MiB): openraft gives each append one heartbeat.
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
  refused. Bundles without an ABI upgrade code-only. The migration runs as v1's
  does (`Executor::migrate`): the export returns the new root state, the app's
  `__calimero_migration_check` must pass, and the host writes the root entry
  (`write_root_state`). The directory records the new bundle only once the
  upgrade succeeded.
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
  print, v1's `BlobId`) or hex in the API.
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
  v1 discarded the writes). A team's creator leads it. A member a manager
  removed does not come back by inheritance until added again (`Team::removed`). Reads are filtered: a
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
- **Storage key** (`kms.rs`): K_r = HKDF(KMS key, cluster). The KMS key comes from
  merod's own client (`merod kms disk-key`: release-policy-verified KMS, key sealed
  to the TD), run by merod-v2 (`--kms-url --kms-identity`) or by the boot script
  (`--storage-key-file`). The cluster's nodes share the disk identity, so they share K_r.
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

rollback counter, placement across zones, snapshot restore into a
running cluster, event audiences, pruning of revoked session ids, blob
deletion, search indexes that survive a restart (rebuilt from state now).

## Commands

```bash
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release   # test fixture
# fixtures whose ABI the node reads (handlers, timers, xcall, search, blobs):
for a in blobs kv-store-with-handlers search-chat xcall-example tee-cards tee-dice \
  migrations/migration-suite-v1 migrations/migration-suite-v2-add-field \
  migrations/scenario-migration-check-fail-v1 migrations/scenario-migration-check-fail-v2; do
  cargo mero build --manifest-path apps/$a/Cargo.toml; done
cargo test -p calimero-cloud-node    # 3-node cluster in process, sealed, real WASM
cargo run -p calimero-cloud-node --bin merod-v2 -- --help
cargo run -p calimero-cloud-node --bin merod-v2 -- local   # dev cluster: 3 nodes on :2501-2503 (`local.rs`)
```
