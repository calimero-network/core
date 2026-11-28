# calimero-node (the inside of `merod`)

The node: an attested TEE node that hosts the cluster's system group and the app groups placed
on it. This crate is the library; the `merod` binary (`crates/merod`) turns its configuration into
a `Config` and calls `run`, and `meroctl` (`crates/meroctl`) is its command line.

This file is the mental model: what must stay true, where each part lives, and where to read
more. The design and its reasons are in the protocol chapters
(`docs/src/content/docs/protocol/`); each ends with **In `calimero-node`**, which names the code
that implements it. A module's own detail is in its `//!` doc. Read
[architecture](../../docs/src/content/docs/contribute/architecture.mdx) for how the crates fit.

## Mental model

- **One Raft log per app** (`calimero-log`). Each app instance is its own group; the system group
  is group 1. A node hosts the groups placed on it and forwards the rest.
- **The leader executes, followers apply the diff.** The leader runs the WASM call and appends
  `{ call, outcome { result, diff, events } }`; followers apply the diff and never re-execute.
- **`base_index` refuses a stale diff.** An entry names the app's last entry it built on
  (`AppState::last_index`); a deposed leader's entry commits as a no-op `conflict`, and
  `Node::call` runs the call again.
- **One engine** (`types::Executor`). An app is a `calimero:app` component (built with
  `calimero-sdk`) on wasmtime, through the host core (`node/component.rs`, `component.rs`). The
  executor is fixed when the app is created and recorded in its group's state.
- **SDK 0.12 apps (wasmer) are gone.** A core-module bundle is refused `400 UNSUPPORTED_BUNDLE`;
  an existing wasmer app is listed and deletable, every other route answers
  `410 UNSUPPORTED_ENGINE`, and its log entries apply as no-ops (`AppMachine::unsupported`).
  `Executor::Wasmer` stays only so records written before still read (no executor = wasmer).
- **The system group** (`system.rs`, group 1) holds the fleet, app directory, bundles and
  releases, accounts and credentials, organizations, teams, invites and used proofs.
- **The log decides authority.** System commands carry their `actor` and their proof; `apply`
  judges them on every node, so a node that only sees the log cannot forge a decision
  (`proof.rs`, `system::authorize`).
- **`Area` dispatch.** Each `SystemCmd` belongs to one `system::Area` (`SystemCmd::area`,
  exhaustive). Its proof is judged in `system/<area>.rs`'s `authorize` and it applies in that
  module's `apply`; `system.rs` keeps the state, the shared rules and the two dispatchers.
- **Refusals carry their kind** (`system/rejection.rs`). A refusal is a `Rejection`: a
  `RejectionKind` and a message. The status and code a client sees come from the kind, never from
  the wording.
- **Routing** (`node.rs`). System writes go to the system leader; calls and strong views go to
  the app's leader (a strong view may run on a replica that applied the leader's read index).
- **Peer channels are attested.** Nodes talk over sealed channels (`internal/*`) whose key is
  attested like any client's, plus the cluster secret (an HMAC of K_c).
- **Peer identity.** A channel names its node (`x-calimero-node`); `Node::identify_peer` attests
  that node's registered address itself and takes the claim only if the quote names the key.
- **The inbound gate.** `api::peer` admits a peer route only for a node whose attested release
  is approved for the app (`caller_may_hold`); a Raft message also needs its sender to be a
  member of the group. Outbound bytes go only to peers `Node::may_send` allows.
- **Raft addressee.** Every Raft message names its `target`; any other node refuses it
  `421 NOT_ADDRESSEE`, so a message that reaches a reused address moves no term or log.
- **Access is judged fresh.** A credentialed or gated request reads the system state at a read
  index (`Node::sys_fresh`), so a removal committed on another node holds at once.
- **Entry format** (`component::ENTRY_FORMAT`). An entry starting `{"v":N,` is format `N`; no
  marker is format 1. An entry this release cannot decode stops the group on that replica
  (`StateTx::fail`), never a partial or skipped apply. A release that raises the format writes it
  only once every replica of the group reads it.
- **Stored formats** are `calimero-log`'s (`STORE_FORMAT`, `SNAPSHOT_FORMAT`); a change raises
  the version and keeps a reader for the old one.
- **Feature gates** (`system/cluster_version.rs`). An entry shape an older release cannot apply
  alike is a `Feature` with the release it ships in; `Node::sys_write` refuses it
  `409 FEATURE_NOT_ACTIVE` below the logged cluster version, which the system leader raises once
  every node supports it.
- **Apply reads only the log.** Apply judges an entry by the cluster version logged before it
  (`SystemState::cluster_brings`) and by the entry's own time, never a node's clock, so a log
  replays the same on every replica.
- **Earlier rules replay as they applied.** Flags such as `owner_in_org` or `close_drops_below`
  name the rule an entry was judged by; from the `RequireCurrentRules` mark on
  (`SystemState::current_rules_from`) apply holds every entry to the current rules.
- **Runs leave the async workers.** A WASM run inside a group's proposal lock goes through
  `node::off_workers` (`block_in_place`), so Raft heartbeats and HTTP keep their workers.
- **Logged runs hold no read lock.** `Group::propose_led_staged` reads the applied state, drops
  the lock, then runs the guest; an entry that commits meanwhile makes the run a `conflict`.
- **Disk work leaves the async workers.** Every store write goes through `node::disk::Disk`
  (`Node::disk()`); `crates/node/clippy.toml` disallows the direct `Engine` writes elsewhere.
- **Bundles are blobs, not log entries.** WASM and `.mpk` bytes sit in the sealed blob store on
  a majority; the log records their hash. Entries stay under `calimero_log::MAX_ENTRY_BYTES`.
- **Operator logs carry no user data.** `info!`, `warn!` and `error!` name ids, kinds, codes and
  counts only; an error's words go through `log_safe`. `scripts/check-no-user-data-at-info.py`
  enforces it.

## Module map

All paths are under `src/`. Each file's `//!` says what it owns; this is the index.

- **Process and wiring**: `lib.rs` (`run`, `Config`, background loops), `node.rs` (`Node`: groups,
  routing, peers, forwarding), `local.rs` (`merod local`), `inspect.rs` (`merod inspect`),
  `testing.rs` (test hooks, feature `testing`).
- **System group**: `system.rs` (state, shared rules, dispatch), `system/<area>.rs` (one per
  `Area`: `fleet`, `packages`, `apps`, `compute`, `accounts`, `recovery`, `approvals`,
  `deployments`, `sign_up`, `orgs`, `updates`), `system/cluster_version.rs`,
  `system/routing.rs` (derived indexes for Raft admission), `proof.rs`, `membership.rs`,
  `placement.rs`. `system/shard.rs` is the tenant-sharding routing layer — `ShardId`,
  `Scope`, `SystemCmd::scope()`, `shard_for`/`owns`/`flipped`, and the `ACTIVE` cutover
  switch (`false` today, so every command stays on the one system group); it splits the
  system group into a root group and per-organization tenant groups when turned on
  ([design note](../../docs/design/system-group-sharding.md)).
- **App groups**: `app.rs` (`AppMachine`), `component.rs` (entries and apply),
  `component/` (`action`, `blobs`, `runs`, `pipelines`, `proofs`, `step_cache`, `search`,
  `xcall`), `idempotency.rs`.
- **Running components**: `node/component.rs` (engine, calls, views, actions, xcalls),
  `node/component/{files,uploads,compute}.rs`, `node/component/compute/` (driving compute
  and pipeline runs), `compile.rs`.
- **HTTP API**: `api.rs` (router, `principal`, `gate`, shared helpers), `api/<area>.rs` (one per
  area, each with its `routes()`), `api/openapi.rs` (`docs/public/openapi.json`),
  `error.rs`, `metrics.rs`, `consent_page.rs` and `consent_page/`, `app_host.rs` (frontends),
  `origins.rs`, `presence.rs`.
- **Packages and releases**: `packages.rs`, `packages/pipelines.rs`, `runtime.rs`,
  `node/runtime.rs`, `report.rs`, `egress.rs`.
- **Upgrades**: `node/cluster_version.rs`, `node/auto_updates.rs`, `node/restore_points.rs`,
  `api/upgrades.rs`, `api/update_policies.rs`, `api/undo.rs`.
- **Keys and attestation**: `kms.rs` (K_r), `cluster_key.rs` (K_c), `signing_key.rs` (receipt
  key), `attest.rs` (quotes), `rollback.rs`, `peer_tls.rs`.
- **Storage, blobs and files**: `files.rs` and `files/wrap.rs`, `node/blob_guards.rs`,
  `node/segments.rs`, `node/disk.rs`, `backup.rs`, `node/restore.rs`, `recovery_disk.rs` and
  `node/recovery_disk.rs`, `node/digests.rs`.
- **Job nodes**: `job_node.rs` and `job_node/`, `job_wire.rs`, `node/job.rs`.
- **Accounting and logs**: `usage.rs`, `log_safe.rs`.

## Where to look

| Subject | Design | Code |
| --- | --- | --- |
| Write path, leader races, async workers | [write-path](../../docs/src/content/docs/protocol/write-path.mdx) | `node.rs`, `node/disk.rs` |
| Fresh reads, strong views, search | [reads](../../docs/src/content/docs/protocol/reads.mdx) | `node.rs`, `component/search.rs` |
| Placement, membership, replica moves | [replication](../../docs/src/content/docs/protocol/replication.mdx) | `placement.rs`, `membership.rs` |
| Peer channels, the gate, Raft addressee | [networking](../../docs/src/content/docs/protocol/networking.mdx) | `api/internal.rs`, `system/routing.rs` |
| System commands and proofs | [system group](../../docs/src/content/docs/protocol/system-group.mdx) | `system.rs`, `proof.rs` |
| Sessions, ceremonies, refresh | [sessions and authority](../../docs/src/content/docs/protocol/sessions-and-authority.mdx) | `api.rs`, `api/auth.rs` |
| Passkeys, recovery, origins | [identities](../../docs/src/content/docs/protocol/identities.mdx) | `api/passkeys.rs`, `origins.rs` |
| Governance, approvals, deployers | [governance](../../docs/src/content/docs/protocol/governance.mdx) | `system/approvals.rs`, `system/deployments.rs` |
| Orgs, teams, grants, guests, links | [organizations](../../docs/src/content/docs/protocol/organizations.mdx) | `system/orgs.rs`, `api/public_links.rs` |
| Packages, instances, deployments, units | [applications](../../docs/src/content/docs/protocol/applications.mdx) | `packages.rs`, `api/apps.rs` |
| Upgrades, cluster version, policies, undo | [upgrades](../../docs/src/content/docs/protocol/upgrades.mdx) | `system/updates.rs`, `node/auto_updates.rs` |
| The consent page | [consent page](../../docs/src/content/docs/protocol/consent-page.mdx) | `consent_page.rs` |
| Component runs, actions, outbound | [execution](../../docs/src/content/docs/protocol/execution.mdx) | `node/component.rs`, `component/action.rs` |
| Xcalls and delegation | [xcall](../../docs/src/content/docs/protocol/xcall.mdx) | `component/xcall.rs` |
| Blobs and sealed files | [blobs](../../docs/src/content/docs/protocol/blobs.mdx) | `component/blobs.rs`, `node/component/files.rs` |
| Compute, job nodes, pipelines | [compute actions](../../docs/src/content/docs/protocol/compute-actions.mdx) | `component/runs.rs`, `job_node.rs` |
| Quotes, receipts, peer trust | [TEE attestation](../../docs/src/content/docs/protocol/tee-attestation.mdx) | `attest.rs`, `signing_key.rs` |
| K_r and K_c | [encryption](../../docs/src/content/docs/protocol/encryption.mdx) | `kms.rs`, `cluster_key.rs` |
| State on disk, backups, recovery | [storage](../../docs/src/content/docs/protocol/storage.mdx) | `backup.rs`, `recovery_disk.rs` |
| Quotas and limits | [limits](../../docs/src/content/docs/protocol/limits.mdx) | `system.rs` (`Quotas`) |
| Presence | [presence](../../docs/src/content/docs/protocol/presence.mdx) | `presence.rs` |
| Metrics, usage, operator logs | [observability](../../docs/src/content/docs/operate/observability.mdx) | `metrics.rs`, `usage.rs` |
| Ops reports and placement read | [admin API](../../docs/src/content/docs/operate/admin-api.mdx) | `report.rs`, `api/ops.rs` |
| Old data and API removals | [compatibility](../../docs/src/content/docs/operate/compatibility.mdx) | `api-deprecations.toml` |
| Tests, scenarios, the process suite | [testing](../../docs/src/content/docs/contribute/testing.mdx) | `tests/` |

## Rules a change must not break

1. **Determinism.** Apply reads only the entry, the state and the logged cluster version; no
   clock, no peer, no other log. A leader may measure or fetch, then logs what it saw.
2. **Gate new log shapes.** A new system or app command, or a field that changes what one does,
   is a `Feature` named in `SystemCmd::features`, never proposed below the cluster version.
3. **Keep old entries replaying.** An entry an earlier release logged applies as it did; a new
   rule is a flag the API sets, or a mark in the log, never a silent change of `apply`.
4. **Raise a format, keep its reader.** `ENTRY_FORMAT`, `STORE_FORMAT` and `SNAPSHOT_FORMAT` move
   with a reader for the old shape and a test that reads it, or a declared hard cutover.
5. **Judge authority twice.** The API refuses before it asks for a passkey or a signature, and
   `Node::sys_write` refuses what `system::prejudge` finds `apply` would refuse before it proposes
   it; `apply` judges the same rule again on every node.
6. **Identity from proofs.** A caller is who its credential or signature says, never a field.
7. **Bound every input.** Each input from outside has a named limit constant, checked before the
   costly work.
8. **Nothing slow on the async workers.** WASM runs go through `off_workers`, store writes
   through `Disk`, quotes and compiles through their bounded pools (`attest::Quoter`,
   `compile::Compiler`).
9. **No user data at `info` and above.** Log what the node names; use `log_safe` for errors.
10. **Reserved methods stay the node's.** `__*` methods run only for a `CallRequest::host` the API
    built; `/intents`, `/views`, xcalls and follow-ups refuse them.
11. **Raft addressee transition.** A Raft message without `target` comes from a peer on rc.4 or
    earlier: this release accepts it, counts it in
    `calimero_raft_misaddressed_total{reason="no_target"}` and warns once per
    `api::NO_TARGET_WARN_EVERY`. Make `RaftBody::target` required only once that count is zero
    on every node.
12. **An app's log pins what it runs.** The bundle and compute component a run uses are the ones
    its group's own log names (`ComponentHead`), never the system record, which another log moves.

## Commands

```bash
cargo test -p calimero-node                         # 3-node clusters in process, sealed, real WASM
cargo test -p calimero-node --test it cluster::     # the integration tests are one binary, `it`
cargo test -p calimero-node --test it component_cluster::end_to_end -- --exact --nocapture
cargo test -p calimero-node --test auto_updates -- --test-threads 1
cargo test -p calimero-node --test scenarios [-- FILTER]   # every scenario file
python3 -I crates/node/tests/fault/suite.py [--only NAME,..]  # the process suite
python3 -I crates/node/tests/fault/compute_e2e.py        # compute end to end, as processes
cargo bench -p calimero-node --bench cluster_throughput -- --secs 15 --out cluster.json
cargo run -p merod -- local                          # dev cluster: 3 nodes on :2501-2503
cargo run -p merod -- --node n1 inspect              # a stopped node's store
```

Fixtures, scenario globs, ports and the fault suites:
[Testing strategy](../../docs/src/content/docs/contribute/testing.mdx).
