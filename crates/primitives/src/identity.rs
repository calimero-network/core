use core::fmt;
use core::ops::Deref;
use core::str::FromStr;

#[cfg(feature = "rand")]
use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

use crate::context::ContextId;
use crate::hash::{Hash, HashError};

use ed25519_dalek::{Signature, SignatureError, Signer, SigningKey, Verifier, VerifyingKey};

#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct PrivateKey(Hash);

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("PrivateKey")
    }
}

impl Drop for PrivateKey {
    fn drop(&mut self) {
        // Zeroize the key material to prevent it from remaining in memory.
        //
        // SAFETY:
        // - The pointer is valid and properly aligned because it comes from a valid
        //   mutable reference (`&mut self.0`).
        // - The size is correct as we use `size_of::<Hash>()` on the actual type.
        // - We have exclusive access to this memory via `&mut self`.
        // - Hash doesn't expose `DerefMut` or implement `Zeroize`, so we use pointer
        //   casting to get a mutable byte slice over the entire structure.
        unsafe {
            let hash_ptr = &mut self.0 as *mut Hash as *mut u8;
            let hash_size = core::mem::size_of::<Hash>();
            core::slice::from_raw_parts_mut(hash_ptr, hash_size).zeroize();
        }
    }
}

impl From<[u8; 32]> for PrivateKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for PrivateKey {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for PrivateKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PrivateKey {
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        SigningKey::from_bytes(self)
            .verifying_key()
            .to_bytes()
            .into()
    }

    #[cfg(feature = "rand")]
    pub fn random<R: CryptoRng + RngCore>(csprng: &mut R) -> Self {
        let mut secret = [0; 32];

        csprng.fill_bytes(&mut secret);

        Self::from(secret)
    }

    pub fn sign(&self, message: &[u8]) -> Result<Signature, SignatureError> {
        SigningKey::from_bytes(self).try_sign(message)
    }
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct PublicKey(Hash);

impl From<[u8; 32]> for PublicKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for PublicKey {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref() // self.0 is a Hash, which is [u8; 32], which can be AsRef'd to &[u8]
    }
}

impl Deref for PublicKey {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PublicKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Verify a signature against this public key.
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), SignatureError> {
        VerifyingKey::from_bytes(self.as_ref())?.verify(message, signature)
    }

    /// Verify a signature passed as a raw bytes against this public key.
    pub fn verify_raw_signature(
        &self,
        message: &[u8],
        signature_bytes: &[u8; 64],
    ) -> Result<(), SignatureError> {
        let signature = Signature::from_bytes(&signature_bytes);
        self.verify(message, &signature)
    }

    // Return represented as a 32-byte array
    pub fn digest(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<PublicKey> for String {
    fn from(id: PublicKey) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&PublicKey> for String {
    fn from(id: &PublicKey) -> Self {
        id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, Error)]
#[error(transparent)]
pub struct InvalidPublicKey(HashError);

impl FromStr for PublicKey {
    type Err = InvalidPublicKey;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidPublicKey)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Did {
    pub id: String,
    pub root_keys: Vec<RootKey>,
    pub client_keys: Vec<ClientKey>,
}

impl Did {
    #[must_use]
    pub const fn new(id: String, root_keys: Vec<RootKey>, client_keys: Vec<ClientKey>) -> Self {
        Self {
            id,
            root_keys,
            client_keys,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RootKey {
    pub signing_key: String,
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub wallet_address: String,
    pub created_at: u64,
}

impl RootKey {
    #[must_use]
    pub const fn new(
        signing_key: String,
        wallet_type: WalletType,
        wallet_address: String,
        created_at: u64,
    ) -> Self {
        Self {
            signing_key,
            wallet_type,
            wallet_address,
            created_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ClientKey {
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub signing_key: String,
    pub created_at: u64,
    pub context_id: Option<ContextId>,
}

impl ClientKey {
    #[must_use]
    pub const fn new(
        wallet_type: WalletType,
        signing_key: String,
        created_at: u64,
        context_id: Option<ContextId>,
    ) -> Self {
        Self {
            wallet_type,
            signing_key,
            created_at,
            context_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContextUser {
    pub user_id: String,
    pub joined_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum WalletType {
    NEAR {
        #[serde(rename = "networkId")]
        network_id: NearNetworkId,
    },
    ETH {
        #[serde(rename = "chainId")]
        chain_id: u64,
    },
    STARKNET {
        #[serde(rename = "walletName")]
        wallet_name: String,
    },
    ICP {
        #[serde(rename = "canisterId")]
        canister_id: String,
        #[serde(rename = "walletName")]
        wallet_name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum NearNetworkId {
    Mainnet,
    Testnet,
    #[serde(untagged)]
    Custom(String),
}

#[cfg(test)]
mod tests {
    use core::mem::ManuallyDrop;

    use super::*;

    #[test]
    fn test_private_key_zeroize_on_drop() {
        // Create a non-zero key wrapped in ManuallyDrop to control when drop occurs
        let secret_bytes: [u8; 32] = [0x42; 32];
        let mut key = ManuallyDrop::new(PrivateKey::from(secret_bytes));

        // Verify the key contains the expected bytes before drop
        assert_eq!(key.as_ref(), &secret_bytes);

        // Get a raw pointer to the key's memory location before dropping
        let key_ptr = &*key as *const PrivateKey as *const u8;
        let hash_size = core::mem::size_of::<Hash>();

        // Manually drop the key, which will call our Drop implementation.
        // SAFETY: The key was created with ManuallyDrop::new, so we need to
        // manually drop it. After this, the ManuallyDrop wrapper prevents
        // double-drop.
        unsafe {
            ManuallyDrop::drop(&mut key);
        }

        // NOTE: Reading memory after drop is technically undefined behavior in Rust's
        // memory model, even though the stack memory is still allocated. We accept
        // this UB in a test-only context to verify the security property that
        // sensitive key material is zeroized. The ManuallyDrop wrapper ensures
        // the stack memory hasn't been reused yet.
        //
        // SAFETY: We're reading stack memory that was just zeroized. While this is
        // technically UB (the value has been invalidated by drop), it's acceptable
        // here for verifying the security-critical zeroization behavior.
        let zeroed = unsafe { core::slice::from_raw_parts(key_ptr, hash_size) };

        // Check that the entire Hash structure is zeroed, not just the key bytes
        assert!(
            zeroed.iter().all(|&b| b == 0),
            "Key material was not properly zeroized on drop"
        );
    }

    #[test]
    fn test_private_key_can_sign_before_drop() {
        // Ensure PrivateKey still works correctly with the Drop implementation
        let secret_bytes: [u8; 32] = [0x42; 32];
        let key = PrivateKey::from(secret_bytes);

        // Key should be usable for signing
        let message = b"test message";
        let signature = key.sign(message);
        assert!(signature.is_ok());

        // Key should be usable for deriving public key
        let public_key = key.public_key();
        assert!(!AsRef::<[u8; 32]>::as_ref(&public_key)
            .iter()
            .all(|&b| b == 0));

        // Signature should verify with the public key
        let sig = signature.unwrap();
        assert!(public_key.verify(message, &sig).is_ok());
    }
}
