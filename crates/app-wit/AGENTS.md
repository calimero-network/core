# calimero-app-wit - the `calimero:app` WIT world

## Package Identity

- **Crate**: `calimero-app-wit` (`publish = false`, prototype)
- **Entry**: `src/lib.rs`; WIT in `wit/calimero-app.wit`, vendored WASI 0.2.12 in `wit/deps/`
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 4 is the authority

## What This Crate Does

Holds the `calimero:app@0.5.0` world from the spec (plus `search`, below) and exposes `WIT_DIR`, `WORLD`, `allowed_imports()` (what the world imports) and `stubbed_imports()` (the nine D44 interfaces accepted at install). Anything in neither list is refused.

## Beyond the spec's section 4

- `interface search` (views only): spec section 7 says full-text search is a host-maintained index queried from views but its world names no interface for it; this one is ours, pending the spec's next version.
- `xcall.call` and `xcall.view` (synchronous xcalls): the spec's `xcall` has only `send`, the outbox. These wait for another app's answer in the same run (`docs/src/content/docs/protocol/xcall.mdx`, "Synchronous xcalls"); a node before them refuses a component that imports them (unknown import).

## Rules

- Change the WIT only together with the spec (or name the gap, as below); keep `allowed_imports()` in step with the world.
- `wit/deps/sockets` is vendored only because `wasi:cli` and `wasi:http` reference it; it is not imported by the world.

## Verify

`cargo test -p calimero-app-wit`
