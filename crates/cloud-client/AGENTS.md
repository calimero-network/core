# calimero-cloud-client

Rust client for the release-v2 cloud: verifies the node's attestation
(`attest.rs`, shared with the node), seals every request (`calimero-sealed`),
signs in with a `Passkey` (feature `testing`: `SoftAuthenticator`) or an API key,
and calls `/apps/{app}/{intents,views,events}`.

The attestation check (`attest::verify`) is the one place clients and peers decide
trust: quote genuine (or an allowed mock), report data =
`nonce || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.
A real quote also needs a measurement policy (`Measurements`): `None` (the default)
refuses every real quote, `Pinned` checks fixed allowlists, `Release` fetches the
Sigstore-verified `published-mrtds.json` of the release the node names and requires a
matching allowed profile; then `policy_valid` (TCB status, debug TD, app-hash binding).
mero-js `src/cloud-v2/attestation.ts` implements the same check; change both or neither.

## `calimero-cloud` (feature `cli`)

The command line, over the same client: `deploy`, `bundle`, `app`, `call`, `view`,
`events`, `apikey`, `org`, `ops` (with `--ops-token`), `raw`. Services use an API key
(`CALIMERO_API_KEY`). On a development cluster (`--allow-mock`) `login --dev-passkey FILE`
signs up or in with a passkey kept in that file; sessions are saved per URL under
`$XDG_CONFIG_HOME/calimero-cloud/` and refreshed when they expire.

```bash
cargo run -p calimero-cloud-client --features cli -- --help
cargo test -p calimero-cloud-client --features cli --test cli   # against an in-process local cluster
```
