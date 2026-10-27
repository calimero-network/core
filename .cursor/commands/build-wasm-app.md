# Build WASM App

Build a Calimero WASM application for deployment.

**Instructions:**

1. If user specified an app (e.g. `/build-wasm-app kv-store`), use that. Otherwise ask which app or use `kv-store` as default.
2. Run:
   ```bash
   rustup target add wasm32-unknown-unknown   # if not already added
   PATH="$(scripts/setup-cargo-mero.sh):$PATH"
   cargo mero build --manifest-path apps/<app-name>/Cargo.toml
   ```
3. Output: `apps/<app-name>/res/<app_name>.wasm`, with the ABI embedded. The node reads handlers, timers, xcall entry points, search exports and migrations from that ABI; a plain `cargo build --target wasm32-unknown-unknown --profile app-release` (what some tests load from `target/wasm32-unknown-unknown/app-release/`) has none.
4. To install it as a signed package, bundle it with `cargo mero bundle`.

**Common apps:** kv-store, kv-store-with-handlers, blobs, collaborative-editor, xcall-example, org-gated
