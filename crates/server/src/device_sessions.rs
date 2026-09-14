//! Devices that logged in with a certified device key. (PoC)
//!
//! A device that never joined a namespace has no binding row, so resolving its
//! key to an account by binding finds nothing. Its login certificate already
//! named the account, and mero-auth reports it here; `caller_account` uses this
//! as the fallback and still refuses a device the namespace has revoked.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use calimero_account::{AccountId, DeviceId};
use calimero_primitives::identity::PublicKey;

fn sessions() -> &'static RwLock<HashMap<[u8; 32], (AccountId, DeviceId)>> {
    static SESSIONS: OnceLock<RwLock<HashMap<[u8; 32], (AccountId, DeviceId)>>> = OnceLock::new();
    SESSIONS.get_or_init(Default::default)
}

/// Register with mero-auth so device logins are recorded.
pub(crate) fn install() {
    mero_auth::providers::impls::device_key::set_device_login_hook(Arc::new(
        |key, account, device| {
            if let Ok(mut sessions) = sessions().write() {
                let bytes: &[u8; 32] = key.as_ref();
                let _ = sessions.insert(*bytes, (account, device));
            }
        },
    ));
}

/// The account and device a logged-in device key speaks for.
pub(crate) fn lookup(key: &PublicKey) -> Option<(AccountId, DeviceId)> {
    let bytes: &[u8; 32] = key.as_ref();
    sessions().read().ok()?.get(bytes).copied()
}
