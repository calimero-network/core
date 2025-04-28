use std::collections::{btree_map, BTreeMap};
use std::mem;
use std::sync::Arc;

use actix::fut::wrap_future;
use actix::{ActorResponse, ActorTryFutureExt, Handler, Message};
use calimero_context_primitives::messages::join_context::{
    JoinContextRequest, JoinContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::layer::ReadLayer;
use calimero_store::{key, types, Store};
use eyre::{bail, OptionExt};
use tokio::sync::{Mutex, OwnedMutexGuard};

use super::execute::execute;
use super::execute::storage::ContextStorage;
use crate::{ContextManager, ContextMeta};

impl Handler<JoinContextRequest> for ContextManager {
    type Result = <JoinContextResponse as Message>::Result;

    fn handle(
        &mut self,
        JoinContextRequest {
            identity_secret,
            invitation_payload,
        }: DeleteContextResponse,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        Ok(None)
    }
}
