# tools/ - Development Tools

Development tools for Calimero apps and the node. A stopped node's store is
inspected with `merod inspect` (`crates/merod`).

## Available Tools

| Tool           | Binary     | Purpose                                      |
| -------------- | ---------- | -------------------------------------------- |
| `mero-sign`    | `mero-sign` (deprecated) | The library `meroctl app sign`, `app signer` and `app pack` sign with (Ed25519 over the JCS manifest); `--dev` uses the well-known development key, which a public registry refuses. The binary is kept one release and points to `meroctl` |
| `mero-compute` | `mero-compute` (deprecated) | The library behind `meroctl local compute run`: one compute method of a compute component alone, as a job node would; `README.md` has the flags and what it leaves out. The binary is kept one release |
| `client-gen`   | `calimero-client-gen` | Generates `calimero-client`'s route layer (`crates/client/src/generated/`) from `docs/public/openapi.json`; its test (`the_checked_in_client_is_what_the_document_generates`) fails while the checked-in code differs |
| `search-bench` | `search-bench` | `calimero-search`'s engine over the node's `RamDirectory` (and `MmapDirectory`): index size, query latency, memory, a query's host time against the work it reports; `README.md` holds the results, CI's `benchmarks.yml` uploads a report |

Everything here is a Rust crate.

An app is a `calimero:app` component, built with `calimero-app-build`
(`meroctl app build <crate-dir>`) and packed into a signed single-service `.mpk`
with `meroctl app pack` (`crates/meroctl/src/make.rs`), which signs through
`mero-sign`'s library. `mero-app-build`, `mero-sign`, `mero-compute` and
`scripts/pack-component.py` remain one release as shims that say so. The SDK 0.12 tools (`cargo-mero`, `mero-abi`,
`storage-cost`, `storage-compare`, `state-disk-cost`) were removed with the wasmer
engine.

## Commands

```bash
cargo run -p meroctl -- app sign path/to/manifest.json --dev
cargo run -p meroctl -- app signer new key.json
cargo run -p meroctl -- app signer id key.json
cargo run --release -p search-bench
# The Rust client's route layer, after the node's document changed
cargo run -p calimero-client-gen
# A stopped node's store (apps, keys, log)
cargo run -p merod -- --node n1 inspect --help
```
