//! Credential management for the relayer

use calimero_context_config::client::config::Credentials;
use calimero_context_config::client::protocol::near::Credentials as ClientNearCredentials;

/// Protocol-specific signing credentials
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProtocolCredentials {
    Near {
        account_id: String,
        public_key: String,
        secret_key: String,
    },
}

impl From<ProtocolCredentials> for Credentials {
    fn from(creds: ProtocolCredentials) -> Self {
        match creds {
            ProtocolCredentials::Near {
                account_id,
                public_key,
                secret_key,
            } => Credentials::Near(ClientNearCredentials {
                account_id: account_id.parse().expect("Invalid NEAR account ID"),
                public_key: public_key.parse().expect("Invalid NEAR public key"),
                secret_key: secret_key.parse().expect("Invalid NEAR secret key"),
            }),
        }
    }
}

/// Create NEAR credentials from environment variables.
pub fn from_env() -> Option<ProtocolCredentials> {
    // Get environment variables
    let account_id = std::env::var("NEAR_ACCOUNT_ID").ok();
    let public_key = std::env::var("NEAR_PUBLIC_KEY").ok();
    let secret_key = std::env::var("NEAR_SECRET_KEY").ok();

    // Helper function to check if all required credentials are present and non-empty
    let has_required_creds =
        |account_id: &Option<String>, public_key: &Option<String>, secret_key: &Option<String>| {
            account_id.as_ref().map_or(false, |s| !s.is_empty())
                && public_key.as_ref().map_or(false, |s| !s.is_empty())
                && secret_key.as_ref().map_or(false, |s| !s.is_empty())
        };

    if has_required_creds(&account_id, &public_key, &secret_key) {
        Some(ProtocolCredentials::Near {
            account_id: account_id.unwrap(),
            public_key: public_key.unwrap(),
            secret_key: secret_key.unwrap(),
        })
    } else {
        None
    }
}
