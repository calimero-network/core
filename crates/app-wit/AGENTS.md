# calimero-app-wit - the `calimero:app` WIT world

## Package Identity

- **Crate**: `calimero-app-wit` (`publish = false`, prototype)
- **Entry**: `src/lib.rs`; WIT in `wit/calimero-app.wit`, vendored WASI 0.2.12 in `wit/deps/`
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 4 is the authority

## What This Crate Does

Holds the `calimero:app@0.5.1` world: the spec's 0.5.0 plus `search`, below, plus the additive `files`, `runs` and `job` interfaces, and exposes `WIT_DIR`, `WORLD`, `COMPUTE_WORLD`, `allowed_imports()` (what the app world imports), `compute_allowed_imports()` (what the compute world imports) and `stubbed_imports()` (the nine D44 interfaces plus the seven `wasi:sockets` ones, accepted at install). Anything in neither list is refused.

## The compute world

`world compute` is the compute methods of an app built as their own component for job nodes: it imports `types`, `files`, `job` (`run-id`, `inputs`, `progress`) and the WASI set a Rust guest needs, and exports `run(method, args)`.
The app world imports `runs` (`start`, `cancel`: logged writes only) and not `job`, so it has 24 imports (`collections` is a PoC interface, host collections; every call traps `HostDenied` until the host core implements them).
It cannot be named `compute`, which is the world's name; the compute world does not import `runs`, so a run cannot start runs.
The compute world imports no state, xcall, context, events, blobs, search, action or `wasi:http`, so a compute component cannot even ask for them: the install-time import check is the enforcement.

## Beyond the spec's section 4

- `interface search` (views only): spec section 7 says full-text search is a host-maintained index queried from views but its world names no interface for it; this one is ours, pending the spec's next version.
- `xcall.call` and `xcall.view` (synchronous xcalls): the spec's `xcall` has only `send`, the outbox. These wait for another app's answer in the same run (`docs/src/content/docs/protocol/xcall.mdx`, "Synchronous xcalls"); a node before them refuses a component that imports them (unknown import).

## Rules

- A patch bump is additive only: no existing type or function changes, so a component built against an older 0.5.z still links (the engine accepts `calimero:app/*` imports at any 0.5 patch up to the world's).
- Change the WIT only together with the spec (or name the gap, as below); keep `allowed_imports()` in step with the world.
- `wit/deps/sockets` is vendored because `wasi:cli` and `wasi:http` reference it; it is not imported by the world. Its seven interfaces are stubs (PoC, approved by Xabi 2026-11-09): componentize-py 0.25.1's default build imports them all, and the engine refuses every socket and lookup.

## Verify

`cargo test -p calimero-app-wit`
