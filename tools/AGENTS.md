# tools/ - Development Tools

Development tools for Calimero apps and the node. A stopped node's store is
inspected with `merod inspect` (`crates/merod`).

## Available Tools

| Tool           | Binary     | Purpose                                      |
| -------------- | ---------- | -------------------------------------------- |
| `mero-sign`    | `mero-sign` | Sign Calimero bundle manifests (Ed25519); `sign --dev` uses the well-known development key, which a public registry refuses |
| `mero-compute` | `mero-compute` | Run one compute method of a compute component alone, as a job node would (`run --method M --out DIR`); `README.md` has the flags and what it leaves out |
| `client-gen`   | `calimero-client-gen` | Generates `calimero-client`'s route layer (`crates/client/src/generated/`) from `docs/public/openapi.json`; its test (`the_checked_in_client_is_what_the_document_generates`) fails while the checked-in code differs |
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
# The Rust client's route layer, after the node's document changed
cargo run -p calimero-client-gen
# A stopped node's store (apps, keys, log)
cargo run -p merod -- --node n1 inspect --help
```
