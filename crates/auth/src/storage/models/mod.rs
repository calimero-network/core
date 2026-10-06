mod keys;

use serde::{Deserialize, Serialize};

pub use keys::{Key, KeyMetadata, KeyType};

/// A username's password login: the root key it opens and the salted hash that opens it.
#[derive(Clone, Serialize, Deserialize)]
pub struct PasswordLogin {
    /// The random id of the root key the password opens
    pub key_id: String,
    /// Random per-password salt
    pub salt: Vec<u8>,
    /// PBKDF2-HMAC-SHA256 of the password under `salt`
    pub hash: Vec<u8>,
}

/// Storage prefixes for different types of data
pub mod prefixes {
    /// Prefix for root keys
    pub const ROOT_KEY: &str = "root_key:";

    /// Prefix for client keys
    pub const CLIENT_KEY: &str = "client_key:";

    /// Prefix for permissions
    pub const PERMISSION: &str = "permission:";

    /// Prefix for refresh tokens
    pub const REFRESH_TOKEN: &str = "refresh:";

    /// Prefix for the secondary index of root key to client keys
    pub const ROOT_CLIENTS: &str = "root_clients:";

    /// Prefix for public key index
    pub const PUBLIC_KEY_INDEX: &str = "index:public_key:";

    /// Prefix for the username index of password logins
    pub const PASSWORD_LOGIN: &str = "index:username:";

    /// Prefix for key permissions
    pub const KEY_PERMISSIONS: &str = "key_permissions:";
}
