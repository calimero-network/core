//! `LinkAccountDeviceRequest` handler - carry a device link for an account this
//! node does not hold.
//!
//! The account has no node of its own. Its root certified a device offline, and
//! the account was added to a group by account (`MemberAdded`), so the namespace
//! binds none of its keys: anything the device signs there - an invitation, most
//! visibly - is refused by every peer as signed by a key bound to no account.
//!
//! `AccountDeviceLinked` closes that gap and is self-certifying: the root signed
//! both the certificate and the scope, so any member may carry it. What only a
//! member can add is the endorsement, which the apply gate requires because an
//! account root is a member nowhere. This node signs it with its own member key
//! and publishes the op; every peer then binds the device key to the account and
//! judges what it signs on the account's own grants.
//!
//! No scope key is delivered. The device has no node to read one with, and what
//! it signs where it lives - invitations, warrants - needs only the binding.

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{LinkAccountDeviceRequest, LinkAccountDeviceResponse};
use calimero_governance_store::{
    plan_carried_link, publish_carried_link, CarriedLink, CarriedLinkRefusal,
};
use calimero_primitives::identity::PrivateKey;

use crate::error::ContextError;
use crate::ContextManager;

/// The status class a refusal belongs to: re-sign (`400`) or stop (`403`).
fn refusal(err: &CarriedLinkRefusal) -> ContextError {
    let reason = err.to_string();
    match err {
        CarriedLinkRefusal::Revoked { .. } | CarriedLinkRefusal::NotAMember { .. } => {
            ContextError::DeviceLinkRefused { reason }
        }
        _ => ContextError::DeviceLinkInvalid { reason },
    }
}

impl Handler<LinkAccountDeviceRequest> for ContextManager {
    type Result = ActorResponse<Self, <LinkAccountDeviceRequest as Message>::Result>;

    fn handle(
        &mut self,
        LinkAccountDeviceRequest {
            namespace_id,
            credential,
            scope,
        }: LinkAccountDeviceRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let (self_pk, signer_sk_bytes) = match self.require_namespace_signing_key(&namespace_id) {
            Ok(key) => key,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let signer_sk = PrivateKey::from(signer_sk_bytes);
        let store = self.datastore.clone();

        // The endorsement is this node's key vouching as a member: a key bound to
        // no account here endorses nobody, and every peer would drop the link.
        if let Err(err) = crate::member_account::require(&store, &namespace_id, &self_pk) {
            return ActorResponse::reply(Err(err));
        }

        let account = credential.statement.account;
        let device = credential.statement.device;
        let link = match plan_carried_link(&store, &namespace_id, &signer_sk, &credential, &scope) {
            Ok(Ok(CarriedLink::AlreadyBound)) => {
                return ActorResponse::reply(Ok(LinkAccountDeviceResponse::new(
                    account, device, true,
                )))
            }
            Ok(Ok(CarriedLink::Publish(link))) => *link,
            Ok(Err(err)) => return ActorResponse::reply(Err(refusal(&err).into())),
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let node_client = self.node_client.clone();
        let ack_router = Arc::clone(&self.ack_router);

        ActorResponse::r#async(
            async move {
                let bound = publish_carried_link(
                    &store,
                    &node_client,
                    &ack_router,
                    &namespace_id,
                    &signer_sk,
                    link,
                )
                .await?;
                // The local apply writes the binding or declines to; declining
                // after the plan admitted it means this node and the plan
                // disagree, and a success would send the caller off to sign
                // invitations every peer refuses.
                if !bound {
                    eyre::bail!(
                        "the device link for {device} was published but {namespace_id:?} \
                         did not record the binding"
                    );
                }
                Ok(LinkAccountDeviceResponse::new(account, device, false))
            }
            .into_actor(self),
        )
    }
}
