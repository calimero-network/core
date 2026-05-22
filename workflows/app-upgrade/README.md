# `workflows/app-upgrade/` — application upgrade & migration e2e

End-to-end merobox workflows that exercise the per-context application
upgrade + state-migration engine introduced in
[#1911](https://github.com/calimero-network/core/pull/1911).

The Rust unit layer is already well-covered — see the 16
`verify_appkey_continuity` tests in
`crates/context/src/handlers/update_application/mod.rs` (signer mismatch,
hijacking, downgrades, case/whitespace edge cases, prefix attacks). These
workflows add the missing **end-to-end** coverage: install a new WASM,
run `upgrade_group` with a `migrate_method`, prove on every peer that the
migration function actually executed and that post-migration state is in
the new schema's shape.

## Fixtures

The workflows reuse the [`apps/migrations/migration-suite-v{1..5}`](../../apps/migrations)
fixtures (also introduced in #1911):

| Suite | Schema change vs prior | Migration function |
|---|---|---|
| `migration-suite-v1` | initial | — |
| `migration-suite-v2-add-field` | + `notes: LwwRegister<String>` | `migrate_v1_to_v2` |
| `migration-suite-v3-remove-field` | − `notes` | `migrate_v2_to_v3` |
| `migration-suite-v4-rename-field` | `description` → `details` | `migrate_v3_to_v4` |
| `migration-suite-v5-change-type` | `counter: u64` → `counter: String` | `migrate_v4_to_v5` |

Each suite exposes a `schema_info()` view returning the current
`schema_version` and the live field shape, which the workflows pipe
through `json_assertion` for post-upgrade verification. Each `#[app::migrate]`
also emits a `Migrated { from_version, to_version }` event, which the
workflows assert on via `assert_log_present`.

## Workflows in this folder

| File | What it proves |
|---|---|
| `01-empty-upgrade-rejected.yml` | Server rejects an `upgrade_group` whose target equals the current application and carries no migration (the bail at `upgrade_group.rs:480`). Asserts both the failed RPC and the canonical error message in logs. |
| `03-single-migration-v1-to-v2.yml` | Install v1, write state, install v2, `upgrade_group` with `migrate_method=migrate_v1_to_v2`. Asserts the `Event::Migrated { v1 -> v2 }` log line, that `schema_info()` reports v2's shape with `notes` populated by the migration, that pre-migration writes survived, and that v2-only setters/getters now work. |
| `20-chain-v1-to-v5.yml` | Sequentially migrates one context through all four migrations (v1 → v2 → v3 → v4 → v5). Writes new data at intermediate versions and asserts after each step that schema_version flipped, field-level changes applied (add/drop/rename/retype), and inherited data survived. Aggregate `Migrated` count must be ≥ 4. |

(Numbering reserves gaps for follow-up workflows. `02-binary-swap-without-migration.yml`
would require a same-schema v1/v2 pair that does not exist in the migration-suite,
and is omitted from this PR — the `update_application_id` no-migration path is
already covered by the unit suite at `update_application/mod.rs`.)

## Building the fixtures

Convenience wrapper:

```bash
bash workflows/app-upgrade/build-wasms.sh
```

Or build individually:

```bash
bash apps/migrations/migration-suite-v1/build.sh
# ...
```

CI builds them inline in `.github/workflows/app-migration-e2e.yml`.

## Running locally

Requires `merobox >= 0.6.16` and Docker. Use the published
`merod:edge` image, or build a local `merod:local` per the pattern in
`workflows/sync-tests/`:

```bash
# Build fixtures
bash workflows/app-upgrade/build-wasms.sh

# Run one workflow against the published edge image
merobox bootstrap run workflows/app-upgrade/03-single-migration-v1-to-v2.yml --verbose

# Or against a locally built merod
merobox bootstrap run workflows/app-upgrade/03-single-migration-v1-to-v2.yml \
    --image merod:local --e2e-mode --verbose
```

## Relationship to other test layers

| Layer | Lives in | Covers |
|---|---|---|
| Unit | `crates/context/src/handlers/update_application/mod.rs` (16 tests) | `verify_appkey_continuity`: signer/hijack/downgrade/encoding |
| Governance convergence | `crates/context/tests/local_group_governance_convergence.rs` | `TargetApplicationSet` + `GroupMigrationSet` DAG-replicate cleanly |
| **This folder** | `workflows/app-upgrade/` | Full pipeline: install → publish governance op → per-context apply → migration runs → state in new shape |
