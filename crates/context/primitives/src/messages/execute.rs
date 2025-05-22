use actix::Message;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::{ContextAtomic, ContextAtomicKey};

#[derive(Debug)]
pub struct ExecuteRequest {
    pub context: ContextId,
    pub executor: PublicKey,
    pub method: String,
    pub payload: Vec<u8>,
    pub aliases: Vec<Alias<PublicKey>>,
    pub atomic: Option<ContextAtomic>,
}

#[derive(Debug)]
pub struct ExecuteResponse {
    // fixme! this is an eyre::Result temporarily until calimero-runtime
    // fixme! exports it's primitives in a lightweight crate
    pub returns: eyre::Result<Option<Vec<u8>>>,
    pub logs: Vec<String>,
    pub events: Vec<ExecuteEvent>,
    pub root_hash: Hash,
    pub artifact: Vec<u8>,
    pub atomic: Option<ContextAtomicKey>,
}

#[derive(Debug)]
pub struct ExecuteEvent {
    pub kind: String,
    pub data: Vec<u8>,
}

impl Message for ExecuteRequest {
    type Result = Result<ExecuteResponse, ExecuteError>;
}

// todo! these types should not be serialize
// todo! the API should redefine its own types
// todo! which should prevent unintentional
// todo! changes to the API
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ExecuteError {
    #[error("context not found")]
    ContextNotFound,
    #[error("cannot execute request as '{public_key}' on context '{context_id}'")]
    Unauthorized {
        context_id: ContextId,
        public_key: PublicKey,
    },
    #[error("context state not initialized, awaiting state sync")]
    Uninitialized,
    #[error("application not installed: '{application_id}'")]
    ApplicationNotInstalled { application_id: ApplicationId },
    #[error("internal error")]
    InternalError,
    #[error("error resolving identity alias '{alias}'")]
    AliasResolutionFailed { alias: Alias<PublicKey> },
}
