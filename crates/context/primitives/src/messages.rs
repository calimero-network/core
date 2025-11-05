use actix::Message;
use calimero_context_config::types::SignedRevealPayload;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use tokio::sync::oneshot;

use crate::{ContextAtomic, ContextAtomicKey};

#[derive(Debug)]
pub struct CreateContextRequest {
    pub protocol: String,
    pub seed: Option<[u8; 32]>,
    pub application_id: ApplicationId,
    pub identity_secret: Option<PrivateKey>,
    pub init_params: Vec<u8>,
}

impl Message for CreateContextRequest {
    type Result = eyre::Result<CreateContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct CreateContextResponse {
    pub context_id: ContextId,
    pub identity: PublicKey,
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteContextRequest {
    pub context_id: ContextId,
}

impl Message for DeleteContextRequest {
    type Result = eyre::Result<DeleteContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteContextResponse {
    pub deleted: bool,
}

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
    pub handler: Option<String>,
}

impl Message for ExecuteRequest {
    type Result = Result<ExecuteResponse, ExecuteError>;
}

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

#[derive(Debug)]
pub struct JoinContextRequest {
    pub invitation_payload: ContextInvitationPayload,
}

impl Message for JoinContextRequest {
    type Result = eyre::Result<JoinContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct JoinContextResponse {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

#[derive(Debug)]
pub struct JoinContextOpenInvitationRequest {
    pub payload: SignedRevealPayload,
}

impl Message for JoinContextOpenInvitationRequest {
    type Result = eyre::Result<JoinContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct SyncRequest {
    pub context_id: ContextId,
    pub application_id: ApplicationId,
}

impl Message for SyncRequest {
    type Result = ();
}

#[derive(Debug)]
pub struct UpdateApplicationRequest {
    pub context_id: ContextId,
    pub application_id: ApplicationId,
    pub public_key: PublicKey,
}

impl Message for UpdateApplicationRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug, Message)]
#[rtype("()")]
pub enum ContextMessage {
    Execute {
        request: ExecuteRequest,
        outcome: oneshot::Sender<<ExecuteRequest as Message>::Result>,
    },
    CreateContext {
        request: CreateContextRequest,
        outcome: oneshot::Sender<<CreateContextRequest as Message>::Result>,
    },
    DeleteContext {
        request: DeleteContextRequest,
        outcome: oneshot::Sender<<DeleteContextRequest as Message>::Result>,
    },
    JoinContext {
        request: JoinContextRequest,
        outcome: oneshot::Sender<<JoinContextRequest as Message>::Result>,
    },
    JoinContextOpenInvitation {
        request: JoinContextOpenInvitationRequest,
        outcome: oneshot::Sender<<JoinContextOpenInvitationRequest as Message>::Result>,
    },
    UpdateApplication {
        request: UpdateApplicationRequest,
        outcome: oneshot::Sender<<UpdateApplicationRequest as Message>::Result>,
    },
    Sync {
        request: SyncRequest,
        outcome: oneshot::Sender<<SyncRequest as Message>::Result>,
    },
    /// Invalidate the cache for a context (forces reload from DB on next access)
    RefreshContextMetadata { context_id: ContextId },
}
