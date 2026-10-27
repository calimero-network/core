# calimero-cloud-node (`merod-v2`)

The release-v2 cloud node: an attested TEE node that hosts the cluster's system
group and the app groups placed on it. Design: the "Calimero Confidential Cloud"
architecture spec.

## Mental model

- **One Raft log per app** (`calimero-log`). The leader runs the call as a
  `calimero:app` component on the wasmtime engine (`calimero-app-engine-wasmtime`,
  one `WasmtimeEngine` per node, `Node::app_engine`; compiled artifacts cached under
  `<data_dir>/components`, S8) through the host core (`calimero-app-host`), and
  appends `{ call, outcome { status, result, error, diff, events, logs } }`.
  Followers apply the diff; nothing is re-executed. `base_index` guards against
  applying a stale diff. There is no v1/wasmer path (D15).
- **State** (`AppState.kv`, D46): `Arc<BTreeMap<stored key, value>>`, keys carrying the
  8-byte generation prefix (D39, generation 0). A run gets the current `Arc` as its
  `StateView`; apply uses `Arc::make_mut`, so it copies only while a view still holds
  the old version. Calls run inside `Group::propose_led` (serialized per app, on the
  machine's read lock, so the diff is built on exactly the state it applies to); views
  run outside the lock on a blocking thread (`spawn_blocking`). Host records
  (idempotency, `last_ts_ms`, `last_id`, `install_ts_ms`, `timers`) are `AppState`
  fields, never keys in `kv`.
- **Outcomes and statuses** (spec section 5, D23, D36): `Ok` → 200; `AppError` →
  `422 APP_ERROR` with `details.{code, fields}`; `Trap` → `422 TRAP` with
  `details.{kind, detail}` (`OutOfFuel`, `OutOfMemory`, `StackOverflow`, `Fault`,
  `HostDenied`); `Timeout` (deadline or no free instance) → `503 TIMEOUT` with
  `details.retryAfter`, never logged, key not spent. An entry whose JSON is over
  `calimero_log::MAX_ENTRY_BYTES` becomes, before proposing, the same call trapped
  `HostDenied` "entry too large" with nothing in it (D50). Call and `init` arguments
  over `MAX_ARGS_BYTES` (512 KiB) are refused first, `413 ARGS_TOO_LARGE`, not run
  (Ruling 30), so that replacement always fits; a `TooLarge` from the log is a `500`,
  never retried.
- **ABI checks in the host** (`Node::abi_gate`, from `Prepared.abi`, S5), default-deny
  (Ruling 31): a method the ABI does not name is `UnknownMethod`, `init` and timers
  are never reached through `/intents` or `/views` (`WrongKind`); a `confirm` method
  is never a view (`ConfirmNotAView`) and a call of one needs a
  consent; only views run as views (`NotAView`); anonymous callers reach `public`
  methods only (`LoginRequired`), `Caller::System` `allowSystem` ones only
  (`Forbidden`); all `422 APP_ERROR` before anything runs. `kind = action` is refused
  by `prepare`. `confirmMethods` on `POST /apps` is ignored.
- **Time** (`Node::app_clock`, D16, D19): app time is the host clock plus a shared
  offset (`Config.clock_offset_ms`; `Node::advance_clock` under feature `testing`).
  It stamps entries (`max(last_ts_ms + 1, app now)`) and decides which timer slots
  are due; nothing else. Sessions, tokens, challenges and API keys use
  `types::now_*`; Raft timers use `Instant`. A view sees `last_ts_ms`, the last
  applied entry's time.
- **Unique ids** (D52, Ruling 29): `id_term` is the term the leadership check that
  admitted the proposal found (`calimero_log::Leadership`, read from the Raft core),
  `id_seq` this node's per-term counter (`Node::next_id`), bumped for every execution
  including conflicted re-runs. Both are logged with every call, tick and `init`,
  and `AppState::last_id` keeps the highest pair applied (conflicted entries included).
  `propose_led` applies the leader's whole log before building, and the counter
  continues from `max(memory, last_id)`: a node restored as leader of the same term
  (openraft does this without an election, e.g. a one-replica group) never reissues
  an id.
- **Timers** (D34): `lib.rs` polls every `node::TIMER_POLL` (100 ms);
  `Node::tick_timers` spawns one round per app group this node leads (at most one
  at a time per app). A round reads the ABI's `every` methods and, for each,
  `node::due_slot`: the latest `install_ts_ms + k · interval` (k ≥ 1) at or before
  `app_clock()`, where `install_ts_ms` is the applied `init` entry's timestamp. If
  it is later than the timer's applied slot (`AppState::timers`, a host record), the
  leader runs it inside `propose_led` as `Caller::System` with `TimerCtx {
  scheduled, previous }` and logs `AppCmd::Tick` (slot, previous, stamp, ids,
  outcome). `apply` ignores a tick at or before the timer's applied slot (two
  leaders around a failover) and otherwise records the slot whatever the status:
  a tick that traps or returns an AppError counts as applied. A tick that times out
  is not logged, so the next poll proposes the same slot (or a later one, if one is
  due by then). Missed slots (no leader, a frozen clock) collapse into one tick for
  the latest; the app sees the gap through `previous`. Nothing ticks before `init`
  succeeded. Tests move app time with `Node::advance_clock` (feature `testing`,
  enabled for the crate's own tests through its dev-dependency on itself); the kv
  fixture's `beat` ticks hourly so other tests see no ticks.
- **Idempotency** (D16): records live for the last `IDEMPOTENCY_ENTRIES` (100,000)
  entries of the app, by log index, not wall time.
- **Identity inside the app**: the host core asks `app::SysDirectory`, which reads
  this node's system replica (org, owner, org roles, teams, `orgs_of`). The caller is
  `types::Caller::to_host` (`System` exists for the platform's own runs; never from a
  client).
- **System group** (`system.rs`, group 1): fleet, app directory, bundles, accounts,
  passkeys, API keys, refresh tokens, session keys, organizations, teams, invites.
  Commands carry their `actor`; `apply` checks authority, so the log decides.
- **Routing** (`node.rs`): system writes go to the system leader, calls and strong
  views to the app's leader, over **peer sealed channels** (`internal/*`) whose key
  is attested like any client's, plus the cluster secret (HMAC of K_r).
- **API** (`api.rs`), all inside the sealed channel except `/attestation` and `/health`:
  `/auth/*` (passkeys, refresh, API keys), `/bundles`, `/apps`, `/apps/{app}/{intents,views,events,upgrade,access}`,
  `/orgs/*`, `/invites/{token}/redeem`, `/internal/*` (peers), `/ops/*` (mdma: counts only, no read path).
- **Confirm methods** (the ABI's `confirm`): `intents` answers `412 CONFIRM_REQUIRED`
  with a challenge over the exact call; the retry carries a passkey assertion, which
  is logged as consent.
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
  client key get a node key (`node:…`) first, so a retry replays a call
  that did commit. System-log forwards keep the long timeout: some system commands
  are not idempotent yet. Regression: merobox `workflow-examples/cloud-v2/frozen-leader.yml`.
- **Leader races**: a call whose diff was built on replaced state applies as a no-op
  (`conflict`, no idempotency record); `Node::call` re-runs it instead of failing.
- **Upgrades** (`POST /apps/{app}/upgrade`) switch the bundle, code only; a named
  `migrate` is refused (`MigrationsNotSupported`): migrations are rebuilds (D38), not
  in this milestone. The directory records the new bundle once the switch committed.
- **Events and audiences** (R9): each `EventRecord` keeps the run's `Audience` (all,
  caller, accounts, team, org members, org admins) and calling account. The SSE
  stream (`local_events`; a forwarded stream names its `subscriber` to the peer)
  checks every event against the subscriber when it is sent
  (`SystemState::receives`: still reaches the app, and is in the audience now), so
  a member removed from the org or team stops receiving at once. A call's answer
  lists only the events its caller is in the audience of.
- **Removed with the wasmer path** (D15, S4): private storage, shared-storage
  writer rotation, in-place migrations. **Deferred** (out of this milestone): blobs
  (R8), search (R2), xcall (D25), event handlers and TEE runs (D17 actions; the v1
  TEE timers are replaced by D34 timers). Their cluster tests are `#[ignore]`d with
  the spec reference. Migrations, so pausing ticks during one (D38), are not here.
- **Devices**: a person is their account, and each passkey is a device: the
  session names the one it was opened with (`SessionClaims::passkey`,
  `passkeyId` in `/auth/me`); `auth/passkeys/add` adds one.
- **Init**: `POST /apps` takes `initArgs` (JSON) for the app's `init`, kept on the
  `AppRecord`; a failed init answers `422 INIT_FAILED` (`AppState::init_error`). The
  engine judges a component when an app is created from it (`POST /bundles` stores
  any bytes): one it refuses fails `init` with the reason.
- **Context ids**: an app answers to its context id (`hex(SHA-256(app_id))`) wherever
  the API takes an app id (`Node::canonical_app_id`).
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
- **Fresh system reads**: `Node::sys_fresh` is a read index (the leader names
  its applied index, `internal/system/index`; this replica waits for it). Org
  and team reads use it, and so does an access check before it refuses.
- **Declared views**: a call (`/intents`) of a method the ABI declares `#[app::view]`
  runs as a strong view: it writes nothing and is not logged (v1's rule).
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
running cluster, pruning of revoked session ids, D21/D26 time bounds,
per-app storage keys (section 9), and the deferred features above.

## Commands

```bash
cargo test --release -p calimero-cloud-node   # 3-node cluster in process, sealed, real components
# the cluster fixtures (apps/v2-fixtures/{kv,org-gated}) are built on first use by
# tests/common/mod.rs into target/app-components (needs the wasm32-wasip2 target)
cargo run -p calimero-cloud-node --bin merod-v2 -- --help
cargo run -p calimero-cloud-node --bin merod-v2 -- local   # dev cluster: 3 nodes on :2501-2503 (`local.rs`)
```
