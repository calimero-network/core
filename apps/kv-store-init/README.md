# kv-store-init

A key-value store on the v2 app SDK (`calimero-app-sdk`, `wasm32-wasip2`) whose
`#[app::init]` seeds three entries (`init_key_1`, `init_key_2`, `welcome`): the
init pattern. Methods: `set`, `get`, `getResult` (`NotFound {key}`),
`getUnchecked` (traps on a missing key), `remove`, `clear`, `entries`, `len`
(a decimal string).

```bash
cargo test --manifest-path apps/kv-store-init/Cargo.toml   # harness tests + scenarios/*.json
cargo run -p calimero-app-build -- apps/kv-store-init --out kv_store_init.wasm
```
