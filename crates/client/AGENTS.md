# calimero-client

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

Errors: `Error::Api` is the node's answer (`{ error, message, details }`);
`Error::Transport` is not reaching, attesting or understanding it. One envelope
refusal is an answer too: a sealed request past `MAX_SEALED_BYTES` comes back
`413` in the clear, and `sealed_error` makes it `Error::Api { 413, TOO_LARGE }`.

Calls are retried with the same key (D70): `Client::call` always sends an
idempotency key (the caller's, else a new `node:c-…` key, which the node keeps
in its short-lived class, 10 minutes) and sends the call again with that key
while the node answers `503` or cannot be reached (`Error::is_unavailable`;
200 ms doubling to 2 s, 6 retries). A call that did commit is answered from its
record. Confirm calls (`call_confirmed`) are not retried: their challenge is
single-use. Tests: `retry_tests`, and `calls_without_a_key_each_run_once` in
`crates/node/tests/cluster.rs`.

The command line over this client is `meroctl` (`crates/meroctl`).

```bash
cargo test -p calimero-client
```
