use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context::group_store;
use calimero_context_client::group::{JoinContextRequest, ListGroupContextsRequest};
use calimero_context_config::types::ContextGroupId;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_server_primitives::admin::FleetJoinRequest;
use calimero_tee_attestation::{build_report_data, generate_attestation};
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<FleetJoinRequest>,
) -> impl IntoResponse {
    let group_id_bytes = match hex::decode(&req.group_id) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        _ => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "group_id must be 64 hex chars (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };

    let group_id = ContextGroupId::from(group_id_bytes);

    info!(
        group_id = %req.group_id,
        "Fleet join: resolving namespace identity and generating attestation"
    );

    // Use namespace identity (per-root-group keypair) instead of a throwaway identity
    let (ns_id, our_public_key, our_sk, _sender) =
        match group_store::get_or_create_namespace_identity(&state.store, &group_id) {
            Ok(result) => result,
            Err(err) => {
                error!(error=?err, "Failed to resolve namespace identity");
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to resolve namespace identity".to_owned(),
                }
                .into_response();
            }
        };

    info!(
        %our_public_key,
        namespace_id = %hex::encode(ns_id.to_bytes()),
        "Using namespace identity for fleet join"
    );

    let pk_hash: [u8; 32] = Sha256::digest(&*our_public_key).into();
    let nonce: [u8; 32] = rand::random();
    let report_data = build_report_data(&nonce, Some(&pk_hash));

    let attestation = match generate_attestation(report_data) {
        Ok(result) => {
            if result.is_mock {
                error!("Mock attestation generated -- fleet-join requires real TDX hardware");
                return ApiError {
                    status_code: StatusCode::NOT_IMPLEMENTED,
                    message: "TDX attestation required -- mock not accepted for fleet join"
                        .to_owned(),
                }
                .into_response();
            }
            result
        }
        Err(err) => {
            error!(error=?err, "Failed to generate TDX attestation");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to generate attestation".to_owned(),
            }
            .into_response();
        }
    };

    let broadcast = BroadcastMessage::TeeAttestationAnnounce {
        quote_bytes: attestation.quote_bytes,
        public_key: our_public_key,
        nonce,
        node_type: SpecializedNodeType::ReadOnly,
    };

    let payload = match borsh::to_vec(&broadcast) {
        Ok(p) => p,
        Err(err) => {
            error!(error=?err, "Failed to serialize TeeAttestationAnnounce");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to serialize announcement".to_owned(),
            }
            .into_response();
        }
    };

    if let Err(err) = state.node_client.subscribe_namespace(group_id_bytes).await {
        error!(error=?err, "Failed to subscribe to namespace topic");
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to subscribe to namespace".to_owned(),
        }
        .into_response();
    }

    if let Err(err) = state
        .node_client
        .publish_on_namespace(group_id_bytes, payload)
        .await
    {
        warn!(error=?err, "Failed to broadcast, unsubscribing from namespace");
        let _ = state
            .node_client
            .unsubscribe_namespace(group_id_bytes)
            .await;
        return ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Failed to broadcast attestation".to_owned(),
        }
        .into_response();
    }

    info!(
        group_id = %req.group_id,
        %our_public_key,
        "TeeAttestationAnnounce broadcast; waiting for admission then joining contexts"
    );

    // Poll for group admission, then auto-join all contexts in the namespace
    let mut contexts_joined = Vec::new();
    let mut admitted = false;
    let mut auto_follow_enabled = false;

    const MAX_ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    const ADMISSION_POLL: std::time::Duration = std::time::Duration::from_secs(2);

    let deadline = tokio::time::Instant::now() + MAX_ADMISSION_WAIT;

    while tokio::time::Instant::now() < deadline {
        match state
            .ctx_client
            .list_group_contexts(ListGroupContextsRequest {
                group_id,
                offset: 0,
                limit: 100,
            })
            .await
        {
            Ok(entries) => {
                info!(
                    group_id = %req.group_id,
                    context_count = entries.len(),
                    "Admitted to group, joining contexts"
                );
                admitted = true;

                for entry in &entries {
                    match state
                        .ctx_client
                        .join_context(JoinContextRequest {
                            context_id: entry.context_id,
                        })
                        .await
                    {
                        Ok(resp) => {
                            info!(
                                context_id = %hex::encode(*resp.context_id),
                                "Joined context via group membership"
                            );
                            contexts_joined.push(hex::encode(*resp.context_id));
                        }
                        Err(err) => {
                            warn!(
                                context_id = %hex::encode(*entry.context_id),
                                error = ?err,
                                "Failed to join context (may already be joined)"
                            );
                        }
                    }
                }

                // Self-enable auto-follow now that we're a confirmed member.
                // Signed with our own namespace identity — satisfies the
                // admin-or-self authorization rule for MemberSetAutoFollow
                // (see the auto-follow architecture doc). The verifier that admitted us cannot do
                // this on our behalf because they're usually not admin and
                // don't hold our signing key. From here on, any new context
                // in the group auto-joins via the core auto-follow handler;
                // no sidecar polling needed.
                let our_sk_typed = calimero_primitives::identity::PrivateKey::from(our_sk);
                match calimero_context::group_store::sign_apply_and_publish(
                    &state.store,
                    &state.node_client,
                    state.ctx_client.ack_router(),
                    &group_id,
                    &our_sk_typed,
                    calimero_context_client::local_governance::GroupOp::MemberSetAutoFollow {
                        target: our_public_key,
                        auto_follow_contexts: true,
                        auto_follow_subgroups: true,
                    },
                )
                .await
                {
                    Ok(_report) => {
                        info!(
                            group_id = %req.group_id,
                            "fleet-join: auto-follow enabled for self"
                        );
                        auto_follow_enabled = true;
                    }
                    Err(err) => warn!(
                        group_id = %req.group_id,
                        ?err,
                        "fleet-join: failed to enable auto-follow — admission succeeded but \
                         subsequent contexts will NOT auto-join until the op is retried. \
                         Operators can re-trigger fleet-join or publish MemberSetAutoFollow."
                    ),
                }

                break;
            }
            Err(err) => {
                tracing::debug!(error=?err, "Admission check not yet successful, retrying...");
                tokio::time::sleep(ADMISSION_POLL).await;
            }
        }
    }

    if !admitted {
        warn!(
            group_id = %req.group_id,
            "Timed out waiting for group admission"
        );
    }

    ApiResponse {
        payload: serde_json::json!({
            "status": if admitted { "joined" } else { "announced" },
            "group_id": req.group_id,
            "namespace_id": hex::encode(ns_id.to_bytes()),
            "public_key": our_public_key.to_string(),
            "admitted": admitted,
            "auto_follow_enabled": auto_follow_enabled,
            "contexts_joined": contexts_joined,
        }),
    }
    .into_response()
}
