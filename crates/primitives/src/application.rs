#[cfg(test)]
#[path = "tests/application.rs"]
mod tests;

use std::collections::BTreeMap;

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
    /// Sentinel value: no application / uninitialized application ID (all-zero hash).
    #[must_use]
    pub const fn zero() -> Self {
        Self(Hash::zero())
    }

    /// Bundle ids are version-stable: the package and signing key together
    /// decide which app a bundle IS, so every later version reuses the pair.
    #[cfg(feature = "borsh")]
    pub fn for_bundle(package: &str, signer_id: &str) -> eyre::Result<Self> {
        Ok(Self::from(*Hash::hash_borsh(&(package, signer_id))?))
    }
}

/// Sentinel value representing "no application" or an uninitialized application ID.
pub const ZERO_APPLICATION_ID: ApplicationId = ApplicationId::zero();

impl Display for ApplicationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, f)
    }
}

impl From<ApplicationId> for String {
    fn from(id: ApplicationId) -> Self {
        id.0.to_base58()
    }
}

impl From<&ApplicationId> for String {
    fn from(id: &ApplicationId) -> Self {
        id.0.to_base58()
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
        let len = u32::try_from(bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "SignerId too long to encode")
        })?;
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

/// A validated application version (`major.minor.patch` semver core, with an
/// optional `-prerelease` / `+build` suffix). A newtype so a raw, unvalidated
/// string can't masquerade as a version.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd, Serialize)]
pub struct Version(Box<str>);

/// Error returned when constructing an invalid [`Version`].
#[derive(Clone, Debug, ThisError)]
#[non_exhaustive]
pub enum InvalidVersion {
    /// The string is not `major.minor.patch` semver.
    #[error("version must be semver `major.minor.patch`, got `{0}`")]
    NotSemver(String),
}

impl Version {
    /// Parse and validate a semver version string.
    ///
    /// # Errors
    /// Returns [`InvalidVersion::NotSemver`] if `s` is not three dot-separated
    /// numeric components (optionally followed by a `-pre`/`+build` suffix).
    pub fn new(s: impl Into<Box<str>>) -> Result<Self, InvalidVersion> {
        let s = s.into();
        let core = s.split(['-', '+']).next().unwrap_or(&s);
        let mut parts = core.split('.');
        let numeric = |p: Option<&str>| {
            p.is_some_and(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
        };
        let valid = numeric(parts.next())
            && numeric(parts.next())
            && numeric(parts.next())
            && parts.next().is_none();
        if valid {
            Ok(Self(s))
        } else {
            Err(InvalidVersion::NotSemver(s.into_string()))
        }
    }

    /// The version as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl FromStr for Version {
    type Err = InvalidVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as Deserialize>::deserialize(deserializer)?;
        Self::new(s).map_err(SerdeError::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Application {
    pub id: ApplicationId,
    pub blob: ApplicationBlob,
    pub size: u64,
    pub source: ApplicationSource,
    pub metadata: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_id: Option<SignerId>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Version>,
    /// Named services. Key = service name, value = WASM blob.
    /// Empty for single-service apps (use `blob` field instead).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub services: BTreeMap<String, ApplicationBlob>,
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
            signer_id: None,
            package: String::new(),
            version: None,
            services: BTreeMap::new(),
        }
    }

    /// Resolve the blob for a given service name.
    /// None service_name returns default blob for single-service apps.
    pub fn resolve_service_blob(&self, service_name: Option<&str>) -> Option<ApplicationBlob> {
        match service_name {
            None if self.services.is_empty() => Some(self.blob),
            None if self.services.len() == 1 => self.services.values().next().copied(),
            None => None,
            Some(name) => self.services.get(name).copied(),
        }
    }

    #[must_use]
    pub fn with_bundle_info(mut self, signer_id: String, package: String, version: String) -> Self {
        // An empty or invalid signer id becomes `None` rather than being stored
        // as an unvalidated string.
        self.signer_id = SignerId::new(signer_id).ok();
        self.package = package;
        // Validate the version; an empty or non-semver string becomes `None`
        // rather than silently storing an invalid version.
        self.version = Version::new(version).ok();
        self
    }
}
