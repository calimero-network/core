# tools/ - Development Tools

Development tools for Calimero apps and the node. A stopped node's store is
inspected with `merod inspect` (`crates/merod`).

## Available Tools

| Tool           | Binary     | Purpose                                      |
| -------------- | ---------- | -------------------------------------------- |
| `mero-sign`    | `mero-sign` | Sign Calimero bundle manifests (Ed25519); `sign --dev` uses the well-known development key, which a public registry refuses |
| `search-bench` | `search-bench` | `calimero-search`'s engine over the node's `RamDirectory` (and `MmapDirectory`): index size, query latency, memory, the search-gas fit; `README.md` holds the results, CI's `benchmarks.yml` uploads a report |

Everything here is a Rust crate.

An app is a `calimero:app` component, built with `calimero-app-build`
(`mero-app-build <crate-dir> --out <file>`, put on PATH by `scripts/install-mero-app-build.sh`)
and packed into a signed single-service `.mpk` with `scripts/pack-component.py`, which
signs through `mero-sign`. The SDK 0.12 tools (`cargo-mero`, `mero-abi`,
`storage-cost`, `storage-compare`, `state-disk-cost`) were removed with the wasmer
engine.

## Commands

```bash
cargo run -p mero-sign -- sign path/to/manifest.json --dev
cargo run -p mero-sign -- generate-key --output key.json
cargo run -p mero-sign -- derive-signer-id --key key.json
cargo run --release -p search-bench
# A stopped node's store (apps, keys, log)
cargo run -p merod -- --node n1 inspect --help
```
