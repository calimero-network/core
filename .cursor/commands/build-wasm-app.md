# Build WASM App

Build a Calimero WASM application for deployment.

**Instructions:**

1. If user specified an app (e.g. `/build-wasm-app kv-store`), use that. Otherwise ask which app or use `kv-store` as default.
2. Run:
   ```bash
   rustup target add wasm32-unknown-unknown   # if not already added
   cargo build -p <app-name> --target wasm32-unknown-unknown --release
   ```
3. Output path: `target/wasm32-unknown-unknown/release/<app_name>.wasm`
4. For production, consider `--profile app-release` if that profile exists in Cargo.toml.

**Common apps:** kv-store, access-control, blobs, collaborative-editor, xcall-example
