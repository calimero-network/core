# calimero-cloud-client

Rust client for the release-v2 cloud: verifies the node's attestation
(`attest.rs`, shared with the node), seals every request (`calimero-sealed`),
signs in with a `Passkey` (feature `testing`: `SoftAuthenticator`) or an API key,
and calls `/apps/{app}/{intents,views,events}`.

The attestation check (`attest::verify`) is the one place clients and peers decide
trust: quote genuine (or an allowed mock), report data =
`nonce || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.
mero-js `src/cloud-v2/attestation.ts` implements the same check; change both or neither.
