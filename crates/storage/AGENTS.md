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
| `FugueTextBlock`           | One block of a `FugueText`| In-bounds block first, then tombstone OR + longer text wins | Structured |
| `RichText<Sc>`             | Text plus formatting marks| Composite: text union + mark union| Structured |
| `RichDocument<Sc>`         | Ordered list of rich-text blocks| Composite: spine union + per-field LWW| Structured |
| `UnorderedMap<K,V>`        | Key-value map            | Entry-wise merge*                 | Structured |
| `UnorderedSet<T>`          | Unique values            | Union (add-wins)                  | Structured |
| `Vector<T>`                | Ordered list             | Element-wise merge*               | Structured |
| `Guarded<C, P>`            | Keyed collection `C` + write policy `P` | Entry-wise, policy-gated at apply§ | Structured |
| `Authored<C>`              | `Guarded<C, Owner>`: entry owned by inserter | Entry-wise, owner-gated at apply | Structured |
| `ContentAddressed<C>`      | `Guarded<C, ContentHash>`: write-once, content-hash keys | First-write-wins | Structured |
| `WriteOnce<C>`             | `Guarded<C, OwnerOnce>`: owned, never edited or deleted | Entry-wise, owner-gated, immutable | Structured |
| `Moderated<C>` / `ModeratedOnce<C>` | `Guarded<C, Moderation<_>>`: owned, a moderator set may delete | Entry-wise, owner- or moderator-gated | Structured |
| `Frozen<T>`                | One value; a writer-set cell whose one writer holds `WRITE_ONCE` | Nothing to merge | Structured |
| `Registry<K, V, A>`        | One owner per name, decided by an authority `A` (`Tee`, `Admin`, `NoAuthority`) | Claims owner-gated; verdicts one entry each, standing = max | Structured |
| `AuthoredMap<K,V>`         | `Authored<UnorderedMap<K,V>>` (alias) | Entry-wise, owner-gated at apply | Structured |
| `AuthoredSortedMap<K,V>`   | `Authored<SortedMap<K,V>>` (alias) | Identical to `AuthoredMap`†    | Structured |
| `IndexedMap<K,V>`          | `UnorderedMap` + secondary indexes | Identical to `UnorderedMap`‡ | Structured |
| `AuthoredVector<T>`        | List, slot owned by author | Element-wise, owner-gated at apply | Structured |
| `UserStorage`              | Per-user data            | LWW per user                      | Blob       |
| `FrozenStorage`            | Immutable data           | First-write-wins                  | Blob       |
| `TeeSecret<T>`             | Value only TEE authorities read | `TeeOnly` + LWW of a `Sealed<T>` | Blob |

*Structured storage: Entries are separate entities with their own CrdtType, merged individually.

†`AuthoredSortedMap` reports `CrdtType::UserStorage`, the SAME variant as
`AuthoredMap`, on purpose: its ordering is a node-local derived index that is
never replicated, so two nodes holding the same entries — one using each
collection — must agree on the root hash, and do. Reach for it when the keys are
hierarchical and reads are slices: `entries()` on an authored collection is
linear in everything anyone has ever written, and on an authored collection
nobody can delete anyone else's entries, so that is a liveness floor and not
just a speed one. Measured in `tests/read_cost_profile.rs`.

§`Guarded<C, P>` separates how entries are read (the collection `C`:
`UnorderedMap`, `SortedMap` or `IndexedMap`) from who may change them (the
policy `P`). See the `Guarded` constraints below.

‡`IndexedMap` reports `CrdtType::UnorderedMap` and serializes byte for byte as
the `UnorderedMap` it wraps: its indexes live in the same node-local, non-synced
keyspace as `SortedMap`'s, so they reach neither the wire nor the root hash, and
switching a field between the two types needs no migration.

### `Guarded` constraints

- A policy is a stamp on each entry, checked by every node in
  `Interface::apply_action`: `StorageType::User { owner, rules }` for `Owner`,
  `OwnerOnce` and `Moderation`, `StorageType::Frozen` for `ContentHash`. The
  checks in `insert`/`modify`/`remove` only fail early. An entry carries
  exactly ONE stamp, so policies are a type parameter, never nested wrappers:
  `Authored<ContentAddressed<C>>` could not mean anything.
- `EntryRules { immutable, moderators }` rides in the `User` stamp and is
  hashed into `payload_for_signing`, so rules are signed and fixed at creation.
  Apply refuses an update or delete naming different rules; an `immutable`
  entry takes no delete but a moderator's, and of its owner's authentic writes
  keeps the one with the lowest `(signed nonce, SHA-256 of bytes)`
  (`written_once_order`), however they arrive. Two devices of one account write
  the same entry, so keeping the first to arrive split nodes for good; the
  lowest is the same everywhere. An earlier write replaces the stored one
  through `replace_written_once` (bypassing `save_internal`'s LWW guard) before
  the stale-nonce skip can drop it; a later one is refused unless its bytes are
  identical. The nonce has no lower bound, so the owner can replace its own
  entry with a backdated write; no one else can. `tests/write_once_devices.rs`
  and `tests/converge_write_once.rs` pin it. A `moderators` delete is checked
  with `resolve_anchor_writers_as_of(anchor, nonce)` for `DELETE`, so revoking
  a moderator never undoes their earlier removals. A collection reads only
  entries whose rules equal its own (`Domain::admits`): an entry written with
  weaker rules is stored and never returned.
- **Deleting an `immutable` entry is terminal** (only a moderator can: the owner
  may not delete one). The delete wins over every write to that owner's key,
  earlier or later: apply skips the delete's nonce check and
  `apply_delete_ref_action`'s LWW comparison (`terminal`), and the upsert arm
  and the local `add_child_to` refuse a write where
  `refuse_deleted_written_once` finds a tombstone. A delete that reaches a node
  before the entry is verified against its own stamp and kept as a seal
  (`Index::seal_written_once`) at an id derived from the entry id AND its rules,
  so a seal signed under rules naming someone else's moderators never matches
  the owner's real write. The lasting record is that tombstone or seal:
  `EntityIndex::is_terminal_tombstone` rows are never collected by the node's
  tombstone GC (`calimero-node`'s `gc.rs`). A receiver stores the delete's
  signature on the tombstone, so a HashComparison repair ships it
  (`deleted_children`) as a delete any node verifies. Network snapshots carry no
  tombstones, so a joiner bootstrapped from one lacks the record until a late
  write lands on it, and a repair from any peer that held the entry then
  deletes it. `tests/write_once_deletes.rs` pins every interleaving.
- **Nested collections inherit the enclosing entry's domain** (`domain.rs`).
  `find_by_id` deserializes under `with_ambient(Domain::inherited_from(stamp))`,
  so a collection loaded from inside a guarded entry carries that entry's
  `Domain` on its `Element` (`#[borsh(skip)]`). A domain is three things at
  once: a read filter (entries whose stamp it does not admit are invisible), a
  stamp for writes (`stamp_for`), and a local authority check
  (`check_authority`). A `Sealed` domain (inside an immutable or content-hashed
  entry) refuses every nested write. Break any of the three and
  `tests/nested_domains.rs` fails; that is the whole protection, since a
  nested entry has its own id and would otherwise be `Public`.
- `Deref<Target = C>` gives every read of the inner collection for free.
  There is deliberately no `DerefMut`; every write goes through the policy.
  Inherent `get`/`contains` shadow the inner ones so `get` returns the value.
- Layout is `{ inner: C, storage: Element, policy: P }`, the inner id derived
  from `P::inner_prefix::<C>()` + field name. A unit policy adds no bytes;
  `Moderation` stores its moderators' `WriterSetCell`, reassigned as
  `__moderators_{field}`. Prefixes: `__authored_map_` (`UnorderedMap` and
  `IndexedMap`), `__authored_sorted_map_`, `__write_once_`, `__moderated_`,
  `__moderated_once_`, `__frozen_storage_` (`ContentAddressed`). Change a
  prefix and existing state stops resolving. `guarded/tests.rs` pins them.
- `Mergeable` is a no-op for every policy: delegating to the inner merge would
  insert a key present only in `other` with a `Public` stamp, stripping it.
- `ContentAddressed::insert` hashes `borsh(value)`, which is what the receiving
  node's `verify_frozen_action_upsert` recomputes from the entry bytes, so a
  content-addressed value is plain data.
- **Keys are per owner.** Every `StorageType::User` entity lives at
  `owned_entry_id(slot, owner)` (`collections.rs`): the slot's first 12 bytes
  (its child-trie bucket, and any TEE-only tag), an 8-byte owned tag, then 12
  bytes of `SHA256(slot[..12] ‖ owner)`. Two accounts writing one key write two
  entities, on every node, in any order. `refuse_entity_at_reserved_id`
  (`interface.rs`, in `apply_action` and snapshot verification) refuses an
  owned entry at an id not bound to its owner and anything else at a tagged
  id; the local write path (`add_child_to`, `save_raw`) refuses the same, and
  `Collection::insert_with_storage_type` derives the id from the FINAL stamp
  (`stored_id`), so a caller passes the slot. Nested ids derive from the stored
  id, so two owners' entries at one key hold distinct nested collections. A slot
  in a `SharedStorage` cell's value subtree takes a different layout; see the
  cell bullets under Common Gotchas.
- **An owned id is one of four kinds** (`OwnedIdKind`, `collections.rs`), told
  by the 8-byte tag at bytes `12..20`. Every kind keeps a 96-bit owner binding
  in bytes `20..32`; the binding hashes leave the tag out, so the keyed and
  unkeyed ids of one owner at one slot differ in the tag alone (`keyed`).

  | kind | bytes `0..12` | tag (`12..20`) | bytes `20..32` | used by |
  | --- | --- | --- | --- | --- |
  | `Owned` | slot `[..12]` | `OWNED_ID_TAG` `CA 'owned' 00 00` | `SHA256(owned sep ‖ slot[..12] ‖ owner)[..12]` | `AuthoredVector`, nested owned entries |
  | `OwnedKeyed` | slot `[..12]` | `OWNED_KEYED_ID_TAG` `CA 'owned' 00 'k'` | as `Owned` | a map's / `UserStorage`'s entry |
  | `CellOwned` | slot `[20..32]` | `CELL_OWNED_ID_TAG` `CA 'cel' 00 'own'` | `SHA256(cell-owned sep ‖ anchor binding ‖ owner)[..12]` | an unkeyed owned entry in a cell |
  | `CellOwnedKeyed` | slot `[20..32]` | `CELL_OWNED_KEYED_ID_TAG` `CA 'celkown'` | as `CellOwned` | a map's / `UserStorage`'s entry in a cell |

  The four tags sit at the same bytes and have the same length, so none is a
  prefix of another; a `const` assertion keeps them distinct. A keyed kind's
  entry ends in `u32_le(key len)` after its id, and its key must derive its
  slot under its parent (`key_fits_id`); a cell kind is bound to its cell
  through the parent (`cell_owned_id_binds`). An owned entry takes a cell kind
  exactly when its parent lies in a cell's value subtree
  (`owned_kind_fits_parent`, in `refuse_unbound_cell_owned_entity`): an id of
  another kind there would be one outside every cell whose first bytes a reader
  in the cell takes for a key's. A keyed collection reads and counts only the
  keyed kind its own id gives its entries (`OwnedIdKind::under`,
  `Collection::key_fits`, `keyed_len`). Apply checks all of this against the
  parent the action names, or the stored one when it names none
  (`owned_parent` in `apply_action`); snapshot verification against the
  record's parent; the local path against the parent it links under.
  `assert_every_owned_entry_is_bound` and `assert_every_shared_entity_is_bound`
  check every kind store-wide. Changing a tag, a separator or a layout changes
  every id a node derives and accepts: bump `SIGNED_NAMESPACE_OP_SCHEMA_VERSION`.
- Every key-only method (`insert`, `get`, `contains`, `update`, `modify`,
  `remove`, `owner_of`, `owned_by_me`, `entry_schema_version`) acts on the
  CALLER's entry (`Collection::resolve`). Name another owner with `get_by`,
  `contains_by`, `entry_schema_version_by`; a moderator uses `remove_by`.
  Authorization-shaped app logic must name the account it is about. Iteration,
  `len`, and the ordered reads span every owner; one key appears once per
  owner, ordered by key then id. A `SortedMap` in an owned domain files one
  index row per entry (`component(key) ‖ id`, see `SortedMap::index_row`), and
  `IndexedMap` rows already carry the entry id. A globally unique name needs
  `Registry` (or `ContentAddressed`, when the key is the content), not an
  owning policy.
- **A keyed collection's owned entry holds the key its id derives.** A map's
  or `UserStorage`'s owned entry lives at a KEYED owned id
  (`owned_keyed_entry_id`: the owned id with its own tag), and its bytes are
  `borsh((V, K)) ‖ id ‖ u32_le(key.as_ref().len())`: the `Element` of an entity
  at a keyed id writes the length after the id. `key.as_ref()` is the tail of
  `borsh(key)` for `String`, `Vec<u8>`, `[u8; N]`, account ids and the
  crate's own keys, so apply finds the key without its type
  (`keyed_entry_key`), and `refuse_misfiled_owned_entry` refuses an entry whose
  key does not derive its slot: in `apply_action` (under the parent the action
  names, or the one it is stored under when it names none), in snapshot
  verification (the leaf index's parent), and on the local write path
  (`add_child_to`, `save_raw`), which is also what refuses a custom key whose
  bytes are not its encoding's tail. Apply never creates a missing ancestor at
  a keyed id, since an ancestor comes without its bytes. A keyed collection in
  an owned domain reads and counts only keyed ids (`Collection::key_fits`,
  `Collection::keyed_len`), so `len` is exact and still reads no entry (nor,
  after a node's first count, any child: see the counting bullet below). What
  apply cannot see is whether the bytes decode: an entry whose value or key
  does not decode, or whose key contradicts its own length, reads as absent and
  is counted, like an undecodable entry of any collection.
  `tests/owned_collisions.rs` pins all of this.
- The read-side key check needs the key's `AsRef<[u8]>` bytes, so the policy
  that sets the domain names them (`bind_slot_keys`, in `Guarded::from_parts`
  and `UserStorage`'s `owned`). Iteration, `Debug`, `PartialEq`, `Ord` and
  `Serialize` therefore ask nothing of `K` beyond borsh (and `Ord` on a
  `SortedMap`), and `get` only that the borrowed key be bytes;
  `tests/key_bounds.rs` holds that. An owned collection whose keys were never
  bound reads no owned entry, so a new owning wrapper must bind them too.
- `GuardedEntries` and `Policy` are sealed: a policy is only as strong as the
  check the storage layer runs for it on apply.
- **A guarded collection counts from a node-local tally, never by loading its
  children** (`admitted_count.rs`). Its trie's `count` includes entries its
  domain does not admit, which apply cannot refuse (the domain is the
  collection's type, never stored), so `len`/`keyed_len` used to load every
  child: `AuthoredVector::len` read ~2 rows per entry, and mero-chat's
  `send_message`, which counts the channel, ran out of gas at ~3,200 messages.
  The tally is one row per counted collection in the index plane
  (`index_meta_put` at `SHA256("calimero:admitted-count:v1" ‖ collection id)`,
  beside `SortedMap`'s markers): the domain, the admitted and keyed counts, and
  the trie root they are exact at. `ChildTrie::insert`/`remove` carry it across
  every link and unlink, local or applied (`admitted_count::before_change`,
  then `Pending::finish`), classifying the linked or unlinked child by the stamp
  in its own index row, which is what the collection's reads see; a child
  replaced in place moves nothing, since no path rewrites a linked child's
  admission except `Index::set_storage_type`, which drops the parent's row.
  `drop_all` drops it. It is trusted only at the root it names,
  so a trie change that bypasses it (snapshot's `insert_with`, an older binary)
  leaves it stale and the next count loads the children once and records it
  again. Nothing in it is synced or hashed, and a read-only call may write it
  (`ReadOnlyContextStorage::with_local_index`). An adaptor without the index
  plane (`PrivateStorage`) keeps the linear count. Every link pays one index
  read for the lookup. `contains` asks the trie for the one child
  (`Index::child_of`) instead of loading the set, which made every keyed
  insert (`Guarded::insert`, `UnorderedMap::entry`, `SortedMap::insert`)
  linear too; a removal likewise finds its child with one bucket read
  (`remove_child_from_inner`) and touches the child cache only if it is
  already loaded (`EntryMut::remove`), as `CollectionMut::insert` does. `tests/owned_collection_cost.rs` and the `authored_*`
  cost-gate workloads pin the cost; `len_stays_exact_as_the_trie_changes_under_it`
  (in `authored_vector.rs`) pins the tally against links it admits, links it
  does not, removals, a bypassing link and a stamp rewritten in place.
- `AuthoredVector`, `FrozenStorage` and `UserStorage` keep their own types:
  index-keyed with tombstones, an older API (`get` returns `T`, not the stored
  wrapper), and one slot per account.

### `Frozen<T>` constraints

- A `WriterSetCell<FrozenValue<T>>` built by `new_write_once`: one writer, the
  creating account, with `OpMask::WRITE_ONCE` (`0b1000`) and nothing else. No
  `DELETE`, no `ADMIN`, so the value cannot be removed or its writer rotated.
- `enforce_put_mask` accepts a `Put` from a `WRITE_ONCE` writer only while the
  entry is absent or the bytes are identical, so genesis lands on a fresh node
  and sync redelivery converges, while a rewrite signed by the writer is
  refused. `tests/frozen_values.rs` plays each peer.
- Two writes never race for one value, so first-arrived is safe here as it is
  not for `WriteOnce`: a root field is written by genesis alone, and a nested
  cell is minted at `cell_id(Id::random(), writers)`, so two devices creating
  one at the same key create two cells. `tests/converge_write_once.rs` pins it.
- Trust is `SharedStorage`'s: the writer set comes from genesis.

### `Registry` constraints

- Two halves, both reassigned from the field name: `claims`, an
  `Authored<UnorderedMap<K, Claim<V>>>` at `__registry_claims_{field}` (one
  owned entry per claimant, so `entries_at` reads every claimant of a name in one
  bucket), and the authority's `VerdictStore` at `__registry_verdicts_{field}`: a
  `TeeOnly<SortedMap<VerdictKey, Verdict>>` for `Tee`, a
  `SharedStorage<SortedMap<..>>` whose writers are the admins for `Admin`,
  nothing for `NoAuthority`. The existing `TeeOnly`/`Shared` apply rules are the
  whole enforcement; verdicts need no rule of their own.
- **One entry per verdict, never one per name.** `VerdictKey` is
  `H(name) ‖ (u32::MAX - epoch) ‖ vacant ‖ order ‖ by`, so a name's verdicts
  share a prefix, sort best first (highest epoch, a grant before a vacancy,
  lowest order), and two devices' verdicts never share an entry. The standing is
  `A::standing` over every well-formed one: the first, by default; a quorum
  authority would count votes there. The standing is decided by the keys, so it
  needs no custom merge. (A per-name cell merged by a custom rule used to split
  nodes by delivery order through the stale-nonce skip below; that is fixed.)
- `order = H(claim_ref ‖ vacant)`, `claim_ref = H(name ‖ epoch ‖ owner)`: no clock, no
  claim bytes, so it cannot be backdated and a claimant moves its rank only with
  another account. Readers recompute both and skip a verdict whose key or hashes
  do not match. The default `pick` is the lowest order among live claims, so the
  merge of two authorities' verdicts is what one authority seeing both picks.
- A grant counts only when its claim (same owner, same epoch) is here; before
  that the name reads `Pending` and `owner_of` is `None`. Release marks the
  owner's claim; the authority answers with a vacancy at `epoch + 1`; claims
  target the open epoch (0, or the vacancy's), and older ones are stale.
- `resolve` is idempotent: it writes only when the standing needs a vacancy or
  a grant. `stable` is `A::stable`: by default, no grant of another claim at
  the standing epoch.
- Tests: `registry/tests.rs` (API, forged verdict and claim refused on apply,
  bound ids), `tests/converge_registry.rs` (`testing::Script`: every causal
  delivery order of two TEEs granting different claimants, a stale lower-epoch
  grant, two admin devices, no authority; and a state field's verdict reaching
  members by delta and by `Script::push`, a HashComparison-style repair).

### `IndexedMap` constraints

- The value type declares its indexes through `Indexed` (normally
  `#[derive(app::Indexed)]`). Each index's rows are keyed by the index NAME, so
  reordering declarations is free and renaming one rebuilds it.
- A row is `components ‖ entry_id`. A component is the value's order-preserving
  bytes (integers big-endian, signed ones offset-binary) with `0x00` escaped to
  `0x00 0xFF`, closed by `0x00 0x01`. That makes byte order value order, makes no
  encoded value a prefix of another, and makes a compound key's leading
  components a prefix of the whole — `eq` on them is a seek, and the rest order
  the result. `ENCODING_VERSION` is in the marker: change the encoding, bump it.
- Correctness never depends on writes going through the map. The marker holds the
  collection's `full_hash` plus a fingerprint of the declarations; a query that
  finds it stale rebuilds (reads every entry, writes only differing rows). A write
  maintains the indexes and re-stamps **only if the marker was current before the
  write** — re-stamping after a stale one would certify rows a sync never wrote.
- There is no `get_mut`: `update(key, f)` is the in-place mutation, and it keeps
  the indexes in step. A mutable guard would still be correct (the marker catches
  it) but would cost a full rebuild on the next query.
- A rebuild whose writes did not all land (an execution with node-local writes
  suppressed, like the migration check) makes that query answer by scanning, not
  from the unbuilt index. An adaptor with no ordered keyspace (`PrivateStorage`)
  always scans. Both give the same answers as the index; `tests.rs` pins that.
- Descending reads use `StorageAdaptor::index_last_in`, one bounded reverse seek
  per row, so "newest twenty" is `O(20 log n)` however large the index.

### `FugueText` constraints

- Every position is an index into Unicode SCALAR VALUES (Rust `char`), never bytes and never UTF-16 code units.
  That is `insert`, `insert_str`, `insert_str_with_replica`, `delete`, `delete_range`, `text_range`, `char_at`, `anchor_at`, `len` and every `TextOp` an `apply_delta` carries.
  An astral character is one position and a combining mark is its own, so a grapheme cluster spans several; a browser counts UTF-16 code units, where an astral character is two, so a TypeScript binding converts on both edges and core stays as it is.
  A run is capped in nodes and one node holds one scalar value, so a cap boundary can never land inside a character (`scalar_value_tests`).

- A block holds at most `MAX_RUN_LEN` (256) nodes, and a full run is never rewritten or split.
  The overflowing character opens a new block parented on the full run's last node, side right.
  Only `tools/storage-cost/tests/keystroke_bytes.rs` gates this, because row counts cannot see it.
- No node-local derived state: order is recomputed from the stored blocks on every call, because gas must be equal on every replica.
  So an insert by position reads one row per block, which is linear in the document (about 40 rows at 10,000 characters).
  A replicated position index does not help: a visible position needs live counts, a count is not joinable (two replicas deleting one character would count it twice), so the index has to hold every block's tombstones, and every keystroke would ship it in its delta.
- A block is in bounds (`TextBlock::is_sound(key)`, judged against the map key) when it is stored at its own start id, holds 1 to `MAX_RUN_LEN` nodes, every node counter and its parent's is below `u32::MAX`, and its tombstone bitmap is trimmed with no bit past the run. `load` and `merge_blocks_from` leave any other block out, and a row filed under an id its key does not derive too. The apply path stores such a row unfiltered; `merge_blocks_from` drops a lone one. `RichDocument`'s spine and every `FugueText` read go through `load`.
- `join_under` orders the sync join by that check: one side in bounds gives exactly that side (the other's tombstones are not merged in); two in bounds go through `join_block`; of two out of bounds the greater by every field stays, so neither turns readable. The result is the same in either order and grouping.
- Minting never produces a block out of bounds: `next_counter` and `bump` return `COUNTER_EXHAUSTED` rather than use `u32::MAX`, and a write moves its first counter past any row left out that its run could land on, since that row's timestamp could win over the write. The goal is a document that stays readable, not that no peer can stop a replica typing: a peer can still store a block in bounds near a replica's top counter and exhaust that replica, which then errors on insert instead of losing the character.
- Rows filed at one id under different keys join by the greater key, whole, and only rows under one key go through `join_under`, so the pick does not depend on which side is held. Rows filed under an id their key does not derive are left out of reads and mints; a write moves past the id such a row holds (the mint probes `entry_id` per counter, only while any exist).
- Mixed versions: when a block out of bounds and one in bounds meet at one key, a node without this change joins them with `join_block` (the result can be out of bounds) while a node with it keeps the side in bounds. Stored bytes differ, and repair churns until every node has upgraded.
- Known limits: two overlapping runs of one replica, both in bounds, can make a delete fail (`find_block`/`bury`). A lone out-of-bounds row is stored by the apply path but dropped by `merge_blocks_from`, so root hashes can differ until a sync reaches it. A mark minted in the partial-sync window can lose to a row left out at its id once the two meet (the write is last-writer-wins), whereas a block in bounds always wins the join. No release wrote untrimmed tombstones: `tomb_set`, `tomb_or` and `tomb_trim` have been the only writers since the file first shipped (0.11.0-rc.54).
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
- `marks()` leaves out a row whose lamport exceeds the number of mark rows, or whose key is not its own id, or that is filed under an id its key does not derive, so minting and every read ignore it. An honest lamport is one more than the greatest the writer saw, hence at most the row count. This assumes rows are never removed or compacted: a compaction must keep the row count at or above the greatest lamport. Under partial sync a replica can hide a row until the earlier rows arrive. Padding rows raise the bar only by their own number, and one left out at the id the next mark would take (found by the stored id of the row, whatever its own id says) makes that mark fail (`mark id already in use`) instead of being written where it cannot be read. A hidden high-lamport row becomes visible once the document reaches that many mark rows and then wins over its whole range. A row at (local replica, greatest visible lamport + 1) keeps failing that replica's marks until another replica mints past it.
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

**A stale signed write still reaches the merge of an entry that merges whatever
the order.** `apply_action`'s `User`, `Shared` and `SharedMember` arms skip a
write whose nonce is below the stored `updated_at`, which is right for
last-write-wins and wrong for an app's rule: the node that saw the newer write
first never took the older in, and a "keep the lower" rule read 9 on one node
and 3 on the other. `stale_write_still_merges` exempts an entry whose STORED
`crdt_type` is one `save_internal` merges before comparing timestamps
(`merges_whatever_the_order`: `Custom` and `RotationLog` off the root, and
`FugueTextBlock` on applied bytes), and only when the write names that same type,
because `crdt_type` is not signed. Every signature, writer-set, mask and owner
check runs before the skip and is unchanged, so a non-writer's stale write is
still refused. The merge is idempotent, so a replayed older write changes
nothing it has not already folded in, and `save_internal` keeps the stored
`updated_at` at the newer of the two. What stays open: a merge whose result
equals neither side's bytes keeps the stored side's signature, which does not
cover the merged bytes, so a snapshot or repair that ships that leaf cannot be
verified by the receiver (true of the newer-write direction before this, too).
`tests/converge_signed_mergeable.rs` replays both orders for a cell and an
`Authored` map; `stale_write_to_a_merging_entry` in `src/tests/interface.rs`
pins the refusals.

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
| `FugueTextBlock`| `merge_fugue_text_block()` | Per-block join, in-bounds block first; the arm the SYNC path reaches* |
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
│   ├── guarded.rs            # Guarded<C, P> and its aliases: one write policy over any keyed collection
│   ├── authored_map.rs       # AuthoredMap = Authored<UnorderedMap> (alias + its tests)
│   ├── authored_sorted_map.rs# AuthoredSortedMap = Authored<SortedMap> (alias + its tests)
│   ├── indexed_map.rs        # UnorderedMap + node-local secondary indexes (Indexed, IndexValue, Query)
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
│   ├── frozen_cell.rs        # Frozen<T>: one write-once value
│   ├── registry.rs           # Registry<K, V, A>: one owner per name, decided by an authority
│   ├── decompose_impls.rs    # Decompose implementations
│   ├── composite_key.rs      # Composite key
│   ├── user.rs               # User collection
│   ├── tee_secret.rs         # Sealed<T> and TeeSecret<T>: values sealed to members or to the TEE
│   ├── error.rs              # Collection errors
│   └── ...
├── address.rs                # Address types
├── action.rs                 # Actions
├── delta.rs                  # Delta handling
├── reclaim.rs                # What tombstone GC may reclaim from raw rows (used by node gc.rs)
├── snapshot.rs               # Snapshots
├── store.rs                  # Store adaptor
├── index.rs                  # Entity indexing (Merkle tree)
├── child_trie.rs             # A parent's children as a hash trie (bounded-cost link/unlink)
├── admitted_count.rs         # Node-local count of the children a guarded collection admits
├── domain.rs                 # Domain: what nested collections inherit from a guarded entry
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

The boundary is **per file for most of the tree, and per symbol in three places.**
`shared.rs`, `access_control.rs` and `permissioned.rs` are entirely principals.
`user.rs`, `guarded.rs` and `authored_vector.rs` hold BOTH: an entry's `owner` is
a gate — it decides
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

**A relay writes on an account's behalf by saying so.** When a relay executes a
call for an account (a warrant-carried delegated run), it signs the resulting
entries with its OWN key: `signature_data.signer` is the relay's key — still the
key the signature verifies under, never a key that did not sign — and
`signature_data.on_behalf` is the account it wrote for. The signed payload
commits to `on_behalf` (`hash_signature_data` in `action.rs`), so it cannot be
stripped, added or swapped in transit. Ownership and the writer-set checks are
asked of that account (`Interface::author_account`): the node must resolve
`signer_account` to exactly the `on_behalf` account, which it does only when the
signer's account is a `RelayTee` in the namespace and the account is a member who
may write (`calimero_governance_store::on_behalf_standing`; no capability bit is
consulted). On the delta path the resolution is per action: the node judges each
on-behalf action at the delta's cut and lists the accepted ones in
`StorageDelta::CausalActions::on_behalf_accounts` (action id → account), which
`Root::sync` uses in place of the delta-wide `signer_account` for that action. So
the delta's author need not be the account (a relay may write any member's
entries), and an on-behalf action the node did not list is checked against the
delta-wide account as before. The node refuses a delta with an on-behalf action
it cannot accept (`calimero-node`'s `delta_store::on_behalf_accounts`).
Any other resolution — the relay's own account, a third account, `None` — is
refused. A User refusal names which check fired: `bad-signature`,
`author-unresolved` or `wrong-author`. `tests/on_behalf.rs` pins every arm.

**Known limitation: a writer-set rotation cannot be made on someone's behalf.**
`RotationLogEntry` records the key that signed a rotation (`signer`) and carries
no `on_behalf`, and rotation-log authentication checks that key's account against
the prior writer set's `ADMIN` bit. A rotation a relay signs for an account is
therefore attributed to the relay, which is not in the set, and is refused by
every peer that authenticates the log. Delegated runs can write `Shared` and
`SharedMember` entries for an account but cannot change a writer set for it.
Deferred: closing it means an `on_behalf` on the rotation entry, covered by its
signature, and the same rule at rotation authentication.

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
- **A local write is stamped after what it overwrites, never just "now".**
  `save_raw` stamps `max(now, stored updated_at + 1, deleted_at + 1)` (the
  `stamp_after_stored` helper, on the index row it already reads), a delete
  `max(now, updated_at + 1)`, and `LwwRegister` `hlc.after(previous)`. The wall
  clock can read earlier than a stored stamp (an NTP step back, a peer up to 5s
  ahead), and the guest HLC restarts every execution, so a plain `time_now()`
  stamp dropped the write. Do not add a write path that stamps from the clock
  alone; a replay that must keep its writer's stamp goes through
  `save_raw_replayed`. `tests/entity_clock.rs` steps the clock back for each case.
- **A register that is an `UnorderedMap` entry's whole value is stored without its
  stamp.** The entry's `updated_at` is its stamp (`lww_register::entry_stamp`): the map
  names its value type on its `Collection` (`stamp_values_of`), the entry offers it to
  the register that starts its value, and `find_by_id` names the row's `updated_at`
  for the decode. Every path already resolved such an entry by `updated_at`, never by
  the register's HLC, so that was 16 dead bytes per entry. Registers anywhere else keep
  their stamp: a state field or a field of a stored value is merged by it, and the other
  collections (`Vector`, `SortedMap`, sets) were left as they were. Code that decodes map
  entry bytes outside the map must do as `tests::common::map_entry_bytes` does, or the
  decode fails.
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
- **A `TeeOnly` field's subtree lives at TEE-only ids, and merge reserves them.** A
  `TeeOnly` cell stores nothing until the TEE's first write, so its id, its value entry
  and every id beneath it are free until then, and an entity planted at any of them would
  refuse the TEE's writes for good. `tee_only_id` derives the field's id with a fixed
  prefix (`is_tee_only_id`), `compute_id` and `compute_collection_id` carry that prefix
  to every child, and `refuse_foreign_entity_at_tee_only_id` (in `apply_action` and
  `verify_snapshot_entity_signature`) refuses anything there except `Shared` with exactly
  `TEE_AUTHORITY` as writer, a `SharedMember` anchored to a TEE-only id, or, at a
  collection's id (`is_tee_only_collection_id`: `compute_collection_id` beneath a TEE-only
  parent tags it `\xCAtee\x00col`), the collection's own `Public` entity. Refuse that
  entity and every entry of a collection inside the cell is refused with it, as its
  ancestor: a `Registry<_, _, Tee>`'s verdicts reached no member. The anchor's
  rotation log is derived with `compute_unmarked_id`, because the node writes it, not the
  TEE. Do not derive an id beneath a TEE-only one by any other function, or it escapes
  the rule.
  Apply also checks every ancestor it would create, because a missing ancestor is
  created from the stamp the action claims for it, which nobody signs.
- **A `SharedStorage` cell's ids say what may hold them.** A node keeps the first entity
  it stores at an id (a stamp and a member's anchor never change, a `Shared` write is
  checked against the stored writer set), so a predictable cell id let whoever reached a
  new joiner first split it, or take the cell. So a field-derived wrapper id is
  `cell_id(field_id, writers)`: a tag, a hash of the field, and a hash of the writer set
  it was created with; a first write whose writers hash otherwise is refused, and a
  forged set lands at an id no state refers to. The value is at `cell_value_id(anchor)`,
  tagged and bound to the anchor, and `compute_id`/`compute_collection_id` carry the tag
  and binding to every id beneath it (collections take their own tag, since their entity
  is `Public`). `refuse_foreign_entity_at_cell_id` (in `apply_action`, for the action and
  its missing ancestors, and in both snapshot verifiers) refuses anything else there.
  A snapshot carries today's writer set, so it holds a wrapper only to being `Shared`;
  a first apply of a rotated wrapper (a HashComparison repair on a node that never had
  genesis) is refused until genesis arrives. `TeeOnly` keeps its own ids and rule.
  `tests/shared_occupation.rs` replays each forgery on a group and a joiner.
- **Every `Shared` entity is at a cell id and every `SharedMember` at an id bound to its
  anchor** (`shared_stamp_fits`), TEE-only ids aside, so apply refuses either anywhere
  else and a plain collection's predictable field id cannot be taken by one. A nested
  cell (`WriterSetCell::new`, `new_write_once`, a lazily created `TeeOnly`) gets
  `cell_id(Id::random(), writers)`, and a random entry id (a vector push) is
  `random_entry_id(parent)`, which carries the parent's cell binding or TEE-only tag as
  `compute_id` does. Mint a `Shared`/`SharedMember` id any other way and peers refuse
  it; `tests::common::assert_every_shared_entity_is_bound` walks a store for that. What
  is left at an untagged id is `Public` (the same type, so it merges) and `Frozen`,
  which must sit at `compute_id(parent, key)`, a hash under a different domain
  separator from `compute_collection_id`, so it cannot land on a field id.
- **An owned collection in a cell's value is bound to its owner and its cell at once.**
  An `Authored`/`WriteOnce`/`Moderated` collection, an `AuthoredVector` or a
  `UserStorage` held in a `SharedStorage` value (or anything built on `WriterSetCell`)
  has slots whose first 20 bytes are the cell's tag and anchor binding, the same for
  every slot there. 32 bytes cannot hold the owned tag, the cell's tag and both 96-bit
  bindings, so `owned_entry_id` gives such a slot `cell_owned_entry_id`: the slot's
  last 12 bytes (the key's), `CELL_OWNED_ID_TAG`, then 12 bytes of
  `SHA256(anchor binding ‖ owner)`. A keyed collection's entry there takes
  `CELL_OWNED_KEYED_ID_TAG` instead (`owned_keyed_entry_id`), so it is bound to
  its cell, its owner and its key at once, and `len` stays exact; written-once
  seals and terminal tombstones key on the id, so they hold for it as for any
  other kind. The anchor binding is the parent's, so
  `cell_owned_id_binds(id, parent, owner)` checks both from the id, the stamp and
  the parent the write names; a writer standing at another account's id would need a
  parent meeting a 96-bit hash. Such an entry is admitted only when its signer speaks
  for the owner (the `User` rule) AND the owner holds `WRITE` in the cell's writer set:
  `Interface::refuse_cell_owner_without_write`, in `apply_action`'s `User` arm, as of
  the write's HLC (`resolve_anchor_writers_as_of`, since the node resolves no writer set
  for a `User` action), and on the local path (`add_child_to`, `save_raw`) against the
  current writers. The cell is found from the parent's id: `cell_value_id` is
  `value_id_for_binding(anchor binding)`, so `bound_value_id(parent)` names the value,
  whose `SharedMember` stamp names the anchor (the index tree is flat, so no ancestor
  does). A node without the value refuses and takes the entry when it is re-driven.
  Snapshot leaves are held to both bindings with the parent their record names
  (`verify_snapshot_entity_signature(id, parent, ..)`); the writer question is left
  to the next delta, as it is for a member. A delete follows the entry's own rules.
  Nothing derived beneath such an entry is bound to the cell (the id no longer carries
  the anchor binding), so what an owned entry nests answers to its owner as it would
  outside a cell. `tests/cell_owned.rs` covers each owned type and each forgery,
  owned maps in a cell (convergence, a misfiled key, an id of another kind, `len`,
  a written-once delete);
  `assert_every_owned_entry_is_bound` and `assert_every_shared_entity_is_bound` check
  the layout store-wide.

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
