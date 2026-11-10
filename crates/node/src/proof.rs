//! What a logged decision carries to prove who made it, checked where the
//! command applies, on every node, from the system state alone.
//!
//! A person's decision (an approval, an upgrade, a transfer, deleting an
//! organization) carries their passkey assertion over a [`Statement`]: the
//! subject, the method and the arguments the API showed them, and a nonce.
//! The challenge they signed is [`calimero_identity::confirm_challenge`] of
//! those, so `apply` rebuilds it, checks the assertion under the credential
//! the actor registered, and checks that the statement says what the command
//! does. The operator's decision carries the same statement signed with the
//! operator key recorded at genesis ([`OperatorKey`]).
//!
//! Either is accepted once: `apply` records what it used
//! ([`crate::system::SystemState::used_proofs`]), so a node cannot log a
//! decision again after its maker reversed it.

use calimero_identity::{from_b64url, sha256, Assertion, Credential};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a person or the operator agreed to: `method` on `subject` with
/// `args` (the exact JSON text the API showed), under a one-time `nonce`
/// (base64url).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub subject: String,
    pub method: String,
    pub args: String,
    pub nonce: String,
}

impl Statement {
    /// A statement over `args` (serialized here, once: what is signed is that
    /// text) with a fresh nonce.
    #[must_use]
    pub fn new(subject: &str, method: &str, args: &Value) -> Self {
        Self {
            subject: subject.to_owned(),
            method: method.to_owned(),
            args: serde_json::to_string(args).unwrap_or_default(),
            nonce: calimero_identity::b64url(&calimero_identity::random::<16>()),
        }
    }

    /// The challenge a passkey signs for it.
    ///
    /// # Errors
    /// When the nonce is not base64url.
    pub fn challenge(&self) -> Result<[u8; 32], &'static str> {
        let nonce = from_b64url(&self.nonce).map_err(|_| "a malformed consent nonce")?;
        Ok(calimero_identity::confirm_challenge(
            &self.subject,
            &self.method,
            self.args.as_bytes(),
            &nonce,
        ))
    }

    /// The bytes the operator signs for it in `cluster`.
    #[must_use]
    pub fn operator_message(&self, cluster: &str) -> Vec<u8> {
        let mut m = OPERATOR_DOMAIN.to_vec();
        for part in [
            cluster,
            &self.subject,
            &self.method,
            &self.args,
            &self.nonce,
        ] {
            m.extend_from_slice(part.as_bytes());
            m.push(0);
        }
        m
    }

    /// The arguments, parsed.
    #[must_use]
    pub fn args(&self) -> Value {
        serde_json::from_str(&self.args).unwrap_or(Value::Null)
    }
}

/// What the operator's signature covers before the statement.
pub const OPERATOR_DOMAIN: &[u8] = b"calimero.operator.v1\0";

/// A person's consent as the log carries it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Consent {
    #[serde(flatten)]
    pub statement: Statement,
    pub assertion: Assertion,
    /// When the passkey gave it (Unix seconds, the clock of the node that
    /// took the step-up): the time of the account's activity entry
    /// (`types::Account::activity`). Outside what the passkey signs, so a
    /// release that does not know it checks the same consent; 0 (a consent
    /// an earlier release logged, or a sign-in's): no entry.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub at: u64,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if"
)]
const fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Consent {
    /// What marks this consent used, as [`verify_consent`] answers it.
    ///
    /// # Errors
    /// When the nonce is not base64url.
    pub fn used(&self) -> Result<String, &'static str> {
        Ok(used(b"consent", &self.statement.challenge()?))
    }
}

/// The operator's signature over a statement (Ed25519, hex).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperatorProof {
    #[serde(flatten)]
    pub statement: Statement,
    pub signature: String,
}

/// The operator's verify key, and the cluster its signatures name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorKey {
    /// Ed25519 public key, hex.
    pub key: String,
    pub cluster: String,
}

/// Check that `consent` is `credentials`' holder's assertion over its own
/// statement, and answer the statement and what marks it used.
///
/// # Errors
/// A consent that is missing, malformed, or not signed by a passkey of the
/// actor.
pub fn verify_consent(
    consent: &Value,
    credentials: &[Credential],
) -> Result<(Statement, String), &'static str> {
    verify_signed(consent, credentials, true)
}

/// [`verify_consent`] for a sign-in or a sign-up: the passkey's holder
/// present, verified or not, as the ceremony asked of them.
///
/// # Errors
/// As [`verify_consent`].
pub fn verify_sign_in(
    consent: &Value,
    credentials: &[Credential],
) -> Result<(Statement, String), &'static str> {
    verify_signed(consent, credentials, false)
}

fn verify_signed(
    consent: &Value,
    credentials: &[Credential],
    verified: bool,
) -> Result<(Statement, String), &'static str> {
    let consent: Consent =
        serde_json::from_value(consent.clone()).map_err(|_| "the decision carries no consent")?;
    let challenge = consent.statement.challenge()?;
    let credential = credentials
        .iter()
        .find(|c| c.id == consent.assertion.credential_id)
        .ok_or("the consent is not signed by a passkey of the actor")?;
    let checked = if verified {
        calimero_identity::verify_consent(&challenge, credential, &consent.assertion)
    } else {
        calimero_identity::verify_sign_in(&challenge, credential, &consent.assertion).map(|_| ())
    };
    checked.map_err(|_| "the consent does not verify")?;
    Ok((consent.statement, used(b"consent", &challenge)))
}

/// Check that `proof` is the operator's signature over its own statement,
/// and answer the statement and what marks it used.
///
/// # Errors
/// A proof that is missing, malformed or not the operator's.
pub fn verify_operator(
    proof: &Value,
    key: Option<&OperatorKey>,
) -> Result<(Statement, String), &'static str> {
    let key = key.ok_or("no operator key is registered: the operator's decisions need one")?;
    let proof: OperatorProof = serde_json::from_value(proof.clone())
        .map_err(|_| "the operator's decision carries no signature")?;
    let public = hex::decode(&key.key).map_err(|_| "a malformed operator key")?;
    let signature = hex::decode(&proof.signature).map_err(|_| "a malformed operator signature")?;
    let message = proof.statement.operator_message(&key.cluster);
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(&message, &signature)
        .map_err(|_| "the operator's signature does not verify")?;
    Ok((proof.statement, used(b"operator", &message)))
}

/// What marks a proof used: a hash of what it signed.
fn used(kind: &[u8], signed: &[u8]) -> String {
    hex::encode(sha256(&[kind, b"\0", signed].concat()))
}

/// What an invite's key is derived from, before the token.
pub const INVITE_DOMAIN: &[u8] = b"calimero.invite.v1\0";

/// What a redemption's signature covers, before the invite and the account.
pub const INVITE_REDEEM_DOMAIN: &[u8] = b"calimero.invite-redeem.v1\0";

/// The Ed25519 key an invite token stands for: its seed is
/// `SHA-256("calimero.invite.v1\0" || token)`. The log keeps only the public
/// half ([`invite_key`]), so a node that sees the log, and not the token,
/// cannot redeem the invite.
#[must_use]
pub fn invite_signing_key(token: &str) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&sha256(&[INVITE_DOMAIN, token.as_bytes()].concat()))
}

/// An invite token's public key, hex: what `CreateInvite` records.
#[must_use]
pub fn invite_key(token: &str) -> String {
    hex::encode(invite_signing_key(token).verifying_key().to_bytes())
}

/// The bytes a redemption signs: `"calimero.invite-redeem.v1\0" || invite_key
/// || 0 || account || 0 || now` (the key hex, `now` in decimal Unix seconds).
#[must_use]
pub fn invite_redeem_message(invite_key: &str, account: &str, now: u64) -> Vec<u8> {
    let mut m = INVITE_REDEEM_DOMAIN.to_vec();
    m.extend_from_slice(invite_key.as_bytes());
    m.push(0);
    m.extend_from_slice(account.as_bytes());
    m.push(0);
    m.extend_from_slice(now.to_string().as_bytes());
    m
}

/// The redemption signature (hex) the node that received `token` makes for
/// `account` at `now`.
#[must_use]
pub fn sign_invite_redeem(token: &str, account: &str, now: u64) -> String {
    use ed25519_dalek::Signer;
    let message = invite_redeem_message(&invite_key(token), account, now);
    hex::encode(invite_signing_key(token).sign(&message).to_bytes())
}

/// What marks a redemption of `invite_key` for `account` at `now` used, as
/// [`verify_invite_redeem`] answers it for a signature that verifies.
#[must_use]
pub fn invite_redeem_used(invite_key: &str, account: &str, now: u64) -> String {
    used(b"invite", &invite_redeem_message(invite_key, account, now))
}

/// Check that `signature` redeems the invite whose public key is
/// `invite_key` for `account` at `now`, and answer what marks it used.
///
/// # Errors
/// A signature that is missing, malformed or not made with the invite's key.
pub fn verify_invite_redeem(
    invite_key: &str,
    account: &str,
    now: u64,
    signature: &str,
) -> Result<String, &'static str> {
    let public = hex::decode(invite_key).map_err(|_| "a malformed invite key")?;
    let signature = hex::decode(signature).map_err(|_| "the redemption carries no signature")?;
    let message = invite_redeem_message(invite_key, account, now);
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(&message, &signature)
        .map_err(|_| "the redemption is not signed with the invite's key")?;
    Ok(invite_redeem_used(invite_key, account, now))
}

/// Check that `signature` is the recovery key `public`'s, handing `org` to
/// `account` under `nonce`, and answer what marks it used.
///
/// # Errors
/// A malformed signature or nonce, or one the organization's key did not make.
pub fn verify_recovery(
    public: &str,
    org: &str,
    account: &str,
    nonce: &str,
    signature: &str,
) -> Result<String, &'static str> {
    use calimero_identity::recovery;
    recovery::verify_recovery(public, org, account, nonce, signature)
        .map_err(|_| "the recovery is not signed with the organization's recovery key")?;
    let message = recovery::recovery_message(org, account, nonce);
    Ok(used(
        b"recovery",
        &[public.as_bytes(), b"\0", &message].concat(),
    ))
}

/// What a refresh token's key is derived from, before the token.
pub const REFRESH_DOMAIN: &[u8] = b"calimero.refresh.v1\0";

/// What an exchange's signature covers, before the tokens it names.
pub const REFRESH_USE_DOMAIN: &[u8] = b"calimero.refresh-use.v1\0";

/// The Ed25519 key a refresh token stands for: its seed is
/// `SHA-256("calimero.refresh.v1\0" || token)`. The log keeps only the public
/// half ([`refresh_key`]), so a node that sees the log, and not the token,
/// cannot exchange it.
#[must_use]
pub fn refresh_signing_key(token: &str) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&sha256(&[REFRESH_DOMAIN, token.as_bytes()].concat()))
}

/// A refresh token's public key, hex: what its record keeps.
#[must_use]
pub fn refresh_key(token: &str) -> String {
    hex::encode(refresh_signing_key(token).verifying_key().to_bytes())
}

/// The bytes an exchange signs: `"calimero.refresh-use.v1\0"`, then
/// `old_hash`, `new_hash`, the new token's key, the session, the new expiry
/// and `now` (decimal Unix seconds), each followed by a zero byte.
#[must_use]
pub fn refresh_use_message(
    old_hash: &str,
    new_hash: &str,
    new_key: &str,
    session: &str,
    expires_at: u64,
    now: u64,
) -> Vec<u8> {
    let mut m = REFRESH_USE_DOMAIN.to_vec();
    for part in [
        old_hash,
        new_hash,
        new_key,
        session,
        &expires_at.to_string(),
        &now.to_string(),
    ] {
        m.extend_from_slice(part.as_bytes());
        m.push(0);
    }
    m
}

/// The exchange signature (hex) the node that received `token` makes.
#[must_use]
pub fn sign_refresh_use(token: &str, message: &[u8]) -> String {
    use ed25519_dalek::Signer;
    hex::encode(refresh_signing_key(token).sign(message).to_bytes())
}

/// Check that `signature` is the key `key` (a token's [`refresh_key`]) over
/// `message`.
///
/// # Errors
/// A key or signature that is missing, malformed or not the token's.
pub fn verify_refresh_use(key: &str, message: &[u8], signature: &str) -> Result<(), &'static str> {
    let public = hex::decode(key).map_err(|_| "the refresh token has no key: sign in again")?;
    let signature = hex::decode(signature).map_err(|_| "the exchange carries no signature")?;
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(message, &signature)
        .map_err(|_| "the exchange is not signed with the refresh token's key")
}

/// What an account recovery code's key is derived from, before the code.
pub const RECOVERY_CODE_DOMAIN: &[u8] = b"calimero.account-recovery-code.v1\0";

/// What an account recovery's start signature covers, before the key, the
/// account and the passkey it would add (organization recovery signs under
/// its own domain, [`calimero_identity::recovery::RECOVERY_DOMAIN`]).
pub const RECOVER_DOMAIN: &[u8] = b"calimero.account-recovery-start.v1\0";

/// The Ed25519 key a recovery code stands for, as invites do
/// ([`invite_signing_key`]): its seed is
/// `SHA-256("calimero.account-recovery-code.v1\0" || canonical code)` ([`calimero_identity::normalize_recovery_code`]). The log
/// keeps only the public half ([`recovery_key`]), so neither the code nor
/// anything that redeems it is ever logged.
#[must_use]
pub fn recovery_signing_key(code: &str) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&sha256(
        &[RECOVERY_CODE_DOMAIN, code.as_bytes()].concat(),
    ))
}

/// A recovery code's public key, hex: what `SetAccountRecoveryKey` records.
#[must_use]
pub fn recovery_key(code: &str) -> String {
    hex::encode(recovery_signing_key(code).verifying_key().to_bytes())
}

/// The bytes an account recovery's start signs:
/// `"calimero.account-recovery-start.v1\0" || key || 0 || account || 0 ||
/// credential id || 0 || SHA-256(credential public key JSON) || 0 || at`. It
/// names the passkey the recovery adds, so a signature cannot add another.
#[must_use]
pub fn recover_message(key: &str, account: &str, credential: &Credential, now: u64) -> Vec<u8> {
    let public = serde_json::to_vec(&credential.public_key).unwrap_or_default();
    let mut m = RECOVER_DOMAIN.to_vec();
    for part in [key.as_bytes(), account.as_bytes(), credential.id.as_bytes()] {
        m.extend_from_slice(part);
        m.push(0);
    }
    m.extend_from_slice(&sha256(&public));
    m.push(0);
    m.extend_from_slice(now.to_string().as_bytes());
    m
}

/// The start signature (hex) the node the person gave `code` to makes,
/// for adding `credential` to `account`, started at `now`.
#[must_use]
pub fn sign_recover(code: &str, account: &str, credential: &Credential, now: u64) -> String {
    use ed25519_dalek::Signer;
    let message = recover_message(&recovery_key(code), account, credential, now);
    hex::encode(recovery_signing_key(code).sign(&message).to_bytes())
}

/// Check that `signature` starts recovering `account` with `credential` at
/// `now` under the recovery key `key`, and answer what marks it used.
///
/// # Errors
/// A signature that is missing, malformed or not made with the code's key.
pub fn verify_recover(
    key: &str,
    account: &str,
    credential: &Credential,
    now: u64,
    signature: &str,
) -> Result<String, &'static str> {
    let public = hex::decode(key).map_err(|_| "a malformed recovery key")?;
    let signature = hex::decode(signature).map_err(|_| "the recovery carries no signature")?;
    let message = recover_message(key, account, credential, now);
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(&message, &signature)
        .map_err(|_| "the recovery is not signed with the code's key")?;
    Ok(used(b"recover", &message))
}

/// What an upgrade key's Ed25519 key is derived from, before the API key.
pub const API_KEY_DOMAIN: &[u8] = b"calimero.api-key.v1\0";

/// What an upgrade key's signature covers, before the statement.
pub const API_KEY_UPGRADE_DOMAIN: &[u8] = b"calimero.api-key-upgrade.v1\0";

/// The Ed25519 key an API key (`cal_…`) stands for: its seed is
/// `SHA-256("calimero.api-key.v1\0" || key)`. The log keeps only the public
/// half ([`api_key_public`]) and the key's hash, so a node that sees the log,
/// and not the key, cannot sign an upgrade with it.
#[must_use]
pub fn api_key_signing_key(key: &str) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&sha256(&[API_KEY_DOMAIN, key.as_bytes()].concat()))
}

/// An API key's public key, hex: what an upgrade key's scope records.
#[must_use]
pub fn api_key_public(key: &str) -> String {
    hex::encode(api_key_signing_key(key).verifying_key().to_bytes())
}

/// An upgrade key's proof over a statement, as the log carries it in
/// `UpgradeApp::consent`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyProof {
    /// The key's id (`ApiKeyRecord::key_id`).
    pub api_key: String,
    #[serde(flatten)]
    pub statement: Statement,
    /// Ed25519, hex.
    pub signature: String,
}

impl Statement {
    /// The bytes an upgrade key signs for it: the domain, the key's id, then
    /// each part, each followed by a zero byte.
    #[must_use]
    pub fn api_key_message(&self, key_id: &str) -> Vec<u8> {
        let mut m = API_KEY_UPGRADE_DOMAIN.to_vec();
        for part in [key_id, &self.subject, &self.method, &self.args, &self.nonce] {
            m.extend_from_slice(part.as_bytes());
            m.push(0);
        }
        m
    }
}

/// The proof the node that received `key` (id `key_id`) makes over
/// `statement`.
#[must_use]
pub fn sign_key_upgrade(key: &str, key_id: &str, statement: Statement) -> KeyProof {
    use ed25519_dalek::Signer;
    let signature = api_key_signing_key(key).sign(&statement.api_key_message(key_id));
    KeyProof {
        api_key: key_id.to_owned(),
        statement,
        signature: hex::encode(signature.to_bytes()),
    }
}

/// Check that `proof` is signed by the key whose public half is `public`,
/// and answer its key id, its statement and what marks it used.
///
/// # Errors
/// A proof that is malformed or not signed with that key.
pub fn verify_key_upgrade(
    proof: &Value,
    public: &str,
) -> Result<(String, Statement, String), &'static str> {
    let proof: KeyProof = serde_json::from_value(proof.clone())
        .map_err(|_| "the upgrade carries no API key signature")?;
    let public = hex::decode(public).map_err(|_| "a malformed API key proof key")?;
    let signature = hex::decode(&proof.signature).map_err(|_| "a malformed API key signature")?;
    let message = proof.statement.api_key_message(&proof.api_key);
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(&message, &signature)
        .map_err(|_| "the upgrade is not signed with the API key")?;
    Ok((proof.api_key, proof.statement, used(b"api-key", &message)))
}

/// The key id a [`KeyProof`] names, before it is verified.
#[must_use]
pub fn key_proof_id(proof: &Value) -> Option<&str> {
    proof.get("apiKey").and_then(Value::as_str)
}
