# merodb Migration Pipeline – Implementation Plan

This plan captures the step-by-step work required to extend `merodb` with a YAML-driven migration pipeline that can copy data between RocksDB instances. Update each step with progress notes (✅/🚧/🛑), links to PRs, and relevant follow-up tasks as work advances.

## Step-by-step Plan

- [x] **Restructure CLI**
  - [x] Convert current flag-based CLI to a `clap::Subcommand` layout (`schema`, `export`, `validate`, `export-dag`, `migrate`).
  - [x] Preserve existing behaviours/help text while adding a `migrate` subcommand stub.

- [x] **Define YAML Plan Schema**
  - [x] Create strongly typed `serde` structs/enums for plan metadata, defaults, and step variants (`copy`, `delete`, `upsert`, `verify`).
  - [x] Add versioning and forward-compatibility guards; emit descriptive errors for malformed documents.

- [x] **Implement Plan Loader**
  - [x] Wire `--plan <FILE>` to deserialize the YAML plan and print a summary (steps, columns, filters).
  - [x] Integrate plan parsing into the `migrate` command, failing fast on validation errors.

- [ ] **Set Up Migration Context**
  - [ ] Open the source RocksDB in read-only mode and lazily load the ABI manifest when `--wasm-file` is supplied.
  - [ ] Add `--target-db` support; keep the target read-only until mutating mode is enabled.

- [ ] **Build Dry-run Engine**
  - [ ] Resolve high-level filters (context IDs, alias names, key prefixes) to RocksDB iterators and collect a structured action preview.
  - [ ] Output dry-run results to stdout/JSON with per-step key counts and examples; guarantee no writes occur.

- [ ] **Develop Test Fixtures & Dry-run Tests**
  - [ ] Introduce utilities for creating temporary RocksDB instances populated with sample Calimero data.
  - [ ] Write unit/integration tests covering plan parsing, filter resolution, and dry-run summaries.

- [ ] **Enable Mutating Execution**
  - [ ] Allow opening the target database with write access when `--apply` (or `--dry-run=false`) is specified.
  - [ ] Execute steps via RocksDB `WriteBatch`, ensuring idempotency and detailed logging.

- [ ] **Add Safety Mechanisms**
  - [ ] Support optional backups (`--backup-dir`), step guards (`requires_validation`, `requires_empty_target`), and configurable batch sizes.
  - [ ] Reuse existing validation logic to re-check the target database when requested.

- [ ] **Implement Verification Steps**
  - [ ] Evaluate assertions (counts, presence/absence) in `verify` steps and abort on failure.
  - [ ] Integrate summary reporting for verification outcomes.

- [ ] **Polish CLI UX**
  - [ ] Refine command output, add `--report <FILE>` for machine-readable run logs, and document exit codes.
  - [ ] Update `README.md` with migration examples, YAML reference, and troubleshooting guidance.

- [ ] **Finalize Testing & CI**
  - [ ] Expand tests to cover apply-mode mutations, rollback scenarios, and CLI smoke tests.
  - [ ] Ensure the migration suite runs in CI; gate heavier scenarios behind feature flags if necessary.

## Related Work

- Testing strategy lives in `tools/merodb/migration_testing.md`.
- Sample plans and fixtures should be added under `tools/merodb/examples/` once available.

---

_Last updated: TODO (replace with date when you next edit)._
