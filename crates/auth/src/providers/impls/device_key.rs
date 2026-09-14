//! Log in with a device key that an account root certified. (PoC)
//!
//! The node hands out a single-use challenge; the device signs
//! [`calimero_account::device_login_payload`] over it and presents the
//! `AccountProof<DeviceCert>` that certifies its key. The proof verifies from
//! the account id alone, so this needs no password and no node state.
//!
//! Two deliberate PoC shortcuts, both to revisit before this is real:
//! - A successful login stores a root key for the device, although the login
//!   path elsewhere never mints keys. Storing the device's public key is what
//!   lets the server guard mark the caller `AuthenticatedKey`.
//! - Challenges live in process memory, so they do not survive a restart.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::Request;
use calimero_account::{device_login_payload, AccountId, AccountProof, DeviceCert, DeviceId};
use calimero_primitives::identity::PublicKey;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;

use crate::api::handlers::auth::TokenRequest;
use crate::providers::core::provider::{AuthProvider, AuthRequestVerifier, AuthVerifierFn};
use crate::providers::core::provider_data_registry::AuthDataType;
use crate::providers::core::provider_registry::ProviderRegistration;
use crate::providers::ProviderContext;
use crate::storage::models::Key;
use crate::storage::KeyManager;
use crate::AuthResponse;

/// The method name a token request uses.
pub const METHOD: &str = "device_key";

/// How long an issued challenge stays usable.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(60);

/// What a device session may do. Reads through JSON-RPC, and submitting intents
/// for writes that carry the device's own account. Membership is still checked
/// per call by the server.
const SESSION_PERMISSIONS: &[&str] = &["context:execute", "context:list", "context:intent"];

fn challenges() -> &'static Mutex<HashMap<[u8; 32], Instant>> {
    static CHALLENGES: OnceLock<Mutex<HashMap<[u8; 32], Instant>>> = OnceLock::new();
    CHALLENGES.get_or_init(Default::default)
}

/// Issue a single-use challenge.
#[must_use]
pub fn issue_challenge() -> [u8; 32] {
    let challenge: [u8; 32] = rand::random();
    let mut issued = challenges().lock();
    issued.retain(|_, at| at.elapsed() < CHALLENGE_TTL);
    let _ = issued.insert(challenge, Instant::now());
    challenge
}

/// Spend a challenge. `false` if it was never issued, already used, or expired.
fn spend_challenge(challenge: &[u8; 32]) -> bool {
    challenges()
        .lock()
        .remove(challenge)
        .is_some_and(|at| at.elapsed() < CHALLENGE_TTL)
}

/// Called after a device logs in, with the key, the account its certificate
/// names and the device id. The node registers one so it can resolve the caller.
pub type DeviceLoginHook = Arc<dyn Fn(PublicKey, AccountId, DeviceId) + Send + Sync>;

static LOGIN_HOOK: OnceLock<DeviceLoginHook> = OnceLock::new();

/// Register the hook. The first registration wins.
pub fn set_device_login_hook(hook: DeviceLoginHook) {
    let _ = LOGIN_HOOK.set(hook);
}

/// The `provider_data` of a `device_key` token request, with the device key
/// taken from the request's `public_key`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceKeyAuthData {
    /// Hex Ed25519 public key of the device.
    pub device_key: String,
    /// Hex of the challenge the node issued.
    pub challenge: String,
    /// Hex of the 64-byte signature over the login payload.
    pub signature: String,
    /// Hex borsh of the `AccountProof<DeviceCert>` certifying the device key.
    pub credential: String,
}

fn hex_array<const N: usize>(value: &str, what: &str) -> eyre::Result<[u8; N]> {
    hex::decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| eyre::eyre!("{what} must be {} hex characters", N * 2))
}

struct DeviceKeyVerifier {
    key_manager: KeyManager,
    data: DeviceKeyAuthData,
}

#[async_trait]
impl AuthVerifierFn for DeviceKeyVerifier {
    async fn verify(&self) -> eyre::Result<AuthResponse> {
        let data = &self.data;
        let device_key = PublicKey::from(hex_array::<32>(&data.device_key, "publicKey")?);
        let challenge = hex_array::<32>(&data.challenge, "challenge")?;
        let signature = hex_array::<64>(&data.signature, "signature")?;

        if !spend_challenge(&challenge) {
            eyre::bail!("unknown, used or expired challenge");
        }
        device_key
            .verify_raw_signature(&device_login_payload(&challenge, &device_key), &signature)
            .map_err(|_| eyre::eyre!("the login signature does not verify"))?;

        let credential =
            hex::decode(&data.credential).map_err(|_| eyre::eyre!("credential must be hex"))?;
        let proof: AccountProof<DeviceCert> = borsh::from_slice(&credential)
            .map_err(|err| eyre::eyre!("credential does not decode: {err}"))?;
        let cert = proof
            .verify(proof.statement.account)
            .map_err(|err| eyre::eyre!("credential does not verify: {err}"))?;
        if cert.sign_pk != device_key {
            eyre::bail!("the credential certifies a different key");
        }

        let permissions: Vec<String> = SESSION_PERMISSIONS
            .iter()
            .map(|permission| (*permission).to_owned())
            .collect();
        let key_id = format!("device-{}", data.device_key);
        let key = Key::new_root_key_with_permissions(
            data.device_key.clone(),
            METHOD.to_owned(),
            permissions.clone(),
            None,
        );
        let _ = self
            .key_manager
            .set_key(&key_id, &key)
            .await
            .map_err(|err| eyre::eyre!("storing the device session key: {err}"))?;

        if let Some(hook) = LOGIN_HOOK.get() {
            hook(device_key, cert.account, cert.device);
        }

        Ok(AuthResponse {
            is_valid: true,
            key_id,
            permissions,
        })
    }
}

/// The `device_key` provider.
#[derive(Clone)]
pub struct DeviceKeyProvider {
    key_manager: KeyManager,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceKeyRequest {
    challenge: String,
    signature: String,
    credential: String,
}

#[async_trait]
impl AuthProvider for DeviceKeyProvider {
    fn name(&self) -> &str {
        METHOD
    }

    fn provider_type(&self) -> &str {
        "device"
    }

    fn description(&self) -> &str {
        "Log in with a device key certified by the account root (PoC)"
    }

    fn supports_method(&self, method: &str) -> bool {
        method == METHOD
    }

    fn is_configured(&self) -> bool {
        true
    }

    async fn is_configured_with_users(&self) -> eyre::Result<bool> {
        Ok(true)
    }

    fn get_config_options(&self) -> Value {
        serde_json::json!({ "enabled": true, "challenge": "/auth/challenge" })
    }

    fn prepare_auth_data(&self, token_request: &TokenRequest) -> eyre::Result<Value> {
        let request: DeviceKeyRequest = serde_json::from_value(token_request.provider_data.clone())
            .map_err(|err| eyre::eyre!("invalid device_key data: {err}"))?;
        Ok(serde_json::json!({
            "deviceKey": token_request.public_key,
            "challenge": request.challenge,
            "signature": request.signature,
            "credential": request.credential,
        }))
    }

    fn create_verifier(
        &self,
        method: &str,
        auth_data: Box<dyn Any + Send + Sync>,
    ) -> eyre::Result<AuthRequestVerifier> {
        if !self.supports_method(method) {
            eyre::bail!("provider {} does not support method {method}", self.name());
        }
        let data = auth_data
            .downcast_ref::<DeviceKeyAuthData>()
            .ok_or_else(|| eyre::eyre!("failed to parse device_key auth data"))?
            .clone();
        Ok(AuthRequestVerifier::new(DeviceKeyVerifier {
            key_manager: self.key_manager.clone(),
            data,
        }))
    }

    fn verify_request(&self, _request: &Request<Body>) -> eyre::Result<AuthRequestVerifier> {
        eyre::bail!("device_key logs in through /auth/challenge and /auth/token")
    }

    async fn create_root_key(
        &self,
        _public_key: &str,
        _auth_method: &str,
        _provider_data: Value,
        _node_url: Option<&str>,
    ) -> eyre::Result<bool> {
        Ok(false)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct DeviceKeyAuthDataType;

impl AuthDataType for DeviceKeyAuthDataType {
    fn method_name(&self) -> &str {
        METHOD
    }

    fn parse_from_value(&self, value: Value) -> eyre::Result<Box<dyn Any + Send + Sync>> {
        let data: DeviceKeyAuthData = serde_json::from_value(value)
            .map_err(|err| eyre::eyre!("invalid device_key auth data: {err}"))?;
        Ok(Box::new(data))
    }

    fn get_sample_structure(&self) -> Value {
        serde_json::json!({
            "challenge": "<hex from POST /auth/challenge>",
            "signature": "<hex, 64 bytes>",
            "credential": "<hex borsh AccountProof<DeviceCert>>",
        })
    }
}

struct DeviceKeyProviderRegistration;

impl ProviderRegistration for DeviceKeyProviderRegistration {
    fn provider_id(&self) -> &str {
        METHOD
    }

    fn create_provider(
        &self,
        context: ProviderContext,
    ) -> Result<Box<dyn AuthProvider>, eyre::Error> {
        Ok(Box::new(DeviceKeyProvider {
            key_manager: context.key_manager,
        }))
    }

    fn is_enabled(&self, config: &crate::config::AuthConfig) -> bool {
        // PoC: on unless a config turns it off.
        config.providers.get(METHOD).copied().unwrap_or(true)
    }
}

crate::register_auth_provider!(DeviceKeyProviderRegistration);
crate::register_auth_data_type!(DeviceKeyAuthDataType);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_is_spent_once() {
        let challenge = issue_challenge();
        assert!(spend_challenge(&challenge));
        assert!(!spend_challenge(&challenge));
    }

    #[test]
    fn an_unissued_challenge_is_refused() {
        assert!(!spend_challenge(&[7u8; 32]));
    }
}
