use core::fmt;
// Required by `PublicKey`'s `Deref` impl below; `PrivateKey` deliberately has none.
use core::ops::Deref;
use core::str::FromStr;

#[cfg(feature = "rand")]
use rand::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;
// `random()` zeroizes its local seed copy, which needs the `Zeroize` trait in
// scope. Both the call site and this import are gated on `rand` so a default
// build (without `random`) doesn't pull in an unused import.
#[cfg(feature = "rand")]
use zeroize::Zeroize;

use crate::hash::{Hash, HashError};

use ed25519_dalek::{Signature, SignatureError, Signer, SigningKey, VerifyingKey};

// NOTE: `PrivateKey` deliberately derives no serialization (Borsh, Serde, …).
// Serializing would copy the secret into a buffer that is never zeroized,
// silently defeating the zeroize-on-drop guarantee and making it trivial to
// persist or transmit the secret. The raw bytes are reachable only through the
// audited [`PrivateKey::as_bytes`] accessor; storage layers that must persist
// key material take an explicit `[u8; 32]` copy at a reviewed call site.
//
// The inner type is a plain `[u8; 32]` (not `Hash`) so that the derived
// `ZeroizeOnDrop` can zeroize the secret directly and safely. The previous
// implementation reached the same goal with a hand-rolled `Drop` that cast a
// `*mut Hash` to `*mut u8` over `size_of::<Hash>()`; that was fragile — any
// padding or extra field added to `Hash` would have silently left key material
// un-zeroized. `#[derive(ZeroizeOnDrop)]` over `[u8; 32]` removes the `unsafe`
// and tracks the field layout automatically.
//
// `Clone` and `Copy` are deliberately NOT derived: either would hand out a copy
// of the secret that is not tracked by `ZeroizeOnDrop`, reintroducing exactly
// the leak this type guards against. Code that genuinely needs the bytes goes
// through `as_bytes` at a reviewed call site.
#[derive(zeroize::ZeroizeOnDrop)]
pub struct PrivateKey([u8; 32]);

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("PrivateKey")
    }
}

impl From<[u8; 32]> for PrivateKey {
    fn from(id: [u8; 32]) -> Self {
        Self(id)
    }
}

impl PrivateKey {
    /// Returns a reference to the raw 32-byte secret key material.
    ///
    /// # Security
    ///
    /// This is the single audited entry point to the secret bytes. It exists
    /// for in-place cryptographic use (signing, key agreement) and for the few
    /// storage layers that must persist an explicit `[u8; 32]` copy. Callers
    /// MUST NOT log, print, or otherwise leak the returned bytes, and should
    /// keep any copy as short-lived and tightly scoped as possible — copies
    /// made out of this reference are not covered by the zeroize-on-drop
    /// guarantee. Every use should be obvious enough to review on sight.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        SigningKey::from_bytes(self.as_bytes())
            .verifying_key()
            .to_bytes()
            .into()
    }

    #[cfg(feature = "rand")]
    pub fn random<R: CryptoRng + RngCore>(csprng: &mut R) -> Self {
        let mut secret = [0; 32];

        csprng.fill_bytes(&mut secret);

        let key = Self::from(secret);

        // Zeroize the local copy of the seed so it doesn't linger on the stack
        // after being moved into the key.
        secret.zeroize();

        key
    }

    pub fn sign(&self, message: &[u8]) -> Result<Signature, SignatureError> {
        SigningKey::from_bytes(self.as_bytes()).try_sign(message)
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
    /// Verify a signature against this public key.
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), SignatureError> {
        VerifyingKey::from_bytes(self.as_ref())?.verify_strict(message, signature)
    }

    /// Verify a signature passed as a raw bytes against this public key.
    pub fn verify_raw_signature(
        &self,
        message: &[u8],
        signature_bytes: &[u8; 64],
    ) -> Result<(), SignatureError> {
        let signature = Signature::from_bytes(signature_bytes);
        self.verify(message, &signature)
    }

    // Return represented as a 32-byte array
    pub fn digest(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<PublicKey> for String {
    fn from(id: PublicKey) -> Self {
        id.0.to_base58()
    }
}

impl From<&PublicKey> for String {
    fn from(id: &PublicKey) -> Self {
        id.0.to_base58()
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

// ---------------------------------------------------------------------------
// Content-addressed ids (accounts and devices)
// ---------------------------------------------------------------------------
//
// These live here rather than in `calimero-account`, which is where the account
// MODEL lives, because `calimero-account` depends on this crate and the ids are
// named in the client-facing event payloads this crate defines. Declaring them
// here and re-exporting from `calimero-account` keeps every existing import
// working while breaking the cycle.

/// Domain-separated, length-prefixed SHA-256 over `parts`.
///
/// Length-prefixing every segment is what stops `("ab", "c")` and `("a", "bc")`
/// hashing alike, which would let two different facts share an id.
#[must_use]
pub fn domain_hash(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_le_bytes());
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// Domain separator for [`DeviceId::mint`].
pub const DEVICE_ID_DOMAIN: &[u8] = b"calimero.device.id.v1";

macro_rules! content_address_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, ::core::hash::Hash)]
        #[cfg_attr(
            feature = "borsh",
            derive(::borsh::BorshSerialize, ::borsh::BorshDeserialize)
        )]
        pub struct $name([u8; 32]);

        impl $name {
            /// The raw 32 bytes of this id.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }

            /// This id from its raw bytes, in a `const` context.
            ///
            /// `From<[u8; 32]>` cannot be `const`, so a sentinel or a fixed
            /// well-known id has no way to be a `const` item without this.
            #[must_use]
            pub const fn from_raw(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
        }

        impl From<[u8; 32]> for $name {
            fn from(value: [u8; 32]) -> Self {
                Self(value)
            }
        }

        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                &self.0
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        ::calimero_wasm_abi::impl_bytes32_abi!($name);

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}", hex::encode(self.0))
            }
        }

        /// Serializes as the hex string [`Display`] writes.
        ///
        /// A string, not a byte array: these ids cross the JSON-RPC boundary as
        /// app method arguments, and an app author typing an account into a call
        /// should be typing the same thing the CLI printed.
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
                raw.parse().map_err(serde::de::Error::custom)
            }
        }

        /// Parses the hex form [`Display`] writes.
        ///
        /// Hex rather than bs58, which is what a *key* is written in around
        /// here: an id that renders like a key invites being pasted where a key
        /// belongs, and both are 32 bytes, so nothing downstream would object.
        impl core::str::FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let bytes = hex::decode(s).map_err(|_| IdParseError)?;
                <[u8; 32]>::try_from(bytes)
                    .map(Self)
                    .map_err(|_| IdParseError)
            }
        }
    };
}

/// A string was not 64 hex characters, so it names no id.
#[derive(Clone, Copy, Debug, Error)]
#[error("expected 64 hex characters (32 bytes)")]
pub struct IdParseError;

content_address_id! {
    /// Stable identity of a person or agent — the **only** authorization
    /// subject in the system, and what an app sees as "who wrote this".
    ///
    /// This is the content address of the account's [`AccountGenesis`], not a
    /// public key. See the crate docs for why that distinction is load-bearing.
    AccountId
}

content_address_id! {
    /// Stable identity of one installation belonging to an account.
    ///
    /// This is the **CRDT replica id**: counter slots and HLC seeds key on it,
    /// and both require one writer per id. It is never an authorization input —
    /// authority always resolves through the [`AccountId`] the device is bound
    /// to.
    DeviceId
}

impl DeviceId {
    /// Mint a device id. Called once per installation, before the device has
    /// any certificate.
    ///
    /// Derived from the account and a fresh nonce rather than from the device's
    /// keys, so rotating a device's keypair keeps its replica identity — and
    /// therefore its counter slots and HLC lineage — intact.
    #[must_use]
    pub fn mint(account: AccountId, nonce: [u8; 16]) -> Self {
        Self::from(domain_hash(DEVICE_ID_DOMAIN, &[account.as_bytes(), &nonce]))
    }

    /// The 16-byte prefix used as this device's HLC instance seed.
    ///
    /// RGA character ids are minted from this seed, and two replicas sharing a
    /// seed mint colliding ids — which loses characters silently. So at most one
    /// of a colliding pair may be live in a scope, and the **lower** device id is
    /// the arbitrary-but-fixed winner. (Scope-local is sufficient: character ids
    /// only need to be unique within the scope that stores them.)
    ///
    /// That rule is applied when the device set is **read**, not when a link is
    /// admitted — see `ScopeState::live_devices`. Deciding it per link cannot
    /// work: "is there a lower colliding id" reads only what has folded so far,
    /// so the live set would depend on arrival order. Minting the id from a fresh
    /// nonce makes a collision vanishingly unlikely in any case; the rule is
    /// there so that a deliberate one is resolved identically everywhere rather
    /// than corrupting the CRDT planes.
    #[must_use]
    pub fn hlc_seed(&self) -> [u8; 16] {
        let mut seed = [0u8; 16];
        seed.copy_from_slice(&self.as_bytes()[..16]);
        seed
    }
}

// ---------------------------------------------------------------------------
// Naming a member
// ---------------------------------------------------------------------------

/// Who a membership call names: a **person**, or one **key** that person signs
/// with.
///
/// Membership rows are keyed by [`AccountId`], and every verb that reads or
/// changes one takes an account. An *add* is the exception, because the caller
/// may hold nothing but a key — an operator adding an outsider knows the key
/// that person signs with long before this node has learned their account. So
/// both are accepted, in the same JSON string field, and the encoding alone
/// decides which was meant.
///
/// That is unambiguous by construction rather than by luck. An account renders
/// as exactly 64 hex characters; [`crate::hash::Hash`] — the parser behind every
/// key — refuses any string longer than the widest base58 a 32-byte key can
/// produce, which is 45 characters. No account string can therefore be read as
/// a key, and no key string is 64 characters long. A caller who confuses the two
/// gets a parse error rather than a different principal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberPrincipal {
    /// The person, named directly — the principal the row is keyed by. Prefer
    /// this: it is what a member listing hands back, and it is what the key
    /// resolves to anyway.
    Account(AccountId),
    /// One signing key the person holds.
    ///
    /// A person may hold several, so this names strictly less than an account
    /// does. It is here for the one caller that has no choice: the subject is
    /// not known here as an account yet.
    Key(PublicKey),
}

impl MemberPrincipal {
    /// The account, when that is what was named.
    #[must_use]
    pub const fn account(&self) -> Option<AccountId> {
        match *self {
            Self::Account(account) => Some(account),
            Self::Key(_) => None,
        }
    }

    /// The key, when that is what was named.
    #[must_use]
    pub const fn key(&self) -> Option<PublicKey> {
        match *self {
            Self::Key(key) => Some(key),
            Self::Account(_) => None,
        }
    }
}

impl From<AccountId> for MemberPrincipal {
    fn from(account: AccountId) -> Self {
        Self::Account(account)
    }
}

impl From<PublicKey> for MemberPrincipal {
    fn from(key: PublicKey) -> Self {
        Self::Key(key)
    }
}

impl fmt::Display for MemberPrincipal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(account) => fmt::Display::fmt(account, f),
            Self::Key(key) => fmt::Display::fmt(key, f),
        }
    }
}

/// A string was neither encoding, so it names no member.
#[derive(Clone, Copy, Debug, Error)]
#[error(
    "expected an account (64 hex characters, as a member listing renders one) \
     or a signing key (base58)"
)]
pub struct InvalidMemberPrincipal;

impl FromStr for MemberPrincipal {
    type Err = InvalidMemberPrincipal;

    /// Accounts are tried first, so the wider parser never gets to claim one.
    /// The orderings agree here — a 64-character string is too long for the key
    /// parser — but relying on that would make the disambiguation a property of
    /// two parsers instead of a property of this function.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(account) = s.parse::<AccountId>() {
            return Ok(Self::Account(account));
        }
        s.parse::<PublicKey>()
            .map(Self::Key)
            .map_err(|_| InvalidMemberPrincipal)
    }
}

/// Serializes as the string form of whichever principal it holds, so a value
/// read back from a listing round-trips unchanged.
impl Serialize for MemberPrincipal {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MemberPrincipal {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <std::borrow::Cow<'de, str> as Deserialize>::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use core::mem::ManuallyDrop;

    use super::*;

    /// The disambiguation the whole two-in-one-field design rests on.
    #[test]
    fn an_account_and_a_key_each_parse_back_to_what_they_are() {
        let account = AccountId::from([0xA1; 32]);
        let key = PublicKey::from([0xB2; 32]);

        assert_eq!(
            account.to_string().parse::<MemberPrincipal>().unwrap(),
            MemberPrincipal::Account(account),
            "64 hex characters is an account"
        );
        assert_eq!(
            key.to_string().parse::<MemberPrincipal>().unwrap(),
            MemberPrincipal::Key(key),
            "base58 is a key"
        );
    }

    /// Neither encoding may be read as the other principal.
    ///
    /// Both are 32 bytes, so a shared encoding would let one be pasted where the
    /// other belongs and silently name somebody who exists nowhere. What rules
    /// that out is a length: a key parses through `Hash`, which refuses anything
    /// longer than the widest base58 32 bytes can produce, and an account is
    /// always longer than that.
    #[test]
    fn neither_encoding_can_be_mistaken_for_the_other() {
        let account = AccountId::from([0xA1; 32]).to_string();
        let key = PublicKey::from([0xB2; 32]).to_string();

        assert_eq!(account.len(), 64, "an account is always 64 characters");
        assert!(
            key.len() < 64,
            "and base58 of 32 bytes is always shorter than that"
        );
        assert!(
            account.parse::<PublicKey>().is_err(),
            "an account string must not parse as a key"
        );
        assert!(
            key.parse::<AccountId>().is_err(),
            "a key string must not parse as an account"
        );
    }

    #[test]
    fn a_string_that_is_neither_names_no_member() {
        for bad in ["", "not-an-id", "0xdeadbeef"] {
            assert!(
                bad.parse::<MemberPrincipal>().is_err(),
                "{bad:?} names no member"
            );
        }
    }

    /// A value read out of a listing and handed straight back must survive the
    /// round trip, or a caller could not use one endpoint's output as another's
    /// input.
    #[test]
    fn a_principal_serializes_as_the_string_it_parses_from() {
        for principal in [
            MemberPrincipal::Account(AccountId::from([0xA1; 32])),
            MemberPrincipal::Key(PublicKey::from([0xB2; 32])),
        ] {
            let json = serde_json::to_value(principal).unwrap();

            assert_eq!(json, serde_json::json!(principal.to_string()));
            assert_eq!(
                serde_json::from_value::<MemberPrincipal>(json).unwrap(),
                principal
            );
        }
    }

    #[test]
    fn test_private_key_zeroize_on_drop() {
        // Create a non-zero key wrapped in ManuallyDrop to control when drop occurs
        let secret_bytes: [u8; 32] = [0x42; 32];
        let mut key = ManuallyDrop::new(PrivateKey::from(secret_bytes));

        // Verify the key contains the expected bytes before drop
        assert_eq!(key.as_bytes(), &secret_bytes);

        // Get a raw pointer to the key's memory location before dropping
        let key_ptr = &*key as *const PrivateKey as *const u8;
        let key_size = core::mem::size_of::<PrivateKey>();

        // Manually drop the key, which will call the derived Drop implementation.
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
        let zeroed = unsafe { core::slice::from_raw_parts(key_ptr, key_size) };

        // Check that the entire key structure is zeroed, not just part of it
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
