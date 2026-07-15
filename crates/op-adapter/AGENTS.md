# calimero-op-adapter - Per-Plane -> Unified-Log Encoders

Transitional pure-function adapter that maps each per-plane operation type onto the single `OpPayload` enum the unified causal log speaks.

## Package Identity

- **Crate**: `calimero-op-adapter`
- **Entry**: `src/lib.rs` (one file: 3 public functions + tests, no submodules)
- **Key deps**: `calimero-op` (`OpPayload`/`ScopeId`, the unified log's vocabulary), `calimero-storage` (`Action`, `RotationLogEntry`, `Id` - the data and ACL plane source types), `calimero-governance-types` (`GroupOp`, `RootOp` - the governance plane source types), `calimero-context-config` (`ContextGroupId`, `VisibilityMode`), `calimero-primitives` (`PublicKey`, `GroupMemberRole`), `calimero-authz` (unused in `lib.rs` directly but pulled in as the crate whose exhaustive `OpPayload` match these encoders must stay compatible with)
- **Dev-deps**: `calimero-projection` (`ScopeState` - folds the encoded ops back down in tests to prove fold-equivalence)

## Commands

```bash
# Build
cargo build -p calimero-op-adapter

# Test (all - 6 unit tests, no doc-tests)
cargo test -p calimero-op-adapter

# Test a single case
cargo test -p calimero-op-adapter group_op_encoder_mapping -- --nocapture
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `payload_from_action(action: &Action) -> Option<OpPayload>` | fn | Data plane: `Action::Add`/`Action::Update` -> `OpPayload::Put`, `Action::DeleteRef` -> `OpPayload::Delete`. Always returns `Some` today; `Option` is reserved for a future non-state-changing action |
| `set_writers_payload(object: Id, entry: &RotationLogEntry) -> OpPayload` | fn | Access-control plane: a writer-set rotation -> `OpPayload::SetWriters { object, writers }`. Infallible - returns `OpPayload` directly, not `Option` |
| `payload_from_group_op(group: ContextGroupId, op: &GroupOp) -> Option<OpPayload>` | fn | Membership plane (per-group ops, already decrypted): maps auth-relevant `GroupOp` variants to `MemberAdded`/`MemberRemoved`/`AdminChanged`/`DefaultCapabilitiesSet`/`MemberCapabilitySet`/`SubgroupVisibilitySet`; everything else -> `None` |
| `payload_from_root_op(op: &RootOp, signer: PublicKey) -> Option<OpPayload>` | fn | Admin/namespace plane (root governance ops): maps to `AdminChanged`/`PolicyUpdated`/`MemberAdded`/`SubgroupCreated`/`SubgroupReparented`/`SubgroupDeleted`; `KeyDelivery` -> `None` |

All four functions are pure - no I/O, no state, no async. They only ever consume a per-plane source type and produce an `OpPayload` (or `None`). Assembling the rest of the `Op` (id, parents, author, hlc, signature) is always the caller's job.

## Mental Model: Bridging Four Planes onto One Log

The system is mid-migration from four separate stores (data Merkle, ACL rotation log, per-group governance log, namespace root governance log) to one unified causal log keyed by `OpPayload`. This crate is the seam: each per-plane apply path still produces its native type (`Action`, `RotationLogEntry`, `GroupOp`, `RootOp`), and one function here re-expresses that same fact as an `OpPayload` so a `calimero-projection::ScopeState` can fold it alongside ops from the other three planes and reach the *same* answer (ACL, membership, admin) that the legacy per-plane resolvers give today.

The crate does not decide what the unified system's semantics are - `OpPayload` (in `calimero-op`) and `ScopeState` (in `calimero-projection`) own that. This crate is only the translation layer, and it is explicitly transitional: `lib.rs`'s doc comment says it "and the per-plane source types it reads" get deleted once everything runs on `OpPayload` directly - the day nothing sources from `Action`/`RotationLogEntry`/`GroupOp`/`RootOp` any more, this crate has no reason to exist.

Each encoder's rustdoc is the actual spec for its plane, cataloguing:
- **in-model** variants - the ones that move the unified `authorize` decision (membership, admin, ACL, the visibility/capability bits that gate inheritance);
- **out-of-model** variants, by design, not by omission - app/upgrade config, metadata, TEE-policy, key transport, the context<->group binding (that one lives in a separate index because `authorize` needs it *at auth time*, not folded into a scope's `ScopeState`).

Both `GroupOp` and `RootOp` are `#[non_exhaustive]` upstream, so every match here carries a mandatory `_ => None` arm. That means a brand-new upstream variant silently lands in "out-of-model" by default - there is no compiler error to catch a forgotten wire-up. The safety net is the fold-equivalence property tests (here and in `calimero-governance-store`): if a new auth-relevant variant should have been folded but wasn't, `acl_plane_matches_resolve_local_*` / `prefix_walk_resolution_matches_reference_under_random_inputs` diverge from the legacy resolver and fail.

## Consumers

- **`calimero-governance-store`** (`src/unified_op_decode.rs`) imports `payload_from_group_op` and `payload_from_root_op` to build the unified `Op` that the governance apply path writes to the op-store on the *same store handle* as the gov-DAG write, so the two writes are atomic.
- **`calimero-context`** (`src/scope_projection.rs`) imports `set_writers_payload` to feed ACL rotations into the per-scope `ScopeState` that backs `acl_view_at`.
- Both consumers pair the payload from this crate with `Op::from_parts` (not `Op::new`): the unified op mirrors the source op's own id/parents (its `delta_id`/`content_hash`) rather than computing a fresh content address, so the projection's op graph shares an id space with the source DAGs.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | The four encoders, their per-plane in-model/out-of-model coverage docs, and all 6 tests |

## Invariants and Gotchas

- **Coverage docs are the contract, not decoration**: each function's doc comment enumerates every plane variant and says explicitly why it is or isn't folded. When `GroupOp` or `RootOp` gains a variant, decide in-model vs out-of-model there before writing the match arm - don't just silently add it to `_ => None`.
- **`#[non_exhaustive]` upstream means new variants default to dropped**: nothing here fails to compile when `GroupOp`/`RootOp` grow a case. Only the fold-equivalence tests in this crate and in `calimero-governance-store` (`prefix_walk_resolution_matches_reference_under_random_inputs`) catch a wrongly-dropped auth-relevant variant. Treat those tests as the real safety net, not the type system.
- **`GroupOp::MemberRoleSet` and `MemberJoinedViaTeeAttestation` collapse to the same `MemberAdded`** as a fresh add - a role change is a re-assert, and `ScopeState`'s per-`(group, member)` LWW keeps whichever write has the latest HLC, so re-encoding a role change as "add" rather than a separate "role changed" op is correct, not lossy.
- **`GroupCreated`'s `restricted` flag round-trips as-is** (`RootOp::GroupCreated.restricted` -> `OpPayload::SubgroupCreated.restricted` directly, since #2771 carries visibility atomically on the live op) - do not hardcode `false` here again; check the op before assuming the old "always Restricted" behavior still applies.
- **`GroupDeleted` maps only `root_group_id`** - the op's `cascade_group_ids` are not expanded into multiple `SubgroupDeleted` payloads by this crate; the live apply path is responsible for emitting one `SubgroupDeleted` per cascaded scope.
- **`MemberJoined`/`MemberJoinedAt` decode `group_id` and role off the admin-signed invitation**, not off caller-supplied fields - the joiner cannot escalate their own role because the invitation (and its `invited_role`) is under the *admin's* signature.
- **`from_parts` vs `new`**: this crate never constructs a full `Op`, only the payload - but every caller pairs it with `Op::from_parts` (explicit id, mirroring the source DAG node), never `Op::compute_id`/`Op::new`. Encoded ops from this crate are internal, unsigned projections of already-verified governance ops and are not passed through `Op::verify`.

Part of [crates/](../AGENTS.md).
