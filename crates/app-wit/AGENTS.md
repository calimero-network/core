# calimero-app-wit - the `calimero:app` WIT world

## Package Identity

- **Crate**: `calimero-app-wit` (`publish = false`, prototype)
- **Entry**: `src/lib.rs`; WIT in `wit/calimero-app.wit`, vendored WASI 0.2.12 in `wit/deps/`
- **Spec**: `docs/specs/app-runtime-v0.14.md` section 4 is the authority

## What This Crate Does

Holds the `calimero:app@0.5.6` world: the spec's 0.5.0 plus `search`, below, plus the additive `files`, `runs` and `job` interfaces (0.5.1), `context.via` (0.5.2), `context.linked` (0.5.3), `xcall.batch` (0.5.4), `context.unit-of` (0.5.5) and `context.visitor` and `context.grant-scope` (0.5.6), and exposes `WIT_DIR`, `WORLD`, `COMPUTE_WORLD`, `allowed_imports()` (what the app world imports), `compute_allowed_imports()` (what the compute world imports) and `stubbed_imports()` (the nine D44 interfaces accepted at install). Anything in neither list is refused.

## The compute world

`runs.publish(id) -> result<blob-id, string>` turns a done run's output into a plain blob; it lives in `runs`, so the compute world cannot import it, and the engine test `a_compute_component_that_imports_the_publish_call_is_refused_at_prepare` pins the link-time refusal.
`world compute` is the compute methods of an app built as their own component for job nodes: it imports `types`, `files`, `job` (`run-id`, `inputs`, `progress`) and the WASI set a Rust guest needs, and exports `run(method, args)`.
`runs.uploader(id) -> option<string>` answers who uploaded a sealed file (none for a file a call or a run made); it is in `runs` because the compute world imports `files` whole and must not see it, and it adds no field to `blob-info`.
The app world imports `runs` (`start`, `cancel`: logged writes only) and not `job`, so it has 23 imports.
It cannot be named `compute`, which is the world's name; the compute world does not import `runs`, so a run cannot start runs.
The compute world imports no state, xcall, context, events, blobs, search, action or `wasi:http`, so a compute component cannot even ask for them: the install-time import check is the enforcement.

## Beyond the spec's section 4

- `interface search` (views only): spec section 7 says full-text search is a host-maintained index queried from views but its world names no interface for it; this one is ours, pending the spec's next version.
- `xcall.call` and `xcall.view` (synchronous xcalls): the spec's `xcall` has only `send`, the outbox. These wait for another app's answer in the same run (`docs/src/content/docs/protocol/xcall.mdx`, "Synchronous xcalls"); a node before them refuses a component that imports them (unknown import).

- `context.via` (0.5.2): for a call another app made as the person it serves, with their grant (`caller` is that person), the app it came through; see `docs/src/content/docs/protocol/xcall.mdx`, "As the person".
- `xcall.batch` (0.5.4): a list of `request { kind: call | view, app, method, args }`, answered in order; the node runs them at once, each with the checks, count and key of the same `call`/`view` alone. The SDK sends every synchronous xcall through it; `call` and `view` stay for components built before.
- `context.visitor` (0.5.6): the visitor a public link let in (`record visitor-caller { link, key }`), when `caller` is `public` because of one; `caller` stays `public` for it, so a component built before 0.5.6 sees an anonymous caller.
- `context.grant-scope` (0.5.6): the scope (JSON text) of the caller's grant on the instance, which its managers set for the app; none for a caller who is no person or is named without one, and in a compute run. The host asks `Directory::grant_scope`, which answers none by default.
- `context.unit-of` (0.5.5): the app whose unit this one is a member of (the instance whose release created it); none otherwise and in a compute run. The host asks `Directory::unit_of`, which answers none by default.
- `context.linked` (0.5.3): the app linked under a name of the release's `uses` (a unit member or an attached app); none in a compute run. The host asks `Directory::linked`, which answers none by default.

## Rules

- A patch bump is additive only: no existing type or function changes, so a component built against an older 0.5.z still links (the engine accepts `calimero:app/*` imports at any 0.5 patch up to the world's).
- Change the WIT only together with the spec (or name the gap, as below); keep `allowed_imports()` in step with the world.
- `wit/deps/sockets` is vendored only because `wasi:cli` and `wasi:http` reference it; it is not imported by the world.

## Verify

`cargo test -p calimero-app-wit`
