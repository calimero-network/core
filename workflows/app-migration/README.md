# `workflows/app-migration/` — application migration e2e

Cluster scenarios (`scenarios/*.json`) that upgrade an app on a 3-node
cluster and check what its state reads after: each
upgrade migrates the app's one replicated state at once, planned from the two
releases' embedded ABIs ([Upgrades](../../docs/src/content/docs/protocol/upgrades.mdx)).

They were migrated from v1's workflows of the same names (by merobox's
`cloud port`, then from YAML to JSON by `scripts/workflow-to-scenario.py`).
Each file's `comment` lists the v1 steps that have no counterpart in
this node (gossip waits, node logs, v1 admin-API reads) and any fix made in
the migration, with its reason. The v1 workflows whose subject has no
counterpart (per-member lazy migration, activation, heartbeats, the
upgrade ladder, frontend hosts) were deleted; the
[testing guide](../../docs/src/content/docs/contribute/testing.mdx#workflows-deleted-in-the-migration)
lists each with why.

## Scenarios in this directory

### Regression guards

| File | What it proves |
|---|---|
| `00-single-group-migration-baseline.json` | An app in a team, `v1 → v2` by the organization's upgrade of that team: the run record says finished at 2.0.0 with every app done, and the state reads migrated. |
| `01-namespace-cascade-migration.json` | One cascade upgrade covers the app in the namespace's subgroup; both members read the migrated state. |

### Per-scenario migration matrix (`apps/migrations/migration-suite-v{1..5}` chain)

| File | Scenario | Fixture transition |
|---|---|---|
| `02-scenario-additive-field.json` | **Additive field** — v2 adds `notes: LwwRegister<String>`. Negative pre-upgrade: `set_notes` not exported by v1. | `v1 → v2-add-field` |
| `03-scenario-remove-field.json` | **Remove field (no archive)** — v3 drops `notes`. Negative post-upgrade: `set_notes` / `get_notes` gone. | `v2 → v3-remove-field` |
| `04-scenario-rename-field.json` | **Rename field** — `description` → `details`. Asserts value carries over; old setter / getter names gone in v4. | `v3 → v4-rename-field` |
| `05-scenario-change-type.json` | **Type change** — `counter: LwwRegister<u64>` → `LwwRegister<String>`. Asserts numeric value survives as string; v5's `set_counter(String)` callable; v4's `increment_counter` gone. | `v4 → v5-change-type` |

### Per-scenario migration matrix (standalone `apps/migrations/scenario-*-v{1,2}` pairs)

| File | Scenario | Migrate? |
|---|---|---|
| `06-scenario-new-method.json` | **New method** — v2 adds `clear_items()`. State byte-identical, no migrate needed. Negative pre-upgrade: `clear_items` not exported by v1. | No |
| `07-scenario-new-enum-variant.json` | **New enum variant** — v2 appends `Archived` to `Status` enum. Borsh indices preserved, no migrate needed. Asserts pre-existing `Paused` value survives byte-for-byte; new variant becomes settable. | No |
| `08-scenario-pure-bugfix.json` | **Pure bugfix** — v2 has byte-identical state, internal logic-only fix (v1 `sum_all_values` has off-by-one, v2 doesn't). Asserts state preserved + observable behavior change. | No |
| `09-scenario-crdt-native.json` | **CRDT-native field growth** — v2 adds `tags: Vector<LwwRegister<String>>`. Migrate **seeds the Vector** from the sorted v1 item keys, exercising cross-node determinism for a Vector populated *inside* a migrate (elements re-keyed by append index + `LwwRegister` metadata zeroed via merge mode). Discriminating check: `tag_count` dedups to 5 across nodes after a post-migrate sync — divergent element ids would inflate the union. | Yes |
| `10-scenario-struct-to-enum.json` | **Struct → enum** — v1 `Status { active: bool, reason: Option<String> }` → v2 `enum Status { Active, Inactive(String) }`. Migrate eliminates the impossible state (`active=true + reason=Some`). | Yes |
| `11-scenario-field-split.json` | **Field split** — v1 `address: String` → v2 `{ street, city, postcode }`. Migrate parses comma-separated v1 address; fallback assigns the whole string to `street`. | Yes |
| `12-scenario-field-remove-archive.json` | **Remove with archive** — v2 drops `legacy_note` but stashes the value in `archived_legacy: UnorderedMap<String, String>` under key `"latest"`. Companion to `03` (which discards). | Yes |
| `13-scenario-invariant-reshuffle.json` | **Invariant reshuffle** — v1 has denormalized `global_count` + `per_item_counts` (invariant easy to violate via two independent setters). v2 funnels both updates through a single `record()` method; migrate re-derives `total` from the per-item map (does NOT trust v1's `global_count`). | Yes |

### Collection-coverage + engine-feature scenarios (14–44)

| File | What it covers |
| --- | --- |
| `14-cascade-status-rpc.json` | A cascade upgrade's run record (`GET /orgs/{org}/upgrades/{run}`), read by both members: finished, nothing failed, every app done. |
| `15-scenario-authored-map.json` | `AuthoredMap` carry-through across a migrate (per-entry owner stamps survive). |
| `16-scenario-user-storage.json` | `UserStorage` carry-through. |
| `17-scenario-frozen-storage.json` | `FrozenStorage` real content-rewrite during migrate. |
| `18-scenario-shared-storage.json` | `SharedStorage` (writer-set-guarded) carry-through. |
| `19-scenario-authored-vector.json` | `AuthoredVector` carry-through. |
| `20-scenario-unordered-set.json` | `UnorderedSet` built during migrate (deterministic re-key). |
| `21-reads-available-during-upgrade.json` | Reads served from the current root while an upgrade is in progress (#2539 step 1). |
| `22-scenario-identity-downgrade.json` | Negative path: the L1 identity-downgrade gate REJECTS an `AuthoredMap → UnorderedMap` upgrade (1-node). |
| `29-migration-check-pass.json` | `migration_check` passes → migration commits. |
| `30-migration-check-fail-abort.json` | `migration_check` fails → zero-residue logical abort, v1 still served. |
| `31-admin-abort-logical.json` | Admin logical abort drops the pending marker. |
| `32-authored-migrate-ux.json` | One-tap `migrate_my_entries`. |
| `33-authored-remaining-count.json` | `count_my_pending` drives the `authored_remaining` rollup. |
| `35-two-namespaces-coexist.json` | Two namespaces coexist on different versions on one node. |
| `38-namespace-invite-join-regression.json` | Namespace invite + join regression guard for the `MemberJoinedAt` invitation-expiry wire change (2-node). |
| `39-missing-abi-refused.json` | Negative path: an upgrade whose target build has no embedded ABI is REFUSED (missing migration evidence) rather than silently proceeding code-only (1-node, core#3285). |
| `40-paired-device-holds-newer-release.json` | A second passkey of the account (v1: a paired device) runs the release the app was deployed on, not a newer one installed. |
| `41-reader-sees-post-upgrade-writes.json` | A member whose first call after a cascade is a read sees what was written on the new release: the switch replays the deltas the node held back before serving the read (2-node). |
| `44-context-reports-the-release-it-runs.json` | An app reports the release it runs, not the latest one installed. |

## Fixtures

Two fixture families live under [`apps/migrations/`](../../apps/migrations):

1. **Chain fixtures** — `migration-suite-v{1..5}`. Each `vN` migrates from
   `vN-1`. Used by scenarios `00`–`05`.
2. **Per-scenario fixture pairs** — `scenario-{name}-v{1,2}`. Each pair is
   self-contained (v2 migrates from v1, no chain) and exercises exactly one
   schema-shape transition. Used by scenarios `06`–`33`.

Every fixture has a `#[app::state]` struct, an optional `#[app::migrate]`
function (omitted when the scenario doesn't need migration — borsh
backwards-compatible cases), and a `schema_info()` view returning the
current `schema_version` plus the live field shape — the scenarios pipe
this through `json_assert` for post-upgrade verification.

`build-wasms.sh` lists the bundles (each pair or chain shares one `--package`,
so both versions are releases of one package); `scripts/build-workflow-fixtures.py`
builds them as signed `.mpk` releases into `target/workflow-wasm/dist/`, which
the scenarios install by name and upgrade by version.

## Running locally

```bash
python3 scripts/build-workflow-fixtures.py        # optional: the runner builds missing fixtures on first use
cargo test -p calimero-node --test scenarios -- app-migration/          # all of them
cargo test -p calimero-node --test scenarios -- app-migration/00-single-group-migration-baseline
```

## CI

`.github/workflows/e2e.yml` (job `scenarios`, four shards) runs every scenario
here with the rest, on an in-process cluster; see the [testing guide](../../docs/src/content/docs/contribute/testing.mdx).
