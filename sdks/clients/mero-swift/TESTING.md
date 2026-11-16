# Testing

| What | How |
| --- | --- |
| Unit and journey tests (no node) | `swift test` |
| The generated API is current, as CI checks it | `scripts/generate-openapi.sh check` (builds swift-openapi-generator once into `.build/openapi-generator`) |
| Lint, as CI runs it | `swiftlint lint --strict` and `swift-format lint --recursive --strict Sources Tests Examples` |
| Against a development cluster | `scripts/dev-cluster.sh up`, then `MERO_E2E_NODE_URL=http://127.0.0.1:2501 [MERO_E2E_BUNDLE=kv_store.wasm] swift test --filter MeroKitE2ETests` |
| The sample's UI tests | CI (`ui.yml`) runs them in the simulator against the in-app cluster (`-uitest-mock`) |

The journey tests run `CalimeroClient`, `CloudAppBackend` and `MeroAppSession`
against `FakeCloudNode` (MeroKitTestSupport): mock attestation, a real Noise NK
responder and sealed frames, so the whole client runs as it does against a
node. `SealedTransportTests` checks the wire against core's published vectors;
the TEE suites check DCAP, Sigstore and signed releases against captured
fixtures.

## A development cluster on a Mac

`merod` 1.0 runs on Linux only. [`scripts/dev-cluster.sh`](scripts/dev-cluster.sh)
runs core's `merod local` (three nodes, mock attestation) from
`ghcr.io/calimero-network/merod` in Docker and publishes the nodes on the Mac,
at the release in `ci/core-version`:

```sh
scripts/dev-cluster.sh up            # or: up --fresh to start from empty state
scripts/dev-cluster.sh install app.mpk   # optional: an app for the sample to open
scripts/dev-cluster.sh down
```

`merod local` listens on 127.0.0.1 inside the container, so a forwarder in the
same network namespace publishes it; plain `docker run -p` cannot reach it.
The sample's "Manually enter node URL" takes `http://localhost:2501` in the
Simulator, or the Mac's LAN address on an iPhone on the same Wi-Fi
(`scripts/dev-cluster.sh urls` prints both). Set `MEROD_BIN` to a merod 1.0
binary to run it natively instead (Linux).
