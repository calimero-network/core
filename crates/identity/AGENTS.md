# calimero-identity

Identity: WebAuthn passkeys for people, API keys for
machines, short session tokens any node verifies, and step-up consent challenges.

- Pure logic: no storage. The node keeps accounts, credentials, API-key
  hashes and revocations in the system group.
- `webauthn.rs`: registration and assertion verification, ES256 and EdDSA via
  `ring`; attestation statements are not checked (passkeys use `none`).
  A sign counter that does not advance is refused unless both are zero.
- Session tokens: `v2.<claims b64url>.<Ed25519 b64url>` (`mint_session`,
  `verify_session`), signed by the issuing node's attested receipt key over
  `"calimero.session.v2\0" || claims`; the claims name the node and its key.
  Which issuer may vouch for an account is the node's to judge. Test:
  `node_signed_session_tokens`.
- `verify_consent`: the part of a step-up assertion every replica can check
  from the system state alone (credential, challenge, UP and UV, signature),
  so the node checks a logged consent where the command applies; origin, RP id
  and the counter stay with `verify_assertion` on the node that took it. Test:
  `a_consent_verifies_from_public_data_only`.
- `confirm_challenge`: what a passkey signs to approve one `#[app::confirm]` call.
- Feature `testing`: `SoftAuthenticator`, an in-memory ES256 passkey.

Tests: `cargo test -p calimero-identity`.
