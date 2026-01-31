#[cfg(test)]
#[path = "tests/application.rs"]
mod tests;

use core::fmt::{self, Display, Formatter};
use core::ops::Deref;
use core::str::FromStr;
#[cfg(feature = "borsh")]
use std::io;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::de::{Error as SerdeError, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error as ThisError;
use url::{ParseError, Url};

use crate::blobs::BlobId;
use crate::hash::{Hash, HashError};

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, Ord, PartialOrd)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
// todo! define macros that construct newtypes
// todo! wrapping Hash<N> with this interface
pub struct ApplicationId(Hash);

impl From<[u8; 32]> for ApplicationId {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl AsRef<[u8; 32]> for ApplicationId {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for ApplicationId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ApplicationId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for ApplicationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<ApplicationId> for String {
    fn from(id: ApplicationId) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&ApplicationId> for String {
    fn from(id: &ApplicationId) -> Self {
        id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[error(transparent)]
pub struct InvalidApplicationId(HashError);

impl FromStr for ApplicationId {
    type Err = InvalidApplicationId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidApplicationId)?))
    }
}

/// Signer identifier derived from the Ed25519 public key that signs the MPK bundle.
/// Establishes cryptographic update authority. Must be non-empty.
/// In v0, encoded as did:key: `did:key:z{base58btc(0xed01 || public_key)}`.

#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct SignerId(Box<str>);

impl SignerId {
    /// Creates a new `SignerId` from a string.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSignerId::Empty`] if the string is empty.
    pub fn new(s: impl Into<Box<str>>) -> Result<Self, InvalidSignerId> {
        let s = s.into();
        if s.is_empty() {
            return Err(InvalidSignerId::Empty);
        }
        Ok(Self(s))
    }

    /// Returns the signerId as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for SignerId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for SignerId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for SignerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl From<SignerId> for String {
    fn from(id: SignerId) -> Self {
        id.0.into_string()
    }
}

impl From<&SignerId> for String {
    fn from(id: &SignerId) -> Self {
        id.0.to_string()
    }
}

/// Error type for invalid signer identifiers.
#[derive(Clone, Copy, Debug, ThisError)]
#[non_exhaustive]
pub enum InvalidSignerId {
    /// The signerId string is empty.
    #[error("signerId cannot be empty")]
    Empty,
}

impl FromStr for SignerId {
    type Err = InvalidSignerId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for SignerId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SignerId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SignerIdVisitor;

        impl Visitor<'_> for SignerIdVisitor {
            type Value = SignerId;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("a non-empty signer identifier string")
            }

            fn visit_str<E: SerdeError>(self, v: &str) -> Result<Self::Value, E> {
                SignerId::new(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(SignerIdVisitor)
    }
}

#[cfg(feature = "borsh")]
impl BorshSerialize for SignerId {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        // Serialize as length-prefixed bytes
        let bytes = self.0.as_bytes();
        let len = bytes.len() as u32;
        BorshSerialize::serialize(&len, writer)?;
        writer.write_all(bytes)
    }
}

#[cfg(feature = "borsh")]
impl BorshDeserialize for SignerId {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let len = u32::deserialize_reader(reader)? as usize;
        let mut bytes = vec![0u8; len];
        reader.read_exact(&mut bytes)?;

        let s =
            String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        SignerId::new(s).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// Stable application identity: (appId, signerId). An app is uniquely
/// identified by its AppKey. appId is the manifest `package` (human-friendly label, not
/// a security boundary); signerId is the cryptographic update authority.
/// Display format is `{app_id}:{signer_id}`, used as keys in Desired State Documents.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct AppKey {
    /// The application identifier (package name from manifest).
    app_id: Box<str>,
    /// The signer identifier (did:key format).
    signer_id: SignerId,
}

impl AppKey {
    /// Creates a new `AppKey` from an app ID and signer ID.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidAppKey::EmptyAppId`] if the app_id is empty.
    pub fn new(app_id: impl Into<Box<str>>, signer_id: SignerId) -> Result<Self, InvalidAppKey> {
        let app_id = app_id.into();
        if app_id.is_empty() {
            return Err(InvalidAppKey::EmptyAppId);
        }
        Ok(Self { app_id, signer_id })
    }

    /// Returns the application identifier.
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Returns the signer identifier.
    #[must_use]
    pub fn signer_id(&self) -> &SignerId {
        &self.signer_id
    }
}

impl Display for AppKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.app_id, self.signer_id)
    }
}

impl From<AppKey> for String {
    fn from(key: AppKey) -> Self {
        key.to_string()
    }
}

impl From<&AppKey> for String {
    fn from(key: &AppKey) -> Self {
        key.to_string()
    }
}

/// Error type for invalid AppKey.
#[derive(Clone, Debug, ThisError)]
#[non_exhaustive]
pub enum InvalidAppKey {
    /// The appId is empty.
    #[error("appId cannot be empty")]
    EmptyAppId,

    /// The signerId is invalid.
    #[error("invalid signerId: {0}")]
    InvalidSignerId(#[from] InvalidSignerId),

    /// The AppKey string format is invalid (missing separator).
    #[error("invalid AppKey format: expected 'appId:signerId', got '{0}'")]
    InvalidFormat(String),
}

impl FromStr for AppKey {
    type Err = InvalidAppKey;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Find the first colon separator
        // Note: signerId (did:key:...) contains colons, so we split on the first colon only
        let separator_pos = s
            .find(':')
            .ok_or_else(|| InvalidAppKey::InvalidFormat(s.to_owned()))?;

        let app_id = &s[..separator_pos];
        let signer_id_str = &s[separator_pos + 1..];

        let signer_id = SignerId::new(signer_id_str)?;
        Self::new(app_id, signer_id)
    }
}

impl Serialize for AppKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AppKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AppKeyVisitor;

        impl Visitor<'_> for AppKeyVisitor {
            type Value = AppKey;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string in format 'appId:signerId'")
            }

            fn visit_str<E: SerdeError>(self, v: &str) -> Result<Self::Value, E> {
                AppKey::from_str(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(AppKeyVisitor)
    }
}

#[cfg(feature = "borsh")]
impl BorshSerialize for AppKey {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        // Serialize app_id as length-prefixed bytes
        let app_id_bytes = self.app_id.as_bytes();
        let app_id_len = app_id_bytes.len() as u32;
        BorshSerialize::serialize(&app_id_len, writer)?;
        writer.write_all(app_id_bytes)?;

        // Serialize signer_id
        BorshSerialize::serialize(&self.signer_id, writer)
    }
}

#[cfg(feature = "borsh")]
impl BorshDeserialize for AppKey {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        // Deserialize app_id
        let app_id_len = u32::deserialize_reader(reader)? as usize;
        let mut app_id_bytes = vec![0u8; app_id_len];
        reader.read_exact(&mut app_id_bytes)?;
        let app_id = String::from_utf8(app_id_bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .into_boxed_str();

        // Deserialize signer_id
        let signer_id = SignerId::deserialize_reader(reader)?;

        AppKey::new(app_id, signer_id).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ApplicationSource(Url);

impl FromStr for ApplicationSource {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl From<Url> for ApplicationSource {
    fn from(value: Url) -> Self {
        Self(value)
    }
}

impl From<ApplicationSource> for Url {
    fn from(value: ApplicationSource) -> Self {
        value.0
    }
}

impl Display for ApplicationSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshDeserialize, borsh::BorshSerialize)
)]
pub struct ApplicationBlob {
    pub bytecode: BlobId,
    pub compiled: BlobId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Application {
    pub id: ApplicationId,
    pub blob: ApplicationBlob,
    pub size: u64,
    pub source: ApplicationSource,
    pub metadata: Vec<u8>,
}

impl Application {
    #[must_use]
    pub const fn new(
        id: ApplicationId,
        blob: ApplicationBlob,
        size: u64,
        source: ApplicationSource,
        metadata: Vec<u8>,
    ) -> Self {
        Self {
            id,
            blob,
            size,
            source,
            metadata,
        }
    }
}
