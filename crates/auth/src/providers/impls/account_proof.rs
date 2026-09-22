//! Login by account device key, for a holder who runs no node.
//!
//! # Why this exists
//!
//! [`super::user_password`] authenticates against a root key provisioned on the
//! node out of band. That works for the person who owns the node and for nobody
//! else. An account holder reaching a relay they do not own has no credential
//! there and cannot be given one: a relay serves several tenants, and
//! provisioning a password per tenant per relay is the model this replaces.
//!
//! What such a holder does have is an account root, a device certified by it,
//! and an [`AccountProof`] tying the two together — self-contained, verifiable
//! with no prior state. This provider turns that into a session.
//!
//! # What a login proves, and in what order
//!
//! Four independent things, each of which is a different attack if skipped:
//!
//! 1. **The challenge is ours and fresh** — [`ChallengeMinter::verify`]. Checked
//!    first because it is cheap and rules out replay before any signature work.
//! 2. **The statement is addressed to this node and this surface** —
//!    [`LoginStatement::addressed_to`]. Without the node check, a hostile relay
//!    could fetch a challenge here, serve it to a user as its own, and replay the
//!    signed result against this node as that user.
//! 3. **The device signed the statement** — [`LoginStatement::verify_signature`].
//! 4. **That device belongs to the account, and the account is the one named** —
//!    [`AccountProof::verify`], plus an explicit check that the certificate is
//!    *about* the key that signed. A proof for one of the account's other devices
//!    verifies perfectly and would otherwise vouch for a key the statement never
//!    used; this is the same failure `Delegation::verify` guards against, and it
//!    is the one that looks like success.
//!
//! Only then is the challenge spent, so a failed or forged attempt cannot burn a
//! challenge its rightful holder is still using.
//!
//! # What a session from here does NOT establish
//!
//! Membership. The token's subject is the account and nothing more; whether that
//! account may read or write any particular context is a question about a causal
//! cut, which this service does not see. It must be answered per request, at the
//! node, against the target context's group — never cached at login, because one
//! relay serves several tenants and a session must not carry a standing right to
//! read. Every scope in [`AccountProofConfig::session_permissions`] is therefore
//! gated a second time at the node: a write by the warrant and
//! `CAN_AUTHOR_ON_BEHALF`, a read and a subscription by a per-call membership
//! check, and the two caller-scoped listings by the caller's groups resolved per
//! request in `calimero-server`'s `admin/caller_scope.rs`. A session from here
//! decides who may ASK, never what the answer contains.

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use calimero_account::{AccountId, AccountProof, Audience, DeviceCert, LoginStatement};
use calimero_primitives::identity::{DeviceId, PublicKey};
use eyre::{bail, eyre, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use crate::api::handlers::auth::TokenRequest;
use crate::auth::challenge::{ChallengeMinter, CHALLENGE_LEN};
use crate::config::{AccountProofConfig, AuthConfig};
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::providers::ProviderContext;
use crate::storage::models::Key;
use crate::storage::{KeyManager, Storage};
use crate::{register_auth_data_type, register_auth_provider, AuthResponse};

/// The auth method this provider answers to.
///
/// Public because the auth layer classifies a stored key by the provider that
/// minted it: a record carrying this method names an ACCOUNT, and its caller
/// must be identified as that account rather than as the node owner. Sharing
/// one constant is what stops the two sides drifting apart silently.
pub const METHOD: &str = "account_proof";

/// The config spelling of an audience, for matching against `allowed_audiences`.
///
/// Rendered here rather than on [`Audience`] itself: this is a config-matching
/// concern, and widening `calimero-account`'s API for it would make a display
/// form look like part of the credential model.
fn audience_label(audience: &Audience) -> String {
    match audience {
        Audience::WebOrigin(origin) => origin.clone(),
        Audience::CodeSigningId(id) => format!("codesign:{id}"),
        Audience::Cli => "cli".to_owned(),
    }
}

/// Decode a hex string into a borsh-decoded `T`.
///
/// Hex-of-borsh, never JSON: the canonical form of a signed structure is its
/// borsh encoding, and re-describing its fields as JSON would create a second
/// spelling that could disagree with the bytes the signature covers.
fn from_hex_borsh<T: borsh::BorshDeserialize>(what: &str, raw: &str) -> Result<T> {
    let bytes = hex::decode(raw).map_err(|err| eyre!("{what} is not valid hex: {err}"))?;
    borsh::from_slice(&bytes).map_err(|err| eyre!("{what} is not a valid encoding: {err}"))
}

/// The provider-specific half of a token request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProofRequest {
    /// Hex of the 32 challenge bytes this node issued.
    pub challenge: String,
    /// Hex borsh of the [`LoginStatement`].
    pub login_statement: String,
    /// Hex borsh of the [`AccountProof<DeviceCert>`].
    pub account_proof: String,
}

/// The parsed auth data the verifier runs against.
///
/// `session_key` is carried here rather than read from the statement so the two
/// can be *compared*: the key the JWT will be minted for is the token request's
/// `public_key`, and a statement naming a different one must not mint a session
/// for a key its signer never saw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProofAuthData {
    /// Hex of the challenge.
    pub challenge: String,
    /// Hex borsh of the login statement.
    pub login_statement: String,
    /// Hex borsh of the account proof.
    pub account_proof: String,
    /// The public key the token request asked to bind the session to.
    pub session_key: String,
}

/// Registry entry for [`AccountProofAuthData`].
pub struct AccountProofAuthDataType;

impl AuthDataType for AccountProofAuthDataType {
    fn method_name(&self) -> &str {
        METHOD
    }

    fn parse_from_value(&self, value: Value) -> Result<Box<dyn Any + Send + Sync>> {
        serde_json::from_value::<AccountProofAuthData>(value)
            .map(|data| Box::new(data) as Box<dyn Any + Send + Sync>)
            .map_err(|err| eyre!("Invalid account-proof auth data: {err}"))
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "challenge": "<64 hex chars>",
            "login_statement": "<hex borsh of LoginStatement>",
            "account_proof": "<hex borsh of AccountProof<DeviceCert>>",
            "session_key": "<hex of the session public key>",
        })
    }
}

/// Authenticates an account holder by a device key it controls.
pub struct AccountProofProvider {
    config: AccountProofConfig,
    /// This node's identity, parsed once at construction so a malformed value
    /// fails at startup rather than on every login.
    node_key: PublicKey,
    challenges: Arc<ChallengeMinter>,
    /// Where the account's key record lives.
    ///
    /// Needed because a verified proof is not enough on its own: both
    /// `generate_token_pair` and `/auth/validate` look the subject up here and
    /// fail closed when it is absent, so an account with no record authenticates
    /// and then cannot be issued a token.
    key_manager: KeyManager,
}

impl Clone for AccountProofProvider {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            node_key: self.node_key,
            challenges: Arc::clone(&self.challenges),
            key_manager: self.key_manager.clone(),
        }
    }
}

impl AccountProofProvider {
    /// Build a provider, refusing a configuration it cannot enforce.
    ///
    /// # Errors
    /// If `node_key` is absent or not a valid key. Both are fail-closed on
    /// purpose: a provider that cannot check which node a statement was minted
    /// for would accept statements minted for any node, which is the one failure
    /// this check exists to prevent.
    pub fn new(
        storage: Arc<dyn Storage>,
        key_manager: KeyManager,
        config: AccountProofConfig,
    ) -> Result<Self> {
        let raw = config.node_key.as_deref().ok_or_else(|| {
            eyre!(
                "the {METHOD} provider is enabled but auth.account_proof.node_key is unset; \
                 without this node's own identity it cannot tell a login statement minted \
                 for it from one minted for another node"
            )
        })?;
        let node_key: PublicKey = raw
            .parse()
            .map_err(|err| eyre!("auth.account_proof.node_key is not a valid public key: {err}"))?;

        if config.allowed_audiences.is_empty() {
            warn!(
                "the {METHOD} provider accepts ANY audience because \
                 auth.account_proof.allowed_audiences is empty; set it on a node that serves \
                 more than one client surface"
            );
        }

        let challenges = Arc::new(ChallengeMinter::new(storage, config.challenge_ttl_secs));

        Ok(Self {
            config,
            node_key,
            challenges,
            key_manager,
        })
    }

    /// Make sure this account has a key record, minting one on first sight.
    ///
    /// Unlike `user_password`, this provider has no registration step to hang a
    /// key off: a keyholder's whole premise is that it has no prior relationship
    /// with this node. Possession of the account root, proven against a
    /// challenge this node minted, IS the registration — so the first successful
    /// proof creates the record and later ones reuse it.
    ///
    /// Registering on first authentication means a successful proof writes a
    /// record, so a caller minting account roots offline could grow the keystore
    /// one tiny row at a time. The login rate limiter in `token_handler` is what
    /// bounds that, as it does for every other login path; nothing here adds a
    /// second quota on top of it.
    ///
    /// **A revoked record is never resurrected.** The lookup deliberately
    /// includes invalid keys: `get_key` hides them, so checking with that would
    /// see "absent", mint a fresh valid record, and silently undo the
    /// revocation. An operator who revokes an account's key needs that to stay
    /// revoked across the account's next login attempt, which is the only moment
    /// it matters.
    async fn ensure_account_key(&self, key_id: &str) -> Result<()> {
        match self.key_manager.get_key_including_invalid(key_id).await {
            Ok(Some(key)) => {
                if !key.is_valid() {
                    bail!("this account's key has been revoked on this node");
                }
                if !key.is_root_key() {
                    bail!("account {key_id} is registered here as a non-root key");
                }
                // Left exactly as stored. The minted token carries the
                // operator's CURRENT `session_permissions` either way, and
                // rewriting the record on every login would churn its metadata
                // for no gain.
                Ok(())
            }
            Ok(None) => {
                debug!(account = %key_id, "minting the key record for a first-time keyholder");
                let key = Key::new_root_key_with_permissions(
                    // The account is its own public-key index entry: it is
                    // derived from the root signing key and names nothing else.
                    key_id.to_owned(),
                    METHOD.to_owned(),
                    self.config.session_permissions.clone(),
                    // Deliberately unset. Neither the token path nor
                    // `/auth/validate` resolves this record per node, and a
                    // node-scoped record would strand the account the first time
                    // the node is reached through a different URL.
                    None,
                );
                self.key_manager
                    .set_key(key_id, &key)
                    .await
                    .map_err(|err| eyre!("could not record this account on the node: {err}"))?;
                Ok(())
            }
            Err(err) => bail!("could not read this account's key record: {err}"),
        }
    }

    /// Whether `audience` is one the operator permits.
    fn audience_allowed(&self, audience: &Audience) -> bool {
        self.config.allowed_audiences.is_empty()
            || self
                .config
                .allowed_audiences
                .iter()
                .any(|allowed| *allowed == audience_label(audience))
    }

    /// Run the whole exchange, returning the account the session belongs to and
    /// the device that asked for it.
    ///
    /// Both, because they answer different questions downstream: governance
    /// rows are keyed by ACCOUNT, and revocation is a per-DEVICE row. A caller
    /// given only the account cannot check whether the key that just
    /// authenticated has since been withdrawn.
    async fn authenticate_core(
        &self,
        data: &AccountProofAuthData,
    ) -> Result<(AccountId, DeviceId)> {
        let challenge_bytes = hex::decode(&data.challenge)
            .map_err(|err| eyre!("challenge is not valid hex: {err}"))?;
        let challenge: [u8; CHALLENGE_LEN] = challenge_bytes
            .try_into()
            .map_err(|_ignored| eyre!("challenge must be {CHALLENGE_LEN} bytes"))?;

        // 1. Ours, and fresh. Cheapest check, and it rules out replay before any
        //    signature work — which is also what keeps this path from being a
        //    free way to make the node do Ed25519 verifications.
        self.challenges.verify(&challenge).await?;

        let statement: LoginStatement = from_hex_borsh("login statement", &data.login_statement)?;
        let proof: AccountProof<DeviceCert> = from_hex_borsh("account proof", &data.account_proof)?;

        // The challenge must be the one this request presented, not merely *a*
        // valid one: otherwise a caller could pair a fresh challenge with a
        // statement signed over an old one and never be bound to either.
        if statement.challenge != challenge {
            bail!("login statement does not cover the challenge it was sent with");
        }

        // 2. Addressed here, from a surface we serve.
        if !self.audience_allowed(&statement.audience) {
            bail!(
                "audience {} is not accepted by this node",
                audience_label(&statement.audience)
            );
        }
        statement.addressed_to(&self.node_key, &statement.audience.clone())?;

        // The session is minted for the key the request names, so the statement
        // has to have signed over that same key. Without this the JWT could be
        // bound to a key the device never authorized.
        let requested: PublicKey = data
            .session_key
            .parse()
            .map_err(|err| eyre!("session key is not a valid public key: {err}"))?;
        if statement.session_key != requested {
            bail!("login statement authorizes a different session key than the one requested");
        }

        if statement.expires_at <= now_secs() {
            bail!("login statement has expired");
        }

        // 3. The device signed it.
        statement.verify_signature()?;

        // 4. The device belongs to the account the proof names — and the
        //    certificate is about the key that actually signed. A proof for
        //    another of the account's devices verifies perfectly, which is why
        //    the second half is explicit.
        let account = proof.statement.account;
        let verified = proof.verify(account)?;
        if verified.sign_pk != statement.device_key {
            bail!("the account proof certifies a different device than the one that signed");
        }

        // Only now is the challenge spent: a forged attempt must not be able to
        // burn a challenge its rightful holder is still signing over.
        self.challenges.redeem(&challenge).await?;

        debug!(%account, "account authenticated by device key");
        Ok((account, verified.device))
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Verifier for one account-proof login.
struct AccountProofVerifier {
    provider: Arc<AccountProofProvider>,
    auth_data: AccountProofAuthData,
}

#[async_trait]
impl AuthVerifierFn for AccountProofVerifier {
    async fn verify(&self) -> Result<AuthResponse> {
        let (account, device) = self.provider.authenticate_core(&self.auth_data).await?;
        let key_id = account.to_string();

        // A verified proof is not the whole job: the subject has to EXIST as a
        // key record. `generate_token_pair` and `/auth/validate` both resolve
        // `key_id` through the key manager and fail closed when it is missing,
        // so returning a subject with no record authenticates successfully and
        // then dies one line later as "Failed to generate tokens" — a 500 that
        // names nothing, which is exactly how this shipped.
        self.provider.ensure_account_key(&key_id).await?;

        Ok(AuthResponse {
            is_valid: true,
            // The subject is the ACCOUNT, not the device. This is what reaches
            // handlers as `X-Auth-User`, and it is the only identity governance
            // rows are keyed by.
            key_id,
            permissions: self.provider.config.session_permissions.clone(),
            // The device, beside the account, because revocation is a
            // per-device row. The subject stays the account — that is what
            // governance keys on — but a session that cannot name its device is
            // one revocation cannot reach, and this provider is the only one
            // that knows which device asked.
            device: Some(hex::encode(device.as_bytes())),
        })
    }
}

#[async_trait]
impl AuthProvider for AccountProofProvider {
    fn name(&self) -> &str {
        METHOD
    }

    fn provider_type(&self) -> &str {
        "credentials"
    }

    fn description(&self) -> &str {
        "Authenticates an account by a device key certified under its root"
    }

    fn supports_method(&self, method: &str) -> bool {
        method == METHOD
    }

    fn is_configured(&self) -> bool {
        // `new` refuses to build without a node key, so an existing instance is
        // configured by construction.
        true
    }

    async fn is_configured_with_users(&self) -> Result<bool> {
        // There are no users to provision: any account holder with a certified
        // device can log in, which is the entire point. Reporting "configured"
        // is the honest answer — the provider is ready.
        Ok(true)
    }

    fn get_config_options(&self) -> Value {
        serde_json::json!({
            "enabled": true,
            "description": "Device-key login for accounts that run no node",
            "challenge_ttl_secs": self.config.challenge_ttl_secs,
            "allowed_audiences": self.config.allowed_audiences,
        })
    }

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> Result<Value> {
        let request: AccountProofRequest =
            serde_json::from_value(token_request.provider_data.clone())
                .map_err(|err| eyre!("Invalid account-proof data: {err}"))?;

        Ok(serde_json::json!({
            "challenge": request.challenge,
            "login_statement": request.login_statement,
            "account_proof": request.account_proof,
            // Carried across so the verifier can compare it against the key the
            // statement signed over.
            "session_key": token_request.public_key,
        }))
    }

    fn create_verifier(
        &self,
        method: &str,
        auth_data: Box<dyn Any + Send + Sync>,
    ) -> Result<AuthRequestVerifier> {
        if !self.supports_method(method) {
            return Err(eyre!(
                "Provider {} does not support method {method}",
                self.name()
            ));
        }

        let data = auth_data
            .downcast_ref::<AccountProofAuthData>()
            .ok_or_else(|| eyre!("Failed to parse account-proof auth data"))?;

        Ok(AuthRequestVerifier::new(AccountProofVerifier {
            provider: Arc::new(self.clone()),
            auth_data: data.clone(),
        }))
    }

    fn verify_request(&self, _request: &Request<Body>) -> Result<AuthRequestVerifier> {
        // Deliberately unsupported. The credential is three hex-borsh structures
        // that do not belong in headers, and a header-shaped variant would be a
        // second spelling of the same exchange with its own bugs. Clients use
        // `POST /auth/token`.
        Err(eyre!(
            "the {METHOD} provider authenticates through POST /auth/token, not request headers"
        ))
    }

    async fn create_root_key(
        &self,
        _public_key: &str,
        _auth_method: &str,
        _provider_data: Value,
        _node_url: Option<&str>,
    ) -> Result<bool> {
        // There is no root key to mint: the account IS the identity, anchored by
        // its own genesis, and this service stores nothing about it. Minting one
        // here would create a second, node-local notion of who the account is,
        // which could then disagree with the credential.
        bail!("the {METHOD} provider does not mint root keys; the account is its own anchor")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Registration for the account-proof provider.
pub struct AccountProofProviderRegistration;

impl ProviderRegistration for AccountProofProviderRegistration {
    fn provider_id(&self) -> &str {
        METHOD
    }

    fn create_provider(&self, context: ProviderContext) -> Result<Box<dyn AuthProvider>> {
        let config = context.config.account_proof.clone();
        Ok(Box::new(AccountProofProvider::new(
            context.storage,
            context.key_manager,
            config,
        )?))
    }

    fn is_enabled(&self, config: &AuthConfig) -> bool {
        config.providers.get(METHOD).copied().unwrap_or(false)
    }
}

register_auth_provider!(AccountProofProviderRegistration);
register_auth_data_type!(AccountProofAuthDataType);

#[cfg(test)]
mod tests {
    use calimero_account::{AccountGenesis, DeviceId, KemPublicKey};
    use calimero_primitives::identity::PrivateKey;

    use super::*;
    use crate::storage::MemoryStorage;

    fn key(seed: u8) -> PrivateKey {
        PrivateKey::from([seed; 32])
    }

    const NODE_SEED: u8 = 0x9e;

    /// An account whose device `device_sk` is certified under root `root_sk`.
    fn account_with_device(
        root_sk: &PrivateKey,
        device_sk: &PrivateKey,
    ) -> AccountProof<DeviceCert> {
        let genesis = AccountGenesis::new(root_sk.public_key());
        let account = genesis.account_id();
        let device = DeviceId::mint(account, [0xa1; 16]);
        let cert = DeviceCert::sign(
            root_sk,
            account,
            device,
            &device_sk.public_key(),
            &KemPublicKey::from([0x3a; 32]),
            0,
            0,
        )
        .expect("sign cert");
        AccountProof {
            genesis,
            chain: Vec::new(),
            statement: cert,
        }
    }

    fn provider(storage: Arc<dyn Storage>) -> AccountProofProvider {
        AccountProofProvider::new(
            Arc::clone(&storage),
            KeyManager::new(storage),
            AccountProofConfig {
                node_key: Some(key(NODE_SEED).public_key().to_string()),
                ..AccountProofConfig::default()
            },
        )
        .expect("the provider must build with a node key")
    }

    /// A full, valid login: challenge, statement, proof, session key.
    async fn valid_login(
        p: &AccountProofProvider,
        root_sk: &PrivateKey,
        device_sk: &PrivateKey,
        session_sk: &PrivateKey,
    ) -> AccountProofAuthData {
        let challenge = p.challenges.issue().await.expect("issue").bytes;
        let statement = LoginStatement::sign(
            device_sk,
            key(NODE_SEED).public_key(),
            Audience::Cli,
            challenge,
            session_sk.public_key(),
            now_secs(),
            now_secs() + 300,
        )
        .expect("sign statement");

        AccountProofAuthData {
            challenge: hex::encode(challenge),
            login_statement: hex::encode(borsh::to_vec(&statement).expect("borsh")),
            account_proof: hex::encode(
                borsh::to_vec(&account_with_device(root_sk, device_sk)).expect("borsh"),
            ),
            session_key: session_sk.public_key().to_string(),
        }
    }

    // --- the criterion: a device key obtains a session, with no password ----

    #[tokio::test]
    async fn a_certified_device_authenticates_as_its_account() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device_key, session) = (key(1), key(2), key(3));
        let data = valid_login(&p, &root, &device_key, &session).await;

        let (account, device) = p
            .authenticate_core(&data)
            .await
            .expect("a certified device must authenticate");

        assert_eq!(
            account,
            AccountGenesis::new(root.public_key()).account_id(),
            "the session belongs to the ACCOUNT, not the device"
        );
        // And it knows WHICH device, which is what lets revocation reach a
        // session at all: revocation is a per-device row, so an account alone
        // cannot be checked against one.
        assert_eq!(
            device,
            account_with_device(&root, &device_key).statement.device,
            "the session must name the device whose key authenticated"
        );
    }

    /// The token subject is what reaches handlers as `X-Auth-User`, and it must
    /// be the ACCOUNT — `TokenManager` puts `key_id` straight into `sub`, and
    /// `validate_handler` copies `sub` into the header. Pinned here because that
    /// chain is three hops of convention with nothing else asserting it.
    #[tokio::test]
    async fn the_session_subject_is_the_account_and_carries_the_configured_scope() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));
        let data = valid_login(&p, &root, &device, &session).await;

        let verifier = p
            .create_verifier(METHOD, Box::new(data))
            .expect("create verifier");
        let response = verifier.verify().await.expect("verify");

        assert!(response.is_valid);
        assert_eq!(
            response.key_id,
            AccountGenesis::new(root.public_key())
                .account_id()
                .to_string(),
            "the subject must be the account, not the device and not a stored key id"
        );
        // The configured scope, verbatim: a minted session carries what the
        // operator set and nothing the provider added on its own.
        assert_eq!(
            response.permissions,
            vec![
                "context:intent".to_owned(),
                "context:list-own".to_owned(),
                "context:query".to_owned(),
                "context:subscribe".to_owned(),
                "namespace:list-own".to_owned()
            ]
        );
    }

    // --- the criterion: a device not certified by the account is refused ----

    /// The signature is valid and the proof is genuine — they are simply about
    /// different accounts. This is the failure that looks like success.
    #[tokio::test]
    async fn a_device_certified_by_another_account_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (attacker_device, session) = (key(0x40), key(3));

        let challenge = p.challenges.issue().await.expect("issue").bytes;
        // Signed by a device that no account certified.
        let statement = LoginStatement::sign(
            &attacker_device,
            key(NODE_SEED).public_key(),
            Audience::Cli,
            challenge,
            session.public_key(),
            now_secs(),
            now_secs() + 300,
        )
        .expect("sign");

        // ...presented with a perfectly valid proof for somebody else's device.
        let victim_proof = account_with_device(&key(1), &key(2));

        let data = AccountProofAuthData {
            challenge: hex::encode(challenge),
            login_statement: hex::encode(borsh::to_vec(&statement).expect("borsh")),
            account_proof: hex::encode(borsh::to_vec(&victim_proof).expect("borsh")),
            session_key: session.public_key().to_string(),
        };

        let err = p
            .authenticate_core(&data)
            .await
            .expect_err("an uncertified device must be refused");
        assert!(
            err.to_string().contains("different device"),
            "expected the cert/signer mismatch, got: {err}"
        );
    }

    // --- the criterion: a challenge is single-use and expires ---------------

    #[tokio::test]
    async fn a_replayed_challenge_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));
        let data = valid_login(&p, &root, &device, &session).await;

        p.authenticate_core(&data).await.expect("first login");
        let err = p
            .authenticate_core(&data)
            .await
            .expect_err("the same challenge must not authenticate twice");
        assert!(
            err.to_string().contains("already been used"),
            "expected a replay refusal, got: {err}"
        );
    }

    #[tokio::test]
    async fn a_challenge_this_node_did_not_issue_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));
        let mut data = valid_login(&p, &root, &device, &session).await;
        data.challenge = hex::encode([0u8; CHALLENGE_LEN]);

        let err = p.authenticate_core(&data).await.expect_err("forged");
        assert!(
            err.to_string().contains("not issued by this node"),
            "got: {err}"
        );
    }

    /// A failed attempt must not burn the challenge — otherwise anyone able to
    /// see one in flight could invalidate it before its holder finishes.
    #[tokio::test]
    async fn a_failed_attempt_leaves_the_challenge_spendable() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));
        let good = valid_login(&p, &root, &device, &session).await;

        let mut bad = good.clone();
        bad.session_key = key(0x55).public_key().to_string();
        assert!(p.authenticate_core(&bad).await.is_err());

        assert!(
            p.authenticate_core(&good).await.is_ok(),
            "a failed attempt must not spend the challenge"
        );
    }

    // --- the substitutions the statement's own fields refuse ----------------

    /// The attack the `node` field exists for: a hostile relay fetches a
    /// challenge here, serves it to a user as its own, and replays the result.
    #[tokio::test]
    async fn a_statement_minted_for_another_node_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));

        let challenge = p.challenges.issue().await.expect("issue").bytes;
        let statement = LoginStatement::sign(
            &device,
            key(0xbb).public_key(), // a DIFFERENT node
            Audience::Cli,
            challenge,
            session.public_key(),
            now_secs(),
            now_secs() + 300,
        )
        .expect("sign");

        let data = AccountProofAuthData {
            challenge: hex::encode(challenge),
            login_statement: hex::encode(borsh::to_vec(&statement).expect("borsh")),
            account_proof: hex::encode(
                borsh::to_vec(&account_with_device(&root, &device)).expect("borsh"),
            ),
            session_key: session.public_key().to_string(),
        };

        let err = p.authenticate_core(&data).await.expect_err("wrong node");
        assert!(err.to_string().contains("different node"), "got: {err}");
    }

    /// The session must be minted for the key the device actually signed over.
    #[tokio::test]
    async fn a_session_key_the_statement_did_not_cover_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));
        let mut data = valid_login(&p, &root, &device, &session).await;
        data.session_key = key(0x55).public_key().to_string();

        let err = p.authenticate_core(&data).await.expect_err("key swap");
        assert!(
            err.to_string().contains("different session key"),
            "got: {err}"
        );
    }

    /// Pairing a fresh challenge with a statement signed over an old one must
    /// not pass: otherwise the statement is bound to neither.
    #[tokio::test]
    async fn a_statement_covering_another_challenge_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));
        let data = valid_login(&p, &root, &device, &session).await;

        let fresh = p.challenges.issue().await.expect("issue").bytes;
        let mismatched = AccountProofAuthData {
            challenge: hex::encode(fresh),
            ..data
        };

        let err = p
            .authenticate_core(&mismatched)
            .await
            .expect_err("mismatch");
        assert!(
            err.to_string().contains("does not cover the challenge"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn an_expired_statement_is_refused() {
        let p = provider(Arc::new(MemoryStorage::new()));
        let (root, device, session) = (key(1), key(2), key(3));

        let challenge = p.challenges.issue().await.expect("issue").bytes;
        let statement = LoginStatement::sign(
            &device,
            key(NODE_SEED).public_key(),
            Audience::Cli,
            challenge,
            session.public_key(),
            0,
            1, // expired in 1970
        )
        .expect("sign");

        let data = AccountProofAuthData {
            challenge: hex::encode(challenge),
            login_statement: hex::encode(borsh::to_vec(&statement).expect("borsh")),
            account_proof: hex::encode(
                borsh::to_vec(&account_with_device(&root, &device)).expect("borsh"),
            ),
            session_key: session.public_key().to_string(),
        };

        let err = p.authenticate_core(&data).await.expect_err("expired");
        assert!(err.to_string().contains("expired"), "got: {err}");
    }

    // --- the key record the rest of the auth stack resolves -----------------

    /// The regression this file exists to prevent recurring.
    ///
    /// Verification used to return the account as `key_id` and stop there. Both
    /// `generate_token_pair` and `/auth/validate` then resolve that subject
    /// through the key manager and fail closed when it is missing, so a
    /// perfectly valid proof authenticated and died one line later with
    /// "Failed to generate tokens" — a 500 naming nothing.
    ///
    /// The older test above asserts the RESPONSE and passed throughout. Nothing
    /// asserted the precondition the next stage requires, which is why this
    /// reached CI.
    #[tokio::test]
    async fn a_first_login_leaves_a_key_record_the_token_path_can_resolve() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let keys = KeyManager::new(Arc::clone(&storage));
        let p = provider(storage);
        let (root, device, session) = (key(1), key(2), key(3));
        let data = valid_login(&p, &root, &device, &session).await;
        let account = AccountGenesis::new(root.public_key())
            .account_id()
            .to_string();

        assert!(
            keys.get_key(&account).await.expect("read").is_none(),
            "precondition: a keyholder has no relationship with this node yet"
        );

        let verifier = p.create_verifier(METHOD, Box::new(data)).expect("verifier");
        let response = verifier.verify().await.expect("verify");

        let stored = keys
            .get_key(&response.key_id)
            .await
            .expect("read")
            .expect("the subject must resolve to a key, or no token can be minted");
        assert!(stored.is_valid());
        assert!(
            stored.is_root_key(),
            "the account owns itself; it is not a client of anything"
        );
        assert_eq!(stored.auth_method.as_deref(), Some(METHOD));
        assert_eq!(
            stored.public_key.as_deref(),
            Some(account.as_str()),
            "the record is indexed by the account it names"
        );
    }

    /// Minting on first sight must not mean re-minting on every sight: a second
    /// login reuses the record rather than churning it.
    #[tokio::test]
    async fn a_repeat_login_reuses_the_existing_record() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let keys = KeyManager::new(Arc::clone(&storage));
        let p = provider(storage);
        let (root, device) = (key(1), key(2));

        for session_seed in [3, 4] {
            let data = valid_login(&p, &root, &device, &key(session_seed)).await;
            let verifier = p.create_verifier(METHOD, Box::new(data)).expect("verifier");
            verifier.verify().await.expect("verify");
        }

        let account = AccountGenesis::new(root.public_key())
            .account_id()
            .to_string();
        assert!(keys.get_key(&account).await.expect("read").is_some());
    }

    /// Revocation has to survive the account's next login, which is the only
    /// moment it matters. `get_key` hides invalid keys, so a mint-if-absent
    /// check written against it would see "absent" and silently un-revoke.
    #[tokio::test]
    async fn a_revoked_account_is_not_resurrected_by_logging_in_again() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let keys = KeyManager::new(Arc::clone(&storage));
        let p = provider(storage);
        let (root, device) = (key(1), key(2));
        let account = AccountGenesis::new(root.public_key())
            .account_id()
            .to_string();

        let data = valid_login(&p, &root, &device, &key(3)).await;
        let verifier = p.create_verifier(METHOD, Box::new(data)).expect("verifier");
        verifier.verify().await.expect("first login");

        let mut stored = keys.get_key(&account).await.expect("read").expect("minted");
        stored.revoke();
        keys.set_key(&account, &stored)
            .await
            .expect("store revoked");
        assert!(
            keys.get_key(&account).await.expect("read").is_none(),
            "precondition: `get_key` hides a revoked key, which is the trap"
        );

        let data = valid_login(&p, &root, &device, &key(4)).await;
        let verifier = p.create_verifier(METHOD, Box::new(data)).expect("verifier");
        let err = verifier
            .verify()
            .await
            .expect_err("a revoked account must not authenticate");
        assert!(err.to_string().contains("revoked"), "got: {err}");
    }

    // --- audience policy ----------------------------------------------------

    #[tokio::test]
    async fn an_audience_outside_the_allow_list_is_refused() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let p = AccountProofProvider::new(
            Arc::clone(&storage),
            KeyManager::new(storage),
            AccountProofConfig {
                node_key: Some(key(NODE_SEED).public_key().to_string()),
                allowed_audiences: vec!["https://app.example".to_owned()],
                ..AccountProofConfig::default()
            },
        )
        .expect("build");

        let (root, device, session) = (key(1), key(2), key(3));
        // `valid_login` signs for `Audience::Cli`, which is not on the list.
        let data = valid_login(&p, &root, &device, &session).await;

        let err = p.authenticate_core(&data).await.expect_err("audience");
        assert!(err.to_string().contains("not accepted"), "got: {err}");
    }

    // --- configuration fails closed -----------------------------------------

    /// Without the node key the provider cannot tell a statement minted for it
    /// from one minted elsewhere, so it must refuse to exist rather than accept
    /// everything.
    #[test]
    fn the_provider_refuses_to_build_without_a_node_key() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let err = AccountProofProvider::new(
            Arc::clone(&storage),
            KeyManager::new(storage),
            AccountProofConfig::default(),
        )
        .map(|_ignored| ())
        .expect_err("a provider with no node key must not build");
        assert!(err.to_string().contains("node_key"), "got: {err}");
    }

    #[test]
    fn a_malformed_node_key_is_refused_at_build_time() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        assert!(AccountProofProvider::new(
            Arc::clone(&storage),
            KeyManager::new(storage),
            AccountProofConfig {
                node_key: Some("not-a-key".to_owned()),
                ..AccountProofConfig::default()
            },
        )
        .map(|_ignored| ())
        .is_err());
    }

    /// The default session grants the delegated surface and nothing else.
    ///
    /// This test used to be `the_default_session_does_not_grant_reads`, and the
    /// reason it no longer is, is the whole point of #3931: reads were withheld
    /// until the node evaluated the caller's membership **per request**, which
    /// `query_context` now does — through `MembershipRepository::is_member`, on
    /// every call, so a removed member stops being served the moment the
    /// governance op lands rather than when their session expires. With that in
    /// place `context:query` is safe to mint by default, and the config comment
    /// that said "do not add reads until (#3931)" has been satisfied rather than
    /// overruled.
    ///
    /// What still has to hold is the ceiling: the delegated surface and nothing
    /// above it. Every entry is separately gated — a write by the warrant and
    /// `CAN_AUTHOR_ON_BEHALF`, a read and a subscription by the per-call
    /// membership check, and the two `-own` listings by the caller's groups
    /// resolved per request in `admin/caller_scope.rs` — so none of them is
    /// authority this token confers on its own. `admin`, `context:execute`, an
    /// alias scope, or the wide `context:list` / `namespace:list` (which also
    /// reach un-scoped sibling reads) would be, which is why this asserts the
    /// exact set rather than `contains`.
    #[test]
    fn the_default_session_grants_the_delegated_surface_and_no_more() {
        let perms = AccountProofConfig::default().session_permissions;
        assert_eq!(
            perms,
            vec![
                "context:intent".to_owned(),
                "context:list-own".to_owned(),
                "context:query".to_owned(),
                "context:subscribe".to_owned(),
                "namespace:list-own".to_owned()
            ]
        );
    }
}
