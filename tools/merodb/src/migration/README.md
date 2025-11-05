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
  - type: verify
    column: State
    assertion:
      expected_count: 123
```

### 2.1 Top-Level Fields

| Field        | Description                                                                                                    |
|--------------|----------------------------------------------------------------------------------------------------------------|
| `version`    | Plan schema version. Only `1` is accepted; upgrades must bump this value.                                      |
| `name`/`description` | Optional metadata shown in CLI output to aid humans.                                                |
| `source`     | Required path to the source RocksDB database plus optional WASM file for ABI decoding.                         |
| `target`     | Optional path to the target database and backup directory (used later for mutating runs).                      |
| `defaults`   | Settings inherited by every step unless overridden (columns, filters, `decode_with_abi`, `write_if_missing`).   |
| `steps`      | Ordered list of migration actions (`copy`, `delete`, `upsert`, `verify`). Each step can override defaults.      |

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
- `insert_state_entry(context_id, state_key, value)` – adds a single state entry

Helper functions:
- `test_context_id(byte)` – creates a 32-byte context ID filled with the given byte
- `test_state_key(byte)` – creates a 32-byte state key filled with the given byte

Additional helper methods can be added as needed for future test scenarios.

### 5.2 Test Coverage

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

The test suite covers all step types (copy, delete, upsert, verify), filter resolution (context IDs, raw key prefixes), and various verification assertions.

---

## 6. Roadmap (from `tools/merodb/migrations.md`)

- **Build Dry-run Engine** – ✅ filter resolution, per-step previews, warnings, and JSON export (`--report`).
- **Develop Test Fixtures & Dry-run Tests** – ✅ dry-run smoke tests; JSON report coverage newly added.
- **Enable Mutating Execution** – 🔄 upcoming (write batches, idempotency, `--apply`).
- **Add Safety Mechanisms** – 🔄 backups, guard rails, context alias support.
- **Implement Verification Steps** – 🔄 integrate with existing validator for post-apply checks.
- **Polish CLI UX + Reporting** – 🚧 `--report` JSON output shipped; exit codes and richer docs outstanding.
- **Finalize Testing & CI** – 🔄 expand coverage, integrate into CI.

---

## 7. Tips and Best Practices

- **Always dry-run first**: treat the current output as the contract for what would happen in apply mode.
- **Leverage filters**: precise `context_ids`, prefixes, and assertions make plans safer and easier to reason about.
- **Watch warnings**: the CLI prints warnings whenever decoding fails, filters are ignored, or the ABI is missing.
- **Document plans**: use `name` and `description` fields so future readers (and CLI output) understand intent.
- **Version control**: plans are code—store them alongside application migrations in Git.

---

With this reference, you should be able to write clear migration plans, reason about the dry-run output, and extend the implementation safely. For day-to-day workflow examples, see `tools/merodb/README.md` which includes CLI quick-starts and GUI usage. 
