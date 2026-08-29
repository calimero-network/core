//! `POST /admin-api/namespaces/:namespace_id/admit` — publish a join a joiner
//! signed but cannot publish itself.
//!
//! The joiner may hold no node at all. That is the case direct admission exists
//! for: an account with a key, a device certificate signed offline, and nowhere
//! to publish from.
//!
//! **The admitter relays; it does not author.** Every peer checks
//! `signer == credential.statement.sign_pk` when applying a join, so an admitter
//! that tried to sign on the joiner's behalf would produce an op the network
//! rejects. The op arrives already signed, and this node's contribution is to
//! decide whether to carry it — which is exactly the authority the invitation's
//! `admitters` list confers.
//!
//! What that buys: a hostile admitter can refuse to publish, and nothing else.
//! It cannot admit a different account, alter the group, or grant itself a role,
//! because all of that is inside a signature it does not hold.
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{AdmitJoinApiRequest, AdmitJoinApiResponse};
use reqwest::StatusCode;
use tracing::info;

use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::identity::get_node_identity::node_identity;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<AdmitJoinApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    if req.invitation.invitation.group_id != namespace_id {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "invitation group_id does not match namespace_id in path".into(),
        }
        .into_response();
    }

    let signed_op_bytes = match hex::decode(&req.signed_op) {
        Ok(bytes) => bytes,
        Err(e) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("signed_op is not hex: {e}"),
            }
            .into_response();
        }
    };

    // Decoded rather than relayed blind. Publishing opaque bytes would make a
    // designated admitter into a general-purpose injector for the namespace
    // topic — anything handed to it, signed by anyone, published under its own
    // connection. Decoding costs one borsh parse and bounds what this endpoint
    // can be used for.
    let op: SignedNamespaceOp = match borsh::from_slice(&signed_op_bytes) {
        Ok(op) => op,
        Err(e) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("signed_op is not a SignedNamespaceOp: {e}"),
            }
            .into_response();
        }
    };

    if op.namespace_id.to_bytes() != namespace_id.to_bytes() {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "signed_op is for a different namespace than the path names".to_owned(),
        }
        .into_response();
    }

    if !carries_a_join(&op.op) {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: "signed_op is not a join; an admitter carries joins only".to_owned(),
        }
        .into_response();
    }

    // Derived from the op, as the governance publisher does. The receiver
    // ignores both today, but zeros would be wrong the moment anything reads
    // them for dedup or parent links.
    let delta_id = match op.content_hash() {
        Ok(hash) => hash,
        Err(e) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: format!("signed_op has no content hash: {e}"),
            }
            .into_response();
        }
    };
    let parent_ids = op.parent_op_hashes.clone();

    // Refuse before publishing, never after: once an op reaches the topic it is
    // on every peer's DAG, so "was this node entitled to carry it" has to be
    // answered while the answer still changes anything.
    let self_account = match node_identity(&state.store) {
        Ok(Some((account, ..))) => account,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::CONFLICT,
                message: "this node holds no device, so it cannot admit anyone".to_owned(),
            }
            .into_response();
        }
        Err(err) => return parse_api_error(err).into_response(),
    };

    if let Err(err) = calimero_governance_store::NamespaceMembershipService::require_may_admit(
        &req.invitation,
        &self_account,
    ) {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: err.to_string(),
        }
        .into_response();
    }

    // Being a designated admitter is permission to carry a *valid* claim, not
    // permission to skip checking it: inviter signature, the group belonging to
    // this namespace, the inviter's permission to invite, and expiry.
    if let Err(err) = calimero_governance_store::NamespaceMembershipService::new(
        &state.store,
        namespace_id.to_bytes().into(),
    )
    .validate_open_invitation(&req.invitation, calimero_governance_store::now_secs())
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: format!("invitation rejected: {err}"),
        }
        .into_response();
    }

    // Published as-is. The op is signed by the joiner's device key and every
    // peer checks that on apply, so this node cannot alter who joined, which
    // group, or with what role — it can only decline to carry it.
    if let Err(err) = state
        .node_client
        .publish_signed_namespace_op(
            namespace_id.to_bytes(),
            delta_id,
            parent_ids,
            signed_op_bytes,
        )
        .await
    {
        return parse_api_error(err).into_response();
    }

    info!(namespace_id=%namespace_id_str, "admitted a joiner's signed join op");

    ApiResponse {
        payload: AdmitJoinApiResponse { published: true },
    }
    .into_response()
}

/// Whether this op is one an admitter is designated to carry.
///
/// Only a join. Being named in `admitters` is authority to admit somebody, not
/// to publish governance at large on their behalf — without this an admitter is
/// a general-purpose injector for the namespace topic, publishing whatever it is
/// handed under its own connection.
///
/// `MemberJoinedOpen` is deliberately absent: it carries no invitation, so there
/// is nothing naming this node as entitled to carry it.
const fn carries_a_join(op: &NamespaceOp) -> bool {
    matches!(
        op,
        NamespaceOp::Root(RootOp::MemberJoined { .. } | RootOp::MemberJoinedAt { .. })
    )
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};

    use super::carries_a_join;

    #[test]
    fn a_join_is_carried() {
        let genesis = calimero_account::AccountGenesis::new(
            calimero_primitives::identity::PublicKey::from([0u8; 32]),
        );
        let credential = calimero_context_client::local_governance::JoinAccountCredential {
            statement: calimero_account::DeviceCert {
                account: genesis.account_id(),
                device: calimero_account::DeviceId::from([0u8; 32]),
                sign_pk: calimero_primitives::identity::PublicKey::from([0u8; 32]),
                kem_pk: calimero_account::KemPublicKey::from([0u8; 32]),
                key_epoch: 0,
                device_epoch: 0,
                signature: [0u8; 64],
            },
            genesis,
            chain: Vec::new(),
        };
        let invitation = calimero_context_config::types::SignedGroupOpenInvitation {
            inviter_account: None,
            invitation: calimero_context_config::types::GroupInvitationFromAdmin {
                inviter_identity: calimero_context_config::types::SignerId::from([0u8; 32]),
                group_id: calimero_context_config::types::ContextGroupId::from([0u8; 32]),
                expiration_timestamp: 0,
                invitation_nonce: [0u8; 32],
                invited_role: 1,
                admitters: Vec::new(),
            },
            inviter_signature: String::new(),
            admitter_hints: Vec::new(),
            application_id: None,
            bytecode_id: None,
        };

        assert!(carries_a_join(&NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_account::AccountId::from([0u8; 32]),
            signed_invitation: invitation,
            account: Box::new(credential),
        })));
    }

    /// Governance that is not a join must be refused.
    ///
    /// This is the rule that stops a designated admitter being used to publish
    /// arbitrary namespace governance signed by whoever asked.
    #[test]
    fn governance_that_is_not_a_join_is_refused() {
        let admin_changed = NamespaceOp::Root(RootOp::AdminChanged {
            new_admin: calimero_account::AccountId::from([0x11; 32]),
        });
        assert!(!carries_a_join(&admin_changed));

        let policy = NamespaceOp::Root(RootOp::PolicyUpdated {
            policy_bytes: Vec::new(),
        });
        assert!(!carries_a_join(&policy));
    }
}
