# calimero-pipeline - Pipeline Language

The pure compiler for the pipeline YAML language: structural checks, canonical JSON, apply-time checks against a cluster's modules and pools, and the worst-case fuel and cost bounds.
It has no node dependency.

## Package Identity

- **Crate**: `calimero-pipeline`
- **Entry**: `src/lib.rs` (API), `src/model.rs` (typed pipeline), `src/compile.rs` (YAML to typed, structural checks), `src/check.rs` (apply-time checks, fuel)
- **Key deps**: `serde_norway` (YAML), `serde_json_canonicalizer` (deterministic bytes)

## Commands

```bash
cargo test -p calimero-pipeline
```

## Public API

| Item | Kind | Purpose |
| --- | --- | --- |
| `compile()` | fn | YAML or JSON source to a `Pipeline`; every structural error is returned, each with a path and a stable code |
| `canonical_json()` | fn | RFC 8785 bytes of a `Pipeline`; the form the API stores and the engine executes |
| `check()` | fn | Apply-time checks of modules, methods, limits and pools against an `Env` |
| `worst_case_fuel()` | fn | Sum over steps of fuel cap times maximum runs |
| `worst_case_cost()`, `run_cost()` | fn | Upper bound of a pipeline, or of one run, priced by a `PriceTable` (u128, rounded up) |
| `priced_cost()` | fn | What a run used (fuel, GiB-microseconds, CPU microseconds), priced and never above `run_cost` of its caps |
| `PriceTable` | struct | The operator's prices; `validate()` refuses a zero fuel rate (`BAD_PRICES`) |
| `SpecError` | struct | `path`, `code`, `message`; codes are stable identifiers |

## Rules

- `when`, `runIf`, `uses` and loop or variable fields are refused with `NOT_IN_V1`; they are never ignored.
- A step with no module and no pipeline module runs in `Env::default_module`; with none either, `check` answers `MISSING_MODULE`.
- A module name is matched as written (`com.acme.pdf@0.1.0` and `com.acme.pdf` are two entries of `Env::modules`); the node resolves each reference and fills `Env::modules` with the releases it pins, so `check` needs no version logic.
- A reference to a step adds it to `needs` in the compiled pipeline, so the canonical form always lists every arrow.
- Every bound is a named constant at the top of `src/compile.rs`.
- A fan-out run's inputs, as the engine builds them: the step's `inputs` references in the order written, with the run's item files where `item` stands, or first when `item` is not listed; a step with no `inputs` gets just the item files. `collect: <step>.outputs` puts all of that step's outputs in item order first, then the step's `inputs`.
