# calimero-client

Rust client for Calimero nodes: verifies the node's attestation
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
The quote step itself (`verify_quote`, then `judge` for a real quote's registers) is
shared by `verify`, `verify_receipt_key` and `attest::release::verify_release`.
A document that carries `kms` (`KmsKeys`: what the node verified of the KMS its storage
and disk keys came from, asked with `purpose=kms`) passes `verify` only if its report
data is `kms_report_data`: `nonce || SHA-256("calimero.tee-attest.kms-evidence.v1" |
transport_binding | SHA-256(JCS(kms)))`. Opt in: a document without `kms` is checked
exactly as before. mero-js does not ask for it yet; the attestation verifier
(mero-tee) recomputes it, pinned by `the_kms_binding_matches_the_published_vector`.

Release attestations (`attest/release.rs`, made by `merod attest`): a quote a node makes
before it holds any key, report data
`nonce || SHA-256("calimero.tee-attest.release.v1" || u32-BE-length-prefixed merodVersion,
merodSha256, teeRelease, teeProfile, identity)`. `verify_release` refuses another format,
nonce or claim, then runs the same quote check (a `Release` policy reads the release the
claim names) and refuses an envelope whose registers or profile are not the verified
quote's. `fetch_release_attestation` gets one from `GET /attestation/release`. Only
managers use it (mdma, `meroctl attest-release`); mero-js has no counterpart.

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
