//! API keys: the Ed25519 key an API key stands for (the key itself, its
//! records and the sessions are routes of the generated client).

use super::prelude::*;

#[pymethods]
impl PyCloudClient {
    /// The Ed25519 public key (hex) an API key stands for, byte-for-byte
    /// core's `proof::api_key_public`: the seed is `SHA-256("calimero.api-key.v1\0"
    /// || key)`. An upgrade key's record (`list_api_keys`) keeps it as
    /// `upgrades.proofKey`, and the node signs each upgrade the key makes
    /// with its private half, so the directory's log proves the key holder
    /// asked for it. Compare the two to check which key a record is.
    #[staticmethod]
    fn api_key_public(key: &str) -> String {
        hex::encode(api_key_signing_key(key).verifying_key().to_bytes())
    }
}

/// What an API key's Ed25519 key is derived from, before the key (core's
/// `proof::API_KEY_DOMAIN`).
const API_KEY_DOMAIN: &[u8] = b"calimero.api-key.v1\0";

/// The Ed25519 key an API key stands for (core's `proof::api_key_signing_key`).
fn api_key_signing_key(key: &str) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&calimero_identity::sha256(
        &[API_KEY_DOMAIN, key.as_bytes()].concat(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Computed apart from this code (Python's `cryptography`) from core's
    /// definition: `SHA-256("calimero.api-key.v1\0" || key)` as the Ed25519
    /// seed, the public key in hex.
    #[test]
    fn an_api_keys_public_key_is_cores() {
        for (key, seed, public) in [
            (
                "cal_0123456789abcdef",
                "d1a9611e1b88cf3dffd051bec6ae3826b8d506c60f9e1492139a2c475d1f23f3",
                "fb2a698c59f05bb66aaedd4badecbce8efb5a9abe976906c0eed14e48da14adf",
            ),
            (
                "cal_ci",
                "27953e2a85fa994e5b77b053c474015c5af18ee97ec58aae4d6fdbd7e5411f3a",
                "288164f1279d77ba71dd86e20149ecf9398f0e884bad8644a096ecefd5555a27",
            ),
        ] {
            assert_eq!(hex::encode(api_key_signing_key(key).to_bytes()), seed);
            assert_eq!(PyCloudClient::api_key_public(key), public);
        }
    }
}
