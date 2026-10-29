//! The sealed transport: HTTP requests encrypted end to end to an attested
//! node's in-memory X25519 transport key, so TLS that ends outside the
//! enclave (a load balancer, a proxy) never sees them.
//!
//! Wire format v2, byte-compatible with mero-js `src/sealed/`:
//!
//! ```text
//! handshake   Noise_NK_25519_AESGCM_SHA256, prologue "calimero/sealed-http/v2"
//!   request   POST /sealed/v2/handshake   0x02 || transport_pk || Noise message 1
//!   response  0x02 || Noise message 2 (payload = session_id(16) || lifetime secs, u32 BE)
//! identified  0x03 instead of 0x02, Noise_IK_25519_AESGCM_SHA256: a node calling a
//!             peer proves its own transport key (peers only; clients stay on NK)
//! exchange    POST /sealed/v2
//!   header    0x02 || session_id(16) || request_id (u64 BE; from 1, each used once)
//!   request   header || AES-256-GCM(req key, nonce = request_id || 0u32, aad = header, inner)
//!   response  frames: u32 BE length || AES-256-GCM(resp key, nonce = request_id || index, aad = header, kind || data)
//!   inner     u32 BE head length || head (JSON) || body
//! ```

pub mod client;
pub mod identity;
pub mod server;
mod session;

pub use identity::{PeerKey, StaticKey, IDENTIFIED_VERSION};
pub use server::{
    derive_shared, expire_sessions, intercept, LowOrder, SealedOptions, SealedTransport,
    HANDSHAKE_PATH, SEALED_PATH,
};
