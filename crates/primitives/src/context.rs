use core::fmt::{self, Display, Formatter};
use core::ops::Deref;
use core::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::application::ApplicationId;
use crate::hash::{Hash, HashError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
// todo! define macros that construct newtypes
// todo! wrapping Hash<N> with this interface
pub struct ContextId(Hash);

impl From<[u8; 32]> for ContextId {
    fn from(id: [u8; 32]) -> Self {
        Self(id.into())
    }
}

impl Deref for ContextId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContextId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for ContextId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl From<ContextId> for String {
    fn from(id: ContextId) -> Self {
        id.as_str().to_owned()
    }
}

impl From<&ContextId> for String {
    fn from(id: &ContextId) -> Self {
        id.as_str().to_owned()
    }
}

#[derive(Clone, Copy, Debug, ThisError)]
#[error(transparent)]
pub struct InvalidContextId(HashError);

impl FromStr for ContextId {
    type Err = InvalidContextId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.parse().map_err(InvalidContextId)?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Context {
    pub id: ContextId,
    pub application_id: ApplicationId,
    pub last_transaction_hash: Hash,
}

impl Context {
    #[must_use]
    pub const fn new(
        id: ContextId,
        application_id: ApplicationId,
        last_transaction_hash: Hash,
    ) -> Self {
        Self {
            id,
            application_id,
            last_transaction_hash,
        }
    }
}
