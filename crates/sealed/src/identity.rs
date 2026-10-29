//! Who opened a sealed session, when the opener is a node.
//!
//! A client opens a session with Noise NK and stays anonymous: the node learns
//! nothing about it. A node calling a peer opens it with Noise IK instead
//! (handshake version [`IDENTIFIED_VERSION`]): it proves it holds the static
//! key it names, which is its own attested transport key. The server hands
//! that key to every request of the session as a [`PeerKey`] extension, so a
//! route can tell which attested node is calling.

use curve25519_dalek::montgomery::MontgomeryPoint;
use zeroize::Zeroizing;

/// The handshake version byte of an identified session (Noise IK). Exchange
/// envelopes keep version 2 whichever handshake opened their session.
pub const IDENTIFIED_VERSION: u8 = 3;

/// A static X25519 key a node identifies itself with: the secret half of its
/// transport key, held in this process only.
#[derive(Clone)]
pub struct StaticKey {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl StaticKey {
    pub(crate) fn from_secret(secret: &[u8; 32]) -> Self {
        Self {
            secret: Zeroizing::new(*secret),
            public: MontgomeryPoint::mul_base_clamped(*secret).0,
        }
    }

    #[must_use]
    pub const fn public(&self) -> [u8; 32] {
        self.public
    }

    pub(crate) fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl core::fmt::Debug for StaticKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StaticKey")
            .field("public", &hex::encode(self.public))
            .finish_non_exhaustive()
    }
}

/// The static key the caller of an identified session proved it holds, set
/// by the server on every request opened in that session. Absent on a plain
/// request and on a session opened anonymously (Noise NK).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerKey(pub [u8; 32]);
