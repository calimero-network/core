# Build WASM App

Build a Calimero app into a `calimero:app` component for deployment.

**Instructions:**

1. If user specified an app (e.g. `/build-wasm-app kv-store`), use that. Otherwise ask which app or use `kv-store` as default.
2. Run, from the repository root:
   ```bash
   rustup target add wasm32-wasip2           # if not already added
   cargo install --locked --path crates/meroctl   # once: puts meroctl on PATH
   meroctl app build apps/<app-name> --out <app_name>.wasm
   ```
3. Output: one component with its ABI in it. The node reads handlers, timers, xcall entry points, search exports and migrations from that ABI, so a plain `cargo build` of the crate is not an installable app.
4. To install it as a signed package, pack and sign it into an `.mpk` with `meroctl app pack` (see `docs/src/content/docs/build/packaging-signing.mdx`).

**Common apps:** kv-store, rooms, indexed-forum, org-gated, compute-redact (all under `apps/`; the node's test fixtures are under `apps/v2-fixtures/`).
