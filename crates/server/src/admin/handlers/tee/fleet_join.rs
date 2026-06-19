use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context::governance_broadcast::ObserveDelivery;
use calimero_context::group_store::NamespaceRepository;
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
        match NamespaceRepository::new(&state.store).get_or_create_identity(&group_id) {
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

    let pk_hash: [u8; 32] = Sha256::digest(*our_public_key).into();
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

    // Fire the first announce up front. A single publish at fleet-join time is
    // lost forever if it lands in an empty gossipsub mesh (no replay), which is
    // the common case for a NAT'd/relay owner whose mesh forms only
    // intermittently. The admission loop below therefore RE-announces every
    // poll cycle until admitted or the deadline, so a *later* mesh window still
    // receives a fresh copy. If even this first publish errors at the transport
    // level we bail (subscription with no announce is useless); a publish into
    // an empty mesh is *not* an error and is expected to be retried below.
    if let Err(err) = state
        .node_client
        .publish_on_namespace_now(group_id_bytes, payload.clone())
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
        "TeeAttestationAnnounce broadcast; re-announcing until admission then joining contexts"
    );

    // Poll for group admission, then auto-join all contexts in the namespace.
    //
    // Re-announce strategy: this loop both (a) checks for admission and (b)
    // re-publishes the announce each cycle the node is not yet admitted. The
    // re-announce is request-scoped (bounded by `MAX_ADMISSION_WAIT`) rather
    // than a long-lived background task: the mdma sidecar already re-polls
    // should-join and re-invokes fleet-join, so each call covering one mesh
    // window is sufficient, and a request-scoped loop needs no extra actor /
    // lifecycle management. See the handler-level rationale comment.
    let mut contexts_joined = Vec::new();
    let mut admitted = false;
    let mut auto_follow_enabled = false;

    // Overall bound for one fleet-join call. The sidecar re-invokes across a
    // larger window, so this only needs to cover a single mesh-formation
    // attempt comfortably.
    const MAX_ADMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    // Interval between admission checks AND between re-announces — short enough
    // that a transient mesh window (mesh peers appear, then vanish) is hit by a
    // fresh publish, but not so tight it spams the topic.
    const ADMISSION_POLL: std::time::Duration = std::time::Duration::from_secs(2);

    let deadline = tokio::time::Instant::now() + MAX_ADMISSION_WAIT;

    // `loop {}` (not `while now < deadline`) so the deadline is only checked
    // *after* an admission check, never right after a sleep — otherwise an
    // admission that completes during the final sleep would be lost to a false
    // "timed out" / `admitted:false`. The deadline break lives in the `Err`
    // arm below, immediately after the (failed) admission check.
    loop {
        // Bound each admission check so a stuck context-manager actor can't
        // extend the handler past MAX_ADMISSION_WAIT: a check that exceeds the
        // poll interval is mapped to a (retriable) error and handled by the
        // `Err` arm below, exactly like a not-yet-admitted result. A
        // slow-but-not-stuck actor whose check nears ADMISSION_POLL makes the
        // effective cycle up to ~2x ADMISSION_POLL; that's acceptable, and we
        // keep the budget at ADMISSION_POLL (rather than shrinking it) so a
        // normally-fast actor isn't spuriously timed out. The overall deadline
        // still bounds total wall-clock either way.
        let admission = tokio::time::timeout(
            ADMISSION_POLL,
            state
                .ctx_client
                .list_group_contexts(ListGroupContextsRequest {
                    group_id,
                    offset: 0,
                    limit: 100,
                }),
        )
        .await
        .unwrap_or_else(|_| {
            Err(eyre::eyre!(
                "list_group_contexts exceeded the admission poll budget"
            ))
        });
        match admission {
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
                    Ok(report) => {
                        report.observe("fleet_join", "MemberSetAutoFollow");
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

                // Stop once past the deadline — but only here, AFTER the
                // admission check above, so an admission that landed during the
                // previous sleep is observed on this iteration instead of being
                // lost to a false "timed out".
                if tokio::time::Instant::now() >= deadline {
                    break;
                }

                // Cap the poll sleep to the remaining budget so the loop wakes
                // for its final admission check right at the deadline rather
                // than up to ADMISSION_POLL past it.
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::time::sleep(remaining.min(ADMISSION_POLL)).await;

                // Re-announce AFTER the poll sleep, and only if we're still
                // before the deadline. Doing it here (rather than before the
                // sleep) avoids both a duplicate publish fired back-to-back with
                // the up-front one at t=0 and a wasted publish right as we give
                // up. A single up-front publish is lost if the mesh was empty at
                // fleet-join (gossipsub does not replay), so re-publishing each
                // cycle delivers a fresh copy to a mesh window that opens later.
                // Best effort — a transport error here is logged, not fatal.
                if tokio::time::Instant::now() < deadline {
                    match state
                        .node_client
                        .publish_on_namespace_now(group_id_bytes, payload.clone())
                        .await
                    {
                        Ok(mesh_peers) => tracing::debug!(
                            group_id = %req.group_id,
                            mesh_peers,
                            "re-announced TeeAttestationAnnounce while awaiting admission"
                        ),
                        Err(reannounce_err) => warn!(
                            group_id = %req.group_id,
                            error = ?reannounce_err,
                            "re-announce publish failed; will retry next cycle"
                        ),
                    }

                    // Bootstrap pull: a bare announcer holds NO namespace
                    // governance state (it only `subscribe_namespace`'d to send
                    // the announce). Once the verifier admits it, the verifier
                    // publishes the membership op (encrypted with the namespace
                    // group key) plus a `KeyDelivery` wrapping that key for this
                    // node — but both ride the namespace governance DAG, which
                    // this node has not pulled yet. The beacon-driven anti-entropy
                    // path deliberately skips a node with no local DAG head (it
                    // would race the bootstrap and pull undecryptable skeletons),
                    // so nothing pulls the DAG for us automatically. Trigger the
                    // pull ourselves each cycle: it fetches the full namespace
                    // governance DAG from a mesh peer, applies the `KeyDelivery`
                    // (decryptable with our namespace identity SK alone), then
                    // retries the previously-undecryptable membership op now that
                    // the group key is present. After that the `list_group_contexts`
                    // self-confirm above resolves and we join + replicate contexts.
                    // Best-effort: a missing mesh peer is logged inside
                    // `sync_namespace` and retried next cycle. Guarded by the same
                    // `now < deadline` check as the re-announce because it is a
                    // network op that should not run past the deadline.
                    if let Err(sync_err) = state.node_client.sync_namespace(group_id_bytes).await {
                        tracing::debug!(
                            group_id = %req.group_id,
                            error = ?sync_err,
                            "namespace governance bootstrap pull failed; will retry next cycle"
                        );
                    }
                }
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
