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
  `a_consent_verifies_from_public_data_only`. `verify_sign_in` is the same
  check with presence only (a sign-in or sign-up asks no verification), and
  `assertion_counter` answers the counter a genuine assertion carries,
  whatever its challenge (what proves `SetSignCount`). Test:
  `a_sign_in_and_a_counter_verify_from_public_data_only`.
- `confirm_challenge`: what a passkey signs to approve one `#[app::confirm]` call.
- `new_recovery_code` / `normalize_recovery_code`: an account recovery code,
  160 random bits as Crockford base32 in groups of four, read back however it
  is typed (at most `MAX_RECOVERY_CODE_CHARS`). Test:
  `a_recovery_code_reads_back_however_it_is_typed`.
- Feature `testing`: `SoftAuthenticator`, an in-memory ES256 passkey.
- `recovery.rs`: an organization's recovery key (Ed25519; the owner keeps the
  seed, the node stores the public half). `sign_recovery`/`verify_recovery`
  over `"calimero.org-recovery.v1\0" || org || 0 || account || 0 || nonce`;
  the seed is written as 64 hex digits in groups of eight (`seed_text`,
  `parse_seed`). mero-js and calimero-client-py sign the same bytes; the
  pinned vectors are in `the_wire_bytes_are_pinned_for_the_clients`.

Tests: `cargo test -p calimero-identity`.
