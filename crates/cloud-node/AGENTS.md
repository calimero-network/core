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
- **Init**: `POST /apps` takes `initArgs` (JSON) for the app's `init`, kept on the
  `AppRecord`; a failed init answers `422 INIT_FAILED` (`AppState::init_error`).
- **Identity inside the app**: `env::account_id` and `env::executor_id` are both the
  caller's account (no devices in release-v2); the context id is `SHA-256(app_id)`.
  `env::caller_org` is the caller's role and teams in the app's organization
  (`app::caller_org`, passed through `Module::run_with_caller_org`).
- **Private storage** (`env::private_storage_*`): v1 kept it on the member's node,
  unsynced. Here it is the caller's own space in the app's state (`app::private_prefix`):
  replicated and sealed, but a run is handed only its caller's space.
- **Shared-storage writers**: a cell's rotated writer set is kept in the app's state
  (`app::writers_key`), written with the run that rotated it; v1 published governance ops.
- **Context ids**: an app answers to its context id (`hex(SHA-256(app_id))`, what the app
  sees as `env::context_id()`) wherever the API takes an app id (`Node::canonical_app_id`).
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
running cluster, event audiences, pruning of revoked session ids.

## Commands

```bash
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release   # test fixture
cargo test -p calimero-cloud-node    # 3-node cluster in process, sealed, real WASM
cargo run -p calimero-cloud-node --bin merod-v2 -- --help
cargo run -p calimero-cloud-node --bin merod-v2 -- local   # dev cluster: 3 nodes on :2501-2503 (`local.rs`)
```
