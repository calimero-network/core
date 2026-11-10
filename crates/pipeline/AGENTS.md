# calimero-pipeline - Pipeline Language

The pure compiler for the pipeline YAML language: structural checks, canonical JSON, apply-time checks against a cluster's modules and pools, and the worst-case fuel bound.
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
| `SpecError` | struct | `path`, `code`, `message`; codes are stable identifiers |

## Rules

- `when`, `runIf`, `uses` and loop or variable fields are refused with `NOT_IN_V1`; they are never ignored.
- A reference to a step adds it to `needs` in the compiled pipeline, so the canonical form always lists every arrow.
- Every bound is a named constant at the top of `src/compile.rs`.
