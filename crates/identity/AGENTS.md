# calimero-identity

Identity: WebAuthn passkeys for people, API keys for
machines, short session tokens any node verifies, and step-up consent challenges.

- Pure logic: no storage. The node keeps accounts, credentials, API-key
  hashes, revocations and the session key K_s in the system group.
- `webauthn.rs`: registration and assertion verification, ES256 and EdDSA via
  `ring`; attestation statements are not checked (passkeys use `none`).
  A sign counter that does not advance is refused unless both are zero.
- Session tokens: `v1.<claims b64url>.<HMAC-SHA256 b64url>`.
- `confirm_challenge`: what a passkey signs to approve one `#[app::confirm]` call.
- Feature `testing`: `SoftAuthenticator`, an in-memory ES256 passkey.

Tests: `cargo test -p calimero-identity`.
