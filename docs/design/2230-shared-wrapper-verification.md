# Design — Authenticate the `SharedStorage` writer-set rotation (#2230 item 1)

| | |
|---|---|
| **Status** | Implemented (item 1) — PR #2601, merged 2026-06-02 |
| **Date** | 2026-06-02 |
| **Owner** | Storage / Context / Node sync |
| **Issue** | #2230 (SharedStorage v2: live per-entity verification) — **item 1 only** |
| **Builds on** | #2588 (collection guarding — child entries verified at merge), #2266/#2233 (DAG-causal `writers_at`), the `stored ∪ claimed` signing fix |

> Illustrative; not final signatures.

## 1. What is already done (scope reduction)

#2230 listed four items. Three are effectively closed:

- **Item 2 — rotate-self-out signing.** Done: `save_raw` stamps the signing
  placeholder on `stored ∪ claimed` writers (`interface.rs:2660`), so a writer
  rotating itself out (executor ∈ stored, ∉ claimed) still signs;
  `sign_authorized_actions` signs whenever the placeholder is present
  (`execute/mod.rs:1956`).
- **Item 4 — signer-pubkey hint.** Done: `SignatureData.signer` + the fast-path
  single-verify (`action.rs:295`, `interface.rs:308`).
- **Item 1 (data writes) — covered by #2588.** A guarded collection's **child
  entries** now sync as their own entities and are verified at merge against
  `effective_writers` (proven by the `shared-storage` adversarial e2e: a
  non-writer's forged map entry is rejected). So *writes to the data* are
  authenticated.

**What remains: authenticating the writer-set *rotation* itself** (and item 3,
populating `signature_data`, which rides on it).

## 2. The remaining gap

The `SharedStorage` **wrapper's** own state — `value` (for a scalar), `writers`,
`writers_nonce` — propagates via the **root-state borsh sync path**: the wrapper
is an inline field of the enclosing `#[app::state]` struct, so its bytes ride in
the root state, and writer-set convergence is `SharedStorage::merge` doing **LWW
on `writers_nonce`** (`shared.rs`). That merge does **not verify who rotated** —
it just takes the higher nonce.

`rotate_writers` checks `executor ∈ writers` *locally* (`shared.rs:283`), but that
is an API-layer check. A malicious context **member** (not a current writer) can
hand-craft a root-state delta that bumps `writers_nonce` and swaps `writers` to
themselves — bypassing the local check — and the LWW merge on honest peers
accepts it. **Writer-set rotation is currently unauthenticated at merge.**

(Data writes are safe — they're per-entity verified — but whoever controls the
writer set controls all future writes, so an unauthenticated rotation defeats the
whole guarantee.)

## 3. Why the obvious fix traps: the dual-write hazard

The wired-but-disabled approach was: on `insert`/`rotate_writers`, also emit a
per-entity `Update` action for the wrapper so the merge-time `Shared` verifier
runs (`shared.rs:256`, `:297`). It is disabled because the wrapper **also**
propagates inline via root-state borsh, so the data flows **twice** →

- the receiver computes a **different root hash** than the sender (permanent
  divergence — "every rotation produces a permanent divergence"), and
- a **WASM trap during `__calimero_sync_next`**.

So per-entity verification can't be *added on top of* root-state borsh. The
duplication has to be removed.

## 4. The fork

- **(a) Dual-write** (root-state borsh + per-entity action). Rejected — it is the
  disabled path above (divergence + trap). Making the root hash ignore the
  wrapper's double contribution is fragile and exactly what bit v2.
- **(b) Suppress the root-state duplication.** The wrapper's mutable state lives
  **only in its own storage entity**, synced via per-entity `Update` actions
  (verified against the writer set, like #2588's child entries). The root state
  carries only a **reference** (the wrapper's entity id), not the inline bytes —
  exactly how a **collection** already behaves (an `UnorderedMap`'s root-state
  borsh is just its id; its data is in child entities). **Recommended.**

Why (b) is now the natural choice: #2588 already makes the wrapper a child of
root (`add_child_to(*ROOT_ID)`) and makes per-entity `Shared` verification a live,
tested path. (b) finishes the job — it stops the wrapper from *also* serializing
its state into the root blob, so the single source of truth is the verified
per-entity entity.

## 5. Mechanism (option b)

1. **Wrapper borsh = ref, not inline.** `SharedStorage<T>` must serialize into the
   root state as its entity id only (+ the `_adaptor` marker), with `value`,
   `writers`, `writers_frozen`, `writers_nonce`, `signature_data` persisted on its
   **own** entity. Two ways:
   - a custom `BorshSerialize/Deserialize` that writes the id and lazy-loads the
     body from storage on access (like `Collection`), or
   - have `#[app::state]` treat `SharedStorage` fields like collection fields
     (it already special-cases them in the deterministic-id pass — same predicate).
2. **Mutation emits a verified per-entity Update.** `insert`/`rotate_writers`
   `Interface::save(self)` (or `add_child_to`) so each mutation produces an
   `Update` action carrying the wrapper's bytes + `StorageType::Shared` metadata,
   signed by the executor (gated by the existing `stored ∪ claimed` rule, which
   already handles rotate-self-out). No root-state duplication ⇒ no divergence.
3. **Merge verifies the rotation.** On apply, the node resolves `effective_writers`
   for the wrapper entity (from its rotation log via `writers_at`) and
   `apply_action` validates the signature — so a forged rotation from a non-writer
   is rejected, exactly like a forged data write today.
4. **Item 3 (populate `signature_data`)** falls out: after signing, write the
   `SignatureData` from the action metadata back onto the wrapper's field (the
   wrapper is now a real entity in the action stream).

The wrapper's own rotation log already exists conceptually (it's the anchor #2590
will also point children at); this makes it the *authenticated* source of truth
for the writer set.

## 5a. Implementation recipe (the template exists: `Root<T>`)

`Root<T>` (`collections/root.rs`) is *exactly* the handle shape to copy:

```rust
pub struct Root<T, S> {
    inner: Collection<T, S>,           // value stored as ONE entry at a fixed id
    #[borsh(skip)] value: RefCell<Option<T>>,  // lazy cache, loaded via inner.get(id)
    #[borsh(skip)] dirty: bool,
}
// borsh(Root) == borsh(inner Collection) == just its Element (id + metadata).
// The value T is NOT in the struct's borsh — it's a separate entity (the entry).
```

So `SharedStorage<T>` becomes:

```rust
pub struct SharedStorage<T, S> {
    inner: Collection<T, S>,           // holds the single value entry, Shared-stamped
    #[borsh(skip)] value: RefCell<Option<T>>,
    #[borsh(skip)] frozen: bool,
    // NO writers / writers_nonce fields — see below.
}
```

- `get`/`get_mut` lazy-load the value entry (Root's `get()` pattern); `insert`
  writes it back; the entry is `Shared{writers}` so writes are verified (#2588).
- `borsh(SharedStorage) == borsh(inner) == its Element` ⇒ root state carries only
  the id, no body. **No double-write.**

### Where the writer set lives (the load-bearing detail)

Do **not** keep `writers` inline, and do **not** rely on it riding in the inner
`Collection`'s Element metadata — that metadata still travels in root state and is
unverified, so a forged root-state delta could still swap it. Instead the writer
set is **resolved from the wrapper entity's rotation log** via
`rotation_log_reader::writers_at` (the verified, per-entity source of truth that
#2588's child-entry verification already consults). `writers()` reads the log;
`rotate_writers` appends a signed entry to the log (a verified per-entity action).
This is what lets `writers_nonce` + the LWW merge be **deleted** — convergence is
the log + ADR 0001, not nonce-LWW.

## 6. Risks

- **Borsh/wire change** for how `SharedStorage` serializes into root state
  (ref vs inline) — needs a migration/version story for existing `SharedStorage`
  state (the `scenario-shared-storage` fixtures, `kv-store-with-shared-storage`).
- **Convergence under concurrent rotation** — once rotations are per-entity
  actions, concurrent rotations converge by the ADR 0001 rule via the wrapper's
  rotation log (the machinery exists); must be tested, it is the split-brain-prone
  part.
- **Migration of in-flight root-state SharedStorage** to the ref form on first
  load.

## 7. Plan

1. **P1 — wrapper-as-entity (suppress root-state inline).** Custom borsh / macro
   special-case so the wrapper serializes as a ref; persist its body on its own
   entity. No behaviour change to verification yet; assert root hashes match
   sender/receiver (no divergence) on a 2-node e2e.
2. **P2 — emit verified per-entity Update on mutation.** Re-enable the disabled
   `Interface::save(self)` in `insert`/`rotate_writers`; drop the root-state LWW
   merge for the wrapper. Negative e2e: a **non-writer member's forged rotation is
   rejected** (extend `kv-store-with-shared-storage`'s adversarial workflow).
3. **P3 — populate `signature_data`** writeback (item 3) + signer-hint already
   present.
4. **P4 — concurrent-rotation convergence tests** on the #2552 harness +
   migration of existing root-state SharedStorage to the ref form.

## 8. Test plan

- **Forged-rotation rejected (the proof):** a non-writer context member crafts a
  writer-set rotation; honest node rejects it, writer set unchanged. Mirror of
  the #2588 forged-data-write e2e.
- **No divergence:** legit rotation propagates, sender/receiver root hashes match
  (this is the thing v2's dual-write broke).
- **Concurrent rotations converge** by ADR 0001.
- **Rotate-self-out** still works (regression — item 2 already landed).

## 9. Relationship to #2590

#2590 (retroactive rotation revocation for guarded *collections*) needs the
wrapper's rotation log to be the authenticated anchor. This issue makes that log
authenticated and live; #2590 then points child entities at it so a rotation
revokes the whole subtree at the causal cut. So **#2230 item 1 is the prerequisite
for #2590** — do it first.
