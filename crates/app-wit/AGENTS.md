# calimero-app-wit - the `calimero:app` WIT world

## Package Identity

- **Crate**: `calimero-app-wit` (`publish = false`, prototype)
- **Entry**: `src/lib.rs`; WIT in `wit/calimero-app.wit`, vendored WASI 0.2.12 in `wit/deps/`
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 4 is the authority

## What This Crate Does

Holds the `calimero:app@0.5.0` world verbatim from the spec and exposes `WIT_DIR`, `WORLD`, `allowed_imports()` (what the world imports) and `stubbed_imports()` (the nine D44 interfaces accepted at install). Anything in neither list is refused.

## Rules

- Change the WIT only together with the spec; keep `allowed_imports()` in step with the world.
- `wit/deps/sockets` is vendored only because `wasi:cli` and `wasi:http` reference it; it is not imported by the world.

## Verify

`cargo test -p calimero-app-wit`
