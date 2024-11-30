cd "mock/ledger"
cargo build --target wasm32-unknown-unknown --release
candid-extractor target/wasm32-unknown-unknown/release/mock_ledger.wasm > mock_ledger.did

cd ../..

cd "mock/external"
cargo build --target wasm32-unknown-unknown --release
candid-extractor target/wasm32-unknown-unknown/release/mock_external.wasm > mock_external.did

