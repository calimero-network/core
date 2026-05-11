# Generic metadata records for namespace / group / member / context

**Status:** Design approved — implementation pending.
**Date:** 2026-05-11
**Scope:** Replace the group-scoped *alias* (a bare id→string label) with a generic, app-extensible **`MetadataRecord`** carried by every group, group member, and group-registered context (a namespace, being a root group, is covered by the group case). Drop the alias store keys, the alias `GroupOp` variants, and the alias-specific HTTP / CLI surface entirely. Add a `CAN_MANAGE_METADATA` capability bit. **Per-namespace name uniqueness is explicitly out of scope** for this PR and recorded here as a planned follow-up.

This is PR 4 of the Slack-app namespace stack (after #2324, #2325, #2326).

---

## 1. Motivation

Today a group / member / context can carry a single human-readable *alias* string:

- store keys `GroupAlias(group_id)`, `GroupMemberAlias(group_id, member)`, `GroupContextAlias(group_id, context_id)` → `String`;
- `GroupOp::GroupAliasSet { alias }`, `MemberAliasSet { member, alias }`, `ContextAliasSet { context_id, alias }`;
- accessors in `group_store::aliases` (`get/set_group_alias`, …), surfaced via `NamespaceSummary.alias`, `enumerate_member_aliases`, `enumerate_group_contexts_with_aliases`, the `store_*_alias` admin handlers/routes, and meroctl flags.

Limitations driving this change:

1. **One opaque label, nothing else.** A Slack-style workspace/channel needs a display name *and* a bag of properties (topic, icon URL, color, archived flag, …). Apps currently have nowhere to put that inside core's governance state, so they either smuggle it into context CRDT state or maintain a side channel.
2. **"Alias" overloads a name that already means something else.** `calimero_primitives::alias::Alias<T>` is a *separate*, node-local resolver (`meroctl context create --as <name>` → `/alias/context/:name` lookup). Having a second, group-scoped "alias" that is really just "the entity's name" is confusing. Folding it into `MetadataRecord.name` removes the overload.
3. **No provenance.** There's no record of who last touched a label or when. `MetadataRecord` adds `updated_by` / `updated_at`.

> **Not in this PR (and not affected by it):** the node-local `calimero_primitives::alias::Alias<T>` system (`/alias/context/:name`, `/alias/application/:name`, `/alias/identity/:scope/:name`, `meroctl … --as …`). That is a different feature with a different storage model and is untouched.

---

## 2. Data model

```rust
/// App-extensible metadata for a group, a group member, or a context
/// registered in a group. A namespace is a root group, so the group
/// variant covers it.
///
/// `data` is opaque to core — core never reads or interprets any key in
/// it. (A future "name uniqueness" policy, see §7, will live in a typed
/// field or a separate op, never inside `data`.)
pub struct MetadataRecord {
    /// The entity's human-readable name (the field formerly called
    /// `alias`). `None` means "no name set".
    pub name: Option<String>,
    /// Arbitrary application-defined properties. Core stores and
    /// replicates this verbatim and never inspects it.
    pub data: BTreeMap<String, String>,
    /// Wall-clock millis when the most recent `*MetadataSet` op was
    /// *applied locally*. Informational only — see §2.1.
    pub updated_at: u64,
    /// Public key of the signer of the most recent `*MetadataSet` op.
    pub updated_by: PublicKey,
}
```

Three store keys, replacing the three alias keys one-for-one (identical key shape, so the rocksdb layout change is "rename the prefix, change the value type"):

| New key | Replaces | Value |
|---|---|---|
| `GroupMetadata(group_id)` | `GroupAlias(group_id)` | `MetadataRecord` |
| `GroupMemberMetadata(group_id, member)` | `GroupMemberAlias(group_id, member)` | `MetadataRecord` |
| `GroupContextMetadata(group_id, context_id)` | `GroupContextAlias(group_id, context_id)` | `MetadataRecord` |

`MetadataRecord` lives next to `MemberCapabilities` (i.e. in `calimero-context-config`, or `calimero-context/primitives` if borsh-derive layering requires) so both the op definitions and the store can reference it.

### 2.1 Determinism of `updated_at` / `updated_by`, and the state hash

- `updated_by` is the signer of the `SignedGroupOp` carrying the `*MetadataSet` — **deterministic** across all peers that apply it.
- `updated_at` is the **applier's** wall-clock at apply time, so peers can disagree by a few ms. That is acceptable because **metadata is deliberately excluded from `compute_group_state_hash`** — exactly as aliases are today (`compute_group_state_hash` hashes only group meta + member set + roles). Metadata is replicated governance state but not consensus-critical state, so a per-peer `updated_at` skew cannot fork the DAG. The doc-comment on `compute_group_state_hash` will be updated to say "metadata records (`name`/`data`/`updated_at`/`updated_by`) are intentionally excluded — like the former alias rows — so the hash stays a function of consensus-relevant state only."

### 2.2 No migration of existing alias rows

There is **no migration step**. On the version that ships this PR, any pre-existing `GroupAlias` / `GroupMemberAlias` / `GroupContextAlias` rows become unreferenced dead keys (harmless; rocksdb won't read them; they can be swept by a future maintenance task). Names are display labels, not load-bearing identifiers, so abandoning them is acceptable for a pre-1.0 system; operators re-set names via `meroctl group metadata` if they care. (Decided: the cost/complexity of a one-time eager migration is not worth it here.) The PR description calls this out as a behavior change.

---

## 3. Wire protocol & application

### 3.1 New `GroupOp` variants

```rust
GroupOp::GroupMetadataSet {
    name: Option<String>,
    data: BTreeMap<String, String>,
},
GroupOp::MemberMetadataSet {
    member: PublicKey,
    name: Option<String>,
    data: BTreeMap<String, String>,
},
GroupOp::ContextMetadataSet {
    context_id: ContextId,
    name: Option<String>,
    data: BTreeMap<String, String>,
},
```

Semantics: each op **wholly replaces** the target `MetadataRecord` — the applier writes `MetadataRecord { name, data, updated_at: now_ms(), updated_by: signer }`. To preserve existing `data` while changing only the name (or vice versa), the caller reads the current record and re-submits the merged map. (Rationale: simplest, no partial-update merge semantics to get wrong; the records are small.) `op_kind_label` gains `"group_metadata_set"` / `"member_metadata_set"` / `"context_metadata_set"`.

### 3.2 Removed `GroupOp` variants

`GroupOp::GroupAliasSet`, `MemberAliasSet`, `ContextAliasSet` are **deleted** — along with their `op_kind_label` arms, their apply arms in `group_store::apply_group_op_mutations`, and the `store_group_alias` / `store_member_alias` / `store_context_alias` request types, `ContextManager` handlers, HTTP routes, `calimero-client` methods, and meroctl flags. An old client that emits one of these ops fails at op deserialization (unknown variant) — an accepted wire break for a pre-1.0 protocol, noted in the PR.

### 3.3 Authorization

Reuses the existing `PermissionChecker` pattern (admin OR capability, with the inherited-admin walk):

| Op | Allowed signer |
|---|---|
| `GroupMetadataSet` | group admin **or** holder of `CAN_MANAGE_METADATA` for the group |
| `ContextMetadataSet` | group admin **or** holder of `CAN_MANAGE_METADATA` for the group |
| `MemberMetadataSet` | group admin **or** holder of `CAN_MANAGE_METADATA` **or** `signer == member` (preserves today's "a member may set their own label" affordance — cf. the `apply_local_member_alias_member_signer_or_admin` test) |

### 3.4 New capability bit

```rust
// MemberCapabilities, after CAN_MANAGE_VISIBILITY (= 1 << 7)
/// Set the name / `data` of the group, its members, or its contexts
/// (the `*MetadataSet` ops). Group admins always have this implicitly;
/// a member may always set *their own* member metadata regardless.
pub const CAN_MANAGE_METADATA: u32 = 1 << 8;
```
Plus `PermissionChecker::require_can_manage_metadata(&self, identity)` mirroring `require_can_manage_visibility` etc., a `meroctl group set-caps --can-manage-metadata` flag, and the `CheckAccess` output bit. The architecture docs' "capability bits" tables (`concepts.html`, `membership-and-leave.html`, `glossary.html`) gain the 9th bit.

---

## 4. `group_store` API

`group_store::aliases` (renamed to `group_store::metadata`) replaces its alias functions:

| Removed | Added |
|---|---|
| `get_group_alias` / `set_group_alias` / `delete_group_alias` | `get_group_metadata` / `set_group_metadata` / `delete_group_metadata` |
| `get_member_alias` / `set_member_alias` | `get_member_metadata` / `set_member_metadata` / `delete_member_metadata` |
| `get_context_alias` / `set_context_alias` | `get_context_metadata` / `set_context_metadata` |
| `enumerate_member_aliases` | `enumerate_member_metadata` |
| `enumerate_group_contexts_with_aliases` | `enumerate_group_contexts_with_names` (returns `Vec<(ContextId, Option<String>)>` — the `name` field) |
| `delete_all_member_aliases` | `delete_all_member_metadata` |

`set_*` write a full `MetadataRecord` (the apply arms call these). `build_namespace_summary` reads `get_group_metadata(...)?.and_then(|r| r.name)`. `delete_group_local_rows` deletes the `*Metadata` keys instead of the `*Alias` keys.

`mod.rs` re-exports update accordingly.

---

## 5. HTTP / client / CLI surface

- **HTTP (`calimero-server` admin):** replace `POST .../alias` group/member/context routes with `POST .../metadata` (request body `{ name: Option<String>, data: Map<String,String> }`); `GET` group / member / context info responses carry the full `MetadataRecord`. Remove the alias routes.
- **`calimero-client`:** `set_group_metadata` / `set_member_metadata` / `set_context_metadata` and `get_*` methods replacing the alias methods.
- **meroctl — new `group metadata` subcommand tree** (dedicated, not under `group settings`):
  - `meroctl group metadata get <group_id>` — print the group's record.
  - `meroctl group metadata set <group_id> [--name <s> | --clear-name] [--set k=v ...] [--unset k ...] [--replace-data]` — read-modify-write (without `--replace-data`, `--set`/`--unset` patch the current `data`; with it, `--set` pairs become the whole map).
  - `meroctl group member metadata get|set <group_id> <member_pk> ...`
  - `meroctl group context metadata get|set <group_id> <context_id> ...`
  - Remove the old `--alias` flags on `group create` / `group settings` / etc. and any `group ... alias` subcommand.
- **`NamespaceSummary`:** field `alias: Option<String>` → `name: Option<String>` (no compat alias — full rename).
- **`broadcast_group_aliases`** handler / `BroadcastGroupAliasesRequest`: this is currently a near-empty stub (`enumerate … ; reply(Ok(()))`). Rename to `broadcast_group_metadata` if it has any live callers; otherwise remove it. (Determined during implementation; pick the smaller diff.)
- The `meroctl group get` / `group members` / `group contexts` output renders `name` (and, optionally, a `data` summary).

---

## 6. Testing

**Unit (`crates/context/src/group_store/tests.rs`):**
- `*MetadataSet` apply: set `name` + `data`; clear `name` (`None`); replace `data`; idempotent re-apply; `updated_by` == signer; record absent → `get_*_metadata` returns `None`.
- Authorization: admin passes all three; bare member rejected for `GroupMetadataSet` / `ContextMetadataSet`; bare member *passes* `MemberMetadataSet` for **their own** member, rejected for another member; granting `CAN_MANAGE_METADATA` flips group/context metadata to allowed.
- `op_kind_label` arms exist for the three new variants; `compute_group_state_hash` is unchanged by a `*MetadataSet` op (regression pin for the "not in the state hash" invariant).
- `delete_group_local_rows` / `delete_all_member_metadata` remove the records.

**e2e (`apps/scaffolding-e2e/workflows/group-metadata.yml`, 2 nodes, + matrix entry in `.github/workflows/e2e-rust-apps.yml`):**
1. node-1 installs the app, creates a namespace, a subgroup, and a context in it.
2. node-1 sets metadata on the namespace, the subgroup, and the context (a `name` + a couple of `data` entries each).
3. node-2 joins the namespace; after sync, node-2 reads each record back and asserts `name` + `data` match.
4. node-2 (no caps) is rejected setting the subgroup's metadata (`expected_failure`); node-1 grants `CAN_MANAGE_METADATA`; node-2 now succeeds and node-1 reads the update.
5. node-2 sets its **own** member metadata in the namespace without any cap — succeeds.

---

## 7. Out of scope — planned follow-ups

### 7.1 Opt-in per-namespace name uniqueness (next PR)

A per-namespace switch that, when on, makes `name` unique **per parent, per kind** (subgroup names unique among siblings under the same parent; member handles unique among a group's members; context names unique among a group's contexts) — like filenames in a directory. The enforcement runs on the `*MetadataSet` apply path and must be **deterministic across exactly the peers that apply the op**, so it can only consult universally-shared state:

- `GroupMetadataSet` on subgroup `G`: if `G` is `Restricted` → skip (its name is private, no peer-wide policy can see it; Open↔Restricted name collisions are allowed). If `G` is `Open` and the flag is on → `for S in list_child_groups(parent(G)) where S != G && visibility(S) == Open: bail if metadata(S).name == G.name`. (All appliers are namespace members ⇒ identical view of `{Open children of parent}` ⇒ identical decision.)
- `MemberMetadataSet` on group `G`: `for M2 in members(G), M2 != M: bail if member_metadata(G, M2).name == name`. (All appliers hold `G`'s key ⇒ see `G`'s member set + metadata.)
- `ContextMetadataSet` on group `G`: `for C2 in contexts(G), C2 != C: bail if context_metadata(G, C2).name == name`. (Same key tier; works for Open *and* Restricted `G`.)

Equality is exact bytes. `name: None` and `name: Some(current)` never collide. Turning the flag *on* is always allowed and only constrains *future* sets (pre-existing dups, including invisible Restricted ones, are grandfathered until a rename). Open question for that PR: whether the flag lives as a typed field on the namespace's `MetadataRecord` or as a typed `RootOp::PolicyUpdated` payload (the latter keeps `MetadataRecord` pure; `PolicyUpdated` already exists and is cleartext on the namespace DAG — readable by all, which the check needs).

### 7.2 Reserved/structured `data` keys, richer value types

`data` stays `BTreeMap<String,String>`, fully opaque to core. A future PR could carve out a `mero.*` reserved-prefix namespace for core-recognized keys, or move to a typed value enum — not now.

---

## 8. Implementation phasing (suggested)

Can ship as one PR, or split if review prefers:
1. `MetadataRecord` type + store keys + `group_store::metadata` module + the migration-less removal of the alias keys; `CAN_MANAGE_METADATA` + checker.
2. The three `GroupOp::*MetadataSet` variants + apply arms + authorization; remove the `*AliasSet` variants.
3. HTTP routes + `calimero-client` methods + meroctl `group metadata` subcommands + `NamespaceSummary.name`; remove alias routes/flags; `broadcast_group_aliases` fate.
4. Unit tests + the `group-metadata.yml` e2e + matrix entry; architecture-doc capability-table updates; rc version bump.
