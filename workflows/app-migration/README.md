# `workflows/app-migration/` — application migration e2e

End-to-end merobox workflows that exercise the per-context application
migration pipeline introduced in [#1911](https://github.com/calimero-network/core/pull/1911)
and the namespace-cascade additions designed in
`docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md`.

## Workflows in this directory

### Regression guards

| File | What it proves |
|---|---|
| `00-single-group-migration-baseline.yml` | 2-node, single subgroup + one context, `v1 → v2` via `upgrade_group(cascade=false)`. **Regression guard for [#2433](https://github.com/calimero-network/core/pull/2433)** — the per-context migration write path that #2433 silently broke and PR-1 of the cascade train repairs. |
| `01-namespace-cascade-migration.yml` | 2-node, namespace + one Open subgroup + one context, ONE `upgrade_group(cascade=true)` call. **Regression guard for the full `app_key` fix triangle** (originator derivation, remote-peer `GroupCreated` inheritance, joiner-side bootstrap) AND cross-node cascade convergence: asserts node 2's `GroupMeta` flips on both layers, the receiver-side `CascadeTargetApplicationSet: applied` log fires, and node 2 self-migrates via the lazy path. |

### Per-scenario migration matrix (`apps/migrations/migration-suite-v{1..5}` chain)

| File | Scenario | Fixture transition |
|---|---|---|
| `02-scenario-additive-field.yml` | **Additive field** — v2 adds `notes: LwwRegister<String>`. Negative pre-upgrade: `set_notes` not exported by v1. | `v1 → v2-add-field` |
| `03-scenario-remove-field.yml` | **Remove field (no archive)** — v3 drops `notes`. Negative post-upgrade: `set_notes` / `get_notes` gone. | `v2 → v3-remove-field` |
| `04-scenario-rename-field.yml` | **Rename field** — `description` → `details`. Asserts value carries over; old setter / getter names gone in v4. | `v3 → v4-rename-field` |
| `05-scenario-change-type.yml` | **Type change** — `counter: LwwRegister<u64>` → `LwwRegister<String>`. Asserts numeric value survives as string; v5's `set_counter(String)` callable; v4's `increment_counter` gone. | `v4 → v5-change-type` |

### Per-scenario migration matrix (standalone `apps/migrations/scenario-*-v{1,2}` pairs)

| File | Scenario | Migrate? |
|---|---|---|
| `06-scenario-new-method.yml` | **New method** — v2 adds `clear_items()`. State byte-identical, no migrate needed. Negative pre-upgrade: `clear_items` not exported by v1. | No |
| `07-scenario-new-enum-variant.yml` | **New enum variant** — v2 appends `Archived` to `Status` enum. Borsh indices preserved, no migrate needed. Asserts pre-existing `Paused` value survives byte-for-byte; new variant becomes settable. | No |
| `08-scenario-pure-bugfix.yml` | **Pure bugfix** — v2 has byte-identical state, internal logic-only fix (v1 `sum_all_values` has off-by-one, v2 doesn't). Asserts state preserved + observable behavior change. | No |
| `09-scenario-crdt-native.yml` | **CRDT-native field growth** — v2 adds `tags: Vector<LwwRegister<String>>`. Migrate **seeds the Vector** from the sorted v1 item keys, exercising cross-node determinism for a Vector populated *inside* a migrate (elements re-keyed by append index + `LwwRegister` metadata zeroed via merge mode). Discriminating check: `tag_count` dedups to 5 across nodes after a post-migrate sync — divergent element ids would inflate the union. | Yes |
| `10-scenario-struct-to-enum.yml` | **Struct → enum** — v1 `Status { active: bool, reason: Option<String> }` → v2 `enum Status { Active, Inactive(String) }`. Migrate eliminates the impossible state (`active=true + reason=Some`). | Yes |
| `11-scenario-field-split.yml` | **Field split** — v1 `address: String` → v2 `{ street, city, postcode }`. Migrate parses comma-separated v1 address; fallback assigns the whole string to `street`. | Yes |
| `12-scenario-field-remove-archive.yml` | **Remove with archive** — v2 drops `legacy_note` but stashes the value in `archived_legacy: UnorderedMap<String, String>` under key `"latest"`. Companion to `03` (which discards). | Yes |
| `13-scenario-invariant-reshuffle.yml` | **Invariant reshuffle** — v1 has denormalized `global_count` + `per_item_counts` (invariant easy to violate via two independent setters). v2 funnels both updates through a single `record()` method; migrate re-derives `total` from the per-item map (does NOT trust v1's `global_count`). | Yes |

### Out of scope (not in this PR)

* `serde-default-field` — borsh-backed state ignores `#[serde(default)]`, so this scenario from the original matrix doesn't have a meaningful borsh-level shape. Could be added later as an ABI-response scenario, not a state-migration one.
* `Coordinated` multi-node upgrade policy — all scenarios use `lazy_on_access` (see below). Eager all-node `Coordinated` migration has no receiver-side migration trigger today and is a separate feature.

## Cross-node migration model (why `lazy_on_access`)

Every scenario sets `upgrade_policy: lazy_on_access` and relies on each
node migrating its **own** state independently. This is deliberate, not
a workaround:

* Migration is a **full root-state replacement**, not a CRDT-mergeable
  delta. The migrate fn produces fully-resolved v2-shaped state, so
  `write_migration_state` writes it via the pre-merged primitive and
  **emits no DAG delta** (`clear_pending_delta`). The migrated bytes are
  therefore *not* propagated over sync — a peer cannot receive another
  node's migrated state, because merging a v1 root entry with a v2 root
  entry at the shared fixed `ROOT_ENTRY_ID` would corrupt it.
* Under `LazyOnAccess` (the SDK default — *"upgrade each context
  transparently on its next execution"*) each node re-derives v2 by
  running the migrate fn on its **own already-synced, byte-identical v1
  state** on the first context access after the upgrade op gossips in.
  Determinism guarantees every node lands on the same v2 root.
* The upgrade op (`TargetApplicationSet` + `GroupMigrationSet`, or their
  cascade equivalents) sets both `target_application_id` and the
  `migration` method on the group's `GroupMeta`; each receiver's
  `maybe_lazy_upgrade` reads those to self-migrate.

**Observability:** the lazy path logs `performing lazy upgrade before
execution`, then `Executing migration` / `Migrated state written
successfully`. Migrating scenarios assert these on the receiver node
via `assert_log_present`, proving it self-migrated rather than silently
diverging. No-migration scenarios (`06`, `07`, `08`) have byte-identical
borsh layouts, so the lazy upgrade only swaps the application pointer
(no migrate fn, no migration log) — verified via cross-node `schema_info`
reads instead.

**Application distribution (admin-only install).** Every scenario installs
both the from- and to-version bytecode on the **admin node only**. Node 2
never runs `install_application`; it auto-fetches the from-version when it
joins the context and the to-version when the upgrade announces the target
blob, pulling both over the `BlobShare` sync protocol (the sync-gate leaves
`BlobShare` open during a pending upgrade for exactly this). App ids are
content-addressed (`blob_id` of the bytecode), so node 1's `app_v1`/`app_v2`
ids are the same ids node 2 resolves. This mirrors a real deployment —
operators upgrade from one node — and exercises the bytecode-propagation
path end-to-end rather than pre-seeding it.

## Fixtures

Two fixture families live under [`apps/migrations/`](../../apps/migrations):

1. **Chain fixtures** — `migration-suite-v{1..5}`. Each `vN` migrates from
   `vN-1`. Used by workflows `00`, `02`, `03`, `04`, `05` and the cascade
   workflow `01`.
2. **Per-scenario fixture pairs** — `scenario-{name}-v{1,2}`. Each pair is
   self-contained (v2 migrates from v1, no chain) and exercises exactly one
   schema-shape transition. Used by workflows `06`..`13`.

Every fixture has a `#[app::state]` struct, an optional `#[app::migrate]`
function (omitted when the scenario doesn't need migration — borsh
backwards-compatible cases), and a `schema_info()` view returning the
current `schema_version` plus the live field shape — the workflows pipe
this through `json_assert` for post-upgrade verification.

## Building the fixtures

```bash
bash workflows/app-migration/build-wasms.sh
```

Or build a single suite:

```bash
bash apps/migrations/migration-suite-v1/build.sh
```

## Running locally

Requires `merobox >= 0.6.16` and Docker. Use the published `merod:edge`
image, or build a local `merod:local`:

```bash
# Build fixtures
bash workflows/app-migration/build-wasms.sh

# Run against edge
merobox bootstrap run workflows/app-migration/00-single-group-migration-baseline.yml --verbose

# Or against locally built merod
merobox bootstrap run workflows/app-migration/00-single-group-migration-baseline.yml \
    --image merod:local --e2e-mode --verbose
```

## CI

`.github/workflows/app-migration-e2e.yml` runs every workflow in this
directory on every PR touching migration-related paths. Per-node docker
logs are uploaded as artefacts for triage.
