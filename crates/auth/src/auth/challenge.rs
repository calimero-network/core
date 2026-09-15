//! Single-use login challenges.
//!
//! # Why it is shaped this way
//!
//! **A challenge carries its own validity.** The 32 bytes are
//! `expiry ‖ nonce ‖ tag`, where the tag is an HMAC over the other two under a
//! key only this service holds. So "did I issue this, and is it still fresh?" is
//! answered from the bytes alone, with no per-challenge record written at issue
//! time. That matters because issuing is unauthenticated: anyone can ask for a
//! challenge, and a design that stored one row per request would hand an
//! anonymous caller a write amplifier.
//!
//! **Single-use is therefore the only thing that needs state, and only after a
//! signature verifies.** [`ChallengeMinter::redeem`] is called at the end of the
//! login exchange, never at the start. Burning on presentation instead would let
//! an attacker who can see a challenge — it crosses the wire in the clear —
//! invalidate it before its rightful holder finishes signing, turning a replay
//! guard into a denial-of-service primitive.
//!
//! **The spent set is bounded by the expiry, not by traffic.** An entry only has
//! to outlive the challenge that created it, so each carries its own expiry and
//! [`ChallengeMinter::redeem`] sweeps what has passed. With a lifetime measured
//! in tens of seconds the set stays proportional to the login rate over that
//! window rather than growing without limit.
//!
//! # Deployment note
//!
//! Single-use holds across exactly as much as the storage is shared across. One
//! process, or several sharing a backend, all see one spent set. Several
//! replicas behind a load balancer with *separate* storage would each accept the
//! same challenge once — so a replicated deployment needs shared storage for
//! this property, not merely for durability.

use std::time::{SystemTime, UNIX_EPOCH};

use eyre::{bail, Result};
use rand::RngExt;
use ring::hmac;
use subtle::ConstantTimeEq;
use tracing::{debug, warn};

use crate::storage::Storage;

/// Storage key holding the HMAC key challenges are minted under.
const CHALLENGE_MAC_KEY: &str = "auth:challenge:mac-key:v1";

/// Prefix for spent-challenge records.
const SPENT_PREFIX: &str = "auth:challenge:spent:";

/// Bytes in a challenge: 8 expiry + 8 nonce + 16 tag.
pub const CHALLENGE_LEN: usize = 32;

/// Truncation length of the HMAC tag, in bytes.
///
/// 128 bits. The full SHA-256 tag does not fit alongside the expiry and nonce in
/// the 32 bytes a [`calimero_account::LoginStatement`] commits to, and 128 bits
/// is far beyond forgeable within a lifetime measured in tens of seconds.
const TAG_LEN: usize = 16;

/// Mints and redeems login challenges.
pub struct ChallengeMinter {
    storage: std::sync::Arc<dyn Storage>,
    /// How long an issued challenge stays valid, in seconds.
    ttl_secs: u64,
}

/// A freshly minted challenge and the moment it stops being valid.
#[derive(Debug, Clone, Copy)]
pub struct Challenge {
    /// The bytes the client signs over.
    pub bytes: [u8; CHALLENGE_LEN],
    /// Unix seconds after which it is refused.
    pub expires_at: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl ChallengeMinter {
    /// A minter over `storage`, issuing challenges valid for `ttl_secs`.
    pub fn new(storage: std::sync::Arc<dyn Storage>, ttl_secs: u64) -> Self {
        Self { storage, ttl_secs }
    }

    /// The HMAC key, generated on first use and reused thereafter.
    ///
    /// Generated rather than configured: it authenticates this service to
    /// itself and never leaves the process, so an operator-supplied value would
    /// be a secret to manage with nothing gained. Losing it on a storage wipe
    /// invalidates outstanding challenges, which are seconds old by
    /// construction.
    async fn mac_key(&self) -> Result<hmac::Key> {
        if let Some(raw) = self.storage.get(CHALLENGE_MAC_KEY).await? {
            if raw.len() == 32 {
                return Ok(hmac::Key::new(hmac::HMAC_SHA256, &raw));
            }
            warn!(
                len = raw.len(),
                "stored challenge MAC key has the wrong length; regenerating"
            );
        }

        let key: [u8; 32] = rand::rng().random();
        self.storage.set(CHALLENGE_MAC_KEY, &key).await?;
        debug!("generated a new challenge MAC key");
        Ok(hmac::Key::new(hmac::HMAC_SHA256, &key))
    }

    /// Issue a challenge valid for the configured lifetime.
    ///
    /// # Errors
    /// Propagates a storage failure while reading or minting the MAC key.
    pub async fn issue(&self) -> Result<Challenge> {
        let key = self.mac_key().await?;
        let expires_at = now_secs().saturating_add(self.ttl_secs);

        let nonce: [u8; 8] = rand::rng().random();

        let mut bytes = [0u8; CHALLENGE_LEN];
        bytes[..8].copy_from_slice(&expires_at.to_le_bytes());
        bytes[8..16].copy_from_slice(&nonce);
        let tag = hmac::sign(&key, &bytes[..16]);
        bytes[16..].copy_from_slice(&tag.as_ref()[..TAG_LEN]);

        Ok(Challenge { bytes, expires_at })
    }

    /// Check that this service issued `challenge` and that it has not expired.
    ///
    /// Says nothing about whether it has already been spent — that is
    /// [`Self::redeem`], and the split is deliberate: this runs before the
    /// expensive signature work, that runs after it.
    ///
    /// # Errors
    /// If the tag does not verify or the expiry has passed.
    pub async fn verify(&self, challenge: &[u8; CHALLENGE_LEN]) -> Result<()> {
        let key = self.mac_key().await?;

        // `hmac::verify` would compare the FULL SHA-256 tag; the challenge only
        // has room for a truncated one, so recompute and compare the prefix.
        // Constant-time via `subtle` — a byte-wise compare would leak the tag one
        // position at a time to a caller able to retry.
        let expected = hmac::sign(&key, &challenge[..16]);
        if expected.as_ref()[..TAG_LEN]
            .ct_eq(&challenge[16..])
            .unwrap_u8()
            != 1
        {
            bail!("challenge was not issued by this node");
        }

        let mut expiry = [0u8; 8];
        expiry.copy_from_slice(&challenge[..8]);
        let expires_at = u64::from_le_bytes(expiry);
        if expires_at <= now_secs() {
            bail!("challenge has expired");
        }

        Ok(())
    }

    /// Spend `challenge`, refusing one already spent.
    ///
    /// Call this **after** the signature over the challenge has verified, so a
    /// failed or forged attempt cannot burn a challenge its rightful holder is
    /// still using.
    ///
    /// # Errors
    /// If the challenge has already been spent, or storage fails.
    pub async fn redeem(&self, challenge: &[u8; CHALLENGE_LEN]) -> Result<()> {
        let record = format!("{SPENT_PREFIX}{}", hex::encode(challenge));

        if self.storage.exists(&record).await? {
            bail!("challenge has already been used");
        }

        // The entry only has to outlive the challenge, so it stores the same
        // expiry the challenge carries and the sweep below reads it back.
        self.storage.set(&record, &challenge[..8]).await?;
        self.sweep_expired().await;

        Ok(())
    }

    /// Drop spent records whose challenges have expired.
    ///
    /// Best-effort on purpose: a failure here leaves stale rows, which costs
    /// space and nothing else, and must not fail a login that has already
    /// authenticated. Logged rather than propagated for that reason.
    async fn sweep_expired(&self) {
        let now = now_secs();

        let keys = match self.storage.list_keys(SPENT_PREFIX).await {
            Ok(keys) => keys,
            Err(err) => {
                warn!(%err, "could not list spent challenges to sweep");
                return;
            }
        };

        for key in keys {
            let Ok(Some(raw)) = self.storage.get(&key).await else {
                continue;
            };
            let Ok(expiry) = <[u8; 8]>::try_from(raw.as_slice()) else {
                continue;
            };
            if u64::from_le_bytes(expiry) <= now {
                if let Err(err) = self.storage.delete(&key).await {
                    debug!(%err, "could not delete an expired spent-challenge record");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::storage::MemoryStorage;

    fn minter(ttl_secs: u64) -> ChallengeMinter {
        ChallengeMinter::new(Arc::new(MemoryStorage::new()), ttl_secs)
    }

    #[tokio::test]
    async fn an_issued_challenge_verifies() {
        let m = minter(60);
        let c = m.issue().await.expect("issue");
        assert!(m.verify(&c.bytes).await.is_ok());
    }

    #[tokio::test]
    async fn a_forged_challenge_is_refused() {
        let m = minter(60);
        let mut c = m.issue().await.expect("issue").bytes;
        // Flip a bit in the nonce; the tag no longer covers it.
        c[8] ^= 0x01;
        assert!(m.verify(&c).await.is_err());
    }

    /// A challenge minted by one service must not verify at another — the MAC
    /// key is what makes a challenge *this* node's.
    #[tokio::test]
    async fn a_challenge_from_another_minter_is_refused() {
        let theirs = minter(60).issue().await.expect("issue").bytes;
        assert!(minter(60).verify(&theirs).await.is_err());
    }

    #[tokio::test]
    async fn an_expired_challenge_is_refused() {
        // ttl 0 puts the expiry at `now`, and the check is `<=`.
        let m = minter(0);
        let c = m.issue().await.expect("issue");
        let err = m.verify(&c.bytes).await.expect_err("expired");
        assert!(err.to_string().contains("expired"), "got: {err}");
    }

    #[tokio::test]
    async fn a_challenge_is_single_use() {
        let m = minter(60);
        let c = m.issue().await.expect("issue");
        assert!(m.redeem(&c.bytes).await.is_ok());
        let err = m.redeem(&c.bytes).await.expect_err("second redeem");
        assert!(err.to_string().contains("already been used"), "got: {err}");
    }

    /// Verification must not spend the challenge: only `redeem` does, and only
    /// after a signature has checked out. Otherwise anyone who can see a
    /// challenge in flight could invalidate it.
    #[tokio::test]
    async fn verifying_does_not_spend() {
        let m = minter(60);
        let c = m.issue().await.expect("issue");
        assert!(m.verify(&c.bytes).await.is_ok());
        assert!(m.verify(&c.bytes).await.is_ok());
        assert!(
            m.redeem(&c.bytes).await.is_ok(),
            "verification must leave the challenge spendable"
        );
    }

    #[tokio::test]
    async fn two_challenges_differ() {
        let m = minter(60);
        let a = m.issue().await.expect("issue").bytes;
        let b = m.issue().await.expect("issue").bytes;
        assert_ne!(a, b, "the nonce must make each issue unique");
    }

    /// The spent set must not grow without bound: an entry outlives only the
    /// challenge that created it.
    #[tokio::test]
    async fn expired_spent_records_are_swept() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());

        // Spend a challenge that is already expired.
        let short = ChallengeMinter::new(Arc::clone(&storage), 0);
        let stale = short.issue().await.expect("issue");
        short.redeem(&stale.bytes).await.expect("redeem");

        // Any later redeem sweeps it, because its expiry has passed.
        let long = ChallengeMinter::new(Arc::clone(&storage), 60);
        let fresh = long.issue().await.expect("issue");
        long.redeem(&fresh.bytes).await.expect("redeem");

        let remaining = storage.list_keys(SPENT_PREFIX).await.expect("list");
        assert_eq!(
            remaining.len(),
            1,
            "only the unexpired record should remain, got {remaining:?}"
        );
    }
}
