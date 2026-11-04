# kv-store-init

A key-value store on the v2 app SDK (`calimero-sdk`, `wasm32-wasip2`) whose
`#[app::init]` seeds three entries (`init_key_1`, `init_key_2`, `welcome`): the
init pattern. Methods: `set`, `get`, `getResult` (`NotFound {key}`),
`getUnchecked` (traps on a missing key), `remove`, `clear`, `entries`, `len`
(a decimal string).

```bash
cargo test --manifest-path apps/kv-store-init/Cargo.toml   # harness tests + scenarios/*.json
scripts/install-mero-app-build.sh   # once: puts mero-app-build on PATH
mero-app-build apps/kv-store-init --out kv_store_init.wasm
```
