//! The sealed transport: HTTP requests encrypted end to end to an attested
//! node's in-memory X25519 transport key, so TLS that ends outside the
//! enclave (a load balancer, a proxy) never sees them.
//!
//! Wire format v2, byte-compatible with mero-js `src/sealed/`:
//!
//! ```text
//! handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2"
//!   request   POST /sealed/handshake   0x02 || transport_pk || Noise message 1
//!   response  0x02 || Noise message 2 (payload = session_id(16) || lifetime secs, u32 BE)
//! identified  0x03 instead of 0x02, Noise_IK_25519_AESGCM_SHA256: a node calling a
//!             peer proves its own transport key (peers only; clients stay on NK)
//! exchange    POST /sealed
//!   header    0x02 || session_id(16) || request_id (u64 BE; from 1, each used once)
//!   request   header || AES-256-GCM(req key, nonce = request_id || 0u32, aad = header, inner)
//!   response  frames: u32 BE length || AES-256-GCM(resp key, nonce = request_id || index, aad = header, kind || data)
//!   inner     u32 BE head length || head (JSON) || body
//! ```
//!
//! Nodes up to 1.0.0-rc.24 served the two at `/sealed/v2/handshake` and
//! `/sealed/v2`; those paths are gone. The `v2` there never named the wire
//! format, which the version byte does.

#![forbid(unsafe_code)]

pub mod client;
pub mod file;
pub mod identity;
pub mod server;
mod session;
pub mod wrap;

pub use identity::{PeerKey, StaticKey, IDENTIFIED_VERSION};
pub use server::{
    derive_shared, expire_sessions, intercept, LowOrder, SealedOptions, SealedTransport,
    HANDSHAKE_PATH, SEALED_PATH,
};

/// Private entry points the fuzz targets drive, compiled only under `cargo fuzz`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod fuzz_api {
    /// [`crate::session::respond`]: message 2 and the session's request and response keys.
    pub fn respond(
        transport_secret: &[u8; 32],
        message_1: &[u8],
        session_id: &[u8; 16],
    ) -> Option<(Vec<u8>, [u8; 32], [u8; 32])> {
        let (message_2, keys) =
            crate::session::respond(transport_secret, message_1, session_id).ok()?;
        Some((message_2, *keys.request, *keys.response))
    }

    /// [`crate::session::respond_identified`], with the static key the caller proved.
    pub fn respond_identified(
        transport_secret: &[u8; 32],
        message_1: &[u8],
        session_id: &[u8; 16],
    ) -> Option<(Vec<u8>, [u8; 32], [u8; 32], [u8; 32])> {
        let (message_2, keys, peer) =
            crate::session::respond_identified(transport_secret, message_1, session_id).ok()?;
        Some((message_2, *keys.request, *keys.response, peer))
    }

    /// A [`crate::file::FileSealer`] under `key`, so a target can seal what it then opens.
    pub fn sealer(key: [u8; 32]) -> crate::file::FileSealer {
        crate::file::FileSealer::with_key(key)
    }
}
