use std::ops::Deref;

use serde::{Deserialize, Serialize};

use crate::application::ApplicationId;

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct ContextId([u8; 32]);

impl From<[u8; 32]> for ContextId {
    fn from(id: [u8; 32]) -> Self {
        Self(id)
    }
}

impl Deref for ContextId {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub id: ContextId,
    pub application_id: ApplicationId,
}
