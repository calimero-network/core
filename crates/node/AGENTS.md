# calimero-node - Node Orchestration

Main node runtime that coordinates sync, storage, networking, and event handling.

## Package Identity

- **Crate**: `calimero-node`
- **Entry**: `src/lib.rs`
- **Framework**: actix (actors), tokio (async)

## Commands

```bash
# Build
cargo build -p calimero-node

# Test
cargo test -p calimero-node

# Test specific
cargo test -p calimero-node test_sync -- --nocapture
```

## File Organization

```
src/
├── lib.rs                    # Crate root: module declarations and re-exports
├── manager.rs                # NodeManager actor
├── state.rs                  # NodeClients, NodeManagers, NodeState
├── run.rs                    # Node startup (start function)
├── handlers.rs               # Handler module parent
├── handlers/
│   ├── network_event.rs      # Network event handler
│   ├── network_event/
│   │   ├── namespace.rs      # ns/<id> topic dispatch (Op/Ack/ReadinessBeacon/ReadinessProbe)
│   │   ├── readiness.rs      # ReadinessBeacon + ReadinessProbe receiver-side handlers
│   │   └── tee_fired.rs      # A TEE's signed statement that a run which wrote nothing fired
│   ├── state_delta/          # State delta handler (mod.rs, buffering.rs, crypto.rs, events.rs, store_setup.rs, verify.rs)
│   ├── stream_opened.rs      # Stream opened handler
│   ├── blob_protocol.rs      # Blob protocol handler
│   ├── blob_announce.rs      # Availability prefetch on an inbound blob announcement
│   └── get_blob_bytes.rs     # Get blob bytes handler
├── readiness.rs              # ReadinessTier FSM + ReadinessCache + ReadinessManager actor
├── readiness/
│   └── tests.rs              # FSM transition tests + cache picker / atomicity tests
├── join_namespace.rs         # J6 namespace-join: join_namespace/await_namespace_ready/with_retry
├── tee_firing.rs             # TEE trigger election, failover turns, fired-marker checks (event + timer triggers)
├── tee_scheduler.rs          # Fires #[app::tee(every = "..")] methods once per tick on TEE authorities
├── sync/
│   ├── mod.rs                # Sync module (exception to no mod.rs rule)
│   ├── manager/              # SyncManager (mod.rs, blob_fetch.rs, handshake.rs, namespace_join.rs, namespace_sync.rs, relay_sealed_join.rs, tee_admission.rs, tests.rs)
│   ├── stream.rs             # Sync streams
│   ├── config.rs             # Sync configuration
│   ├── tracking.rs           # Sync tracking
│   ├── blobs.rs              # Blob sync
│   ├── delta_request.rs      # Delta request handling
│   ├── helpers.rs            # Sync helpers
│   ├── namespace_backfill.rs # decode_backfill (cap, namespace check, parents-first order) and collect_ancestry (targeted fetch of a refused op's ancestors by id)
│   └── snapshot.rs           # Snapshot handling
├── delta_store.rs            # Delta storage + applier (merge-applies via `ContextClient::apply_remote_delta`, so read-only replicas keep the result)
├── dag_compactor.rs          # DAG compaction actor: prunes live and cold contexts' delta history, then compacts the delta-column slices
├── dag_compactor/
│   ├── disk.rs               # Bounded on-disk prune of one context's ContextDagDelta rows and their side rows (count, retain walk, batched deletes)
│   ├── side_rows.rs          # The per-delta side tables (events hash, TEE trigger) a pruned row takes with it
│   ├── orphans.rs            # Bounded two-look sweep of side rows no delta row, absorb record or live DAG claims
│   └── sweep_tests.rs        # RocksDB sweeps: cold / restarted / pending contexts, side rows, orphans, SST bytes given back
├── gc.rs                     # Tombstone GC (+ parents' deleted_children), under each context's lock
├── tombstone_stability.rs    # When a tombstone may go: every member device caught up (signed StateBeacon)
├── constants.rs              # Constants
├── arbiter_pool.rs           # Actix arbiter pool
└── utils.rs                  # Utilities
primitives/                   # calimero-node-primitives
├── src/
│   ├── lib.rs                # Shared types
│   ├── client.rs             # NodeClient
│   ├── client/application/
│   │   ├── acquire.rs        # acquire_bytecode + the ApplicationStore impl
│   │   ├── bind.rs           # Application-row binding + blob release on failure
│   │   └── install.rs        # Installs by coordinates or local path
│   ├── sync.rs               # Sync types
│   └── messages/             # Message types
```

## Key Components

### NodeManager Actor

Main coordinator using actix actor pattern:

```rust
// src/manager.rs
pub struct NodeManager {
    clients: NodeClients,      // External service clients
    managers: NodeManagers,    // Service managers
    state: NodeState,          // Runtime state
}

impl Actor for NodeManager {
    type Context = Context<Self>;
}
```

### Handler Pattern

- ✅ DO: Follow pattern in `src/handlers/network_event.rs`
- ✅ DO: Use actix message handlers

```rust
// src/handlers/network_event.rs
impl Handler<NetworkEvent> for NodeManager {
    type Result = ();

    fn handle(&mut self, msg: NetworkEvent, ctx: &mut Self::Context) -> Self::Result {
        // Handle network events
    }
}
```

### SyncManager

Handles state synchronization between nodes:

```rust
// src/sync/manager/mod.rs
pub struct SyncManager {
    // Sync configuration and state
}
```

### ReadinessManager

Per-namespace readiness-beacon emitter and FSM driver. Closes the
cold-start gap between joining a namespace and being able to publish
governance ops without losing them to gossipsub's GRAFT handshake.
See #2237 and the `crates/node/src/readiness.rs` module doc.

```rust
// src/readiness.rs
pub struct ReadinessManager {
    pub cache: Arc<ReadinessCache>,           // shared with receiver
    pub config: ReadinessConfig,
    pub state_per_namespace: HashMap<[u8; 32], ReadinessState>,
    pub node_client: NodeClient,              // raw publish (bypasses 10s mesh-wait)
    pub datastore: Store,                     // namespace-identity loading
    pub last_probe_response_at: HashMap<(PeerId, [u8; 32]), Instant>,
    pub pending_republish: HashMap<[u8; 32], PendingJoin>,  // zero-ack membership ops
}
```

- Beacons signed via `READINESS_BEACON_SIGN_DOMAIN` (canonical
  `signable_bytes`) and verified by
  `calimero_context::governance_broadcast::verify_readiness_beacon`
  (signature + namespace member set). A beacon that fails the membership
  check falls through to the stranded-member arm and is otherwise dropped.
  It used to be rescuable by a verifiable admission proof it carried; that
  path existed so an invitation could be claimed by broadcast, which direct
  admission replaced — the invitation names who may admit it, and the
  admitter applies and publishes the join, so no stranger's beacon needs to
  unlock a pull.
- Periodic emission on `beacon_interval` ticks for `*Ready` tiers.
- Edge-trigger emission on tier transition into `*Ready` (via
  `LocalStateChanged` / `ApplyBeaconLocal`).
- Probe-response rate-limit at `BEACON_INTERVAL / 2` per
  `(peer, namespace)` to close traffic + mailbox amplification.
- A `MemberJoinedAt` broadcast that collected zero acks is queued via
  `NodeClient::queue_membership_republish` (`PendingRepublish`) and
  rebroadcast on the next `EmitOutOfCycleBeacon` for that namespace -
  fired both when a namespace peer subscribes and when one sends a
  `ReadinessProbe`. The handler's per-(peer, namespace) rate-limit
  check returns before reaching the republish drain, so a
  subscribe/probe that lands inside that window does not retry until
  the next one outside it. The stored op is rebroadcast verbatim, never
  re-signed. Entries expire after `REPUBLISH_CAP` (10 min) and are NOT
  removed on republish, so a later subscriber gets another attempt.

### J6 namespace-join (Phase 8)

Free functions in `src/join_namespace.rs` (not on `ContextClient`
because of a Cargo dep cycle):

```rust
pub async fn join_namespace(...) -> Result<JoinStarted, JoinError>;
pub async fn await_namespace_ready(...) -> Result<ReadyReport, ReadyError>;
pub async fn join_and_wait_ready(...) -> Result<ReadyReport, ReadyError>;
pub async fn join_namespace_with_retry(...) -> Result<JoinStarted, JoinError>;
```

The fast path (`join_namespace`) provisions the namespace identity,
seeds local trust by writing a minimal `GroupMetaValue` with the
invitation's inviter as `admin_identity` (so beacons signed by the
inviter pass `verify_readiness_beacon`), subscribes to `ns/<id>`,
publishes a `ReadinessProbe`, and awaits the first fresh beacon.

## Key Files

| File                            | Purpose                        |
| ------------------------------- | ------------------------------ |
| `src/manager.rs`                | NodeManager actor definition   |
| `src/state.rs`                  | NodeClients, NodeManagers, NodeState |
| `src/run.rs`                    | `start()` function, NodeConfig; starts the full-text search indexer (`calimero_search::SearchService::run_indexer` over `NodeContextSource`) when `[context.search] enabled` (the default). Sync writes no search rows: the indexer finds state a sync moved by its root chain |
| `src/handlers/network_event.rs` | Network event handling         |
| `src/handlers/network_event/namespace.rs` | `ns/<id>` topic dispatch (Op/Ack/Beacon/Probe) |
| `src/handlers/network_event/readiness.rs` | Beacon receive + probe forwarding |
| `src/handlers/state_delta/`     | State delta processing         |
| `src/readiness.rs`              | Readiness FSM + cache + manager (#2237) |
| `src/join_namespace.rs`         | J6 namespace-join flow         |
| `src/sync/manager/mod.rs`       | Sync coordination              |
| `primitives/src/client.rs`      | NodeClient interface           |
| `primitives/src/client/application/acquire.rs` | `acquire_bytecode`; `NodeClient`'s `ApplicationStore` impl |

## JIT Index

```bash
# Find handlers
rg -n "impl Handler" src/

# Find actor messages
rg -n "impl Message" src/

# Find sync logic
rg -n "pub async fn" src/sync/

# Find constants
rg -n "const " src/constants.rs
```

## Testing

```bash
# Run all node tests
cargo test -p calimero-node

# Run specific test
cargo test -p calimero-node concurrent_branches -- --nocapture

# Integration tests in tests/ directory
cargo test -p calimero-node --test network_simulation
```

## Common Gotchas

- NodeManager is an actix Actor - use message passing
- Sync operations are async - use proper await handling
- Delta stores are per-context (ContextId key)
- **A repair leaf defers only when there is something to merge it with.**
  `sync/helpers.rs`'s `classify_leaf` sends a `Custom`-typed leaf to
  `dispatch_deferred_custom_merges` (`__calimero_merge_custom`) only when this
  node stores a value for it (`stores_value`), because that pass skips an entry
  with nothing stored. One the receiver lacks applies through the plain path,
  which stores it as it arrives. Deferring it anyway made HashComparison and
  level-wise unable to deliver an entry a node had refused (its delta applied
  before the author's binding folded), so the replicas stayed divergent on the
  same DAG heads (#4310)
- `ReadinessCache` and `ReadinessCacheNotify` use poison-recoverable
  mutex helpers (`entries_lock` / `waiters_lock`); never call `.lock()`
  directly on those fields
- **Only two things make a peer acceptable to serve a group key** (`key_server_accepted`): it is a trusted anchor of that group, or it proved with a certificate chaining to this node's own account root that it is a device of this node's own account. Decided **per response**, never by pruning the candidate list first — the second ground cannot be known until the answer is in hand. An awaited `key_id` is NOT a ground and the predicate deliberately takes no such argument: that id is read from a cleartext field no gate checks, so whoever mints it can then satisfy its own hash check (#3888). Anchor-first ordering decides who is *asked* first, never who is *believed*. A responder is an anchor when the envelope it served is signed by an anchor's key (`envelope_signed_by`, checked before the gate), and by nothing else. `peer_identities` is not a ground: it is filled by gossip ops a peer relayed, which any peer can re-publish, so it only orders who is asked first. The signature also serves a joiner that bootstrapped by pull and saw none of the anchor's gossip, which was refused the owner forever when a TEE was admitted by another TEE and the owner, answering `AlreadyMember`, published nothing (`tee-cards-late-tee`). A responder that only *claims* an anchor's identity is refused like any non-anchor, so the round goes on to the next candidate. And do not reduce this to anchors alone: `trusted_anchors` reads group meta, so a node holding no governance state identifies NO anchor, and that is exactly the freshly paired device the pull exists for — the `account-device-*` scenarios fail on it, surfacing two steps away as "context does not belong to any group" because without the key the GroupOps mapping a context to its group never fold (#3892). A namespace founded through a relay has no anchor node in its cleartext genesis except that relay, which `trusted_anchors` therefore counts (see governance-store's AGENTS.md).
- `ReadinessCache::insert` does NOT verify signatures or membership -
  the receiver-side gate `verify_readiness_beacon` is the choke point;
  callers from outside the receiver path must verify first
- A beacon's `dag_head` is the lex-min of the sender's head SET, so it cannot
  represent a fork: a peer holding `{L, G}` advertises whichever sorts lower,
  and if we already hold that one we look caught up. `applied_through` is what
  detects the fork, hence `peer_applied_more` in `beacon_indicates_divergence`.
  The repair pull then goes to the beacon's own signer, the one node
  demonstrably holding what we lack, not a subscriber that may be as far behind
- `ns/<id>` topic publishes wrap inner `NamespaceTopicMsg` in
  `BroadcastMessage::NamespaceGovernanceDelta { namespace_id, delta_id,
  parent_ids, payload: borsh(NamespaceTopicMsg) }` - sender-side
  envelope skips break receive-side decoding silently, and the receiver
  drops an envelope whose `namespace_id` is not the arrival topic's
- `VerifiedBundle` is the only way to read artifact bytes out of a
  `.mpk`; `extract_bundle_files` is module-private so the compiler
  enforces it. Construction requires a valid manifest signature, and
  every wasm artifact is digest-checked against the signed manifest
  before its bytes are returned. Nothing is unpacked to disk: the
  `.mpk` stays a content-addressed blob, and a multi-service install
  copies each service's wasm into a blob of its own
- A bundle's manifest is the entry at exactly `manifest.json`, never a
  nested one: an `old/manifest.json` would be an authentically signed
  older release, so basename matching is a signed rollback. Only a
  leading `./` is normalised away, since that is how ordinary tar
  spells a top-level entry; every other component is significant, on
  the manifest's declared paths as much as on the archive's entries. A
  second entry at that path is refused, after the signature rather than
  during the pre-auth scan, since the check has to reach the archive's end
- `BundleManifest::artifacts` destructures the manifest without `..`,
  so adding an artifact field is a compile error until it is
  classified; keep it that way rather than reaching for a wildcard
- `acquire_bytecode` is a thin wrapper over
  `calimero-app-downloader`, which picks the node's ONE source from
  `[registry] mode` and states the contract; see
  [app-downloader/AGENTS.md](../app-downloader/AGENTS.md). This crate
  owns the storage half: the `ApplicationStore` impl, the `PeerBlobs`
  impl, and the row binding (`bind_application`) in `acquire.rs`, backed
  by `bind.rs`'s `put_bundle_row`. Add a source there, not by
  fetching inline at a call site
- In `Http` mode a node is not a source of application bytecode, so it
  neither announces nor serves it: `NodeClient::may_share_blob` gates
  `announce_blob_to_network`, `sync/blobs.rs`'s
  `handle_blob_share_request`, and `handlers/blob_protocol.rs`. Gate every
  new serve site through it. User-data blobs are untouched in both modes
- **A peer is served a blob only for a context this node holds it for.**
  `NodeClient::is_blob_held_for_context` (a node-local `key::BlobOwner` row, or
  the context's own application artifact) gates the signed path of
  `handlers/blob_protocol.rs`, `sync/blobs.rs`'s responder and the runtime's
  `blob_open`; gate every new serve or read site through it too.
  `record_blob_owner` writes the row where bytes enter for a context: the
  runtime's `blob_create`, the admin upload with a `context_id`,
  `upgrade_group`, `fetch_blob_for_context`'s peer fetch and the `BlobShare`
  initiator. Record only after the bytes are verified, and never on a local hit
  or on an announcement's say-so: either would let one context's member claim
  another context's blob by id
- `NodeClient::get_blob`'s discovery leg does NOT trust the DHT alone: a
  provider record is opportunistic (nothing announces application
  bytecode at install, and a restart drops what was announced), so an
  empty or failed lookup falls back to the context topic's subscribers.
  A blob a context member holds must stay reachable without a record
- `sync/manager/blob_fetch.rs` acquires a context's bytecode through
  `acquire_bytecode` too, never through `initiate_blob_share_process`.
  `Unavailable` there is non-fatal by type - it returns a bare `Outcome`,
  so nothing can `?` it into a session abort
- `bind_application` derives a bundle's application id from its
  signed manifest and compares it to the one governance named *before*
  calling `install_bundle`. That call writes the `ApplicationMeta` row
  and a blob per service, and nothing reclaims either, so validating
  afterwards would leave the artifact's own (legitimate, possibly
  unrelated) application row pointing at a blob the failure path then
  deletes
- The `ApplicationMeta` row follows `InstallOrigin`. An `Operator` install
  (the admin API) may set it to any release, including an older one. A
  `Remote` install (downloader, blob share, join bootstrap, relay) replaces
  a signed release only with an equal or semver-newer one, keeping any
  other as a blob. Raw wasm is refused on every remote path
  (`derive_bundle_id`), and never runs: `application_bytes_from_blob`
  refuses it, since a group target, marker or stub can name any held blob.
  Writes re-check under `lock_application_rows`, the process-wide lock every
  stub writer (governance `ContextRegistered` and upgrade-target seeds, the
  join bootstrap) also takes
- A joiner that holds no key to seal its own join does NOT publish it in
  the clear. `sync/manager/relay_sealed_join.rs` carries both halves of
  the exchange that replaced that fallback (#3904): the joiner sends
  `InitPayload::RelaySealedJoinRequest` with its own signed op, and the
  admitter wraps it as `NamespaceOp::RootRelaySealed` and publishes.
  Three things about it are load-bearing. The responder does **no**
  membership or authority check on the requester — the authority is the
  endorsement sealed inside the op, which is self-authenticating, so a
  gate there would only reject legitimate relays. The initiator tries the
  endorsing admitter first because it is known reachable (the endorsement
  arrived over a stream to it), which is why `JoinBundle` carries
  `admitter_peer` at all; it is filled by the requester, never asserted by
  the responder — and it then falls through to the rest of the namespace
  topic, because an admitter is NOT guaranteed to hold the key: one still
  awaiting its own `KeyDelivery` endorses the join and serves an empty
  envelope, which is precisely how a joiner ends up unkeyed. And a relay that finds no keyholder **fails the join**:
  falling back to a cleartext publish would make "sealed" and "leaked" the
  same silence, and an older responder that cannot decode the payload
  lands in exactly that branch.
- **A TEE may ask for admission directly, and the answer is still decided in one place.** `sync/manager/tee_admission.rs` carries both halves: `fleet-join` sends `InitPayload::TeeAdmissionRequest` to each of its `admitter_addrs` in turn, and the responder runs `handlers::tee_attestation_admission::verify_and_admit` — the same function the `TeeAttestationAnnounce` broadcast receiver runs — and answers `TeeAdmissionResponse`. Keep it that way: a second copy of the quote/credential/policy/vouching checks on the direct path would drift from the broadcast one, and the two would admit different sets. The request requires a proof of possession, and its `public_key` must equal the proven `party_id`, or a dialer could relay another replica's attestation on its own stream. The addresses carry no authority (every peer re-checks the voucher at apply), and the broadcast stays: an older responder cannot decode the request and drops the stream. A refusal is re-checked against membership before it is answered (`direct_admission_answer`): `fleet-join` sends the direct request with the quote its broadcast carries, so when the broadcast admits first the direct request is refused as a replay, and a requester whose credential certifies its key and who holds a TEE row at the root is told it is admitted. The channel reaches the manager through `SyncClient::with_tee_admission` / `SyncManager::with_tee_admission_rx` rather than `new`, so test harnesses need not build it; `run.rs` always does. **An initiator never waits on itself or on a silent peer**: `group_admitter_routes` drops this node's own peer id (a self-dial ends in libp2p's `LocalPeerId` and the pending stream open is never answered), every open on this path goes through `SyncManager::open_stream_bounded`, and `SyncDriver` runs each admission beside its loop (at most `MAX_TEE_ADMISSIONS_IN_FLIGHT`, each answered as refused after `TEE_ADMISSION_DEADLINE`) rather than awaiting it inside its `select!` arm, which stalled periodic sync and outbound joins. `fleet-join` answers `admitted=true` without asking anyone when the node already holds a row and the covering key.
- **Each TEE admission message has a second form that names the node release.** `InitPayload::TeeReleaseAdmissionRequest` and `BroadcastMessage::TeeReleaseAttestationAnnounce` are the old forms plus `release_version`, appended at their enums' tails; both land in the same `verify_and_admit` through `TeeAdmissionClaim`, whose `release_version` is `None` for the old forms. `fleet-join` sends the named direct request when merod knows its release (`MERO_TEE_VERSION`) and broadcasts both announces, so an admitter that predates the new form still hears it. The release is checked in `admit_tee_node`, under a signed-release policy only.
- **A TEE's delta is signed under `SignatureDomain::Tee`, over its trigger's cause.** The cause (`TeeTriggerCause`) rides in the clear as `tee_trigger` on `BroadcastMessage::StateDelta`, `DeltaResponse` and `BufferedDelta`, and every receive path — gossip, buffered replay, parent fetch, `delta_request.rs`'s catch-up and the head-pull in `sync/manager/mod.rs` — must pass it to `verify_delta_envelope`, run `check_tee_envelope` on the result, and call `record_accepted_tee_delta` once the store takes the delta. That call is where the fired marker comes from; nothing else records one for a peer's delta. The cause is kept in a side table (`calimero_context_client::tee_trigger::delta_trigger`), not on the `ContextDagDelta` or `AbsorbRecord` row, because both are plain borsh with no migration: the catch-up responder serves it from there, and an absorbed delta's replay reads it back from there. A new receive path that drops the field refuses every TEE delta as untriggered. A run that writes nothing has no delta, so `TeeFiring` gossips a signed `BroadcastMessage::TeeFired` (`SignatureDomain::TeeFired`) instead, checked in `handlers/network_event/tee_fired.rs`; it is never persisted.
- **A delta's id covers its events.** `CausalDelta::compute_id` hashes `events_hash` (`CausalDelta::hash_events` of the sealed events bytes) first, so a delta re-sealed with other events no longer matches its id or its author's signature. Gossip and buffered replay hash the decrypted events before `content_address_matches`; catch-up, parent fetch and head-pull get `events_hash` on the wire `CausalDelta` and check it through `id_matches_content`. Every receive path calls `record_accepted_events_hash` once the store takes the delta, and `delta_request.rs`'s responder serves the hash from that side table (`calimero_context_client::delta_events`), because the row's events are cleared once the handlers ran. A receive path that drops it serves deltas no peer can verify.
- **Event handlers run through `execute_event_handler`.** `state_delta/events.rs` never calls `execute` for an event: the context manager refuses a method the app's ABI does not declare `#[app::handler]` (`ExecuteError::NotAnEventHandler`), and the node treats that refusal as settled so the events are not replayed, warning with the context, the application and the method. `tee_firing.rs` does the same for a TEE trigger.
- **A snapshot leaf buffered as future-schema drains to the page apply's verdict.** `persist_buffered_snapshot_entity` (`sync/snapshot.rs`, run by `drain_absorbed_leaves`) asks what `request_and_apply_snapshot_pages` asks: the signature, the TEE-only rule, and `snapshot_leaf_authorship`, with a `Forged` `Shared` / `SharedMember` leaf rescued only by `rotation_removed_the_signer` / `member_signer_was_a_writer_then`. What the page apply takes from the rest of the snapshot, the drain reads from the store: a member's writers from its stored anchor, the rotation log from the anchor's stored log (`delta_store::load_rotation_log_direct`). Either missing is `Pending`, and the drain sorts entity records by `buffered_snapshot_entity_pass` (plain leaves, then `Shared`, then `SharedMember`) so a snapshot, whose entities share one schema, lands in one pass. A missing log is `Refused` instead when `anchor_proves_no_rotation` holds: the anchor's children, stored or still buffered (`buffered_snapshot_children`), reproduce its shipped `full_hash` without a rotation-log child, and a rotation always links that child under its anchor. Its writer set cannot prove this: a writer's own removal leaves the rotated set on the anchor while the log is in flight, and a set rotated back to the one the cell id binds looks never rotated. A signature that fails is `Refused` too, since the key rides in the leaf. What stays `Pending` is retried on the absorb-drain triggers (governance op apply, namespace sync, blob fetch, startup recovery) at most `MAX_GOVERNANCE_DRAIN_ATTEMPTS` times, counted in the record's otherwise unused `governance_drain_attempts` (`drain_buffered_snapshot_entity`), then deleted: a member cannot grow the buffer with records that wait for nothing
- **A repair leaf for the app-state entry (`ROOT_ENTRY_ID`) always defers**, whatever `crdt_type` the peer names, and `dispatch_deferred_root_merges` merges it through the module's `__calimero_merge_root_state` by the rule a delta takes. Never apply or last-writer-wins it from the DFS: repair and delta would then settle one conflict two ways. The HashComparison initiator pushes its own entry back, and every responder hands a pushed entry to the same dispatch (the protocol-trait responders return it to their caller), so one session converges both sides. An `Id::root()` leaf goes through `Interface::apply_remote_action`, which holds the root shell rule
- **A namespace backfill response is ordered before it is applied.** The responder serves ops in delta-id (hash) order, which is unrelated to causality. The three backfill receivers (`fetch_and_apply_namespace_backfill`, the governance catch-up and `sync_namespace_from_peer`) go through `sync::namespace_backfill::decode_backfill`, which caps the batch at `MAX_BACKFILL_OPS`, drops undecodable ops and ops naming another namespace, and sorts parents before children (`calimero_governance_types::order_parents_first`; `join_group` in `calimero-context` applies the same ordering to a join response's ops). The order matters because the governance DAG refuses to buffer an op whose signer is not yet certified: in hash order a member's op could be refused because its join sits later in the same batch. Signatures are checked by the DAG's entry point, not here. A gossip op that comes back `NotAdmitted` or `Pending` triggers `fetch_and_apply_ancestry`: `namespace_backfill::collect_ancestry` asks its sender for the op's missing parents by id (an empty request would only return the first `MAX_BACKFILL_OPS` ops by hash, always the same window), then for what those need, for at most `MAX_ANCESTRY_ROUNDS` rounds and `MAX_ANCESTRY_OPS` ops, keeps only ops that were asked for, and applies the whole set parents first once (nothing is applied between rounds, since an older ancestor may be what certifies a newer op's signer). The `NotAdmitted` fetch is limited to one per (namespace, sender) per `NAMESPACE_REFUSAL_BACKFILL_INTERVAL`, over at most `MAX_REFUSAL_BACKFILL_SLOTS` slots. Paging the empty request, which every sync round uses, is not done: on a namespace with more than `MAX_BACKFILL_OPS` ops it returns the same window each time.
- **Presence is one `PresenceUpdate`, signed by its author inside the seal** (`calimero_node_primitives::presence`). `BroadcastMessage::Ephemeral` carries only `context_id`, `key_id`, `nonce` and the sealed update; the statement (context, author, seq, time, state hash) is the author's signature, and freshness is judged on its signed stamp after decryption. A node signs its own; an account's update carries its device certificate, and a relay publishes it through `outbound::publish_delegated` after `admit_delegated`. Receivers check an account's membership and revocation by their own data (`ephemeral/standing.rs`, shared with the relay so the two cannot disagree); a node author is gated only by holding the key, as before. A retract (`state: None`) is seq-gated in `AwarenessStore::retract`, so a replayed old one cannot remove a live entry. The relay keeps no `ephemeral_local` entry for an account: the account resends, and the relay's sweep expires it.
- **A delegated delta's warrant is admitted, and its nonce spent, inside `ContextStorageApplier::apply`**, so every path that applies one runs the same gate: the primary of an add, a cascaded child, a persisted parent `get_missing_parents` loads, a delta re-driven after a restart. The envelope (cut and warrant) comes from the armed slot (`arm_author` takes the warrant with the author and the cut) or, with nothing armed, from the delta's `ContextDagDelta` row (`persisted_envelope`), which is why a delegated delta that goes pending gets a row (`DeltaStore::keep_pending_delegated`): without one a cascade would apply it with no warrant at all. The nonce is spent only once the apply has succeeded, never on arrival — a held-back delta has not used it, and spending early made a pending delta's own re-drive after a restart read as a replay. A re-delivery of an applied delta is a DAG duplicate and never reaches `apply`, so it is not refused. A refusal or an undecidable cut fails the apply; the DAG drops the delta and it is retried from its row or a re-fetch. A cascaded child's refusal surfaces as the error of the add that cascaded it, as any cascaded failure does.
- **An entry a relay wrote on an account's behalf (`SignatureData::on_behalf`) is that account's only when its signer is a `RelayTee` writing for a member** (`calimero_governance_store::on_behalf_standing`). Each path places it with the cut it already uses. On the delta path `delta_store::on_behalf_accounts` judges each on-behalf action at the delta's own cut — the one armed with its author, or, for a delta applied with nothing armed (a cascaded child, a persisted parent loaded into the DAG, including after a restart), the cut its `ContextDagDelta` row records — and hands storage the account per action (`StorageDelta::CausalActions::on_behalf_accounts`). The delta's author plays no part: a `RelayTee` may write any member's entries, so a relay's entries apply in a delta the member did not author, and a delegated delta applies where no author is armed. An on-behalf action naming no signer, refused by the rule, or undecidable at the cut (not folded, or no cut known) refuses the whole delta, which is retried. Repair (`signer_account_for`, `is_leaf_currently_authorized`) asks the rule live, never through the TEE-authority mapping, and does not run the signer's own read-only gate, which would drop every such entry. Snapshot (`snapshot_leaf_authorship`, `snapshot_signer_accounts`) asks only the relay half, live, so a departed member keeps what was written for them, and what a relay wrote is dropped from cold joiners once it stops being a `RelayTee`.
- **DAG compaction prunes the delta column by its own rows, not by the in-memory DAG** (`dag_compactor.rs`, `dag_compactor/disk.rs`). The in-memory count says nothing about the rows: a context nothing touched since start has no `DeltaStore`, and after a restart a compacted context's DAG cannot be rebuilt (the oldest retained row's parent is gone, so `load_persisted_deltas` restores none of the chain). Each sweep visits every live `DeltaStore` (`DeltaStore::compact`: in-memory prune, then the disk prune keeping every id the DAG still holds) and every other context in `ContextMeta` (disk prune only, the DAG never loaded). The disk prune counts at most `min_deltas_before_compact + 1` keys, walks back from the persisted `dag_heads` for the retain window, scans at most `MAX_COMPACTION_SCAN_ROWS` and deletes at most `MAX_COMPACTION_DELETES_PER_CONTEXT` applied rows per sweep, never a head and never an `applied: false` row. It runs under the context's execution lock (`ContextClient::acquire_lock`), taken after the `dag` write lock on the live path — the same order as an inbound apply — because that lock is held by every commit of an applied row with the heads; without it a head committed mid-scan would be judged against the old heads and deleted. No lock (unknown context) means no rows are deleted. The heads are re-read before each delete batch and a change stops the pass. A pending delta no longer blocks compaction: `prune_to_recent` keeps pending deltas and every parent they already hold. Deleted rows are given back by compacting the context's `Delta` slice when `gc::worth_compacting` (the tombstone GC's bar) says so, with no lock held.
- **A pruned delta row takes its side rows with it, in the same transaction** (`dag_compactor/side_rows.rs`): the events hash (`delta_events`) and the TEE trigger (`tee_trigger::delta_trigger`), both in `Column::Generic`. A side row goes only with its own delta's row, so a retained, pending, head or kept delta keeps it; a batch holds whole deltas and at most `COMPACTION_DELETE_BATCH` rows. Their keys are hashed from `(context, delta id)`, so a context's side rows are not a range and a side row's key names no delta: `dag_compactor/orphans.rs` sweeps those earlier compactions left behind by reading at most `MAX_ORPHAN_SCAN_ROWS` of each table and ruling out every one claimed by a row in the delta column (of any context, a deleted one included: its rows stay servable), an absorb record (an absorbed replay reads its trigger back) or a delta a live DAG holds (pending deltas have no row), over at most `MAX_ORPHAN_LIVE_ROWS` claims. No context lock can cover it, and every receive path records a side row before its delta enters the DAG, so a row is deleted only when two sweeps an interval apart both found it unclaimed. Nothing reads a side row without its delta (the responder looks one up only after finding the row, so a pruned delta is `DeltaNotFound` either way). Side-table bytes count in `DiskPrune::bytes`; the delta slice is compacted by `delta_bytes()` and the two side tables together by the sweep's total. A new per-delta table must join `SideTable`, or it leaks the same way. `NamespaceGovOp` (governance DAG), `UnifiedOp` (the op-log, never pruned) and `ContextWarrantNonce` (per author device) are not per-delta side tables; the TEE fired markers are per trigger and stay. Context deletion keeps `Delta` and so keeps its side rows; a deleted context is never compacted (no `ContextMeta`)
- `add_blob`'s `expected_size` asserts a length the caller already
  knows; it is never a ceiling. Passing a cap through it rejects every
  correct blob under that cap. Bound a stream where the bytes arrive
  instead - see `sync/blobs.rs`
