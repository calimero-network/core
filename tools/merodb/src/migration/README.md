# Merodb Migration Pipeline – Detailed Reference

This document explains the structure, runtime behaviour, and roadmap of the `merodb migrate` workflow. It is intended for engineers authoring migration plans or extending the implementation.

---

## 1. High-Level Flow

```
┌──────────────┐   load_plan   ┌────────────────────┐   DryRunReport   ┌───────────────┐
│  plan.yaml   │ ─────────────►│ MigrationPlan      │ ────────────────►│ CLI Rendering │
└──────────────┘                │  + validation      │                  └───────────────┘
                                └────────┬──────────┘
                                         │ build context
                                         ▼
                                ┌────────────────────┐
                                │ MigrationContext   │
                                │  (source + target) │
                                └────────┬──────────┘
                                         │ generate_report
                                         ▼
                                ┌────────────────────┐
                                │ Dry-run Engine     │
                                │  (scans + verify)  │
                                └────────────────────┘
```

1. **Plan loading** – `migration::loader::load_plan` parses the YAML, producing `MigrationPlan`. Validation rejects unsupported versions, bad filters, empty step lists, etc.
2. **Context construction** – `migration::context::MigrationContext::new` applies CLI overrides, opens the RocksDB handles read-only, and wires the optional ABI manifest.
3. **Dry-run report** – `migration::dry_run::generate_report` walks every plan step, applies filters, inspects the source database, and returns a `DryRunReport`.
4. **CLI output** – `run_migrate` (in `main.rs`) prints metadata, dry-run summaries, warnings, and confirms that dry-run mode is in effect.

Mutating execution (`--apply`) will reuse the same plan/context infrastructure once we implement the corresponding roadmap item.

---

## 2. MigrationPlan Breakdown

```yaml
version: 1
name: optional-name
description: optional description
source:
  db_path: /path/to/source
  wasm_file: /path/to/contract.wasm
target:
  db_path: /path/to/target
  backup_dir: /optional/backup
defaults:
  columns: ["State"]
  decode_with_abi: true
  write_if_missing: false
  batch_size: 1000  # Default batch size for copy/delete steps
  filters:
    context_ids:
      - "0x112233..."
    raw_key_prefix: "abcd"
    key_range:
      start: "00ff"
      end: "0100"
steps:
  - type: copy
    name: copy-state
    column: State
    filters:
      context_ids: ["0x112233..."]
    transform:
      decode_with_abi: true
      jq: ".value.parsed | del(.internal)"
    guards:
      requires_empty_target: true  # Ensure column is empty
      requires_validation: false   # Skip validation checks
    batch_size: 2000  # Override default for this step
  - type: verify
    column: State
    assertion:
      expected_count: 123
    guards:
      requires_validation: true  # Validate before verification
```

### 2.1 Top-Level Fields

| Field        | Description                                                                                                    |
|--------------|----------------------------------------------------------------------------------------------------------------|
| `version`    | Plan schema version. Only `1` is accepted; upgrades must bump this value.                                      |
| `name`/`description` | Optional metadata shown in CLI output to aid humans.                                                |
| `source`     | Required path to the source RocksDB database plus optional WASM file for ABI decoding.                         |
| `target`     | Optional path to the target database and backup directory (used for mutating runs and automatic backups).      |
| `defaults`   | Settings inherited by every step unless overridden (columns, filters, `decode_with_abi`, `write_if_missing`, `batch_size`).   |
| `steps`      | Ordered list of migration actions (`copy`, `delete`, `upsert`, `verify`). Each step can override defaults and add guards.      |

### 2.2 Filters (`PlanFilters`)

| Field             | Type              | Description                                                                                 |
|-------------------|-------------------|---------------------------------------------------------------------------------------------|
| `context_ids`     | `Vec<String>`     | Exact 32-byte context IDs (hex strings with or without `0x`).                               |
| `context_aliases` | `Vec<String>`     | Alias names – **currently warn-only** in dry-run; support planned for later.                |
| `state_key_prefix`| `Option<String>`  | ABI-decoded state key prefix (auto-decoded if hex, otherwise treated as UTF-8 bytes).       |
| `raw_key_prefix`  | `Option<String>`  | Raw RocksDB key prefix (hex).                                                               |
| `alias_name`      | `Option<String>`  | Alias key filter (only meaningful on the `Alias` column).                                   |
| `key_range`       | `Option<KeyRange>`| Lexicographic start/end bounds (hex).                                                       |

### 2.3 Step Types

| Type    | Required Fields                                        | Behaviour                                                                                                         |
|---------|--------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------|
| `copy`  | `column`, optional `filters`, optional `transform`     | Preview: counts matched keys and captures samples. Future “apply” will copy data into the target database.        |
| `delete`| `column`, optional `filters`                           | Preview: counts matches only. Future “apply” will delete matching keys from the target.                           |
| `upsert`| `column`, `entries` (`[{ key, value }]`)               | Preview: shows how many literal entries would be written. Future “apply” will write them to the target.           |
| `verify`| `column`, `assertion`                                  | Preview: evaluates `expected_count`, `min_count`, `max_count`, `contains_key`, or `missing_key` immediately.      |

Assertions use `VerificationAssertion` in `plan.rs`. For `contains_key`/`missing_key` the key is an `EncodedValue` (`hex`, `base64`, `utf8`, or `json`).

---

## 3. Dry-Run Engine (`migration/dry_run.rs`)

### 3.1 Filter Resolution

1. Merge defaults with step overrides (`PlanDefaults::merge_filters`).
2. `ResolvedFilters::resolve` decodes:
   - Context IDs to `[u8; 32]`.
   - Raw prefixes/key ranges to `Vec<u8>`.
   - Alias names to `String`.
   - Logs warnings for unsupported filters (e.g., `context_aliases`) instead of failing silently.

### 3.2 Column Scan

* `scan_column` iterates the source column family using RocksDB iterators.
* Filters are applied within `ResolvedFilters::matches`.
* For the first `SAMPLE_LIMIT` matches (currently 3) we record a preview string via `sample_from_key`.
* Results per step are captured as `StepReport` (matched count, samples, warnings, and `StepDetail`).

### 3.3 Step Detail

| Detail Variant            | Notes                                                                                                                    |
|---------------------------|--------------------------------------------------------------------------------------------------------------------------|
| `Copy { decode_with_abi }`   | Indicates whether ABI decoding was requested and available.                                                         |
| `Delete`                     | No extra data beyond counts/samples.                                                                                |
| `Upsert { entries }`         | Presents the number of literal entries in the plan; previews show key/value snippets.                               |
| `Verify { summary, passed }` | Stores a human-readable summary and optional pass/fail boolean (warnings mask the result when decoding failed).     |

### 3.4 Verify Evaluation

`evaluate_assertion` counts matching rows and then checks one of:

- `ExpectedCount { expected_count }`
- `MinCount { min_count }`
- `MaxCount { max_count }`
- `ContainsKey { contains_key }`
- `MissingKey { missing_key }`

The first three operate on counts; the last two decode the provided key (via `EncodedValue::to_bytes`) and probe RocksDB for presence.

### 3.5 CLI Rendering

`print_dry_run_report` prints, for each step:

- Step number and formatted label (from the plan).
- Matched key count and filters summary.
- Step-specific details.
- Example keys (“samples”).
- Warnings (missing ABI, bad hex, unsupported filters, etc.).

---

## 4. CLI Behaviour (`main.rs`)

1. Parse arguments with Clap (`MigrateArgs`).
2. Load the plan (`load_plan`), applying overrides to source/target/WASM paths.
3. Build a `MigrationContext` (opens RocksDB handles read-only, lazily loads ABI).
4. Print plan metadata and context data (paths, ABI status, key counts).
5. Generate dry-run report (`generate_report`) and print the rich preview.
6. If `--report <FILE>` is supplied, serialize the same data as JSON for machine consumption.
7. Remind the user that dry-run mode is currently enforced.

When mutating execution lands, the same plan parsing and context wiring will be reused; we will simply branch on `--apply` to run the future execution engine.

---

## 5. Testing Strategy

### 5.1 Test Infrastructure

The `migration::test_utils` module provides `DbFixture` for creating temporary RocksDB instances with all Calimero column families. Key utilities:

- `DbFixture::new(path)` – creates a fresh RocksDB with all column families
- `insert_state_entry(context_id, state_key, value)` – adds a single state entry (uses `calimero_primitives::context::ContextId`)
- `insert_meta_entry(context_id, meta_value)` – adds a meta entry with serialized `ContextMeta` value

Helper functions:
- `test_context_id(byte)` – creates a `ContextId` filled with the given byte (returns actual Calimero type)
- `test_state_key(byte)` – creates a 32-byte state key filled with the given byte

**Note:** The test utilities now use actual Calimero types (`ContextId`, `ContextStateKey`, etc.) to ensure that tests will fail if the underlying storage types change, preventing silent breakage of migration logic.

Additional helper methods can be added as needed for future test scenarios.

### 5.2 Test Coverage

#### Core Functionality Tests

| Test Module                                     | Purpose                                                                                          |
|-------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `migration::plan::validation_tests`             | Ensures invalid plans are rejected (unsupported version, empty steps, malformed filters, etc.). |
| `migration::plan::validation_tests::encoded_value_to_bytes_decodes` | Sanity-checks `EncodedValue` decoding helpers.                                |
| `migration::dry_run::tests::dry_run_reports_copy_and_verify` | End-to-end dry-run smoke test using a temporary RocksDB.                            |
| `migration::dry_run::tests::dry_run_reports_delete_step` | Tests delete step dry-run behavior and reporting.                                        |
| `migration::dry_run::tests::dry_run_reports_upsert_step` | Tests upsert step dry-run with multiple encoded entries.                                 |
| `migration::dry_run::tests::dry_run_filters_multiple_contexts` | Validates filtering by multiple context IDs.                                       |
| `migration::dry_run::tests::dry_run_verify_min_count` | Tests min_count verification assertions.                                               |
| `migration::dry_run::tests::dry_run_verify_max_count_fails` | Tests max_count verification that should fail.                                    |
| `migration::dry_run::tests::dry_run_filters_raw_key_prefix` | Tests raw_key_prefix filter functionality.                                        |
| `migration::test_utils::tests`                  | Tests the test utilities themselves (fixture creation, entry insertion).                         |

#### Edge Case Tests

| Test Module                                     | Purpose                                                                                          |
|-------------------------------------------------|--------------------------------------------------------------------------------------------------|
| `dry_run_empty_database_returns_zero_matches`   | Verifies graceful handling of empty databases (no keys to match).                                |
| `dry_run_filters_matching_nothing`              | Tests filters that are too restrictive and match zero keys in a populated database.              |
| `dry_run_handles_malformed_short_keys`          | Tests resilience against keys shorter than expected (< 32 bytes for context ID).                 |
| `dry_run_verify_expected_count`                 | Tests ExpectedCount assertion with both passing and failing scenarios.                           |
| `dry_run_verify_contains_key`                   | Tests ContainsKey assertion for verifying key existence.                                         |
| `dry_run_verify_missing_key`                    | Tests MissingKey assertion for verifying key absence.                                            |
| `dry_run_filters_state_key_prefix`              | Tests state_key_prefix filter which operates on bytes [32..] of State keys.                     |
| `dry_run_filters_key_range`                     | Tests lexicographic key_range filtering with start (inclusive) and end (exclusive) bounds.       |
| `dry_run_filters_combined_context_and_prefix`   | Tests combining multiple filters (context_id AND raw_key_prefix) using AND logic.               |
| `dry_run_filters_context_id_on_meta_column`     | Tests that context_id filtering works on non-State columns (Meta, Config, Identity, Delta).     |

The test suite covers all step types (copy, delete, upsert, verify), filter resolution (context IDs, state key prefixes, raw key prefixes, key ranges), all verification assertions (ExpectedCount, MinCount, MaxCount, ContainsKey, MissingKey), and comprehensive edge cases including empty databases, malformed keys, filter combinations, and different column types.

---

## 6. Execution Engine (`migration/execute.rs`)

### 6.1 Overview

The execution engine implements mutating operations for migration plans when `--apply` mode is enabled. It performs actual write operations to the target database using RocksDB `WriteBatch` for atomicity.

### 6.2 Key Features

- **WriteBatch Operations**: All writes within a step are batched and committed atomically
- **Progress Logging**: Real-time progress updates during long-running operations
- **Idempotency**: Steps can be safely re-run if interrupted
- **Filter Reuse**: Leverages the same filter resolution logic from dry-run mode
- **Verification Integration**: Verify steps can abort migrations if assertions fail

### 6.3 Execution Flow

1. **Validation**: Ensures context has write access and target database is configured
2. **Step Execution**: Iterates through each step in the plan:
   - **Copy**: Reads matching keys from source, writes to target in batches
   - **Delete**: Identifies matching keys in target, deletes them in batches
   - **Upsert**: Writes literal key-value entries to target in a single batch
   - **Verify**: Evaluates assertions against target database (read-only)
3. **Reporting**: Collects execution statistics and returns detailed report

### 6.4 Safety Mechanisms

The execution engine includes comprehensive safety features to protect against data loss and ensure migration integrity:

#### Automatic Backups

- **Backup creation**: When `backup_dir` is configured in the migration plan's target endpoint, a timestamped backup is automatically created before any mutations
- **Incremental backups**: Uses RocksDB's native backup engine for efficient storage
- **Timestamped directories**: Each backup is stored in `backup_dir/backup-{timestamp}` for easy identification and restoration

Example configuration:
```yaml
target:
  db_path: /path/to/target
  backup_dir: /path/to/backups  # Automatic backup before mutations
```

#### Step Guards

Guards enforce preconditions that must be met before a step executes. Add guards to any step type:

```yaml
steps:
  - type: copy
    column: State
    guards:
      requires_empty_target: true  # Abort if target column has any keys
      requires_validation: true    # Run validation checks on target
```

Available guards:
- **`requires_empty_target`**: Ensures the target column is completely empty before executing (useful for initial migrations)
- **`requires_validation`**: Runs validation logic on the target database to ensure it's in a consistent state

Guards are checked before step execution and abort the migration with a clear error message if requirements aren't met.

#### Configurable Batch Sizes

Control memory usage and transaction sizes by configuring batch sizes at the plan level or per-step:

```yaml
defaults:
  batch_size: 500  # Plan-level default for all copy/delete steps

steps:
  - type: copy
    column: State
    batch_size: 2000  # Override for this specific step
  - type: delete
    column: Config
    # Uses plan default (500)
```

Batch size priority: step override > plan default > engine default (1000 keys).

#### Additional Safety Features

- Explicit `--apply` flag required to enable mutations
- Target database must be opened with write access
- WriteBatch ensures atomic commits per step
- Verification steps abort the migration if assertions fail
- Progress logging for operations processing large key sets
- Read-only mode enforced in dry-run to prevent accidental writes

### 6.5 CLI Usage

The migration system operates in two distinct modes. **Each command runs only one mode** - you choose either dry-run (preview) or apply (execute), not both:

#### Dry-Run Mode (Default - No Changes Made)

```bash
# Preview what the migration will do (read-only, safe to run anytime)
merodb migrate --plan plan.yaml --target-db /path/to/target
```

- Opens target database in **read-only** mode
- Scans and counts matching keys
- Shows preview with samples of what *would* happen
- **No changes are written** to the database
- Generates warnings and statistics

#### Apply Mode (Mutations - Changes Database)

```bash
# Execute the migration and write changes (requires explicit --apply flag)
merodb migrate --plan plan.yaml --target-db /path/to/target --apply
```

- Opens target database in **read-write** mode
- Actually copies/deletes/upserts keys
- Writes changes to the target database
- Shows execution progress and statistics

#### Recommended Two-Step Workflow

```bash
# Step 1: Preview the migration first (safety check)
merodb migrate --plan plan.yaml --target-db /path/to/target

# Review the output: verify key counts, check samples, read warnings...
# If everything looks correct, proceed to Step 2:

# Step 2: Execute the actual migration
merodb migrate --plan plan.yaml --target-db /path/to/target --apply

# Optional: Generate JSON report of execution results
merodb migrate --plan plan.yaml --target-db /path/to/target --apply --report results.json
```

**Important**: The `--apply` flag is what enables mutations. Without it, you're always in safe preview mode.

---

## 7. Roadmap (from `tools/merodb/migrations.md`)

- **Build Dry-run Engine** – ✅ filter resolution, per-step previews, warnings, and JSON export (`--report`).
- **Develop Test Fixtures & Dry-run Tests** – ✅ dry-run smoke tests; JSON report coverage newly added.
- **Enable Mutating Execution** – ✅ write batches, progress logging, `--apply` flag support.
- **Add Safety Mechanisms** – ✅ automatic backups (`backup_dir`), step guards (`requires_validation`, `requires_empty_target`), configurable batch sizes.
- **Implement Verification Steps** – ✅ integrated into execution engine with fail-fast behavior.
- **Polish CLI UX + Reporting** – ✅ `--report` JSON output for both dry-run and apply modes.
- **Finalize Testing & CI** – 🔄 expand coverage, integrate into CI.

---

## 8. Tips and Best Practices

- **Always dry-run first**: treat the dry-run output as the contract for what would happen in apply mode. Verify the key counts and samples before executing.
- **Use --apply cautiously**: the `--apply` flag performs actual mutations. Always ensure you have backups before running in apply mode.
- **Enable automatic backups**: configure `backup_dir` in your target endpoint to automatically create backups before mutations. This is your safety net.
- **Use step guards for safety**: add `requires_empty_target` guard for initial migrations, or `requires_validation` to ensure database consistency before risky operations.
- **Tune batch sizes for performance**: adjust `batch_size` based on your key sizes and available memory. Larger batches = fewer commits but more memory usage.
- **Leverage filters**: precise `context_ids`, prefixes, and assertions make plans safer and easier to reason about.
- **Watch warnings**: the CLI prints warnings whenever decoding fails, filters are ignored, or the ABI is missing.
- **Monitor progress**: execution mode logs progress every batch (configurable batch size) for long-running operations.
- **Verification steps**: use verify steps to validate the target database state at critical points in the migration.
- **Idempotency**: design steps to be idempotent where possible, so migrations can be safely re-run if interrupted.
- **Document plans**: use `name` and `description` fields so future readers (and CLI output) understand intent.
- **Version control**: plans are code—store them alongside application migrations in Git.
- **JSON reports**: use `--report` to generate machine-readable reports for both dry-run and apply modes.

---

With this reference, you should be able to write clear migration plans, reason about the dry-run output, execute migrations safely, and extend the implementation as needed. For day-to-day workflow examples, see `tools/merodb/README.md` which includes CLI quick-starts and GUI usage. 
