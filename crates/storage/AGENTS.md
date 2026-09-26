# calimero-storage - CRDT Collections

Conflict-free Replicated Data Types (CRDTs) for automatic conflict resolution in distributed state.

## Package Identity

- **Crate**: `calimero-storage`
- **Entry**: `src/lib.rs`
- **Framework**: borsh (serialization)

## Commands

```bash
# Build
cargo build -p calimero-storage

# Test
cargo test -p calimero-storage

# Test specific CRDT
cargo test -p calimero-storage test_counter -- --nocapture

# Test merge dispatch
cargo test -p calimero-storage merge_dispatch -- --nocapture
```

## CRDT Types and Merge Strategies

| Type                       | Purpose                  | Merge Strategy                    | Storage    |
| -------------------------- | ------------------------ | --------------------------------- | ---------- |
| `GCounter`                 | Grow-only counter        | Max per executor                  | Blob       |
| `PnCounter`                | Positive-negative counter| Max per executor (pos & neg maps) | Blob       |
| `LwwRegister<T>`           | Last-write-wins register | Timestamp-based (later wins)      | Blob       |
| `ReplicatedGrowableArray`  | Collaborative text (RGA) | Union of characters               | Blob       |
| `FugueText`                | Collaborative text (Fugue)| Union of run-length blocks       | Structured |
| `FugueTextBlock`           | One block of a `FugueText`| Tombstone OR + longer text wins  | Structured |
| `RichText<Sc>`             | Text plus formatting marks| Composite: text union + mark union| Structured |
| `RichDocument<Sc>`         | Ordered list of rich-text blocks| Composite: spine union + per-field LWW| Structured |
| `UnorderedMap<K,V>`        | Key-value map            | Entry-wise merge*                 | Structured |
| `UnorderedSet<T>`          | Unique values            | Union (add-wins)                  | Structured |
| `Vector<T>`                | Ordered list             | Element-wise merge*               | Structured |
| `AuthoredMap<K,V>`         | Map, entry owned by inserter | Entry-wise, owner-gated at apply | Structured |
| `AuthoredSortedMap<K,V>`   | `AuthoredMap` + ordered index | Identical to `AuthoredMap`†    | Structured |
| `AuthoredVector<T>`        | List, slot owned by author | Element-wise, owner-gated at apply | Structured |
| `UserStorage`              | Per-user data            | LWW per user                      | Blob       |
| `FrozenStorage`            | Immutable data           | First-write-wins                  | Blob       |

*Structured storage: Entries are separate entities with their own CrdtType, merged individually.

†`AuthoredSortedMap` reports `CrdtType::UserStorage`, the SAME variant as
`AuthoredMap`, on purpose: its ordering is a node-local derived index that is
never replicated, so two nodes holding the same entries — one using each
collection — must agree on the root hash, and do. Reach for it when the keys are
hierarchical and reads are slices: `entries()` on an authored collection is
linear in everything anyone has ever written, and on an authored collection
nobody can delete anyone else's entries, so that is a liveness floor and not
just a speed one. Measured in `tests/read_cost_profile.rs`.

### `FugueText` constraints

- Every position is an index into Unicode SCALAR VALUES (Rust `char`), never bytes and never UTF-16 code units.
  That is `insert`, `insert_str`, `insert_str_with_replica`, `delete`, `delete_range`, `text_range`, `char_at`, `anchor_at`, `len` and every `TextOp` an `apply_delta` carries.
  An astral character is one position and a combining mark is its own, so a grapheme cluster spans several; a browser counts UTF-16 code units, where an astral character is two, so a TypeScript binding converts on both edges and core stays as it is.
  A run is capped in nodes and one node holds one scalar value, so a cap boundary can never land inside a character (`scalar_value_tests`).

- A block holds at most `MAX_RUN_LEN` (256) nodes, and a full run is never rewritten or split.
  The overflowing character opens a new block parented on the full run's last node, side right.
  Only `tools/storage-cost/tests/keystroke_bytes.rs` gates this, because row counts cannot see it.
- No node-local derived state: order is recomputed from the stored blocks on every call, because gas must be equal on every replica.
- Tombstones are one bit per NODE, because coalescing grows a run after the fact.
- Blocks are mutable under one key, so entries carry their own `crdt_type`: the `FugueTextBlock` tag routes to a join instead of the untagged last-writer-wins, which drops every node only the loser defines.
  It dispatches on the APPLIED path only, since a local write is not a merge.
- The join is a tombstone OR plus the maximum of `(node count, text, parent, side)`, which stays convergent against untrusted peer bytes.
- The stored entry tuple is `(value, key)`; the reverse order still decodes, so getting it wrong is a silent bad join rather than an error.
- A replica id derives from the device id, and a counter is never reused, because `FugueTree::integrate` keeps the first definition of a node.
- Plain Fugue, not FugueMax; the residual ordering case is pinned by `figure_7__right_siblings_order_by_id_not_by_right_origin`.
- `insert_str` resolves the insert rule once, for the first character, and writes each block it touches exactly once.
- `apply_delta` takes a whole editor change (`TextOp::{Retain, Insert, Delete}`) in one call: one load, one tree build, one traversal, one write per touched block, and nothing written if any op fails.
  It must store byte for byte what the same ops store one call at a time; `apply_delta_stores_what_the_ops_stored_one_at_a_time` is that gate.
- Undo is local and built by the app: each edit returns what its inverse takes (`insert_str` and `insert_str_at` an `IdRange` for `delete_ids`; `delete_range` and `delete_ids` a `Removed` for `insert_str_at`; `apply_delta` a `Vec<Undo>` for `undo`, which is a thin reverse loop over those two primitives and returns the redo steps).
  Tombstones are monotone, so undoing a delete mints new characters at the anchor and never clears a bit.
- `Removed` also carries the ids it took, coalesced into runs, because one delete can span several writers and the single anchor cannot name them.
  App events must carry those ids and never positions: an event is recorded in the delta and re-emitted on the RECEIVING node, where a concurrent edit has already moved every position the author counted.
- A cursor is an `Anchor`: a character id plus a `Bias`, or a document edge. `anchor_at` and `resolve` each cost one tree rebuild and store nothing; `resolve_many` resolves a whole slice against one.
- `visible_ids` reads the id of every visible character, aligned with `get_text` and coalesced into `IdRange` runs, because a client rebasing a refused write must diff by identity: identical characters from different writers are indistinguishable as text.
- `Anchor`, `Bias`, `IdRange`, `Removed` and `Undo` carry borsh AND serde. Borsh is the persisted format; the JSON is the JSON-RPC shape, with a `RawId` as the two-element array `[replica, counter]`. Both are pinned as formats.
  They have no `AbiType`, so a guest method still cannot take or return one directly - `cargo mero build` rejects it - and the reference app ships them as bs58-encoded borsh.
  An anchor on a deleted character resolves to the gap it left, which is only possible because tombstoned runs are never removed.

### `RichText` constraints

- A composite of `FugueText` and an `UnorderedMap` of mark rows, with **no `CrdtType` of its own**, no arm in `merge_by_crdt_type` and no `CrdtCollectionType` in the ABI (it reports `crdt_type: None` like `UserStorage`, and the value it advertises is the rendered `Span`).
  That is sound for exactly one reason: a mark row is written ONCE and never rewritten, so two replicas holding one `MarkId` hold byte-identical values, and the last-writer-wins that an untagged entry falls back to cannot pick wrong.
  Removing formatting is a NEW row with a greater id and `value: None`, never an edit or a delete. Stamping a mark row with a converging type would route it through the wrong arm; `sync_sim`'s `rich_text` scenarios pin that it stays opaque.
- The read rule is the whole format contract: per character, per key, the covering mark with the greatest `MarkId` wins, and a `None` value means the key is absent. A future compaction may replace any set of marks by an equivalent one as long as that rule still renders the same spans.
- `MarkId` is `(lamport, replica)` with `lamport = 1 + the greatest this replica can see`, NOT an HLC. The WASM clock is quantised to about 15 microseconds and re-seeded per instance, so two marks minted in one call would share a timestamp - harmless for a register's value, silent data loss for a map KEY.
- Where a mark grows when text is typed at its edge is decided ONCE, at write time, as the two stored anchor biases; `MarkSchema` is consulted on the write side only. A replica running an older schema therefore renders identical spans, and a removal uses `Expand::inverted()` so turning bold off keeps growing the way turning it on did.
- A boundary insert follows Peritext: scan the tombstones in the gap, and if one carries the `After` anchor of any mark, insert after the last such tombstone. It reads stored anchor sides, never the schema, which is what makes it identical on every replica. `FugueTree::insert_after_in` exists for it, because a visible index cannot name a position among tombstones.
- A mark naming a character this replica has not received is RETAINED and skipped for the read; dropping it would diverge from a replica that has the text. A mark whose start resolves at or after its end covers nothing.
- `to_delta` merges adjacent runs with equal attributes. That is load-bearing, not cosmetic: without it a replica holding a mark that loses everywhere still emits a span boundary, and two replicas would render different span lists for the same document.
- Redundant-write suppression is the only defence against unbounded row growth before a compaction rule exists: re-asserting formatting already in effect writes zero rows. Toggling one range `n` times still writes `n` rows, which `rich_text_marks.rs` makes visible rather than acceptable.
- `apply_delta` runs every fallible check against a pure length walk before the first write, because it spans two collections and cannot share one draft. A rejected delta therefore stores nothing at all. A delete past the end still clamps silently, matching `FugueText`.
- `mark`, `unmark`, `mark_at`, `apply_delta` and `apply_undo` panic in merge mode: they mint ids from the node-local device id. A migration seeds formatting with `mark_with_replica`.

### `RichDocument` constraints

- A composite of a spine `FugueText`, an `UnorderedMap` of block rows and, per block, one property map, one attribute map and a `RichText` body. Like `RichText` it has **no `CrdtType` of its own** and reports `crdt_type: None` in the ABI, advertising the rendered `BlockView`.
- Block order is the spine: creating a block mints one `U+FFFC` placeholder, and the block's id IS that character's id, so it survives every later move.
- **A block's mutable structure is one row per field, never one row per block.** A map entry carries no `crdt_type`, so an entry holding four registers in one blob would reconcile last-writer-wins as a whole: a concurrent `set_depth` would lose to a `set_kind`, and a `set_kind` racing a delete could resurrect the block. One row per field makes the storage layer's per-row last-write-wins exactly per-field last-write-wins, which is why the composite still needs no `CrdtType`.
- The read rule is: a block renders at the position its `place` anchor resolves to, ties break on `BlockId`, and a block whose spine slot has NOT arrived renders at the END rather than disappearing - content that exists must never be invisible, and the state self-heals when the slot lands. A spine character no block names is invisible, because the read enumerates blocks and never spine characters.
- Deleting is a tombstone row, never `UnorderedMap::remove`: it reclaims exactly as much storage (none, since the body rows survive either way) and it makes an undelete an ordinary last write instead of a race against an index tombstone.
- A move mints a NEW spine slot and last-write-wins on `place`, leaving the old slot live and unreferenced. The block id never changes, so two concurrent moves settle on one placement and can never duplicate a block.
- `split_block` and `merge_blocks` carry the tail's text AND its formatting, because a mark is anchored to the characters it was written over and cannot follow them: the tail's resolved spans are re-asserted as the complete desired attribute set on the insert, which writes one mark row per carried key rather than one per span. Both validate every carried key before the first write.
- **The split anomaly is specified behaviour, not a defect.** A peer typing in the tail while another replica splits keeps its text in the FIRST block: the split deletes only the characters that existed when it ran, and the snapshot it copied did not contain the peer's. It is pinned by `rich_document_blocks.rs` and by a `sync_sim` scenario rather than discovered.
- Nesting is a `depth` number on a flat list. Two adjacent lists of one kind at one depth render as a single list; a renderer synthesises the container from `(depth, kind)`.
- `insert_block`, `move_block` and `split_block` mint spine ids, so they panic inside a state migration exactly as `FugueText::insert_str` does, and for the same reason.

## AI Agent Mental Model: CRDT Merge Architecture

### Two Merge Contexts (Critical for Understanding)

The storage system has **two different merge contexts**:

#### Context A: Non-Root Entity Sync (try_merge_non_root)

When individual entities (map entries, vector elements, etc.) are synced:

```
Entity Conflict -> Has CrdtType? -> is_builtin_crdt()? -> merge_by_crdt_type()
                       |                  |
                       No                 No (Custom only)
                       v                  v
                    LWW fallback      app-defined merge
```

**Custom types dispatch two different ways depending on WHERE the apply runs:**

| Apply path | How the app's rule is reached |
| ---------- | ----------------------------- |
| in-WASM (`__calimero_sync_next`, normal delta apply) | `try_merge_non_root` looks the entry's `CustomTypeId` up in the in-module registry and calls the app's `Mergeable::merge` directly |
| host-side (HashComparison / level-wise repair) | the DFS cannot call into WASM — it is synchronous, inside `with_runtime_env` — so it DEFERS the entry, and the sync driver dispatches `__calimero_merge_custom` after the session |

Neither path falls back to LWW for a `Custom`. An entry the app can no longer
merge stays divergent until the next round, which is recoverable; resolving it by
a rule the app did not choose is not.

**Key insight**: Collections (UnorderedMap, Vector, UnorderedSet) return incoming at
container-level because entries are stored as **separate entities** - each entry merges
with its own CrdtType.

An entry holding an `#[app::mergeable]` type is stamped with that type's
`CustomTypeId` at insert, which is what makes the entry reach the app's rule at
all. Without the stamp it carries `crdt_type: None`, takes the legacy branch, and
resolves last-write-wins with the app's `merge` never consulted.

#### Context B: Root Entity Sync (merge_root_state)

When the entire app state (root entity) conflicts:

```
Root Conflict -> Try merge registry (Mergeable trait) -> Error if not registered
```

The Mergeable trait implementations in crdt_impls.rs provide **recursive merge**:
- UnorderedMap::merge() iterates entries and calls value.merge(&other_value)
- Vector::merge() merges elements at same indices recursively
- This is where nested CRDT merging happens!

**I5 Enforcement**: `merge_root_state()` requires explicit registration. If no merge
function is registered, it returns an error rather than silently falling back to LWW.

### Merge Decision Tree (Corrected)

```
+---------------------------------------------------------------------+
|                    ENTITY CONFLICT DETECTED                         |
|               (existing.updated_at <= incoming.updated_at)          |
+-------------------------------+-------------------------------------+
                                |
                    +-----------+-----------+
                    |   Is ROOT entity?     |
                    +-----------+-----------+
                                |
            +-------Yes---------+----------No-----------+
            |                   |                       |
            v                   |                       v
+-----------------------+       |       +-------------------------+
|  merge_root_state()   |       |       |  Has CrdtType metadata? |
|  1. Try merge registry|       |       +-----------+-------------+
|     (Mergeable trait) |       |                   |
|  2. Error if not      |       |           +---No--+---Yes---+
|     registered (I5)   |       |           |                 |
+-----------------------+       |           v                 v
                                |      LWW fallback    is_builtin_crdt()?
                                |      (legacy data)          |
                                |                     +---Yes-+---No---+
                                |                     |                |
                                |                     v                v
                                |          merge_by_crdt_type()   WASM callback
                                |                                 (or LWW fallback)
```

### merge_by_crdt_type() Dispatch Table

| CrdtType       | Function              | Behavior                              |
| -------------- | --------------------- | ------------------------------------- |
| `GCounter`     | `merge_g_counter()`   | Counter::merge() - max per executor   |
| `PnCounter`    | `merge_pn_counter()`  | Counter::merge() - max per executor   |
| `Rga`          | `merge_rga()`         | RGA::merge() - union characters       |
| `FugueText`    | `merge_fugue_text()`  | FugueText::merge() - union blocks     |
| `FugueTextBlock`| `merge_fugue_text_block()` | Per-block join; the arm the SYNC path reaches* |
| `LwwRegister`  | Returns incoming      | Timestamp comparison done by caller   |
| `UnorderedMap` | Returns incoming      | Entries are separate entities*        |
| `UnorderedSet` | Returns incoming      | Entries are separate entities*        |
| `Vector`       | Returns incoming      | Entries are separate entities*        |
| `UserStorage`  | Returns incoming      | LWW per user                          |
| `FrozenStorage`| Returns existing      | First-write-wins (immutable)          |
| `Custom`       | `WasmRequired` error  | Variant-only dispatch cannot resolve it — the caller must, using the entry's `CustomTypeId`. See above. |

*These types use "Structured" storage - container metadata only; entries sync separately.

*`FugueTextBlock` is the per-block join the sync path reaches; see the `FugueText`
constraints above for why those entries carry their own tag.

### is_builtin_crdt() Definition

```rust
pub fn is_builtin_crdt(crdt_type: &CrdtType) -> bool {
    !matches!(crdt_type, CrdtType::Custom(_))
}
```

**ALL variants except Custom are built-in!** `Custom` is not a gap — it is the
opt-in from `#[app::mergeable]`, and the app's own rule decides those entries.

## Key Files for Merge Understanding

| File                              | Purpose                                    |
| --------------------------------- | ------------------------------------------ |
| `src/merge.rs`                    | merge_by_crdt_type(), merge_root_state()   |
| `src/merge/registry.rs`           | Merge registry for Mergeable types         |
| `src/interface.rs`                | try_merge_non_root(), save_internal()      |
| `src/collections/crdt_meta.rs`    | Mergeable, CrdtMeta traits (re-exports CrdtType from calimero-primitives) |
| `src/collections/crdt_impls.rs`   | Mergeable implementations for all CRDTs    |

## File Organization

```
src/
├── lib.rs                    # Public exports
├── entities.rs               # Entity traits
├── interface.rs              # Storage interface (merge dispatch here!)
├── merge.rs                  # merge_by_crdt_type(), merge_root_state()
├── merge/
│   └── registry.rs           # Merge registry for Mergeable types
├── collections.rs            # Collections re-exports
├── collections/
│   ├── crdt_meta.rs          # Mergeable, CrdtMeta traits (re-exports CrdtType)
│   ├── crdt_impls.rs         # Mergeable implementations
│   ├── counter.rs            # GCounter/PnCounter CRDT
│   ├── lww_register.rs       # Last-write-wins register
│   ├── unordered_map.rs      # Unordered map
│   ├── sorted_map.rs         # Ordered map (node-local index: range/prefix/page)
│   ├── authored_map.rs       # Map with per-entry ownership
│   ├── authored_sorted_map.rs# Per-entry ownership + the ordered index
│   ├── authored_vector.rs    # List with per-element ownership
│   ├── unordered_set.rs      # Unordered set
│   ├── vector.rs             # Vector CRDT
│   ├── rga.rs                # RGA (replicated growable array)
│   ├── fugue.rs              # Pure Tree-Fugue algorithm (no storage)
│   ├── fugue_text.rs         # Storage-backed Fugue text, run-length blocks
│   ├── root.rs               # Root collection
│   ├── nested.rs             # Nested CRDTs
│   ├── nested_map.rs         # Nested map
│   ├── frozen.rs             # Frozen collections
│   ├── frozen_value.rs       # Frozen value
│   ├── decompose_impls.rs    # Decompose implementations
│   ├── composite_key.rs      # Composite key
│   ├── user.rs               # User collection
│   ├── error.rs              # Collection errors
│   └── ...
├── address.rs                # Address types
├── action.rs                 # Actions
├── delta.rs                  # Delta handling
├── snapshot.rs               # Snapshots
├── store.rs                  # Store adaptor
├── index.rs                  # Entity indexing (Merkle tree)
├── env.rs                    # RuntimeEnv (storage backend injection)
├── js.rs                     # JS bindings
├── logical_clock.rs          # HLC (Hybrid Logical Clock)
├── constants.rs              # Constants
├── error.rs                  # Errors
└── tests/
    ├── merge_dispatch.rs     # Tests for merge_by_crdt_type
    ├── merge_integration.rs  # Integration tests
    └── ...
```

## Gates take accounts; stamps keep devices

The one rule to get right when touching access control here. Both ids are 32
bytes, so confusing them **compiles**, and most tests pass either way.

| | reads | why |
| --- | --- | --- |
| **Gate** — "may this person write?" | `env::account_id()` | a writer set names people, so one grant covers every device they hold |
| **Stamp** — "who wrote this?" | `env::device_id()` | per-writer state: two devices sharing a counter slot or an HLC seed lose each other's writes |

The boundary is **per file for most of the tree, and per symbol in two places.**
`shared.rs`, `access_control.rs` and `permissioned.rs` are entirely principals.
`user.rs` and `authored_*.rs` hold BOTH: an entry's `owner` is a gate — it decides
who may `update`/`remove` — so it is an `AccountId`, while the device that wrote
the entry is stamped on `signature_data.signer`. Everything else in those files
that reads `device_id()` (an LWW tiebreak, a counter slot, an HLC seed) is still a
stamp and must stay one.

Still do not run a blanket `PublicKey → AccountId` replace: it builds, and it
would sweep those per-writer stamps onto accounts, where two devices of one person
share a counter slot and lose each other's writes.

A signature can only name a **key**, so the bridge is
`ApplyContext.signer_account`: the account that key speaks for, resolved by the
NODE at the write's causal cut and passed in. This crate has no store and no cut,
so it cannot resolve one itself, and it must never fall back to the locally
executing account — a remote action would then authorize itself.

`None` means "this path could not resolve it", and what that costs depends on the
gate. For a `Shared` writer set it is a refusal. For a `User` owner it DEFERS —
the sync repair paths (HashComparison, snapshot, level-wise) apply through an
`ApplyContext::empty()` and refusing there would drop every legitimately repaired
entry. The ownership check for those runs in `calimero-node`
(`is_leaf_currently_authorized`), which has the bindings this crate lacks.
Signature authenticity is enforced here on every path, because that needs none.

**The caller owes a contract this crate cannot check:** `signer_account` must be
the resolution of *that action's own* signer. Storage verifies the signature under
the key the action names and checks the account against the writer set, but has no
bindings with which to confirm the two describe the same principal.

Writing a test here? Derive the account from a different domain than the key (see
`tests::common::account_of_key`). A test where the two are equal cannot tell an
account-keyed gate from a device-keyed one.

## CIP Invariants (Sync Protocol)

When working with merge logic, these invariants MUST be preserved:

- **I5 (No Silent Data Loss)**: Built-in CRDT types MUST use their semantic merge rules,
  never be overwritten by LWW. GCounter contributions from different nodes must sum.
- **I10 (Metadata Persistence)**: crdt_type MUST be persisted in entity metadata for
  correct merge dispatch.

## Patterns

### Using a CRDT

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct AppState {
    data: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    Updated { key: &'a str },
}

#[app::logic]
impl AppState {
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        self.data.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.data.get(key)?.map(|v| v.get().clone()))
    }
}
```

### Counter Pattern

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::Counter;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct CounterApp {
    count: Counter,  // GCounter by default (ALLOW_DECREMENT=false)
}

#[app::logic]
impl CounterApp {
    pub fn increment(&mut self) -> app::Result<()> {
        self.count.increment()?;
        Ok(())
    }

    pub fn value(&self) -> app::Result<u64> {
        Ok(self.count.value()?)
    }
}
```

### LwwRegister Pattern

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::LwwRegister;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ConfigApp {
    setting: LwwRegister<String>,
}

#[app::logic]
impl ConfigApp {
    pub fn update(&mut self, value: String) -> app::Result<()> {
        self.setting.set(value);
        Ok(())
    }

    pub fn get(&self) -> app::Result<String> {
        Ok(self.setting.get().clone())
    }
}
```

## JIT Index

```bash
# Find merge dispatch logic
rg -n "merge_by_crdt_type" src/

# Find Mergeable trait implementations
rg -n "impl.*Mergeable" src/collections/crdt_impls.rs

# Find CrdtType definition (enum lives in calimero-primitives)
rg -n "pub enum CrdtType" ../primitives/src/crdt.rs

# Find non-root merge logic
rg -n "try_merge_non_root" src/interface.rs

# Find root merge logic
rg -n "merge_root_state" src/merge.rs

# Find CRDT implementations
rg -n "impl.*CrdtMeta" src/collections/

# Find collection traits
rg -n "pub trait" src/

# Find is_builtin_crdt
rg -n "is_builtin_crdt" src/merge.rs
```

## Storage Macros

Use calimero-storage-macros for derive macros:

```rust
use calimero_storage_macros::AtomicUnit;

#[derive(AtomicUnit)]
struct MyType {
    // fields
}
```

## Common Gotchas

- Use #[app::state] macro attribute - it auto-generates Mergeable impl
- CRDTs auto-merge on sync - no manual conflict resolution needed
- Use nested CRDTs (UnorderedMap<String, LwwRegister<String>>) for last-write-wins semantics
- Convert values with .into() when inserting: self.data.insert(key, value.into())?
- Extract values from LwwRegister with .get().clone()
- Return app::Result<T> from methods, not plain T or Option<T>
- Use ? operator for error propagation from CRDT operations
- UnorderedMap keys must be unique per context
- Vector operations are position-based
- Counter only supports increment by default (use PnCounter for decrement)
- All CRDTs must be serializable with borsh
- **Structured vs Blob storage**: Collections use structured storage (entries are separate
  entities), while counters and registers use blob storage (single serialized value)

## Further Documentation

- readme/architecture.md - Deep dive into three-layer conflict resolution
- readme/DOCUMENTATION_INDEX.md - Full documentation index
- src/merge.rs - Module-level docs on merge dispatch
- src/collections/crdt_impls.rs - Module-level docs on Mergeable implementations


## Root Entity Merge Requirement

The `merge_root_state()` function **requires** a merge function to be registered.
If no merge function is registered, it returns an error rather than silently falling
back to LWW (which would violate Invariant I5 - No Silent Data Loss).

**How to Register:**

| Method | When to Use |
| ------ | ----------- |
| `#[app::state]` macro | WASM apps (recommended, auto-registers) |
| `register_crdt_merge::<T>()` | Tests, manual registration |

**Error Message:**
```
No merge function registered for root entity.
Use #[app::state] macro or call register_crdt_merge::<YourState>().
```

👉 **See `readme/merging.md`** for detailed merge behavior documentation.
