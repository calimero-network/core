# `workflows/app-migration/` — application migration e2e

End-to-end merobox workflows that exercise the per-context application
migration pipeline introduced in [#1911](https://github.com/calimero-network/core/pull/1911)
and the namespace-cascade additions designed in
`docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md`.

## Workflows in this directory

| File | What it proves |
|---|---|
| `00-single-group-migration-baseline.yml` | Single-node, single-group `v1 → v2` migration via `upgrade_group(cascade=false)`. **Regression guard for [#2433](https://github.com/calimero-network/core/pull/2433)** — the per-context migration write path that #2433 silently broke and PR-1 of the cascade train repairs. |

Later PRs (PR-2, PR-3) add workflows `01`..`06` covering namespace cascade,
HLC fence, multi-version coexistence, etc.

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
