# calimero-governance-store - Local Group-Governance Apply Pipeline

Signature/nonce-checked apply of signed group and namespace governance ops into per-group RocksDB state, plus the acked-broadcast machinery that publishes them to peers.

## Package Identity

- **Crate**: `calimero-governance-store`
- **Entry**: `src/lib.rs`
- **Key deps**: `calimero-context-client` (`SignedGroupOp`/`SignedNamespaceOp`/`GroupOp` wire types + `AckRouter`), `calimero-store` (typed RocksDB column families), `calimero-op` / `calimero-op-adapter` (unified causal-log `Op`/`OpPayload`, decoded from applied ops), `calimero-dag` (`CausalDelta`), `calimero-node-primitives` (`NodeClient`, gossip sizing constants)

Extracted from `crates/context/src/group_store` in #2307 (closes epic #2300); `calimero-context` still re-exports this crate's public surface through compat shims so external call sites did not need to change imports.

## Commands

```bash
# Build
cargo build -p calimero-governance-store

# Test (all - 434 tests as of this writing)
cargo test -p calimero-governance-store

# Test a single case
cargo test -p calimero-governance-store permission_checker_enforces_admin_and_capability_rules -- --nocapture

# Test the undecidable-authority (F5) apply-gate behavior
cargo test -p calimero-governance-store undecidable_authority_parks -- --nocapture

# Test op-events (process-global broadcast bus - runs serialized via serial_test)
cargo test -p calimero-governance-store tee_member_removed_event_tests -- --test-threads=1
```

## Module Map

| Module | Purpose |
| --- | --- |
| `lib.rs` | `GroupHandle` / `NamespaceHandle` / `GroupStoreIndex` facades, `apply_local_signed_group_op[_at_cut]`, `sign_apply_local_group_op_borsh`, post-apply state-hash divergence check |
| `ops::group` (`ops/group.rs` + `ops/group/*.rs`) | One file per `GroupOp` variant (`member_added.rs`, `member_removed.rs`, `group_key_rotated.rs`, `transfer_ownership.rs`, ...); `dispatch()` is a thin match routing to each |
| `ops::namespace` (`ops/namespace.rs` + `ops/namespace/*.rs`) | One file per `RootOp` variant (`namespace_created.rs`, `group_created.rs`, `member_joined.rs`, `admin_changed.rs`, ...) |
| `namespace/` (`core`, `dag`, `governance`, `membership`, `op_log`, `retry`) | `NamespaceGovernance` (namespace-scoped DAG apply/publish), `NamespaceDagService` (head tracking), `NamespaceRepository` (identity/topology), retry of encrypted ops once a key arrives |
| `membership/` (`core`, `policy`, `policy_rules`, `status`, `view`) | `MembershipRepository` (member rows, ancestor walks, admin/capability checks), `MembershipPolicy`, `GroupMembershipView` |
| `governance_broadcast.rs` | `publish_and_await_ack_namespace`, `AckRouter`-based ack collection, `PublishReadiness` classification, per-op-kind timeouts |
| `group_governance_publisher.rs` | `GroupGovernancePublisher` - orchestrates sign + local apply + encrypted namespace publish for group ops, including removal-triggered key rotation |
| `governance_signer.rs` | `GovernanceSigner` - signing-key resolution for a group/namespace identity |
| `unified_op_decode.rs` | Decodes an applied `SignedNamespaceOp`/`GroupOp` into a `calimero_op::Op` for the unified causal log, on the same store handle as the gov-DAG write |
| `authorizer.rs` | `AtCutAuthorizer` trait - the apply-time authorization seam (see Mental Model) |
| `permission_checker.rs` | `PermissionChecker` - admin/capability gate wrapper used by `GroupApplyCtx` |
| `cascade.rs` / `cascade/walk.rs` | Descendant-subgroup fan-out walk for `CascadeTargetApplicationSet` / `CascadeGroupMigrationSet` |
| `absorb.rs` / `absorb_record.rs` | `AbsorbRepository` - durable buffer for stale-schema (future-version) straggler deltas that can't yet be applied |
| `meta.rs` / `metadata.rs` | `MetaRepository` (group meta row, state-hash computation), `MetadataRepository` (arbitrary metadata records for group/member/context) |
| `capabilities.rs` | `CapabilitiesRepository` - per-member and per-context-member capability bitmasks |
| `group_keys.rs` | `GroupKeyring` - per-group symmetric key storage and epoch lookup |
| `nonce_window.rs` | Per-signer sliding nonce window (anti-replay + out-of-order-sibling tolerance) |
| `deny_list.rs`, `signing_keys.rs`, `pending_rotation.rs`, `pending_self_purge.rs`, `upgrades.rs`, `upgrade_ladder.rs`, `tee.rs`, `context_registration.rs`, `context_tree.rs`, `contexts.rs`, `group_settings.rs` | Smaller per-domain repositories (deny-listed members, group signing keys, pending FS rotations/self-purges, app upgrade state, TEE admission records, context<->group registration) |
| `op_events.rs` | `OpEvent` enum + process-global broadcast `notify()`/subscribe - fired only after an op is durably logged |
| `registration_notify.rs` | Notification hook fired when a context finishes registering into a group |
| `metrics.rs` | Prometheus counters/histograms for apply outcomes and publish delivery |
| `errors.rs` | Per-domain typed error enums (`ApplyError`, `MembershipError`, `NamespaceError`, ...), recovered via `eyre::Report::downcast_ref` |
| `local_state.rs` | Op-log append/read, op-head read/advance, local nonce persistence - the low-level store-write primitives `lib.rs`'s apply functions call |

## Mental Model: Apply Pipeline and Broadcast

**Two apply entry points, one shared mutation core.** A group op reaches local state through either `apply_local_signed_group_op[_at_cut]` (direct group-DAG apply - local replay, tests) or `NamespaceGovernance::apply_signed_op` (the gossipsub-receive path, which unwraps an encrypted `NamespaceOp::Group` first). Both funnel into `apply_group_op_mutations`, which builds a `GroupApplyCtx` and calls `ops::group::dispatch`. `dispatch` is a flat match over every `GroupOp` variant, each handled by its own `ops/group/<variant>.rs` file - this is why the module list above is so long: the file-per-variant split keeps each apply handler's authorization + mutation logic independently reviewable and testable, instead of one giant match arm.

**Apply order per op**: validate size bounds (`op.validate()`, `MAX_PARENT_OP_HASHES`) -> verify Ed25519 signature -> check the per-signer nonce window (dedup/replay guard, short-circuits with `Ok(())` on a stale nonce) -> run the mutation (`apply_group_op_mutations`) -> record the nonce -> idempotent op-log append keyed by content hash (a second dedup gate covers the case where `apply_group_op_mutations` re-ran on a DAG replay after a crash between the nonce-window write and the op-log append) -> advance `dag_heads`/`sequence` -> flush any `OpEvent`s queued during apply. Every handler MUST be idempotent on re-apply (e.g. `MemberAdded` is an upsert, never an insert-or-error) because this replay-then-dedup sequencing re-runs the mutation before checking whether the op was already logged.

**Authorization is resolved at the op's causal cut, not live state.** `AtCutAuthorizer` (`authorizer.rs`) is the seam: this crate defines the trait and calls it from `GroupApplyCtx`'s admin/capability gates, but the real implementation (backed by the unified projection folded up to the op's `parent_op_hashes`) lives in `crates/context`, which depends on this crate - so the dependency is inverted through the trait. `None` from any `AtCutAuthorizer` method means "not yet decidable here" (the projection hasn't folded that far), and every method contracts to return `None` on an empty cut. `LiveFallbackAuthorizer`/`LIVE_FALLBACK_AUTHORIZER` is the default no-op implementation used by plain `apply_local_signed_group_op` (tests, replay) where no projection context is wired up. When the cut genuinely can't be resolved, the apply returns `ApplyError::AuthorityUndecidable` rather than guessing from live rows - guessing would make the verdict depend on this replica's fold progress and could silently diverge two peers forever; `AuthorityUndecidable` instead stalls (head not advanced, nonce not burned) until the missing ancestry arrives and the op is retried.

**Post-apply, state hashes are compared, never enforced.** `MemberRemoved`/`MemberLeft` carry `expected_group_state_hash` / `expected_context_state_hashes` that the signer precomputed before applying locally. `verify_post_apply_state_hashes` recomputes the receiver's own post-apply hashes and logs a structured warn on mismatch - it never rolls back the apply (the signed op is already valid) or errors. A mismatch signals cross-DAG divergence to be healed by reconcile-via-anchor sync, not a failure of this op.

**Broadcast is acked but not blocking.** `GroupGovernancePublisher::sign_apply_and_publish[_removal|_rotation]` signs, applies locally, encrypts as `NamespaceOp::Group`, and calls `publish_and_await_ack_namespace` (`governance_broadcast.rs`), which publishes on the `ns/<namespace_id>` gossipsub topic and waits (per-op-kind timeout: cheap/member-change/heavy) for `min_acks` distinct `SignedAck`s from namespace members. The local apply already committed before publish is attempted - a timeout or zero-ack result (`PublishReadiness::Degraded`/`Solo`) is not a failure signal, just "no one confirmed yet, sync will catch peers up eventually." Only `Publish` (gossipsub itself rejected the message) and `NoAckReceived`-with-`min_acks>0`-and-zero-mesh-peers are hard errors.

**The unified causal log rides along, not behind.** `unified_op_decode::decode_*` builds a `calimero_op::Op` from the just-applied `SignedNamespaceOp`/`GroupOp` and persists it to the op-store on the SAME `Store` handle as the gov-DAG write, in this crate (not in `calimero-context`, which can't see the projection but does consume these decode functions) - so the op-store can never lag the governance DAG it mirrors.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Apply entry points, `GroupHandle`/`NamespaceHandle`/`GroupStoreIndex`, divergence-hash check, `now_millis`/`now_secs` |
| `src/ops/group.rs` | `dispatch()` - the `GroupOp` apply router |
| `src/ops/group/member_removed.rs`, `member_left.rs`, `group_key_rotated.rs` | Forward-secrecy-relevant handlers - removal/leave/rotation interplay |
| `src/namespace/governance.rs` | `NamespaceGovernance::apply_signed_op` - the gossipsub-receive entry point, key unwrap, divergence report plumbing |
| `src/authorizer.rs` | `AtCutAuthorizer` trait + `LiveFallbackAuthorizer` |
| `src/governance_broadcast.rs` | Ack collection, `PublishReadiness`, per-op timeouts, `verify_ack`/`sign_ack` |
| `src/group_governance_publisher.rs` | `GroupGovernancePublisher` - the sign+apply+publish orchestration entry point most handlers call |
| `src/unified_op_decode.rs` | `Op` construction for the unified causal log |
| `src/errors.rs` | All typed error enums and the downcast contract |
| `src/nonce_window.rs` | Anti-replay nonce window implementation |

## Invariants and Gotchas

- **Serialize per group, not globally.** `apply_local_signed_group_op` is documented as requiring callers to serialize access per `group_id` (the node relies on the single-threaded actix `ContextManager` actor for this). Concurrent calls for the *same* group from multiple threads are not safe and can mint duplicate sequence numbers; different groups are independent.
- **`parent_op_hashes` are not validated against current `dag_heads`.** An op may cite ancestors further back in the DAG - this is fine because authorization is checked against the op's own causal cut (via `AtCutAuthorizer`), not against `dag_heads`, and `DagStore` topologically orders independently.
- **Every `GroupOp`/`RootOp` handler must be idempotent on re-apply.** The replay-then-dedup ordering in `apply_local_signed_group_op_at_cut` re-runs the mutation before the op-log dedup check fires; a handler that errors on "already applied" (instead of upserting) leaves the nonce window unpersisted and the node stuck retrying forever.
- **`OpEvent`s are dropped, not re-emitted, on a deduped replay.** If the op-log already contains the op's content hash, `pending_events` from the re-run mutation are discarded rather than re-notified - firing them again on every ordinary duplicate (re-gossip, DAG replay) would be worse than the rare one-time-signal loss on a crash landing exactly between op-log append and event flush.
- **`AtCutAuthorizer` methods must return `None` for an empty cut.** A genesis op has no causal context to resolve against; an implementation that returns `Some` there could falsely reject an op a live admin legitimately signed.
- **`ensure_rotation_is_publishable` runs before any mutation, not after.** A `MANAGE_MEMBERS`-capability (non-admin) removal from a group encrypted under its own key would mint a rotation peers reject (rotation requires admin), leaving this node's peers permanently unable to decrypt its future ops. The publisher fails closed by refusing the removal outright, before the local apply happens - bailing afterward would leave the removal committed but never published.
- **`AbsorbRepository` buffers verbatim bytes, never deserializes.** A future-schema (newer binary version) straggler entity is stored as opaque bytes keyed by the *sender's* schema/app-key, not interpreted by the receiving (older) binary - protects against corrupting or rejecting bytes the receiver's schema can't parse yet.
- **`calimero-projection` is a dev-dependency for fold-equivalence cross-checking** (the unified projection's membership fold against this crate's live `MembershipRepository` resolver over the same op sequences) - `serial_test` is also dev-only, needed because the `op_events` tests observe a process-global broadcast bus that would cross-talk under parallel test execution.
- **`PLACEHOLDER_ADMIN_IDENTITY` (`[0u8; 32]`) is a safe sentinel, not an `Option` dodge.** It decodes to a torsion point outside the Ed25519 prime-order subgroup, so no real keypair's public key can ever equal it - the genesis established-check can rely on the cheap `[u8; 32]` comparison without risking collision with a legitimate admin key.

Part of [crates/](../AGENTS.md).
