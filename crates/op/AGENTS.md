# calimero-op - Unified Op Envelope

The single `Op` envelope type carried by every scope's causal log, plus its canonical id and root hashing.

## Package Identity

- **Crate**: `calimero-op`
- **Entry**: `src/lib.rs` (single file, ~565 lines, about a third of it tests)
- **Key deps**: `borsh` (canonical wire format, derives), `sha2` (`Sha256` for id/root hashing), `calimero-context-config` (`ContextGroupId`, `MemberCapabilities`), `calimero-primitives` (`GroupMemberRole`, `PublicKey`), `calimero-storage` (`address::Id`, `entities::OpMask`, `logical_clock::HybridTimestamp`)

## Commands

```bash
# Build
cargo build -p calimero-op

# Test (all)
cargo test -p calimero-op

# Test a single case
cargo test -p calimero-op op_payload_discriminants_are_pinned -- --nocapture
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `ScopeId` | struct (`[u8; 32]`) | Stable id of a visibility scope (governance root, a context, a subgroup, ...); each scope is its own replication/encryption/convergence domain |
| `ScopeId::as_bytes()` | fn | Borrow the raw 32 bytes |
| `ScopeId::from([u8; 32])` | `From` impl | Construct from raw bytes |
| `Op` | struct | The one envelope for every change: `scope`, `parents`, `author`, `hlc`, `payload`, `expected_scope_root`, `signature`, plus a private cached `id` |
| `Op::new(scope, parents, author, hlc, payload, expected_scope_root, signature)` | fn | Builds an op and computes `id` from the content, so id and content can never disagree |
| `Op::from_parts(id, scope, parents, author, hlc, payload, expected_scope_root, signature)` | fn | Builds an op from an **explicit** id, for the unified-op bridge (see Mental Model) - bypasses `verify` |
| `Op::id()` | fn | Returns the cached content-address id |
| `Op::verify()` | fn | Recomputes the id from content and checks the Ed25519 signature against `author`; MUST pass before any op is folded |
| `Op::compute_id(scope, parents, author, hlc, payload)` | fn | The canonical hash function (see Mental Model) |
| `OpPayload` | enum | The change itself, across four planes: data, access-control, membership, admin/namespace, plus a capability plane and a graph-only `Noop` - NOT `#[non_exhaustive]` |
| `scope_root(entities_root, acl_hash, groups_root)` | fn | Combines the three projection component hashes into the one convergence root for a scope |

`OpPayload` variants: `Put`, `Delete` (data); `SetWriters` (access-control); `MemberAdded`, `MemberRemoved` (membership); `AdminChanged`, `PolicyUpdated`, `SubgroupCreated`, `SubgroupReparented`, `SubgroupDeleted`, `SubgroupVisibilitySet` (admin/namespace); `DefaultCapabilitiesSet`, `MemberCapabilitySet` (capability); `Noop` (graph-only, no projection effect).

## Mental Model

Everything that can happen to a scope - a data write, a writer-set rotation, a membership change, an admin/policy change - is the same `Op`, carried by the generic `CausalDelta<T>` / `DagStore<T>` transport. A scope's state is the deterministic projection of its op-log (`calimero-projection`); its single `scope_root` is the only convergence signal; authorization is one fold over the op's causal cut (`calimero-authz`). This crate is the small foundation underneath all of that: just the envelope type and the two hash functions.

**Id computation.** `Op::compute_id` hashes `Sha256(scope ‖ length-prefixed sorted(parents) ‖ author ‖ borsh(hlc) ‖ borsh(payload))`. Two properties matter:
- Parents are sorted before hashing, so `id` does not depend on the order a builder happened to list causal predecessors in.
- The parent list is length-prefixed (`sorted.len() as u64`) before the parent bytes, so a boundary shift between parents and the following `author` field can never produce a hash collision (`parents=[A,B], author=C` vs `parents=[A,B,C], author=...`).

`Op::new` always computes `id` from content, so the id and content can never desync when signing. `Op::from_parts` is the one escape hatch: it exists only for the unified-op *bridge*, where a `SignedNamespaceOp` / rotation entry from the governance DAG already has its own identity (`content_hash`/`delta_id`) and the unified `Op` mirrors it verbatim, keyed by that same id rather than by `compute_id` of the payload. Bridge ops built this way are internal, unsigned projections of already-verified governance ops and are never passed through `verify`. Anything freshly, independently signed must go through `Op::new`.

**Signature is over the id, not folded into it.** `signature` is an Ed25519 signature by `author` over `id` (the `compute_id` preimage). It is deliberately *not* part of the preimage itself - signing the id and then hashing the signature back into the id would be circular. `Op::verify()` does two checks: recompute the id from content and compare, then verify the signature against `author`. `calimero-projection` and `calimero-authz` assume every `Op` they see has already passed `verify()` - they fold/authorize on content alone with no signature check of their own. Feeding an unverified op into either bypasses authentication entirely.

**`expected_scope_root` is an assertion, not a trust input.** It records what the author expects `scope_root` to be after applying the op, but it is deliberately excluded from the `compute_id` preimage (kept unsigned). Peers recompute their own `scope_root` from their own projection and compare against it - a tampered value can flag a spurious divergence at worst, never grant authority. No security property depends on this field.

**Root hashing.** `scope_root(entities_root, acl_hash, groups_root) = Sha256(entities_root ‖ acl_hash ‖ groups_root)`. Folding the ACL/membership hash into the same root as the data hash is what makes a hash-neutral writer/membership rotation impossible to hide: any divergent authorization state necessarily produces a divergent root, so sync can never declare convergence while ACLs disagree. `calimero-op` only provides the combining function; `calimero-projection` computes the three component hashes from a `ScopeState`.

**Borsh discriminants are append-only, forever.** An op's id is a hash over `borsh(payload)`, and borsh encodes an enum variant by its positional (declaration-order) tag byte. Reordering, inserting in the middle, or removing an `OpPayload` variant renumbers every later variant's tag, silently changing the `id` (and therefore invalidating the `signature`) of every already-persisted op using one of the shifted variants. New variants may only be appended at the end. The test `op_payload_discriminants_are_pinned` hard-codes the expected tag for every current variant and fails to compile if a variant is added without updating the exhaustive match - that failure is the trip-wire for this rule.

`OpPayload` is intentionally not `#[non_exhaustive]`: `calimero-authz` authorizes ops via an exhaustive match, so a new variant should fail to *compile* there until someone gives it an authorization rule, rather than silently falling into a catch-all arm.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Everything: `ScopeId`, `Op`, `OpPayload`, `compute_id`, `scope_root`, and all tests |

## Invariants and Gotchas

- **Never reorder, insert into the middle of, or remove an `OpPayload` variant.** Only append. This is the single most consequential rule in the crate - violating it silently invalidates every already-signed op that used a shifted variant. `op_payload_discriminants_are_pinned` is the guard; keep it in sync with any addition.
- **`Op::from_parts` skips `verify`-worthiness by design.** It exists solely for the governance-DAG bridge, where the id already comes from a verified source. Do not reach for it for freshly authored ops - use `Op::new` so the id is a true content address.
- **Never trust `expected_scope_root` for authorization or convergence decisions.** It is excluded from the id preimage on purpose; always recompute and compare against a locally-derived `scope_root`.
- **`id` is a private field for a reason** - construct it only through `Op::new`/`Op::from_parts` so it can never drift from the content it addresses; read it via `Op::id()`.
- **Every op crossing a trust boundary (deserialized, received from a peer) must pass `Op::verify()` before being folded.** Downstream crates (`calimero-projection`, `calimero-authz`) do not re-check signatures.
- **Parent sort order is a hashing detail, not a semantic one**: `compute_id` sorts `parents` before hashing so id computation is order-independent, but the `Op.parents` field itself preserves whatever order the caller supplied.
- **`parents` may cross scopes.** A subgroup op may reference an ancestor governance scope's head (subgroup members are ancestor members by construction) - one causal model spans both data and governance, don't assume `parents` stays within `scope`.

Part of [crates/](../AGENTS.md).
