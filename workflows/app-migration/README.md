# `workflows/app-migration/` — application migration e2e

End-to-end merobox workflows that exercise the per-context application
migration pipeline introduced in [#1911](https://github.com/calimero-network/core/pull/1911)
and the namespace-cascade additions designed in
`docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md`.

## Workflows in this directory

| File | What it proves |
|---|---|
| File | What it proves |
|---|---|
| `00-single-group-migration-baseline.yml` | Single-node, single-group `v1 → v2` migration via `upgrade_group(cascade=false)`. **Regression guard for [#2433](https://github.com/calimero-network/core/pull/2433)** — the per-context migration write path that #2433 silently broke and PR-1 of the cascade train repairs. |
| `01-namespace-cascade-migration.yml` | Single-node, namespace + one subgroup + one context, ONE `upgrade_group(cascade=true)` call. **Regression guard for the random-`app_key` bug** at namespace/subgroup creation. |
| `02-scenario-additive-field.yml` | **Additive field** (`v1 → v2-add-field`): adds `notes: LwwRegister<String>`. Negative pre-upgrade assertion: v2-only `set_notes` is not exported by v1; post-upgrade it is. |
| `03-scenario-remove-field.yml` | **Remove field** (`v2 → v3-remove-field`): drops `notes`. Negative post-upgrade assertion: `set_notes` / `get_notes` no longer exist on v3. |
| `04-scenario-rename-field.yml` | **Rename field** (`v3 → v4-rename-field`): `description` → `details`. Asserts value carries over and old names (`set_description` / `get_description`) are gone in v4. |
| `05-scenario-change-type.yml` | **Type change** (`v4 → v5-change-type`): `counter: LwwRegister<u64>` → `LwwRegister<String>`. Asserts the numeric value survives as a string, v5's `set_counter(String)` is callable, and v4's `increment_counter` is gone. |

(The above two tables collapse into one in the rendered docs; the split
keeps the namespace-cascade row visually separate from the per-context
schema-shape rows.)

Later commits in this PR add per-scenario fixtures + workflows for the
remaining matrix rows: `new-method`, `new-enum-variant`, `pure-bugfix`,
`crdt-native` (Vector growth), `struct-to-enum`, `field-split`,
`field-remove-archive` (with archive), `invariant-reshuffle`.

## Fixtures

These workflows use the [`apps/migrations/migration-suite-v{1..5}`](../../apps/migrations)
fixtures (introduced in #1911). Each suite has a `#[app::state]` struct, a
`#[app::migrate]` function from the previous version, and a `schema_info()`
view returning the current `schema_version` plus the live field shape — the
workflows pipe this through `json_assert` for post-upgrade verification.

PR-1 only depends on v1 and v2-add-field; later PRs add the v3/v4/v5
fixtures to the build helper.

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
