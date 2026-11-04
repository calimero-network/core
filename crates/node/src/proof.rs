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
