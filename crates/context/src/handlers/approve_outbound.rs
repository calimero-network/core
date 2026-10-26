//! Approve URL prefixes for an app's publisher in a namespace.
//!
//! Called by the server after an install into a namespace or an upgrade that
//! asked for it (`approveOutbound`), and by the namespace's own approvals
//! endpoint. Publishes a `GroupOp::OutboundApprovalSet` at the namespace root,
//! whose apply decides whether the signer may (admin or `MANAGE_APPLICATION`).

use std::sync::Arc;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{ApproveOutboundRequest, ApproveOutboundResponse};
use calimero_context_client::local_governance::GroupOp;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_governance_store::{NamespaceRepository, OutboundApprovalRepository};
use eyre::{eyre, Result as EyreResult};
use tracing::info;

use crate::outbound::{app_outbound, merge_approval, Approval};
use crate::ContextManager;

impl Handler<ApproveOutboundRequest> for ContextManager {
    type Result = ActorResponse<Self, <ApproveOutboundRequest as Message>::Result>;

    fn handle(
        &mut self,
        request: ApproveOutboundRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let planned = match self.plan_outbound_approval(&request) {
            Ok(planned) => planned,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let (response, op) = match planned {
            Planned::Nothing(response) => return ActorResponse::reply(Ok(response)),
            Planned::Publish(response, op) => (response, op),
        };
        // Admin is not required here: the apply also admits MANAGE_APPLICATION.
        let preflight = match self.governance_preflight(&response.namespace_id, false) {
            Ok(preflight) => preflight,
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let ack_router = Arc::clone(&self.ack_router);
        ActorResponse::r#async(
            async move {
                let report = calimero_governance_store::sign_apply_and_publish_required(
                    &preflight.datastore,
                    &preflight.node_client,
                    &ack_router,
                    &response.namespace_id,
                    &preflight.signing_key,
                    *op,
                )
                .await?;
                report.observe("approve_outbound", "OutboundApprovalSet");
                info!(
                    namespace_id = ?response.namespace_id,
                    signer = %response.signer,
                    approved = response.prefixes.len(),
                    "outbound approval updated"
                );
                Ok(response)
            }
            .into_actor(self),
        )
    }
}

enum Planned {
    Nothing(ApproveOutboundResponse),
    Publish(ApproveOutboundResponse, Box<GroupOp>),
}

impl ContextManager {
    fn plan_outbound_approval(&self, request: &ApproveOutboundRequest) -> EyreResult<Planned> {
        let namespace_id = NamespaceRepository::new(&self.datastore).resolve(&request.group_id)?;
        let app = self
            .node_client
            .get_application(&request.application_id)?
            .ok_or_else(|| eyre!("application {} is not installed", request.application_id))?;
        let Some((identity, declared)) = app_outbound(&app) else {
            // Nothing to approve: the app makes no outbound requests.
            return Ok(Planned::Nothing(ApproveOutboundResponse {
                namespace_id,
                signer: app.signer_id.map(|s| s.to_string()).unwrap_or_default(),
                prefixes: Vec::new(),
                reviewed: Vec::new(),
                published: false,
            }));
        };
        let existing = OutboundApprovalRepository::new(&self.datastore)
            .get(&namespace_id, &identity.signer)?
            .map(|a| Approval {
                prefixes: a.prefixes,
                reviewed: a.reviewed,
            });
        let merged = merge_approval(
            existing.as_ref(),
            &declared,
            &request.selection,
            request.replace,
        )?;
        let response = ApproveOutboundResponse {
            namespace_id,
            signer: identity.signer.clone(),
            prefixes: merged.prefixes.clone(),
            reviewed: merged.reviewed.clone(),
            published: existing.as_ref() != Some(&merged),
        };
        if !response.published {
            return Ok(Planned::Nothing(response));
        }
        Ok(Planned::Publish(
            response,
            Box::new(GroupOp::OutboundApprovalSet {
                signer: identity.signer,
                prefixes: merged.prefixes,
                reviewed: merged.reviewed,
            }),
        ))
    }
}
