# calimero-op-adapter - Per-Plane -> Unified-Log Encoders

Transitional pure-function adapter that maps each per-plane operation type onto the single `OpPayload` enum the unified causal log speaks.

## Package Identity

- **Crate**: `calimero-op-adapter`
- **Entry**: `src/lib.rs` (crate docs + the flat re-export facade; one module per plane holds the encoders themselves)
- **Key deps**: `calimero-op` (`OpPayload`/`ScopeId`, the unified log's vocabulary), `calimero-storage` (`Action`, `RotationLogEntry`, `Id` - the data and ACL plane source types), `calimero-governance-types` (`GroupOp`, `RootOp` - the governance plane source types), `calimero-account` (`AccountId`, `DeviceCert`, `verify_device_cert` - the account plane the credentials are checked against), `calimero-context-config` (`ContextGroupId`, `VisibilityMode`), `calimero-primitives` (`PublicKey`, `GroupMemberRole`)
- **Dev-deps**: `calimero-projection` (`ScopeState` - folds the encoded ops back down in tests to prove fold-equivalence), `calimero-authz` (`AclView` - the shape a receiver resolves a signature against, used only by the writer-plane test)

## Commands

```bash
# Build
cargo build -p calimero-op-adapter

# Test (all - 12 unit tests, no doc-tests)
cargo test -p calimero-op-adapter

# Test one plane (the test tree mirrors the module tree)
cargo test -p calimero-op-adapter tests::root

# Test a single case
cargo test -p calimero-op-adapter group_op_encoder_mapping -- --nocapture
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `payload_from_action(action: &Action) -> Option<OpPayload>` | fn | Data plane: `Action::Add`/`Action::Update` -> `OpPayload::Put`, `Action::DeleteRef` -> `OpPayload::Delete`. Always returns `Some` today; `Option` is reserved for a future non-state-changing action |
| `set_writers_payload(object: Id, entry: &RotationLogEntry) -> OpPayload` | fn | Access-control plane: a writer-set rotation -> `OpPayload::SetWriters { object, writers }`. Infallible - returns `OpPayload` directly, not `Option` |
| `payload_from_group_op(group: ContextGroupId, op: &GroupOp) -> Option<OpPayload>` | fn | Membership plane (per-group ops, already decrypted): maps auth-relevant `GroupOp` variants to `MemberAdded`/`MemberRemoved`/`AdminChanged`/`DeviceLinked`/`DeviceRevoked`/`AccountKeysRotated`/`DefaultCapabilitiesSet`/`MemberCapabilitySet`/`SubgroupVisibilitySet`; everything else -> `None` |
| `payload_from_root_op(op: &RootOp) -> Option<OpPayload>` | fn | Admin/namespace plane (root governance ops): maps to `AdminChanged`/`PolicyUpdated`/`MemberAdded`/`MemberJoinedWithDevice`/`DeviceLinked`/`SubgroupCreated`/`SubgroupReparented`/`SubgroupDeleted`; `KeyDelivery` -> `None`. Takes **no signer** - every arm reads the account off the op |
| `join_credential_binds(member: &AccountId, genesis, chain, cert) -> bool` | fn | The op-local half of credential admission: does this credential name `member`, and does it verify? Shared verbatim with the governance apply path |
| `join_credential_certifies(member: &PublicKey, genesis, chain, cert) -> bool` | fn | The same question for the one join op that names a **key** (`MemberJoinedViaTeeAttestation`, whose quote binds to the attested signing key) |

Every function is pure - no I/O, no state, no async. They only ever consume a per-plane source type and produce an `OpPayload` (or `None`, or a verdict). Assembling the rest of the `Op` (id, parents, author, hlc, signature) is always the caller's job.

## Where things live

One module per plane, because the planes are what the coverage docs are written
against - a reviewer asking "is this `GroupOp` variant folded?" opens exactly one
file to answer it. The modules are private; `src/lib.rs` re-exports every public
item flat, so `calimero_op_adapter::payload_from_root_op` works regardless of
which file it lives in.

```text
   per-plane source type                 this crate                    OpPayload
   ─────────────────────                 ──────────                    ─────────
   Action ─────────────────────────▶ data.rs ──────────────────▶ Put / Delete
   RotationLogEntry ───────────────▶ acl.rs ───────────────────▶ SetWriters
   GroupOp ────────────────────────▶ group.rs ─────────────────▶ MemberAdded / …
   RootOp ─────────────────────────▶ root.rs ──────────────────▶ AdminChanged / …
                                          │
                                          ▼
                                    credential.rs   ◀── the governance apply path
                                    the op-local admission predicates, called
                                    from BOTH places on purpose: a rule stated
                                    twice is how one node folds a device its
                                    peer refuses

                                    that is deleted first at cutover
```

`root.rs` is the only module with an intra-crate edge (`credential.rs`); the four
plane encoders are otherwise independent of each other.

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
| `src/lib.rs` | Crate docs (the WHY), module declarations, and the flat `pub use` facade |
| `src/data.rs` | Data plane: `payload_from_action` |
| `src/acl.rs` | Access-control plane: `set_writers_payload` |
| `src/group.rs` | Membership plane: `payload_from_group_op` + its in-model/out-of-model coverage doc |
| `src/root.rs` | Admin/namespace plane: `payload_from_root_op` + its coverage doc and caveats |
| `src/credential.rs` | `join_credential_binds`, `join_credential_certifies`, and the private `credential_binds_the_member` dispatcher over the join variants |
| `src/tests.rs` | Declares the test tree; every test in the crate lives under `src/tests/` |
| `src/tests/<module>.rs` | Tests for the module of the same name, reaching it through `crate::` paths |
| `src/tests/support.rs` | Shared fixtures (`authorship_of`, `hlc`, `real_join_account_for`, `test_join_account_for`) |

## Invariants and Gotchas

- **Coverage docs are the contract, not decoration**: each function's doc comment enumerates every plane variant and says explicitly why it is or isn't folded. When `GroupOp` or `RootOp` gains a variant, decide in-model vs out-of-model there before writing the match arm - don't just silently add it to `_ => None`.
- **`#[non_exhaustive]` upstream means new variants default to dropped**: nothing here fails to compile when `GroupOp`/`RootOp` grow a case. Only the fold-equivalence tests in this crate and in `calimero-governance-store` (`prefix_walk_resolution_matches_reference_under_random_inputs`) catch a wrongly-dropped auth-relevant variant. Treat those tests as the real safety net, not the type system.
- **`GroupOp::MemberRoleSet` and `MemberJoinedViaTeeAttestation` collapse to the same `MemberAdded`** as a fresh add - a role change is a re-assert, and `ScopeState`'s per-`(group, member)` LWW keeps whichever write has the latest HLC, so re-encoding a role change as "add" rather than a separate "role changed" op is correct, not lossy.
- **`GroupCreated`'s `restricted` flag round-trips as-is** (`RootOp::GroupCreated.restricted` -> `OpPayload::SubgroupCreated.restricted` directly, since #2771 carries visibility atomically on the live op) - do not hardcode `false` here again; check the op before assuming the old "always Restricted" behavior still applies.
- **`GroupDeleted` maps only `root_group_id`** - the op's `cascade_group_ids` are not expanded into multiple `SubgroupDeleted` payloads by this crate; the live apply path is responsible for emitting one `SubgroupDeleted` per cascaded scope.
- **`MemberJoined`/`MemberJoinedAt` decode `group_id` and role off the admin-signed invitation**, not off caller-supplied fields - the joiner cannot escalate their own role because the invitation (and its `invited_role`) is under the *admin's* signature.
- **Two credential fixtures, and picking the wrong one silently inverts a test**: `real_join_account_for` mints a credential that actually passes `verify_device_cert`; `test_join_account_for` is filler whose signature does not verify. A test asserting the device half folds needs the first - handed the second, it asserts `Noop`/`MemberAdded` and passes for the wrong reason.
- **`from_parts` vs `new`**: this crate never constructs a full `Op`, only the payload - but every caller pairs it with `Op::from_parts` (explicit id, mirroring the source DAG node), never `Op::compute_id`/`Op::new`. Encoded ops from this crate are internal, unsigned projections of already-verified governance ops and are not passed through `Op::verify`.

Part of [crates/](../AGENTS.md).
