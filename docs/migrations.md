# Writing app state migrations

When you ship a new version of a Calimero app whose **state shape changes**, you
write a *migration* that reshapes each context's existing data into the new
layout. This guide covers when you need one, how to write it (the easy way and
the hand-written way), the one rule that actually matters (convergence), how the
SDK keeps you from getting it wrong, and how to test and ship it.

> **Audience:** app developers using `calimero-sdk`. For the internals, see
> `crates/sdk/AGENTS.md`.

---

## 1. Do you even need a migration?

A migration is needed only when old serialized state can no longer be read as the
new state. Borsh is **positional**, so:

| Change | Migration? |
|---|---|
| Add a method, fix logic (no field change) | **No** |
| Append a variant to an enum (kept indices) | **No** |
| **Add a field** | **Yes** — old bytes have no value for it |
| **Remove / rename a field** | **Yes** |
| **Change a field's type** | **Yes** |

If you're unsure, the `calimero-abi diff` CI lint compares the old and new
`state-schema.json` and tells you whether the change is additive, breaking, or an
unsafe identity downgrade (see [§6](#6-the-no-silent-downgrade-rail)).

A migration runs **once per node**, the first time each context is accessed after
the upgrade, under the `LazyOnAccess` upgrade policy.

---

## 2. Quick start — `#[derive(Migrate)]`

For the common cases — **add / remove / rename a field** — you don't write the
migration body at all. Declare the new state, derive `Migrate`, point it at the
old layout, and annotate only what changed:

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::BorshDeserialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state]
#[derive(app::Migrate)]
#[migrate(from = DocV1Data, method = migrate_v1_to_v2)]
pub struct DocV2 {
    entries: UnorderedMap<String, LwwRegister<String>>,  // carried automatically
    title:   LwwRegister<String>,                         // carried automatically
    #[migrate(new = LwwRegister::new("".to_owned()))]
    notes:   LwwRegister<String>,                         // added → you give the seed
}

// The old layout, as a borsh-only shadow. Field order MUST match v1's
// `#[app::state]` struct (borsh is positional). Don't import the v1 crate's
// `#[app::state]` — it would pull in v1's full SDK surface.
#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct DocV1Data {
    entries: UnorderedMap<String, LwwRegister<String>>,
    title:   LwwRegister<String>,
}
```

The derive generates `migrate_v1_to_v2()` for you. The rules:

| Annotation | Where | Result |
|---|---|---|
| *(none)* | field | carried from the old state by name (`old.field`) |
| `#[migrate(new = EXPR)]` | field | additive — you provide the seed value |
| `#[migrate(from = old_name)]` | field | renamed — carry `old.old_name` |
| `#[migrate(with = EXPR)]` | field | **transform** — `EXPR(old.field)` (combine with `from` to convert a renamed field). Handles type changes, struct→enum, etc. |
| *field omitted from the new struct* | — | dropped (the remove case) |
| `#[migrate(emit = EXPR)]` | struct | emit an app event from the migration (e.g. `Migrated { from, to }`) |

`with` and `emit` cover the two most common reasons to drop to a hand-written
body — a **type change** and an **event** — so much of what used to need
`#[app::migrate]` is now a one-line annotation. Example:

```rust
#[app::state(emits = for<'a> MigrateEvent<'a>)]
#[derive(app::Migrate)]
#[migrate(from = V1, method = migrate_v1_to_v2,
          emit = MigrateEvent::Migrated { from: "1.0.0", to: "2.0.0" })]
pub struct V2 {
    items: UnorderedMap<String, LwwRegister<String>>,   // carried
    #[migrate(from = count, with = u64_reg_to_string)]
    count: LwwRegister<String>,                          // u64 -> String via `with`
}
fn u64_reg_to_string(c: LwwRegister<u64>) -> LwwRegister<String> {
    LwwRegister::new(c.get().to_string())
}
```

A new field you forget to annotate is a **compile error** ("no field `notes` on
the old type") — it can't silently misbuild. A *dropped* field, by contrast, is
silent (that's the remove case), so review your new field list against the old
one deliberately.

`method` defaults to `migrate`; give each derive an explicit `method = …` when a
module has more than one (e.g. a `v1→v2` and a `v2→v3`).

---

## 3. Writing a migration by hand

When you need real transformation — a **type change**, splitting a field, seeding
from other data — write the `#[app::migrate]` function yourself. The derive isn't
magic; it just generates this shape:

```rust
use calimero_sdk::state::read_raw;

#[app::migrate]
pub fn migrate_v1_to_v2() -> DocV2 {
    // 1. Read the old root bytes (None only if no prior state exists).
    let old_bytes = read_raw().unwrap_or_else(|| panic!("no prior state"));

    // 2. Deserialize into the old-layout shadow.
    let old: DocV1Data = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| panic!("v1 deserialize: {e:?}"));

    // 3. Build and return the new state.
    DocV2 {
        entries: old.entries,                       // carry a collection — handle survives
        title:   old.title,
        notes:   LwwRegister::new("".to_owned()),   // seed a new field
    }
}
```

Notes:
- A migration **returns the new state by value**; there is no `Result`. On
  unrecoverable input (no prior state, undeserializable bytes) you `panic!`,
  which aborts the upgrade and **leaves v1 state intact** for a retry — a failed
  migration is non-destructive.
- Carrying a collection (`entries: old.entries`) reuses its existing storage
  handle; no re-population needed.

### When you must hand-write it

The derive (with `with` / `emit`) handles a single-field transform, a type
change, a struct→enum, and emitting an event. You still need a hand-written
`#[app::migrate]` when **one source feeds many fields** or a **new field is
derived from a field you're also keeping** — i.e. the transform crosses fields:

| You need to… | Why the derive can't | Example scenario |
|---|---|---|
| **Split one field into several** | a `with` yields one field, not three | `scenario-field-split` |
| **Re-derive a value from a field you also carry** | that source would be moved twice | `scenario-invariant-reshuffle` |
| **Seed an ordered collection from another you carry** | same double-use, plus ordering logic | `scenario-crdt-native` |
| Anything genuinely multi-step / imperative | — | — |

(A *single-field* type change, struct→enum, content transform, or archiving a
*dropped* field is now a `#[migrate(with = …)]` one-liner — see §2.)

These are all in `apps/migrations/` as `scenario-*` crate pairs with a matching
merobox workflow — use them as a cookbook.

### Common hand-written patterns

**Split a field** — parse one field into several, with an explicit fallback:
```rust
let parts: Vec<&str> = old.address.get().split(", ").collect();
let (street, city, zip) = match parts.as_slice() {
    [s, c, z] => (s.to_string(), c.to_string(), z.to_string()),
    _ => (old.address.get().clone(), String::new(), String::new()), // handle malformed input
};
```

**Seed an ordered structure (determinism!)** — building a `Vector` from a map you
also carry; **sort first**, because two nodes may iterate an unordered source in
different orders:
```rust
let mut keys: Vec<String> = old.items.entries()?.map(|(k, _)| k).collect();
keys.sort();                                   // ← required, or the roots diverge
let mut tags = Vector::new();
for k in keys { tags.push(k.into())?; }
DocV2 { items: old.items, tags, .. }           // `items` carried + re-read above
```

> For a *single-field* type change, struct→enum, or content transform, prefer
> `#[migrate(with = …)]` (§2) — these hand-written patterns are for the cross-field
> cases the derive can't express. (A single-field type change still works
> hand-written too: `counter: LwwRegister::new(old.counter.get().to_string())`.)

> All of these run under the convergence rules in [§4](#4-the-convergence-rule-the-important-part)
> and the category rules in [§5](#5-the-three-data-categories) — the same as a
> derived migration. The merge-mode guards (Counter/RGA) still apply.

---

## 4. The convergence rule (the important part)

`#[app::migrate]` runs **independently on every node**, against that node's own
(already-synced, byte-identical) v1 state. The migrated state is **not** sent over
sync — each node re-derives it locally. Therefore:

> **A migration's output must be a deterministic, pure function of the old
> state.** Two nodes running the same migration on the same v1 bytes must produce
> a byte-identical v2 root, or they fork and CRDT sync breaks.

### What the SDK handles for you

A migration body runs under storage **merge mode**, and the SDK removes the two
*structural* sources of per-node entropy automatically:

- **Node-local timestamps.** `LwwRegister::new(...)` / `.set(...)` and `Element`
  update times are zeroed during a migration (instead of stamping this node's
  clock + id).
- **Random collection ids.** New collections and `Vector`/`AuthoredVector`
  elements are re-keyed deterministically (by field name / append index) instead
  of the `Id::random()` the live path uses.

So a migration that only **carries fields and adds new ones with `::new()`**
cannot trigger a determinism bug.

### What you must still avoid (app-level)

- **Wall-clock / RNG / iteration order.** If you materialize an *ordered*
  structure (a `Vector`) from an *unordered* one (a map/set), **sort first** —
  two nodes may iterate the source in different orders.
- See the data categories below for the CRDT-specific rules.

---

## 5. The three data categories

Which migration moves are safe depends on the field type:

| Category | Types | In a migration |
|---|---|---|
| **Convergent** | `UnorderedMap`, `Vector`, `UnorderedSet`, `UserStorage`, `FrozenStorage` | key/content-addressed — rebuild freely; auto-converges |
| **Replayable** | `Counter`/`GCounter`/`PNCounter`, `RGA` | **carry across**, or replay deterministically — see below |
| **Identity-gated** | `AuthoredMap`, `AuthoredVector`, `SharedStorage` | **carry-through only** — re-inserting stamps *this* node as owner and diverges |

### Replayable: Counter and RGA are guarded

`Counter::increment` / `decrement` stamp the running node's id; `RGA::insert` /
`insert_str` stamp a node-local clock. Calling them in a migration would silently
fork the network — so the SDK makes them **panic** during a migration:

```text
Counter::increment() is non-deterministic during a state migration: it stamps
this node's identity… Carry the counter across unchanged (`field: old.field`)
or replay with `increment_for(executor_id, …)`.
```

To rebuild one in a migration, carry it (`c: old.c`) or use the deterministic
replay APIs — `increment_for(id, …)`, `decrement_for(id, …)`,
`insert_str_at_timestamp(pos, fixed_ts, s)` — which take the identity/clock
explicitly so every node produces the same result.

### Identity-gated: carry, don't re-insert

`AuthoredMap`/`AuthoredVector`/`SharedStorage` record ownership as the running
node's `executor_id`. Re-inserting their entries during a migration would stamp
each node as the owner and diverge. **Carry the whole collection through**
(`entries: old.entries`); the v1 owner stamps are preserved.

Rewriting the *content* of identity-gated collections (owner-driven re-signing)
is a separate, in-progress capability — carry-through is the rule today.

---

## 6. The no-silent-downgrade rail

Changing an identity-gated type to a plain one — `AuthoredMap → UnorderedMap`,
`SharedStorage → UnorderedMap`, `AuthoredVector → Vector`, or dropping the field —
**strips per-entry authorship / the writer ACL across the whole network**. This
is refused:

- in **CI**, by `calimero-abi diff` (an `UNSAFE_IDENTITY_DOWNGRADE` finding), and
- at the **node**, by the upgrade gate, before the upgrade op is even emitted.

If you hit this, the fix is **not** to strip the type — it's an owner-driven
rewrite (each owner re-migrates their own signed entries). Stripping authorship
is almost never what you want.

---

## 7. Testing your migration

### Fast, in-process — `TestHost`

Run the migration entirely in memory (`cargo test`, no Docker), and — most
importantly — assert it **converges across nodes**:

```rust
#[cfg(test)]
mod tests {
    use calimero_sdk::testing::{assert_migrate_converges, TestHost};

    #[test]
    fn migrate_carries_and_seeds() {
        let mut app = TestHost::new(DocV1::init);
        app.call(|s| s.set_title("my-doc".to_owned())).unwrap();

        let v2 = app.migrate(migrate_v1_to_v2);          // run it in-process

        assert_eq!(v2.view(|s| s.title().unwrap()), "my-doc");   // title carried
        assert_eq!(v2.view(|s| s.notes().unwrap()), "");          // notes seeded
    }

    #[test]
    fn migration_converges_across_nodes() {
        // Runs the migration as two different node identities from an identical
        // v1 and asserts the two merkle roots match — a non-deterministic
        // migration fails here in milliseconds instead of forking production.
        assert_migrate_converges::<DocV1, DocV2>(
            install_v1, migrate_v1_to_v2, [1u8; 32], [2u8; 32],
        );
    }
}
```

`assert_migrate_converges` compares the **merkle root hash**, which folds in every
child-collection entry — so a per-node value baked anywhere in the migrated state
(a field, *or a value inside a carried collection*) is caught.

**In-process limits.** Both "nodes" share one deterministic mock store, so this
catches identity/value divergence but **not iteration-order** divergence (the
mock sorts child entries by id). Cover ordering with a merobox e2e.

### Full end-to-end — merobox

For real cross-node behaviour (and iteration-order determinism), run a merobox
workflow. See `workflows/app-migration/README.md` ("Running locally":
`merobox bootstrap run …`) and the worked scenarios in `apps/migrations/`.

### Worked examples

- `apps/migration-harness-example/` — `#[derive(Migrate)]` + `TestHost` unit
  tests (carry, seed, rename, convergence, divergence-detection).
- `apps/migrations/scenario-*` + `workflows/app-migration/*.yml` — one crate pair
  and one merobox workflow per migration shape (additive, remove, rename,
  type-change, CRDT-native, authored-map, …).

---

## 8. Shipping a migration

Migrations run only under `UpgradePolicy::LazyOnAccess`. Trigger the upgrade:

```text
upgrade_group(
    target_application = <v2 blob id>,
    migrate_method     = "migrate_v1_to_v2",   // must match your exported fn name
    policy             = LazyOnAccess,
    cascade            = true,                  // optional: fan out across a namespace subtree
)
```

Each node self-migrates on its next context access (logged as `performing lazy
upgrade before execution` → `Executing migration` → `Migrated state written
successfully`). Picking `Automatic`/`Coordinated` for a migration is rejected at
emit time — only `LazyOnAccess` runs the migrate function.

---

## 9. Quick reference

| Do | Don't |
|---|---|
| Carry fields: `field: old.field` | Re-insert an `AuthoredMap`/`UserStorage`/`SharedStorage` in a migration |
| Seed new fields with `::new()` / `#[migrate(new = …)]` | Call `Counter::increment`/`decrement` or `RGA::insert` in a migration (they panic) |
| `increment_for` / `insert_str_at_timestamp` to replay a CRDT deterministically | Use wall-clock, RNG, or unsorted iteration order |
| `sort()` before building a `Vector` from a map/set | Change an identity-gated type to a plain one (refused) |
| Prove convergence with `assert_migrate_converges` | Assume single-node tests prove cross-node determinism |
| `panic!` on bad input (non-destructive abort) | Expect a `Result` from the migrate fn |
