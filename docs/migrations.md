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

#[app::state(version = 2)]
#[derive(app::Migrate)]
#[migrate(from = DocV1Data)]
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

`method` defaults to the versioned name `migrate_v{N-1}_to_v{N}` derived from
`#[app::state(version = N)]` (plain `migrate` only for unversioned states), so
omitting it is the norm — names stay unique across releases and across multiple
derives in one module. Explicit `method = …` remains supported and resolves
identically.

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

**Drop entries — `remove` from the carried collection; don't rebuild a fresh one.**
A new same-named `UnorderedMap::new()` is re-keyed to that field's deterministic id
during migrate, so it **shares the carried v1 storage and unions with it** — the
entries you "skipped" survive, so nothing is dropped. To actually drop, mutate the
carried collection (sort first for determinism):
```rust
let mut items = old.items;                     // carry — same storage id
let mut keys: Vec<String> = items.entries()?.map(|(k, _)| k).collect();
keys.sort();
if let Some(k) = keys.first() { items.remove(k)?; }   // actually deletes one entry
DocV2 { items, .. }
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
(`entries: old.entries`); the v1 owner stamps are preserved. Inside the
`#[app::migrate]` body, carry-through is still the only rule — converting an
entry to the new schema happens *after* the migration, owner-by-owner (below).

### Identity-gated: converting entries to the new schema (owner-driven)

Carry-through preserves the v1 entries, but each keeps its v1 `schema_version`
(a Merkle-invisible tag) until its **owner** re-signs it — nobody can re-sign
another identity's entry. Until then the entry is served at its v1 shape via
dual-read. Two paths re-stamp an entry to the binary's target:

- **Organically** — the owner's (or a current writer's) *next ordinary signed
  write* of that entry stamps `schema_version = target` and re-signs on its
  monotonic nonce, replicating as a normal `Action::Update`. A non-owner can
  never drive it.
- **One tap** — `#[app::state]` auto-generates a `migrate_my_entries()` method
  (a wasm export) for any state with an `AuthoredMap`/`AuthoredVector` field.
  One signed call sweeps every entry the caller owns that is still below target,
  converting each through the path above; it returns `{converted, remaining}`
  and is idempotent. Call it from the app/frontend ("migrate my data") after an
  upgrade; `remaining == 0` means the caller's data is fully converted.

For either path to fire, the new binary must declare its schema target with
**`#[app::state(version = N)]`** — the value the convert compares each entry's
tag against. It defaults to `0` (inert), so a v2 binary that omits it never
converts its identity-gated data. `WriterSetCell`/`SharedStorage` (group
writer-set data) converts only via the organic writer-write path, never the
one-tap batch (it is group data, not single-owner "my data").

**You do not write `migrate_my_entries` — `#[app::state(version = N)]` generates
it.** All you do is declare the version; the method appears on your app and is
exported for RPC:

```rust
// v2 binary. `version = 2` both sets the convert target AND generates
// `migrate_my_entries()` because the state has an AuthoredMap field.
#[app::state(version = 2, emits = for<'a> Event<'a>)]
#[derive(app::Migrate)]
#[migrate(from = NotesV1, method = migrate_v1_to_v2)]
pub struct NotesV2 {
    notes: AuthoredMap<String, LwwRegister<String>>,  // carried by the migrate
    #[migrate(new = LwwRegister::new(String::new()))]
    migration_note: LwwRegister<String>,
}
// No migrate_my_entries body anywhere — the macro emits it.
```

Then the owner triggers it like any other method — one signed call, no args,
returning `{converted, remaining}`:

```text
# after the upgrade, from the owner's node (frontend "migrate my data" button):
app_call(context_id, "migrate_my_entries", {})
  → { "converted": 2, "remaining": 0 }   # this owner's 2 stale notes converted
```

Loop until `remaining == 0` if you want to drain everything in one sitting
(a single call already converts all of the caller's currently-stale entries; a
second call returns `{converted: 0, remaining: 0}`). It only ever touches
entries the caller owns, so each user converts their own data independently.

### Migration UX surfaces

Two node-level events help frontends track migration progress without polling:

**`AppVersionChanged` event** — fired once, over the context's SSE/WebSocket event stream, when the context's application version flips (i.e. a migrate/upgrade was applied). `contextId` rides on the outer `ContextEvent` envelope; the payload carries the `fromVersion` / `toVersion` semver strings (either may be `null` if the corresponding `ApplicationMeta` row was unavailable at emit time):

```json
{
  "type": "AppVersionChanged",
  "data": {
    "fromVersion": "1.0.0",
    "toVersion": "2.0.0"
  }
}
```

Subscribe to context events via the node's SSE endpoint and react to this event to prompt owners to run `migrate_my_entries()` — this avoids bundle-skew (spec skew #2) where the frontend loads v2 assets but the context is still running v1.

**`authored_remaining` in the migration status endpoint** — tracks how many members in the cohort still have uncoverted identity-gated entries:

```text
GET /admin-api/groups/{namespace_id}/migration/status
→ { ..., "authored_remaining": 3 }   # 3 cohort members still have stale entries
```

`authored_remaining == 0` means all members have run `migrate_my_entries()` or had no entries to convert. Use this to drive a "migration complete" indicator in admin UIs.

> **Authorization.** This is an `/admin-api/` route and is gated by the same
> admin-credential check as the rest of that surface — the node rejects
> unauthenticated callers before any migration state is returned. The
> `authored_remaining` count (and the group/namespace structure it implies) is
> internal operational data: never expose it on an unauthenticated path, and
> scope the admin token to operators of that node/namespace.

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

## 7. Guarding a migration: `migration_check` + abort

A migration that *compiles and runs* can still be wrong — drop entries, break an
invariant, orphan a reference. To catch that **before it commits**, declare an
optional `#[app::migration_check]`. The migrate runs against an in-memory
**staging buffer**; the check runs against that same buffer **before anything is
written to the live store**. If it returns `false`, the runtime **logically
aborts** — the staging buffer is dropped, so the context stays on v1 with **zero
residue** (root *and* every child entry intact; no byte snapshot/restore needed).
An app with no check commits as before (backwards-compatible).

### What the check can read (the contract)

The check receives `old` (the committed v1 root) and `new` (the produced v2
root). Staging makes these **asymmetric**:

- **`new` is fully trustworthy** — its scalar/inline fields *and* its lazy
  collections (read through the staging buffer) reflect the produced v2 state.
  Read `new.items.len()`, walk `new`'s collections, etc.
- **`old`'s scalar/inline fields are pristine v1** (decoded from the committed v1
  root bytes) — a safe baseline.
- **`old`'s lazy collections are NOT pristine**: `old.items` and `new.items`
  resolve to the *same* deterministic bucket, so in one check execution they read
  the *same* (staged) data. **Do not diff `old` vs `new` collections** — the
  comparison is always trivially equal.

So write the check as an **invariant over `new`** (optionally against an `old`
scalar baseline), not as an `old`-vs-`new` collection diff.

### Carrying a v1 baseline: the transient migration witness

When the invariant needs a v1 value the v2 schema doesn't keep (e.g. "every item
survived"), the migrate returns a `(State, Witness)` tuple. The `Witness` is a
borsh blob delivered to the check and **never persisted** — it rides out on the
runtime Outcome like logs/events:

```rust
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct MigrationWitness { v1_count: u64 }

#[app::migrate]
fn migrate() -> (DocV2, MigrationWitness) {
    let mut items = old.items;
    let v1_count = items.len().unwrap_or(0) as u64;   // captured BEFORE any change
    // ... transform ...
    (DocV2 { items, /* .. */ }, MigrationWitness { v1_count })
}

#[app::migration_check]
fn check(_old: DocV1, new: DocV2, witness: MigrationWitness) -> bool {
    // `new.items` is the produced collection; compare it to the v1 baseline.
    matches!(new.items.len(), Ok(n) if n as u64 == witness.v1_count)
}
```

A migrate returning a plain `State` (no tuple) and a 2-arg `check(old, new)` stay
valid — the witness is opt-in. Prefer invariants that need **no** extra field
where you can: a required key present (`new.items.get("alpha")?.is_some()`),
conservation against an existing field (`new.total == new.items.values().sum()`),
or a monotonic version (`new.version > old.version`, an `old` scalar).

Built-in helpers (`calimero_sdk::migration_check`) operate on slices *you* build
from soundly-readable data (`new` collections, scalars, a witness):
- `entity_count_parity(a, b, delta)` — counts match within `delta`
- `no_orphaned_refs(refs, keys)` — every reference still resolves
- `conservation(old_total, new_total)` — a total is preserved

The check **and any witness** must be a **deterministic pure function** of the v1
state, exactly like the migrate: they run independently on every node against
byte-identical input, so all nodes reach the **same** verdict — either all commit
or all abort. (A non-deterministic check or witness is a split-verdict bug, the
same hazard `assert_migrate_converges` guards.) A failed check is **retryable**:
no migration marker is recorded, so the context re-runs migrate+check on its
**next access** — a transient cause (e.g. not-yet-synced v1) self-heals once the
input is complete.

> Diffing `old` vs `new` *collection* cardinality directly (without a
> witness/baseline) would need a pristine-snapshot read path for `old` that is
> not yet implemented — tracked as a follow-up. Use the witness pattern above.

### Aborting an in-flight migration (admin)

An operator can call off a migration that's rolling out:

```text
POST /admin-api/groups/{namespace_id}/migration/abort
```

It flips the group's pending target **back** to the pre-migration app id and drops
the pending marker, **cascading** to every descendant subgroup carrying the same
pending migration. Idempotent — a subtree with nothing pending is a no-op. It's a
forward "stop" (un-migrated contexts stop switching), not a rewind of any context
that already migrated.

---

## 8. Testing your migration

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

## 9. Shipping a migration

Migrations run only under `UpgradePolicy::LazyOnAccess`. Trigger the upgrade —
note there is **no method name to pass**; the node resolves the migration from
the version + edge your app's build embeds in its ABI:

```text
upgrade_group(
    target_application = <new bundle's application id>,
    cascade            = true,   // optional: fan out across a namespace subtree
)
```

Per service, the node compares the state version of the bytecode the group
currently runs against the target's and decides: equal → code-only swap
(nothing runs); one ahead with a declared edge → run that edge's migrate;
version bump with NO edge, more than one hop, or an older target → the
upgrade is **rejected with an actionable error** instead of silently shipping
new code onto old-layout state. There is no caller-supplied method at all —
the declared ABI is the single source of truth.

Each node self-migrates on its next context access (logged as `Executing
migration` → `Migrated state written successfully`; the extra `performing lazy
upgrade before execution` line precedes them only on the *non-cascade*
lazy-on-read path — under `cascade: true` the cascade propagator drives the
migrate instead). Picking `Automatic`/`Coordinated` for a migration is rejected
at emit time — only `LazyOnAccess` runs the migrate function.

If the app has **identity-gated** state (`AuthoredMap`/`AuthoredVector`),
declare the new binary's schema target with `#[app::state(version = N)]` so the
post-migrate owner-driven convert (§5) has a target to compare against — see
[§5](#5-the-three-data-categories).

### 9.1 The policy prerequisite: create groups as `LazyOnAccess`

Migrations run **only** under `UpgradePolicy::LazyOnAccess`; an
`upgrade_group` whose target declares a migration is **rejected at emit
time** under `Automatic` or `Coordinated`. The trap: if your app creates its namespace with
`upgradePolicy: 'Automatic'` (a tempting-sounding default), every migration
you ever ship to that workspace fails at the upgrade call. Either create
groups as `LazyOnAccess` from day one, or heal before upgrading:

```text
PATCH /admin-api/groups/:group_id   { "upgradePolicy": "LazyOnAccess" }
```

### 9.2 Bundle apps: the id never changes — the blob does

A bundle (`.mpk`) derives `ApplicationId = hash(package, signer)` — it is
**version-stable**. v1 and v2 of the same package share one id; installing v2
overwrites the blob *in place* under that id. Consequences:

* **Never key "is an update available / applied" off the ApplicationId
  changing.** It won't. The version discriminator is the bytecode blob id,
  recorded on the group as `appKey` at upgrade time.
* **Download ≠ apply.** Installing the new bundle (registry download) flips
  the *local node's* installed version immediately — but the workspace group
  still targets the old bytecode until `upgrade_group` runs. A UI that only
  compares "installed version vs registry latest" reads **"up to date" in the
  downloaded-but-never-migrated state** and strands the admin. The ground
  truth for "has this workspace been migrated" is:
  `group.appKey == installed_application.blob.bytecode` (note: `appKey` is
  hex on the admin API, blob ids are base58 — decode before comparing).
* Single-wasm apps content-address the id per version, so the old
  id-comparison instinct works there — bundles are the exception that ships
  to real users.

### 9.2b The node resolves the method — callers carry nothing

Your build embeds `state_version` and the declared migration edges
(`migrations: [{method, fromVersion}]`) in each service's ABI, and the node
reads them at upgrade time. Updater UIs/CLIs never carry a method name; the
typo/omission class of failure is structurally gone. The export name is
generated (`migrate_v{N-1}_to_v{N}`) unless you pass `method = …` explicitly
in the derive — explicit names remain supported and resolve identically.

### 9.2c Multi-service bundles: per-service version arithmetic

A multi-service bundle (e.g. a registry service + a data service) upgrades as
one unit, but the node judges **each service by its own declared state
version**:

* a service whose version is unchanged is code-only **by arithmetic** — no
  wasm probing, no fake failure, nothing to declare;
* a service one version ahead with a declared edge runs its own migrate;
* a service that bumps its version without declaring an edge rejects the
  whole upgrade at emit time (mis-built bundle).

So each service simply declares its own truth and the release composes:
"only service A changed", "only B", and "both" all work with zero
coordination. One current restriction: if two services declare *different*
explicit method names for the same release, the upgrade is rejected (the
wire carries one method for pre-v2 receivers) — omit `method = …` or use the
same name; full per-service actuation lifts this later.

### 9.3 Reaching every context: `cascade: true`

`upgrade_group` without `cascade` upgrades **only the target group**. If your
app puts data in subgroup contexts (per-folder/per-channel child contexts),
they stay on the old schema forever and the UI on top of them keeps reading
v1. Pass `cascade: true` to fan the upgrade out across the namespace subtree
as one atomic op.

### 9.4 What each member's node does (and when)

After the admin's `upgrade_group`, peers converge lazily:

1. the upgrade replicates via the group op stream; a peer's **sync gate**
   detects the pending upgrade (id change, or same-id `appKey` divergence for
   bundles) and pre-stages the new blob over BlobShare;
2. the node **migrates on its next access** to each context — reads count, so
   merely opening the app advances it; an idle node stays on v1 until then
   (write your UI copy accordingly: "updates next time you use it", not
   "updates automatically");
3. while a migration is in flight, writes are refused with
   `upgrade in progress … writes refused until migration completes` — handle
   this error in the frontend as a status ("workspace updating"), not a raw
   failure;
4. on completion the node emits the `AppVersionChanged` SSE event — subscribe
   to flip version displays live (`mero.events.onAppVersionChanged`).

### 9.5 Frontend checklist (the part nobody tests until a real user does)

* Show the installed version to **every member**, not just admins; pair it
  with honest LazyOnAccess copy.
* Detect the **downloaded-but-not-applied** state via the `appKey`-vs-blob
  comparison (§9.2) and keep offering the apply step — it must survive a page
  reload (the bundle id is stable, so the group's `targetApplicationId` still
  addresses the downloaded bytecode).
* Map the upgrade-gate write refusal (§9.4.3) to an "updating…" banner.
* Subscribe to `AppVersionChanged` for the completion signal.
* Browser-test the full admin flow once per release: CLI clients skip CORS,
  so a missing preflight method (e.g. PATCH for `updateGroupSettings`)
  surfaces only in browsers, as an opaque `HTTP 0` network error.

### 9.6 Verifying propagation by hand

```bash
# the group's target bytecode (hex appKey) — same on every member after sync
meroctl --output-format json --node <n> namespace ls

# what's actually installed under the (stable) application id
meroctl --node <n> app ls          # check the Source/Blob columns' version

# poke a context to trigger the lazy migrate (reads count)
meroctl --node <n> call <any_read_method> --context <ctx_id>
```

`appKey` equal everywhere + each node's installed blob matching it + the call
returning post-migration data = the migration has fully landed.

### 9.7 Multiple versions on one node

A context executes the bytecode **its own group points at** — not whatever was
downloaded last. Installing a newer bundle never changes what existing
workspaces run: each migrates when its own admin upgrades it. Consequences you
can rely on:

* two workspaces on one node can run different versions of the same app
  indefinitely;
* `createNamespace` accepts an optional `appKey` to pin a new workspace to any
  installed version (default: latest);
* the admin API lists every retained version of a package
  (`GET /admin-api/applications/:id/versions`) and namespace DTOs carry a
  per-workspace `appVersion` — display that, never the shared installed
  version.

### 9.8 Catching up several versions behind

A context behind its group by **more than one** version catches up by
replaying the group's recorded **upgrade ladder** hop by hop — each hop runs in
**that release's own bytecode**, not the latest. The node fetches each rung's
blob from peers as needed and runs that version's own migrate. Because every
member runs the same frozen bytes for a given hop, the result is identical
across the group by construction (no cross-node divergence). A single access
on a stale context walks all pending hops in order and lands on the current
version; nothing is run eagerly.

An admin can also move a group several versions in one action: the upgrade
plans a rung per intermediate **state version** (a release that doesn't change
state adds no rung) from the versions installed on the node, and the group
advances rung by rung.

### 9.9 Support window and recovering a stranded member

The versions whose blobs remain obtainable (from peers or the registry) define
which upgrades chain directly. A member below that window — an intermediate
blob is gone everywhere, or it was offline across several releases and never
fetched one — is **detected, not silently wedged**: it stays on its current
real version and reports `failed` with `no_migration_path` in the migration
rollup. It never runs a later hop's migrate against older state.

Two recoveries, both operator-initiated:

* **Stepwise reinstall** — install an intermediate version from the registry.
  Any installed version can be an upgrade target (§9.7), so the gap becomes a
  single hop and the next access finishes the chain.
* **Resync** — `POST /admin-api/contexts/{id}/resync` adopts an up-to-date
  peer's state wholesale (a full-state snapshot), bypassing replay. This is
  **destructive**: any local edits the context hasn't broadcast are discarded,
  so it refuses with the local-head count unless you pass `{"force": true}`.
  After it completes the member is back at the current version.

A self-heal needs no action: if the missing blob simply arrives later, the next
access resolves the hop, migrates, and clears the `failed` marker.

---

## 10. Quick reference

| Do | Don't |
|---|---|
| Carry fields: `field: old.field` | Re-insert an `AuthoredMap`/`UserStorage`/`SharedStorage` in a migration |
| Seed new fields with `::new()` / `#[migrate(new = …)]` | Call `Counter::increment`/`decrement` or `RGA::insert` in a migration (they panic) |
| `increment_for` / `insert_str_at_timestamp` to replay a CRDT deterministically | Use wall-clock, RNG, or unsorted iteration order |
| `sort()` before building a `Vector` from a map/set | Change an identity-gated type to a plain one (refused) |
| Prove convergence with `assert_migrate_converges` | Assume single-node tests prove cross-node determinism |
| `panic!` on bad input (non-destructive abort) | Expect a `Result` from the migrate fn |
| Keep old version blobs obtainable so behind members can chain | Drop an intermediate version and expect members below it to auto-upgrade (they strand → reinstall or resync) |
