# calimero-components — CRDT-aware access-control components

Status: **Partially Implemented** (tracked in #2687; supersedes #2557; op-aware ACL slice of
#2541). Targets the merged enforcement substrate:
#2588 (collection guarding), #2230/#2601/#2612 (authenticated rotation),
#2655 (SharedMember anchor / retroactive revocation), #2665 (concurrent-rotation
convergence).

### Implementation status (as of 0.11.0-rc.2)

| Phase | Status | PR / commit | Notes |
|---|---|---|---|
| P0 — field-id registry | ✅ shipped | #2544 | `TypeId AssignFieldId` registry |
| P1 — `Ownable<T>` + `PermissionedStorage<T,A>` + `Authorizer` seam | ✅ shipped | #2700 (`c444e8ef`) | `WriterSetAcl`, `OwnerAcl`; `only_owner`/`transfer`/`renounce` |
| P2 — `AccessControl` roles | ✅ shipped | #2700 (`c444e8ef`) | `grant`/`revoke`; `AccessControl::project_onto` (#2741) |
| OpMask (§8) — WRITE/DELETE/ADMIN bits | ✅ shipped | #2735/#2736/#2741 (`bbcc9f3a`, `99fe48b4`, `744eaeb1`) | `OpMask` in writer map; `ProtocolAuthorizer` + `grant_capability` |
| OpMask — INSERT vs UPDATE distinction | ⚠️ deferred | — | `v2_upsert` tag split; existence-at-cut check (§8.9); see scorecard |
| P3 — `Pausable` (advisory) | 🔲 not started | — | |
| P4 — `#[component]` macro sugar | 🔲 not started | — | |

## 1. Motivation

App authors keep hand-rolling the same authorization patterns: an owner key, a
set of admins, a pause switch. Today they write them by hand against
`SharedStorage`/`UserStorage`, and the easy way to write them is **wrong**:

```rust
pub fn delete_entry(&mut self, k: String) -> app::Result<()> {
    require!(env::executor_id() == self.owner, "not owner"); // advisory only
    self.entries.remove(&k);
    Ok(())
}
```

That `require!` is **not a security boundary**. A malicious context member never
calls `delete_entry`; they run a patched node, craft a signed delta containing a
`DeleteRef` against the entry's entity, and gossip it. It reaches honest nodes
through the **sync/merge path**, which never runs the app's wasm. The check is
bypassed.

`calimero-components` exists to make the *correct* pattern the *easy* pattern:
ship `Ownable`, `AccessControl`, and `Pausable` as thin facades over
writer-set-guarded storage, so protected data physically lives inside an entity
that is signature-verified at merge — and app authors can't accidentally store
it somewhere world-writable.

## 2. The non-negotiable principle

> **Enforcement lives at merge, never in the method.**

A call-site guard (`only_owner()?`) is UX sugar: it fails fast with a good error
and saves a round-trip. The actual boundary is `Interface::verify_*_signature`
(`crates/storage/src/interface.rs:300`): for any `User` / `Shared` /
`SharedMember` entity, an unsigned action is `InvalidSignature`, and a signed
action must `ed25519_verify` against a key in the entity's writer set
(`interface.rs:368`). Deletes are guarded identically (`interface.rs:1481`).
`Public`/`Frozen` skip verification because there is nothing to guard.

Therefore: a component is secure **iff the data it guards is stored inside a
`Shared`/`SharedMember`/`User` entity whose writer set encodes the policy.** The
component's whole job is to guarantee that placement and keep the API predicate
and the merge predicate in sync.

## 3. Primitives recap (what we build on)

| Storage type | Shape | Owner determined by | Transfer / rotation | Verified against |
|---|---|---|---|---|
| `User { owner }` | map of per-member self-owned slots | the writer (`executor_id`) | ❌ none | the one fixed owner |
| `Shared { writers }` | one guarded cell/collection | chosen explicitly | ✅ `rotate_writers` (signed, ADR 0001) | the writer set (any member) |
| `SharedMember { anchor }` | entries under a `Shared` anchor | inherited from anchor | ✅ rotate the anchor (retroactive) | anchor's writers, resolved at the causal cut |
| `Frozen` | immutable after freeze | n/a | ❌ | n/a (read-only) |

Components are built on **`Shared`/`SharedMember`** (not `User`) because they need
**transferable ownership** and the path to **roles** — both of which only exist on
the writer-set side. `UserStorage` stays the right primitive for genuinely
per-member private data (each member owns their own slot, no transfer).

## 4. Architecture

### 4.1 The `Authorizer` seam

One predicate, evaluated at two sites (API + merge), so they cannot drift.

```rust
/// What an action wants to do, for op-granular policies (#2541).
pub enum Op { Read, Write, Delete, Admin }

pub trait Authorizer {
    /// Is `who` permitted to perform `op` on the guarded resource, given its
    /// current writer set? Pure function of (who, op, writers) — no I/O, so the
    /// same call is valid at the API call-site and conceptually mirrors what
    /// merge enforces. An associated function (no `&self`): policies are
    /// zero-sized markers carried as a type parameter, not stateful values.
    fn authorize(who: &PublicKey, op: Op, writers: &BTreeSet<PublicKey>) -> bool;
}

/// Default: membership in the writer set authorizes any op. This is exactly
/// what merge-time `verify_*_signature` already enforces, so API and merge agree
/// by construction.
pub struct WriterSetAcl;
impl Authorizer for WriterSetAcl {
    fn authorize(who: &PublicKey, _op: Op, writers: &BTreeSet<PublicKey>) -> bool {
        writers.contains(who)
    }
}
```

As of #2736 (`ProtocolAuthorizer` + `grant_capability`) and #2735 (`OpMask`
merge-time enforcement), merge enforces **membership + op mask** (WRITE, DELETE,
ADMIN bits). `ProtocolAuthorizer` has shipped and resolves the causal op mask
from the anchor log at merge time — `WRITE` vs `DELETE` distinctions are now
merge-enforced. INSERT vs UPDATE (§8.9) remains advisory (deferred) until the
`exists_at_cut` query is implemented.

### 4.2 `Ownable<T>` — single transferable owner

A newtype over `SharedStorage<T>` constrained to a one-key writer set, with
ergonomic guards.

```rust
pub struct Ownable<T> {
    inner: SharedStorage<T>,
}

impl<T> Ownable<T> {
    /// New resource owned by `owner`.
    pub fn new(owner: PublicKey, value: T) -> Self { /* SharedStorage::new({owner}) + insert */ }

    /// New resource owned by the installer (common case).
    pub fn new_owned_by_caller(value: T) -> Self {
        Self::new(env::executor_id().into(), value)
    }

    pub fn owner(&self) -> Option<PublicKey> { self.inner.writers().into_iter().next() }

    /// Fail-fast API guard. NOT the security boundary — merge is (see §2).
    pub fn only_owner(&self) -> app::Result<()> {
        let me: PublicKey = env::executor_id().into();
        if self.owner() == Some(me) { Ok(()) } else { Err(NotOwner.into()) }
    }

    /// Read (anyone) / mutate (guarded at merge).
    pub fn get(&self) -> app::Result<&T> { Ok(self.inner.get()?) }
    pub fn get_mut(&mut self) -> app::Result<&mut T> { Ok(self.inner.get_mut()?) }

    /// Transfer ownership — signed, DAG-causal rotation. The thing `User` can't do.
    pub fn transfer_ownership(&mut self, new_owner: PublicKey) -> app::Result<()> {
        self.inner.rotate_writers(BTreeSet::from([new_owner]))?;
        Ok(())
    }

    /// One-way: freeze ownership so the resource becomes immutable.
    pub fn renounce_ownership(&mut self) -> app::Result<()> { /* freeze */ }
}
```

Security note: `transfer_ownership` is enforced at merge — a non-owner's forged
rotation delta is rejected (`forged_shared_rotation_rejected_at_merge`).
`only_owner()` is just the courtesy fail-fast.

### 4.3 `AccessControl` — roles as writer sets

A role *is* a `SharedStorage`-backed writer set; `grant`/`revoke` are rotations.
An admin role gates membership changes to the other roles.

```rust
pub struct AccessControl {
    /// role-name → guarded member set. Each role is its own anchor, so revoking
    /// a member is one anchor rotation and is retroactive (#2655).
    roles: UnorderedMap<String, SharedStorage<UnorderedSet<PublicKey>>>,
    admin_role: String,
}

impl AccessControl {
    pub fn has_role(&self, role: &str, who: &PublicKey) -> bool { /* ... */ }

    /// Fail-fast guard; merge is the boundary.
    pub fn only_role(&self, role: &str) -> app::Result<()> { /* has_role(role, executor) */ }

    /// Grant/revoke = rotate the role's writer set. Caller must hold admin_role;
    /// enforced at merge because the role set is itself a guarded entity whose
    /// writers are the admins.
    pub fn grant(&mut self, role: &str, who: PublicKey) -> app::Result<()> { /* rotate += who */ }
    pub fn revoke(&mut self, role: &str, who: PublicKey) -> app::Result<()> { /* rotate -= who */ }
}
```

Revoke is **remove-wins / retroactive** thanks to the `SharedMember` anchor model:
rotate the role's anchor and a revoked member's older signatures no longer verify
against the new causal writer set (#2655). This is the concrete win over a
hand-rolled `UnorderedSet<PublicKey>` guarded only by an app method.

**Do not** let `AccessControl` grow into a parallel permission system (#2541
forbids it). Keep it a thin map of writer sets so it composes with the protocol
authorizer later.

### 4.4 `Pausable` — advisory, honestly labeled

```rust
pub struct Pausable {
    paused_epoch: LwwRegister<u64>,    // pause-dominant
    unpaused_through: LwwRegister<u64>,
}
impl Pausable {
    pub fn is_paused(&self) -> bool { /* paused_epoch > unpaused_through */ }
    pub fn when_not_paused(&self) -> app::Result<()> { /* require !is_paused */ }
}
```

**Caveat that must ship in the docs:** there is no merge-time "reject because
paused" today. A patched node can still gossip a write while the group considers
itself paused. `when_not_paused()` is an **advisory** gate on the honest
execution path only. Merge-enforced pause requires freeze-rotation semantics and
is out of scope for v1. Pause-dominant epoch merge (a stale `unpause` can't lift a
newer `pause`) is the one real guarantee here, and it's a *convergence* property,
not an *authorization* one.

## 5. The `#[component]` macro & the field-id prerequisite (#2544)

### Problem
`#[app::state]` assigns each collection field a deterministic id by **string-
matching the field's type** (`is_collection_type` in
`crates/sdk/macros/src/state.rs`). A field typed `Ownable<Rga>` or
`SharedStorage<UnorderedMap<..>>` does **not** string-match → the inner storage
keeps a random id → it never merges with the same field on a peer → **silent
split-brain**. This is a P0 blocker for components wrapping collections.

### Fix (build before components ship)
Replace the string-match with a **TypeId registry**, mirroring
`crates/storage/src/collections/rekey.rs`:

- a trait `AssignFieldId` that types self-register in their constructor;
- the `#[app::state]` pass emits `assign_field_id_dyn(&mut self.field, "field")`
  for **every** field (no-op if the type isn't registered);
- `SharedStorage`/`Ownable`/`AccessControl` register themselves and propagate the
  field name to their inner collection.

Consequence: `#[component]` becomes **optional sugar** (opt-in compile-time
`T: Component` armor + generated `only_*` helpers), not a correctness requirement.
This is shared infrastructure with #2544 — do it once, there.

## 6. Examples

### 6.1 Ownable config cell

```rust
#[app::state]
pub struct Treasury {
    config: Ownable<LwwRegister<String>>,  // owner-gated settings blob
    balances: UserStorage<u64>,            // each member's own balance slot
}

#[app::logic]
impl Treasury {
    #[app::init]
    pub fn init() -> Self {
        Self {
            config: Ownable::new_owned_by_caller(LwwRegister::new("{}".into())),
            balances: UserStorage::new(),
        }
    }

    /// Owner-only. The require is fail-fast; the real gate is that `config` is a
    /// Shared entity whose sole writer is the owner — a forged delta is rejected
    /// at merge.
    pub fn set_config(&mut self, json: String) -> app::Result<()> {
        self.config.only_owner()?;                 // fail-fast UX
        *self.config.get_mut()?.get_mut() = json;  // enforced at merge
        Ok(())
    }

    pub fn get_config(&self) -> app::Result<String> {
        Ok(self.config.get()?.get().clone())       // anyone may read
    }

    /// Hand the treasury to a new owner. Signed rotation; a non-owner cannot.
    pub fn transfer(&mut self, new_owner: PublicKey) -> app::Result<()> {
        self.config.transfer_ownership(new_owner)
    }

    /// Each member writes only their own balance — UserStorage stamps owner =
    /// executor automatically, so nobody can forge another member's balance.
    pub fn set_my_balance(&mut self, amount: u64) -> app::Result<()> {
        self.balances.insert(amount)?;
        Ok(())
    }
}
```

This example deliberately contrasts the two primitives: `config` is a single
**transferable** owned resource (`Ownable`); `balances` is **per-member
self-owned** data (`UserStorage`) that is never transferred.

### 6.2 Role-gated collection

```rust
#[app::state]
pub struct Forum {
    acl: AccessControl,
    // Writers of `posts` are the "editor" role's members; an editor's delete is
    // enforced at merge, a non-editor's forged DeleteRef is rejected.
    posts: SharedStorage<UnorderedMap<u64, LwwRegister<String>>>,
}

#[app::logic]
impl Forum {
    #[app::init]
    pub fn init() -> Self {
        let me: PublicKey = env::executor_id().into();
        let acl = AccessControl::new(/*admin*/ me, &["editor"]);
        Self { posts: SharedStorage::new(BTreeSet::from([me]), false), acl }
    }

    pub fn add_editor(&mut self, who: PublicKey) -> app::Result<()> {
        self.acl.grant("editor", who)              // admin-gated rotation
    }

    pub fn remove_editor(&mut self, who: PublicKey) -> app::Result<()> {
        self.acl.revoke("editor", who)             // retroactive — old writes lose validity
    }

    pub fn delete_post(&mut self, id: u64) -> app::Result<()> {
        self.acl.only_role("editor")?;             // fail-fast
        self.posts.get_mut()?.remove(&id)?;        // merge rejects a non-editor's delete
        Ok(())
    }
}
```

The `delete_post` case is the exact attack the issue raised: even if an attacker
skips `delete_post` and crafts a `DeleteRef` delta directly, honest nodes reject
it because the `posts` entries are `Shared`/`SharedMember` and the delete must be
signed by a current editor (`interface.rs:1481`).

### 6.3 Test with `TestHost`

```rust
#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;
    use super::*;

    #[test]
    fn only_owner_can_set_config() {
        let mut app = TestHost::new(Treasury::init);     // caller is owner
        app.call(|s| s.set_config("{\"fee\":1}".into())).unwrap();
        assert_eq!(app.view(|s| s.get_config()).unwrap(), "{\"fee\":1}");
    }

    #[test]
    fn transfer_moves_ownership() {
        let mut app = TestHost::new(Treasury::init);
        let alice: PublicKey = app.executor_id().into();
        app.call(|s| s.transfer(alice)).unwrap();
        // post-transfer the original owner's fail-fast guard now refuses
    }
}
```

For the security property (forged delta rejected at merge), unit tests aren't
enough — it needs a 2-node merobox adversarial e2e in the shape of
`kv-store-with-shared-storage`'s workflow: a non-writer's forged write/delete is
applied locally, the honest node never converges to it.

## 7. Implementation plan (phased, each its own PR)

- **P0 — field-id registry (#2544).** ✅ **Shipped.** TypeId `AssignFieldId` registry replacing
  the macro string-match; components self-register.
- **P1 — `Ownable<T>` / `PermissionedStorage<T,A>` + `Authorizer`/`WriterSetAcl` seam.** ✅ **Shipped (#2700).** Facade over
  `WriterSetCell`; `only_owner`/`transfer`/`renounce`; `WriterSetAcl`/`OwnerAcl`. Unit tests
  via `TestHost` + adversarial e2e.
- **P2 — `AccessControl`.** ✅ **Shipped (#2700, #2741).** Roles as `SharedStorage`-backed sets; admin-gated
  `grant`/`revoke`; `AccessControl::project_onto` for role-based op-mask projection.
- **P3 — `Pausable` (advisory).** 🔲 Not started. Pause-dominant epoch merge; advisory caveat.
- **P4 — `#[component]` macro sugar + app migrations.** 🔲 Not started.
- **P5 — `ProtocolAuthorizer` (#2541).** ✅ **Shipped (#2736, #2735, #2741).** Op-granular bounds (WRITE/DELETE/ADMIN) enforced at merge via `OpMask` in the anchor writer map; `grant_capability` for per-principal mask assignment. INSERT vs UPDATE remains deferred (§8.9).

## 8. Protocol-level extension (#2541): `OpMask`

Components ship enforcing *membership* ("a writer may do anything"). #2541 is the
protocol-level extension that makes authorization **operation-aware** — enforced
at merge, bound into the signed delta — by attaching a per-principal `OpMask` to
the writer set and checking it after signature verification. Almost all the
substrate is already merged; the only genuinely new machinery is the op check.

### 8.1 Which ops are enforceable at merge

Merge only ever sees signed deltas, and a delta is one of three signed shapes
(`action.rs:164`):

| Signed payload tag | Action | Enforceable at merge? |
|---|---|---|
| `v2_upsert` | `Add` / `Update` | ✅ |
| `v2_delete` | `DeleteRef` | ✅ |
| `v2_compare` | `Compare` | n/a (unsigned reconciliation) |

`Read` is **not** merge-enforceable: in a replicated CRDT every node already holds
the bytes, so there is no apply step to reject. Real read-control needs encryption
(TEE / per-recipient keys), a separate mechanism. An `OpMask` `READ` bit may exist
as an advisory API gate or an "this scope is encrypted" marker, but it MUST NOT be
advertised as protocol-enforced. Enforceable ops: **Insert, Update, Delete, Admin.**

### 8.2 The mask

```rust
bitflags! {
    pub struct OpMask: u8 {
        const INSERT = 0b0000_0001;  // create a new entity (Add on a fresh id)
        const UPDATE = 0b0000_0010;  // modify an existing entity (Update)
        const DELETE = 0b0000_0100;  // DeleteRef
        const ADMIN  = 0b0000_1000;  // rotate writers / grant / revoke / change masks

        const APPEND = Self::INSERT.bits() | Self::UPDATE.bits();                   // write, no delete
        const WRITE  = Self::INSERT.bits() | Self::UPDATE.bits() | Self::DELETE.bits();
        const FULL   = Self::WRITE.bits() | Self::ADMIN.bits();
    }
}
```

The #2541 headline ("write but not delete") is `OpMask::APPEND`.

### 8.3 Where masks live + `capability_at`

The writer set gains a mask per key:

```rust
// today
Shared { writers: BTreeSet<PublicKey>, .. }
// extended
Shared { writers: BTreeMap<PublicKey, OpMask>, .. }
```

`capability_at(key, anchor_log, parents)` is `writers_at` with `OpMask` as the
resolved value — the same ADR-0001 causal fold over the anchor's rotation log
(merged, #2665). Because of the `SharedMember` anchor model, collection *entries*
commit only the anchor id (`action.rs` `hash_authorization_for_payload`), not
writers — so **masks live only at the anchor; entries are byte-unchanged.** Only
the anchor's representation changes.

### 8.4 The merge check (the one new thing)

```rust
// in verify_*_signature / apply_action, AFTER ed25519 verify succeeds:
let required = match verified_tag {
    Tag::Upsert if entity_exists_at(cut) => OpMask::UPDATE,
    Tag::Upsert                          => OpMask::INSERT,
    Tag::Delete                          => OpMask::DELETE,
};
let granted = capability_at(signer, anchor_log, delta.parents);
if !granted.contains(required) {
    return Err(StorageError::Unauthorized); // "not for THIS op", not just "not a writer"
}
```

This *replaces* today's binary `writers.contains(signer)`.

### 8.5 Grant / revoke = `ADMIN` op, retroactive by construction

A grant is an `ADMIN`-op action appended to the anchor log, authorized iff the
caller holds `ADMIN` at the cut (verified at merge like any rotation). `revoke =
grant(who, OpMask::empty())`. Because `capability_at` folds at the operation's
causal position (#2655 semantics): writes causally *before* a revoke stay valid;
writes *after* it are checked against the reduced mask → rejected. Retroactive
revocation falls out of the representation — no history rewrite.

### 8.6 Concurrent conflicting grants → intersection (least privilege)

Two concurrent entries setting different masks for one key merge by bitwise AND:

```rust
fn merge_concurrent(a: OpMask, b: OpMask) -> OpMask { a & b }   // revoke-wins, fail-safe
```

Deterministic, order-independent, default-deny — matches #2541's conflict rule.
Must resolve the *causal* set (reuse `writers_at` discipline), never insertion
order (the #2673 trap).

### 8.7 Back-compat + wire

- A key with no explicit mask resolves to `OpMask::FULL` → today's "in the set ⇒
  anything" preserved exactly.
- The **anchor** `Shared` writer set is committed into its signed payload, so
  `BTreeSet→BTreeMap<_,OpMask>` changes that hash → a wire/format change **at
  anchors only** (version tag in `entities.rs`); entries unaffected.

### 8.8 Status scorecard

| OpMask bit | Status |
|---|---|
| `DELETE` vs `WRITE` | ✅ **shipped** (#2735/#2736) — merge-time enforcement via `ProtocolAuthorizer`; op mask in anchor writer map |
| `ADMIN` | ✅ **shipped** (#2736) — `grant_capability` appends to anchor log as `ADMIN`-op action |
| `INSERT` vs `UPDATE` | ⚠️ deferred — needs `exists_at_cut` query; `v2_upsert` tag split optional hardening (§8.9) |
| `READ` | ❌ not merge-enforceable — needs encryption; advisory only |
| wire change | anchors only (writer map: `BTreeSet → BTreeMap<PublicKey, OpMask>`); entries unchanged |

### 8.9 What `INSERT` vs `UPDATE` specifically needs

`WRITE` vs `DELETE` is free because `v2_upsert` and `v2_delete` are already
distinct signed payloads. `INSERT` vs `UPDATE` is *not* — both ride `v2_upsert`
(`action.rs:164`) — so to require `INSERT` for creates and `UPDATE` for edits the
verifier must decide, for an incoming upsert, **whether the target already
exists**, and decide it identically on every node.

**The load-bearing mechanism: existence at the causal cut.** Every delta carries
its DAG parents, which pin an immutable causal position. Define *exists-at-cut* =
"an `Add` for this id is reachable from those parents and not superseded by a
tombstone." Because the parents are fixed, this is a **pure function of the
delta** — order-independent, identical on every node, no race. (A naive "does it
exist *now*" check against the local head is the wrong thing: it differs by apply
order → two nodes reach different authorization verdicts → split-brain.) Then:

```rust
let required = if exists_at_cut(id, delta.parents) { OpMask::UPDATE } else { OpMask::INSERT };
```

This is the same family of causal query as `writers_at` / `membership_status_at`:
fold the entity's lifecycle (`Add`/`Delete`) along the DAG up to the cut. The new
cost is one causal reachability query per upsert verify (amortizable via the
per-entity index).

**Concurrency falls out for free.** Two concurrent `Add`s of the same id each see
"not present at *my* cut" → both are INSERTs (both need `INSERT` in the mask),
then merge per CRDT. An edit whose creating `Add` is concurrent (not reachable
from its cut) is likewise an INSERT at its cut. No special-case rule, because each
delta is judged at its own fixed parents.

**The `v2_upsert` tag split is optional hardening, not a requirement.** Splitting
the tag into `v2_insert` / `v2_update` lets the signer *declare* intent, but the
tag alone can't be trusted — an `INSERT`-only signer could label an edit as an
insert — so it still has to be validated against `exists_at_cut`. The existence
check is what actually enforces the boundary; the tag is defense-in-depth (and a
signed-payload format change, so version-gated) worth adding only if we want the
intent committed for auditing/replay-hardening.

Cost/benefit: this buys "append-only / create-but-not-edit" (audit logs,
claim-once registries). Until a concrete use case wants it, ship `APPEND` as
`INSERT|UPDATE` (no existence query, no split) and keep `WRITE`/`DELETE`/`ADMIN` —
which cover the #2541 headline gap — as the v1 granularity.

## 9. Open decisions

1. **`Ownable` representation** — single-writer `SharedStorage` (recommended, for
   rotation/transfer) vs `User{owner}` (no transfer). Leaning `Shared`.
2. **`revoke` semantics** — remove-wins tombstone (recommended, retroactive via
   #2655) vs add-wins. Lock with the #2541 owner so `AccessControl` and the
   protocol authorizer agree.
3. **`Op` surface** — final enum shape; must match #2541's signed-action op bound
   so the seam is a no-op swap.
4. **`Pausable` scope** — ship advisory-only in v1, or block on freeze-rotation
   merge enforcement? Recommended: advisory v1, labeled.

## 10. Known constraints / footguns

- **Don't enforce in the method.** Every guard is fail-fast only; the data must
  live in a guarded entity or there is no security.
- **`SharedStorage<UnorderedMap<K,V>>` requires `V: Mergeable`** — `String` is
  not; use `LwwRegister<String>`.
- **Collection mutation goes through `get_mut`**, which re-stamps the writer
  domain on the value entry each call (sidesteps the persistence round-trip) —
  components must route mutation there, never a back-door `set`.
- **Local-causal resolution under concurrent rotation (#2673)** is a liveness/UX
  wrinkle, not a security gap — merge-time `writers_at` is the boundary. Components
  inherit this; don't paper over it with extra local checks.
- **One `SharedStorage` field = one independent writer set.** A component holding
  several must rotate each explicitly; name such methods per the field they touch.

## 11. References

- Enforcement: `crates/storage/src/interface.rs:300` (verify), `:1481` (delete).
- `SharedStorage` API: `crates/storage/src/collections/shared.rs`.
- `UserStorage`: `crates/storage/src/collections/user.rs`.
- Example app: `apps/kv-store-with-shared-storage/src/lib.rs`.
- Re-key registry to mirror for #2544: `crates/storage/src/collections/rekey.rs`.
- Related issues: #2544 (field-id), #2541 (fine-grained perms), #2230/#2655/#2665
  (rotation substrate, merged), #2673 (local causal resolution).
